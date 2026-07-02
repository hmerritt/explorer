use std::{
    fs::{self, File},
    io::BufReader,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::RenderImage;
use image::ImageDecoder;

use crate::image_viewer::color::{apply_icc_profile_to_srgb, convert_rgba_to_srgb};

type ImageFileReader = image::ImageReader<BufReader<File>>;

#[derive(Clone)]
pub(super) struct DecodedImage {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) source_decompressed_size_bytes: Option<u64>,
    pub(super) deferred_icc_correction: Option<DeferredIccCorrection>,
    pub(super) source: DecodedImageSource,
}

#[derive(Clone)]
pub(super) enum DecodedImageSource {
    Raster(Arc<RenderImage>),
    Svg(Arc<Vec<u8>>),
}

#[derive(Clone)]
pub(super) struct DeferredIccCorrection {
    pub(super) source_image: Arc<RenderImage>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) icc_profile: Arc<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(feature = "benchmarks"), allow(dead_code))]
pub(super) enum IccDecodeMode {
    ApplySynchronously,
    Defer,
    Ignore,
}

#[derive(Clone, Debug, Default)]
#[cfg_attr(not(feature = "benchmarks"), allow(dead_code))]
pub(super) struct ImageDecodeTimings {
    pub(super) source_read: Option<Duration>,
    pub(super) format_detect: Option<Duration>,
    pub(super) metadata: Option<Duration>,
    pub(super) raster_decode: Option<Duration>,
    pub(super) orientation: Option<Duration>,
    pub(super) rgba_convert: Option<Duration>,
    pub(super) icc_convert: Option<Duration>,
    pub(super) render_image_build: Option<Duration>,
    pub(super) svg_parse: Option<Duration>,
}

#[cfg_attr(not(feature = "benchmarks"), allow(dead_code))]
pub(super) struct TimedDecodedImage {
    pub(super) result: Result<DecodedImage, String>,
    #[allow(dead_code)]
    pub(super) timings: ImageDecodeTimings,
}

pub(super) fn decode_image_source(path: &Path) -> Result<DecodedImage, String> {
    decode_image_source_with_options(path, IccDecodeMode::Defer, false).result
}

pub(super) fn decode_image_source_with_options(
    path: &Path,
    icc_mode: IccDecodeMode,
    timings_enabled: bool,
) -> TimedDecodedImage {
    let mut timings = ImageDecodeTimings::default();
    let result = if path_is_svg(path) {
        decode_svg_source(path, timings_enabled, &mut timings)
    } else {
        decode_raster_source(path, icc_mode, timings_enabled, &mut timings)
    };

    TimedDecodedImage { result, timings }
}

pub(super) fn apply_deferred_icc_correction(
    correction: DeferredIccCorrection,
) -> Result<Arc<RenderImage>, String> {
    let mut rgba = correction
        .source_image
        .as_bytes(0)
        .ok_or_else(|| "Failed to read deferred ICC source image.".to_owned())?
        .to_vec();
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let source = image::RgbaImage::from_raw(correction.width, correction.height, rgba)
        .ok_or_else(|| "Failed to create deferred ICC source image.".to_owned())?;
    let corrected = convert_rgba_to_srgb(&source, &correction.icc_profile)
        .ok_or_else(|| "Failed to apply image color profile.".to_owned())?;
    Ok(render_image_from_rgba(corrected))
}

pub(super) fn render_image_from_rgba(mut image: image::RgbaImage) -> Arc<RenderImage> {
    for pixel in image.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(image)]))
}

fn decode_raster_source(
    path: &Path,
    icc_mode: IccDecodeMode,
    timings_enabled: bool,
    timings: &mut ImageDecodeTimings,
) -> Result<DecodedImage, String> {
    let reader = open_image_reader_with_extension(path, timings_enabled, timings)?;
    if reader.format().is_some() {
        match decode_raster_reader(reader, icc_mode, timings_enabled, timings) {
            Ok(decoded) => return Ok(decoded),
            Err(extension_error) => {
                let reader = open_image_reader_with_guessed_format(path, timings_enabled, timings)?;
                return decode_raster_reader(reader, icc_mode, timings_enabled, timings).map_err(
                    |guess_error| {
                        format!(
                            "{guess_error}; extension-based decode also failed: {extension_error}"
                        )
                    },
                );
            }
        }
    }

    let reader = open_image_reader_with_guessed_format(path, timings_enabled, timings)?;
    decode_raster_reader(reader, icc_mode, timings_enabled, timings)
}

fn open_image_reader_with_extension(
    path: &Path,
    timings_enabled: bool,
    timings: &mut ImageDecodeTimings,
) -> Result<ImageFileReader, String> {
    let read_started = timing_started(timings_enabled);
    let reader =
        image::ImageReader::open(path).map_err(|error| format!("Failed to open image: {error}"));
    timing_add_finished(&mut timings.source_read, read_started);
    reader
}

fn open_image_reader_with_guessed_format(
    path: &Path,
    timings_enabled: bool,
    timings: &mut ImageDecodeTimings,
) -> Result<ImageFileReader, String> {
    let reader = open_image_reader_with_extension(path, timings_enabled, timings)?;
    let format_started = timing_started(timings_enabled);
    let reader = reader
        .with_guessed_format()
        .map_err(|error| format!("Failed to inspect image format: {error}"));
    timing_add_finished(&mut timings.format_detect, format_started);
    reader
}

fn decode_raster_reader(
    reader: ImageFileReader,
    icc_mode: IccDecodeMode,
    timings_enabled: bool,
    timings: &mut ImageDecodeTimings,
) -> Result<DecodedImage, String> {
    let metadata_started = timing_started(timings_enabled);
    let mut decoder = reader
        .into_decoder()
        .map_err(|error| format!("Unsupported image format: {error}"))?;
    let source_color_type = decoder.color_type();
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let icc_profile = decoder.icc_profile().ok().flatten();
    timing_add_finished(&mut timings.metadata, metadata_started);

    let decode_started = timing_started(timings_enabled);
    let mut image = image::DynamicImage::from_decoder(decoder)
        .map_err(|error| format!("Failed to decode image: {error}"))?;
    timing_add_finished(&mut timings.raster_decode, decode_started);

    let orientation_started = timing_started(timings_enabled);
    image.apply_orientation(orientation);
    timing_add_finished(&mut timings.orientation, orientation_started);

    let rgba_started = timing_started(timings_enabled);
    let mut image = image.into_rgba8();
    timing_add_finished(&mut timings.rgba_convert, rgba_started);

    if image.width() == 0 || image.height() == 0 {
        return Err("Image has no renderable dimensions.".to_owned());
    }

    let deferred_icc_profile = match (icc_mode, icc_profile) {
        (IccDecodeMode::ApplySynchronously, Some(icc_profile)) => {
            let icc_started = timing_started(timings_enabled);
            image = apply_icc_profile_to_srgb(image, Some(icc_profile));
            timing_add_finished(&mut timings.icc_convert, icc_started);
            None
        }
        (IccDecodeMode::Defer, Some(icc_profile)) => Some(Arc::new(icc_profile)),
        (IccDecodeMode::Ignore, _) | (_, None) => None,
    };

    let width = image.width();
    let height = image.height();
    let source_decompressed_size_bytes =
        source_decompressed_size_bytes(width, height, source_color_type);
    let render_started = timing_started(timings_enabled);
    let image = render_image_from_rgba(image);
    timing_add_finished(&mut timings.render_image_build, render_started);
    let deferred_icc_correction = deferred_icc_profile.map(|icc_profile| DeferredIccCorrection {
        source_image: image.clone(),
        width,
        height,
        icc_profile,
    });

    Ok(DecodedImage {
        width,
        height,
        source_decompressed_size_bytes,
        deferred_icc_correction,
        source: DecodedImageSource::Raster(image),
    })
}

fn decode_svg_source(
    path: &Path,
    timings_enabled: bool,
    timings: &mut ImageDecodeTimings,
) -> Result<DecodedImage, String> {
    let read_started = timing_started(timings_enabled);
    let bytes = fs::read(path).map_err(|error| format!("Failed to read SVG file: {error}"))?;
    timing_add_finished(&mut timings.source_read, read_started);

    let parse_started = timing_started(timings_enabled);
    let tree = usvg::Tree::from_data(&bytes, &usvg::Options::default())
        .map_err(|error| format!("Failed to parse SVG: {error}"))?;
    timing_add_finished(&mut timings.svg_parse, parse_started);
    let size = tree.size();
    let width = size.width().round() as u32;
    let height = size.height().round() as u32;
    if width == 0 || height == 0 {
        return Err("SVG has no renderable dimensions.".to_owned());
    }

    Ok(DecodedImage {
        width,
        height,
        source_decompressed_size_bytes: None,
        deferred_icc_correction: None,
        source: DecodedImageSource::Svg(Arc::new(bytes)),
    })
}

fn source_decompressed_size_bytes(
    width: u32,
    height: u32,
    color_type: image::ColorType,
) -> Option<u64> {
    u64::from(width)
        .checked_mul(u64::from(height))?
        .checked_mul(u64::from(color_type.bytes_per_pixel()))
}

fn path_is_svg(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
}

fn timing_started(enabled: bool) -> Option<Instant> {
    enabled.then(Instant::now)
}

fn timing_add_finished(slot: &mut Option<Duration>, started: Option<Instant>) {
    if let Some(started) = started {
        let elapsed = started.elapsed();
        *slot = Some(slot.unwrap_or_default() + elapsed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        io::Cursor,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn raster_decode_produces_full_size_render_image() {
        let temp = TestDir::new("decode-render-image");
        let path = temp.path().join("photo.png");
        fs::write(&path, png_bytes(4, 2)).unwrap();

        let decoded = decode_image_source(&path).unwrap();
        let DecodedImageSource::Raster(image) = decoded.source else {
            panic!("expected raster image");
        };
        let size = image.size(0);

        assert_eq!(decoded.width, 4);
        assert_eq!(decoded.height, 2);
        assert_eq!(size.width.0, 4);
        assert_eq!(size.height.0, 2);
        assert!(decoded.deferred_icc_correction.is_none());
    }

    #[test]
    fn extension_decode_falls_back_to_content_sniffing() {
        let temp = TestDir::new("extension-fallback");
        let path = temp.path().join("photo.jpg");
        fs::write(&path, png_bytes(3, 2)).unwrap();

        let decoded = decode_image_source(&path).unwrap();

        assert_eq!(decoded.width, 3);
        assert_eq!(decoded.height, 2);
    }

    #[test]
    fn render_image_from_rgba_uses_gpui_bgra_order() {
        let image = image::RgbaImage::from_raw(1, 1, vec![10, 20, 30, 255]).unwrap();
        let image = render_image_from_rgba(image);

        assert_eq!(image.as_bytes(0).unwrap(), &[30, 20, 10, 255]);
    }

    #[test]
    fn timed_decode_records_raster_stages() {
        let temp = TestDir::new("timed-raster");
        let path = temp.path().join("photo.png");
        fs::write(&path, png_bytes(4, 2)).unwrap();

        let timed = decode_image_source_with_options(&path, IccDecodeMode::Defer, true);

        timed.result.unwrap();
        assert!(timed.timings.source_read.is_some());
        assert!(timed.timings.metadata.is_some());
        assert!(timed.timings.raster_decode.is_some());
        assert!(timed.timings.orientation.is_some());
        assert!(timed.timings.rgba_convert.is_some());
        assert!(timed.timings.render_image_build.is_some());
        assert!(timed.timings.svg_parse.is_none());
    }

    #[test]
    fn timed_decode_records_svg_stages() {
        let temp = TestDir::new("timed-svg");
        let path = temp.path().join("vector.svg");
        fs::write(
            &path,
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="5"></svg>"#,
        )
        .unwrap();

        let timed = decode_image_source_with_options(&path, IccDecodeMode::Defer, true);

        timed.result.unwrap();
        assert!(timed.timings.source_read.is_some());
        assert!(timed.timings.svg_parse.is_some());
        assert!(timed.timings.raster_decode.is_none());
    }

    #[test]
    fn source_decompressed_size_uses_color_type_bytes_per_pixel() {
        assert_eq!(
            source_decompressed_size_bytes(10, 20, image::ColorType::Rgb8),
            Some(600)
        );
        assert_eq!(
            source_decompressed_size_bytes(10, 20, image::ColorType::Rgba16),
            Some(1600)
        );
    }

    #[test]
    fn source_decompressed_size_returns_none_on_overflow() {
        assert_eq!(
            source_decompressed_size_bytes(u32::MAX, u32::MAX, image::ColorType::Rgba32F),
            None
        );
    }

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(width, height));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "explorer-image-decode-{name}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
