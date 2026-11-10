use std::{fs, io::ErrorKind, path::Path};

use serde::{Deserialize, Serialize};

use super::NativePath;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct Volume {
    root: NativePath,
    source: NativePath,
    #[cfg(unix)]
    filesystem: String,
    #[cfg(unix)]
    identity: Option<UnixIdentity>,
}

#[cfg(unix)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum UnixIdentity {
    Created(std::time::SystemTime),
    #[cfg(target_os = "linux")]
    Uuid(NativePath),
}

pub(super) fn confirmed_absent(path: &Path, volume: &Volume) -> bool {
    confirmed_absent_with(
        path,
        volume,
        |path| fs::metadata(path).map(|meta| meta.is_dir()),
        |path| fs::symlink_metadata(path).map(|meta| meta.is_dir() || meta.is_symlink()),
        for_path,
    )
}

fn confirmed_absent_with(
    path: &Path,
    volume: &Volume,
    reachable: impl Fn(&Path) -> std::io::Result<bool>,
    directory_or_link: impl Fn(&Path) -> std::io::Result<bool>,
    backing: impl Fn(&Path) -> Option<Volume>,
) -> bool {
    let missing = |kind| matches!(kind, ErrorKind::NotFound | ErrorKind::NotADirectory);
    if !matches!(reachable(&volume.root.0), Ok(true)) {
        return false;
    }
    match directory_or_link(path) {
        Ok(true) => return false,
        Ok(false) => {}
        Err(error) if missing(error.kind()) => {}
        Err(_) => return false,
    }
    for ancestor in path.ancestors().skip(1) {
        match reachable(ancestor) {
            Ok(true) => return backing(ancestor).as_ref() == Some(volume),
            Ok(false) => {}
            Err(error) if missing(error.kind()) => {}
            Err(_) => return false,
        }
    }
    false
}

#[cfg(target_os = "linux")]
pub(super) fn for_path(path: &Path) -> Option<Volume> {
    let path = fs::canonicalize(path).ok()?;
    let (root, source, filesystem) = crate::explorer::filesystem::folder_view_mount(&path)?;
    let source = NativePath(source.into());
    let identity = if source.0.starts_with("/dev") {
        let device = fs::canonicalize(&source.0).ok()?;
        let uuid = fs::read_dir("/dev/disk/by-uuid").ok().and_then(|entries| {
            entries.filter_map(Result::ok).find_map(|entry| {
                (fs::canonicalize(entry.path()).ok()? == device)
                    .then(|| UnixIdentity::Uuid(NativePath(entry.file_name().into())))
            })
        });
        Some(uuid.or_else(|| volume_creation(&root))?)
    } else {
        None
    };
    Some(Volume {
        root: NativePath(root),
        source,
        filesystem,
        identity,
    })
}

#[cfg(unix)]
fn volume_creation(root: &Path) -> Option<UnixIdentity> {
    Some(UnixIdentity::Created(
        fs::metadata(root).ok()?.created().ok()?,
    ))
}

#[cfg(target_os = "macos")]
pub(super) fn for_path(path: &Path) -> Option<Volume> {
    use std::{
        ffi::{CStr, CString, OsStr},
        mem::MaybeUninit,
        os::unix::ffi::OsStrExt,
        path::PathBuf,
    };

    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat = MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::statfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return None;
    }
    let stat = unsafe { stat.assume_init() };
    let native_path = |name: &CStr| PathBuf::from(OsStr::from_bytes(name.to_bytes()));
    let root = native_path(unsafe { CStr::from_ptr(stat.f_mntonname.as_ptr()) });
    let source = native_path(unsafe { CStr::from_ptr(stat.f_mntfromname.as_ptr()) });
    let filesystem = unsafe { CStr::from_ptr(stat.f_fstypename.as_ptr()) }
        .to_str()
        .ok()?;
    let identity = if stat.f_flags & libc::MNT_LOCAL as u32 != 0 {
        Some(volume_creation(&root)?)
    } else {
        None
    };
    Some(Volume {
        root: NativePath(root),
        source: NativePath(source),
        filesystem: filesystem.to_owned(),
        identity,
    })
}

#[cfg(windows)]
pub(super) fn for_path(path: &Path) -> Option<Volume> {
    use std::{
        ffi::OsString,
        os::windows::ffi::OsStringExt,
        path::{Component, PathBuf, Prefix},
    };
    use windows::{
        Win32::{
            Foundation::ERROR_SUCCESS,
            NetworkManagement::WNet::WNetGetConnectionW,
            Storage::FileSystem::{
                GetDriveTypeW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
            },
        },
        core::{PCWSTR, PWSTR},
    };

    let path = fs::canonicalize(path).ok()?;
    if let Some(Component::Prefix(prefix)) = path.components().next()
        && let Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _) = prefix.kind()
    {
        let root = NativePath(path.components().take(2).collect());
        return Some(Volume {
            source: root.clone(),
            root,
        });
    }

    let wide_path = crate::os_paths::native_path_wide(&path);
    let mut root = vec![0; 32768];
    unsafe { GetVolumePathNameW(PCWSTR(wide_path.as_ptr()), &mut root) }.ok()?;
    let decode = |units: &[u16]| {
        let end = units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(units.len());
        PathBuf::from(OsString::from_wide(&units[..end]))
    };
    let root_path = decode(&root);
    let mut source = vec![0; 32768];
    if unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } == 4 {
        let Component::Prefix(prefix) = root_path.components().next()? else {
            return None;
        };
        let (Prefix::Disk(letter) | Prefix::VerbatimDisk(letter)) = prefix.kind() else {
            return None;
        };
        let drive = [u16::from(letter), u16::from(b':'), 0];
        let mut size = source.len() as u32;
        if unsafe {
            WNetGetConnectionW(
                PCWSTR(drive.as_ptr()),
                Some(PWSTR(source.as_mut_ptr())),
                &mut size,
            )
        } != ERROR_SUCCESS
        {
            return None;
        }
    } else {
        unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(root.as_ptr()), &mut source) }.ok()?;
    }
    Some(Volume {
        root: NativePath(root_path),
        source: NativePath(decode(&source)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_retains_records_when_backing_is_unavailable_or_changed() {
        let temp = tempfile::tempdir().unwrap();
        let volume = for_path(temp.path()).unwrap();
        let path = temp.path().join("offline");
        let missing = |_: &Path| Err(ErrorKind::NotFound.into());
        assert!(!confirmed_absent_with(
            &path,
            &volume,
            missing,
            missing,
            |_| Some(volume.clone())
        ));
        let denied = |_: &Path| Err(ErrorKind::PermissionDenied.into());
        assert!(!confirmed_absent_with(
            &path,
            &volume,
            |_| Ok(true),
            denied,
            |_| Some(volume.clone())
        ));
        let mut other = volume.clone();
        other.source = NativePath("different-volume".into());
        assert!(!confirmed_absent_with(
            &path,
            &volume,
            |_| Ok(true),
            missing,
            |_| Some(other.clone())
        ));
        assert!(confirmed_absent_with(
            &path,
            &volume,
            |_| Ok(true),
            missing,
            |_| Some(volume.clone())
        ));
    }
}
