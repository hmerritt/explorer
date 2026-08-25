use std::{path::Path, time::Duration};

use gpui::{
    Animation, AnimationExt as _, AnyElement, AnyView, App, AppContext as _, Context, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Window, div, font, prelude::FluentBuilder as _, px, rgb,
};
use thousands::Separable as _;

use crate::explorer::{
    clipboard::{
        ClipboardMetric, ClipboardSummary, ClipboardSummaryDetails, ClipboardTextPreview,
        ClipboardUrlPreview, FileClipboardOperation,
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
                .gap(px(8.0))
                .px(px(12.0))
                .pt(px(10.0))
                .pb(px(8.0))
                .child(div().flex_shrink_0().pt(px(1.0)).child(image_icon(
                    PASTE_ICON.clone(),
                    16.0,
                    16.0,
                )))
                .child(
                    div()
                        .id("clipboard-popup-action")
                        .debug_selector(|| "clipboard-popup-action".to_owned())
                        .min_w(px(0.0))
                        .text_size(px(13.0))
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
                .px(px(12.0))
                .py(px(9.0))
                .text_size(px(12.0))
                .line_height(px(16.0))
                .child(clipboard_detail_row(
                    "Destination",
                    destination_label,
                    "clipboard-popup-destination",
                ))
                .child(render_clipboard_summary_details(summary.details)),
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

fn render_clipboard_summary_details(details: ClipboardSummaryDetails) -> AnyElement {
    match details {
        ClipboardSummaryDetails::Files {
            operation,
            folder_count,
            file_count,
            total_size,
        } => div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(clipboard_detail_row(
                "Action",
                match operation {
                    FileClipboardOperation::Copy => "Copy files and folders",
                    FileClipboardOperation::Cut => "Move files and folders",
                },
                "clipboard-popup-operation",
            ))
            .child(clipboard_detail_row(
                "Folders",
                count_metric_label(folder_count),
                "clipboard-popup-folder-count",
            ))
            .child(clipboard_detail_row(
                "Files",
                count_metric_label(file_count),
                "clipboard-popup-file-count",
            ))
            .child(clipboard_detail_row(
                "Total size",
                size_metric_label(total_size),
                "clipboard-popup-total-size",
            ))
            .into_any_element(),
        ClipboardSummaryDetails::Image {
            source_format,
            output_file_name,
            byte_size,
        } => div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(clipboard_detail_row(
                "Clipboard type",
                image_format_label(source_format),
                "clipboard-popup-image-format",
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
            .child(clipboard_after_paste_note())
            .into_any_element(),
        ClipboardSummaryDetails::Downloads { count, urls } => div()
            .flex()
            .flex_col()
            .gap(px(6.0))
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
            count,
            site_summary,
            urls,
        } => div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(clipboard_detail_row(
                "Contents",
                count_label(count, "video URL", "video URLs"),
                "clipboard-popup-url-count",
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
            .child(clipboard_after_paste_note())
            .into_any_element(),
    }
}

fn clipboard_detail_row(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    selector: &'static str,
) -> AnyElement {
    div()
        .id(selector)
        .debug_selector(move || selector.to_owned())
        .flex()
        .flex_row()
        .items_start()
        .gap(px(8.0))
        .child(
            div()
                .w(px(78.0))
                .flex_shrink_0()
                .text_color(rgb(CLIPBOARD_POPUP_TERTIARY_TEXT))
                .child(label.into()),
        )
        .child(
            div()
                .min_w(px(0.0))
                .text_color(rgb(CLIPBOARD_POPUP_SECONDARY_TEXT))
                .child(value.into()),
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
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(2.0))
        .bg(rgb(CLIPBOARD_POPUP_PREVIEW_BG))
        .text_size(px(11.0))
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
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(2.0))
        .bg(rgb(CLIPBOARD_POPUP_PREVIEW_BG))
        .text_size(px(11.0))
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

fn clipboard_after_paste_note() -> AnyElement {
    div()
        .mt(px(2.0))
        .text_size(px(11.0))
        .text_color(rgb(CLIPBOARD_POPUP_TERTIARY_TEXT))
        .child("The new file will be selected for renaming.")
        .into_any_element()
}

fn count_metric_label(metric: ClipboardMetric<usize>) -> String {
    match metric {
        ClipboardMetric::Pending {
            discovered: Some(count),
        } => format!("{}+ (counting…)", count.separate_with_commas()),
        ClipboardMetric::Pending { discovered: None } => "Counting…".to_owned(),
        ClipboardMetric::Ready(count) => count.separate_with_commas(),
        ClipboardMetric::Unavailable => "Unavailable".to_owned(),
    }
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

fn image_format_label(format: gpui::ImageFormat) -> &'static str {
    match format {
        gpui::ImageFormat::Png => "PNG image",
        gpui::ImageFormat::Jpeg => "JPEG image",
        gpui::ImageFormat::Webp => "WebP image",
        gpui::ImageFormat::Gif => "GIF image",
        gpui::ImageFormat::Svg => "SVG vector image",
        gpui::ImageFormat::Bmp => "BMP image",
        gpui::ImageFormat::Tiff => "TIFF image (converted to PNG)",
    }
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
    fn clipboard_metric_labels_expose_staged_and_unavailable_states() {
        assert_eq!(
            count_metric_label(ClipboardMetric::Pending {
                discovered: Some(1_234),
            }),
            "1,234+ (counting…)"
        );
        assert_eq!(
            count_metric_label(ClipboardMetric::Pending { discovered: None }),
            "Counting…"
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
}
