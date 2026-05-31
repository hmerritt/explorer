use std::borrow::Cow;

use gpui::{
    App, Application, Bounds, Context, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions, prelude::*, px, size,
};

use crate::explorer::{ExplorerView, default_start_path};

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
            |_, cx| {
                cx.new(|cx| UniversalExplorer {
                    explorer: cx.new(|_| ExplorerView::new(default_start_path())),
                })
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
