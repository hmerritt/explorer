use std::{fs, path::PathBuf, sync::Arc};

use gpui::{
    AnyElement, App, Bounds, ClickEvent, Context, CursorStyle, FocusHandle, Focusable, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement, Pixels, Point, Render,
    RenderImage, ScrollWheelEvent, SharedString, Styled, Task, TitlebarOptions, Window,
    WindowBounds, WindowDecorations, WindowOptions, canvas, div, img, point, prelude::*, px, rgb,
    size,
};

use crate::{
    explorer::{
        HorizontalScrollbarDrag, HorizontalScrollbarMetrics, ScrollbarArrow, ScrollbarDrag,
        ScrollbarMetrics,
        constants::{
            HORIZONTAL_SCROLLBAR_LINE_DELTA, SCROLLBAR_ARROW_HEIGHT, SCROLLBAR_GUTTER_WIDTH,
            SCROLLBAR_THUMB_ACTIVE_BG, SCROLLBAR_THUMB_BG, SCROLLBAR_THUMB_HOVER_BG,
            SCROLLBAR_THUMB_HOVER_WIDTH, SCROLLBAR_THUMB_WIDTH, SCROLLBAR_TRACK_BG,
            STATUS_BAR_HEIGHT, STATUS_BAR_HORIZONTAL_PADDING, STATUS_BAR_SEPARATOR_COLOR,
            STATUS_BAR_TEXT_COLOR, STATUS_BAR_TEXT_SIZE,
        },
        explorer_tooltip, format_size, horizontal_scrollbar_arrow_button, scrollbar_arrow_button,
        scrollbar_corner,
    },
    image_viewer::{
        ImageToggleActualSize, ImageZoomIn, ImageZoomOut,
        decode::{DecodedImage, DecodedImageSource, decode_image_source},
        resize::{
            ImageFitTarget, native_image_target, raster_initial_native_zoom,
            svg_initial_native_zoom,
        },
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
const STATUS_TOOLTIP_RENDERED_RESOLUTION: &str = "Rendered resolution";
const STATUS_TOOLTIP_SCALING: &str = "Rendered resolution percentage";
const STATUS_TOOLTIP_SIZE: &str = "Size";
const STATUS_TOOLTIP_DECOMPRESSED_SIZE: &str = "Decompressed size";
const STATUS_TOOLTIP_ZOOM_100: &str = "Set rendered resolution to 100%";
const STATUS_TOOLTIP_FIT_WIDTH: &str = "Fit width";
const STATUS_TOOLTIP_FIT_HEIGHT: &str = "Fit height";
const IMAGE_STATUS_ZOOM_BUTTON_WIDTH: f32 = 48.0;
const IMAGE_STATUS_FIT_BUTTON_WIDTH: f32 = 72.0;
const IMAGE_STATUS_BUTTON_HEIGHT: f32 = 20.0;
const IMAGE_STATUS_BUTTON_GAP: f32 = 6.0;
const IMAGE_VIEWER_MIN_ZOOM: f64 = 0.02;
const IMAGE_VIEWER_MAX_ZOOM: f64 = 28.0;
const IMAGE_VIEWER_MIN_ZOOM_PERCENT: u32 = 2;
const IMAGE_VIEWER_MAX_ZOOM_PERCENT: u32 = 2800;
const IMAGE_VIEWER_ZOOM_STEP_FACTOR: f64 = 1.10;
const IMAGE_VIEWER_SCROLLBAR_LINE_DELTA: f32 = 40.0;
const IMAGE_VIEWER_WHEEL_LINE_HEIGHT: f32 = 40.0;
const IMAGE_VIEWER_WHEEL_STEP_PIXELS: f32 = 120.0;
const ZOOM_EPSILON: f64 = 0.000_001;

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
        move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            cx.new(|cx| ImageViewer::new(path, title, focus_handle, cx))
        },
    )
    .expect("failed to open image viewer window");
}

struct ImageViewer {
    path: PathBuf,
    title: SharedString,
    file_size_bytes: Option<u64>,
    focus_handle: FocusHandle,
    state: ImageViewerState,
    decode_generation: u64,
    decode_task: Option<Task<()>>,
    svg_render_generation: u64,
    svg_render_task: Option<Task<()>>,
    svg_render_pending: Option<ImageFitTarget>,
    svg_render_failed: Option<ImageFitTarget>,
    svg_rendered_image: Option<SvgRenderedImage>,
    zoom: Option<f64>,
    manual_transform: bool,
    pan_offset: ImagePanOffset,
    pan_drag: Option<ImagePanDrag>,
    vertical_scrollbar_hovered: bool,
    vertical_scrollbar_drag: Option<ScrollbarDrag>,
    horizontal_scrollbar_hovered: bool,
    horizontal_scrollbar_drag: Option<HorizontalScrollbarDrag>,
    wheel_zoom_delta: f32,
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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ImagePanOffset {
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ImageBodyPoint {
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Debug)]
struct ImagePanDrag {
    start_position: Point<Pixels>,
    start_pan: ImagePanOffset,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ImageDisplayPlacement {
    target: ImageFitTarget,
    offset: ImagePanOffset,
    pan_limit: ImagePanOffset,
}

impl ImageDisplayPlacement {
    fn can_pan(self) -> bool {
        self.pan_limit.x > 0.0 || self.pan_limit.y > 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ImageViewportLayout {
    viewport_width: f32,
    viewport_height: f32,
    has_horizontal_scrollbar: bool,
    has_vertical_scrollbar: bool,
}

#[derive(Clone)]
enum ReadyImageRenderSource {
    Raster(Arc<RenderImage>),
    Svg(Arc<Vec<u8>>),
}

#[derive(Clone, Copy)]
enum ReadyImageKind {
    Raster,
    Svg,
}

impl ImageViewer {
    fn new(
        path: PathBuf,
        title: SharedString,
        focus_handle: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let file_size_bytes = image_file_size(&path);
        let mut viewer = Self {
            path,
            title,
            file_size_bytes,
            focus_handle,
            state: ImageViewerState::Loading,
            decode_generation: 0,
            decode_task: None,
            svg_render_generation: 0,
            svg_render_task: None,
            svg_render_pending: None,
            svg_render_failed: None,
            svg_rendered_image: None,
            zoom: None,
            manual_transform: false,
            pan_offset: ImagePanOffset::default(),
            pan_drag: None,
            vertical_scrollbar_hovered: false,
            vertical_scrollbar_drag: None,
            horizontal_scrollbar_hovered: false,
            horizontal_scrollbar_drag: None,
            wheel_zoom_delta: 0.0,
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
                viewer.reset_transform();
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

    fn reset_transform(&mut self) {
        self.zoom = None;
        self.manual_transform = false;
        self.pan_offset = ImagePanOffset::default();
        self.pan_drag = None;
        self.vertical_scrollbar_hovered = false;
        self.vertical_scrollbar_drag = None;
        self.horizontal_scrollbar_hovered = false;
        self.horizontal_scrollbar_drag = None;
        self.wheel_zoom_delta = 0.0;
    }

    fn ready_render_source(&self) -> Option<(u32, u32, ReadyImageRenderSource)> {
        let ImageViewerState::Ready(decoded) = &self.state else {
            return None;
        };

        let source = match &decoded.source {
            DecodedImageSource::Raster(image) => ReadyImageRenderSource::Raster(image.clone()),
            DecodedImageSource::Svg(bytes) => ReadyImageRenderSource::Svg(bytes.clone()),
        };
        Some((decoded.width, decoded.height, source))
    }

    fn ready_image_kind(&self) -> Option<(u32, u32, ReadyImageKind)> {
        let ImageViewerState::Ready(decoded) = &self.state else {
            return None;
        };

        let kind = match &decoded.source {
            DecodedImageSource::Raster(_) => ReadyImageKind::Raster,
            DecodedImageSource::Svg(_) => ReadyImageKind::Svg,
        };
        Some((decoded.width, decoded.height, kind))
    }

    fn sync_zoom_to_initial(&mut self, initial_zoom: f64) -> f64 {
        let initial_zoom = initial_zoom.clamp(0.0, IMAGE_VIEWER_MAX_ZOOM);
        if !self.manual_transform || self.zoom.is_none() {
            self.zoom = Some(initial_zoom);
            self.pan_offset = ImagePanOffset::default();
            self.pan_drag = None;
            return initial_zoom;
        }

        let zoom = self
            .zoom
            .unwrap_or(initial_zoom)
            .clamp(IMAGE_VIEWER_MIN_ZOOM, IMAGE_VIEWER_MAX_ZOOM);
        self.zoom = Some(zoom);
        zoom
    }

    fn handle_zoom_in(&mut self, _: &ImageZoomIn, window: &mut Window, cx: &mut Context<Self>) {
        let (available_width, available_height) = self.current_image_viewport_size(window);
        let anchor = ImageBodyPoint {
            x: available_width / 2.0,
            y: available_height / 2.0,
        };
        if self.zoom_by_steps(
            1,
            anchor,
            available_width,
            available_height,
            window.scale_factor(),
        ) {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn handle_zoom_out(&mut self, _: &ImageZoomOut, window: &mut Window, cx: &mut Context<Self>) {
        let (available_width, available_height) = self.current_image_viewport_size(window);
        let anchor = ImageBodyPoint {
            x: available_width / 2.0,
            y: available_height / 2.0,
        };
        if self.zoom_by_steps(
            -1,
            anchor,
            available_width,
            available_height,
            window.scale_factor(),
        ) {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn handle_toggle_actual_size(
        &mut self,
        _: &ImageToggleActualSize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (available_width, available_height) = self.current_image_viewport_size(window);
        if self.toggle_actual_size(available_width, available_height, window.scale_factor()) {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn handle_zoom_100_click(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (available_width, available_height) = self.current_image_viewport_size(window);
        cx.stop_propagation();
        if self.set_zoom_to_native_resolution(
            available_width,
            available_height,
            window.scale_factor(),
        ) {
            cx.notify();
        }
    }

    fn handle_fit_width_click(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (body_width, body_height) = current_body_available_size(window);
        let (available_width, available_height) = self.current_image_viewport_size(window);
        cx.stop_propagation();
        if self.set_zoom_to_fit_axis(
            ImageFitAxis::Width,
            body_width,
            body_height,
            available_width,
            available_height,
            window.scale_factor(),
        ) {
            cx.notify();
        }
    }

    fn handle_fit_height_click(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (body_width, body_height) = current_body_available_size(window);
        let (available_width, available_height) = self.current_image_viewport_size(window);
        cx.stop_propagation();
        if self.set_zoom_to_fit_axis(
            ImageFitAxis::Height,
            body_width,
            body_height,
            available_width,
            available_height,
            window.scale_factor(),
        ) {
            cx.notify();
        }
    }

    fn handle_wheel_zoom(
        &mut self,
        delta_y: f32,
        anchor: ImageBodyPoint,
        body_size: gpui::Size<Pixels>,
        window: &Window,
    ) -> bool {
        if delta_y == 0.0 || self.ready_image_kind().is_none() {
            return false;
        }

        let steps = wheel_zoom_steps(&mut self.wheel_zoom_delta, delta_y);

        let (available_width, available_height) = available_size_from_body_size(body_size);
        if steps != 0 {
            self.zoom_by_steps(
                steps,
                anchor,
                available_width,
                available_height,
                window.scale_factor(),
            );
        }
        true
    }

    fn handle_wheel_pan(
        &mut self,
        delta_x: f32,
        delta_y: f32,
        body_size: gpui::Size<Pixels>,
        scale_factor: f32,
    ) -> bool {
        let Some(placement) = self.current_placement_for_body(body_size, scale_factor) else {
            return false;
        };

        let horizontal_metrics = horizontal_scrollbar_metrics_for_placement(placement);
        let vertical_metrics = vertical_scrollbar_metrics_for_placement(placement);
        if horizontal_metrics.is_none() && vertical_metrics.is_none() {
            return false;
        }

        let mut pan_offset = placement.offset;
        if let Some(metrics) = vertical_metrics {
            let effective_delta_y = if delta_y != 0.0 { delta_y } else { delta_x };
            if effective_delta_y != 0.0 {
                let scroll_top = metrics.clamp_scroll_top(metrics.scroll_top - effective_delta_y);
                pan_offset.y = pan_offset_y_from_scroll_top(scroll_top, placement.pan_limit.y);
            }
        }

        if let Some(metrics) = horizontal_metrics {
            let effective_delta_x = if delta_x != 0.0 || vertical_metrics.is_some() {
                delta_x
            } else {
                delta_y
            };
            if effective_delta_x != 0.0 {
                let scroll_left =
                    metrics.clamp_scroll_left(metrics.scroll_left - effective_delta_x);
                pan_offset.x = pan_offset_x_from_scroll_left(scroll_left, placement.pan_limit.x);
            }
        }

        let pan_offset = clamp_pan_offset_to_limits(pan_offset, placement.pan_limit);
        if pan_offset == self.pan_offset {
            return false;
        }

        self.pan_offset = pan_offset;
        self.manual_transform = true;
        true
    }

    fn set_zoom_to_native_resolution(
        &mut self,
        available_width: f32,
        available_height: f32,
        scale_factor: f32,
    ) -> bool {
        self.set_zoom_to(1.0, available_width, available_height, scale_factor)
    }

    fn set_zoom_to_fit_axis(
        &mut self,
        axis: ImageFitAxis,
        body_width: f32,
        body_height: f32,
        available_width: f32,
        available_height: f32,
        scale_factor: f32,
    ) -> bool {
        let Some((image_width, image_height, _)) = self.ready_image_kind() else {
            return false;
        };
        let Some(zoom) = fit_axis_zoom(
            image_width,
            image_height,
            body_width,
            body_height,
            scale_factor,
            axis,
        ) else {
            return false;
        };

        self.set_zoom_to(zoom, available_width, available_height, scale_factor)
    }

    fn set_zoom_to(
        &mut self,
        zoom: f64,
        available_width: f32,
        available_height: f32,
        scale_factor: f32,
    ) -> bool {
        let Some((image_width, image_height, kind)) = self.ready_image_kind() else {
            return false;
        };
        let Some(initial_zoom) = initial_native_zoom_for_kind(
            kind,
            image_width,
            image_height,
            available_width,
            available_height,
            scale_factor,
        ) else {
            return false;
        };

        let old_zoom = self
            .zoom
            .unwrap_or_else(|| initial_zoom.clamp(0.0, IMAGE_VIEWER_MAX_ZOOM));
        let new_zoom = zoom.clamp(IMAGE_VIEWER_MIN_ZOOM, IMAGE_VIEWER_MAX_ZOOM);
        let Some(old_target) =
            native_image_target(image_width, image_height, old_zoom, scale_factor)
        else {
            return false;
        };
        let Some(new_target) =
            native_image_target(image_width, image_height, new_zoom, scale_factor)
        else {
            return false;
        };
        let old_pan = clamp_pan_offset(
            self.pan_offset,
            old_target,
            available_width,
            available_height,
        );
        let anchor = ImageBodyPoint {
            x: available_width / 2.0,
            y: available_height / 2.0,
        };
        let new_pan = pan_offset_after_zoom(
            old_pan,
            old_target,
            new_target,
            available_width,
            available_height,
            anchor,
        );

        let changed = (new_zoom - old_zoom).abs() >= ZOOM_EPSILON
            || self.zoom != Some(new_zoom)
            || !self.manual_transform
            || self.pan_offset != new_pan;

        self.zoom = Some(new_zoom);
        self.manual_transform = true;
        self.pan_offset = new_pan;
        changed
    }

    fn toggle_actual_size(
        &mut self,
        available_width: f32,
        available_height: f32,
        scale_factor: f32,
    ) -> bool {
        let Some((image_width, image_height, kind)) = self.ready_image_kind() else {
            return false;
        };
        let Some(initial_zoom) = initial_native_zoom_for_kind(
            kind,
            image_width,
            image_height,
            available_width,
            available_height,
            scale_factor,
        ) else {
            return false;
        };

        let old_zoom = self
            .zoom
            .unwrap_or_else(|| initial_zoom.clamp(0.0, IMAGE_VIEWER_MAX_ZOOM));
        if actual_size_toggle_for_zoom(old_zoom) == ActualSizeToggle::ResetToInitial {
            self.reset_transform();
            return true;
        }

        let Some(old_target) =
            native_image_target(image_width, image_height, old_zoom, scale_factor)
        else {
            return false;
        };
        let Some(new_target) = native_image_target(image_width, image_height, 1.0, scale_factor)
        else {
            return false;
        };
        let old_pan = clamp_pan_offset(
            self.pan_offset,
            old_target,
            available_width,
            available_height,
        );
        let anchor = ImageBodyPoint {
            x: available_width / 2.0,
            y: available_height / 2.0,
        };

        self.zoom = Some(1.0);
        self.manual_transform = true;
        self.pan_offset = pan_offset_after_zoom(
            old_pan,
            old_target,
            new_target,
            available_width,
            available_height,
            anchor,
        );
        true
    }

    fn current_image_viewport_size(&self, window: &Window) -> (f32, f32) {
        let (available_width, available_height) = current_body_available_size(window);
        let Some((image_width, image_height, kind)) = self.ready_image_kind() else {
            return (available_width, available_height);
        };
        let Some(initial_zoom) = initial_native_zoom_for_kind(
            kind,
            image_width,
            image_height,
            available_width,
            available_height,
            window.scale_factor(),
        ) else {
            return (available_width, available_height);
        };
        let zoom = self
            .zoom
            .unwrap_or_else(|| initial_zoom.clamp(0.0, IMAGE_VIEWER_MAX_ZOOM));
        let Some(target) =
            native_image_target(image_width, image_height, zoom, window.scale_factor())
        else {
            return (available_width, available_height);
        };
        let layout = image_viewport_layout(target, available_width, available_height);
        (layout.viewport_width, layout.viewport_height)
    }

    fn zoom_by_steps(
        &mut self,
        steps: i32,
        anchor: ImageBodyPoint,
        available_width: f32,
        available_height: f32,
        scale_factor: f32,
    ) -> bool {
        if steps == 0 {
            return false;
        }
        let Some((image_width, image_height, kind)) = self.ready_image_kind() else {
            return false;
        };
        let Some(initial_zoom) = initial_native_zoom_for_kind(
            kind,
            image_width,
            image_height,
            available_width,
            available_height,
            scale_factor,
        ) else {
            return false;
        };

        let old_zoom = self
            .zoom
            .unwrap_or_else(|| initial_zoom.clamp(0.0, IMAGE_VIEWER_MAX_ZOOM));
        let new_zoom = zoom_after_steps(old_zoom, steps);
        if (new_zoom - old_zoom).abs() < ZOOM_EPSILON {
            return false;
        }

        let Some(old_target) =
            native_image_target(image_width, image_height, old_zoom, scale_factor)
        else {
            return false;
        };
        let Some(new_target) =
            native_image_target(image_width, image_height, new_zoom, scale_factor)
        else {
            return false;
        };

        let old_pan = clamp_pan_offset(
            self.pan_offset,
            old_target,
            available_width,
            available_height,
        );
        self.zoom = Some(new_zoom);
        self.manual_transform = true;
        self.pan_offset = pan_offset_after_zoom(
            old_pan,
            old_target,
            new_target,
            available_width,
            available_height,
            anchor,
        );
        true
    }

    fn begin_pan_drag(
        &mut self,
        position: Point<Pixels>,
        bounds: &Bounds<Pixels>,
        window: &Window,
    ) -> bool {
        let Some(placement) = self.current_placement_for_body(bounds.size, window.scale_factor())
        else {
            return false;
        };
        if !placement.can_pan() {
            return false;
        }

        self.pan_offset = placement.offset;
        self.pan_drag = Some(ImagePanDrag {
            start_position: position,
            start_pan: placement.offset,
        });
        self.manual_transform = true;
        true
    }

    fn update_pan_drag(
        &mut self,
        position: Point<Pixels>,
        bounds: &Bounds<Pixels>,
        window: &Window,
    ) -> bool {
        let Some(drag) = self.pan_drag else {
            return false;
        };
        let Some(placement) = self.current_placement_for_body(bounds.size, window.scale_factor())
        else {
            return false;
        };

        let delta_x = f32::from(position.x - drag.start_position.x);
        let delta_y = f32::from(position.y - drag.start_position.y);
        self.pan_offset = clamp_pan_offset(
            ImagePanOffset {
                x: drag.start_pan.x + delta_x,
                y: drag.start_pan.y + delta_y,
            },
            placement.target,
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
        );
        true
    }

    fn handle_vertical_scrollbar_mouse_down(
        &mut self,
        local_y: f32,
        metrics: ScrollbarMetrics,
        placement: ImageDisplayPlacement,
    ) {
        if local_y < SCROLLBAR_ARROW_HEIGHT {
            self.set_vertical_scroll_top(
                metrics.scroll_by(-IMAGE_VIEWER_SCROLLBAR_LINE_DELTA),
                placement,
            );
        } else if local_y > metrics.viewport_height - SCROLLBAR_ARROW_HEIGHT {
            self.set_vertical_scroll_top(
                metrics.scroll_by(IMAGE_VIEWER_SCROLLBAR_LINE_DELTA),
                placement,
            );
        } else if local_y >= metrics.thumb_top && local_y <= metrics.thumb_bottom() {
            self.vertical_scrollbar_drag = Some(ScrollbarDrag {
                pointer_offset_from_thumb_top: local_y - metrics.thumb_top,
            });
        } else if local_y < metrics.thumb_top {
            self.set_vertical_scroll_top(metrics.scroll_by(-metrics.viewport_height), placement);
        } else {
            self.set_vertical_scroll_top(metrics.scroll_by(metrics.viewport_height), placement);
        }
    }

    fn handle_vertical_scrollbar_drag(
        &mut self,
        local_y: f32,
        metrics: ScrollbarMetrics,
        placement: ImageDisplayPlacement,
    ) {
        let Some(drag) = self.vertical_scrollbar_drag else {
            return;
        };

        let thumb_top = local_y - drag.pointer_offset_from_thumb_top;
        self.set_vertical_scroll_top(metrics.scroll_top_for_thumb_top(thumb_top), placement);
    }

    fn handle_horizontal_scrollbar_mouse_down(
        &mut self,
        local_x: f32,
        metrics: HorizontalScrollbarMetrics,
        placement: ImageDisplayPlacement,
    ) {
        if local_x < SCROLLBAR_ARROW_HEIGHT {
            self.set_horizontal_scroll_left(
                metrics.scroll_by(-HORIZONTAL_SCROLLBAR_LINE_DELTA),
                placement,
            );
        } else if local_x > metrics.viewport_width - SCROLLBAR_ARROW_HEIGHT {
            self.set_horizontal_scroll_left(
                metrics.scroll_by(HORIZONTAL_SCROLLBAR_LINE_DELTA),
                placement,
            );
        } else if local_x >= metrics.thumb_left && local_x <= metrics.thumb_right() {
            self.horizontal_scrollbar_drag = Some(HorizontalScrollbarDrag {
                pointer_offset_from_thumb_left: local_x - metrics.thumb_left,
            });
        } else if local_x < metrics.thumb_left {
            self.set_horizontal_scroll_left(metrics.scroll_by(-metrics.viewport_width), placement);
        } else {
            self.set_horizontal_scroll_left(metrics.scroll_by(metrics.viewport_width), placement);
        }
    }

    fn handle_horizontal_scrollbar_drag(
        &mut self,
        local_x: f32,
        metrics: HorizontalScrollbarMetrics,
        placement: ImageDisplayPlacement,
    ) {
        let Some(drag) = self.horizontal_scrollbar_drag else {
            return;
        };

        let thumb_left = local_x - drag.pointer_offset_from_thumb_left;
        self.set_horizontal_scroll_left(metrics.scroll_left_for_thumb_left(thumb_left), placement);
    }

    fn set_vertical_scroll_top(&mut self, scroll_top: f32, placement: ImageDisplayPlacement) {
        self.pan_offset = clamp_pan_offset_to_limits(
            ImagePanOffset {
                x: placement.offset.x,
                y: pan_offset_y_from_scroll_top(scroll_top, placement.pan_limit.y),
            },
            placement.pan_limit,
        );
        self.manual_transform = true;
    }

    fn set_horizontal_scroll_left(&mut self, scroll_left: f32, placement: ImageDisplayPlacement) {
        self.pan_offset = clamp_pan_offset_to_limits(
            ImagePanOffset {
                x: pan_offset_x_from_scroll_left(scroll_left, placement.pan_limit.x),
                y: placement.offset.y,
            },
            placement.pan_limit,
        );
        self.manual_transform = true;
    }

    fn current_placement_for_body(
        &self,
        body_size: gpui::Size<Pixels>,
        scale_factor: f32,
    ) -> Option<ImageDisplayPlacement> {
        let (available_width, available_height) = available_size_from_body_size(body_size);
        let (image_width, image_height, kind) = self.ready_image_kind()?;
        let initial_zoom = initial_native_zoom_for_kind(
            kind,
            image_width,
            image_height,
            available_width,
            available_height,
            scale_factor,
        )?;
        let zoom = self
            .zoom
            .unwrap_or_else(|| initial_zoom.clamp(0.0, IMAGE_VIEWER_MAX_ZOOM));
        let target = native_image_target(image_width, image_height, zoom, scale_factor)?;
        Some(image_display_placement(
            target,
            available_width,
            available_height,
            self.pan_offset,
        ))
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
            ImageViewerState::Ready(_) => self
                .render_ready_body(available_width, available_height, scale_factor, cx)
                .unwrap_or_else(|| image_viewer_status("Cannot display image.")),
        };

        div()
            .id("image-viewer-body")
            .relative()
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

    fn render_ready_body(
        &mut self,
        available_width: f32,
        available_height: f32,
        scale_factor: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (image_width, image_height, source) = self.ready_render_source()?;
        let initial_zoom = initial_native_zoom_for_kind(
            source.kind(),
            image_width,
            image_height,
            available_width,
            available_height,
            scale_factor,
        )?;
        let zoom = self.sync_zoom_to_initial(initial_zoom);
        let target = native_image_target(image_width, image_height, zoom, scale_factor)?;
        let layout = image_viewport_layout(target, available_width, available_height);
        let placement = image_display_placement(
            target,
            layout.viewport_width,
            layout.viewport_height,
            self.pan_offset,
        );
        self.pan_offset = placement.offset;
        if !layout.has_vertical_scrollbar {
            self.vertical_scrollbar_hovered = false;
            self.vertical_scrollbar_drag = None;
        }
        if !layout.has_horizontal_scrollbar {
            self.horizontal_scrollbar_hovered = false;
            self.horizontal_scrollbar_drag = None;
        }

        let content = match source {
            ReadyImageRenderSource::Raster(image) => {
                render_ready_image(image, placement.target, placement.offset)
            }
            ReadyImageRenderSource::Svg(bytes) => {
                self.ensure_svg_rendered_image(bytes, placement.target, cx);
                if let Some(display_target) = svg_render_display_target(
                    self.svg_rendered_image
                        .as_ref()
                        .map(|rendered| rendered.target),
                    placement.target,
                    self.svg_render_pending,
                    self.svg_render_failed,
                ) {
                    let rendered = self
                        .svg_rendered_image
                        .as_ref()
                        .expect("svg rendered image target");
                    render_ready_image(rendered.image.clone(), display_target, placement.offset)
                } else {
                    image_viewer_status("Loading image...")
                }
            }
        };

        Some(self.render_ready_body_layout(content, placement, layout, cx))
    }

    fn render_ready_body_layout(
        &self,
        content: AnyElement,
        placement: ImageDisplayPlacement,
        layout: ImageViewportLayout,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("image-viewer-ready-body")
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .id("image-viewer-image-viewport")
                            .relative()
                            .flex()
                            .items_center()
                            .justify_center()
                            .flex_1()
                            .h_full()
                            .overflow_hidden()
                            .child(content)
                            .child(self.render_body_hit_layer(placement.can_pan(), cx)),
                    )
                    .when(layout.has_vertical_scrollbar, |this| {
                        this.child(self.render_vertical_scrollbar(placement, cx))
                    }),
            )
            .when(layout.has_horizontal_scrollbar, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_row()
                        .w_full()
                        .h(px(SCROLLBAR_GUTTER_WIDTH))
                        .flex_shrink_0()
                        .child(self.render_horizontal_scrollbar(placement, cx))
                        .when(layout.has_vertical_scrollbar, |this| {
                            this.child(scrollbar_corner())
                        }),
                )
            })
            .into_any_element()
    }

    fn render_vertical_scrollbar(
        &self,
        placement: ImageDisplayPlacement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(metrics) = vertical_scrollbar_metrics_for_placement(placement) else {
            return div().into_any_element();
        };

        let hovered_or_dragged =
            self.vertical_scrollbar_hovered || self.vertical_scrollbar_drag.is_some();
        let thumb_width = if hovered_or_dragged {
            SCROLLBAR_THUMB_HOVER_WIDTH
        } else {
            SCROLLBAR_THUMB_WIDTH
        };
        let thumb_right = (SCROLLBAR_GUTTER_WIDTH - thumb_width) / 2.0;
        let thumb_color = if self.vertical_scrollbar_drag.is_some() {
            SCROLLBAR_THUMB_ACTIVE_BG
        } else if hovered_or_dragged {
            SCROLLBAR_THUMB_HOVER_BG
        } else {
            SCROLLBAR_THUMB_BG
        };
        let bottom_arrow_top = (metrics.viewport_height - SCROLLBAR_ARROW_HEIGHT).max(0.0);

        div()
            .id("image-viewer-vertical-scrollbar")
            .relative()
            .w(px(SCROLLBAR_GUTTER_WIDTH))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(SCROLLBAR_TRACK_BG))
            .cursor_default()
            .block_mouse_except_scroll()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.vertical_scrollbar_hovered = *hovered;
                cx.notify();
            }))
            .when(hovered_or_dragged, |this| {
                this.child(scrollbar_arrow_button(0.0, ScrollbarArrow::Up))
                    .child(scrollbar_arrow_button(
                        bottom_arrow_top,
                        ScrollbarArrow::Down,
                    ))
            })
            .child(
                div()
                    .absolute()
                    .top(px(metrics.thumb_top))
                    .right(px(thumb_right))
                    .w(px(thumb_width))
                    .h(px(metrics.thumb_height))
                    .rounded(px(thumb_width / 2.0))
                    .bg(rgb(thumb_color)),
            )
            .child(self.render_vertical_scrollbar_hit_layer(metrics, placement, cx))
            .into_any_element()
    }

    fn render_vertical_scrollbar_hit_layer(
        &self,
        metrics: ScrollbarMetrics,
        placement: ImageDisplayPlacement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entity = cx.entity();

        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                window.on_mouse_event({
                    let entity = entity.clone();
                    move |event: &MouseDownEvent, _, window, cx| {
                        if event.button != MouseButton::Left || !bounds.contains(&event.position) {
                            return;
                        }

                        let local_y = f32::from(event.position.y - bounds.origin.y);
                        let _ = entity.update(cx, |this, cx| {
                            this.handle_vertical_scrollbar_mouse_down(local_y, metrics, placement);
                            cx.stop_propagation();
                            window.prevent_default();
                            cx.notify();
                        });
                    }
                });

                window.on_mouse_event({
                    let entity = entity.clone();
                    move |event: &MouseMoveEvent, _, window, cx| {
                        if !event.dragging() {
                            return;
                        }

                        let local_y = f32::from(event.position.y - bounds.origin.y);
                        let _ = entity.update(cx, |this, cx| {
                            if this.vertical_scrollbar_drag.is_none() {
                                return;
                            }

                            this.handle_vertical_scrollbar_drag(local_y, metrics, placement);
                            cx.stop_propagation();
                            window.prevent_default();
                            cx.notify();
                        });
                    }
                });

                window.on_mouse_event(move |event: &MouseUpEvent, _, _, cx| {
                    if event.button != MouseButton::Left {
                        return;
                    }

                    let _ = entity.update(cx, |this, cx| {
                        if this.vertical_scrollbar_drag.take().is_some() {
                            cx.stop_propagation();
                            cx.notify();
                        }
                    });
                });
            },
        )
        .size_full()
        .into_any_element()
    }

    fn render_horizontal_scrollbar(
        &self,
        placement: ImageDisplayPlacement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(metrics) = horizontal_scrollbar_metrics_for_placement(placement) else {
            return div().into_any_element();
        };

        let hovered_or_dragged =
            self.horizontal_scrollbar_hovered || self.horizontal_scrollbar_drag.is_some();
        let thumb_height = if hovered_or_dragged {
            SCROLLBAR_THUMB_HOVER_WIDTH
        } else {
            SCROLLBAR_THUMB_WIDTH
        };
        let thumb_top = (SCROLLBAR_GUTTER_WIDTH - thumb_height) / 2.0;
        let thumb_color = if self.horizontal_scrollbar_drag.is_some() {
            SCROLLBAR_THUMB_ACTIVE_BG
        } else if hovered_or_dragged {
            SCROLLBAR_THUMB_HOVER_BG
        } else {
            SCROLLBAR_THUMB_BG
        };
        let right_arrow_left = (metrics.viewport_width - SCROLLBAR_ARROW_HEIGHT).max(0.0);

        div()
            .id("image-viewer-horizontal-scrollbar")
            .relative()
            .flex_1()
            .h(px(SCROLLBAR_GUTTER_WIDTH))
            .flex_shrink_0()
            .bg(rgb(SCROLLBAR_TRACK_BG))
            .cursor_default()
            .block_mouse_except_scroll()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.horizontal_scrollbar_hovered = *hovered;
                cx.notify();
            }))
            .when(hovered_or_dragged, |this| {
                this.child(horizontal_scrollbar_arrow_button(0.0, ScrollbarArrow::Left))
                    .child(horizontal_scrollbar_arrow_button(
                        right_arrow_left,
                        ScrollbarArrow::Right,
                    ))
            })
            .child(
                div()
                    .absolute()
                    .left(px(metrics.thumb_left))
                    .top(px(thumb_top))
                    .w(px(metrics.thumb_width))
                    .h(px(thumb_height))
                    .rounded(px(thumb_height / 2.0))
                    .bg(rgb(thumb_color)),
            )
            .child(self.render_horizontal_scrollbar_hit_layer(metrics, placement, cx))
            .into_any_element()
    }

    fn render_horizontal_scrollbar_hit_layer(
        &self,
        metrics: HorizontalScrollbarMetrics,
        placement: ImageDisplayPlacement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entity = cx.entity();

        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                window.on_mouse_event({
                    let entity = entity.clone();
                    move |event: &MouseDownEvent, _, window, cx| {
                        if event.button != MouseButton::Left || !bounds.contains(&event.position) {
                            return;
                        }

                        let local_x = f32::from(event.position.x - bounds.origin.x);
                        let _ = entity.update(cx, |this, cx| {
                            this.handle_horizontal_scrollbar_mouse_down(
                                local_x, metrics, placement,
                            );
                            cx.stop_propagation();
                            window.prevent_default();
                            cx.notify();
                        });
                    }
                });

                window.on_mouse_event({
                    let entity = entity.clone();
                    move |event: &MouseMoveEvent, _, window, cx| {
                        if !event.dragging() {
                            return;
                        }

                        let local_x = f32::from(event.position.x - bounds.origin.x);
                        let _ = entity.update(cx, |this, cx| {
                            if this.horizontal_scrollbar_drag.is_none() {
                                return;
                            }

                            this.handle_horizontal_scrollbar_drag(local_x, metrics, placement);
                            cx.stop_propagation();
                            window.prevent_default();
                            cx.notify();
                        });
                    }
                });

                window.on_mouse_event(move |event: &MouseUpEvent, _, _, cx| {
                    if event.button != MouseButton::Left {
                        return;
                    }

                    let _ = entity.update(cx, |this, cx| {
                        if this.horizontal_scrollbar_drag.take().is_some() {
                            cx.stop_propagation();
                            cx.notify();
                        }
                    });
                });
            },
        )
        .size_full()
        .into_any_element()
    }

    fn render_status_bar(
        &self,
        target: Option<ImageFitTarget>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let labels = image_status_labels(&self.state, self.file_size_bytes, target);
        let buttons_enabled = matches!(self.state, ImageViewerState::Ready(_));

        div()
            .id("image-viewer-status-bar")
            .debug_selector(|| "image-viewer-status-bar".to_owned())
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
            .child(
                div()
                    .id("image-viewer-status-metadata")
                    .debug_selector(|| "image-viewer-status-metadata".to_owned())
                    .flex()
                    .flex_row()
                    .items_center()
                    .min_w(px(0.0))
                    .flex_shrink()
                    .overflow_hidden()
                    .child(image_status_item(
                        "image-viewer-status-resolution",
                        labels.resolution,
                        STATUS_TOOLTIP_RESOLUTION,
                    ))
                    .child(image_status_separator())
                    .child(image_status_item(
                        "image-viewer-status-rendered-resolution",
                        labels.rendered_resolution,
                        STATUS_TOOLTIP_RENDERED_RESOLUTION,
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
                    )),
            )
            .child(div().min_w(px(8.0)).flex_1())
            .child(
                div()
                    .id("image-viewer-status-zoom-buttons")
                    .debug_selector(|| "image-viewer-status-zoom-buttons".to_owned())
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(IMAGE_STATUS_BUTTON_GAP))
                    .flex_shrink_0()
                    .child(image_status_button(
                        "image-viewer-status-zoom-100",
                        "100%",
                        IMAGE_STATUS_ZOOM_BUTTON_WIDTH,
                        STATUS_TOOLTIP_ZOOM_100,
                        buttons_enabled,
                        cx.listener(Self::handle_zoom_100_click),
                    ))
                    .child(image_status_button(
                        "image-viewer-status-fit-width",
                        "Fit Width",
                        IMAGE_STATUS_FIT_BUTTON_WIDTH,
                        STATUS_TOOLTIP_FIT_WIDTH,
                        buttons_enabled,
                        cx.listener(Self::handle_fit_width_click),
                    ))
                    .child(image_status_button(
                        "image-viewer-status-fit-height",
                        "Fit Height",
                        IMAGE_STATUS_FIT_BUTTON_WIDTH,
                        STATUS_TOOLTIP_FIT_HEIGHT,
                        buttons_enabled,
                        cx.listener(Self::handle_fit_height_click),
                    )),
            )
            .into_any_element()
    }

    fn render_body_hit_layer(&self, can_pan: bool, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let cursor = if self.pan_drag.is_some() {
            CursorStyle::ClosedHand
        } else if can_pan {
            CursorStyle::OpenHand
        } else {
            CursorStyle::Arrow
        };

        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                window.on_mouse_event({
                    let entity = entity.clone();
                    move |event: &MouseDownEvent, _, window, cx| {
                        if !bounds.contains(&event.position) {
                            return;
                        }

                        let _ = entity.update(cx, |this, cx| {
                            this.focus_handle.focus(window);
                            if event.button == MouseButton::Right
                                && this.begin_pan_drag(event.position, &bounds, window)
                            {
                                cx.stop_propagation();
                                window.prevent_default();
                                cx.notify();
                            }
                        });
                    }
                });

                window.on_mouse_event({
                    let entity = entity.clone();
                    move |event: &MouseMoveEvent, _, window, cx| {
                        if event.pressed_button != Some(MouseButton::Right) {
                            return;
                        }

                        let _ = entity.update(cx, |this, cx| {
                            if this.update_pan_drag(event.position, &bounds, window) {
                                cx.stop_propagation();
                                window.prevent_default();
                                cx.notify();
                            }
                        });
                    }
                });

                window.on_mouse_event({
                    let entity = entity.clone();
                    move |event: &MouseUpEvent, _, _, cx| {
                        if event.button != MouseButton::Right {
                            return;
                        }

                        let _ = entity.update(cx, |this, cx| {
                            if this.pan_drag.take().is_some() {
                                cx.stop_propagation();
                                cx.notify();
                            }
                        });
                    }
                });

                window.on_mouse_event(move |event: &ScrollWheelEvent, _, window, cx| {
                    if !bounds.contains(&event.position) {
                        return;
                    }

                    let delta = event.delta.pixel_delta(px(IMAGE_VIEWER_WHEEL_LINE_HEIGHT));
                    let _ = entity.update(cx, |this, cx| {
                        let handled = if event.modifiers.secondary() {
                            let anchor = local_body_point(event.position, &bounds);
                            this.handle_wheel_zoom(f32::from(delta.y), anchor, bounds.size, window)
                        } else {
                            this.handle_wheel_pan(
                                f32::from(delta.x),
                                f32::from(delta.y),
                                bounds.size,
                                window.scale_factor(),
                            )
                        };

                        if handled {
                            cx.stop_propagation();
                            window.prevent_default();
                            cx.notify();
                        }
                    });
                });
            },
        )
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .size_full()
        .cursor(cursor)
        .into_any_element()
    }

    fn current_render_target(&self, window: &Window) -> Option<ImageFitTarget> {
        let (image_width, image_height, kind) = self.ready_image_kind()?;
        let viewport = window.viewport_size();
        let (available_width, available_height) =
            image_body_available_size(f32::from(viewport.width), f32::from(viewport.height));
        let initial_zoom = initial_native_zoom_for_kind(
            kind,
            image_width,
            image_height,
            available_width,
            available_height,
            window.scale_factor(),
        )?;
        let zoom = self
            .zoom
            .unwrap_or_else(|| initial_zoom.min(IMAGE_VIEWER_MAX_ZOOM));

        native_image_target(image_width, image_height, zoom, window.scale_factor())
    }
}

fn render_ready_image(
    image: Arc<RenderImage>,
    target: ImageFitTarget,
    offset: ImagePanOffset,
) -> AnyElement {
    div()
        .relative()
        .left(px(offset.x))
        .top(px(offset.y))
        .child(
            img(image)
                .w(px(target.display_width))
                .h(px(target.display_height))
                .object_fit(ObjectFit::Contain),
        )
        .into_any_element()
}

impl ReadyImageRenderSource {
    fn kind(&self) -> ReadyImageKind {
        match self {
            Self::Raster(_) => ReadyImageKind::Raster,
            Self::Svg(_) => ReadyImageKind::Svg,
        }
    }
}

fn initial_native_zoom_for_kind(
    kind: ReadyImageKind,
    image_width: u32,
    image_height: u32,
    available_width: f32,
    available_height: f32,
    scale_factor: f32,
) -> Option<f64> {
    match kind {
        ReadyImageKind::Raster => raster_initial_native_zoom(
            image_width,
            image_height,
            available_width,
            available_height,
            scale_factor,
        ),
        ReadyImageKind::Svg => svg_initial_native_zoom(
            image_width,
            image_height,
            available_width,
            available_height,
            scale_factor,
        ),
    }
}

fn zoom_after_steps(mut zoom: f64, steps: i32) -> f64 {
    let direction = steps.signum();
    for _ in 0..steps.unsigned_abs() {
        zoom = next_zoom_level(zoom, direction);
    }
    zoom
}

fn next_zoom_level(zoom: f64, direction: i32) -> f64 {
    if direction > 0 {
        if zoom < IMAGE_VIEWER_MIN_ZOOM - ZOOM_EPSILON {
            return IMAGE_VIEWER_MIN_ZOOM;
        }

        let percent = zoom_percent(zoom);
        let mut next_zoom = zoom * IMAGE_VIEWER_ZOOM_STEP_FACTOR;
        if zoom_percent(next_zoom) <= percent {
            next_zoom = zoom_from_percent(percent.saturating_add(1));
        }
        return next_zoom.clamp(IMAGE_VIEWER_MIN_ZOOM, IMAGE_VIEWER_MAX_ZOOM);
    }

    if direction < 0 {
        if zoom <= IMAGE_VIEWER_MIN_ZOOM + ZOOM_EPSILON {
            return zoom;
        }

        let percent = zoom_percent(zoom);
        let mut next_zoom = zoom / IMAGE_VIEWER_ZOOM_STEP_FACTOR;
        if zoom_percent(next_zoom) >= percent {
            next_zoom = zoom_from_percent(percent.saturating_sub(1));
        }
        return next_zoom.clamp(IMAGE_VIEWER_MIN_ZOOM, IMAGE_VIEWER_MAX_ZOOM);
    }

    zoom
}

fn zoom_percent(zoom: f64) -> u32 {
    if !zoom.is_finite() {
        return IMAGE_VIEWER_MAX_ZOOM_PERCENT;
    }

    ((zoom * 100.0).round() as u32)
        .clamp(IMAGE_VIEWER_MIN_ZOOM_PERCENT, IMAGE_VIEWER_MAX_ZOOM_PERCENT)
}

fn zoom_from_percent(percent: u32) -> f64 {
    f64::from(percent.clamp(IMAGE_VIEWER_MIN_ZOOM_PERCENT, IMAGE_VIEWER_MAX_ZOOM_PERCENT)) / 100.0
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActualSizeToggle {
    ZoomToActualSize,
    ResetToInitial,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ImageFitAxis {
    Width,
    Height,
}

fn actual_size_toggle_for_zoom(zoom: f64) -> ActualSizeToggle {
    if (zoom - 1.0).abs() <= ZOOM_EPSILON {
        ActualSizeToggle::ResetToInitial
    } else {
        ActualSizeToggle::ZoomToActualSize
    }
}

fn fit_axis_zoom(
    image_width: u32,
    image_height: u32,
    available_width: f32,
    available_height: f32,
    scale_factor: f32,
    axis: ImageFitAxis,
) -> Option<f64> {
    if image_width == 0 || image_height == 0 {
        return None;
    }

    let initial_axis_size = match axis {
        ImageFitAxis::Width => available_width,
        ImageFitAxis::Height => available_height,
    };
    let initial_zoom = native_zoom_for_axis(
        image_axis_size(image_width, image_height, axis),
        initial_axis_size,
        scale_factor,
    )?;
    let initial_target =
        native_image_target(image_width, image_height, initial_zoom, scale_factor)?;
    let final_axis_size = match axis {
        ImageFitAxis::Width => image_viewport_axis_size(
            available_width,
            initial_target.display_height > available_height.max(1.0),
        ),
        ImageFitAxis::Height => image_viewport_axis_size(
            available_height,
            initial_target.display_width > available_width.max(1.0),
        ),
    };

    native_zoom_for_axis(
        image_axis_size(image_width, image_height, axis),
        final_axis_size,
        scale_factor,
    )
    .map(|zoom| zoom.clamp(IMAGE_VIEWER_MIN_ZOOM, IMAGE_VIEWER_MAX_ZOOM))
}

fn image_axis_size(image_width: u32, image_height: u32, axis: ImageFitAxis) -> u32 {
    match axis {
        ImageFitAxis::Width => image_width,
        ImageFitAxis::Height => image_height,
    }
}

fn native_zoom_for_axis(
    source_axis_size: u32,
    available_axis_size: f32,
    scale_factor: f32,
) -> Option<f64> {
    if source_axis_size == 0 {
        return None;
    }

    let scale_factor = scale_factor.max(1.0);
    let pixel_size = ((available_axis_size.max(1.0) * scale_factor).floor() as u32).max(1);
    Some(f64::from(pixel_size) / f64::from(source_axis_size))
}

fn wheel_zoom_steps(accumulator: &mut f32, delta_y: f32) -> i32 {
    if delta_y == 0.0 {
        return 0;
    }
    if *accumulator != 0.0 && accumulator.signum() != delta_y.signum() {
        *accumulator = 0.0;
    }
    *accumulator += delta_y;

    let mut steps = 0;
    while *accumulator >= IMAGE_VIEWER_WHEEL_STEP_PIXELS {
        steps += 1;
        *accumulator -= IMAGE_VIEWER_WHEEL_STEP_PIXELS;
    }
    while *accumulator <= -IMAGE_VIEWER_WHEEL_STEP_PIXELS {
        steps -= 1;
        *accumulator += IMAGE_VIEWER_WHEEL_STEP_PIXELS;
    }
    steps
}

fn image_display_placement(
    target: ImageFitTarget,
    available_width: f32,
    available_height: f32,
    pan_offset: ImagePanOffset,
) -> ImageDisplayPlacement {
    let pan_limit = pan_limits(target, available_width, available_height);
    ImageDisplayPlacement {
        target,
        offset: clamp_pan_offset_to_limits(pan_offset, pan_limit),
        pan_limit,
    }
}

fn image_viewport_layout(
    target: ImageFitTarget,
    available_width: f32,
    available_height: f32,
) -> ImageViewportLayout {
    let mut has_horizontal_scrollbar = false;
    let mut has_vertical_scrollbar = false;

    loop {
        let viewport_width = image_viewport_axis_size(available_width, has_vertical_scrollbar);
        let viewport_height = image_viewport_axis_size(available_height, has_horizontal_scrollbar);
        let next_has_horizontal_scrollbar = target.display_width > viewport_width;
        let next_has_vertical_scrollbar = target.display_height > viewport_height;

        if next_has_horizontal_scrollbar == has_horizontal_scrollbar
            && next_has_vertical_scrollbar == has_vertical_scrollbar
        {
            return ImageViewportLayout {
                viewport_width,
                viewport_height,
                has_horizontal_scrollbar,
                has_vertical_scrollbar,
            };
        }

        has_horizontal_scrollbar = next_has_horizontal_scrollbar;
        has_vertical_scrollbar = next_has_vertical_scrollbar;
    }
}

fn image_viewport_axis_size(available: f32, reserve_cross_axis_scrollbar: bool) -> f32 {
    (available
        - if reserve_cross_axis_scrollbar {
            SCROLLBAR_GUTTER_WIDTH
        } else {
            0.0
        })
    .max(1.0)
}

fn horizontal_scrollbar_metrics_for_placement(
    placement: ImageDisplayPlacement,
) -> Option<HorizontalScrollbarMetrics> {
    if placement.pan_limit.x <= 0.0 {
        return None;
    }

    HorizontalScrollbarMetrics::new(
        placement.target.display_width - (placement.pan_limit.x * 2.0),
        placement.target.display_width,
        scroll_left_from_pan_offset(placement.offset.x, placement.pan_limit.x),
    )
}

fn vertical_scrollbar_metrics_for_placement(
    placement: ImageDisplayPlacement,
) -> Option<ScrollbarMetrics> {
    if placement.pan_limit.y <= 0.0 {
        return None;
    }

    ScrollbarMetrics::new(
        placement.target.display_width - (placement.pan_limit.x * 2.0),
        placement.target.display_height - (placement.pan_limit.y * 2.0),
        placement.target.display_height,
        scroll_top_from_pan_offset(placement.offset.y, placement.pan_limit.y),
    )
}

fn scroll_left_from_pan_offset(pan_x: f32, pan_limit_x: f32) -> f32 {
    (pan_limit_x - pan_x).clamp(0.0, pan_limit_x * 2.0)
}

fn scroll_top_from_pan_offset(pan_y: f32, pan_limit_y: f32) -> f32 {
    (pan_limit_y - pan_y).clamp(0.0, pan_limit_y * 2.0)
}

fn pan_offset_x_from_scroll_left(scroll_left: f32, pan_limit_x: f32) -> f32 {
    pan_limit_x - scroll_left.clamp(0.0, pan_limit_x * 2.0)
}

fn pan_offset_y_from_scroll_top(scroll_top: f32, pan_limit_y: f32) -> f32 {
    pan_limit_y - scroll_top.clamp(0.0, pan_limit_y * 2.0)
}

fn pan_limits(
    target: ImageFitTarget,
    available_width: f32,
    available_height: f32,
) -> ImagePanOffset {
    ImagePanOffset {
        x: ((target.display_width - available_width) / 2.0).max(0.0),
        y: ((target.display_height - available_height) / 2.0).max(0.0),
    }
}

fn clamp_pan_offset(
    offset: ImagePanOffset,
    target: ImageFitTarget,
    available_width: f32,
    available_height: f32,
) -> ImagePanOffset {
    clamp_pan_offset_to_limits(
        offset,
        pan_limits(target, available_width, available_height),
    )
}

fn clamp_pan_offset_to_limits(offset: ImagePanOffset, limit: ImagePanOffset) -> ImagePanOffset {
    ImagePanOffset {
        x: if limit.x > 0.0 {
            offset.x.clamp(-limit.x, limit.x)
        } else {
            0.0
        },
        y: if limit.y > 0.0 {
            offset.y.clamp(-limit.y, limit.y)
        } else {
            0.0
        },
    }
}

fn pan_offset_after_zoom(
    old_pan: ImagePanOffset,
    old_target: ImageFitTarget,
    new_target: ImageFitTarget,
    available_width: f32,
    available_height: f32,
    anchor: ImageBodyPoint,
) -> ImagePanOffset {
    let anchor_x = anchor.x - (available_width / 2.0);
    let anchor_y = anchor.y - (available_height / 2.0);
    let image_x = if old_target.display_width > 0.0 {
        (anchor_x - old_pan.x) / old_target.display_width
    } else {
        0.0
    };
    let image_y = if old_target.display_height > 0.0 {
        (anchor_y - old_pan.y) / old_target.display_height
    } else {
        0.0
    };

    clamp_pan_offset(
        ImagePanOffset {
            x: anchor_x - (image_x * new_target.display_width),
            y: anchor_y - (image_y * new_target.display_height),
        },
        new_target,
        available_width,
        available_height,
    )
}

fn current_body_available_size(window: &Window) -> (f32, f32) {
    let viewport = window.viewport_size();
    image_body_available_size(f32::from(viewport.width), f32::from(viewport.height))
}

fn available_size_from_body_size(body_size: gpui::Size<Pixels>) -> (f32, f32) {
    (
        f32::from(body_size.width).max(1.0),
        f32::from(body_size.height).max(1.0),
    )
}

fn local_body_point(position: Point<Pixels>, bounds: &Bounds<Pixels>) -> ImageBodyPoint {
    ImageBodyPoint {
        x: f32::from(position.x - bounds.origin.x),
        y: f32::from(position.y - bounds.origin.y),
    }
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

impl Focusable for ImageViewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ImageViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = div()
            .key_context("ImageViewer")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::handle_zoom_in))
            .on_action(cx.listener(Self::handle_zoom_out))
            .on_action(cx.listener(Self::handle_toggle_actual_size))
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(0xffffff))
            .child(self.render_titlebar(window, cx))
            .child(self.render_body(window, cx))
            .child(self.render_status_bar(self.current_render_target(window), cx))
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
    rendered_resolution: String,
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
            rendered_resolution: status_rendered_resolution(target),
            scaling: status_scaling_percent(decoded.width, target),
            file_size: status_file_size(file_size_bytes),
            decompressed_size: status_decompressed_size(decoded.source_decompressed_size_bytes),
        },
        ImageViewerState::Loading | ImageViewerState::Failed(_) => ImageStatusLabels {
            resolution: "--".to_owned(),
            rendered_resolution: "--".to_owned(),
            scaling: "--".to_owned(),
            file_size: status_file_size(file_size_bytes),
            decompressed_size: status_decompressed_size(None),
        },
    }
}

fn status_rendered_resolution(target: Option<ImageFitTarget>) -> String {
    target
        .map(|target| format!("{} x {}", target.pixel_width, target.pixel_height))
        .unwrap_or_else(|| "--".to_owned())
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
        .debug_selector(move || id.to_owned())
        .min_w(px(0.0))
        .flex_shrink_0()
        .truncate()
        .tooltip(explorer_tooltip(tooltip))
        .child(SharedString::from(text))
        .into_any_element()
}

fn image_status_button(
    id: &'static str,
    label: &'static str,
    width: f32,
    tooltip: &'static str,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .flex()
        .items_center()
        .justify_center()
        .w(px(width))
        .h(px(IMAGE_STATUS_BUTTON_HEIGHT))
        .flex_shrink_0()
        .overflow_hidden()
        .rounded(px(2.0))
        .border_1()
        .border_color(rgb(0xd8d8d8))
        .bg(rgb(0xf8f8f8))
        .text_color(rgb(0x1f1f1f))
        .cursor_default()
        .tooltip(explorer_tooltip(tooltip))
        .when(enabled, |this| {
            this.hover(|style| style.bg(rgb(0xe5f3ff)).border_color(rgb(0x7aa7d9)))
                .active(|style| style.opacity(0.72))
                .on_click(on_click)
        })
        .when(!enabled, |this| this.opacity(0.45))
        .child(SharedString::from(label))
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
    use gpui::{AppContext, TestAppContext};

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
                rendered_resolution: "1000 x 500".to_owned(),
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
                rendered_resolution: "--".to_owned(),
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
                rendered_resolution: "--".to_owned(),
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
        assert_eq!(labels.rendered_resolution, "200 x 100");
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
        assert_eq!(STATUS_TOOLTIP_RENDERED_RESOLUTION, "Rendered resolution");
        assert_eq!(STATUS_TOOLTIP_SCALING, "Rendered resolution percentage");
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
    fn rendered_resolution_uses_current_target_pixels() {
        assert_eq!(
            status_rendered_resolution(Some(ImageFitTarget {
                pixel_width: 56,
                pixel_height: 28,
                display_width: 56.0,
                display_height: 28.0,
            })),
            "56 x 28"
        );
        assert_eq!(status_rendered_resolution(None), "--");
    }

    #[test]
    fn zoom_levels_use_rounded_ten_percent_steps() {
        let mut zoom = 1.0;
        for expected_percent in [110, 121, 133, 146, 161, 177] {
            zoom = next_zoom_level(zoom, 1);
            assert_eq!(zoom_percent(zoom), expected_percent);
        }

        let mut zoom = 1.0;
        for expected_percent in [91, 83, 75, 68, 62, 56, 51, 47, 42, 39, 35, 32] {
            zoom = next_zoom_level(zoom, -1);
            assert_eq!(zoom_percent(zoom), expected_percent);
        }
    }

    #[test]
    fn zoom_levels_preserve_min_max_and_monotonic_steps() {
        assert_eq!(next_zoom_level(0.01, 1), 0.02);
        assert_eq!(next_zoom_level(0.02, -1), 0.02);
        assert_eq!(next_zoom_level(27.95, 1), 28.0);
        assert_eq!(next_zoom_level(28.0, 1), 28.0);
        assert_eq!(zoom_percent(next_zoom_level(0.02, 1)), 3);
        assert_eq!(zoom_percent(next_zoom_level(0.03, -1)), 2);
        assert_eq!(zoom_percent(next_zoom_level(0.10, -1)), 9);
        assert_eq!(zoom_percent(next_zoom_level(0.10, 1)), 11);
        assert_eq!(zoom_percent(next_zoom_level(0.30, 1)), 33);
        assert_eq!(zoom_percent(next_zoom_level(0.30, -1)), 27);
    }

    #[test]
    fn zoom_levels_snap_arbitrary_fit_scales() {
        assert_eq!(zoom_percent(next_zoom_level(0.333, 1)), 37);
        assert_eq!(zoom_percent(next_zoom_level(0.333, -1)), 30);
        assert_eq!(zoom_percent(next_zoom_level(0.055, 1)), 7);
        assert_eq!(zoom_percent(next_zoom_level(0.055, -1)), 5);
        assert_eq!(next_zoom_level(0.005, 1), 0.02);
        assert_eq!(next_zoom_level(0.005, -1), 0.005);
    }

    #[test]
    fn actual_size_toggle_decision_uses_zoom_epsilon() {
        assert_eq!(
            actual_size_toggle_for_zoom(0.5),
            ActualSizeToggle::ZoomToActualSize
        );
        assert_eq!(
            actual_size_toggle_for_zoom(1.5),
            ActualSizeToggle::ZoomToActualSize
        );
        assert_eq!(
            actual_size_toggle_for_zoom(1.0 - (ZOOM_EPSILON / 2.0)),
            ActualSizeToggle::ResetToInitial
        );
        assert_eq!(
            actual_size_toggle_for_zoom(1.0 + (ZOOM_EPSILON / 2.0)),
            ActualSizeToggle::ResetToInitial
        );
    }

    #[gpui::test]
    fn actual_size_toggle_at_actual_size_resets_to_initial_fit_state(cx: &mut TestAppContext) {
        let viewer = cx.new(|cx| ImageViewer {
            path: PathBuf::new(),
            title: SharedString::from("image.png"),
            file_size_bytes: None,
            focus_handle: cx.focus_handle(),
            state: ImageViewerState::Ready(raster_decoded_image(2000, 1000, None)),
            decode_generation: 0,
            decode_task: None,
            svg_render_generation: 0,
            svg_render_task: None,
            svg_render_pending: None,
            svg_render_failed: None,
            svg_rendered_image: None,
            zoom: Some(1.0),
            manual_transform: true,
            pan_offset: ImagePanOffset { x: 40.0, y: -20.0 },
            pan_drag: Some(ImagePanDrag {
                start_position: point(px(10.0), px(10.0)),
                start_pan: ImagePanOffset { x: 40.0, y: -20.0 },
            }),
            vertical_scrollbar_hovered: true,
            vertical_scrollbar_drag: None,
            horizontal_scrollbar_hovered: true,
            horizontal_scrollbar_drag: None,
            wheel_zoom_delta: 60.0,
            should_move_window: true,
        });

        viewer.update(cx, |viewer, _| {
            assert!(viewer.toggle_actual_size(800.0, 600.0, 1.0));
            assert_eq!(viewer.zoom, None);
            assert!(!viewer.manual_transform);
            assert_eq!(viewer.pan_offset, ImagePanOffset::default());
            assert!(viewer.pan_drag.is_none());
            assert!(!viewer.vertical_scrollbar_hovered);
            assert!(!viewer.horizontal_scrollbar_hovered);
            assert_eq!(viewer.wheel_zoom_delta, 0.0);
        });
    }

    #[gpui::test]
    fn zoom_100_sets_native_resolution_without_toggling(cx: &mut TestAppContext) {
        let viewer = cx.new(|cx| ImageViewer {
            path: PathBuf::new(),
            title: SharedString::from("image.png"),
            file_size_bytes: None,
            focus_handle: cx.focus_handle(),
            state: ImageViewerState::Ready(raster_decoded_image(2000, 1000, None)),
            decode_generation: 0,
            decode_task: None,
            svg_render_generation: 0,
            svg_render_task: None,
            svg_render_pending: None,
            svg_render_failed: None,
            svg_rendered_image: None,
            zoom: Some(0.5),
            manual_transform: true,
            pan_offset: ImagePanOffset::default(),
            pan_drag: None,
            vertical_scrollbar_hovered: false,
            vertical_scrollbar_drag: None,
            horizontal_scrollbar_hovered: false,
            horizontal_scrollbar_drag: None,
            wheel_zoom_delta: 0.0,
            should_move_window: false,
        });

        viewer.update(cx, |viewer, _| {
            assert!(viewer.set_zoom_to_native_resolution(800.0, 600.0, 1.0));
            assert_eq!(viewer.zoom, Some(1.0));
            let target = native_image_target(2000, 1000, viewer.zoom.unwrap(), 1.0).unwrap();
            assert_eq!(target.pixel_width, 2000);
            assert_eq!(target.pixel_height, 1000);

            viewer.pan_offset = ImagePanOffset { x: 100.0, y: 0.0 };
            assert!(!viewer.set_zoom_to_native_resolution(800.0, 600.0, 1.0));
            assert_eq!(viewer.zoom, Some(1.0));
            assert!(viewer.manual_transform);
            assert_eq!(viewer.pan_offset, ImagePanOffset { x: 100.0, y: 0.0 });
        });
    }

    #[test]
    fn fit_width_zoom_uses_hidpi_pixels_and_reserves_vertical_scrollbar() {
        let zoom = fit_axis_zoom(1000, 1000, 400.0, 300.0, 2.0, ImageFitAxis::Width).unwrap();
        let target = native_image_target(1000, 1000, zoom, 2.0).unwrap();
        let layout = image_viewport_layout(target, 400.0, 300.0);

        assert_eq!(target.pixel_width, 764);
        assert_eq!(target.display_width, 382.0);
        assert_eq!(target.display_height, 382.0);
        assert_eq!(layout.viewport_width, 382.0);
        assert!(layout.has_vertical_scrollbar);
        assert!(!layout.has_horizontal_scrollbar);
    }

    #[test]
    fn fit_height_zoom_uses_hidpi_pixels_and_reserves_horizontal_scrollbar() {
        let zoom = fit_axis_zoom(1000, 500, 400.0, 300.0, 2.0, ImageFitAxis::Height).unwrap();
        let target = native_image_target(1000, 500, zoom, 2.0).unwrap();
        let layout = image_viewport_layout(target, 400.0, 300.0);

        assert_eq!(target.pixel_height, 564);
        assert_eq!(target.display_width, 564.0);
        assert_eq!(target.display_height, 282.0);
        assert_eq!(layout.viewport_height, 282.0);
        assert!(layout.has_horizontal_scrollbar);
        assert!(!layout.has_vertical_scrollbar);
    }

    #[gpui::test]
    fn status_bar_zoom_buttons_render_right_aligned(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ImageViewer {
                path: PathBuf::new(),
                title: SharedString::from("image.png"),
                file_size_bytes: Some(1536),
                focus_handle,
                state: ImageViewerState::Ready(raster_decoded_image(2000, 1000, Some(8_000_000))),
                decode_generation: 0,
                decode_task: None,
                svg_render_generation: 0,
                svg_render_task: None,
                svg_render_pending: None,
                svg_render_failed: None,
                svg_rendered_image: None,
                zoom: Some(0.5),
                manual_transform: true,
                pan_offset: ImagePanOffset::default(),
                pan_drag: None,
                vertical_scrollbar_hovered: false,
                vertical_scrollbar_drag: None,
                horizontal_scrollbar_hovered: false,
                horizontal_scrollbar_drag: None,
                wheel_zoom_delta: 0.0,
                should_move_window: false,
            }
        });

        cx.run_until_parked();

        let status = cx
            .debug_bounds("image-viewer-status-bar")
            .expect("status bar bounds");
        let metadata = cx
            .debug_bounds("image-viewer-status-metadata")
            .expect("metadata bounds");
        let zoom_100 = cx
            .debug_bounds("image-viewer-status-zoom-100")
            .expect("100 percent button bounds");
        let fit_width = cx
            .debug_bounds("image-viewer-status-fit-width")
            .expect("fit width button bounds");
        let fit_height = cx
            .debug_bounds("image-viewer-status-fit-height")
            .expect("fit height button bounds");

        let metadata_right = f32::from(metadata.origin.x) + f32::from(metadata.size.width);
        let status_right = f32::from(status.origin.x) + f32::from(status.size.width);
        let fit_height_right = f32::from(fit_height.origin.x) + f32::from(fit_height.size.width);

        assert!(metadata_right <= f32::from(zoom_100.origin.x));
        assert!(f32::from(zoom_100.origin.x) < f32::from(fit_width.origin.x));
        assert!(f32::from(fit_width.origin.x) < f32::from(fit_height.origin.x));
        assert!((status_right - STATUS_BAR_HORIZONTAL_PADDING - fit_height_right).abs() <= 1.0);
    }

    #[test]
    fn display_placement_centers_and_clamps_pan_to_overflow_bounds() {
        let target = ImageFitTarget {
            pixel_width: 1000,
            pixel_height: 600,
            display_width: 1000.0,
            display_height: 600.0,
        };
        let placement = image_display_placement(
            target,
            400.0,
            300.0,
            ImagePanOffset {
                x: 999.0,
                y: -999.0,
            },
        );

        assert_eq!(placement.pan_limit, ImagePanOffset { x: 300.0, y: 150.0 });
        assert_eq!(
            placement.offset,
            ImagePanOffset {
                x: 300.0,
                y: -150.0
            }
        );

        let centered = image_display_placement(
            ImageFitTarget {
                pixel_width: 200,
                pixel_height: 100,
                display_width: 200.0,
                display_height: 100.0,
            },
            400.0,
            300.0,
            ImagePanOffset { x: 80.0, y: 20.0 },
        );
        assert_eq!(centered.offset, ImagePanOffset::default());
        assert!(!centered.can_pan());
    }

    #[test]
    fn display_placement_allows_one_axis_overflow() {
        let placement = image_display_placement(
            ImageFitTarget {
                pixel_width: 1000,
                pixel_height: 100,
                display_width: 1000.0,
                display_height: 100.0,
            },
            400.0,
            300.0,
            ImagePanOffset { x: -200.0, y: 80.0 },
        );

        assert_eq!(placement.offset, ImagePanOffset { x: -200.0, y: 0.0 });
        assert_eq!(placement.pan_limit, ImagePanOffset { x: 300.0, y: 0.0 });
        assert!(placement.can_pan());
    }

    #[test]
    fn scrollbar_layout_rechecks_cross_axis_gutters() {
        let vertical_then_horizontal = image_viewport_layout(
            ImageFitTarget {
                pixel_width: 390,
                pixel_height: 320,
                display_width: 390.0,
                display_height: 320.0,
            },
            400.0,
            318.0,
        );
        assert_eq!(
            vertical_then_horizontal,
            ImageViewportLayout {
                viewport_width: 400.0 - SCROLLBAR_GUTTER_WIDTH,
                viewport_height: 318.0 - SCROLLBAR_GUTTER_WIDTH,
                has_horizontal_scrollbar: true,
                has_vertical_scrollbar: true,
            }
        );

        let horizontal_then_vertical = image_viewport_layout(
            ImageFitTarget {
                pixel_width: 405,
                pixel_height: 301,
                display_width: 405.0,
                display_height: 301.0,
            },
            400.0,
            318.0,
        );
        assert_eq!(
            horizontal_then_vertical,
            ImageViewportLayout {
                viewport_width: 400.0 - SCROLLBAR_GUTTER_WIDTH,
                viewport_height: 318.0 - SCROLLBAR_GUTTER_WIDTH,
                has_horizontal_scrollbar: true,
                has_vertical_scrollbar: true,
            }
        );
    }

    #[test]
    fn scrollbar_metrics_map_to_center_based_pan_offsets() {
        let placement = image_display_placement(
            ImageFitTarget {
                pixel_width: 1000,
                pixel_height: 600,
                display_width: 1000.0,
                display_height: 600.0,
            },
            400.0,
            300.0,
            ImagePanOffset::default(),
        );
        let horizontal = horizontal_scrollbar_metrics_for_placement(placement).unwrap();
        let vertical = vertical_scrollbar_metrics_for_placement(placement).unwrap();

        assert_eq!(placement.pan_limit, ImagePanOffset { x: 300.0, y: 150.0 });
        assert_eq!(horizontal.scroll_left, 300.0);
        assert_eq!(horizontal.scroll_max, 600.0);
        assert_eq!(vertical.scroll_top, 150.0);
        assert_eq!(vertical.scroll_max, 300.0);
        assert_eq!(
            pan_offset_x_from_scroll_left(0.0, placement.pan_limit.x),
            300.0
        );
        assert_eq!(
            pan_offset_y_from_scroll_top(0.0, placement.pan_limit.y),
            150.0
        );
        assert_eq!(
            pan_offset_x_from_scroll_left(horizontal.scroll_max, placement.pan_limit.x),
            -300.0
        );
        assert_eq!(
            pan_offset_y_from_scroll_top(vertical.scroll_max, placement.pan_limit.y),
            -150.0
        );
    }

    #[test]
    fn image_scrollbar_scroll_values_clamp_to_pan_limits() {
        assert_eq!(pan_offset_x_from_scroll_left(-50.0, 300.0), 300.0);
        assert_eq!(pan_offset_x_from_scroll_left(999.0, 300.0), -300.0);
        assert_eq!(pan_offset_y_from_scroll_top(-50.0, 150.0), 150.0);
        assert_eq!(pan_offset_y_from_scroll_top(999.0, 150.0), -150.0);
        assert_eq!(scroll_left_from_pan_offset(999.0, 300.0), 0.0);
        assert_eq!(scroll_left_from_pan_offset(-999.0, 300.0), 600.0);
        assert_eq!(scroll_top_from_pan_offset(999.0, 150.0), 0.0);
        assert_eq!(scroll_top_from_pan_offset(-999.0, 150.0), 300.0);
    }

    #[test]
    fn image_scrollbar_line_page_and_thumb_movement_update_pan_coordinates() {
        let placement = image_display_placement(
            ImageFitTarget {
                pixel_width: 1000,
                pixel_height: 600,
                display_width: 1000.0,
                display_height: 600.0,
            },
            400.0,
            300.0,
            ImagePanOffset::default(),
        );
        let horizontal = horizontal_scrollbar_metrics_for_placement(placement).unwrap();
        let vertical = vertical_scrollbar_metrics_for_placement(placement).unwrap();

        let line_right = horizontal.scroll_by(HORIZONTAL_SCROLLBAR_LINE_DELTA);
        assert_eq!(
            pan_offset_x_from_scroll_left(line_right, placement.pan_limit.x),
            -40.0
        );
        let line_down = vertical.scroll_by(IMAGE_VIEWER_SCROLLBAR_LINE_DELTA);
        assert_eq!(
            pan_offset_y_from_scroll_top(line_down, placement.pan_limit.y),
            -40.0
        );
        let page_down = vertical.scroll_by(vertical.viewport_height);
        assert_eq!(
            pan_offset_y_from_scroll_top(page_down, placement.pan_limit.y),
            -150.0
        );

        let thumb_bottom = vertical.track_top + vertical.track_height - vertical.thumb_height;
        assert_eq!(
            pan_offset_y_from_scroll_top(
                vertical.scroll_top_for_thumb_top(thumb_bottom),
                placement.pan_limit.y
            ),
            -150.0
        );
    }

    #[test]
    fn zoom_anchor_preserves_viewed_image_point_then_clamps() {
        let old_target = ImageFitTarget {
            pixel_width: 400,
            pixel_height: 300,
            display_width: 400.0,
            display_height: 300.0,
        };
        let new_target = ImageFitTarget {
            pixel_width: 800,
            pixel_height: 600,
            display_width: 800.0,
            display_height: 600.0,
        };

        let centered = pan_offset_after_zoom(
            ImagePanOffset::default(),
            old_target,
            new_target,
            400.0,
            300.0,
            ImageBodyPoint { x: 200.0, y: 150.0 },
        );
        assert_eq!(centered, ImagePanOffset::default());

        let anchored = pan_offset_after_zoom(
            ImagePanOffset::default(),
            old_target,
            new_target,
            400.0,
            300.0,
            ImageBodyPoint { x: 300.0, y: 150.0 },
        );
        assert_eq!(anchored, ImagePanOffset { x: -100.0, y: 0.0 });

        let clamped = pan_offset_after_zoom(
            ImagePanOffset { x: -200.0, y: 0.0 },
            old_target,
            new_target,
            400.0,
            300.0,
            ImageBodyPoint { x: 400.0, y: 150.0 },
        );
        assert_eq!(clamped, ImagePanOffset { x: -200.0, y: 0.0 });
    }

    #[test]
    fn wheel_zoom_accumulates_thresholds_and_resets_on_direction_change() {
        let mut accumulator = 0.0;

        assert_eq!(wheel_zoom_steps(&mut accumulator, 60.0), 0);
        assert_eq!(accumulator, 60.0);
        assert_eq!(wheel_zoom_steps(&mut accumulator, 60.0), 1);
        assert_eq!(accumulator, 0.0);
        assert_eq!(wheel_zoom_steps(&mut accumulator, -240.0), -2);
        assert_eq!(accumulator, 0.0);
        assert_eq!(wheel_zoom_steps(&mut accumulator, 40.0), 0);
        assert_eq!(accumulator, 40.0);
        assert_eq!(wheel_zoom_steps(&mut accumulator, -40.0), 0);
        assert_eq!(accumulator, -40.0);
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
