use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyView, App, AppContext as _, Context, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div, px, rgb,
};

const TOOLTIP_FADE_MS: u64 = 80;
const TOOLTIP_MAX_WIDTH: f32 = 260.0;

pub(super) struct ExplorerTooltip {
    label: SharedString,
}

impl ExplorerTooltip {
    pub(super) fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

impl Render for ExplorerTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        explorer_tooltip_element(self.label.clone())
    }
}

pub(super) fn explorer_tooltip_element(label: impl Into<SharedString>) -> impl IntoElement {
    div()
        .id("explorer-tooltip")
        .debug_selector(|| "explorer-tooltip".to_owned())
        .max_w(px(TOOLTIP_MAX_WIDTH))
        .px(px(7.0))
        .py(px(4.0))
        .rounded(px(2.0))
        .border_1()
        .border_color(rgb(0x767676))
        .bg(rgb(0xffffff))
        .shadow_md()
        .text_size(px(12.0))
        .line_height(px(16.0))
        .text_color(rgb(0x1f1f1f))
        .child(label.into())
        .with_animation(
            "explorer-tooltip-fade",
            Animation::new(Duration::from_millis(TOOLTIP_FADE_MS)),
            |this, delta| this.opacity(delta),
        )
}

pub(crate) fn explorer_tooltip(
    label: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView {
    let label = label.into();
    move |_, cx| {
        let label = label.clone();
        cx.new(|_| ExplorerTooltip::new(label)).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_stores_label() {
        let tooltip = ExplorerTooltip::new("Refresh");
        assert_eq!(tooltip.label, SharedString::from("Refresh"));
    }
}
