use moxcms::{ColorProfile, Layout, TransformOptions};

pub(super) fn apply_icc_profile_to_srgb(
    image: image::RgbaImage,
    icc_profile: Option<Vec<u8>>,
) -> image::RgbaImage {
    let Some(icc_profile) = icc_profile else {
        return image;
    };

    convert_rgba_to_srgb(&image, &icc_profile).unwrap_or(image)
}

fn convert_rgba_to_srgb(image: &image::RgbaImage, icc_profile: &[u8]) -> Option<image::RgbaImage> {
    let source = ColorProfile::new_from_slice(icc_profile).ok()?;
    let target = ColorProfile::new_srgb();
    let transform = source
        .create_transform_8bit(
            Layout::Rgba,
            &target,
            Layout::Rgba,
            TransformOptions::default(),
        )
        .ok()?;
    let mut converted = vec![0; image.as_raw().len()];
    transform.transform(image.as_raw(), &mut converted).ok()?;
    image::RgbaImage::from_raw(image.width(), image.height(), converted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_icc_profile_falls_back_to_source_pixels() {
        let source = image::RgbaImage::from_pixel(2, 1, image::Rgba([10, 20, 30, 255]));
        let converted = apply_icc_profile_to_srgb(source.clone(), Some(vec![1, 2, 3]));

        assert_eq!(converted, source);
    }

    #[test]
    fn missing_icc_profile_keeps_source_pixels() {
        let source = image::RgbaImage::from_pixel(1, 1, image::Rgba([40, 50, 60, 128]));
        let converted = apply_icc_profile_to_srgb(source.clone(), None);

        assert_eq!(converted, source);
    }
}
