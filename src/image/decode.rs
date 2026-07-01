use std::{fs, path::Path, sync::Arc};

use image::ImageDecoder;

use crate::image_viewer::color::apply_icc_profile_to_srgb;

#[derive(Clone)]
pub(super) struct DecodedImage {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) source_decompressed_size_bytes: Option<u64>,
    pub(super) source: DecodedImageSource,
}

#[derive(Clone)]
pub(super) enum DecodedImageSource {
    Raster(Arc<image::RgbaImage>),
    Svg(Arc<Vec<u8>>),
}

pub(super) fn decode_image_source(path: &Path) -> Result<DecodedImage, String> {
    if path_is_svg(path) {
        return decode_svg_source(path);
    }

    decode_raster_source(path)
}

fn decode_raster_source(path: &Path) -> Result<DecodedImage, String> {
    let reader =
        image::ImageReader::open(path).map_err(|error| format!("Failed to open image: {error}"))?;
    let reader = reader
        .with_guessed_format()
        .map_err(|error| format!("Failed to inspect image format: {error}"))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|error| format!("Unsupported image format: {error}"))?;
    let source_color_type = decoder.color_type();
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let icc_profile = decoder.icc_profile().ok().flatten();
    let mut image = image::DynamicImage::from_decoder(decoder)
        .map_err(|error| format!("Failed to decode image: {error}"))?;
    image.apply_orientation(orientation);

    let image = apply_icc_profile_to_srgb(image.into_rgba8(), icc_profile);
    if image.width() == 0 || image.height() == 0 {
        return Err("Image has no renderable dimensions.".to_owned());
    }

    Ok(DecodedImage {
        width: image.width(),
        height: image.height(),
        source_decompressed_size_bytes: source_decompressed_size_bytes(
            image.width(),
            image.height(),
            source_color_type,
        ),
        source: DecodedImageSource::Raster(Arc::new(image)),
    })
}

fn decode_svg_source(path: &Path) -> Result<DecodedImage, String> {
    let bytes = fs::read(path).map_err(|error| format!("Failed to read SVG file: {error}"))?;
    let tree = usvg::Tree::from_data(&bytes, &usvg::Options::default())
        .map_err(|error| format!("Failed to parse SVG: {error}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
