use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

mod color;
mod decode;
mod resize;
mod view;

pub(crate) use view::open_image_window;

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

fn image_like_existing_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) && path_is_image_like(path)
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
