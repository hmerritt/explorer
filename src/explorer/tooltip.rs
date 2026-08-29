use std::{path::Path, time::Duration};

use gpui::{
    Animation, AnimationExt as _, AnyElement, AnyView, App, AppContext as _, Context, FontWeight,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Render, SharedString,
    Styled as _, StyledImage as _, TextRun, Window, div, font, img, prelude::FluentBuilder as _,
    px, relative, rgb,
};
use thousands::Separable as _;

use crate::explorer::{
    clipboard::{
        ClipboardFileSourcePreview, ClipboardMetric, ClipboardSourceCounts, ClipboardSummary,
        ClipboardSummaryDetails, ClipboardTextPreview, ClipboardUrlPreview, FileClipboardOperation,
    },
    formatting::format_size,
    icons::{PASTE_ICON, image_icon},
};

const TOOLTIP_FADE_MS: u64 = 80;
const TOOLTIP_MAX_WIDTH: f32 = 260.0;
const CLIPBOARD_POPUP_RADIUS: f32 = 15.0;
const CLIPBOARD_POPUP_PRIMARY_TEXT: u32 = 0x1f1f1f;
const CLIPBOARD_POPUP_SECONDARY_TEXT: u32 = 0x595959;
const CLIPBOARD_POPUP_TERTIARY_TEXT: u32 = 0x767676;
const CLIPBOARD_POPUP_PREVIEW_BG: u32 = 0xf5f5f5;
const CLIPBOARD_POPUP_HORIZONTAL_PADDING: f32 = 12.0;
const CLIPBOARD_POPUP_HEADER_ICON_SIZE: f32 = 16.0;
const CLIPBOARD_POPUP_HEADER_GAP: f32 = 8.0;
const CLIPBOARD_POPUP_HEADER_TEXT_SIZE: f32 = 13.0;
const CLIPBOARD_DETAIL_LABEL_WIDTH: f32 = 78.0;
const CLIPBOARD_DETAIL_ROW_GAP: f32 = 8.0;
const CLIPBOARD_DETAIL_TEXT_SIZE: f32 = 12.0;
const CLIPBOARD_SOURCE_TREE_PREFIX_WIDTH: f32 = 28.0;
const CLIPBOARD_SOURCE_TREE_LINE_HEIGHT: f32 = 16.0;
const CLIPBOARD_PREVIEW_HORIZONTAL_PADDING: f32 = 8.0;
const CLIPBOARD_PREVIEW_TEXT_SIZE: f32 = 11.0;
const CLIPBOARD_IMAGE_PREVIEW_MAX_HEIGHT: f32 = 150.0;

pub(super) struct ExplorerTooltip {
    label: SharedString,
}

impl ExplorerTooltip {
    pub(super) fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

impl Render for ExplorerTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        explorer_tooltip_element(self.label.clone())
    }
}

pub(super) fn explorer_tooltip_element(label: impl Into<SharedString>) -> impl IntoElement {
    div()
        .id("explorer-tooltip")
        .debug_selector(|| "explorer-tooltip".to_owned())
        .max_w(px(TOOLTIP_MAX_WIDTH))
        .px(px(7.0))
        .py(px(4.0))
        .rounded(px(2.0))
        .border_1()
        .border_color(rgb(0x767676))
        .bg(rgb(0xffffff))
        .shadow_md()
        .text_size(px(12.0))
        .line_height(px(16.0))
        .text_color(rgb(0x1f1f1f))
        .child(label.into())
        .with_animation(
            "explorer-tooltip-fade",
            Animation::new(Duration::from_millis(TOOLTIP_FADE_MS)),
            |this, delta| this.opacity(delta),
        )
}

pub(crate) fn explorer_tooltip(
    label: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView {
    let label = label.into();
    move |_, cx| {
        let label = label.clone();
        cx.new(|_| ExplorerTooltip::new(label)).into()
    }
}

pub(super) fn clipboard_status_popup(
    summary: ClipboardSummary,
    destination: &Path,
    destination_label: String,
    can_paste: bool,
    width: f32,
    max_height: f32,
    popup_font: &gpui::Font,
    window: &Window,
) -> AnyElement {
    let destination_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(&destination_label);
    let action = clipboard_action_label(&summary, destination_name, can_paste);

    div()
        .id("clipboard-status-popup")
        .debug_selector(|| "clipboard-status-popup".to_owned())
        .flex()
        .flex_col()
        .w(px(width))
        .max_h(px(max_height))
        .overflow_hidden()
        .rounded(px(CLIPBOARD_POPUP_RADIUS))
        .bg(rgb(0xffffff))
        .shadow_md()
        .text_color(rgb(CLIPBOARD_POPUP_PRIMARY_TEXT))
        .child(
            div()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(CLIPBOARD_POPUP_HEADER_GAP))
                .px(px(CLIPBOARD_POPUP_HORIZONTAL_PADDING))
                .pt(px(10.0))
                .pb(px(8.0))
                .child(div().flex_shrink_0().pt(px(1.0)).child(image_icon(
                    PASTE_ICON.clone(),
                    CLIPBOARD_POPUP_HEADER_ICON_SIZE,
                    CLIPBOARD_POPUP_HEADER_ICON_SIZE,
                )))
                .child(
                    div()
                        .id("clipboard-popup-action")
                        .debug_selector(|| "clipboard-popup-action".to_owned())
                        .min_w(px(0.0))
                        .text_size(px(CLIPBOARD_POPUP_HEADER_TEXT_SIZE))
                        .line_height(px(18.0))
                        .font_weight(FontWeight::MEDIUM)
                        .child(action),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .px(px(CLIPBOARD_POPUP_HORIZONTAL_PADDING))
                .py(px(9.0))
                .text_size(px(CLIPBOARD_DETAIL_TEXT_SIZE))
                .line_height(px(16.0))
                .child(render_clipboard_summary_details(
                    summary.details,
                    destination_label,
                    width,
                    popup_font,
                    window,
                )),
        )
        .with_animation(
            "clipboard-status-popup-fade",
            Animation::new(Duration::from_millis(TOOLTIP_FADE_MS)),
            |this, delta| this.opacity(delta),
        )
        .into_any_element()
}

fn clipboard_action_label(
    summary: &ClipboardSummary,
    destination_name: &str,
    can_paste: bool,
) -> String {
    if !can_paste {
        return "Paste unavailable in this location".to_owned();
    }
    match &summary.details {
        ClipboardSummaryDetails::Files { operation, .. } => {
            let verb = match operation {
                FileClipboardOperation::Copy => "Copy",
                FileClipboardOperation::Cut => "Move",
            };
            format!("{verb} clipboard items to {destination_name}")
        }
        ClipboardSummaryDetails::Image {
            output_file_name, ..
        } => format!("Create {output_file_name} in {destination_name}"),
        ClipboardSummaryDetails::Downloads { count, .. } => format!(
            "Download {} to {destination_name}",
            count_label(*count, "file", "files")
        ),
        ClipboardSummaryDetails::VideoDownloads { count, .. } => format!(
            "Download {} to {destination_name}",
            count_label(*count, "video", "videos")
        ),
        ClipboardSummaryDetails::Materialization {
            output_file_name, ..
        } => format!("Create {output_file_name} in {destination_name}"),
    }
}

pub(super) fn clipboard_status_popup_preferred_width(
    summary: &ClipboardSummary,
    destination: &Path,
    destination_label: &str,
    can_paste: bool,
    popup_font: &gpui::Font,
    window: &Window,
) -> f32 {
    let destination_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(destination_label);
    let action = clipboard_action_label(summary, destination_name, can_paste);
    let mut header_font = popup_font.clone();
    header_font.weight = FontWeight::MEDIUM;
    let mut preferred_width = CLIPBOARD_POPUP_HORIZONTAL_PADDING * 2.0
        + CLIPBOARD_POPUP_HEADER_ICON_SIZE
        + CLIPBOARD_POPUP_HEADER_GAP
        + popup_text_width_at_size(
            &action,
            &header_font,
            CLIPBOARD_POPUP_HEADER_TEXT_SIZE,
            window,
        );

    preferred_width = preferred_width.max(detail_value_required_width(
        destination_label,
        popup_font,
        window,
    ));

    match &summary.details {
        ClipboardSummaryDetails::Files {
            operation,
            source_preview,
            folder_count,
            file_count,
            total_size,
        } => {
            preferred_width = preferred_width.max(detail_value_required_width(
                file_operation_label(*operation),
                popup_font,
                window,
            ));
            for path in &source_preview.paths {
                let path = normalized_clipboard_path(path);
                let path_width =
                    popup_text_width_at_size(&path, popup_font, CLIPBOARD_DETAIL_TEXT_SIZE, window);
                let row_chrome = CLIPBOARD_POPUP_HORIZONTAL_PADDING * 2.0
                    + CLIPBOARD_DETAIL_LABEL_WIDTH
                    + CLIPBOARD_DETAIL_ROW_GAP
                    + if source_preview.source_count > 1 {
                        CLIPBOARD_SOURCE_TREE_PREFIX_WIDTH
                    } else {
                        0.0
                    };
                preferred_width = preferred_width.max(row_chrome + path_width);
            }
            if let Some(counts) = &source_preview.omitted_counts {
                let omitted_count = source_preview
                    .source_count
                    .saturating_sub(source_preview.paths.len());
                preferred_width = preferred_width.max(source_tree_required_width(
                    &omitted_source_label(counts.clone(), omitted_count),
                    popup_font,
                    window,
                ));
            }
            preferred_width = preferred_width.max(detail_value_required_width(
                &items_metric_label(folder_count.clone(), file_count.clone()),
                popup_font,
                window,
            ));
            preferred_width = preferred_width.max(detail_value_required_width(
                &size_metric_label(total_size.clone()),
                popup_font,
                window,
            ));
        }
        ClipboardSummaryDetails::Image {
            output_file_name,
            byte_size,
            ..
        } => {
            for value in [
                format!("{output_file_name} (or next available name)"),
                format_size(Some(*byte_size)),
            ] {
                preferred_width =
                    preferred_width.max(detail_value_required_width(&value, popup_font, window));
            }
        }
        ClipboardSummaryDetails::Downloads { count, urls } => {
            for value in [
                count_label(*count, "URL", "URLs"),
                "Unknown until download".to_owned(),
            ] {
                preferred_width =
                    preferred_width.max(detail_value_required_width(&value, popup_font, window));
            }
            preferred_width = preferred_width.max(url_preview_required_width(urls, window));
        }
        ClipboardSummaryDetails::VideoDownloads {
            count,
            site_summary,
            urls,
        } => {
            for value in [
                count_label(*count, "video URL", "video URLs"),
                site_summary.clone(),
                "Unknown until download".to_owned(),
            ] {
                preferred_width =
                    preferred_width.max(detail_value_required_width(&value, popup_font, window));
            }
            preferred_width = preferred_width.max(url_preview_required_width(urls, window));
        }
        ClipboardSummaryDetails::Materialization {
            output_file_name,
            source_size,
            output_size,
            source_preview,
        } => {
            for value in [
                format!("{output_file_name} (or next available name)"),
                format_size(Some(*output_size)),
            ] {
                preferred_width =
                    preferred_width.max(detail_value_required_width(&value, popup_font, window));
            }
            if source_size != output_size {
                preferred_width = preferred_width.max(detail_value_required_width(
                    &format_size(Some(*source_size)),
                    popup_font,
                    window,
                ));
            }
            preferred_width =
                preferred_width.max(text_preview_required_width(source_preview, window));
        }
    }

    preferred_width.ceil()
}

fn render_clipboard_summary_details(
    details: ClipboardSummaryDetails,
    destination_label: String,
    popup_width: f32,
    popup_font: &gpui::Font,
    window: &Window,
) -> AnyElement {
    match details {
        ClipboardSummaryDetails::Files {
            operation,
            source_preview,
            folder_count,
            file_count,
            total_size,
        } => div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(clipboard_detail_row(
                "Action",
                file_operation_label(operation),
                "clipboard-popup-operation",
            ))
            .child(clipboard_detail_row(
                "Destination",
                destination_label,
                "clipboard-popup-destination",
            ))
            .child(render_file_source_preview(
                source_preview,
                popup_width,
                popup_font,
                window,
            ))
            .child(clipboard_detail_row(
                "Items",
                items_metric_label(folder_count, file_count),
                "clipboard-popup-items",
            ))
            .child(clipboard_detail_row(
                "Total size",
                size_metric_label(total_size),
                "clipboard-popup-total-size",
            ))
            .into_any_element(),
        ClipboardSummaryDetails::Image {
            preview,
            output_file_name,
            byte_size,
        } => div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(clipboard_detail_row(
                "Destination",
                destination_label,
                "clipboard-popup-destination",
            ))
            .child(clipboard_detail_row(
                "Output",
                format!("{output_file_name} (or next available name)"),
                "clipboard-popup-output",
            ))
            .child(clipboard_detail_row(
                "Size",
                format_size(Some(byte_size)),
                "clipboard-popup-total-size",
            ))
            .child(render_clipboard_image_preview(preview))
            .into_any_element(),
        ClipboardSummaryDetails::Downloads { count, urls } => div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(clipboard_detail_row(
                "Destination",
                destination_label,
                "clipboard-popup-destination",
            ))
            .child(clipboard_detail_row(
                "Contents",
                count_label(count, "URL", "URLs"),
                "clipboard-popup-url-count",
            ))
            .child(clipboard_detail_row(
                "Total size",
                "Unknown until download",
                "clipboard-popup-total-size",
            ))
            .child(render_url_preview(urls))
            .into_any_element(),
        ClipboardSummaryDetails::VideoDownloads {
            site_summary, urls, ..
        } => div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(clipboard_detail_row(
                "Destination",
                destination_label,
                "clipboard-popup-destination",
            ))
            .child(clipboard_detail_row(
                "Site",
                site_summary,
                "clipboard-popup-video-site",
            ))
            .child(clipboard_detail_row(
                "Total size",
                "Unknown until download",
                "clipboard-popup-total-size",
            ))
            .child(render_url_preview(urls))
            .into_any_element(),
        ClipboardSummaryDetails::Materialization {
            output_file_name,
            source_size,
            output_size,
            source_preview,
        } => div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(clipboard_detail_row(
                "Destination",
                destination_label,
                "clipboard-popup-destination",
            ))
            .child(clipboard_detail_row(
                "Output",
                format!("{output_file_name} (or next available name)"),
                "clipboard-popup-output",
            ))
            .child(clipboard_detail_row(
                "Total size",
                format_size(Some(output_size)),
                "clipboard-popup-total-size",
            ))
            .when(source_size != output_size, |this| {
                this.child(clipboard_detail_row(
                    "Source size",
                    format_size(Some(source_size)),
                    "clipboard-popup-source-size",
                ))
            })
            .child(render_text_preview(source_preview))
            .into_any_element(),
    }
}

fn render_clipboard_image_preview(image: std::sync::Arc<gpui::Image>) -> AnyElement {
    div()
        .id("clipboard-popup-image-preview-container")
        .debug_selector(|| "clipboard-popup-image-preview-container".to_owned())
        .flex()
        .w_full()
        .justify_center()
        .child(
            img(image)
                .id("clipboard-popup-image-preview")
                .debug_selector(|| "clipboard-popup-image-preview".to_owned())
                .max_w(relative(1.0))
                .max_h(px(CLIPBOARD_IMAGE_PREVIEW_MAX_HEIGHT))
                .object_fit(ObjectFit::Contain)
                .with_fallback(|| div().into_any_element()),
        )
        .into_any_element()
}

fn file_operation_label(operation: FileClipboardOperation) -> &'static str {
    match operation {
        FileClipboardOperation::Copy => "Copy",
        FileClipboardOperation::Cut => "Move",
    }
}

fn render_file_source_preview(
    preview: ClipboardFileSourcePreview,
    popup_width: f32,
    popup_font: &gpui::Font,
    window: &Window,
) -> AnyElement {
    let value_width = (popup_width
        - CLIPBOARD_POPUP_HORIZONTAL_PADDING * 2.0
        - CLIPBOARD_DETAIL_LABEL_WIDTH
        - CLIPBOARD_DETAIL_ROW_GAP)
        .max(0.0);

    if preview.source_count == 1 {
        let path = preview
            .paths
            .first()
            .map(|path| normalized_clipboard_path(path))
            .unwrap_or_default();
        return clipboard_detail_row(
            "Source",
            middle_ellipsized_text(&path, value_width, popup_font, window),
            "clipboard-popup-source",
        );
    }

    let path_width = (value_width - CLIPBOARD_SOURCE_TREE_PREFIX_WIDTH).max(0.0);
    let mut sources = div()
        .id("clipboard-popup-source-list")
        .debug_selector(|| "clipboard-popup-source-list".to_owned())
        .flex()
        .flex_col()
        .min_w(px(0.0))
        .gap(px(2.0))
        .child(
            div()
                .id("clipboard-popup-source-multiple")
                .debug_selector(|| "clipboard-popup-source-multiple".to_owned())
                .italic()
                .child("multiple"),
        );

    for (index, path) in preview.paths.iter().enumerate() {
        let path = normalized_clipboard_path(path);
        let path = middle_ellipsized_text(&path, path_width, popup_font, window);
        sources = sources.child(clipboard_source_tree_row(
            path,
            SharedString::from(format!("clipboard-popup-source-path-{index}")),
        ));
    }

    if let Some(counts) = preview.omitted_counts {
        let omitted_count = preview.source_count.saturating_sub(preview.paths.len());
        sources = sources.child(clipboard_source_tree_row(
            SharedString::from(omitted_source_label(counts, omitted_count)),
            SharedString::from("clipboard-popup-source-overflow"),
        ));
    }

    clipboard_detail_element(
        file_source_label(preview.source_count),
        sources,
        "clipboard-popup-source",
    )
}

fn file_source_label(source_count: usize) -> &'static str {
    if source_count > 1 {
        "Sources"
    } else {
        "Source"
    }
}

fn normalized_clipboard_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn clipboard_source_tree_row(value: SharedString, selector: SharedString) -> AnyElement {
    let debug_selector = selector.clone();
    div()
        .id(selector)
        .debug_selector(move || debug_selector.to_string())
        .flex()
        .flex_row()
        .items_center()
        .min_w(px(0.0))
        .h(px(CLIPBOARD_SOURCE_TREE_LINE_HEIGHT))
        .line_height(px(CLIPBOARD_SOURCE_TREE_LINE_HEIGHT))
        .child(
            div()
                .w(px(CLIPBOARD_SOURCE_TREE_PREFIX_WIDTH))
                .flex_shrink_0()
                .text_color(rgb(CLIPBOARD_POPUP_TERTIARY_TEXT))
                .child("└──"),
        )
        .child(div().min_w(px(0.0)).whitespace_nowrap().child(value))
        .into_any_element()
}

fn omitted_source_label(
    counts: ClipboardMetric<ClipboardSourceCounts>,
    omitted_count: usize,
) -> String {
    match counts {
        ClipboardMetric::Pending { .. } => "+ counting…".to_owned(),
        ClipboardMetric::Ready(counts) => format!(
            "+ {}",
            item_counts_label(counts.folder_count, counts.file_count)
        ),
        ClipboardMetric::Unavailable => format!(
            "+ {} (types unavailable)",
            count_label(omitted_count, "item", "items")
        ),
    }
}

fn middle_ellipsized_text(
    text: &str,
    available_width: f32,
    popup_font: &gpui::Font,
    window: &Window,
) -> SharedString {
    if text.is_empty() || popup_text_width(text, popup_font, window) <= available_width {
        return SharedString::from(text.to_owned());
    }

    let chars = text.chars().collect::<Vec<_>>();
    let mut best = "…".to_owned();
    let mut low = 0usize;
    let mut high = chars.len();
    while low <= high {
        let visible = low + (high - low) / 2;
        let candidate = middle_ellipsis_candidate(&chars, visible);
        if popup_text_width(&candidate, popup_font, window) <= available_width {
            best = candidate;
            low = visible.saturating_add(1);
        } else if visible == 0 {
            break;
        } else {
            high = visible - 1;
        }
    }
    SharedString::from(best)
}

fn middle_ellipsis_candidate(chars: &[char], visible: usize) -> String {
    if visible >= chars.len() {
        return chars.iter().collect();
    }
    if visible == 0 {
        return "…".to_owned();
    }

    let prefix_count = if visible >= 2 {
        (visible / 3).max(1)
    } else {
        0
    };
    let suffix_count = visible.saturating_sub(prefix_count);
    chars[..prefix_count]
        .iter()
        .chain(['…'].iter())
        .chain(chars[chars.len() - suffix_count..].iter())
        .collect()
}

fn popup_text_width(text: &str, popup_font: &gpui::Font, window: &Window) -> f32 {
    popup_text_width_at_size(text, popup_font, CLIPBOARD_DETAIL_TEXT_SIZE, window)
}

fn popup_text_width_at_size(
    text: &str,
    popup_font: &gpui::Font,
    text_size: f32,
    window: &Window,
) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let run = TextRun {
        len: text.len(),
        font: popup_font.clone(),
        color: rgb(CLIPBOARD_POPUP_SECONDARY_TEXT).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    f32::from(
        window
            .text_system()
            .layout_line(text, px(text_size), &[run], None)
            .width,
    )
}

fn detail_value_required_width(value: &str, popup_font: &gpui::Font, window: &Window) -> f32 {
    CLIPBOARD_POPUP_HORIZONTAL_PADDING * 2.0
        + CLIPBOARD_DETAIL_LABEL_WIDTH
        + CLIPBOARD_DETAIL_ROW_GAP
        + popup_text_width(value, popup_font, window)
}

fn source_tree_required_width(value: &str, popup_font: &gpui::Font, window: &Window) -> f32 {
    detail_value_required_width(value, popup_font, window) + CLIPBOARD_SOURCE_TREE_PREFIX_WIDTH
}

fn preview_line_required_width(value: &str, window: &Window) -> f32 {
    let preview_font = clipboard_preview_font();
    CLIPBOARD_POPUP_HORIZONTAL_PADDING * 2.0
        + CLIPBOARD_PREVIEW_HORIZONTAL_PADDING * 2.0
        + popup_text_width_at_size(value, &preview_font, CLIPBOARD_PREVIEW_TEXT_SIZE, window)
}

fn url_preview_required_width(preview: &ClipboardUrlPreview, window: &Window) -> f32 {
    let mut width = preview
        .urls
        .iter()
        .map(|url| preview_line_required_width(url, window))
        .fold(0.0, f32::max);
    if preview.omitted_count > 0 {
        width = width.max(preview_line_required_width(
            &format!(
                "+ {}",
                count_label(preview.omitted_count, "more URL", "more URLs")
            ),
            window,
        ));
    }
    if preview.truncated {
        width = width.max(preview_line_required_width(
            "… URL preview truncated",
            window,
        ));
    }
    width
}

fn text_preview_required_width(preview: &ClipboardTextPreview, window: &Window) -> f32 {
    let mut width = preview
        .lines
        .iter()
        .map(|line| preview_line_required_width(if line.is_empty() { " " } else { line }, window))
        .fold(0.0, f32::max);
    if preview.truncated {
        width = width.max(preview_line_required_width(
            "… raw clipboard text truncated",
            window,
        ));
    }
    width
}

fn clipboard_detail_row(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    selector: &'static str,
) -> AnyElement {
    clipboard_detail_element(label, div().min_w(px(0.0)).child(value.into()), selector)
}

fn clipboard_detail_element(
    label: impl Into<SharedString>,
    value: impl IntoElement,
    selector: &'static str,
) -> AnyElement {
    div()
        .id(selector)
        .debug_selector(move || selector.to_owned())
        .flex()
        .flex_row()
        .items_start()
        .gap(px(CLIPBOARD_DETAIL_ROW_GAP))
        .child(
            div()
                .w(px(CLIPBOARD_DETAIL_LABEL_WIDTH))
                .flex_shrink_0()
                .text_color(rgb(CLIPBOARD_POPUP_TERTIARY_TEXT))
                .child(label.into()),
        )
        .child(
            div()
                .min_w(px(0.0))
                .text_color(rgb(CLIPBOARD_POPUP_SECONDARY_TEXT))
                .child(value),
        )
        .into_any_element()
}

fn render_url_preview(preview: ClipboardUrlPreview) -> AnyElement {
    let mut content = div()
        .id("clipboard-popup-url-preview")
        .debug_selector(|| "clipboard-popup-url-preview".to_owned())
        .flex()
        .flex_col()
        .gap(px(2.0))
        .mt(px(2.0))
        .px(px(CLIPBOARD_PREVIEW_HORIZONTAL_PADDING))
        .py(px(6.0))
        .rounded(px(2.0))
        .bg(rgb(CLIPBOARD_POPUP_PREVIEW_BG))
        .text_size(px(CLIPBOARD_PREVIEW_TEXT_SIZE))
        .line_height(px(15.0))
        .font(clipboard_preview_font())
        .text_color(rgb(CLIPBOARD_POPUP_SECONDARY_TEXT));
    for (index, url) in preview.urls.into_iter().enumerate() {
        content = content.child(
            div()
                .id(SharedString::from(format!("clipboard-popup-url-{index}")))
                .debug_selector(move || format!("clipboard-popup-url-{index}"))
                .min_w(px(0.0))
                .truncate()
                .child(url),
        );
    }
    if preview.omitted_count > 0 {
        content = content.child(div().text_color(rgb(CLIPBOARD_POPUP_TERTIARY_TEXT)).child(
            format!(
                "+ {}",
                count_label(preview.omitted_count, "more URL", "more URLs")
            ),
        ));
    }
    if preview.truncated {
        content = content.child(
            div()
                .text_color(rgb(CLIPBOARD_POPUP_TERTIARY_TEXT))
                .child("… URL preview truncated"),
        );
    }
    content.into_any_element()
}

fn render_text_preview(preview: ClipboardTextPreview) -> AnyElement {
    let mut content = div()
        .id("clipboard-popup-text-preview")
        .debug_selector(|| "clipboard-popup-text-preview".to_owned())
        .flex()
        .flex_col()
        .gap(px(1.0))
        .mt(px(2.0))
        .px(px(CLIPBOARD_PREVIEW_HORIZONTAL_PADDING))
        .py(px(6.0))
        .rounded(px(2.0))
        .bg(rgb(CLIPBOARD_POPUP_PREVIEW_BG))
        .text_size(px(CLIPBOARD_PREVIEW_TEXT_SIZE))
        .line_height(px(15.0))
        .font(clipboard_preview_font())
        .text_color(rgb(CLIPBOARD_POPUP_SECONDARY_TEXT));
    for (index, line) in preview.lines.into_iter().enumerate() {
        content = content.child(
            div()
                .id(SharedString::from(format!(
                    "clipboard-popup-text-line-{index}"
                )))
                .debug_selector(move || format!("clipboard-popup-text-line-{index}"))
                .min_w(px(0.0))
                .truncate()
                .child(if line.is_empty() { " " } else { &line }.to_owned()),
        );
    }
    if preview.truncated {
        content = content.child(
            div()
                .text_color(rgb(CLIPBOARD_POPUP_TERTIARY_TEXT))
                .child("… raw clipboard text truncated"),
        );
    }
    content.into_any_element()
}

fn items_metric_label(
    folder_count: ClipboardMetric<usize>,
    file_count: ClipboardMetric<usize>,
) -> String {
    if matches!(folder_count, ClipboardMetric::Unavailable)
        || matches!(file_count, ClipboardMetric::Unavailable)
    {
        return "Unavailable".to_owned();
    }
    if matches!(folder_count, ClipboardMetric::Pending { discovered: None })
        || matches!(file_count, ClipboardMetric::Pending { discovered: None })
    {
        return "Counting…".to_owned();
    }

    let (folders, folders_pending) = item_metric_component(folder_count, "folder", "folders");
    let (files, files_pending) = item_metric_component(file_count, "file", "files");
    let suffix = if folders_pending || files_pending {
        " (counting…)"
    } else {
        ""
    };
    format!("{folders}, {files}{suffix}")
}

fn item_metric_component(
    metric: ClipboardMetric<usize>,
    singular: &str,
    plural: &str,
) -> (String, bool) {
    match metric {
        ClipboardMetric::Pending {
            discovered: Some(count),
        } => (
            format!(
                "{}+ {}",
                count.separate_with_commas(),
                if count == 1 { singular } else { plural }
            ),
            true,
        ),
        ClipboardMetric::Ready(count) => (count_label(count, singular, plural), false),
        ClipboardMetric::Pending { discovered: None } | ClipboardMetric::Unavailable => {
            unreachable!("unresolved item metrics are handled before formatting")
        }
    }
}

fn item_counts_label(folder_count: usize, file_count: usize) -> String {
    format!(
        "{}, {}",
        count_label(folder_count, "folder", "folders"),
        count_label(file_count, "file", "files")
    )
}

fn size_metric_label(metric: ClipboardMetric<u64>) -> String {
    match metric {
        ClipboardMetric::Pending { .. } => "Calculating…".to_owned(),
        ClipboardMetric::Ready(size) => format_size(Some(size)),
        ClipboardMetric::Unavailable => "Unavailable".to_owned(),
    }
}

fn count_label(count: usize, singular: &str, plural: &str) -> String {
    let noun = if count == 1 { singular } else { plural };
    format!("{} {noun}", count.separate_with_commas())
}

fn clipboard_preview_font() -> gpui::Font {
    if cfg!(target_os = "windows") {
        font("Consolas")
    } else if cfg!(target_os = "macos") {
        font("SF Mono")
    } else {
        font("DejaVu Sans Mono")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_stores_label() {
        let tooltip = ExplorerTooltip::new("Refresh");
        assert_eq!(tooltip.label, SharedString::from("Refresh"));
    }

    #[test]
    fn clipboard_action_labels_match_payload_and_availability() {
        let files = ClipboardSummary {
            label: "1 file".to_owned(),
            details: ClipboardSummaryDetails::Files {
                operation: FileClipboardOperation::Cut,
                source_preview: ClipboardFileSourcePreview {
                    paths: vec![Path::new("C:/source/file.txt").to_path_buf()],
                    source_count: 1,
                    omitted_counts: None,
                },
                folder_count: ClipboardMetric::Ready(0),
                file_count: ClipboardMetric::Ready(1),
                total_size: ClipboardMetric::Ready(5),
            },
        };
        assert_eq!(
            clipboard_action_label(&files, "Downloads", true),
            "Move clipboard items to Downloads"
        );
        assert_eq!(
            clipboard_action_label(&files, "archive.zip", false),
            "Paste unavailable in this location"
        );

        let download = ClipboardSummary {
            label: "2 URL downloads".to_owned(),
            details: ClipboardSummaryDetails::Downloads {
                count: 2,
                urls: ClipboardUrlPreview {
                    urls: Vec::new(),
                    omitted_count: 0,
                    truncated: false,
                },
            },
        };
        assert_eq!(
            clipboard_action_label(&download, "Downloads", true),
            "Download 2 files to Downloads"
        );
    }

    #[test]
    fn clipboard_file_operation_labels_are_exact() {
        assert_eq!(file_operation_label(FileClipboardOperation::Copy), "Copy");
        assert_eq!(file_operation_label(FileClipboardOperation::Cut), "Move");
    }

    #[test]
    fn clipboard_source_labels_are_singular_and_plural() {
        assert_eq!(file_source_label(1), "Source");
        assert_eq!(file_source_label(2), "Sources");
        assert_eq!(file_source_label(20), "Sources");
    }

    #[test]
    fn clipboard_source_paths_use_forward_slashes_for_display_only() {
        for (source, expected) in [
            (r"C:\Folder\File.txt", "C:/Folder/File.txt"),
            (r"C:\Folder/mixed\File.txt", "C:/Folder/mixed/File.txt"),
            (
                r"\\server\share\Folder\File.txt",
                "//server/share/Folder/File.txt",
            ),
            ("/home/user/資料.txt", "/home/user/資料.txt"),
        ] {
            let path = Path::new(source).to_path_buf();
            let original = path.clone();
            assert_eq!(normalized_clipboard_path(&path), expected);
            assert_eq!(path, original);
        }
    }

    #[test]
    fn clipboard_item_labels_expose_staged_and_unavailable_states() {
        assert_eq!(
            items_metric_label(
                ClipboardMetric::Pending {
                    discovered: Some(1),
                },
                ClipboardMetric::Pending {
                    discovered: Some(1_234),
                },
            ),
            "1+ folder, 1,234+ files (counting…)"
        );
        assert_eq!(
            items_metric_label(
                ClipboardMetric::Pending { discovered: None },
                ClipboardMetric::Pending { discovered: None },
            ),
            "Counting…"
        );
        assert_eq!(
            items_metric_label(ClipboardMetric::Ready(1), ClipboardMetric::Ready(0)),
            "1 folder, 0 files"
        );
        assert_eq!(
            items_metric_label(ClipboardMetric::Unavailable, ClipboardMetric::Ready(3)),
            "Unavailable"
        );
        assert_eq!(
            size_metric_label(ClipboardMetric::Pending { discovered: None }),
            "Calculating…"
        );
        assert_eq!(
            size_metric_label(ClipboardMetric::Unavailable),
            "Unavailable"
        );
    }

    #[test]
    fn clipboard_source_overflow_labels_cover_each_stage() {
        assert_eq!(
            omitted_source_label(ClipboardMetric::Pending { discovered: None }, 4),
            "+ counting…"
        );
        assert_eq!(
            omitted_source_label(
                ClipboardMetric::Ready(ClipboardSourceCounts {
                    folder_count: 2,
                    file_count: 1,
                }),
                3,
            ),
            "+ 2 folders, 1 file"
        );
        assert_eq!(
            omitted_source_label(ClipboardMetric::Unavailable, 3),
            "+ 3 items (types unavailable)"
        );
    }

    #[test]
    fn middle_ellipsis_preserves_utf8_and_favors_the_final_component() {
        for (text, visible, expected_start, expected_end) in [
            (
                r"C:\source\資料\very-long-file-name.txt",
                12,
                r"C:\",
                "name.txt",
            ),
            ("/home/user/documents/report.txt", 9, "/ho", "rt.txt"),
            (r"\\server\share\folder\archive.zip", 10, r"\\s", "ve.zip"),
            ("single-very-long-filename.md", 6, "si", "e.md"),
            ("é資料.txt", 2, "é", "t"),
        ] {
            let chars = text.chars().collect::<Vec<_>>();
            let shortened = middle_ellipsis_candidate(&chars, visible);
            assert!(shortened.starts_with(expected_start), "{shortened:?}");
            assert!(shortened.ends_with(expected_end), "{shortened:?}");
            assert!(shortened.contains('…'));
            assert!(std::str::from_utf8(shortened.as_bytes()).is_ok());
            assert_eq!(
                middle_ellipsis_candidate(&chars, chars.len()),
                text.to_owned()
            );
        }

        assert_eq!(middle_ellipsis_candidate(&['a'], 0), "…");
    }
}
