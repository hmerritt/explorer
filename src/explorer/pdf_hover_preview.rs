use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use pdf_oxide::{
    PdfDocument,
    rendering::{ImageFormat, RenderOptions, render_page_fit},
};

pub(super) fn path_may_have_pdf_preview(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

pub(super) fn load_pdf_first_page_rgba(
    path: &Path,
    size: u32,
    cancel: &AtomicBool,
) -> Result<image::RgbaImage, String> {
    if size == 0 {
        return Err("PDF preview target has no dimensions.".to_owned());
    }

    check_pdf_preview_cancelled(cancel)?;
    let document =
        PdfDocument::open(path).map_err(|error| format!("Failed to open PDF document: {error}"))?;
    check_pdf_preview_cancelled(cancel)?;

    let options = RenderOptions::default().as_raw();
    let rendered = render_page_fit(&document, 0, size, size, &options)
        .map_err(|error| format!("Failed to render first PDF page: {error}"))?;
    check_pdf_preview_cancelled(cancel)?;

    if rendered.format != ImageFormat::RawRgba8 || rendered.width == 0 || rendered.height == 0 {
        return Err("PDF renderer returned an invalid image.".to_owned());
    }
    let expected_len = usize::try_from(rendered.width)
        .ok()
        .and_then(|width| {
            usize::try_from(rendered.height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "PDF preview dimensions overflowed.".to_owned())?;
    if rendered.data.len() != expected_len {
        return Err("PDF renderer returned an unexpected pixel buffer.".to_owned());
    }

    image::RgbaImage::from_raw(rendered.width, rendered.height, rendered.data)
        .ok_or_else(|| "Failed to create the PDF preview image.".to_owned())
}

fn check_pdf_preview_cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("PDF preview was cancelled.".to_owned())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_oxide::writer::PdfWriter;
    use tempfile::tempdir;

    #[test]
    fn pdf_preview_path_detection_is_case_insensitive() {
        assert!(path_may_have_pdf_preview(Path::new("document.pdf")));
        assert!(path_may_have_pdf_preview(Path::new("document.PDF")));
        assert!(!path_may_have_pdf_preview(Path::new("document.pdf.txt")));
        assert!(!path_may_have_pdf_preview(Path::new("document")));
    }

    #[test]
    fn zero_sized_and_cancelled_previews_fail_before_file_io() {
        let active = AtomicBool::new(false);
        assert!(load_pdf_first_page_rgba(Path::new("missing.pdf"), 0, &active).is_err());

        let cancelled = AtomicBool::new(true);
        assert!(load_pdf_first_page_rgba(Path::new("missing.pdf"), 400, &cancelled).is_err());
    }

    #[test]
    fn first_page_is_rendered_to_the_requested_aspect_fit() {
        let temp = tempdir().expect("create PDF preview test directory");
        let path = temp.path().join("landscape.pdf");
        let mut writer = PdfWriter::new();
        {
            let mut page = writer.add_page(400.0, 200.0);
            page.fill_rect_colored(0.0, 0.0, 400.0, 200.0, 1.0, 0.0, 0.0);
            page.finish();
        }
        {
            let mut page = writer.add_page(400.0, 200.0);
            page.fill_rect_colored(0.0, 0.0, 400.0, 200.0, 0.0, 0.0, 1.0);
            page.finish();
        }
        std::fs::write(&path, writer.finish().expect("finish test PDF")).expect("save test PDF");

        let preview = load_pdf_first_page_rgba(&path, 400, &AtomicBool::new(false))
            .expect("render first PDF page");

        assert_eq!(preview.dimensions(), (400, 200));
        assert_eq!(preview.as_raw().len(), 400 * 200 * 4);
        assert_eq!(preview.get_pixel(200, 100).0, [255, 0, 0, 255]);
    }

    #[test]
    fn invalid_pdf_fails_without_panicking() {
        let temp = tempdir().expect("create PDF preview test directory");
        let path = temp.path().join("broken.pdf");
        std::fs::write(&path, b"not a PDF").expect("write invalid PDF");

        assert!(load_pdf_first_page_rgba(&path, 400, &AtomicBool::new(false)).is_err());
    }
}
