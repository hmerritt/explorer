use std::path::PathBuf;

use gpui::{
    AnyElement, Div, FontFallbacks, ObjectFit, Pixels, RenderImage, div, font, img, prelude::*, px,
    rgb,
};

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

const DOWNLOADS_FOLDER_FALLBACK_GLYPH: &str = "\u{E896}";
const DOWNLOADS_FOLDER_FALLBACK_ICON_SIZE_PHYSICAL: f32 = 18.0;
const DOWNLOADS_FOLDER_FALLBACK_ICON_COLOR: u32 = 0x10893e;
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

pub(super) fn desktop_folder_icon(path: PathBuf, scale_factor: f32) -> AnyElement {
    system_or_fallback_folder_icon(path, scale_factor, desktop_folder_fallback_icon)
}

pub(super) fn downloads_folder_icon(path: PathBuf, scale_factor: f32) -> AnyElement {
    system_or_fallback_folder_icon(path, scale_factor, downloads_folder_fallback_icon)
}

fn system_or_fallback_folder_icon(
    path: PathBuf,
    scale_factor: f32,
    fallback: fn(f32) -> Div,
) -> AnyElement {
    #[cfg(target_os = "windows")]
    {
        if let Some(image) = windows_shell_small_icon(&path) {
            return img(image)
                .w(device_px(22.0, scale_factor))
                .h(device_px(20.0, scale_factor))
                .object_fit(ObjectFit::Contain)
                .flex_shrink_0()
                .into_any_element();
        }

        fallback(scale_factor).into_any_element()
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        fallback(scale_factor).into_any_element()
    }
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
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(device_px(22.0, scale_factor))
        .h(device_px(20.0, scale_factor))
        .flex_shrink_0()
        .font(nav_icon_font())
        .text_size(device_px(
            DOWNLOADS_FOLDER_FALLBACK_ICON_SIZE_PHYSICAL,
            scale_factor,
        ))
        .text_color(rgb(DOWNLOADS_FOLDER_FALLBACK_ICON_COLOR))
        .child(DOWNLOADS_FOLDER_FALLBACK_GLYPH)
}

#[cfg(target_os = "windows")]
fn windows_shell_small_icon(path: &std::path::Path) -> Option<std::sync::Arc<RenderImage>> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt, ptr, sync::Arc};

    use image::{Frame, ImageBuffer, Rgba};
    use windows::{
        Win32::{
            Foundation::HANDLE,
            Graphics::Gdi::{
                BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection,
                DIB_RGB_COLORS, DeleteDC, DeleteObject, HGDIOBJ, SelectObject,
            },
            Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES,
            UI::{
                Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON, SHGetFileInfoW},
                WindowsAndMessaging::{DI_NORMAL, DestroyIcon, DrawIconEx},
            },
        },
        core::PCWSTR,
    };

    const ICON_SIZE: i32 = 20;

    let mut wide_path = OsStr::new(path).encode_wide().collect::<Vec<_>>();
    wide_path.push(0);

    let mut info = SHFILEINFOW::default();
    let result = unsafe {
        SHGetFileInfoW(
            PCWSTR(wide_path.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_SMALLICON,
        )
    };

    if result == 0 || info.hIcon.is_invalid() {
        return None;
    }

    let mut bits = ptr::null_mut();
    let bitmap_info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: ICON_SIZE,
            biHeight: -ICON_SIZE,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let hdc = unsafe { CreateCompatibleDC(None) };
    if hdc.is_invalid() {
        let _ = unsafe { DestroyIcon(info.hIcon) };
        return None;
    }

    let bitmap = unsafe {
        CreateDIBSection(
            Some(hdc),
            &bitmap_info,
            DIB_RGB_COLORS,
            &mut bits,
            Some(HANDLE::default()),
            0,
        )
        .ok()?
    };

    if bits.is_null() {
        let _ = unsafe { DeleteObject(HGDIOBJ::from(bitmap)) };
        let _ = unsafe { DeleteDC(hdc) };
        let _ = unsafe { DestroyIcon(info.hIcon) };
        return None;
    }

    let previous = unsafe { SelectObject(hdc, HGDIOBJ::from(bitmap)) };
    if previous.is_invalid() {
        let _ = unsafe { DeleteObject(HGDIOBJ::from(bitmap)) };
        let _ = unsafe { DeleteDC(hdc) };
        let _ = unsafe { DestroyIcon(info.hIcon) };
        return None;
    }

    let draw_result = unsafe {
        DrawIconEx(
            hdc, 0, 0, info.hIcon, ICON_SIZE, ICON_SIZE, 0, None, DI_NORMAL,
        )
    };

    let len = ICON_SIZE as usize * ICON_SIZE as usize * 4;
    let bytes = if draw_result.is_ok() {
        Some(unsafe { std::slice::from_raw_parts(bits.cast::<u8>(), len) }.to_vec())
    } else {
        None
    };

    let _ = unsafe { SelectObject(hdc, previous) };
    let _ = unsafe { DeleteObject(HGDIOBJ::from(bitmap)) };
    let _ = unsafe { DeleteDC(hdc) };
    let _ = unsafe { DestroyIcon(info.hIcon) };

    let buffer =
        ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(ICON_SIZE as u32, ICON_SIZE as u32, bytes?)?;
    Some(Arc::new(RenderImage::new([Frame::new(buffer)])))
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
    fn downloads_fallback_icon_uses_windows_explorer_download_glyph() {
        assert_eq!(DOWNLOADS_FOLDER_FALLBACK_GLYPH, "\u{E896}");
        assert_eq!(DOWNLOADS_FOLDER_FALLBACK_ICON_SIZE_PHYSICAL, 18.0);
        assert_eq!(DOWNLOADS_FOLDER_FALLBACK_ICON_COLOR, 0x10893e);
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
    fn nav_icon_size_converts_from_physical_pixels() {
        assert_eq!(device_px_value(NAV_ICON_SIZE_PHYSICAL, 1.0), 18.0);
        assert_eq!(device_px_value(NAV_ICON_SIZE_PHYSICAL, 1.5), 12.0);
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
}
