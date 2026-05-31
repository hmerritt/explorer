use gpui::{
    App, Application, Bounds, Context, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions, prelude::*, px, size,
};

use crate::explorer::{ExplorerView, default_start_path};

const APP_ID: &str = "com.hmerritt.universal-explorer";
const APP_TITLE: &str = "Universal Explorer";

struct UniversalExplorer {
    explorer: gpui::Entity<ExplorerView>,
}

impl Render for UniversalExplorer {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.explorer.clone()
    }
}
pub fn run() {
    Application::new().run(|cx: &mut App| {
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
