use gpui::{Div, FontFallbacks, Pixels, div, font, prelude::*, px, rgb};

use crate::explorer::constants::{
    FILE_ICON_FOLD_SIZE_PHYSICAL, FILE_ICON_PAGE_HEIGHT_PHYSICAL, FILE_ICON_PAGE_LEFT_PHYSICAL,
    FILE_ICON_PAGE_WIDTH_PHYSICAL, FILE_ICON_SLOT_HEIGHT_PHYSICAL, FILE_ICON_SLOT_WIDTH_PHYSICAL,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NavIcon {
    Back,
    Forward,
    Up,
    Refresh,
}

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

pub(super) fn file_icon(scale_factor: f32) -> Div {
    div()
        .relative()
        .w(device_px(FILE_ICON_SLOT_WIDTH_PHYSICAL, scale_factor))
        .h(device_px(FILE_ICON_SLOT_HEIGHT_PHYSICAL, scale_factor))
        .flex_shrink_0()
        .child(
            div()
                .relative()
                .absolute()
                .left(device_px(FILE_ICON_PAGE_LEFT_PHYSICAL, scale_factor))
                .top(device_px(0.0, scale_factor))
                .w(device_px(FILE_ICON_PAGE_WIDTH_PHYSICAL, scale_factor))
                .h(device_px(FILE_ICON_PAGE_HEIGHT_PHYSICAL, scale_factor))
                .border_1()
                .border_color(rgb(0x9a9a9a))
                .bg(rgb(0xffffff))
                .child(
                    div()
                        .absolute()
                        .right(device_px(0.0, scale_factor))
                        .top(device_px(0.0, scale_factor))
                        .w(device_px(FILE_ICON_FOLD_SIZE_PHYSICAL, scale_factor))
                        .h(device_px(FILE_ICON_FOLD_SIZE_PHYSICAL, scale_factor))
                        .border_l_1()
                        .border_b_1()
                        .border_color(rgb(0xc8c8c8))
                        .bg(rgb(0xf4f4f4)),
                ),
        )
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::explorer::constants::{
        FILE_ICON_PAGE_HEIGHT_PHYSICAL, FILE_ICON_PAGE_LEFT_PHYSICAL,
        FILE_ICON_PAGE_WIDTH_PHYSICAL, FILE_ICON_SLOT_HEIGHT_PHYSICAL,
        FILE_ICON_SLOT_WIDTH_PHYSICAL, NAV_ICON_SIZE_PHYSICAL,
    };

    #[test]
    fn device_pixel_values_convert_to_logical_pixels() {
        assert_eq!(device_px_value(22.0, 1.0), 22.0);
        assert!((device_px_value(22.0, 1.5) - 14.666_667).abs() < 0.000_01);
        assert!((device_px_value(17.0, 1.5) - 11.333_333).abs() < 0.000_01);
    }

    #[test]
    fn default_file_icon_uses_portrait_page_in_fixed_slot() {
        assert!(FILE_ICON_PAGE_HEIGHT_PHYSICAL > FILE_ICON_PAGE_WIDTH_PHYSICAL);
        assert_eq!(FILE_ICON_SLOT_WIDTH_PHYSICAL, 22.0);
        assert_eq!(
            FILE_ICON_PAGE_HEIGHT_PHYSICAL,
            FILE_ICON_SLOT_HEIGHT_PHYSICAL
        );
        assert_eq!(
            FILE_ICON_PAGE_LEFT_PHYSICAL,
            (FILE_ICON_SLOT_WIDTH_PHYSICAL - FILE_ICON_PAGE_WIDTH_PHYSICAL) / 2.0
        );
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
    fn nav_icon_size_converts_from_physical_pixels() {
        assert_eq!(device_px_value(NAV_ICON_SIZE_PHYSICAL, 1.0), 18.0);
        assert_eq!(device_px_value(NAV_ICON_SIZE_PHYSICAL, 1.5), 12.0);
    }
}
