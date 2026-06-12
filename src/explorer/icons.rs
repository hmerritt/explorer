use crate::explorer::constants::FILE_ICON_SIZE;
use crate::explorer::constants::SIDEBAR_ICON_SIZE;
use std::sync::{Arc, LazyLock};

use crate::explorer::directory_kind::DirectoryKind;
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

png_icon!(DOCUMENT_ICON, "files", "generic.png");
png_icon!(DELETE_FILE_DIALOG_ICON, "files/large", "delete.png");
png_icon!(DELETE_FOLDER_DIALOG_ICON, "folders", "delete.png");
png_icon!(DELETE_MIXED_DIALOG_ICON, "emblems", "alert.png");
png_icon!(FOLDER_ICON, "folders", "folder.png");
png_icon!(FOLDER_SHORTCUT_ICON, "folders", "shortcut.png");

png_icon!(
    APPLICATIONS_SIDEBAR_ICON,
    "sidebar",
    "macos-applications.png"
);
png_icon!(DRIVE_ICON, "devices/drives", "drive.png");
png_icon!(DRIVE_WINDOWS_ICON, "devices/drives", "windows.png");
png_icon!(BIN_SIDEBAR_ICON, "sidebar", "bin.png");
png_icon!(DESKTOP_SIDEBAR_ICON, "sidebar", "desktop.png");
png_icon!(DOCUMENTS_SIDEBAR_ICON, "sidebar", "documents.png");
png_icon!(DOWNLOADS_SIDEBAR_ICON, "sidebar", "downloads.png");
png_icon!(MUSIC_SIDEBAR_ICON, "sidebar", "music.png");
png_icon!(PICTURES_SIDEBAR_ICON, "sidebar", "pictures.png");
png_icon!(VIDEOS_SIDEBAR_ICON, "sidebar", "videos.png");

svg_icon!(COPY_ICON, "utility", "copy.svg");
svg_icon!(CUT_ICON, "utility", "cut.svg");
svg_icon!(DELETE_ICON, "utility", "delete.svg");
svg_icon!(DETAILS_ICON, "utility", "details.svg");
svg_icon!(EXTRACT_ICON, "utility", "extract.svg");
svg_icon!(
    FAVORITE_PIN_REMOVE_ICON,
    "utility",
    "favorite_pin_remove.svg"
);
svg_icon!(NEW_ITEM_ICON, "utility", "new_item.svg");
svg_icon!(NEW_TAB_ICON, "utility", "new_tab.svg");
svg_icon!(PASTE_ICON, "utility", "paste.svg");
svg_icon!(RENAME_ICON, "utility", "rename.svg");

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
    div()
        .w(px(FILE_ICON_SIZE))
        .h(px(FILE_ICON_SIZE))
        .flex_shrink_0()
        .child(image_icon(
            FOLDER_SHORTCUT_ICON.clone(),
            FILE_ICON_SIZE,
            FILE_ICON_SIZE,
        ))
}

pub(super) fn file_icon() -> Div {
    file_icon_sized(FILE_ICON_SIZE)
}

pub(super) fn file_icon_sized(size: f32) -> Div {
    div()
        .w(px(size))
        .h(px(size))
        .flex_shrink_0()
        .child(image_icon(DOCUMENT_ICON.clone(), size, size))
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

pub(super) fn drive_windows_icon() -> AnyElement {
    image_sidebar_icon(DRIVE_WINDOWS_ICON.clone())
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
    }

    #[test]
    fn dialog_delete_icons_use_bundled_png_assets() {
        assert!(!DELETE_FILE_DIALOG_ICON_BYTES.is_empty());
        assert!(!DELETE_FOLDER_DIALOG_ICON_BYTES.is_empty());
        assert!(!DELETE_MIXED_DIALOG_ICON_BYTES.is_empty());
    }
}
