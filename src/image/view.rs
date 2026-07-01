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
        decode::{DecodedImage, DecodedImageSource, decode_image_source},
        resize::{ImageFitTarget, fitted_image_target, svg_image_target},
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
    svg_render_generation: u64,
    svg_render_task: Option<Task<()>>,
    svg_render_pending: Option<ImageFitTarget>,
    svg_render_failed: Option<ImageFitTarget>,
    svg_rendered_image: Option<SvgRenderedImage>,
    should_move_window: bool,
}

enum ImageViewerState {
    Loading,
    Ready(DecodedImage),
    Failed(String),
}

struct SvgRenderedImage {
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
            svg_render_generation: 0,
            svg_render_task: None,
            svg_render_pending: None,
            svg_render_failed: None,
            svg_rendered_image: None,
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
                viewer.drop_decoded_image(cx);
                viewer.drop_svg_rendered_image(cx);
                viewer.svg_render_generation = viewer.svg_render_generation.wrapping_add(1);
                viewer.svg_render_task = None;
                viewer.svg_render_pending = None;
                viewer.svg_render_failed = None;
                viewer.state = match result {
                    Ok(decoded) => ImageViewerState::Ready(decoded),
                    Err(error) => ImageViewerState::Failed(error),
                };
                cx.notify();
            });
        }));
    }

    fn drop_decoded_image(&mut self, cx: &mut Context<Self>) {
        if let ImageViewerState::Ready(decoded) = &self.state
            && let DecodedImageSource::Raster(image) = &decoded.source
        {
            cx.drop_image(image.clone(), None);
        }
    }

    fn ensure_svg_rendered_image(
        &mut self,
        bytes: Arc<Vec<u8>>,
        target: ImageFitTarget,
        cx: &mut Context<Self>,
    ) {
        if self
            .svg_rendered_image
            .as_ref()
            .is_some_and(|rendered| rendered.target == target)
        {
            self.cancel_pending_svg_render();
            self.svg_render_failed = None;
            return;
        }
        if self.svg_render_pending == Some(target) {
            return;
        }
        if self.svg_render_failed == Some(target) {
            return;
        }

        self.svg_render_generation = self.svg_render_generation.wrapping_add(1);
        let generation = self.svg_render_generation;
        self.svg_render_pending = Some(target);
        self.svg_render_failed = None;
        self.svg_render_task = Some(cx.spawn(async move |viewer, cx| {
            let worker = cx
                .background_executor()
                .spawn(async move { render_svg_for_target(&bytes, target) });
            let result = worker.await;
            let _ = viewer.update(cx, |viewer, cx| {
                if viewer.svg_render_generation != generation
                    || viewer.svg_render_pending != Some(target)
                {
                    if let Ok(image) = result {
                        cx.drop_image(image, None);
                    }
                    return;
                }

                viewer.svg_render_task = None;
                viewer.svg_render_pending = None;
                match result {
                    Ok(image) => {
                        viewer.replace_svg_rendered_image(SvgRenderedImage { target, image }, cx);
                        viewer.svg_render_failed = None;
                    }
                    Err(error) => {
                        if viewer.svg_rendered_image.is_some() {
                            viewer.svg_render_failed = Some(target);
                        } else {
                            viewer.state = ImageViewerState::Failed(error);
                        }
                    }
                }
                cx.notify();
            });
        }));
    }

    fn cancel_pending_svg_render(&mut self) {
        if self.svg_render_pending.is_some() {
            self.svg_render_generation = self.svg_render_generation.wrapping_add(1);
            self.svg_render_pending = None;
            self.svg_render_task = None;
        }
    }

    fn replace_svg_rendered_image(&mut self, rendered: SvgRenderedImage, cx: &mut Context<Self>) {
        if let Some(previous) = self.svg_rendered_image.replace(rendered) {
            cx.drop_image(previous.image, None);
        }
    }

    fn drop_svg_rendered_image(&mut self, cx: &mut Context<Self>) {
        if let Some(rendered) = self.svg_rendered_image.take() {
            cx.drop_image(rendered.image, None);
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
            ImageViewerState::Ready(decoded) => match &decoded.source {
                DecodedImageSource::Raster(image) => {
                    let target = fitted_image_target(
                        decoded.width,
                        decoded.height,
                        available_width,
                        available_height,
                        scale_factor,
                    );
                    if let Some(target) = target {
                        render_ready_raster_image(image.clone(), target)
                    } else {
                        image_viewer_status("Cannot display image.")
                    }
                }
                DecodedImageSource::Svg(bytes) => {
                    let target = svg_image_target(
                        decoded.width,
                        decoded.height,
                        available_width,
                        available_height,
                        scale_factor,
                    );
                    if let Some(target) = target {
                        self.ensure_svg_rendered_image(bytes.clone(), target, cx);
                        if let Some(display_target) = svg_render_display_target(
                            self.svg_rendered_image
                                .as_ref()
                                .map(|rendered| rendered.target),
                            target,
                            self.svg_render_pending,
                            self.svg_render_failed,
                        ) {
                            let rendered = self
                                .svg_rendered_image
                                .as_ref()
                                .expect("svg rendered image target");
                            render_ready_raster_image(rendered.image.clone(), display_target)
                        } else {
                            image_viewer_status("Loading image...")
                        }
                    } else {
                        image_viewer_status("Cannot display image.")
                    }
                }
            },
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
        match &decoded.source {
            DecodedImageSource::Raster(_) => fitted_image_target(
                decoded.width,
                decoded.height,
                available_width,
                available_height,
                window.scale_factor(),
            ),
            DecodedImageSource::Svg(_) => svg_image_target(
                decoded.width,
                decoded.height,
                available_width,
                available_height,
                window.scale_factor(),
            ),
        }
    }
}

fn render_ready_raster_image(image: Arc<RenderImage>, target: ImageFitTarget) -> AnyElement {
    img(image)
        .w(px(target.display_width))
        .h(px(target.display_height))
        .object_fit(ObjectFit::Contain)
        .into_any_element()
}

fn svg_render_display_target(
    cached_target: Option<ImageFitTarget>,
    requested_target: ImageFitTarget,
    render_pending: Option<ImageFitTarget>,
    render_failed: Option<ImageFitTarget>,
) -> Option<ImageFitTarget> {
    let cached_target = cached_target?;
    if cached_target == requested_target {
        return Some(cached_target);
    }

    (render_pending == Some(requested_target) || render_failed == Some(requested_target))
        .then_some(requested_target)
}

fn render_svg_for_target(bytes: &[u8], target: ImageFitTarget) -> Result<Arc<RenderImage>, String> {
    let tree = usvg::Tree::from_data(bytes, &usvg::Options::default())
        .map_err(|error| format!("Failed to parse SVG: {error}"))?;
    let svg_size = tree.size();
    let scale_x = target.pixel_width as f32 / svg_size.width();
    let scale_y = target.pixel_height as f32 / svg_size.height();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(target.pixel_width, target.pixel_height)
        .ok_or_else(|| "Failed to allocate SVG render target.".to_owned())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale_x, scale_y),
        &mut pixmap.as_mut(),
    );
    let mut image =
        image::RgbaImage::from_raw(target.pixel_width, target.pixel_height, pixmap.take())
            .ok_or_else(|| "Failed to create SVG image buffer.".to_owned())?;
    unpremultiply_rgba(&mut image);

    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(image)])))
}

fn unpremultiply_rgba(image: &mut image::RgbaImage) {
    for pixel in image.pixels_mut() {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 || alpha == 255 {
            continue;
        }

        for channel in &mut pixel.0[..3] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
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

    let percent = ((f64::from(target.pixel_width) / f64::from(image_width)) * 100.0).round() as u32;
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

    #[test]
    fn svg_render_selection_uses_exact_target() {
        let target = image_fit_target(400, 400);

        assert_eq!(
            svg_render_display_target(Some(target), target, None, None),
            Some(target)
        );
    }

    #[test]
    fn svg_render_selection_uses_previous_render_while_new_target_is_pending() {
        let cached = image_fit_target(300, 300);
        let requested = image_fit_target(400, 400);

        assert_eq!(
            svg_render_display_target(Some(cached), requested, Some(requested), None),
            Some(requested)
        );
    }

    #[test]
    fn svg_render_selection_without_cached_render_returns_fallback() {
        let requested = image_fit_target(400, 400);

        assert_eq!(
            svg_render_display_target(None, requested, Some(requested), None),
            None
        );
    }

    #[test]
    fn svg_render_helper_preserves_requested_dimensions() {
        let image = render_svg_for_target(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="100" height="50" fill="red"/></svg>"#,
            ImageFitTarget {
                pixel_width: 80,
                pixel_height: 40,
                display_width: 80.0,
                display_height: 40.0,
            },
        )
        .unwrap();
        let size = image.size(0);

        assert_eq!(size.width.0, 80);
        assert_eq!(size.height.0, 40);
    }

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
    fn scaling_percent_uses_target_source_pixel_ratio() {
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
            "120%"
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
            source: DecodedImageSource::Raster(Arc::new(RenderImage::new(vec![
                image::Frame::new(image::RgbaImage::new(width.max(1), height.max(1))),
            ]))),
        }
    }

    fn image_fit_target(width: u32, height: u32) -> ImageFitTarget {
        ImageFitTarget {
            pixel_width: width,
            pixel_height: height,
            display_width: width as f32,
            display_height: height as f32,
        }
    }
}
