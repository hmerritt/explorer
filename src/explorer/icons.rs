use crate::explorer::constants::FILE_ICON_SIZE;
use crate::explorer::constants::SIDEBAR_ICON_SIZE;
use std::{
    path::Path,
    sync::{Arc, LazyLock},
};

use crate::explorer::{
    directory_kind::DirectoryKind,
    filesystem::{
        DriveDiscKind, SshfsMountState, WslDistroKind, archive_path_is_supported,
        drive_root_disc_kind, wsl_distro_kind_for_path,
    },
};
use gpui::{
    AnyElement, Div, FontFallbacks, Image, ImageFormat, ObjectFit, StyledImage, div, font, img,
    prelude::*, px,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NavIcon {
    Back,
    Forward,
    Up,
    Refresh,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FileIconKind {
    Archive,
    Audio,
    Configuration,
    Disc,
    Document,
    Executable,
    Font,
    Generic,
    Image,
    MsAccess,
    MsExcel,
    MsPowerpoint,
    MsProject,
    MsWord,
    Program,
    Text,
    Video,
}

// Helper to define SVG icons with consistent naming
macro_rules! svg_icon {
    ($name:ident, $sub_dir:expr, $file:expr) => {
        paste::paste! {
            const [<$name _BYTES>]: &[u8] = include_bytes!(concat!("../../assets/icons/",  $sub_dir, "/", $file));
            pub(super) static $name: LazyLock<Arc<Image>> = LazyLock::new(|| {
                Arc::new(Image::from_bytes(
                    ImageFormat::Svg,
                    [<$name _BYTES>].to_vec(),
                ))
            });
        }
    };
}

fn svg_image(bytes: &[u8]) -> Arc<Image> {
    Arc::new(Image::from_bytes(ImageFormat::Svg, bytes.to_vec()))
}

fn rotate_svg_180(bytes: &[u8]) -> Vec<u8> {
    let source = std::str::from_utf8(bytes).expect("bundled SVG icon is UTF-8");
    source
        .replacen("<path", r#"<g transform="rotate(180 12 12)"><path"#, 1)
        .replacen("</svg>", "</g></svg>", 1)
        .into_bytes()
}

macro_rules! png_icon {
    ($name:ident, $sub_dir:expr, $file:expr) => {
        paste::paste! {
            const [<$name _BYTES>]: &[u8] = include_bytes!(concat!("../../assets/icons/", $sub_dir, "/", $file));
            pub(super) static $name: LazyLock<Arc<Image>> = LazyLock::new(|| {
                Arc::new(Image::from_bytes(
                    ImageFormat::Png,
                    [<$name _BYTES>].to_vec(),
                ))
            });
        }
    };
}

png_icon!(AUDIO_FILE_ICON, "files", "audio.png");
png_icon!(CONFIGURATION_FILE_ICON, "files", "configuration.png");
png_icon!(DISC_FILE_ICON, "files", "disc.png");
png_icon!(DOCUMENT_FILE_ICON, "files", "document.png");
png_icon!(EXECUTABLE_FILE_ICON, "files", "executable.png");
png_icon!(FONT_FILE_ICON, "files", "font.png");
png_icon!(GENERIC_FILE_ICON, "files", "generic.png");
png_icon!(IMAGE_FILE_ICON, "files", "image.png");
png_icon!(MS_ACCESS_FILE_ICON, "files", "ms-access.png");
png_icon!(MS_EXCEL_FILE_ICON, "files", "ms-excel.png");
png_icon!(MS_POWERPOINT_FILE_ICON, "files", "ms-powerpoint.png");
png_icon!(MS_PROJECT_FILE_ICON, "files", "ms-project.png");
png_icon!(MS_WORD_FILE_ICON, "files", "ms-word.png");
png_icon!(PROGRAM_FILE_ICON, "files", "program.png");
png_icon!(TEXT_FILE_ICON, "files", "text.png");
png_icon!(VIDEO_FILE_ICON, "files", "video.png");
png_icon!(LARGE_AUDIO_FILE_ICON, "files/large", "audio.png");
png_icon!(
    LARGE_CONFIGURATION_FILE_ICON,
    "files/large",
    "configuration.png"
);
png_icon!(LARGE_DISC_FILE_ICON, "files/large", "disc.png");
png_icon!(LARGE_DOCUMENT_FILE_ICON, "files/large", "document.png");
png_icon!(LARGE_EXECUTABLE_FILE_ICON, "files/large", "executable.png");
png_icon!(LARGE_FONT_FILE_ICON, "files/large", "font.png");
png_icon!(LARGE_GENERIC_FILE_ICON, "files/large", "generic.png");
png_icon!(LARGE_IMAGE_FILE_ICON, "files/large", "image.png");
png_icon!(LARGE_MS_ACCESS_FILE_ICON, "files/large", "ms-access.png");
png_icon!(LARGE_MS_EXCEL_FILE_ICON, "files/large", "ms-excel.png");
png_icon!(
    LARGE_MS_POWERPOINT_FILE_ICON,
    "files/large",
    "ms-powerpoint.png"
);
png_icon!(LARGE_MS_PROJECT_FILE_ICON, "files/large", "ms-project.png");
png_icon!(LARGE_MS_WORD_FILE_ICON, "files/large", "ms-word.png");
png_icon!(LARGE_PROGRAM_FILE_ICON, "files/large", "program.png");
png_icon!(LARGE_TEXT_FILE_ICON, "files/large", "text.png");
png_icon!(LARGE_VIDEO_FILE_ICON, "files/large", "video.png");
png_icon!(DELETE_FILE_DIALOG_ICON, "files/large", "delete.png");
png_icon!(COPY_FILE_DIALOG_ICON, "files/large", "copy.png");
png_icon!(DELETE_FOLDER_DIALOG_ICON, "folders", "delete.png");
png_icon!(DELETE_MIXED_DIALOG_ICON, "emblems", "alert.png");
png_icon!(FOLDER_ICON, "folders", "folder.png");
png_icon!(FOLDER_SHORTCUT_ICON, "folders", "shortcut.png");
png_icon!(ARCHIVE_FILE_ICON, "folders", "zip.png");

png_icon!(
    APPLICATIONS_SIDEBAR_ICON,
    "sidebar",
    "macos-applications.png"
);
png_icon!(DRIVE_ICON, "devices/drives", "drive.png");
png_icon!(DRIVE_DISC_ICON, "devices/discs", "disc.png");
png_icon!(MOUNT_DISC_ICON, "devices/discs", "disc.png");
png_icon!(NETWORK_DRIVE_ICON, "devices/drives", "network.png");
png_icon!(
    NETWORK_DRIVE_DISCONNECTED_ICON,
    "devices/drives",
    "networkdelete.png"
);
png_icon!(DRIVE_WINDOWS_ICON, "devices/drives", "windows.png");
png_icon!(BLU_RAY_DISC_ICON, "devices/discs", "bd.png");
png_icon!(DVD_DISC_ICON, "devices/discs", "dvd.png");
png_icon!(BIN_SIDEBAR_ICON, "sidebar", "bin.png");
png_icon!(DESKTOP_SIDEBAR_ICON, "sidebar", "desktop.png");
png_icon!(DOCUMENTS_SIDEBAR_ICON, "sidebar", "documents.png");
png_icon!(DOWNLOADS_SIDEBAR_ICON, "sidebar", "downloads.png");
png_icon!(MUSIC_SIDEBAR_ICON, "sidebar", "music.png");
png_icon!(PICTURES_SIDEBAR_ICON, "sidebar", "pictures.png");
png_icon!(VIDEOS_SIDEBAR_ICON, "sidebar", "videos.png");

svg_icon!(COPY_ICON, "utility", "copy.svg");
svg_icon!(COPY_AS_PATH_ICON, "utility", "copy_as_path.svg");
svg_icon!(CUT_ICON, "utility", "cut.svg");
svg_icon!(DELETE_ICON, "utility", "delete.svg");
svg_icon!(DETAILS_ICON, "utility", "details.svg");
svg_icon!(EJECT_ICON, "utility", "eject.svg");
svg_icon!(EXTRACT_ICON, "utility", "extract.svg");
svg_icon!(
    FAVORITE_PIN_REMOVE_ICON,
    "utility",
    "favorite_pin_remove.svg"
);
svg_icon!(HAMBURGER_ICON, "utility", "hamburger.svg");
svg_icon!(GIT_BRANCH_ICON, "git", "git_branch.svg");
svg_icon!(GIT_ICON, "git", "git.svg");
svg_icon!(LARGE_ICONS_ICON, "utility", "large_icons.svg");
svg_icon!(NEW_ITEM_ICON, "utility", "new_item.svg");
svg_icon!(NEW_TAB_ICON, "utility", "new_tab.svg");
svg_icon!(OPEN_WITH_ICON, "utility", "open_with.svg");
svg_icon!(PASTE_ICON, "utility", "paste.svg");
svg_icon!(PROPERTIES_ICON, "utility", "properties.svg");
svg_icon!(RENAME_ICON, "utility", "rename.svg");
svg_icon!(RUN_ELEVATED_ICON, "utility", "run_elevated.svg");
svg_icon!(SETTINGS_ICON, "utility", "settings.svg");
#[cfg(feature = "rclone")]
svg_icon!(SUCCESS_ICON, "utility", "success.svg");
png_icon!(WSL_ALPINE_ICON, "wsl", "alpine.png");
png_icon!(WSL_DEBIAN_ICON, "wsl", "debian.png");
png_icon!(WSL_GENERIC_ICON, "wsl", "generic.png");
png_icon!(WSL_KALI_ICON, "wsl", "kali.png");
png_icon!(WSL_OPENSUSE_ICON, "wsl", "opensuse.png");
png_icon!(WSL_UBUNTU_ICON, "wsl", "ubuntu.png");
const SORT_CHEVRON_UP_ICON_BYTES: &[u8] =
    include_bytes!("../../assets/icons/utility/chevron_up.svg");
pub(super) static SORT_CHEVRON_UP_ICON: LazyLock<Arc<Image>> =
    LazyLock::new(|| svg_image(SORT_CHEVRON_UP_ICON_BYTES));
pub(super) static SORT_CHEVRON_DOWN_ICON: LazyLock<Arc<Image>> =
    LazyLock::new(|| svg_image(&rotate_svg_180(SORT_CHEVRON_UP_ICON_BYTES)));

impl NavIcon {
    pub(super) fn glyph(self) -> &'static str {
        match self {
            Self::Back => "\u{E72B}",
            Self::Forward => "\u{E72A}",
            Self::Up => "\u{E74A}",
            Self::Refresh => "\u{E72C}",
        }
    }
}

impl FileIconKind {
    fn for_path(path: &Path) -> Self {
        if archive_path_is_supported(path) {
            return Self::Archive;
        }

        let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
            return Self::Generic;
        };

        Self::for_extension(&extension.to_ascii_lowercase())
    }

    fn for_extension(extension: &str) -> Self {
        match extension {
            "txt" | "text" | "md" | "markdown" | "log" | "nfo" | "csv" | "tsv" => Self::Text,
            "cfg" | "conf" | "config" | "ini" | "properties" | "reg" | "toml" | "yaml" | "yml"
            | "json" | "json5" | "xml" | "plist" => Self::Configuration,
            "pdf" | "rtf" | "odt" | "ods" | "odp" | "odg" | "odf" | "epub" | "mobi" | "azw"
            | "azw3" | "djvu" | "djv" => Self::Document,
            "mp3" | "wav" | "wave" | "flac" | "aac" | "m4a" | "wma" | "opus" | "oga" | "mid"
            | "midi" | "aif" | "aiff" | "aifc" | "ape" | "amr" | "au" | "snd" | "ac3" | "dts"
            | "ra" => Self::Audio,
            "bmp" | "gif" | "jpg" | "jpeg" | "jpe" | "jfif" | "png" | "apng" | "webp" | "tif"
            | "tiff" | "svg" | "svgz" | "heic" | "heif" | "avif" | "dng" | "cr2" | "cr3"
            | "nef" | "arw" | "orf" | "rw2" | "psd" | "xcf" => Self::Image,
            "webm" | "mkv" | "flv" | "vob" | "ogv" | "ogg" | "rrc" | "gifv" | "mng" | "mov"
            | "avi" | "qt" | "wmv" | "yuv" | "rm" | "asf" | "amv" | "m2ts" | "mp4" | "m4p"
            | "m4v" | "mpg" | "mp2" | "mpeg" | "mpe" | "mpv" | "svi" | "3gp" | "3g2" | "mxf"
            | "roq" | "nsv" | "f4v" | "f4p" | "f4a" | "f4b" => Self::Video,
            "ttf" | "otf" | "woff" | "woff2" | "eot" | "fon" => Self::Font,
            "ico" | "iso" | "img" | "dmg" | "cue" | "nrg" | "toast" | "vhd" | "vhdx" | "vdi"
            | "qcow" | "qcow2" => Self::Disc,
            "msi" | "msix" | "msixbundle" | "appx" | "appxbundle" | "appimage" | "deb" | "rpm"
            | "apk" | "ipa" | "pkg" | "flatpak" | "snap" | "jar" => Self::Program,
            "exe" | "com" | "bat" | "cmd" | "ps1" | "sh" | "bash" | "zsh" | "fish" | "run"
            | "elf" | "scr" | "cpl" | "dll" | "so" | "dylib" | "sys" => Self::Executable,
            "accdb" | "accde" | "accdr" | "accdt" | "accda" | "accdc" | "mdb" | "mde" | "mdw"
            | "adp" | "ade" => Self::MsAccess,
            "xls" | "xlsx" | "xlsm" | "xlsb" | "xlt" | "xltx" | "xltm" | "xla" | "xlam" | "xlw"
            | "xll" => Self::MsExcel,
            "ppt" | "pptx" | "pptm" | "pot" | "potx" | "potm" | "pps" | "ppsx" | "ppsm" | "ppa"
            | "ppam" | "sldx" | "sldm" => Self::MsPowerpoint,
            "mpp" | "mpt" | "mpd" | "mpx" => Self::MsProject,
            "doc" | "docx" | "docm" | "dot" | "dotx" | "dotm" | "wbk" => Self::MsWord,
            _ => Self::Generic,
        }
    }

    fn is_media(self) -> bool {
        matches!(self, Self::Image | Self::Video)
    }

    fn image(self, large: bool) -> Arc<Image> {
        match self {
            Self::Archive => ARCHIVE_FILE_ICON.clone(),
            Self::Audio if large => LARGE_AUDIO_FILE_ICON.clone(),
            Self::Audio => AUDIO_FILE_ICON.clone(),
            Self::Configuration if large => LARGE_CONFIGURATION_FILE_ICON.clone(),
            Self::Configuration => CONFIGURATION_FILE_ICON.clone(),
            Self::Disc if large => LARGE_DISC_FILE_ICON.clone(),
            Self::Disc => DISC_FILE_ICON.clone(),
            Self::Document if large => LARGE_DOCUMENT_FILE_ICON.clone(),
            Self::Document => DOCUMENT_FILE_ICON.clone(),
            Self::Executable if large => LARGE_EXECUTABLE_FILE_ICON.clone(),
            Self::Executable => EXECUTABLE_FILE_ICON.clone(),
            Self::Font if large => LARGE_FONT_FILE_ICON.clone(),
            Self::Font => FONT_FILE_ICON.clone(),
            Self::Generic if large => LARGE_GENERIC_FILE_ICON.clone(),
            Self::Generic => GENERIC_FILE_ICON.clone(),
            Self::Image if large => LARGE_IMAGE_FILE_ICON.clone(),
            Self::Image => IMAGE_FILE_ICON.clone(),
            Self::MsAccess if large => LARGE_MS_ACCESS_FILE_ICON.clone(),
            Self::MsAccess => MS_ACCESS_FILE_ICON.clone(),
            Self::MsExcel if large => LARGE_MS_EXCEL_FILE_ICON.clone(),
            Self::MsExcel => MS_EXCEL_FILE_ICON.clone(),
            Self::MsPowerpoint if large => LARGE_MS_POWERPOINT_FILE_ICON.clone(),
            Self::MsPowerpoint => MS_POWERPOINT_FILE_ICON.clone(),
            Self::MsProject if large => LARGE_MS_PROJECT_FILE_ICON.clone(),
            Self::MsProject => MS_PROJECT_FILE_ICON.clone(),
            Self::MsWord if large => LARGE_MS_WORD_FILE_ICON.clone(),
            Self::MsWord => MS_WORD_FILE_ICON.clone(),
            Self::Program if large => LARGE_PROGRAM_FILE_ICON.clone(),
            Self::Program => PROGRAM_FILE_ICON.clone(),
            Self::Text if large => LARGE_TEXT_FILE_ICON.clone(),
            Self::Text => TEXT_FILE_ICON.clone(),
            Self::Video if large => LARGE_VIDEO_FILE_ICON.clone(),
            Self::Video => VIDEO_FILE_ICON.clone(),
        }
    }
}

pub(super) fn nav_icon_font() -> gpui::Font {
    let mut font = font("Segoe Fluent Icons");
    font.fallbacks = Some(FontFallbacks::from_fonts(vec![
        "Segoe MDL2 Assets".to_owned(),
    ]));
    font
}

pub(super) fn folder_icon() -> Div {
    folder_icon_sized(FILE_ICON_SIZE)
}

pub(super) fn folder_icon_sized(size: f32) -> Div {
    div()
        .w(px(size))
        .h(px(size))
        .flex_shrink_0()
        .child(image_icon(FOLDER_ICON.clone(), size, size))
}

pub(super) fn directory_shortcut_icon() -> Div {
    directory_shortcut_icon_sized(FILE_ICON_SIZE)
}

pub(super) fn directory_shortcut_icon_sized(size: f32) -> Div {
    div()
        .w(px(size))
        .h(px(size))
        .flex_shrink_0()
        .child(image_icon(FOLDER_SHORTCUT_ICON.clone(), size, size))
}

pub(super) fn file_icon() -> Div {
    file_icon_sized(FILE_ICON_SIZE)
}

pub(super) fn file_icon_sized(size: f32) -> Div {
    sized_file_icon(FileIconKind::Generic, size)
}

pub(super) fn executable_icon_sized(size: f32) -> Div {
    sized_file_icon(FileIconKind::Executable, size)
}

pub(super) fn file_icon_for_path(path: &Path) -> Div {
    file_icon_for_path_sized(path, FILE_ICON_SIZE)
}

pub(super) fn file_icon_for_path_sized(path: &Path, size: f32) -> Div {
    sized_file_icon(FileIconKind::for_path(path), size)
}

pub(super) fn file_path_counts_for_media_view(path: &Path) -> bool {
    FileIconKind::for_path(path).is_media()
        || path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("ico"))
}

pub(super) fn large_file_icon_for_path_sized(path: &Path, size: f32) -> Div {
    sized_large_file_icon(FileIconKind::for_path(path), size)
}

pub(super) fn copy_file_dialog_icon_sized(size: f32) -> Div {
    div()
        .w(px(size))
        .h(px(size))
        .flex_shrink_0()
        .child(image_icon(COPY_FILE_DIALOG_ICON.clone(), size, size))
}

fn sized_file_icon(kind: FileIconKind, size: f32) -> Div {
    div()
        .w(px(size))
        .h(px(size))
        .flex_shrink_0()
        .child(image_icon(kind.image(false), size, size))
}

fn sized_large_file_icon(kind: FileIconKind, size: f32) -> Div {
    div()
        .w(px(size))
        .h(px(size))
        .flex_shrink_0()
        .child(image_icon(kind.image(true), size, size))
}

pub(super) fn folder_sidebar_icon() -> AnyElement {
    image_sidebar_icon(FOLDER_ICON.clone())
}

pub(super) fn desktop_folder_icon() -> AnyElement {
    image_sidebar_icon(DESKTOP_SIDEBAR_ICON.clone())
}

pub(super) fn documents_folder_icon() -> AnyElement {
    image_sidebar_icon(DOCUMENTS_SIDEBAR_ICON.clone())
}

pub(super) fn downloads_folder_icon() -> AnyElement {
    image_sidebar_icon(DOWNLOADS_SIDEBAR_ICON.clone())
}

pub(super) fn pictures_folder_icon() -> AnyElement {
    image_sidebar_icon(PICTURES_SIDEBAR_ICON.clone())
}

pub(super) fn videos_folder_icon() -> AnyElement {
    image_sidebar_icon(VIDEOS_SIDEBAR_ICON.clone())
}

pub(super) fn music_folder_icon() -> AnyElement {
    image_sidebar_icon(MUSIC_SIDEBAR_ICON.clone())
}

pub(super) fn applications_sidebar_icon() -> AnyElement {
    image_sidebar_icon(APPLICATIONS_SIDEBAR_ICON.clone())
}

pub(super) fn bin_sidebar_icon() -> AnyElement {
    image_sidebar_icon(BIN_SIDEBAR_ICON.clone())
}

pub(super) fn drive_icon() -> AnyElement {
    image_sidebar_icon(DRIVE_ICON.clone())
}

pub(super) fn drive_disc_icon_for_path(path: &Path) -> AnyElement {
    image_sidebar_icon(drive_disc_image_for_path(path))
}

pub(super) fn drive_disc_icon_sized_for_path(path: &Path, size: f32) -> AnyElement {
    image_icon(drive_disc_image_for_path(path), size, size)
}

fn drive_disc_image_for_path(path: &Path) -> Arc<Image> {
    drive_root_disc_kind(path)
        .map(drive_disc_image_for_kind)
        .unwrap_or_else(|| DRIVE_DISC_ICON.clone())
}

fn drive_disc_image_for_kind(kind: DriveDiscKind) -> Arc<Image> {
    match kind {
        DriveDiscKind::BluRay => BLU_RAY_DISC_ICON.clone(),
        DriveDiscKind::Dvd => DVD_DISC_ICON.clone(),
    }
}

pub(super) fn drive_windows_icon() -> AnyElement {
    image_sidebar_icon(DRIVE_WINDOWS_ICON.clone())
}

pub(super) fn drive_wsl_icon() -> AnyElement {
    image_sidebar_icon(wsl_distro_image(WslDistroKind::Generic))
}

pub(super) fn drive_wsl_icon_for_path(path: &Path) -> AnyElement {
    let kind = wsl_distro_kind_for_path(path).unwrap_or(WslDistroKind::Generic);
    image_sidebar_icon(wsl_distro_image(kind))
}

pub(super) fn drive_wsl_icon_sized_for_path(path: &Path, size: f32) -> AnyElement {
    let kind = wsl_distro_kind_for_path(path).unwrap_or(WslDistroKind::Generic);
    image_icon(wsl_distro_image(kind), size, size)
}

pub(super) fn sshfs_drive_icon(state: SshfsMountState) -> AnyElement {
    image_sidebar_icon(sshfs_drive_image_for_state(state))
}

fn sshfs_drive_image_for_state(state: SshfsMountState) -> Arc<Image> {
    match state {
        SshfsMountState::Connected => NETWORK_DRIVE_ICON.clone(),
        SshfsMountState::Disconnected => NETWORK_DRIVE_DISCONNECTED_ICON.clone(),
    }
}

#[cfg(feature = "rclone")]
pub(super) fn rclone_drive_icon(state: crate::explorer::rclone::RcloneSidebarState) -> AnyElement {
    image_sidebar_icon(rclone_drive_image_for_state(state))
}

#[cfg(feature = "rclone")]
fn rclone_drive_image_for_state(state: crate::explorer::rclone::RcloneSidebarState) -> Arc<Image> {
    match state {
        crate::explorer::rclone::RcloneSidebarState::Mounted => NETWORK_DRIVE_ICON.clone(),
        _ => NETWORK_DRIVE_DISCONNECTED_ICON.clone(),
    }
}

fn wsl_distro_image(kind: WslDistroKind) -> Arc<Image> {
    match kind {
        WslDistroKind::Alpine => WSL_ALPINE_ICON.clone(),
        WslDistroKind::Debian => WSL_DEBIAN_ICON.clone(),
        WslDistroKind::Generic => WSL_GENERIC_ICON.clone(),
        WslDistroKind::Kali => WSL_KALI_ICON.clone(),
        WslDistroKind::OpenSuse => WSL_OPENSUSE_ICON.clone(),
        WslDistroKind::Ubuntu => WSL_UBUNTU_ICON.clone(),
    }
}

pub(super) fn directory_kind_icon(kind: DirectoryKind) -> AnyElement {
    match kind {
        DirectoryKind::Home => folder_sidebar_icon().into_any_element(),
        DirectoryKind::Desktop => desktop_folder_icon(),
        DirectoryKind::Documents => documents_folder_icon(),
        DirectoryKind::Downloads => downloads_folder_icon(),
        DirectoryKind::Pictures => pictures_folder_icon(),
        DirectoryKind::Music => music_folder_icon(),
        DirectoryKind::Videos => videos_folder_icon(),
        DirectoryKind::Applications => applications_sidebar_icon(),
        DirectoryKind::Bin => bin_sidebar_icon(),
        DirectoryKind::Drive => drive_icon(),
        DirectoryKind::DriveWindows => drive_windows_icon(),
        DirectoryKind::DriveWsl => drive_wsl_icon(),
    }
}

pub(super) fn directory_kind_icon_sized(kind: DirectoryKind, size: f32) -> AnyElement {
    let image = match kind {
        DirectoryKind::Home => FOLDER_ICON.clone(),
        DirectoryKind::Desktop => DESKTOP_SIDEBAR_ICON.clone(),
        DirectoryKind::Documents => DOCUMENTS_SIDEBAR_ICON.clone(),
        DirectoryKind::Downloads => DOWNLOADS_SIDEBAR_ICON.clone(),
        DirectoryKind::Pictures => PICTURES_SIDEBAR_ICON.clone(),
        DirectoryKind::Music => MUSIC_SIDEBAR_ICON.clone(),
        DirectoryKind::Videos => VIDEOS_SIDEBAR_ICON.clone(),
        DirectoryKind::Applications => APPLICATIONS_SIDEBAR_ICON.clone(),
        DirectoryKind::Bin => BIN_SIDEBAR_ICON.clone(),
        DirectoryKind::Drive => DRIVE_ICON.clone(),
        DirectoryKind::DriveWindows => DRIVE_WINDOWS_ICON.clone(),
        DirectoryKind::DriveWsl => WSL_GENERIC_ICON.clone(),
    };
    image_icon(image, size, size)
}

pub(super) fn image_icon(image: Arc<Image>, width: f32, height: f32) -> AnyElement {
    img(image)
        .w(px(width))
        .h(px(height))
        .flex_shrink_0()
        .object_fit(ObjectFit::Contain)
        .into_any_element()
}

pub(super) fn image_sidebar_icon(image: Arc<Image>) -> AnyElement {
    image_icon(image, SIDEBAR_ICON_SIZE, SIDEBAR_ICON_SIZE)
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::explorer::constants::{
        FILE_ICON_SLOT_HEIGHT, FILE_ICON_SLOT_WIDTH, NAV_ICON_TEXT_SIZE,
    };

    #[test]
    fn nav_icons_use_windows_explorer_glyphs() {
        assert_eq!(NavIcon::Back.glyph(), "\u{E72B}");
        assert_eq!(NavIcon::Forward.glyph(), "\u{E72A}");
        assert_eq!(NavIcon::Up.glyph(), "\u{E74A}");
        assert_eq!(NavIcon::Refresh.glyph(), "\u{E72C}");
    }

    #[test]
    fn nav_icon_size_is_logical_and_scale_independent() {
        assert_eq!(NAV_ICON_TEXT_SIZE, 12.0);
    }

    #[test]
    fn drive_icon_uses_fixed_explorer_list_slot() {
        assert_eq!(FILE_ICON_SLOT_WIDTH, 16.0);
        assert_eq!(FILE_ICON_SLOT_HEIGHT, 16.0);
    }

    #[test]
    fn sidebar_image_icons_use_bundled_png_assets() {
        assert!(!APPLICATIONS_SIDEBAR_ICON_BYTES.is_empty());
        assert!(!BIN_SIDEBAR_ICON_BYTES.is_empty());
        assert!(!DRIVE_DISC_ICON_BYTES.is_empty());
        assert!(!BLU_RAY_DISC_ICON_BYTES.is_empty());
        assert!(!DVD_DISC_ICON_BYTES.is_empty());
    }

    #[test]
    fn network_drive_png_assets_load_as_png_images() {
        assert_eq!(NETWORK_DRIVE_ICON.format, ImageFormat::Png);
        assert_eq!(NETWORK_DRIVE_DISCONNECTED_ICON.format, ImageFormat::Png);
        assert!(!NETWORK_DRIVE_ICON.bytes.is_empty());
        assert!(!NETWORK_DRIVE_DISCONNECTED_ICON.bytes.is_empty());
    }

    #[test]
    fn sshfs_drive_icons_use_connected_image_only_for_connected_state() {
        assert_eq!(
            sshfs_drive_image_for_state(SshfsMountState::Connected).id(),
            NETWORK_DRIVE_ICON.clone().id()
        );
        assert_eq!(
            sshfs_drive_image_for_state(SshfsMountState::Disconnected).id(),
            NETWORK_DRIVE_DISCONNECTED_ICON.clone().id()
        );
    }

    #[cfg(feature = "rclone")]
    #[test]
    fn rclone_drive_icons_use_connected_image_only_for_mounted_state() {
        use crate::explorer::rclone::RcloneSidebarState;

        assert_eq!(
            rclone_drive_image_for_state(RcloneSidebarState::Mounted).id(),
            NETWORK_DRIVE_ICON.clone().id()
        );
        for state in [
            RcloneSidebarState::Disconnected,
            RcloneSidebarState::Connecting,
            RcloneSidebarState::TransferMode,
            RcloneSidebarState::Error,
        ] {
            assert_eq!(
                rclone_drive_image_for_state(state).id(),
                NETWORK_DRIVE_DISCONNECTED_ICON.clone().id()
            );
        }
    }

    #[test]
    fn optical_drive_png_assets_load_as_png_images() {
        assert_eq!(BLU_RAY_DISC_ICON.format, ImageFormat::Png);
        assert_eq!(DVD_DISC_ICON.format, ImageFormat::Png);
        assert!(!BLU_RAY_DISC_ICON.bytes.is_empty());
        assert!(!DVD_DISC_ICON.bytes.is_empty());
    }

    #[test]
    fn optical_drive_disc_icons_use_specific_images_with_generic_fallback() {
        assert_ne!(
            drive_disc_image_for_kind(DriveDiscKind::BluRay).id(),
            drive_disc_image_for_kind(DriveDiscKind::Dvd).id()
        );
        assert_eq!(
            drive_disc_image_for_path(Path::new("not-a-drive-root")).id(),
            DRIVE_DISC_ICON.clone().id()
        );
    }

    #[test]
    fn utility_icons_use_bundled_svg_assets() {
        assert!(!EJECT_ICON_BYTES.is_empty());
        assert!(!SETTINGS_ICON_BYTES.is_empty());
        #[cfg(feature = "rclone")]
        assert!(!SUCCESS_ICON_BYTES.is_empty());
    }

    #[test]
    fn dialog_delete_icons_use_bundled_png_assets() {
        assert!(!DELETE_FILE_DIALOG_ICON_BYTES.is_empty());
        assert!(!COPY_FILE_DIALOG_ICON_BYTES.is_empty());
        assert!(!DELETE_FOLDER_DIALOG_ICON_BYTES.is_empty());
        assert!(!DELETE_MIXED_DIALOG_ICON_BYTES.is_empty());
    }

    #[test]
    fn large_file_icons_use_bundled_png_assets() {
        assert!(!LARGE_AUDIO_FILE_ICON_BYTES.is_empty());
        assert!(!LARGE_CONFIGURATION_FILE_ICON_BYTES.is_empty());
        assert!(!LARGE_DISC_FILE_ICON_BYTES.is_empty());
        assert!(!LARGE_DOCUMENT_FILE_ICON_BYTES.is_empty());
        assert!(!LARGE_EXECUTABLE_FILE_ICON_BYTES.is_empty());
        assert!(!LARGE_FONT_FILE_ICON_BYTES.is_empty());
        assert!(!LARGE_GENERIC_FILE_ICON_BYTES.is_empty());
        assert!(!LARGE_IMAGE_FILE_ICON_BYTES.is_empty());
        assert!(!LARGE_MS_ACCESS_FILE_ICON_BYTES.is_empty());
        assert!(!LARGE_MS_EXCEL_FILE_ICON_BYTES.is_empty());
        assert!(!LARGE_MS_POWERPOINT_FILE_ICON_BYTES.is_empty());
        assert!(!LARGE_MS_PROJECT_FILE_ICON_BYTES.is_empty());
        assert!(!LARGE_MS_WORD_FILE_ICON_BYTES.is_empty());
        assert!(!LARGE_PROGRAM_FILE_ICON_BYTES.is_empty());
        assert!(!LARGE_TEXT_FILE_ICON_BYTES.is_empty());
        assert!(!LARGE_VIDEO_FILE_ICON_BYTES.is_empty());
    }

    #[test]
    fn sort_chevron_icons_use_bundled_svg_asset() {
        assert!(
            std::str::from_utf8(SORT_CHEVRON_UP_ICON_BYTES)
                .expect("chevron SVG is UTF-8")
                .contains("M12 9.5 L6.5 15.5 H17.5 Z")
        );
        assert!(
            std::str::from_utf8(&rotate_svg_180(SORT_CHEVRON_UP_ICON_BYTES))
                .expect("rotated chevron SVG is UTF-8")
                .contains(r#"transform="rotate(180 12 12)""#)
        );
    }

    #[test]
    fn wsl_drive_icons_use_bundled_svg_assets() {
        assert!(!WSL_ALPINE_ICON_BYTES.is_empty());
        assert!(!WSL_DEBIAN_ICON_BYTES.is_empty());
        assert!(!WSL_GENERIC_ICON_BYTES.is_empty());
        assert!(!WSL_KALI_ICON_BYTES.is_empty());
        assert!(!WSL_OPENSUSE_ICON_BYTES.is_empty());
        assert!(!WSL_UBUNTU_ICON_BYTES.is_empty());
    }

    #[test]
    fn file_icon_extensions_map_to_expected_kinds() {
        let mappings = [
            (FileIconKind::Text, "txt text md markdown log nfo csv tsv"),
            (
                FileIconKind::Configuration,
                "cfg conf config ini properties reg toml yaml yml json json5 xml plist",
            ),
            (
                FileIconKind::Document,
                "pdf rtf odt ods odp odg odf epub mobi azw azw3 djvu djv",
            ),
            (
                FileIconKind::Audio,
                "mp3 wav wave flac aac m4a wma opus oga mid midi aif aiff aifc ape amr au snd ac3 dts ra",
            ),
            (
                FileIconKind::Image,
                "bmp gif jpg jpeg jpe jfif png apng webp tif tiff svg svgz heic heif avif dng cr2 cr3 nef arw orf rw2 psd xcf",
            ),
            (
                FileIconKind::Video,
                "webm mkv flv vob ogv ogg rrc gifv mng mov avi qt wmv yuv rm asf amv m2ts mp4 m4p m4v mpg mp2 mpeg mpe mpv svi 3gp 3g2 mxf roq nsv f4v f4p f4a f4b",
            ),
            (FileIconKind::Font, "ttf otf woff woff2 eot fon"),
            (
                FileIconKind::Disc,
                "ico iso img dmg cue nrg toast vhd vhdx vdi qcow qcow2",
            ),
            (
                FileIconKind::Program,
                "msi msix msixbundle appx appxbundle appimage deb rpm apk ipa pkg flatpak snap jar",
            ),
            (
                FileIconKind::Executable,
                "exe com bat cmd ps1 sh bash zsh fish run elf scr cpl dll so dylib sys",
            ),
            (
                FileIconKind::MsAccess,
                "accdb accde accdr accdt accda accdc mdb mde mdw adp ade",
            ),
            (
                FileIconKind::MsExcel,
                "xls xlsx xlsm xlsb xlt xltx xltm xla xlam xlw xll",
            ),
            (
                FileIconKind::MsPowerpoint,
                "ppt pptx pptm pot potx potm pps ppsx ppsm ppa ppam sldx sldm",
            ),
            (FileIconKind::MsProject, "mpp mpt mpd mpx"),
            (FileIconKind::MsWord, "doc docx docm dot dotx dotm wbk"),
        ];

        for (expected, extensions) in mappings {
            for extension in extensions.split_ascii_whitespace() {
                assert_eq!(
                    FileIconKind::for_extension(extension),
                    expected,
                    "unexpected icon for .{extension}"
                );
            }
        }
    }

    #[test]
    fn supported_archives_use_archive_icon() {
        let archives = [
            "zip", "tar", "tgz", "tbz", "txz", "tzst", "ar", "gz", "bz", "bz2", "xz", "zst", "rar",
            "7z", "tar.gz", "tar.bz2", "tar.xz", "tar.zst",
        ];

        for extension in archives {
            let path = format!("archive.{extension}");
            assert_eq!(
                FileIconKind::for_path(Path::new(&path)),
                FileIconKind::Archive,
                "unexpected icon for {path}"
            );
        }
    }

    #[test]
    fn archive_icon_mapping_is_case_insensitive() {
        assert_eq!(
            FileIconKind::for_path(Path::new("ARCHIVE.ZIP")),
            FileIconKind::Archive
        );
        assert_eq!(
            FileIconKind::for_path(Path::new("ARCHIVE.TAR.GZ")),
            FileIconKind::Archive
        );
    }

    #[test]
    fn package_formats_remain_program_icons() {
        for extension in ["jar", "deb", "rpm", "apk", "ipa", "pkg", "flatpak", "snap"] {
            let path = format!("package.{extension}");
            assert_eq!(
                FileIconKind::for_path(Path::new(&path)),
                FileIconKind::Program,
                "unexpected icon for {path}"
            );
        }
    }

    #[test]
    fn archive_like_names_without_supported_suffix_remain_generic() {
        for path in ["zip", ".zip", "archive.zip.backup", "archive.tar.gz.backup"] {
            assert_eq!(
                FileIconKind::for_path(Path::new(path)),
                FileIconKind::Generic,
                "expected generic icon for {path}"
            );
        }
    }

    #[test]
    fn file_icon_mapping_is_case_insensitive() {
        assert_eq!(
            FileIconKind::for_path(Path::new("Quarterly Report.XLSX")),
            FileIconKind::MsExcel
        );
        assert_eq!(
            FileIconKind::for_path(Path::new("MOVIE.MP4")),
            FileIconKind::Video
        );
    }

    #[test]
    fn file_icon_mapping_keeps_intentional_conflicts() {
        assert_eq!(
            FileIconKind::for_path(Path::new("sound.ogg")),
            FileIconKind::Video
        );
        assert_eq!(
            FileIconKind::for_path(Path::new("favicon.ico")),
            FileIconKind::Disc
        );
        assert_eq!(
            FileIconKind::for_path(Path::new("data.csv")),
            FileIconKind::Text
        );
    }

    #[test]
    fn media_view_file_detection_includes_ico_case_insensitively() {
        assert!(file_path_counts_for_media_view(Path::new("photo.jpg")));
        assert!(file_path_counts_for_media_view(Path::new("movie.mp4")));
        assert!(file_path_counts_for_media_view(Path::new("favicon.ico")));
        assert!(file_path_counts_for_media_view(Path::new("favicon.ICO")));
        assert!(!file_path_counts_for_media_view(Path::new("image.iso")));
        assert!(!file_path_counts_for_media_view(Path::new("notes.txt")));
    }

    #[test]
    fn file_icon_mapping_falls_back_to_generic() {
        for path in ["source.rs", "unknown.thing", "README", ".env"] {
            assert_eq!(
                FileIconKind::for_path(Path::new(path)),
                FileIconKind::Generic,
                "expected generic icon for {path}"
            );
        }
    }
}
