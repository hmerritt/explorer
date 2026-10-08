use std::{
    collections::HashMap,
    fs::File,
    io::Write,
    path::Path,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

use gpui::{ScrollDelta, TestAppContext, point, px, rgb};
use zip::{ZipWriter, write::FileOptions};

use super::{book::*, pagination::*, state::*, *};

pub(crate) fn write_book(
    path: &Path,
    version: &str,
    metadata: &str,
    chapters: &[(&str, &str)],
    extra: &[(&str, &str, &[u8])],
) {
    write_book_with_guide(path, version, metadata, chapters, extra, "");
}

pub(crate) fn write_book_with_guide(
    path: &Path,
    version: &str,
    metadata: &str,
    chapters: &[(&str, &str)],
    extra: &[(&str, &str, &[u8])],
    guide: &str,
) {
    let mut zip = ZipWriter::new(File::create(path).unwrap());
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut write = |name: &str, bytes: &[u8]| {
        zip.start_file(name, options).unwrap();
        zip.write_all(bytes).unwrap();
    };
    write("mimetype", b"application/epub+zip");
    write("META-INF/container.xml", br#"<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="book/package.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#);
    let manifest = chapters
        .iter()
        .enumerate()
        .map(|(index, (name, _))| {
            format!(r#"<item id="c{index}" href="{name}" media-type="application/xhtml+xml"/>"#)
        })
        .collect::<String>();
    let resources = extra
        .iter()
        .enumerate()
        .filter(|(_, (name, _, _))| !name.starts_with("META-INF/"))
        .map(|(index, (name, mime, _))| {
            let properties = if *name == "nav.xhtml" {
                " properties=\"nav\""
            } else if version == "3.0" && matches!(*name, "cover.png" | "cover.svg") {
                " properties=\"cover-image\""
            } else {
                ""
            };
            format!(r#"<item id="r{index}" href="{name}" media-type="{mime}"{properties}/>"#)
        })
        .collect::<String>();
    let spine = chapters
        .iter()
        .enumerate()
        .map(|(index, _)| format!(r#"<itemref idref="c{index}"/>"#))
        .collect::<String>();
    let ncx = extra
        .iter()
        .position(|(_, mime, _)| *mime == "application/x-dtbncx+xml")
        .map(|index| format!(" toc=\"r{index}\""))
        .unwrap_or_default();
    let opf = format!(
        r#"<package xmlns="http://www.idpf.org/2007/opf" version="{version}" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">urn:test:reader</dc:identifier><dc:title>Reader fixture</dc:title><dc:language>en</dc:language>{metadata}</metadata><manifest>{manifest}{resources}</manifest><spine{ncx}>{spine}</spine>{guide}</package>"#
    );
    write("book/package.opf", opf.as_bytes());
    for (name, text) in chapters {
        write(&format!("book/{name}"), text.as_bytes());
    }
    for (name, _, bytes) in extra {
        write(
            &if name.starts_with("META-INF/") {
                name.to_string()
            } else {
                format!("book/{name}")
            },
            bytes,
        );
    }
    zip.finish().unwrap();
}

fn chapter(body: &str) -> Chapter {
    parse_chapter(&format!("<html><head><title>Ignored</title><style>Ignored</style></head><body>{body}</body></html>"), "/book/chapter.xhtml", &AtomicBool::new(false)).unwrap()
}

fn all_text(chapter: &Chapter) -> String {
    chapter
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[test]
fn epub2_and_epub3_read_spine_order_and_fallback_chapters_without_reading_all_resources() {
    let dir = tempfile::tempdir().unwrap();
    for version in ["2.0", "3.0"] {
        let path = dir.path().join("book.epub");
        write_book(
            &path,
            version,
            "",
            &[
                ("first.xhtml", "<html><body><p>First</p></body></html>"),
                ("second.xhtml", "<html><body><p>Second</p></body></html>"),
                ("later.xhtml", "this chapter is deliberately not valid XML"),
            ],
            &[],
        );
        let book = Book::open(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(book.sections.len(), 3);
        assert_eq!(book.toc.len(), 3);
        assert_eq!(
            all_text(
                &book
                    .chapter(&book.sections[0].href, &AtomicBool::new(false))
                    .unwrap()
            ),
            "First"
        );
        assert_eq!(
            all_text(
                &book
                    .chapter(&book.sections[1].href, &AtomicBool::new(false))
                    .unwrap()
            ),
            "Second"
        );
    }
}

#[test]
fn fixed_layout_and_drm_are_rejected_but_obfuscated_fonts_are_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.epub");
    let chapters = [("c.xhtml", "<html><body><p>Readable</p></body></html>")];
    write_book(
        &path,
        "3.0",
        r#"<meta property="rendition:layout">pre-paginated</meta>"#,
        &chapters,
        &[],
    );
    assert!(
        Book::open(&path, &AtomicBool::new(false))
            .err()
            .unwrap()
            .contains("Fixed-layout")
    );
    for (algorithm, allowed) in [
        ("http://www.idpf.org/2008/embedding", true),
        ("http://www.w3.org/2001/04/xmlenc#aes128-cbc", false),
    ] {
        let encryption =
            format!(r#"<encryption><EncryptionMethod Algorithm="{algorithm}"/></encryption>"#);
        write_book(
            &path,
            "3.0",
            "",
            &chapters,
            &[(
                "META-INF/encryption.xml",
                "application/xml",
                encryption.as_bytes(),
            )],
        );
        assert_eq!(Book::open(&path, &AtomicBool::new(false)).is_ok(), allowed);
    }
}

#[test]
fn semantic_styles_entities_anchors_and_relative_links_are_preserved() {
    let chapter = chapter(
        "<h2 id='start'>Heading</h2><p>A <strong>bold <em>word</em></strong> &amp; café&nbsp;字 <a href='../notes.xhtml#n'>note</a>.</p><ul><li>One</li><li>Two</li></ul><blockquote>Quote</blockquote><table><tr><td>A</td><td>B</td></tr></table><script>Ignored</script>",
    );
    let text = all_text(&chapter);
    assert!(text.contains("A bold word & café\u{a0}字 note."));
    assert!(text.contains("• One\n\n• Two"));
    assert!(text.contains("A | B"));
    assert!(!text.contains("Ignored"));
    assert_eq!(chapter.anchors["start"], ContentPoint::default());
    let Block::Text { spans, .. } = &chapter.blocks[1] else {
        panic!()
    };
    assert!(
        spans
            .iter()
            .any(|span| span.style.bold && span.style.italic)
    );
    assert!(
        spans
            .iter()
            .any(|span| span.style.link.as_deref() == Some("/notes.xhtml#n"))
    );
}

#[test]
fn images_inline_svg_and_empty_chapters_are_semantic_blocks() {
    let chapter = chapter(
        "<p>Before<img src='images/cover.png' alt='Cover'/>After</p><svg xmlns='http://www.w3.org/2000/svg' width='20' height='10'><rect width='20' height='10'/></svg>",
    );
    assert_eq!(all_text(&chapter), "Before\n\nAfter");
    assert!(
        matches!(&chapter.blocks[1], Block::Image { source: ImageSource::Resource(href), alt } if href == "/book/images/cover.png" && alt == "Cover")
    );
    assert!(
        matches!(&chapter.blocks[3], Block::Image { source: ImageSource::Svg { bytes, base_href }, .. } if bytes.starts_with(b"<svg") && base_href == "/book/chapter.xhtml")
    );
    assert!(self::chapter("<p> </p>").blocks.is_empty());
}

#[test]
fn unusable_table_of_contents_falls_back_to_spine_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.epub");
    write_book(&path, "3.0", "", &[("c.xhtml", "<html><body><p>Readable</p></body></html>")], &[("nav.xhtml", "application/xhtml+xml", br#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><ol><li><a href="missing.xhtml">Broken</a></li></ol></nav></body></html>"#)]);
    let book = Book::open(&path, &AtomicBool::new(false)).unwrap();
    assert_eq!(book.toc.len(), 1);
    assert_eq!(book.toc[0].target.as_deref(), Some("/book/c.xhtml"));
}

#[test]
fn epub_navigation_preserves_hierarchy_and_fragment_targets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.epub");
    let nav = br##"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><h1>Contents</h1><ol><li><a href="c.xhtml">Part</a><ol><li><a href="c.xhtml#middle">Middle</a></li></ol></li></ol></nav></body></html>"##;
    let ncx = br##"<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><head/><docTitle><text>Fixture</text></docTitle><navMap><navPoint id="part" playOrder="1"><navLabel><text>Part</text></navLabel><content src="c.xhtml"/><navPoint id="middle" playOrder="2"><navLabel><text>Middle</text></navLabel><content src="c.xhtml#middle"/></navPoint></navPoint></navMap></ncx>"##;
    for (version, name, mime, bytes) in [
        ("3.0", "nav.xhtml", "application/xhtml+xml", nav.as_slice()),
        ("2.0", "nav.ncx", "application/x-dtbncx+xml", ncx.as_slice()),
    ] {
        write_book(
            &path,
            version,
            "",
            &[(
                "c.xhtml",
                "<html><body><p>Start</p><p id='middle'>Middle</p></body></html>",
            )],
            &[(name, mime, bytes)],
        );
        let book = Book::open(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(book.toc.len(), 2);
        assert_eq!(book.toc[0].title, "Part");
        assert_eq!(book.toc[0].depth, 0);
        assert_eq!(book.toc[1].title, "Middle");
        assert_eq!(book.toc[1].depth, 1);
        assert_eq!(book.toc[1].target.as_deref(), Some("/book/c.xhtml#middle"));
    }
}

#[test]
fn invalid_chapters_and_cancelled_reads_report_errors() {
    assert!(
        parse_chapter(
            "<html><body><p>Unclosed",
            "/c.xhtml",
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert!(parse_chapter("<html/>", "/c.xhtml", &AtomicBool::new(true)).is_err());
    assert_eq!(
        resolve_href("/book/a.xhtml", "../notes/café.xhtml#note"),
        Some("/notes/caf%C3%A9.xhtml#note".into())
    );
    assert!(resolve_href("/book/a.xhtml", "file:///etc/passwd").is_none());
}

#[test]
fn wheel_notches_trackpad_threshold_rate_limit_reversal_and_idle() {
    let mut wheel = WheelPager::default();
    let now = Instant::now();
    let lines = |y| ScrollDelta::Lines(point(0.0, y));
    let pixels = |y| ScrollDelta::Pixels(point(px(0.0), px(y)));
    assert_eq!(wheel.turn(lines(-3.0), 3.0, now), 1);
    assert_eq!(wheel.turn(lines(6.0), 3.0, now), -2);
    assert_eq!(wheel.turn(lines(-1.5), 3.0, now), 0);
    assert_eq!(wheel.turn(lines(-1.5), 3.0, now), 1);
    assert_eq!(wheel.turn(pixels(-40.0), 3.0, now), 0);
    assert_eq!(wheel.turn(pixels(-40.0), 3.0, now), 1);
    assert_eq!(
        wheel.turn(pixels(-240.0), 3.0, now + Duration::from_millis(50)),
        0
    );
    assert_eq!(
        wheel.turn(pixels(-1.0), 3.0, now + Duration::from_millis(150)),
        0
    );
    assert_eq!(
        wheel.turn(pixels(80.0), 3.0, now + Duration::from_millis(151)),
        -1
    );
    assert_eq!(
        wheel.turn(pixels(-40.0), 3.0, now + Duration::from_millis(400)),
        0
    );
    assert_eq!(
        wheel.turn(pixels(-40.0), 3.0, now + Duration::from_millis(601)),
        0
    );
    assert_eq!(
        wheel.turn(pixels(-40.0), 3.0, now + Duration::from_millis(602)),
        1
    );
}

#[test]
fn copying_preserves_paragraph_breaks_and_utf8_and_handles_reverse_selections() {
    let chapter = chapter("<p>café 字</p><p>second</p>");
    let a = ContentPoint {
        block: 0,
        offset: 0,
    };
    let b = ContentPoint {
        block: 1,
        offset: 3,
    };
    assert_eq!(selected_text(&chapter, a, b), "café 字\n\nsec");
    assert_eq!(selected_text(&chapter, b, a), "café 字\n\nsec");
    assert_eq!(
        normalize_point(
            &chapter,
            ContentPoint {
                block: 0,
                offset: 4
            }
        )
        .offset,
        3
    );
}

#[test]
fn saved_state_round_trips_multiple_books_invalidates_changes_and_tolerates_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let stamp = Fingerprint {
        size: 20,
        modified: 123,
        nanos: 4,
    };
    let location = Location {
        href: "/book/c.xhtml".into(),
        point: ContentPoint {
            block: 3,
            offset: 24,
        },
    };
    let mut store = Store::default();
    store.remember("first".into(), stamp.clone(), location.clone());
    store.remember("second".into(), stamp.clone(), Location::default());
    save_store(&path, &store).unwrap();
    save_store(&path, &store).unwrap(); // atomic replacement also works on Windows
    let loaded = Store::load(&path);
    assert_eq!(loaded.resume("first", &stamp), Some(location));
    assert_eq!(loaded.resume("second", &stamp), Some(Location::default()));
    assert!(
        loaded
            .resume("first", &Fingerprint { size: 21, ..stamp })
            .is_none()
    );
    std::fs::write(&path, "{").unwrap();
    assert!(
        Store::load(&path)
            .resume(
                "first",
                &Fingerprint {
                    size: 20,
                    modified: 123,
                    nanos: 4
                }
            )
            .is_none()
    );
}

#[test]
fn epub_launch_detection_handles_debug_flags_uppercase_and_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("café book.EPUB");
    std::fs::write(&path, "fixture").unwrap();
    assert_eq!(
        startup_epub_path([
            "explorer".into(),
            "--debug".into(),
            "timings".into(),
            path.clone().into_os_string()
        ]),
        Some(path.clone())
    );
    assert!(!epub_existing_file(dir.path()));
    assert!(!epub_existing_file(&dir.path().join("missing.epub")));
}

#[gpui::test]
fn measured_pagination_preserves_text_and_reflows_at_content_offsets(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let chapter = chapter(&format!(
        "<h1>Heading</h1><p>{}</p><p>End</p>",
        "café 字 long paragraph with emphasized text. ".repeat(700)
    ));
    let mut cursor = ContentPoint::default();
    let mut copied = vec![String::new(); chapter.blocks.len()];
    let mut starts = Vec::new();
    cx.update(|window, _| {
        for _ in 0..2000 {
            starts.push(cursor);
            let page = layout_page(
                &chapter,
                cursor,
                PageGeometry::new(360.0, 280.0),
                "Arial",
                20.0,
                rgb(0x202020).into(),
                rgb(0x0759b5).into(),
                &HashMap::new(),
                window,
            )
            .unwrap();
            for item in &page.items {
                if let PageItem::Text { block, range, .. } = item {
                    if let Block::Text { text, .. } = &chapter.blocks[*block] {
                        copied[*block].push_str(&text[range.clone()]);
                    }
                }
            }
            if page.end.block >= chapter.blocks.len() {
                break;
            }
            assert!(page.end > cursor);
            cursor = page.end;
        }
        assert!(starts.len() > 10);
        for (index, block) in chapter.blocks.iter().enumerate() {
            if let Block::Text { text, .. } = block {
                assert_eq!(copied[index], *text);
            }
        }
        let anchor = starts[5];
        let page = layout_page(
            &chapter,
            anchor,
            PageGeometry::new(1200.0, 900.0),
            "Arial",
            28.0,
            rgb(0x202020).into(),
            rgb(0x0759b5).into(),
            &HashMap::new(),
            window,
        )
        .unwrap();
        assert_eq!(page.start, anchor);
        assert!(page.end > anchor);
    });
}

#[test]
fn margins_and_image_fit_use_the_available_page_without_upscaling() {
    let geometry = PageGeometry::new(1024.0, 750.0);
    assert_eq!(geometry.width, 900.0);
    assert_eq!(geometry.margin_x, 62.0);
    assert_eq!(geometry.height, 686.0);
    assert_eq!(fit_image(2000.0, 1000.0, geometry), (900.0, 450.0));
    assert_eq!(fit_image(20.0, 10.0, geometry), (20.0, 10.0));
    let small = PageGeometry::new(360.0, 200.0);
    assert_eq!(small.margin_x, 16.0);
    assert_eq!(small.margin_y, 16.0);
}

#[test]
fn reading_column_caps_at_900_and_keeps_responsive_padding() {
    for (width, content, margin) in [
        (360.0, 328.0, 16.0),
        (400.0, 304.0, 48.0),
        (995.0, 899.0, 48.0),
        (996.0, 900.0, 48.0),
        (1600.0, 900.0, 350.0),
    ] {
        let geometry = PageGeometry::new(width, 750.0);
        assert_eq!(geometry.width, content);
        assert_eq!(geometry.margin_x, margin);
        assert_eq!(geometry.height, 686.0);
    }
    assert_eq!(PageGeometry::new(1.0, 1.0).width, 1.0);
    assert_eq!(PageGeometry::new(1.0, 1.0).height, 1.0);
}

#[gpui::test]
fn justification_expands_word_spaces_without_mutating_cached_shapes(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    cx.update(|window, _| {
        let text = "café bold 字 end";
        let run = gpui::TextRun {
            len: text.len(),
            font: gpui::font("Arial"),
            color: rgb(0x202020).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let natural = window
            .text_system()
            .shape_line(text.into(), px(20.0), &[run.clone()], None);
        let mut justified = natural.clone();
        justify_line(&mut justified, f32::from(natural.width) + 120.0);
        assert_eq!(justified.width, natural.width + px(120.0));
        assert_eq!(justified.text, natural.text);
        for (word, expansion) in [("bold", 40.0), ("字", 80.0), ("end", 120.0)] {
            let index = text.find(word).unwrap();
            let x = justified.x_for_index(index);
            assert!((f32::from(x - natural.x_for_index(index)) - expansion).abs() < 0.01);
            assert_eq!(justified.index_for_x(x + px(0.01)), Some(index));
            assert_eq!(justified.closest_index_for_x(x), index);
        }
        let cached = window
            .text_system()
            .shape_line(text.into(), px(20.0), &[run], None);
        assert_eq!(cached.width, natural.width);
        assert_eq!(
            cached.x_for_index(text.find("end").unwrap()),
            natural.x_for_index(text.find("end").unwrap())
        );
        for text in ["singleword", "no\u{a0}break", "  word  "] {
            let run = gpui::TextRun {
                len: text.len(),
                font: gpui::font("Arial"),
                color: rgb(0x202020).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let mut line = window
                .text_system()
                .shape_line(text.into(), px(20.0), &[run], None);
            let width = line.width;
            justify_line(&mut line, 900.0);
            assert_eq!(line.width, width);
        }
    });
}

#[gpui::test]
fn body_justification_respects_paragraphs_breaks_indents_and_shaping_chunks(
    cx: &mut TestAppContext,
) {
    let cx = cx.add_empty_window();
    let chapter = chapter(&format!(
        "<h1>Natural heading</h1><pre>Natural preformatted line</pre><p>{}<br/>Explicit break<br/>{}</p><blockquote>{}</blockquote><ul><li>{}</li></ul><p>Short final paragraph</p>",
        "café <b>bold</b> <a href='notes.xhtml'>linked words</a> 字 prose. ".repeat(700),
        "Continued prose after a break. ".repeat(20),
        "Indented quoted prose. ".repeat(20),
        "Indented list prose. ".repeat(20),
    ));
    let geometry = PageGeometry::new(1200.0, 900.0);
    let mut cursor = ContentPoint::default();
    let mut justified_count = 0;
    let mut natural_count = 0;
    let mut seen = vec![String::new(); chapter.blocks.len()];
    cx.update(|window, _| {
        for _ in 0..1000 {
            let page = layout_page(
                &chapter,
                cursor,
                geometry,
                "Arial",
                20.0,
                rgb(0x202020).into(),
                rgb(0x0759b5).into(),
                &HashMap::new(),
                window,
            )
            .unwrap();
            for item in &page.items {
                if let PageItem::Text {
                    block,
                    range,
                    line,
                    x,
                    ..
                } = item
                {
                    let Block::Text { text, kind, .. } = &chapter.blocks[*block] else {
                        panic!()
                    };
                    seen[*block].push_str(&text[range.clone()]);
                    let continuation =
                        range.end < text.len() && !text[range.end..].starts_with('\n');
                    if kind.heading == 0
                        && !kind.pre
                        && continuation
                        && line.text.trim().contains(' ')
                    {
                        assert!((f32::from(line.width) - (geometry.width - x)).abs() < 0.01);
                        justified_count += 1;
                    } else {
                        assert!(f32::from(line.width) < geometry.width - x);
                        natural_count += 1;
                    }
                }
            }
            if page.end.block == chapter.blocks.len() {
                break;
            }
            assert!(page.end > cursor);
            cursor = page.end;
        }
    });
    assert!(justified_count > 100);
    assert!(natural_count >= 8);
    for (index, block) in chapter.blocks.iter().enumerate() {
        if let Block::Text { text, .. } = block {
            // Explicit breaks have no glyphs; copying still uses the source.
            assert_eq!(seen[index], text.replace('\n', ""));
        }
    }
}

pub(crate) const COVER_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="600"><rect width="400" height="600" fill="red"/></svg>"#;

pub(crate) fn cover_png() -> Vec<u8> {
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        400,
        600,
        image::Rgb([200, 40, 40]),
    ));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

#[test]
fn declared_epub2_and_epub3_images_add_stable_cover_locations() {
    let dir = tempfile::tempdir().unwrap();
    for version in ["2.0", "3.0"] {
        for (name, mime, bytes) in [
            ("cover.png", "image/png", cover_png()),
            ("cover.svg", "image/svg+xml", COVER_SVG.to_vec()),
        ] {
            let path = dir.path().join("book.epub");
            let metadata = if version == "2.0" {
                "<meta name='cover' content='r0'/>"
            } else {
                ""
            };
            write_book(
                &path,
                version,
                metadata,
                &[
                    ("title.xhtml", "<html><body><p>Title page</p></body></html>"),
                    ("text.xhtml", "<html><body><p>Main text</p></body></html>"),
                ],
                &[(name, mime, &bytes)],
            );
            let book = Book::open(&path, &AtomicBool::new(false)).unwrap();
            assert_eq!(book.sections.len(), 3);
            let href = book.cover_href.as_ref().unwrap();
            assert_eq!(href, "explorer:epub-cover");
            assert_eq!(book.sections[0].href, *href);
            assert_eq!(book.toc[0].title, "Cover");
            assert!(book.readable_location(href));
            let chapter = book.chapter(href, &AtomicBool::new(false)).unwrap();
            assert!(chapter.is_cover);
            assert!(matches!(
                chapter.blocks.as_slice(),
                [Block::Image {
                    source: ImageSource::Resource(_),
                    ..
                }]
            ));
            assert_eq!(
                Book::open(&path, &AtomicBool::new(false))
                    .unwrap()
                    .cover_href,
                book.cover_href
            );
        }
    }
}

#[test]
fn cover_documents_use_guides_and_landmarks_without_duplicate_opening_images() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.epub");
    for version in ["2.0", "3.0"] {
        let guide = if version == "2.0" {
            "<guide><reference type='cover' title='Cover' href='cover.xhtml'/></guide>"
        } else {
            ""
        };
        let nav = br#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="landmarks"><ol><li><a epub:type="cover" href="cover.xhtml">Cover</a></li></ol></nav></body></html>"#;
        write_book_with_guide(
            &path,
            version,
            if version == "2.0" {
                "<meta name='cover' content='r0'/>"
            } else {
                ""
            },
            &[
                (
                    "opening.xhtml",
                    "<html><body><img src='cover.svg'/></body></html>",
                ),
                ("text.xhtml", "<html><body><p>Main text</p></body></html>"),
            ],
            &[
                ("cover.svg", "image/svg+xml", COVER_SVG),
                (
                    "cover.xhtml",
                    "application/xhtml+xml",
                    b"<html><body><img src='cover.svg'/></body></html>",
                ),
                ("nav.xhtml", "application/xhtml+xml", nav),
            ],
            guide,
        );
        let book = Book::open(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(book.cover_href.as_deref(), Some("/book/cover.xhtml"));
        assert_eq!(book.sections.len(), 2);
        assert_eq!(book.sections[1].href, "/book/text.xhtml");
        assert_eq!(
            book.reading_href("/book/opening.xhtml"),
            "/book/cover.xhtml"
        );
        assert_eq!(
            book.toc.iter().filter(|item| item.title == "Cover").count(),
            1
        );
    }
    write_book(
        &path,
        "3.0",
        "",
        &[
            (
                "opening.xhtml",
                "<html><body><img src='cover.svg'/></body></html>",
            ),
            ("text.xhtml", "<html><body><p>Main text</p></body></html>"),
        ],
        &[("cover.svg", "image/svg+xml", COVER_SVG)],
    );
    let book = Book::open(&path, &AtomicBool::new(false)).unwrap();
    assert_eq!(book.cover_href.as_deref(), Some("/book/opening.xhtml"));
    assert_eq!(book.sections.len(), 2);
}

#[test]
fn absent_or_unreadable_cover_documents_leave_main_text_readable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.epub");
    for guide in [
        "",
        "<guide><reference type='cover' title='Cover' href='missing.xhtml'/></guide>",
        "<guide><reference type='cover' title='Cover' href='broken.xhtml'/></guide>",
    ] {
        write_book_with_guide(
            &path,
            "2.0",
            "",
            &[
                ("broken.xhtml", "<html><body><p>"),
                ("text.xhtml", "<html><body><p>Main text</p></body></html>"),
            ],
            &[],
            guide,
        );
        let book = Book::open(&path, &AtomicBool::new(false)).unwrap();
        assert!(book.cover_href.is_none());
        assert_eq!(
            all_text(
                &book
                    .chapter("/book/text.xhtml", &AtomicBool::new(false))
                    .unwrap()
            ),
            "Main text"
        );
        if guide.contains("broken.xhtml") {
            assert_eq!(book.sections.len(), 1);
        }
    }
}
