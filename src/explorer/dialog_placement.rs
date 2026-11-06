use gpui::{App, Bounds, DisplayId, Pixels, Size, Window, WindowBounds, WindowOptions};

/// Placement of the Explorer window, independent of whichever window is active.
/// Cached by its views so asynchronous prompts do not need to re-enter the owner window.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct DialogPlacement {
    bounds: Option<Bounds<Pixels>>,
    display_id: Option<DisplayId>,
}

impl DialogPlacement {
    pub(super) fn from_window(window: &Window, cx: &App) -> Self {
        Self {
            bounds: Some(window.bounds()),
            display_id: window.display(cx).map(|display| display.id()),
        }
    }

    pub(super) fn apply(self, options: &mut WindowOptions) {
        // Unattached views (used by logic tests) have no owner yet.
        if let (Some(parent), Some(WindowBounds::Windowed(bounds))) =
            (self.bounds, options.window_bounds)
        {
            options.window_bounds = Some(centered_dialog_bounds(parent, bounds.size));
            options.display_id = self.display_id;
        }
    }
}

pub(super) fn centered_dialog_bounds(
    parent: Bounds<Pixels>,
    dialog_size: Size<Pixels>,
) -> WindowBounds {
    WindowBounds::Windowed(Bounds::centered_at(parent.center(), dialog_size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px, size};

    #[test]
    fn dialogs_center_on_owner_in_global_coordinates() {
        for (x, y, width, height) in [
            (100., 80., 1000., 700.),
            (-1800., -400., 1000., 700.),
            (600., 300., 200., 100.),
        ] {
            let parent = Bounds::new(point(px(x), px(y)), size(px(width), px(height)));
            let dialog_size = size(px(460.), px(260.));
            assert_eq!(
                centered_dialog_bounds(parent, dialog_size),
                WindowBounds::Windowed(Bounds::new(
                    point(px(x + (width - 460.) / 2.), px(y + (height - 260.) / 2.)),
                    dialog_size,
                )),
            );
        }
    }
}
