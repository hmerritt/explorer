use gpui::{
    AnyElement, Context, Decorations, Div, IntoElement, MouseButton, MouseDownEvent, ParentElement,
    Stateful, Styled, Window, WindowControlArea, div, font, prelude::*, px, rgb,
};
#[cfg(target_os = "linux")]
use gpui::{CursorStyle, transparent_black};
#[cfg(any(target_os = "linux", test))]
use gpui::{ResizeEdge, Tiling, WindowControls};

pub(crate) const TITLEBAR_HEIGHT: f32 = 36.0;
pub(crate) const MAC_TRAFFIC_LIGHT_PADDING: f32 = 83.0;

const TITLEBAR_DRAG_MIN_WIDTH: f32 = 36.0;
const WINDOW_CONTROL_WIDTH: f32 = 36.0;
#[cfg(target_os = "linux")]
const CLIENT_DECORATION_INSET: f32 = 8.0;
#[cfg(target_os = "linux")]
const CLIENT_DECORATION_ROUNDING: f32 = 8.0;
const WINDOW_MINIMIZE_GLYPH: &str = "\u{E921}";
const WINDOW_MAXIMIZE_GLYPH: &str = "\u{E922}";
const WINDOW_RESTORE_GLYPH: &str = "\u{E923}";
const WINDOW_CLOSE_GLYPH: &str = "\u{E8BB}";

pub(crate) trait WindowDragState {
    fn set_window_drag_pending(&mut self, pending: bool);
    fn take_window_drag_pending(&mut self) -> bool;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowCaptionButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl WindowCaptionButton {
    fn id(self) -> &'static str {
        match self {
            Self::Minimize => "explorer-window-minimize",
            Self::Maximize => "explorer-window-maximize",
            Self::Restore => "explorer-window-restore",
            Self::Close => "explorer-window-close",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Minimize => WINDOW_MINIMIZE_GLYPH,
            Self::Maximize => WINDOW_MAXIMIZE_GLYPH,
            Self::Restore => WINDOW_RESTORE_GLYPH,
            Self::Close => WINDOW_CLOSE_GLYPH,
        }
    }

    #[cfg(target_os = "windows")]
    fn control_area(self) -> WindowControlArea {
        match self {
            Self::Minimize => WindowControlArea::Min,
            Self::Maximize | Self::Restore => WindowControlArea::Max,
            Self::Close => WindowControlArea::Close,
        }
    }
}

fn maximize_caption_button(is_maximized: bool) -> WindowCaptionButton {
    if is_maximized {
        WindowCaptionButton::Restore
    } else {
        WindowCaptionButton::Maximize
    }
}

#[cfg(any(target_os = "windows", test))]
fn windows_caption_buttons(is_maximized: bool, is_fullscreen: bool) -> Vec<WindowCaptionButton> {
    if is_fullscreen {
        return Vec::new();
    }

    vec![
        WindowCaptionButton::Minimize,
        maximize_caption_button(is_maximized),
        WindowCaptionButton::Close,
    ]
}

#[cfg(any(target_os = "linux", test))]
fn linux_caption_buttons(
    decorations: Decorations,
    controls: WindowControls,
    is_maximized: bool,
    is_fullscreen: bool,
) -> Vec<WindowCaptionButton> {
    if is_fullscreen || matches!(decorations, Decorations::Server) {
        return Vec::new();
    }

    let mut buttons = Vec::with_capacity(3);
    if controls.minimize {
        buttons.push(WindowCaptionButton::Minimize);
    }
    if controls.maximize {
        buttons.push(maximize_caption_button(is_maximized));
    }
    buttons.push(WindowCaptionButton::Close);
    buttons
}

pub(crate) fn render_titlebar_drag_surface<T: WindowDragState + 'static>(
    id: &'static str,
    decorations: Decorations,
    cx: &mut Context<T>,
) -> Stateful<Div> {
    div()
        .id(id)
        .window_control_area(WindowControlArea::Drag)
        .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, _| {
            if event.button == MouseButton::Left {
                this.set_window_drag_pending(false);
            }
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _, _, _| this.set_window_drag_pending(false)),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, _| this.set_window_drag_pending(true)),
        )
        .on_mouse_move(cx.listener(|this, _, window, _| {
            if this.take_window_drag_pending() {
                window.start_window_move();
            }
        }))
        .on_click(|event, window, _| {
            if event.click_count() != 2 {
                return;
            }

            if cfg!(target_os = "macos") {
                window.titlebar_double_click();
            } else if cfg!(target_os = "linux") {
                window.zoom_window();
            }
        })
        .when(
            cfg!(target_os = "linux") && matches!(decorations, Decorations::Client { .. }),
            |this| {
                this.on_mouse_down(MouseButton::Right, |event: &MouseDownEvent, window, cx| {
                    if window.window_controls().window_menu {
                        window.show_window_menu(event.position);
                        cx.stop_propagation();
                    }
                })
            },
        )
}

pub(crate) fn render_titlebar_drag_region<T: WindowDragState + 'static>(
    id: &'static str,
    decorations: Decorations,
    cx: &mut Context<T>,
) -> AnyElement {
    render_titlebar_drag_surface(id, decorations, cx)
        .h_full()
        .min_w(px(TITLEBAR_DRAG_MIN_WIDTH))
        .flex_1()
        .into_any_element()
}

pub(crate) fn render_titlebar_drag_overlay<T: WindowDragState + 'static>(
    id: &'static str,
    decorations: Decorations,
    cx: &mut Context<T>,
) -> AnyElement {
    render_titlebar_drag_surface(id, decorations, cx)
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .size_full()
        .into_any_element()
}

pub(crate) fn render_window_controls(window: &Window) -> Option<AnyElement> {
    #[cfg(target_os = "windows")]
    {
        return windows_window_controls(window);
    }

    #[cfg(target_os = "linux")]
    {
        let buttons = linux_caption_buttons(
            window.window_decorations(),
            window.window_controls(),
            window.is_maximized(),
            window.is_fullscreen(),
        );
        if buttons.is_empty() {
            return None;
        }
        return Some(linux_window_controls(buttons).into_any_element());
    }

    #[cfg(target_os = "macos")]
    {
        let _ = window;
        None
    }
}

#[cfg(target_os = "windows")]
fn windows_window_controls(window: &Window) -> Option<AnyElement> {
    let buttons = windows_caption_buttons(window.is_maximized(), window.is_fullscreen());
    (!buttons.is_empty()).then(|| {
        div()
            .id("explorer-windows-window-controls")
            .flex()
            .flex_row()
            .h_full()
            .flex_none()
            .children(buttons.into_iter().map(windows_caption_button))
            .into_any_element()
    })
}

#[cfg(target_os = "windows")]
fn windows_caption_button(button: WindowCaptionButton) -> AnyElement {
    let is_close = button == WindowCaptionButton::Close;

    div()
        .id(button.id())
        .flex()
        .items_center()
        .justify_center()
        .w(px(WINDOW_CONTROL_WIDTH))
        .h_full()
        .flex_none()
        .occlude()
        .font(titlebar_icon_font())
        .text_size(px(10.0))
        .text_color(rgb(0x202020))
        .window_control_area(button.control_area())
        .when(is_close, |this| {
            this.hover(|style| style.bg(rgb(0xe81123)).text_color(rgb(0xffffff)))
        })
        .when(!is_close, |this| {
            this.hover(|style| style.bg(rgb(0xd8d8d8)))
                .active(|style| style.bg(rgb(0xc8c8c8)))
        })
        .child(button.glyph())
        .into_any_element()
}

#[cfg(target_os = "linux")]
fn linux_window_controls(buttons: Vec<WindowCaptionButton>) -> AnyElement {
    div()
        .id("explorer-linux-window-controls")
        .flex()
        .flex_row()
        .h_full()
        .flex_none()
        .children(buttons.into_iter().map(linux_caption_button))
        .into_any_element()
}

#[cfg(target_os = "linux")]
fn linux_caption_button(button: WindowCaptionButton) -> AnyElement {
    let is_close = button == WindowCaptionButton::Close;

    div()
        .id(button.id())
        .flex()
        .items_center()
        .justify_center()
        .w(px(WINDOW_CONTROL_WIDTH))
        .h_full()
        .flex_none()
        .occlude()
        .font(titlebar_icon_font())
        .text_size(px(10.0))
        .text_color(rgb(0x202020))
        .when(is_close, |this| {
            this.hover(|style| style.bg(rgb(0xe81123)).text_color(rgb(0xffffff)))
                .active(|style| style.bg(rgb(0xc50f1f)).text_color(rgb(0xffffff)))
        })
        .when(!is_close, |this| {
            this.hover(|style| style.bg(rgb(0xd8d8d8)))
                .active(|style| style.bg(rgb(0xc8c8c8)))
        })
        .child(button.glyph())
        .on_click(move |_, window, cx| {
            match button {
                WindowCaptionButton::Minimize => window.minimize_window(),
                WindowCaptionButton::Maximize | WindowCaptionButton::Restore => {
                    window.zoom_window()
                }
                WindowCaptionButton::Close => window.remove_window(),
            }
            cx.stop_propagation();
        })
        .into_any_element()
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn render_platform_window_frame(
    content: AnyElement,
    _window: &mut Window,
) -> AnyElement {
    content
}

#[cfg(target_os = "linux")]
pub(crate) fn render_platform_window_frame(content: AnyElement, window: &mut Window) -> AnyElement {
    let Some(tiling) = client_decoration_tiling(window.window_decorations()) else {
        window.set_client_inset(px(0.0));
        return content;
    };

    window.set_client_inset(px(CLIENT_DECORATION_INSET));

    let inner = div()
        .relative()
        .size_full()
        .bg(rgb(0xffffff))
        .when(!(tiling.top || tiling.right), |this| {
            this.rounded_tr(px(CLIENT_DECORATION_ROUNDING))
        })
        .when(!(tiling.top || tiling.left), |this| {
            this.rounded_tl(px(CLIENT_DECORATION_ROUNDING))
        })
        .when(!(tiling.bottom || tiling.right), |this| {
            this.rounded_br(px(CLIENT_DECORATION_ROUNDING))
        })
        .when(!(tiling.bottom || tiling.left), |this| {
            this.rounded_bl(px(CLIENT_DECORATION_ROUNDING))
        })
        .when(!tiling.top, |this| this.border_t(px(1.0)))
        .when(!tiling.bottom, |this| this.border_b(px(1.0)))
        .when(!tiling.left, |this| this.border_l(px(1.0)))
        .when(!tiling.right, |this| this.border_r(px(1.0)))
        .border_color(rgb(0xa0a0a0))
        .when(!tiling.is_tiled(), |this| this.shadow_md())
        .overflow_hidden()
        .child(content);

    let mut frame = div()
        .id("explorer-client-decoration-frame")
        .relative()
        .size_full()
        .bg(transparent_black())
        .when(!tiling.top, |this| this.pt(px(CLIENT_DECORATION_INSET)))
        .when(!tiling.bottom, |this| this.pb(px(CLIENT_DECORATION_INSET)))
        .when(!tiling.left, |this| this.pl(px(CLIENT_DECORATION_INSET)))
        .when(!tiling.right, |this| this.pr(px(CLIENT_DECORATION_INSET)))
        .child(inner);

    for edge in [
        ResizeEdge::Top,
        ResizeEdge::Bottom,
        ResizeEdge::Left,
        ResizeEdge::Right,
        ResizeEdge::TopLeft,
        ResizeEdge::TopRight,
        ResizeEdge::BottomLeft,
        ResizeEdge::BottomRight,
    ] {
        if resize_edge_enabled(edge, tiling) {
            frame = frame.child(linux_resize_handle(edge));
        }
    }

    frame.into_any_element()
}

#[cfg(any(target_os = "linux", test))]
fn client_decoration_tiling(decorations: Decorations) -> Option<Tiling> {
    match decorations {
        Decorations::Server => None,
        Decorations::Client { tiling } => Some(tiling),
    }
}

#[cfg(any(target_os = "linux", test))]
fn resize_edge_enabled(edge: ResizeEdge, tiling: Tiling) -> bool {
    match edge {
        ResizeEdge::Top => !tiling.top,
        ResizeEdge::TopRight => !(tiling.top || tiling.right),
        ResizeEdge::Right => !tiling.right,
        ResizeEdge::BottomRight => !(tiling.bottom || tiling.right),
        ResizeEdge::Bottom => !tiling.bottom,
        ResizeEdge::BottomLeft => !(tiling.bottom || tiling.left),
        ResizeEdge::Left => !tiling.left,
        ResizeEdge::TopLeft => !(tiling.top || tiling.left),
    }
}

#[cfg(target_os = "linux")]
fn linux_resize_handle(edge: ResizeEdge) -> AnyElement {
    let inset = px(CLIENT_DECORATION_INSET);
    let handle = div()
        .id(resize_edge_id(edge))
        .absolute()
        .cursor(resize_edge_cursor(edge))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.start_window_resize(edge);
            cx.stop_propagation();
        });

    match edge {
        ResizeEdge::Top => handle.left(px(0.0)).top(px(0.0)).w_full().h(inset),
        ResizeEdge::TopRight => handle.right(px(0.0)).top(px(0.0)).size(inset),
        ResizeEdge::Right => handle.right(px(0.0)).top(px(0.0)).h_full().w(inset),
        ResizeEdge::BottomRight => handle.right(px(0.0)).bottom(px(0.0)).size(inset),
        ResizeEdge::Bottom => handle.left(px(0.0)).bottom(px(0.0)).w_full().h(inset),
        ResizeEdge::BottomLeft => handle.left(px(0.0)).bottom(px(0.0)).size(inset),
        ResizeEdge::Left => handle.left(px(0.0)).top(px(0.0)).h_full().w(inset),
        ResizeEdge::TopLeft => handle.left(px(0.0)).top(px(0.0)).size(inset),
    }
    .into_any_element()
}

#[cfg(target_os = "linux")]
fn resize_edge_id(edge: ResizeEdge) -> &'static str {
    match edge {
        ResizeEdge::Top => "explorer-window-resize-top",
        ResizeEdge::TopRight => "explorer-window-resize-top-right",
        ResizeEdge::Right => "explorer-window-resize-right",
        ResizeEdge::BottomRight => "explorer-window-resize-bottom-right",
        ResizeEdge::Bottom => "explorer-window-resize-bottom",
        ResizeEdge::BottomLeft => "explorer-window-resize-bottom-left",
        ResizeEdge::Left => "explorer-window-resize-left",
        ResizeEdge::TopLeft => "explorer-window-resize-top-left",
    }
}

#[cfg(target_os = "linux")]
fn resize_edge_cursor(edge: ResizeEdge) -> CursorStyle {
    match edge {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
    }
}

pub(crate) fn titlebar_icon_font() -> gpui::Font {
    font("Segoe Fluent Icons")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_caption_buttons_match_explorer_order() {
        assert_eq!(
            windows_caption_buttons(false, false),
            vec![
                WindowCaptionButton::Minimize,
                WindowCaptionButton::Maximize,
                WindowCaptionButton::Close,
            ]
        );
        assert_eq!(
            windows_caption_buttons(true, false),
            vec![
                WindowCaptionButton::Minimize,
                WindowCaptionButton::Restore,
                WindowCaptionButton::Close,
            ]
        );
        assert!(windows_caption_buttons(false, true).is_empty());
        assert_eq!(WindowCaptionButton::Maximize.glyph(), WINDOW_MAXIMIZE_GLYPH);
        assert_eq!(WindowCaptionButton::Restore.glyph(), WINDOW_RESTORE_GLYPH);
    }

    #[test]
    fn linux_caption_buttons_follow_supported_controls_and_decorations() {
        let supported = WindowControls {
            fullscreen: true,
            minimize: true,
            maximize: true,
            window_menu: true,
        };
        let no_maximize = WindowControls {
            maximize: false,
            ..supported
        };
        let client = Decorations::Client {
            tiling: Tiling::default(),
        };

        assert_eq!(
            linux_caption_buttons(client, supported, false, false),
            vec![
                WindowCaptionButton::Minimize,
                WindowCaptionButton::Maximize,
                WindowCaptionButton::Close,
            ]
        );
        assert_eq!(
            linux_caption_buttons(client, supported, true, false),
            vec![
                WindowCaptionButton::Minimize,
                WindowCaptionButton::Restore,
                WindowCaptionButton::Close,
            ]
        );
        assert_eq!(
            linux_caption_buttons(client, no_maximize, false, false),
            vec![WindowCaptionButton::Minimize, WindowCaptionButton::Close]
        );
        assert!(linux_caption_buttons(Decorations::Server, supported, false, false).is_empty());
        assert!(linux_caption_buttons(client, supported, false, true).is_empty());
    }

    #[test]
    fn resize_edges_are_disabled_on_tiled_sides() {
        let tiled = Tiling {
            top: true,
            right: false,
            bottom: false,
            left: true,
        };

        assert!(!resize_edge_enabled(ResizeEdge::Top, tiled));
        assert!(!resize_edge_enabled(ResizeEdge::Left, tiled));
        assert!(!resize_edge_enabled(ResizeEdge::TopLeft, tiled));
        assert!(!resize_edge_enabled(ResizeEdge::TopRight, tiled));
        assert!(resize_edge_enabled(ResizeEdge::Right, tiled));
        assert!(resize_edge_enabled(ResizeEdge::Bottom, tiled));
        assert!(resize_edge_enabled(ResizeEdge::BottomRight, tiled));
        assert!(!resize_edge_enabled(ResizeEdge::BottomLeft, tiled));
    }

    #[test]
    fn client_decoration_tiling_only_exists_for_client_decorations() {
        let tiling = Tiling {
            top: true,
            ..Tiling::default()
        };
        assert_eq!(client_decoration_tiling(Decorations::Server), None);
        assert_eq!(
            client_decoration_tiling(Decorations::Client { tiling }),
            Some(tiling)
        );
    }

    #[test]
    fn titlebar_icon_font_remains_dedicated() {
        assert_eq!(titlebar_icon_font().family, "Segoe Fluent Icons");
    }
}
