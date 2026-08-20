use std::{
    fs::File,
    io::{self, BufRead, BufReader, Cursor, Read},
    path::{Path, PathBuf},
    time::SystemTime,
};

use gpui::{Context, Task};

use crate::explorer::{
    entry::FileEntry,
    image_thumbnails::{
        entry_may_have_hover_image_preview, entry_may_have_hover_pdf_preview,
        entry_may_have_hover_video_preview,
    },
    view::ExplorerView,
};

pub(super) const TEXT_HOVER_PREVIEW_PADDING: f32 = 12.0;
pub(super) const TEXT_HOVER_PREVIEW_TEXT_SIZE: f32 = 12.0;
pub(super) const TEXT_HOVER_PREVIEW_LINE_HEIGHT: f32 = 18.0;
const TEXT_HOVER_PREVIEW_MAX_LINE_BYTES: usize = 8 * 1024;
const TEXT_HOVER_PREVIEW_MAX_TOTAL_BYTES: usize = 64 * 1024;

const TEXT_PREVIEW_EXTENSIONS: &[&str] = &[
    "txt",
    "md",
    "markdown",
    "log",
    "json",
    "jsonl",
    "ndjson",
    "toml",
    "yaml",
    "yml",
    "csv",
    "tsv",
    "xml",
    "ini",
    "cfg",
    "conf",
    "env",
    "properties",
    "lock",
    "rs",
    "go",
    "py",
    "pyi",
    "js",
    "mjs",
    "cjs",
    "jsx",
    "ts",
    "mts",
    "cts",
    "tsx",
    "c",
    "h",
    "cc",
    "cpp",
    "cxx",
    "hpp",
    "hxx",
    "cs",
    "java",
    "kt",
    "kts",
    "swift",
    "rb",
    "php",
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "bat",
    "cmd",
    "sql",
    "html",
    "htm",
    "css",
    "scss",
    "sass",
    "less",
    "vue",
    "svelte",
];

const TEXT_PREVIEW_FILE_NAMES: &[&str] = &[
    "readme",
    "license",
    "changelog",
    "makefile",
    "dockerfile",
    "procfile",
    "cmakelists.txt",
    ".gitignore",
    ".gitattributes",
    ".gitmodules",
    ".editorconfig",
    ".npmrc",
    ".yarnrc",
    ".prettierrc",
    ".eslintrc",
    ".stylelintrc",
    ".dockerignore",
    ".env",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HoverPreviewKind {
    Image,
    Pdf,
    Video,
    Text,
}

pub(super) fn hover_preview_kind(entry: &FileEntry) -> Option<HoverPreviewKind> {
    if entry_may_have_hover_video_preview(entry) {
        return Some(HoverPreviewKind::Video);
    }
    if entry_may_have_hover_image_preview(entry) {
        return Some(HoverPreviewKind::Image);
    }
    if entry_may_have_hover_pdf_preview(entry) {
        return Some(HoverPreviewKind::Pdf);
    }
    entry_may_have_hover_text_preview(entry).then_some(HoverPreviewKind::Text)
}

pub(super) fn entry_may_have_hover_text_preview(entry: &FileEntry) -> bool {
    !entry.is_directory_like() && path_may_have_text_preview(&entry.path)
}

fn path_may_have_text_preview(path: &Path) -> bool {
    let extension_matches = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            let extension = extension.to_ascii_lowercase();
            TEXT_PREVIEW_EXTENSIONS.contains(&extension.as_str())
        });
    if extension_matches {
        return true;
    }

    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let file_name = file_name.to_ascii_lowercase();
    TEXT_PREVIEW_FILE_NAMES.contains(&file_name.as_str())
        || file_name.starts_with(".env.")
        || text_family_file_name(&file_name, "readme")
        || text_family_file_name(&file_name, "license")
        || text_family_file_name(&file_name, "changelog")
}

fn text_family_file_name(file_name: &str, family: &str) -> bool {
    file_name
        .strip_prefix(family)
        .is_some_and(|suffix| matches!(suffix.as_bytes().first(), Some(b'-' | b'_')))
}

pub(super) fn text_hover_preview_line_limit(rendered_height: f32) -> usize {
    ((rendered_height - (TEXT_HOVER_PREVIEW_PADDING * 2.0)).max(0.0)
        / TEXT_HOVER_PREVIEW_LINE_HEIGHT)
        .floor() as usize
}

pub(super) struct TextHoverPreviewSession {
    path: PathBuf,
    size: Option<u64>,
    modified: Option<SystemTime>,
    line_limit: usize,
    generation: u64,
    task: Option<Task<()>>,
    content: Option<TextHoverPreviewContent>,
    failed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TextHoverPreviewLine {
    pub(super) text: String,
    pub(super) truncated: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct TextHoverPreviewContent {
    pub(super) lines: Vec<TextHoverPreviewLine>,
    pub(super) truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum TextHoverPreviewLookup {
    Loading,
    Ready(TextHoverPreviewContent),
    Failed,
}

impl ExplorerView {
    pub(super) fn hover_text_preview_for_entry(
        &mut self,
        entry: &FileEntry,
        line_limit: usize,
        cx: &mut Context<Self>,
    ) -> Option<TextHoverPreviewLookup> {
        if !entry_may_have_hover_text_preview(entry) {
            return None;
        }

        if self.text_hover_preview.as_ref().is_none_or(|session| {
            session.path != entry.path
                || session.size != entry.size
                || session.modified != entry.modified
                || session.line_limit != line_limit
        }) {
            self.start_text_hover_preview(entry, line_limit, cx);
        }

        let session = self.text_hover_preview.as_ref()?;
        if session.failed {
            return Some(TextHoverPreviewLookup::Failed);
        }
        if let Some(content) = session.content.clone() {
            return Some(TextHoverPreviewLookup::Ready(content));
        }
        Some(TextHoverPreviewLookup::Loading)
    }

    fn start_text_hover_preview(
        &mut self,
        entry: &FileEntry,
        line_limit: usize,
        cx: &mut Context<Self>,
    ) {
        self.cancel_text_hover_preview();
        self.text_hover_preview_generation = self.text_hover_preview_generation.wrapping_add(1);
        let generation = self.text_hover_preview_generation;
        let path = entry.path.clone();
        let task = start_text_hover_preview_task(path.clone(), generation, line_limit, cx);

        self.text_hover_preview = Some(TextHoverPreviewSession {
            path,
            size: entry.size,
            modified: entry.modified,
            line_limit,
            generation,
            task: Some(task),
            content: None,
            failed: false,
        });
    }

    pub(super) fn cancel_text_hover_preview(&mut self) -> bool {
        let Some(mut session) = self.text_hover_preview.take() else {
            return false;
        };
        drop(session.task.take());
        true
    }

    fn text_hover_preview_matches(&self, path: &Path, generation: u64) -> bool {
        self.text_hover_preview
            .as_ref()
            .is_some_and(|session| session.path == path && session.generation == generation)
    }

    #[cfg(test)]
    pub(super) fn text_hover_preview_content_for_test(&self) -> Option<&TextHoverPreviewContent> {
        self.text_hover_preview
            .as_ref()
            .and_then(|session| session.content.as_ref())
    }

    #[cfg(test)]
    pub(super) fn text_hover_preview_path_for_test(&self) -> Option<&Path> {
        self.text_hover_preview
            .as_ref()
            .map(|session| session.path.as_path())
    }
}

fn start_text_hover_preview_task(
    path: PathBuf,
    generation: u64,
    line_limit: usize,
    cx: &mut Context<ExplorerView>,
) -> Task<()> {
    cx.spawn(async move |view, cx| {
        let result = cx
            .background_executor()
            .spawn({
                let path = path.clone();
                async move { read_text_hover_preview(&path, line_limit) }
            })
            .await;

        let _ = view.update(cx, |view, cx| {
            if !view.text_hover_preview_matches(&path, generation) {
                return;
            }
            if let Some(session) = view.text_hover_preview.as_mut() {
                match result {
                    Ok(content) => session.content = Some(content),
                    Err(_) => session.failed = true,
                }
                session.task = None;
            }
            cx.notify();
        });
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TextEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}

fn read_text_hover_preview(path: &Path, line_limit: usize) -> io::Result<TextHoverPreviewContent> {
    if line_limit == 0 {
        return Ok(TextHoverPreviewContent::default());
    }

    let mut file = File::open(path)?;
    let mut prefix = [0_u8; 3];
    let mut prefix_len = 0;
    while prefix_len < prefix.len() {
        let read = file.read(&mut prefix[prefix_len..])?;
        if read == 0 {
            break;
        }
        prefix_len += read;
    }
    let prefix = &prefix[..prefix_len];
    let (encoding, content_prefix) = if prefix.starts_with(&[0xef, 0xbb, 0xbf]) {
        (TextEncoding::Utf8, &prefix[3..])
    } else if prefix.starts_with(&[0xff, 0xfe]) {
        (TextEncoding::Utf16Le, &prefix[2..])
    } else if prefix.starts_with(&[0xfe, 0xff]) {
        (TextEncoding::Utf16Be, &prefix[2..])
    } else {
        (TextEncoding::Utf8, prefix)
    };

    let reader = BufReader::new(Cursor::new(content_prefix.to_vec()).chain(file));
    match encoding {
        TextEncoding::Utf8 => read_utf8_preview(reader, line_limit),
        TextEncoding::Utf16Le => read_utf16_preview(reader, line_limit, u16::from_le_bytes),
        TextEncoding::Utf16Be => read_utf16_preview(reader, line_limit, u16::from_be_bytes),
    }
}

fn read_utf8_preview(
    mut reader: impl BufRead,
    line_limit: usize,
) -> io::Result<TextHoverPreviewContent> {
    let mut content = TextHoverPreviewContent::default();
    let mut total_bytes = 0_usize;

    while content.lines.len() < line_limit {
        let mut line = Vec::new();
        let mut reached_eof = false;
        let mut truncated = false;

        loop {
            let buffer = reader.fill_buf()?;
            if buffer.is_empty() {
                reached_eof = true;
                break;
            }

            let newline = buffer.iter().position(|byte| *byte == b'\n');
            let before_newline = newline.unwrap_or(buffer.len());
            let line_remaining = TEXT_HOVER_PREVIEW_MAX_LINE_BYTES.saturating_sub(line.len());
            let total_remaining = TEXT_HOVER_PREVIEW_MAX_TOTAL_BYTES.saturating_sub(total_bytes);
            let take = before_newline.min(line_remaining).min(total_remaining);
            line.extend_from_slice(&buffer[..take]);
            reader.consume(take);
            total_bytes += take;

            if take < before_newline {
                truncated = true;
                break;
            }
            if let Some(newline) = newline {
                reader.consume(1);
                total_bytes = total_bytes.saturating_add(1);
                let _ = newline;
                break;
            }
            if line.len() == TEXT_HOVER_PREVIEW_MAX_LINE_BYTES
                || total_bytes >= TEXT_HOVER_PREVIEW_MAX_TOTAL_BYTES
            {
                truncated = true;
                break;
            }
        }

        if reached_eof && line.is_empty() {
            break;
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let mut text = String::from_utf8_lossy(&line).into_owned();
        text = text.replace('\t', "    ");
        if truncated {
            text.push('…');
        }
        content.lines.push(TextHoverPreviewLine { text, truncated });
        if truncated {
            content.truncated = true;
            break;
        }
        if reached_eof {
            break;
        }
    }

    Ok(content)
}

fn read_utf16_preview(
    mut reader: impl Read,
    line_limit: usize,
    decode_unit: fn([u8; 2]) -> u16,
) -> io::Result<TextHoverPreviewContent> {
    let mut content = TextHoverPreviewContent::default();
    let mut total_bytes = 0_usize;

    while content.lines.len() < line_limit {
        let mut line = Vec::new();
        let mut reached_eof = false;
        let mut truncated = false;

        loop {
            if line.len() * 2 >= TEXT_HOVER_PREVIEW_MAX_LINE_BYTES
                || total_bytes + 2 > TEXT_HOVER_PREVIEW_MAX_TOTAL_BYTES
            {
                truncated = true;
                break;
            }
            let mut pair = [0_u8; 2];
            match reader.read_exact(&mut pair) {
                Ok(()) => total_bytes += 2,
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    reached_eof = true;
                    break;
                }
                Err(error) => return Err(error),
            }
            let unit = decode_unit(pair);
            if unit == u16::from(b'\n') {
                break;
            }
            line.push(unit);
        }

        if reached_eof && line.is_empty() {
            break;
        }
        if line.last() == Some(&u16::from(b'\r')) {
            line.pop();
        }
        let mut text: String = char::decode_utf16(line)
            .map(|character| character.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect();
        text = text.replace('\t', "    ");
        if truncated {
            text.push('…');
        }
        content.lines.push(TextHoverPreviewLine { text, truncated });
        if truncated {
            content.truncated = true;
            break;
        }
        if reached_eof {
            break;
        }
    }

    Ok(content)
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write};

    use super::*;
    use crate::explorer::entry::FileEntry;
    use crate::explorer::test_support::TempDir;

    fn preview_entry(name: &str) -> FileEntry {
        FileEntry::test(name, false, Some(1), None)
    }

    #[test]
    fn text_preview_classification_supports_extensions_and_special_names() {
        for name in [
            "notes.TXT",
            "README",
            "LICENSE-MIT",
            "Dockerfile",
            ".gitignore",
            ".env.production",
            "main.rs",
            "page.tsx",
            "settings.yaml",
        ] {
            assert!(
                entry_may_have_hover_text_preview(&preview_entry(name)),
                "{name}"
            );
        }
        assert!(!entry_may_have_hover_text_preview(&preview_entry(
            "archive.zip"
        )));
        assert!(!entry_may_have_hover_text_preview(&FileEntry::test(
            "notes.txt",
            true,
            None,
            None,
        )));
    }

    #[test]
    fn hover_preview_kind_keeps_media_ahead_of_text() {
        assert_eq!(
            hover_preview_kind(&preview_entry("photo.svg")),
            Some(HoverPreviewKind::Image)
        );
        assert_eq!(
            hover_preview_kind(&preview_entry("movie.mp4")),
            Some(HoverPreviewKind::Video)
        );
        assert_eq!(
            hover_preview_kind(&preview_entry("document.PDF")),
            Some(HoverPreviewKind::Pdf)
        );
        assert_eq!(
            hover_preview_kind(&preview_entry("notes.md")),
            Some(HoverPreviewKind::Text)
        );
    }

    #[test]
    fn line_limit_uses_rendered_height_and_vertical_padding() {
        assert_eq!(text_hover_preview_line_limit(400.0), 20);
        assert_eq!(text_hover_preview_line_limit(240.0), 12);
        assert_eq!(text_hover_preview_line_limit(23.0), 0);
    }

    #[test]
    fn reader_limits_lines_strips_crlf_and_expands_tabs() {
        let temp = TempDir::new();
        let path = temp.path().join("notes.txt");
        fs::write(&path, b"one\r\ntwo\tvalue\r\nthree\r\n").unwrap();

        let preview = read_text_hover_preview(&path, 2).unwrap();
        assert_eq!(
            preview.lines,
            vec![
                TextHoverPreviewLine {
                    text: "one".to_owned(),
                    truncated: false,
                },
                TextHoverPreviewLine {
                    text: "two    value".to_owned(),
                    truncated: false,
                },
            ]
        );
        assert!(!preview.truncated);
    }

    #[test]
    fn reader_handles_utf8_bom_and_lossy_invalid_bytes() {
        let temp = TempDir::new();
        let path = temp.path().join("notes.txt");
        fs::write(&path, [0xef, 0xbb, 0xbf, b'o', b'k', b' ', 0xff, b'\n']).unwrap();

        let preview = read_text_hover_preview(&path, 1).unwrap();
        assert_eq!(preview.lines[0].text, "ok �");
    }

    #[test]
    fn reader_handles_utf16_bom_byte_orders() {
        let temp = TempDir::new();
        for (name, bom, encode) in [
            (
                "little.txt",
                [0xff, 0xfe],
                u16::to_le_bytes as fn(u16) -> [u8; 2],
            ),
            ("big.txt", [0xfe, 0xff], u16::to_be_bytes),
        ] {
            let path = temp.path().join(name);
            let mut file = File::create(&path).unwrap();
            file.write_all(&bom).unwrap();
            for unit in "alpha\r\nbeta\n".encode_utf16() {
                file.write_all(&encode(unit)).unwrap();
            }
            drop(file);

            let preview = read_text_hover_preview(&path, 2).unwrap();
            assert_eq!(
                preview
                    .lines
                    .iter()
                    .map(|line| line.text.as_str())
                    .collect::<Vec<_>>(),
                vec!["alpha", "beta"]
            );
        }
    }

    #[test]
    fn reader_stops_at_oversized_line_without_scanning_the_file() {
        let temp = TempDir::new();
        let path = temp.path().join("large.json");
        fs::write(&path, vec![b'x'; TEXT_HOVER_PREVIEW_MAX_TOTAL_BYTES * 4]).unwrap();

        let preview = read_text_hover_preview(&path, 20).unwrap();
        assert_eq!(preview.lines.len(), 1);
        assert!(preview.lines[0].truncated);
        assert!(preview.truncated);
        assert_eq!(
            preview.lines[0].text.chars().count(),
            TEXT_HOVER_PREVIEW_MAX_LINE_BYTES + 1
        );
    }

    #[test]
    fn empty_file_has_no_preview_lines() {
        let temp = TempDir::new();
        let path = temp.path().join("empty.txt");
        fs::write(&path, []).unwrap();
        assert_eq!(
            read_text_hover_preview(&path, 20).unwrap(),
            TextHoverPreviewContent::default()
        );
    }
}
