use std::time::Duration;

use gpui::{Animation, AnimationExt as _, AnyElement, div, prelude::*, px, relative, rgb};

use crate::explorer::constants::EXPLORER_COPY_GREEN;

const LINEAR_PROGRESS_HEIGHT: f32 = 4.0;
const LINEAR_PROGRESS_TRACK_GREEN: u32 = 0xe1f3e4;

// MUI's standard indeterminate animation cycle is 2.1 seconds.
const PROGRESS_ANIMATION_MS: u64 = 1_500;

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

#[derive(Clone, Copy)]
enum BarType {
    Primary,
    Secondary,
}

pub(crate) fn linear_indeterminate(id: &'static str, style: LinearProgressStyle) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .relative()
        .w_full()
        .h(px(style.height))
        .flex_shrink_0()
        .overflow_hidden()
        .bg(rgb(style.track_color))
        .child(animated_linear_progress_bar(
            (id, 0),
            style.color,
            BarType::Primary,
        ))
        .child(animated_linear_progress_bar(
            (id, 1),
            style.color,
            BarType::Secondary,
        ))
        .into_any_element()
}

fn animated_linear_progress_bar(
    id: (&'static str, usize),
    color: u32,
    bar_type: BarType,
) -> AnyElement {
    let (root_id, segment_ix) = id;
    div()
        .debug_selector(move || format!("{root_id}-segment-{segment_ix}"))
        .absolute()
        .top(px(0.0))
        .bottom(px(0.0))
        .bg(rgb(color))
        .with_animation(
            id,
            Animation::new(Duration::from_millis(PROGRESS_ANIMATION_MS)).repeat(),
            move |bar, delta| {
                let (left, right) = match bar_type {
                    BarType::Primary => {
                        // The primary bar animates for the first 60% of the cycle.
                        if delta <= 0.6 {
                            let t = delta / 0.6;
                            // MUI cubic-bezier(0.65, 0.815, 0.735, 0.395)
                            let progress = solve_cubic_bezier(0.65, 0.815, 0.735, 0.395, t);
                            let l = -0.35 + (1.0 - (-0.35)) * progress;
                            let r = 1.0 + (-0.9 - 1.0) * progress;
                            (l, r)
                        } else {
                            (1.0, -0.9)
                        }
                    }
                    BarType::Secondary => {
                        // The secondary bar has a CSS animation-delay of 1.15s.
                        // On a 2.1s cycle, this constitutes a phase shift of ~0.4524.
                        let phase_shifted_delta = (delta + 0.4524) % 1.0;
                        if phase_shifted_delta <= 0.6 {
                            let t = phase_shifted_delta / 0.6;
                            // MUI cubic-bezier(0.165, 0.84, 0.44, 1.0)
                            let progress = solve_cubic_bezier(0.165, 0.84, 0.44, 1.0, t);
                            let l = -2.00 + (1.07 - (-2.00)) * progress;
                            let r = 1.00 + (-0.08 - 1.00) * progress;
                            (l, r)
                        } else {
                            (1.07, -0.08)
                        }
                    }
                };

                // GPUI utilizes relative dimensions for absolute positioning.
                // The logical width is the remainder of the space after subtracting the left and right offsets.
                let width = 1.0 - left - right;
                bar.left(relative(left)).w(relative(width))
            },
        )
        .into_any_element()
}

/// Computes the corresponding Y value for a cubic-Bézier curve given an X progression.
fn solve_cubic_bezier(p1x: f32, p1y: f32, p2x: f32, p2y: f32, x: f32) -> f32 {
    let mut t = x;

    // Newton-Raphson approximation to converge on the 't' parameter.
    for _ in 0..8 {
        let x_est = evaluate_bezier(p1x, p2x, t);
        let dx = evaluate_bezier_derivative(p1x, p2x, t);
        if dx.abs() < 1e-6 {
            break;
        }
        t -= (x_est - x) / dx;
    }

    evaluate_bezier(p1y, p2y, t.clamp(0.0, 1.0))
}

fn evaluate_bezier(p1: f32, p2: f32, t: f32) -> f32 {
    let mt = 1.0 - t;
    3.0 * mt * mt * t * p1 + 3.0 * mt * t * t * p2 + t * t * t
}

fn evaluate_bezier_derivative(p1: f32, p2: f32, t: f32) -> f32 {
    let mt = 1.0 - t;
    3.0 * mt * mt * p1 + 6.0 * mt * t * (p2 - p1) + 3.0 * t * t * (1.0 - p2)
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

    #[test]
    fn cubic_bezier_solver_preserves_progress_endpoints() {
        assert_eq!(solve_cubic_bezier(0.65, 0.815, 0.735, 0.395, 0.0), 0.0);
        assert_eq!(solve_cubic_bezier(0.65, 0.815, 0.735, 0.395, 1.0), 1.0);
        assert_eq!(evaluate_bezier(0.0, 1.0, 0.0), 0.0);
        assert_eq!(evaluate_bezier(0.0, 1.0, 1.0), 1.0);
    }

    #[test]
    fn cubic_bezier_solver_returns_monotonic_y_values_for_progress_animation() {
        let samples =
            [0.1, 0.25, 0.5, 0.75, 0.9].map(|x| solve_cubic_bezier(0.165, 0.84, 0.44, 1.0, x));

        assert!(samples.windows(2).all(|window| window[0] < window[1]));
        assert!(samples.iter().all(|sample| (0.0..=1.0).contains(sample)));
    }

    #[test]
    fn cubic_bezier_derivative_matches_linear_control_points() {
        assert_eq!(evaluate_bezier_derivative(0.0, 1.0, 0.0), 0.0);
        assert_eq!(evaluate_bezier_derivative(0.0, 1.0, 0.5), 1.5);
        assert_eq!(evaluate_bezier_derivative(0.0, 1.0, 1.0), 0.0);
    }
}
