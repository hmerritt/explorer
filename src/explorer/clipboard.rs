use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use gpui::{
    App, BorrowAppContext, ClipboardEntry, ClipboardFileOperation, ClipboardItem, Global, Image,
    ImageFormat, http_client::Url,
};
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use thousands::Separable;
use xxhash_rust::xxh3::xxh3_64;

use crate::explorer::{
    folder_size::{
        FolderSizeError, RecursiveContentCounts, calculate_folder_size,
        calculate_recursive_content_counts,
    },
    formatting::format_size,
};

const CLIPBOARD_KIND: &str = "explorer.file-clipboard";
const CLIPBOARD_VERSION: u8 = 1;
const CLIPBOARD_DETAIL_MAX_URLS: usize = 5;
const CLIPBOARD_DETAIL_MAX_PREVIEW_LINES: usize = 8;
const CLIPBOARD_DETAIL_MAX_PREVIEW_BYTES: usize = 2 * 1024;

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

#[derive(Clone, Eq, PartialEq)]
pub(super) struct ClipboardDownload {
    pub(super) url: Url,
    pub(super) file_name: String,
}

impl std::fmt::Debug for ClipboardDownload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut url = self.url.clone();
        if url.password().is_some() {
            let _ = url.set_password(Some("<redacted>"));
        }
        formatter
            .debug_struct("ClipboardDownload")
            .field("url", &url)
            .field("file_name", &self.file_name)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ClipboardVideoDownload {
    pub(super) url: Url,
    pub(super) site_domain: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ClipboardMaterialization {
    pub(super) file_name: &'static str,
    pub(super) contents: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ClipboardTextPayload {
    Downloads(Vec<ClipboardDownload>),
    VideoDownloads(Vec<ClipboardVideoDownload>),
    Materialization(ClipboardMaterialization),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ClipboardSummary {
    pub(super) label: String,
    pub(super) details: ClipboardSummaryDetails,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ClipboardMetric<T> {
    Pending { discovered: Option<T> },
    Ready(T),
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ClipboardTextPreview {
    pub(super) lines: Vec<String>,
    pub(super) truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ClipboardUrlPreview {
    pub(super) urls: Vec<String>,
    pub(super) omitted_count: usize,
    pub(super) truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ClipboardSummaryDetails {
    Files {
        operation: FileClipboardOperation,
        folder_count: ClipboardMetric<usize>,
        file_count: ClipboardMetric<usize>,
        total_size: ClipboardMetric<u64>,
    },
    Image {
        source_format: ImageFormat,
        output_file_name: String,
        byte_size: u64,
    },
    Downloads {
        count: usize,
        urls: ClipboardUrlPreview,
    },
    VideoDownloads {
        count: usize,
        site_summary: String,
        urls: ClipboardUrlPreview,
    },
    Materialization {
        output_file_name: &'static str,
        source_size: u64,
        output_size: u64,
        source_preview: ClipboardTextPreview,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ClipboardFingerprint {
    Files {
        operation: FileClipboardOperation,
        paths: Vec<PathBuf>,
    },
    Image {
        format: ImageFormat,
        byte_len: usize,
        digest: u64,
    },
    Text {
        byte_len: usize,
        digest: u64,
    },
}

#[derive(Default)]
pub(super) struct ClipboardSummaryState {
    fingerprint: Option<ClipboardFingerprint>,
    generation: u64,
    summary: Option<ClipboardSummary>,
    cancel: Option<Arc<AtomicBool>>,
}

impl Global for ClipboardSummaryState {}

struct ClipboardInspection {
    fingerprint: ClipboardFingerprint,
    summary: ClipboardSummary,
    file_paths: Option<Vec<PathBuf>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ClipboardFilesystemMetadata {
    folder_paths: Vec<PathBuf>,
    folder_count: usize,
    file_count: usize,
    file_size: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClipboardSummaryScanError {
    Cancelled,
    Unavailable,
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

pub(crate) fn initialize_clipboard_summary(cx: &mut App) {
    cx.set_global(ClipboardSummaryState::default());
}

pub(super) fn clipboard_summary(cx: &App) -> Option<&ClipboardSummary> {
    cx.try_global::<ClipboardSummaryState>()?.summary.as_ref()
}

pub(super) fn write_to_clipboard_and_refresh(item: ClipboardItem, cx: &mut App) {
    cx.write_to_clipboard(item);
    refresh_clipboard_summary(cx);
}

pub(super) fn refresh_clipboard_summary(cx: &mut App) {
    if cx.try_global::<ClipboardSummaryState>().is_none() {
        cx.set_global(ClipboardSummaryState::default());
    }

    let inspection = cx
        .read_from_clipboard()
        .as_ref()
        .and_then(clipboard_summary_inspection);
    let next_fingerprint = inspection
        .as_ref()
        .map(|inspection| inspection.fingerprint.clone());
    let fingerprint_is_unchanged =
        cx.global::<ClipboardSummaryState>().fingerprint.as_ref() == next_fingerprint.as_ref();
    let filesystem_payload = inspection
        .as_ref()
        .is_some_and(|inspection| inspection.file_paths.is_some());
    if fingerprint_is_unchanged && !filesystem_payload {
        return;
    }

    let (generation, cancel) = cx.update_global::<ClipboardSummaryState, _>(|state, _| {
        if let Some(cancel) = state.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        state.generation = state.generation.wrapping_add(1);
        state.fingerprint = next_fingerprint.clone();
        state.summary = inspection
            .as_ref()
            .map(|inspection| inspection.summary.clone());
        state.cancel = inspection
            .as_ref()
            .and_then(|inspection| inspection.file_paths.as_ref())
            .map(|_| Arc::new(AtomicBool::new(false)));
        (state.generation, state.cancel.clone())
    });

    let Some(inspection) = inspection else {
        return;
    };
    let Some(paths) = inspection.file_paths else {
        return;
    };
    let Some(cancel) = cancel else {
        return;
    };
    let initial_label = inspection.summary.label.clone();
    let ClipboardSummaryDetails::Files { operation, .. } = inspection.summary.details else {
        return;
    };
    let fingerprint = inspection.fingerprint;

    cx.spawn(async move |cx| {
        let metadata_cancel = cancel.clone();
        let metadata_paths = paths.clone();
        let metadata_task = cx.background_executor().spawn(async move {
            scan_clipboard_filesystem_metadata(&metadata_paths, &metadata_cancel)
        });
        let metadata = match metadata_task.await {
            Ok(metadata) => metadata,
            Err(ClipboardSummaryScanError::Cancelled) => return,
            Err(ClipboardSummaryScanError::Unavailable) => {
                let unavailable = ClipboardSummary {
                    label: initial_label,
                    details: ClipboardSummaryDetails::Files {
                        operation,
                        folder_count: ClipboardMetric::Unavailable,
                        file_count: ClipboardMetric::Unavailable,
                        total_size: ClipboardMetric::Unavailable,
                    },
                };
                let _ = cx.update(|cx| {
                    update_clipboard_summary_if_current(cx, generation, &fingerprint, unavailable)
                });
                return;
            }
        };
        if cancel.load(Ordering::Relaxed) {
            return;
        }

        let top_level_summary = ClipboardSummary {
            label: clipboard_filesystem_summary_label(
                metadata.folder_count,
                metadata.file_count,
                None,
            ),
            details: ClipboardSummaryDetails::Files {
                operation,
                folder_count: ClipboardMetric::Pending {
                    discovered: Some(metadata.folder_count),
                },
                file_count: ClipboardMetric::Pending {
                    discovered: Some(metadata.file_count),
                },
                total_size: ClipboardMetric::Pending { discovered: None },
            },
        };
        if cx
            .update(|cx| {
                update_clipboard_summary_if_current(cx, generation, &fingerprint, top_level_summary)
            })
            .ok()
            != Some(true)
        {
            return;
        }

        let recursive_paths = paths.clone();
        let count_cancel = cancel.clone();
        let count_task = cx
            .background_executor()
            .spawn(async move { scan_clipboard_recursive_counts(&recursive_paths, &count_cancel) });
        let count_result = count_task.await;
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let (folder_count, file_count) = match count_result {
            Ok(counts) => (
                ClipboardMetric::Ready(counts.folder_count),
                ClipboardMetric::Ready(counts.file_count),
            ),
            Err(ClipboardSummaryScanError::Cancelled) => return,
            Err(ClipboardSummaryScanError::Unavailable) => {
                (ClipboardMetric::Unavailable, ClipboardMetric::Unavailable)
            }
        };
        let counted_summary = ClipboardSummary {
            label: clipboard_filesystem_summary_label(
                metadata.folder_count,
                metadata.file_count,
                None,
            ),
            details: ClipboardSummaryDetails::Files {
                operation,
                folder_count: folder_count.clone(),
                file_count: file_count.clone(),
                total_size: ClipboardMetric::Pending { discovered: None },
            },
        };
        if cx
            .update(|cx| {
                update_clipboard_summary_if_current(cx, generation, &fingerprint, counted_summary)
            })
            .ok()
            != Some(true)
        {
            return;
        }

        let folder_paths = metadata.folder_paths.clone();
        let file_size = metadata.file_size;
        let folder_cancel = cancel.clone();
        let folder_size_task = cx.background_executor().spawn(async move {
            scan_clipboard_folder_sizes(&folder_paths, file_size, &folder_cancel)
        });
        let size_result = folder_size_task.await;
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let (label_size, total_size) = match size_result {
            Ok(size) => (Some(size), ClipboardMetric::Ready(size)),
            Err(ClipboardSummaryScanError::Cancelled) => return,
            Err(ClipboardSummaryScanError::Unavailable) => (None, ClipboardMetric::Unavailable),
        };

        let final_summary = ClipboardSummary {
            label: clipboard_filesystem_summary_label(
                metadata.folder_count,
                metadata.file_count,
                label_size,
            ),
            details: ClipboardSummaryDetails::Files {
                operation,
                folder_count,
                file_count,
                total_size,
            },
        };
        let _ = cx.update(|cx| {
            update_clipboard_summary_if_current(cx, generation, &fingerprint, final_summary)
        });
    })
    .detach();
}

fn clipboard_summary_inspection(item: &ClipboardItem) -> Option<ClipboardInspection> {
    if let Some(clipboard) = file_clipboard_from_item(item)
        && !clipboard.paths.is_empty()
    {
        let summary = ClipboardSummary {
            label: clipboard_count_label(clipboard.paths.len(), "item", "items"),
            details: ClipboardSummaryDetails::Files {
                operation: clipboard.operation,
                folder_count: ClipboardMetric::Pending { discovered: None },
                file_count: ClipboardMetric::Pending { discovered: None },
                total_size: ClipboardMetric::Pending { discovered: None },
            },
        };
        return Some(ClipboardInspection {
            fingerprint: ClipboardFingerprint::Files {
                operation: clipboard.operation,
                paths: clipboard.paths.clone(),
            },
            summary,
            file_paths: Some(clipboard.paths),
        });
    }

    if let Some(image) = image_clipboard_from_item(item) {
        let label = if image.format() == ImageFormat::Svg {
            "SVG vector file"
        } else {
            "Image file"
        };
        return Some(ClipboardInspection {
            fingerprint: ClipboardFingerprint::Image {
                format: image.format(),
                byte_len: image.bytes().len(),
                digest: xxh3_64(image.bytes()),
            },
            summary: ClipboardSummary {
                label: clipboard_typed_summary_label(label, image.bytes().len() as u64),
                details: ClipboardSummaryDetails::Image {
                    source_format: image.format(),
                    output_file_name: clipboard_image_output_file_name(image.format()),
                    byte_size: image.bytes().len() as u64,
                },
            },
            file_paths: None,
        });
    }

    let payload = clipboard_text_payload_from_item(item)?;
    let text = item.text().unwrap_or_default();
    let markdown = item.markdown().unwrap_or_default();
    let mut fingerprint_bytes = Vec::with_capacity(text.len() + markdown.len() + 1);
    fingerprint_bytes.extend_from_slice(text.as_bytes());
    fingerprint_bytes.push(0);
    fingerprint_bytes.extend_from_slice(markdown.as_bytes());

    let (label, details) = match payload {
        ClipboardTextPayload::Downloads(downloads) => (
            clipboard_count_label(downloads.len(), "URL download", "URL downloads"),
            ClipboardSummaryDetails::Downloads {
                count: downloads.len(),
                urls: clipboard_url_preview(&text),
            },
        ),
        ClipboardTextPayload::VideoDownloads(downloads) => (
            video_download_summary_label(&downloads),
            ClipboardSummaryDetails::VideoDownloads {
                count: downloads.len(),
                site_summary: video_download_site_summary(&downloads),
                urls: clipboard_url_preview(&text),
            },
        ),
        ClipboardTextPayload::Materialization(materialization) => {
            let source = clipboard_materialization_source(item, materialization.file_name, &text);
            (
                clipboard_typed_summary_label(
                    clipboard_materialization_type_label(materialization.file_name),
                    materialization.contents.len() as u64,
                ),
                ClipboardSummaryDetails::Materialization {
                    output_file_name: materialization.file_name,
                    source_size: source.len() as u64,
                    output_size: materialization.contents.len() as u64,
                    source_preview: clipboard_text_preview(&source),
                },
            )
        }
    };

    Some(ClipboardInspection {
        fingerprint: ClipboardFingerprint::Text {
            byte_len: fingerprint_bytes.len(),
            digest: xxh3_64(&fingerprint_bytes),
        },
        summary: ClipboardSummary { label, details },
        file_paths: None,
    })
}

fn clipboard_image_output_file_name(format: ImageFormat) -> String {
    let extension = match format {
        ImageFormat::Png | ImageFormat::Tiff => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
    };
    format!("image.{extension}")
}

fn clipboard_materialization_source(item: &ClipboardItem, file_name: &str, text: &str) -> String {
    if file_name == "document.md"
        && let Some(markdown) = item.markdown().filter(|markdown| !markdown.is_empty())
    {
        markdown
    } else {
        text.to_owned()
    }
}

fn clipboard_text_preview(source: &str) -> ClipboardTextPreview {
    let mut lines = Vec::new();
    let mut used_bytes = 0usize;
    let mut truncated = false;
    let source_lines = source.split('\n').collect::<Vec<_>>();

    for (index, line) in source_lines.iter().enumerate() {
        if index >= CLIPBOARD_DETAIL_MAX_PREVIEW_LINES
            || used_bytes >= CLIPBOARD_DETAIL_MAX_PREVIEW_BYTES
        {
            truncated = true;
            break;
        }

        let separator_bytes = usize::from(index > 0);
        let available = CLIPBOARD_DETAIL_MAX_PREVIEW_BYTES
            .saturating_sub(used_bytes)
            .saturating_sub(separator_bytes);
        if line.len() > available {
            lines.push(utf8_prefix(line, available).to_owned());
            truncated = true;
            break;
        }

        lines.push((*line).to_owned());
        used_bytes = used_bytes.saturating_add(separator_bytes + line.len());
    }

    if lines.len() < source_lines.len() {
        truncated = true;
    }
    ClipboardTextPreview { lines, truncated }
}

fn clipboard_url_preview(source: &str) -> ClipboardUrlPreview {
    let source_urls = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let mut urls = Vec::new();
    let mut used_bytes = 0usize;
    let mut truncated = false;

    for url in source_urls.iter().take(CLIPBOARD_DETAIL_MAX_URLS) {
        let separator_bytes = usize::from(!urls.is_empty());
        let available = CLIPBOARD_DETAIL_MAX_PREVIEW_BYTES
            .saturating_sub(used_bytes)
            .saturating_sub(separator_bytes);
        if available == 0 {
            truncated = true;
            break;
        }
        let prefix = utf8_prefix(url, available);
        truncated |= prefix.len() < url.len();
        urls.push(prefix.to_owned());
        used_bytes = used_bytes.saturating_add(separator_bytes + prefix.len());
        if prefix.len() < url.len() {
            break;
        }
    }

    ClipboardUrlPreview {
        omitted_count: source_urls.len().saturating_sub(urls.len()),
        urls,
        truncated,
    }
}

fn utf8_prefix(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut boundary = max_bytes.min(text.len());
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &text[..boundary]
}

fn clipboard_materialization_type_label(file_name: &str) -> &'static str {
    match file_name {
        "data.json" => "JSON file",
        "table.csv" => "CSV file",
        "vector.svg" => "SVG vector file",
        "document.md" => "MD file",
        _ => "Text file",
    }
}

fn clipboard_typed_summary_label(kind: &str, size: u64) -> String {
    format!("{kind} · {}", format_size(Some(size)))
}

fn clipboard_filesystem_summary_label(
    folder_count: usize,
    file_count: usize,
    size: Option<u64>,
) -> String {
    let counts = match (folder_count, file_count) {
        (0, files) => clipboard_count_label(files, "file", "files"),
        (folders, 0) => clipboard_count_label(folders, "folder", "folders"),
        (folders, files) => format!(
            "{}, {}",
            clipboard_count_label(folders, "folder", "folders"),
            clipboard_count_label(files, "file", "files")
        ),
    };
    match size {
        Some(size) => format!("{counts} · {}", format_size(Some(size))),
        None => counts,
    }
}

fn clipboard_count_label(count: usize, singular: &str, plural: &str) -> String {
    let noun = if count == 1 { singular } else { plural };
    format!("{} {noun}", count.separate_with_commas())
}

fn scan_clipboard_filesystem_metadata(
    paths: &[PathBuf],
    cancel: &AtomicBool,
) -> Result<ClipboardFilesystemMetadata, ClipboardSummaryScanError> {
    let mut folder_paths = Vec::new();
    let mut file_count = 0usize;
    let mut file_size = 0u64;

    for path in paths {
        if cancel.load(Ordering::Relaxed) {
            return Err(ClipboardSummaryScanError::Cancelled);
        }
        let metadata =
            fs::symlink_metadata(path).map_err(|_| ClipboardSummaryScanError::Unavailable)?;
        if metadata.is_dir() {
            folder_paths.push(path.clone());
        } else {
            file_count += 1;
            file_size = file_size.saturating_add(metadata.len());
        }
    }

    Ok(ClipboardFilesystemMetadata {
        folder_count: folder_paths.len(),
        folder_paths,
        file_count,
        file_size,
    })
}

fn scan_clipboard_folder_sizes(
    folder_paths: &[PathBuf],
    initial_size: u64,
    cancel: &Arc<AtomicBool>,
) -> Result<u64, ClipboardSummaryScanError> {
    let mut size = initial_size;
    for path in folder_paths {
        if cancel.load(Ordering::Relaxed) {
            return Err(ClipboardSummaryScanError::Cancelled);
        }
        let folder_size =
            calculate_folder_size(path, cancel.clone()).map_err(|error| match error {
                FolderSizeError::Cancelled => ClipboardSummaryScanError::Cancelled,
                FolderSizeError::Unavailable => ClipboardSummaryScanError::Unavailable,
            })?;
        size = size.saturating_add(folder_size);
    }
    Ok(size)
}

fn scan_clipboard_recursive_counts(
    paths: &[PathBuf],
    cancel: &Arc<AtomicBool>,
) -> Result<RecursiveContentCounts, ClipboardSummaryScanError> {
    let mut counts = RecursiveContentCounts::default();
    for path in paths {
        if cancel.load(Ordering::Relaxed) {
            return Err(ClipboardSummaryScanError::Cancelled);
        }
        let path_counts = calculate_recursive_content_counts(path, cancel.clone()).map_err(
            |error| match error {
                FolderSizeError::Cancelled => ClipboardSummaryScanError::Cancelled,
                FolderSizeError::Unavailable => ClipboardSummaryScanError::Unavailable,
            },
        )?;
        counts.folder_count = counts.folder_count.saturating_add(path_counts.folder_count);
        counts.file_count = counts.file_count.saturating_add(path_counts.file_count);
    }
    Ok(counts)
}

fn update_clipboard_summary_if_current(
    cx: &mut App,
    generation: u64,
    fingerprint: &ClipboardFingerprint,
    summary: ClipboardSummary,
) -> bool {
    if cx
        .try_global::<ClipboardSummaryState>()
        .is_none_or(|state| {
            state.generation != generation || state.fingerprint.as_ref() != Some(fingerprint)
        })
    {
        return false;
    }
    cx.update_global::<ClipboardSummaryState, _>(|state, _| {
        state.summary = Some(summary);
    });
    true
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
        return text_can_materialize(&markdown).then(|| materialization("document.md", markdown));
    }

    let text = item.text().unwrap_or_default();
    if let Some(downloads) = video_downloads_from_text(&text) {
        return Some(ClipboardTextPayload::VideoDownloads(downloads));
    }
    if let Some(downloads) = downloads_from_text(&text) {
        return Some(ClipboardTextPayload::Downloads(downloads));
    }
    if !text_can_materialize(&text) {
        return None;
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
    if is_comma_separated_table(&text) {
        return Some(materialization("table.csv", text));
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

fn text_can_materialize(text: &str) -> bool {
    text.bytes()
        .any(|byte| matches!(byte, b' ' | b'\n' | b'\r'))
}

fn materialization(file_name: &'static str, contents: impl Into<Vec<u8>>) -> ClipboardTextPayload {
    ClipboardTextPayload::Materialization(ClipboardMaterialization {
        file_name,
        contents: contents.into(),
    })
}

fn video_downloads_from_text(text: &str) -> Option<Vec<ClipboardVideoDownload>> {
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return None;
    }

    lines
        .into_iter()
        .map(video_download_from_url_text)
        .collect()
}

fn video_download_from_url_text(text: &str) -> Option<ClipboardVideoDownload> {
    let url = Url::parse(text).ok()?;
    let site_domain = crate::explorer::ytdlp_sites::video_site_domain(&url)?;
    Some(ClipboardVideoDownload { url, site_domain })
}

fn video_download_summary_label(downloads: &[ClipboardVideoDownload]) -> String {
    let Some(first) = downloads.first() else {
        return "Download videos".to_owned();
    };
    if downloads.len() == 1 {
        return format!("Download video from {}", first.site_domain);
    }
    if downloads
        .iter()
        .all(|download| download.site_domain == first.site_domain)
    {
        format!(
            "Download {} videos from {}",
            downloads.len(),
            first.site_domain
        )
    } else {
        format!("Download {} videos from multiple sites", downloads.len())
    }
}

fn video_download_site_summary(downloads: &[ClipboardVideoDownload]) -> String {
    let Some(first) = downloads.first() else {
        return "Unknown site".to_owned();
    };
    if downloads
        .iter()
        .all(|download| download.site_domain == first.site_domain)
    {
        first.site_domain.clone()
    } else {
        "Multiple sites".to_owned()
    }
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
    if !matches!(url.scheme(), "http" | "https" | "ftp" | "sftp") || url.host().is_none() {
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
    if !delimited_rows_have_table_shape(&rows, 1) {
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

fn is_comma_separated_table(text: &str) -> bool {
    parse_delimited_rows(text, ',').is_some_and(|rows| delimited_rows_have_table_shape(&rows, 2))
}

fn delimited_rows_have_table_shape(rows: &[Vec<String>], minimum_rows: usize) -> bool {
    let Some(width) = rows.first().map(Vec::len) else {
        return false;
    };
    rows.len() >= minimum_rows && width >= 2 && rows.iter().all(|row| row.len() == width)
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
    use crate::explorer::{
        constants::{KB_BYTES, MB_BYTES},
        test_support::TempDir,
    };
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
    fn paste_payload_accepts_files_and_spaced_text_but_rejects_single_tokens() {
        let explorer_item = clipboard_item_for_files(&FileClipboard::new(
            FileClipboardOperation::Copy,
            vec![PathBuf::from("a.txt")],
        ))
        .expect("clipboard item");
        assert!(clipboard_item_can_paste(Some(&explorer_item)));
        for allowed in ["plain text", "plain\ntext", "plain\rtext"] {
            assert!(clipboard_item_can_paste(Some(&ClipboardItem::new_string(
                allowed.to_owned()
            ))));
        }
        for blocked in ["", "password", "a.txt", "{\"ok\":true}", "<svg/>", "a\tb"] {
            assert!(!clipboard_item_can_paste(Some(&ClipboardItem::new_string(
                blocked.to_owned()
            ))));
        }
        assert!(!clipboard_item_can_paste(None));
    }

    #[test]
    fn download_clipboard_accepts_http_files_and_decodes_the_file_name() {
        let item = ClipboardItem::new_string(
            "https://example.com/releases/My%20File.tar.gz?download=1#asset".to_owned(),
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
    fn video_clipboard_accepts_supported_video_urls() {
        for (text, expected_site) in [
            ("https://youtube.com/watch?v=dQw4w9WgXcQ", "youtube.com"),
            ("https://youtu.be/dQw4w9WgXcQ?si=share-token", "youtu.be"),
            ("https://player.vimeo.com/video/76979871", "vimeo.com"),
            (
                "https://www.dailymotion.com/video/x84sh87",
                "dailymotion.com",
            ),
            (
                "https://www.tiktok.com/@scout2015/video/6718335390845095173",
                "tiktok.com",
            ),
            ("https://x.com/jack/status/20", "x.com"),
            (
                "https://www.instagram.com/reel/Example123/",
                "instagram.com",
            ),
            ("https://www.facebook.com/watch/?v=123456", "facebook.com"),
            ("https://www.twitch.tv/videos/123456", "twitch.tv"),
            (
                "https://www.reddit.com/r/videos/comments/example/title/",
                "reddit.com",
            ),
            (
                "https://www.bilibili.com/video/BV1xx411c7mD",
                "bilibili.com",
            ),
            ("https://www.bbc.co.uk/iplayer/episode/example", "bbc.co.uk"),
        ] {
            let item = ClipboardItem::new_string(text.to_owned());
            let Some(ClipboardTextPayload::VideoDownloads(downloads)) =
                clipboard_text_payload_from_item(&item)
            else {
                panic!("expected video download for {text:?}");
            };
            assert_eq!(downloads.len(), 1);
            assert_eq!(downloads[0].url.as_str(), text);
            assert_eq!(downloads[0].site_domain, expected_site);
            assert!(clipboard_item_can_paste(Some(&item)));
        }
    }

    #[test]
    fn youtube_clipboard_rejects_non_video_and_deceptive_urls() {
        for text in [
            "youtube.com/watch?v=dQw4w9WgXcQ",
            "ftp://youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtube.com/",
            "https://youtube.com/watch",
            "https://youtube.com/watch?v=short",
            "https://youtube.com/playlist?list=PL123",
            "https://youtube.com/channel/UC123",
            "https://youtube.com/@example",
            "https://notyoutube.com/watch?v=dQw4w9WgXcQ",
            "https://youtube.com.example/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/",
            "https://youtu.be/dQw4w9WgXcQ/extra",
            "https://youtube-nocookie.com/embed/dQw4w9WgXcQ",
        ] {
            assert!(
                !matches!(
                    clipboard_text_payload_from_item(&ClipboardItem::new_string(text.to_owned())),
                    Some(ClipboardTextPayload::VideoDownloads(_))
                ),
                "unexpectedly accepted {text:?}"
            );
        }
    }

    #[test]
    fn video_clipboard_accepts_batches_and_rejects_mixed_url_lists() {
        let item = ClipboardItem::new_string(
            "https://youtu.be/dQw4w9WgXcQ\n\n https://www.youtube.com/shorts/aqz-KE-bpKQ "
                .to_owned(),
        );
        let Some(ClipboardTextPayload::VideoDownloads(downloads)) =
            clipboard_text_payload_from_item(&item)
        else {
            panic!("expected video batch");
        };
        assert_eq!(downloads.len(), 2);

        let mixed = ClipboardItem::new_string(
            "https://youtu.be/dQw4w9WgXcQ\nhttps://example.com/video.mp4".to_owned(),
        );
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
    fn download_clipboard_rejects_non_files_and_unsafe_names() {
        for text in [
            "plain text",
            "example.com/file.zip",
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
    fn download_clipboard_accepts_ftp_and_sftp_file_urls() {
        let item = ClipboardItem::new_string(
            "ftp://example.com/releases/one.zip\nsftp://alice@example.com/files/two.tar.gz"
                .to_owned(),
        );
        let Some(ClipboardTextPayload::Downloads(downloads)) =
            clipboard_text_payload_from_item(&item)
        else {
            panic!("expected remote URL batch");
        };
        assert_eq!(downloads.len(), 2);
        assert_eq!(downloads[0].url.scheme(), "ftp");
        assert_eq!(downloads[0].file_name, "one.zip");
        assert_eq!(downloads[1].url.scheme(), "sftp");
        assert_eq!(downloads[1].file_name, "two.tar.gz");
    }

    #[test]
    fn clipboard_download_debug_redacts_url_passwords() {
        let download =
            download_from_url_text("sftp://alice:super-secret@example.com/files/archive.zip")
                .unwrap();
        let debug = format!("{download:?}");
        assert!(!debug.contains("super-secret"));
        assert!(debug.contains("%3Credacted%3E") || debug.contains("<redacted>"));
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
        for single_token in [
            "true",
            "42",
            "\"string\"",
            "<div>hello</div>",
            "{\"ok\":true}",
            "<svg/>",
            "a\tb",
        ] {
            let item = ClipboardItem::new_string(single_token.to_owned());
            assert_eq!(clipboard_text_payload_from_item(&item), None);
        }
    }

    #[test]
    fn native_markdown_precedes_other_plain_text_classifiers() {
        let item = ClipboardItem::new_string_with_markdown(
            "https://example.com/file.zip".to_owned(),
            "[download link](https://example.com/file.zip)".to_owned(),
        );
        let Some(ClipboardTextPayload::Materialization(materialization)) =
            clipboard_text_payload_from_item(&item)
        else {
            panic!("expected Markdown");
        };
        assert_eq!(materialization.file_name, "document.md");
        assert_eq!(
            materialization.contents,
            b"[download link](https://example.com/file.zip)"
        );
    }

    #[test]
    fn native_markdown_without_a_space_or_newline_is_rejected() {
        let item = ClipboardItem::new_string_with_markdown(
            "password".to_owned(),
            "**password**".to_owned(),
        );

        assert_eq!(clipboard_text_payload_from_item(&item), None);
        assert!(!clipboard_item_can_paste(Some(&item)));
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
    fn comma_separated_tables_materialize_as_csv_without_changing_source() {
        for source in ["Name,Age\nAda,36", "Name,Age\r\nAda,36"] {
            let item = ClipboardItem::new_string(source.to_owned());
            let Some(ClipboardTextPayload::Materialization(materialization)) =
                clipboard_text_payload_from_item(&item)
            else {
                panic!("expected CSV for {source:?}");
            };

            assert_eq!(materialization.file_name, "table.csv");
            assert_eq!(materialization.contents, source.as_bytes());
        }

        let source = "Name,Note\r\nAda,\"one,\r\ntwo\"\r\nLin,\"said \"\"hi\"\"\"";
        let item = ClipboardItem::new_string(source.to_owned());
        let Some(ClipboardTextPayload::Materialization(materialization)) =
            clipboard_text_payload_from_item(&item)
        else {
            panic!("expected quoted CSV");
        };

        assert_eq!(materialization.file_name, "table.csv");
        assert_eq!(materialization.contents, source.as_bytes());
    }

    #[test]
    fn comma_separated_detection_rejects_weak_or_malformed_tables() {
        for source in [
            "hello, world",
            "a,b\n1",
            "a,b\n1,\"unterminated",
            "a,b\n1,\"quoted\"suffix",
            "a;b\n1;2",
        ] {
            let item = ClipboardItem::new_string(source.to_owned());
            let Some(ClipboardTextPayload::Materialization(materialization)) =
                clipboard_text_payload_from_item(&item)
            else {
                panic!("expected text fallback for {source:?}");
            };

            assert_eq!(materialization.file_name, "text.txt", "source: {source:?}");
            assert_eq!(materialization.contents, source.as_bytes());
        }
    }

    #[test]
    fn json_precedes_comma_separated_detection() {
        let source = "[\n  [1, 2],\n  [3, 4]\n]";
        let item = ClipboardItem::new_string(source.to_owned());
        let Some(ClipboardTextPayload::Materialization(materialization)) =
            clipboard_text_payload_from_item(&item)
        else {
            panic!("expected JSON");
        };

        assert_eq!(materialization.file_name, "data.json");
        assert_eq!(materialization.contents, source.as_bytes());
    }

    #[test]
    fn svg_detection_accepts_complete_roots_and_rejects_nested_or_malformed_source() {
        for source in [
            "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
            " <?xml version=\"1.0\"?>\n<SVG viewBox=\"0 0 1 1\"></SVG> ",
            "<svg />",
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

    #[test]
    fn clipboard_summary_classifies_materialized_text_payloads() {
        for (item, expected) in [
            (
                ClipboardItem::new_string("{\"ok\": true}".to_owned()),
                "JSON file · 12 bytes",
            ),
            (
                ClipboardItem::new_string("a\tb\n1\t2".to_owned()),
                "CSV file · 8 bytes",
            ),
            (
                ClipboardItem::new_string("a,b\n1,2".to_owned()),
                "CSV file · 7 bytes",
            ),
            (
                ClipboardItem::new_string_with_markdown(
                    "Heading".to_owned(),
                    "# Heading".to_owned(),
                ),
                "MD file · 9 bytes",
            ),
            (
                ClipboardItem::new_string("<svg viewBox=\"0 0 1 1\"/>".to_owned()),
                "SVG vector file · 24 bytes",
            ),
            (
                ClipboardItem::new_string("ordinary prose".to_owned()),
                "Text file · 14 bytes",
            ),
        ] {
            assert_eq!(
                clipboard_summary_inspection(&item)
                    .expect("clipboard summary")
                    .summary
                    .label,
                expected
            );
        }
    }

    #[test]
    fn clipboard_summary_ignores_single_token_text() {
        for text in ["password", "a.txt", "{\"ok\":true}", "<svg/>", "a\tb"] {
            assert!(
                clipboard_summary_inspection(&ClipboardItem::new_string(text.to_owned())).is_none(),
                "unexpected summary for {text:?}"
            );
        }
    }

    #[test]
    fn clipboard_summary_classifies_images_and_url_batches() {
        let image = Image::from_bytes(ImageFormat::Png, vec![0; 200 * KB_BYTES as usize]);
        assert_eq!(
            clipboard_summary_inspection(&ClipboardItem::new_image(&image))
                .expect("image summary")
                .summary
                .label,
            "Image file · 200.0 KB"
        );

        let svg = Image::from_bytes(ImageFormat::Svg, b"<svg/>".to_vec());
        assert_eq!(
            clipboard_summary_inspection(&ClipboardItem::new_image(&svg))
                .expect("SVG image summary")
                .summary
                .label,
            "SVG vector file · 6 bytes"
        );

        let urls = ClipboardItem::new_string(
            "https://example.com/one.zip\nhttps://example.com/two.zip".to_owned(),
        );
        assert_eq!(
            clipboard_summary_inspection(&urls)
                .expect("URL summary")
                .summary
                .label,
            "2 URL downloads"
        );

        let youtube_urls = ClipboardItem::new_string(
            "https://youtu.be/dQw4w9WgXcQ\nhttps://youtube.com/watch?v=aqz-KE-bpKQ".to_owned(),
        );
        assert_eq!(
            clipboard_summary_inspection(&youtube_urls)
                .expect("YouTube URL summary")
                .summary
                .label,
            "Download 2 videos from multiple sites"
        );

        let vimeo_url = ClipboardItem::new_string("https://vimeo.com/76979871".to_owned());
        assert_eq!(
            clipboard_summary_inspection(&vimeo_url)
                .expect("single Vimeo URL summary")
                .summary
                .label,
            "Download video from vimeo.com"
        );

        let vimeo_urls = ClipboardItem::new_string(
            "https://vimeo.com/76979871\nhttps://player.vimeo.com/video/22439234".to_owned(),
        );
        assert_eq!(
            clipboard_summary_inspection(&vimeo_urls)
                .expect("Vimeo URL summary")
                .summary
                .label,
            "Download 2 videos from vimeo.com"
        );

        let mixed_video_urls = ClipboardItem::new_string(
            "https://vimeo.com/76979871\nhttps://www.dailymotion.com/video/x84sh87".to_owned(),
        );
        assert_eq!(
            clipboard_summary_inspection(&mixed_video_urls)
                .expect("mixed video URL summary")
                .summary
                .label,
            "Download 2 videos from multiple sites"
        );
    }

    #[test]
    fn clipboard_summary_details_keep_exact_urls_and_payload_actions() {
        let source = "sftp://alice:secret@example.com/files/archive.zip?token=visible";
        let summary = clipboard_summary_inspection(&ClipboardItem::new_string(source.to_owned()))
            .expect("download summary")
            .summary;
        let ClipboardSummaryDetails::Downloads { count, urls } = summary.details else {
            panic!("expected download details");
        };
        assert_eq!(count, 1);
        assert_eq!(urls.urls, vec![source]);
        assert_eq!(urls.omitted_count, 0);
        assert!(!urls.truncated);

        let image = Image::from_bytes(ImageFormat::Tiff, vec![1, 2, 3]);
        let image_summary = clipboard_summary_inspection(&ClipboardItem::new_image(&image))
            .expect("image summary")
            .summary;
        assert!(matches!(
            image_summary.details,
            ClipboardSummaryDetails::Image {
                source_format: ImageFormat::Tiff,
                output_file_name,
                byte_size: 3,
            } if output_file_name == "image.png"
        ));
    }

    #[test]
    fn clipboard_text_details_preview_raw_source_before_materialization() {
        let source = "Name\tNote\nAda\tone, two";
        let summary = clipboard_summary_inspection(&ClipboardItem::new_string(source.to_owned()))
            .expect("TSV summary")
            .summary;
        let ClipboardSummaryDetails::Materialization {
            output_file_name,
            source_size,
            output_size,
            source_preview,
        } = summary.details
        else {
            panic!("expected materialization details");
        };
        assert_eq!(output_file_name, "table.csv");
        assert_eq!(source_size, source.len() as u64);
        assert_ne!(source_size, output_size);
        assert_eq!(source_preview.lines, vec!["Name\tNote", "Ada\tone, two"]);
        assert!(!source_preview.truncated);

        let markdown = ClipboardItem::new_string_with_markdown(
            "rendered fallback".to_owned(),
            "# Raw markdown".to_owned(),
        );
        let summary = clipboard_summary_inspection(&markdown)
            .expect("Markdown summary")
            .summary;
        let ClipboardSummaryDetails::Materialization { source_preview, .. } = summary.details
        else {
            panic!("expected Markdown details");
        };
        assert_eq!(source_preview.lines, vec!["# Raw markdown"]);
    }

    #[test]
    fn clipboard_detail_previews_are_utf8_safe_and_explicitly_bounded() {
        let text = (0..10)
            .map(|index| format!("líne {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let preview = clipboard_text_preview(&text);
        assert_eq!(preview.lines.len(), CLIPBOARD_DETAIL_MAX_PREVIEW_LINES);
        assert!(preview.truncated);

        let long_url = format!("https://example.com/{}.zip", "é".repeat(2_000));
        let urls = clipboard_url_preview(&long_url);
        assert_eq!(urls.urls.len(), 1);
        assert!(urls.urls[0].len() <= CLIPBOARD_DETAIL_MAX_PREVIEW_BYTES);
        assert!(urls.urls[0].is_char_boundary(urls.urls[0].len()));
        assert!(urls.truncated);

        let many_urls = (0..7)
            .map(|index| format!("https://example.com/{index}.zip"))
            .collect::<Vec<_>>()
            .join("\n");
        let urls = clipboard_url_preview(&many_urls);
        assert_eq!(urls.urls.len(), CLIPBOARD_DETAIL_MAX_URLS);
        assert_eq!(urls.omitted_count, 2);
    }

    #[test]
    fn clipboard_summary_uses_existing_size_precision() {
        assert_eq!(
            clipboard_typed_summary_label("Image file", 200 * KB_BYTES),
            "Image file · 200.0 KB"
        );
        assert_eq!(
            clipboard_typed_summary_label("Image file", 2 * MB_BYTES),
            "Image file · 2.00 MB"
        );
    }

    #[test]
    fn clipboard_filesystem_summary_counts_top_level_items_and_recurses_for_size() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        let nested = folder.join("nested");
        fs::create_dir(&folder).expect("create folder");
        fs::create_dir(&nested).expect("create nested folder");
        fs::write(nested.join("nested.bin"), vec![0; 5]).expect("write nested file");
        let file = temp.path().join("top.bin");
        fs::write(&file, vec![0; 7]).expect("write top-level file");
        let cancel = Arc::new(AtomicBool::new(false));

        let paths = vec![folder.clone(), file];
        let metadata = scan_clipboard_filesystem_metadata(&paths, cancel.as_ref())
            .expect("clipboard metadata");
        assert_eq!(metadata.folder_count, 1);
        assert_eq!(metadata.file_count, 1);
        assert_eq!(metadata.file_size, 7);
        assert_eq!(
            clipboard_filesystem_summary_label(1, 1, None),
            "1 folder, 1 file"
        );

        let total =
            scan_clipboard_folder_sizes(&metadata.folder_paths, metadata.file_size, &cancel)
                .expect("recursive clipboard size");
        assert_eq!(total, 12);
        assert_eq!(
            clipboard_filesystem_summary_label(1, 1, Some(total)),
            "1 folder, 1 file · 12 bytes"
        );

        let recursive =
            scan_clipboard_recursive_counts(&paths, &cancel).expect("recursive clipboard counts");
        assert_eq!(recursive.folder_count, 2);
        assert_eq!(recursive.file_count, 2);
    }

    #[test]
    fn clipboard_filesystem_summary_reports_unavailable_and_cancelled_scans() {
        let cancel = Arc::new(AtomicBool::new(false));
        assert_eq!(
            scan_clipboard_filesystem_metadata(
                &[PathBuf::from("missing-clipboard-summary-item")],
                cancel.as_ref(),
            ),
            Err(ClipboardSummaryScanError::Unavailable)
        );

        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            scan_clipboard_filesystem_metadata(&[PathBuf::from("ignored")], cancel.as_ref()),
            Err(ClipboardSummaryScanError::Cancelled)
        );
        assert_eq!(
            scan_clipboard_folder_sizes(&[PathBuf::from("ignored")], 0, &cancel),
            Err(ClipboardSummaryScanError::Cancelled)
        );
        assert_eq!(
            scan_clipboard_recursive_counts(&[PathBuf::from("ignored")], &cancel),
            Err(ClipboardSummaryScanError::Cancelled)
        );
    }

    #[cfg(unix)]
    #[test]
    fn clipboard_filesystem_summary_does_not_follow_directory_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        fs::create_dir(&folder).expect("create folder");
        fs::write(folder.join("large.bin"), vec![0; 1024]).expect("write target file");
        let link = temp.path().join("folder-link");
        symlink(&folder, &link).expect("create directory symlink");
        let cancel = AtomicBool::new(false);

        let metadata = scan_clipboard_filesystem_metadata(&[link], &cancel)
            .expect("symlink clipboard metadata");
        assert_eq!(metadata.folder_count, 0);
        assert_eq!(metadata.file_count, 1);
        assert!(metadata.file_size < 1024);
    }

    #[gpui::test]
    fn stale_clipboard_summary_updates_are_rejected(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            initialize_clipboard_summary(app);
            let current = ClipboardFingerprint::Text {
                byte_len: 1,
                digest: 1,
            };
            app.update_global::<ClipboardSummaryState, _>(|state, _| {
                state.generation = 2;
                state.fingerprint = Some(current);
            });

            assert!(!update_clipboard_summary_if_current(
                app,
                1,
                &ClipboardFingerprint::Text {
                    byte_len: 1,
                    digest: 1,
                },
                ClipboardSummary {
                    label: "stale".to_owned(),
                    details: ClipboardSummaryDetails::Materialization {
                        output_file_name: "text.txt",
                        source_size: 5,
                        output_size: 5,
                        source_preview: ClipboardTextPreview {
                            lines: vec!["stale".to_owned()],
                            truncated: false,
                        },
                    },
                },
            ));
            assert!(app.global::<ClipboardSummaryState>().summary.is_none());
        });
    }
}
