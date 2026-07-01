const SVG_TARGET_BODY_FRACTION: f64 = 0.8;

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

pub(super) fn svg_image_target(
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
    let max_display_width = f64::from(available_width.max(1.0)) * SVG_TARGET_BODY_FRACTION;
    let max_display_height = f64::from(available_height.max(1.0)) * SVG_TARGET_BODY_FRACTION;
    let display_scale = (max_display_width / f64::from(image_width))
        .min(max_display_height / f64::from(image_height));
    let pixel_width =
        (f64::from(image_width) * display_scale * f64::from(scale_factor)).floor() as u32;
    let pixel_height =
        (f64::from(image_height) * display_scale * f64::from(scale_factor)).floor() as u32;
    let pixel_width = pixel_width.max(1);
    let pixel_height = pixel_height.max(1);

    Some(ImageFitTarget {
        pixel_width,
        pixel_height,
        display_width: pixel_width as f32 / scale_factor,
        display_height: pixel_height as f32 / scale_factor,
    })
}

pub(super) fn native_image_target(
    image_width: u32,
    image_height: u32,
    zoom: f64,
    scale_factor: f32,
) -> Option<ImageFitTarget> {
    if image_width == 0 || image_height == 0 {
        return None;
    }

    let scale_factor = scale_factor.max(1.0);
    let zoom = zoom.max(0.0);
    let pixel_width = scaled_dimension(image_width, zoom);
    let pixel_height = scaled_dimension(image_height, zoom);

    Some(ImageFitTarget {
        pixel_width,
        pixel_height,
        display_width: pixel_width as f32 / scale_factor,
        display_height: pixel_height as f32 / scale_factor,
    })
}

pub(super) fn raster_initial_native_zoom(
    image_width: u32,
    image_height: u32,
    available_width: f32,
    available_height: f32,
    scale_factor: f32,
) -> Option<f64> {
    fitted_image_target(
        image_width,
        image_height,
        available_width,
        available_height,
        scale_factor,
    )
    .map(|target| f64::from(target.pixel_width) / f64::from(image_width))
}

pub(super) fn svg_initial_native_zoom(
    image_width: u32,
    image_height: u32,
    available_width: f32,
    available_height: f32,
    scale_factor: f32,
) -> Option<f64> {
    svg_image_target(
        image_width,
        image_height,
        available_width,
        available_height,
        scale_factor,
    )
    .map(|target| f64::from(target.pixel_width) / f64::from(image_width))
}

fn scaled_dimension(source: u32, zoom: f64) -> u32 {
    let scaled = (f64::from(source) * zoom).round();
    if !scaled.is_finite() {
        return u32::MAX;
    }

    scaled.clamp(1.0, f64::from(u32::MAX)) as u32
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
    fn svg_target_uses_eighty_percent_of_square_body() {
        let target = svg_image_target(100, 100, 500.0, 500.0, 1.0).unwrap();

        assert_eq!(target.pixel_width, 400);
        assert_eq!(target.pixel_height, 400);
        assert_eq!(target.display_width, 400.0);
        assert_eq!(target.display_height, 400.0);
    }

    #[test]
    fn svg_target_preserves_landscape_aspect_inside_eighty_percent_box() {
        let target = svg_image_target(200, 100, 500.0, 500.0, 1.0).unwrap();

        assert_eq!(target.pixel_width, 400);
        assert_eq!(target.pixel_height, 200);
        assert_eq!(target.display_width, 400.0);
        assert_eq!(target.display_height, 200.0);
    }

    #[test]
    fn svg_target_preserves_portrait_aspect_inside_eighty_percent_box() {
        let target = svg_image_target(100, 200, 500.0, 500.0, 1.0).unwrap();

        assert_eq!(target.pixel_width, 200);
        assert_eq!(target.pixel_height, 400);
        assert_eq!(target.display_width, 200.0);
        assert_eq!(target.display_height, 400.0);
    }

    #[test]
    fn svg_target_uses_device_pixels_for_scale_factor() {
        let target = svg_image_target(100, 100, 500.0, 500.0, 2.0).unwrap();

        assert_eq!(target.pixel_width, 800);
        assert_eq!(target.pixel_height, 800);
        assert_eq!(target.display_width, 400.0);
        assert_eq!(target.display_height, 400.0);
    }

    #[test]
    fn native_target_uses_zoom_as_source_pixel_ratio_and_device_pixels_for_display() {
        let target = native_image_target(2000, 1000, 0.25, 2.0).unwrap();

        assert_eq!(target.pixel_width, 500);
        assert_eq!(target.pixel_height, 250);
        assert_eq!(target.display_width, 250.0);
        assert_eq!(target.display_height, 125.0);
    }

    #[test]
    fn native_target_clamps_to_at_least_one_pixel() {
        let target = native_image_target(10, 10, 0.02, 1.0).unwrap();

        assert_eq!(target.pixel_width, 1);
        assert_eq!(target.pixel_height, 1);
        assert_eq!(native_image_target(0, 10, 1.0, 1.0), None);
    }

    #[test]
    fn raster_initial_zoom_is_fit_ratio_capped_at_native_size() {
        let large = raster_initial_native_zoom(2000, 1000, 800.0, 600.0, 1.0).unwrap();
        let small = raster_initial_native_zoom(200, 100, 800.0, 600.0, 2.0).unwrap();

        assert_eq!(large, 0.4);
        assert_eq!(small, 1.0);
    }

    #[test]
    fn svg_initial_zoom_uses_eighty_percent_target_ratio() {
        let zoom = svg_initial_native_zoom(100, 100, 500.0, 500.0, 2.0).unwrap();

        assert_eq!(zoom, 8.0);
    }
}
