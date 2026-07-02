use std::{fs, io, path::Path};

use gpui::{Bounds, Pixels, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WindowStateOptions {
    pub(crate) min_width: f32,
    pub(crate) min_height: f32,
    pub(crate) include_fullscreen: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum StoredWindowMode {
    Windowed,
    Maximized,
    Fullscreen,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct StoredWindowState {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    state: StoredWindowMode,
}

impl StoredWindowState {
    pub(crate) fn new(x: f32, y: f32, width: f32, height: f32, state: StoredWindowMode) -> Self {
        Self {
            x,
            y,
            width,
            height,
            state,
        }
    }

    pub(crate) fn is_valid(&self, options: WindowStateOptions) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width >= options.min_width
            && self.height >= options.min_height
            && (options.include_fullscreen || self.state != StoredWindowMode::Fullscreen)
    }

    pub(crate) fn from_window_bounds(
        window_bounds: WindowBounds,
        options: WindowStateOptions,
    ) -> Option<Self> {
        let (bounds, state) = match window_bounds {
            WindowBounds::Windowed(bounds) => (bounds, StoredWindowMode::Windowed),
            WindowBounds::Maximized(bounds) => (bounds, StoredWindowMode::Maximized),
            WindowBounds::Fullscreen(bounds) if options.include_fullscreen => {
                (bounds, StoredWindowMode::Fullscreen)
            }
            WindowBounds::Fullscreen(_) => return None,
        };

        let state = Self::new(
            f32::from(bounds.origin.x),
            f32::from(bounds.origin.y),
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            state,
        );
        state.is_valid(options).then_some(state)
    }

    pub(crate) fn to_window_bounds(
        self,
        display_bounds: &[Bounds<Pixels>],
        options: WindowStateOptions,
    ) -> Option<WindowBounds> {
        if !self.is_valid(options) {
            return None;
        }

        let bounds = Bounds::new(
            point(px(self.x), px(self.y)),
            size(px(self.width), px(self.height)),
        );
        if !bounds_fit_current_display(bounds, display_bounds) {
            return None;
        }

        Some(match self.state {
            StoredWindowMode::Windowed => WindowBounds::Windowed(bounds),
            StoredWindowMode::Maximized => WindowBounds::Maximized(bounds),
            StoredWindowMode::Fullscreen => WindowBounds::Fullscreen(bounds),
        })
    }
}

pub(crate) fn bounds_fit_current_display(
    window_bounds: Bounds<Pixels>,
    display_bounds: &[Bounds<Pixels>],
) -> bool {
    display_bounds
        .iter()
        .any(|display_bounds| window_bounds.is_contained_within(display_bounds))
}

pub(crate) fn load_window_state_from_path(
    path: &Path,
    options: WindowStateOptions,
) -> Option<StoredWindowState> {
    let state = serde_json::from_str::<StoredWindowState>(&fs::read_to_string(path).ok()?).ok()?;
    state.is_valid(options).then_some(state)
}

pub(crate) fn save_window_state_to_path(path: &Path, state: &StoredWindowState) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(state).map_err(io::Error::other)?;
    fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    const TEST_OPTIONS: WindowStateOptions = WindowStateOptions {
        min_width: 400.0,
        min_height: 120.0,
        include_fullscreen: true,
    };
    const NO_FULLSCREEN_OPTIONS: WindowStateOptions = WindowStateOptions {
        include_fullscreen: false,
        ..TEST_OPTIONS
    };

    #[test]
    fn window_state_serializes_with_lowercase_state() {
        let state = StoredWindowState::new(10.0, 20.0, 800.0, 600.0, StoredWindowMode::Fullscreen);
        let json = serde_json::to_string(&state).expect("serialize state");

        assert!(json.contains("\"x\":10.0"));
        assert!(json.contains("\"y\":20.0"));
        assert!(json.contains("\"state\":\"fullscreen\""));
        assert_eq!(
            serde_json::from_str::<StoredWindowState>(&json).expect("deserialize state"),
            state
        );
    }

    #[test]
    fn window_state_rejects_invalid_dimensions() {
        assert!(
            !StoredWindowState::new(
                0.0,
                0.0,
                TEST_OPTIONS.min_width - 1.0,
                600.0,
                StoredWindowMode::Windowed,
            )
            .is_valid(TEST_OPTIONS)
        );
        assert!(
            !StoredWindowState::new(
                0.0,
                0.0,
                800.0,
                TEST_OPTIONS.min_height - 1.0,
                StoredWindowMode::Windowed,
            )
            .is_valid(TEST_OPTIONS)
        );
        assert!(
            !StoredWindowState::new(f32::NAN, 0.0, 800.0, 600.0, StoredWindowMode::Windowed)
                .is_valid(TEST_OPTIONS)
        );
        assert!(
            !StoredWindowState::new(0.0, f32::NAN, 800.0, 600.0, StoredWindowMode::Windowed)
                .is_valid(TEST_OPTIONS)
        );
        assert!(
            !StoredWindowState::new(0.0, 0.0, f32::NAN, 600.0, StoredWindowMode::Windowed)
                .is_valid(TEST_OPTIONS)
        );
        assert!(
            StoredWindowState::new(
                0.0,
                0.0,
                TEST_OPTIONS.min_width,
                TEST_OPTIONS.min_height,
                StoredWindowMode::Windowed,
            )
            .is_valid(TEST_OPTIONS)
        );
    }

    #[test]
    fn window_state_rejects_fullscreen_when_disabled() {
        assert!(
            !StoredWindowState::new(0.0, 0.0, 800.0, 600.0, StoredWindowMode::Fullscreen)
                .is_valid(NO_FULLSCREEN_OPTIONS)
        );
    }

    #[test]
    fn window_bounds_state_preserves_allowed_modes() {
        let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(900.0), px(700.0)));

        assert_eq!(
            StoredWindowState::from_window_bounds(WindowBounds::Windowed(bounds), TEST_OPTIONS),
            Some(StoredWindowState::new(
                10.0,
                20.0,
                900.0,
                700.0,
                StoredWindowMode::Windowed,
            ))
        );
        assert_eq!(
            StoredWindowState::from_window_bounds(WindowBounds::Maximized(bounds), TEST_OPTIONS),
            Some(StoredWindowState::new(
                10.0,
                20.0,
                900.0,
                700.0,
                StoredWindowMode::Maximized,
            ))
        );
        assert_eq!(
            StoredWindowState::from_window_bounds(WindowBounds::Fullscreen(bounds), TEST_OPTIONS),
            Some(StoredWindowState::new(
                10.0,
                20.0,
                900.0,
                700.0,
                StoredWindowMode::Fullscreen,
            ))
        );
        assert_eq!(
            StoredWindowState::from_window_bounds(
                WindowBounds::Fullscreen(bounds),
                NO_FULLSCREEN_OPTIONS,
            ),
            None
        );
    }

    #[test]
    fn stored_window_state_restores_modes_when_safe() {
        let display = Bounds::new(point(px(0.0), px(0.0)), size(px(1920.0), px(1080.0)));
        let expected = Bounds::new(point(px(10.0), px(20.0)), size(px(900.0), px(700.0)));

        assert_eq!(
            StoredWindowState::new(10.0, 20.0, 900.0, 700.0, StoredWindowMode::Windowed)
                .to_window_bounds(&[display], TEST_OPTIONS),
            Some(WindowBounds::Windowed(expected))
        );
        assert_eq!(
            StoredWindowState::new(10.0, 20.0, 900.0, 700.0, StoredWindowMode::Maximized)
                .to_window_bounds(&[display], TEST_OPTIONS),
            Some(WindowBounds::Maximized(expected))
        );
        assert_eq!(
            StoredWindowState::new(10.0, 20.0, 900.0, 700.0, StoredWindowMode::Fullscreen)
                .to_window_bounds(&[display], TEST_OPTIONS),
            Some(WindowBounds::Fullscreen(expected))
        );
    }

    #[test]
    fn stored_window_state_rejects_bounds_outside_current_displays() {
        let display = Bounds::new(point(px(0.0), px(0.0)), size(px(1920.0), px(1080.0)));

        assert_eq!(
            StoredWindowState::new(1700.0, 20.0, 900.0, 700.0, StoredWindowMode::Windowed)
                .to_window_bounds(&[display], TEST_OPTIONS),
            None
        );
        assert_eq!(
            StoredWindowState::new(10.0, 20.0, 900.0, 700.0, StoredWindowMode::Windowed)
                .to_window_bounds(&[], TEST_OPTIONS),
            None
        );
    }

    #[test]
    fn window_state_loader_handles_missing_malformed_and_invalid_files() {
        let dir = unique_temp_dir("loader");
        let missing = dir.join("missing.json");
        assert_eq!(load_window_state_from_path(&missing, TEST_OPTIONS), None);

        let malformed = dir.join("malformed.json");
        fs::create_dir_all(&dir).expect("create temp dir");
        fs::write(&malformed, "{").expect("write malformed state");
        assert_eq!(load_window_state_from_path(&malformed, TEST_OPTIONS), None);

        let invalid = dir.join("invalid.json");
        fs::write(
            &invalid,
            serde_json::to_string(&StoredWindowState::new(
                0.0,
                0.0,
                TEST_OPTIONS.min_width - 1.0,
                600.0,
                StoredWindowMode::Windowed,
            ))
            .expect("serialize invalid state"),
        )
        .expect("write invalid state");
        assert_eq!(load_window_state_from_path(&invalid, TEST_OPTIONS), None);

        let legacy = dir.join("legacy.json");
        fs::write(
            &legacy,
            r#"{"width":900.0,"height":700.0,"state":"windowed"}"#,
        )
        .expect("write legacy state");
        assert_eq!(load_window_state_from_path(&legacy, TEST_OPTIONS), None);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn window_state_writer_creates_parent_directory_and_round_trips() {
        let path = unique_temp_dir("writer")
            .join("nested")
            .join("window-state.json");
        let state = StoredWindowState::new(12.0, 34.0, 960.0, 540.0, StoredWindowMode::Windowed);

        save_window_state_to_path(&path, &state).expect("save state");
        assert_eq!(
            load_window_state_from_path(&path, TEST_OPTIONS),
            Some(state)
        );

        let root = path
            .parent()
            .and_then(Path::parent)
            .expect("state path has test root");
        let _ = fs::remove_dir_all(root);
    }

    fn unique_temp_dir(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time is after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("explorer-window-state-{name}-{nanos}"))
    }
}
