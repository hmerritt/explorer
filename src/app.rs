use std::borrow::Cow;

use gpui::{
    App, Application, Bounds, Context, KeyBinding, SharedString, TitlebarOptions, Window,
    WindowBounds, WindowOptions, prelude::*, px, size,
};

use crate::explorer::{
    EnterSelected, ExplorerView, ExtendDown, ExtendEnd, ExtendHome, ExtendUp, GoBack, GoForward,
    GoUp, MoveDown, MoveEnd, MoveHome, MoveUp, OpenSelected, Refresh, SelectAll,
    default_start_path,
};

const APP_ID: &str = "com.hmerritt.universal-explorer";
const APP_TITLE: &str = "Universal Explorer";
const SEGOE_FLUENT_ICONS: &[u8] = include_bytes!("../assets/Segoe Fluent Icons.ttf");
const SEGOE_MDL2_ASSETS: &[u8] = include_bytes!("../assets/Segoe MDL2 Assets.ttf");

struct UniversalExplorer {
    explorer: gpui::Entity<ExplorerView>,
}

impl Render for UniversalExplorer {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.explorer.clone()
    }
}

fn register_embedded_fonts(cx: &mut App) {
    cx.text_system()
        .add_fonts(vec![
            Cow::Borrowed(SEGOE_FLUENT_ICONS),
            Cow::Borrowed(SEGOE_MDL2_ASSETS),
        ])
        .expect("failed to register embedded icon fonts");
}

pub fn run() {
    Application::new().run(|cx: &mut App| {
        register_embedded_fonts(cx);
        cx.bind_keys([
            KeyBinding::new("up", MoveUp, None),
            KeyBinding::new("down", MoveDown, None),
            KeyBinding::new("shift-up", ExtendUp, None),
            KeyBinding::new("shift-down", ExtendDown, None),
            KeyBinding::new("home", MoveHome, None),
            KeyBinding::new("end", MoveEnd, None),
            KeyBinding::new("shift-home", ExtendHome, None),
            KeyBinding::new("shift-end", ExtendEnd, None),
            KeyBinding::new("left", GoBack, None),
            KeyBinding::new("alt-left", GoBack, None),
            KeyBinding::new("right", OpenSelected, None),
            KeyBinding::new("alt-right", GoForward, None),
            KeyBinding::new("enter", EnterSelected, None),
            KeyBinding::new("f5", Refresh, None),
            KeyBinding::new("backspace", GoUp, None),
            KeyBinding::new("alt-up", GoUp, None),
            KeyBinding::new("ctrl-a", SelectAll, None),
        ]);

        let bounds = Bounds::centered(None, size(px(1064.0), px(506.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(400.0), px(120.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::from(APP_TITLE)),
                    ..Default::default()
                }),
                app_id: Some(APP_ID.to_owned()),
                ..Default::default()
            },
            |window, cx| {
                let explorer = cx.new(|cx| {
                    let focus_handle = cx.focus_handle();
                    focus_handle.focus(window);
                    ExplorerView::new_with_focus_handle(default_start_path(), focus_handle)
                });

                cx.new(|_| UniversalExplorer { explorer })
            },
        )
        .expect("failed to open Universal Explorer window");

        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icon_fonts_are_present() {
        assert!(!SEGOE_FLUENT_ICONS.is_empty());
        assert!(!SEGOE_MDL2_ASSETS.is_empty());
        assert!(SEGOE_FLUENT_ICONS.len() > SEGOE_MDL2_ASSETS.len());
    }
}
