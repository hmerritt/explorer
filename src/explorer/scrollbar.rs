use gpui::{
    AnyElement, Context, Div, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, canvas,
    div, point, prelude::*, px, rgb,
};

use crate::explorer::{
    constants::{
        HORIZONTAL_SCROLLBAR_LINE_DELTA, SCROLLBAR_ARROW_COLOR, SCROLLBAR_ARROW_HEIGHT,
        SCROLLBAR_ARROW_HOVER_BG, SCROLLBAR_GUTTER_WIDTH, SCROLLBAR_MIN_THUMB_HEIGHT,
        SCROLLBAR_THUMB_ACTIVE_BG, SCROLLBAR_THUMB_BG, SCROLLBAR_THUMB_HOVER_BG,
        SCROLLBAR_THUMB_HOVER_WIDTH, SCROLLBAR_THUMB_WIDTH, SCROLLBAR_TRACK_BG,
    },
    icons::nav_icon_font,
    view::ExplorerView,
};
use crate::settings::FileViewMode;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollbarDrag {
    pub(crate) pointer_offset_from_thumb_top: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HorizontalScrollbarDrag {
    pub(crate) pointer_offset_from_thumb_left: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollbarMetrics {
    pub(crate) viewport_width: f32,
    pub(crate) viewport_height: f32,
    pub(crate) content_height: f32,
    pub(crate) scroll_top: f32,
    pub(crate) scroll_max: f32,
    pub(crate) track_top: f32,
    pub(crate) track_height: f32,
    pub(crate) thumb_top: f32,
    pub(crate) thumb_height: f32,
}

impl ScrollbarMetrics {
    pub(crate) fn new(
        viewport_width: f32,
        viewport_height: f32,
        content_height: f32,
        scroll_top: f32,
    ) -> Option<Self> {
        if viewport_height <= 0.0 || content_height <= viewport_height {
            return None;
        }

        let track_top = SCROLLBAR_ARROW_HEIGHT;
        let track_height = viewport_height - (SCROLLBAR_ARROW_HEIGHT * 2.0);
        if track_height <= 0.0 {
            return None;
        }

        let scroll_max = content_height - viewport_height;
        let scroll_top = scroll_top.clamp(0.0, scroll_max);
        let thumb_height = (track_height * viewport_height / content_height)
            .clamp(SCROLLBAR_MIN_THUMB_HEIGHT.min(track_height), track_height);
        let thumb_travel = track_height - thumb_height;
        let thumb_top = if thumb_travel <= 0.0 {
            track_top
        } else {
            track_top + (scroll_top / scroll_max) * thumb_travel
        };

        Some(Self {
            viewport_width,
            viewport_height,
            content_height,
            scroll_top,
            scroll_max,
            track_top,
            track_height,
            thumb_top,
            thumb_height,
        })
    }

    pub(crate) fn thumb_bottom(self) -> f32 {
        self.thumb_top + self.thumb_height
    }

    pub(crate) fn clamp_scroll_top(self, scroll_top: f32) -> f32 {
        scroll_top.clamp(0.0, self.scroll_max)
    }

    pub(crate) fn scroll_by(self, delta: f32) -> f32 {
        self.clamp_scroll_top(self.scroll_top + delta)
    }

    pub(crate) fn scroll_top_for_thumb_top(self, thumb_top: f32) -> f32 {
        let thumb_travel = self.track_height - self.thumb_height;
        if thumb_travel <= 0.0 {
            return 0.0;
        }

        let thumb_top = thumb_top.clamp(self.track_top, self.track_top + thumb_travel);
        ((thumb_top - self.track_top) / thumb_travel) * self.scroll_max
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScrollbarArrow {
    Up,
    Down,
    Left,
    Right,
}

impl ScrollbarArrow {
    pub(crate) fn glyph(self) -> &'static str {
        match self {
            Self::Up => "\u{E70E}",
            Self::Down => "\u{E70D}",
            Self::Left => "\u{E76B}",
            Self::Right => "\u{E76C}",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HorizontalScrollbarMetrics {
    pub(crate) viewport_width: f32,
    pub(crate) content_width: f32,
    pub(crate) scroll_left: f32,
    pub(crate) scroll_max: f32,
    pub(crate) track_left: f32,
    pub(crate) track_width: f32,
    pub(crate) thumb_left: f32,
    pub(crate) thumb_width: f32,
}

impl HorizontalScrollbarMetrics {
    pub(crate) fn new(viewport_width: f32, content_width: f32, scroll_left: f32) -> Option<Self> {
        if viewport_width <= 0.0 || content_width <= viewport_width {
            return None;
        }

        let track_left = SCROLLBAR_ARROW_HEIGHT;
        let track_width = viewport_width - (SCROLLBAR_ARROW_HEIGHT * 2.0);
        if track_width <= 0.0 {
            return None;
        }

        let scroll_max = content_width - viewport_width;
        let scroll_left = scroll_left.clamp(0.0, scroll_max);
        let thumb_width = (track_width * viewport_width / content_width)
            .clamp(SCROLLBAR_MIN_THUMB_HEIGHT.min(track_width), track_width);
        let thumb_travel = track_width - thumb_width;
        let thumb_left = if thumb_travel <= 0.0 {
            track_left
        } else {
            track_left + (scroll_left / scroll_max) * thumb_travel
        };

        Some(Self {
            viewport_width,
            content_width,
            scroll_left,
            scroll_max,
            track_left,
            track_width,
            thumb_left,
            thumb_width,
        })
    }

    pub(crate) fn thumb_right(self) -> f32 {
        self.thumb_left + self.thumb_width
    }

    pub(crate) fn clamp_scroll_left(self, scroll_left: f32) -> f32 {
        scroll_left.clamp(0.0, self.scroll_max)
    }

    pub(crate) fn scroll_by(self, delta: f32) -> f32 {
        self.clamp_scroll_left(self.scroll_left + delta)
    }

    pub(crate) fn scroll_left_for_thumb_left(self, thumb_left: f32) -> f32 {
        let thumb_travel = self.track_width - self.thumb_width;
        if thumb_travel <= 0.0 {
            return 0.0;
        }

        let thumb_left = thumb_left.clamp(self.track_left, self.track_left + thumb_travel);
        ((thumb_left - self.track_left) / thumb_travel) * self.scroll_max
    }
}

fn clamped_visible_horizontal_scroll_offset(
    viewport_width: f32,
    content_width: f32,
    raw_scroll_left: f32,
) -> f32 {
    HorizontalScrollbarMetrics::new(viewport_width, content_width, raw_scroll_left)
        .map_or(0.0, |metrics| metrics.scroll_left)
}

impl ExplorerView {
    pub(super) fn scroll_to_top(&self) {
        if self.view_mode == FileViewMode::LargeIcons {
            self.large_icon_list_state
                .set_offset_from_scrollbar(point(px(0.0), px(0.0)));
            return;
        }

        let scroll_handle = self.scroll_handle.0.borrow().base_handle.clone();
        scroll_handle.set_offset(point(px(0.0), px(0.0)));
    }

    pub(super) fn set_scroll_offset(&self, scroll_top: f32) {
        if self.view_mode == FileViewMode::LargeIcons {
            self.large_icon_list_state
                .set_offset_from_scrollbar(point(px(0.0), px(-scroll_top.max(0.0))));
            return;
        }

        let scroll_handle = self.scroll_handle.0.borrow().base_handle.clone();
        let offset = scroll_handle.offset();
        scroll_handle.set_offset(point(offset.x, px(-scroll_top.max(0.0))));
    }

    pub(super) fn scrollbar_metrics(&self) -> Option<ScrollbarMetrics> {
        if self.view_mode == FileViewMode::LargeIcons {
            let bounds = self.large_icon_list_state.viewport_bounds();
            let viewport_width = f32::from(bounds.size.width);
            let viewport_height = f32::from(bounds.size.height);
            let scroll_max =
                f32::from(self.large_icon_list_state.max_offset_for_scrollbar().height);
            let content_height = viewport_height + scroll_max;
            let scroll_top = -f32::from(
                self.large_icon_list_state
                    .scroll_px_offset_for_scrollbar()
                    .y,
            );

            return ScrollbarMetrics::new(
                viewport_width,
                viewport_height,
                content_height,
                scroll_top,
            );
        }

        let scroll_state = self.scroll_handle.0.borrow();
        let item_size = scroll_state.last_item_size?;
        let viewport_width = f32::from(item_size.item.width);
        let viewport_height = f32::from(item_size.item.height);
        let content_height = f32::from(item_size.contents.height);
        let scroll_top = -f32::from(scroll_state.base_handle.offset().y);

        ScrollbarMetrics::new(viewport_width, viewport_height, content_height, scroll_top)
    }

    pub(super) fn scrollbar_is_hovered_or_dragged(&self) -> bool {
        self.scrollbar_hovered || self.scrollbar_drag.is_some()
    }

    pub(super) fn visible_horizontal_scroll_offset(&self) -> f32 {
        let scroll_state = self.scroll_handle.0.borrow();
        let Some(item_size) = scroll_state.last_item_size else {
            return 0.0;
        };
        let raw_scroll_left = -f32::from(scroll_state.base_handle.offset().x);

        clamped_visible_horizontal_scroll_offset(
            f32::from(item_size.item.width),
            f32::from(item_size.contents.width),
            raw_scroll_left,
        )
    }

    pub(super) fn set_horizontal_scroll_offset(&self, scroll_left: f32) {
        let scroll_handle = self.scroll_handle.0.borrow().base_handle.clone();
        let offset = scroll_handle.offset();
        scroll_handle.set_offset(point(px(-scroll_left.max(0.0)), offset.y));
    }

    pub(super) fn horizontal_scrollbar_metrics(&self) -> Option<HorizontalScrollbarMetrics> {
        let scroll_state = self.scroll_handle.0.borrow();
        let item_size = scroll_state.last_item_size?;
        let viewport_width = f32::from(item_size.item.width);
        let content_width = f32::from(item_size.contents.width);
        let scroll_left = -f32::from(scroll_state.base_handle.offset().x);

        HorizontalScrollbarMetrics::new(viewport_width, content_width, scroll_left)
    }

    pub(super) fn handle_horizontal_scrollbar_mouse_down(
        &mut self,
        local_x: f32,
        metrics: HorizontalScrollbarMetrics,
    ) {
        if local_x < SCROLLBAR_ARROW_HEIGHT {
            self.set_horizontal_scroll_offset(metrics.scroll_by(-HORIZONTAL_SCROLLBAR_LINE_DELTA));
        } else if local_x > metrics.viewport_width - SCROLLBAR_ARROW_HEIGHT {
            self.set_horizontal_scroll_offset(metrics.scroll_by(HORIZONTAL_SCROLLBAR_LINE_DELTA));
        } else if local_x >= metrics.thumb_left && local_x <= metrics.thumb_right() {
            self.horizontal_scrollbar_drag = Some(HorizontalScrollbarDrag {
                pointer_offset_from_thumb_left: local_x - metrics.thumb_left,
            });
        } else if local_x < metrics.thumb_left {
            self.set_horizontal_scroll_offset(metrics.scroll_by(-metrics.viewport_width));
        } else {
            self.set_horizontal_scroll_offset(metrics.scroll_by(metrics.viewport_width));
        }
    }

    pub(super) fn handle_horizontal_scrollbar_drag(
        &mut self,
        local_x: f32,
        metrics: HorizontalScrollbarMetrics,
    ) {
        let Some(drag) = self.horizontal_scrollbar_drag else {
            return;
        };

        let thumb_left = local_x - drag.pointer_offset_from_thumb_left;
        self.set_horizontal_scroll_offset(metrics.scroll_left_for_thumb_left(thumb_left));
    }

    pub(super) fn handle_scrollbar_mouse_down(&mut self, local_y: f32, metrics: ScrollbarMetrics) {
        if local_y < SCROLLBAR_ARROW_HEIGHT {
            self.set_scroll_offset(metrics.scroll_by(-self.entry_row_height()));
        } else if local_y > metrics.viewport_height - SCROLLBAR_ARROW_HEIGHT {
            self.set_scroll_offset(metrics.scroll_by(self.entry_row_height()));
        } else if local_y >= metrics.thumb_top && local_y <= metrics.thumb_bottom() {
            if self.view_mode == FileViewMode::LargeIcons {
                self.large_icon_list_state.scrollbar_drag_started();
            }
            self.scrollbar_drag = Some(ScrollbarDrag {
                pointer_offset_from_thumb_top: local_y - metrics.thumb_top,
            });
        } else if local_y < metrics.thumb_top {
            self.set_scroll_offset(metrics.scroll_by(-metrics.viewport_height));
        } else {
            self.set_scroll_offset(metrics.scroll_by(metrics.viewport_height));
        }
    }

    pub(super) fn handle_scrollbar_drag(&mut self, local_y: f32, metrics: ScrollbarMetrics) {
        let Some(drag) = self.scrollbar_drag else {
            return;
        };

        let thumb_top = local_y - drag.pointer_offset_from_thumb_top;
        self.set_scroll_offset(metrics.scroll_top_for_thumb_top(thumb_top));
    }

    pub(super) fn render_scrollbar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(metrics) = self.scrollbar_metrics() else {
            return div()
                .id("explorer-scrollbar")
                .w(px(SCROLLBAR_GUTTER_WIDTH))
                .h_full()
                .flex_shrink_0()
                .into_any_element();
        };

        let hovered_or_dragged = self.scrollbar_is_hovered_or_dragged();
        let thumb_width = if hovered_or_dragged {
            SCROLLBAR_THUMB_HOVER_WIDTH
        } else {
            SCROLLBAR_THUMB_WIDTH
        };
        let thumb_right = (SCROLLBAR_GUTTER_WIDTH - thumb_width) / 2.0;
        let thumb_color = if self.scrollbar_drag.is_some() {
            SCROLLBAR_THUMB_ACTIVE_BG
        } else if hovered_or_dragged {
            SCROLLBAR_THUMB_HOVER_BG
        } else {
            SCROLLBAR_THUMB_BG
        };
        let bottom_arrow_top = (metrics.viewport_height - SCROLLBAR_ARROW_HEIGHT).max(0.0);

        div()
            .id("explorer-scrollbar")
            .relative()
            .w(px(SCROLLBAR_GUTTER_WIDTH))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(SCROLLBAR_TRACK_BG))
            .cursor_default()
            .block_mouse_except_scroll()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.scrollbar_hovered = *hovered;
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
            .child(self.render_scrollbar_hit_layer(cx))
            .into_any_element()
    }

    pub(super) fn render_scrollbar_hit_layer(&self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();

        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                window.on_mouse_event({
                    let entity = entity.clone();
                    move |event: &MouseDownEvent, _, _, cx| {
                        if event.button != MouseButton::Left || !bounds.contains(&event.position) {
                            return;
                        }

                        let local_y = f32::from(event.position.y - bounds.origin.y);
                        let _ = entity.update(cx, |this, cx| {
                            if let Some(metrics) = this.scrollbar_metrics() {
                                this.handle_scrollbar_mouse_down(local_y, metrics);
                                cx.notify();
                            }
                        });
                    }
                });

                window.on_mouse_event({
                    let entity = entity.clone();
                    move |event: &MouseMoveEvent, _, _, cx| {
                        if !event.dragging() {
                            return;
                        }

                        let local_y = f32::from(event.position.y - bounds.origin.y);
                        let _ = entity.update(cx, |this, cx| {
                            if this.scrollbar_drag.is_none() {
                                return;
                            }

                            if let Some(metrics) = this.scrollbar_metrics() {
                                this.handle_scrollbar_drag(local_y, metrics);
                                cx.notify();
                            }
                        });
                    }
                });

                window.on_mouse_event(move |event: &MouseUpEvent, _, _, cx| {
                    if event.button != MouseButton::Left {
                        return;
                    }

                    let _ = entity.update(cx, |this, cx| {
                        if this.scrollbar_drag.take().is_some() {
                            if this.view_mode == FileViewMode::LargeIcons {
                                this.large_icon_list_state.scrollbar_drag_ended();
                            }
                            cx.notify();
                        }
                    });
                });
            },
        )
        .size_full()
        .into_any_element()
    }

    pub(super) fn render_horizontal_scrollbar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(metrics) = self.horizontal_scrollbar_metrics() else {
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
            .id("explorer-horizontal-scrollbar")
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
            .child(self.render_horizontal_scrollbar_hit_layer(cx))
            .into_any_element()
    }

    fn render_horizontal_scrollbar_hit_layer(&self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();

        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                window.on_mouse_event({
                    let entity = entity.clone();
                    move |event: &MouseDownEvent, _, _, cx| {
                        if event.button != MouseButton::Left || !bounds.contains(&event.position) {
                            return;
                        }

                        let local_x = f32::from(event.position.x - bounds.origin.x);
                        let _ = entity.update(cx, |this, cx| {
                            if let Some(metrics) = this.horizontal_scrollbar_metrics() {
                                this.handle_horizontal_scrollbar_mouse_down(local_x, metrics);
                                cx.notify();
                            }
                        });
                    }
                });

                window.on_mouse_event({
                    let entity = entity.clone();
                    move |event: &MouseMoveEvent, _, _, cx| {
                        if !event.dragging() {
                            return;
                        }

                        let local_x = f32::from(event.position.x - bounds.origin.x);
                        let _ = entity.update(cx, |this, cx| {
                            if this.horizontal_scrollbar_drag.is_none() {
                                return;
                            }

                            if let Some(metrics) = this.horizontal_scrollbar_metrics() {
                                this.handle_horizontal_scrollbar_drag(local_x, metrics);
                                cx.notify();
                            }
                        });
                    }
                });

                window.on_mouse_event(move |event: &MouseUpEvent, _, _, cx| {
                    if event.button != MouseButton::Left {
                        return;
                    }

                    let _ = entity.update(cx, |this, cx| {
                        if this.horizontal_scrollbar_drag.take().is_some() {
                            cx.notify();
                        }
                    });
                });
            },
        )
        .size_full()
        .into_any_element()
    }
}

pub(crate) fn scrollbar_arrow_button(top: f32, arrow: ScrollbarArrow) -> Div {
    div()
        .absolute()
        .top(px(top))
        .right(px(0.0))
        .flex()
        .items_center()
        .justify_center()
        .w(px(SCROLLBAR_GUTTER_WIDTH))
        .h(px(SCROLLBAR_ARROW_HEIGHT))
        .font(nav_icon_font())
        .text_size(px(8.0))
        .text_color(rgb(SCROLLBAR_ARROW_COLOR))
        .hover(|style| style.bg(rgb(SCROLLBAR_ARROW_HOVER_BG)))
        .child(arrow.glyph())
}

pub(super) fn scrollbar_header_spacer() -> Div {
    div()
        .h_full()
        .w(px(SCROLLBAR_GUTTER_WIDTH))
        .flex_shrink_0()
        .bg(rgb(0xffffff))
}

pub(crate) fn horizontal_scrollbar_arrow_button(left: f32, arrow: ScrollbarArrow) -> Div {
    div()
        .absolute()
        .left(px(left))
        .top(px(0.0))
        .flex()
        .items_center()
        .justify_center()
        .w(px(SCROLLBAR_ARROW_HEIGHT))
        .h(px(SCROLLBAR_GUTTER_WIDTH))
        .font(nav_icon_font())
        .text_size(px(8.0))
        .text_color(rgb(SCROLLBAR_ARROW_COLOR))
        .hover(|style| style.bg(rgb(SCROLLBAR_ARROW_HOVER_BG)))
        .child(arrow.glyph())
}

pub(crate) fn scrollbar_corner() -> Div {
    div()
        .w(px(SCROLLBAR_GUTTER_WIDTH))
        .h(px(SCROLLBAR_GUTTER_WIDTH))
        .flex_shrink_0()
        .bg(rgb(SCROLLBAR_TRACK_BG))
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::explorer::{
        constants::{
            HORIZONTAL_SCROLLBAR_LINE_DELTA, ROW_HEIGHT, SCROLLBAR_ARROW_HEIGHT,
            SCROLLBAR_GUTTER_WIDTH, SCROLLBAR_MIN_THUMB_HEIGHT, SCROLLBAR_THUMB_HOVER_WIDTH,
            SCROLLBAR_THUMB_WIDTH, minimum_file_columns_width,
        },
        test_support::assert_approx_eq,
    };

    #[test]
    fn scrollbar_metrics_hide_without_overflow() {
        assert!(ScrollbarMetrics::new(200.0, 200.0, 200.0, 0.0).is_none());
        assert!(ScrollbarMetrics::new(200.0, 200.0, 180.0, 0.0).is_none());
    }

    #[test]
    fn scrollbar_thumb_is_proportional_and_respects_minimum_height() {
        let proportional = ScrollbarMetrics::new(200.0, 200.0, 400.0, 0.0).unwrap();
        assert_approx_eq(proportional.thumb_height, 84.0);

        let minimum = ScrollbarMetrics::new(100.0, 100.0, 10_000.0, 0.0).unwrap();
        assert_approx_eq(minimum.thumb_height, SCROLLBAR_MIN_THUMB_HEIGHT);
    }

    #[test]
    fn scrollbar_thumb_top_clamps_to_scroll_bounds() {
        let top = ScrollbarMetrics::new(200.0, 200.0, 1_000.0, -50.0).unwrap();
        assert_approx_eq(top.scroll_top, 0.0);
        assert_approx_eq(top.thumb_top, SCROLLBAR_ARROW_HEIGHT);

        let bottom = ScrollbarMetrics::new(200.0, 200.0, 1_000.0, 900.0).unwrap();
        assert_approx_eq(bottom.scroll_top, 800.0);
        assert_approx_eq(
            bottom.thumb_bottom(),
            SCROLLBAR_ARROW_HEIGHT + bottom.track_height,
        );
    }

    #[test]
    fn scrollbar_drag_positions_map_to_scroll_offsets() {
        let metrics = ScrollbarMetrics::new(200.0, 200.0, 1_000.0, 0.0).unwrap();
        let bottom_thumb_top = metrics.track_top + metrics.track_height - metrics.thumb_height;
        let middle_thumb_top = metrics.track_top + (bottom_thumb_top - metrics.track_top) / 2.0;

        assert_approx_eq(metrics.scroll_top_for_thumb_top(metrics.track_top), 0.0);
        assert_approx_eq(
            metrics.scroll_top_for_thumb_top(middle_thumb_top),
            metrics.scroll_max / 2.0,
        );
        assert_approx_eq(
            metrics.scroll_top_for_thumb_top(bottom_thumb_top),
            metrics.scroll_max,
        );
    }

    #[test]
    fn scrollbar_line_and_page_deltas_clamp_at_bounds() {
        let top = ScrollbarMetrics::new(200.0, 200.0, 1_000.0, 0.0).unwrap();
        assert_approx_eq(top.scroll_by(-ROW_HEIGHT), 0.0);
        assert_approx_eq(top.scroll_by(200.0), 200.0);

        let bottom = ScrollbarMetrics::new(200.0, 200.0, 1_000.0, 800.0).unwrap();
        assert_approx_eq(bottom.scroll_by(ROW_HEIGHT), bottom.scroll_max);
        assert_approx_eq(bottom.scroll_by(-200.0), 600.0);
    }

    #[test]
    fn scrollbar_widths_match_reserved_layout_behavior() {
        assert_eq!(SCROLLBAR_THUMB_WIDTH, 4.0);
        assert_eq!(SCROLLBAR_THUMB_HOVER_WIDTH, 6.0);
        assert!(SCROLLBAR_THUMB_HOVER_WIDTH > SCROLLBAR_THUMB_WIDTH);
        assert_eq!(SCROLLBAR_GUTTER_WIDTH, 18.0);
        assert!(SCROLLBAR_GUTTER_WIDTH > SCROLLBAR_THUMB_HOVER_WIDTH);
    }

    #[test]
    fn horizontal_scrollbar_metrics_hide_without_overflow() {
        let content_width = minimum_file_columns_width();

        assert!(HorizontalScrollbarMetrics::new(content_width, content_width, 0.0).is_none());
        assert!(HorizontalScrollbarMetrics::new(content_width + 1.0, content_width, 0.0).is_none());
        assert!(HorizontalScrollbarMetrics::new(content_width - 1.0, content_width, 0.0).is_some());
    }

    #[test]
    fn visible_horizontal_scroll_offset_clamps_trackpad_overscroll() {
        assert_eq!(
            clamped_visible_horizontal_scroll_offset(400.0, 670.0, -50.0),
            0.0
        );
        assert_eq!(
            clamped_visible_horizontal_scroll_offset(400.0, 670.0, 500.0),
            270.0
        );
    }

    #[test]
    fn visible_horizontal_scroll_offset_is_zero_without_overflow() {
        assert_eq!(
            clamped_visible_horizontal_scroll_offset(670.0, 670.0, 100.0),
            0.0
        );
        assert_eq!(
            clamped_visible_horizontal_scroll_offset(800.0, 670.0, 100.0),
            0.0
        );
    }

    #[test]
    fn horizontal_scrollbar_thumb_is_proportional_and_clamped() {
        let metrics = HorizontalScrollbarMetrics::new(200.0, 400.0, 500.0).unwrap();

        assert_approx_eq(metrics.scroll_left, 200.0);
        assert_approx_eq(metrics.scroll_max, 200.0);
        assert_approx_eq(
            metrics.thumb_right(),
            metrics.track_left + metrics.track_width,
        );
    }

    #[test]
    fn horizontal_scrollbar_drag_positions_map_to_scroll_offsets() {
        let metrics = HorizontalScrollbarMetrics::new(200.0, 1_000.0, 0.0).unwrap();
        let bottom_thumb_left = metrics.track_left + metrics.track_width - metrics.thumb_width;
        let middle_thumb_left = metrics.track_left + (bottom_thumb_left - metrics.track_left) / 2.0;

        assert_approx_eq(metrics.scroll_left_for_thumb_left(metrics.track_left), 0.0);
        assert_approx_eq(
            metrics.scroll_left_for_thumb_left(middle_thumb_left),
            metrics.scroll_max / 2.0,
        );
        assert_approx_eq(
            metrics.scroll_left_for_thumb_left(bottom_thumb_left),
            metrics.scroll_max,
        );
    }

    #[test]
    fn horizontal_scrollbar_line_and_page_deltas_clamp_at_bounds() {
        let top = HorizontalScrollbarMetrics::new(200.0, 1_000.0, 0.0).unwrap();
        assert_approx_eq(top.scroll_by(-HORIZONTAL_SCROLLBAR_LINE_DELTA), 0.0);
        assert_approx_eq(top.scroll_by(HORIZONTAL_SCROLLBAR_LINE_DELTA), 40.0);
        assert_approx_eq(top.scroll_by(top.viewport_width), 200.0);

        let bottom = HorizontalScrollbarMetrics::new(200.0, 1_000.0, 800.0).unwrap();
        assert_approx_eq(
            bottom.scroll_by(HORIZONTAL_SCROLLBAR_LINE_DELTA),
            bottom.scroll_max,
        );
        assert_approx_eq(bottom.scroll_by(-bottom.viewport_width), 600.0);
    }
}
