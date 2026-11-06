use crate::{
    Bounds, Font, IsZero, Pixels, Point, RenderGlyphParams, SharedString, TextRun,
    WindowTextSystem, point, px,
};
use image::{Pixel, Rgba, RgbaImage};

/// A native drag image in straight-alpha RGBA, with a logical cursor hotspot.
#[derive(Clone, Debug)]
pub struct NativeDragImage {
    pub(crate) pixels: RgbaImage,
    pub(crate) scale_factor: f32,
    pub(crate) cursor_offset: Point<Pixels>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_argb_pixels_are_premultiplied_and_hotspot_stays_logical() {
        let pixels = RgbaImage::from_pixel(2, 3, Rgba([100, 50, 200, 128]));
        let image = NativeDragImage::new(pixels, 1.5, point(px(1.), px(2.)));
        assert_eq!(&image.premultiplied_bgra()[..4], &[100, 25, 50, 128]);
        assert_eq!(image.cursor_offset, point(px(1.), px(2.)));
        assert_eq!(image.scale_factor, 1.5);
    }

    #[test]
    fn transparent_pixels_do_not_leak_rgb_into_native_surfaces() {
        let pixels = RgbaImage::from_pixel(1, 1, Rgba([255, 255, 255, 0]));
        assert_eq!(
            NativeDragImage::new(pixels, 1.0, Point::default()).premultiplied_bgra(),
            vec![0; 4]
        );
    }
}

impl NativeDragImage {
    /// The image's logical size is its pixel size divided by `scale_factor`.
    pub fn new(pixels: RgbaImage, scale_factor: f32, cursor_offset: Point<Pixels>) -> Self {
        assert!(scale_factor.is_finite() && scale_factor > 0.0);
        assert!(pixels.width() > 0 && pixels.height() > 0);
        Self {
            pixels,
            scale_factor,
            cursor_offset,
        }
    }

    /// Access the straight-alpha bitmap for compositing additional decorations.
    pub fn pixels_mut(&mut self) -> &mut RgbaImage {
        &mut self.pixels
    }

    /// Draw shaped text using the same fonts and rasterizer as GPUI. Text is
    /// vertically centered and ellipsized to fit the logical rectangle.
    pub fn draw_text(
        &mut self,
        text: SharedString,
        font: Font,
        font_size: Pixels,
        color: [u8; 3],
        bounds: Bounds<Pixels>,
        text_system: &WindowTextSystem,
    ) -> anyhow::Result<()> {
        let shape = |text: SharedString| {
            let run = TextRun {
                len: text.len(),
                font: font.clone(),
                color: crate::black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            text_system.shape_line(text, font_size, &[run], None)
        };
        let mut line = shape(text);
        if line.width > bounds.size.width {
            let mut truncated = line.text.to_string();
            loop {
                truncated.pop();
                line = shape(format!("{truncated}…").into());
                if line.width <= bounds.size.width || truncated.is_empty() {
                    break;
                }
            }
        }
        let baseline =
            bounds.origin.y + (bounds.size.height - line.ascent - line.descent) / 2.0 + line.ascent;
        for run in &line.runs {
            for glyph in &run.glyphs {
                let params = RenderGlyphParams {
                    font_id: run.font_id,
                    glyph_id: glyph.id,
                    font_size,
                    subpixel_variant: point(0, 0),
                    scale_factor: self.scale_factor,
                    is_emoji: glyph.is_emoji,
                };
                let raster = text_system.raster_bounds(&params)?;
                if raster.is_zero() {
                    continue;
                }
                let (size, bytes) = text_system.rasterize_glyph(&params)?;
                let x = ((bounds.origin.x + glyph.position.x).0 * self.scale_factor).floor() as i32
                    + raster.origin.x.0;
                let y = ((baseline + glyph.position.y).0 * self.scale_factor).floor() as i32
                    + raster.origin.y.0;
                let channels = bytes.len() / (size.width.0 * size.height.0) as usize;
                for row in 0..size.height.0 {
                    for col in 0..size.width.0 {
                        let dx = x + col;
                        let dy = y + row;
                        if dx < 0
                            || dy < 0
                            || dx >= self.pixels.width() as i32
                            || dy >= self.pixels.height() as i32
                        {
                            continue;
                        }
                        let logical = point(
                            px(dx as f32 / self.scale_factor),
                            px(dy as f32 / self.scale_factor),
                        );
                        if !bounds.contains(&logical) {
                            continue;
                        }
                        let i = ((row * size.width.0 + col) as usize) * channels;
                        let pixel = if channels == 4 && glyph.is_emoji {
                            // Platform text systems return straight-alpha BGRA.
                            Rgba([bytes[i + 2], bytes[i + 1], bytes[i], bytes[i + 3]])
                        } else {
                            let alpha = if channels >= 3 {
                                ((bytes[i] as u16 + bytes[i + 1] as u16 + bytes[i + 2] as u16) / 3)
                                    as u8
                            } else {
                                bytes[i]
                            };
                            Rgba([color[0], color[1], color[2], alpha])
                        };
                        self.pixels
                            .get_pixel_mut(dx as u32, dy as u32)
                            .blend(&pixel);
                    }
                }
            }
        }
        Ok(())
    }

    /// Native ARGB surfaces use premultiplied BGRA on little-endian hosts.
    pub(crate) fn premultiplied_bgra(&self) -> Vec<u8> {
        self.pixels
            .pixels()
            .flat_map(|p| {
                let a = p[3] as u16;
                [
                    (p[2] as u16 * a / 255) as u8,
                    (p[1] as u16 * a / 255) as u8,
                    (p[0] as u16 * a / 255) as u8,
                    p[3],
                ]
            })
            .collect()
    }
}
