use std::{
    fs::{self, File},
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use gpui::{App, RenderImage};
#[cfg(any(test, feature = "benchmarks"))]
use image::ImageEncoder;
#[cfg(test)]
use std::time::UNIX_EPOCH;

use crate::explorer::image_resize::{dimensions_for_longest_side, resize_dynamic_to_rgba};

#[cfg(any(test, feature = "benchmarks"))]
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const TIFF_LITTLE_ENDIAN_SIGNATURE: &[u8] = b"II*\x00";
const TIFF_BIG_ENDIAN_SIGNATURE: &[u8] = b"MM\x00*";
const BIG_TIFF_LITTLE_ENDIAN_SIGNATURE: &[u8] = b"II+\x00";
const BIG_TIFF_BIG_ENDIAN_SIGNATURE: &[u8] = b"MM\x00+";
const TIFF_REDUCED_IMAGE_SUBFILE_BIT: u32 = 1;
const TIFF_OLD_REDUCED_IMAGE_SUBFILE_TYPE: u16 = 2;
const TIFF_ASPECT_RATIO_TOLERANCE: f64 = 0.05;
const TIFF_SPARSE_ROW_MIN_BYTES: usize = 8 * 1024 * 1024;
const TIFF_SPARSE_ROW_READ_RATIO: usize = 8;
#[cfg(test)]
const SVG_IMAGE_RASTER_LONGEST_SIDE: u32 = 500;

macro_rules! thumbnail_stages {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(usize)]
        pub(super) enum ThumbnailStage { $($variant),+ }

        impl ThumbnailStage {
            pub(super) const ALL: [Self; Self::COUNT] = [$(Self::$variant),+];
            pub(super) const COUNT: usize = [$(stringify!($variant)),+].len();
            pub(super) const fn name(self) -> &'static str {
                const NAMES: [&str; ThumbnailStage::COUNT] = [$($name),+];
                NAMES[self as usize]
            }
        }
    };
}

thumbnail_stages! {
    QueueWait => "queue_wait",
    WriterQueue => "writer_queue",
    CacheRead => "cache_read",
    CacheDecode => "cache_decode",
    CacheEncode => "cache_encode",
    CacheWrite => "cache_write",
    ManifestFlush => "manifest_flush",
    Extract => "extract",
    EmbeddedThumbnailScan => "embedded_thumbnail_scan",
    EmbeddedThumbnailDecode => "embedded_thumbnail_decode",
    SourceRead => "source_read",
    FormatDetect => "format_detect",
    RasterDecode => "raster_decode",
    RgbaConvert => "rgba_convert",
    TiffIfdScan => "tiff_ifd_scan",
    TiffRawSample => "tiff_raw_sample",
    TiffChunkDecode => "tiff_chunk_decode",
    TiffChunkSample => "tiff_chunk_sample",
    SvgParse => "svg_parse",
    SvgRender => "svg_render",
    SvgUnpremultiply => "svg_unpremultiply",
    ResizeCanvas => "resize_canvas",
    PngEncode => "png_encode",
    RenderPrepare => "render_prepare",
    Commit => "commit",
    RequestTotal => "request_total",
}

#[derive(Clone, Debug)]
pub(super) struct PropertyImagePreview {
    pub(super) image: Arc<RenderImage>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) animated_source: Option<AnimatedImageSource>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AnimatedImageSource {
    pub(super) path: PathBuf,
    pub(super) cache_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ImageThumbnailExtractionTimings {
    stages: [Option<Duration>; ThumbnailStage::COUNT],
}

impl Default for ImageThumbnailExtractionTimings {
    fn default() -> Self {
        Self {
            stages: [None; ThumbnailStage::COUNT],
        }
    }
}

impl ImageThumbnailExtractionTimings {
    pub(super) fn get(&self, stage: ThumbnailStage) -> Option<Duration> {
        self.stages[stage as usize]
    }

    pub(super) fn stages(&self) -> impl Iterator<Item = (ThumbnailStage, Option<Duration>)> + '_ {
        ThumbnailStage::ALL
            .into_iter()
            .map(|stage| (stage, self.get(stage)))
    }

    pub(super) fn record(&mut self, stage: ThumbnailStage, elapsed: Duration) {
        self.stages[stage as usize] = Some(elapsed);
    }

    pub(super) fn set(&mut self, stage: ThumbnailStage, elapsed: Option<Duration>) {
        self.stages[stage as usize] = elapsed;
    }

    fn finish(&mut self, stage: ThumbnailStage, started: Option<Instant>) {
        if let Some(started) = started {
            self.stages[stage as usize] = Some(started.elapsed());
        }
    }

    fn add(&mut self, stage: ThumbnailStage, started: Option<Instant>) {
        if let Some(started) = started {
            let slot = &mut self.stages[stage as usize];
            *slot = Some(slot.unwrap_or_default() + started.elapsed());
        }
    }
}

#[cfg(any(test, feature = "benchmarks"))]
#[cfg_attr(all(feature = "benchmarks", not(test)), allow(dead_code))]
pub(super) struct TimedImageThumbnailPng {
    pub(super) result: Result<Vec<u8>, String>,
    pub(super) timings: ImageThumbnailExtractionTimings,
}

pub(super) struct TimedImageThumbnailRgba {
    pub(super) result: Result<image::RgbaImage, String>,
    pub(super) timings: ImageThumbnailExtractionTimings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ThumbnailSpec {
    pub(super) longest_side: u32,
    pub(super) allow_embedded_preview: bool,
}

impl ThumbnailSpec {
    pub(super) const fn standard(longest_side: u32) -> Self {
        Self {
            longest_side,
            allow_embedded_preview: true,
        }
    }

    pub(super) const fn hover(longest_side: u32) -> Self {
        Self {
            longest_side,
            allow_embedded_preview: false,
        }
    }
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

#[cfg(test)]
pub(super) fn load_property_image_preview(path: &Path) -> Result<PropertyImagePreview, String> {
    let image = load_image_rgba(path, SVG_IMAGE_RASTER_LONGEST_SIDE)?;
    property_image_preview_from_rgba(image, property_animated_gif_source(path))
}

pub(super) fn path_may_have_animated_gif_preview(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("gif"))
}

pub(super) fn animated_gif_source_for_path(
    path: &Path,
    cache_key: String,
) -> Option<AnimatedImageSource> {
    path_may_have_animated_gif_preview(path).then(|| AnimatedImageSource {
        path: path.to_path_buf(),
        cache_key,
    })
}

pub(super) fn evict_animated_image_source_asset(source: &AnimatedImageSource, cx: &mut App) {
    let resource: gpui::Resource = source.path.clone().into();
    cx.remove_asset::<gpui::ImgResourceLoader>(&resource);
}

#[cfg(test)]
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
    dimensions_for_longest_side(width, height, longest_side)
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

#[cfg(any(test, feature = "benchmarks"))]
pub(super) fn load_image_thumbnail_png_with_cancel_timed(
    path: &Path,
    size: u32,
    cancel: &AtomicBool,
    timings_enabled: bool,
) -> TimedImageThumbnailPng {
    load_thumbnail_png_with_cancel_timed(
        path,
        ThumbnailSpec::standard(size),
        cancel,
        timings_enabled,
    )
}

#[cfg(any(test, feature = "benchmarks"))]
fn load_thumbnail_png_with_cancel_timed(
    path: &Path,
    spec: ThumbnailSpec,
    cancel: &AtomicBool,
    timings_enabled: bool,
) -> TimedImageThumbnailPng {
    let mut timings = ImageThumbnailExtractionTimings::default();
    let result = load_thumbnail_png_with_cancel_timed_result(
        path,
        spec,
        cancel,
        timings_enabled,
        &mut timings,
    );
    TimedImageThumbnailPng { result, timings }
}

pub(super) fn load_thumbnail_rgba_with_cancel_timed(
    path: &Path,
    spec: ThumbnailSpec,
    cancel: &AtomicBool,
    timings_enabled: bool,
) -> TimedImageThumbnailRgba {
    let mut timings = ImageThumbnailExtractionTimings::default();
    let result = load_thumbnail_rgba_with_cancel_timed_result(
        path,
        spec,
        cancel,
        timings_enabled,
        &mut timings,
    );
    TimedImageThumbnailRgba { result, timings }
}

#[cfg(test)]
pub(super) fn load_hover_image_preview_png_with_cancel_timed(
    path: &Path,
    size: u32,
    cancel: &AtomicBool,
    timings_enabled: bool,
) -> TimedImageThumbnailPng {
    load_thumbnail_png_with_cancel_timed(path, ThumbnailSpec::hover(size), cancel, timings_enabled)
}

#[cfg(any(test, feature = "benchmarks"))]
fn load_thumbnail_png_with_cancel_timed_result(
    path: &Path,
    spec: ThumbnailSpec,
    cancel: &AtomicBool,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<Vec<u8>, String> {
    let thumbnail =
        load_thumbnail_rgba_with_cancel_timed_result(path, spec, cancel, timings_enabled, timings)?;
    check_image_cancelled(cancel)?;
    let encode_started = thumbnail_timing_started(timings_enabled);
    let encoded = encode_rgba_png_bytes(thumbnail.as_raw(), thumbnail.width(), thumbnail.height());
    timings.finish(ThumbnailStage::PngEncode, encode_started);
    encoded.ok_or_else(|| "Failed to encode image thumbnail.".to_owned())
}

fn load_thumbnail_rgba_with_cancel_timed_result(
    path: &Path,
    spec: ThumbnailSpec,
    cancel: &AtomicBool,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<image::RgbaImage, String> {
    if spec.longest_side == 0 {
        return Err("Thumbnail target has no dimensions.".to_owned());
    }

    check_image_cancelled(cancel)?;
    let image =
        load_source_thumbnail_rgba_with_cancel_timed(path, spec, cancel, timings_enabled, timings)?;
    check_image_cancelled(cancel)?;
    Ok(image)
}

fn load_source_thumbnail_rgba_with_cancel_timed(
    path: &Path,
    spec: ThumbnailSpec,
    cancel: &AtomicBool,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<image::RgbaImage, String> {
    check_image_cancelled(cancel)?;
    if path_is_svg(path) {
        return load_svg_rgba_with_cancel_timed(
            path,
            spec.longest_side,
            cancel,
            timings_enabled,
            timings,
        );
    }

    if spec.allow_embedded_preview && path_may_be_jpeg(path) {
        if let Some(image) = load_embedded_jpeg_thumbnail_rgba_with_cancel_timed(
            path,
            spec.longest_side,
            cancel,
            timings_enabled,
            timings,
        )? {
            return Ok(image);
        }
    }

    load_raster_thumbnail_rgba_with_cancel_timed(
        path,
        spec.longest_side,
        cancel,
        timings_enabled,
        timings,
    )
}

#[cfg(test)]
fn load_image_rgba(path: &Path, svg_longest_side: u32) -> Result<image::RgbaImage, String> {
    let cancel = AtomicBool::new(false);
    load_image_rgba_with_cancel(path, svg_longest_side, &cancel)
}

#[cfg(test)]
fn load_image_rgba_with_cancel(
    path: &Path,
    svg_longest_side: u32,
    cancel: &AtomicBool,
) -> Result<image::RgbaImage, String> {
    let mut timings = ImageThumbnailExtractionTimings::default();
    load_image_rgba_with_cancel_timed(path, svg_longest_side, cancel, false, &mut timings)
}

#[cfg(test)]
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
            timings.finish(ThumbnailStage::SourceRead, read_started);
            return Err(format!("Failed to read image file: {error}"));
        }
    };
    timings.finish(ThumbnailStage::SourceRead, read_started);
    check_image_cancelled(cancel)?;
    let format_started = thumbnail_timing_started(timings_enabled);
    let format = image::guess_format(&bytes).or_else(|_| image::ImageFormat::from_path(path));
    timings.finish(ThumbnailStage::FormatDetect, format_started);
    let format = format.map_err(|error| format!("Unsupported image format: {error}"))?;
    check_image_cancelled(cancel)?;
    let decode_started = thumbnail_timing_started(timings_enabled);
    let image = image::load_from_memory_with_format(&bytes, format);
    timings.finish(ThumbnailStage::RasterDecode, decode_started);
    let image = image.map_err(|error| format!("Failed to decode image: {error}"))?;
    check_image_cancelled(cancel)?;
    let rgba_started = thumbnail_timing_started(timings_enabled);
    let image = image.into_rgba8();
    timings.finish(ThumbnailStage::RgbaConvert, rgba_started);
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

    match decode_image_reader_to_rgba_timed(
        reader,
        thumbnail_longest_side,
        timings_enabled,
        timings,
    ) {
        Ok(image) => Ok(image),
        Err(extension_error) => {
            check_image_cancelled(cancel)?;
            let reader =
                open_image_reader_with_guessed_format_timed(path, timings_enabled, timings)?;
            check_image_cancelled(cancel)?;
            decode_image_reader_to_rgba_timed(
                reader,
                thumbnail_longest_side,
                timings_enabled,
                timings,
            )
            .map_err(|guess_error| {
                format!("{guess_error}; extension-based decode also failed: {extension_error}")
            })
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

struct TiffDecodedChunk<'a> {
    width: u32,
    height: u32,
    pixels: &'a tiff::decoder::DecodingResult,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TiffImageCandidate {
    ifd_offset: u64,
    dimensions: (u32, u32),
    reduced: bool,
}

struct TiffRootReader<R> {
    inner: R,
    position: u64,
    patch_start: u64,
    patch: [u8; 8],
    patch_len: usize,
}

impl<R: Read + Seek> TiffRootReader<R> {
    fn new(mut inner: R, root_ifd_offset: Option<u64>) -> io::Result<Self> {
        let mut patch_start = 0;
        let mut patch = [0u8; 8];
        let mut patch_len = 0;

        if let Some(root_ifd_offset) = root_ifd_offset {
            inner.seek(SeekFrom::Start(0))?;
            let mut signature = [0u8; 4];
            inner.read_exact(&mut signature)?;
            match signature.as_slice() {
                TIFF_LITTLE_ENDIAN_SIGNATURE => {
                    patch_start = 4;
                    patch_len = 4;
                    patch[..4].copy_from_slice(
                        &u32::try_from(root_ifd_offset)
                            .map_err(|_| {
                                io::Error::new(
                                    io::ErrorKind::InvalidInput,
                                    "classic TIFF IFD offset exceeds 32 bits",
                                )
                            })?
                            .to_le_bytes(),
                    );
                }
                TIFF_BIG_ENDIAN_SIGNATURE => {
                    patch_start = 4;
                    patch_len = 4;
                    patch[..4].copy_from_slice(
                        &u32::try_from(root_ifd_offset)
                            .map_err(|_| {
                                io::Error::new(
                                    io::ErrorKind::InvalidInput,
                                    "classic TIFF IFD offset exceeds 32 bits",
                                )
                            })?
                            .to_be_bytes(),
                    );
                }
                BIG_TIFF_LITTLE_ENDIAN_SIGNATURE => {
                    patch_start = 8;
                    patch_len = 8;
                    patch.copy_from_slice(&root_ifd_offset.to_le_bytes());
                }
                BIG_TIFF_BIG_ENDIAN_SIGNATURE => {
                    patch_start = 8;
                    patch_len = 8;
                    patch.copy_from_slice(&root_ifd_offset.to_be_bytes());
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "TIFF signature not found",
                    ));
                }
            }
        }

        inner.seek(SeekFrom::Start(0))?;
        Ok(Self {
            inner,
            position: 0,
            patch_start,
            patch,
            patch_len,
        })
    }
}

impl<R: Read + Seek> Read for TiffRootReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read_start = self.position;
        let read = self.inner.read(buffer)?;
        let read_end = read_start.saturating_add(read as u64);
        let patch_end = self.patch_start.saturating_add(self.patch_len as u64);
        let overlap_start = read_start.max(self.patch_start);
        let overlap_end = read_end.min(patch_end);
        if overlap_start < overlap_end {
            let destination_start = (overlap_start - read_start) as usize;
            let source_start = (overlap_start - self.patch_start) as usize;
            let overlap_len = (overlap_end - overlap_start) as usize;
            buffer[destination_start..destination_start + overlap_len]
                .copy_from_slice(&self.patch[source_start..source_start + overlap_len]);
        }
        self.position = read_end;
        Ok(read)
    }
}

impl<R: Read + Seek> Seek for TiffRootReader<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.position = self.inner.seek(position)?;
        Ok(self.position)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TiffChunkSample {
    chunk_index: u32,
    output_offset: usize,
    local_x: u32,
    local_y: u32,
}

struct TiffSamplePlan {
    output_width: u32,
    output_height: u32,
    source_xs: Vec<u32>,
    source_ys: Vec<u32>,
}

impl TiffSamplePlan {
    fn new(
        metadata: TiffImageMetadata,
        thumbnail_longest_side: u32,
    ) -> Result<Self, TiffFastThumbnailError> {
        let (output_width, output_height) =
            dimensions_for_longest_side(metadata.width, metadata.height, thumbnail_longest_side)
                .ok_or(TiffFastThumbnailError::Unsupported)?;
        Ok(Self {
            output_width,
            output_height,
            source_xs: tiff_sampled_source_pixels(output_width, metadata.width),
            source_ys: tiff_sampled_source_pixels(output_height, metadata.height),
        })
    }

    fn source_x_offsets(&self, pixel_bytes: usize) -> Vec<usize> {
        self.source_xs
            .iter()
            .map(|source_x| *source_x as usize * pixel_bytes)
            .collect()
    }
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
    timings.add(ThumbnailStage::SourceRead, read_started);
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
    timings.add(ThumbnailStage::SourceRead, read_started);
    let file = file?;

    check_tiff_cancelled(cancel)?;
    let ifd_started = thumbnail_timing_started(timings_enabled);
    let reader = match TiffRootReader::new(file, None) {
        Ok(reader) => reader,
        Err(_) => {
            timings.finish(ThumbnailStage::TiffIfdScan, ifd_started);
            return Err(TiffFastThumbnailError::Unsupported);
        }
    };
    let mut scanner = match tiff::decoder::Decoder::new(BufReader::new(reader)) {
        Ok(scanner) => scanner,
        Err(_) => {
            timings.finish(ThumbnailStage::TiffIfdScan, ifd_started);
            return Err(TiffFastThumbnailError::Unsupported);
        }
    };
    let candidates = tiff_image_candidates(&mut scanner, thumbnail_longest_side, cancel);
    timings.finish(ThumbnailStage::TiffIfdScan, ifd_started);
    let candidates = candidates?;
    let mut already_tried = None;

    if let Some(candidate) = candidates.first().copied() {
        if scanner.ifd_pointer().map(|pointer| pointer.0) == Some(candidate.ifd_offset) {
            match decode_tiff_candidate(
                &mut scanner,
                candidate,
                thumbnail_longest_side,
                cancel,
                timings_enabled,
                timings,
            ) {
                Ok(image) => return Ok(image),
                Err(TiffFastThumbnailError::Cancelled) => {
                    return Err(TiffFastThumbnailError::Cancelled);
                }
                Err(TiffFastThumbnailError::Unsupported) => {
                    already_tried = Some(candidate.ifd_offset);
                }
            }
        }
    }

    for candidate in candidates {
        if already_tried == Some(candidate.ifd_offset) {
            continue;
        }
        check_tiff_cancelled(cancel)?;
        let read_started = thumbnail_timing_started(timings_enabled);
        let file = File::open(path).map_err(|_| TiffFastThumbnailError::Unsupported);
        timings.add(ThumbnailStage::SourceRead, read_started);
        let file = file?;
        let reader = TiffRootReader::new(file, Some(candidate.ifd_offset))
            .map_err(|_| TiffFastThumbnailError::Unsupported)?;
        let mut decoder = match tiff::decoder::Decoder::new(BufReader::new(reader)) {
            Ok(decoder) => decoder,
            Err(_) => continue,
        };
        match decode_tiff_candidate(
            &mut decoder,
            candidate,
            thumbnail_longest_side,
            cancel,
            timings_enabled,
            timings,
        ) {
            Ok(image) => return Ok(image),
            Err(TiffFastThumbnailError::Cancelled) => {
                return Err(TiffFastThumbnailError::Cancelled);
            }
            Err(TiffFastThumbnailError::Unsupported) => {}
        }
    }

    Err(TiffFastThumbnailError::Unsupported)
}

fn decode_tiff_candidate<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
    candidate: TiffImageCandidate,
    thumbnail_longest_side: u32,
    cancel: &AtomicBool,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<image::RgbaImage, TiffFastThumbnailError> {
    check_tiff_cancelled(cancel)?;
    let metadata = tiff_current_image_metadata(decoder)?;
    if metadata.width != candidate.dimensions.0 || metadata.height != candidate.dimensions.1 {
        return Err(TiffFastThumbnailError::Unsupported);
    }
    match load_uncompressed_stripped_tiff_thumbnail_rgba(
        decoder,
        metadata,
        thumbnail_longest_side,
        cancel,
        timings_enabled,
        timings,
    ) {
        Ok(image) => Ok(image),
        Err(TiffFastThumbnailError::Unsupported) => load_chunked_tiff_thumbnail_rgba(
            decoder,
            metadata,
            thumbnail_longest_side,
            cancel,
            timings_enabled,
            timings,
        ),
        Err(TiffFastThumbnailError::Cancelled) => Err(TiffFastThumbnailError::Cancelled),
    }
}

fn tiff_image_candidates<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
    thumbnail_longest_side: u32,
    cancel: &AtomicBool,
) -> Result<Vec<TiffImageCandidate>, TiffFastThumbnailError> {
    check_tiff_cancelled(cancel)?;
    let primary = decoder
        .dimensions()
        .map_err(|_| TiffFastThumbnailError::Unsupported)?;
    let target = dimensions_for_longest_side(primary.0, primary.1, thumbnail_longest_side)
        .ok_or(TiffFastThumbnailError::Unsupported)?;
    let primary_ifd_offset = decoder
        .ifd_pointer()
        .map(|pointer| pointer.0)
        .ok_or(TiffFastThumbnailError::Unsupported)?;
    let mut reduced = Vec::new();
    let mut index = 0usize;

    loop {
        check_tiff_cancelled(cancel)?;
        let dimensions = decoder
            .dimensions()
            .map_err(|_| TiffFastThumbnailError::Unsupported)?;
        if index > 0
            && tiff_current_ifd_is_reduced(decoder).unwrap_or(false)
            && tiff_aspect_ratio_compatible(primary, dimensions)
        {
            if let Some(ifd_offset) = decoder.ifd_pointer().map(|pointer| pointer.0) {
                push_tiff_candidate(
                    &mut reduced,
                    TiffImageCandidate {
                        ifd_offset,
                        dimensions,
                        reduced: true,
                    },
                );
            }
        }

        if let Ok(Some(value)) = decoder.find_tag(tiff::tags::Tag::SubIfd) {
            if let Ok(sub_ifds) = value.into_ifd_vec() {
                for sub_ifd in sub_ifds {
                    check_tiff_cancelled(cancel)?;
                    let Ok(directory) = decoder.read_directory(sub_ifd) else {
                        continue;
                    };
                    let Some(candidate) = tiff_sub_ifd_candidate(decoder, &directory, sub_ifd.0)
                    else {
                        continue;
                    };
                    if candidate.reduced
                        && tiff_aspect_ratio_compatible(primary, candidate.dimensions)
                    {
                        push_tiff_candidate(&mut reduced, candidate);
                    }
                }
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

    reduced.sort_unstable_by(|left, right| {
        let left_sufficient = left.dimensions.0 >= target.0 && left.dimensions.1 >= target.1;
        let right_sufficient = right.dimensions.0 >= target.0 && right.dimensions.1 >= target.1;
        match (left_sufficient, right_sufficient) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (true, true) => tiff_candidate_area(*left)
                .cmp(&tiff_candidate_area(*right))
                .then_with(|| left.ifd_offset.cmp(&right.ifd_offset)),
            (false, false) => tiff_candidate_area(*right)
                .cmp(&tiff_candidate_area(*left))
                .then_with(|| left.ifd_offset.cmp(&right.ifd_offset)),
        }
    });
    reduced.push(TiffImageCandidate {
        ifd_offset: primary_ifd_offset,
        dimensions: primary,
        reduced: false,
    });
    Ok(reduced)
}

fn tiff_sub_ifd_candidate<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
    directory: &tiff::Directory,
    ifd_offset: u64,
) -> Option<TiffImageCandidate> {
    let mut tags = decoder.read_directory_tags(directory);
    let width = tags
        .find_tag_unsigned::<u32>(tiff::tags::Tag::ImageWidth)
        .ok()??;
    let height = tags
        .find_tag_unsigned::<u32>(tiff::tags::Tag::ImageLength)
        .ok()??;
    if width == 0 || height == 0 {
        return None;
    }
    let new_subfile_type = tags
        .find_tag_unsigned::<u32>(tiff::tags::Tag::NewSubfileType)
        .ok()
        .flatten()
        .is_some_and(|value| value & TIFF_REDUCED_IMAGE_SUBFILE_BIT != 0);
    let old_subfile_type = tags
        .find_tag_unsigned::<u16>(tiff::tags::Tag::SubfileType)
        .ok()
        .flatten()
        .is_some_and(|value| value == TIFF_OLD_REDUCED_IMAGE_SUBFILE_TYPE);
    Some(TiffImageCandidate {
        ifd_offset,
        dimensions: (width, height),
        reduced: new_subfile_type || old_subfile_type,
    })
}

fn push_tiff_candidate(candidates: &mut Vec<TiffImageCandidate>, candidate: TiffImageCandidate) {
    if !candidates
        .iter()
        .any(|existing| existing.ifd_offset == candidate.ifd_offset)
    {
        candidates.push(candidate);
    }
}

fn tiff_candidate_area(candidate: TiffImageCandidate) -> u64 {
    u64::from(candidate.dimensions.0).saturating_mul(u64::from(candidate.dimensions.1))
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
    timings.finish(ThumbnailStage::TiffRawSample, sample_started);
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
    let plan = TiffSamplePlan::new(metadata, thumbnail_longest_side)?;
    let mut thumbnail = image::RgbaImage::new(plan.output_width, plan.output_height);
    let source_x_offsets = plan.source_x_offsets(pixel_bytes);
    let sampled_row_len = source_x_offsets
        .len()
        .checked_mul(pixel_bytes)
        .ok_or(TiffFastThumbnailError::Unsupported)?;
    let use_sparse_rows = tiff_should_read_sparse_rows(row_len, sampled_row_len);
    let mut full_row = (!use_sparse_rows).then(|| vec![0u8; row_len]);
    let mut sampled_row = use_sparse_rows.then(|| vec![0u8; sampled_row_len]);
    let mut cached_row_start = None;
    let thumbnail_stride = usize::try_from(plan.output_width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or(TiffFastThumbnailError::Unsupported)?;
    let thumbnail_pixels: &mut [u8] = thumbnail.as_mut();

    for (thumbnail_y, source_y) in plan.source_ys.iter().copied().enumerate() {
        check_tiff_cancelled(cancel)?;
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

        if cached_row_start != Some(row_start) {
            let reader = decoder.inner();
            if use_sparse_rows {
                let row = sampled_row
                    .as_mut()
                    .ok_or(TiffFastThumbnailError::Unsupported)?;
                read_tiff_sparse_row(reader, row_start, &source_x_offsets, pixel_bytes, row)?;
            } else {
                let row = full_row
                    .as_mut()
                    .ok_or(TiffFastThumbnailError::Unsupported)?;
                reader
                    .seek(SeekFrom::Start(row_start))
                    .map_err(|_| TiffFastThumbnailError::Unsupported)?;
                reader
                    .read_exact(row)
                    .map_err(|_| TiffFastThumbnailError::Unsupported)?;
            }
            cached_row_start = Some(row_start);
        }

        let output_row_start = thumbnail_y
            .checked_mul(thumbnail_stride)
            .ok_or(TiffFastThumbnailError::Unsupported)?;
        let output_row = thumbnail_pixels
            .get_mut(output_row_start..output_row_start + thumbnail_stride)
            .ok_or(TiffFastThumbnailError::Unsupported)?;
        if use_sparse_rows {
            write_tiff_raw_row(
                output_row,
                sampled_row
                    .as_deref()
                    .ok_or(TiffFastThumbnailError::Unsupported)?,
                (0..sampled_row_len).step_by(pixel_bytes),
                metadata,
                byte_order,
            )?;
        } else {
            write_tiff_raw_row(
                output_row,
                full_row
                    .as_deref()
                    .ok_or(TiffFastThumbnailError::Unsupported)?,
                source_x_offsets.iter().copied(),
                metadata,
                byte_order,
            )?;
        }
    }

    Ok(thumbnail)
}

fn write_tiff_raw_row(
    output: &mut [u8],
    source: &[u8],
    offsets: impl Iterator<Item = usize>,
    metadata: TiffImageMetadata,
    byte_order: tiff::tags::ByteOrder,
) -> Result<(), TiffFastThumbnailError> {
    for (pixel, offset) in output.chunks_exact_mut(4).zip(offsets) {
        pixel.copy_from_slice(&tiff_raw_pixel_to_rgba(
            source, offset, metadata, byte_order,
        )?);
    }
    Ok(())
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
    timings.finish(ThumbnailStage::TiffChunkSample, sample_started);
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
    let plan = TiffSamplePlan::new(metadata, thumbnail_longest_side)?;
    let chunk_type = decoder.get_chunk_type();
    let sample_count = usize::try_from(plan.output_width)
        .ok()
        .and_then(|width| {
            usize::try_from(plan.output_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or(TiffFastThumbnailError::Unsupported)?;
    let mut samples = Vec::with_capacity(sample_count);

    for (thumbnail_y, source_y) in plan.source_ys.iter().copied().enumerate() {
        check_tiff_cancelled(cancel)?;
        for (thumbnail_x, source_x) in plan.source_xs.iter().copied().enumerate() {
            let chunk_index = match chunk_type {
                tiff::decoder::ChunkType::Strip => source_y / chunk_height,
                tiff::decoder::ChunkType::Tile => (source_y / chunk_height)
                    .checked_mul(chunks_across)
                    .and_then(|row| row.checked_add(source_x / chunk_width))
                    .ok_or(TiffFastThumbnailError::Unsupported)?,
            };
            if chunk_index >= chunk_count {
                return Err(TiffFastThumbnailError::Unsupported);
            }
            samples.push(TiffChunkSample {
                chunk_index,
                output_offset: thumbnail_rgba_offset(thumbnail_x, thumbnail_y, plan.output_width)?,
                local_x: source_x % chunk_width,
                local_y: source_y % chunk_height,
            });
        }
    }
    samples.sort_unstable_by_key(|sample| sample.chunk_index);

    let mut thumbnail = image::RgbaImage::new(plan.output_width, plan.output_height);
    let thumbnail_pixels: &mut [u8] = thumbnail.as_mut();
    let mut chunk_pixels = None;

    let mut first = 0;
    while first < samples.len() {
        check_tiff_cancelled(cancel)?;
        let chunk_index = samples[first].chunk_index;
        let mut end = first + 1;
        while end < samples.len() && samples[end].chunk_index == chunk_index {
            end += 1;
        }
        let (chunk_width, chunk_height) = read_tiff_chunk(
            decoder,
            chunk_index,
            &mut chunk_pixels,
            timings_enabled,
            timings,
        )?;
        let chunk = TiffDecodedChunk {
            width: chunk_width,
            height: chunk_height,
            pixels: chunk_pixels
                .as_ref()
                .ok_or(TiffFastThumbnailError::Unsupported)?,
        };
        for sample in &samples[first..end] {
            let pixel =
                tiff_chunk_pixel_to_rgba(&chunk, sample.local_x, sample.local_y, metadata.layout)?;
            thumbnail_pixels
                .get_mut(sample.output_offset..sample.output_offset + 4)
                .ok_or(TiffFastThumbnailError::Unsupported)?
                .copy_from_slice(&pixel);
        }
        first = end;
    }

    Ok(thumbnail)
}

fn read_tiff_chunk<R: Read + Seek>(
    decoder: &mut tiff::decoder::Decoder<R>,
    chunk_index: u32,
    pixels: &mut Option<tiff::decoder::DecodingResult>,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<(u32, u32), TiffFastThumbnailError> {
    let (width, height) = decoder.chunk_data_dimensions(chunk_index);
    let decode_started = thumbnail_timing_started(timings_enabled);
    let result = if let Some(pixels) = pixels.as_mut() {
        decoder.read_chunk_bytes(chunk_index, pixels.as_buffer(0).as_bytes_mut())
    } else {
        decoder.read_chunk(chunk_index).map(|decoded| {
            *pixels = Some(decoded);
        })
    };
    timings.add(ThumbnailStage::TiffChunkDecode, decode_started);
    result.map_err(|_| TiffFastThumbnailError::Unsupported)?;
    Ok((width, height))
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
            let bytes = row
                .get(sample_offset..sample_offset + 2)
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
    let base = pixel_index
        .checked_mul(layout.samples)
        .ok_or(TiffFastThumbnailError::Unsupported)?;

    match chunk.pixels {
        tiff::decoder::DecodingResult::U8(pixels) => {
            tiff_samples_to_rgba(layout.kind, false, |index| {
                pixels
                    .get(base + index)
                    .copied()
                    .ok_or(TiffFastThumbnailError::Unsupported)
            })
        }
        tiff::decoder::DecodingResult::U16(pixels) => {
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

fn tiff_sampled_source_pixels(destination_len: u32, source_len: u32) -> Vec<u32> {
    (0..destination_len)
        .map(|destination| nearest_source_pixel(destination, destination_len, source_len))
        .collect()
}

fn tiff_should_read_sparse_rows(row_len: usize, sampled_row_len: usize) -> bool {
    row_len >= TIFF_SPARSE_ROW_MIN_BYTES
        && row_len
            > sampled_row_len
                .saturating_mul(TIFF_SPARSE_ROW_READ_RATIO)
                .max(sampled_row_len)
}

fn read_tiff_sparse_row<R: Read + Seek>(
    reader: &mut R,
    row_start: u64,
    source_x_offsets: &[usize],
    pixel_bytes: usize,
    row: &mut [u8],
) -> Result<(), TiffFastThumbnailError> {
    for (index, source_offset) in source_x_offsets.iter().copied().enumerate() {
        let target_start = index
            .checked_mul(pixel_bytes)
            .ok_or(TiffFastThumbnailError::Unsupported)?;
        let target_end = target_start
            .checked_add(pixel_bytes)
            .ok_or(TiffFastThumbnailError::Unsupported)?;
        let source_start = row_start
            .checked_add(source_offset as u64)
            .ok_or(TiffFastThumbnailError::Unsupported)?;
        reader
            .seek(SeekFrom::Start(source_start))
            .map_err(|_| TiffFastThumbnailError::Unsupported)?;
        reader
            .read_exact(
                row.get_mut(target_start..target_end)
                    .ok_or(TiffFastThumbnailError::Unsupported)?,
            )
            .map_err(|_| TiffFastThumbnailError::Unsupported)?;
    }
    Ok(())
}

fn thumbnail_rgba_offset(x: usize, y: usize, width: u32) -> Result<usize, TiffFastThumbnailError> {
    let width = usize::try_from(width).map_err(|_| TiffFastThumbnailError::Unsupported)?;
    y.checked_mul(width)
        .and_then(|row| row.checked_add(x))
        .and_then(|pixel| pixel.checked_mul(4))
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
    timings.add(ThumbnailStage::SourceRead, read_started);
    let reader = reader.map_err(|error| format!("Failed to read image file: {error}"))?;

    let format_started = thumbnail_timing_started(timings_enabled);
    let _ = reader.format();
    timings.add(ThumbnailStage::FormatDetect, format_started);

    Ok(reader)
}

fn open_image_reader_with_guessed_format_timed(
    path: &Path,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<image::ImageReader<BufReader<File>>, String> {
    let read_started = thumbnail_timing_started(timings_enabled);
    let reader = image::ImageReader::open(path);
    timings.add(ThumbnailStage::SourceRead, read_started);
    let reader = reader.map_err(|error| format!("Failed to read image file: {error}"))?;

    let format_started = thumbnail_timing_started(timings_enabled);
    let reader = reader
        .with_guessed_format()
        .map_err(|error| format!("Failed to detect image format: {error}"));
    timings.add(ThumbnailStage::FormatDetect, format_started);

    reader
}

fn decode_image_reader_to_rgba_timed(
    reader: image::ImageReader<BufReader<File>>,
    longest_side: u32,
    timings_enabled: bool,
    timings: &mut ImageThumbnailExtractionTimings,
) -> Result<image::RgbaImage, String> {
    let decode_started = thumbnail_timing_started(timings_enabled);
    let image = reader.decode();
    timings.add(ThumbnailStage::RasterDecode, decode_started);
    let image = image.map_err(|error| format!("Failed to decode image: {error}"))?;

    let resize_started = thumbnail_timing_started(timings_enabled);
    let image = resize_dynamic_to_rgba(image, longest_side);
    timings.add(ThumbnailStage::ResizeCanvas, resize_started);
    let image = image?;
    Ok(image)
}

fn load_embedded_jpeg_thumbnail_rgba_with_cancel_timed(
    path: &Path,
    longest_side: u32,
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
    timings.finish(ThumbnailStage::EmbeddedThumbnailScan, scan_started);
    let Some(thumbnail) = thumbnail else {
        return Ok(None);
    };

    check_image_cancelled(cancel)?;
    let decode_started = thumbnail_timing_started(timings_enabled);
    let image = image::load_from_memory_with_format(&thumbnail, image::ImageFormat::Jpeg).ok();
    timings.finish(ThumbnailStage::EmbeddedThumbnailDecode, decode_started);
    let Some(image) = image else {
        return Ok(None);
    };

    check_image_cancelled(cancel)?;
    let resize_started = thumbnail_timing_started(timings_enabled);
    let image = resize_dynamic_to_rgba(image, longest_side)?;
    timings.add(ThumbnailStage::ResizeCanvas, resize_started);
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
            timings.finish(ThumbnailStage::SourceRead, read_started);
            return Err(format!("Failed to read SVG file: {error}"));
        }
    };
    timings.finish(ThumbnailStage::SourceRead, read_started);
    check_image_cancelled(cancel)?;
    let options = usvg::Options::default();
    let parse_started = thumbnail_timing_started(timings_enabled);
    let tree = usvg::Tree::from_data(&bytes, &options);
    timings.finish(ThumbnailStage::SvgParse, parse_started);
    let tree = tree.map_err(|error| format!("Failed to parse SVG: {error}"))?;
    check_image_cancelled(cancel)?;
    let render_started = thumbnail_timing_started(timings_enabled);
    let image = render_svg_rgba(&tree, longest_side);
    timings.finish(ThumbnailStage::SvgRender, render_started);
    let mut image = image?;
    check_image_cancelled(cancel)?;

    let unpremultiply_started = thumbnail_timing_started(timings_enabled);
    for pixel in image.chunks_exact_mut(4) {
        unpremultiply_rgba(pixel);
    }
    timings.finish(ThumbnailStage::SvgUnpremultiply, unpremultiply_started);

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

#[cfg(test)]
fn property_image_preview_from_rgba(
    mut image: image::RgbaImage,
    animated_source: Option<AnimatedImageSource>,
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
        animated_source,
    })
}

#[cfg(test)]
fn property_animated_gif_source(path: &Path) -> Option<AnimatedImageSource> {
    animated_gif_source_for_path(path, property_animated_gif_cache_key(path))
}

#[cfg(test)]
fn property_animated_gif_cache_key(path: &Path) -> String {
    let mut key = String::from("property-gif:");
    key.push_str(&path.to_string_lossy().replace('\\', "/"));

    match fs::metadata(path) {
        Ok(metadata) => {
            key.push(':');
            key.push_str(&metadata.len().to_string());
            key.push(':');
            key.push_str(
                &metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map(|duration| {
                        duration
                            .as_secs()
                            .saturating_mul(1_000_000_000)
                            .saturating_add(u64::from(duration.subsec_nanos()))
                    })
                    .unwrap_or(0)
                    .to_string(),
            );
        }
        Err(_) => key.push_str(":missing:0"),
    }

    key
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

#[cfg(any(test, feature = "benchmarks"))]
pub(super) fn encode_rgba_png_bytes(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
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

#[cfg(feature = "benchmarks")]
pub mod benchmark_support {
    use super::*;

    pub fn load_image_thumbnail_for_benchmark(path: &Path, size: u32) -> Result<Vec<u8>, String> {
        let cancel = AtomicBool::new(false);
        load_image_thumbnail_png_with_cancel_timed(path, size, &cancel, false).result
    }

    pub fn load_image_thumbnail_ready_for_benchmark(
        path: &Path,
        size: u32,
    ) -> Result<image::RgbaImage, String> {
        let cancel = AtomicBool::new(false);
        load_thumbnail_rgba_with_cancel_timed(path, ThumbnailSpec::standard(size), &cancel, false)
            .result
    }

    pub fn resize_rgba_for_benchmark(
        image: image::RgbaImage,
        longest_side: u32,
    ) -> Result<image::RgbaImage, String> {
        crate::explorer::image_resize::resize_rgba_to_longest_side(image, longest_side)
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
    fn image_preview_detects_gif_animation_source_by_extension() {
        assert!(path_may_have_animated_gif_preview(Path::new("loop.gif")));
        assert!(path_may_have_animated_gif_preview(Path::new("loop.GIF")));
        assert!(!path_may_have_animated_gif_preview(Path::new("loop.gifv")));
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
    fn image_preview_records_animated_gif_source_and_keeps_static_copy_frame() {
        let temp = TempDir::new();
        let path = temp.path().join("loop.GIF");
        fs::write(&path, animated_gif_bytes(4, 2)).unwrap();

        let preview = load_property_image_preview(&path).unwrap();

        assert_eq!(preview.width, 4);
        assert_eq!(preview.height, 2);
        assert_render_image_size(&preview, 4, 2);
        assert_eq!(preview.image.frame_count(), 1);
        assert_eq!(
            preview.animated_source.as_ref().map(|source| &source.path),
            Some(&path)
        );
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
    fn image_thumbnail_png_preserves_landscape_aspect_ratio() {
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
        assert_eq!(decoded.dimensions(), (128, 64));
    }

    #[test]
    fn image_thumbnail_png_preserves_portrait_aspect_ratio() {
        let temp = TempDir::new();
        let path = temp.path().join("portrait.png");
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(2, 4));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        fs::write(&path, bytes).unwrap();

        let thumbnail = load_image_thumbnail_png(&path, 128).unwrap();
        let decoded = image::load_from_memory(&thumbnail).unwrap().into_rgba8();

        assert_eq!(decoded.dimensions(), (64, 128));
    }

    #[test]
    fn image_thumbnail_png_upscales_small_images_to_128px_longest_side() {
        let temp = TempDir::new();
        let path = temp.path().join("small.png");
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(8, 4));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        fs::write(&path, bytes).unwrap();

        let thumbnail = load_image_thumbnail_png(&path, 128).unwrap();
        let decoded = image::load_from_memory(&thumbnail).unwrap().into_rgba8();

        assert_eq!(decoded.dimensions(), (128, 64));
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
        assert!(thumbnail.timings.get(ThumbnailStage::SourceRead).is_some());
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::FormatDetect)
                .is_some()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::RasterDecode)
                .is_some()
        );
        assert!(thumbnail.timings.get(ThumbnailStage::RgbaConvert).is_none());
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::ResizeCanvas)
                .is_some()
        );
        assert!(thumbnail.timings.get(ThumbnailStage::PngEncode).is_some());
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::EmbeddedThumbnailScan)
                .is_none()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::EmbeddedThumbnailDecode)
                .is_none()
        );
        assert!(thumbnail.timings.get(ThumbnailStage::SvgParse).is_none());
        assert!(thumbnail.timings.get(ThumbnailStage::SvgRender).is_none());
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::SvgUnpremultiply)
                .is_none()
        );
        assert!(thumbnail.timings.get(ThumbnailStage::TiffIfdScan).is_none());
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::TiffRawSample)
                .is_none()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::TiffChunkDecode)
                .is_none()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::TiffChunkSample)
                .is_none()
        );
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
            assert!(thumbnail.timings.get(ThumbnailStage::TiffIfdScan).is_some());
            assert!(
                thumbnail
                    .timings
                    .get(ThumbnailStage::TiffRawSample)
                    .is_some()
            );
            assert!(
                thumbnail
                    .timings
                    .get(ThumbnailStage::TiffChunkDecode)
                    .is_none()
            );
            assert!(
                thumbnail
                    .timings
                    .get(ThumbnailStage::RasterDecode)
                    .is_none()
            );
            let decoded = image::load_from_memory(&thumbnail.result.unwrap())
                .unwrap()
                .into_rgba8();
            assert_eq!(decoded.dimensions(), (128, 96));
        }
    }

    #[test]
    fn tiff_thumbnail_reuses_chunk_decoder_for_lzw_rgb8() {
        let temp = TempDir::new();
        let path = temp.path().join("lzw-rgb.tif");
        fs::write(&path, tiff_rgb8_lzw_bytes(512, 384, 8, &[30, 140, 220])).unwrap();
        let cancel = AtomicBool::new(false);

        let thumbnail = load_image_thumbnail_png_with_cancel_timed(&path, 128, &cancel, true);

        assert!(thumbnail.result.is_ok());
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::TiffChunkDecode)
                .is_some()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::RasterDecode)
                .is_none()
        );
        let decoded = image::load_from_memory(&thumbnail.result.unwrap())
            .unwrap()
            .into_rgba8();
        assert_eq!(decoded.dimensions(), (128, 96));
    }

    #[test]
    fn tiff_thumbnail_fast_path_detects_tiff_magic_without_known_extension() {
        let temp = TempDir::new();
        let path = temp.path().join("scan");
        fs::write(&path, tiff_rgb8_bytes(64, 64, &[30, 140, 220])).unwrap();
        let cancel = AtomicBool::new(false);

        let thumbnail = load_image_thumbnail_png_with_cancel_timed(&path, 128, &cancel, true);

        assert!(thumbnail.result.is_ok());
        assert!(thumbnail.timings.get(ThumbnailStage::TiffIfdScan).is_some());
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::TiffRawSample)
                .is_some()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::RasterDecode)
                .is_none()
        );
    }

    #[test]
    fn tiff_thumbnail_selects_smallest_sufficient_reduced_ifd() {
        let temp = TempDir::new();
        let path = temp.path().join("reduced.tif");
        fs::write(
            &path,
            tiff_rgb8_directories_bytes(&[
                (1024, 768, [220, 30, 30], false),
                (96, 72, [30, 220, 30], true),
                (256, 192, [30, 30, 220], true),
            ]),
        )
        .unwrap();
        let cancel = AtomicBool::new(false);

        let thumbnail = load_image_thumbnail_png_with_cancel_timed(&path, 128, &cancel, true);

        assert!(thumbnail.result.is_ok());
        assert!(thumbnail.timings.get(ThumbnailStage::TiffIfdScan).is_some());
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::TiffRawSample)
                .is_some()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::RasterDecode)
                .is_none()
        );
        let decoded = image::load_from_memory(&thumbnail.result.unwrap())
            .unwrap()
            .into_rgba8();
        assert_eq!(decoded.dimensions(), (128, 96));
        let pixel = decoded.get_pixel(64, 48);
        assert!(
            pixel[2] > pixel[0] && pixel[2] > pixel[1],
            "expected sufficient reduced blue IFD to be selected, got {pixel:?}"
        );
    }

    #[test]
    fn tiff_thumbnail_selects_smallest_sufficient_reduced_sub_ifd_for_each_target() {
        let temp = TempDir::new();
        let path = temp.path().join("sub-ifd-pyramid.tif");
        fs::write(
            &path,
            tiff_rgb8_sub_ifd_bytes(
                (1024, 768, [220, 30, 30]),
                &[
                    (96, 72, [30, 220, 30], false),
                    (256, 192, [30, 30, 220], false),
                    (512, 384, [220, 220, 30], false),
                ],
                false,
            ),
        )
        .unwrap();
        let cancel = AtomicBool::new(false);

        let standard = load_image_thumbnail_png_with_cancel_timed(&path, 128, &cancel, true);
        let hover = load_hover_image_preview_png_with_cancel_timed(&path, 400, &cancel, true);

        let standard = image::load_from_memory(&standard.result.unwrap())
            .unwrap()
            .into_rgba8();
        let hover = image::load_from_memory(&hover.result.unwrap())
            .unwrap()
            .into_rgba8();
        assert_eq!(standard.dimensions(), (128, 96));
        assert_eq!(hover.dimensions(), (400, 300));
        let standard_pixel = standard.get_pixel(64, 48);
        assert!(standard_pixel[2] > standard_pixel[0] && standard_pixel[2] > standard_pixel[1]);
        let hover_pixel = hover.get_pixel(200, 150);
        assert!(hover_pixel[0] > 180 && hover_pixel[1] > 180 && hover_pixel[2] < 80);
    }

    #[test]
    fn tiff_thumbnail_skips_malformed_and_unsupported_reduced_sub_ifds() {
        let temp = TempDir::new();
        let path = temp.path().join("sub-ifd-fallback.tif");
        fs::write(
            &path,
            tiff_rgb8_sub_ifd_bytes(
                (1024, 768, [220, 30, 30]),
                &[
                    (256, 192, [30, 220, 30], true),
                    (512, 384, [30, 30, 220], false),
                ],
                true,
            ),
        )
        .unwrap();
        let cancel = AtomicBool::new(false);

        let thumbnail = load_image_thumbnail_png_with_cancel_timed(&path, 128, &cancel, true);

        assert!(thumbnail.result.is_ok());
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::RasterDecode)
                .is_none()
        );
        let decoded = image::load_from_memory(&thumbnail.result.unwrap())
            .unwrap()
            .into_rgba8();
        let pixel = decoded.get_pixel(64, 48);
        assert!(pixel[2] > pixel[0] && pixel[2] > pixel[1]);
    }

    #[test]
    fn tiff_root_reader_patches_classic_and_big_tiff_offsets_in_both_byte_orders() {
        let fixtures = [
            (
                b"II*\0\x08\0\0\0".as_slice(),
                0x1234u64,
                4usize,
                4usize,
                true,
            ),
            (
                b"MM\0*\0\0\0\x08".as_slice(),
                0x1234u64,
                4usize,
                4usize,
                false,
            ),
            (
                b"II+\0\x08\0\0\0\x10\0\0\0\0\0\0\0".as_slice(),
                0x1234_5678_9abcu64,
                8usize,
                8usize,
                true,
            ),
            (
                b"MM\0+\0\x08\0\0\0\0\0\0\0\0\0\x10".as_slice(),
                0x1234_5678_9abcu64,
                8usize,
                8usize,
                false,
            ),
        ];

        for (header, offset, start, len, little_endian) in fixtures {
            let mut reader =
                TiffRootReader::new(Cursor::new(header.to_vec()), Some(offset)).unwrap();
            let mut patched = Vec::new();
            reader.read_to_end(&mut patched).unwrap();
            let expected = if len == 4 {
                if little_endian {
                    (offset as u32).to_le_bytes().to_vec()
                } else {
                    (offset as u32).to_be_bytes().to_vec()
                }
            } else if little_endian {
                offset.to_le_bytes().to_vec()
            } else {
                offset.to_be_bytes().to_vec()
            };
            assert_eq!(&patched[start..start + len], expected.as_slice());
        }
    }

    #[test]
    fn tiff_thumbnail_decoder_downcasts_16_bit_samples_before_resize() {
        let temp = TempDir::new();
        let path = temp.path().join("rgb16.tif");
        fs::write(&path, tiff_rgb16_bytes(64, 64, &[0xf000, 0x1000, 0x8000])).unwrap();
        let cancel = AtomicBool::new(false);

        let thumbnail = load_image_thumbnail_png_with_cancel_timed(&path, 128, &cancel, true);

        assert!(thumbnail.result.is_ok());
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::TiffRawSample)
                .is_some()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::RasterDecode)
                .is_none()
        );
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
    fn tiff_thumbnail_decoder_handles_white_is_zero_grayscale() {
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
        assert!(thumbnail.timings.get(ThumbnailStage::TiffIfdScan).is_some());
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::TiffRawSample)
                .is_none()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::TiffChunkDecode)
                .is_some()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::TiffChunkSample)
                .is_some()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::RasterDecode)
                .is_none()
        );
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
        assert!(thumbnail.timings.get(ThumbnailStage::TiffIfdScan).is_some());
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::TiffRawSample)
                .is_none()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::RasterDecode)
                .is_some()
        );
        let decoded = image::load_from_memory(&thumbnail.result.unwrap())
            .unwrap()
            .into_rgba8();
        assert_eq!(decoded.dimensions(), (128, 64));
    }

    #[test]
    fn unsupported_tiff_fast_path_falls_back_to_generic_decoder() {
        let temp = TempDir::new();
        let path = temp.path().join("cmyk.tif");
        fs::write(&path, tiff_cmyk8_bytes(16, 16, &[0, 255, 255, 0])).unwrap();
        let cancel = AtomicBool::new(false);

        let thumbnail = load_image_thumbnail_png_with_cancel_timed(&path, 128, &cancel, true);

        assert!(thumbnail.result.is_ok());
        assert!(thumbnail.timings.get(ThumbnailStage::TiffIfdScan).is_some());
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::TiffRawSample)
                .is_none()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::RasterDecode)
                .is_some()
        );
        let decoded = image::load_from_memory(&thumbnail.result.unwrap())
            .unwrap()
            .into_rgba8();
        assert_eq!(decoded.dimensions(), (128, 128));
    }

    #[test]
    fn tiff_fast_decoder_honors_cancellation_during_decode() {
        let bytes = tiff_rgb8_bytes(512, 384, &[30, 140, 220]);
        let cancel = Arc::new(AtomicBool::new(false));
        let reader = CancellingReader {
            inner: Cursor::new(bytes),
            cancel: cancel.clone(),
            read_count: 0,
            cancel_after_read: usize::MAX,
        };
        let mut decoder =
            tiff::decoder::Decoder::new(BufReader::new(reader)).expect("open test tiff");
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
        assert!(timings.get(ThumbnailStage::TiffRawSample).is_some());
    }

    #[test]
    fn tiff_fast_decoder_reads_sparse_rows_for_very_wide_uncompressed_strips() {
        let width = 3_000_000;
        let height = 2;
        let bytes = tiff_rgb8_bytes(width, height, &[30, 140, 220]);
        let cancel = AtomicBool::new(false);
        let reader = CountingReader {
            inner: Cursor::new(bytes),
            read_bytes: 0,
        };
        let mut decoder = tiff::decoder::Decoder::new(BufReader::with_capacity(1, reader))
            .expect("open test tiff");
        let metadata = tiff_current_image_metadata(&mut decoder).expect("read tiff metadata");
        let row_bytes = tiff_row_bytes(metadata).unwrap();
        let read_before_sample = decoder.inner().get_ref().read_bytes;
        let mut timings = ImageThumbnailExtractionTimings::default();

        let thumbnail = load_uncompressed_stripped_tiff_thumbnail_rgba(
            &mut decoder,
            metadata,
            128,
            &cancel,
            true,
            &mut timings,
        )
        .expect("decode sparse thumbnail");

        let sample_read_bytes = decoder.inner().get_ref().read_bytes - read_before_sample;
        assert_eq!(thumbnail.dimensions(), (128, 1));
        assert!(timings.get(ThumbnailStage::TiffRawSample).is_some());
        assert!(
            sample_read_bytes < row_bytes / 8,
            "expected sparse sampling to read far less than a full row; read {sample_read_bytes} of {row_bytes} bytes"
        );
    }

    #[test]
    fn svg_thumbnail_png_preserves_aspect_ratio() {
        let temp = TempDir::new();
        let path = temp.path().join("vector.svg");
        fs::write(
            &path,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="250"><rect width="1000" height="250" fill="red"/></svg>"#,
        )
        .unwrap();

        let thumbnail = load_image_thumbnail_png(&path, 128).unwrap();
        let decoded = image::load_from_memory(&thumbnail).unwrap().into_rgba8();

        assert_eq!(decoded.dimensions(), (128, 32));
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
        assert!(thumbnail.timings.get(ThumbnailStage::SourceRead).is_some());
        assert!(thumbnail.timings.get(ThumbnailStage::SvgParse).is_some());
        assert!(thumbnail.timings.get(ThumbnailStage::SvgRender).is_some());
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::SvgUnpremultiply)
                .is_some()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::ResizeCanvas)
                .is_none()
        );
        assert!(thumbnail.timings.get(ThumbnailStage::PngEncode).is_some());
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::EmbeddedThumbnailScan)
                .is_none()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::EmbeddedThumbnailDecode)
                .is_none()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::FormatDetect)
                .is_none()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::RasterDecode)
                .is_none()
        );
        assert!(thumbnail.timings.get(ThumbnailStage::RgbaConvert).is_none());
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

        assert_eq!(decoded.dimensions(), (128, 64));
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
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::EmbeddedThumbnailScan)
                .is_some()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::EmbeddedThumbnailDecode)
                .is_some()
        );
        assert!(
            thumbnail
                .timings
                .get(ThumbnailStage::RasterDecode)
                .is_none()
        );

        let bytes = thumbnail.result.unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap().into_rgba8();
        let pixel = decoded.get_pixel(64, 32);
        assert!(
            pixel[1] > pixel[0],
            "expected embedded green thumbnail to be used, got {pixel:?}"
        );
    }

    #[test]
    fn jpeg_hover_preview_uses_primary_image_instead_of_embedded_thumbnail() {
        let temp = TempDir::new();
        let path = temp.path().join("photo.jpg");
        let primary = jpeg_bytes(16, 16, [220, 20, 20]);
        let embedded = jpeg_bytes(2, 1, [20, 220, 20]);
        fs::write(&path, jpeg_with_embedded_thumbnail(&primary, &embedded)).unwrap();
        let cancel = AtomicBool::new(false);

        let preview = load_hover_image_preview_png_with_cancel_timed(&path, 400, &cancel, true);

        assert!(preview.result.is_ok());
        assert!(
            preview
                .timings
                .get(ThumbnailStage::EmbeddedThumbnailScan)
                .is_none()
        );
        assert!(
            preview
                .timings
                .get(ThumbnailStage::EmbeddedThumbnailDecode)
                .is_none()
        );
        assert!(preview.timings.get(ThumbnailStage::RasterDecode).is_some());

        let bytes = preview.result.unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap().into_rgba8();
        let pixel = decoded.get_pixel(200, 200);
        assert!(
            pixel[0] > pixel[1],
            "expected primary red image to be used, got {pixel:?}"
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

    fn tiff_rgb8_lzw_bytes(width: u32, height: u32, rows_per_strip: u32, rgb: &[u8; 3]) -> Vec<u8> {
        let mut data = Vec::with_capacity((width * height * 3) as usize);
        for _ in 0..(width * height) {
            data.extend_from_slice(rgb);
        }
        let mut bytes = Vec::new();
        {
            let cursor = Cursor::new(&mut bytes);
            let mut encoder = tiff::encoder::TiffEncoder::new(cursor)
                .unwrap()
                .with_compression(tiff::encoder::Compression::Lzw);
            let mut image = encoder
                .new_image::<tiff::encoder::colortype::RGB8>(width, height)
                .unwrap();
            image.rows_per_strip(rows_per_strip).unwrap();
            image.write_data(data.as_slice()).unwrap();
        }
        bytes
    }

    fn tiff_cmyk8_bytes(width: u32, height: u32, cmyk: &[u8; 4]) -> Vec<u8> {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            data.extend_from_slice(cmyk);
        }
        encode_tiff::<tiff::encoder::colortype::CMYK8, u8>(width, height, 16, &data)
    }

    fn tiff_rgb8_directories_bytes(images: &[(u32, u32, [u8; 3], bool)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let cursor = Cursor::new(&mut bytes);
            let mut encoder = tiff::encoder::TiffEncoder::new(cursor).unwrap();
            for (width, height, rgb, reduced) in images {
                let mut data = Vec::with_capacity((*width * *height * 3) as usize);
                for _ in 0..(*width * *height) {
                    data.extend_from_slice(rgb);
                }
                let mut directory = encoder.image_directory().unwrap();
                let strip_offset = directory.write_data(&data[..]).unwrap();
                directory
                    .write_tag(
                        tiff::tags::Tag::NewSubfileType,
                        if *reduced {
                            TIFF_REDUCED_IMAGE_SUBFILE_BIT
                        } else {
                            0
                        },
                    )
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::ImageWidth, *width)
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::ImageLength, *height)
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::BitsPerSample, &[8u16, 8, 8][..])
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::Compression, 1u16)
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::PhotometricInterpretation, 2u16)
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::StripOffsets, strip_offset as u32)
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::RowsPerStrip, *height)
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::StripByteCounts, data.len() as u32)
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::SamplesPerPixel, 3u16)
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::PlanarConfiguration, 1u16)
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::SampleFormat, &[1u16, 1, 1][..])
                    .unwrap();
                directory.finish().unwrap();
            }
        }
        bytes
    }

    fn tiff_rgb8_sub_ifd_bytes(
        primary: (u32, u32, [u8; 3]),
        reduced: &[(u32, u32, [u8; 3], bool)],
        include_malformed_offset: bool,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let cursor = Cursor::new(&mut bytes);
            let mut encoder = tiff::encoder::TiffEncoder::new(cursor).unwrap();
            let mut sub_ifd_offsets = Vec::new();
            if include_malformed_offset {
                sub_ifd_offsets.push(u32::MAX);
            }

            for (width, height, rgb, unsupported) in reduced {
                let samples = if *unsupported { 4usize } else { 3usize };
                let mut data = Vec::with_capacity((*width * *height) as usize * samples);
                for _ in 0..(*width * *height) {
                    data.extend_from_slice(rgb);
                    if *unsupported {
                        data.push(0);
                    }
                }
                let mut directory = encoder.extra_directory().unwrap();
                let strip_offset = directory.write_data(&data[..]).unwrap();
                directory
                    .write_tag(
                        tiff::tags::Tag::NewSubfileType,
                        TIFF_REDUCED_IMAGE_SUBFILE_BIT,
                    )
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::ImageWidth, *width)
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::ImageLength, *height)
                    .unwrap();
                if *unsupported {
                    directory
                        .write_tag(tiff::tags::Tag::BitsPerSample, &[8u16, 8, 8, 8][..])
                        .unwrap();
                } else {
                    directory
                        .write_tag(tiff::tags::Tag::BitsPerSample, &[8u16, 8, 8][..])
                        .unwrap();
                }
                directory
                    .write_tag(tiff::tags::Tag::Compression, 1u16)
                    .unwrap();
                directory
                    .write_tag(
                        tiff::tags::Tag::PhotometricInterpretation,
                        if *unsupported { 5u16 } else { 2u16 },
                    )
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::StripOffsets, strip_offset as u32)
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::RowsPerStrip, *height)
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::StripByteCounts, data.len() as u32)
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::SamplesPerPixel, samples as u16)
                    .unwrap();
                directory
                    .write_tag(tiff::tags::Tag::PlanarConfiguration, 1u16)
                    .unwrap();
                directory
                    .write_tag(
                        tiff::tags::Tag::SampleFormat,
                        vec![1u16; samples].as_slice(),
                    )
                    .unwrap();
                let offset = directory.finish_with_offsets().unwrap().pointer.0;
                sub_ifd_offsets.push(offset as u32);
            }

            let (width, height, rgb) = primary;
            let mut data = Vec::with_capacity((width * height * 3) as usize);
            for _ in 0..(width * height) {
                data.extend_from_slice(&rgb);
            }
            let mut directory = encoder.image_directory().unwrap();
            let strip_offset = directory.write_data(&data[..]).unwrap();
            directory
                .write_tag(tiff::tags::Tag::SubIfd, sub_ifd_offsets.as_slice())
                .unwrap();
            directory
                .write_tag(tiff::tags::Tag::NewSubfileType, 0u32)
                .unwrap();
            directory
                .write_tag(tiff::tags::Tag::ImageWidth, width)
                .unwrap();
            directory
                .write_tag(tiff::tags::Tag::ImageLength, height)
                .unwrap();
            directory
                .write_tag(tiff::tags::Tag::BitsPerSample, &[8u16, 8, 8][..])
                .unwrap();
            directory
                .write_tag(tiff::tags::Tag::Compression, 1u16)
                .unwrap();
            directory
                .write_tag(tiff::tags::Tag::PhotometricInterpretation, 2u16)
                .unwrap();
            directory
                .write_tag(tiff::tags::Tag::StripOffsets, strip_offset as u32)
                .unwrap();
            directory
                .write_tag(tiff::tags::Tag::RowsPerStrip, height)
                .unwrap();
            directory
                .write_tag(tiff::tags::Tag::StripByteCounts, data.len() as u32)
                .unwrap();
            directory
                .write_tag(tiff::tags::Tag::SamplesPerPixel, 3u16)
                .unwrap();
            directory
                .write_tag(tiff::tags::Tag::PlanarConfiguration, 1u16)
                .unwrap();
            directory
                .write_tag(tiff::tags::Tag::SampleFormat, &[1u16, 1, 1][..])
                .unwrap();
            directory.finish().unwrap();
        }
        bytes
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

    struct CountingReader {
        inner: Cursor<Vec<u8>>,
        read_bytes: u64,
    }

    impl Read for CountingReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let read = self.inner.read(buf)?;
            self.read_bytes += read as u64;
            Ok(read)
        }
    }

    impl Seek for CountingReader {
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

    fn animated_gif_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            encoder
                .set_repeat(image::codecs::gif::Repeat::Infinite)
                .unwrap();
            for rgba in [[220, 40, 80, 255], [40, 140, 220, 255]] {
                encoder
                    .encode_frame(image::Frame::from_parts(
                        image::RgbaImage::from_pixel(width, height, image::Rgba(rgba)),
                        0,
                        0,
                        image::Delay::from_numer_denom_ms(80, 1),
                    ))
                    .unwrap();
            }
        }
        bytes
    }
}
