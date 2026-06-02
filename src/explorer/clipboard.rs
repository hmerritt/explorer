use std::path::PathBuf;

use gpui::ClipboardItem;
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

    Ok(ClipboardItem::new_string_with_metadata(
        clipboard_text(&clipboard.paths),
        metadata,
    ))
}

pub(super) fn file_clipboard_from_item(item: &ClipboardItem) -> Option<FileClipboard> {
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

fn clipboard_text(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_clipboard_metadata_round_trips() {
        let clipboard = FileClipboard::new(
            FileClipboardOperation::Copy,
            vec![PathBuf::from("a.txt"), PathBuf::from("folder")],
        );

        let item = clipboard_item_for_files(&clipboard).expect("clipboard item");

        assert_eq!(item.text(), Some("a.txt\nfolder".to_owned()));
        assert_eq!(file_clipboard_from_item(&item), Some(clipboard));
    }

    #[test]
    fn cut_clipboard_metadata_round_trips() {
        let clipboard = FileClipboard::new(
            FileClipboardOperation::Cut,
            vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")],
        );

        let item = clipboard_item_for_files(&clipboard).expect("clipboard item");

        assert_eq!(file_clipboard_from_item(&item), Some(clipboard));
    }

    #[test]
    fn non_explorer_clipboard_item_is_ignored() {
        let item = ClipboardItem::new_string("plain text".to_owned());

        assert_eq!(file_clipboard_from_item(&item), None);
    }
}
