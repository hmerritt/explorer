use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::os::unix::net::UnixStream;
use std::{
    borrow::Cow,
    env,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process, thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures::{
    StreamExt,
    channel::mpsc::{self, UnboundedReceiver, UnboundedSender},
};
use gpui::{
    App, Application, Bounds, Context, DisplayId, Global, KeyBinding, Pixels, SharedString,
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
    OpenSettings, PasteClipboard, PermanentlyDeleteSelected, PropertiesOpenNext,
    PropertiesOpenPrevious, RecursiveSearchEdit, Refresh, RenameBackspace, RenameBackspaceWord,
    RenameCancel, RenameCommit, RenameCopy, RenameCut, RenameDelete, RenameEnd, RenameHome,
    RenameLeft, RenameNoop, RenamePaste, RenameRight, RenameSelectAll, RenameSelectEnd,
    RenameSelectHome, RenameSelectLeft, RenameSelectRight, RenameSelectWordLeft,
    RenameSelectWordRight, RenameSelected, RenameWordLeft, RenameWordRight, SearchBackspace,
    SearchBackspaceWord, SearchCancel, SearchCommit, SearchCopy, SearchCut, SearchDelete,
    SearchEdit, SearchEnd, SearchHome, SearchLeft, SearchPaste, SearchRight, SearchSelectAll,
    SearchSelectEnd, SearchSelectHome, SearchSelectLeft, SearchSelectRight, SearchSelectWordLeft,
    SearchSelectWordRight, SearchWordLeft, SearchWordRight, SelectAll, SelectNextTab,
    SelectPreviousTab, SelectTabByIndex, TextInputRedo, TextInputUndo, TrashSelected,
    UndoFileOperation,
};
use crate::image_viewer::{
    ImageOpenNext, ImageOpenPrevious, ImageToggleActualSize, ImageZoomIn, ImageZoomOut,
};
use crate::settings::{APP_ID, NewWindowBehaviour, SettingsState, config_dir};
use crate::window_state::{
    StoredWindowState, WindowStateOptions,
    load_window_state_from_path as load_stored_window_state_from_path, save_window_state_to_path,
};

const APP_TITLE: &str = "Explorer";
const WINDOW_STATE_FILE_NAME: &str = "window-state.json";
const DEFAULT_WINDOW_WIDTH: f32 = 1024.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 820.0;
const MIN_WINDOW_WIDTH: f32 = 400.0;
const MIN_WINDOW_HEIGHT: f32 = 120.0;
const NEW_WINDOW_OFFSET: f32 = 50.0;
const SINGLE_INSTANCE_LOCK_FILE_NAME: &str = "single-instance.lock";
const SINGLE_INSTANCE_ENDPOINT_FILE_NAME: &str = "single-instance.json";
const SINGLE_INSTANCE_PROTOCOL_VERSION: u32 = 1;
const SINGLE_INSTANCE_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const SINGLE_INSTANCE_IO_TIMEOUT: Duration = Duration::from_secs(2);
const EXPLORER_WINDOW_STATE_OPTIONS: WindowStateOptions = WindowStateOptions {
    min_width: MIN_WINDOW_WIDTH,
    min_height: MIN_WINDOW_HEIGHT,
    include_fullscreen: false,
};
const SEGOE_FLUENT_ICONS: &[u8] = include_bytes!("../assets/fonts/Segoe Fluent Icons.ttf");
const SEGOE_MDL2_ASSETS: &[u8] = include_bytes!("../assets/fonts/Segoe MDL2 Assets.ttf");
#[cfg(any(target_os = "linux", test))]
const DEFAULT_WAYLAND_DISPLAY: &str = "wayland-0";

struct Explorer {
    explorer: gpui::Entity<ExplorerTabs>,
}

struct SingleInstanceServer {
    _guard: SingleInstanceGuard,
}

impl Global for SingleInstanceServer {}

struct SingleInstanceGuard {
    _lock_file: File,
    paths: SingleInstancePaths,
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.paths.endpoint_path);
        let _ = fs::remove_file(&self.paths.lock_path);
    }
}

struct SingleInstancePrimary {
    guard: SingleInstanceGuard,
    requests: UnboundedReceiver<LaunchRequest>,
}

enum SingleInstanceLaunch {
    Primary(Option<SingleInstancePrimary>),
    RoutedToPrimary,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
struct LaunchRequest {
    image_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SingleInstancePaths {
    lock_path: PathBuf,
    endpoint_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
struct SingleInstanceEndpoint {
    version: u32,
    port: u16,
    token: String,
    pid: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
struct SingleInstanceMessage {
    token: String,
    request: LaunchRequest,
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
        .and_then(|state| state.to_window_bounds(display_bounds, EXPLORER_WINDOW_STATE_OPTIONS))
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
    load_stored_window_state_from_path(path, EXPLORER_WINDOW_STATE_OPTIONS)
}

#[cfg_attr(test, allow(dead_code))]
fn save_window_bounds(window_bounds: WindowBounds) {
    let Some(state) =
        StoredWindowState::from_window_bounds(window_bounds, EXPLORER_WINDOW_STATE_OPTIONS)
    else {
        return;
    };
    let Some(path) = window_state_path() else {
        return;
    };

    let _ = save_window_state_to_path(&path, &state);
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

impl LaunchRequest {
    fn from_args(args: impl IntoIterator<Item = OsString>) -> Self {
        Self {
            image_path: crate::image_viewer::startup_image_path(args),
        }
    }
}

fn handle_initial_launch(request: LaunchRequest, cx: &mut App) {
    handle_launch_request(request, cx);
}

fn handle_launch_request(request: LaunchRequest, cx: &mut App) {
    if let Some(path) = request.image_path {
        crate::image_viewer::open_image_window(path, cx);
    } else {
        handle_explorer_launch_request(cx);
    }
    cx.activate(true);
}

fn handle_explorer_launch_request(cx: &mut App) {
    match cx.global::<SettingsState>().value.app.new_window_behaviour {
        NewWindowBehaviour::Open => open_explorer_window(cx),
        NewWindowBehaviour::Focus => {
            if !focus_existing_window(cx) {
                open_explorer_window(cx);
            }
        }
    }
}

fn focus_existing_window(cx: &mut App) -> bool {
    let mut windows = cx.window_stack().unwrap_or_default();
    if windows.is_empty()
        && let Some(active_window) = cx.active_window()
    {
        windows.push(active_window);
    }
    if windows.is_empty() {
        windows = cx.windows();
    }

    for handle in windows {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return true;
        }
    }

    false
}

fn start_single_instance_request_handler(
    mut requests: UnboundedReceiver<LaunchRequest>,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        while let Some(request) = requests.next().await {
            if let Err(error) = cx.update(|cx| handle_launch_request(request, cx)) {
                eprintln!("Unable to handle Explorer launch request: {error}");
                break;
            }
        }
    })
    .detach();
}

#[cfg(any(target_os = "macos", test))]
fn launch_requests_from_open_urls(urls: impl IntoIterator<Item = String>) -> Vec<LaunchRequest> {
    urls.into_iter()
        .filter_map(|url| {
            let path = reqwest::Url::parse(&url).ok()?.to_file_path().ok()?;
            crate::image_viewer::image_like_existing_file(&path).then_some(LaunchRequest {
                image_path: Some(path),
            })
        })
        .collect()
}

#[cfg(any(target_os = "macos", test))]
fn handle_open_urls(urls: Vec<String>, cx: &mut App) {
    for request in launch_requests_from_open_urls(urls) {
        handle_launch_request(request, cx);
    }
}

#[cfg(target_os = "macos")]
fn start_open_url_request_handler(mut requests: UnboundedReceiver<Vec<String>>, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Some(urls) = requests.next().await {
            if let Err(error) = cx.update(|cx| handle_open_urls(urls, cx)) {
                eprintln!("Unable to handle macOS open URL request: {error}");
                break;
            }
        }
    })
    .detach();
}

fn prepare_single_instance_launch(request: &LaunchRequest) -> SingleInstanceLaunch {
    let Some(config_dir) = config_dir() else {
        eprintln!(
            "Unable to determine Explorer settings directory; single-instance routing disabled."
        );
        return SingleInstanceLaunch::Primary(None);
    };

    prepare_single_instance_launch_in_dir(config_dir, request)
}

fn prepare_single_instance_launch_in_dir(
    config_dir: PathBuf,
    request: &LaunchRequest,
) -> SingleInstanceLaunch {
    let paths = single_instance_paths(&config_dir);
    match start_single_instance_primary(paths.clone()) {
        Ok(primary) => SingleInstanceLaunch::Primary(Some(primary)),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            match send_launch_request_from_paths(&paths, request) {
                Ok(()) => SingleInstanceLaunch::RoutedToPrimary,
                Err(error) => {
                    eprintln!(
                        "Unable to route Explorer launch to the existing instance: {error}. Starting a new instance."
                    );
                    remove_single_instance_files(&paths);
                    match start_single_instance_primary(paths) {
                        Ok(primary) => SingleInstanceLaunch::Primary(Some(primary)),
                        Err(error) => {
                            eprintln!(
                                "Unable to claim Explorer single-instance ownership after stale cleanup: {error}"
                            );
                            SingleInstanceLaunch::Primary(None)
                        }
                    }
                }
            }
        }
        Err(error) => {
            eprintln!("Unable to claim Explorer single-instance ownership: {error}");
            SingleInstanceLaunch::Primary(None)
        }
    }
}

fn single_instance_paths(config_dir: &Path) -> SingleInstancePaths {
    SingleInstancePaths {
        lock_path: config_dir.join(SINGLE_INSTANCE_LOCK_FILE_NAME),
        endpoint_path: config_dir.join(SINGLE_INSTANCE_ENDPOINT_FILE_NAME),
    }
}

fn start_single_instance_primary(paths: SingleInstancePaths) -> io::Result<SingleInstancePrimary> {
    if let Some(parent) = paths.lock_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let lock_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&paths.lock_path)?;
    let listener = match TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = fs::remove_file(&paths.lock_path);
            return Err(error);
        }
    };
    let token = single_instance_token();
    let endpoint = SingleInstanceEndpoint {
        version: SINGLE_INSTANCE_PROTOCOL_VERSION,
        port: listener.local_addr()?.port(),
        token: token.clone(),
        pid: process::id(),
    };

    if let Err(error) = save_single_instance_endpoint(&paths.endpoint_path, &endpoint) {
        let _ = fs::remove_file(&paths.lock_path);
        return Err(error);
    }

    let (tx, requests) = mpsc::unbounded();
    spawn_single_instance_listener(listener, token, tx);

    Ok(SingleInstancePrimary {
        guard: SingleInstanceGuard {
            _lock_file: lock_file,
            paths,
        },
        requests,
    })
}

fn save_single_instance_endpoint(path: &Path, endpoint: &SingleInstanceEndpoint) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string(endpoint).map_err(io::Error::other)?;
    fs::write(path, json)
}

fn spawn_single_instance_listener(
    listener: TcpListener,
    token: String,
    tx: UnboundedSender<LaunchRequest>,
) {
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_single_instance_stream(stream, &token, &tx),
                Err(error) => {
                    eprintln!("Unable to accept Explorer launch request: {error}");
                    break;
                }
            }
        }
    });
}

fn handle_single_instance_stream(
    mut stream: TcpStream,
    token: &str,
    tx: &UnboundedSender<LaunchRequest>,
) {
    let _ = stream.set_read_timeout(Some(SINGLE_INSTANCE_IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(SINGLE_INSTANCE_IO_TIMEOUT));

    let mut line = String::new();
    let read_result = {
        let mut reader = BufReader::new(&mut stream);
        reader.read_line(&mut line)
    };

    let response = match read_result {
        Ok(0) => "empty\n",
        Ok(_) => match serde_json::from_str::<SingleInstanceMessage>(line.trim_end()) {
            Ok(message) if message.token == token => {
                if tx.unbounded_send(message.request).is_ok() {
                    "ok\n"
                } else {
                    "closed\n"
                }
            }
            Ok(_) => "invalid-token\n",
            Err(_) => "invalid-json\n",
        },
        Err(_) => "read-error\n",
    };

    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn send_launch_request_from_paths(
    paths: &SingleInstancePaths,
    request: &LaunchRequest,
) -> io::Result<()> {
    let endpoint = load_single_instance_endpoint(&paths.endpoint_path)?;
    send_launch_request_to_endpoint(&endpoint, request)
}

fn load_single_instance_endpoint(path: &Path) -> io::Result<SingleInstanceEndpoint> {
    let endpoint = serde_json::from_str::<SingleInstanceEndpoint>(&fs::read_to_string(path)?)
        .map_err(io::Error::other)?;
    if endpoint.version == SINGLE_INSTANCE_PROTOCOL_VERSION {
        Ok(endpoint)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported single-instance endpoint version",
        ))
    }
}

fn send_launch_request_to_endpoint(
    endpoint: &SingleInstanceEndpoint,
    request: &LaunchRequest,
) -> io::Result<()> {
    let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, endpoint.port));
    let mut stream = TcpStream::connect_timeout(&addr, SINGLE_INSTANCE_CONNECT_TIMEOUT)?;
    stream.set_read_timeout(Some(SINGLE_INSTANCE_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(SINGLE_INSTANCE_IO_TIMEOUT))?;

    let message = SingleInstanceMessage {
        token: endpoint.token.clone(),
        request: request.clone(),
    };
    let json = serde_json::to_string(&message).map_err(io::Error::other)?;
    stream.write_all(json.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response)?;
    if response.trim_end() == "ok" {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "single-instance server returned {}",
            response.trim_end()
        )))
    }
}

fn remove_single_instance_files(paths: &SingleInstancePaths) {
    let _ = fs::remove_file(&paths.endpoint_path);
    let _ = fs::remove_file(&paths.lock_path);
}

fn single_instance_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{:x}-{nanos:x}", process::id())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KeyBindingProfile {
    Mac,
    WindowsLike,
}

fn platform_key_binding_profile() -> KeyBindingProfile {
    if cfg!(target_os = "macos") {
        KeyBindingProfile::Mac
    } else {
        KeyBindingProfile::WindowsLike
    }
}

fn platform_key_bindings() -> Vec<KeyBinding> {
    key_bindings_for_profile(platform_key_binding_profile())
}

fn key_bindings_for_profile(profile: KeyBindingProfile) -> Vec<KeyBinding> {
    let mut bindings = vec![
        KeyBinding::new("up", MoveUp, None),
        KeyBinding::new("down", MoveDown, None),
        KeyBinding::new("shift-up", ExtendUp, None),
        KeyBinding::new("shift-down", ExtendDown, None),
        KeyBinding::new("home", MoveHome, None),
        KeyBinding::new("end", MoveEnd, None),
        KeyBinding::new("shift-home", ExtendHome, None),
        KeyBinding::new("shift-end", ExtendEnd, None),
        KeyBinding::new("right", OpenSelected, None),
        KeyBinding::new("enter", EnterSelected, None),
        KeyBinding::new("f5", Refresh, None),
        KeyBinding::new("escape", CancelDrag, None),
        KeyBinding::new("escape", DialogCancel, Some("ExplorerDialog")),
        KeyBinding::new("enter", DialogConfirm, Some("ExplorerDialog")),
        KeyBinding::new("left", DialogFocusPrimary, Some("ExplorerDialog")),
        KeyBinding::new("up", DialogFocusPrimary, Some("ExplorerDialog")),
        KeyBinding::new("right", DialogFocusSecondary, Some("ExplorerDialog")),
        KeyBinding::new("down", DialogFocusSecondary, Some("ExplorerDialog")),
        KeyBinding::new("left", ImageOpenPrevious, Some("ImageViewer")),
        KeyBinding::new("right", ImageOpenNext, Some("ImageViewer")),
        KeyBinding::new("left", PropertiesOpenPrevious, Some("PropertiesDialog")),
        KeyBinding::new("right", PropertiesOpenNext, Some("PropertiesDialog")),
        KeyBinding::new(
            "left",
            PropertiesOpenPrevious,
            Some("PropertiesDialog > ImageViewer"),
        ),
        KeyBinding::new(
            "right",
            PropertiesOpenNext,
            Some("PropertiesDialog > ImageViewer"),
        ),
        KeyBinding::new("+", ImageZoomIn, Some("ImageViewer")),
        KeyBinding::new("=", ImageZoomIn, Some("ImageViewer")),
        KeyBinding::new("-", ImageZoomOut, Some("ImageViewer")),
        KeyBinding::new("f", ImageToggleActualSize, Some("ImageViewer")),
        KeyBinding::new("ctrl-tab", SelectNextTab, None),
        KeyBinding::new("ctrl-shift-tab", SelectPreviousTab, None),
    ];

    match profile {
        KeyBindingProfile::Mac => push_mac_key_bindings(&mut bindings),
        KeyBindingProfile::WindowsLike => push_windows_like_key_bindings(&mut bindings),
    }

    bindings
}

fn push_mac_key_bindings(bindings: &mut Vec<KeyBinding>) {
    bindings.extend([
        KeyBinding::new("cmd-[", GoBack, None),
        KeyBinding::new("alt-left", GoBack, None),
        KeyBinding::new("cmd-]", GoForward, None),
        KeyBinding::new("alt-right", GoForward, None),
        KeyBinding::new("cmd-up", GoUp, None),
        KeyBinding::new("alt-up", GoUp, None),
        KeyBinding::new("cmd-down", EnterSelected, None),
        KeyBinding::new("cmd-o", EnterSelected, None),
        KeyBinding::new("cmd-i", OpenProperties, Some("Explorer")),
        KeyBinding::new("alt-enter", OpenProperties, Some("Explorer")),
        KeyBinding::new("cmd-a", SelectAll, None),
        KeyBinding::new("cmd-c", CopySelected, None),
        KeyBinding::new("cmd-x", CutSelected, None),
        KeyBinding::new("cmd-v", PasteClipboard, None),
        KeyBinding::new("cmd-z", UndoFileOperation, Some("Explorer")),
        KeyBinding::new("cmd-backspace", TrashSelected, None),
        KeyBinding::new("cmd-delete", TrashSelected, None),
        KeyBinding::new("alt-cmd-backspace", PermanentlyDeleteSelected, None),
        KeyBinding::new("alt-cmd-delete", PermanentlyDeleteSelected, None),
        KeyBinding::new("shift-cmd-g", AddressEdit, Some("Explorer")),
        KeyBinding::new("cmd-f", SearchEdit, Some("Explorer")),
        KeyBinding::new("shift-cmd-n", CreateNewFolder, Some("Explorer")),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-n", NewWindow, None),
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("f2", RenameSelected, Some("Explorer")),
    ]);
    push_mac_text_input_key_bindings(bindings);
}

fn push_windows_like_key_bindings(bindings: &mut Vec<KeyBinding>) {
    bindings.extend([
        KeyBinding::new("left", GoUp, None),
        KeyBinding::new("alt-left", GoBack, None),
        KeyBinding::new("ctrl-right", OpenSelectedInNewTab, None),
        KeyBinding::new("alt-right", GoForward, None),
        KeyBinding::new("ctrl-enter", EnterSelectedInNewTab, None),
        KeyBinding::new("alt-enter", OpenProperties, Some("Explorer")),
        KeyBinding::new("backspace", GoUp, None),
        KeyBinding::new("alt-up", GoUp, None),
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
    ]);
    push_windows_like_text_input_key_bindings(bindings);
}

fn push_mac_text_input_key_bindings(bindings: &mut Vec<KeyBinding>) {
    push_mac_rename_input_key_bindings(bindings);
    push_mac_address_input_key_bindings(bindings);
    push_mac_search_input_key_bindings(bindings);
}

fn push_windows_like_text_input_key_bindings(bindings: &mut Vec<KeyBinding>) {
    push_windows_like_rename_input_key_bindings(bindings);
    push_windows_like_address_input_key_bindings(bindings);
    push_windows_like_search_input_key_bindings(bindings);
}

fn push_mac_rename_input_key_bindings(bindings: &mut Vec<KeyBinding>) {
    let context = Some("ExplorerRenameInput");
    bindings.extend([
        KeyBinding::new("enter", RenameCommit, context),
        KeyBinding::new("escape", RenameCancel, context),
        KeyBinding::new("backspace", RenameBackspace, context),
        KeyBinding::new("alt-backspace", RenameBackspaceWord, context),
        KeyBinding::new("cmd-backspace", gpui::NoAction {}, context),
        KeyBinding::new("alt-cmd-backspace", gpui::NoAction {}, context),
        KeyBinding::new("delete", RenameDelete, context),
        KeyBinding::new("left", RenameLeft, context),
        KeyBinding::new("right", RenameRight, context),
        KeyBinding::new("alt-left", RenameWordLeft, context),
        KeyBinding::new("alt-right", RenameWordRight, context),
        KeyBinding::new("shift-left", RenameSelectLeft, context),
        KeyBinding::new("shift-right", RenameSelectRight, context),
        KeyBinding::new("alt-shift-left", RenameSelectWordLeft, context),
        KeyBinding::new("alt-shift-right", RenameSelectWordRight, context),
        KeyBinding::new("home", RenameHome, context),
        KeyBinding::new("end", RenameEnd, context),
        KeyBinding::new("cmd-left", RenameHome, context),
        KeyBinding::new("cmd-right", RenameEnd, context),
        KeyBinding::new("shift-home", RenameSelectHome, context),
        KeyBinding::new("shift-end", RenameSelectEnd, context),
        KeyBinding::new("cmd-shift-left", RenameSelectHome, context),
        KeyBinding::new("cmd-shift-right", RenameSelectEnd, context),
        KeyBinding::new("cmd-a", RenameSelectAll, context),
        KeyBinding::new("cmd-c", RenameCopy, context),
        KeyBinding::new("cmd-x", RenameCut, context),
        KeyBinding::new("cmd-v", RenamePaste, context),
        KeyBinding::new("cmd-z", TextInputUndo, context),
        KeyBinding::new("cmd-shift-z", TextInputRedo, context),
        KeyBinding::new("up", RenameNoop, context),
        KeyBinding::new("down", RenameNoop, context),
        KeyBinding::new("shift-up", RenameNoop, context),
        KeyBinding::new("shift-down", RenameNoop, context),
    ]);
}

fn push_windows_like_rename_input_key_bindings(bindings: &mut Vec<KeyBinding>) {
    let context = Some("ExplorerRenameInput");
    bindings.extend([
        KeyBinding::new("enter", RenameCommit, context),
        KeyBinding::new("escape", RenameCancel, context),
        KeyBinding::new("backspace", RenameBackspace, context),
        KeyBinding::new("ctrl-backspace", RenameBackspaceWord, context),
        KeyBinding::new("delete", RenameDelete, context),
        KeyBinding::new("left", RenameLeft, context),
        KeyBinding::new("right", RenameRight, context),
        KeyBinding::new("ctrl-left", RenameWordLeft, context),
        KeyBinding::new("ctrl-right", RenameWordRight, context),
        KeyBinding::new("shift-left", RenameSelectLeft, context),
        KeyBinding::new("shift-right", RenameSelectRight, context),
        KeyBinding::new("ctrl-shift-left", RenameSelectWordLeft, context),
        KeyBinding::new("ctrl-shift-right", RenameSelectWordRight, context),
        KeyBinding::new("home", RenameHome, context),
        KeyBinding::new("end", RenameEnd, context),
        KeyBinding::new("shift-home", RenameSelectHome, context),
        KeyBinding::new("shift-end", RenameSelectEnd, context),
        KeyBinding::new("ctrl-a", RenameSelectAll, context),
        KeyBinding::new("ctrl-c", RenameCopy, context),
        KeyBinding::new("ctrl-x", RenameCut, context),
        KeyBinding::new("ctrl-v", RenamePaste, context),
        KeyBinding::new("ctrl-z", TextInputUndo, context),
        KeyBinding::new("ctrl-y", TextInputRedo, context),
        KeyBinding::new("up", RenameNoop, context),
        KeyBinding::new("down", RenameNoop, context),
        KeyBinding::new("shift-up", RenameNoop, context),
        KeyBinding::new("shift-down", RenameNoop, context),
    ]);
}

fn push_mac_address_input_key_bindings(bindings: &mut Vec<KeyBinding>) {
    let context = Some("ExplorerAddressInput");
    bindings.extend([
        KeyBinding::new("enter", AddressCommit, context),
        KeyBinding::new("escape", AddressCancel, context),
        KeyBinding::new("backspace", AddressBackspace, context),
        KeyBinding::new("alt-backspace", AddressBackspaceWord, context),
        KeyBinding::new("cmd-backspace", gpui::NoAction {}, context),
        KeyBinding::new("alt-cmd-backspace", gpui::NoAction {}, context),
        KeyBinding::new("delete", AddressDelete, context),
        KeyBinding::new("left", AddressLeft, context),
        KeyBinding::new("right", AddressRight, context),
        KeyBinding::new("alt-left", AddressWordLeft, context),
        KeyBinding::new("alt-right", AddressWordRight, context),
        KeyBinding::new("shift-left", AddressSelectLeft, context),
        KeyBinding::new("shift-right", AddressSelectRight, context),
        KeyBinding::new("alt-shift-left", AddressSelectWordLeft, context),
        KeyBinding::new("alt-shift-right", AddressSelectWordRight, context),
        KeyBinding::new("home", AddressHome, context),
        KeyBinding::new("end", AddressEnd, context),
        KeyBinding::new("cmd-left", AddressHome, context),
        KeyBinding::new("cmd-right", AddressEnd, context),
        KeyBinding::new("shift-home", AddressSelectHome, context),
        KeyBinding::new("shift-end", AddressSelectEnd, context),
        KeyBinding::new("cmd-shift-left", AddressSelectHome, context),
        KeyBinding::new("cmd-shift-right", AddressSelectEnd, context),
        KeyBinding::new("cmd-a", AddressSelectAll, context),
        KeyBinding::new("cmd-c", AddressCopy, context),
        KeyBinding::new("cmd-x", AddressCut, context),
        KeyBinding::new("cmd-v", AddressPaste, context),
        KeyBinding::new("cmd-z", TextInputUndo, context),
        KeyBinding::new("cmd-shift-z", TextInputRedo, context),
        KeyBinding::new("up", AddressSuggestionUp, context),
        KeyBinding::new("down", AddressSuggestionDown, context),
        KeyBinding::new("tab", AddressAcceptSuggestion, context),
    ]);
}

fn push_windows_like_address_input_key_bindings(bindings: &mut Vec<KeyBinding>) {
    let context = Some("ExplorerAddressInput");
    bindings.extend([
        KeyBinding::new("enter", AddressCommit, context),
        KeyBinding::new("escape", AddressCancel, context),
        KeyBinding::new("backspace", AddressBackspace, context),
        KeyBinding::new("ctrl-backspace", AddressBackspaceWord, context),
        KeyBinding::new("delete", AddressDelete, context),
        KeyBinding::new("left", AddressLeft, context),
        KeyBinding::new("right", AddressRight, context),
        KeyBinding::new("ctrl-left", AddressWordLeft, context),
        KeyBinding::new("ctrl-right", AddressWordRight, context),
        KeyBinding::new("shift-left", AddressSelectLeft, context),
        KeyBinding::new("shift-right", AddressSelectRight, context),
        KeyBinding::new("ctrl-shift-left", AddressSelectWordLeft, context),
        KeyBinding::new("ctrl-shift-right", AddressSelectWordRight, context),
        KeyBinding::new("home", AddressHome, context),
        KeyBinding::new("end", AddressEnd, context),
        KeyBinding::new("shift-home", AddressSelectHome, context),
        KeyBinding::new("shift-end", AddressSelectEnd, context),
        KeyBinding::new("ctrl-a", AddressSelectAll, context),
        KeyBinding::new("ctrl-c", AddressCopy, context),
        KeyBinding::new("ctrl-x", AddressCut, context),
        KeyBinding::new("ctrl-v", AddressPaste, context),
        KeyBinding::new("ctrl-z", TextInputUndo, context),
        KeyBinding::new("ctrl-y", TextInputRedo, context),
        KeyBinding::new("up", AddressSuggestionUp, context),
        KeyBinding::new("down", AddressSuggestionDown, context),
        KeyBinding::new("tab", AddressAcceptSuggestion, context),
    ]);
}

fn push_mac_search_input_key_bindings(bindings: &mut Vec<KeyBinding>) {
    let context = Some("ExplorerSearchInput");
    bindings.extend([
        KeyBinding::new("enter", SearchCommit, context),
        KeyBinding::new("escape", SearchCancel, context),
        KeyBinding::new("backspace", SearchBackspace, context),
        KeyBinding::new("alt-backspace", SearchBackspaceWord, context),
        KeyBinding::new("cmd-backspace", gpui::NoAction {}, context),
        KeyBinding::new("alt-cmd-backspace", gpui::NoAction {}, context),
        KeyBinding::new("delete", SearchDelete, context),
        KeyBinding::new("left", SearchLeft, context),
        KeyBinding::new("right", SearchRight, context),
        KeyBinding::new("alt-left", SearchWordLeft, context),
        KeyBinding::new("alt-right", SearchWordRight, context),
        KeyBinding::new("shift-left", SearchSelectLeft, context),
        KeyBinding::new("shift-right", SearchSelectRight, context),
        KeyBinding::new("alt-shift-left", SearchSelectWordLeft, context),
        KeyBinding::new("alt-shift-right", SearchSelectWordRight, context),
        KeyBinding::new("home", SearchHome, context),
        KeyBinding::new("end", SearchEnd, context),
        KeyBinding::new("cmd-left", SearchHome, context),
        KeyBinding::new("cmd-right", SearchEnd, context),
        KeyBinding::new("shift-home", SearchSelectHome, context),
        KeyBinding::new("shift-end", SearchSelectEnd, context),
        KeyBinding::new("cmd-shift-left", SearchSelectHome, context),
        KeyBinding::new("cmd-shift-right", SearchSelectEnd, context),
        KeyBinding::new("cmd-a", SearchSelectAll, context),
        KeyBinding::new("cmd-c", SearchCopy, context),
        KeyBinding::new("cmd-x", SearchCut, context),
        KeyBinding::new("cmd-v", SearchPaste, context),
        KeyBinding::new("cmd-z", TextInputUndo, context),
        KeyBinding::new("cmd-shift-z", TextInputRedo, context),
    ]);
}

fn push_windows_like_search_input_key_bindings(bindings: &mut Vec<KeyBinding>) {
    let context = Some("ExplorerSearchInput");
    bindings.extend([
        KeyBinding::new("enter", SearchCommit, context),
        KeyBinding::new("escape", SearchCancel, context),
        KeyBinding::new("backspace", SearchBackspace, context),
        KeyBinding::new("ctrl-backspace", SearchBackspaceWord, context),
        KeyBinding::new("delete", SearchDelete, context),
        KeyBinding::new("left", SearchLeft, context),
        KeyBinding::new("right", SearchRight, context),
        KeyBinding::new("ctrl-left", SearchWordLeft, context),
        KeyBinding::new("ctrl-right", SearchWordRight, context),
        KeyBinding::new("shift-left", SearchSelectLeft, context),
        KeyBinding::new("shift-right", SearchSelectRight, context),
        KeyBinding::new("ctrl-shift-left", SearchSelectWordLeft, context),
        KeyBinding::new("ctrl-shift-right", SearchSelectWordRight, context),
        KeyBinding::new("home", SearchHome, context),
        KeyBinding::new("end", SearchEnd, context),
        KeyBinding::new("shift-home", SearchSelectHome, context),
        KeyBinding::new("shift-end", SearchSelectEnd, context),
        KeyBinding::new("ctrl-a", SearchSelectAll, context),
        KeyBinding::new("ctrl-c", SearchCopy, context),
        KeyBinding::new("ctrl-x", SearchCut, context),
        KeyBinding::new("ctrl-v", SearchPaste, context),
        KeyBinding::new("ctrl-z", TextInputUndo, context),
        KeyBinding::new("ctrl-y", TextInputRedo, context),
    ]);
}

pub fn run() {
    #[cfg(target_os = "linux")]
    configure_linux_display_backend();

    let initial_launch_request = LaunchRequest::from_args(env::args_os());
    let single_instance_primary = match prepare_single_instance_launch(&initial_launch_request) {
        SingleInstanceLaunch::Primary(primary) => primary,
        SingleInstanceLaunch::RoutedToPrimary => return,
    };

    let app = Application::new();

    #[cfg(target_os = "macos")]
    let open_url_requests = {
        let (open_url_tx, open_url_requests) = mpsc::unbounded();
        app.on_open_urls(move |urls| {
            if open_url_tx.unbounded_send(urls).is_err() {
                eprintln!("Unable to queue macOS open URL request: receiver is unavailable");
            }
        });
        app.on_reopen(|cx| {
            handle_explorer_launch_request(cx);
            cx.activate(true);
        });
        open_url_requests
    };

    app.run(move |cx: &mut App| {
        register_embedded_fonts(cx);
        crate::http_client::initialize(cx);
        crate::debug_options::initialize(cx, env::args_os());
        crate::settings::initialize(cx);
        crate::explorer::initialize_cache_directory();
        crate::explorer::initialize_native_icon_cache(cx);
        crate::explorer::initialize_image_thumbnail_cache(cx);
        crate::explorer::initialize_folder_size_cache(cx);
        crate::explorer::initialize_file_checksum_cache(cx);
        crate::explorer::initialize_cache_cleanup(cx);
        cx.bind_keys(platform_key_bindings());

        if let Some(primary) = single_instance_primary {
            start_single_instance_request_handler(primary.requests, cx);
            cx.set_global(SingleInstanceServer {
                _guard: primary.guard,
            });
        }

        #[cfg(target_os = "macos")]
        start_open_url_request_handler(open_url_requests, cx);

        handle_initial_launch(initial_launch_request, cx);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{ConfigPlatform, ExplorerSettings, config_dir_for};
    use crate::window_state::StoredWindowMode;
    use gpui::{Keystroke, TestAppContext};
    use std::{
        fs, thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    fn has_binding<A: gpui::Action>(
        bindings: &[KeyBinding],
        action: A,
        keystroke: &str,
        context: Option<&str>,
    ) -> bool {
        let keystroke = Keystroke::parse(keystroke).expect("valid test keystroke");
        bindings.iter().any(|binding| {
            binding.action().partial_eq(&action)
                && binding
                    .predicate()
                    .as_ref()
                    .map(ToString::to_string)
                    .as_deref()
                    == context
                && matches!(binding.match_keystrokes(&[keystroke.clone()]), Some(false))
        })
    }

    fn assert_text_clipboard_and_history_bindings(
        bindings: &[KeyBinding],
        modifier: &str,
        undo_keystroke: &str,
        redo_keystroke: &str,
    ) {
        macro_rules! assert_context {
            ($context:literal, $cut:expr, $paste:expr) => {
                assert!(has_binding(
                    bindings,
                    $cut,
                    &format!("{modifier}-x"),
                    Some($context)
                ));
                assert!(has_binding(
                    bindings,
                    $paste,
                    &format!("{modifier}-v"),
                    Some($context)
                ));
                assert!(has_binding(
                    bindings,
                    TextInputUndo,
                    undo_keystroke,
                    Some($context)
                ));
                assert!(has_binding(
                    bindings,
                    TextInputRedo,
                    redo_keystroke,
                    Some($context)
                ));
            };
        }

        assert_context!("ExplorerRenameInput", RenameCut, RenamePaste);
        assert_context!("ExplorerAddressInput", AddressCut, AddressPaste);
        assert_context!("ExplorerSearchInput", SearchCut, SearchPaste);
    }

    #[test]
    fn embedded_icon_fonts_are_present() {
        assert!(!SEGOE_FLUENT_ICONS.is_empty());
        assert!(!SEGOE_MDL2_ASSETS.is_empty());
        assert!(SEGOE_FLUENT_ICONS.len() > SEGOE_MDL2_ASSETS.len());
    }

    #[test]
    fn image_viewer_arrow_bindings_navigate_sibling_images() {
        for profile in [KeyBindingProfile::Mac, KeyBindingProfile::WindowsLike] {
            let bindings = key_bindings_for_profile(profile);

            assert!(has_binding(
                &bindings,
                ImageOpenPrevious,
                "left",
                Some("ImageViewer")
            ));
            assert!(has_binding(
                &bindings,
                ImageOpenNext,
                "right",
                Some("ImageViewer")
            ));
            assert!(has_binding(
                &bindings,
                PropertiesOpenPrevious,
                "left",
                Some("PropertiesDialog")
            ));
            assert!(has_binding(
                &bindings,
                PropertiesOpenNext,
                "right",
                Some("PropertiesDialog")
            ));
            assert!(has_binding(
                &bindings,
                PropertiesOpenPrevious,
                "left",
                Some("PropertiesDialog > ImageViewer")
            ));
            assert!(has_binding(
                &bindings,
                PropertiesOpenNext,
                "right",
                Some("PropertiesDialog > ImageViewer")
            ));
            assert!(!has_binding(
                &bindings,
                ImageOpenPrevious,
                "left",
                Some("PropertiesImageDialog")
            ));
            assert!(!has_binding(
                &bindings,
                ImageOpenNext,
                "right",
                Some("PropertiesImageDialog")
            ));
            assert!(has_binding(&bindings, SelectNextTab, "ctrl-tab", None));
            assert!(has_binding(
                &bindings,
                SelectPreviousTab,
                "ctrl-shift-tab",
                None
            ));
            assert!(has_binding(&bindings, OpenSelected, "right", None));
        }

        let windows_like_bindings = key_bindings_for_profile(KeyBindingProfile::WindowsLike);
        assert!(has_binding(&windows_like_bindings, GoUp, "left", None));
    }

    #[test]
    fn mac_key_bindings_use_finder_style_shortcuts_and_requested_aliases() {
        let bindings = key_bindings_for_profile(KeyBindingProfile::Mac);

        assert!(has_binding(&bindings, EnterSelected, "enter", None));
        assert!(has_binding(&bindings, EnterSelected, "cmd-down", None));
        assert!(has_binding(&bindings, EnterSelected, "cmd-o", None));
        assert!(!has_binding(
            &bindings,
            RenameSelected,
            "enter",
            Some("Explorer")
        ));
        assert!(has_binding(
            &bindings,
            RenameSelected,
            "f2",
            Some("Explorer")
        ));

        assert!(has_binding(
            &bindings,
            OpenProperties,
            "cmd-i",
            Some("Explorer")
        ));
        assert!(has_binding(
            &bindings,
            OpenProperties,
            "alt-enter",
            Some("Explorer")
        ));
        assert!(has_binding(&bindings, GoBack, "cmd-[", None));
        assert!(has_binding(&bindings, GoBack, "alt-left", None));
        assert!(has_binding(&bindings, GoForward, "cmd-]", None));
        assert!(has_binding(&bindings, GoForward, "alt-right", None));
        assert!(has_binding(&bindings, GoUp, "cmd-up", None));
        assert!(has_binding(&bindings, GoUp, "alt-up", None));

        assert!(has_binding(&bindings, SelectAll, "cmd-a", None));
        assert!(has_binding(&bindings, CopySelected, "cmd-c", None));
        assert!(has_binding(&bindings, CutSelected, "cmd-x", None));
        assert!(has_binding(&bindings, PasteClipboard, "cmd-v", None));
        assert!(has_binding(
            &bindings,
            UndoFileOperation,
            "cmd-z",
            Some("Explorer")
        ));
        assert!(has_binding(&bindings, TrashSelected, "cmd-delete", None));
        assert!(has_binding(&bindings, TrashSelected, "cmd-backspace", None));
        assert!(has_binding(
            &bindings,
            PermanentlyDeleteSelected,
            "alt-cmd-delete",
            None
        ));
        assert!(has_binding(
            &bindings,
            AddressEdit,
            "shift-cmd-g",
            Some("Explorer")
        ));
        assert!(has_binding(
            &bindings,
            SearchEdit,
            "cmd-f",
            Some("Explorer")
        ));
        assert!(has_binding(
            &bindings,
            CreateNewFolder,
            "shift-cmd-n",
            Some("Explorer")
        ));
        assert!(has_binding(&bindings, OpenSettings, "cmd-,", None));
        assert!(has_binding(&bindings, NewWindow, "cmd-n", None));
        assert!(has_binding(&bindings, NewTab, "cmd-t", None));
        assert!(has_binding(&bindings, CloseTab, "cmd-w", None));
    }

    #[test]
    fn mac_text_input_bindings_use_command_and_option_navigation() {
        let bindings = key_bindings_for_profile(KeyBindingProfile::Mac);

        assert_text_clipboard_and_history_bindings(&bindings, "cmd", "cmd-z", "cmd-shift-z");

        assert!(has_binding(
            &bindings,
            RenameCopy,
            "cmd-c",
            Some("ExplorerRenameInput")
        ));
        assert!(has_binding(
            &bindings,
            RenameBackspaceWord,
            "alt-backspace",
            Some("ExplorerRenameInput")
        ));
        assert!(has_binding(
            &bindings,
            RenameWordLeft,
            "alt-left",
            Some("ExplorerRenameInput")
        ));
        assert!(has_binding(
            &bindings,
            RenameSelectWordRight,
            "alt-shift-right",
            Some("ExplorerRenameInput")
        ));
        assert!(has_binding(
            &bindings,
            RenameHome,
            "cmd-left",
            Some("ExplorerRenameInput")
        ));
        assert!(has_binding(
            &bindings,
            RenameSelectEnd,
            "cmd-shift-right",
            Some("ExplorerRenameInput")
        ));

        assert!(has_binding(
            &bindings,
            AddressSelectAll,
            "cmd-a",
            Some("ExplorerAddressInput")
        ));
        assert!(has_binding(
            &bindings,
            AddressWordRight,
            "alt-right",
            Some("ExplorerAddressInput")
        ));
        assert!(has_binding(
            &bindings,
            AddressSelectHome,
            "cmd-shift-left",
            Some("ExplorerAddressInput")
        ));

        assert!(has_binding(
            &bindings,
            SearchPaste,
            "cmd-v",
            Some("ExplorerSearchInput")
        ));
        assert!(has_binding(
            &bindings,
            SearchWordLeft,
            "alt-left",
            Some("ExplorerSearchInput")
        ));
        assert!(has_binding(
            &bindings,
            SearchSelectWordRight,
            "alt-shift-right",
            Some("ExplorerSearchInput")
        ));
    }

    #[test]
    fn windows_like_key_bindings_preserve_existing_shortcuts() {
        let bindings = key_bindings_for_profile(KeyBindingProfile::WindowsLike);

        assert_text_clipboard_and_history_bindings(&bindings, "ctrl", "ctrl-z", "ctrl-y");

        assert!(has_binding(&bindings, EnterSelected, "enter", None));
        assert!(has_binding(&bindings, GoUp, "left", None));
        assert!(has_binding(&bindings, GoUp, "backspace", None));
        assert!(has_binding(&bindings, GoBack, "alt-left", None));
        assert!(has_binding(&bindings, GoForward, "alt-right", None));
        assert!(has_binding(
            &bindings,
            OpenSelectedInNewTab,
            "ctrl-right",
            None
        ));
        assert!(has_binding(
            &bindings,
            EnterSelectedInNewTab,
            "ctrl-enter",
            None
        ));
        assert!(has_binding(
            &bindings,
            OpenProperties,
            "alt-enter",
            Some("Explorer")
        ));
        assert!(has_binding(&bindings, SelectAll, "ctrl-a", None));
        assert!(has_binding(&bindings, TrashSelected, "delete", None));
        assert!(has_binding(
            &bindings,
            PermanentlyDeleteSelected,
            "shift-delete",
            None
        ));
        assert!(has_binding(
            &bindings,
            AddressEdit,
            "ctrl-l",
            Some("Explorer")
        ));
        assert!(has_binding(
            &bindings,
            AddressEdit,
            "alt-d",
            Some("Explorer")
        ));
        assert!(has_binding(
            &bindings,
            SearchEdit,
            "ctrl-f",
            Some("Explorer")
        ));
        assert!(has_binding(&bindings, NewWindow, "ctrl-n", None));
        assert!(has_binding(&bindings, NewTab, "ctrl-t", None));
        assert!(has_binding(&bindings, CloseTab, "ctrl-w", None));
        assert!(has_binding(
            &bindings,
            RenameWordLeft,
            "ctrl-left",
            Some("ExplorerRenameInput")
        ));
        assert!(has_binding(
            &bindings,
            AddressSelectWordRight,
            "ctrl-shift-right",
            Some("ExplorerAddressInput")
        ));
        assert!(has_binding(
            &bindings,
            SearchBackspaceWord,
            "ctrl-backspace",
            Some("ExplorerSearchInput")
        ));

        assert!(!has_binding(&bindings, SelectAll, "cmd-a", None));
        assert!(!has_binding(&bindings, GoBack, "cmd-[", None));
        assert!(!has_binding(
            &bindings,
            RenameCopy,
            "cmd-c",
            Some("ExplorerRenameInput")
        ));
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
            .is_valid(EXPLORER_WINDOW_STATE_OPTIONS)
        );
        assert!(
            !StoredWindowState::new(
                0.0,
                0.0,
                800.0,
                MIN_WINDOW_HEIGHT - 1.0,
                StoredWindowMode::Windowed
            )
            .is_valid(EXPLORER_WINDOW_STATE_OPTIONS)
        );
        assert!(
            !StoredWindowState::new(f32::NAN, 0.0, 800.0, 600.0, StoredWindowMode::Windowed)
                .is_valid(EXPLORER_WINDOW_STATE_OPTIONS)
        );
        assert!(
            !StoredWindowState::new(0.0, f32::NAN, 800.0, 600.0, StoredWindowMode::Windowed)
                .is_valid(EXPLORER_WINDOW_STATE_OPTIONS)
        );
        assert!(
            !StoredWindowState::new(0.0, 0.0, f32::NAN, 600.0, StoredWindowMode::Windowed)
                .is_valid(EXPLORER_WINDOW_STATE_OPTIONS)
        );
        assert!(
            StoredWindowState::new(
                0.0,
                0.0,
                MIN_WINDOW_WIDTH,
                MIN_WINDOW_HEIGHT,
                StoredWindowMode::Windowed
            )
            .is_valid(EXPLORER_WINDOW_STATE_OPTIONS)
        );
    }

    #[test]
    fn window_bounds_state_preserves_windowed_and_maximized_but_skips_fullscreen() {
        let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(900.0), px(700.0)));

        assert_eq!(
            StoredWindowState::from_window_bounds(
                WindowBounds::Windowed(bounds),
                EXPLORER_WINDOW_STATE_OPTIONS,
            ),
            Some(StoredWindowState::new(
                10.0,
                20.0,
                900.0,
                700.0,
                StoredWindowMode::Windowed
            ))
        );
        assert_eq!(
            StoredWindowState::from_window_bounds(
                WindowBounds::Maximized(bounds),
                EXPLORER_WINDOW_STATE_OPTIONS,
            ),
            Some(StoredWindowState::new(
                10.0,
                20.0,
                900.0,
                700.0,
                StoredWindowMode::Maximized
            ))
        );
        assert_eq!(
            StoredWindowState::from_window_bounds(
                WindowBounds::Fullscreen(bounds),
                EXPLORER_WINDOW_STATE_OPTIONS,
            ),
            None
        );
    }

    #[test]
    fn stored_window_state_restores_bounds_that_fit_a_current_display() {
        let display = Bounds::new(point(px(0.0), px(0.0)), size(px(1920.0), px(1080.0)));
        let state = StoredWindowState::new(10.0, 20.0, 900.0, 700.0, StoredWindowMode::Windowed);
        let expected = Bounds::new(point(px(10.0), px(20.0)), size(px(900.0), px(700.0)));

        assert_eq!(
            state.to_window_bounds(&[display], EXPLORER_WINDOW_STATE_OPTIONS),
            Some(WindowBounds::Windowed(expected))
        );
    }

    #[test]
    fn stored_window_state_preserves_maximized_restore_bounds_when_safe() {
        let display = Bounds::new(point(px(0.0), px(0.0)), size(px(1920.0), px(1080.0)));
        let state = StoredWindowState::new(10.0, 20.0, 900.0, 700.0, StoredWindowMode::Maximized);
        let expected = Bounds::new(point(px(10.0), px(20.0)), size(px(900.0), px(700.0)));

        assert_eq!(
            state.to_window_bounds(&[display], EXPLORER_WINDOW_STATE_OPTIONS),
            Some(WindowBounds::Maximized(expected))
        );
    }

    #[test]
    fn stored_window_state_rejects_bounds_outside_current_displays() {
        let display = Bounds::new(point(px(0.0), px(0.0)), size(px(1920.0), px(1080.0)));

        assert_eq!(
            StoredWindowState::new(1700.0, 20.0, 900.0, 700.0, StoredWindowMode::Windowed)
                .to_window_bounds(&[display], EXPLORER_WINDOW_STATE_OPTIONS),
            None
        );
        assert_eq!(
            StoredWindowState::new(10.0, 20.0, 900.0, 700.0, StoredWindowMode::Windowed)
                .to_window_bounds(&[], EXPLORER_WINDOW_STATE_OPTIONS),
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
                PathBuf::from("profile")
                    .join(".config")
                    .join("explorer")
                    .join(WINDOW_STATE_FILE_NAME)
            )
        );
        assert_eq!(
            test_window_state_path(ConfigPlatform::Windows, &[("USERPROFILE", "profile")]),
            Some(
                PathBuf::from("profile")
                    .join(".config")
                    .join("explorer")
                    .join(WINDOW_STATE_FILE_NAME)
            )
        );
        assert_eq!(
            test_window_state_path(ConfigPlatform::Windows, &[("APPDATA", "appdata")]),
            None
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

    #[gpui::test]
    fn focus_launch_opens_window_when_none_exist(cx: &mut TestAppContext) {
        initialize_test_explorer_app(cx);
        let dir = unique_temp_dir("focus-launch-opens");
        fs::create_dir_all(&dir).unwrap();
        cx.set_global(SettingsState::for_test(settings_with_launch_behaviour(
            NewWindowBehaviour::Focus,
            dir.clone(),
        )));

        cx.update(handle_explorer_launch_request);
        cx.run_until_parked();

        assert_eq!(cx.windows().len(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[gpui::test]
    fn focus_launch_activates_existing_window_without_opening_another(cx: &mut TestAppContext) {
        initialize_test_explorer_app(cx);
        let dir = unique_temp_dir("focus-launch-existing");
        fs::create_dir_all(&dir).unwrap();
        cx.set_global(SettingsState::for_test(settings_with_launch_behaviour(
            NewWindowBehaviour::Focus,
            dir.clone(),
        )));
        cx.update(|cx| {
            open_explorer_window_at(
                dir.clone(),
                WindowBounds::Windowed(Bounds::new(
                    point(px(0.0), px(0.0)),
                    size(px(900.0), px(700.0)),
                )),
                None,
                cx,
            )
        });
        let window = cx.windows()[0];

        cx.update(handle_explorer_launch_request);
        cx.run_until_parked();

        assert_eq!(cx.windows().len(), 1);
        assert!(
            window
                .update(cx, |_, window, _| window.is_window_active())
                .expect("read active state")
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[gpui::test]
    fn open_launch_opens_another_window(cx: &mut TestAppContext) {
        initialize_test_explorer_app(cx);
        let dir = unique_temp_dir("open-launch-window");
        fs::create_dir_all(&dir).unwrap();
        cx.set_global(SettingsState::for_test(settings_with_launch_behaviour(
            NewWindowBehaviour::Open,
            dir.clone(),
        )));

        cx.update(handle_explorer_launch_request);
        cx.update(handle_explorer_launch_request);
        cx.run_until_parked();

        assert_eq!(cx.windows().len(), 2);
        for window in cx.windows() {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[gpui::test]
    fn image_launch_opens_image_window_even_when_focus_is_configured(cx: &mut TestAppContext) {
        initialize_test_explorer_app(cx);
        let dir = unique_temp_dir("image-launch-focus");
        fs::create_dir_all(&dir).unwrap();
        let image_path = dir.join("photo.png");
        cx.set_global(SettingsState::for_test(settings_with_launch_behaviour(
            NewWindowBehaviour::Focus,
            dir.clone(),
        )));
        cx.update(handle_explorer_launch_request);

        cx.update(|cx| {
            handle_launch_request(
                LaunchRequest {
                    image_path: Some(image_path),
                },
                cx,
            )
        });
        cx.run_until_parked();

        assert_eq!(cx.windows().len(), 2);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mac_open_urls_decode_image_paths_with_spaces_unicode_and_no_extension() {
        let dir = unique_temp_dir("mac-open-url-decode");
        fs::create_dir_all(&dir).unwrap();
        let named_image = dir.join("café image.png");
        let extensionless_image = dir.join("extensionless image");
        fs::write(&named_image, b"routed by extension").unwrap();
        image::DynamicImage::new_rgba8(2, 1)
            .save_with_format(&extensionless_image, image::ImageFormat::Png)
            .unwrap();

        let requests = launch_requests_from_open_urls([
            file_url(&named_image),
            file_url(&extensionless_image),
        ]);

        assert_eq!(
            requests,
            vec![
                LaunchRequest {
                    image_path: Some(named_image),
                },
                LaunchRequest {
                    image_path: Some(extensionless_image),
                },
            ]
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mac_open_urls_ignore_nonlocal_missing_directory_and_non_image_targets() {
        let dir = unique_temp_dir("mac-open-url-rejections");
        fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("missing.png");
        let directory = dir.join("folder.png");
        let text = dir.join("notes.txt");
        fs::create_dir(&directory).unwrap();
        fs::write(&text, b"not an image").unwrap();

        let requests = launch_requests_from_open_urls([
            "https://example.com/photo.png".to_owned(),
            "not a URL".to_owned(),
            file_url(&missing),
            file_url(&directory),
            file_url(&text),
        ]);

        assert!(requests.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mac_open_urls_preserve_valid_image_order_in_mixed_batches() {
        let dir = unique_temp_dir("mac-open-url-order");
        fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first.png");
        let ignored = dir.join("ignored.txt");
        let second = dir.join("second.jpg");
        fs::write(&first, b"first").unwrap();
        fs::write(&ignored, b"ignored").unwrap();
        fs::write(&second, b"second").unwrap();

        let requests = launch_requests_from_open_urls([
            file_url(&first),
            file_url(&ignored),
            file_url(&second),
        ]);

        assert_eq!(
            requests
                .into_iter()
                .filter_map(|request| request.image_path)
                .collect::<Vec<_>>(),
            vec![first, second]
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[gpui::test]
    fn mac_open_url_uses_image_launch_flow_when_focus_is_configured(cx: &mut TestAppContext) {
        initialize_test_explorer_app(cx);
        let dir = unique_temp_dir("mac-open-url-focus");
        fs::create_dir_all(&dir).unwrap();
        let image_path = dir.join("photo.png");
        fs::write(&image_path, b"routed by extension").unwrap();
        cx.set_global(SettingsState::for_test(settings_with_launch_behaviour(
            NewWindowBehaviour::Focus,
            dir.clone(),
        )));
        cx.update(handle_explorer_launch_request);

        cx.update(|cx| handle_open_urls(vec![file_url(&image_path)], cx));
        cx.run_until_parked();

        assert_eq!(cx.windows().len(), 2);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn single_instance_primary_accepts_secondary_launch_request() {
        let dir = unique_temp_dir("single-instance-primary");
        let paths = single_instance_paths(&dir);
        let mut primary = start_single_instance_primary(paths.clone()).expect("start primary");
        let request = LaunchRequest {
            image_path: Some(PathBuf::from("photo.png")),
        };

        send_launch_request_from_paths(&paths, &request).expect("send launch request");

        assert_eq!(
            receive_single_instance_request(&mut primary.requests),
            Some(request)
        );
        drop(primary);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn single_instance_secondary_routes_to_primary() {
        let dir = unique_temp_dir("single-instance-secondary");
        let initial = LaunchRequest { image_path: None };
        let mut primary = match prepare_single_instance_launch_in_dir(dir.clone(), &initial) {
            SingleInstanceLaunch::Primary(Some(primary)) => primary,
            _ => panic!("initial launch should become primary"),
        };
        let request = LaunchRequest {
            image_path: Some(PathBuf::from("photo.png")),
        };

        let secondary = prepare_single_instance_launch_in_dir(dir.clone(), &request);

        assert!(matches!(secondary, SingleInstanceLaunch::RoutedToPrimary));
        assert_eq!(
            receive_single_instance_request(&mut primary.requests),
            Some(request)
        );
        drop(primary);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_single_instance_endpoint_falls_back_to_primary() {
        let dir = unique_temp_dir("single-instance-stale");
        let paths = single_instance_paths(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(&paths.lock_path, "stale").unwrap();
        let stale_listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = stale_listener.local_addr().unwrap().port();
        drop(stale_listener);
        save_single_instance_endpoint(
            &paths.endpoint_path,
            &SingleInstanceEndpoint {
                version: SINGLE_INSTANCE_PROTOCOL_VERSION,
                port,
                token: "stale".to_owned(),
                pid: 0,
            },
        )
        .unwrap();

        let launch =
            prepare_single_instance_launch_in_dir(dir.clone(), &LaunchRequest { image_path: None });

        assert!(matches!(launch, SingleInstanceLaunch::Primary(Some(_))));
        drop(launch);
        let _ = fs::remove_dir_all(dir);
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

    fn settings_with_launch_behaviour(
        new_window_behaviour: NewWindowBehaviour,
        start: PathBuf,
    ) -> ExplorerSettings {
        ExplorerSettings {
            app: crate::settings::AppSettings {
                new_window_behaviour,
                start,
                ..crate::settings::AppSettings::default()
            },
            ..ExplorerSettings::default()
        }
    }

    fn file_url(path: &Path) -> String {
        reqwest::Url::from_file_path(path)
            .expect("absolute file path")
            .to_string()
    }

    fn receive_single_instance_request(
        requests: &mut UnboundedReceiver<LaunchRequest>,
    ) -> Option<LaunchRequest> {
        for _ in 0..100 {
            match requests.try_recv() {
                Ok(request) => return Some(request),
                Err(error) if error.is_empty() => thread::sleep(Duration::from_millis(10)),
                Err(_) => return None,
            }
        }
        None
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
