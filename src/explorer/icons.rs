use std::sync::{Arc, LazyLock};

use gpui::{
    AnyElement, Div, FontFallbacks, Image, ImageFormat, ObjectFit, Pixels, StyledImage, div, font,
    img, prelude::*, px, rgb,
};

use crate::explorer::constants::{FILE_ICON_SLOT_HEIGHT_PHYSICAL, FILE_ICON_SLOT_WIDTH_PHYSICAL};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NavIcon {
    Back,
    Forward,
    Up,
    Refresh,
}

const FILE_ICON_FALLBACK_GLYPH: &str = "\u{E8A5}";
const FILE_ICON_FALLBACK_ICON_SIZE_PHYSICAL: f32 = 20.0;
const FILE_ICON_FALLBACK_ICON_COLOR: u32 = 0x9a9a9a;
const DOWNLOADS_FOLDER_FALLBACK_GLYPH: &str = "\u{E896}";
const DOWNLOADS_FOLDER_FALLBACK_ICON_SIZE_PHYSICAL: f32 = 18.0;
const DOWNLOADS_FOLDER_FALLBACK_ICON_COLOR: u32 = 0x10893e;
const DOCUMENTS_FOLDER_FALLBACK_SLOT_WIDTH_PHYSICAL: f32 = 22.0;
const DOCUMENTS_FOLDER_FALLBACK_SLOT_HEIGHT_PHYSICAL: f32 = 20.0;
const DOCUMENTS_FOLDER_FALLBACK_PAGE_LEFT_PHYSICAL: f32 = 4.0;
const DOCUMENTS_FOLDER_FALLBACK_PAGE_TOP_PHYSICAL: f32 = 1.0;
const DOCUMENTS_FOLDER_FALLBACK_PAGE_WIDTH_PHYSICAL: f32 = 14.0;
const DOCUMENTS_FOLDER_FALLBACK_PAGE_HEIGHT_PHYSICAL: f32 = 18.0;
const DOCUMENTS_FOLDER_FALLBACK_PAGE_COLOR: u32 = 0x7897b6;
const DOCUMENTS_FOLDER_FALLBACK_LINE_COLOR: u32 = 0xffffff;
const DESKTOP_FOLDER_FALLBACK_SLOT_WIDTH_PHYSICAL: f32 = 22.0;
const DESKTOP_FOLDER_FALLBACK_SLOT_HEIGHT_PHYSICAL: f32 = 20.0;
const DESKTOP_FOLDER_FALLBACK_SCREEN_WIDTH_PHYSICAL: f32 = 20.0;
const DESKTOP_FOLDER_FALLBACK_SCREEN_HEIGHT_PHYSICAL: f32 = 15.0;
const DESKTOP_FOLDER_FALLBACK_SCREEN_COLOR: u32 = 0x2aa9d8;
const DESKTOP_FOLDER_FALLBACK_SCREEN_HIGHLIGHT_COLOR: u32 = 0x86f2ee;
const DESKTOP_FOLDER_FALLBACK_SCREEN_BOTTOM_COLOR: u32 = 0x1381a8;
const DESKTOP_FOLDER_FALLBACK_DETAIL_WIDTH_PHYSICAL: f32 = 2.0;
const DESKTOP_FOLDER_FALLBACK_DETAIL_HEIGHT_PHYSICAL: f32 = 3.0;
const DESKTOP_FOLDER_FALLBACK_DETAIL_LIGHT_COLOR: u32 = 0xe5ffff;
const DESKTOP_FOLDER_FALLBACK_DETAIL_MID_COLOR: u32 = 0x70d7d7;
const APPLICATIONS_SIDEBAR_ICON_BYTES: &[u8] =
    include_bytes!("../../assets/icons/macos-applications.png");
const BIN_SIDEBAR_ICON_BYTES: &[u8] = include_bytes!("../../assets/icons/bin.png");

static APPLICATIONS_SIDEBAR_ICON: LazyLock<Arc<Image>> = LazyLock::new(|| {
    Arc::new(Image::from_bytes(
        ImageFormat::Png,
        APPLICATIONS_SIDEBAR_ICON_BYTES.to_vec(),
    ))
});
static BIN_SIDEBAR_ICON: LazyLock<Arc<Image>> = LazyLock::new(|| {
    Arc::new(Image::from_bytes(
        ImageFormat::Png,
        BIN_SIDEBAR_ICON_BYTES.to_vec(),
    ))
});

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

pub(super) fn device_px(value: f32, scale_factor: f32) -> Pixels {
    px(device_px_value(value, scale_factor))
}

pub(super) fn device_px_value(value: f32, scale_factor: f32) -> f32 {
    if scale_factor <= 0.0 {
        value
    } else {
        value / scale_factor
    }
}

pub(super) fn nav_icon_font() -> gpui::Font {
    let mut font = font("Segoe Fluent Icons");
    font.fallbacks = Some(FontFallbacks::from_fonts(vec![
        "Segoe MDL2 Assets".to_owned(),
    ]));
    font
}

pub(super) fn folder_icon(scale_factor: f32) -> Div {
    div()
        .relative()
        .w(device_px(22.0, scale_factor))
        .h(device_px(17.0, scale_factor))
        .flex_shrink_0()
        .child(
            div()
                .absolute()
                .left(device_px(1.0, scale_factor))
                .top(device_px(0.0, scale_factor))
                .w(device_px(9.0, scale_factor))
                .h(device_px(5.0, scale_factor))
                .bg(rgb(0xf5c242)),
        )
        .child(
            div()
                .absolute()
                .left(device_px(0.0, scale_factor))
                .top(device_px(4.0, scale_factor))
                .w(device_px(22.0, scale_factor))
                .h(device_px(13.0, scale_factor))
                .bg(rgb(0xffcc4d)),
        )
        .child(
            div()
                .absolute()
                .left(device_px(0.0, scale_factor))
                .top(device_px(14.0, scale_factor))
                .w(device_px(22.0, scale_factor))
                .h(device_px(3.0, scale_factor))
                .bg(rgb(0xf3b839)),
        )
}

pub(super) fn desktop_folder_icon(scale_factor: f32) -> AnyElement {
    desktop_folder_fallback_icon(scale_factor).into_any_element()
}

pub(super) fn documents_folder_icon(scale_factor: f32) -> AnyElement {
    documents_folder_fallback_icon(scale_factor).into_any_element()
}

pub(super) fn downloads_folder_icon(scale_factor: f32) -> AnyElement {
    downloads_folder_fallback_icon(scale_factor).into_any_element()
}

pub(super) fn applications_sidebar_icon(scale_factor: f32) -> AnyElement {
    image_icon(APPLICATIONS_SIDEBAR_ICON.clone(), 22.0, 20.0, scale_factor)
}

pub(super) fn bin_sidebar_icon(scale_factor: f32) -> AnyElement {
    image_icon(BIN_SIDEBAR_ICON.clone(), 22.0, 20.0, scale_factor)
}

pub(super) fn image_icon(
    image: Arc<Image>,
    width_physical: f32,
    height_physical: f32,
    scale_factor: f32,
) -> AnyElement {
    img(image)
        .w(device_px(width_physical, scale_factor))
        .h(device_px(height_physical, scale_factor))
        .flex_shrink_0()
        .object_fit(ObjectFit::Contain)
        .into_any_element()
}

fn desktop_folder_fallback_icon(scale_factor: f32) -> Div {
    div()
        .relative()
        .w(device_px(
            DESKTOP_FOLDER_FALLBACK_SLOT_WIDTH_PHYSICAL,
            scale_factor,
        ))
        .h(device_px(
            DESKTOP_FOLDER_FALLBACK_SLOT_HEIGHT_PHYSICAL,
            scale_factor,
        ))
        .flex_shrink_0()
        .child(
            div()
                .absolute()
                .left(device_px(1.0, scale_factor))
                .top(device_px(2.0, scale_factor))
                .w(device_px(
                    DESKTOP_FOLDER_FALLBACK_SCREEN_WIDTH_PHYSICAL,
                    scale_factor,
                ))
                .h(device_px(
                    DESKTOP_FOLDER_FALLBACK_SCREEN_HEIGHT_PHYSICAL,
                    scale_factor,
                ))
                .bg(rgb(DESKTOP_FOLDER_FALLBACK_SCREEN_COLOR)),
        )
        .child(
            div()
                .absolute()
                .left(device_px(1.0, scale_factor))
                .top(device_px(2.0, scale_factor))
                .w(device_px(
                    DESKTOP_FOLDER_FALLBACK_SCREEN_WIDTH_PHYSICAL,
                    scale_factor,
                ))
                .h(device_px(2.0, scale_factor))
                .bg(rgb(DESKTOP_FOLDER_FALLBACK_SCREEN_HIGHLIGHT_COLOR)),
        )
        .child(
            div()
                .absolute()
                .left(device_px(1.0, scale_factor))
                .top(device_px(15.0, scale_factor))
                .w(device_px(
                    DESKTOP_FOLDER_FALLBACK_SCREEN_WIDTH_PHYSICAL,
                    scale_factor,
                ))
                .h(device_px(2.0, scale_factor))
                .bg(rgb(DESKTOP_FOLDER_FALLBACK_SCREEN_BOTTOM_COLOR)),
        )
        .child(
            div()
                .absolute()
                .left(device_px(4.0, scale_factor))
                .top(device_px(6.0, scale_factor))
                .w(device_px(
                    DESKTOP_FOLDER_FALLBACK_DETAIL_WIDTH_PHYSICAL,
                    scale_factor,
                ))
                .h(device_px(
                    DESKTOP_FOLDER_FALLBACK_DETAIL_HEIGHT_PHYSICAL,
                    scale_factor,
                ))
                .bg(rgb(DESKTOP_FOLDER_FALLBACK_DETAIL_LIGHT_COLOR)),
        )
        .child(
            div()
                .absolute()
                .left(device_px(4.0, scale_factor))
                .top(device_px(10.0, scale_factor))
                .w(device_px(
                    DESKTOP_FOLDER_FALLBACK_DETAIL_WIDTH_PHYSICAL,
                    scale_factor,
                ))
                .h(device_px(
                    DESKTOP_FOLDER_FALLBACK_DETAIL_HEIGHT_PHYSICAL,
                    scale_factor,
                ))
                .bg(rgb(DESKTOP_FOLDER_FALLBACK_DETAIL_MID_COLOR)),
        )
}

fn downloads_folder_fallback_icon(scale_factor: f32) -> Div {
    special_folder_glyph_icon(
        DOWNLOADS_FOLDER_FALLBACK_GLYPH,
        DOWNLOADS_FOLDER_FALLBACK_ICON_SIZE_PHYSICAL,
        DOWNLOADS_FOLDER_FALLBACK_ICON_COLOR,
        scale_factor,
    )
}

fn documents_folder_fallback_icon(scale_factor: f32) -> Div {
    div()
        .relative()
        .w(device_px(
            DOCUMENTS_FOLDER_FALLBACK_SLOT_WIDTH_PHYSICAL,
            scale_factor,
        ))
        .h(device_px(
            DOCUMENTS_FOLDER_FALLBACK_SLOT_HEIGHT_PHYSICAL,
            scale_factor,
        ))
        .flex_shrink_0()
        .child(
            div()
                .absolute()
                .left(device_px(
                    DOCUMENTS_FOLDER_FALLBACK_PAGE_LEFT_PHYSICAL,
                    scale_factor,
                ))
                .top(device_px(
                    DOCUMENTS_FOLDER_FALLBACK_PAGE_TOP_PHYSICAL,
                    scale_factor,
                ))
                .w(device_px(
                    DOCUMENTS_FOLDER_FALLBACK_PAGE_WIDTH_PHYSICAL,
                    scale_factor,
                ))
                .h(device_px(
                    DOCUMENTS_FOLDER_FALLBACK_PAGE_HEIGHT_PHYSICAL,
                    scale_factor,
                ))
                .bg(rgb(DOCUMENTS_FOLDER_FALLBACK_PAGE_COLOR)),
        )
        .child(documents_folder_line(7.0, 3.0, 2.0, scale_factor))
        .child(documents_folder_line(7.0, 6.0, 3.0, scale_factor))
        .child(documents_folder_line(7.0, 9.0, 8.0, scale_factor))
        .child(documents_folder_line(7.0, 12.0, 8.0, scale_factor))
        .child(documents_folder_line(7.0, 15.0, 8.0, scale_factor))
}

fn special_folder_glyph_icon(
    glyph: &'static str,
    icon_size_physical: f32,
    icon_color: u32,
    scale_factor: f32,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(device_px(22.0, scale_factor))
        .h(device_px(20.0, scale_factor))
        .flex_shrink_0()
        .font(nav_icon_font())
        .text_size(device_px(icon_size_physical, scale_factor))
        .text_color(rgb(icon_color))
        .child(glyph)
}

fn documents_folder_line(
    left_physical: f32,
    top_physical: f32,
    width_physical: f32,
    scale_factor: f32,
) -> Div {
    div()
        .absolute()
        .left(device_px(left_physical, scale_factor))
        .top(device_px(top_physical, scale_factor))
        .w(device_px(width_physical, scale_factor))
        .h(device_px(1.0, scale_factor))
        .bg(rgb(DOCUMENTS_FOLDER_FALLBACK_LINE_COLOR))
}

pub(super) fn drive_icon(scale_factor: f32) -> Div {
    div()
        .relative()
        .w(device_px(22.0, scale_factor))
        .h(device_px(18.0, scale_factor))
        .flex_shrink_0()
        .child(
            div()
                .absolute()
                .left(device_px(1.0, scale_factor))
                .top(device_px(4.0, scale_factor))
                .w(device_px(20.0, scale_factor))
                .h(device_px(11.0, scale_factor))
                .bg(rgb(0xe9eef5))
                .border_1()
                .border_color(rgb(0x8a95a3)),
        )
        .child(
            div()
                .absolute()
                .left(device_px(3.0, scale_factor))
                .top(device_px(12.0, scale_factor))
                .w(device_px(16.0, scale_factor))
                .h(device_px(2.0, scale_factor))
                .bg(rgb(0xc9d3df)),
        )
        .child(
            div()
                .absolute()
                .right(device_px(4.0, scale_factor))
                .top(device_px(11.0, scale_factor))
                .w(device_px(3.0, scale_factor))
                .h(device_px(3.0, scale_factor))
                .bg(rgb(0x2aa7ff)),
        )
}

pub(super) fn directory_shortcut_icon(scale_factor: f32) -> Div {
    div()
        .relative()
        .w(device_px(FILE_ICON_SLOT_WIDTH_PHYSICAL, scale_factor))
        .h(device_px(FILE_ICON_SLOT_HEIGHT_PHYSICAL, scale_factor))
        .flex_shrink_0()
        .child(
            div()
                .absolute()
                .left(device_px(0.0, scale_factor))
                .top(device_px(1.0, scale_factor))
                .child(folder_icon(scale_factor)),
        )
        .child(
            div()
                .absolute()
                .left(device_px(0.0, scale_factor))
                .top(device_px(11.0, scale_factor))
                .w(device_px(11.0, scale_factor))
                .h(device_px(9.0, scale_factor))
                .bg(rgb(0xffffff))
                .border_1()
                .border_color(rgb(0xb7b7b7)),
        )
        .child(
            div()
                .absolute()
                .left(device_px(2.0, scale_factor))
                .top(device_px(14.0, scale_factor))
                .w(device_px(6.0, scale_factor))
                .h(device_px(2.0, scale_factor))
                .bg(rgb(0x1f1f1f)),
        )
        .child(
            div()
                .absolute()
                .left(device_px(5.0, scale_factor))
                .top(device_px(12.0, scale_factor))
                .w(device_px(2.0, scale_factor))
                .h(device_px(6.0, scale_factor))
                .bg(rgb(0x1f1f1f)),
        )
}

pub(super) fn file_icon(scale_factor: f32) -> Div {
    special_folder_glyph_icon(
        FILE_ICON_FALLBACK_GLYPH,
        FILE_ICON_FALLBACK_ICON_SIZE_PHYSICAL,
        FILE_ICON_FALLBACK_ICON_COLOR,
        scale_factor,
    )
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::explorer::constants::NAV_ICON_TEXT_SIZE;

    #[test]
    fn device_pixel_values_convert_to_logical_pixels() {
        assert_eq!(device_px_value(22.0, 1.0), 22.0);
        assert!((device_px_value(22.0, 1.5) - 14.666_667).abs() < 0.000_01);
        assert!((device_px_value(17.0, 1.5) - 11.333_333).abs() < 0.000_01);
    }

    #[test]
    fn device_pixel_conversion_handles_invalid_scale() {
        assert_eq!(device_px_value(22.0, 0.0), 22.0);
        assert_eq!(device_px_value(22.0, -1.0), 22.0);
    }

    #[test]
    fn nav_icons_use_windows_explorer_glyphs() {
        assert_eq!(NavIcon::Back.glyph(), "\u{E72B}");
        assert_eq!(NavIcon::Forward.glyph(), "\u{E72A}");
        assert_eq!(NavIcon::Up.glyph(), "\u{E74A}");
        assert_eq!(NavIcon::Refresh.glyph(), "\u{E72C}");
    }

    #[test]
    fn downloads_fallback_icon_uses_windows_explorer_download_glyph() {
        assert_eq!(DOWNLOADS_FOLDER_FALLBACK_GLYPH, "\u{E896}");
        assert_eq!(DOWNLOADS_FOLDER_FALLBACK_ICON_SIZE_PHYSICAL, 18.0);
        assert_eq!(DOWNLOADS_FOLDER_FALLBACK_ICON_COLOR, 0x10893e);
    }

    #[test]
    fn documents_fallback_icon_uses_windows_explorer_documents_geometry() {
        assert_eq!(DOCUMENTS_FOLDER_FALLBACK_SLOT_WIDTH_PHYSICAL, 22.0);
        assert_eq!(DOCUMENTS_FOLDER_FALLBACK_SLOT_HEIGHT_PHYSICAL, 20.0);
        assert_eq!(DOCUMENTS_FOLDER_FALLBACK_PAGE_LEFT_PHYSICAL, 4.0);
        assert_eq!(DOCUMENTS_FOLDER_FALLBACK_PAGE_TOP_PHYSICAL, 1.0);
        assert_eq!(DOCUMENTS_FOLDER_FALLBACK_PAGE_WIDTH_PHYSICAL, 14.0);
        assert_eq!(DOCUMENTS_FOLDER_FALLBACK_PAGE_HEIGHT_PHYSICAL, 18.0);
        assert_eq!(DOCUMENTS_FOLDER_FALLBACK_PAGE_COLOR, 0x7897b6);
        assert_eq!(DOCUMENTS_FOLDER_FALLBACK_LINE_COLOR, 0xffffff);
    }

    #[test]
    fn desktop_fallback_icon_uses_windows_explorer_desktop_glyph_geometry() {
        assert_eq!(DESKTOP_FOLDER_FALLBACK_SLOT_WIDTH_PHYSICAL, 22.0);
        assert_eq!(DESKTOP_FOLDER_FALLBACK_SLOT_HEIGHT_PHYSICAL, 20.0);
        assert_eq!(DESKTOP_FOLDER_FALLBACK_SCREEN_WIDTH_PHYSICAL, 20.0);
        assert_eq!(DESKTOP_FOLDER_FALLBACK_SCREEN_HEIGHT_PHYSICAL, 15.0);
        assert_eq!(DESKTOP_FOLDER_FALLBACK_SCREEN_COLOR, 0x2aa9d8);
        assert_eq!(DESKTOP_FOLDER_FALLBACK_SCREEN_HIGHLIGHT_COLOR, 0x86f2ee);
        assert_eq!(DESKTOP_FOLDER_FALLBACK_SCREEN_BOTTOM_COLOR, 0x1381a8);
        assert_eq!(DESKTOP_FOLDER_FALLBACK_DETAIL_WIDTH_PHYSICAL, 2.0);
        assert_eq!(DESKTOP_FOLDER_FALLBACK_DETAIL_HEIGHT_PHYSICAL, 3.0);
        assert_eq!(DESKTOP_FOLDER_FALLBACK_DETAIL_LIGHT_COLOR, 0xe5ffff);
        assert_eq!(DESKTOP_FOLDER_FALLBACK_DETAIL_MID_COLOR, 0x70d7d7);
    }

    #[test]
    fn nav_icon_size_is_logical_and_scale_independent() {
        assert_eq!(NAV_ICON_TEXT_SIZE, 12.0);
    }

    #[test]
    fn drive_icon_uses_fixed_explorer_list_slot() {
        assert_eq!(device_px_value(22.0, 1.0), 22.0);
        assert_eq!(device_px_value(18.0, 1.0), 18.0);
    }

    #[test]
    fn special_folder_fallback_icons_use_sidebar_icon_slot() {
        assert_eq!(device_px_value(22.0, 1.0), 22.0);
        assert_eq!(device_px_value(20.0, 1.0), 20.0);
    }

    #[test]
    fn sidebar_image_icons_use_bundled_png_assets() {
        assert!(!APPLICATIONS_SIDEBAR_ICON_BYTES.is_empty());
        assert!(!BIN_SIDEBAR_ICON_BYTES.is_empty());
    }
}
