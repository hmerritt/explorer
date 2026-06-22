use std::{
    collections::{HashMap, HashSet},
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use filetime::FileTime;
use rayon::prelude::*;
use thousands::Separable;

use crate::explorer::archive_diagnostics::{ArchiveDiagnostics, ArchiveHandle, CountingReader};
use crate::explorer::{
    entry::FileEntry,
    resumable_copy::{
        CopyDurability, CopyOptions, cleanup_resumable_copy_progress,
        copy_file_contents_parallel_with_progress, copy_with_delta_progress_with_options,
        destination_content_matches_source_with_progress, destination_quick_matches_source,
    },
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::path::Prefix;

const COPY_BUFFER_SIZE: usize = 1024 * 1024;
const COMPOUND_ARCHIVE_EXTENSIONS: &[&str] = &["tar.gz", "tar.bz2", "tar.xz", "tar.zst"];
const SIMPLE_ARCHIVE_EXTENSIONS: &[&str] = &[
    "zip", "tar", "tgz", "tbz", "txz", "tzst", "ar", "gz", "bz", "bz2", "xz", "zst", "rar", "7z",
];
const MACOSX_ARCHIVE_METADATA_DIRECTORY: &str = "__MACOSX";
const ARCHIVE_PROGRESS_PUBLISH_INTERVAL: Duration = Duration::from_millis(100);
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(1);
static COPY_PARALLELISM_OVERRIDE: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DriveDiscKind {
    BluRay,
    Dvd,
}

pub fn default_start_path() -> PathBuf {
    let home_dir = user_home_dir();
    let downloads_dir = user_downloads_dir(home_dir.as_deref());

    preferred_start_path(downloads_dir, home_dir, std::env::current_dir().ok())
}

fn preferred_start_path(
    downloads_dir: Option<PathBuf>,
    home_dir: Option<PathBuf>,
    current_dir: Option<PathBuf>,
) -> PathBuf {
    downloads_dir
        .filter(|path| path.is_dir())
        .or(home_dir)
        .or(current_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

#[cfg(target_os = "windows")]
fn known_folder_path(folder_id: &windows::core::GUID) -> Option<PathBuf> {
    use windows::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{KNOWN_FOLDER_FLAG, SHGetKnownFolderPath},
    };

    unsafe {
        let known_folder = SHGetKnownFolderPath(folder_id, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let path = known_folder.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(known_folder.as_ptr().cast()));
        path
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn user_desktop_dir(_home_dir: Option<&Path>) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::FOLDERID_Desktop;

    known_folder_path(&FOLDERID_Desktop)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn user_desktop_dir(home_dir: Option<&Path>) -> Option<PathBuf> {
    home_dir.map(|home_dir| home_dir.join("Desktop"))
}

#[cfg(target_os = "windows")]
pub(crate) fn user_documents_dir(_home_dir: Option<&Path>) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::FOLDERID_Documents;

    known_folder_path(&FOLDERID_Documents)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn user_documents_dir(home_dir: Option<&Path>) -> Option<PathBuf> {
    home_dir.map(|home_dir| home_dir.join("Documents"))
}

#[cfg(target_os = "windows")]
pub(crate) fn user_downloads_dir(_home_dir: Option<&Path>) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::FOLDERID_Downloads;

    known_folder_path(&FOLDERID_Downloads)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn user_downloads_dir(home_dir: Option<&Path>) -> Option<PathBuf> {
    home_dir.map(|home_dir| home_dir.join("Downloads"))
}

#[cfg(target_os = "windows")]
pub(crate) fn user_pictures_dir(_home_dir: Option<&Path>) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::FOLDERID_Pictures;

    known_folder_path(&FOLDERID_Pictures)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn user_pictures_dir(home_dir: Option<&Path>) -> Option<PathBuf> {
    home_dir.map(|home_dir| home_dir.join("Pictures"))
}

#[cfg(target_os = "windows")]
pub(crate) fn user_videos_dir(_home_dir: Option<&Path>) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::FOLDERID_Videos;

    known_folder_path(&FOLDERID_Videos)
}

#[cfg(target_os = "macos")]
pub(crate) fn user_videos_dir(home_dir: Option<&Path>) -> Option<PathBuf> {
    home_dir.map(|home_dir| home_dir.join("Movies"))
}
#[cfg(target_os = "linux")]
pub(crate) fn user_videos_dir(home_dir: Option<&Path>) -> Option<PathBuf> {
    home_dir.map(|home_dir| home_dir.join("Videos"))
}

#[cfg(target_os = "windows")]
pub(crate) fn user_music_dir(_home_dir: Option<&Path>) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::FOLDERID_Music;

    known_folder_path(&FOLDERID_Music)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn user_music_dir(home_dir: Option<&Path>) -> Option<PathBuf> {
    home_dir.map(|home_dir| home_dir.join("Music"))
}

pub(crate) fn macos_applications_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(PathBuf::from("/Applications"))
    } else {
        None
    }
}

pub(crate) fn macos_bin_dir(home_dir: Option<&Path>) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        home_dir.map(|home| home.join(".Trash"))
    } else {
        None
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_local_os_drive_root() -> Option<PathBuf> {
    static CACHE: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            use std::path::Path;
            use windows::Win32::System::SystemInformation::GetSystemDirectoryW;

            let mut buffer = vec![0u16; 260]; // MAX_PATH length
            unsafe {
                let length = GetSystemDirectoryW(Some(&mut buffer));
                if length > 0 {
                    buffer.truncate(length as usize);
                    let system_dir = String::from_utf16(&buffer).ok()?;
                    if let Some(root) = Path::new(&system_dir).components().next() {
                        println!("windows_local_os_drive_root: {:?}", root.as_os_str());
                        return Some(PathBuf::from(format!(
                            "{}\\",
                            root.as_os_str().to_string_lossy()
                        )));
                    }
                }
            }
            None
        })
        .clone()
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn windows_local_os_drive_root() -> Option<PathBuf> {
    None
}

#[cfg(target_os = "windows")]
pub(crate) fn local_drive_roots() -> Vec<PathBuf> {
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    use windows::core::PCWSTR;

    let mask = unsafe { GetLogicalDrives() };
    let mut roots = Vec::new();

    for drive_index in 0..26 {
        if mask & (1 << drive_index) == 0 {
            continue;
        }

        let letter = char::from(b'A' + drive_index as u8);
        let root = format!("{letter}:\\");
        let mut encoded = root.encode_utf16().collect::<Vec<_>>();
        encoded.push(0);

        let drive_type = unsafe { GetDriveTypeW(PCWSTR(encoded.as_ptr())) };
        if windows_drive_type_is_explorer_local(drive_type) {
            roots.push(PathBuf::from(root));
        }
    }

    roots
}

#[cfg(target_os = "windows")]
pub(crate) fn wsl_drive_roots() -> Vec<PathBuf> {
    wsl_drive_roots_from_distribution_names(windows_wsl_distribution_names())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn wsl_drive_roots() -> Vec<PathBuf> {
    Vec::new()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WslDistroKind {
    Alpine,
    Debian,
    Generic,
    Kali,
    OpenSuse,
    Ubuntu,
}

pub(crate) fn wsl_distro_kind_from_name(name: &str) -> WslDistroKind {
    let normalized = name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();

    if normalized.contains("alpine") {
        WslDistroKind::Alpine
    } else if normalized.contains("debian") {
        WslDistroKind::Debian
    } else if normalized.contains("kali") {
        WslDistroKind::Kali
    } else if normalized.contains("opensuse") {
        WslDistroKind::OpenSuse
    } else if normalized.contains("ubuntu") {
        WslDistroKind::Ubuntu
    } else {
        WslDistroKind::Generic
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn wsl_distro_kind_for_path(path: &Path) -> Option<WslDistroKind> {
    if !path_has_no_components_below_prefix_root(path) {
        return None;
    }

    windows_wsl_unc_distribution_name(path)
        .as_deref()
        .map(wsl_distro_kind_from_name)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn wsl_distro_kind_for_path(_: &Path) -> Option<WslDistroKind> {
    None
}

pub(crate) fn drive_display_label(path: &Path) -> String {
    let display = path.display().to_string();

    #[cfg(target_os = "windows")]
    {
        return windows_drive_display_label(&display, windows_volume_label(path).as_deref());
    }

    #[cfg(not(target_os = "windows"))]
    {
        display
    }
}

pub(super) fn path_is_filesystem_root(path: &Path) -> bool {
    path.has_root() && path.parent().is_none()
}

pub(super) fn path_is_same_or_descendant(path: &Path, root: &Path) -> bool {
    let mut path_components = path.components();
    for root_component in root.components() {
        let Some(path_component) = path_components.next() else {
            return false;
        };
        if !path_components_match(path_component, root_component) {
            return false;
        }
    }
    true
}

#[cfg(target_os = "windows")]
fn path_components_match(left: Component<'_>, right: Component<'_>) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

#[cfg(not(target_os = "windows"))]
fn path_components_match(left: Component<'_>, right: Component<'_>) -> bool {
    left == right
}

#[cfg(target_os = "windows")]
pub(super) fn path_is_wsl_unc(path: &Path) -> bool {
    windows_wsl_unc_distribution_name(path).is_some()
}

#[cfg(not(target_os = "windows"))]
pub(super) fn path_is_wsl_unc(_: &Path) -> bool {
    false
}

#[cfg(target_os = "windows")]
pub(super) fn path_is_wsl_unc_root(path: &Path) -> bool {
    path_is_wsl_unc(path) && path_has_no_components_below_prefix_root(path)
}

#[cfg(not(target_os = "windows"))]
pub(super) fn path_is_wsl_unc_root(_: &Path) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn windows_wsl_unc_distribution_name(path: &Path) -> Option<String> {
    use std::path::Prefix;

    let Component::Prefix(prefix) = path.components().next()? else {
        return None;
    };

    let (server, share) = match prefix.kind() {
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => (server, share),
        _ => return None,
    };

    let server = server.to_string_lossy();
    if !(server.eq_ignore_ascii_case("wsl.localhost") || server.eq_ignore_ascii_case("wsl$")) {
        return None;
    }

    let share = share.to_string_lossy();
    (!share.is_empty()).then(|| share.into_owned())
}

#[cfg(target_os = "windows")]
fn path_has_no_components_below_prefix_root(path: &Path) -> bool {
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Prefix(_))) {
        return false;
    }
    if matches!(components.clone().next(), Some(Component::RootDir)) {
        components.next();
    }
    components.next().is_none()
}

#[cfg(target_os = "windows")]
fn windows_volume_label(path: &Path) -> Option<String> {
    use windows::Win32::Storage::FileSystem::GetVolumeInformationW;
    use windows::core::PCWSTR;

    let root = path.display().to_string();
    let mut encoded = root.encode_utf16().collect::<Vec<_>>();
    encoded.push(0);

    let mut volume_name = [0u16; 261];
    unsafe {
        GetVolumeInformationW(
            PCWSTR(encoded.as_ptr()),
            Some(&mut volume_name),
            None,
            None,
            None,
            None,
        )
        .ok()?;
    }

    let length = volume_name
        .iter()
        .position(|code_unit| *code_unit == 0)
        .unwrap_or(volume_name.len());

    String::from_utf16(&volume_name[..length]).ok()
}

#[cfg(target_os = "windows")]
fn windows_drive_display_label(path_display: &str, volume_label: Option<&str>) -> String {
    let drive = path_display.trim_end_matches(['\\', '/']);
    let label = volume_label
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .unwrap_or("Local Disk");

    format!("{label} ({drive})")
}

#[cfg(target_os = "windows")]
fn windows_drive_type_is_explorer_local(drive_type: u32) -> bool {
    matches!(drive_type, 2 | 3 | 5 | 6)
}

#[cfg(any(target_os = "windows", test))]
fn wsl_drive_roots_from_distribution_names<I, S>(names: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut names = names
        .into_iter()
        .filter_map(|name| {
            let name = name.as_ref().trim();
            (!name.is_empty()).then(|| name.to_owned())
        })
        .collect::<Vec<_>>();

    names.sort_by(|left, right| {
        left.to_ascii_lowercase()
            .cmp(&right.to_ascii_lowercase())
            .then_with(|| left.cmp(right))
    });

    names.into_iter().map(wsl_drive_root).collect()
}

#[cfg(any(target_os = "windows", test))]
fn wsl_drive_root(name: String) -> PathBuf {
    let mut path = String::from(r"\\wsl.localhost\");
    path.push_str(&name);
    path.push('\\');
    PathBuf::from(path)
}

#[cfg(target_os = "windows")]
fn windows_wsl_distribution_names() -> Vec<String> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, RegCloseKey, RegOpenKeyExW,
    };
    use windows::core::PCWSTR;

    const WSL_REGISTRY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Lxss";
    const DISTRIBUTION_NAME_VALUE: &str = "DistributionName";

    let key_path = wide_null(WSL_REGISTRY_PATH);
    let mut root = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_path.as_ptr()),
            None,
            KEY_READ,
            &mut root,
        )
    };
    if status != ERROR_SUCCESS {
        return Vec::new();
    }

    let mut names = Vec::new();
    for subkey in windows_registry_subkey_names(root) {
        if let Some(name) = windows_registry_string_value(root, &subkey, DISTRIBUTION_NAME_VALUE) {
            names.push(name);
        }
    }

    let _ = unsafe { RegCloseKey(root) };
    names
}

#[cfg(target_os = "windows")]
fn windows_registry_subkey_names(root: windows::Win32::System::Registry::HKEY) -> Vec<String> {
    use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
    use windows::Win32::System::Registry::RegEnumKeyExW;
    use windows::core::PWSTR;

    let mut names = Vec::new();
    let mut index = 0;

    loop {
        let mut capacity = 256usize;
        let mut found = None;

        loop {
            let mut buffer = vec![0u16; capacity];
            let mut len = buffer.len() as u32;
            let status = unsafe {
                RegEnumKeyExW(
                    root,
                    index,
                    Some(PWSTR(buffer.as_mut_ptr())),
                    &mut len,
                    None,
                    None,
                    None,
                    None,
                )
            };

            if status == ERROR_NO_MORE_ITEMS {
                return names;
            }
            if status == ERROR_MORE_DATA {
                capacity *= 2;
                continue;
            }
            if status == ERROR_SUCCESS {
                buffer.truncate(len as usize);
                found = String::from_utf16(&buffer).ok();
            }
            break;
        }

        if let Some(name) = found.filter(|name| !name.is_empty()) {
            names.push(name);
        }
        index += 1;
    }
}

#[cfg(target_os = "windows")]
fn windows_registry_string_value(
    root: windows::Win32::System::Registry::HKEY,
    subkey: &str,
    value_name: &str,
) -> Option<String> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{REG_VALUE_TYPE, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::PCWSTR;

    let subkey = wide_null(subkey);
    let value_name = wide_null(value_name);
    let mut value_type = REG_VALUE_TYPE(0);
    let mut byte_len = 0u32;

    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            Some(&mut value_type),
            None,
            Some(&mut byte_len),
        )
    };
    if status != ERROR_SUCCESS || byte_len < 2 {
        return None;
    }

    let mut buffer = vec![0u16; byte_len.div_ceil(2) as usize];
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            Some(&mut value_type),
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut byte_len),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }

    let mut char_len = (byte_len / 2) as usize;
    if char_len > 0 && buffer.get(char_len - 1) == Some(&0) {
        char_len -= 1;
    }
    String::from_utf16(&buffer[..char_len]).ok()
}

#[cfg(target_os = "windows")]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "macos")]
pub(crate) fn local_drive_roots() -> Vec<PathBuf> {
    macos_volume_drive_roots_from_dir(Path::new("/Volumes"))
}

#[cfg(target_os = "linux")]
pub(crate) fn local_drive_roots() -> Vec<PathBuf> {
    linux_mountinfo_drive_roots_from_path(Path::new("/proc/self/mountinfo"))
        .unwrap_or_else(|_| vec![PathBuf::from("/")])
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub(crate) fn local_drive_roots() -> Vec<PathBuf> {
    vec![PathBuf::from("/")]
}

#[cfg(any(target_os = "macos", test))]
fn macos_volume_drive_roots_from_dir(volumes_dir: &Path) -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/")];
    let Ok(entries) = fs::read_dir(volumes_dir) else {
        return roots;
    };

    let mut volumes = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    volumes.sort();
    volumes.dedup();
    roots.extend(volumes);
    roots
}

#[cfg(any(target_os = "linux", test))]
fn linux_mountinfo_drive_roots_from_path(path: &Path) -> io::Result<Vec<PathBuf>> {
    fs::read_to_string(path).map(|mountinfo| linux_mountinfo_drive_roots(&mountinfo))
}

#[cfg(any(target_os = "linux", test))]
fn linux_mountinfo_drive_roots(mountinfo: &str) -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/")];
    let mut seen = HashSet::from([PathBuf::from("/")]);
    let mut mounted_volumes = mountinfo
        .lines()
        .filter_map(linux_mountinfo_entry)
        .map(|entry| entry.mount_point)
        .filter(|path| linux_mount_point_is_visible_drive(path))
        .filter(|path| seen.insert(path.clone()))
        .collect::<Vec<_>>();

    mounted_volumes.sort();
    roots.extend(mounted_volumes);
    roots
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Debug, Eq, PartialEq)]
struct LinuxMountInfoEntry {
    mount_point: PathBuf,
    fs_type: String,
    source: String,
}

#[cfg(any(target_os = "linux", test))]
fn linux_mountinfo_entry(line: &str) -> Option<LinuxMountInfoEntry> {
    let (mount_fields, filesystem_fields) = line.split_once(" - ")?;
    let mount_point = mount_fields
        .split_whitespace()
        .nth(4)
        .map(linux_mountinfo_unescape)
        .map(PathBuf::from)?;
    let mut filesystem_fields = filesystem_fields.split_whitespace();
    let fs_type = filesystem_fields.next()?.to_owned();
    let source = linux_mountinfo_unescape(filesystem_fields.next()?);

    Some(LinuxMountInfoEntry {
        mount_point,
        fs_type,
        source,
    })
}

#[cfg(any(target_os = "linux", test))]
fn linux_mountinfo_unescape(value: &str) -> String {
    let mut bytes = Vec::with_capacity(value.len());
    let raw = value.as_bytes();
    let mut index = 0;

    while index < raw.len() {
        if raw[index] == b'\\' && index + 3 < raw.len() {
            let digits = &raw[index + 1..index + 4];
            if digits.iter().all(u8::is_ascii_digit)
                && let Ok(octal) = std::str::from_utf8(digits)
                && let Ok(byte) = u8::from_str_radix(octal, 8)
            {
                bytes.push(byte);
                index += 4;
                continue;
            }
        }
        bytes.push(raw[index]);
        index += 1;
    }

    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(any(target_os = "linux", test))]
fn linux_mount_point_is_visible_drive(path: &Path) -> bool {
    path_has_components_below(path, Path::new("/media"), 2)
        || path_has_components_below(path, Path::new("/run/media"), 2)
        || path_has_components_below(path, Path::new("/mnt"), 1)
}

#[cfg(any(target_os = "linux", test))]
fn path_has_components_below(path: &Path, root: &Path, min_components: usize) -> bool {
    path.strip_prefix(root)
        .ok()
        .map(|relative| relative.components().count() >= min_components)
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
pub(crate) fn drive_root_is_ejectable(path: &Path) -> bool {
    windows_drive_type(path).is_some_and(|drive_type| matches!(drive_type, 2 | 5))
}

#[cfg(target_os = "macos")]
pub(crate) fn drive_root_is_ejectable(path: &Path) -> bool {
    path != Path::new("/") && path.starts_with("/Volumes")
}

#[cfg(target_os = "linux")]
pub(crate) fn drive_root_is_ejectable(path: &Path) -> bool {
    linux_mount_point_is_visible_drive(path)
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub(crate) fn drive_root_is_ejectable(_: &Path) -> bool {
    false
}

pub(crate) fn drive_root_disc_kind(path: &Path) -> Option<DriveDiscKind> {
    let marker_kind = drive_root_marker_disc_kind(path)?;
    platform_drive_root_is_physical_optical(path).then_some(marker_kind)
}

fn drive_root_marker_disc_kind(path: &Path) -> Option<DriveDiscKind> {
    let entries = fs::read_dir(path).ok()?;
    let mut has_dvd_marker = false;

    for entry in entries.filter_map(Result::ok) {
        if !entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
            continue;
        }

        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.eq_ignore_ascii_case("BDMV") {
            return Some(DriveDiscKind::BluRay);
        }
        if name.eq_ignore_ascii_case("VIDEO_TS") || name.eq_ignore_ascii_case("AUDIO_TS") {
            has_dvd_marker = true;
        }
    }

    has_dvd_marker.then_some(DriveDiscKind::Dvd)
}

#[cfg(target_os = "windows")]
fn platform_drive_root_is_physical_optical(path: &Path) -> bool {
    if windows_drive_type(path) != Some(5) {
        return false;
    }

    windows_drive_device_descriptor(path).is_some_and(|(device_type, removable_media, bus_type)| {
        windows_storage_descriptor_is_physical_optical(device_type, removable_media, bus_type)
    })
}

#[cfg(target_os = "macos")]
fn platform_drive_root_is_physical_optical(path: &Path) -> bool {
    if path == Path::new("/") {
        return false;
    }

    macos_diskutil_disc_evidence(path)
        .and_then(|evidence| macos_diskutil_disc_evidence_is_physical_optical(&evidence))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn platform_drive_root_is_physical_optical(path: &Path) -> bool {
    fs::read_to_string("/proc/self/mountinfo")
        .ok()
        .is_some_and(|mountinfo| linux_mountinfo_path_is_physical_optical_drive(&mountinfo, path))
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn platform_drive_root_is_physical_optical(_: &Path) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn windows_drive_type(path: &Path) -> Option<u32> {
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;
    use windows::core::PCWSTR;

    let root = windows_drive_root(path)?;
    let encoded = wide_null(&root);
    Some(unsafe { GetDriveTypeW(PCWSTR(encoded.as_ptr())) })
}

#[cfg(target_os = "windows")]
fn windows_drive_root(path: &Path) -> Option<String> {
    path.components()
        .next()
        .and_then(|component| match component {
            Component::Prefix(prefix) => {
                Some(format!("{}\\", prefix.as_os_str().to_string_lossy()))
            }
            _ => None,
        })
}

#[cfg(target_os = "windows")]
struct WindowsHandle(windows::Win32::Foundation::HANDLE);

#[cfg(target_os = "windows")]
impl Drop for WindowsHandle {
    fn drop(&mut self) {
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.0) };
    }
}

#[cfg(target_os = "windows")]
fn windows_drive_device_descriptor(path: &Path) -> Option<(u8, bool, i32)> {
    use std::{ffi::c_void, mem::size_of};
    use windows::Win32::{
        Storage::FileSystem::{
            CreateFileW, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        System::{
            IO::DeviceIoControl,
            Ioctl::{
                IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_DEVICE_DESCRIPTOR,
                STORAGE_PROPERTY_QUERY, StorageDeviceProperty,
            },
        },
    };
    use windows::core::PCWSTR;

    let root = windows_drive_root(path)?;
    let drive = root.trim_end_matches(['\\', '/']);
    let encoded = wide_null(&format!(r"\\.\{drive}"));
    let handle = WindowsHandle(unsafe {
        CreateFileW(
            PCWSTR(encoded.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            Default::default(),
            None,
        )
        .ok()?
    });

    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut descriptor = vec![0u8; 1024];
    let mut bytes_returned = 0;

    unsafe {
        DeviceIoControl(
            handle.0,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&query as *const STORAGE_PROPERTY_QUERY as *const c_void),
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            Some(descriptor.as_mut_ptr().cast::<c_void>()),
            descriptor.len() as u32,
            Some(&mut bytes_returned),
            None,
        )
        .ok()?;
    }

    if bytes_returned < size_of::<STORAGE_DEVICE_DESCRIPTOR>() as u32 {
        return None;
    }
    let descriptor = unsafe {
        descriptor
            .as_ptr()
            .cast::<STORAGE_DEVICE_DESCRIPTOR>()
            .read_unaligned()
    };

    Some((
        descriptor.DeviceType,
        descriptor.RemovableMedia,
        descriptor.BusType.0,
    ))
}

#[cfg(any(target_os = "windows", test))]
fn windows_storage_descriptor_is_physical_optical(
    device_type: u8,
    removable_media: bool,
    bus_type: i32,
) -> bool {
    const SCSI_DEVICE_TYPE_CD_DVD: u8 = 5;
    const BUS_TYPE_SCSI: i32 = 1;
    const BUS_TYPE_ATAPI: i32 = 2;
    const BUS_TYPE_ATA: i32 = 3;
    const BUS_TYPE_1394: i32 = 4;
    const BUS_TYPE_USB: i32 = 7;
    const BUS_TYPE_SATA: i32 = 11;

    device_type == SCSI_DEVICE_TYPE_CD_DVD
        && removable_media
        && matches!(
            bus_type,
            BUS_TYPE_SCSI
                | BUS_TYPE_ATAPI
                | BUS_TYPE_ATA
                | BUS_TYPE_1394
                | BUS_TYPE_USB
                | BUS_TYPE_SATA
        )
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Default, Eq, PartialEq)]
struct MacosDiskutilDiscEvidence {
    optical: Option<bool>,
    virtual_or_physical: Option<String>,
}

#[cfg(target_os = "macos")]
fn macos_diskutil_disc_evidence(path: &Path) -> Option<MacosDiskutilDiscEvidence> {
    use std::process::Command;

    let output = Command::new("/usr/sbin/diskutil")
        .args(["info", "-plist"])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let value = plist::Value::from_reader(output.stdout.as_slice()).ok()?;
    let dictionary = value.as_dictionary()?;
    Some(MacosDiskutilDiscEvidence {
        optical: dictionary.get("Optical").and_then(plist::Value::as_boolean),
        virtual_or_physical: dictionary
            .get("VirtualOrPhysical")
            .and_then(plist::Value::as_string)
            .map(str::to_owned),
    })
}

#[cfg(any(target_os = "macos", test))]
fn macos_diskutil_disc_evidence_is_physical_optical(
    evidence: &MacosDiskutilDiscEvidence,
) -> Option<bool> {
    Some(
        evidence.optical?
            && evidence
                .virtual_or_physical
                .as_deref()?
                .eq_ignore_ascii_case("Physical"),
    )
}

#[cfg(any(target_os = "linux", test))]
fn linux_mountinfo_path_is_physical_optical_drive(mountinfo: &str, path: &Path) -> bool {
    let entry = mountinfo
        .lines()
        .filter_map(linux_mountinfo_entry)
        .find(|entry| entry.mount_point == path);

    entry.is_some_and(|entry| {
        linux_mount_point_is_visible_drive(&entry.mount_point)
            && linux_disc_filesystem_is_optical(&entry.fs_type)
            && linux_disc_source_is_physical_optical(&entry.source)
    })
}

#[cfg(any(target_os = "linux", test))]
fn linux_disc_filesystem_is_optical(fs_type: &str) -> bool {
    fs_type.eq_ignore_ascii_case("udf")
        || fs_type.eq_ignore_ascii_case("iso9660")
        || fs_type.eq_ignore_ascii_case("cdfs")
}

#[cfg(any(target_os = "linux", test))]
fn linux_disc_source_is_physical_optical(source: &str) -> bool {
    source == "/dev/cdrom"
        || source == "/dev/dvd"
        || source
            .strip_prefix("/dev/sr")
            .is_some_and(|suffix| suffix.chars().all(|character| character.is_ascii_digit()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct EntryVisibility {
    pub(super) show_dotfiles: bool,
    pub(super) show_hidden_attributes: bool,
}

impl EntryVisibility {
    pub(super) fn new(show_dotfiles: bool, show_hidden_attributes: bool) -> Self {
        Self {
            show_dotfiles,
            show_hidden_attributes,
        }
    }
}

impl From<bool> for EntryVisibility {
    fn from(show_hidden: bool) -> Self {
        Self::new(show_hidden, show_hidden)
    }
}

pub(super) fn load_entries(
    path: &Path,
    visibility: impl Into<EntryVisibility>,
) -> std::io::Result<Vec<FileEntry>> {
    load_entries_with_options(path, EntryLoadOptions::for_path(path, visibility.into()))
}

#[derive(Clone, Copy)]
struct EntryLoadOptions {
    visibility: EntryVisibility,
    applications_view: bool,
}

#[derive(Default)]
struct EntryLoadTimingStats {
    directory_entries: usize,
    entry_errors: usize,
    hidden_entries: usize,
    skipped_entries: usize,
    materialized_entries: usize,
    filter_elapsed: Duration,
    materialize_elapsed: Duration,
}

impl EntryLoadTimingStats {
    fn add(&mut self, other: Self) {
        self.directory_entries += other.directory_entries;
        self.entry_errors += other.entry_errors;
        self.hidden_entries += other.hidden_entries;
        self.skipped_entries += other.skipped_entries;
        self.materialized_entries += other.materialized_entries;
        self.filter_elapsed += other.filter_elapsed;
        self.materialize_elapsed += other.materialize_elapsed;
    }
}

impl EntryLoadOptions {
    fn for_path(path: &Path, visibility: EntryVisibility) -> Self {
        Self {
            visibility,
            applications_view: should_use_applications_view(path),
        }
    }
}

fn load_entries_with_options(
    path: &Path,
    options: EntryLoadOptions,
) -> std::io::Result<Vec<FileEntry>> {
    if options.applications_view {
        return load_applications_entries(path, options);
    }

    let total_started = Instant::now();
    let read_dir_started = Instant::now();
    let directory_entries = match fs::read_dir(path) {
        Ok(entries) => {
            crate::debug_options::log_nav_timing(
                read_dir_started.elapsed(),
                format_args!("load_entries.read_dir path={path:?} ok=true"),
            );
            entries
        }
        Err(error) => {
            crate::debug_options::log_nav_timing(
                read_dir_started.elapsed(),
                format_args!("load_entries.read_dir path={path:?} ok=false error={error}"),
            );
            return Err(error);
        }
    };

    let mut entries = Vec::new();
    let scan_started = Instant::now();
    let timings_enabled = crate::debug_options::nav_timings_enabled();
    let stats = collect_visible_entries(
        directory_entries,
        options,
        timings_enabled,
        &mut entries,
        |_| true,
    );
    crate::debug_options::log_nav_timing(
        stats.filter_elapsed,
        format_args!(
            "load_entries.filter path={path:?} scanned={} hidden={} entry_errors={}",
            stats.directory_entries.separate_with_commas(),
            stats.hidden_entries.separate_with_commas(),
            stats.entry_errors.separate_with_commas()
        ),
    );
    crate::debug_options::log_nav_timing(
        stats.materialize_elapsed,
        format_args!(
            "load_entries.materialize path={path:?} entries={} skipped={}",
            stats.materialized_entries.separate_with_commas(),
            stats.skipped_entries.separate_with_commas()
        ),
    );
    crate::debug_options::log_nav_timing(
        scan_started.elapsed(),
        format_args!(
            "load_entries.scan path={path:?} scanned={} entries={}",
            stats.directory_entries.separate_with_commas(),
            entries.len().separate_with_commas()
        ),
    );

    crate::debug_options::log_nav_timing(
        total_started.elapsed(),
        format_args!(
            "load_entries.total path={path:?} entries={} show_dotfiles={} show_hidden={}",
            entries.len().separate_with_commas(),
            options.visibility.show_dotfiles,
            options.visibility.show_hidden_attributes
        ),
    );
    Ok(entries)
}

fn load_applications_entries(
    path: &Path,
    options: EntryLoadOptions,
) -> std::io::Result<Vec<FileEntry>> {
    let total_started = Instant::now();
    let read_dir_started = Instant::now();
    let directory_entries = match fs::read_dir(path) {
        Ok(entries) => {
            crate::debug_options::log_nav_timing(
                read_dir_started.elapsed(),
                format_args!("load_entries.read_dir path={path:?} applications_view=true ok=true"),
            );
            entries
        }
        Err(error) => {
            crate::debug_options::log_nav_timing(
                read_dir_started.elapsed(),
                format_args!(
                    "load_entries.read_dir path={path:?} applications_view=true ok=false error={error}"
                ),
            );
            return Err(error);
        }
    };

    let mut entries = Vec::new();
    let mut stats = EntryLoadTimingStats::default();
    let scan_started = Instant::now();
    let timings_enabled = crate::debug_options::nav_timings_enabled();

    for directory_entry in directory_entries {
        stats.directory_entries += 1;
        let Ok(directory_entry) = directory_entry else {
            stats.entry_errors += 1;
            continue;
        };

        let filter_started = timings_enabled.then(Instant::now);
        let candidate = visible_directory_entry_candidate(&directory_entry, options);
        if let Some(started) = filter_started {
            stats.filter_elapsed += started.elapsed();
        }
        let DirectoryEntryCandidate::Visible {
            path,
            link_metadata,
        } = candidate
        else {
            match candidate {
                DirectoryEntryCandidate::Hidden => stats.hidden_entries += 1,
                DirectoryEntryCandidate::Skipped => stats.skipped_entries += 1,
                DirectoryEntryCandidate::Visible { .. } => unreachable!(),
            }
            continue;
        };

        let materialize_started = timings_enabled.then(Instant::now);
        let Some(entry) = materialize_visible_entry(path, link_metadata) else {
            if let Some(started) = materialize_started {
                stats.materialize_elapsed += started.elapsed();
            }
            stats.skipped_entries += 1;
            continue;
        };
        if let Some(started) = materialize_started {
            stats.materialize_elapsed += started.elapsed();
        }
        stats.materialized_entries += 1;

        if entry.is_app_bundle() {
            entries.push(entry);
        } else if entry.is_directory_like() {
            let nested_stats = collect_nested_applications(
                entry.navigation_path(),
                options,
                timings_enabled,
                &mut entries,
            );
            stats.add(nested_stats);
        }
    }

    crate::debug_options::log_nav_timing(
        stats.filter_elapsed,
        format_args!(
            "load_entries.filter path={path:?} applications_view=true scanned={} hidden={} entry_errors={}",
            stats.directory_entries.separate_with_commas(),
            stats.hidden_entries.separate_with_commas(),
            stats.entry_errors.separate_with_commas()
        ),
    );
    crate::debug_options::log_nav_timing(
        stats.materialize_elapsed,
        format_args!(
            "load_entries.materialize path={path:?} applications_view=true entries={} skipped={}",
            stats.materialized_entries.separate_with_commas(),
            stats.skipped_entries.separate_with_commas()
        ),
    );
    crate::debug_options::log_nav_timing(
        scan_started.elapsed(),
        format_args!(
            "load_entries.scan path={path:?} applications_view=true scanned={} entries={}",
            stats.directory_entries.separate_with_commas(),
            entries.len().separate_with_commas()
        ),
    );

    crate::debug_options::log_nav_timing(
        total_started.elapsed(),
        format_args!(
            "load_entries.total path={path:?} applications_view=true entries={} show_dotfiles={} show_hidden={}",
            entries.len().separate_with_commas(),
            options.visibility.show_dotfiles,
            options.visibility.show_hidden_attributes
        ),
    );
    Ok(entries)
}

fn collect_nested_applications(
    path: &Path,
    options: EntryLoadOptions,
    timings_enabled: bool,
    entries: &mut Vec<FileEntry>,
) -> EntryLoadTimingStats {
    let Ok(nested_entries) = fs::read_dir(path) else {
        return EntryLoadTimingStats::default();
    };

    collect_visible_entries(
        nested_entries,
        options,
        timings_enabled,
        entries,
        FileEntry::is_app_bundle,
    )
}

fn collect_visible_entries(
    directory_entries: fs::ReadDir,
    options: EntryLoadOptions,
    timings_enabled: bool,
    entries: &mut Vec<FileEntry>,
    keep_entry: impl Fn(&FileEntry) -> bool,
) -> EntryLoadTimingStats {
    let mut stats = EntryLoadTimingStats::default();

    for directory_entry in directory_entries {
        stats.directory_entries += 1;
        let Ok(directory_entry) = directory_entry else {
            stats.entry_errors += 1;
            continue;
        };

        let filter_started = timings_enabled.then(Instant::now);
        let candidate = visible_directory_entry_candidate(&directory_entry, options);
        if let Some(started) = filter_started {
            stats.filter_elapsed += started.elapsed();
        }
        let DirectoryEntryCandidate::Visible {
            path,
            link_metadata,
        } = candidate
        else {
            match candidate {
                DirectoryEntryCandidate::Hidden => stats.hidden_entries += 1,
                DirectoryEntryCandidate::Skipped => stats.skipped_entries += 1,
                DirectoryEntryCandidate::Visible { .. } => unreachable!(),
            }
            continue;
        };

        let materialize_started = timings_enabled.then(Instant::now);
        let Some(entry) = materialize_visible_entry(path, link_metadata) else {
            if let Some(started) = materialize_started {
                stats.materialize_elapsed += started.elapsed();
            }
            stats.skipped_entries += 1;
            continue;
        };
        if let Some(started) = materialize_started {
            stats.materialize_elapsed += started.elapsed();
        }
        stats.materialized_entries += 1;

        if keep_entry(&entry) {
            entries.push(entry);
        }
    }

    stats
}

enum DirectoryEntryCandidate {
    Hidden,
    Skipped,
    Visible {
        path: PathBuf,
        link_metadata: Option<fs::Metadata>,
    },
}

fn visible_directory_entry_candidate(
    entry: &fs::DirEntry,
    options: EntryLoadOptions,
) -> DirectoryEntryCandidate {
    let name = entry.file_name();
    let path = entry.path();
    if is_always_hidden_entry(&name, &path) {
        return DirectoryEntryCandidate::Hidden;
    }

    if !options.visibility.show_dotfiles && name.to_string_lossy().starts_with('.') {
        return DirectoryEntryCandidate::Hidden;
    }

    if options.visibility.show_hidden_attributes {
        return DirectoryEntryCandidate::Visible {
            path,
            link_metadata: None,
        };
    }

    let Ok(link_metadata) = fs::symlink_metadata(&path) else {
        return DirectoryEntryCandidate::Skipped;
    };

    if has_macos_hidden_flag_with_metadata(&path, &link_metadata)
        || has_windows_hidden_attribute_with_metadata(&path, &link_metadata)
    {
        return DirectoryEntryCandidate::Hidden;
    }

    DirectoryEntryCandidate::Visible {
        path,
        link_metadata: Some(link_metadata),
    }
}

fn materialize_visible_entry(
    path: PathBuf,
    link_metadata: Option<fs::Metadata>,
) -> Option<FileEntry> {
    match link_metadata {
        Some(link_metadata) => FileEntry::from_path_with_link_metadata(path, link_metadata),
        None => FileEntry::from_path(path),
    }
}

#[cfg(target_os = "macos")]
fn should_use_applications_view(path: &Path) -> bool {
    path == Path::new("/Applications")
}

#[cfg(not(target_os = "macos"))]
fn should_use_applications_view(_: &Path) -> bool {
    false
}

pub(super) fn should_hide_directory_entry(
    entry: &fs::DirEntry,
    visibility: impl Into<EntryVisibility>,
) -> bool {
    should_hide_entry(&entry.file_name(), &entry.path(), visibility)
}

pub(super) fn should_hide_entry(
    name: &OsStr,
    path: &Path,
    visibility: impl Into<EntryVisibility>,
) -> bool {
    let visibility = visibility.into();
    is_always_hidden_entry(name, path)
        || (!visibility.show_dotfiles && name.to_string_lossy().starts_with('.'))
        || (!visibility.show_hidden_attributes
            && (has_macos_hidden_flag(path) || has_windows_hidden_attribute(path)))
}

pub(super) fn should_hide_entry_with_metadata(
    name: &OsStr,
    path: &Path,
    visibility: impl Into<EntryVisibility>,
    metadata: &fs::Metadata,
) -> bool {
    let visibility = visibility.into();
    is_always_hidden_entry(name, path)
        || (!visibility.show_dotfiles && name.to_string_lossy().starts_with('.'))
        || (!visibility.show_hidden_attributes
            && (has_macos_hidden_flag_with_metadata(path, metadata)
                || has_windows_hidden_attribute_with_metadata(path, metadata)))
}

fn is_always_hidden_entry(name: &OsStr, path: &Path) -> bool {
    is_always_hidden_metadata_entry_name(name)
        || is_windows_drive_root_protected_directory(name, path)
}

fn is_always_hidden_metadata_entry_name(name: &OsStr) -> bool {
    name == OsStr::new(".localized")
        || name == OsStr::new(".DS_Store")
        || name == OsStr::new(MACOSX_ARCHIVE_METADATA_DIRECTORY)
}

#[cfg(target_os = "windows")]
fn is_windows_drive_root_protected_directory(name: &OsStr, path: &Path) -> bool {
    is_windows_drive_root_protected_directory_name(name)
        && path_is_direct_child_of_windows_drive_root(path)
}

#[cfg(not(target_os = "windows"))]
fn is_windows_drive_root_protected_directory(_: &OsStr, _: &Path) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn is_windows_drive_root_protected_directory_name(name: &OsStr) -> bool {
    const PROTECTED_NAMES: &[&str] = &[
        "$RECYCLEBIN",
        "$Recycle.Bin",
        "Config.Msi",
        "Recovery",
        "System Volume Information",
        "Documents and Settings",
    ];

    let name = name.to_string_lossy();
    PROTECTED_NAMES
        .iter()
        .any(|protected_name| name.eq_ignore_ascii_case(protected_name))
}

#[cfg(target_os = "windows")]
fn path_is_direct_child_of_windows_drive_root(path: &Path) -> bool {
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return false;
    };
    match prefix.kind() {
        Prefix::Disk(_) | Prefix::VerbatimDisk(_) => {}
        _ => return false,
    }
    if !matches!(components.next(), Some(Component::RootDir)) {
        return false;
    }

    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

#[cfg(target_os = "macos")]
fn has_macos_hidden_flag(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| has_macos_hidden_flag_with_metadata(path, &metadata))
}

#[cfg(not(target_os = "macos"))]
fn has_macos_hidden_flag(_: &Path) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn has_macos_hidden_flag_with_metadata(_: &Path, metadata: &fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;

    const UF_HIDDEN: u32 = 0x0000_8000;

    metadata.st_flags() & UF_HIDDEN != 0
}

#[cfg(not(target_os = "macos"))]
fn has_macos_hidden_flag_with_metadata(_: &Path, _: &fs::Metadata) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn has_windows_hidden_attribute(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| has_windows_hidden_attribute_with_metadata(path, &metadata))
}

#[cfg(not(target_os = "windows"))]
fn has_windows_hidden_attribute(_: &Path) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn has_windows_hidden_attribute_with_metadata(_: &Path, metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN;

    metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN.0 != 0
}

#[cfg(not(target_os = "windows"))]
fn has_windows_hidden_attribute_with_metadata(_: &Path, _: &fs::Metadata) -> bool {
    false
}

#[cfg(feature = "benchmarks")]
#[doc(hidden)]
pub mod benchmark_support {
    use std::path::{Path, PathBuf};

    use crate::explorer::FileEntry;

    pub struct PreparedArchiveExtraction(super::FileOperationJob);
    pub struct PreparedCopyOperation(super::FileOperationJob);

    pub fn load_entries(path: &Path, show_hidden_files: bool) -> Vec<FileEntry> {
        super::load_entries(path, show_hidden_files).expect("load benchmark entries")
    }

    pub fn extract_archives(archives: &[PathBuf], destination: &Path) {
        let prepared = super::prepare_extract_archives_to_directory(archives, destination)
            .expect("prepare archive benchmark extraction");
        let job = match prepared {
            super::PreparedFileOperation::Ready(job) => job,
            super::PreparedFileOperation::Conflicts(conflicts) => conflicts.into_job(),
        };
        super::execute_file_operation_with_progress(
            job,
            super::ConflictChoice::Replace,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            |_| {},
        )
        .expect("execute archive benchmark extraction");
    }

    pub fn list_archive(archive: &Path) -> usize {
        super::archive_listing(archive)
            .expect("list benchmark archive")
            .entries
            .len()
    }

    pub fn plan_archives(archives: &[PathBuf], destination: &Path) -> usize {
        super::prepare_extract_archive_operation(archives, destination)
            .expect("plan benchmark archive extraction")
            .stats
            .total_files
    }

    pub fn prepare_archive_extraction(
        archives: &[PathBuf],
        destination: &Path,
    ) -> PreparedArchiveExtraction {
        PreparedArchiveExtraction(
            super::prepare_extract_archive_operation(archives, destination)
                .expect("prepare benchmark archive extraction"),
        )
    }

    pub fn execute_prepared_archive_extraction(prepared: PreparedArchiveExtraction) {
        super::execute_file_operation_with_progress(
            prepared.0,
            super::ConflictChoice::Replace,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            |_| {},
        )
        .expect("execute prepared benchmark archive extraction");
    }

    pub fn extract_archives_with_progress(archives: &[PathBuf], destination: &Path) -> usize {
        let prepared = super::prepare_extract_archives_to_directory(archives, destination)
            .expect("prepare archive progress benchmark");
        let job = match prepared {
            super::PreparedFileOperation::Ready(job) => job,
            super::PreparedFileOperation::Conflicts(conflicts) => conflicts.into_job(),
        };
        let mut callbacks = 0;
        super::execute_file_operation_with_progress(
            job,
            super::ConflictChoice::Replace,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            |_| callbacks += 1,
        )
        .expect("execute archive progress benchmark");
        callbacks
    }

    pub fn copy_paths(paths: &[PathBuf], destination: &Path) {
        let prepared = super::prepare_copy_paths_to_directory(paths, destination)
            .expect("prepare copy benchmark");
        let job = match prepared {
            super::PreparedFileOperation::Ready(job) => job,
            super::PreparedFileOperation::Conflicts(conflicts) => conflicts.into_job(),
        };
        super::execute_file_operation_with_progress(
            job,
            super::ConflictChoice::Replace,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            |_| {},
        )
        .expect("execute copy benchmark");
    }

    pub fn prepare_copy(paths: &[PathBuf], destination: &Path) -> PreparedCopyOperation {
        let prepared = super::prepare_copy_paths_to_directory(paths, destination)
            .expect("prepare copy benchmark");
        PreparedCopyOperation(match prepared {
            super::PreparedFileOperation::Ready(job) => job,
            super::PreparedFileOperation::Conflicts(conflicts) => conflicts.into_job(),
        })
    }

    pub fn execute_prepared_copy(prepared: PreparedCopyOperation) {
        super::execute_file_operation_with_progress(
            prepared.0,
            super::ConflictChoice::Replace,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            |_| {},
        )
        .expect("execute prepared copy benchmark");
    }

    pub fn copy_with_cancel_after_progress(paths: &[PathBuf], destination: &Path) -> bool {
        let prepared = super::prepare_copy_paths_to_directory(paths, destination)
            .expect("prepare cancellable copy benchmark");
        let job = match prepared {
            super::PreparedFileOperation::Ready(job) => job,
            super::PreparedFileOperation::Conflicts(conflicts) => conflicts.into_job(),
        };
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut requested_cancel = false;
        let result = super::execute_file_operation_with_progress(
            job,
            super::ConflictChoice::Replace,
            cancel.clone(),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            |progress| {
                if progress.copied_bytes > 0 && !requested_cancel {
                    requested_cancel = true;
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            },
        );
        matches!(result, Err(super::FileOperationError::Cancelled))
    }

    pub fn set_copy_parallelism(parallelism: Option<usize>) {
        super::COPY_PARALLELISM_OVERRIDE.store(
            parallelism.unwrap_or(0),
            std::sync::atomic::Ordering::Relaxed,
        );
    }
}

pub(super) fn format_open_error(path: &Path, error: &std::io::Error) -> String {
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned());

    format!("Could not open {name}: {error}")
}

#[cfg(test)]
pub(super) fn move_paths_to_directory(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<FileOperationOutcome, String> {
    prepare_move_paths_to_directory(paths, destination).and_then(run_prepared_file_operation)
}

pub(super) fn prepare_move_paths_to_directory(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<PreparedFileOperation, String> {
    prepare_file_operation(
        paths,
        destination,
        FileOperationKind::Move,
        CopyNamePolicy::Original,
    )
    .map(prepared_or_conflicts)
}

#[cfg(test)]
pub(super) fn copy_paths_to_directory(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<FileOperationOutcome, String> {
    prepare_copy_paths_to_directory(paths, destination).and_then(run_prepared_file_operation)
}

#[cfg(any(test, feature = "benchmarks"))]
pub(super) fn prepare_copy_paths_to_directory(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<PreparedFileOperation, String> {
    prepare_file_operation(
        paths,
        destination,
        FileOperationKind::Copy,
        CopyNamePolicy::Original,
    )
    .map(prepared_or_conflicts)
}

#[cfg(test)]
pub(super) fn copy_paths_to_directory_with_copy_names(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<FileOperationOutcome, String> {
    prepare_copy_paths_to_directory_with_copy_names(paths, destination)
        .and_then(run_prepared_file_operation)
}

pub(super) fn prepare_copy_paths_to_directory_with_copy_names(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<PreparedFileOperation, String> {
    prepare_file_operation(
        paths,
        destination,
        FileOperationKind::Copy,
        CopyNamePolicy::UseCopyNamesInSameDirectory,
    )
    .map(prepared_or_conflicts)
}

#[cfg(test)]
pub(super) fn copy_paths_to_directory_for_paste(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<FileOperationOutcome, String> {
    prepare_copy_paths_to_directory_for_paste(paths, destination)
        .and_then(run_prepared_file_operation)
}

pub(super) fn prepare_copy_paths_to_directory_for_paste(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<PreparedFileOperation, String> {
    prepare_copy_paths_to_directory_with_copy_names(paths, destination)
}

pub(super) fn archive_path_is_supported(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(archive_name_has_supported_extension)
}

pub(super) fn mountable_image_path_is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(mountable_image_extension_is_supported)
}

pub(super) fn mountable_image_extension_is_supported(extension: &str) -> bool {
    extension.eq_ignore_ascii_case("iso")
        || extension.eq_ignore_ascii_case("img")
        || (cfg!(target_os = "macos") && extension.eq_ignore_ascii_case("dmg"))
        || (cfg!(target_os = "windows")
            && (extension.eq_ignore_ascii_case("vhd") || extension.eq_ignore_ascii_case("vhdx")))
}

pub(super) fn prepare_extract_archives_to_directory(
    archives: &[PathBuf],
    destination: &Path,
) -> Result<PreparedFileOperation, String> {
    prepare_extract_archive_operation(archives, destination).map(prepared_or_conflicts)
}

#[cfg(test)]
pub(super) fn resolve_file_conflicts(
    conflicts: FileConflictBatch,
    choice: ConflictChoice,
) -> Result<FileOperationSummary, String> {
    execute_file_operation(conflicts.job, choice)
}

#[cfg(test)]
fn run_prepared_file_operation(
    prepared: PreparedFileOperation,
) -> Result<FileOperationOutcome, String> {
    match prepared {
        PreparedFileOperation::Ready(job) => {
            execute_file_operation(job, ConflictChoice::Replace).map(FileOperationOutcome::Finished)
        }
        PreparedFileOperation::Conflicts(conflicts) => {
            Ok(FileOperationOutcome::Conflicts(conflicts))
        }
    }
}

fn prepared_or_conflicts(job: FileOperationJob) -> PreparedFileOperation {
    let conflicts = file_conflicts_for_job(&job);
    if conflicts.is_empty() {
        PreparedFileOperation::Ready(job)
    } else {
        PreparedFileOperation::Conflicts(FileConflictBatch { conflicts, job })
    }
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum FileOperationOutcome {
    Finished(FileOperationSummary),
    Conflicts(FileConflictBatch),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum PreparedFileOperation {
    Ready(FileOperationJob),
    Conflicts(FileConflictBatch),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileOperationSummary {
    pub(super) kind: FileOperationKind,
    pub(super) destination_paths: Vec<PathBuf>,
    pub(super) moved_source_paths: Vec<PathBuf>,
    pub(super) moved_paths: Vec<FileOperationMove>,
    pub(super) archive_diagnostics: Option<ArchiveDiagnostics>,
}

impl Default for FileOperationSummary {
    fn default() -> Self {
        Self {
            kind: FileOperationKind::Copy,
            destination_paths: Vec::new(),
            moved_source_paths: Vec::new(),
            moved_paths: Vec::new(),
            archive_diagnostics: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileOperationMove {
    pub(super) source: PathBuf,
    pub(super) destination: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileConflictBatch {
    pub(super) conflicts: Vec<FileConflict>,
    job: FileOperationJob,
}

impl FileConflictBatch {
    pub(super) fn len(&self) -> usize {
        self.conflicts.len()
    }

    pub(super) fn operation_label(&self) -> &'static str {
        self.job.kind.progress_title()
    }

    pub(super) fn into_job(self) -> FileOperationJob {
        self.job
    }

    pub(super) fn archive_diagnostics(&self) -> Option<ArchiveDiagnostics> {
        self.job.archive_diagnostics.clone()
    }

    pub(super) fn item_count_label(&self) -> String {
        let count = self.job.roots.len();
        let count_friendly = count.separate_with_commas();
        if count == 1 {
            "1 item".to_owned()
        } else {
            format!("{count_friendly} items")
        }
    }

    pub(super) fn source_location_name(&self) -> String {
        self.unique_root_parent_name(|root| root.source.parent())
    }

    pub(super) fn destination_location_name(&self) -> String {
        self.unique_root_parent_name(|root| root.destination.parent())
    }

    pub(super) fn first_destination_name(&self) -> String {
        self.conflicts
            .first()
            .map(|conflict| path_display_name(&conflict.destination))
            .unwrap_or_else(|| "this file".to_owned())
    }

    fn unique_root_parent_name<'a>(
        &'a self,
        parent_for_root: impl Fn(&'a FileOperationRoot) -> Option<&'a Path>,
    ) -> String {
        let parents = self
            .job
            .roots
            .iter()
            .filter_map(parent_for_root)
            .collect::<HashSet<_>>();

        if parents.len() == 1 {
            path_display_name(parents.iter().next().expect("one parent"))
        } else {
            "multiple locations".to_owned()
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileConflict {
    pub(super) source: PathBuf,
    pub(super) destination: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ConflictChoice {
    Replace,
    Skip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FileOperationKind {
    Move,
    Copy,
    Extract,
}

impl FileOperationKind {
    pub(super) fn progress_title(self) -> &'static str {
        match self {
            FileOperationKind::Move => "Moving",
            FileOperationKind::Copy => "Copying",
            FileOperationKind::Extract => "Extracting",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FileOperationPhase {
    Preparing,
    Indexing,
    Resuming,
    Copying,
    Verifying,
    Extracting,
    Moving,
    Removing,
    Finished,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileOperationProgress {
    pub(super) kind: FileOperationKind,
    pub(super) phase: FileOperationPhase,
    pub(super) total_bytes: u64,
    pub(super) copied_bytes: u64,
    pub(super) verified_bytes: u64,
    pub(super) work_total_bytes: u64,
    pub(super) work_completed_bytes: u64,
    pub(super) total_files: usize,
    pub(super) completed_files: usize,
    pub(super) current_item: Option<PathBuf>,
    pub(super) cancellable: bool,
}

impl FileOperationProgress {
    pub(super) fn percent(&self) -> Option<f32> {
        (self.work_total_bytes > 0).then(|| {
            (self.work_completed_bytes as f32 / self.work_total_bytes as f32).clamp(0.0, 1.0)
        })
    }

    pub(super) fn reserve_work_bytes(&mut self, bytes: u64) {
        self.work_total_bytes = self.work_total_bytes.saturating_add(bytes);
    }

    pub(super) fn add_copied_bytes(&mut self, bytes: u64) {
        self.copied_bytes = self.copied_bytes.saturating_add(bytes);
        self.add_completed_work_bytes(bytes);
    }

    pub(super) fn add_verified_bytes(&mut self, bytes: u64) {
        self.verified_bytes = self.verified_bytes.saturating_add(bytes);
        self.add_completed_work_bytes(bytes);
    }

    pub(super) fn add_completed_work_bytes(&mut self, bytes: u64) {
        self.work_completed_bytes = self.work_completed_bytes.saturating_add(bytes);
    }

    pub(super) fn finish_work(&mut self) {
        self.work_completed_bytes = self.work_completed_bytes.max(self.work_total_bytes);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileOperationStats {
    pub(super) total_bytes: u64,
    pub(super) total_files: usize,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum FileOperationError {
    Cancelled,
    Failed(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CopyNamePolicy {
    Original,
    UseCopyNamesInSameDirectory,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CopyEngine {
    Standard,
    ResumableDelta,
}

fn copy_options_for_operation() -> CopyOptions {
    let durability = if crate::debug_options::copy_fast_enabled() {
        CopyDurability::Fast
    } else {
        CopyDurability::Safe
    };
    CopyOptions::rsync_update(durability)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileOperationJob {
    pub(super) kind: FileOperationKind,
    pub(super) stats: FileOperationStats,
    steps: Vec<FileOperationStep>,
    roots: Vec<FileOperationRoot>,
    archive_diagnostics: Option<ArchiveDiagnostics>,
}

impl FileOperationJob {
    pub(super) fn initial_progress(&self) -> FileOperationProgress {
        FileOperationProgress {
            kind: self.kind,
            phase: FileOperationPhase::Preparing,
            total_bytes: self.stats.total_bytes,
            copied_bytes: 0,
            verified_bytes: 0,
            work_total_bytes: self.stats.total_bytes,
            work_completed_bytes: 0,
            total_files: self.stats.total_files,
            completed_files: 0,
            current_item: None,
            cancellable: self.kind != FileOperationKind::Extract,
        }
    }

    pub(super) fn archive_diagnostics(&self) -> Option<ArchiveDiagnostics> {
        self.archive_diagnostics.clone()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileOperationRoot {
    source: PathBuf,
    destination: PathBuf,
    source_is_dir: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ArchiveExtractEntry {
    display_path: PathBuf,
    destination: PathBuf,
    conflict: bool,
    byte_weight: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ArchiveExtractPlan {
    entries: Vec<ArchiveExtractEntry>,
    by_display_path: HashMap<PathBuf, usize>,
    by_destination: HashMap<PathBuf, usize>,
}

impl ArchiveExtractPlan {
    fn new(entries: Vec<ArchiveExtractEntry>) -> Self {
        let mut by_display_path = HashMap::with_capacity(entries.len());
        let mut by_destination = HashMap::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            by_display_path.insert(entry.display_path.clone(), index);
            by_destination.insert(entry.destination.clone(), index);
        }
        Self {
            entries,
            by_display_path,
            by_destination,
        }
    }

    fn entry_by_display_path(&self, path: &Path) -> Option<&ArchiveExtractEntry> {
        self.by_display_path
            .get(path)
            .and_then(|index| self.entries.get(*index))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum FileOperationStep {
    CreateDirectory(PathBuf),
    CopyFile {
        source: PathBuf,
        destination: PathBuf,
        conflict: bool,
        engine: CopyEngine,
    },
    MoveFile {
        source: PathBuf,
        destination: PathBuf,
        conflict: bool,
        copy_engine: CopyEngine,
    },
    ExtractArchive {
        archive: PathBuf,
        destination: PathBuf,
        plan: ArchiveExtractPlan,
        diagnostics: Option<ArchiveHandle>,
    },
    RemoveEmptyDirectory(PathBuf),
}

#[derive(Clone, Debug)]
enum ParallelFileTask {
    Copy {
        source: PathBuf,
        destination: PathBuf,
        engine: CopyEngine,
    },
    Move {
        source: PathBuf,
        destination: PathBuf,
        conflict: bool,
        copy_engine: CopyEngine,
    },
}

#[derive(Clone, Debug)]
struct ParallelFileTaskResult {
    destination: PathBuf,
}

#[derive(Clone, Debug)]
enum ParallelFileTaskEvent {
    Phase {
        phase: FileOperationPhase,
        current_item: Option<PathBuf>,
    },
    CopiedBytes(u64),
    VerifiedBytes(u64),
    WorkTotalBytes(u64),
    WorkCompletedBytes(u64),
    Completed,
}

fn prepare_file_operation(
    paths: &[PathBuf],
    destination: &Path,
    kind: FileOperationKind,
    copy_name_policy: CopyNamePolicy,
) -> Result<FileOperationJob, String> {
    if paths.is_empty() {
        return Err("No items were selected for drag-and-drop.".to_owned());
    }

    if !destination.is_dir() {
        return Err(format!(
            "{} is not a folder.",
            path_display_name(destination)
        ));
    }

    let destination_canonical = canonicalize_for_operation(destination)?;
    let mut reserved_destinations = HashSet::new();
    let mut steps = Vec::new();
    let mut roots = Vec::new();
    let mut stats = FileOperationStats {
        total_bytes: 0,
        total_files: 0,
    };

    for source in paths {
        if !source.exists() {
            return Err(format!("Could not find {}.", path_display_name(source)));
        }

        let file_name = source
            .file_name()
            .ok_or_else(|| format!("{} cannot be copied.", path_display_name(source)))?;
        let source_parent = source
            .parent()
            .ok_or_else(|| format!("{} cannot be moved or copied.", path_display_name(source)))?;
        let source_parent_canonical = canonicalize_for_operation(source_parent)?;
        let same_directory = source_parent_canonical == destination_canonical;
        if kind == FileOperationKind::Move && same_directory {
            continue;
        }

        let planned_destination = if kind == FileOperationKind::Copy
            && same_directory
            && copy_name_policy == CopyNamePolicy::UseCopyNamesInSameDirectory
        {
            paste_copy_destination(destination, file_name, &mut reserved_destinations)
        } else {
            let planned_destination = destination.join(file_name);
            if !reserved_destinations.insert(planned_destination.clone()) {
                return Err(format!(
                    "Multiple selected items are named {}.",
                    file_name.to_string_lossy()
                ));
            }
            planned_destination
        };

        if source.is_dir() {
            let source_canonical = canonicalize_for_operation(source)?;
            let canonical_planned_destination = destination_canonical.join(
                planned_destination
                    .file_name()
                    .unwrap_or_else(|| OsStr::new("")),
            );
            if canonical_planned_destination.starts_with(&source_canonical) {
                let operation = match kind {
                    FileOperationKind::Move => "move",
                    FileOperationKind::Copy => "copy",
                    FileOperationKind::Extract => "extract",
                };
                return Err(format!(
                    "Cannot {operation} {} into itself.",
                    path_display_name(source)
                ));
            }
        }

        plan_path_operation(source, &planned_destination, kind, &mut steps, &mut stats)?;
        roots.push(FileOperationRoot {
            source: source.clone(),
            destination: planned_destination,
            source_is_dir: source.is_dir(),
        });
    }

    Ok(FileOperationJob {
        kind,
        stats,
        steps,
        roots,
        archive_diagnostics: None,
    })
}

pub(super) fn trash_paths(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("No items were selected to delete.".to_owned());
    }

    trash::delete_all(paths)
        .map_err(|error| format!("Could not move selected items to the Recycle Bin: {error}"))
}

pub(super) fn remove_paths_permanently(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("No items were selected to delete.".to_owned());
    }

    for path in paths {
        if !path.exists() {
            return Err(format!("Could not find {}.", path_display_name(path)));
        }
    }

    for path in paths {
        remove_source(path).map_err(|error| format_path_error("delete", path, error))?;
    }

    Ok(())
}

pub(super) fn remove_existing_paths_permanently(paths: &[PathBuf]) -> Result<bool, String> {
    let mut removed_any = false;

    for path in paths {
        match path.try_exists() {
            Ok(true) => {
                remove_source(path).map_err(|error| format_path_error("delete", path, error))?;
                removed_any = true;
            }
            Ok(false) => {}
            Err(error) => return Err(format_path_error("delete", path, error)),
        }
    }

    Ok(removed_any)
}

fn plan_path_operation(
    source: &Path,
    destination: &Path,
    kind: FileOperationKind,
    steps: &mut Vec<FileOperationStep>,
    stats: &mut FileOperationStats,
) -> Result<(), String> {
    let metadata =
        fs::metadata(source).map_err(|error| format_path_error("read", source, error))?;

    if metadata.is_dir() {
        if destination.exists() {
            if !destination.is_dir() {
                return Err(format!(
                    "{} already exists and is not a folder.",
                    path_display_name(destination)
                ));
            }
        } else {
            steps.push(FileOperationStep::CreateDirectory(
                destination.to_path_buf(),
            ));
        }

        for entry in
            fs::read_dir(source).map_err(|error| format_path_error("read", source, error))?
        {
            let entry = entry.map_err(|error| format_path_error("read", source, error))?;
            plan_path_operation(
                &entry.path(),
                &destination.join(entry.file_name()),
                kind,
                steps,
                stats,
            )?;
        }

        if kind == FileOperationKind::Move {
            steps.push(FileOperationStep::RemoveEmptyDirectory(
                source.to_path_buf(),
            ));
        }
    } else if destination.is_dir() {
        return Err(format!(
            "{} already exists and is a folder.",
            path_display_name(destination)
        ));
    } else {
        let conflict = destination.exists();
        stats.total_files += 1;
        stats.total_bytes = stats.total_bytes.saturating_add(metadata.len());
        let copy_engine = copy_engine_for_paths(source, destination, conflict);
        match kind {
            FileOperationKind::Copy => steps.push(FileOperationStep::CopyFile {
                source: source.to_path_buf(),
                destination: destination.to_path_buf(),
                conflict,
                engine: copy_engine,
            }),
            FileOperationKind::Move => steps.push(FileOperationStep::MoveFile {
                source: source.to_path_buf(),
                destination: destination.to_path_buf(),
                conflict,
                copy_engine,
            }),
            FileOperationKind::Extract => {}
        }
    }

    Ok(())
}

fn prepare_extract_archive_operation(
    archives: &[PathBuf],
    destination: &Path,
) -> Result<FileOperationJob, String> {
    let archive_diagnostics = ArchiveDiagnostics::start();
    let mut total_timing = crate::debug_options::ArchiveTiming::start(
        "prepare.total",
        format_args!("archives={}", archives.len()),
    );

    if archives.is_empty() {
        return Err("No archive files were selected.".to_owned());
    }

    if !destination.is_dir() {
        return Err(format!(
            "{} is not a folder.",
            path_display_name(destination)
        ));
    }

    let mut steps = Vec::new();
    let mut roots = Vec::new();
    let mut reserved_destinations = HashSet::new();
    let mut stats = FileOperationStats {
        total_bytes: 0,
        total_files: 0,
    };

    for archive in archives {
        if !archive.exists() {
            return Err(format!("Could not find {}.", path_display_name(archive)));
        }

        if !archive.is_file() || !archive_path_is_supported(archive) {
            return Err(format!(
                "{} is not a supported archive.",
                path_display_name(archive)
            ));
        }

        let mut listing_timing = crate::debug_options::ArchiveTiming::start(
            "prepare.list",
            format_args!("archive={archive:?}"),
        );
        let listing_started = Instant::now();
        let listing = archive_listing(archive);
        let listing_elapsed = listing_started.elapsed();
        if listing.is_ok() {
            listing_timing.ok();
        }
        drop(listing_timing);
        let listing = listing?;

        let mut plan_timing = crate::debug_options::ArchiveTiming::start(
            "prepare.plan",
            format_args!("archive={archive:?}"),
        );
        let plan_started = Instant::now();
        let archive_size = fs::metadata(archive)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let sanitized_entries = sanitized_entries_from_listing(&listing.entries);
        let top_level_entries = top_level_entries_from_sanitized(&sanitized_entries);
        if top_level_entries.is_empty() {
            return Err(format!(
                "{} does not contain any files.",
                path_display_name(archive)
            ));
        }

        let extract_to = archive_extract_destination(archive, destination, &top_level_entries)?;
        let output_roots = archive_output_roots(&extract_to, &top_level_entries);
        let mut entries = planned_extract_entries_from_sanitized(&sanitized_entries, &extract_to);
        assign_archive_entry_byte_weights(&mut entries, archive_size);

        for entry in &mut entries {
            if !reserved_destinations.insert(entry.destination.clone()) {
                return Err(format!(
                    "Multiple selected archives contain {}.",
                    path_display_name(&entry.destination)
                ));
            }
            entry.conflict = entry.destination.exists();
        }

        let diagnostics = archive_diagnostics.as_ref().map(|operation| {
            let handle = operation.add_archive(
                listing.id,
                archive_extract_backend(archive),
                archive_size,
                listing.entries.len(),
                entries.len(),
            );
            handle.phase("listing", listing_elapsed);
            handle.phase("planning", plan_started.elapsed());
            handle
        });
        let entry_count = entries.len();
        steps.push(FileOperationStep::ExtractArchive {
            archive: archive.clone(),
            destination: extract_to,
            plan: ArchiveExtractPlan::new(entries),
            diagnostics,
        });

        stats.total_files = stats.total_files.saturating_add(entry_count.max(1));
        stats.total_bytes = stats.total_bytes.saturating_add(archive_size);

        for output in output_roots {
            roots.push(FileOperationRoot {
                source: archive.clone(),
                destination: output,
                source_is_dir: false,
            });
        }
        plan_timing.ok();
    }

    let job = FileOperationJob {
        kind: FileOperationKind::Extract,
        stats,
        steps,
        roots,
        archive_diagnostics,
    };
    total_timing.ok();
    Ok(job)
}

fn file_conflicts_for_job(job: &FileOperationJob) -> Vec<FileConflict> {
    let mut file_conflicts = Vec::new();
    for step in &job.steps {
        match step {
            FileOperationStep::CopyFile {
                source,
                destination,
                conflict: true,
                ..
            }
            | FileOperationStep::MoveFile {
                source,
                destination,
                conflict: true,
                ..
            } => file_conflicts.push(FileConflict {
                source: source.clone(),
                destination: destination.clone(),
            }),
            FileOperationStep::ExtractArchive { archive, plan, .. } => {
                file_conflicts.extend(plan.entries.iter().filter_map(|entry| {
                    entry.conflict.then(|| FileConflict {
                        source: archive.clone(),
                        destination: entry.destination.clone(),
                    })
                }));
            }
            _ => {}
        }
    }
    file_conflicts
}

pub(super) fn execute_file_operation(
    job: FileOperationJob,
    conflict_choice: ConflictChoice,
) -> Result<FileOperationSummary, String> {
    execute_file_operation_with_progress(
        job,
        conflict_choice,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
        |_| {},
    )
    .map_err(|error| match error {
        FileOperationError::Cancelled => "The file operation was cancelled.".to_owned(),
        FileOperationError::Failed(error) => error,
    })
}

fn execute_copy_move_operation_with_progress(
    job: FileOperationJob,
    conflict_choice: ConflictChoice,
    cancel: Arc<AtomicBool>,
    terminate: Arc<AtomicBool>,
    on_progress: impl FnMut(FileOperationProgress),
) -> Result<FileOperationSummary, FileOperationError> {
    let cleanup_targets = resumable_copy_cleanup_targets(&job, conflict_choice);
    let result =
        execute_copy_move_operation_with_progress_impl(job, conflict_choice, cancel, on_progress);
    if matches!(result, Err(FileOperationError::Cancelled)) && terminate.load(Ordering::Relaxed) {
        cleanup_resumable_copy_targets(&cleanup_targets);
    }
    result
}

fn execute_copy_move_operation_with_progress_impl(
    job: FileOperationJob,
    conflict_choice: ConflictChoice,
    cancel: Arc<AtomicBool>,
    mut on_progress: impl FnMut(FileOperationProgress),
) -> Result<FileOperationSummary, FileOperationError> {
    let copy_options = copy_options_for_operation();
    let mut operated_destinations = HashSet::new();
    let mut progress = job.initial_progress();
    on_progress(progress.clone());

    let create_directories = job
        .steps
        .iter()
        .filter_map(|step| match step {
            FileOperationStep::CreateDirectory(path) => Some(path.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !create_directories.is_empty() {
        progress.phase = FileOperationPhase::Preparing;
        on_progress(progress.clone());
        create_directories.par_iter().try_for_each(|path| {
            if cancel.load(Ordering::Relaxed) {
                return Err(FileOperationError::Cancelled);
            }
            fs::create_dir_all(path).map_err(|error| operation_error("create", path, error))
        })?;
    }

    let file_tasks = job
        .steps
        .iter()
        .filter_map(|step| match step {
            FileOperationStep::CopyFile {
                source,
                destination,
                conflict,
                engine,
            } => {
                if *conflict && conflict_choice == ConflictChoice::Skip {
                    None
                } else {
                    Some(ParallelFileTask::Copy {
                        source: source.clone(),
                        destination: destination.clone(),
                        engine: *engine,
                    })
                }
            }
            FileOperationStep::MoveFile {
                source,
                destination,
                conflict,
                copy_engine,
            } => {
                if *conflict && conflict_choice == ConflictChoice::Skip {
                    None
                } else {
                    Some(ParallelFileTask::Move {
                        source: source.clone(),
                        destination: destination.clone(),
                        conflict: *conflict,
                        copy_engine: *copy_engine,
                    })
                }
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    let task_results = run_parallel_file_tasks(
        &file_tasks,
        job.kind,
        copy_options,
        cancel.clone(),
        &mut progress,
        &mut on_progress,
    )?;
    for result in task_results {
        operated_destinations.insert(result.destination);
    }

    for step in &job.steps {
        if cancel.load(Ordering::Relaxed) {
            progress.phase = FileOperationPhase::Cancelled;
            progress.cancellable = false;
            on_progress(progress);
            return Err(FileOperationError::Cancelled);
        }

        if let FileOperationStep::RemoveEmptyDirectory(path) = step {
            progress.phase = FileOperationPhase::Removing;
            progress.current_item = Some(path.clone());
            on_progress(progress.clone());
            remove_empty_directory(path).map_err(|error| operation_error("remove", path, error))?;
        }
    }

    finish_file_operation_summary(job, operated_destinations, progress, on_progress)
}

fn run_parallel_file_tasks(
    tasks: &[ParallelFileTask],
    kind: FileOperationKind,
    copy_options: CopyOptions,
    cancel: Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) -> Result<Vec<ParallelFileTaskResult>, FileOperationError> {
    if tasks.is_empty() {
        return Ok(Vec::new());
    }
    if let [task] = tasks {
        return run_single_file_task(task, kind, copy_options, cancel, progress, on_progress)
            .map(|result| vec![result]);
    }

    let parallelism = file_operation_parallelism(tasks.len());
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(parallelism)
        .thread_name(|index| format!("explorer-copy-{index}"))
        .build()
        .map_err(|error| FileOperationError::Failed(error.to_string()))?;
    let (event_tx, event_rx) = mpsc::channel::<ParallelFileTaskEvent>();
    let (result_tx, result_rx) =
        mpsc::channel::<Result<Vec<ParallelFileTaskResult>, FileOperationError>>();

    std::thread::scope(|scope| {
        let tasks = tasks.to_vec();
        let cancel_for_workers = cancel.clone();
        scope.spawn(move || {
            let result = pool.install(|| {
                tasks
                    .par_iter()
                    .map(|task| {
                        let result = run_parallel_file_task(
                            task,
                            kind,
                            copy_options,
                            cancel_for_workers.clone(),
                            event_tx.clone(),
                        );
                        if result.is_err() {
                            cancel_for_workers.store(true, Ordering::Relaxed);
                        }
                        result
                    })
                    .collect::<Result<Vec<_>, _>>()
            });
            let _ = result_tx.send(result);
        });

        loop {
            match result_rx.recv_timeout(Duration::from_millis(25)) {
                Ok(result) => {
                    drain_parallel_file_task_events(&event_rx, progress, on_progress);
                    return result;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    drain_parallel_file_task_events(&event_rx, progress, on_progress);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(FileOperationError::Failed(
                        "File operation worker disconnected".to_owned(),
                    ));
                }
            }
        }
    })
}

fn resumable_copy_cleanup_targets(
    job: &FileOperationJob,
    conflict_choice: ConflictChoice,
) -> Vec<(PathBuf, PathBuf)> {
    job.steps
        .iter()
        .filter_map(|step| match step {
            FileOperationStep::CopyFile {
                source,
                destination,
                conflict,
                engine: CopyEngine::ResumableDelta,
            } if !*conflict || conflict_choice == ConflictChoice::Replace => {
                Some((source.clone(), destination.clone()))
            }
            FileOperationStep::MoveFile {
                source,
                destination,
                conflict,
                copy_engine: CopyEngine::ResumableDelta,
            } if !*conflict || conflict_choice == ConflictChoice::Replace => {
                Some((source.clone(), destination.clone()))
            }
            _ => None,
        })
        .collect()
}

fn cleanup_resumable_copy_targets(targets: &[(PathBuf, PathBuf)]) {
    for (source, destination) in targets {
        cleanup_resumable_copy_progress(source, destination);
    }
}

fn run_single_file_task(
    task: &ParallelFileTask,
    kind: FileOperationKind,
    copy_options: CopyOptions,
    cancel: Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) -> Result<ParallelFileTaskResult, FileOperationError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(FileOperationError::Cancelled);
    }

    match task {
        ParallelFileTask::Copy {
            source,
            destination,
            engine,
        } => {
            progress.phase = FileOperationPhase::Copying;
            progress.current_item = Some(source.clone());
            on_progress(progress.clone());
            copy_source_file_with_progress(
                source,
                destination,
                &cancel,
                progress,
                on_progress,
                *engine,
                copy_options,
            )
            .map_err(|error| operation_error("copy", source, error))?;
            Ok(ParallelFileTaskResult {
                destination: destination.clone(),
            })
        }
        ParallelFileTask::Move {
            source,
            destination,
            conflict,
            copy_engine,
        } => {
            if *conflict {
                progress.phase = FileOperationPhase::Copying;
                progress.current_item = Some(source.clone());
                on_progress(progress.clone());
                copy_source_file_with_progress(
                    source,
                    destination,
                    &cancel,
                    progress,
                    on_progress,
                    *copy_engine,
                    copy_options,
                )
                .map_err(|error| operation_error("move", source, error))?;
                remove_source(source).map_err(|error| operation_error("remove", source, error))?;
            } else {
                progress.phase = if kind == FileOperationKind::Move {
                    FileOperationPhase::Moving
                } else {
                    FileOperationPhase::Copying
                };
                progress.current_item = Some(source.clone());
                on_progress(progress.clone());
                move_source_file_with_progress(
                    source,
                    destination,
                    &cancel,
                    progress,
                    on_progress,
                    *copy_engine,
                    copy_options,
                )
                .map_err(|error| operation_error("move", source, error))?;
            }
            Ok(ParallelFileTaskResult {
                destination: destination.clone(),
            })
        }
    }
}

fn run_parallel_file_task(
    task: &ParallelFileTask,
    kind: FileOperationKind,
    copy_options: CopyOptions,
    cancel: Arc<AtomicBool>,
    event_tx: mpsc::Sender<ParallelFileTaskEvent>,
) -> Result<ParallelFileTaskResult, FileOperationError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(FileOperationError::Cancelled);
    }

    let (source, destination, phase) = match task {
        ParallelFileTask::Copy {
            source,
            destination,
            ..
        } => (source, destination, FileOperationPhase::Copying),
        ParallelFileTask::Move {
            source,
            destination,
            conflict,
            ..
        } => (
            source,
            destination,
            if *conflict {
                FileOperationPhase::Copying
            } else {
                FileOperationPhase::Moving
            },
        ),
    };
    let _ = event_tx.send(ParallelFileTaskEvent::Phase {
        phase,
        current_item: Some(source.clone()),
    });

    let mut task_progress = FileOperationProgress {
        kind,
        phase,
        total_bytes: 0,
        copied_bytes: 0,
        verified_bytes: 0,
        work_total_bytes: 0,
        work_completed_bytes: 0,
        total_files: 0,
        completed_files: 0,
        current_item: Some(source.clone()),
        cancellable: true,
    };
    let mut last_copied_bytes = 0u64;
    let mut last_verified_bytes = 0u64;
    let mut last_work_total_bytes = 0u64;
    let mut last_work_completed_bytes = 0u64;
    let mut last_phase = phase;
    let mut last_item = Some(source.clone());
    let mut publish_worker_progress = |progress: FileOperationProgress| {
        if progress.phase != last_phase || progress.current_item != last_item {
            last_phase = progress.phase;
            last_item = progress.current_item.clone();
            let _ = event_tx.send(ParallelFileTaskEvent::Phase {
                phase: progress.phase,
                current_item: progress.current_item,
            });
        }
        if progress.work_total_bytes > last_work_total_bytes {
            let delta = progress.work_total_bytes - last_work_total_bytes;
            last_work_total_bytes = progress.work_total_bytes;
            let _ = event_tx.send(ParallelFileTaskEvent::WorkTotalBytes(delta));
        }
        if progress.copied_bytes > last_copied_bytes {
            let delta = progress.copied_bytes - last_copied_bytes;
            last_copied_bytes = progress.copied_bytes;
            let _ = event_tx.send(ParallelFileTaskEvent::CopiedBytes(delta));
        }
        if progress.verified_bytes > last_verified_bytes {
            let delta = progress.verified_bytes - last_verified_bytes;
            last_verified_bytes = progress.verified_bytes;
            let _ = event_tx.send(ParallelFileTaskEvent::VerifiedBytes(delta));
        }
        if progress.work_completed_bytes > last_work_completed_bytes {
            let delta = progress.work_completed_bytes - last_work_completed_bytes;
            last_work_completed_bytes = progress.work_completed_bytes;
            let _ = event_tx.send(ParallelFileTaskEvent::WorkCompletedBytes(delta));
        }
    };

    match task {
        ParallelFileTask::Copy {
            source,
            destination,
            engine,
        } => copy_source_file_with_progress(
            source,
            destination,
            &cancel,
            &mut task_progress,
            &mut publish_worker_progress,
            *engine,
            copy_options,
        )
        .map_err(|error| operation_error("copy", source, error))?,
        ParallelFileTask::Move {
            source,
            destination,
            conflict,
            copy_engine,
        } => {
            if *conflict {
                copy_source_file_with_progress(
                    source,
                    destination,
                    &cancel,
                    &mut task_progress,
                    &mut publish_worker_progress,
                    *copy_engine,
                    copy_options,
                )
                .map_err(|error| operation_error("move", source, error))?;
                remove_source(source).map_err(|error| operation_error("remove", source, error))?;
            } else {
                move_source_file_with_progress(
                    source,
                    destination,
                    &cancel,
                    &mut task_progress,
                    &mut publish_worker_progress,
                    *copy_engine,
                    copy_options,
                )
                .map_err(|error| operation_error("move", source, error))?;
            }
        }
    }

    let _ = event_tx.send(ParallelFileTaskEvent::Completed);
    Ok(ParallelFileTaskResult {
        destination: destination.clone(),
    })
}

fn drain_parallel_file_task_events(
    event_rx: &mpsc::Receiver<ParallelFileTaskEvent>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) {
    while let Ok(event) = event_rx.try_recv() {
        match event {
            ParallelFileTaskEvent::Phase {
                phase,
                current_item,
            } => {
                progress.phase = phase;
                progress.current_item = current_item;
            }
            ParallelFileTaskEvent::CopiedBytes(bytes) => {
                progress.copied_bytes = progress.copied_bytes.saturating_add(bytes);
            }
            ParallelFileTaskEvent::VerifiedBytes(bytes) => {
                progress.verified_bytes = progress.verified_bytes.saturating_add(bytes);
            }
            ParallelFileTaskEvent::WorkTotalBytes(bytes) => {
                progress.work_total_bytes = progress.work_total_bytes.saturating_add(bytes);
            }
            ParallelFileTaskEvent::WorkCompletedBytes(bytes) => {
                progress.add_completed_work_bytes(bytes);
            }
            ParallelFileTaskEvent::Completed => {
                progress.completed_files = progress.completed_files.saturating_add(1);
            }
        }
        on_progress(progress.clone());
    }
}

fn file_operation_parallelism(task_count: usize) -> usize {
    let override_parallelism = COPY_PARALLELISM_OVERRIDE.load(Ordering::Relaxed);
    if override_parallelism > 0 {
        return task_count.max(1).min(override_parallelism.max(1));
    }

    let available = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4);
    task_count.max(1).min(available.max(1)).min(16)
}

fn finish_file_operation_summary(
    job: FileOperationJob,
    operated_destinations: HashSet<PathBuf>,
    mut progress: FileOperationProgress,
    mut on_progress: impl FnMut(FileOperationProgress),
) -> Result<FileOperationSummary, FileOperationError> {
    let mut summary = FileOperationSummary::default();
    summary.kind = job.kind;
    summary.archive_diagnostics = job.archive_diagnostics.clone();
    for root in &job.roots {
        if root.source_is_dir {
            if root.destination.exists() {
                summary.destination_paths.push(root.destination.clone());
            }
        } else if operated_destinations.contains(&root.destination) {
            summary.destination_paths.push(root.destination.clone());
        }

        if job.kind == FileOperationKind::Move && !root.source.exists() {
            summary.moved_source_paths.push(root.source.clone());
            if root.destination.exists() {
                summary.moved_paths.push(FileOperationMove {
                    source: root.source.clone(),
                    destination: root.destination.clone(),
                });
            }
        }
    }

    progress.phase = FileOperationPhase::Finished;
    progress.current_item = None;
    progress.finish_work();
    progress.completed_files = progress.completed_files.max(progress.total_files);
    progress.cancellable = false;
    on_progress(progress);
    Ok(summary)
}

pub(super) fn execute_file_operation_with_progress(
    job: FileOperationJob,
    conflict_choice: ConflictChoice,
    cancel: Arc<AtomicBool>,
    terminate: Arc<AtomicBool>,
    mut on_progress: impl FnMut(FileOperationProgress),
) -> Result<FileOperationSummary, FileOperationError> {
    if job.kind != FileOperationKind::Extract {
        return execute_copy_move_operation_with_progress(
            job,
            conflict_choice,
            cancel,
            terminate,
            on_progress,
        );
    }

    let operation_diagnostics = job.archive_diagnostics.clone();
    let archive_timings_enabled = crate::debug_options::archive_timings_enabled();
    let archive_count = if job.kind == FileOperationKind::Extract && archive_timings_enabled {
        job.steps
            .iter()
            .filter(|step| matches!(step, FileOperationStep::ExtractArchive { .. }))
            .count()
    } else {
        0
    };
    let mut total_timing = (job.kind == FileOperationKind::Extract).then(|| {
        crate::debug_options::ArchiveTiming::start(
            "execute.total",
            format_args!("archives={archive_count}"),
        )
    });
    let mut operated_destinations = HashSet::new();
    let copy_options = copy_options_for_operation();
    let mut progress = job.initial_progress();
    on_progress(progress.clone());

    for step in &job.steps {
        if cancel.load(Ordering::Relaxed) {
            progress.phase = FileOperationPhase::Cancelled;
            progress.cancellable = false;
            on_progress(progress);
            if let Some(timing) = total_timing.as_mut() {
                timing.cancelled();
            }
            if let Some(diagnostics) = &operation_diagnostics {
                diagnostics.finish("cancelled");
            }
            return Err(FileOperationError::Cancelled);
        }

        match step {
            FileOperationStep::CreateDirectory(path) => {
                progress.phase = FileOperationPhase::Preparing;
                progress.current_item = Some(path.clone());
                on_progress(progress.clone());
                fs::create_dir(path).map_err(|error| operation_error("create", path, error))?;
            }
            FileOperationStep::CopyFile {
                source,
                destination,
                conflict,
                engine,
            } => {
                if *conflict && conflict_choice == ConflictChoice::Skip {
                    continue;
                }
                progress.phase = FileOperationPhase::Copying;
                progress.current_item = Some(source.clone());
                on_progress(progress.clone());
                copy_source_file_with_progress(
                    source,
                    destination,
                    &cancel,
                    &mut progress,
                    &mut on_progress,
                    *engine,
                    copy_options,
                )
                .map_err(|error| operation_error("copy", source, error))?;
                operated_destinations.insert(destination.clone());
            }
            FileOperationStep::MoveFile {
                source,
                destination,
                conflict,
                copy_engine,
            } => {
                if *conflict && conflict_choice == ConflictChoice::Skip {
                    continue;
                }
                if *conflict {
                    progress.phase = FileOperationPhase::Copying;
                    progress.current_item = Some(source.clone());
                    on_progress(progress.clone());
                    copy_source_file_with_progress(
                        source,
                        destination,
                        &cancel,
                        &mut progress,
                        &mut on_progress,
                        *copy_engine,
                        copy_options,
                    )
                    .map_err(|error| operation_error("move", source, error))?;
                    remove_source(source)
                        .map_err(|error| operation_error("remove", source, error))?;
                } else {
                    progress.phase = FileOperationPhase::Moving;
                    progress.current_item = Some(source.clone());
                    on_progress(progress.clone());
                    move_source_file_with_progress(
                        source,
                        destination,
                        &cancel,
                        &mut progress,
                        &mut on_progress,
                        *copy_engine,
                        copy_options,
                    )
                    .map_err(|error| operation_error("move", source, error))?;
                }
                operated_destinations.insert(destination.clone());
            }
            FileOperationStep::ExtractArchive {
                archive,
                destination,
                plan,
                diagnostics,
            } => {
                progress.phase = FileOperationPhase::Extracting;
                on_progress(progress.clone());
                let backend = archive_timings_enabled
                    .then(|| archive_extract_backend(archive))
                    .unwrap_or_default();
                let mut extract_timing = crate::debug_options::ArchiveTiming::start(
                    "execute.extract",
                    format_args!("archive={archive:?} backend={backend}"),
                );
                let sampler = diagnostics.as_ref().map(ArchiveHandle::sampler);
                if let Some(diagnostics) = diagnostics {
                    let conflicts =
                        plan.entries.iter().filter(|entry| entry.conflict).count() as u64;
                    match conflict_choice {
                        ConflictChoice::Replace => {
                            diagnostics
                                .metrics()
                                .entries_replaced
                                .fetch_add(conflicts, Ordering::Relaxed);
                        }
                        ConflictChoice::Skip => {
                            diagnostics
                                .metrics()
                                .entries_skipped
                                .fetch_add(conflicts, Ordering::Relaxed);
                        }
                    }
                }
                let diagnostic_metrics =
                    diagnostics.as_ref().map(|handle| handle.metrics().clone());
                let mut last_progress_publish = Instant::now();
                let mut last_published_completed = progress.completed_files;
                let mut last_published_bytes = progress.copied_bytes;
                let mut diagnostic_progress = |progress: FileOperationProgress| {
                    let skipped_item_completed = progress.completed_files
                        > last_published_completed
                        && progress.copied_bytes == last_published_bytes;
                    if !skipped_item_completed
                        && last_progress_publish.elapsed() < ARCHIVE_PROGRESS_PUBLISH_INTERVAL
                    {
                        return;
                    }
                    let publish_started = Instant::now();
                    last_published_completed = progress.completed_files;
                    last_published_bytes = progress.copied_bytes;
                    on_progress(progress);
                    last_progress_publish = Instant::now();
                    if let Some(metrics) = &diagnostic_metrics {
                        metrics.progress_callbacks.fetch_add(1, Ordering::Relaxed);
                    }
                    if let Some(diagnostics) = diagnostics {
                        diagnostics.phase("progress_publication", publish_started.elapsed());
                    }
                };
                let result = extract_archive_with_entry_progress(
                    archive,
                    destination,
                    plan,
                    conflict_choice,
                    &cancel,
                    &mut progress,
                    &mut diagnostic_progress,
                    diagnostics.as_ref(),
                );
                on_progress(progress.clone());
                if let Some(metrics) = &diagnostic_metrics {
                    metrics.progress_callbacks.fetch_add(1, Ordering::Relaxed);
                }
                let outcome = match &result {
                    Ok(()) => "ok",
                    Err(FileOperationError::Cancelled) => "cancelled",
                    Err(FileOperationError::Failed(_)) => "error",
                };
                if let (Some(diagnostics), Some(sampler)) = (diagnostics, sampler) {
                    diagnostics.finish(outcome, sampler.finish());
                }
                match &result {
                    Ok(()) => extract_timing.ok(),
                    Err(FileOperationError::Cancelled) => {
                        extract_timing.cancelled();
                        if let Some(timing) = total_timing.as_mut() {
                            timing.cancelled();
                        }
                    }
                    Err(FileOperationError::Failed(_)) => {}
                }
                drop(extract_timing);
                if result.is_err() {
                    if let Some(diagnostics) = &operation_diagnostics {
                        diagnostics.finish(outcome);
                    }
                }
                result?;
                operated_destinations.insert(destination.clone());
            }
            FileOperationStep::RemoveEmptyDirectory(path) => {
                progress.phase = FileOperationPhase::Removing;
                progress.current_item = Some(path.clone());
                on_progress(progress.clone());
                remove_empty_directory(path)
                    .map_err(|error| operation_error("remove", path, error))?;
            }
        }
    }

    let mut finalize_timing = (job.kind == FileOperationKind::Extract).then(|| {
        crate::debug_options::ArchiveTiming::start(
            "execute.finalize",
            format_args!("archives={archive_count}"),
        )
    });
    let mut summary = FileOperationSummary::default();
    summary.kind = job.kind;
    summary.archive_diagnostics = operation_diagnostics;
    for root in &job.roots {
        if job.kind == FileOperationKind::Extract {
            if root.destination.exists() {
                summary.destination_paths.push(root.destination.clone());
            }
        } else if root.source_is_dir {
            if root.destination.exists() {
                summary.destination_paths.push(root.destination.clone());
            }
        } else if operated_destinations.contains(&root.destination) {
            summary.destination_paths.push(root.destination.clone());
        }

        if job.kind == FileOperationKind::Move && !root.source.exists() {
            summary.moved_source_paths.push(root.source.clone());
            if root.destination.exists() {
                summary.moved_paths.push(FileOperationMove {
                    source: root.source.clone(),
                    destination: root.destination.clone(),
                });
            }
        }
    }

    progress.phase = FileOperationPhase::Finished;
    progress.current_item = None;
    progress.finish_work();
    progress.completed_files = progress.completed_files.max(progress.total_files);
    progress.cancellable = false;
    on_progress(progress);

    if let Some(timing) = finalize_timing.as_mut() {
        timing.ok();
    }
    if let Some(timing) = total_timing.as_mut() {
        timing.ok();
    }
    Ok(summary)
}

fn archive_extract_backend(archive: &Path) -> &'static str {
    if archive_is_rar(archive) {
        "rar"
    } else if archive_is_7z(archive) {
        "7z"
    } else if archive_is_ar(archive) {
        "ar"
    } else if archive_supports_filtered_extract(archive)
        || archive_is_single_file_compression(archive)
    {
        "decompress-filtered"
    } else {
        "decompress"
    }
}

struct DecompressDiagnosticsObserver {
    diagnostics: ArchiveHandle,
    entry_started: std::sync::Mutex<HashMap<PathBuf, Instant>>,
}

impl DecompressDiagnosticsObserver {
    fn new(diagnostics: ArchiveHandle) -> Self {
        Self {
            diagnostics,
            entry_started: std::sync::Mutex::new(HashMap::new()),
        }
    }
}

impl decompress::Observer for DecompressDiagnosticsObserver {
    fn observe(&self, event: decompress::ObserveEvent<'_>) {
        let diagnostics_started = Instant::now();
        let metrics = self.diagnostics.metrics();
        metrics.observer_callbacks.fetch_add(1, Ordering::Relaxed);
        match event {
            decompress::ObserveEvent::BackendInit => {}
            decompress::ObserveEvent::EntryStart { path, is_directory } => {
                self.entry_started
                    .lock()
                    .expect("archive entry diagnostics")
                    .insert(path.to_path_buf(), Instant::now());
                if is_directory {
                    metrics.directories.fetch_add(1, Ordering::Relaxed);
                } else {
                    metrics.files.fetch_add(1, Ordering::Relaxed);
                }
            }
            decompress::ObserveEvent::EntryComplete {
                path,
                bytes,
                is_directory,
            } => {
                let elapsed = self
                    .entry_started
                    .lock()
                    .expect("archive entry diagnostics")
                    .remove(path)
                    .map_or(Duration::ZERO, |started| started.elapsed());
                metrics.entries_completed.fetch_add(1, Ordering::Relaxed);
                if !is_directory {
                    metrics
                        .logical_output_bytes
                        .fetch_add(bytes, Ordering::Relaxed);
                    metrics.decoded_bytes.fetch_add(bytes, Ordering::Relaxed);
                    if bytes == 0 {
                        metrics.zero_byte_files.fetch_add(1, Ordering::Relaxed);
                    }
                }
                self.diagnostics.record_entry(path, bytes, elapsed, "ok");
            }
            decompress::ObserveEvent::DirectoryCreate => {
                metrics.directory_creates.fetch_add(1, Ordering::Relaxed);
            }
            decompress::ObserveEvent::FileCreate => {
                metrics.file_creates.fetch_add(1, Ordering::Relaxed);
            }
            decompress::ObserveEvent::MetadataOperation => {
                metrics.metadata_operations.fetch_add(1, Ordering::Relaxed);
            }
            decompress::ObserveEvent::OutputWrite { bytes, elapsed } => {
                metrics
                    .output_bytes_written
                    .fetch_add(bytes, Ordering::Relaxed);
                self.diagnostics.phase("entry_copy", elapsed);
            }
            decompress::ObserveEvent::Flush => {
                metrics.flushes.fetch_add(1, Ordering::Relaxed);
            }
        }
        metrics.diagnostics_nanos.fetch_add(
            diagnostics_started
                .elapsed()
                .as_nanos()
                .min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }
}

fn record_completed_entry(
    diagnostics: &ArchiveHandle,
    entry: &ArchiveExtractEntry,
    bytes: u64,
    elapsed: Duration,
    outcome: &'static str,
) {
    let diagnostics_started = Instant::now();
    let metrics = diagnostics.metrics();
    metrics.entries_completed.fetch_add(1, Ordering::Relaxed);
    metrics.files.fetch_add(1, Ordering::Relaxed);
    metrics
        .logical_output_bytes
        .fetch_add(bytes, Ordering::Relaxed);
    metrics.decoded_bytes.fetch_add(bytes, Ordering::Relaxed);
    metrics
        .output_bytes_written
        .fetch_add(bytes, Ordering::Relaxed);
    if bytes == 0 {
        metrics.zero_byte_files.fetch_add(1, Ordering::Relaxed);
    }
    diagnostics.record_entry_with_phase("entry_copy", &entry.display_path, bytes, elapsed, outcome);
    metrics.diagnostics_nanos.fetch_add(
        diagnostics_started
            .elapsed()
            .as_nanos()
            .min(u64::MAX as u128) as u64,
        Ordering::Relaxed,
    );
}

fn extract_archive_with_entry_progress(
    archive: &Path,
    destination: &Path,
    plan: &ArchiveExtractPlan,
    conflict_choice: ConflictChoice,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    diagnostics: Option<&ArchiveHandle>,
) -> Result<(), FileOperationError> {
    if archive_is_rar(archive) {
        extract_rar_archive_with_entry_progress(
            archive,
            destination,
            plan,
            conflict_choice,
            cancel,
            progress,
            on_progress,
            diagnostics,
        )?;
    } else if archive_is_7z(archive) {
        extract_7z_archive_with_entry_progress(
            archive,
            destination,
            plan,
            conflict_choice,
            cancel,
            progress,
            on_progress,
            diagnostics,
        )?;
    } else if archive_is_ar(archive) {
        extract_ar_archive_with_entry_progress(
            archive,
            destination,
            plan,
            conflict_choice,
            cancel,
            progress,
            on_progress,
            diagnostics,
        )?;
    } else if archive_supports_filtered_extract(archive)
        || archive_is_single_file_compression(archive)
    {
        let entry_details = plan
            .by_destination
            .iter()
            .map(|(destination, index)| {
                let entry = &plan.entries[*index];
                (
                    destination.clone(),
                    (*index, entry.byte_weight, entry.conflict),
                )
            })
            .collect::<HashMap<_, _>>();

        let cancel_filter = cancel.clone();
        let (tx, rx) = std::sync::mpsc::channel();

        if let Some(entry) = plan.entries.first() {
            progress.current_item = Some(entry.display_path.clone());
            on_progress(progress.clone());
        }

        let archive_buf = archive.to_path_buf();
        let destination_buf = destination.to_path_buf();

        let diagnostics = diagnostics.cloned();
        let handle = std::thread::spawn(move || {
            let observer = diagnostics.map(|handle| {
                Arc::new(DecompressDiagnosticsObserver::new(handle))
                    as Arc<dyn decompress::Observer>
            });
            let mut builder = decompress::ExtractOptsBuilder::default().filter(move |path| {
                if cancel_filter.load(Ordering::Relaxed) {
                    return false;
                }

                let Some((index, _weight, conflict)) = entry_details.get(path) else {
                    return false;
                };
                let allowed = !conflict || conflict_choice == ConflictChoice::Replace;
                if allowed {
                    let _ = tx.send(*index);
                }
                allowed
            });
            if let Some(observer) = observer {
                builder = builder.observer(observer);
            }
            builder = builder.collect_output_paths(false);
            let opts = builder.build().map_err(|error| {
                FileOperationError::Failed(format!("Could not prepare extraction: {error}"))
            })?;

            decompress::decompress(&archive_buf, &destination_buf, &opts).map_err(|error| {
                FileOperationError::Failed(format_path_error(
                    "extract",
                    &archive_buf,
                    io::Error::other(error.to_string()),
                ))
            })?;

            Ok::<(), FileOperationError>(())
        });

        while let Ok(index) = rx.recv() {
            let entry = &plan.entries[index];
            progress.current_item = Some(entry.display_path.clone());
            progress.add_copied_bytes(entry.byte_weight);
            progress.completed_files = progress.completed_files.saturating_add(1);
            on_progress(progress.clone());
        }

        handle
            .join()
            .map_err(|_| FileOperationError::Failed("Extraction thread panicked".to_owned()))??;
    } else {
        if let Some(entry) = plan.entries.first() {
            progress.current_item = Some(entry.display_path.clone());
            on_progress(progress.clone());
        }

        let entry_details = plan
            .by_destination
            .iter()
            .map(|(destination, index)| (destination.clone(), plan.entries[*index].conflict))
            .collect::<HashMap<_, _>>();
        let cancel_filter = cancel.clone();
        let mut builder = decompress::ExtractOptsBuilder::default().filter(move |path| {
            if cancel_filter.load(Ordering::Relaxed) {
                return false;
            }
            entry_details
                .get(path)
                .is_some_and(|conflict| conflict_choice == ConflictChoice::Replace || !conflict)
        });
        if let Some(diagnostics) = diagnostics {
            builder = builder.observer(Arc::new(DecompressDiagnosticsObserver::new(
                diagnostics.clone(),
            )));
        }
        builder = builder.collect_output_paths(false);
        let opts = builder.build().map_err(|error| {
            FileOperationError::Failed(format!("Could not prepare extraction: {error}"))
        })?;

        decompress::decompress(archive, destination, &opts).map_err(|error| {
            FileOperationError::Failed(format_path_error(
                "extract",
                archive,
                io::Error::other(error.to_string()),
            ))
        })?;

        progress.completed_files = progress
            .completed_files
            .saturating_add(plan.entries.len().max(1));
        progress.add_copied_bytes(archive_entry_byte_total(&plan.entries));
        on_progress(progress.clone());
    }

    Ok(())
}

fn extract_7z_archive_with_entry_progress(
    archive: &Path,
    _destination: &Path,
    plan: &ArchiveExtractPlan,
    conflict_choice: ConflictChoice,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    diagnostics: Option<&ArchiveHandle>,
) -> Result<(), FileOperationError> {
    let file = File::open(archive).map_err(|error| operation_error("open", archive, error))?;
    if let Some(diagnostics) = diagnostics {
        extract_7z_archive_from_reader(
            archive,
            CountingReader::new(file, Some(diagnostics)),
            plan,
            conflict_choice,
            cancel,
            progress,
            on_progress,
            Some(diagnostics),
        )
    } else {
        extract_7z_archive_from_reader(
            archive,
            file,
            plan,
            conflict_choice,
            cancel,
            progress,
            on_progress,
            None,
        )
    }
}

fn extract_7z_archive_from_reader(
    archive: &Path,
    file: impl Read + std::io::Seek,
    plan: &ArchiveExtractPlan,
    conflict_choice: ConflictChoice,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    diagnostics: Option<&ArchiveHandle>,
) -> Result<(), FileOperationError> {
    let mut reader = sevenz_rust2::ArchiveReader::new(file, sevenz_rust2::Password::empty())
        .map_err(|error| {
            operation_error("extract", archive, io::Error::other(error.to_string()))
        })?;

    let mut prepared_parents = HashSet::new();
    reader
        .for_each_entries(|entry, reader| {
            if cancel.load(Ordering::Relaxed) {
                return Ok(false);
            }

            let display_path = sanitized_archive_entry_path(Path::new(entry.name()));
            let lookup_started = Instant::now();
            let Some(planned_entry) = plan.entry_by_display_path(&display_path) else {
                return Ok(true);
            };
            if let Some(diagnostics) = diagnostics {
                diagnostics.phase("lookup", lookup_started.elapsed());
            }

            progress.current_item = Some(planned_entry.display_path.clone());
            on_progress(progress.clone());

            if planned_entry.conflict && conflict_choice == ConflictChoice::Skip {
                progress.completed_files = progress.completed_files.saturating_add(1);
                on_progress(progress.clone());
                return Ok(true);
            }

            if let Some(parent) = planned_entry.destination.parent() {
                if prepared_parents.insert(parent.to_path_buf()) {
                    let directory_started = Instant::now();
                    fs::create_dir_all(parent).map_err(|error| {
                        sevenz_rust2::Error::Io(error, "Could not create directory".into())
                    })?;
                    if let Some(diagnostics) = diagnostics {
                        diagnostics
                            .metrics()
                            .directory_creates
                            .fetch_add(1, Ordering::Relaxed);
                        diagnostics.phase("directory_create", directory_started.elapsed());
                    }
                }
            }

            if !entry.is_directory() {
                let entry_started = Instant::now();
                let file_create_started = Instant::now();
                let mut output = File::create(&planned_entry.destination).map_err(|error| {
                    sevenz_rust2::Error::Io(error, "Could not create file".into())
                })?;
                if let Some(diagnostics) = diagnostics {
                    diagnostics
                        .metrics()
                        .file_creates
                        .fetch_add(1, Ordering::Relaxed);
                    diagnostics.phase("file_create", file_create_started.elapsed());
                }
                let bytes = io::copy(reader, &mut output).map_err(|error| {
                    sevenz_rust2::Error::Io(error, "Could not extract file".into())
                })?;
                if let Some(diagnostics) = diagnostics {
                    record_completed_entry(
                        diagnostics,
                        planned_entry,
                        bytes,
                        entry_started.elapsed(),
                        "ok",
                    );
                }
                progress.add_copied_bytes(planned_entry.byte_weight);
            }

            progress.completed_files = progress.completed_files.saturating_add(1);
            on_progress(progress.clone());

            Ok(true)
        })
        .map_err(|error| {
            operation_error("extract", archive, io::Error::other(error.to_string()))
        })?;

    if cancel.load(Ordering::Relaxed) {
        return Err(FileOperationError::Cancelled);
    }

    Ok(())
}

fn extract_ar_archive_with_entry_progress(
    archive: &Path,
    _destination: &Path,
    plan: &ArchiveExtractPlan,
    conflict_choice: ConflictChoice,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    diagnostics: Option<&ArchiveHandle>,
) -> Result<(), FileOperationError> {
    let file = File::open(archive).map_err(|error| operation_error("read", archive, error))?;
    if let Some(diagnostics) = diagnostics {
        extract_ar_archive_from_reader(
            archive,
            CountingReader::new(file, Some(diagnostics)),
            plan,
            conflict_choice,
            cancel,
            progress,
            on_progress,
            Some(diagnostics),
        )
    } else {
        extract_ar_archive_from_reader(
            archive,
            file,
            plan,
            conflict_choice,
            cancel,
            progress,
            on_progress,
            None,
        )
    }
}

fn extract_ar_archive_from_reader(
    archive: &Path,
    file: impl Read,
    plan: &ArchiveExtractPlan,
    conflict_choice: ConflictChoice,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    diagnostics: Option<&ArchiveHandle>,
) -> Result<(), FileOperationError> {
    let mut reader = ar::Archive::new(file);
    let mut prepared_parents = HashSet::new();

    while let Some(entry) = reader.next_entry() {
        if cancel.load(Ordering::Relaxed) {
            return Err(FileOperationError::Cancelled);
        }
        let mut archive_entry =
            entry.map_err(|error| operation_error("extract", archive, error))?;
        let entry_name = String::from_utf8_lossy(archive_entry.header().identifier());
        let display_path = sanitized_archive_entry_path(Path::new(entry_name.as_ref()));
        let lookup_started = Instant::now();
        let Some(planned_entry) = plan.entry_by_display_path(&display_path) else {
            continue;
        };
        if let Some(diagnostics) = diagnostics {
            diagnostics.phase("lookup", lookup_started.elapsed());
        }

        progress.current_item = Some(planned_entry.display_path.clone());
        on_progress(progress.clone());

        if planned_entry.conflict && conflict_choice == ConflictChoice::Skip {
            progress.completed_files = progress.completed_files.saturating_add(1);
            on_progress(progress.clone());
            continue;
        }

        if let Some(parent) = planned_entry.destination.parent() {
            if prepared_parents.insert(parent.to_path_buf()) {
                let directory_started = Instant::now();
                fs::create_dir_all(parent)
                    .map_err(|error| operation_error("create", parent, error))?;
                if let Some(diagnostics) = diagnostics {
                    diagnostics
                        .metrics()
                        .directory_creates
                        .fetch_add(1, Ordering::Relaxed);
                    diagnostics.phase("directory_create", directory_started.elapsed());
                }
            }
        }
        let file_create_started = Instant::now();
        let mut output = File::create(&planned_entry.destination)
            .map_err(|error| operation_error("extract", &planned_entry.destination, error))?;
        if let Some(diagnostics) = diagnostics {
            diagnostics
                .metrics()
                .file_creates
                .fetch_add(1, Ordering::Relaxed);
            diagnostics.phase("file_create", file_create_started.elapsed());
        }
        let entry_started = Instant::now();
        let bytes = io::copy(&mut archive_entry, &mut output)
            .map_err(|error| operation_error("extract", &planned_entry.destination, error))?;
        if let Some(diagnostics) = diagnostics {
            record_completed_entry(
                diagnostics,
                planned_entry,
                bytes,
                entry_started.elapsed(),
                "ok",
            );
        }
        progress.add_copied_bytes(planned_entry.byte_weight);
        progress.completed_files = progress.completed_files.saturating_add(1);
        on_progress(progress.clone());
    }

    Ok(())
}

fn extract_rar_archive_with_entry_progress(
    archive: &Path,
    destination: &Path,
    plan: &ArchiveExtractPlan,
    conflict_choice: ConflictChoice,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    diagnostics: Option<&ArchiveHandle>,
) -> Result<(), FileOperationError> {
    let temp_directory = temp_extract_directory_for(destination)
        .map_err(|error| operation_error("create", destination, error))?;
    fs::create_dir_all(&temp_directory)
        .map_err(|error| operation_error("create", &temp_directory, error))?;

    let result = extract_rar_archive_to_temp(
        archive,
        &temp_directory,
        plan,
        conflict_choice,
        cancel,
        progress,
        on_progress,
        diagnostics,
    );

    let cleanup_started = Instant::now();
    let cleanup = fs::remove_dir_all(&temp_directory);
    if let Some(diagnostics) = diagnostics {
        diagnostics.phase("rar_cleanup", cleanup_started.elapsed());
    }
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(error)) => Err(operation_error("remove", &temp_directory, error)),
        (Err(error), _) => Err(error),
    }
}

fn extract_rar_archive_to_temp(
    archive: &Path,
    temp_directory: &Path,
    plan: &ArchiveExtractPlan,
    conflict_choice: ConflictChoice,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    diagnostics: Option<&ArchiveHandle>,
) -> Result<(), FileOperationError> {
    let archive_file = archive.to_string_lossy().to_string();
    let temp_destination = temp_directory.to_string_lossy().to_string();
    let mut archive_reader = unrar::Archive::new(archive_file)
        .extract_to(temp_destination)
        .map_err(|error| {
            FileOperationError::Failed(format!(
                "Could not extract {}: {error}",
                path_display_name(archive)
            ))
        })?;
    let mut index = 0;

    loop {
        let entry_started = Instant::now();
        let result = archive_reader.next();
        if let Some(diagnostics) = diagnostics {
            diagnostics.phase("rar_temp_extract", entry_started.elapsed());
        }
        let Some(result) = result else {
            break;
        };

        if cancel.load(Ordering::Relaxed) {
            return Err(FileOperationError::Cancelled);
        }
        if let Some(entry) = plan.entries.get(index) {
            progress.current_item = Some(entry.display_path.clone());
            on_progress(progress.clone());
        }

        let rar_entry = result.map_err(|error| {
            FileOperationError::Failed(format!(
                "Could not extract {}: {error}",
                path_display_name(archive)
            ))
        })?;
        let display_path = sanitized_archive_entry_path(Path::new(&rar_entry.filename));
        if !archive_sanitized_entry_should_extract(&display_path) {
            let output = temp_directory.join(&display_path);
            remove_temp_extract_output(&output)
                .map_err(|error| operation_error("remove", &output, error))?;
            continue;
        }
        let planned_entry = plan
            .entry_by_display_path(&display_path)
            .or_else(|| plan.entries.get(index));
        index += 1;

        let Some(planned_entry) = planned_entry else {
            continue;
        };

        progress.current_item = Some(planned_entry.display_path.clone());
        if planned_entry.conflict && conflict_choice == ConflictChoice::Skip {
            remove_temp_extract_output(&temp_directory.join(&planned_entry.display_path))
                .map_err(|error| operation_error("remove", &planned_entry.destination, error))?;
            progress.completed_files = progress.completed_files.saturating_add(1);
            on_progress(progress.clone());
            continue;
        }

        let merge_started = Instant::now();
        merge_temp_extract_output(
            &temp_directory.join(&planned_entry.display_path),
            &planned_entry.destination,
            rar_entry.is_directory(),
        )?;
        if let Some(diagnostics) = diagnostics {
            let size = fs::metadata(&planned_entry.destination)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            record_completed_entry(
                diagnostics,
                planned_entry,
                size,
                entry_started.elapsed(),
                "ok",
            );
            diagnostics.phase("rar_merge", merge_started.elapsed());
        }
        progress.add_copied_bytes(planned_entry.byte_weight);
        progress.completed_files = progress.completed_files.saturating_add(1);
        on_progress(progress.clone());
    }

    Ok(())
}

fn archive_listing(archive: &Path) -> Result<decompress::Listing, String> {
    if archive_is_7z(archive) {
        let file = File::open(archive)
            .map_err(|error| format!("Could not open {}: {error}", path_display_name(archive)))?;
        let mut reader = sevenz_rust2::ArchiveReader::new(file, sevenz_rust2::Password::empty())
            .map_err(|error| {
                format!(
                    "Could not read 7z archive {}: {error}",
                    path_display_name(archive)
                )
            })?;
        let mut entries = Vec::new();
        reader
            .for_each_entries(|entry, _| {
                entries.push(PathBuf::from(entry.name()));
                Ok(true)
            })
            .map_err(|error| {
                format!(
                    "Could not list 7z archive {}: {error}",
                    path_display_name(archive)
                )
            })?;
        Ok(decompress::Listing { id: "7z", entries })
    } else {
        let opts = default_extract_opts()?;
        decompress::list(archive, &opts)
            .map_err(|error| format!("Could not list {}: {error}", path_display_name(archive)))
    }
}

fn archive_is_7z(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".7z") && name.len() > ".7z".len()
        })
        .unwrap_or(false)
}

fn default_extract_opts() -> Result<decompress::ExtractOpts, String> {
    decompress::ExtractOptsBuilder::default()
        .build()
        .map_err(|error| format!("Could not prepare extraction: {error}"))
}

fn sanitized_entries_from_listing(entries: &[PathBuf]) -> Vec<PathBuf> {
    let mut sanitized_entries = Vec::with_capacity(entries.len());
    let mut seen = HashSet::with_capacity(entries.len());
    for entry in entries {
        let relative = sanitized_archive_entry_path(entry);
        if archive_sanitized_entry_should_extract(&relative) && seen.insert(relative.clone()) {
            sanitized_entries.push(relative);
        }
    }
    sanitized_entries
}

fn top_level_entries_from_sanitized(entries: &[PathBuf]) -> Vec<PathBuf> {
    let mut top_level_entries = Vec::with_capacity(entries.len().min(16));
    let mut seen = HashSet::new();
    for entry in entries {
        let Some(top_level) = top_level_archive_component_from_sanitized(entry) else {
            continue;
        };
        if seen.insert(top_level.clone()) {
            top_level_entries.push(top_level);
        }
    }

    top_level_entries
}

#[cfg(test)]
fn planned_output_paths_from_listing(entries: &[String], destination: &Path) -> Vec<PathBuf> {
    let entries = entries.iter().map(PathBuf::from).collect::<Vec<_>>();
    planned_extract_entries_from_sanitized(&sanitized_entries_from_listing(&entries), destination)
        .into_iter()
        .map(|entry| entry.destination)
        .collect()
}

#[cfg(test)]
fn top_level_entries_from_listing(entries: &[String]) -> Vec<PathBuf> {
    let entries = entries.iter().map(PathBuf::from).collect::<Vec<_>>();
    top_level_entries_from_sanitized(&sanitized_entries_from_listing(&entries))
}

fn planned_extract_entries_from_sanitized(
    entries: &[PathBuf],
    destination: &Path,
) -> Vec<ArchiveExtractEntry> {
    entries
        .iter()
        .map(|relative| ArchiveExtractEntry {
            display_path: relative.clone(),
            destination: destination.join(relative),
            conflict: false,
            byte_weight: 0,
        })
        .collect()
}

fn assign_archive_entry_byte_weights(entries: &mut [ArchiveExtractEntry], archive_size: u64) {
    let entry_count = entries.len() as u64;
    if entry_count == 0 {
        return;
    }

    let base_weight = archive_size / entry_count;
    let mut remainder = archive_size % entry_count;
    for entry in entries {
        entry.byte_weight = base_weight + u64::from(remainder > 0);
        remainder = remainder.saturating_sub(1);
    }
}

fn archive_entry_byte_total(entries: &[ArchiveExtractEntry]) -> u64 {
    entries
        .iter()
        .map(|entry| entry.byte_weight)
        .fold(0_u64, u64::saturating_add)
}

fn archive_extract_destination(
    archive: &Path,
    destination: &Path,
    top_level_entries: &[PathBuf],
) -> Result<PathBuf, String> {
    if top_level_entries.len() == 1 {
        return Ok(destination.to_path_buf());
    }

    let name = archive_extract_root_name(archive)?;
    Ok(destination.join(name))
}

fn archive_output_roots(destination: &Path, top_level_entries: &[PathBuf]) -> Vec<PathBuf> {
    if top_level_entries.len() > 1 {
        return vec![destination.to_path_buf()];
    }

    top_level_entries
        .iter()
        .map(|entry| destination.join(entry))
        .collect()
}

fn archive_extract_root_name(archive: &Path) -> Result<OsString, String> {
    let file_name = archive
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| format!("{} cannot be extracted.", path_display_name(archive)))?;
    let lower = file_name.to_ascii_lowercase();

    for extension in COMPOUND_ARCHIVE_EXTENSIONS {
        let suffix = format!(".{extension}");
        if lower.ends_with(&suffix) && file_name.len() > suffix.len() {
            return Ok(OsString::from(&file_name[..file_name.len() - suffix.len()]));
        }
    }

    for extension in SIMPLE_ARCHIVE_EXTENSIONS {
        let suffix = format!(".{extension}");
        if lower.ends_with(&suffix) && file_name.len() > suffix.len() {
            return Ok(OsString::from(&file_name[..file_name.len() - suffix.len()]));
        }
    }

    archive
        .file_stem()
        .map(OsStr::to_os_string)
        .ok_or_else(|| format!("{} cannot be extracted.", path_display_name(archive)))
}

fn archive_name_has_supported_extension(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    COMPOUND_ARCHIVE_EXTENSIONS
        .iter()
        .chain(SIMPLE_ARCHIVE_EXTENSIONS.iter())
        .any(|extension| {
            let suffix = format!(".{extension}");
            lower.ends_with(&suffix) && file_name.len() > suffix.len()
        })
}

fn archive_is_single_file_compression(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            ["gz", "bz", "bz2", "xz", "zst"].iter().any(|extension| {
                lower.ends_with(&format!(".{extension}"))
                    && !COMPOUND_ARCHIVE_EXTENSIONS
                        .iter()
                        .any(|compound| lower.ends_with(&format!(".{compound}")))
            })
        })
        .unwrap_or(false)
}

fn archive_supports_filtered_extract(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            [
                "zip", "tar", "tar.gz", "tgz", "tar.bz2", "tbz", "tar.xz", "txz", "tar.zst", "tzst",
            ]
            .iter()
            .any(|extension| lower.ends_with(&format!(".{extension}")))
        })
        .unwrap_or(false)
}

fn archive_is_ar(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".ar") && name.len() > ".ar".len()
        })
        .unwrap_or(false)
}

fn archive_is_rar(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".rar") && name.len() > ".rar".len()
        })
        .unwrap_or(false)
}

fn top_level_archive_component_from_sanitized(path: &Path) -> Option<PathBuf> {
    path.components()
        .next()
        .and_then(|component| match component {
            Component::Normal(name) => Some(PathBuf::from(name)),
            _ => None,
        })
}

fn archive_sanitized_entry_should_extract(path: &Path) -> bool {
    path.components().next().is_some_and(|component| {
        !matches!(
            component,
            Component::Normal(name) if name == OsStr::new(MACOSX_ARCHIVE_METADATA_DIRECTORY)
        )
    })
}

fn sanitized_archive_entry_path(path: &Path) -> PathBuf {
    let mut sanitized = PathBuf::new();
    for component in path.components() {
        if let Component::Normal(name) = component {
            sanitized.push(name);
        }
    }
    sanitized
}

fn move_source_file_with_progress(
    source: &Path,
    destination: &Path,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    copy_engine: CopyEngine,
    copy_options: CopyOptions,
) -> std::io::Result<()> {
    match fs::rename(source, destination) {
        Ok(()) => {
            let file_size = fs::metadata(destination)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            progress.add_copied_bytes(file_size);
            progress.completed_files += 1;
            on_progress(progress.clone());
            Ok(())
        }
        Err(error) if is_cross_device_error(&error) => {
            progress.phase = FileOperationPhase::Copying;
            on_progress(progress.clone());
            copy_source_file_with_progress(
                source,
                destination,
                cancel,
                progress,
                on_progress,
                copy_engine,
                copy_options,
            )?;
            remove_source(source)
        }
        Err(error) => Err(error),
    }
}

fn copy_source_file_with_progress(
    source: &Path,
    destination: &Path,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    engine: CopyEngine,
    options: CopyOptions,
) -> std::io::Result<()> {
    match engine {
        CopyEngine::Standard => {
            copy_source_file_standard_with_progress(
                source,
                destination,
                cancel,
                progress,
                on_progress,
                options,
            )?;
        }
        CopyEngine::ResumableDelta => {
            copy_with_delta_progress_with_options(
                source,
                destination,
                cancel,
                progress,
                on_progress,
                options,
            )?;
        }
    }

    progress.completed_files += 1;
    on_progress(progress.clone());
    Ok(())
}

fn copy_source_file_standard_with_progress(
    source: &Path,
    destination: &Path,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    options: CopyOptions,
) -> std::io::Result<()> {
    let metadata = fs::metadata(source)?;
    let verified_before_existing_check = progress.verified_bytes;
    if destination.is_file()
        && destination_quick_matches_source(&metadata, destination).unwrap_or(false)
        && destination_content_matches_source_with_progress(
            source,
            destination,
            metadata.len(),
            COPY_BUFFER_SIZE,
            cancel,
            progress,
            on_progress,
        )?
    {
        preserve_file_metadata(&metadata, destination)?;
        if options.should_sync() {
            sync_file_best_effort(destination);
            sync_parent_directory_best_effort(destination);
        }
        return Ok(());
    }
    let existing_check_verified = progress
        .verified_bytes
        .saturating_sub(verified_before_existing_check);
    if existing_check_verified > 0 {
        progress.reserve_work_bytes(existing_check_verified);
    }

    progress.phase = FileOperationPhase::Copying;
    progress.current_item = Some(source.to_path_buf());
    on_progress(progress.clone());

    let temp_destination = temp_destination_for(destination)?;
    let copy_result = copy_source_file_to_temp(
        source,
        &temp_destination,
        metadata.len(),
        cancel,
        progress,
        on_progress,
        options,
    );

    match copy_result {
        Ok(()) => {
            preserve_file_metadata(&metadata, &temp_destination)?;
            replace_destination_with_temp(&temp_destination, destination)?;
            if options.should_sync() {
                sync_parent_directory_best_effort(destination);
            }
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temp_destination);
            Err(error)
        }
    }
}

fn copy_source_file_to_temp(
    source: &Path,
    temp_destination: &Path,
    source_len: u64,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    options: CopyOptions,
) -> std::io::Result<()> {
    let mut source_file = File::open(source)?;
    let destination_file = File::create(temp_destination)?;
    if source_len >= COPY_BUFFER_SIZE as u64 * 8 {
        copy_file_contents_parallel_with_progress(
            source,
            &destination_file,
            source_len,
            cancel,
            |bytes| {
                progress.add_copied_bytes(bytes);
                on_progress(progress.clone());
            },
        )?;
        if options.should_sync() {
            destination_file.sync_all()?;
        }
        return Ok(());
    }

    let mut destination_file = destination_file;
    let mut buffer = vec![0; COPY_BUFFER_SIZE];

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "file operation cancelled",
            ));
        }

        let read = source_file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        destination_file.write_all(&buffer[..read])?;
        progress.add_copied_bytes(read as u64);
        on_progress(progress.clone());
    }

    if options.should_sync() {
        destination_file.sync_all()?;
    }
    Ok(())
}

pub(super) fn preserve_file_metadata(
    metadata: &fs::Metadata,
    destination: &Path,
) -> std::io::Result<()> {
    fs::set_permissions(destination, metadata.permissions())?;
    let accessed = FileTime::from_last_access_time(metadata);
    let modified = FileTime::from_last_modification_time(metadata);
    filetime::set_file_times(destination, accessed, modified)
}

fn sync_file_best_effort(path: &Path) {
    if let Ok(file) = File::open(path) {
        let _ = file.sync_all();
    }
}

#[cfg(unix)]
fn sync_parent_directory_best_effort(path: &Path) {
    if let Some(parent) = path.parent()
        && let Ok(directory) = File::open(parent)
    {
        let _ = directory.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_directory_best_effort(_: &Path) {}

fn temp_destination_for(destination: &Path) -> std::io::Result<PathBuf> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .unwrap_or_else(|| OsStr::new("file"))
        .to_string_lossy();
    let process_id = std::process::id();

    loop {
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".explorer-copy-{process_id}-{counter}-{file_name}.tmp"
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
}

fn temp_extract_directory_for(destination: &Path) -> std::io::Result<PathBuf> {
    let parent = if destination.is_dir() {
        destination
    } else {
        destination.parent().unwrap_or_else(|| Path::new("."))
    };
    let process_id = std::process::id();

    loop {
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".explorer-extract-{process_id}-{counter}.tmp"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
}

fn merge_temp_extract_output(
    source: &Path,
    destination: &Path,
    is_directory: bool,
) -> Result<(), FileOperationError> {
    if is_directory || source.is_dir() {
        fs::create_dir_all(destination)
            .map_err(|error| operation_error("create", destination, error))?;
        remove_temp_extract_output(source)
            .map_err(|error| operation_error("remove", source, error))?;
        return Ok(());
    }

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| operation_error("create", parent, error))?;
    }

    if destination.is_dir() {
        return Err(FileOperationError::Failed(format!(
            "{} already exists and is a folder.",
            path_display_name(destination)
        )));
    }

    replace_destination_with_temp(source, destination)
        .map_err(|error| operation_error("extract", destination, error))
}

fn remove_temp_extract_output(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else if path.exists() {
        fs::remove_file(path)
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
pub(super) fn replace_destination_with_temp(
    temp: &Path,
    destination: &Path,
) -> std::io::Result<()> {
    fs::rename(temp, destination)
}

#[cfg(target_os = "windows")]
pub(super) fn replace_destination_with_temp(
    temp: &Path,
    destination: &Path,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::core::PCWSTR;

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let temp = wide(temp);
    let destination = wide(destination);
    unsafe {
        MoveFileExW(
            PCWSTR(temp.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|error| std::io::Error::other(error.to_string()))
    }
}

fn remove_empty_directory(path: &Path) -> std::io::Result<()> {
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn remove_source(source: &Path) -> std::io::Result<()> {
    if source.is_dir() {
        fs::remove_dir_all(source)
    } else {
        fs::remove_file(source)
    }
}

fn paste_copy_destination(
    destination: &Path,
    file_name: &OsStr,
    reserved_destinations: &mut HashSet<PathBuf>,
) -> PathBuf {
    let mut copy_number = 1;

    loop {
        let candidate = destination.join(copy_file_name(file_name, copy_number));
        if !candidate.exists() && reserved_destinations.insert(candidate.clone()) {
            return candidate;
        }
        copy_number += 1;
    }
}

fn copy_file_name(file_name: &OsStr, copy_number: usize) -> OsString {
    let file_name = file_name.to_string_lossy();
    let path = Path::new(file_name.as_ref());
    let stem = path
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or(file_name.as_ref());
    let extension = path.extension().and_then(OsStr::to_str);
    let suffix = if copy_number == 1 {
        " - Copy".to_owned()
    } else {
        format!(" - Copy ({copy_number})")
    };

    match extension {
        Some(extension) => OsString::from(format!("{stem}{suffix}.{extension}")),
        None => OsString::from(format!("{stem}{suffix}")),
    }
}

fn copy_engine_for_paths(_source: &Path, _destination: &Path, _conflict: bool) -> CopyEngine {
    CopyEngine::ResumableDelta
}

pub(super) fn paths_are_on_same_volume(source: &Path, destination: &Path) -> bool {
    match path_volume_relation(source, destination) {
        Some(same_volume) => same_volume,
        None => true,
    }
}

fn path_volume_relation(source: &Path, destination: &Path) -> Option<bool> {
    Some(path_volume_key(source)? == path_volume_key(destination)?)
}

#[cfg(test)]
static TEST_VOLUME_KEYS: std::sync::Mutex<Vec<(PathBuf, Option<String>)>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(test)]
pub(super) struct TestVolumeKeyGuard {
    path: PathBuf,
}

#[cfg(test)]
impl Drop for TestVolumeKeyGuard {
    fn drop(&mut self) {
        TEST_VOLUME_KEYS
            .lock()
            .expect("test volume keys")
            .retain(|(path, _)| path != &self.path);
    }
}

#[cfg(test)]
pub(super) fn set_test_path_volume_key(path: &Path, key: Option<&str>) -> TestVolumeKeyGuard {
    let path = path.to_path_buf();
    TEST_VOLUME_KEYS
        .lock()
        .expect("test volume keys")
        .push((path.clone(), key.map(str::to_owned)));
    TestVolumeKeyGuard { path }
}

#[cfg(test)]
fn test_path_volume_key(path: &Path) -> Option<Option<String>> {
    TEST_VOLUME_KEYS
        .lock()
        .expect("test volume keys")
        .iter()
        .rev()
        .find(|(prefix, _)| path.starts_with(prefix))
        .map(|(_, key)| key.clone())
}

#[cfg(windows)]
fn path_volume_key(path: &Path) -> Option<String> {
    #[cfg(test)]
    if let Some(key) = test_path_volume_key(path) {
        return key;
    }

    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let Component::Prefix(prefix) = path.components().next()? else {
        return None;
    };

    Some(match prefix.kind() {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
            char::from(letter).to_ascii_uppercase().to_string()
        }
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            format!(
                r"\\{}\{}",
                server.to_string_lossy().to_ascii_lowercase(),
                share.to_string_lossy().to_ascii_lowercase()
            )
        }
        _ => prefix.as_os_str().to_string_lossy().to_ascii_lowercase(),
    })
}

#[cfg(unix)]
fn path_volume_key(path: &Path) -> Option<String> {
    #[cfg(test)]
    if let Some(key) = test_path_volume_key(path) {
        return key;
    }

    let path = existing_volume_probe_path(path)?;
    let metadata = fs::metadata(path).ok()?;
    Some(metadata.dev().to_string())
}

#[cfg(not(any(windows, unix)))]
fn path_volume_key(path: &Path) -> Option<String> {
    #[cfg(test)]
    if let Some(key) = test_path_volume_key(path) {
        return key;
    }
    None
}

#[cfg(unix)]
fn existing_volume_probe_path(path: &Path) -> Option<PathBuf> {
    if let Ok(canonical) = fs::canonicalize(path) {
        return Some(canonical);
    }

    let mut current = Some(path);
    while let Some(path) = current {
        if path.exists() {
            return Some(path.to_path_buf());
        }
        current = path.parent();
    }

    None
}

fn canonicalize_for_operation(path: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|error| format_path_error("read", path, error))
}

fn is_cross_device_error(error: &std::io::Error) -> bool {
    matches!(error.kind(), std::io::ErrorKind::CrossesDevices)
}

fn operation_error(operation: &str, path: &Path, error: std::io::Error) -> FileOperationError {
    if error.kind() == std::io::ErrorKind::Interrupted && error.to_string().contains("cancelled") {
        FileOperationError::Cancelled
    } else {
        FileOperationError::Failed(format_path_error(operation, path, error))
    }
}

fn format_path_error(operation: &str, path: &Path, error: std::io::Error) -> String {
    format!("Could not {operation} {}: {error}", path_display_name(path))
}

fn path_display_name(path: &Path) -> String {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;
    #[cfg(target_os = "windows")]
    use windows::{
        Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES,
            SetFileAttributesW,
        },
        core::PCWSTR,
    };

    #[cfg(target_os = "windows")]
    fn set_windows_file_attributes(path: &Path, attributes: FILE_FLAGS_AND_ATTRIBUTES) {
        use std::os::windows::ffi::OsStrExt;

        let mut wide_path = path.as_os_str().encode_wide().collect::<Vec<_>>();
        wide_path.push(0);
        unsafe {
            SetFileAttributesW(PCWSTR(wide_path.as_ptr()), attributes)
                .expect("set windows file attributes");
        }
    }

    fn create_ar_archive(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).expect("create ar archive");
        let mut builder = ar::Builder::new(file);
        for (name, data) in entries {
            let header = ar::Header::new(name.as_bytes().to_vec(), data.len() as u64);
            let mut reader = *data;
            builder
                .append(&header, &mut reader)
                .expect("append ar entry");
        }
        builder.into_inner().expect("finish ar archive");
    }

    fn sorted_entry_names(entries: &[FileEntry]) -> Vec<&str> {
        let mut names = entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        names.sort_unstable();
        names
    }

    #[test]
    fn mountable_image_extensions_match_platform_support() {
        assert!(mountable_image_extension_is_supported("iso"));
        assert!(mountable_image_extension_is_supported("ISO"));
        assert!(mountable_image_extension_is_supported("img"));
        assert!(mountable_image_extension_is_supported("IMG"));
        assert_eq!(
            mountable_image_extension_is_supported("dmg"),
            cfg!(target_os = "macos")
        );
        assert_eq!(
            mountable_image_extension_is_supported("vhd"),
            cfg!(target_os = "windows")
        );
        assert_eq!(
            mountable_image_extension_is_supported("vhdx"),
            cfg!(target_os = "windows")
        );
        assert!(!mountable_image_extension_is_supported("zip"));
    }

    #[cfg(target_os = "macos")]
    fn sorted_entry_paths(entries: &[FileEntry]) -> Vec<PathBuf> {
        let mut paths = entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }

    #[test]
    fn path_same_or_descendant_uses_component_boundaries() {
        let root = temp_like_absolute_path("drive");
        let child = root.join("folder");
        let sibling_prefix = temp_like_absolute_path("drive-other");

        assert!(path_is_same_or_descendant(&root, &root));
        assert!(path_is_same_or_descendant(&child, &root));
        assert!(!path_is_same_or_descendant(&sibling_prefix, &root));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn path_same_or_descendant_is_case_insensitive_on_windows() {
        assert!(path_is_same_or_descendant(
            Path::new(r"D:\Folder\Child"),
            Path::new(r"d:\folder")
        ));
    }

    fn temp_like_absolute_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(name);
        path
    }

    #[test]
    fn archive_extract_root_name_strips_simple_and_compound_extensions() {
        assert_eq!(
            archive_extract_root_name(Path::new("package.zip")).unwrap(),
            OsString::from("package")
        );
        assert_eq!(
            archive_extract_root_name(Path::new("package.tar.gz")).unwrap(),
            OsString::from("package")
        );
        assert_eq!(
            archive_extract_root_name(Path::new("package.tar.zst")).unwrap(),
            OsString::from("package")
        );
        assert_eq!(
            archive_extract_root_name(Path::new("package.rar")).unwrap(),
            OsString::from("package")
        );
    }

    #[test]
    fn archive_extract_backend_labels_pipeline_implementations() {
        assert_eq!(archive_extract_backend(Path::new("archive.rar")), "rar");
        assert_eq!(archive_extract_backend(Path::new("archive.7z")), "7z");
        assert_eq!(archive_extract_backend(Path::new("archive.ar")), "ar");
        assert_eq!(
            archive_extract_backend(Path::new("archive.zip")),
            "decompress-filtered"
        );
        assert_eq!(
            archive_extract_backend(Path::new("archive.bz")),
            "decompress-filtered"
        );
        assert_eq!(
            archive_extract_backend(Path::new("archive.unknown")),
            "decompress"
        );
    }

    #[test]
    fn top_level_entries_from_listing_counts_unique_roots() {
        let entries = vec![
            "file.txt".to_owned(),
            "folder/a.txt".to_owned(),
            "folder/nested/b.txt".to_owned(),
            "__MACOSX/._file.txt".to_owned(),
            "../ignored.txt".to_owned(),
            "/rooted.txt".to_owned(),
        ];

        assert_eq!(
            top_level_entries_from_listing(&entries),
            vec![
                PathBuf::from("file.txt"),
                PathBuf::from("folder"),
                PathBuf::from("ignored.txt"),
                PathBuf::from("rooted.txt"),
            ]
        );
    }

    #[test]
    fn archive_destination_uses_current_directory_for_single_root() {
        let destination = Path::new("downloads");
        let top_level_entries = vec![PathBuf::from("folder")];

        assert_eq!(
            archive_extract_destination(
                Path::new("downloads/archive.zip"),
                destination,
                &top_level_entries,
            )
            .unwrap(),
            PathBuf::from("downloads")
        );
        assert_eq!(
            archive_output_roots(destination, &top_level_entries),
            vec![PathBuf::from("downloads/folder")]
        );
    }

    #[test]
    fn archive_destination_uses_archive_named_folder_for_multiple_roots() {
        let destination = Path::new("downloads");
        let top_level_entries = vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")];

        let extract_to = archive_extract_destination(
            Path::new("downloads/archive.tar.gz"),
            destination,
            &top_level_entries,
        )
        .unwrap();

        assert_eq!(extract_to, PathBuf::from("downloads/archive"));
        assert_eq!(
            archive_output_roots(&extract_to, &top_level_entries),
            vec![PathBuf::from("downloads/archive")]
        );
    }

    #[test]
    fn planned_output_paths_sanitize_and_dedupe_listing_entries() {
        let entries = vec![
            "folder/a.txt".to_owned(),
            "folder/a.txt".to_owned(),
            "./folder/b.txt".to_owned(),
            "__MACOSX/._a.txt".to_owned(),
            "../outside.txt".to_owned(),
        ];

        assert_eq!(
            planned_output_paths_from_listing(&entries, Path::new("dest")),
            vec![
                PathBuf::from("dest/folder/a.txt"),
                PathBuf::from("dest/folder/b.txt"),
                PathBuf::from("dest/outside.txt"),
            ]
        );
    }

    #[test]
    fn archive_planning_skips_top_level_macosx_metadata_directory() {
        let entries = vec![
            "__MACOSX/._a.txt".to_owned(),
            "__MACOSX/nested/._b.txt".to_owned(),
            "folder/__MACOSX/kept.txt".to_owned(),
            "folder/file.txt".to_owned(),
        ];

        assert_eq!(
            top_level_entries_from_listing(&entries),
            vec![PathBuf::from("folder")]
        );
        assert_eq!(
            planned_output_paths_from_listing(&entries, Path::new("dest")),
            vec![
                PathBuf::from("dest/folder/__MACOSX/kept.txt"),
                PathBuf::from("dest/folder/file.txt"),
            ]
        );
    }

    #[test]
    fn archive_planning_treats_macosx_metadata_only_listing_as_empty() {
        let entries = vec![
            "__MACOSX".to_owned(),
            "__MACOSX/".to_owned(),
            "__MACOSX/._file.txt".to_owned(),
        ];

        assert!(top_level_entries_from_listing(&entries).is_empty());
        assert!(planned_output_paths_from_listing(&entries, Path::new("dest")).is_empty());
    }

    #[test]
    fn archive_extract_plan_indexes_entries_by_sanitized_display_path() {
        let entry = ArchiveExtractEntry {
            display_path: PathBuf::from("folder/file.txt"),
            destination: PathBuf::from("dest/folder/file.txt"),
            conflict: false,
            byte_weight: 10,
        };
        let plan = ArchiveExtractPlan::new(vec![entry.clone()]);
        assert_eq!(
            plan.entry_by_display_path(Path::new("folder/file.txt")),
            Some(&entry)
        );
        assert_eq!(
            plan.by_destination.get(Path::new("dest/folder/file.txt")),
            Some(&0)
        );
    }

    #[test]
    fn archive_planning_deduplicates_sanitized_paths() {
        let entries = vec![
            PathBuf::from("folder/file.txt"),
            PathBuf::from("./folder/file.txt"),
            PathBuf::from("../folder/file.txt"),
        ];
        let sanitized = sanitized_entries_from_listing(&entries);
        assert_eq!(sanitized, vec![PathBuf::from("folder/file.txt")]);
    }

    #[test]
    fn extract_progress_reports_archive_bytes_per_entry() {
        let temp = TempDir::new();
        let archive = temp.path().join("archive.ar");
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).expect("create destination");
        create_ar_archive(&archive, &[("a.txt", b"one"), ("b.txt", b"two")]);
        let archive_size = fs::metadata(&archive).expect("archive metadata").len();
        let job = ready_job(prepare_extract_archives_to_directory(
            std::slice::from_ref(&archive),
            &destination,
        ));
        let mut progress_events = Vec::new();

        let summary = execute_file_operation_with_progress(
            job,
            ConflictChoice::Replace,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            |progress| progress_events.push(progress),
        )
        .expect("extract with progress");

        let extracted_root = destination.join("archive");
        assert_eq!(fs::read(extracted_root.join("a.txt")).unwrap(), b"one");
        assert_eq!(fs::read(extracted_root.join("b.txt")).unwrap(), b"two");
        assert_eq!(summary.destination_paths, vec![extracted_root]);
        assert!(progress_events.iter().any(|progress| {
            progress.phase == FileOperationPhase::Extracting && progress.copied_bytes > 0
        }));
        assert_eq!(
            progress_events.last().map(|progress| progress.copied_bytes),
            Some(archive_size)
        );
    }

    #[test]
    fn archive_extract_progress_publication_is_throttled_and_forces_completion() {
        let temp = TempDir::new();
        let archive = temp.path().join("archive.ar");
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).expect("create destination");
        let entries = (0..100)
            .map(|index| (format!("f{index:03}.txt"), vec![b'x'; 16]))
            .collect::<Vec<_>>();
        let refs = entries
            .iter()
            .map(|(name, data)| (name.as_str(), data.as_slice()))
            .collect::<Vec<_>>();
        create_ar_archive(&archive, &refs);
        let job = ready_job(prepare_extract_archives_to_directory(
            std::slice::from_ref(&archive),
            &destination,
        ));
        let mut updates = Vec::new();
        execute_file_operation_with_progress(
            job,
            ConflictChoice::Replace,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            |progress| updates.push(progress),
        )
        .expect("extract with progress");

        assert!(updates.len() < 20);
        let final_progress = updates.last().expect("final progress");
        assert_eq!(final_progress.completed_files, 100);
        assert_eq!(final_progress.phase, FileOperationPhase::Finished);
    }

    #[test]
    fn extract_progress_skip_conflict_advances_items_without_skipped_bytes() {
        let temp = TempDir::new();
        let archive = temp.path().join("archive.ar");
        let destination = temp.path().join("destination");
        let extracted_root = destination.join("archive");
        fs::create_dir(&destination).expect("create destination");
        fs::create_dir(&extracted_root).expect("create extracted root");
        fs::write(extracted_root.join("a.txt"), b"existing").expect("create conflict");
        create_ar_archive(&archive, &[("a.txt", b"one"), ("b.txt", b"two")]);
        let conflicts = prepared_conflict_batch(prepare_extract_archives_to_directory(
            std::slice::from_ref(&archive),
            &destination,
        ));
        let mut progress_events = Vec::new();

        execute_file_operation_with_progress(
            conflicts.into_job(),
            ConflictChoice::Skip,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            |progress| progress_events.push(progress),
        )
        .expect("extract with skipped conflict");

        assert_eq!(fs::read(extracted_root.join("a.txt")).unwrap(), b"existing");
        assert_eq!(fs::read(extracted_root.join("b.txt")).unwrap(), b"two");
        assert!(progress_events.iter().any(|progress| {
            progress.phase == FileOperationPhase::Extracting
                && progress.completed_files == 1
                && progress.copied_bytes == 0
        }));
        assert!(progress_events.iter().any(|progress| {
            progress.phase == FileOperationPhase::Extracting && progress.copied_bytes > 0
        }));
    }

    #[test]
    fn default_start_path_prefers_existing_downloads_directory() {
        let temp = TempDir::new();
        let home = temp.path().join("home");
        let downloads = home.join("Downloads");
        let current = temp.path().join("current");
        fs::create_dir_all(&downloads).expect("create downloads");
        fs::create_dir(&current).expect("create current directory");

        let start_path = preferred_start_path(
            Some(downloads.clone()),
            Some(home.clone()),
            Some(current.clone()),
        );

        assert_eq!(start_path, downloads);
    }

    #[test]
    fn default_start_path_falls_back_to_home_when_downloads_is_missing() {
        let temp = TempDir::new();
        let home = temp.path().join("home");
        let downloads = home.join("Downloads");
        let current = temp.path().join("current");
        fs::create_dir(&home).expect("create home");
        fs::create_dir(&current).expect("create current directory");

        let start_path =
            preferred_start_path(Some(downloads), Some(home.clone()), Some(current.clone()));

        assert_eq!(start_path, home);
    }

    #[test]
    fn default_start_path_falls_back_to_current_directory_or_dot_without_home() {
        let temp = TempDir::new();
        let current = temp.path().join("current");
        fs::create_dir(&current).expect("create current directory");

        assert_eq!(
            preferred_start_path(None, None, Some(current.clone())),
            current
        );
        assert_eq!(preferred_start_path(None, None, None), PathBuf::from("."));
    }

    #[test]
    fn macos_volume_drive_roots_include_volumes_after_filesystem_root() {
        let temp = TempDir::new();
        let volumes = temp.path().join("Volumes");
        let archive = volumes.join("Archive Disk");
        let backup = volumes.join("Backup");
        fs::create_dir_all(&archive).expect("create archive volume");
        fs::create_dir_all(&backup).expect("create backup volume");
        fs::write(volumes.join("not-a-volume"), "").expect("create file");

        assert_eq!(
            macos_volume_drive_roots_from_dir(&volumes),
            vec![PathBuf::from("/"), archive, backup]
        );
    }

    #[test]
    fn linux_mountinfo_drive_roots_include_visible_user_mounts() {
        let mountinfo = "\
36 25 8:1 / / rw,relatime - ext4 /dev/sda1 rw
40 25 8:17 / /media/alex/USB\\040Disk rw,relatime - vfat /dev/sdb1 rw
41 25 8:33 / /run/media/alex/Camera rw,relatime - vfat /dev/sdc1 rw
42 25 8:49 / /mnt/projects rw,relatime - ext4 /dev/sdd1 rw
43 25 0:4 / /proc rw,nosuid,nodev,noexec,relatime - proc proc rw
44 25 8:17 / /media/alex/USB\\040Disk rw,relatime - vfat /dev/sdb1 rw
";

        assert_eq!(
            linux_mountinfo_drive_roots(mountinfo),
            vec![
                PathBuf::from("/"),
                PathBuf::from("/media/alex/USB Disk"),
                PathBuf::from("/mnt/projects"),
                PathBuf::from("/run/media/alex/Camera"),
            ]
        );
    }

    #[test]
    fn linux_mountinfo_drive_roots_can_read_mountinfo_file() {
        let temp = TempDir::new();
        let mountinfo = temp.path().join("mountinfo");
        fs::write(
            &mountinfo,
            "36 25 8:1 / / rw,relatime - ext4 /dev/sda1 rw\n\
             40 25 8:17 / /media/alex/USB rw,relatime - vfat /dev/sdb1 rw\n",
        )
        .expect("write mountinfo");

        assert_eq!(
            linux_mountinfo_drive_roots_from_path(&mountinfo).expect("parse mountinfo"),
            vec![PathBuf::from("/"), PathBuf::from("/media/alex/USB")]
        );
    }

    #[test]
    fn drive_root_disc_marker_prefers_blu_ray_over_dvd() {
        let temp = TempDir::new();
        fs::create_dir(temp.path().join("VIDEO_TS")).expect("create dvd marker");
        fs::create_dir(temp.path().join("bdmv")).expect("create blu-ray marker");

        assert_eq!(
            drive_root_marker_disc_kind(temp.path()),
            Some(DriveDiscKind::BluRay)
        );
    }

    #[test]
    fn drive_root_disc_marker_detects_dvd_audio_or_video_folders() {
        let temp = TempDir::new();
        fs::create_dir(temp.path().join("audio_ts")).expect("create dvd marker");

        assert_eq!(
            drive_root_marker_disc_kind(temp.path()),
            Some(DriveDiscKind::Dvd)
        );
    }

    #[test]
    fn drive_root_disc_marker_returns_none_without_markers() {
        let temp = TempDir::new();

        assert_eq!(drive_root_marker_disc_kind(temp.path()), None);
    }

    #[test]
    fn drive_root_disc_kind_does_not_classify_regular_directories_with_markers() {
        let temp = TempDir::new();
        fs::create_dir(temp.path().join("BDMV")).expect("create blu-ray marker");

        assert_eq!(drive_root_disc_kind(temp.path()), None);
    }

    #[test]
    fn linux_mountinfo_physical_optical_detection_rejects_disk_images() {
        let mountinfo = "\
36 25 8:1 / / rw,relatime - ext4 /dev/sda1 rw
40 25 11:0 / /media/alex/Movie rw,relatime - udf /dev/sr0 ro
41 25 7:0 / /media/alex/Image rw,relatime - iso9660 /dev/loop0 ro
42 25 8:17 / /media/alex/USB rw,relatime - udf /dev/sdb1 rw
43 25 11:1 / /home/alex/Disc rw,relatime - udf /dev/sr1 ro
";

        assert!(linux_mountinfo_path_is_physical_optical_drive(
            mountinfo,
            Path::new("/media/alex/Movie"),
        ));
        assert!(!linux_mountinfo_path_is_physical_optical_drive(
            mountinfo,
            Path::new("/media/alex/Image"),
        ));
        assert!(!linux_mountinfo_path_is_physical_optical_drive(
            mountinfo,
            Path::new("/media/alex/USB"),
        ));
        assert!(!linux_mountinfo_path_is_physical_optical_drive(
            mountinfo,
            Path::new("/home/alex/Disc"),
        ));
    }

    #[test]
    fn linux_physical_optical_source_detection_rejects_loop_devices() {
        assert!(linux_disc_source_is_physical_optical("/dev/sr0"));
        assert!(linux_disc_source_is_physical_optical("/dev/sr12"));
        assert!(linux_disc_source_is_physical_optical("/dev/cdrom"));
        assert!(linux_disc_source_is_physical_optical("/dev/dvd"));
        assert!(!linux_disc_source_is_physical_optical("/dev/loop0"));
        assert!(!linux_disc_source_is_physical_optical("/dev/sdb1"));
    }

    #[test]
    fn macos_diskutil_evidence_requires_optical_and_physical_metadata() {
        assert_eq!(
            macos_diskutil_disc_evidence_is_physical_optical(&MacosDiskutilDiscEvidence {
                optical: Some(true),
                virtual_or_physical: Some("Physical".to_owned()),
            }),
            Some(true)
        );
        assert_eq!(
            macos_diskutil_disc_evidence_is_physical_optical(&MacosDiskutilDiscEvidence {
                optical: Some(true),
                virtual_or_physical: Some("Virtual".to_owned()),
            }),
            Some(false)
        );
        assert_eq!(
            macos_diskutil_disc_evidence_is_physical_optical(&MacosDiskutilDiscEvidence {
                optical: Some(false),
                virtual_or_physical: Some("Physical".to_owned()),
            }),
            Some(false)
        );
        assert_eq!(
            macos_diskutil_disc_evidence_is_physical_optical(&MacosDiskutilDiscEvidence {
                optical: Some(true),
                virtual_or_physical: None,
            }),
            None
        );
    }

    #[test]
    fn windows_storage_descriptor_evidence_rejects_virtual_or_unknown_devices() {
        const SCSI_DEVICE_TYPE_CD_DVD: u8 = 5;
        const BUS_TYPE_SCSI: i32 = 1;
        const BUS_TYPE_UNKNOWN: i32 = 0;
        const BUS_TYPE_VIRTUAL: i32 = 14;
        const BUS_TYPE_FILE_BACKED_VIRTUAL: i32 = 15;

        assert!(windows_storage_descriptor_is_physical_optical(
            SCSI_DEVICE_TYPE_CD_DVD,
            true,
            BUS_TYPE_SCSI,
        ));
        assert!(!windows_storage_descriptor_is_physical_optical(
            SCSI_DEVICE_TYPE_CD_DVD,
            true,
            BUS_TYPE_UNKNOWN,
        ));
        assert!(!windows_storage_descriptor_is_physical_optical(
            SCSI_DEVICE_TYPE_CD_DVD,
            true,
            BUS_TYPE_VIRTUAL,
        ));
        assert!(!windows_storage_descriptor_is_physical_optical(
            SCSI_DEVICE_TYPE_CD_DVD,
            true,
            BUS_TYPE_FILE_BACKED_VIRTUAL,
        ));
        assert!(!windows_storage_descriptor_is_physical_optical(
            SCSI_DEVICE_TYPE_CD_DVD,
            false,
            BUS_TYPE_SCSI,
        ));
    }

    #[test]
    fn wsl_drive_roots_from_distribution_names_builds_sorted_unc_roots() {
        let roots =
            wsl_drive_roots_from_distribution_names(["Ubuntu-24.04", "docker-desktop", "Alpine"]);

        assert_eq!(
            roots,
            vec![
                PathBuf::from("\\\\wsl.localhost\\Alpine\\"),
                PathBuf::from("\\\\wsl.localhost\\docker-desktop\\"),
                PathBuf::from("\\\\wsl.localhost\\Ubuntu-24.04\\"),
            ]
        );
    }

    #[test]
    fn wsl_drive_roots_from_distribution_names_omits_blank_names() {
        let roots = wsl_drive_roots_from_distribution_names(["", "  ", "Ubuntu"]);

        assert_eq!(roots, vec![PathBuf::from("\\\\wsl.localhost\\Ubuntu\\")]);
    }

    #[test]
    fn wsl_distro_kind_matches_partial_distribution_names() {
        let cases = [
            ("Alpine", WslDistroKind::Alpine),
            ("Debian GNU/Linux", WslDistroKind::Debian),
            ("kali-linux", WslDistroKind::Kali),
            ("openSUSE-Leap", WslDistroKind::OpenSuse),
            ("Ubuntu-24.04", WslDistroKind::Ubuntu),
            ("docker-desktop", WslDistroKind::Generic),
        ];

        for (name, expected) in cases {
            assert_eq!(wsl_distro_kind_from_name(name), expected, "{name}");
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn wsl_distro_kind_for_path_detects_exact_unc_roots() {
        assert_eq!(
            wsl_distro_kind_for_path(Path::new(r"\\wsl.localhost\Ubuntu-24.04\")),
            Some(WslDistroKind::Ubuntu)
        );
        assert_eq!(
            wsl_distro_kind_for_path(Path::new(r"\\wsl$\openSUSE-Leap\")),
            Some(WslDistroKind::OpenSuse)
        );
        assert_eq!(
            wsl_distro_kind_for_path(Path::new(r"\\wsl.localhost\Ubuntu-24.04\home")),
            None
        );
    }

    #[test]
    fn filesystem_root_detection_requires_absolute_root() {
        let root = if cfg!(target_os = "windows") {
            PathBuf::from("C:\\")
        } else {
            PathBuf::from("/")
        };

        assert!(path_is_filesystem_root(&root));
        assert!(!path_is_filesystem_root(Path::new("relative")));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn wsl_unc_detection_distinguishes_root_from_subdirectories() {
        let root = PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\");
        let legacy_root = PathBuf::from(r"\\wsl$\Ubuntu\");
        let home = PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\home");
        let normal_unc = PathBuf::from(r"\\server\share\");

        assert!(path_is_wsl_unc(&root));
        assert!(path_is_wsl_unc_root(&root));
        assert!(path_is_wsl_unc(&legacy_root));
        assert!(path_is_wsl_unc_root(&legacy_root));
        assert!(path_is_wsl_unc(&home));
        assert!(!path_is_wsl_unc_root(&home));
        assert!(!path_is_wsl_unc(&normal_unc));
        assert!(!path_is_wsl_unc_root(&normal_unc));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_drive_root_protected_directories_are_always_hidden() {
        let protected_entries = [
            ("$RECYCLEBIN", r"C:\$RECYCLEBIN"),
            ("$Recycle.Bin", r"C:\$Recycle.Bin"),
            ("Config.Msi", r"C:\Config.Msi"),
            ("Recovery", r"C:\Recovery"),
            ("System Volume Information", r"C:\System Volume Information"),
            ("Documents and Settings", r"C:\Documents and Settings"),
        ];

        for (name, path) in protected_entries {
            assert!(
                should_hide_entry(std::ffi::OsStr::new(name), Path::new(path), true),
                "{name} should stay hidden when Hidden Items is enabled"
            );
            assert!(
                should_hide_entry(std::ffi::OsStr::new(name), Path::new(path), false),
                "{name} should be hidden when Hidden Items is disabled"
            );
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_drive_root_protected_directory_filter_is_case_insensitive() {
        assert!(should_hide_entry(
            std::ffi::OsStr::new("sYsTeM VoLuMe InFoRmAtIoN"),
            Path::new(r"C:\sYsTeM VoLuMe InFoRmAtIoN"),
            true,
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_drive_root_protected_directory_filter_only_applies_to_drive_root_children() {
        assert!(!should_hide_entry(
            std::ffi::OsStr::new("Recovery"),
            Path::new(r"C:\Users\Recovery"),
            true,
        ));
        assert!(!should_hide_entry(
            std::ffi::OsStr::new("Recovery"),
            Path::new(r"\\server\share\Recovery"),
            true,
        ));
        assert!(!should_hide_entry(
            std::ffi::OsStr::new("Recovery"),
            Path::new(r"\\wsl.localhost\Ubuntu\Recovery"),
            true,
        ));
        assert!(!should_hide_entry(
            std::ffi::OsStr::new("Recovery-old"),
            Path::new(r"C:\Recovery-old"),
            true,
        ));
    }

    #[test]
    fn hidden_entry_filter_omits_dot_prefixed_entries_when_enabled() {
        let temp = TempDir::new();
        fs::write(temp.path().join(".hidden"), b"hidden").expect("create hidden file");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible file");

        let entries = load_entries_with_options(
            temp.path(),
            EntryLoadOptions {
                visibility: EntryVisibility::new(false, false),
                applications_view: false,
            },
        )
        .expect("load entries");

        assert_eq!(sorted_entry_names(&entries), vec!["visible.txt"]);
    }

    #[test]
    fn hidden_entry_filter_keeps_dot_prefixed_entries_when_disabled() {
        let temp = TempDir::new();
        fs::write(temp.path().join(".hidden"), b"hidden").expect("create hidden file");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible file");

        let entries = load_entries_with_options(
            temp.path(),
            EntryLoadOptions {
                visibility: EntryVisibility::new(true, true),
                applications_view: false,
            },
        )
        .expect("load entries");

        assert_eq!(sorted_entry_names(&entries), vec![".hidden", "visible.txt"]);
    }

    #[test]
    fn load_entries_omits_hidden_entries_when_show_hidden_files_is_false() {
        let temp = TempDir::new();
        fs::write(temp.path().join(".hidden"), b"hidden").expect("create hidden file");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible file");

        let entries = load_entries(temp.path(), false).expect("load entries");

        assert_eq!(sorted_entry_names(&entries), vec!["visible.txt"]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn load_entries_omits_windows_hidden_attribute_entries_when_show_hidden_files_is_false() {
        let temp = TempDir::new();
        let hidden = temp.path().join("hidden.txt");
        fs::write(&hidden, b"hidden").expect("create hidden file");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible file");
        set_windows_file_attributes(&hidden, FILE_ATTRIBUTE_HIDDEN);

        let hidden_off = load_entries(temp.path(), false).expect("load entries");
        let hidden_on = load_entries(temp.path(), true).expect("load entries");

        assert_eq!(sorted_entry_names(&hidden_off), vec!["visible.txt"]);
        assert_eq!(
            sorted_entry_names(&hidden_on),
            vec!["hidden.txt", "visible.txt"]
        );

        set_windows_file_attributes(&hidden, FILE_ATTRIBUTE_NORMAL);
    }

    #[test]
    fn load_entries_keeps_hidden_entries_when_show_hidden_files_is_true() {
        let temp = TempDir::new();
        fs::write(temp.path().join(".hidden"), b"hidden").expect("create hidden file");
        fs::write(temp.path().join(".DS_Store"), b"metadata").expect("create metadata file");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible file");

        let entries = load_entries(temp.path(), true).expect("load entries");

        assert_eq!(sorted_entry_names(&entries), vec![".hidden", "visible.txt"]);
    }

    #[test]
    fn dot_and_hidden_attribute_visibility_are_independent() {
        let temp = TempDir::new();
        fs::write(temp.path().join(".dot"), b"dot").expect("create dot file");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible file");

        let neither = load_entries(temp.path(), EntryVisibility::new(false, false))
            .expect("load entries with both disabled");
        let dots = load_entries(temp.path(), EntryVisibility::new(true, false))
            .expect("load entries with dots enabled");
        let hidden = load_entries(temp.path(), EntryVisibility::new(false, true))
            .expect("load entries with hidden attributes enabled");
        let both = load_entries(temp.path(), EntryVisibility::new(true, true))
            .expect("load entries with both enabled");

        assert_eq!(sorted_entry_names(&neither), vec!["visible.txt"]);
        assert_eq!(sorted_entry_names(&dots), vec![".dot", "visible.txt"]);
        assert_eq!(sorted_entry_names(&hidden), vec!["visible.txt"]);
        assert_eq!(sorted_entry_names(&both), vec![".dot", "visible.txt"]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn dotfile_with_hidden_attribute_requires_both_visibility_options() {
        let temp = TempDir::new();
        let hidden_dot = temp.path().join(".hidden-dot");
        fs::write(&hidden_dot, b"hidden").expect("create hidden dot file");
        set_windows_file_attributes(&hidden_dot, FILE_ATTRIBUTE_HIDDEN);

        for visibility in [
            EntryVisibility::new(false, false),
            EntryVisibility::new(true, false),
            EntryVisibility::new(false, true),
        ] {
            assert!(
                load_entries(temp.path(), visibility)
                    .expect("load entries")
                    .is_empty()
            );
        }
        assert_eq!(
            sorted_entry_names(
                &load_entries(temp.path(), EntryVisibility::new(true, true)).expect("load entries")
            ),
            vec![".hidden-dot"]
        );

        set_windows_file_attributes(&hidden_dot, FILE_ATTRIBUTE_NORMAL);
    }

    #[test]
    fn metadata_entry_filter_omits_macos_metadata_names_even_when_hidden_filter_is_disabled() {
        let temp = TempDir::new();
        fs::write(temp.path().join(".DS_Store"), b"metadata").expect("create ds store file");
        fs::write(temp.path().join(".hidden"), b"hidden").expect("create hidden file");
        fs::write(temp.path().join(".localized"), b"metadata").expect("create localized file");
        fs::create_dir(temp.path().join("__MACOSX")).expect("create macos archive metadata dir");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible file");

        let entries = load_entries_with_options(
            temp.path(),
            EntryLoadOptions {
                visibility: EntryVisibility::new(true, true),
                applications_view: false,
            },
        )
        .expect("load entries");

        assert_eq!(sorted_entry_names(&entries), vec![".hidden", "visible.txt"]);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn applications_view_includes_direct_and_one_level_nested_app_bundles() {
        let temp = TempDir::new();
        let preview = temp.path().join("Preview.app");
        let utilities = temp.path().join("Utilities");
        let terminal = utilities.join("Terminal.app");
        let macports = temp.path().join("MacPorts");
        fs::create_dir(&preview).expect("create direct app");
        fs::create_dir(&utilities).expect("create utilities");
        fs::create_dir(&terminal).expect("create nested app");
        fs::create_dir(&macports).expect("create non-app folder");
        fs::write(temp.path().join("readme.txt"), b"not an app").expect("create file");

        let entries = load_entries_with_options(
            temp.path(),
            EntryLoadOptions {
                visibility: EntryVisibility::new(false, false),
                applications_view: true,
            },
        )
        .expect("load applications view");

        assert_eq!(sorted_entry_paths(&entries), vec![preview, terminal]);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn applications_view_omits_hidden_direct_and_nested_app_bundles() {
        let temp = TempDir::new();
        let visible = temp.path().join("Visible.app");
        let hidden_direct = temp.path().join(".Hidden.app");
        let utilities = temp.path().join("Utilities");
        let hidden_nested = utilities.join(".Nested.app");
        fs::create_dir(&visible).expect("create visible app");
        fs::create_dir(&hidden_direct).expect("create hidden direct app");
        fs::create_dir(&utilities).expect("create utilities");
        fs::create_dir(&hidden_nested).expect("create hidden nested app");

        let entries = load_entries_with_options(
            temp.path(),
            EntryLoadOptions {
                visibility: EntryVisibility::new(false, false),
                applications_view: true,
            },
        )
        .expect("load applications view");

        assert_eq!(sorted_entry_paths(&entries), vec![visible]);
    }

    #[test]
    fn normal_entries_view_keeps_non_app_folders() {
        let temp = TempDir::new();
        let utilities = temp.path().join("Utilities");
        let terminal = utilities.join("Terminal.app");
        fs::create_dir(&utilities).expect("create utilities");
        fs::create_dir(&terminal).expect("create nested app");

        let entries = load_entries_with_options(
            temp.path(),
            EntryLoadOptions {
                visibility: EntryVisibility::new(true, true),
                applications_view: false,
            },
        )
        .expect("load normal view");

        assert_eq!(sorted_entry_names(&entries), vec!["Utilities"]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_drive_filter_includes_this_pc_local_drive_types() {
        assert!(windows_drive_type_is_explorer_local(3));
        assert!(windows_drive_type_is_explorer_local(2));
        assert!(windows_drive_type_is_explorer_local(5));
        assert!(windows_drive_type_is_explorer_local(6));
        assert!(!windows_drive_type_is_explorer_local(4));
        assert!(!windows_drive_type_is_explorer_local(1));
        assert!(!windows_drive_type_is_explorer_local(0));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_drive_label_uses_custom_volume_name() {
        assert_eq!(
            windows_drive_display_label(r"C:\", Some("Work")),
            "Work (C:)"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_drive_label_falls_back_for_empty_volume_name() {
        assert_eq!(
            windows_drive_display_label(r"C:\", Some("")),
            "Local Disk (C:)"
        );
        assert_eq!(windows_drive_display_label(r"C:\", None), "Local Disk (C:)");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn drive_display_label_uses_path_label_on_non_windows() {
        assert_eq!(drive_display_label(Path::new("/")), "/");
    }

    #[test]
    fn copy_engine_selection_defaults_to_delta_for_all_file_copies() {
        let temp = TempDir::new();
        let source_root = temp.path().join("source");
        let destination_root = temp.path().join("destination");
        fs::create_dir_all(&source_root).expect("create source root");
        fs::create_dir_all(&destination_root).expect("create destination root");
        let source = source_root.join("file.txt");
        let destination = destination_root.join("file.txt");

        let _source_volume = set_test_path_volume_key(&source_root, Some("source-volume"));
        let destination_volume =
            set_test_path_volume_key(&destination_root, Some("destination-volume"));
        assert_eq!(
            copy_engine_for_paths(&source, &destination, false),
            CopyEngine::ResumableDelta
        );
        assert!(!paths_are_on_same_volume(&source, &destination));

        drop(destination_volume);
        let destination_volume = set_test_path_volume_key(&destination_root, Some("source-volume"));
        assert_eq!(
            copy_engine_for_paths(&source, &destination, false),
            CopyEngine::ResumableDelta
        );
        assert!(paths_are_on_same_volume(&source, &destination));
        assert_eq!(
            copy_engine_for_paths(&source, &destination, true),
            CopyEngine::ResumableDelta
        );

        drop(destination_volume);
        let _destination_volume = set_test_path_volume_key(&destination_root, None);
        assert_eq!(
            copy_engine_for_paths(&source, &destination, false),
            CopyEngine::ResumableDelta
        );
        assert!(paths_are_on_same_volume(&source, &destination));
        assert_eq!(
            copy_engine_for_paths(&source, &destination, true),
            CopyEngine::ResumableDelta
        );
    }

    fn finished_summary(result: Result<FileOperationOutcome, String>) -> FileOperationSummary {
        match result.expect("file operation") {
            FileOperationOutcome::Finished(summary) => summary,
            FileOperationOutcome::Conflicts(conflicts) => {
                panic!(
                    "expected operation to finish, found {} conflicts",
                    conflicts.len()
                )
            }
        }
    }

    fn conflict_batch(result: Result<FileOperationOutcome, String>) -> FileConflictBatch {
        match result.expect("file operation") {
            FileOperationOutcome::Conflicts(conflicts) => conflicts,
            FileOperationOutcome::Finished(_) => panic!("expected file conflicts"),
        }
    }

    fn ready_job(result: Result<PreparedFileOperation, String>) -> FileOperationJob {
        match result.expect("prepared operation") {
            PreparedFileOperation::Ready(job) => job,
            PreparedFileOperation::Conflicts(conflicts) => {
                panic!(
                    "expected ready operation, found {} conflicts",
                    conflicts.len()
                )
            }
        }
    }

    fn prepared_conflict_batch(result: Result<PreparedFileOperation, String>) -> FileConflictBatch {
        match result.expect("prepared operation") {
            PreparedFileOperation::Conflicts(conflicts) => conflicts,
            PreparedFileOperation::Ready(_) => panic!("expected file conflicts"),
        }
    }

    fn temp_copy_files(path: &Path) -> Vec<PathBuf> {
        fs::read_dir(path)
            .expect("read temp files")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(OsStr::to_str)
                    .is_some_and(|name| name.starts_with(".explorer-copy-"))
            })
            .collect()
    }

    #[test]
    fn prepared_operation_counts_nested_file_totals() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        let destination = temp.path().join("destination");
        fs::create_dir_all(source.join("nested")).expect("create nested source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(source.join("a.txt"), b"abc").expect("create first file");
        fs::write(source.join("nested").join("b.txt"), b"defg").expect("create second file");

        let job = ready_job(prepare_copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));

        assert_eq!(job.stats.total_files, 2);
        assert_eq!(job.stats.total_bytes, 7);
    }

    #[test]
    fn progress_copy_uses_temp_file_and_reports_bytes() {
        let temp = TempDir::new();
        let source = temp.path().join("large.bin");
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).expect("create destination");
        let data = vec![7; COPY_BUFFER_SIZE + 128];
        fs::write(&source, &data).expect("create source");
        let job = ready_job(prepare_copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let mut progress_events = Vec::new();

        let summary = execute_file_operation_with_progress(
            job,
            ConflictChoice::Replace,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            |progress| progress_events.push(progress),
        )
        .expect("copy with progress");

        let copied = destination.join("large.bin");
        assert_eq!(fs::read(&copied).unwrap(), data);
        assert_eq!(summary.destination_paths, vec![copied]);
        assert!(temp_copy_files(&destination).is_empty());
        assert!(
            progress_events
                .iter()
                .any(|progress| progress.copied_bytes > 0)
        );
        assert_eq!(
            progress_events.last().map(|progress| progress.phase),
            Some(FileOperationPhase::Finished)
        );
    }

    #[test]
    fn progress_copy_reports_intermediate_bytes_for_parallel_large_file() {
        let temp = TempDir::new();
        let source = temp.path().join("large.bin");
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).expect("create destination");
        let data = vec![5; COPY_BUFFER_SIZE * 9 + 128];
        fs::write(&source, &data).expect("create source");
        let total_bytes = data.len() as u64;
        let job = ready_job(prepare_copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let mut progress_events = Vec::new();

        let summary = execute_file_operation_with_progress(
            job,
            ConflictChoice::Replace,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            |progress| progress_events.push(progress),
        )
        .expect("copy with live progress");

        let copied = destination.join("large.bin");
        assert_eq!(fs::read(&copied).unwrap(), data);
        assert_eq!(summary.destination_paths, vec![copied]);
        assert!(progress_events.iter().any(|progress| {
            progress.phase == FileOperationPhase::Copying
                && progress.copied_bytes > 0
                && progress.copied_bytes < total_bytes
        }));
        assert_eq!(
            progress_events.last().map(|progress| progress.phase),
            Some(FileOperationPhase::Finished)
        );
    }

    #[test]
    fn cancelling_delta_copy_preserves_sidecars_and_keeps_source() {
        let temp = TempDir::new();
        let source = temp.path().join("large.bin");
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).expect("create destination");
        fs::write(&source, vec![9; COPY_BUFFER_SIZE + 128]).expect("create source");
        let job = ready_job(prepare_copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut requested_cancel = false;

        let result = execute_file_operation_with_progress(
            job,
            ConflictChoice::Replace,
            cancel.clone(),
            Arc::new(AtomicBool::new(false)),
            |progress| {
                if progress.copied_bytes > 0 && !requested_cancel {
                    requested_cancel = true;
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );

        assert_eq!(result, Err(FileOperationError::Cancelled));
        assert!(source.exists());
        assert!(!destination.join("large.bin").exists());
        let copy_files = temp_copy_files(&destination);
        assert!(copy_files.iter().any(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.ends_with(".partial"))
        }));
        assert!(copy_files.iter().any(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.ends_with(".json"))
        }));
    }

    #[test]
    fn terminating_delta_copy_removes_sidecars_and_keeps_source() {
        let temp = TempDir::new();
        let source = temp.path().join("large.bin");
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).expect("create destination");
        fs::write(&source, vec![11; COPY_BUFFER_SIZE + 128]).expect("create source");
        let job = ready_job(prepare_copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let cancel = Arc::new(AtomicBool::new(false));
        let terminate = Arc::new(AtomicBool::new(false));
        let mut requested_terminate = false;

        let result = execute_file_operation_with_progress(
            job,
            ConflictChoice::Replace,
            cancel.clone(),
            terminate.clone(),
            |progress| {
                if progress.copied_bytes > 0 && !requested_terminate {
                    requested_terminate = true;
                    terminate.store(true, Ordering::Relaxed);
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );

        assert_eq!(result, Err(FileOperationError::Cancelled));
        assert!(source.exists());
        assert!(!destination.join("large.bin").exists());
        assert!(temp_copy_files(&destination).is_empty());
    }

    #[test]
    fn terminating_before_resume_starts_removes_existing_sidecars() {
        let temp = TempDir::new();
        let source = temp.path().join("large.bin");
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).expect("create destination");
        fs::write(&source, vec![13; COPY_BUFFER_SIZE + 128]).expect("create source");

        let job = ready_job(prepare_copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut requested_cancel = false;
        let result = execute_file_operation_with_progress(
            job,
            ConflictChoice::Replace,
            cancel.clone(),
            Arc::new(AtomicBool::new(false)),
            |progress| {
                if progress.copied_bytes > 0 && !requested_cancel {
                    requested_cancel = true;
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );
        assert_eq!(result, Err(FileOperationError::Cancelled));
        assert!(!temp_copy_files(&destination).is_empty());

        let job = ready_job(prepare_copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let cancel = Arc::new(AtomicBool::new(false));
        let terminate = Arc::new(AtomicBool::new(false));
        let mut requested_terminate = false;
        let result = execute_file_operation_with_progress(
            job,
            ConflictChoice::Replace,
            cancel.clone(),
            terminate.clone(),
            |_| {
                if !requested_terminate {
                    requested_terminate = true;
                    terminate.store(true, Ordering::Relaxed);
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );

        assert_eq!(result, Err(FileOperationError::Cancelled));
        assert!(source.exists());
        assert!(!destination.join("large.bin").exists());
        assert!(temp_copy_files(&destination).is_empty());
    }

    #[test]
    fn chunked_copy_preserves_modified_time() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).expect("create destination");
        fs::write(&source, b"data").expect("create source");
        let modified = FileTime::from_unix_time(1_700_000_000, 0);
        filetime::set_file_mtime(&source, modified).expect("set modified time");
        let job = ready_job(prepare_copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));

        execute_file_operation_with_progress(
            job,
            ConflictChoice::Replace,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .expect("copy with metadata");

        let copied_metadata = fs::metadata(destination.join("file.txt")).unwrap();
        assert_eq!(
            FileTime::from_last_modification_time(&copied_metadata),
            modified
        );
    }

    #[test]
    fn move_file_to_directory() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::write(&source, b"data").expect("create file");
        fs::create_dir(&destination).expect("create destination");

        let moved = move_paths_to_directory(std::slice::from_ref(&source), &destination)
            .expect("move file");
        let moved = finished_summary(Ok(moved));

        assert!(!source.exists());
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"data");
        assert_eq!(moved.destination_paths, vec![destination.join("file.txt")]);
        assert_eq!(moved.moved_source_paths, vec![source]);
    }

    #[test]
    fn move_directory_recursively_to_directory() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        let nested = source.join("nested");
        let destination = temp.path().join("destination");
        fs::create_dir_all(&nested).expect("create nested source");
        fs::write(nested.join("file.txt"), b"data").expect("create nested file");
        fs::create_dir(&destination).expect("create destination");

        let moved = move_paths_to_directory(std::slice::from_ref(&source), &destination)
            .expect("move directory");
        let moved = finished_summary(Ok(moved));

        assert!(!source.exists());
        assert_eq!(
            fs::read(destination.join("folder").join("nested").join("file.txt")).unwrap(),
            b"data"
        );
        assert_eq!(moved.destination_paths, vec![destination.join("folder")]);
        assert_eq!(moved.moved_source_paths, vec![source]);
    }

    #[test]
    fn copy_file_to_directory() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::write(&source, b"data").expect("create file");
        fs::create_dir(&destination).expect("create destination");

        let copied = copy_paths_to_directory(std::slice::from_ref(&source), &destination)
            .expect("copy file");
        let copied = finished_summary(Ok(copied));

        assert_eq!(fs::read(&source).unwrap(), b"data");
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"data");
        assert_eq!(copied.destination_paths, vec![destination.join("file.txt")]);
        assert!(copied.moved_source_paths.is_empty());
    }

    #[test]
    fn copy_directory_recursively_to_directory() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        let nested = source.join("nested");
        let destination = temp.path().join("destination");
        fs::create_dir_all(&nested).expect("create nested source");
        fs::write(nested.join("file.txt"), b"data").expect("create nested file");
        fs::create_dir(&destination).expect("create destination");

        let copied = copy_paths_to_directory(std::slice::from_ref(&source), &destination)
            .expect("copy directory");
        let copied = finished_summary(Ok(copied));

        assert!(source.exists());
        assert_eq!(
            fs::read(destination.join("folder").join("nested").join("file.txt")).unwrap(),
            b"data"
        );
        assert_eq!(copied.destination_paths, vec![destination.join("folder")]);
    }

    #[test]
    fn copy_directory_update_leaves_destination_only_files() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        let destination = temp.path().join("destination");
        fs::create_dir_all(source.join("nested")).expect("create source");
        fs::create_dir_all(destination.join("folder").join("nested")).expect("create destination");
        fs::write(source.join("nested").join("file.txt"), b"source").expect("write source");
        fs::write(
            destination.join("folder").join("nested").join("extra.txt"),
            b"extra",
        )
        .expect("write extra");

        let copied = copy_paths_to_directory(std::slice::from_ref(&source), &destination)
            .expect("copy directory");
        let copied = finished_summary(Ok(copied));

        assert_eq!(
            fs::read(destination.join("folder").join("nested").join("file.txt")).unwrap(),
            b"source"
        );
        assert_eq!(
            fs::read(destination.join("folder").join("nested").join("extra.txt")).unwrap(),
            b"extra"
        );
        assert_eq!(copied.destination_paths, vec![destination.join("folder")]);
    }

    #[test]
    fn parallel_copy_multiple_files_preserves_root_summary_order() {
        let temp = TempDir::new();
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).expect("create destination");
        let sources = (0..8)
            .map(|index| {
                let source = temp.path().join(format!("file-{index}.txt"));
                fs::write(&source, format!("content-{index}")).expect("write source");
                source
            })
            .collect::<Vec<_>>();
        let mut progress_events = Vec::new();
        let job = ready_job(prepare_copy_paths_to_directory(&sources, &destination));

        let summary = execute_file_operation_with_progress(
            job,
            ConflictChoice::Replace,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            |progress| progress_events.push(progress),
        )
        .expect("copy files");

        for (index, source) in sources.iter().enumerate() {
            assert_eq!(
                fs::read(destination.join(source.file_name().unwrap())).unwrap(),
                format!("content-{index}").as_bytes()
            );
        }
        assert_eq!(
            summary.destination_paths,
            sources
                .iter()
                .map(|source| destination.join(source.file_name().unwrap()))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            progress_events
                .last()
                .map(|progress| progress.completed_files),
            Some(8)
        );
    }

    #[test]
    fn copy_conflict_replace_overwrites_destination() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::write(&source, b"source").expect("create source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(destination.join("file.txt"), b"existing").expect("create existing");

        let conflicts = conflict_batch(copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let summary =
            resolve_file_conflicts(conflicts, ConflictChoice::Replace).expect("replace conflict");

        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"source");
        assert_eq!(
            summary.destination_paths,
            vec![destination.join("file.txt")]
        );
    }

    #[test]
    fn copy_conflict_replace_on_same_volume_uses_resumable_copy() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::write(&source, b"aaaabbbbccccdddd").expect("create source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(destination.join("file.txt"), b"aaaaXXXXccccdddd").expect("create existing");
        let conflicts = prepared_conflict_batch(prepare_copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let mut progress_events = Vec::new();

        let summary = execute_file_operation_with_progress(
            conflicts.into_job(),
            ConflictChoice::Replace,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            |progress| progress_events.push(progress),
        )
        .expect("replace with resumable copy");

        assert_eq!(
            fs::read(destination.join("file.txt")).unwrap(),
            b"aaaabbbbccccdddd"
        );
        assert_eq!(
            summary.destination_paths,
            vec![destination.join("file.txt")]
        );
        assert!(
            progress_events
                .iter()
                .any(|progress| progress.phase == FileOperationPhase::Verifying)
        );
    }

    #[test]
    fn copy_conflict_skip_leaves_files_unchanged() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::write(&source, b"source").expect("create source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(destination.join("file.txt"), b"existing").expect("create existing");

        let conflicts = conflict_batch(copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let summary =
            resolve_file_conflicts(conflicts, ConflictChoice::Skip).expect("skip conflict");

        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"existing");
        assert!(summary.destination_paths.is_empty());
    }

    #[test]
    fn move_conflict_replace_overwrites_destination_and_removes_source() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::write(&source, b"source").expect("create source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(destination.join("file.txt"), b"existing").expect("create existing");

        let conflicts = conflict_batch(move_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let summary =
            resolve_file_conflicts(conflicts, ConflictChoice::Replace).expect("replace conflict");

        assert!(!source.exists());
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"source");
        assert_eq!(
            summary.destination_paths,
            vec![destination.join("file.txt")]
        );
        assert_eq!(summary.moved_source_paths, vec![source]);
    }

    #[test]
    fn move_conflict_replace_across_known_volumes_uses_resumable_copy_before_removing_source() {
        let temp = TempDir::new();
        let source_root = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir_all(&source_root).expect("create source root");
        fs::create_dir(&destination).expect("create destination");
        let source = source_root.join("file.txt");
        fs::write(&source, b"aaaabbbbccccdddd").expect("create source");
        fs::write(destination.join("file.txt"), b"aaaaXXXXccccdddd").expect("create existing");
        let _source_volume = set_test_path_volume_key(&source_root, Some("source-volume"));
        let _destination_volume =
            set_test_path_volume_key(&destination, Some("destination-volume"));
        let conflicts = prepared_conflict_batch(prepare_move_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let mut progress_events = Vec::new();

        let summary = execute_file_operation_with_progress(
            conflicts.into_job(),
            ConflictChoice::Replace,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            |progress| progress_events.push(progress),
        )
        .expect("replace with resumable copy");

        assert!(!source.exists());
        assert_eq!(
            fs::read(destination.join("file.txt")).unwrap(),
            b"aaaabbbbccccdddd"
        );
        assert_eq!(
            summary.destination_paths,
            vec![destination.join("file.txt")]
        );
        assert_eq!(summary.moved_source_paths, vec![source]);
        assert!(
            progress_events
                .iter()
                .any(|progress| progress.phase == FileOperationPhase::Indexing)
        );
        assert!(
            progress_events
                .iter()
                .any(|progress| progress.phase == FileOperationPhase::Verifying)
        );
    }

    #[test]
    fn move_conflict_skip_leaves_files_unchanged() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::write(&source, b"source").expect("create source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(destination.join("file.txt"), b"existing").expect("create existing");

        let conflicts = conflict_batch(move_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let summary =
            resolve_file_conflicts(conflicts, ConflictChoice::Skip).expect("skip conflict");

        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"existing");
        assert!(summary.destination_paths.is_empty());
        assert!(summary.moved_source_paths.is_empty());
    }

    #[test]
    fn multiple_conflicts_replace_applies_to_all_conflicts() {
        let temp = TempDir::new();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir(&source).expect("create source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(source.join("a.txt"), b"new a").expect("create source a");
        fs::write(source.join("b.txt"), b"new b").expect("create source b");
        fs::write(destination.join("a.txt"), b"old a").expect("create destination a");
        fs::write(destination.join("b.txt"), b"old b").expect("create destination b");

        let conflicts = conflict_batch(copy_paths_to_directory(
            &[source.join("a.txt"), source.join("b.txt")],
            &destination,
        ));
        assert_eq!(conflicts.len(), 2);

        resolve_file_conflicts(conflicts, ConflictChoice::Replace).expect("replace conflicts");

        assert_eq!(fs::read(destination.join("a.txt")).unwrap(), b"new a");
        assert_eq!(fs::read(destination.join("b.txt")).unwrap(), b"new b");
    }

    #[test]
    fn multiple_conflicts_skip_applies_to_all_conflicts() {
        let temp = TempDir::new();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir(&source).expect("create source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(source.join("a.txt"), b"new a").expect("create source a");
        fs::write(source.join("b.txt"), b"new b").expect("create source b");
        fs::write(destination.join("a.txt"), b"old a").expect("create destination a");
        fs::write(destination.join("b.txt"), b"old b").expect("create destination b");

        let conflicts = conflict_batch(copy_paths_to_directory(
            &[source.join("a.txt"), source.join("b.txt")],
            &destination,
        ));
        assert_eq!(conflicts.len(), 2);

        let summary =
            resolve_file_conflicts(conflicts, ConflictChoice::Skip).expect("skip conflicts");

        assert_eq!(fs::read(destination.join("a.txt")).unwrap(), b"old a");
        assert_eq!(fs::read(destination.join("b.txt")).unwrap(), b"old b");
        assert!(summary.destination_paths.is_empty());
    }

    #[test]
    fn mixed_conflicting_and_non_conflicting_files_continue_after_skip() {
        let temp = TempDir::new();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir(&source).expect("create source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(source.join("conflict.txt"), b"new").expect("create conflict source");
        fs::write(source.join("new.txt"), b"new file").expect("create non-conflict source");
        fs::write(destination.join("conflict.txt"), b"old").expect("create conflict destination");

        let conflicts = conflict_batch(copy_paths_to_directory(
            &[source.join("conflict.txt"), source.join("new.txt")],
            &destination,
        ));
        let summary =
            resolve_file_conflicts(conflicts, ConflictChoice::Skip).expect("skip conflicts");

        assert_eq!(fs::read(destination.join("conflict.txt")).unwrap(), b"old");
        assert_eq!(fs::read(destination.join("new.txt")).unwrap(), b"new file");
        assert_eq!(summary.destination_paths, vec![destination.join("new.txt")]);
    }

    #[test]
    fn duplicate_destination_names_fail_before_copying() {
        let temp = TempDir::new();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let destination = temp.path().join("destination");
        fs::create_dir_all(&first).expect("create first");
        fs::create_dir_all(&second).expect("create second");
        fs::create_dir(&destination).expect("create destination");
        let first_file = first.join("file.txt");
        let second_file = second.join("file.txt");
        fs::write(&first_file, b"first").expect("create first file");
        fs::write(&second_file, b"second").expect("create second file");

        let error = copy_paths_to_directory(&[first_file, second_file], &destination)
            .expect_err("duplicate names should fail");

        assert!(error.contains("Multiple selected items are named file.txt"));
        assert!(!destination.join("file.txt").exists());
    }

    #[test]
    fn same_directory_move_is_no_op() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        fs::write(&source, b"data").expect("create source");

        let moved =
            move_paths_to_directory(std::slice::from_ref(&source), temp.path()).expect("move noop");
        let moved = finished_summary(Ok(moved));

        assert!(moved.destination_paths.is_empty());
        assert_eq!(fs::read(&source).unwrap(), b"data");
    }

    #[test]
    fn moving_directory_into_descendant_fails() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        let descendant = source.join("child");
        fs::create_dir_all(&descendant).expect("create descendant");

        let error = move_paths_to_directory(std::slice::from_ref(&source), &descendant)
            .expect_err("descendant move should fail");

        assert!(error.contains("Cannot move folder into itself"));
        assert!(source.exists());
        assert!(descendant.exists());
    }

    #[test]
    fn paste_copy_in_same_directory_uses_windows_copy_names() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        fs::write(&source, b"data").expect("create source");
        fs::write(temp.path().join("file - Copy.txt"), b"existing").expect("create first copy");

        let copied = copy_paths_to_directory_for_paste(std::slice::from_ref(&source), temp.path())
            .expect("paste copy");
        let copied = finished_summary(Ok(copied));

        assert_eq!(
            copied.destination_paths,
            vec![temp.path().join("file - Copy (2).txt")]
        );
        assert_eq!(fs::read(&source).unwrap(), b"data");
        assert_eq!(
            fs::read(temp.path().join("file - Copy (2).txt")).unwrap(),
            b"data"
        );
    }

    #[test]
    fn paste_copy_directory_in_same_directory_uses_copy_name() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        fs::create_dir(&source).expect("create source directory");
        fs::write(source.join("nested.txt"), b"data").expect("create nested file");

        let copied = copy_paths_to_directory_for_paste(std::slice::from_ref(&source), temp.path())
            .expect("paste copy");
        let copied = finished_summary(Ok(copied));

        let copied_folder = temp.path().join("folder - Copy");
        assert_eq!(copied.destination_paths, vec![copied_folder.clone()]);
        assert_eq!(fs::read(copied_folder.join("nested.txt")).unwrap(), b"data");
    }

    #[test]
    fn folder_merge_copies_non_conflicting_nested_files() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        let destination = temp.path().join("destination");
        fs::create_dir_all(source.join("nested")).expect("create source nested");
        fs::create_dir_all(destination.join("folder")).expect("create destination folder");
        fs::write(source.join("nested").join("file.txt"), b"data").expect("create source file");

        let summary = finished_summary(copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));

        assert_eq!(
            fs::read(destination.join("folder").join("nested").join("file.txt")).unwrap(),
            b"data"
        );
        assert_eq!(summary.destination_paths, vec![destination.join("folder")]);
    }

    #[test]
    fn folder_merge_includes_nested_file_conflicts_in_global_choice() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        let destination = temp.path().join("destination");
        let destination_folder = destination.join("folder");
        fs::create_dir_all(source.join("nested")).expect("create source nested");
        fs::create_dir_all(destination_folder.join("nested")).expect("create destination nested");
        fs::write(source.join("nested").join("file.txt"), b"new").expect("create source file");
        fs::write(destination_folder.join("nested").join("file.txt"), b"old")
            .expect("create destination file");

        let conflicts = conflict_batch(copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        assert_eq!(conflicts.len(), 1);

        resolve_file_conflicts(conflicts, ConflictChoice::Replace).expect("replace nested");

        assert_eq!(
            fs::read(destination_folder.join("nested").join("file.txt")).unwrap(),
            b"new"
        );
    }

    #[test]
    fn paste_copy_directory_into_descendant_fails() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        let descendant = source.join("child");
        fs::create_dir_all(&descendant).expect("create descendant directory");

        let error = copy_paths_to_directory_for_paste(std::slice::from_ref(&source), &descendant)
            .expect_err("descendant copy should fail");

        assert!(error.contains("Cannot copy folder into itself"));
    }

    #[test]
    fn permanent_delete_removes_files_and_directories() {
        let temp = TempDir::new();
        let file = temp.path().join("file.txt");
        let folder = temp.path().join("folder");
        fs::write(&file, b"data").expect("create file");
        fs::create_dir(&folder).expect("create folder");
        fs::write(folder.join("nested.txt"), b"data").expect("create nested file");

        remove_paths_permanently(&[file.clone(), folder.clone()]).expect("delete paths");

        assert!(!file.exists());
        assert!(!folder.exists());
    }

    #[test]
    fn permanent_delete_existing_paths_ignores_already_missing_paths() {
        let temp = TempDir::new();
        let file = temp.path().join("file.txt");
        let missing = temp.path().join("missing.txt");
        fs::write(&file, b"data").expect("create file");

        let removed_any = remove_existing_paths_permanently(&[file.clone(), missing.clone()])
            .expect("delete existing paths");

        assert!(removed_any);
        assert!(!file.exists());
        assert!(!missing.exists());
        assert!(
            !remove_existing_paths_permanently(std::slice::from_ref(&missing))
                .expect("ignore missing path")
        );
    }

    #[test]
    fn trash_delete_missing_selection_errors() {
        assert!(trash_paths(&[]).is_err());
    }
}
