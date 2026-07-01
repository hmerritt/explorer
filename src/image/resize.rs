use std::sync::Arc;

use fast_image_resize::{FilterType, ResizeAlg, ResizeOptions, Resizer};
use gpui::RenderImage;

use crate::image_viewer::decode::{DecodedImage, DecodedImageSource};

const LANCZOS_OPTIONS: ResizeOptions = ResizeOptions {
    algorithm: ResizeAlg::Convolution(FilterType::Lanczos3),
    cropping: fast_image_resize::SrcCropping::None,
    mul_div_alpha: true,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ImageFitTarget {
    pub(super) pixel_width: u32,
    pub(super) pixel_height: u32,
    pub(super) display_width: f32,
    pub(super) display_height: f32,
}

pub(super) fn fitted_image_target(
    image_width: u32,
    image_height: u32,
    available_width: f32,
    available_height: f32,
    scale_factor: f32,
) -> Option<ImageFitTarget> {
    if image_width == 0 || image_height == 0 {
        return None;
    }

    let scale_factor = scale_factor.max(1.0);
    let max_pixel_width = ((available_width.max(1.0) * scale_factor).floor() as u32).max(1);
    let max_pixel_height = ((available_height.max(1.0) * scale_factor).floor() as u32).max(1);
    let scale = (max_pixel_width as f64 / f64::from(image_width))
        .min(max_pixel_height as f64 / f64::from(image_height))
        .min(1.0);
    let pixel_width = (f64::from(image_width) * scale).round() as u32;
    let pixel_height = (f64::from(image_height) * scale).round() as u32;
    let pixel_width = pixel_width.clamp(1, image_width);
    let pixel_height = pixel_height.clamp(1, image_height);

    Some(ImageFitTarget {
        pixel_width,
        pixel_height,
        display_width: pixel_width as f32 / scale_factor,
        display_height: pixel_height as f32 / scale_factor,
    })
}

pub(super) fn render_image_for_target(
    decoded: &DecodedImage,
    target: ImageFitTarget,
) -> Result<Arc<RenderImage>, String> {
    match &decoded.source {
        DecodedImageSource::Raster(image) => {
            let resized = resize_raster_to_target(image, target)?;
            Ok(Arc::new(RenderImage::new(vec![image::Frame::new(resized)])))
        }
        DecodedImageSource::Svg(bytes) => {
            let rendered = render_svg_to_target(bytes, target)?;
            Ok(Arc::new(RenderImage::new(vec![image::Frame::new(
                rendered,
            )])))
        }
    }
}

fn resize_raster_to_target(
    image: &image::RgbaImage,
    target: ImageFitTarget,
) -> Result<image::RgbaImage, String> {
    if image.width() == target.pixel_width && image.height() == target.pixel_height {
        return Ok(image.clone());
    }

    let mut resized = image::RgbaImage::new(target.pixel_width, target.pixel_height);
    Resizer::new()
        .resize(image, &mut resized, &LANCZOS_OPTIONS)
        .map_err(|error| format!("Failed to resize image: {error}"))?;
    Ok(resized)
}

fn render_svg_to_target(bytes: &[u8], target: ImageFitTarget) -> Result<image::RgbaImage, String> {
    let tree = usvg::Tree::from_data(bytes, &usvg::Options::default())
        .map_err(|error| format!("Failed to parse SVG: {error}"))?;
    let svg_size = tree.size();
    let scale_x = target.pixel_width as f32 / svg_size.width();
    let scale_y = target.pixel_height as f32 / svg_size.height();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(target.pixel_width, target.pixel_height)
        .ok_or_else(|| "Failed to allocate SVG render target.".to_owned())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale_x, scale_y),
        &mut pixmap.as_mut(),
    );
    let mut image =
        image::RgbaImage::from_raw(target.pixel_width, target.pixel_height, pixmap.take())
            .ok_or_else(|| "Failed to create SVG image buffer.".to_owned())?;
    unpremultiply_rgba(&mut image);
    Ok(image)
}

fn unpremultiply_rgba(image: &mut image::RgbaImage) {
    for pixel in image.pixels_mut() {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 || alpha == 255 {
            continue;
        }

        for channel in &mut pixel.0[..3] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
}

#[cfg(test)]
pub(super) fn resize_algorithm_is_lanczos3() -> bool {
    matches!(
        LANCZOS_OPTIONS.algorithm,
        ResizeAlg::Convolution(FilterType::Lanczos3)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_target_shrinks_large_landscape_and_excludes_titlebar_area() {
        let target = fitted_image_target(2000, 1000, 800.0, 564.0, 1.0).unwrap();

        assert_eq!(target.pixel_width, 800);
        assert_eq!(target.pixel_height, 400);
        assert_eq!(target.display_width, 800.0);
        assert_eq!(target.display_height, 400.0);
    }

    #[test]
    fn fit_target_shrinks_large_portrait() {
        let target = fitted_image_target(1000, 2000, 800.0, 600.0, 1.0).unwrap();

        assert_eq!(target.pixel_width, 300);
        assert_eq!(target.pixel_height, 600);
    }

    #[test]
    fn fit_target_never_upscales_small_images_and_uses_device_pixels() {
        let target = fitted_image_target(200, 100, 800.0, 600.0, 2.0).unwrap();

        assert_eq!(target.pixel_width, 200);
        assert_eq!(target.pixel_height, 100);
        assert_eq!(target.display_width, 100.0);
        assert_eq!(target.display_height, 50.0);
    }

    #[test]
    fn fit_target_clamps_empty_available_space_and_rejects_empty_images() {
        assert_eq!(fitted_image_target(0, 1, 100.0, 100.0, 1.0), None);
        let target = fitted_image_target(100, 50, 0.0, 0.0, 1.0).unwrap();

        assert_eq!(target.pixel_width, 1);
        assert_eq!(target.pixel_height, 1);
    }

    #[test]
    fn resize_helper_uses_lanczos3() {
        assert!(resize_algorithm_is_lanczos3());
    }

    #[test]
    fn raster_resize_preserves_requested_dimensions() {
        let image = image::RgbaImage::from_pixel(4, 2, image::Rgba([20, 40, 60, 255]));
        let resized = resize_raster_to_target(
            &image,
            ImageFitTarget {
                pixel_width: 2,
                pixel_height: 1,
                display_width: 2.0,
                display_height: 1.0,
            },
        )
        .unwrap();

        assert_eq!(resized.dimensions(), (2, 1));
    }
}
