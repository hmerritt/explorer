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

pub(super) fn download_from_clipboard_item(item: &ClipboardItem) -> Option<ClipboardDownload> {
    let text = item.text()?;
    download_from_text(text.trim())
}

fn download_from_text(text: &str) -> Option<ClipboardDownload> {
    if text.is_empty() || text.lines().count() != 1 {
        return None;
    }

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
            || download_from_clipboard_item(item).is_some()
    })
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
    fn paste_payload_accepts_files_but_rejects_plain_text_and_empty_clipboard() {
        let explorer_item = clipboard_item_for_files(&FileClipboard::new(
            FileClipboardOperation::Copy,
            vec![PathBuf::from("a.txt")],
        ))
        .expect("clipboard item");
        let plain_item = ClipboardItem::new_string("plain text".to_owned());

        assert!(clipboard_item_can_paste(Some(&explorer_item)));
        assert!(!clipboard_item_can_paste(Some(&plain_item)));
        assert!(!clipboard_item_can_paste(None));
    }

    #[test]
    fn download_clipboard_accepts_http_files_and_decodes_the_file_name() {
        let item = ClipboardItem::new_string(
            "  https://example.com/releases/My%20File.tar.gz?download=1#asset  ".to_owned(),
        );

        let download = download_from_clipboard_item(&item).expect("download URL");
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
            "https://example.com/file.zip\nhttps://example.com/other.zip",
        ] {
            let item = ClipboardItem::new_string(text.to_owned());
            assert!(
                download_from_clipboard_item(&item).is_none(),
                "unexpectedly accepted {text:?}"
            );
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
