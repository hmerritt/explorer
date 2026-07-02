use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use gpui::actions;

mod color;
mod decode;
mod resize;
mod view;

actions!(
    image_viewer,
    [
        ImageZoomIn,
        ImageZoomOut,
        ImageToggleActualSize,
        ImageOpenPrevious,
        ImageOpenNext
    ]
);

pub(crate) use view::open_image_window;

#[cfg(feature = "benchmarks")]
pub mod benchmark_support {
    use std::{fs, path::Path};

    use super::{
        decode::{
            DecodedImageSource, IccDecodeMode, apply_deferred_icc_correction,
            decode_image_source_with_options, render_image_from_rgba,
        },
        resize::ImageFitTarget,
        view::render_svg_for_target,
    };

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum ImageViewerBenchmarkIccMode {
        ApplySynchronously,
        Defer,
        Ignore,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct ImageViewerOpenBenchmarkResult {
        pub width: u32,
        pub height: u32,
        pub render_bytes: usize,
        pub source_decompressed_size_bytes: Option<u64>,
        pub has_deferred_icc: bool,
    }

    #[derive(Clone)]
    pub struct ImageViewerDeferredIccBenchmarkInput {
        correction: super::decode::DeferredIccCorrection,
    }

    pub fn open_image_viewer_for_benchmark(
        path: &Path,
        mode: ImageViewerBenchmarkIccMode,
    ) -> Result<ImageViewerOpenBenchmarkResult, String> {
        let decoded =
            decode_image_source_with_options(path, mode.into_decode_mode(), false).result?;
        let has_deferred_icc = decoded.deferred_icc_correction.is_some();
        let render_bytes = match &decoded.source {
            DecodedImageSource::Raster(image) => image.as_bytes(0).map_or(0, |bytes| bytes.len()),
            DecodedImageSource::Svg(bytes) => {
                let image = render_svg_for_target(
                    bytes,
                    ImageFitTarget {
                        pixel_width: decoded.width,
                        pixel_height: decoded.height,
                        display_width: decoded.width as f32,
                        display_height: decoded.height as f32,
                    },
                )?;
                image.as_bytes(0).map_or(0, |bytes| bytes.len())
            }
        };

        Ok(ImageViewerOpenBenchmarkResult {
            width: decoded.width,
            height: decoded.height,
            render_bytes,
            source_decompressed_size_bytes: decoded.source_decompressed_size_bytes,
            has_deferred_icc,
        })
    }

    pub fn apply_deferred_icc_for_benchmark(path: &Path) -> Result<usize, String> {
        let input = deferred_icc_input_for_benchmark(path)?;
        apply_deferred_icc_input_for_benchmark(input)
    }

    pub fn deferred_icc_input_for_benchmark(
        path: &Path,
    ) -> Result<ImageViewerDeferredIccBenchmarkInput, String> {
        let decoded = decode_image_source_with_options(path, IccDecodeMode::Defer, false).result?;
        let correction = decoded
            .deferred_icc_correction
            .ok_or_else(|| "Benchmark fixture did not contain an ICC profile.".to_owned())?;

        Ok(ImageViewerDeferredIccBenchmarkInput { correction })
    }

    pub fn apply_deferred_icc_input_for_benchmark(
        input: ImageViewerDeferredIccBenchmarkInput,
    ) -> Result<usize, String> {
        let image = apply_deferred_icc_correction(input.correction)?;

        Ok(image.as_bytes(0).map_or(0, |bytes| bytes.len()))
    }

    pub fn render_image_from_rgba_for_benchmark(image: image::RgbaImage) -> usize {
        render_image_from_rgba(image)
            .as_bytes(0)
            .map_or(0, |bytes| bytes.len())
    }

    pub fn render_svg_native_for_benchmark(path: &Path) -> Result<usize, String> {
        let decoded = decode_image_source_with_options(path, IccDecodeMode::Defer, false).result?;
        let DecodedImageSource::Svg(bytes) = decoded.source else {
            return Err("Benchmark fixture was not an SVG.".to_owned());
        };
        let image = render_svg_for_target(
            &bytes,
            ImageFitTarget {
                pixel_width: decoded.width,
                pixel_height: decoded.height,
                display_width: decoded.width as f32,
                display_height: decoded.height as f32,
            },
        )?;

        Ok(image.as_bytes(0).map_or(0, |bytes| bytes.len()))
    }

    pub fn image_file_len_for_benchmark(path: &Path) -> u64 {
        fs::metadata(path).map_or(0, |metadata| metadata.len())
    }

    impl ImageViewerBenchmarkIccMode {
        fn into_decode_mode(self) -> IccDecodeMode {
            match self {
                Self::ApplySynchronously => IccDecodeMode::ApplySynchronously,
                Self::Defer => IccDecodeMode::Defer,
                Self::Ignore => IccDecodeMode::Ignore,
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImageNavigationDirection {
    Previous,
    Next,
}

pub(crate) fn startup_image_path(args: impl IntoIterator<Item = OsString>) -> Option<PathBuf> {
    let mut args = args.into_iter();
    let _program = args.next();

    while let Some(arg) = args.next() {
        let arg_text = arg.to_string_lossy();
        if arg_text == "--debug" {
            let _debug_value = args.next();
            continue;
        }
        if arg_text.starts_with("--debug=") || arg_text.starts_with('-') {
            continue;
        }

        let path = PathBuf::from(arg);
        return image_like_existing_file(&path).then_some(path);
    }

    None
}

pub(crate) fn adjacent_image_path(
    path: &Path,
    direction: ImageNavigationDirection,
) -> Option<PathBuf> {
    let current_name = path.file_name()?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut images = fs::read_dir(parent)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| image_like_existing_file(path))
        .collect::<Vec<_>>();

    if images.len() <= 1 {
        return None;
    }

    images.sort_by(|left, right| {
        crate::explorer::compare_file_names(
            image_sort_name(left).as_ref(),
            image_sort_name(right).as_ref(),
        )
        .then_with(|| left.cmp(right))
    });

    let current_index = images
        .iter()
        .position(|candidate| candidate.file_name() == Some(current_name))?;
    let target_index = match direction {
        ImageNavigationDirection::Previous => {
            if current_index == 0 {
                images.len() - 1
            } else {
                current_index - 1
            }
        }
        ImageNavigationDirection::Next => (current_index + 1) % images.len(),
    };

    Some(images[target_index].clone())
}

fn image_like_existing_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) && path_is_image_like(path)
}

fn image_sort_name(path: &Path) -> std::borrow::Cow<'_, str> {
    path.file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| path.as_os_str().to_string_lossy())
}

fn path_is_image_like(path: &Path) -> bool {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("svg")
                || image::ImageFormat::from_extension(extension).is_some()
        })
    {
        return true;
    }

    if mime_guess::from_path(path)
        .first_raw()
        .is_some_and(|mime| mime.starts_with("image/"))
    {
        return true;
    }

    image::ImageReader::open(path)
        .and_then(|reader| reader.with_guessed_format())
        .is_ok_and(|reader| reader.format().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        io::Cursor,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn args(values: &[&Path]) -> Vec<OsString> {
        values
            .iter()
            .map(|value| value.as_os_str().to_os_string())
            .collect()
    }

    fn string_args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn startup_image_path_routes_existing_image_extension() {
        let temp = TestDir::new("image-extension");
        let path = temp.path().join("photo.png");
        fs::write(&path, b"not decoded during routing").unwrap();

        let mut values = vec![OsString::from("explorer")];
        values.extend(args(&[&path]));

        assert_eq!(startup_image_path(values), Some(path));
    }

    #[test]
    fn startup_image_path_routes_extensionless_detectable_image() {
        let temp = TestDir::new("extensionless-image");
        let path = temp.path().join("photo");
        fs::write(&path, png_bytes(2, 1)).unwrap();

        let mut values = vec![OsString::from("explorer")];
        values.extend(args(&[&path]));

        assert_eq!(startup_image_path(values), Some(path));
    }

    #[test]
    fn startup_image_path_falls_back_for_missing_directory_and_non_image_paths() {
        let temp = TestDir::new("fallback");
        let missing = temp.path().join("missing.png");
        let directory = temp.path().join("folder.jpg");
        let text = temp.path().join("note.txt");
        fs::create_dir(&directory).unwrap();
        fs::write(&text, b"hello").unwrap();

        for path in [missing, directory, text] {
            let mut values = vec![OsString::from("explorer")];
            values.extend(args(&[&path]));
            assert_eq!(startup_image_path(values), None);
        }
    }

    #[test]
    fn startup_image_path_skips_debug_args_before_image_path() {
        let temp = TestDir::new("debug-args");
        let path = temp.path().join("photo.png");
        fs::write(&path, b"not decoded during routing").unwrap();
        let mut values = string_args(&["explorer", "--debug=nav", "--debug", "icons"]);
        values.push(path.as_os_str().to_os_string());

        assert_eq!(startup_image_path(values), Some(path));
    }

    #[test]
    fn startup_image_path_does_not_scan_after_first_positional_fallback() {
        let temp = TestDir::new("first-positional");
        let text = temp.path().join("note.txt");
        let image = temp.path().join("photo.png");
        fs::write(&text, b"hello").unwrap();
        fs::write(&image, b"not decoded during routing").unwrap();
        let mut values = vec![OsString::from("explorer")];
        values.extend(args(&[&text, &image]));

        assert_eq!(startup_image_path(values), None);
    }

    #[test]
    fn adjacent_image_path_filters_supported_files_and_sorts_naturally() {
        let temp = TestDir::new("adjacent-sort");
        let file1 = temp.path().join("file1.png");
        let file2 = temp.path().join("file2");
        let file10 = temp.path().join("file10.png");
        let unsupported = temp.path().join("file3.txt");
        let directory = temp.path().join("file0.png");
        fs::write(&file1, b"extension is enough for routing").unwrap();
        fs::write(&file2, png_bytes(1, 1)).unwrap();
        fs::write(&file10, b"extension is enough for routing").unwrap();
        fs::write(&unsupported, b"not an image").unwrap();
        fs::create_dir(&directory).unwrap();

        assert_eq!(
            adjacent_image_path(&file2, ImageNavigationDirection::Previous),
            Some(file1.clone())
        );
        assert_eq!(
            adjacent_image_path(&file2, ImageNavigationDirection::Next),
            Some(file10.clone())
        );
    }

    #[test]
    fn adjacent_image_path_wraps_at_directory_edges() {
        let temp = TestDir::new("adjacent-wrap");
        let first = temp.path().join("file1.png");
        let middle = temp.path().join("file2.png");
        let last = temp.path().join("file10.png");
        fs::write(&first, b"extension is enough for routing").unwrap();
        fs::write(&middle, b"extension is enough for routing").unwrap();
        fs::write(&last, b"extension is enough for routing").unwrap();

        assert_eq!(
            adjacent_image_path(&first, ImageNavigationDirection::Previous),
            Some(last.clone())
        );
        assert_eq!(
            adjacent_image_path(&last, ImageNavigationDirection::Next),
            Some(first)
        );
    }

    #[test]
    fn adjacent_image_path_requires_another_supported_image_and_current_entry() {
        let temp = TestDir::new("adjacent-none");
        let only = temp.path().join("only.png");
        let missing = temp.path().join("missing.png");
        fs::write(&only, b"extension is enough for routing").unwrap();

        assert_eq!(
            adjacent_image_path(&only, ImageNavigationDirection::Next),
            None
        );

        let other = temp.path().join("other.png");
        fs::write(&other, b"extension is enough for routing").unwrap();
        assert_eq!(
            adjacent_image_path(&missing, ImageNavigationDirection::Next),
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
                "explorer-image-viewer-{name}-{}-{nanos}",
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
