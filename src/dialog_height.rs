use std::{cell::Cell, rc::Rc};

use gpui::{AnyElement, App, Div, IntoElement, Pixels, Size, Window, div, prelude::*, px, size};

#[derive(Clone)]
pub(super) struct DialogHeight {
    state: Rc<Cell<FitState>>,
    allow_manual_resize: bool,
    max_height: Option<Pixels>,
}

#[derive(Clone, Copy, Default)]
struct FitState {
    viewport: Option<(Size<Pixels>, f32)>,
    requested: Option<Size<Pixels>>,
    previous_request: Option<Size<Pixels>>,
    manually_resized: bool,
}

fn same_size(a: Size<Pixels>, b: Size<Pixels>) -> bool {
    (a.width - b.width).abs() <= px(1.0) && (a.height - b.height).abs() <= px(1.0)
}

impl FitState {
    fn observe(&mut self, viewport: Size<Pixels>, scale: f32, allow_manual: bool) {
        if let Some((previous, previous_scale)) = self.viewport {
            if previous_scale != scale {
                self.requested = None;
                self.previous_request = None;
            } else if !same_size(previous, viewport) {
                let matches_latest = self.requested.is_some_and(|size| same_size(size, viewport));
                let matches_previous = self
                    .previous_request
                    .is_some_and(|size| same_size(size, viewport));
                self.manually_resized |= allow_manual && !matches_latest && !matches_previous;
                if matches_latest || !matches_previous {
                    self.requested = None;
                }
                self.previous_request = None;
            }
        }
        self.viewport = Some((viewport, scale));
    }

    fn request(&mut self, viewport: Size<Pixels>, target: Size<Pixels>) -> bool {
        if same_size(viewport, target) || self.requested == Some(target) {
            return false;
        }

        self.previous_request = self.requested.replace(target);
        true
    }
}

impl DialogHeight {
    pub(super) fn new(
        window: &mut Window,
        cx: &mut App,
        allow_manual_resize: bool,
        max_height: Option<Pixels>,
    ) -> Self {
        let state = window.use_keyed_state("explorer-dialog-height", cx, |_, _| {
            Rc::new(Cell::new(FitState::default()))
        });
        Self {
            state: state.read(cx).clone(),
            allow_manual_resize,
            max_height,
        }
    }

    /// Measure the body at its actual width, inside a scroll viewport which can shrink
    /// independently of the actions and the dialog's surrounding padding.
    pub(super) fn layout(&self, body: impl IntoElement, actions: Option<AnyElement>) -> Div {
        let body_height = Rc::new(Cell::new(px(0.0)));
        let measure = body_height.clone();
        let fit = self.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("dialog-scroll-body")
                    .debug_selector(|| "dialog-scroll-body".into())
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .child(div().w_full().child(body).on_children_prepainted(
                        move |children, _, _| {
                            measure.set(children[0].size.height);
                        },
                    )),
            )
            .children(actions.map(|actions| div().flex_shrink_0().child(actions)))
            .on_children_prepainted(move |children, window, cx| {
                let chrome = window.viewport_size().height - children[0].size.height;
                fit.resize(body_height.get() + chrome, chrome, window, cx);
            })
    }

    pub(super) fn measure_body(&self, body: impl IntoElement, chrome: Pixels) -> Div {
        let fit = self.clone();
        div()
            .w_full()
            .flex_shrink_0()
            .child(body)
            .on_children_prepainted(move |children, window, cx| {
                fit.resize(children[0].size.height + chrome, chrome, window, cx);
            })
    }

    pub(super) fn resize(&self, height: Pixels, chrome: Pixels, window: &mut Window, cx: &mut App) {
        let viewport = window.viewport_size();
        let mut state = self.state.get();
        state.observe(viewport, window.scale_factor(), self.allow_manual_resize);
        if self.allow_manual_resize && (window.is_maximized() || window.is_fullscreen()) {
            state.manually_resized = true;
        }
        self.state.set(state);
        if state.manually_resized {
            return;
        }

        let mut limit = self.max_height.unwrap_or(Pixels::MAX);
        if let Some(display) = window.display(cx) {
            let bounds = display.bounds();
            // Allow for native decorations and desktop panels. Use the remaining space
            // below the window when its reported origin is on this display.
            let reserve = px(48.0);
            let top = window.bounds().top();
            let minimum = chrome + px(24.0);
            let remaining = bounds.bottom() - top - reserve;
            let display_limit = if top >= bounds.top() && remaining >= minimum {
                remaining
            } else {
                bounds.size.height - reserve
            };
            limit = limit.min(display_limit.max(minimum));
        }
        let target = size(viewport.width, height.ceil().min(limit.floor()));
        if !state.request(viewport, target) {
            return;
        }

        self.state.set(state);
        let state = self.state.clone();
        window.defer(cx, move |window, _| {
            let current = state.get();
            if !current.manually_resized && current.requested == Some(target) {
                window.resize(target);
            }
        });
    }
}

#[cfg(test)]
pub(super) fn settle_dialog_height(cx: &mut gpui::VisualTestContext) -> Size<Pixels> {
    cx.run_until_parked();
    let (client, viewport) = cx.update(|window, _| (window.bounds().size, window.viewport_size()));
    // GPUI's test platform updates the bounds for programmatic resizes but does not
    // send a resize notification to the layout engine.
    if client != viewport {
        cx.simulate_resize(client);
        cx.run_until_parked();
    }
    cx.update(|window, _| window.bounds().size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_programmatic_and_dpi_resizes_before_locking_manual_size() {
        let mut state = FitState::default();
        let initial = size(px(430.0), px(266.0));
        let fitted = size(initial.width, px(310.0));
        state.observe(initial, 1.0, true);
        assert!(state.request(initial, fitted));
        assert!(!state.request(initial, fitted));
        state.observe(size(fitted.width, fitted.height - px(0.5)), 1.0, true);
        assert!(!state.manually_resized);
        assert!(state.requested.is_none());

        state.observe(size(px(500.0), px(350.0)), 1.5, true);
        assert!(!state.manually_resized);
        state.observe(size(px(550.0), px(400.0)), 1.5, true);
        assert!(state.manually_resized);
        state.observe(initial, 1.0, true);
        assert!(state.manually_resized);
    }

    #[test]
    fn recognizes_resize_results_when_content_changes_before_the_first_result() {
        let mut state = FitState::default();
        let initial = size(px(430.0), px(520.0));
        let loading = size(initial.width, px(146.0));
        let loaded = size(initial.width, px(310.0));
        state.observe(initial, 1.0, true);
        assert!(state.request(initial, loading));
        assert!(state.request(initial, loaded));
        state.observe(loading, 1.0, true);
        assert!(!state.manually_resized);
        assert_eq!(state.requested, Some(loaded));
        state.observe(loaded, 1.0, true);
        assert!(!state.manually_resized);
        assert!(state.requested.is_none());
    }
}
