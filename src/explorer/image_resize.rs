use std::cell::RefCell;

use fast_image_resize::{FilterType, ResizeAlg, ResizeOptions, Resizer};

thread_local! {
    static RGBA_RESIZER: RefCell<Resizer> = RefCell::new(Resizer::new());
}

const CATMULL_ROM_OPTIONS: ResizeOptions = ResizeOptions {
    algorithm: ResizeAlg::Interpolation(FilterType::CatmullRom),
    cropping: fast_image_resize::SrcCropping::None,
    mul_div_alpha: true,
};

pub(super) fn resize_rgba(
    image: image::RgbaImage,
    width: u32,
    height: u32,
) -> Result<image::RgbaImage, String> {
    if image.width() == 0 || image.height() == 0 {
        return Err("Source image has no dimensions.".to_owned());
    }
    if width == 0 || height == 0 {
        return Err("Image resize target has no dimensions.".to_owned());
    }
    if image.width() == width && image.height() == height {
        return Ok(image);
    }

    let mut resized = image::RgbaImage::new(width, height);
    RGBA_RESIZER.with(|resizer| {
        resizer
            .borrow_mut()
            .resize(&image, &mut resized, &CATMULL_ROM_OPTIONS)
            .map_err(|error| format!("Failed to resize image: {error}"))
    })?;
    Ok(resized)
}

pub(super) fn resize_rgb(
    image: image::RgbImage,
    width: u32,
    height: u32,
) -> Result<image::RgbImage, String> {
    if image.width() == 0 || image.height() == 0 || width == 0 || height == 0 {
        return Err("Image resize dimensions must be non-zero.".to_owned());
    }
    if image.width() == width && image.height() == height {
        return Ok(image);
    }

    let mut resized = image::RgbImage::new(width, height);
    RGBA_RESIZER.with(|resizer| {
        resizer
            .borrow_mut()
            .resize(&image, &mut resized, &CATMULL_ROM_OPTIONS)
            .map_err(|error| format!("Failed to resize image: {error}"))
    })?;
    Ok(resized)
}

pub(super) fn resize_luma(
    image: image::GrayImage,
    width: u32,
    height: u32,
) -> Result<image::GrayImage, String> {
    if image.width() == 0 || image.height() == 0 || width == 0 || height == 0 {
        return Err("Image resize dimensions must be non-zero.".to_owned());
    }
    if image.width() == width && image.height() == height {
        return Ok(image);
    }

    let mut resized = image::GrayImage::new(width, height);
    RGBA_RESIZER.with(|resizer| {
        resizer
            .borrow_mut()
            .resize(&image, &mut resized, &CATMULL_ROM_OPTIONS)
            .map_err(|error| format!("Failed to resize image: {error}"))
    })?;
    Ok(resized)
}

pub(super) fn resize_dynamic_to_rgba(
    image: image::DynamicImage,
    longest_side: u32,
) -> Result<image::RgbaImage, String> {
    let (width, height) = dimensions_for_longest_side(image.width(), image.height(), longest_side)
        .ok_or_else(|| "Image has no dimensions.".to_owned())?;
    match image {
        image::DynamicImage::ImageRgb8(image) => {
            Ok(image::DynamicImage::ImageRgb8(resize_rgb(image, width, height)?).into_rgba8())
        }
        image::DynamicImage::ImageLuma8(image) => {
            Ok(image::DynamicImage::ImageLuma8(resize_luma(image, width, height)?).into_rgba8())
        }
        image::DynamicImage::ImageRgba8(image) => resize_rgba(image, width, height),
        image => resize_rgba(image.into_rgba8(), width, height),
    }
}

pub(super) fn resize_rgba_to_longest_side(
    image: image::RgbaImage,
    longest_side: u32,
) -> Result<image::RgbaImage, String> {
    let (width, height) = dimensions_for_longest_side(image.width(), image.height(), longest_side)
        .ok_or_else(|| "Image has no dimensions.".to_owned())?;
    resize_rgba(image, width, height)
}

pub(super) fn dimensions_for_longest_side(
    width: u32,
    height: u32,
    longest_side: u32,
) -> Option<(u32, u32)> {
    if width == 0 || height == 0 || longest_side == 0 {
        return None;
    }

    let scale = longest_side as f64 / f64::from(width.max(height));
    let resized_width = (f64::from(width) * scale).round() as u32;
    let resized_height = (f64::from(height) * scale).round() as u32;
    Some((
        resized_width.clamp(1, longest_side),
        resized_height.clamp(1, longest_side),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_side_dimensions_preserve_orientation_and_aspect_ratio() {
        assert_eq!(dimensions_for_longest_side(8, 4, 128), Some((128, 64)));
        assert_eq!(dimensions_for_longest_side(3, 6, 128), Some((64, 128)));
        assert_eq!(dimensions_for_longest_side(4, 4, 128), Some((128, 128)));
        assert_eq!(dimensions_for_longest_side(0, 4, 128), None);
        assert_eq!(dimensions_for_longest_side(4, 4, 0), None);
    }

    #[test]
    fn resize_rgba_returns_owned_source_for_matching_dimensions() {
        let image = image::RgbaImage::from_pixel(2, 1, image::Rgba([10, 20, 30, 40]));
        let source_ptr = image.as_ptr();

        let resized = resize_rgba(image, 2, 1).unwrap();

        assert_eq!(resized.as_ptr(), source_ptr);
    }

    #[test]
    fn catmull_rom_resize_interpolates_opaque_pixels() {
        let image =
            image::RgbaImage::from_raw(2, 1, vec![0, 0, 0, 255, 255, 255, 255, 255]).unwrap();

        let resized = resize_rgba(image, 3, 1).unwrap();

        let middle = resized.get_pixel(1, 0);
        assert!(middle[0] > 100 && middle[0] < 155, "{middle:?}");
        assert_eq!(middle[0], middle[1]);
        assert_eq!(middle[1], middle[2]);
        assert_eq!(middle[3], 255);
    }

    #[test]
    fn catmull_rom_resize_accounts_for_alpha_at_transparent_edges() {
        let image = image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 0]).unwrap();

        let resized = resize_rgba(image, 3, 1).unwrap();

        let middle = resized.get_pixel(1, 0);
        assert!(middle[0] > 240, "{middle:?}");
        assert!(middle[2] < 16, "{middle:?}");
        assert!(middle[3] > 100 && middle[3] < 155, "{middle:?}");
    }

    #[test]
    fn resize_rejects_zero_dimensions() {
        assert!(resize_rgba(image::RgbaImage::new(0, 1), 1, 1).is_err());
        assert!(resize_rgba(image::RgbaImage::new(1, 1), 0, 1).is_err());
    }
}
