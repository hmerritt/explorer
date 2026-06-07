use std::time::Duration;

use gpui::{Animation, AnimationExt as _, AnyElement, div, prelude::*, px, relative, rgb};

use crate::explorer::constants::EXPLORER_COPY_GREEN;

const LINEAR_PROGRESS_HEIGHT: f32 = 4.0;
const LINEAR_PROGRESS_TRACK_GREEN: u32 = 0xe1f3e4;
const PRIMARY_BAR_WIDTH: f32 = 0.42;
const SECONDARY_BAR_WIDTH: f32 = 0.28;
const PRIMARY_ANIMATION_MS: u64 = 1_450;
const SECONDARY_ANIMATION_MS: u64 = 1_900;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LinearProgressStyle {
    pub(crate) color: u32,
    pub(crate) track_color: u32,
    pub(crate) height: f32,
}

impl LinearProgressStyle {
    pub(crate) fn explorer_copy_green() -> Self {
        Self {
            color: EXPLORER_COPY_GREEN,
            track_color: LINEAR_PROGRESS_TRACK_GREEN,
            height: LINEAR_PROGRESS_HEIGHT,
        }
    }
}

pub(crate) fn linear_indeterminate(id: &'static str, style: LinearProgressStyle) -> AnyElement {
    div()
        .id(id)
        .relative()
        .w_full()
        .h(px(style.height))
        .flex_shrink_0()
        .overflow_hidden()
        .bg(rgb(style.track_color))
        .child(animated_linear_progress_bar(
            (id, 0),
            style.color,
            PRIMARY_BAR_WIDTH,
            PRIMARY_ANIMATION_MS,
            -PRIMARY_BAR_WIDTH,
            1.0,
        ))
        .child(animated_linear_progress_bar(
            (id, 1),
            style.color,
            SECONDARY_BAR_WIDTH,
            SECONDARY_ANIMATION_MS,
            -SECONDARY_BAR_WIDTH * 2.0,
            1.0,
        ))
        .into_any_element()
}

fn animated_linear_progress_bar(
    id: (&'static str, usize),
    color: u32,
    width_fraction: f32,
    duration_ms: u64,
    start_fraction: f32,
    end_fraction: f32,
) -> AnyElement {
    div()
        .absolute()
        .top(px(0.0))
        .bottom(px(0.0))
        .w(relative(width_fraction))
        .bg(rgb(color))
        .with_animation(
            id,
            Animation::new(Duration::from_millis(duration_ms)).repeat(),
            move |bar, delta| {
                let left = start_fraction + (end_fraction - start_fraction) * delta;
                bar.left(relative(left))
            },
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_copy_green_style_uses_shared_green() {
        let style = LinearProgressStyle::explorer_copy_green();

        assert_eq!(style.color, EXPLORER_COPY_GREEN);
        assert_eq!(style.track_color, LINEAR_PROGRESS_TRACK_GREEN);
        assert!(style.height > 0.0);
    }
}
