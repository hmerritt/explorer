use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufReader, Read, Seek, SeekFrom},
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
const TIFF_LITTLE_ENDIAN_SIGNATURE: &[u8] = b"II*\x00";
const TIFF_BIG_ENDIAN_SIGNATURE: &[u8] = b"MM\x00*";
const BIG_TIFF_LITTLE_ENDIAN_SIGNATURE: &[u8] = b"II+\x00";
const BIG_TIFF_BIG_ENDIAN_SIGNATURE: &[u8] = b"MM\x00+";
const TIFF_REDUCED_IMAGE_SUBFILE_BIT: u32 = 1;
const TIFF_OLD_REDUCED_IMAGE_SUBFILE_TYPE: u16 = 2;
const TIFF_ASPECT_RATIO_TOLERANCE: f64 = 0.05;
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
    pub(super) tiff_ifd_scan: Option<Duration>,
    pub(super) tiff_raw_sample: Option<Duration>,
    pub(super) tiff_chunk_decode: Option<Duration>,
    pub(super) tiff_chunk_sample: Option<Duration>,
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

pub(super) fn hover_image_preview_dimensions(
    path: &Path,
    longest_side: u32,
) -> Result<(u32, u32), String> {
    if longest_side == 0 {
        return Err("Image preview target has no dimensions.".to_owned());
    }

    if path_is_svg(path) {
        let bytes = fs::read(path).map_err(|error| format!("Failed to read SVG file: {error}"))?;
        let tree = usvg::Tree::from_data(&bytes, &usvg::Options::default())
            .map_err(|error| format!("Failed to parse SVG: {error}"))?;
        let size = tree.size();
        return svg_raster_dimensions(size.width(), size.height(), longest_side)
            .ok_or_else(|| "SVG has no renderable dimensions.".to_owned());
    }

    let (width, height) = image::image_dimensions(path)
        .map_err(|error| format!("Failed to read image dimensions: {error}"))?;
    thumbnail_content_dimensions(width, height, longest_side)
        .ok_or_else(|| "Image has no dimensions.".to_owned())
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

pub(super) fn load_hover_image_preview_png_with_cancel_timed(
    path: &Path,
    size: u32,
    cancel: &AtomicBool,
    timings_enabled: bool,
) -> TimedImageThumbnailPng {
    let mut timings = ImageThumbnailExtractionTimings::default();
    let result = load_hover_image_preview_png_with_cancel_timed_result(
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

fn load_hover_image_preview_png_with_cancel_timed_result(
    path: &Path,
    size: u32,
    cancel: &AtomicBool,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<Vec<u8>, String> {
    if size == 0 {
        return Err("Image preview target has no dimensions.".to_owned());
    }

    check_image_cancelled(cancel)?;
    let image =
        load_image_thumbnail_rgba_with_cancel_timed(path, size, cancel, timings_enabled, timings)?;
    check_image_cancelled(cancel)?;
    let resize_started = thumbnail_timing_started(timings_enabled);
    let preview = resize_rgba_image_to_longest_side(image, size);
    thumbnail_timing_finished(&mut timings.resize_canvas, resize_started);
    let preview = preview?;
    check_image_cancelled(cancel)?;
    let encode_started = thumbnail_timing_started(timings_enabled);
    let encoded = encode_rgba_png_bytes(preview.as_raw(), preview.width(), preview.height());
    thumbnail_timing_finished(&mut timings.png_encode, encode_started);
    encoded.ok_or_else(|| "Failed to encode image preview.".to_owned())
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

    load_raster_thumbnail_rgba_with_cancel_timed(
        path,
        svg_longest_side,
        cancel,
        timings_enabled,
        timings,
    )
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
    thumbnail_longest_side: u32,
    cancel: &AtomicBool,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<image::RgbaImage, String> {
    check_image_cancelled(cancel)?;
    if path_should_try_tiff_fast_path(path, timings_enabled, timings) {
        match load_tiff_thumbnail_rgba_with_cancel_timed(
            path,
            thumbnail_longest_side,
            cancel,
            timings_enabled,
            timings,
        ) {
            Ok(image) => return Ok(image),
            Err(TiffFastThumbnailError::Cancelled) => {
                return Err("Image thumbnail loading was cancelled.".to_owned());
            }
            Err(TiffFastThumbnailError::Unsupported) => check_image_cancelled(cancel)?,
        }
    }

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

#[derive(Debug)]
enum TiffFastThumbnailError {
    Unsupported,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TiffPixelKind {
    Gray,
    Rgb,
    Rgba,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TiffPixelLayout {
    kind: TiffPixelKind,
    samples: usize,
    bit_depth: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TiffImageMetadata {
    width: u32,
    height: u32,
    layout: TiffPixelLayout,
    white_is_zero: bool,
}

struct TiffDecodedChunk {
    width: u32,
    height: u32,
    pixels: tiff::decoder::DecodingResult,
}

fn path_should_try_tiff_fast_path(
    path: &Path,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> bool {
    if path_may_be_tiff(path) {
        return true;
    }
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| image::ImageFormat::from_extension(extension).is_some())
    {
        return false;
    }

    let read_started = thumbnail_timing_started(timings_enabled);
    let mut signature = [0u8; 4];
    let is_tiff = File::open(path)
        .and_then(|mut file| file.read_exact(&mut signature))
        .is_ok_and(|_| bytes_have_tiff_signature(&signature));
    thumbnail_timing_add_finished(&mut timings.source_read, read_started);
    is_tiff
}

fn load_tiff_thumbnail_rgba_with_cancel_timed(
    path: &Path,
    thumbnail_longest_side: u32,
    cancel: &AtomicBool,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<image::RgbaImage, TiffFastThumbnailError> {
    check_tiff_cancelled(cancel)?;
    let read_started = thumbnail_timing_started(timings_enabled);
    let file = File::open(path).map_err(|_| TiffFastThumbnailError::Unsupported);
    thumbnail_timing_add_finished(&mut timings.source_read, read_started);
    let file = file?;

    check_tiff_cancelled(cancel)?;
    let ifd_started = thumbnail_timing_started(timings_enabled);
    let result = open_and_select_tiff_image(BufReader::new(file), cancel);
    thumbnail_timing_finished(&mut timings.tiff_ifd_scan, ifd_started);
    let mut decoder = result?;

    check_tiff_cancelled(cancel)?;
    let metadata = tiff_current_image_metadata(&mut decoder)?;
    match load_uncompressed_stripped_tiff_thumbnail_rgba(
        &mut decoder,
        metadata,
        thumbnail_longest_side,
        cancel,
        timings_enabled,
        timings,
    ) {
        Ok(image) => Ok(image),
        Err(TiffFastThumbnailError::Unsupported) => load_chunked_tiff_thumbnail_rgba(
            &mut decoder,
            metadata,
            thumbnail_longest_side,
            cancel,
            timings_enabled,
            timings,
        ),
        Err(TiffFastThumbnailError::Cancelled) => Err(TiffFastThumbnailError::Cancelled),
    }
}

fn open_and_select_tiff_image<R: Read + Seek>(
    reader: R,
    cancel: &AtomicBool,
) -> Result<tiff::decoder::Decoder<R>, TiffFastThumbnailError> {
    let mut decoder =
        tiff::decoder::Decoder::new(reader).map_err(|_| TiffFastThumbnailError::Unsupported)?;
    select_reduced_tiff_image(&mut decoder, cancel)?;
    Ok(decoder)
}

fn select_reduced_tiff_image<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
    cancel: &AtomicBool,
) -> Result<(), TiffFastThumbnailError> {
    check_tiff_cancelled(cancel)?;
    let primary = decoder
        .dimensions()
        .map_err(|_| TiffFastThumbnailError::Unsupported)?;
    let mut best_reduced_index = None;
    let mut best_reduced_area = u64::MAX;
    let mut index = 0usize;

    loop {
        check_tiff_cancelled(cancel)?;
        let dimensions = decoder
            .dimensions()
            .map_err(|_| TiffFastThumbnailError::Unsupported)?;
        if index > 0
            && tiff_current_ifd_is_reduced(decoder)?
            && tiff_aspect_ratio_compatible(primary, dimensions)
        {
            let area = u64::from(dimensions.0).saturating_mul(u64::from(dimensions.1));
            if area < best_reduced_area {
                best_reduced_area = area;
                best_reduced_index = Some(index);
            }
        }

        if !decoder.more_images() {
            break;
        }
        decoder
            .next_image()
            .map_err(|_| TiffFastThumbnailError::Unsupported)?;
        index += 1;
    }

    decoder
        .seek_to_image(best_reduced_index.unwrap_or(0))
        .map_err(|_| TiffFastThumbnailError::Unsupported)
}

fn tiff_current_ifd_is_reduced<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
) -> Result<bool, TiffFastThumbnailError> {
    let new_subfile_type = decoder
        .find_tag_unsigned::<u32>(tiff::tags::Tag::NewSubfileType)
        .map_err(|_| TiffFastThumbnailError::Unsupported)?
        .is_some_and(|value| value & TIFF_REDUCED_IMAGE_SUBFILE_BIT != 0);
    let old_subfile_type = decoder
        .find_tag_unsigned::<u16>(tiff::tags::Tag::SubfileType)
        .map_err(|_| TiffFastThumbnailError::Unsupported)?
        .is_some_and(|value| value == TIFF_OLD_REDUCED_IMAGE_SUBFILE_TYPE);
    Ok(new_subfile_type || old_subfile_type)
}

fn tiff_aspect_ratio_compatible(primary: (u32, u32), candidate: (u32, u32)) -> bool {
    if primary.0 == 0 || primary.1 == 0 || candidate.0 == 0 || candidate.1 == 0 {
        return false;
    }

    let lhs = f64::from(primary.0) * f64::from(candidate.1);
    let rhs = f64::from(primary.1) * f64::from(candidate.0);
    let scale = lhs.max(rhs).max(1.0);
    ((lhs - rhs).abs() / scale) <= TIFF_ASPECT_RATIO_TOLERANCE
}

fn tiff_current_image_metadata<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
) -> Result<TiffImageMetadata, TiffFastThumbnailError> {
    let (width, height) = decoder
        .dimensions()
        .map_err(|_| TiffFastThumbnailError::Unsupported)?;
    if width == 0 || height == 0 {
        return Err(TiffFastThumbnailError::Unsupported);
    }

    let color_type = decoder
        .colortype()
        .map_err(|_| TiffFastThumbnailError::Unsupported)?;
    let layout = tiff_pixel_layout(color_type)?;
    if !tiff_current_image_is_chunky(decoder)? || !tiff_current_image_is_unsigned(decoder)? {
        return Err(TiffFastThumbnailError::Unsupported);
    }
    let white_is_zero = decoder
        .find_tag_unsigned::<u16>(tiff::tags::Tag::PhotometricInterpretation)
        .map_err(|_| TiffFastThumbnailError::Unsupported)?
        .is_some_and(|value| value == 0);

    Ok(TiffImageMetadata {
        width,
        height,
        layout,
        white_is_zero,
    })
}

fn tiff_pixel_layout(
    color_type: tiff::ColorType,
) -> Result<TiffPixelLayout, TiffFastThumbnailError> {
    let (kind, samples, bit_depth) = match color_type {
        tiff::ColorType::Gray(8) => (TiffPixelKind::Gray, 1, 8),
        tiff::ColorType::Gray(16) => (TiffPixelKind::Gray, 1, 16),
        tiff::ColorType::RGB(8) => (TiffPixelKind::Rgb, 3, 8),
        tiff::ColorType::RGB(16) => (TiffPixelKind::Rgb, 3, 16),
        tiff::ColorType::RGBA(8) => (TiffPixelKind::Rgba, 4, 8),
        tiff::ColorType::RGBA(16) => (TiffPixelKind::Rgba, 4, 16),
        _ => return Err(TiffFastThumbnailError::Unsupported),
    };
    Ok(TiffPixelLayout {
        kind,
        samples,
        bit_depth,
    })
}

fn tiff_current_image_is_chunky<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
) -> Result<bool, TiffFastThumbnailError> {
    decoder
        .find_tag_unsigned::<u16>(tiff::tags::Tag::PlanarConfiguration)
        .map(|value| value.unwrap_or(1) == 1)
        .map_err(|_| TiffFastThumbnailError::Unsupported)
}

fn tiff_current_image_is_unsigned<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
) -> Result<bool, TiffFastThumbnailError> {
    decoder
        .find_tag_unsigned_vec::<u16>(tiff::tags::Tag::SampleFormat)
        .map(|formats| formats.is_none_or(|formats| formats.iter().all(|format| *format == 1)))
        .map_err(|_| TiffFastThumbnailError::Unsupported)
}

fn load_uncompressed_stripped_tiff_thumbnail_rgba<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
    metadata: TiffImageMetadata,
    thumbnail_longest_side: u32,
    cancel: &AtomicBool,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<image::RgbaImage, TiffFastThumbnailError> {
    if decoder.get_chunk_type() != tiff::decoder::ChunkType::Strip {
        return Err(TiffFastThumbnailError::Unsupported);
    }
    if decoder
        .find_tag_unsigned::<u16>(tiff::tags::Tag::Compression)
        .map_err(|_| TiffFastThumbnailError::Unsupported)?
        .unwrap_or(1)
        != 1
    {
        return Err(TiffFastThumbnailError::Unsupported);
    }
    if decoder
        .find_tag_unsigned::<u16>(tiff::tags::Tag::Predictor)
        .map_err(|_| TiffFastThumbnailError::Unsupported)?
        .unwrap_or(1)
        != 1
    {
        return Err(TiffFastThumbnailError::Unsupported);
    }

    let sample_started = thumbnail_timing_started(timings_enabled);
    let result = load_uncompressed_stripped_tiff_thumbnail_rgba_result(
        decoder,
        metadata,
        thumbnail_longest_side,
        cancel,
    );
    thumbnail_timing_finished(&mut timings.tiff_raw_sample, sample_started);
    result
}

fn load_uncompressed_stripped_tiff_thumbnail_rgba_result<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
    metadata: TiffImageMetadata,
    thumbnail_longest_side: u32,
    cancel: &AtomicBool,
) -> Result<image::RgbaImage, TiffFastThumbnailError> {
    check_tiff_cancelled(cancel)?;
    let rows_per_strip = decoder
        .find_tag_unsigned::<u32>(tiff::tags::Tag::RowsPerStrip)
        .map_err(|_| TiffFastThumbnailError::Unsupported)?
        .unwrap_or(metadata.height);
    if rows_per_strip == 0 {
        return Err(TiffFastThumbnailError::Unsupported);
    }

    let strip_offsets = decoder
        .find_tag_unsigned_vec::<u64>(tiff::tags::Tag::StripOffsets)
        .map_err(|_| TiffFastThumbnailError::Unsupported)?
        .ok_or(TiffFastThumbnailError::Unsupported)?;
    let strip_byte_counts = decoder
        .find_tag_unsigned_vec::<u64>(tiff::tags::Tag::StripByteCounts)
        .map_err(|_| TiffFastThumbnailError::Unsupported)?
        .ok_or(TiffFastThumbnailError::Unsupported)?;
    let strip_count = metadata.height.div_ceil(rows_per_strip) as usize;
    if strip_offsets.len() < strip_count || strip_byte_counts.len() < strip_count {
        return Err(TiffFastThumbnailError::Unsupported);
    }

    let row_bytes = tiff_row_bytes(metadata)?;
    let row_len = usize::try_from(row_bytes).map_err(|_| TiffFastThumbnailError::Unsupported)?;
    let pixel_bytes = tiff_pixel_bytes(metadata)?;
    let byte_order = decoder.byte_order();
    let (thumbnail_width, thumbnail_height) =
        thumbnail_content_dimensions(metadata.width, metadata.height, thumbnail_longest_side)
            .ok_or(TiffFastThumbnailError::Unsupported)?;
    let mut thumbnail = image::RgbaImage::new(thumbnail_width, thumbnail_height);
    let mut row = vec![0u8; row_len];

    for thumbnail_y in 0..thumbnail_height {
        check_tiff_cancelled(cancel)?;
        let source_y = nearest_source_pixel(thumbnail_y, thumbnail_height, metadata.height);
        let strip_index = (source_y / rows_per_strip) as usize;
        let row_in_strip = source_y % rows_per_strip;
        let row_start = u64::from(row_in_strip)
            .checked_mul(row_bytes)
            .and_then(|offset| strip_offsets[strip_index].checked_add(offset))
            .ok_or(TiffFastThumbnailError::Unsupported)?;
        let row_end_in_strip = u64::from(row_in_strip + 1)
            .checked_mul(row_bytes)
            .ok_or(TiffFastThumbnailError::Unsupported)?;
        if row_end_in_strip > strip_byte_counts[strip_index] {
            return Err(TiffFastThumbnailError::Unsupported);
        }

        let reader = decoder.inner();
        reader
            .seek(SeekFrom::Start(row_start))
            .map_err(|_| TiffFastThumbnailError::Unsupported)?;
        reader
            .read_exact(&mut row)
            .map_err(|_| TiffFastThumbnailError::Unsupported)?;

        for thumbnail_x in 0..thumbnail_width {
            let source_x = nearest_source_pixel(thumbnail_x, thumbnail_width, metadata.width);
            let offset = usize::try_from(source_x)
                .ok()
                .and_then(|source_x| source_x.checked_mul(pixel_bytes))
                .ok_or(TiffFastThumbnailError::Unsupported)?;
            let pixel = tiff_raw_pixel_to_rgba(&row, offset, metadata, byte_order)?;
            thumbnail.put_pixel(thumbnail_x, thumbnail_y, image::Rgba(pixel));
        }
    }

    Ok(thumbnail)
}

fn load_chunked_tiff_thumbnail_rgba<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
    metadata: TiffImageMetadata,
    thumbnail_longest_side: u32,
    cancel: &AtomicBool,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<image::RgbaImage, TiffFastThumbnailError> {
    let sample_started = thumbnail_timing_started(timings_enabled);
    let result = load_chunked_tiff_thumbnail_rgba_result(
        decoder,
        metadata,
        thumbnail_longest_side,
        cancel,
        timings_enabled,
        timings,
    );
    thumbnail_timing_finished(&mut timings.tiff_chunk_sample, sample_started);
    result
}

fn load_chunked_tiff_thumbnail_rgba_result<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
    metadata: TiffImageMetadata,
    thumbnail_longest_side: u32,
    cancel: &AtomicBool,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<image::RgbaImage, TiffFastThumbnailError> {
    check_tiff_cancelled(cancel)?;
    let (chunk_width, chunk_height) = decoder.chunk_dimensions();
    if chunk_width == 0 || chunk_height == 0 {
        return Err(TiffFastThumbnailError::Unsupported);
    }
    let chunks_across = metadata.width.div_ceil(chunk_width);
    let chunk_count = match decoder.get_chunk_type() {
        tiff::decoder::ChunkType::Strip => decoder
            .strip_count()
            .map_err(|_| TiffFastThumbnailError::Unsupported)?,
        tiff::decoder::ChunkType::Tile => decoder
            .tile_count()
            .map_err(|_| TiffFastThumbnailError::Unsupported)?,
    };
    let (thumbnail_width, thumbnail_height) =
        thumbnail_content_dimensions(metadata.width, metadata.height, thumbnail_longest_side)
            .ok_or(TiffFastThumbnailError::Unsupported)?;
    let mut thumbnail = image::RgbaImage::new(thumbnail_width, thumbnail_height);
    let mut chunks = HashMap::new();

    for thumbnail_y in 0..thumbnail_height {
        check_tiff_cancelled(cancel)?;
        let source_y = nearest_source_pixel(thumbnail_y, thumbnail_height, metadata.height);
        for thumbnail_x in 0..thumbnail_width {
            let source_x = nearest_source_pixel(thumbnail_x, thumbnail_width, metadata.width);
            let chunk_index = match decoder.get_chunk_type() {
                tiff::decoder::ChunkType::Strip => source_y / chunk_height,
                tiff::decoder::ChunkType::Tile => (source_y / chunk_height)
                    .checked_mul(chunks_across)
                    .and_then(|row| row.checked_add(source_x / chunk_width))
                    .ok_or(TiffFastThumbnailError::Unsupported)?,
            };
            if chunk_index >= chunk_count {
                return Err(TiffFastThumbnailError::Unsupported);
            }
            if !chunks.contains_key(&chunk_index) {
                let chunk = read_tiff_chunk(decoder, chunk_index, timings_enabled, timings)?;
                chunks.insert(chunk_index, chunk);
            }
            let chunk = chunks
                .get(&chunk_index)
                .ok_or(TiffFastThumbnailError::Unsupported)?;
            let local_x = source_x % chunk_width;
            let local_y = source_y % chunk_height;
            let pixel = tiff_chunk_pixel_to_rgba(chunk, local_x, local_y, metadata.layout)?;
            thumbnail.put_pixel(thumbnail_x, thumbnail_y, image::Rgba(pixel));
        }
    }

    Ok(thumbnail)
}

fn read_tiff_chunk<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
    chunk_index: u32,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<TiffDecodedChunk, TiffFastThumbnailError> {
    let decode_started = thumbnail_timing_started(timings_enabled);
    let pixels = decoder.read_chunk(chunk_index);
    thumbnail_timing_add_finished(&mut timings.tiff_chunk_decode, decode_started);
    let pixels = pixels.map_err(|_| TiffFastThumbnailError::Unsupported)?;
    let (width, height) = decoder.chunk_data_dimensions(chunk_index);
    Ok(TiffDecodedChunk {
        width,
        height,
        pixels,
    })
}

fn tiff_raw_pixel_to_rgba(
    row: &[u8],
    offset: usize,
    metadata: TiffImageMetadata,
    byte_order: tiff::tags::ByteOrder,
) -> Result<[u8; 4], TiffFastThumbnailError> {
    let sample = |index| tiff_raw_sample_to_u8(row, offset, metadata.layout, index, byte_order);
    tiff_samples_to_rgba(metadata.layout.kind, metadata.white_is_zero, sample)
}

fn tiff_raw_sample_to_u8(
    row: &[u8],
    offset: usize,
    layout: TiffPixelLayout,
    index: usize,
    byte_order: tiff::tags::ByteOrder,
) -> Result<u8, TiffFastThumbnailError> {
    let sample_offset = offset
        .checked_add(
            index
                .checked_mul(usize::from(layout.bit_depth / 8))
                .ok_or(TiffFastThumbnailError::Unsupported)?,
        )
        .ok_or(TiffFastThumbnailError::Unsupported)?;
    match layout.bit_depth {
        8 => row
            .get(sample_offset)
            .copied()
            .ok_or(TiffFastThumbnailError::Unsupported),
        16 => {
            let sample_end = sample_offset
                .checked_add(2)
                .ok_or(TiffFastThumbnailError::Unsupported)?;
            let bytes = row
                .get(sample_offset..sample_end)
                .ok_or(TiffFastThumbnailError::Unsupported)?;
            Ok(if byte_order == tiff::tags::ByteOrder::LittleEndian {
                bytes[1]
            } else {
                bytes[0]
            })
        }
        _ => Err(TiffFastThumbnailError::Unsupported),
    }
}

fn tiff_chunk_pixel_to_rgba(
    chunk: &TiffDecodedChunk,
    local_x: u32,
    local_y: u32,
    layout: TiffPixelLayout,
) -> Result<[u8; 4], TiffFastThumbnailError> {
    if local_x >= chunk.width || local_y >= chunk.height {
        return Err(TiffFastThumbnailError::Unsupported);
    }
    let pixel_index = usize::try_from(local_y)
        .ok()
        .and_then(|y| {
            usize::try_from(chunk.width)
                .ok()
                .and_then(|width| y.checked_mul(width))
        })
        .and_then(|row| {
            usize::try_from(local_x)
                .ok()
                .and_then(|x| row.checked_add(x))
        })
        .ok_or(TiffFastThumbnailError::Unsupported)?;

    match &chunk.pixels {
        tiff::decoder::DecodingResult::U8(pixels) => {
            let base = pixel_index
                .checked_mul(layout.samples)
                .ok_or(TiffFastThumbnailError::Unsupported)?;
            tiff_samples_to_rgba(layout.kind, false, |index| {
                pixels
                    .get(base + index)
                    .copied()
                    .ok_or(TiffFastThumbnailError::Unsupported)
            })
        }
        tiff::decoder::DecodingResult::U16(pixels) => {
            let base = pixel_index
                .checked_mul(layout.samples)
                .ok_or(TiffFastThumbnailError::Unsupported)?;
            tiff_samples_to_rgba(layout.kind, false, |index| {
                pixels
                    .get(base + index)
                    .map(|sample| (sample >> 8) as u8)
                    .ok_or(TiffFastThumbnailError::Unsupported)
            })
        }
        _ => Err(TiffFastThumbnailError::Unsupported),
    }
}

fn tiff_samples_to_rgba(
    kind: TiffPixelKind,
    white_is_zero: bool,
    mut sample: impl FnMut(usize) -> Result<u8, TiffFastThumbnailError>,
) -> Result<[u8; 4], TiffFastThumbnailError> {
    match kind {
        TiffPixelKind::Gray => {
            let value = sample(0)?;
            let value = if white_is_zero { 255 - value } else { value };
            Ok([value, value, value, 255])
        }
        TiffPixelKind::Rgb => Ok([sample(0)?, sample(1)?, sample(2)?, 255]),
        TiffPixelKind::Rgba => Ok([sample(0)?, sample(1)?, sample(2)?, sample(3)?]),
    }
}

fn tiff_row_bytes(metadata: TiffImageMetadata) -> Result<u64, TiffFastThumbnailError> {
    u64::from(metadata.width)
        .checked_mul(u64::try_from(metadata.layout.samples).unwrap_or(u64::MAX))
        .and_then(|bytes| bytes.checked_mul(u64::from(metadata.layout.bit_depth / 8)))
        .ok_or(TiffFastThumbnailError::Unsupported)
}

fn tiff_pixel_bytes(metadata: TiffImageMetadata) -> Result<usize, TiffFastThumbnailError> {
    metadata
        .layout
        .samples
        .checked_mul(usize::from(metadata.layout.bit_depth / 8))
        .ok_or(TiffFastThumbnailError::Unsupported)
}

fn nearest_source_pixel(destination: u32, destination_len: u32, source_len: u32) -> u32 {
    if destination_len == 0 || source_len == 0 {
        return 0;
    }

    let source = (u64::from(destination)
        .saturating_mul(u64::from(source_len))
        .saturating_add(u64::from(destination_len / 2)))
        / u64::from(destination_len);
    (source as u32).min(source_len - 1)
}

fn check_tiff_cancelled(cancel: &AtomicBool) -> Result<(), TiffFastThumbnailError> {
    if cancel.load(Ordering::Relaxed) {
        Err(TiffFastThumbnailError::Cancelled)
    } else {
        Ok(())
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
    let resized = resize_rgba_image_to_longest_side(image, size)?;
    let resized_width = resized.width();
    let resized_height = resized.height();
    let mut canvas = image::RgbaImage::from_pixel(size, size, image::Rgba([0, 0, 0, 0]));
    let x = ((size - resized_width) / 2) as i64;
    let y = ((size - resized_height) / 2) as i64;
    image::imageops::overlay(&mut canvas, &resized, x, y);
    Ok(canvas)
}

fn resize_rgba_image_to_longest_side(
    image: image::RgbaImage,
    size: u32,
) -> Result<image::RgbaImage, String> {
    let width = image.width();
    let height = image.height();
    let (resized_width, resized_height) = thumbnail_content_dimensions(width, height, size)
        .ok_or_else(|| "Image has no dimensions.".to_owned())?;
    Ok(image::imageops::thumbnail(
        &image,
        resized_width,
        resized_height,
    ))
}

fn thumbnail_content_dimensions(width: u32, height: u32, size: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 || size == 0 {
        return None;
    }

    let scale = size as f32 / width.max(height) as f32;
    let resized_width = ((width as f32 * scale).round() as u32).clamp(1, size);
    let resized_height = ((height as f32 * scale).round() as u32).clamp(1, size);
    Some((resized_width, resized_height))
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

fn path_may_be_tiff(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "tif" | "tiff"))
}

fn path_is_svg(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
}

fn bytes_have_tiff_signature(bytes: &[u8]) -> bool {
    bytes.starts_with(TIFF_LITTLE_ENDIAN_SIGNATURE)
        || bytes.starts_with(TIFF_BIG_ENDIAN_SIGNATURE)
        || bytes.starts_with(BIG_TIFF_LITTLE_ENDIAN_SIGNATURE)
        || bytes.starts_with(BIG_TIFF_BIG_ENDIAN_SIGNATURE)
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
    use std::io::{self, Cursor, Read, Seek, SeekFrom};

    #[test]
    fn image_preview_accepts_webp_extension() {
        assert!(path_may_have_image_preview(Path::new("poster.webp")));
    }

    #[test]
    fn image_preview_accepts_tiff_extensions() {
        assert!(path_may_have_image_preview(Path::new("scan.tif")));
        assert!(path_may_have_image_preview(Path::new("scan.tiff")));
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
    fn hover_preview_dimensions_probe_raster_aspect_ratio() {
        let temp = TempDir::new();
        let landscape = temp.path().join("landscape.png");
        let portrait = temp.path().join("portrait.png");
        write_test_png(&landscape, 8, 4);
        write_test_png(&portrait, 3, 6);

        assert_eq!(
            hover_image_preview_dimensions(&landscape, 400).unwrap(),
            (400, 200)
        );
        assert_eq!(
            hover_image_preview_dimensions(&portrait, 400).unwrap(),
            (200, 400)
        );
    }

    #[test]
    fn hover_preview_dimensions_probe_svg_aspect_ratio() {
        let temp = TempDir::new();
        let path = temp.path().join("image.svg");
        fs::write(
            &path,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="250"></svg>"#,
        )
        .unwrap();

        assert_eq!(
            hover_image_preview_dimensions(&path, 400).unwrap(),
            (400, 100)
        );
    }

    #[test]
    fn hover_preview_dimensions_reject_invalid_images() {
        let temp = TempDir::new();
        let path = temp.path().join("broken.png");
        fs::write(&path, b"not an image").unwrap();

        assert!(hover_image_preview_dimensions(&path, 400).is_err());
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
        assert!(thumbnail.timings.tiff_ifd_scan.is_none());
        assert!(thumbnail.timings.tiff_raw_sample.is_none());
        assert!(thumbnail.timings.tiff_chunk_decode.is_none());
        assert!(thumbnail.timings.tiff_chunk_sample.is_none());
    }

    #[test]
    fn tiff_thumbnail_uses_fast_raw_sampler_for_large_uncompressed_rgb_and_rgba() {
        let temp = TempDir::new();
        let fixtures = [
            (
                temp.path().join("rgb.tif"),
                tiff_rgb8_bytes(512, 384, &[30, 140, 220]),
            ),
            (
                temp.path().join("rgba.tiff"),
                tiff_rgba8_bytes(512, 384, &[220, 120, 30, 200]),
            ),
        ];

        for (path, bytes) in fixtures {
            fs::write(&path, bytes).unwrap();
            let cancel = AtomicBool::new(false);

            let thumbnail = load_image_thumbnail_png_with_cancel_timed(&path, 128, &cancel, true);

            assert!(thumbnail.result.is_ok());
            assert!(thumbnail.timings.tiff_ifd_scan.is_some());
            assert!(thumbnail.timings.tiff_raw_sample.is_some());
            assert!(thumbnail.timings.tiff_chunk_decode.is_none());
            assert!(thumbnail.timings.raster_decode.is_none());
            let decoded = image::load_from_memory(&thumbnail.result.unwrap())
                .unwrap()
                .into_rgba8();
            assert_eq!(decoded.dimensions(), (128, 128));
        }
    }

    #[test]
    fn tiff_thumbnail_fast_path_detects_tiff_magic_without_known_extension() {
        let temp = TempDir::new();
        let path = temp.path().join("scan");
        fs::write(&path, tiff_rgb8_bytes(64, 64, &[30, 140, 220])).unwrap();
        let cancel = AtomicBool::new(false);

        let thumbnail = load_image_thumbnail_png_with_cancel_timed(&path, 128, &cancel, true);

        assert!(thumbnail.result.is_ok());
        assert!(thumbnail.timings.tiff_ifd_scan.is_some());
        assert!(thumbnail.timings.tiff_raw_sample.is_some());
        assert!(thumbnail.timings.raster_decode.is_none());
    }

    #[test]
    fn tiff_thumbnail_fast_sampler_downcasts_16_bit_samples() {
        let temp = TempDir::new();
        let path = temp.path().join("rgb16.tif");
        fs::write(&path, tiff_rgb16_bytes(64, 64, &[0xf000, 0x1000, 0x8000])).unwrap();
        let cancel = AtomicBool::new(false);

        let thumbnail = load_image_thumbnail_png_with_cancel_timed(&path, 128, &cancel, true);

        assert!(thumbnail.result.is_ok());
        assert!(thumbnail.timings.tiff_raw_sample.is_some());
        assert!(thumbnail.timings.raster_decode.is_none());
        let decoded = image::load_from_memory(&thumbnail.result.unwrap())
            .unwrap()
            .into_rgba8();
        let pixel = decoded.get_pixel(64, 64);
        assert!(
            pixel[0] > 220 && pixel[1] < 32 && pixel[2] > 110 && pixel[2] < 150,
            "expected 16-bit samples to be downcast, got {pixel:?}"
        );
    }

    #[test]
    fn tiff_thumbnail_chunked_sampler_handles_white_is_zero_grayscale() {
        let temp = TempDir::new();
        let path = temp.path().join("white-is-zero.tif");
        let mut data = Vec::new();
        for _ in 0..4 {
            data.extend_from_slice(&[0, 0, 255, 255]);
        }
        fs::write(
            &path,
            tiff_white_is_zero_gray8_deflate_bytes(4, 4, 1, &data),
        )
        .unwrap();
        let cancel = AtomicBool::new(false);

        let thumbnail = load_image_thumbnail_png_with_cancel_timed(&path, 128, &cancel, true);

        assert!(thumbnail.result.is_ok());
        assert!(thumbnail.timings.tiff_ifd_scan.is_some());
        assert!(thumbnail.timings.tiff_raw_sample.is_none());
        assert!(thumbnail.timings.tiff_chunk_decode.is_some());
        assert!(thumbnail.timings.tiff_chunk_sample.is_some());
        assert!(thumbnail.timings.raster_decode.is_none());
        let decoded = image::load_from_memory(&thumbnail.result.unwrap())
            .unwrap()
            .into_rgba8();
        assert_eq!(decoded.dimensions(), (128, 128));

        let white = decoded.get_pixel(16, 64);
        assert!(
            white[0] > 240 && white[1] > 240 && white[2] > 240 && white[3] == 255,
            "expected encoded zero to render white, got {white:?}"
        );
        let black = decoded.get_pixel(96, 64);
        assert!(
            black[0] < 16 && black[1] < 16 && black[2] < 16 && black[3] == 255,
            "expected encoded 255 to render black, got {black:?}"
        );
    }

    #[test]
    fn tiff_thumbnail_falls_back_to_content_sniffing_for_png_payload() {
        let temp = TempDir::new();
        let path = temp.path().join("actually-png.tif");
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(4, 2));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        fs::write(&path, bytes).unwrap();
        let cancel = AtomicBool::new(false);

        let thumbnail = load_image_thumbnail_png_with_cancel_timed(&path, 128, &cancel, true);

        assert!(thumbnail.result.is_ok());
        assert!(thumbnail.timings.tiff_ifd_scan.is_some());
        assert!(thumbnail.timings.tiff_raw_sample.is_none());
        assert!(thumbnail.timings.raster_decode.is_some());
        let decoded = image::load_from_memory(&thumbnail.result.unwrap())
            .unwrap()
            .into_rgba8();
        assert_eq!(decoded.dimensions(), (128, 128));
    }

    #[test]
    fn unsupported_tiff_fast_path_falls_back_to_generic_decoder() {
        let temp = TempDir::new();
        let path = temp.path().join("cmyk.tif");
        fs::write(&path, tiff_cmyk8_bytes(16, 16, &[0, 255, 255, 0])).unwrap();
        let cancel = AtomicBool::new(false);

        let thumbnail = load_image_thumbnail_png_with_cancel_timed(&path, 128, &cancel, true);

        assert!(thumbnail.result.is_ok());
        assert!(thumbnail.timings.tiff_ifd_scan.is_some());
        assert!(thumbnail.timings.tiff_raw_sample.is_none());
        assert!(thumbnail.timings.raster_decode.is_some());
        let decoded = image::load_from_memory(&thumbnail.result.unwrap())
            .unwrap()
            .into_rgba8();
        assert_eq!(decoded.dimensions(), (128, 128));
    }

    #[test]
    fn tiff_fast_sampler_honors_cancellation_during_sampling() {
        let bytes = tiff_rgb8_bytes(512, 384, &[30, 140, 220]);
        let cancel = Arc::new(AtomicBool::new(false));
        let reader = CancellingReader {
            inner: Cursor::new(bytes),
            cancel: cancel.clone(),
            read_count: 0,
            cancel_after_read: usize::MAX,
        };
        let mut decoder = open_and_select_tiff_image(BufReader::new(reader), cancel.as_ref())
            .expect("open test tiff");
        let metadata = tiff_current_image_metadata(&mut decoder).expect("read tiff metadata");
        {
            let reader = decoder.inner().get_mut();
            reader.cancel_after_read = reader.read_count + 1;
        }
        let mut timings = ImageThumbnailExtractionTimings::default();

        let result = load_uncompressed_stripped_tiff_thumbnail_rgba(
            &mut decoder,
            metadata,
            128,
            cancel.as_ref(),
            true,
            &mut timings,
        );

        assert!(matches!(result, Err(TiffFastThumbnailError::Cancelled)));
        assert!(timings.tiff_raw_sample.is_some());
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

    fn tiff_rgb8_bytes(width: u32, height: u32, rgb: &[u8; 3]) -> Vec<u8> {
        let mut data = Vec::with_capacity((width * height * 3) as usize);
        for _ in 0..(width * height) {
            data.extend_from_slice(rgb);
        }
        encode_tiff::<tiff::encoder::colortype::RGB8, u8>(width, height, 16, &data)
    }

    fn tiff_rgba8_bytes(width: u32, height: u32, rgba: &[u8; 4]) -> Vec<u8> {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            data.extend_from_slice(rgba);
        }
        encode_tiff::<tiff::encoder::colortype::RGBA8, u8>(width, height, 16, &data)
    }

    fn tiff_rgb16_bytes(width: u32, height: u32, rgb: &[u16; 3]) -> Vec<u8> {
        let mut data = Vec::with_capacity((width * height * 3) as usize);
        for _ in 0..(width * height) {
            data.extend_from_slice(rgb);
        }
        encode_tiff::<tiff::encoder::colortype::RGB16, u16>(width, height, 16, &data)
    }

    fn tiff_cmyk8_bytes(width: u32, height: u32, cmyk: &[u8; 4]) -> Vec<u8> {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            data.extend_from_slice(cmyk);
        }
        encode_tiff::<tiff::encoder::colortype::CMYK8, u8>(width, height, 16, &data)
    }

    struct WhiteGray8;

    impl tiff::encoder::colortype::ColorType for WhiteGray8 {
        type Inner = u8;

        const TIFF_VALUE: tiff::tags::PhotometricInterpretation =
            tiff::tags::PhotometricInterpretation::WhiteIsZero;
        const BITS_PER_SAMPLE: &'static [u16] = &[8];
        const SAMPLE_FORMAT: &'static [tiff::tags::SampleFormat] =
            &[tiff::tags::SampleFormat::Uint];

        fn horizontal_predict(row: &[Self::Inner], result: &mut Vec<Self::Inner>) {
            result.extend_from_slice(row);
        }
    }

    fn tiff_white_is_zero_gray8_deflate_bytes(
        width: u32,
        height: u32,
        rows_per_strip: u32,
        data: &[u8],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let cursor = Cursor::new(&mut bytes);
            let mut encoder = tiff::encoder::TiffEncoder::new(cursor)
                .unwrap()
                .with_compression(tiff::encoder::Compression::Deflate(
                    tiff::encoder::DeflateLevel::Fast,
                ));
            let mut image = encoder.new_image::<WhiteGray8>(width, height).unwrap();
            image.rows_per_strip(rows_per_strip).unwrap();
            image.write_data(data).unwrap();
        }
        bytes
    }

    fn encode_tiff<C, T>(width: u32, height: u32, rows_per_strip: u32, data: &[T]) -> Vec<u8>
    where
        C: tiff::encoder::colortype::ColorType<Inner = T>,
        [T]: tiff::encoder::TiffValue,
    {
        let mut bytes = Vec::new();
        {
            let cursor = Cursor::new(&mut bytes);
            let mut encoder = tiff::encoder::TiffEncoder::new(cursor).unwrap();
            let mut image = encoder.new_image::<C>(width, height).unwrap();
            image.rows_per_strip(rows_per_strip).unwrap();
            image.write_data(data).unwrap();
        }
        bytes
    }

    struct CancellingReader {
        inner: Cursor<Vec<u8>>,
        cancel: Arc<AtomicBool>,
        read_count: usize,
        cancel_after_read: usize,
    }

    impl Read for CancellingReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let read = self.inner.read(buf)?;
            self.read_count += 1;
            if self.read_count >= self.cancel_after_read {
                self.cancel.store(true, Ordering::Relaxed);
            }
            Ok(read)
        }
    }

    impl Seek for CancellingReader {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.inner.seek(pos)
        }
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

    fn write_test_png(path: &Path, width: u32, height: u32) {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(width, height));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        fs::write(path, bytes).unwrap();
    }
}
