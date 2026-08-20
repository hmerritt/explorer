use std::path::{Path, PathBuf};

use gpui::{ClipboardEntry, ClipboardFileOperation, ClipboardItem, Image, http_client::Url};
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};

const CLIPBOARD_KIND: &str = "explorer.file-clipboard";
const CLIPBOARD_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum FileClipboardOperation {
    Copy,
    Cut,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileClipboard {
    pub(super) operation: FileClipboardOperation,
    pub(super) paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ClipboardDownload {
    pub(super) url: Url,
    pub(super) file_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ClipboardMaterialization {
    pub(super) file_name: &'static str,
    pub(super) contents: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ClipboardTextPayload {
    Downloads(Vec<ClipboardDownload>),
    Materialization(ClipboardMaterialization),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct FileClipboardMetadata {
    kind: String,
    version: u8,
    operation: FileClipboardOperation,
    paths: Vec<PathBuf>,
}

impl FileClipboard {
    pub(super) fn new(operation: FileClipboardOperation, paths: Vec<PathBuf>) -> Self {
        Self { operation, paths }
    }
}

pub(super) fn clipboard_item_for_files(clipboard: &FileClipboard) -> Result<ClipboardItem, String> {
    let metadata = FileClipboardMetadata {
        kind: CLIPBOARD_KIND.to_owned(),
        version: CLIPBOARD_VERSION,
        operation: clipboard.operation,
        paths: clipboard.paths.clone(),
    };
    let metadata = serde_json::to_string(&metadata)
        .map_err(|error| format!("Could not write Explorer clipboard data: {error}"))?;

    if clipboard
        .paths
        .iter()
        .any(|path| crate::explorer::portable_devices::is_portable_path(path))
    {
        // Synthetic portable locations are meaningful only inside Explorer. Keep
        // them in our metadata without advertising them as native filesystem paths.
        Ok(ClipboardItem::new_string_with_metadata(
            clipboard_text(&clipboard.paths),
            metadata,
        ))
    } else {
        Ok(ClipboardItem::new_files_with_metadata(
            clipboard.paths.clone(),
            native_clipboard_operation(clipboard.operation),
            clipboard_text(&clipboard.paths),
            metadata,
        ))
    }
}

pub(super) fn file_clipboard_from_item(item: &ClipboardItem) -> Option<FileClipboard> {
    if let Some(files) = item.files() {
        if !files.paths.is_empty() {
            return Some(FileClipboard {
                operation: explorer_clipboard_operation(files.operation),
                paths: files.paths.clone(),
            });
        }
    }

    let metadata = item.metadata()?;
    let metadata = serde_json::from_str::<FileClipboardMetadata>(metadata).ok()?;

    if metadata.kind != CLIPBOARD_KIND || metadata.version != CLIPBOARD_VERSION {
        return None;
    }

    Some(FileClipboard {
        operation: metadata.operation,
        paths: metadata.paths,
    })
}

pub(super) fn image_clipboard_from_item(item: &ClipboardItem) -> Option<&Image> {
    item.entries().iter().find_map(|entry| match entry {
        ClipboardEntry::Image(image) => Some(image),
        ClipboardEntry::String(_) => None,
        ClipboardEntry::Files(_) => None,
    })
}

pub(super) fn clipboard_text_payload_from_item(
    item: &ClipboardItem,
) -> Option<ClipboardTextPayload> {
    if let Some(markdown) = item.markdown().filter(|markdown| !markdown.is_empty()) {
        return Some(materialization("document.md", markdown));
    }

    let text = item.text().unwrap_or_default();
    if let Some(downloads) = downloads_from_text(&text) {
        return Some(ClipboardTextPayload::Downloads(downloads));
    }
    let trimmed = text.trim();
    if !trimmed.is_empty()
        && serde_json::from_str::<serde_json::Value>(trimmed).is_ok_and(|value| {
            matches!(
                value,
                serde_json::Value::Array(_) | serde_json::Value::Object(_)
            )
        })
    {
        return Some(materialization("data.json", text));
    }
    if let Some(csv) = tab_separated_text_to_csv(&text) {
        return Some(materialization("table.csv", csv));
    }

    if is_svg_document(&text) {
        return Some(materialization("vector.svg", text));
    }
    if has_strong_markdown_syntax(&text) {
        return Some(materialization("document.md", text));
    }
    if !text.is_empty() {
        return Some(materialization("text.txt", text));
    }
    None
}

fn materialization(file_name: &'static str, contents: impl Into<Vec<u8>>) -> ClipboardTextPayload {
    ClipboardTextPayload::Materialization(ClipboardMaterialization {
        file_name,
        contents: contents.into(),
    })
}

fn downloads_from_text(text: &str) -> Option<Vec<ClipboardDownload>> {
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return None;
    }

    lines.into_iter().map(download_from_url_text).collect()
}

fn download_from_url_text(text: &str) -> Option<ClipboardDownload> {
    let url = Url::parse(text).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return None;
    }

    let encoded_name = url.path_segments()?.next_back()?;
    if encoded_name.is_empty() {
        return None;
    }
    let file_name = percent_decode_str(encoded_name)
        .decode_utf8()
        .ok()?
        .into_owned();
    if !download_file_name_is_valid(&file_name) {
        return None;
    }

    Some(ClipboardDownload { url, file_name })
}

fn download_file_name_is_valid(file_name: &str) -> bool {
    if file_name.is_empty()
        || matches!(file_name, "." | "..")
        || file_name.ends_with(['.', ' '])
        || file_name.chars().any(|ch| {
            ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        })
    {
        return false;
    }

    let extension_is_present = Path::new(file_name)
        .extension()
        .is_some_and(|extension| !extension.is_empty());
    if !extension_is_present {
        return false;
    }

    let windows_stem = file_name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(['.', ' '])
        .to_ascii_uppercase();
    !matches!(
        windows_stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$"
    ) && !matches!(
        windows_stem.strip_prefix("COM"),
        Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
    ) && !matches!(
        windows_stem.strip_prefix("LPT"),
        Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
    )
}

pub(super) fn clipboard_item_can_paste(item: Option<&ClipboardItem>) -> bool {
    item.is_some_and(|item| {
        file_clipboard_from_item(item).is_some()
            || image_clipboard_from_item(item).is_some()
            || clipboard_text_payload_from_item(item).is_some()
    })
}

fn tab_separated_text_to_csv(text: &str) -> Option<String> {
    let rows = parse_delimited_rows(text, '\t')?;
    let width = rows.first()?.len();
    if width < 2 || rows.iter().any(|row| row.len() != width) {
        return None;
    }

    let mut csv = String::new();
    for (row_index, row) in rows.iter().enumerate() {
        if row_index > 0 {
            csv.push_str("\r\n");
        }
        for (column_index, field) in row.iter().enumerate() {
            if column_index > 0 {
                csv.push(',');
            }
            if field.contains([',', '"', '\r', '\n']) {
                csv.push('"');
                csv.push_str(&field.replace('"', "\"\""));
                csv.push('"');
            } else {
                csv.push_str(field);
            }
        }
    }
    Some(csv)
}

fn parse_delimited_rows(text: &str, delimiter: char) -> Option<Vec<Vec<String>>> {
    if !text.contains(delimiter) {
        return None;
    }
    let chars = text.chars().collect::<Vec<_>>();
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut quoted_field = false;
    let mut just_ended_row = false;
    let mut index = 0usize;
    while index < chars.len() {
        let ch = chars[index];
        if in_quotes {
            if ch == '"' {
                if chars.get(index + 1) == Some(&'"') {
                    field.push('"');
                    index += 2;
                    continue;
                }
                in_quotes = false;
            } else {
                field.push(ch);
            }
            index += 1;
            continue;
        }

        match ch {
            '"' if field.is_empty() && !quoted_field => {
                in_quotes = true;
                quoted_field = true;
            }
            ch if ch == delimiter => {
                row.push(std::mem::take(&mut field));
                quoted_field = false;
                just_ended_row = false;
            }
            '\r' | '\n' => {
                if ch == '\r' && chars.get(index + 1) == Some(&'\n') {
                    index += 1;
                }
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                quoted_field = false;
                just_ended_row = true;
            }
            _ if quoted_field => return None,
            _ => {
                field.push(ch);
                just_ended_row = false;
            }
        }
        index += 1;
    }
    if in_quotes {
        return None;
    }
    if !just_ended_row || !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    (!rows.is_empty()).then_some(rows)
}

fn is_svg_document(text: &str) -> bool {
    let mut source = text.trim_start_matches('\u{feff}').trim();
    if source
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("<?xml"))
    {
        let Some(end) = source.find("?>") else {
            return false;
        };
        source = source[end + 2..].trim_start();
    }
    if !starts_with_tag(source, "svg") {
        return false;
    }
    let source = source.trim_end();
    source
        .get(source.len().saturating_sub(6)..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case("</svg>"))
        || (source.ends_with("/>") && source[1..source.len() - 2].find('<').is_none())
}

fn starts_with_tag(source: &str, tag: &str) -> bool {
    let Some(prefix) = source.get(..1 + tag.len()) else {
        return false;
    };
    source.starts_with('<')
        && prefix[1..].eq_ignore_ascii_case(tag)
        && source[1 + tag.len()..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_whitespace() || matches!(ch, '>' | '/'))
}

fn has_strong_markdown_syntax(text: &str) -> bool {
    let lines = text.lines().collect::<Vec<_>>();
    if lines.iter().any(|line| is_atx_heading(line))
        || has_paired_fence(&lines)
        || has_markdown_link(text)
        || has_markdown_table(&lines)
        || has_closed_front_matter(&lines)
    {
        return true;
    }

    lines
        .iter()
        .map(|line| is_markdown_list_or_quote(line))
        .fold((false, 0usize), |(found, run), structured| {
            let run = if structured { run + 1 } else { 0 };
            (found || run >= 2, run)
        })
        .0
}

fn is_atx_heading(line: &str) -> bool {
    let line = line.trim_start();
    let hashes = line.chars().take_while(|ch| *ch == '#').count();
    (1..=6).contains(&hashes)
        && line[hashes..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
}

fn has_paired_fence(lines: &[&str]) -> bool {
    let mut opening = None;
    for line in lines {
        let line = line.trim_start();
        let Some(marker) = line.chars().next().filter(|ch| matches!(ch, '`' | '~')) else {
            continue;
        };
        let count = line.chars().take_while(|ch| *ch == marker).count();
        if count < 3 {
            continue;
        }
        if opening
            .is_some_and(|(open_marker, open_count)| marker == open_marker && count >= open_count)
        {
            return true;
        }
        opening = Some((marker, count));
    }
    false
}

fn has_markdown_link(text: &str) -> bool {
    let mut offset = 0usize;
    while let Some(close) = text[offset..].find("](") {
        let close = offset + close;
        if text[..close].rfind('[').is_some() && text[close + 2..].find(')').is_some() {
            return true;
        }
        offset = close + 2;
    }
    false
}

fn has_markdown_table(lines: &[&str]) -> bool {
    lines.windows(2).any(|pair| {
        pair[0].contains('|')
            && pair[1]
                .trim()
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .filter(|cell| !cell.is_empty())
                .all(|cell| {
                    cell.trim_matches(':').len() >= 3
                        && cell.trim_matches(':').chars().all(|ch| ch == '-')
                })
            && pair[1].contains('-')
    })
}

fn has_closed_front_matter(lines: &[&str]) -> bool {
    lines.first().is_some_and(|line| line.trim() == "---")
        && lines.iter().skip(1).any(|line| line.trim() == "---")
}

fn is_markdown_list_or_quote(line: &str) -> bool {
    let line = line.trim_start();
    if line.starts_with("> ")
        || line.starts_with("- ")
        || line.starts_with("* ")
        || line.starts_with("+ ")
    {
        return true;
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    digits > 0
        && line[digits..]
            .get(..2)
            .is_some_and(|suffix| matches!(suffix, ". " | ") "))
}

fn clipboard_text(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n")
}

fn native_clipboard_operation(operation: FileClipboardOperation) -> ClipboardFileOperation {
    match operation {
        FileClipboardOperation::Copy => ClipboardFileOperation::Copy,
        FileClipboardOperation::Cut => ClipboardFileOperation::Move,
    }
}

fn explorer_clipboard_operation(operation: ClipboardFileOperation) -> FileClipboardOperation {
    match operation {
        ClipboardFileOperation::Copy => FileClipboardOperation::Copy,
        ClipboardFileOperation::Move => FileClipboardOperation::Cut,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Image, ImageFormat};

    #[test]
    fn copy_clipboard_metadata_round_trips() {
        let clipboard = FileClipboard::new(
            FileClipboardOperation::Copy,
            vec![PathBuf::from("a.txt"), PathBuf::from("folder")],
        );

        let item = clipboard_item_for_files(&clipboard).expect("clipboard item");

        assert_eq!(item.text(), Some("a.txt\nfolder".to_owned()));
        assert_eq!(
            item.files().map(|files| files.operation),
            Some(ClipboardFileOperation::Copy)
        );
        assert_eq!(file_clipboard_from_item(&item), Some(clipboard));
    }

    #[test]
    fn cut_clipboard_metadata_round_trips() {
        let clipboard = FileClipboard::new(
            FileClipboardOperation::Cut,
            vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")],
        );

        let item = clipboard_item_for_files(&clipboard).expect("clipboard item");

        assert_eq!(
            item.files().map(|files| files.operation),
            Some(ClipboardFileOperation::Move)
        );
        assert_eq!(file_clipboard_from_item(&item), Some(clipboard));
    }

    #[test]
    fn native_file_clipboard_round_trips() {
        let item = ClipboardItem::new_files(
            vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")],
            ClipboardFileOperation::Move,
        );

        assert_eq!(
            file_clipboard_from_item(&item),
            Some(FileClipboard::new(
                FileClipboardOperation::Cut,
                vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")],
            ))
        );
    }

    #[test]
    fn legacy_metadata_clipboard_round_trips() {
        let metadata = FileClipboardMetadata {
            kind: CLIPBOARD_KIND.to_owned(),
            version: CLIPBOARD_VERSION,
            operation: FileClipboardOperation::Copy,
            paths: vec![PathBuf::from("a.txt")],
        };
        let item = ClipboardItem::new_string_with_metadata(
            "a.txt".to_owned(),
            serde_json::to_string(&metadata).expect("metadata"),
        );

        assert_eq!(
            file_clipboard_from_item(&item),
            Some(FileClipboard::new(
                FileClipboardOperation::Copy,
                vec![PathBuf::from("a.txt")],
            ))
        );
    }

    #[test]
    fn plain_text_clipboard_item_is_ignored() {
        let item = ClipboardItem::new_string("C:\\Users\\test\\file.txt".to_owned());

        assert_eq!(file_clipboard_from_item(&item), None);
    }

    #[test]
    fn image_clipboard_item_is_detected_as_paste_payload() {
        let image = Image::from_bytes(ImageFormat::Png, vec![1, 2, 3]);
        let item = ClipboardItem::new_image(&image);

        assert_eq!(
            image_clipboard_from_item(&item).map(|image| image.bytes()),
            Some([1, 2, 3].as_slice())
        );
        assert!(clipboard_item_can_paste(Some(&item)));
    }

    #[test]
    fn paste_payload_accepts_files_and_plain_text_but_rejects_empty_clipboard() {
        let explorer_item = clipboard_item_for_files(&FileClipboard::new(
            FileClipboardOperation::Copy,
            vec![PathBuf::from("a.txt")],
        ))
        .expect("clipboard item");
        let plain_item = ClipboardItem::new_string("plain text".to_owned());

        assert!(clipboard_item_can_paste(Some(&explorer_item)));
        assert!(clipboard_item_can_paste(Some(&plain_item)));
        assert!(!clipboard_item_can_paste(Some(&ClipboardItem::new_string(
            String::new()
        ))));
        assert!(!clipboard_item_can_paste(None));
    }

    #[test]
    fn download_clipboard_accepts_http_files_and_decodes_the_file_name() {
        let item = ClipboardItem::new_string(
            "  https://example.com/releases/My%20File.tar.gz?download=1#asset  ".to_owned(),
        );

        let ClipboardTextPayload::Downloads(downloads) =
            clipboard_text_payload_from_item(&item).expect("download URL")
        else {
            panic!("expected downloads");
        };
        let download = &downloads[0];
        assert_eq!(
            download.url.as_str(),
            "https://example.com/releases/My%20File.tar.gz?download=1#asset"
        );
        assert_eq!(download.file_name, "My File.tar.gz");
        assert!(clipboard_item_can_paste(Some(&item)));
    }

    #[test]
    fn download_clipboard_rejects_non_files_and_unsafe_names() {
        for text in [
            "plain text",
            "example.com/file.zip",
            "ftp://example.com/file.zip",
            "https://example.com/folder/",
            "https://example.com/README",
            "https://example.com/a%2Fb.zip",
            "https://example.com/CON.txt",
        ] {
            let item = ClipboardItem::new_string(text.to_owned());
            assert!(
                !matches!(
                    clipboard_text_payload_from_item(&item),
                    Some(ClipboardTextPayload::Downloads(_))
                ),
                "unexpectedly accepted {text:?}"
            );
        }
    }

    #[test]
    fn download_clipboard_accepts_multiple_urls_and_rejects_mixed_lists() {
        let item = ClipboardItem::new_string(
            "https://example.com/one.zip\n\n https://example.com/two.tar.gz ".to_owned(),
        );
        let Some(ClipboardTextPayload::Downloads(downloads)) =
            clipboard_text_payload_from_item(&item)
        else {
            panic!("expected URL batch");
        };
        assert_eq!(
            downloads
                .iter()
                .map(|download| download.file_name.as_str())
                .collect::<Vec<_>>(),
            vec!["one.zip", "two.tar.gz"]
        );

        let mixed = ClipboardItem::new_string("https://example.com/one.zip\nnot a URL".to_owned());
        assert!(matches!(
            clipboard_text_payload_from_item(&mixed),
            Some(ClipboardTextPayload::Materialization(
                ClipboardMaterialization {
                    file_name: "text.txt",
                    ..
                }
            ))
        ));
    }

    #[test]
    fn structured_text_classification_preserves_source() {
        for (source, expected_name) in [
            (" {\n  \"ok\": true\n} ", "data.json"),
            ("[1, 2, 3]", "data.json"),
            ("<div>hello</div>", "text.txt"),
            ("# Heading\nBody", "document.md"),
            ("ordinary prose", "text.txt"),
        ] {
            let item = ClipboardItem::new_string(source.to_owned());
            let Some(ClipboardTextPayload::Materialization(materialization)) =
                clipboard_text_payload_from_item(&item)
            else {
                panic!("expected materialization for {source:?}");
            };
            assert_eq!(materialization.file_name, expected_name);
            assert_eq!(materialization.contents, source.as_bytes());
        }
        for scalar in ["true", "42", "\"string\""] {
            let item = ClipboardItem::new_string(scalar.to_owned());
            let Some(ClipboardTextPayload::Materialization(materialization)) =
                clipboard_text_payload_from_item(&item)
            else {
                panic!("expected scalar text");
            };
            assert_eq!(materialization.file_name, "text.txt");
        }
    }

    #[test]
    fn native_markdown_precedes_other_plain_text_classifiers() {
        let item = ClipboardItem::new_string_with_markdown(
            "https://example.com/file.zip".to_owned(),
            "[download](https://example.com/file.zip)".to_owned(),
        );
        let Some(ClipboardTextPayload::Materialization(materialization)) =
            clipboard_text_payload_from_item(&item)
        else {
            panic!("expected Markdown");
        };
        assert_eq!(materialization.file_name, "document.md");
        assert_eq!(
            materialization.contents,
            b"[download](https://example.com/file.zip)"
        );
    }

    #[test]
    fn tsv_conversion_quotes_csv_and_rejects_ragged_rows() {
        let item = ClipboardItem::new_string(
            "Name\tNote\r\nAda\t\"one, two\"\r\nLin\t\"said \"\"hi\"\"\"".to_owned(),
        );
        let Some(ClipboardTextPayload::Materialization(materialization)) =
            clipboard_text_payload_from_item(&item)
        else {
            panic!("expected CSV");
        };
        assert_eq!(materialization.file_name, "table.csv");
        assert_eq!(
            String::from_utf8(materialization.contents).unwrap(),
            "Name,Note\r\nAda,\"one, two\"\r\nLin,\"said \"\"hi\"\"\""
        );

        let ragged = ClipboardItem::new_string("a\tb\nc".to_owned());
        let Some(ClipboardTextPayload::Materialization(materialization)) =
            clipboard_text_payload_from_item(&ragged)
        else {
            panic!("expected text fallback");
        };
        assert_eq!(materialization.file_name, "text.txt");
    }

    #[test]
    fn svg_detection_accepts_complete_roots_and_rejects_nested_or_malformed_source() {
        for source in [
            "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
            " <?xml version=\"1.0\"?>\n<SVG viewBox=\"0 0 1 1\"></SVG> ",
            "<svg/>",
        ] {
            let item = ClipboardItem::new_string(source.to_owned());
            let Some(ClipboardTextPayload::Materialization(materialization)) =
                clipboard_text_payload_from_item(&item)
            else {
                panic!("expected SVG");
            };
            assert_eq!(materialization.file_name, "vector.svg");
        }
        for source in [
            "<div><svg></svg></div>",
            "<svg><path/></div>",
            "not <svg></svg>",
        ] {
            assert!(!is_svg_document(source), "unexpected SVG: {source:?}");
        }
    }

    #[test]
    fn spreadsheet_plain_text_materializes_as_csv() {
        let item = ClipboardItem::new_string("a\tb\n1\t2".to_owned());
        let Some(ClipboardTextPayload::Materialization(materialization)) =
            clipboard_text_payload_from_item(&item)
        else {
            panic!("expected CSV");
        };
        assert_eq!(materialization.file_name, "table.csv");
    }

    #[test]
    fn markdown_detection_is_conservative() {
        for source in [
            "## Heading",
            "```rust\nfn main() {}\n```",
            "See [the docs](https://example.com)",
            "a | b\n--- | ---",
            "---\ntitle: Test\n---",
            "- one\n- two",
            "> one\n> two",
        ] {
            assert!(has_strong_markdown_syntax(source), "missed {source:?}");
        }
        for source in ["ordinary prose", "- one", "1. one", "#not a heading"] {
            assert!(!has_strong_markdown_syntax(source), "accepted {source:?}");
        }
    }

    #[test]
    fn portable_clipboard_round_trips_only_through_explorer_metadata() {
        let path = crate::explorer::portable_devices::virtual_root()
            .join("device-1")
            .join("storage-g1-1")
            .join("object-1-photo.jpg");
        let clipboard = FileClipboard::new(FileClipboardOperation::Copy, vec![path]);
        let item = clipboard_item_for_files(&clipboard).expect("clipboard item");

        assert!(item.files().is_none());
        assert_eq!(file_clipboard_from_item(&item), Some(clipboard));
    }
}
