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
svg_icon!(NEW_ITEM_ICON, "utility", "new_item.svg");
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
    div()
        .w(px(22.0))
        .h(px(22.0))
        .flex_shrink_0()
        .child(image_icon(FOLDER_ICON.clone(), 22.0, 22.0))
}

pub(super) fn file_icon() -> Div {
    div()
        .w(px(22.0))
        .h(px(22.0))
        .flex_shrink_0()
        .child(image_icon(DOCUMENT_ICON.clone(), 22.0, 22.0))
}

pub(super) fn folder_sidebar_icon() -> Div {
    div()
        .w(px(24.0))
        .h(px(24.0))
        .flex_shrink_0()
        .child(image_icon(FOLDER_ICON.clone(), 24.0, 24.0))
}

pub(super) fn desktop_folder_icon() -> AnyElement {
    image_icon(DESKTOP_SIDEBAR_ICON.clone(), 24.0, 24.0)
}

pub(super) fn documents_folder_icon() -> AnyElement {
    image_icon(DOCUMENTS_SIDEBAR_ICON.clone(), 24.0, 24.0)
}

pub(super) fn downloads_folder_icon() -> AnyElement {
    image_icon(DOWNLOADS_SIDEBAR_ICON.clone(), 24.0, 24.0)
}

pub(super) fn pictures_folder_icon() -> AnyElement {
    image_icon(PICTURES_SIDEBAR_ICON.clone(), 24.0, 24.0)
}

pub(super) fn videos_folder_icon() -> AnyElement {
    image_icon(VIDEOS_SIDEBAR_ICON.clone(), 24.0, 24.0)
}

pub(super) fn music_folder_icon() -> AnyElement {
    image_icon(MUSIC_SIDEBAR_ICON.clone(), 24.0, 24.0)
}

pub(super) fn applications_sidebar_icon() -> AnyElement {
    image_icon(APPLICATIONS_SIDEBAR_ICON.clone(), 24.0, 24.0)
}

pub(super) fn bin_sidebar_icon() -> AnyElement {
    image_icon(BIN_SIDEBAR_ICON.clone(), 24.0, 24.0)
}

pub(super) fn drive_icon() -> AnyElement {
    image_icon(DRIVE_ICON.clone(), 24.0, 24.0)
}

pub(super) fn drive_windows_icon() -> AnyElement {
    image_icon(DRIVE_WINDOWS_ICON.clone(), 24.0, 24.0)
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

pub(super) fn image_icon(image: Arc<Image>, width: f32, height: f32) -> AnyElement {
    img(image)
        .w(px(width))
        .h(px(height))
        .flex_shrink_0()
        .object_fit(ObjectFit::Contain)
        .into_any_element()
}

pub(super) fn directory_shortcut_icon() -> Div {
    div()
        .w(px(22.0))
        .h(px(22.0))
        .flex_shrink_0()
        .child(image_icon(FOLDER_SHORTCUT_ICON.clone(), 22.0, 22.0))
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
        assert_eq!(FILE_ICON_SLOT_WIDTH, 22.0);
        assert_eq!(FILE_ICON_SLOT_HEIGHT, 22.0);
    }

    #[test]
    fn sidebar_image_icons_use_bundled_png_assets() {
        assert!(!APPLICATIONS_SIDEBAR_ICON_BYTES.is_empty());
        assert!(!BIN_SIDEBAR_ICON_BYTES.is_empty());
    }
}
