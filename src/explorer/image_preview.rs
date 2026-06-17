use std::{
    fs::{self, File},
    io::BufReader,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct ImageThumbnailExtractionTimings {
    pub(super) embedded_thumbnail_scan: Option<Duration>,
    pub(super) embedded_thumbnail_decode: Option<Duration>,
    pub(super) source_read: Option<Duration>,
    pub(super) format_detect: Option<Duration>,
    pub(super) raster_decode: Option<Duration>,
    pub(super) rgba_convert: Option<Duration>,
    pub(super) svg_parse: Option<Duration>,
    pub(super) svg_render: Option<Duration>,
    pub(super) svg_unpremultiply: Option<Duration>,
    pub(super) resize_canvas: Option<Duration>,
    pub(super) png_encode: Option<Duration>,
}

pub(super) struct TimedImageThumbnailPng {
    pub(super) result: Result<Vec<u8>, String>,
    pub(super) timings: ImageThumbnailExtractionTimings,
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

#[cfg(test)]
pub(super) fn load_image_thumbnail_png_with_cancel(
    path: &Path,
    size: u32,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    load_image_thumbnail_png_with_cancel_timed(path, size, cancel, false).result
}

pub(super) fn load_image_thumbnail_png_with_cancel_timed(
    path: &Path,
    size: u32,
    cancel: &AtomicBool,
    timings_enabled: bool,
) -> TimedImageThumbnailPng {
    let mut timings = ImageThumbnailExtractionTimings::default();
    let result = load_image_thumbnail_png_with_cancel_timed_result(
        path,
        size,
        cancel,
        timings_enabled,
        &mut timings,
    );
    TimedImageThumbnailPng { result, timings }
}

fn load_image_thumbnail_png_with_cancel_timed_result(
    path: &Path,
    size: u32,
    cancel: &AtomicBool,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<Vec<u8>, String> {
    if size == 0 {
        return Err("Thumbnail target has no dimensions.".to_owned());
    }

    check_image_cancelled(cancel)?;
    let image =
        load_image_thumbnail_rgba_with_cancel_timed(path, size, cancel, timings_enabled, timings)?;
    check_image_cancelled(cancel)?;
    let resize_started = thumbnail_timing_started(timings_enabled);
    let thumbnail = fit_rgba_image_on_square_canvas(image, size);
    thumbnail_timing_finished(&mut timings.resize_canvas, resize_started);
    let thumbnail = thumbnail?;
    check_image_cancelled(cancel)?;
    let encode_started = thumbnail_timing_started(timings_enabled);
    let encoded = encode_rgba_png_bytes(thumbnail.as_raw(), size, size);
    thumbnail_timing_finished(&mut timings.png_encode, encode_started);
    encoded.ok_or_else(|| "Failed to encode image thumbnail.".to_owned())
}

fn load_image_thumbnail_rgba_with_cancel_timed(
    path: &Path,
    svg_longest_side: u32,
    cancel: &AtomicBool,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<image::RgbaImage, String> {
    check_image_cancelled(cancel)?;
    if path_is_svg(path) {
        return load_svg_rgba_with_cancel_timed(
            path,
            svg_longest_side,
            cancel,
            timings_enabled,
            timings,
        );
    }

    if path_may_be_jpeg(path) {
        if let Some(image) = load_embedded_jpeg_thumbnail_rgba_with_cancel_timed(
            path,
            cancel,
            timings_enabled,
            timings,
        )? {
            return Ok(image);
        }
    }

    load_raster_thumbnail_rgba_with_cancel_timed(path, cancel, timings_enabled, timings)
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
    let mut timings = ImageThumbnailExtractionTimings::default();
    load_image_rgba_with_cancel_timed(path, svg_longest_side, cancel, false, &mut timings)
}

fn load_image_rgba_with_cancel_timed(
    path: &Path,
    svg_longest_side: u32,
    cancel: &AtomicBool,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<image::RgbaImage, String> {
    check_image_cancelled(cancel)?;
    if path_is_svg(path) {
        return load_svg_rgba_with_cancel_timed(
            path,
            svg_longest_side,
            cancel,
            timings_enabled,
            timings,
        );
    }

    let read_started = thumbnail_timing_started(timings_enabled);
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            thumbnail_timing_finished(&mut timings.source_read, read_started);
            return Err(format!("Failed to read image file: {error}"));
        }
    };
    thumbnail_timing_finished(&mut timings.source_read, read_started);
    check_image_cancelled(cancel)?;
    let format_started = thumbnail_timing_started(timings_enabled);
    let format = image::guess_format(&bytes).or_else(|_| image::ImageFormat::from_path(path));
    thumbnail_timing_finished(&mut timings.format_detect, format_started);
    let format = format.map_err(|error| format!("Unsupported image format: {error}"))?;
    check_image_cancelled(cancel)?;
    let decode_started = thumbnail_timing_started(timings_enabled);
    let image = image::load_from_memory_with_format(&bytes, format);
    thumbnail_timing_finished(&mut timings.raster_decode, decode_started);
    let image = image.map_err(|error| format!("Failed to decode image: {error}"))?;
    check_image_cancelled(cancel)?;
    let rgba_started = thumbnail_timing_started(timings_enabled);
    let image = image.into_rgba8();
    thumbnail_timing_finished(&mut timings.rgba_convert, rgba_started);
    Ok(image)
}

fn load_raster_thumbnail_rgba_with_cancel_timed(
    path: &Path,
    cancel: &AtomicBool,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<image::RgbaImage, String> {
    check_image_cancelled(cancel)?;
    let reader = open_image_reader_with_extension_timed(path, timings_enabled, timings)?;
    check_image_cancelled(cancel)?;

    match decode_image_reader_to_rgba_timed(reader, timings_enabled, timings) {
        Ok(image) => Ok(image),
        Err(extension_error) => {
            check_image_cancelled(cancel)?;
            let reader =
                open_image_reader_with_guessed_format_timed(path, timings_enabled, timings)?;
            check_image_cancelled(cancel)?;
            decode_image_reader_to_rgba_timed(reader, timings_enabled, timings).map_err(
                |guess_error| {
                    format!("{guess_error}; extension-based decode also failed: {extension_error}")
                },
            )
        }
    }
}

fn open_image_reader_with_extension_timed(
    path: &Path,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<image::ImageReader<BufReader<File>>, String> {
    let read_started = thumbnail_timing_started(timings_enabled);
    let reader = image::ImageReader::open(path);
    thumbnail_timing_add_finished(&mut timings.source_read, read_started);
    let reader = reader.map_err(|error| format!("Failed to read image file: {error}"))?;

    let format_started = thumbnail_timing_started(timings_enabled);
    let _ = reader.format();
    thumbnail_timing_add_finished(&mut timings.format_detect, format_started);

    Ok(reader)
}

fn open_image_reader_with_guessed_format_timed(
    path: &Path,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<image::ImageReader<BufReader<File>>, String> {
    let read_started = thumbnail_timing_started(timings_enabled);
    let reader = image::ImageReader::open(path);
    thumbnail_timing_add_finished(&mut timings.source_read, read_started);
    let reader = reader.map_err(|error| format!("Failed to read image file: {error}"))?;

    let format_started = thumbnail_timing_started(timings_enabled);
    let reader = reader
        .with_guessed_format()
        .map_err(|error| format!("Failed to detect image format: {error}"));
    thumbnail_timing_add_finished(&mut timings.format_detect, format_started);

    reader
}

fn decode_image_reader_to_rgba_timed(
    reader: image::ImageReader<BufReader<File>>,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<image::RgbaImage, String> {
    let decode_started = thumbnail_timing_started(timings_enabled);
    let image = reader.decode();
    thumbnail_timing_add_finished(&mut timings.raster_decode, decode_started);
    let image = image.map_err(|error| format!("Failed to decode image: {error}"))?;

    let rgba_started = thumbnail_timing_started(timings_enabled);
    let image = image.into_rgba8();
    thumbnail_timing_add_finished(&mut timings.rgba_convert, rgba_started);
    Ok(image)
}

fn load_embedded_jpeg_thumbnail_rgba_with_cancel_timed(
    path: &Path,
    cancel: &AtomicBool,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<Option<image::RgbaImage>, String> {
    check_image_cancelled(cancel)?;
    let scan_started = thumbnail_timing_started(timings_enabled);
    let thumbnail = File::open(path).ok().and_then(|file| {
        let mut reader = BufReader::new(file);
        exif::Reader::new()
            .read_from_container(&mut reader)
            .ok()
            .and_then(|exif| embedded_jpeg_thumbnail_bytes(&exif).map(Vec::from))
    });
    thumbnail_timing_finished(&mut timings.embedded_thumbnail_scan, scan_started);
    let Some(thumbnail) = thumbnail else {
        return Ok(None);
    };

    check_image_cancelled(cancel)?;
    let decode_started = thumbnail_timing_started(timings_enabled);
    let image = image::load_from_memory_with_format(&thumbnail, image::ImageFormat::Jpeg).ok();
    thumbnail_timing_finished(&mut timings.embedded_thumbnail_decode, decode_started);
    let Some(image) = image else {
        return Ok(None);
    };

    check_image_cancelled(cancel)?;
    let rgba_started = thumbnail_timing_started(timings_enabled);
    let image = image.into_rgba8();
    thumbnail_timing_add_finished(&mut timings.rgba_convert, rgba_started);
    Ok(Some(image))
}

fn embedded_jpeg_thumbnail_bytes(exif: &exif::Exif) -> Option<&[u8]> {
    let offset = exif
        .get_field(exif::Tag::JPEGInterchangeFormat, exif::In::THUMBNAIL)?
        .value
        .get_uint(0)? as usize;
    let len = exif
        .get_field(exif::Tag::JPEGInterchangeFormatLength, exif::In::THUMBNAIL)?
        .value
        .get_uint(0)? as usize;
    let end = offset.checked_add(len)?;
    exif.buf().get(offset..end)
}

fn load_svg_rgba_with_cancel_timed(
    path: &Path,
    longest_side: u32,
    cancel: &AtomicBool,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<image::RgbaImage, String> {
    check_image_cancelled(cancel)?;
    let read_started = thumbnail_timing_started(timings_enabled);
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            thumbnail_timing_finished(&mut timings.source_read, read_started);
            return Err(format!("Failed to read SVG file: {error}"));
        }
    };
    thumbnail_timing_finished(&mut timings.source_read, read_started);
    check_image_cancelled(cancel)?;
    let options = usvg::Options::default();
    let parse_started = thumbnail_timing_started(timings_enabled);
    let tree = usvg::Tree::from_data(&bytes, &options);
    thumbnail_timing_finished(&mut timings.svg_parse, parse_started);
    let tree = tree.map_err(|error| format!("Failed to parse SVG: {error}"))?;
    check_image_cancelled(cancel)?;
    let render_started = thumbnail_timing_started(timings_enabled);
    let image = render_svg_rgba(&tree, longest_side);
    thumbnail_timing_finished(&mut timings.svg_render, render_started);
    let mut image = image?;
    check_image_cancelled(cancel)?;

    let unpremultiply_started = thumbnail_timing_started(timings_enabled);
    for pixel in image.chunks_exact_mut(4) {
        unpremultiply_rgba(pixel);
    }
    thumbnail_timing_finished(&mut timings.svg_unpremultiply, unpremultiply_started);

    Ok(image)
}

fn render_svg_rgba(tree: &usvg::Tree, longest_side: u32) -> Result<image::RgbaImage, String> {
    let svg_size = tree.size();
    let (width, height) = svg_raster_dimensions(svg_size.width(), svg_size.height(), longest_side)
        .ok_or_else(|| "SVG has no renderable dimensions.".to_owned())?;
    let scale = width as f32 / svg_size.width();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| "SVG raster target has invalid dimensions.".to_owned())?;
    resvg::render(
        tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    image::RgbaImage::from_raw(width, height, pixmap.take())
        .ok_or_else(|| "SVG rasterizer returned invalid pixel data.".to_owned())
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
    let resized = image::imageops::thumbnail(&image, resized_width, resized_height);
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
    image::codecs::png::PngEncoder::new_with_quality(
        &mut bytes,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::NoFilter,
    )
    .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
    .ok()?;
    bytes.starts_with(PNG_SIGNATURE).then_some(bytes)
}

fn path_may_be_jpeg(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "jpe"
            )
        })
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

fn thumbnail_timing_started(enabled: bool) -> Option<Instant> {
    enabled.then(Instant::now)
}

fn thumbnail_timing_finished(slot: &mut Option<Duration>, started: Option<Instant>) {
    if let Some(started) = started {
        *slot = Some(started.elapsed());
    }
}

fn thumbnail_timing_add_finished(slot: &mut Option<Duration>, started: Option<Instant>) {
    if let Some(started) = started {
        let elapsed = started.elapsed();
        *slot = Some(slot.unwrap_or_default() + elapsed);
    }
}

#[cfg(feature = "benchmarks")]
pub mod benchmark_support {
    use super::*;

    pub fn load_image_thumbnail_for_benchmark(path: &Path, size: u32) -> Result<Vec<u8>, String> {
        let cancel = AtomicBool::new(false);
        load_image_thumbnail_png_with_cancel_timed(path, size, &cancel, false).result
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

        assert!(thumbnail.starts_with(PNG_SIGNATURE));
        assert_eq!(decoded.dimensions(), (128, 128));
    }

    #[test]
    fn timed_raster_thumbnail_records_extraction_stages() {
        let temp = TempDir::new();
        let path = temp.path().join("image.png");
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(4, 2));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        fs::write(&path, bytes).unwrap();
        let cancel = AtomicBool::new(false);

        let thumbnail = load_image_thumbnail_png_with_cancel_timed(&path, 128, &cancel, true);

        assert!(thumbnail.result.is_ok());
        assert!(thumbnail.timings.source_read.is_some());
        assert!(thumbnail.timings.format_detect.is_some());
        assert!(thumbnail.timings.raster_decode.is_some());
        assert!(thumbnail.timings.rgba_convert.is_some());
        assert!(thumbnail.timings.resize_canvas.is_some());
        assert!(thumbnail.timings.png_encode.is_some());
        assert!(thumbnail.timings.embedded_thumbnail_scan.is_none());
        assert!(thumbnail.timings.embedded_thumbnail_decode.is_none());
        assert!(thumbnail.timings.svg_parse.is_none());
        assert!(thumbnail.timings.svg_render.is_none());
        assert!(thumbnail.timings.svg_unpremultiply.is_none());
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

    #[test]
    fn timed_svg_thumbnail_records_extraction_stages() {
        let temp = TempDir::new();
        let path = temp.path().join("vector.svg");
        fs::write(
            &path,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="250"><rect width="1000" height="250" fill="red"/></svg>"#,
        )
        .unwrap();
        let cancel = AtomicBool::new(false);

        let thumbnail = load_image_thumbnail_png_with_cancel_timed(&path, 128, &cancel, true);

        assert!(thumbnail.result.is_ok());
        assert!(thumbnail.timings.source_read.is_some());
        assert!(thumbnail.timings.svg_parse.is_some());
        assert!(thumbnail.timings.svg_render.is_some());
        assert!(thumbnail.timings.svg_unpremultiply.is_some());
        assert!(thumbnail.timings.resize_canvas.is_some());
        assert!(thumbnail.timings.png_encode.is_some());
        assert!(thumbnail.timings.embedded_thumbnail_scan.is_none());
        assert!(thumbnail.timings.embedded_thumbnail_decode.is_none());
        assert!(thumbnail.timings.format_detect.is_none());
        assert!(thumbnail.timings.raster_decode.is_none());
        assert!(thumbnail.timings.rgba_convert.is_none());
    }

    #[test]
    fn thumbnail_decode_falls_back_to_content_sniffing_for_mismatched_extension() {
        let temp = TempDir::new();
        let path = temp.path().join("actually-png.jpg");
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
    fn jpeg_thumbnail_uses_embedded_exif_thumbnail_when_present() {
        let temp = TempDir::new();
        let path = temp.path().join("photo.jpg");
        let primary = jpeg_bytes(16, 16, [220, 20, 20]);
        let embedded = jpeg_bytes(2, 1, [20, 220, 20]);
        fs::write(&path, jpeg_with_embedded_thumbnail(&primary, &embedded)).unwrap();
        let cancel = AtomicBool::new(false);

        let thumbnail = load_image_thumbnail_png_with_cancel_timed(&path, 128, &cancel, true);

        assert!(thumbnail.result.is_ok());
        assert!(thumbnail.timings.embedded_thumbnail_scan.is_some());
        assert!(thumbnail.timings.embedded_thumbnail_decode.is_some());
        assert!(thumbnail.timings.raster_decode.is_none());

        let bytes = thumbnail.result.unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap().into_rgba8();
        let pixel = decoded.get_pixel(64, 64);
        assert!(
            pixel[1] > pixel[0],
            "expected embedded green thumbnail to be used, got {pixel:?}"
        );
    }

    fn jpeg_bytes(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            width,
            height,
            image::Rgb(rgb),
        ));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
            .unwrap();
        bytes
    }

    fn jpeg_with_embedded_thumbnail(primary: &[u8], thumbnail: &[u8]) -> Vec<u8> {
        assert!(primary.starts_with(&[0xff, 0xd8]));
        let tiff = exif_tiff_with_jpeg_thumbnail(thumbnail);
        let app1_len = 2 + 6 + tiff.len();
        let mut jpeg = Vec::new();
        jpeg.extend_from_slice(&primary[..2]);
        jpeg.extend_from_slice(&[0xff, 0xe1]);
        jpeg.extend_from_slice(&(app1_len as u16).to_be_bytes());
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(&tiff);
        jpeg.extend_from_slice(&primary[2..]);
        jpeg
    }

    fn exif_tiff_with_jpeg_thumbnail(thumbnail: &[u8]) -> Vec<u8> {
        let ifd0_offset = 8usize;
        let ifd1_offset = ifd0_offset + 2 + 4;
        let jpeg_offset = ifd1_offset + 2 + 2 * 12 + 4;
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II");
        tiff.extend_from_slice(&42u16.to_le_bytes());
        tiff.extend_from_slice(&(ifd0_offset as u32).to_le_bytes());
        tiff.extend_from_slice(&0u16.to_le_bytes());
        tiff.extend_from_slice(&(ifd1_offset as u32).to_le_bytes());
        tiff.extend_from_slice(&2u16.to_le_bytes());
        push_ifd_entry(&mut tiff, 0x0201, 4, 1, jpeg_offset as u32);
        push_ifd_entry(&mut tiff, 0x0202, 4, 1, thumbnail.len() as u32);
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.extend_from_slice(thumbnail);
        tiff
    }

    fn push_ifd_entry(tiff: &mut Vec<u8>, tag: u16, field_type: u16, count: u32, value: u32) {
        tiff.extend_from_slice(&tag.to_le_bytes());
        tiff.extend_from_slice(&field_type.to_le_bytes());
        tiff.extend_from_slice(&count.to_le_bytes());
        tiff.extend_from_slice(&value.to_le_bytes());
    }

    fn assert_render_image_size(preview: &PropertyImagePreview, width: u32, height: u32) {
        let size = preview.image.size(0);
        assert_eq!(size.width.0, width as i32);
        assert_eq!(size.height.0, height as i32);
    }
}
