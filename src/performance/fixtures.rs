use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

pub const FIXTURE_VERSION: &str = "performance-fixtures-v1";

#[derive(Clone, Debug)]
pub struct Fixtures {
    pub root: PathBuf,
}

impl Fixtures {
    pub fn mixed(&self, count: usize) -> PathBuf {
        self.root.join(format!("mixed-{count}"))
    }
    pub fn media(&self, kind: &str) -> PathBuf {
        self.root.join(kind)
    }

    pub fn ensure(parent: &Path, video: bool) -> Result<Self, String> {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let root = parent.join(FIXTURE_VERSION);
        if !root.exists() {
            let staging = tempfile::tempdir_in(parent).map_err(|e| e.to_string())?;
            create(staging.path()).map_err(|e| e.to_string())?;
            fs::write(staging.path().join(".complete"), FIXTURE_VERSION)
                .map_err(|e| e.to_string())?;
            match fs::rename(staging.path(), &root) {
                Ok(()) => {}
                Err(_) if root.join(".complete").exists() => {}
                Err(error) => return Err(format!("publish fixtures: {error}")),
            }
        }
        let fixture = Self {
            root: canonical_fixture_path(&root)?,
        };
        fixture.validate()?;
        if video && !fixture.root.join("video/.complete").exists() {
            let staging = tempfile::tempdir_in(parent).map_err(|e| e.to_string())?;
            let video_path = staging.path().join("video.mp4");
            let output = Command::new("ffmpeg")
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-nostdin",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=640x360:rate=15",
                    "-t",
                    "2",
                    "-c:v",
                    "mpeg4",
                    "-threads",
                    "1",
                    "-y",
                ])
                .arg(&video_path)
                .output()
                .map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err(format!(
                    "generate video fixture: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            let video_root = fixture.root.join("video");
            fs::create_dir_all(&video_root).map_err(|e| e.to_string())?;
            fs::copy(&video_path, video_root.join("video.mp4")).map_err(|e| e.to_string())?;
            let folder = fixture.root.join("video_thumbnails");
            fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
            for i in 0..12 {
                fs::copy(&video_path, folder.join(format!("video-{i:02}.mp4")))
                    .map_err(|e| e.to_string())?;
            }
            fs::write(video_root.join(".complete"), FIXTURE_VERSION).map_err(|e| e.to_string())?;
        }
        Ok(fixture)
    }

    pub fn validate(&self) -> Result<(), String> {
        if fs::read_to_string(self.root.join(".complete")).map_err(|e| e.to_string())?
            != FIXTURE_VERSION
        {
            return Err("fixture version mismatch; remove only the benchmark fixture directory and regenerate".into());
        }
        for count in [0, 100, 1_000, 10_000] {
            let directory = self.mixed(count);
            let actual = fs::read_dir(&directory)
                .map_err(|e| e.to_string())?
                .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
                .collect::<Result<std::collections::BTreeSet<_>, _>>()
                .map_err(|e| e.to_string())?;
            let expected = count + usize::from(count > 0);
            if actual.len() != expected {
                return Err(format!(
                    "fixture mixed-{count}: expected {expected} filesystem entries, found {}",
                    actual.len()
                ));
            }
            if count > 0 {
                let expected_names = ["child".to_string(), ".hidden.txt".to_string()]
                    .into_iter()
                    .chain((1..count).map(mixed_name))
                    .collect::<std::collections::BTreeSet<_>>();
                if actual != expected_names {
                    return Err(format!("fixture mixed-{count}: names have changed"));
                }
                for i in 1..count {
                    let metadata =
                        fs::metadata(directory.join(mixed_name(i))).map_err(|e| e.to_string())?;
                    if metadata.is_dir() != (i % 17 == 0)
                        || i % 17 != 0 && metadata.len() != (32 + i % 2048) as u64
                    {
                        return Err(format!(
                            "fixture mixed-{count}: type/size changed for entry {i}"
                        ));
                    }
                    if filetime::FileTime::from_last_modification_time(&metadata).unix_seconds()
                        != 1_700_000_000 + (i % 173) as i64
                    {
                        return Err(format!(
                            "fixture mixed-{count}: modification time changed for entry {i}"
                        ));
                    }
                }
                if fs::read_dir(directory.join("child"))
                    .map_err(|e| e.to_string())?
                    .count()
                    != 24
                {
                    return Err(format!("fixture mixed-{count}: nested search tree changed"));
                }
            }
        }
        for (folder, name) in [
            ("image", "image.png"),
            ("viewer", "photo.jpg"),
            ("text", "notes.txt"),
            ("pdf", "document.pdf"),
            ("epub", "book.epub"),
        ] {
            if fs::metadata(self.media(folder).join(name))
                .map_err(|e| e.to_string())?
                .len()
                == 0
            {
                return Err(format!("empty media fixture: {folder}/{name}"));
            }
        }
        Ok(())
    }
}

fn canonical_fixture_path(path: &Path) -> Result<PathBuf, String> {
    let path = fs::canonicalize(path).map_err(|e| e.to_string())?;
    // Settings intentionally serialize configured paths using forward slashes.
    // Windows verbatim prefixes cannot undergo that spelling change. Keep the
    // canonical location, using the ordinary disk/UNC prefix for these fixtures.
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
        if units.starts_with(&[92, 92, 63, 92, 85, 78, 67, 92]) {
            let mut ordinary = vec![92, 92];
            ordinary.extend_from_slice(&units[8..]);
            return Ok(PathBuf::from(std::ffi::OsString::from_wide(&ordinary)));
        }
        if units.starts_with(&[92, 92, 63, 92]) && units.get(5) == Some(&58) {
            return Ok(PathBuf::from(std::ffi::OsString::from_wide(&units[4..])));
        }
    }
    Ok(path)
}

fn create(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for count in [0, 100, 1_000, 10_000] {
        let folder = root.join(format!("mixed-{count}"));
        fs::create_dir(&folder)?;
        if count == 0 {
            continue;
        }
        let child = folder.join("child");
        fs::create_dir(&child)?;
        for i in 0..24 {
            fs::write(
                child.join(format!(
                    "{}-{i:03}.txt",
                    if i % 8 == 0 { "needle" } else { "nested" }
                )),
                b"nested fixture\n",
            )?;
        }
        for i in 1..count {
            let name = mixed_name(i);
            let path = folder.join(name);
            if i % 17 == 0 {
                fs::create_dir(&path)?;
            } else {
                fs::write(&path, vec![(i % 251) as u8; 32 + i % 2_048])?;
            }
            filetime::set_file_mtime(
                &path,
                filetime::FileTime::from_unix_time(1_700_000_000 + (i % 173) as i64, 0),
            )?;
        }
        fs::write(folder.join(".hidden.txt"), b"hidden fixture")?;
    }
    for folder in ["image", "viewer", "text", "pdf", "epub", "image_thumbnails"] {
        fs::create_dir(root.join(folder))?;
    }
    let rgba = image::RgbaImage::from_fn(1600, 1200, |x, y| {
        image::Rgba([
            (x % 251) as u8,
            (y % 253) as u8,
            ((x / 3 + y / 5) % 255) as u8,
            255,
        ])
    });
    rgba.save(root.join("image/image.png"))?;
    let photo = image::RgbImage::from_fn(4000, 3000, |x, y| {
        image::Rgb([
            (x % 251) as u8,
            (y % 253) as u8,
            ((x / 3 + y / 5) % 255) as u8,
        ])
    });
    photo.save(root.join("viewer/photo.jpg"))?;
    let thumbnail = image::imageops::resize(&rgba, 800, 600, image::imageops::FilterType::Triangle);
    for i in 0..12 {
        thumbnail.save(root.join(format!("image_thumbnails/image-{i:02}.png")))?;
    }
    fs::write(
        root.join("text/notes.txt"),
        (0..120)
            .map(|i| format!("Line {i}: deterministic Explorer preview fixture.\n"))
            .collect::<String>(),
    )?;
    let mut pdf = pdf_oxide::writer::PdfWriter::new();
    let mut page = pdf.add_page(400., 200.);
    page.fill_rect_colored(0., 0., 400., 200., 1., 0., 0.);
    page.finish();
    fs::write(root.join("pdf/document.pdf"), pdf.finish()?)?;
    let mut book = zip::ZipWriter::new(fs::File::create(root.join("epub/book.epub"))?);
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in [
        ("mimetype", b"application/epub+zip".as_slice()),
        ("META-INF/container.xml", br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/package.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#.as_slice()),
        ("OEBPS/package.opf", br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">urn:explorer:benchmark</dc:identifier><dc:title>Benchmark</dc:title><dc:language>en</dc:language></metadata><manifest><item id="cover" href="cover.png" media-type="image/png" properties="cover-image"/><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="chapter"/></spine></package>"#.as_slice()),
        ("OEBPS/chapter.xhtml", br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Benchmark</title></head><body><p>Benchmark chapter</p></body></html>"#.as_slice()),
    ] { book.start_file(name, options)?; book.write_all(bytes)?; }
    book.start_file("OEBPS/cover.png", options)?;
    book.write_all(&fs::read(root.join("image/image.png"))?)?;
    book.finish()?;
    Ok(())
}

pub fn mixed_name(i: usize) -> String {
    let prefix = if i % 10 == 0 {
        "needle"
    } else if i % 7 == 0 {
        "résumé-文件"
    } else {
        "item"
    };
    let suffix = if i % 31 == 0 {
        "-a-long-file-name-with-several-words"
    } else {
        ""
    };
    format!(
        "{prefix}-{i}{suffix}.{}",
        ["txt", "dat", "rs", "json"][i % 4]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_names_have_numeric_unicode_long_and_query_variants() {
        assert!(mixed_name(10).contains("needle"));
        assert!(mixed_name(7).contains("文件"));
        assert!(mixed_name(31).len() > 40);
        assert_ne!(mixed_name(1), mixed_name(2));
    }

    #[test]
    fn canonical_fixture_paths_survive_settings_serialization() {
        let temp = tempfile::tempdir().unwrap();
        let root = canonical_fixture_path(temp.path()).unwrap();
        let mut settings = crate::settings::ExplorerSettings::default();
        settings.app.start = root.clone();
        let parsed: crate::settings::ExplorerSettings =
            serde_json::from_value(serde_json::to_value(settings).unwrap()).unwrap();
        assert_eq!(parsed.app.start, root);
        assert!(parsed.app.start.is_dir());
    }
}
