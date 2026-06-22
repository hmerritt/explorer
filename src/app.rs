#[cfg(any(target_os = "linux", test))]
use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::os::unix::net::UnixStream;
use std::{
    borrow::Cow,
    env, fs, io,
    path::{Path, PathBuf},
};

use gpui::{
    App, Application, Bounds, Context, DisplayId, KeyBinding, Pixels, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowDecorations, WindowOptions, point, prelude::*, px,
    size,
};
use serde::{Deserialize, Serialize};

use crate::explorer::{
    AddressAcceptSuggestion, AddressBackspace, AddressBackspaceWord, AddressCancel, AddressCommit,
    AddressCopy, AddressCut, AddressDelete, AddressEdit, AddressEnd, AddressHome, AddressLeft,
    AddressPaste, AddressRight, AddressSelectAll, AddressSelectEnd, AddressSelectHome,
    AddressSelectLeft, AddressSelectRight, AddressSelectWordLeft, AddressSelectWordRight,
    AddressSuggestionDown, AddressSuggestionUp, AddressWordLeft, AddressWordRight, CancelDrag,
    CloseTab, CopySelected, CreateNewFolder, CutSelected, DialogCancel, DialogConfirm,
    DialogFocusPrimary, DialogFocusSecondary, EnterSelected, EnterSelectedInNewTab, ExplorerTabs,
    ExtendDown, ExtendEnd, ExtendHome, ExtendUp, GoBack, GoForward, GoUp, MoveDown, MoveEnd,
    MoveHome, MoveUp, NewTab, NewWindow, OpenProperties, OpenSelected, OpenSelectedInNewTab,
    OpenSettings, PasteClipboard, PermanentlyDeleteSelected, RecursiveSearchEdit, Refresh,
    RenameBackspace, RenameBackspaceWord, RenameCancel, RenameCommit, RenameCopy, RenameCut,
    RenameDelete, RenameEnd, RenameHome, RenameLeft, RenameNoop, RenamePaste, RenameRight,
    RenameSelectAll, RenameSelectEnd, RenameSelectHome, RenameSelectLeft, RenameSelectRight,
    RenameSelectWordLeft, RenameSelectWordRight, RenameSelected, RenameWordLeft, RenameWordRight,
    SearchBackspace, SearchBackspaceWord, SearchCancel, SearchCommit, SearchCopy, SearchCut,
    SearchDelete, SearchEdit, SearchEnd, SearchHome, SearchLeft, SearchPaste, SearchRight,
    SearchSelectAll, SearchSelectEnd, SearchSelectHome, SearchSelectLeft, SearchSelectRight,
    SearchSelectWordLeft, SearchSelectWordRight, SearchWordLeft, SearchWordRight, SelectAll,
    SelectNextTab, SelectPreviousTab, SelectTabByIndex, TrashSelected, UndoFileOperation,
};
use crate::settings::{APP_ID, SettingsState, config_dir};

const APP_TITLE: &str = "Explorer";
const WINDOW_STATE_FILE_NAME: &str = "window-state.json";
const DEFAULT_WINDOW_WIDTH: f32 = 1024.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 820.0;
const MIN_WINDOW_WIDTH: f32 = 400.0;
const MIN_WINDOW_HEIGHT: f32 = 120.0;
const NEW_WINDOW_OFFSET: f32 = 50.0;
const SEGOE_FLUENT_ICONS: &[u8] = include_bytes!("../assets/fonts/Segoe Fluent Icons.ttf");
const SEGOE_MDL2_ASSETS: &[u8] = include_bytes!("../assets/fonts/Segoe MDL2 Assets.ttf");
#[cfg(any(target_os = "linux", test))]
const DEFAULT_WAYLAND_DISPLAY: &str = "wayland-0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum StoredWindowMode {
    Windowed,
    Maximized,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
struct StoredWindowState {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    state: StoredWindowMode,
}

impl StoredWindowState {
    fn new(x: f32, y: f32, width: f32, height: f32, state: StoredWindowMode) -> Self {
        Self {
            x,
            y,
            width,
            height,
            state,
        }
    }

    fn is_valid(&self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
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
            f32::from(bounds.origin.x),
            f32::from(bounds.origin.y),
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            state,
        );
        state.is_valid().then_some(state)
    }

    fn to_window_bounds(self, display_bounds: &[Bounds<Pixels>]) -> Option<WindowBounds> {
        if !self.is_valid() {
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
        })
    }
}

fn bounds_fit_current_display(
    window_bounds: Bounds<Pixels>,
    display_bounds: &[Bounds<Pixels>],
) -> bool {
    display_bounds
        .iter()
        .any(|display_bounds| window_bounds.is_contained_within(display_bounds))
}

struct Explorer {
    explorer: gpui::Entity<ExplorerTabs>,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Debug, Eq, PartialEq)]
enum LinuxDisplayBackend {
    Wayland { display: OsString },
    X11,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinuxDisplayBackendPreference {
    Auto,
    Wayland,
    X11,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Debug, Eq, PartialEq)]
enum LinuxDisplaySelection {
    Backend(LinuxDisplayBackend),
    FatalNoDisplay,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Debug, Default)]
struct LinuxDisplayEnv {
    wayland_display: Option<OsString>,
    xdg_runtime_dir: Option<OsString>,
    x11_display: Option<OsString>,
    backend_preference: Option<OsString>,
    zed_headless: Option<OsString>,
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

#[cfg(any(target_os = "linux", test))]
fn non_empty_os(value: Option<OsString>) -> Option<OsString> {
    value.filter(|value| !value.as_os_str().is_empty())
}

#[cfg(any(target_os = "linux", test))]
fn wayland_display_path(display: &OsString, xdg_runtime_dir: Option<&OsString>) -> Option<PathBuf> {
    let display_path = PathBuf::from(display);
    if display_path.is_absolute() {
        Some(display_path)
    } else {
        xdg_runtime_dir.map(|runtime_dir| PathBuf::from(runtime_dir).join(display))
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_display_backend_preference(value: Option<OsString>) -> LinuxDisplayBackendPreference {
    let Some(value) = value.and_then(|value| value.into_string().ok()) else {
        return LinuxDisplayBackendPreference::Auto;
    };

    match value.to_ascii_lowercase().as_str() {
        "wayland" => LinuxDisplayBackendPreference::Wayland,
        "x11" => LinuxDisplayBackendPreference::X11,
        _ => LinuxDisplayBackendPreference::Auto,
    }
}

#[cfg(any(target_os = "linux", test))]
fn connectable_wayland_display(
    wayland_display: Option<OsString>,
    xdg_runtime_dir: Option<&OsString>,
    mut can_connect_to_wayland: impl FnMut(&Path) -> bool,
) -> Option<OsString> {
    if let Some(display) = wayland_display {
        if wayland_display_path(&display, xdg_runtime_dir)
            .is_some_and(|path| can_connect_to_wayland(&path))
        {
            return Some(display);
        }
    } else if let Some(path) =
        xdg_runtime_dir.map(|runtime_dir| PathBuf::from(runtime_dir).join(DEFAULT_WAYLAND_DISPLAY))
        && can_connect_to_wayland(&path)
    {
        return Some(OsString::from(DEFAULT_WAYLAND_DISPLAY));
    }

    None
}

#[cfg(any(target_os = "linux", test))]
fn select_linux_display_backend(
    env: LinuxDisplayEnv,
    mut can_connect_to_wayland: impl FnMut(&Path) -> bool,
) -> LinuxDisplaySelection {
    let wayland_display = non_empty_os(env.wayland_display);
    let xdg_runtime_dir = non_empty_os(env.xdg_runtime_dir);
    let x11_display = non_empty_os(env.x11_display);
    let backend_preference = linux_display_backend_preference(non_empty_os(env.backend_preference));
    let _zed_headless = non_empty_os(env.zed_headless);

    if matches!(
        backend_preference,
        LinuxDisplayBackendPreference::Auto | LinuxDisplayBackendPreference::X11
    ) && x11_display.is_some()
    {
        return LinuxDisplaySelection::Backend(LinuxDisplayBackend::X11);
    }

    if matches!(
        backend_preference,
        LinuxDisplayBackendPreference::Auto | LinuxDisplayBackendPreference::Wayland
    ) && let Some(display) =
        connectable_wayland_display(wayland_display, xdg_runtime_dir.as_ref(), |path| {
            can_connect_to_wayland(path)
        })
    {
        return LinuxDisplaySelection::Backend(LinuxDisplayBackend::Wayland { display });
    }

    LinuxDisplaySelection::FatalNoDisplay
}

#[cfg(target_os = "linux")]
fn configure_linux_display_backend() {
    let selection = select_linux_display_backend(
        LinuxDisplayEnv {
            wayland_display: env::var_os("WAYLAND_DISPLAY"),
            xdg_runtime_dir: env::var_os("XDG_RUNTIME_DIR"),
            x11_display: env::var_os("DISPLAY"),
            backend_preference: env::var_os("EXPLORER_LINUX_BACKEND"),
            zed_headless: env::var_os("ZED_HEADLESS"),
        },
        |path| UnixStream::connect(path).is_ok(),
    );

    match selection {
        LinuxDisplaySelection::Backend(LinuxDisplayBackend::Wayland { display }) => {
            // SAFETY: Explorer is still single-threaded here, before GPUI starts any
            // executors or windows. This is the only startup code that mutates the
            // process environment for display backend selection.
            unsafe {
                env::remove_var("ZED_HEADLESS");
                env::set_var("WAYLAND_DISPLAY", display);
            }
        }
        LinuxDisplaySelection::Backend(LinuxDisplayBackend::X11) => {
            // SAFETY: Explorer is still single-threaded here, before GPUI starts any
            // executors or windows. This is the only startup code that mutates the
            // process environment for display backend selection.
            unsafe {
                env::remove_var("ZED_HEADLESS");
                env::remove_var("WAYLAND_DISPLAY");
            }
        }
        LinuxDisplaySelection::FatalNoDisplay => {
            eprintln!(
                "Explorer requires a graphical Linux session. Set WAYLAND_DISPLAY to a connectable Wayland socket or DISPLAY to an X11 display."
            );
            std::process::exit(1);
        }
    }
}

fn window_state_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(WINDOW_STATE_FILE_NAME))
}

fn default_window_bounds(cx: &App) -> WindowBounds {
    WindowBounds::Windowed(Bounds::centered(
        None,
        size(px(DEFAULT_WINDOW_WIDTH), px(DEFAULT_WINDOW_HEIGHT)),
        cx,
    ))
}

fn startup_window_bounds_from_state(
    state: Option<StoredWindowState>,
    display_bounds: &[Bounds<Pixels>],
    default_bounds: WindowBounds,
) -> WindowBounds {
    state
        .and_then(|state| state.to_window_bounds(display_bounds))
        .unwrap_or(default_bounds)
}

fn startup_window_bounds(cx: &App) -> WindowBounds {
    let display_bounds = cx
        .displays()
        .into_iter()
        .map(|display| display.bounds())
        .collect::<Vec<_>>();

    startup_window_bounds_from_state(
        load_window_state(),
        &display_bounds,
        default_window_bounds(cx),
    )
}

fn window_bounds_rect(window_bounds: WindowBounds) -> Bounds<Pixels> {
    match window_bounds {
        WindowBounds::Windowed(bounds)
        | WindowBounds::Maximized(bounds)
        | WindowBounds::Fullscreen(bounds) => bounds,
    }
}

fn display_index_for_window_bounds(
    window_bounds: Bounds<Pixels>,
    display_bounds: &[Bounds<Pixels>],
) -> Option<usize> {
    let center = window_bounds.center();
    display_bounds
        .iter()
        .position(|display_bounds| display_bounds.contains(&center))
        .or_else(|| {
            display_bounds
                .iter()
                .position(|display_bounds| window_bounds.intersects(display_bounds))
        })
        .or_else(|| (!display_bounds.is_empty()).then_some(0))
}

fn offset_bounds(bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::new(
        point(
            bounds.origin.x + px(NEW_WINDOW_OFFSET),
            bounds.origin.y + px(NEW_WINDOW_OFFSET),
        ),
        bounds.size,
    )
}

fn wrapped_bounds_for_display(
    source_bounds: Bounds<Pixels>,
    display_bounds: Bounds<Pixels>,
) -> Bounds<Pixels> {
    let origin = point(
        display_bounds.origin.x + px(NEW_WINDOW_OFFSET),
        display_bounds.origin.y + px(NEW_WINDOW_OFFSET),
    );
    let display_right = f32::from(display_bounds.origin.x) + f32::from(display_bounds.size.width);
    let display_bottom = f32::from(display_bounds.origin.y) + f32::from(display_bounds.size.height);
    let max_width_from_origin = display_right - f32::from(origin.x);
    let max_height_from_origin = display_bottom - f32::from(origin.y);
    let width = f32::from(source_bounds.size.width);
    let height = f32::from(source_bounds.size.height);

    Bounds::new(
        origin,
        size(
            px(width.min(max_width_from_origin.max(MIN_WINDOW_WIDTH))),
            px(height.min(max_height_from_origin.max(MIN_WINDOW_HEIGHT))),
        ),
    )
}

fn clamp_window_bounds_to_display(
    bounds: Bounds<Pixels>,
    display_bounds: Bounds<Pixels>,
) -> Bounds<Pixels> {
    let display_left = f32::from(display_bounds.origin.x);
    let display_top = f32::from(display_bounds.origin.y);
    let display_right = display_left + f32::from(display_bounds.size.width);
    let display_bottom = display_top + f32::from(display_bounds.size.height);
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let x = f32::from(bounds.origin.x);
    let y = f32::from(bounds.origin.y);
    let max_x = (display_right - width).max(display_left);
    let max_y = (display_bottom - height).max(display_top);

    Bounds::new(
        point(
            px(x.clamp(display_left, max_x)),
            px(y.clamp(display_top, max_y)),
        ),
        bounds.size,
    )
}

fn new_window_placement_from_source(
    source_window_bounds: WindowBounds,
    display_bounds: &[Bounds<Pixels>],
) -> (WindowBounds, Option<usize>) {
    let source_bounds = window_bounds_rect(source_window_bounds);
    let display_index = display_index_for_window_bounds(source_bounds, display_bounds);
    let bounds = if let Some(display_bounds) = display_index.map(|index| display_bounds[index]) {
        let offset = offset_bounds(source_bounds);
        if offset.is_contained_within(&display_bounds) {
            offset
        } else {
            clamp_window_bounds_to_display(
                wrapped_bounds_for_display(source_bounds, display_bounds),
                display_bounds,
            )
        }
    } else {
        offset_bounds(source_bounds)
    };

    (WindowBounds::Windowed(bounds), display_index)
}

fn load_window_state() -> Option<StoredWindowState> {
    load_window_state_from_path(&window_state_path()?)
}

fn load_window_state_from_path(path: &Path) -> Option<StoredWindowState> {
    let state = serde_json::from_str::<StoredWindowState>(&fs::read_to_string(path).ok()?).ok()?;
    state.is_valid().then_some(state)
}

#[cfg_attr(test, allow(dead_code))]
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

#[cfg(not(test))]
fn observe_explorer_window_bounds(window: &mut Window, cx: &mut Context<Explorer>) {
    cx.observe_window_bounds(window, |_, window, _| {
        save_window_bounds(window.window_bounds());
    })
    .detach();
}

#[cfg(test)]
fn observe_explorer_window_bounds(_: &mut Window, _: &mut Context<Explorer>) {}

pub(crate) fn open_explorer_window_at(
    initial_path: PathBuf,
    window_bounds: WindowBounds,
    display_id: Option<DisplayId>,
    cx: &mut App,
) {
    cx.open_window(
        WindowOptions {
            window_bounds: Some(window_bounds),
            display_id,
            window_min_size: Some(size(px(MIN_WINDOW_WIDTH), px(MIN_WINDOW_HEIGHT))),
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from(APP_TITLE)),
                appears_transparent: true,
                traffic_light_position: cfg!(target_os = "macos")
                    .then_some(point(px(12.0), px(11.0))),
                ..Default::default()
            }),
            window_decorations: Some(if cfg!(target_os = "linux") {
                WindowDecorations::Client
            } else {
                WindowDecorations::Server
            }),
            app_id: Some(APP_ID.to_owned()),
            ..Default::default()
        },
        move |window, cx| {
            let path = initial_path;
            let explorer = cx.new(|cx| {
                let focus_handle = cx.focus_handle();
                focus_handle.focus(window);
                ExplorerTabs::new(path, focus_handle, window, cx)
            });

            cx.new(|cx| {
                observe_explorer_window_bounds(window, cx);

                Explorer { explorer }
            })
        },
    )
    .expect("failed to open Explorer window");
}

fn open_explorer_window(cx: &mut App) {
    let initial_path = cx.global::<SettingsState>().startup_path();
    let window_bounds = startup_window_bounds(cx);
    open_explorer_window_at(initial_path, window_bounds, None, cx);
}

pub(crate) fn open_new_explorer_window(
    initial_path: PathBuf,
    source_window_bounds: WindowBounds,
    cx: &mut App,
) {
    let displays = cx.displays();
    let display_bounds = displays
        .iter()
        .map(|display| display.bounds())
        .collect::<Vec<_>>();
    let (window_bounds, display_index) =
        new_window_placement_from_source(source_window_bounds, &display_bounds);
    let display_id =
        display_index.and_then(|index| displays.get(index).map(|display| display.id()));

    open_explorer_window_at(initial_path, window_bounds, display_id, cx);
}

#[cfg(any(target_os = "macos", test))]
fn should_open_window_on_reopen(open_window_count: usize) -> bool {
    open_window_count == 0
}

pub fn run() {
    #[cfg(target_os = "linux")]
    configure_linux_display_backend();

    let app = Application::new();

    #[cfg(target_os = "macos")]
    app.on_reopen(|cx| {
        if should_open_window_on_reopen(cx.windows().len()) {
            open_explorer_window(cx);
        }
        cx.activate(true);
    });

    app.run(|cx: &mut App| {
        register_embedded_fonts(cx);
        crate::http_client::initialize(cx);
        crate::debug_options::initialize(cx, env::args_os());
        crate::settings::initialize(cx);
        crate::explorer::initialize_native_icon_cache(cx);
        crate::explorer::initialize_image_thumbnail_cache(cx);
        crate::explorer::initialize_folder_size_cache(cx);
        crate::explorer::initialize_file_checksum_cache(cx);
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
            KeyBinding::new("ctrl-right", OpenSelectedInNewTab, None),
            KeyBinding::new("alt-right", GoForward, None),
            KeyBinding::new("enter", EnterSelected, None),
            KeyBinding::new("ctrl-enter", EnterSelectedInNewTab, None),
            KeyBinding::new("alt-enter", OpenProperties, Some("Explorer")),
            KeyBinding::new("f5", Refresh, None),
            KeyBinding::new("backspace", GoUp, None),
            KeyBinding::new("alt-up", GoUp, None),
            KeyBinding::new("escape", CancelDrag, None),
            KeyBinding::new("escape", DialogCancel, Some("ExplorerDialog")),
            KeyBinding::new("enter", DialogConfirm, Some("ExplorerDialog")),
            KeyBinding::new("left", DialogFocusPrimary, Some("ExplorerDialog")),
            KeyBinding::new("up", DialogFocusPrimary, Some("ExplorerDialog")),
            KeyBinding::new("right", DialogFocusSecondary, Some("ExplorerDialog")),
            KeyBinding::new("down", DialogFocusSecondary, Some("ExplorerDialog")),
            KeyBinding::new("ctrl-a", SelectAll, None),
            KeyBinding::new("ctrl-c", CopySelected, None),
            KeyBinding::new("ctrl-x", CutSelected, None),
            KeyBinding::new("ctrl-v", PasteClipboard, None),
            KeyBinding::new("ctrl-z", UndoFileOperation, Some("Explorer")),
            KeyBinding::new("delete", TrashSelected, None),
            KeyBinding::new("shift-delete", PermanentlyDeleteSelected, None),
            KeyBinding::new("alt-d", AddressEdit, Some("Explorer")),
            KeyBinding::new("ctrl-l", AddressEdit, Some("Explorer")),
            KeyBinding::new("f4", AddressEdit, Some("Explorer")),
            KeyBinding::new("ctrl-f", SearchEdit, Some("Explorer")),
            KeyBinding::new("ctrl-shift-n", CreateNewFolder, Some("Explorer")),
            KeyBinding::new("ctrl-shift-f", RecursiveSearchEdit, Some("Explorer")),
            KeyBinding::new("ctrl-shift-s", OpenSettings, None),
            KeyBinding::new("ctrl-n", NewWindow, None),
            KeyBinding::new("ctrl-t", NewTab, None),
            KeyBinding::new("ctrl-w", CloseTab, None),
            KeyBinding::new("ctrl-tab", SelectNextTab, None),
            KeyBinding::new("ctrl-shift-tab", SelectPreviousTab, None),
            KeyBinding::new("ctrl-1", SelectTabByIndex { index: 0 }, None),
            KeyBinding::new("ctrl-2", SelectTabByIndex { index: 1 }, None),
            KeyBinding::new("ctrl-3", SelectTabByIndex { index: 2 }, None),
            KeyBinding::new("ctrl-4", SelectTabByIndex { index: 3 }, None),
            KeyBinding::new("ctrl-5", SelectTabByIndex { index: 4 }, None),
            KeyBinding::new("ctrl-6", SelectTabByIndex { index: 5 }, None),
            KeyBinding::new("ctrl-7", SelectTabByIndex { index: 6 }, None),
            KeyBinding::new("ctrl-8", SelectTabByIndex { index: 7 }, None),
            KeyBinding::new("ctrl-9", SelectTabByIndex { index: 8 }, None),
            KeyBinding::new("f2", RenameSelected, Some("Explorer")),
            KeyBinding::new("enter", RenameCommit, Some("ExplorerRenameInput")),
            KeyBinding::new("escape", RenameCancel, Some("ExplorerRenameInput")),
            KeyBinding::new("backspace", RenameBackspace, Some("ExplorerRenameInput")),
            KeyBinding::new(
                "ctrl-backspace",
                RenameBackspaceWord,
                Some("ExplorerRenameInput"),
            ),
            KeyBinding::new("delete", RenameDelete, Some("ExplorerRenameInput")),
            KeyBinding::new("left", RenameLeft, Some("ExplorerRenameInput")),
            KeyBinding::new("right", RenameRight, Some("ExplorerRenameInput")),
            KeyBinding::new("ctrl-left", RenameWordLeft, Some("ExplorerRenameInput")),
            KeyBinding::new("ctrl-right", RenameWordRight, Some("ExplorerRenameInput")),
            KeyBinding::new("shift-left", RenameSelectLeft, Some("ExplorerRenameInput")),
            KeyBinding::new(
                "shift-right",
                RenameSelectRight,
                Some("ExplorerRenameInput"),
            ),
            KeyBinding::new(
                "ctrl-shift-left",
                RenameSelectWordLeft,
                Some("ExplorerRenameInput"),
            ),
            KeyBinding::new(
                "ctrl-shift-right",
                RenameSelectWordRight,
                Some("ExplorerRenameInput"),
            ),
            KeyBinding::new("home", RenameHome, Some("ExplorerRenameInput")),
            KeyBinding::new("end", RenameEnd, Some("ExplorerRenameInput")),
            KeyBinding::new("shift-home", RenameSelectHome, Some("ExplorerRenameInput")),
            KeyBinding::new("shift-end", RenameSelectEnd, Some("ExplorerRenameInput")),
            KeyBinding::new("ctrl-a", RenameSelectAll, Some("ExplorerRenameInput")),
            KeyBinding::new("ctrl-c", RenameCopy, Some("ExplorerRenameInput")),
            KeyBinding::new("ctrl-x", RenameCut, Some("ExplorerRenameInput")),
            KeyBinding::new("ctrl-v", RenamePaste, Some("ExplorerRenameInput")),
            KeyBinding::new("up", RenameNoop, Some("ExplorerRenameInput")),
            KeyBinding::new("down", RenameNoop, Some("ExplorerRenameInput")),
            KeyBinding::new("shift-up", RenameNoop, Some("ExplorerRenameInput")),
            KeyBinding::new("shift-down", RenameNoop, Some("ExplorerRenameInput")),
            KeyBinding::new("enter", AddressCommit, Some("ExplorerAddressInput")),
            KeyBinding::new("escape", AddressCancel, Some("ExplorerAddressInput")),
            KeyBinding::new("backspace", AddressBackspace, Some("ExplorerAddressInput")),
            KeyBinding::new(
                "ctrl-backspace",
                AddressBackspaceWord,
                Some("ExplorerAddressInput"),
            ),
            KeyBinding::new("delete", AddressDelete, Some("ExplorerAddressInput")),
            KeyBinding::new("left", AddressLeft, Some("ExplorerAddressInput")),
            KeyBinding::new("right", AddressRight, Some("ExplorerAddressInput")),
            KeyBinding::new("ctrl-left", AddressWordLeft, Some("ExplorerAddressInput")),
            KeyBinding::new("ctrl-right", AddressWordRight, Some("ExplorerAddressInput")),
            KeyBinding::new(
                "shift-left",
                AddressSelectLeft,
                Some("ExplorerAddressInput"),
            ),
            KeyBinding::new(
                "shift-right",
                AddressSelectRight,
                Some("ExplorerAddressInput"),
            ),
            KeyBinding::new(
                "ctrl-shift-left",
                AddressSelectWordLeft,
                Some("ExplorerAddressInput"),
            ),
            KeyBinding::new(
                "ctrl-shift-right",
                AddressSelectWordRight,
                Some("ExplorerAddressInput"),
            ),
            KeyBinding::new("home", AddressHome, Some("ExplorerAddressInput")),
            KeyBinding::new("end", AddressEnd, Some("ExplorerAddressInput")),
            KeyBinding::new(
                "shift-home",
                AddressSelectHome,
                Some("ExplorerAddressInput"),
            ),
            KeyBinding::new("shift-end", AddressSelectEnd, Some("ExplorerAddressInput")),
            KeyBinding::new("ctrl-a", AddressSelectAll, Some("ExplorerAddressInput")),
            KeyBinding::new("ctrl-c", AddressCopy, Some("ExplorerAddressInput")),
            KeyBinding::new("ctrl-x", AddressCut, Some("ExplorerAddressInput")),
            KeyBinding::new("ctrl-v", AddressPaste, Some("ExplorerAddressInput")),
            KeyBinding::new("up", AddressSuggestionUp, Some("ExplorerAddressInput")),
            KeyBinding::new("down", AddressSuggestionDown, Some("ExplorerAddressInput")),
            KeyBinding::new("tab", AddressAcceptSuggestion, Some("ExplorerAddressInput")),
            KeyBinding::new("enter", SearchCommit, Some("ExplorerSearchInput")),
            KeyBinding::new("escape", SearchCancel, Some("ExplorerSearchInput")),
            KeyBinding::new("backspace", SearchBackspace, Some("ExplorerSearchInput")),
            KeyBinding::new(
                "ctrl-backspace",
                SearchBackspaceWord,
                Some("ExplorerSearchInput"),
            ),
            KeyBinding::new("delete", SearchDelete, Some("ExplorerSearchInput")),
            KeyBinding::new("left", SearchLeft, Some("ExplorerSearchInput")),
            KeyBinding::new("right", SearchRight, Some("ExplorerSearchInput")),
            KeyBinding::new("ctrl-left", SearchWordLeft, Some("ExplorerSearchInput")),
            KeyBinding::new("ctrl-right", SearchWordRight, Some("ExplorerSearchInput")),
            KeyBinding::new("shift-left", SearchSelectLeft, Some("ExplorerSearchInput")),
            KeyBinding::new(
                "shift-right",
                SearchSelectRight,
                Some("ExplorerSearchInput"),
            ),
            KeyBinding::new(
                "ctrl-shift-left",
                SearchSelectWordLeft,
                Some("ExplorerSearchInput"),
            ),
            KeyBinding::new(
                "ctrl-shift-right",
                SearchSelectWordRight,
                Some("ExplorerSearchInput"),
            ),
            KeyBinding::new("home", SearchHome, Some("ExplorerSearchInput")),
            KeyBinding::new("end", SearchEnd, Some("ExplorerSearchInput")),
            KeyBinding::new("shift-home", SearchSelectHome, Some("ExplorerSearchInput")),
            KeyBinding::new("shift-end", SearchSelectEnd, Some("ExplorerSearchInput")),
            KeyBinding::new("ctrl-a", SearchSelectAll, Some("ExplorerSearchInput")),
            KeyBinding::new("ctrl-c", SearchCopy, Some("ExplorerSearchInput")),
            KeyBinding::new("ctrl-x", SearchCut, Some("ExplorerSearchInput")),
            KeyBinding::new("ctrl-v", SearchPaste, Some("ExplorerSearchInput")),
        ]);

        open_explorer_window(cx);
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{ConfigPlatform, ExplorerSettings, config_dir_for};
    use gpui::TestAppContext;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn embedded_icon_fonts_are_present() {
        assert!(!SEGOE_FLUENT_ICONS.is_empty());
        assert!(!SEGOE_MDL2_ASSETS.is_empty());
        assert!(SEGOE_FLUENT_ICONS.len() > SEGOE_MDL2_ASSETS.len());
    }

    #[test]
    fn window_state_serializes_with_lowercase_state() {
        let state = StoredWindowState::new(10.0, 20.0, 800.0, 600.0, StoredWindowMode::Maximized);
        let json = serde_json::to_string(&state).expect("serialize state");

        assert!(json.contains("\"x\":10.0"));
        assert!(json.contains("\"y\":20.0"));
        assert!(json.contains("\"state\":\"maximized\""));
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
                MIN_WINDOW_WIDTH - 1.0,
                600.0,
                StoredWindowMode::Windowed
            )
            .is_valid()
        );
        assert!(
            !StoredWindowState::new(
                0.0,
                0.0,
                800.0,
                MIN_WINDOW_HEIGHT - 1.0,
                StoredWindowMode::Windowed
            )
            .is_valid()
        );
        assert!(
            !StoredWindowState::new(f32::NAN, 0.0, 800.0, 600.0, StoredWindowMode::Windowed)
                .is_valid()
        );
        assert!(
            !StoredWindowState::new(0.0, f32::NAN, 800.0, 600.0, StoredWindowMode::Windowed)
                .is_valid()
        );
        assert!(
            !StoredWindowState::new(0.0, 0.0, f32::NAN, 600.0, StoredWindowMode::Windowed)
                .is_valid()
        );
        assert!(
            StoredWindowState::new(
                0.0,
                0.0,
                MIN_WINDOW_WIDTH,
                MIN_WINDOW_HEIGHT,
                StoredWindowMode::Windowed
            )
            .is_valid()
        );
    }

    #[test]
    fn window_bounds_state_preserves_windowed_and_maximized_but_skips_fullscreen() {
        let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(900.0), px(700.0)));

        assert_eq!(
            StoredWindowState::from_window_bounds(WindowBounds::Windowed(bounds)),
            Some(StoredWindowState::new(
                10.0,
                20.0,
                900.0,
                700.0,
                StoredWindowMode::Windowed
            ))
        );
        assert_eq!(
            StoredWindowState::from_window_bounds(WindowBounds::Maximized(bounds)),
            Some(StoredWindowState::new(
                10.0,
                20.0,
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
    fn stored_window_state_restores_bounds_that_fit_a_current_display() {
        let display = Bounds::new(point(px(0.0), px(0.0)), size(px(1920.0), px(1080.0)));
        let state = StoredWindowState::new(10.0, 20.0, 900.0, 700.0, StoredWindowMode::Windowed);
        let expected = Bounds::new(point(px(10.0), px(20.0)), size(px(900.0), px(700.0)));

        assert_eq!(
            state.to_window_bounds(&[display]),
            Some(WindowBounds::Windowed(expected))
        );
    }

    #[test]
    fn stored_window_state_preserves_maximized_restore_bounds_when_safe() {
        let display = Bounds::new(point(px(0.0), px(0.0)), size(px(1920.0), px(1080.0)));
        let state = StoredWindowState::new(10.0, 20.0, 900.0, 700.0, StoredWindowMode::Maximized);
        let expected = Bounds::new(point(px(10.0), px(20.0)), size(px(900.0), px(700.0)));

        assert_eq!(
            state.to_window_bounds(&[display]),
            Some(WindowBounds::Maximized(expected))
        );
    }

    #[test]
    fn stored_window_state_rejects_bounds_outside_current_displays() {
        let display = Bounds::new(point(px(0.0), px(0.0)), size(px(1920.0), px(1080.0)));

        assert_eq!(
            StoredWindowState::new(1700.0, 20.0, 900.0, 700.0, StoredWindowMode::Windowed)
                .to_window_bounds(&[display]),
            None
        );
        assert_eq!(
            StoredWindowState::new(10.0, 20.0, 900.0, 700.0, StoredWindowMode::Windowed)
                .to_window_bounds(&[]),
            None
        );
    }

    #[test]
    fn startup_window_bounds_falls_back_to_default_when_saved_bounds_are_not_safe() {
        let display = Bounds::new(point(px(0.0), px(0.0)), size(px(1920.0), px(1080.0)));
        let default_bounds = WindowBounds::Windowed(Bounds::new(
            point(px(448.0), px(130.0)),
            size(px(DEFAULT_WINDOW_WIDTH), px(DEFAULT_WINDOW_HEIGHT)),
        ));

        assert_eq!(
            startup_window_bounds_from_state(None, &[display], default_bounds),
            default_bounds
        );
        assert_eq!(
            startup_window_bounds_from_state(
                Some(StoredWindowState::new(
                    1700.0,
                    20.0,
                    900.0,
                    700.0,
                    StoredWindowMode::Windowed,
                )),
                &[display],
                default_bounds,
            ),
            default_bounds
        );
    }

    #[test]
    fn new_window_placement_offsets_windowed_source_by_fifty_pixels() {
        let display = Bounds::new(point(px(0.0), px(0.0)), size(px(1920.0), px(1080.0)));
        let source = Bounds::new(point(px(100.0), px(120.0)), size(px(800.0), px(600.0)));
        let expected = Bounds::new(point(px(150.0), px(170.0)), source.size);

        assert_eq!(
            new_window_placement_from_source(WindowBounds::Windowed(source), &[display]),
            (WindowBounds::Windowed(expected), Some(0))
        );
    }

    #[test]
    fn new_window_placement_opens_maximized_source_as_offset_windowed_bounds() {
        let display = Bounds::new(point(px(0.0), px(0.0)), size(px(1920.0), px(1080.0)));
        let source = Bounds::new(point(px(100.0), px(120.0)), size(px(800.0), px(600.0)));
        let expected = Bounds::new(point(px(150.0), px(170.0)), source.size);

        assert_eq!(
            new_window_placement_from_source(WindowBounds::Maximized(source), &[display]),
            (WindowBounds::Windowed(expected), Some(0))
        );
    }

    #[test]
    fn new_window_placement_wraps_near_display_edge() {
        let display = Bounds::new(point(px(0.0), px(0.0)), size(px(1280.0), px(720.0)));
        let source = Bounds::new(point(px(900.0), px(500.0)), size(px(800.0), px(400.0)));
        let expected = Bounds::new(point(px(50.0), px(50.0)), source.size);

        assert_eq!(
            new_window_placement_from_source(WindowBounds::Windowed(source), &[display]),
            (WindowBounds::Windowed(expected), Some(0))
        );
    }

    #[test]
    fn new_window_placement_clamps_oversized_wrapped_bounds_to_display() {
        let display = Bounds::new(point(px(0.0), px(0.0)), size(px(1280.0), px(720.0)));
        let source = Bounds::new(point(px(0.0), px(0.0)), size(px(1280.0), px(720.0)));
        let expected = Bounds::new(point(px(50.0), px(50.0)), size(px(1230.0), px(670.0)));

        assert_eq!(
            new_window_placement_from_source(WindowBounds::Fullscreen(source), &[display]),
            (WindowBounds::Windowed(expected), Some(0))
        );
    }

    #[test]
    fn new_window_placement_prefers_display_containing_source_center() {
        let first = Bounds::new(point(px(0.0), px(0.0)), size(px(1920.0), px(1080.0)));
        let second = Bounds::new(point(px(1920.0), px(0.0)), size(px(1920.0), px(1080.0)));
        let source = Bounds::new(point(px(2100.0), px(120.0)), size(px(800.0), px(600.0)));
        let expected = Bounds::new(point(px(2150.0), px(170.0)), source.size);

        assert_eq!(
            new_window_placement_from_source(WindowBounds::Windowed(source), &[first, second]),
            (WindowBounds::Windowed(expected), Some(1))
        );
    }

    #[gpui::test]
    fn new_window_action_opens_offset_window_at_active_path(cx: &mut TestAppContext) {
        initialize_test_explorer_app(cx);
        let dir = unique_temp_dir("new-window-action");
        fs::create_dir_all(&dir).expect("create test directory");
        let initial_bounds = Bounds::new(point(px(100.0), px(120.0)), size(px(800.0), px(600.0)));

        cx.update(|app| {
            open_explorer_window_at(
                dir.clone(),
                WindowBounds::Windowed(initial_bounds),
                None,
                app,
            );
        });
        cx.run_until_parked();

        let first_window = cx.windows()[0];
        cx.dispatch_action(first_window, NewWindow);
        cx.run_until_parked();

        let windows = cx.windows();
        assert_eq!(windows.len(), 2);
        let new_window_bounds = windows[1]
            .update(cx, |_, window, _| window.window_bounds())
            .expect("read new window bounds");
        assert_eq!(
            new_window_bounds,
            WindowBounds::Windowed(Bounds::new(
                point(px(150.0), px(170.0)),
                initial_bounds.size
            ))
        );

        let new_window_path = windows[1]
            .read(cx, |explorer: gpui::Entity<Explorer>, app| {
                explorer.read(app).explorer.read(app).active_path(app)
            })
            .expect("read new window root")
            .expect("new window has active tab");
        assert_eq!(new_window_path, dir);

        for window in windows {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
        cx.run_until_parked();
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn window_state_paths_follow_platform_conventions() {
        assert_eq!(
            test_window_state_path(ConfigPlatform::MacOS, &[("HOME", "home")]),
            Some(
                PathBuf::from("home")
                    .join(".config")
                    .join("explorer")
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
                    .join("explorer")
                    .join(WINDOW_STATE_FILE_NAME)
            )
        );
        assert_eq!(
            test_window_state_path(ConfigPlatform::Linux, &[("HOME", "home")]),
            Some(
                PathBuf::from("home")
                    .join(".config")
                    .join("explorer")
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
                0.0,
                0.0,
                MIN_WINDOW_WIDTH - 1.0,
                600.0,
                StoredWindowMode::Windowed,
            ))
            .expect("serialize invalid state"),
        )
        .expect("write invalid state");
        assert_eq!(load_window_state_from_path(&invalid), None);

        let legacy = dir.join("legacy.json");
        fs::write(
            &legacy,
            r#"{"width":900.0,"height":700.0,"state":"windowed"}"#,
        )
        .expect("write legacy state");
        assert_eq!(load_window_state_from_path(&legacy), None);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn window_state_writer_creates_parent_directory_and_round_trips() {
        let path = unique_temp_dir("writer")
            .join("nested")
            .join(WINDOW_STATE_FILE_NAME);
        let state = StoredWindowState::new(12.0, 34.0, 960.0, 540.0, StoredWindowMode::Windowed);

        save_window_state_to_path(&path, &state).expect("save state");
        assert_eq!(load_window_state_from_path(&path), Some(state));

        let root = path
            .parent()
            .and_then(Path::parent)
            .expect("state path has test root");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn app_reopen_only_opens_window_when_none_exist() {
        assert!(should_open_window_on_reopen(0));
        assert!(!should_open_window_on_reopen(1));
        assert!(!should_open_window_on_reopen(2));
    }

    #[test]
    fn linux_display_selector_prefers_x11_over_valid_wayland_by_default() {
        assert_eq!(
            select_linux_display_backend(
                linux_display_env(&[
                    ("WAYLAND_DISPLAY", "wayland-1"),
                    ("XDG_RUNTIME_DIR", "/run/user/1000"),
                    ("DISPLAY", ":0"),
                ]),
                |path| path == Path::new("/run/user/1000/wayland-1")
            ),
            LinuxDisplaySelection::Backend(LinuxDisplayBackend::X11)
        );
    }

    #[test]
    fn linux_display_selector_uses_wayland_when_x11_is_unavailable() {
        assert_eq!(
            select_linux_display_backend(
                linux_display_env(&[
                    ("WAYLAND_DISPLAY", "wayland-1"),
                    ("XDG_RUNTIME_DIR", "/run/user/1000"),
                ]),
                |path| path == Path::new("/run/user/1000/wayland-1")
            ),
            LinuxDisplaySelection::Backend(LinuxDisplayBackend::Wayland {
                display: OsString::from("wayland-1")
            })
        );
    }

    #[test]
    fn linux_display_selector_uses_x11_when_only_x11_is_available() {
        assert_eq!(
            select_linux_display_backend(linux_display_env(&[("DISPLAY", ":0")]), |_| false),
            LinuxDisplaySelection::Backend(LinuxDisplayBackend::X11)
        );
    }

    #[test]
    fn linux_display_selector_probes_default_wayland_socket() {
        assert_eq!(
            select_linux_display_backend(
                linux_display_env(&[("XDG_RUNTIME_DIR", "/run/user/1000")]),
                |path| path == Path::new("/run/user/1000/wayland-0")
            ),
            LinuxDisplaySelection::Backend(LinuxDisplayBackend::Wayland {
                display: OsString::from("wayland-0")
            })
        );
    }

    #[test]
    fn linux_display_selector_forces_wayland_when_requested_and_connectable() {
        assert_eq!(
            select_linux_display_backend(
                linux_display_env(&[
                    ("EXPLORER_LINUX_BACKEND", "wayland"),
                    ("WAYLAND_DISPLAY", "wayland-1"),
                    ("XDG_RUNTIME_DIR", "/run/user/1000"),
                    ("DISPLAY", ":0"),
                ]),
                |path| path == Path::new("/run/user/1000/wayland-1")
            ),
            LinuxDisplaySelection::Backend(LinuxDisplayBackend::Wayland {
                display: OsString::from("wayland-1")
            })
        );
    }

    #[test]
    fn linux_display_selector_auto_override_uses_default_backend_order() {
        assert_eq!(
            select_linux_display_backend(
                linux_display_env(&[
                    ("EXPLORER_LINUX_BACKEND", "auto"),
                    ("WAYLAND_DISPLAY", "wayland-1"),
                    ("XDG_RUNTIME_DIR", "/run/user/1000"),
                    ("DISPLAY", ":0"),
                ]),
                |path| path == Path::new("/run/user/1000/wayland-1")
            ),
            LinuxDisplaySelection::Backend(LinuxDisplayBackend::X11)
        );
    }

    #[test]
    fn linux_display_selector_forces_x11_when_requested_and_available() {
        assert_eq!(
            select_linux_display_backend(
                linux_display_env(&[
                    ("EXPLORER_LINUX_BACKEND", "x11"),
                    ("WAYLAND_DISPLAY", "wayland-1"),
                    ("XDG_RUNTIME_DIR", "/run/user/1000"),
                    ("DISPLAY", ":0"),
                ]),
                |path| path == Path::new("/run/user/1000/wayland-1")
            ),
            LinuxDisplaySelection::Backend(LinuxDisplayBackend::X11)
        );
    }

    #[test]
    fn linux_display_selector_returns_fatal_when_requested_wayland_is_unavailable() {
        assert_eq!(
            select_linux_display_backend(
                linux_display_env(&[
                    ("EXPLORER_LINUX_BACKEND", "wayland"),
                    ("WAYLAND_DISPLAY", "wayland-1"),
                    ("XDG_RUNTIME_DIR", "/run/user/1000"),
                    ("DISPLAY", ":0"),
                ]),
                |_| false
            ),
            LinuxDisplaySelection::FatalNoDisplay
        );
    }

    #[test]
    fn linux_display_selector_returns_fatal_when_requested_x11_is_unavailable() {
        assert_eq!(
            select_linux_display_backend(
                linux_display_env(&[
                    ("EXPLORER_LINUX_BACKEND", "x11"),
                    ("WAYLAND_DISPLAY", "wayland-1"),
                    ("XDG_RUNTIME_DIR", "/run/user/1000"),
                ]),
                |path| path == Path::new("/run/user/1000/wayland-1")
            ),
            LinuxDisplaySelection::FatalNoDisplay
        );
    }

    #[test]
    fn linux_display_selector_ignores_empty_display_variables() {
        assert_eq!(
            select_linux_display_backend(
                linux_display_env(&[
                    ("WAYLAND_DISPLAY", ""),
                    ("XDG_RUNTIME_DIR", "/run/user/1000"),
                    ("DISPLAY", ""),
                ]),
                |_| false
            ),
            LinuxDisplaySelection::FatalNoDisplay
        );
    }

    #[test]
    fn linux_display_selector_never_selects_headless() {
        assert_eq!(
            select_linux_display_backend(linux_display_env(&[("ZED_HEADLESS", "1")]), |_| false),
            LinuxDisplaySelection::FatalNoDisplay
        );
    }

    #[test]
    fn linux_display_selector_returns_fatal_when_no_gui_display_exists() {
        assert_eq!(
            select_linux_display_backend(linux_display_env(&[]), |_| false),
            LinuxDisplaySelection::FatalNoDisplay
        );
    }

    fn test_window_state_path(platform: ConfigPlatform, vars: &[(&str, &str)]) -> Option<PathBuf> {
        config_dir_for(platform, |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| PathBuf::from(value))
        })
        .map(|dir| dir.join(WINDOW_STATE_FILE_NAME))
    }

    fn linux_display_env(vars: &[(&str, &str)]) -> LinuxDisplayEnv {
        LinuxDisplayEnv {
            wayland_display: test_env_var(vars, "WAYLAND_DISPLAY"),
            xdg_runtime_dir: test_env_var(vars, "XDG_RUNTIME_DIR"),
            x11_display: test_env_var(vars, "DISPLAY"),
            backend_preference: test_env_var(vars, "EXPLORER_LINUX_BACKEND"),
            zed_headless: test_env_var(vars, "ZED_HEADLESS"),
        }
    }

    fn test_env_var(vars: &[(&str, &str)], name: &str) -> Option<OsString> {
        vars.iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| OsString::from(value))
    }

    fn initialize_test_explorer_app(cx: &TestAppContext) {
        cx.update(|app| {
            register_embedded_fonts(app);
            app.set_global(SettingsState::for_test(ExplorerSettings::default()));
            crate::explorer::initialize_native_icon_cache(app);
            crate::explorer::initialize_image_thumbnail_cache(app);
            crate::explorer::initialize_folder_size_cache(app);
            crate::explorer::initialize_file_checksum_cache(app);
        });
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
