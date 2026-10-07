use std::{
    io::Cursor,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use rbook::Epub;

use super::{image_preview::load_svg_rgba_from_bytes, image_resize::resize_dynamic_to_rgba};

pub(super) fn path_may_have_epub_preview(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("epub"))
}

pub(super) fn load_epub_cover_rgba(
    path: &Path,
    size: u32,
    cancel: &AtomicBool,
) -> Result<image::RgbaImage, String> {
    if size == 0 {
        return Err("EPUB preview target has no dimensions.".to_owned());
    }
    check_epub_preview_cancelled(cancel)?;
    let epub = Epub::options()
        .skip_spine(true)
        .skip_toc(true)
        .open(path)
        .map_err(|error| format!("Failed to open EPUB: {error}"))?;
    check_epub_preview_cancelled(cancel)?;

    let cover = epub
        .manifest()
        .cover_image()
        .ok_or_else(|| "EPUB has no cover image.".to_owned())?;
    let bytes = cover
        .read_bytes()
        .map_err(|error| format!("Failed to read EPUB cover: {error}"))?;
    check_epub_preview_cancelled(cancel)?;

    let preview = if cover.media_type() == "image/svg+xml" {
        load_svg_rgba_from_bytes(&bytes, size, cancel)?
    } else {
        let image = image::ImageReader::new(Cursor::new(&bytes))
            .with_guessed_format()
            .map_err(|error| format!("Failed to detect EPUB cover format: {error}"))?
            .decode()
            .map_err(|error| format!("Failed to decode EPUB cover: {error}"))?;
        check_epub_preview_cancelled(cancel)?;
        resize_dynamic_to_rgba(image, size)?
    };
    check_epub_preview_cancelled(cancel)?;
    Ok(preview)
}

fn check_epub_preview_cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("EPUB preview was cancelled.".to_owned())
    } else {
        Ok(())
    }
}

#[cfg(test)]
pub(super) mod test_support {
    use std::{fs::File, io::Write, path::Path};

    use zip::{ZipWriter, write::FileOptions};

    pub(in crate::explorer) fn raster_cover(
        width: u32,
        height: u32,
        format: image::ImageFormat,
    ) -> Vec<u8> {
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            width,
            height,
            image::Rgb([255, 0, 0]),
        ));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    pub(in crate::explorer) fn write_epub(
        path: &Path,
        version: &str,
        cover: Option<(&str, &str)>,
        resource: Option<(&str, &[u8])>,
    ) {
        let mut zip = ZipWriter::new(File::create(path).unwrap());
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("mimetype", options).unwrap();
        zip.write_all(b"application/epub+zip").unwrap();
        zip.start_file("META-INF/container.xml", options).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/package.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();
        let cover_metadata = if version == "2.0" && cover.is_some() {
            r#"<meta name="cover" content="cover"/>"#
        } else {
            ""
        };
        let cover_entry = cover.map_or_else(String::new, |(href, media_type)| {
            let properties = if version == "3.0" {
                r#" properties="cover-image""#
            } else {
                ""
            };
            format!(r#"<item id="cover" href="{href}" media-type="{media_type}"{properties}/>"#)
        });
        let opf = format!(
            r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="{version}" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">urn:test:cover</dc:identifier><dc:title>Cover test</dc:title><dc:language>en</dc:language>{cover_metadata}</metadata><manifest><item id="illustration" href="illustration.png" media-type="image/png"/>{cover_entry}<item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="chapter"/></spine></package>"#,
        );
        zip.start_file("OEBPS/package.opf", options).unwrap();
        zip.write_all(opf.as_bytes()).unwrap();
        zip.start_file("OEBPS/chapter.xhtml", options).unwrap();
        zip.write_all(br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Test</title></head><body><p>Test chapter</p></body></html>"#).unwrap();
        zip.start_file("OEBPS/illustration.png", options).unwrap();
        zip.write_all(&raster_cover(8, 8, image::ImageFormat::Png))
            .unwrap();
        if let Some((name, bytes)) = resource {
            zip.start_file(format!("OEBPS/{name}"), options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::{test_support::*, *};
    use tempfile::tempdir;

    #[test]
    fn epub_preview_path_detection_is_case_insensitive() {
        assert!(path_may_have_epub_preview(Path::new("book.epub")));
        assert!(path_may_have_epub_preview(Path::new("book.EPUB")));
        assert!(!path_may_have_epub_preview(Path::new("book.epub.txt")));
        assert!(!path_may_have_epub_preview(Path::new("book")));
    }

    #[test]
    fn epub2_and_epub3_declared_raster_covers_preserve_aspect_ratio() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("book.EPUB");
        for version in ["2.0", "3.0"] {
            for (format, mime) in [
                (image::ImageFormat::Png, "image/png"),
                (image::ImageFormat::Jpeg, "image/jpeg"),
            ] {
                for (width, height, expected) in [(8, 4, (400, 200)), (4, 8, (200, 400))] {
                    let bytes = raster_cover(width, height, format);
                    write_epub(
                        &path,
                        version,
                        Some(("images/cover%20image", mime)),
                        Some(("images/cover image", &bytes)),
                    );
                    let preview = load_epub_cover_rgba(&path, 400, &AtomicBool::new(false))
                        .expect("decode declared cover instead of first illustration");
                    assert_eq!(preview.dimensions(), expected);
                    let pixel = preview.get_pixel(expected.0 / 2, expected.1 / 2).0;
                    assert!(pixel[0] >= 250 && pixel[1] <= 5 && pixel[2] <= 5);
                    assert_eq!(pixel[3], 255);
                }
            }
        }
    }

    #[test]
    fn svg_cover_preserves_aspect_ratio_and_straight_alpha() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("vector.epub");
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="200"><rect width="100" height="200" fill="red" fill-opacity="0.5"/></svg>"#;
        write_epub(
            &path,
            "3.0",
            Some(("images/cover.svg", "image/svg+xml")),
            Some(("images/cover.svg", svg)),
        );
        let preview = load_epub_cover_rgba(&path, 400, &AtomicBool::new(false)).unwrap();
        assert_eq!(preview.dimensions(), (200, 400));
        let pixel = preview.get_pixel(100, 200).0;
        assert!(pixel[0] >= 254);
        assert_eq!(&pixel[1..], &[0, 0, 128]);
    }

    #[test]
    fn missing_or_unreadable_covers_fail_without_using_other_images() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("book.epub");
        for version in ["2.0", "3.0"] {
            write_epub(&path, version, None, None);
            assert!(load_epub_cover_rgba(&path, 400, &AtomicBool::new(false)).is_err());
            write_epub(&path, version, Some(("missing.png", "image/png")), None);
            assert!(load_epub_cover_rgba(&path, 400, &AtomicBool::new(false)).is_err());
            for (name, mime, bytes) in [
                ("broken.png", "image/png", b"not an image".as_slice()),
                ("broken.svg", "image/svg+xml", b"not SVG".as_slice()),
            ] {
                write_epub(&path, version, Some((name, mime)), Some((name, bytes)));
                assert!(load_epub_cover_rgba(&path, 400, &AtomicBool::new(false)).is_err());
            }
        }
        std::fs::write(&path, b"not an EPUB").unwrap();
        assert!(load_epub_cover_rgba(&path, 400, &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn zero_sized_and_cancelled_previews_fail_before_file_io() {
        let path = Path::new("missing.epub");
        assert_eq!(
            load_epub_cover_rgba(path, 0, &AtomicBool::new(false)).unwrap_err(),
            "EPUB preview target has no dimensions.",
        );
        assert_eq!(
            load_epub_cover_rgba(path, 400, &AtomicBool::new(true)).unwrap_err(),
            "EPUB preview was cancelled.",
        );
    }
}
