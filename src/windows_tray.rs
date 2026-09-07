use std::{
    ffi::c_void,
    mem::size_of,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc as std_mpsc,
    },
    thread,
    time::Duration,
};

use futures::{StreamExt, channel::mpsc, future::Either};
use gpui::{
    App, Bounds, ClickEvent, Context, Global, Render, SharedString, TitlebarOptions, Window,
    WindowBounds, WindowDecorations, WindowHandle, WindowOptions, div, point, prelude::*, px, rgb,
    size,
};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM},
        Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection,
            DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HGDIOBJ, SelectObject,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{LIM_SMALL, LoadIconMetric},
            HiDpi::GetDpiForWindow,
            Shell::{
                NIF_GUID, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION,
                NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
            },
            WindowsAndMessaging::{
                AppendMenuW, CREATESTRUCTW, CreatePopupMenu, CreateWindowExW, DI_NORMAL,
                DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW,
                DrawIconEx, GWLP_USERDATA, GetCursorPos, GetMessageW, GetWindowLongPtrW,
                HBMMENU_POPUP_CLOSE, HICON, HMENU, MB_ICONERROR, MB_OK, MB_SYSTEMMODAL,
                MENU_ITEM_FLAGS, MENUITEMINFOW, MF_SEPARATOR, MF_STRING, MIIM_BITMAP, MSG,
                MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
                SetForegroundWindow, SetMenuDefaultItem, SetMenuItemInfoW, SetWindowLongPtrW,
                TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WINDOW_EX_STYLE, WINDOW_STYLE,
                WM_APP, WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONUP, WM_NCCREATE, WM_NULL, WNDCLASSW,
            },
        },
    },
    core::{GUID, HSTRING, PCWSTR, w},
};

use crate::update_check::{UpdateCheckError, UpdateCheckResult, check_latest_release};
use crate::window_chrome::{
    TITLEBAR_HEIGHT, WindowDragState, render_titlebar_drag_region, render_window_controls,
};

const TRAY_CALLBACK_MESSAGE: u32 = WM_APP + 40;
const TRAY_SHUTDOWN_MESSAGE: u32 = WM_APP + 41;
const TRAY_ICON_ID: u32 = 1;
const MENU_OPEN: u32 = 1;
const MENU_SETTINGS: u32 = 2;
const MENU_CHECK_UPDATES: u32 = 3;
const MENU_EXIT: u32 = 4;
const MENU_VERSION: u32 = 5;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
const MENU_ICON_LOGICAL_SIZE: u32 = 16;
const DEFAULT_DPI: u32 = 96;
const SYNC_ICON_SVG: &[u8] = include_bytes!("../assets/icons/utility/sync.svg");
const UPDATE_TIMEOUT: Duration = Duration::from_secs(15);
const TRAY_GUID: GUID = GUID::from_u128(0x4ca420c7_651e_49a8_a881_e2d0b0bd747f);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrayEvent {
    OpenExplorer,
    About,
    OpenSettings,
    CheckForUpdates,
    Exit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TrayMenuItem {
    Command {
        id: u32,
        label: String,
        icon: Option<TrayMenuIcon>,
    },
    Separator,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrayMenuIcon {
    Explorer,
    Sync,
    Close,
}

struct OwnedMenuBitmap(HBITMAP);

impl Drop for OwnedMenuBitmap {
    fn drop(&mut self) {
        // SAFETY: this wrapper exclusively owns a bitmap created by CreateDIBSection.
        let _ = unsafe { DeleteObject(HGDIOBJ::from(self.0)) };
    }
}

struct MenuBitmapDib {
    bitmap: OwnedMenuBitmap,
    bits: *mut u8,
    byte_count: usize,
}

impl MenuBitmapDib {
    fn pixels_mut(&mut self) -> &mut [u8] {
        // SAFETY: bits points at byte_count writable bytes owned by bitmap for this value's life.
        unsafe { std::slice::from_raw_parts_mut(self.bits, self.byte_count) }
    }

    fn into_bitmap(self) -> OwnedMenuBitmap {
        self.bitmap
    }
}

struct TrayController {
    hwnd: isize,
    thread: Option<thread::JoinHandle<()>>,
}

impl Global for TrayController {}

impl TrayController {
    fn shutdown(mut self) {
        // SAFETY: hwnd belongs to the tray thread and remains valid until it handles this message.
        let _ = unsafe {
            PostMessageW(
                Some(HWND(self.hwnd as *mut c_void)),
                TRAY_SHUTDOWN_MESSAGE,
                WPARAM(0),
                LPARAM(0),
            )
        };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct AboutWindowRegistry {
    handle: Option<WindowHandle<AboutUpdateWindow>>,
}

impl Global for AboutWindowRegistry {}

pub(crate) fn initialize(cx: &mut App) -> Result<(), String> {
    let (events_tx, events_rx) = mpsc::unbounded();
    let (ready_tx, ready_rx) = std_mpsc::sync_channel(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    let tray_cancelled = cancelled.clone();
    let tray_thread = thread::Builder::new()
        .name("explorer-tray".into())
        .spawn(move || tray_thread_main(events_tx, ready_tx, tray_cancelled))
        .map_err(|error| format!("unable to start tray thread: {error}"))?;

    let hwnd = match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(hwnd)) => hwnd,
        Ok(Err(error)) => {
            let _ = tray_thread.join();
            return Err(error);
        }
        Err(error) => {
            cancelled.store(true, Ordering::SeqCst);
            return Err(format!("tray initialization did not complete: {error}"));
        }
    };

    cx.set_global(TrayController {
        hwnd,
        thread: Some(tray_thread),
    });
    cx.set_global(AboutWindowRegistry { handle: None });
    start_event_handler(events_rx, cx);
    cx.on_app_quit(|cx| {
        cx.remove_global::<TrayController>().shutdown();
        async {}
    })
    .detach();
    Ok(())
}

fn start_event_handler(mut events: mpsc::UnboundedReceiver<TrayEvent>, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Some(event) = events.next().await {
            if cx
                .update(|cx| match event {
                    TrayEvent::OpenExplorer => crate::app::open_or_focus_explorer_window(cx),
                    TrayEvent::About => open_about_window(AboutMode::About, cx),
                    TrayEvent::OpenSettings => open_settings(cx),
                    TrayEvent::CheckForUpdates => open_about_window(AboutMode::CheckForUpdates, cx),
                    TrayEvent::Exit => cx.quit(),
                })
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
}

fn open_settings(cx: &App) {
    let path = cx
        .global::<crate::settings::SettingsState>()
        .settings_path();
    if let Err(error) = open_settings_file_with(path, |path| open::that_detached(path)) {
        show_error_dialog(&error);
    }
}

fn open_settings_file_with(
    path: Option<&Path>,
    open: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    let path = path.ok_or_else(|| {
        "Could not open settings.json: settings file path is unavailable".to_owned()
    })?;
    open(path).map_err(|error| format!("Could not open {}: {error}", path.display()))
}

fn show_error_dialog(message: &str) {
    // SAFETY: MessageBoxW copies both strings before returning and has no owner window.
    let _ = unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(message),
            &HSTRING::from("Explorer"),
            MB_OK | MB_ICONERROR | MB_SYSTEMMODAL,
        )
    };
}

#[derive(Clone, Copy)]
enum AboutMode {
    About,
    CheckForUpdates,
}

#[derive(Clone)]
enum UpdatePanelState {
    Idle,
    Checking,
    Complete(UpdateCheckResult),
    Failed(UpdateCheckError),
}

struct AboutUpdateWindow {
    mode: AboutMode,
    update: UpdatePanelState,
    should_move_window: bool,
}

impl AboutUpdateWindow {
    fn new(mode: AboutMode, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            mode,
            update: UpdatePanelState::Idle,
            should_move_window: false,
        };
        if matches!(mode, AboutMode::CheckForUpdates) {
            this.begin_update_check(cx);
        }
        this
    }

    fn show_mode(&mut self, mode: AboutMode, cx: &mut Context<Self>) {
        self.mode = mode;
        if matches!(mode, AboutMode::CheckForUpdates)
            && !matches!(self.update, UpdatePanelState::Checking)
        {
            self.begin_update_check(cx);
        }
        cx.notify();
    }

    fn begin_update_check(&mut self, cx: &mut Context<Self>) {
        if matches!(self.update, UpdatePanelState::Checking) {
            return;
        }
        self.update = UpdatePanelState::Checking;
        cx.notify();
        let client = cx.http_client();
        let timeout = cx.background_executor().timer(UPDATE_TIMEOUT);
        cx.spawn(async move |this, cx| {
            let check = check_latest_release(client);
            let result = match futures::future::select(Box::pin(check), Box::pin(timeout)).await {
                Either::Left((result, _)) => result,
                Either::Right((_, _)) => Err(UpdateCheckError::Timeout),
            };
            let _ = this.update(cx, |this, cx| {
                this.update = match result {
                    Ok(result) => UpdatePanelState::Complete(result),
                    Err(error) => UpdatePanelState::Failed(error),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn render_about(&self) -> gpui::AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(div().text_size(px(24.0)).child("Explorer"))
            .child(
                div()
                    .text_color(rgb(0x555555))
                    .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
            )
            .child("A cross-platform file explorer with the Windows Explorer experience.")
            .into_any_element()
    }

    fn render_update(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let content = match &self.update {
            UpdatePanelState::Idle => div().child("Ready to check for updates."),
            UpdatePanelState::Checking => div().child("Checking for updates…"),
            UpdatePanelState::Complete(UpdateCheckResult::UpdateAvailable {
                current,
                latest,
                release_url,
            }) => {
                let release_url = release_url.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    .child(div().child("An Explorer update is available."))
                    .child(div().child(format!("Installed: {current}    Latest: {latest}")))
                    .child(
                        tray_button("open-release-page", "Open release page").on_click(
                            cx.listener(move |_, _: &ClickEvent, _, cx| cx.open_url(&release_url)),
                        ),
                    )
            }
            UpdatePanelState::Complete(UpdateCheckResult::UpToDate { current, latest }) => div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(if current > latest {
                    "This Explorer build is newer than the latest published release."
                } else {
                    "Explorer is up to date."
                })
                .child(format!("Installed: {current}    Latest: {latest}")),
            UpdatePanelState::Failed(error) => div()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child("The update check failed.")
                .child(div().text_color(rgb(0x9c1c1c)).child(error.to_string()))
                .child(tray_button("retry-update-check", "Try again").on_click(
                    cx.listener(|this, _: &ClickEvent, _, cx| this.begin_update_check(cx)),
                )),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(div().text_size(px(20.0)).child("Explorer updates"))
            .child(content)
            .into_any_element()
    }
}

impl WindowDragState for AboutUpdateWindow {
    fn set_window_drag_pending(&mut self, pending: bool) {
        self.should_move_window = pending;
    }

    fn take_window_drag_pending(&mut self) -> bool {
        let pending = self.should_move_window;
        self.should_move_window = false;
        pending
    }
}

impl Render for AboutUpdateWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.mode {
            AboutMode::About => self.render_about(),
            AboutMode::CheckForUpdates => self.render_update(cx),
        };
        let titlebar = div()
            .id("explorer-about-update-titlebar")
            .debug_selector(|| "explorer-about-update-titlebar".to_owned())
            .w_full()
            .h(px(TITLEBAR_HEIGHT))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .bg(rgb(0xe8e8e8))
            .child(render_titlebar_drag_region(
                "explorer-about-update-titlebar-drag-region",
                window.window_decorations(),
                cx,
            ))
            .children(render_window_controls(window));
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xffffff))
            .text_size(px(13.0))
            .text_color(rgb(0x202020))
            .child(titlebar)
            .child(div().flex_1().p(px(24.0)).child(content))
    }
}

fn tray_button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .min_w(px(110.0))
        .h(px(30.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(rgb(0xc7c7c7))
        .bg(rgb(0xf7f7f7))
        .hover(|style| style.bg(rgb(0xe5f3ff)))
        .active(|style| style.bg(rgb(0xcce4f7)))
        .child(label)
}

fn open_about_window(mode: AboutMode, cx: &mut App) {
    if let Some(handle) = cx.global::<AboutWindowRegistry>().handle
        && handle
            .update(cx, |view, window, cx| {
                view.show_mode(mode, cx);
                window.activate_window();
            })
            .is_ok()
    {
        cx.activate(true);
        return;
    }

    let handle = match cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(470.0), px(260.0)),
                cx,
            ))),
            window_min_size: Some(size(px(420.0), px(220.0))),
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("About Explorer")),
                appears_transparent: true,
                traffic_light_position: cfg!(target_os = "macos")
                    .then_some(point(px(12.0), px(11.0))),
                ..Default::default()
            }),
            window_decorations: Some(WindowDecorations::Server),
            app_id: Some(crate::settings::APP_ID.to_owned()),
            ..Default::default()
        },
        move |_, cx| cx.new(|cx| AboutUpdateWindow::new(mode, cx)),
    ) {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("Unable to open Explorer About window: {error}");
            return;
        }
    };
    cx.global_mut::<AboutWindowRegistry>().handle = Some(handle);
    cx.activate(true);
}

struct TrayNativeState {
    events: mpsc::UnboundedSender<TrayEvent>,
    taskbar_created_message: u32,
    icon: HICON,
    icon_added: bool,
}

fn tray_thread_main(
    events: mpsc::UnboundedSender<TrayEvent>,
    ready: std_mpsc::SyncSender<Result<isize, String>>,
    cancelled: Arc<AtomicBool>,
) {
    let result = unsafe { create_tray_window(events) };
    let (hwnd, mut state) = match result {
        Ok(value) => value,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if cancelled.load(Ordering::SeqCst) {
        // SAFETY: initialization timed out, so this thread still exclusively owns the window.
        unsafe {
            remove_icon(hwnd);
            let _ = DestroyWindow(hwnd);
            let _ = DestroyIcon(state.icon);
        }
        return;
    }
    let _ = ready.send(Ok(hwnd.0 as isize));

    let mut message = MSG::default();
    // SAFETY: this thread owns the native window and its message queue.
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            DispatchMessageW(&message);
        }
        remove_icon(hwnd);
        state.icon_added = false;
        let _ = DestroyIcon(state.icon);
    }
}

unsafe fn create_tray_window(
    events: mpsc::UnboundedSender<TrayEvent>,
) -> Result<(HWND, Box<TrayNativeState>), String> {
    let module = unsafe { GetModuleHandleW(None) }
        .map_err(|error| format!("unable to find Explorer module: {error}"))?;
    let instance = HINSTANCE(module.0);
    let class_name = wide("Explorer.Tray.CallbackWindow");
    let window_class = WNDCLASSW {
        lpfnWndProc: Some(tray_window_proc),
        hInstance: instance,
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&window_class) } == 0 {
        return Err(format!(
            "unable to register tray callback window: {}",
            std::io::Error::last_os_error()
        ));
    }

    let icon = unsafe { LoadIconMetric(Some(instance), PCWSTR(1usize as *const u16), LIM_SMALL) }
        .map_err(|error| format!("unable to load Explorer tray icon: {error}"))?;
    let taskbar_created_message = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    if taskbar_created_message == 0 {
        let _ = unsafe { DestroyIcon(icon) };
        return Err("unable to register the TaskbarCreated message".into());
    }
    let mut state = Box::new(TrayNativeState {
        events,
        taskbar_created_message,
        icon,
        icon_added: false,
    });
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            w!("Explorer tray callback"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            Some((&mut *state as *mut TrayNativeState).cast()),
        )
    }
    .map_err(|error| format!("unable to create tray callback window: {error}"))?;
    if let Err(error) = unsafe { add_icon(hwnd, state.icon) } {
        let _ = unsafe { DestroyWindow(hwnd) };
        let _ = unsafe { DestroyIcon(state.icon) };
        return Err(error);
    }
    state.icon_added = true;
    Ok((hwnd, state))
}

unsafe extern "system" fn tray_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
    }
    let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayNativeState };
    if !state.is_null() {
        let state = unsafe { &mut *state };
        if message == state.taskbar_created_message {
            state.icon_added = unsafe { add_icon(hwnd, state.icon) }.is_ok();
            return LRESULT(0);
        }
        match message {
            TRAY_CALLBACK_MESSAGE => {
                let notification = lparam.0 as u32 & 0xffff;
                match notification {
                    NIN_SELECT | NIN_KEYSELECT | WM_LBUTTONUP => {
                        let _ = state.events.unbounded_send(TrayEvent::OpenExplorer);
                    }
                    WM_CONTEXTMENU => unsafe {
                        show_context_menu(hwnd, wparam, state.icon, &state.events)
                    },
                    _ => {}
                }
                return LRESULT(0);
            }
            TRAY_SHUTDOWN_MESSAGE => {
                unsafe { remove_icon(hwnd) };
                state.icon_added = false;
                let _ = unsafe { DestroyWindow(hwnd) };
                return LRESULT(0);
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                return LRESULT(0);
            }
            _ => {}
        }
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

unsafe fn add_icon(hwnd: HWND, icon: HICON) -> Result<(), String> {
    let mut data = notify_icon_data(hwnd);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_GUID;
    data.uCallbackMessage = TRAY_CALLBACK_MESSAGE;
    data.hIcon = icon;
    copy_wide(&mut data.szTip, "Explorer");
    if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
        return Err(format!(
            "Shell_NotifyIconW(NIM_ADD) failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    if !unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) }.as_bool() {
        unsafe { remove_icon(hwnd) };
        return Err(format!(
            "Shell_NotifyIconW(NIM_SETVERSION) failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

unsafe fn remove_icon(hwnd: HWND) {
    let data = notify_icon_data(hwnd);
    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
}

fn notify_icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        guidItem: TRAY_GUID,
        ..Default::default()
    }
}

unsafe fn show_context_menu(
    hwnd: HWND,
    callback_coordinates: WPARAM,
    explorer_icon: HICON,
    events: &mpsc::UnboundedSender<TrayEvent>,
) {
    let root = match unsafe { CreatePopupMenu() } {
        Ok(menu) => menu,
        Err(_) => return,
    };
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(DEFAULT_DPI);
    let mut owned_bitmaps = Vec::new();
    for item in tray_menu_items() {
        match item {
            TrayMenuItem::Command { id, label, icon } => {
                let _ = append_menu(root, MF_STRING, id as usize, &label);
                if let Some(icon) = icon {
                    attach_menu_icon(root, id, icon, explorer_icon, dpi, &mut owned_bitmaps);
                }
            }
            TrayMenuItem::Separator => {
                let _ = append_menu(root, MF_SEPARATOR, 0, "");
            }
        }
    }
    let _ = unsafe { SetMenuDefaultItem(root, MENU_OPEN, 0) };
    let _ = unsafe { SetForegroundWindow(hwnd) };

    let mut point = callback_point(callback_coordinates);
    if point.x == -1 && point.y == -1 {
        let _ = unsafe { GetCursorPos(&mut point) };
    }
    let selected = unsafe {
        TrackPopupMenuEx(
            root,
            TPM_RIGHTBUTTON.0 | TPM_RETURNCMD.0,
            point.x,
            point.y,
            hwnd,
            None,
        )
    }
    .0 as u32;
    let _ = unsafe { PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0)) };
    let _ = unsafe { DestroyMenu(root) };
    drop(owned_bitmaps);

    if let Some(event) = tray_event_for_command(selected) {
        let _ = events.unbounded_send(event);
    }
}

fn tray_menu_items() -> Vec<TrayMenuItem> {
    vec![
        TrayMenuItem::Command {
            id: MENU_VERSION,
            label: format!("Explorer [Version {}]", env!("CARGO_PKG_VERSION")),
            icon: None,
        },
        TrayMenuItem::Separator,
        TrayMenuItem::Command {
            id: MENU_OPEN,
            label: "&Open".into(),
            icon: Some(TrayMenuIcon::Explorer),
        },
        TrayMenuItem::Separator,
        TrayMenuItem::Command {
            id: MENU_SETTINGS,
            label: "&Settings".into(),
            icon: None,
        },
        TrayMenuItem::Command {
            id: MENU_CHECK_UPDATES,
            label: "&Check for updates".into(),
            icon: Some(TrayMenuIcon::Sync),
        },
        TrayMenuItem::Separator,
        TrayMenuItem::Command {
            id: MENU_EXIT,
            label: "E&xit".into(),
            icon: Some(TrayMenuIcon::Close),
        },
    ]
}

fn attach_menu_icon(
    menu: HMENU,
    command: u32,
    icon: TrayMenuIcon,
    explorer_icon: HICON,
    dpi: u32,
    owned_bitmaps: &mut Vec<OwnedMenuBitmap>,
) {
    if icon == TrayMenuIcon::Close {
        if let Err(error) = set_menu_item_bitmap(menu, command, HBMMENU_POPUP_CLOSE) {
            eprintln!("Unable to attach the Exit tray-menu icon: {error}");
        }
        return;
    }

    let size = menu_icon_size(dpi);
    let bitmap = match icon {
        TrayMenuIcon::Explorer => create_explorer_menu_bitmap(explorer_icon, size),
        TrayMenuIcon::Sync => create_sync_menu_bitmap(size),
        TrayMenuIcon::Close => unreachable!(),
    };
    match bitmap {
        Ok(bitmap) => match set_menu_item_bitmap(menu, command, bitmap.0) {
            Ok(()) => owned_bitmaps.push(bitmap),
            Err(error) => {
                eprintln!("Unable to attach a tray-menu icon for command {command}: {error}")
            }
        },
        Err(error) => {
            eprintln!("Unable to create a tray-menu icon for command {command}: {error}")
        }
    }
}

fn menu_icon_size(dpi: u32) -> u32 {
    MENU_ICON_LOGICAL_SIZE
        .saturating_mul(dpi.max(DEFAULT_DPI))
        .saturating_add(DEFAULT_DPI / 2)
        / DEFAULT_DPI
}

fn set_menu_item_bitmap(menu: HMENU, command: u32, bitmap: HBITMAP) -> windows::core::Result<()> {
    let info = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_BITMAP,
        hbmpItem: bitmap,
        ..Default::default()
    };
    // SAFETY: menu and bitmap are valid for the duration of the popup menu.
    unsafe { SetMenuItemInfoW(menu, command, false, &info) }
}

fn create_explorer_menu_bitmap(icon: HICON, size: u32) -> Result<OwnedMenuBitmap, String> {
    let mut dib = create_argb_dib(size, size)?;
    dib.pixels_mut().fill(0);

    // SAFETY: the memory DC owns no selected resources after the original object is restored.
    let dc = unsafe { CreateCompatibleDC(None) };
    if dc.is_invalid() {
        return Err(format!(
            "CreateCompatibleDC failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let previous = unsafe { SelectObject(dc, HGDIOBJ::from(dib.bitmap.0)) };
    if previous.is_invalid() {
        let _ = unsafe { DeleteDC(dc) };
        return Err(format!(
            "SelectObject failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let draw_result =
        unsafe { DrawIconEx(dc, 0, 0, icon, size as i32, size as i32, 0, None, DI_NORMAL) };
    unsafe {
        let _ = SelectObject(dc, previous);
        let _ = DeleteDC(dc);
    }
    draw_result
        .map(|_| dib.into_bitmap())
        .map_err(|error| format!("DrawIconEx failed: {error}"))
}

fn create_sync_menu_bitmap(size: u32) -> Result<OwnedMenuBitmap, String> {
    let pixels = rasterize_sync_icon(size)?;
    let mut dib = create_argb_dib(size, size)?;
    dib.pixels_mut().copy_from_slice(&pixels);
    Ok(dib.into_bitmap())
}

fn rasterize_sync_icon(size: u32) -> Result<Vec<u8>, String> {
    let tree = usvg::Tree::from_data(SYNC_ICON_SVG, &usvg::Options::default())
        .map_err(|error| format!("unable to parse sync.svg: {error}"))?;
    let svg_size = tree.size();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)
        .ok_or_else(|| format!("unable to allocate {size}x{size} sync icon"))?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(
            size as f32 / svg_size.width(),
            size as f32 / svg_size.height(),
        ),
        &mut pixmap.as_mut(),
    );
    let mut pixels = pixmap.take();
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok(pixels)
}

fn create_argb_dib(width: u32, height: u32) -> Result<MenuBitmapDib, String> {
    let byte_count = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "menu bitmap dimensions overflowed".to_owned())?
        as usize;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: byte_count as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY: info describes a 32-bit top-down DIB; bits is initialized on success and owned by
    // the returned bitmap. The slice cannot outlive the bitmap at any call site.
    let bitmap = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }
        .map_err(|error| format!("CreateDIBSection failed: {error}"))?;
    if bits.is_null() {
        let _ = unsafe { DeleteObject(HGDIOBJ::from(bitmap)) };
        return Err("CreateDIBSection returned no pixel buffer".into());
    }
    Ok(MenuBitmapDib {
        bitmap: OwnedMenuBitmap(bitmap),
        bits: bits.cast::<u8>(),
        byte_count,
    })
}

fn tray_event_for_command(command: u32) -> Option<TrayEvent> {
    match command {
        MENU_VERSION => Some(TrayEvent::About),
        MENU_OPEN => Some(TrayEvent::OpenExplorer),
        MENU_SETTINGS => Some(TrayEvent::OpenSettings),
        MENU_CHECK_UPDATES => Some(TrayEvent::CheckForUpdates),
        MENU_EXIT => Some(TrayEvent::Exit),
        _ => None,
    }
}

fn append_menu(
    menu: HMENU,
    flags: MENU_ITEM_FLAGS,
    id: usize,
    label: &str,
) -> windows::core::Result<()> {
    let label = wide(label);
    // SAFETY: the UTF-16 buffer remains alive for the duration of AppendMenuW.
    unsafe { AppendMenuW(menu, flags, id, PCWSTR(label.as_ptr())) }
}

fn callback_point(value: WPARAM) -> POINT {
    POINT {
        x: (value.0 as u16) as i16 as i32,
        y: ((value.0 >> 16) as u16) as i16 as i32,
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn copy_wide<const N: usize>(target: &mut [u16; N], value: &str) {
    for (target, source) in target.iter_mut().zip(value.encode_utf16()) {
        *target = source;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[test]
    fn tray_menu_starts_with_version_and_has_no_windows_submenu() {
        assert_eq!(
            tray_menu_items(),
            vec![
                TrayMenuItem::Command {
                    id: MENU_VERSION,
                    label: format!("Explorer [Version {}]", env!("CARGO_PKG_VERSION")),
                    icon: None,
                },
                TrayMenuItem::Separator,
                TrayMenuItem::Command {
                    id: MENU_OPEN,
                    label: "&Open".into(),
                    icon: Some(TrayMenuIcon::Explorer),
                },
                TrayMenuItem::Separator,
                TrayMenuItem::Command {
                    id: MENU_SETTINGS,
                    label: "&Settings".into(),
                    icon: None,
                },
                TrayMenuItem::Command {
                    id: MENU_CHECK_UPDATES,
                    label: "&Check for updates".into(),
                    icon: Some(TrayMenuIcon::Sync),
                },
                TrayMenuItem::Separator,
                TrayMenuItem::Command {
                    id: MENU_EXIT,
                    label: "E&xit".into(),
                    icon: Some(TrayMenuIcon::Close),
                },
            ]
        );
    }

    #[test]
    fn menu_icon_size_scales_with_dpi() {
        assert_eq!(menu_icon_size(96), 16);
        assert_eq!(menu_icon_size(144), 24);
        assert_eq!(menu_icon_size(192), 32);
    }

    #[test]
    fn sync_icon_raster_has_opaque_and_transparent_pixels() {
        for size in [16, 24, 32] {
            let pixels = rasterize_sync_icon(size).expect("render sync icon");
            assert_eq!(pixels.len(), size as usize * size as usize * 4);
            assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] == 0));
            assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] != 0));
        }
    }

    #[test]
    fn native_menu_accepts_generated_and_predefined_bitmaps() {
        use windows::Win32::UI::WindowsAndMessaging::GetMenuItemInfoW;

        let menu = unsafe { CreatePopupMenu() }.expect("create popup menu");
        append_menu(
            menu,
            MF_STRING,
            MENU_CHECK_UPDATES as usize,
            "Check for updates",
        )
        .expect("append generated-bitmap item");
        append_menu(menu, MF_STRING, MENU_EXIT as usize, "Exit")
            .expect("append predefined-bitmap item");

        let sync = create_sync_menu_bitmap(16).expect("create sync bitmap");
        set_menu_item_bitmap(menu, MENU_CHECK_UPDATES, sync.0).expect("attach generated bitmap");
        set_menu_item_bitmap(menu, MENU_EXIT, HBMMENU_POPUP_CLOSE)
            .expect("attach predefined bitmap");

        let mut sync_info = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_BITMAP,
            ..Default::default()
        };
        unsafe { GetMenuItemInfoW(menu, MENU_CHECK_UPDATES, false, &mut sync_info) }
            .expect("read generated bitmap");
        assert_eq!(sync_info.hbmpItem, sync.0);

        let mut exit_info = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_BITMAP,
            ..Default::default()
        };
        unsafe { GetMenuItemInfoW(menu, MENU_EXIT, false, &mut exit_info) }
            .expect("read predefined bitmap");
        assert_eq!(exit_info.hbmpItem, HBMMENU_POPUP_CLOSE);

        unsafe { DestroyMenu(menu) }.expect("destroy popup menu");
        drop(sync);
    }

    #[test]
    fn version_menu_item_opens_about() {
        assert_eq!(tray_event_for_command(MENU_VERSION), Some(TrayEvent::About));
    }

    #[test]
    fn settings_menu_item_routes_to_settings() {
        assert_eq!(
            tray_event_for_command(MENU_SETTINGS),
            Some(TrayEvent::OpenSettings)
        );
    }

    #[test]
    fn settings_open_uses_the_configured_path() {
        let path = Path::new(r"C:\Users\me\AppData\Roaming\Explorer\settings.json");
        let mut opened = None;

        let result = open_settings_file_with(Some(path), |path| {
            opened = Some(path.to_path_buf());
            Ok(())
        });

        assert_eq!(result, Ok(()));
        assert_eq!(opened.as_deref(), Some(path));
    }

    #[test]
    fn settings_open_reports_unavailable_path() {
        let error = open_settings_file_with(None, |_| Ok(())).expect_err("missing path");

        assert_eq!(
            error,
            "Could not open settings.json: settings file path is unavailable"
        );
    }

    #[test]
    fn settings_open_reports_launcher_failure() {
        let path = Path::new(r"C:\Explorer\settings.json");
        let error = open_settings_file_with(Some(path), |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "access denied",
            ))
        })
        .expect_err("launcher failure");

        assert_eq!(
            error,
            r"Could not open C:\Explorer\settings.json: access denied"
        );
    }

    #[gpui::test]
    fn about_update_window_renders_custom_caption_controls(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, cx| AboutUpdateWindow::new(AboutMode::About, cx));

        cx.debug_bounds("explorer-about-update-titlebar")
            .expect("utility titlebar bounds");
        cx.debug_bounds("explorer-windows-window-controls")
            .expect("Windows caption control bounds");
        cx.debug_bounds("explorer-window-close")
            .expect("close caption button bounds");
    }

    #[gpui::test]
    fn closed_about_window_is_recreated(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(AboutWindowRegistry { handle: None }));
        cx.update(|cx| open_about_window(AboutMode::About, cx));
        let first = cx.update(|cx| {
            cx.global::<AboutWindowRegistry>()
                .handle
                .expect("first About window")
        });
        first
            .update(cx, |_, window, _| window.remove_window())
            .expect("close first About window");

        cx.update(|cx| open_about_window(AboutMode::About, cx));
        let second = cx.update(|cx| {
            cx.global::<AboutWindowRegistry>()
                .handle
                .expect("replacement About window")
        });
        assert_ne!(first, second);
    }

    #[test]
    fn version_four_callback_coordinates_are_signed() {
        let packed = ((20u32 << 16) | (-10i16 as u16 as u32)) as usize;
        assert_eq!(callback_point(WPARAM(packed)), POINT { x: -10, y: 20 });
    }
}
