//! The native counterpart of `tab_preview_visual`, using the same assets and
//! GPUI text rasterizer. Native drag protocols require a bitmap, not an element.
use super::*;
use gpui::{NativeDragImage, size};
use image::{Rgba, RgbaImage};
use resvg::tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

const SHADOW_PADDING: f32 = 10.0;

pub(super) fn tab_drag_image(
    label: SharedString,
    path: &Path,
    group: Option<SidebarGroupKind>,
    is_active: bool,
    pane_count: usize,
    font: gpui::Font,
    offset: Point<Pixels>,
    window: &Window,
) -> NativeDragImage {
    let scale = window.scale_factor();
    let width = ((TAB_WIDTH + SHADOW_PADDING * 2.0) * scale).ceil() as u32;
    let height = ((TAB_BAR_HEIGHT + SHADOW_PADDING * 2.0) * scale).ceil() as u32;
    let mut bitmap = RgbaImage::new(width, height);
    // A soft shadow with transparent padding, outside the opaque tab rectangle.
    for (x, y, pixel) in bitmap.enumerate_pixels_mut() {
        let x = x as f32 / scale - SHADOW_PADDING;
        let y = y as f32 / scale - SHADOW_PADDING;
        let dx = (-x).max(x - TAB_WIDTH).max(0.0);
        let dy = (2.0 - y).max(y - TAB_BAR_HEIGHT - 2.0).max(0.0);
        let alpha = (36.0 * (-(dx * dx + dy * dy) / 24.0).exp()) as u8;
        *pixel = Rgba([0, 0, 0, alpha]);
    }
    let rect = |x, y, w, h| Bounds {
        origin: gpui::point(px(x + SHADOW_PADDING), px(y + SHADOW_PADDING)),
        size: size(px(w), px(h)),
    };
    let mut fill = |bounds: Bounds<Pixels>, color: u32| {
        let left = (f32::from(bounds.origin.x) * scale).round() as u32;
        let top = (f32::from(bounds.origin.y) * scale).round() as u32;
        let right = (f32::from(bounds.origin.x + bounds.size.width) * scale).round() as u32;
        let bottom = (f32::from(bounds.origin.y + bounds.size.height) * scale).round() as u32;
        for y in top..bottom.min(height) {
            for x in left..right.min(width) {
                bitmap.put_pixel(
                    x,
                    y,
                    Rgba([(color >> 16) as u8, (color >> 8) as u8, color as u8, 255]),
                );
            }
        }
    };
    fill(rect(0.0, 0.0, TAB_WIDTH, TAB_BAR_HEIGHT), TAB_BORDER);
    fill(
        rect(1.0, 1.0, TAB_WIDTH - 2.0, TAB_BAR_HEIGHT - 2.0),
        if is_active {
            TAB_ACTIVE_BG
        } else {
            TAB_INACTIVE_BG
        },
    );

    let icon = crate::explorer::icons::tab_directory_image(Some(path), group);
    let icon_size = (16.0 * scale).round().max(1.0) as u32;
    let icon_bitmap = if icon.format == gpui::ImageFormat::Svg {
        usvg::Tree::from_data(&icon.bytes, &usvg::Options::default())
            .ok()
            .and_then(|tree| {
                let mut pixmap = Pixmap::new(icon_size, icon_size)?;
                resvg::render(
                    &tree,
                    Transform::from_scale(
                        icon_size as f32 / tree.size().width(),
                        icon_size as f32 / tree.size().height(),
                    ),
                    &mut pixmap.as_mut(),
                );
                let pixels: Vec<u8> = pixmap
                    .pixels()
                    .iter()
                    .flat_map(|p| {
                        let c = p.demultiply();
                        [c.red(), c.green(), c.blue(), c.alpha()]
                    })
                    .collect();
                RgbaImage::from_raw(icon_size, icon_size, pixels)
            })
    } else {
        image::load_from_memory(&icon.bytes).ok().map(|image| {
            image
                .resize_exact(icon_size, icon_size, image::imageops::FilterType::Lanczos3)
                .into_rgba8()
        })
    };
    if let Some(icon) = icon_bitmap {
        image::imageops::overlay(
            &mut bitmap,
            &icon,
            ((SHADOW_PADDING + TAB_HORIZONTAL_PADDING + 1.0) * scale).round() as i64,
            ((SHADOW_PADDING + (TAB_BAR_HEIGHT - 16.0) / 2.0) * scale).round() as i64,
        );
    }
    let hotspot = offset + gpui::point(px(SHADOW_PADDING), px(SHADOW_PADDING));
    let mut image = NativeDragImage::new(bitmap, scale, hotspot);
    let close_x = TAB_WIDTH - TAB_HORIZONTAL_PADDING - TAB_CLOSE_SIZE - 1.0;
    let text_x = TAB_HORIZONTAL_PADDING + 1.0 + 16.0 + TAB_ICON_GAP;
    let badge_width = if pane_count > 1 {
        20.0_f32.max(pane_count.to_string().len() as f32 * 6.0 + 10.0)
    } else {
        0.0
    };
    let text_end = close_x
        - TAB_ICON_GAP
        - if pane_count > 1 {
            badge_width + TAB_ICON_GAP
        } else {
            0.0
        };
    let _ = image.draw_text(
        label,
        font.clone(),
        px(TAB_TEXT_SIZE),
        [31, 31, 31],
        rect(text_x, 0.0, text_end - text_x, TAB_BAR_HEIGHT),
        window.text_system(),
    );
    let _ = image.draw_text(
        CLOSE_GLYPH.into(),
        tab_icon_font(),
        px(TAB_ICON_TEXT_SIZE),
        [64, 64, 64],
        rect(
            close_x + (TAB_CLOSE_SIZE - TAB_ICON_TEXT_SIZE) / 2.0,
            0.0,
            TAB_ICON_TEXT_SIZE + 1.0,
            TAB_BAR_HEIGHT,
        ),
        window.text_system(),
    );
    if pane_count > 1 {
        // Composite the same rounded split-count badge as the element preview.
        let mut badge = Pixmap::new(
            (badge_width * scale).ceil() as u32,
            (18.0 * scale).ceil() as u32,
        )
        .unwrap();
        let mut paint = Paint::default();
        paint.set_color_rgba8(229, 243, 255, 255);
        let mut path = PathBuilder::new();
        let r = 3.0;
        let k = r * 0.5522848;
        path.move_to(r, 0.0);
        path.line_to(badge_width - r, 0.0);
        path.cubic_to(badge_width - r + k, 0.0, badge_width, r - k, badge_width, r);
        path.line_to(badge_width, 18.0 - r);
        path.cubic_to(
            badge_width,
            18.0 - r + k,
            badge_width - r + k,
            18.0,
            badge_width - r,
            18.0,
        );
        path.line_to(r, 18.0);
        path.cubic_to(r - k, 18.0, 0.0, 18.0 - r + k, 0.0, 18.0 - r);
        path.line_to(0.0, r);
        path.cubic_to(0.0, r - k, r - k, 0.0, r, 0.0);
        path.close();
        badge.fill_path(
            &path.finish().unwrap(),
            &paint,
            FillRule::Winding,
            Transform::from_scale(scale, scale),
            None,
        );
        // Text and badge are painted below through the shared text rasterizer.
        let pixels = image.pixels_mut();
        let badge =
            RgbaImage::from_raw(badge.width(), badge.height(), badge.data().to_vec()).unwrap();
        image::imageops::overlay(
            pixels,
            &badge,
            ((SHADOW_PADDING + text_end + TAB_ICON_GAP) * scale).round() as i64,
            ((SHADOW_PADDING + (TAB_BAR_HEIGHT - 18.0) / 2.0) * scale).round() as i64,
        );
        let _ = image.draw_text(
            pane_count.to_string().into(),
            font,
            px(10.0),
            [0, 90, 158],
            rect(
                text_end + TAB_ICON_GAP + 5.0,
                0.0,
                badge_width - 10.0,
                TAB_BAR_HEIGHT,
            ),
            window.text_system(),
        );
    }
    image
}
