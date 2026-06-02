use std::{
    borrow::Cow,
    env,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

use gpui::{
    App, Application, Bounds, Context, KeyBinding, SharedString, TitlebarOptions, Window,
    WindowBounds, WindowOptions, prelude::*, px, size,
};
use serde::{Deserialize, Serialize};

use crate::explorer::{
    CancelDrag, CopySelected, CutSelected, EnterSelected, ExplorerView, ExtendDown, ExtendEnd,
    ExtendHome, ExtendUp, GoBack, GoForward, GoUp, MoveDown, MoveEnd, MoveHome, MoveUp,
    OpenSelected, PasteClipboard, PermanentlyDeleteSelected, Refresh, SelectAll, TrashSelected,
    default_start_path,
};

const APP_ID: &str = "com.hmerritt.explorer";
const APP_TITLE: &str = "Explorer";
const LINUX_CONFIG_DIR_NAME: &str = "explorer";
const WINDOW_STATE_FILE_NAME: &str = "window-state.json";
const DEFAULT_WINDOW_WIDTH: f32 = 1024.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 820.0;
const MIN_WINDOW_WIDTH: f32 = 400.0;
const MIN_WINDOW_HEIGHT: f32 = 120.0;
const SEGOE_FLUENT_ICONS: &[u8] = include_bytes!("../assets/Segoe Fluent Icons.ttf");
const SEGOE_MDL2_ASSETS: &[u8] = include_bytes!("../assets/Segoe MDL2 Assets.ttf");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConfigPlatform {
    MacOS,
    Linux,
    Windows,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum StoredWindowMode {
    Windowed,
    Maximized,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
struct StoredWindowState {
    width: f32,
    height: f32,
    state: StoredWindowMode,
}

impl StoredWindowState {
    fn new(width: f32, height: f32, state: StoredWindowMode) -> Self {
        Self {
            width,
            height,
            state,
        }
    }

    fn is_valid(&self) -> bool {
        self.width.is_finite()
            && self.height.is_finite()
            && self.width >= MIN_WINDOW_WIDTH
            && self.height >= MIN_WINDOW_HEIGHT
    }

    fn from_window_bounds(window_bounds: WindowBounds) -> Option<Self> {
        let (bounds, state) = match window_bounds {
            WindowBounds::Windowed(bounds) => (bounds, StoredWindowMode::Windowed),
            WindowBounds::Maximized(bounds) => (bounds, StoredWindowMode::Maximized),
            WindowBounds::Fullscreen(_) => return None,
        };

        let state = Self::new(
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            state,
        );
        state.is_valid().then_some(state)
    }

    fn to_window_bounds(self, cx: &App) -> Option<WindowBounds> {
        if !self.is_valid() {
            return None;
        }

        let bounds = Bounds::centered(None, size(px(self.width), px(self.height)), cx);
        Some(match self.state {
            StoredWindowMode::Windowed => WindowBounds::Windowed(bounds),
            StoredWindowMode::Maximized => WindowBounds::Maximized(bounds),
        })
    }
}

struct Explorer {
    explorer: gpui::Entity<ExplorerView>,
}

impl Render for Explorer {
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

fn current_config_platform() -> ConfigPlatform {
    if cfg!(target_os = "macos") {
        ConfigPlatform::MacOS
    } else if cfg!(target_os = "windows") {
        ConfigPlatform::Windows
    } else {
        ConfigPlatform::Linux
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    non_empty_path(env::var_os(name)?)
}

fn non_empty_path(value: OsString) -> Option<PathBuf> {
    (!value.as_os_str().is_empty()).then(|| PathBuf::from(value))
}

fn window_state_path() -> Option<PathBuf> {
    window_state_path_for(current_config_platform(), env_path)
}

fn window_state_path_for(
    platform: ConfigPlatform,
    mut env_path: impl FnMut(&str) -> Option<PathBuf>,
) -> Option<PathBuf> {
    match platform {
        ConfigPlatform::MacOS => env_path("HOME").map(|home| {
            home.join("Library")
                .join("Application Support")
                .join(APP_ID)
        }),
        ConfigPlatform::Linux => env_path("XDG_CONFIG_HOME")
            .map(|config_home| config_home.join(LINUX_CONFIG_DIR_NAME))
            .or_else(|| {
                env_path("HOME").map(|home| home.join(".config").join(LINUX_CONFIG_DIR_NAME))
            }),
        ConfigPlatform::Windows => env_path("APPDATA")
            .map(|appdata| appdata.join(APP_ID))
            .or_else(|| {
                env_path("USERPROFILE")
                    .map(|profile| profile.join("AppData").join("Roaming").join(APP_ID))
            }),
    }
    .map(|dir| dir.join(WINDOW_STATE_FILE_NAME))
}

fn default_window_bounds(cx: &App) -> WindowBounds {
    WindowBounds::Windowed(Bounds::centered(
        None,
        size(px(DEFAULT_WINDOW_WIDTH), px(DEFAULT_WINDOW_HEIGHT)),
        cx,
    ))
}

fn startup_window_bounds(cx: &App) -> WindowBounds {
    load_window_state()
        .and_then(|state| state.to_window_bounds(cx))
        .unwrap_or_else(|| default_window_bounds(cx))
}

fn load_window_state() -> Option<StoredWindowState> {
    load_window_state_from_path(&window_state_path()?)
}

fn load_window_state_from_path(path: &Path) -> Option<StoredWindowState> {
    let state = serde_json::from_str::<StoredWindowState>(&fs::read_to_string(path).ok()?).ok()?;
    state.is_valid().then_some(state)
}

fn save_window_bounds(window_bounds: WindowBounds) {
    let Some(state) = StoredWindowState::from_window_bounds(window_bounds) else {
        return;
    };
    let Some(path) = window_state_path() else {
        return;
    };

    let _ = save_window_state_to_path(&path, &state);
}

fn save_window_state_to_path(path: &Path, state: &StoredWindowState) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(state).map_err(io::Error::other)?;
    fs::write(path, json)
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
            KeyBinding::new("left", GoUp, None),
            KeyBinding::new("alt-left", GoBack, None),
            KeyBinding::new("right", OpenSelected, None),
            KeyBinding::new("alt-right", GoForward, None),
            KeyBinding::new("enter", EnterSelected, None),
            KeyBinding::new("f5", Refresh, None),
            KeyBinding::new("backspace", GoUp, None),
            KeyBinding::new("alt-up", GoUp, None),
            KeyBinding::new("escape", CancelDrag, None),
            KeyBinding::new("ctrl-a", SelectAll, None),
            KeyBinding::new("ctrl-c", CopySelected, None),
            KeyBinding::new("ctrl-x", CutSelected, None),
            KeyBinding::new("ctrl-v", PasteClipboard, None),
            KeyBinding::new("delete", TrashSelected, None),
            KeyBinding::new("shift-delete", PermanentlyDeleteSelected, None),
        ]);

        let window_bounds = startup_window_bounds(cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(window_bounds),
                window_min_size: Some(size(px(MIN_WINDOW_WIDTH), px(MIN_WINDOW_HEIGHT))),
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

                cx.new(|cx| {
                    cx.observe_window_bounds(window, |_, window, _| {
                        save_window_bounds(window.window_bounds());
                    })
                    .detach();

                    Explorer { explorer }
                })
            },
        )
        .expect("failed to open Explorer window");

        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn embedded_icon_fonts_are_present() {
        assert!(!SEGOE_FLUENT_ICONS.is_empty());
        assert!(!SEGOE_MDL2_ASSETS.is_empty());
        assert!(SEGOE_FLUENT_ICONS.len() > SEGOE_MDL2_ASSETS.len());
    }

    #[test]
    fn window_state_serializes_with_lowercase_state() {
        let state = StoredWindowState::new(800.0, 600.0, StoredWindowMode::Maximized);
        let json = serde_json::to_string(&state).expect("serialize state");

        assert!(json.contains("\"state\":\"maximized\""));
        assert_eq!(
            serde_json::from_str::<StoredWindowState>(&json).expect("deserialize state"),
            state
        );
    }

    #[test]
    fn window_state_rejects_invalid_dimensions() {
        assert!(
            !StoredWindowState::new(MIN_WINDOW_WIDTH - 1.0, 600.0, StoredWindowMode::Windowed)
                .is_valid()
        );
        assert!(
            !StoredWindowState::new(800.0, MIN_WINDOW_HEIGHT - 1.0, StoredWindowMode::Windowed)
                .is_valid()
        );
        assert!(!StoredWindowState::new(f32::NAN, 600.0, StoredWindowMode::Windowed).is_valid());
        assert!(
            StoredWindowState::new(
                MIN_WINDOW_WIDTH,
                MIN_WINDOW_HEIGHT,
                StoredWindowMode::Windowed
            )
            .is_valid()
        );
    }

    #[test]
    fn window_bounds_state_preserves_windowed_and_maximized_but_skips_fullscreen() {
        let bounds = Bounds::new(gpui::point(px(10.0), px(20.0)), size(px(900.0), px(700.0)));

        assert_eq!(
            StoredWindowState::from_window_bounds(WindowBounds::Windowed(bounds)),
            Some(StoredWindowState::new(
                900.0,
                700.0,
                StoredWindowMode::Windowed
            ))
        );
        assert_eq!(
            StoredWindowState::from_window_bounds(WindowBounds::Maximized(bounds)),
            Some(StoredWindowState::new(
                900.0,
                700.0,
                StoredWindowMode::Maximized
            ))
        );
        assert_eq!(
            StoredWindowState::from_window_bounds(WindowBounds::Fullscreen(bounds)),
            None
        );
    }

    #[test]
    fn window_state_paths_follow_platform_conventions() {
        assert_eq!(
            test_window_state_path(ConfigPlatform::MacOS, &[("HOME", "home")]),
            Some(
                PathBuf::from("home")
                    .join("Library")
                    .join("Application Support")
                    .join(APP_ID)
                    .join(WINDOW_STATE_FILE_NAME)
            )
        );
        assert_eq!(
            test_window_state_path(
                ConfigPlatform::Linux,
                &[("XDG_CONFIG_HOME", "xdg"), ("HOME", "home")]
            ),
            Some(
                PathBuf::from("xdg")
                    .join(LINUX_CONFIG_DIR_NAME)
                    .join(WINDOW_STATE_FILE_NAME)
            )
        );
        assert_eq!(
            test_window_state_path(ConfigPlatform::Linux, &[("HOME", "home")]),
            Some(
                PathBuf::from("home")
                    .join(".config")
                    .join(LINUX_CONFIG_DIR_NAME)
                    .join(WINDOW_STATE_FILE_NAME)
            )
        );
        assert_eq!(
            test_window_state_path(
                ConfigPlatform::Windows,
                &[("APPDATA", "appdata"), ("USERPROFILE", "profile")]
            ),
            Some(
                PathBuf::from("appdata")
                    .join(APP_ID)
                    .join(WINDOW_STATE_FILE_NAME)
            )
        );
        assert_eq!(
            test_window_state_path(ConfigPlatform::Windows, &[("USERPROFILE", "profile")]),
            Some(
                PathBuf::from("profile")
                    .join("AppData")
                    .join("Roaming")
                    .join(APP_ID)
                    .join(WINDOW_STATE_FILE_NAME)
            )
        );
        assert_eq!(test_window_state_path(ConfigPlatform::Linux, &[]), None);
    }

    #[test]
    fn window_state_loader_handles_missing_malformed_and_invalid_files() {
        let dir = unique_temp_dir("loader");
        let missing = dir.join("missing.json");
        assert_eq!(load_window_state_from_path(&missing), None);

        let malformed = dir.join("malformed.json");
        fs::create_dir_all(&dir).expect("create temp dir");
        fs::write(&malformed, "{").expect("write malformed state");
        assert_eq!(load_window_state_from_path(&malformed), None);

        let invalid = dir.join("invalid.json");
        fs::write(
            &invalid,
            serde_json::to_string(&StoredWindowState::new(
                MIN_WINDOW_WIDTH - 1.0,
                600.0,
                StoredWindowMode::Windowed,
            ))
            .expect("serialize invalid state"),
        )
        .expect("write invalid state");
        assert_eq!(load_window_state_from_path(&invalid), None);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn window_state_writer_creates_parent_directory_and_round_trips() {
        let path = unique_temp_dir("writer")
            .join("nested")
            .join(WINDOW_STATE_FILE_NAME);
        let state = StoredWindowState::new(960.0, 540.0, StoredWindowMode::Windowed);

        save_window_state_to_path(&path, &state).expect("save state");
        assert_eq!(load_window_state_from_path(&path), Some(state));

        let root = path
            .parent()
            .and_then(Path::parent)
            .expect("state path has test root");
        let _ = fs::remove_dir_all(root);
    }

    fn test_window_state_path(platform: ConfigPlatform, vars: &[(&str, &str)]) -> Option<PathBuf> {
        window_state_path_for(platform, |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| PathBuf::from(value))
        })
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time is after unix epoch")
            .as_nanos();
        env::temp_dir().join(format!(
            "explorer-window-state-{name}-{}-{nanos}",
            std::process::id()
        ))
    }
}
