use std::{fs, path::PathBuf, sync::Arc};

use gpui::{
    AnyElement, App, Bounds, Context, ObjectFit, ParentElement, Render, RenderImage, SharedString,
    Styled, Task, TitlebarOptions, Window, WindowBounds, WindowDecorations, WindowOptions, div,
    img, point, prelude::*, px, rgb, size,
};

use crate::{
    explorer::{
        constants::{
            STATUS_BAR_HEIGHT, STATUS_BAR_HORIZONTAL_PADDING, STATUS_BAR_SEPARATOR_COLOR,
            STATUS_BAR_TEXT_COLOR, STATUS_BAR_TEXT_SIZE,
        },
        explorer_tooltip, format_size,
    },
    image_viewer::{
        decode::{DecodedImage, decode_image_source},
        resize::{ImageFitTarget, fitted_image_target, render_image_for_target},
    },
    settings::APP_ID,
    window_chrome::{
        MAC_TRAFFIC_LIGHT_PADDING, TITLEBAR_HEIGHT, WindowDragState, render_platform_window_frame,
        render_titlebar_drag_region, render_window_controls,
    },
};

const IMAGE_VIEWER_WINDOW_WIDTH: f32 = 1024.0;
const IMAGE_VIEWER_WINDOW_HEIGHT: f32 = 820.0;
const IMAGE_VIEWER_MIN_WIDTH: f32 = 400.0;
const IMAGE_VIEWER_MIN_HEIGHT: f32 = 120.0;
const STATUS_TOOLTIP_RESOLUTION: &str = "Resolution";
const STATUS_TOOLTIP_SCALING: &str = "Render percentage of resolution";
const STATUS_TOOLTIP_SIZE: &str = "Size";
const STATUS_TOOLTIP_DECOMPRESSED_SIZE: &str = "Decompressed size";

pub(crate) fn open_image_window(path: PathBuf, cx: &mut App) {
    let title = SharedString::from(image_title(&path));
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(
                    px(IMAGE_VIEWER_WINDOW_WIDTH),
                    px(IMAGE_VIEWER_WINDOW_HEIGHT),
                ),
                cx,
            ))),
            window_min_size: Some(size(
                px(IMAGE_VIEWER_MIN_WIDTH),
                px(IMAGE_VIEWER_MIN_HEIGHT),
            )),
            titlebar: Some(TitlebarOptions {
                title: Some(title.clone()),
                appears_transparent: true,
                traffic_light_position: cfg!(target_os = "macos")
                    .then_some(point(px(12.0), px(11.0))),
                ..Default::default()
            }),
            window_decorations: Some(if cfg!(target_os = "linux") {
                WindowDecorations::Client
            } else {
                WindowDecorations::Server
            }),
            app_id: Some(APP_ID.to_owned()),
            focus: true,
            ..Default::default()
        },
        move |_, cx| cx.new(|cx| ImageViewer::new(path, title, cx)),
    )
    .expect("failed to open image viewer window");
}

struct ImageViewer {
    path: PathBuf,
    title: SharedString,
    file_size_bytes: Option<u64>,
    state: ImageViewerState,
    decode_generation: u64,
    decode_task: Option<Task<()>>,
    resize_generation: u64,
    resize_task: Option<Task<()>>,
    resize_pending: Option<ImageFitTarget>,
    scaled_image: Option<ScaledImage>,
    should_move_window: bool,
}

enum ImageViewerState {
    Loading,
    Ready(DecodedImage),
    Failed(String),
}

struct ScaledImage {
    target: ImageFitTarget,
    image: Arc<RenderImage>,
}

impl ImageViewer {
    fn new(path: PathBuf, title: SharedString, cx: &mut Context<Self>) -> Self {
        let file_size_bytes = image_file_size(&path);
        let mut viewer = Self {
            path,
            title,
            file_size_bytes,
            state: ImageViewerState::Loading,
            decode_generation: 0,
            decode_task: None,
            resize_generation: 0,
            resize_task: None,
            resize_pending: None,
            scaled_image: None,
            should_move_window: false,
        };
        viewer.start_decode(cx);
        viewer
    }

    fn start_decode(&mut self, cx: &mut Context<Self>) {
        self.decode_generation = self.decode_generation.wrapping_add(1);
        let generation = self.decode_generation;
        let path = self.path.clone();
        self.decode_task = Some(cx.spawn(async move |viewer, cx| {
            let worker = cx
                .background_executor()
                .spawn(async move { decode_image_source(&path) });
            let result = worker.await;
            let _ = viewer.update(cx, |viewer, cx| {
                if viewer.decode_generation != generation {
                    return;
                }

                viewer.decode_task = None;
                viewer.resize_pending = None;
                viewer.resize_task = None;
                viewer.drop_scaled_image(cx);
                viewer.state = match result {
                    Ok(decoded) => ImageViewerState::Ready(decoded),
                    Err(error) => ImageViewerState::Failed(error),
                };
                cx.notify();
            });
        }));
    }

    fn ensure_scaled_image(
        &mut self,
        decoded: DecodedImage,
        target: ImageFitTarget,
        cx: &mut Context<Self>,
    ) {
        if self
            .scaled_image
            .as_ref()
            .is_some_and(|scaled| scaled.target == target)
        {
            return;
        }
        if self.resize_pending == Some(target) {
            return;
        }

        self.resize_generation = self.resize_generation.wrapping_add(1);
        let generation = self.resize_generation;
        self.resize_pending = Some(target);
        self.resize_task = Some(cx.spawn(async move |viewer, cx| {
            let worker = cx
                .background_executor()
                .spawn(async move { render_image_for_target(&decoded, target) });
            let result = worker.await;
            let _ = viewer.update(cx, |viewer, cx| {
                if viewer.resize_generation != generation || viewer.resize_pending != Some(target) {
                    if let Ok(image) = result {
                        cx.drop_image(image, None);
                    }
                    return;
                }

                viewer.resize_task = None;
                viewer.resize_pending = None;
                match result {
                    Ok(image) => {
                        viewer.drop_scaled_image(cx);
                        viewer.scaled_image = Some(ScaledImage { target, image });
                    }
                    Err(error) => {
                        viewer.drop_scaled_image(cx);
                        viewer.state = ImageViewerState::Failed(error);
                    }
                }
                cx.notify();
            });
        }));
    }

    fn drop_scaled_image(&mut self, cx: &mut Context<Self>) {
        if let Some(scaled) = self.scaled_image.take() {
            cx.drop_image(scaled.image, None);
        }
    }

    fn render_titlebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let decorations = window.window_decorations();
        div()
            .id("image-viewer-titlebar")
            .flex()
            .flex_row()
            .items_center()
            .relative()
            .h(px(TITLEBAR_HEIGHT))
            .w_full()
            .flex_shrink_0()
            .overflow_hidden()
            .bg(rgb(0xe8e8e8))
            .when(
                cfg!(target_os = "macos") && !window.is_fullscreen(),
                |this| {
                    this.child(
                        div()
                            .id("image-viewer-macos-traffic-light-space")
                            .h_full()
                            .w(px(MAC_TRAFFIC_LIGHT_PADDING))
                            .flex_none()
                            .occlude(),
                    )
                },
            )
            .child(
                div()
                    .id("image-viewer-filename")
                    .h_full()
                    .max_w(px(420.0))
                    .min_w(px(0.0))
                    .flex()
                    .items_center()
                    .px(px(12.0))
                    .overflow_hidden()
                    .text_size(px(12.0))
                    .text_color(rgb(0x1f1f1f))
                    .child(self.title.clone()),
            )
            .child(render_titlebar_drag_region(
                "image-viewer-titlebar-drag-region",
                decorations,
                cx,
            ))
            .children(render_window_controls(window))
            .into_any_element()
    }

    fn render_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let viewport = window.viewport_size();
        let (available_width, available_height) =
            image_body_available_size(f32::from(viewport.width), f32::from(viewport.height));
        let scale_factor = window.scale_factor();
        let content = match &self.state {
            ImageViewerState::Loading => image_viewer_status("Loading image..."),
            ImageViewerState::Failed(error) => {
                image_viewer_status(format!("Cannot display {}: {error}", self.title))
            }
            ImageViewerState::Ready(decoded) => {
                let decoded = decoded.clone();
                let target = fitted_image_target(
                    decoded.width,
                    decoded.height,
                    available_width,
                    available_height,
                    scale_factor,
                );
                if let Some(target) = target {
                    self.ensure_scaled_image(decoded, target, cx);
                    if let Some(scaled) = self
                        .scaled_image
                        .as_ref()
                        .filter(|scaled| scaled.target == target)
                    {
                        img(scaled.image.clone())
                            .w(px(scaled.target.display_width))
                            .h(px(scaled.target.display_height))
                            .object_fit(ObjectFit::Contain)
                            .into_any_element()
                    } else {
                        image_viewer_status("Loading image...")
                    }
                } else {
                    image_viewer_status("Cannot display image.")
                }
            }
        };

        div()
            .id("image-viewer-body")
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .bg(rgb(0xffffff))
            .child(content)
            .into_any_element()
    }

    fn render_status_bar(&self, target: Option<ImageFitTarget>) -> AnyElement {
        let labels = image_status_labels(&self.state, self.file_size_bytes, target);

        div()
            .id("image-viewer-status-bar")
            .flex()
            .flex_row()
            .items_center()
            .h(px(STATUS_BAR_HEIGHT))
            .w_full()
            .flex_shrink_0()
            .overflow_hidden()
            .bg(rgb(0xffffff))
            .px(px(STATUS_BAR_HORIZONTAL_PADDING))
            .text_size(px(STATUS_BAR_TEXT_SIZE))
            .text_color(rgb(STATUS_BAR_TEXT_COLOR))
            .child(image_status_item(
                "image-viewer-status-resolution",
                labels.resolution,
                STATUS_TOOLTIP_RESOLUTION,
            ))
            .child(image_status_separator())
            .child(image_status_item(
                "image-viewer-status-scaling",
                labels.scaling,
                STATUS_TOOLTIP_SCALING,
            ))
            .child(image_status_separator())
            .child(image_status_item(
                "image-viewer-status-size",
                labels.file_size,
                STATUS_TOOLTIP_SIZE,
            ))
            .child(image_status_slash())
            .child(image_status_item(
                "image-viewer-status-decompressed-size",
                labels.decompressed_size,
                STATUS_TOOLTIP_DECOMPRESSED_SIZE,
            ))
            .into_any_element()
    }

    fn current_fit_target(&self, window: &Window) -> Option<ImageFitTarget> {
        let ImageViewerState::Ready(decoded) = &self.state else {
            return None;
        };

        let viewport = window.viewport_size();
        let (available_width, available_height) =
            image_body_available_size(f32::from(viewport.width), f32::from(viewport.height));
        fitted_image_target(
            decoded.width,
            decoded.height,
            available_width,
            available_height,
            window.scale_factor(),
        )
    }
}

impl WindowDragState for ImageViewer {
    fn set_window_drag_pending(&mut self, pending: bool) {
        self.should_move_window = pending;
    }

    fn take_window_drag_pending(&mut self) -> bool {
        let pending = self.should_move_window;
        self.should_move_window = false;
        pending
    }
}

impl Render for ImageViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(0xffffff))
            .child(self.render_titlebar(window, cx))
            .child(self.render_body(window, cx))
            .child(self.render_status_bar(self.current_fit_target(window)))
            .into_any_element();

        render_platform_window_frame(content, window)
    }
}

fn image_viewer_status(text: impl Into<SharedString>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .px(px(20.0))
        .text_size(px(13.0))
        .text_color(rgb(0x5f5f5f))
        .child(text.into())
        .into_any_element()
}

#[derive(Debug, Eq, PartialEq)]
struct ImageStatusLabels {
    resolution: String,
    scaling: String,
    file_size: String,
    decompressed_size: String,
}

fn image_status_labels(
    state: &ImageViewerState,
    file_size_bytes: Option<u64>,
    target: Option<ImageFitTarget>,
) -> ImageStatusLabels {
    match state {
        ImageViewerState::Ready(decoded) => ImageStatusLabels {
            resolution: format!("{} x {}", decoded.width, decoded.height),
            scaling: status_scaling_percent(decoded.width, target),
            file_size: status_file_size(file_size_bytes),
            decompressed_size: status_decompressed_size(decoded.source_decompressed_size_bytes),
        },
        ImageViewerState::Loading | ImageViewerState::Failed(_) => ImageStatusLabels {
            resolution: "--".to_owned(),
            scaling: "--".to_owned(),
            file_size: status_file_size(file_size_bytes),
            decompressed_size: status_decompressed_size(None),
        },
    }
}

fn status_scaling_percent(image_width: u32, target: Option<ImageFitTarget>) -> String {
    let Some(target) = target else {
        return "--".to_owned();
    };
    if image_width == 0 {
        return "--".to_owned();
    }

    let percent = ((f64::from(target.pixel_width) / f64::from(image_width)) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u32;
    format!("{percent}%")
}

fn status_file_size(file_size_bytes: Option<u64>) -> String {
    match file_size_bytes {
        Some(bytes) => format_size(Some(bytes)),
        None => "--".to_owned(),
    }
}

fn status_decompressed_size(size_bytes: Option<u64>) -> String {
    match size_bytes {
        Some(bytes) => format_size(Some(bytes)),
        None => "n/a".to_owned(),
    }
}

fn image_status_item(id: &'static str, text: String, tooltip: &'static str) -> AnyElement {
    div()
        .id(id)
        .min_w(px(0.0))
        .flex_shrink_0()
        .truncate()
        .tooltip(explorer_tooltip(tooltip))
        .child(SharedString::from(text))
        .into_any_element()
}

fn image_status_slash() -> AnyElement {
    div()
        .flex_shrink_0()
        .mx(px(4.0))
        .child(SharedString::from("/"))
        .into_any_element()
}

fn image_status_separator() -> AnyElement {
    div()
        .h(px(14.0))
        .w(px(1.0))
        .mx(px(12.0))
        .flex_shrink_0()
        .bg(rgb(STATUS_BAR_SEPARATOR_COLOR))
        .into_any_element()
}

fn image_body_available_size(viewport_width: f32, viewport_height: f32) -> (f32, f32) {
    (
        viewport_width,
        (viewport_height - TITLEBAR_HEIGHT - STATUS_BAR_HEIGHT).max(1.0),
    )
}

fn image_file_size(path: &std::path::Path) -> Option<u64> {
    let metadata = fs::metadata(path).ok()?;
    metadata.is_file().then_some(metadata.len())
}

fn image_title(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_viewer::decode::DecodedImageSource;

    #[test]
    fn ready_status_labels_include_resolution_scaling_file_size_and_decompressed_size() {
        let decoded = raster_decoded_image(2000, 1000, Some(8_000_000));
        let labels = image_status_labels(
            &ImageViewerState::Ready(decoded),
            Some(1536),
            Some(ImageFitTarget {
                pixel_width: 1000,
                pixel_height: 500,
                display_width: 1000.0,
                display_height: 500.0,
            }),
        );

        assert_eq!(
            labels,
            ImageStatusLabels {
                resolution: "2000 x 1000".to_owned(),
                scaling: "50%".to_owned(),
                file_size: "1.5 KB".to_owned(),
                decompressed_size: "7.63 MB".to_owned(),
            }
        );
        assert_eq!(
            format!("{} / {}", labels.file_size, labels.decompressed_size),
            "1.5 KB / 7.63 MB"
        );
    }

    #[test]
    fn loading_and_failed_status_labels_keep_file_size_and_placeholder_decode_fields() {
        assert_eq!(
            image_status_labels(&ImageViewerState::Loading, Some(350), None),
            ImageStatusLabels {
                resolution: "--".to_owned(),
                scaling: "--".to_owned(),
                file_size: "350 bytes".to_owned(),
                decompressed_size: "n/a".to_owned(),
            }
        );
        assert_eq!(
            image_status_labels(
                &ImageViewerState::Failed("bad image".to_owned()),
                None,
                None
            ),
            ImageStatusLabels {
                resolution: "--".to_owned(),
                scaling: "--".to_owned(),
                file_size: "--".to_owned(),
                decompressed_size: "n/a".to_owned(),
            }
        );
    }

    #[test]
    fn svg_ready_status_labels_show_decompressed_size_as_not_available() {
        let decoded = DecodedImage {
            width: 400,
            height: 200,
            source_decompressed_size_bytes: None,
            source: DecodedImageSource::Svg(Arc::new(Vec::new())),
        };

        let labels = image_status_labels(
            &ImageViewerState::Ready(decoded),
            Some(2048),
            Some(ImageFitTarget {
                pixel_width: 200,
                pixel_height: 100,
                display_width: 200.0,
                display_height: 100.0,
            }),
        );

        assert_eq!(labels.decompressed_size, "n/a");
        assert_eq!(labels.scaling, "50%");
        assert_eq!(labels.file_size, "2.0 KB");
        assert_eq!(
            format!("{} / {}", labels.file_size, labels.decompressed_size),
            "2.0 KB / n/a"
        );
    }

    #[test]
    fn status_item_tooltips_use_requested_wording() {
        assert_eq!(STATUS_TOOLTIP_RESOLUTION, "Resolution");
        assert_eq!(STATUS_TOOLTIP_SCALING, "Render percentage of resolution");
        assert_eq!(STATUS_TOOLTIP_SIZE, "Size");
        assert_eq!(STATUS_TOOLTIP_DECOMPRESSED_SIZE, "Decompressed size");
    }

    #[test]
    fn scaling_percent_uses_fitted_source_pixel_ratio_and_caps_at_no_upscale() {
        assert_eq!(
            status_scaling_percent(
                200,
                Some(ImageFitTarget {
                    pixel_width: 200,
                    pixel_height: 100,
                    display_width: 100.0,
                    display_height: 50.0,
                })
            ),
            "100%"
        );
        assert_eq!(
            status_scaling_percent(
                300,
                Some(ImageFitTarget {
                    pixel_width: 100,
                    pixel_height: 50,
                    display_width: 100.0,
                    display_height: 50.0,
                })
            ),
            "33%"
        );
        assert_eq!(
            status_scaling_percent(
                100,
                Some(ImageFitTarget {
                    pixel_width: 120,
                    pixel_height: 120,
                    display_width: 120.0,
                    display_height: 120.0,
                })
            ),
            "100%"
        );
        assert_eq!(status_scaling_percent(0, None), "--");
    }

    #[test]
    fn image_body_available_size_excludes_titlebar_and_status_bar() {
        assert_eq!(
            image_body_available_size(800.0, 600.0),
            (800.0, 600.0 - TITLEBAR_HEIGHT - STATUS_BAR_HEIGHT)
        );
        assert_eq!(image_body_available_size(800.0, 0.0), (800.0, 1.0));
    }

    fn raster_decoded_image(
        width: u32,
        height: u32,
        source_decompressed_size_bytes: Option<u64>,
    ) -> DecodedImage {
        DecodedImage {
            width,
            height,
            source_decompressed_size_bytes,
            source: DecodedImageSource::Raster(Arc::new(image::RgbaImage::new(1, 1))),
        }
    }
}
