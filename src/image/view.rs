use std::{fs, path::PathBuf, sync::Arc};

use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, FocusHandle, Focusable, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement, Pixels, Point, Render,
    RenderImage, ScrollWheelEvent, SharedString, Styled, Task, TitlebarOptions, Window,
    WindowBounds, WindowDecorations, WindowOptions, canvas, div, img, point, prelude::*, px, rgb,
    size,
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
        ImageZoomIn, ImageZoomOut,
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
const IMAGE_VIEWER_MIN_ZOOM: f64 = 0.02;
const IMAGE_VIEWER_FINE_ZOOM_LIMIT: f64 = 0.10;
const IMAGE_VIEWER_FINE_ZOOM_STEP: f64 = 0.01;
const IMAGE_VIEWER_ZOOM_STEP: f64 = 0.10;
const IMAGE_VIEWER_MAX_ZOOM: f64 = 28.0;
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
        let (available_width, available_height) = current_body_available_size(window);
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
        let (available_width, available_height) = current_body_available_size(window);
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
        let mut can_pan = false;
        let content = match &self.state {
            ImageViewerState::Loading => image_viewer_status("Loading image..."),
            ImageViewerState::Failed(error) => {
                image_viewer_status(format!("Cannot display {}: {error}", self.title))
            }
            ImageViewerState::Ready(_) => {
                match self.render_ready_body(available_width, available_height, scale_factor, cx) {
                    Some((content, ready_can_pan)) => {
                        can_pan = ready_can_pan;
                        content
                    }
                    None => image_viewer_status("Cannot display image."),
                }
            }
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
            .child(self.render_body_hit_layer(can_pan, cx))
            .into_any_element()
    }

    fn render_ready_body(
        &mut self,
        available_width: f32,
        available_height: f32,
        scale_factor: f32,
        cx: &mut Context<Self>,
    ) -> Option<(AnyElement, bool)> {
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
        let placement =
            image_display_placement(target, available_width, available_height, self.pan_offset);
        self.pan_offset = placement.offset;

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

        Some((content, placement.can_pan()))
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
            ))
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
                    if !bounds.contains(&event.position) || !event.modifiers.secondary() {
                        return;
                    }

                    let anchor = local_body_point(event.position, &bounds);
                    let delta_y = f32::from(
                        event
                            .delta
                            .pixel_delta(px(IMAGE_VIEWER_WHEEL_LINE_HEIGHT))
                            .y,
                    );
                    let _ = entity.update(cx, |this, cx| {
                        if this.handle_wheel_zoom(delta_y, anchor, bounds.size, window) {
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
        if zoom < IMAGE_VIEWER_FINE_ZOOM_LIMIT - ZOOM_EPSILON {
            let next = stepped_zoom(zoom, IMAGE_VIEWER_FINE_ZOOM_STEP, 1.0);
            return next.clamp(IMAGE_VIEWER_MIN_ZOOM, IMAGE_VIEWER_FINE_ZOOM_LIMIT);
        }
        let next = stepped_zoom(zoom, IMAGE_VIEWER_ZOOM_STEP, 1.0);
        return next.clamp(IMAGE_VIEWER_MIN_ZOOM, IMAGE_VIEWER_MAX_ZOOM);
    }

    if direction < 0 {
        if zoom <= IMAGE_VIEWER_MIN_ZOOM + ZOOM_EPSILON {
            return zoom;
        }
        if zoom <= IMAGE_VIEWER_FINE_ZOOM_LIMIT + ZOOM_EPSILON {
            let next = stepped_zoom(zoom, IMAGE_VIEWER_FINE_ZOOM_STEP, -1.0);
            return next.clamp(IMAGE_VIEWER_MIN_ZOOM, IMAGE_VIEWER_FINE_ZOOM_LIMIT);
        }
        let next = stepped_zoom(zoom, IMAGE_VIEWER_ZOOM_STEP, -1.0);
        return next.clamp(IMAGE_VIEWER_MIN_ZOOM, IMAGE_VIEWER_MAX_ZOOM);
    }

    zoom
}

fn stepped_zoom(zoom: f64, step: f64, direction: f64) -> f64 {
    let step_count = if direction > 0.0 {
        ((zoom + ZOOM_EPSILON) / step).floor() + 1.0
    } else {
        ((zoom - ZOOM_EPSILON) / step).ceil() - 1.0
    };
    ((step_count * step) * 100.0).round() / 100.0
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
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(0xffffff))
            .child(self.render_titlebar(window, cx))
            .child(self.render_body(window, cx))
            .child(self.render_status_bar(self.current_render_target(window)))
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
    fn zoom_levels_use_requested_min_max_and_steps() {
        assert_eq!(next_zoom_level(0.01, 1), 0.02);
        assert_eq!(next_zoom_level(0.02, -1), 0.02);
        assert_eq!(next_zoom_level(27.95, 1), 28.0);
        assert_eq!(next_zoom_level(28.0, 1), 28.0);
        assert_eq!(next_zoom_level(0.05, 1), 0.06);
        assert_eq!(next_zoom_level(0.06, -1), 0.05);
        assert_eq!(next_zoom_level(0.10, -1), 0.09);
        assert_eq!(next_zoom_level(0.10, 1), 0.20);
        assert_eq!(next_zoom_level(0.30, 1), 0.40);
        assert_eq!(next_zoom_level(0.30, -1), 0.20);
    }

    #[test]
    fn zoom_levels_snap_arbitrary_fit_scales() {
        assert_eq!(next_zoom_level(0.333, 1), 0.4);
        assert_eq!(next_zoom_level(0.333, -1), 0.3);
        assert_eq!(next_zoom_level(0.055, 1), 0.06);
        assert_eq!(next_zoom_level(0.055, -1), 0.05);
        assert_eq!(next_zoom_level(0.005, 1), 0.02);
        assert_eq!(next_zoom_level(0.005, -1), 0.005);
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
