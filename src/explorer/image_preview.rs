use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use gpui::RenderImage;
use image::ImageEncoder;

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const SVG_IMAGE_RASTER_LONGEST_SIDE: u32 = 500;

#[derive(Clone, Debug)]
pub(super) struct PropertyImagePreview {
    pub(super) image: Arc<RenderImage>,
    pub(super) width: u32,
    pub(super) height: u32,
}

pub(super) fn path_may_have_image_preview(path: &Path) -> bool {
    let Some(extension) = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
    else {
        return mime_guess::from_path(path)
            .first_raw()
            .is_some_and(|mime| mime.starts_with("image/"));
    };

    extension == "svg"
        || image::ImageFormat::from_extension(&extension).is_some()
        || mime_guess::from_path(path)
            .first_raw()
            .is_some_and(|mime| mime.starts_with("image/"))
}

pub(super) fn load_property_image_preview(path: &Path) -> Result<PropertyImagePreview, String> {
    let image = load_image_rgba(path, SVG_IMAGE_RASTER_LONGEST_SIDE)?;
    property_image_preview_from_rgba(image)
}

#[cfg(test)]
pub(super) fn load_image_thumbnail_png(path: &Path, size: u32) -> Result<Vec<u8>, String> {
    let cancel = AtomicBool::new(false);
    load_image_thumbnail_png_with_cancel(path, size, &cancel)
}

pub(super) fn load_image_thumbnail_png_with_cancel(
    path: &Path,
    size: u32,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    if size == 0 {
        return Err("Thumbnail target has no dimensions.".to_owned());
    }

    check_image_cancelled(cancel)?;
    let image = load_image_rgba_with_cancel(path, size, cancel)?;
    check_image_cancelled(cancel)?;
    let thumbnail = fit_rgba_image_on_square_canvas(image, size)?;
    check_image_cancelled(cancel)?;
    encode_rgba_png_bytes(thumbnail.as_raw(), size, size)
        .ok_or_else(|| "Failed to encode image thumbnail.".to_owned())
}

fn load_image_rgba(path: &Path, svg_longest_side: u32) -> Result<image::RgbaImage, String> {
    let cancel = AtomicBool::new(false);
    load_image_rgba_with_cancel(path, svg_longest_side, &cancel)
}

fn load_image_rgba_with_cancel(
    path: &Path,
    svg_longest_side: u32,
    cancel: &AtomicBool,
) -> Result<image::RgbaImage, String> {
    check_image_cancelled(cancel)?;
    if path_is_svg(path) {
        return load_svg_rgba_with_cancel(path, svg_longest_side, cancel);
    }

    let bytes = fs::read(path).map_err(|error| format!("Failed to read image file: {error}"))?;
    check_image_cancelled(cancel)?;
    let format = image::guess_format(&bytes)
        .or_else(|_| image::ImageFormat::from_path(path))
        .map_err(|error| format!("Unsupported image format: {error}"))?;
    check_image_cancelled(cancel)?;
    let image = image::load_from_memory_with_format(&bytes, format)
        .map_err(|error| format!("Failed to decode image: {error}"))?;
    check_image_cancelled(cancel)?;
    Ok(image.into_rgba8())
}

fn load_svg_rgba_with_cancel(
    path: &Path,
    longest_side: u32,
    cancel: &AtomicBool,
) -> Result<image::RgbaImage, String> {
    check_image_cancelled(cancel)?;
    let bytes = fs::read(path).map_err(|error| format!("Failed to read SVG file: {error}"))?;
    check_image_cancelled(cancel)?;
    let options = usvg::Options::default();
    let tree = usvg::Tree::from_data(&bytes, &options)
        .map_err(|error| format!("Failed to parse SVG: {error}"))?;
    check_image_cancelled(cancel)?;
    let svg_size = tree.size();
    let (width, height) = svg_raster_dimensions(svg_size.width(), svg_size.height(), longest_side)
        .ok_or_else(|| "SVG has no renderable dimensions.".to_owned())?;
    let scale = width as f32 / svg_size.width();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| "SVG raster target has invalid dimensions.".to_owned())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    check_image_cancelled(cancel)?;

    let mut image = image::RgbaImage::from_raw(width, height, pixmap.take())
        .ok_or_else(|| "SVG rasterizer returned invalid pixel data.".to_owned())?;
    for pixel in image.chunks_exact_mut(4) {
        unpremultiply_rgba(pixel);
    }

    Ok(image)
}

fn check_image_cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Image thumbnail loading was cancelled.".to_owned())
    } else {
        Ok(())
    }
}

fn property_image_preview_from_rgba(
    mut image: image::RgbaImage,
) -> Result<PropertyImagePreview, String> {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return Err("Image has no dimensions.".to_owned());
    }

    for pixel in image.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    Ok(PropertyImagePreview {
        image: Arc::new(RenderImage::new(vec![image::Frame::new(image)])),
        width,
        height,
    })
}

fn fit_rgba_image_on_square_canvas(
    image: image::RgbaImage,
    size: u32,
) -> Result<image::RgbaImage, String> {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return Err("Image has no dimensions.".to_owned());
    }

    let scale = size as f32 / width.max(height) as f32;
    let resized_width = ((width as f32 * scale).round() as u32).clamp(1, size);
    let resized_height = ((height as f32 * scale).round() as u32).clamp(1, size);
    let resized = image::imageops::resize(
        &image,
        resized_width,
        resized_height,
        image::imageops::FilterType::Lanczos3,
    );
    let mut canvas = image::RgbaImage::from_pixel(size, size, image::Rgba([0, 0, 0, 0]));
    let x = ((size - resized_width) / 2) as i64;
    let y = ((size - resized_height) / 2) as i64;
    image::imageops::overlay(&mut canvas, &resized, x, y);
    Ok(canvas)
}

pub(super) fn svg_raster_dimensions(
    width: f32,
    height: f32,
    longest_side: u32,
) -> Option<(u32, u32)> {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    if longest_side == 0 {
        return None;
    }

    let scale = longest_side as f32 / width.max(height);
    let raster_width = (width * scale).round().max(1.0) as u32;
    let raster_height = (height * scale).round().max(1.0) as u32;
    Some((raster_width, raster_height))
}

fn encode_rgba_png_bytes(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .ok()?;
    bytes.starts_with(PNG_SIGNATURE).then_some(bytes)
}

fn path_is_svg(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
}

fn unpremultiply_rgba(color: &mut [u8]) {
    if color[3] > 0 {
        let alpha = color[3] as f32 / 255.0;
        color[0] = (color[0] as f32 / alpha) as u8;
        color[1] = (color[1] as f32 / alpha) as u8;
        color[2] = (color[2] as f32 / alpha) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;
    use std::io::Cursor;

    #[test]
    fn image_preview_accepts_webp_extension() {
        assert!(path_may_have_image_preview(Path::new("poster.webp")));
    }

    #[test]
    fn svg_raster_dimensions_scale_longest_side() {
        assert_eq!(svg_raster_dimensions(1000.0, 250.0, 500), Some((500, 125)));
        assert_eq!(svg_raster_dimensions(250.0, 1000.0, 500), Some((125, 500)));
        assert_eq!(svg_raster_dimensions(400.0, 400.0, 500), Some((500, 500)));
        assert_eq!(svg_raster_dimensions(3.0, 1.0, 500), Some((500, 167)));
    }

    #[test]
    fn svg_raster_dimensions_reject_invalid_inputs() {
        assert_eq!(svg_raster_dimensions(0.0, 100.0, 500), None);
        assert_eq!(svg_raster_dimensions(100.0, 0.0, 500), None);
        assert_eq!(svg_raster_dimensions(f32::NAN, 100.0, 500), None);
        assert_eq!(svg_raster_dimensions(100.0, f32::INFINITY, 500), None);
        assert_eq!(svg_raster_dimensions(100.0, 100.0, 0), None);
    }

    #[test]
    fn image_preview_decodes_png_dimensions() {
        let temp = TempDir::new();
        let path = temp.path().join("image.png");
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(4, 2));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        fs::write(&path, bytes).unwrap();

        let preview = load_property_image_preview(&path).unwrap();

        assert_eq!(preview.width, 4);
        assert_eq!(preview.height, 2);
        assert_render_image_size(&preview, 4, 2);
        assert!(!preview.image.as_bytes(0).unwrap().is_empty());
    }

    #[test]
    fn image_preview_rasterizes_svg_to_500px_longest_side() {
        let temp = TempDir::new();
        let path = temp.path().join("vector.svg");
        fs::write(
            &path,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="250"><rect width="1000" height="250" fill="red"/></svg>"#,
        )
        .unwrap();

        let preview = load_property_image_preview(&path).unwrap();

        assert_eq!(preview.width, 500);
        assert_eq!(preview.height, 125);
        assert_render_image_size(&preview, 500, 125);
        assert!(!preview.image.as_bytes(0).unwrap().is_empty());
    }

    #[test]
    fn image_thumbnail_png_uses_square_128px_canvas() {
        let temp = TempDir::new();
        let path = temp.path().join("image.png");
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(4, 2));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        fs::write(&path, bytes).unwrap();

        let thumbnail = load_image_thumbnail_png(&path, 128).unwrap();
        let decoded = image::load_from_memory(&thumbnail).unwrap().into_rgba8();

        assert_eq!(decoded.dimensions(), (128, 128));
    }

    #[test]
    fn svg_thumbnail_png_uses_square_128px_canvas() {
        let temp = TempDir::new();
        let path = temp.path().join("vector.svg");
        fs::write(
            &path,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="250"><rect width="1000" height="250" fill="red"/></svg>"#,
        )
        .unwrap();

        let thumbnail = load_image_thumbnail_png(&path, 128).unwrap();
        let decoded = image::load_from_memory(&thumbnail).unwrap().into_rgba8();

        assert_eq!(decoded.dimensions(), (128, 128));
    }

    fn assert_render_image_size(preview: &PropertyImagePreview, width: u32, height: u32) {
        let size = preview.image.size(0);
        assert_eq!(size.width.0, width as i32);
        assert_eq!(size.height.0, height as i32);
    }
}
