#![deny(unsafe_op_in_unsafe_fn)]

use std::{
    any::Any,
    cell::{Cell, RefCell},
    collections::VecDeque,
    fs::File,
    io::Write as _,
    mem,
    num::NonZeroIsize,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::{Arc, LazyLock, Mutex, Once, atomic::{AtomicU64, Ordering}},
    time::{Duration, Instant},
};

use ::util::ResultExt;
use anyhow::{Context as _, Result};
use async_task::Runnable;
use futures::channel::oneshot::{self, Receiver};
use raw_window_handle as rwh;
use smallvec::SmallVec;
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY,
        System::{
            Com::{Marshal::*, StructuredStorage::CoGetInterfaceAndReleaseStream, *},
            DataExchange::RegisterClipboardFormatW,
            LibraryLoader::*,
            Memory::*,
            Ole::*,
            SystemServices::*,
        },
        UI::{Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
    },
    core::*,
};

use crate::*;
use super::{
    WM_GPUI_START_DEFERRED_EXTERNAL_PATHS_DROP, WM_GPUI_START_EXTERNAL_PATHS_DRAG,
    text_services::TsfCaretContext,
};

pub(crate) struct WindowsWindow(pub Rc<WindowsWindowInner>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SystemCaretGeometry {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: i32,
    pub(super) height: i32,
}

pub(super) fn system_caret_geometry(
    bounds: Bounds<Pixels>,
    scale_factor: f32,
) -> SystemCaretGeometry {
    let scale = |value: Pixels| (value.0 * scale_factor).round() as i32;
    SystemCaretGeometry {
        x: scale(bounds.origin.x),
        y: scale(bounds.origin.y),
        width: scale(px(1.0)).max(1),
        height: scale(bounds.size.height).max(1),
    }
}

struct SystemCaretBitmap(HBITMAP);

impl SystemCaretBitmap {
    fn new() -> Option<Self> {
        // A shown caret is discoverable by Windows text services. Supplying a
        // zeroed monochrome bitmap keeps that system caret visually transparent
        // while GPUI continues to render its own caret.
        let bits = 0_u32;
        let bitmap = unsafe {
            CreateBitmap(
                2,
                2,
                1,
                1,
                Some(std::ptr::from_ref(&bits).cast()),
            )
        };
        if bitmap.is_invalid() {
            log::error!(
                "failed to create transparent system caret bitmap: {}",
                windows::core::Error::from_win32()
            );
            None
        } else {
            Some(Self(bitmap))
        }
    }
}

impl Drop for SystemCaretBitmap {
    fn drop(&mut self) {
        if !unsafe { DeleteObject(self.0.into()) }.as_bool() {
            log::error!("failed to delete transparent system caret bitmap");
        }
    }
}

pub struct WindowsWindowState {
    pub origin: Point<Pixels>,
    pub logical_size: Size<Pixels>,
    pub min_size: Option<Size<Pixels>>,
    pub fullscreen_restore_bounds: Bounds<Pixels>,
    pub border_offset: WindowBorderOffset,
    pub appearance: WindowAppearance,
    pub scale_factor: f32,
    pub restore_from_minimized: Option<Box<dyn FnMut(RequestFrameOptions)>>,

    pub callbacks: Callbacks,
    pub input_handler: Option<PlatformInputHandler>,
    pub pending_surrogate: Option<u16>,
    pub last_reported_modifiers: Option<Modifiers>,
    pub last_reported_capslock: Option<Capslock>,
    pub system_key_handled: bool,
    pub system_caret_created: bool,
    pub hovered: bool,

    pub renderer: DirectXRenderer,

    pub click_state: ClickState,
    pub current_cursor: Option<HCURSOR>,
    pub nc_button_pressed: Option<u32>,

    pub display: WindowsDisplay,
    fullscreen: Option<StyleAndBounds>,
    initial_placement: Option<WindowOpenStatus>,
    hwnd: HWND,
}

pub(crate) struct WindowsWindowInner {
    hwnd: HWND,
    pub(super) this: Weak<Self>,
    drop_target_helper: IDropTargetHelper,
    pub(crate) state: RefCell<WindowsWindowState>,
    pending_external_paths_drag: RefCell<PendingExternalPathsDrag>,
    pending_deferred_external_paths_drops:
        RefCell<VecDeque<PendingDeferredWindowsExternalDrop>>,
    active_external_paths_drops: RefCell<Vec<Rc<ActiveWindowsExternalDrop>>>,
    external_paths_drop_is_accepted: Cell<bool>,
    external_paths_drop_is_deferred: Cell<bool>,
    pub(crate) system_settings: RefCell<WindowsSystemSettings>,
    pub(crate) handle: AnyWindowHandle,
    pub(crate) hide_title_bar: bool,
    system_caret_bitmap: Option<SystemCaretBitmap>,
    tsf_caret_context: Option<TsfCaretContext>,
    pub(crate) is_movable: bool,
    pub(crate) executor: ForegroundExecutor,
    pub(crate) windows_version: WindowsVersion,
    pub(crate) validation_number: usize,
    pub(crate) main_receiver: flume::Receiver<Runnable>,
    pub(crate) platform_window_handle: HWND,
}

impl WindowsWindowState {
    fn new(
        hwnd: HWND,
        directx_devices: &DirectXDevices,
        window_params: &CREATESTRUCTW,
        current_cursor: Option<HCURSOR>,
        display: WindowsDisplay,
        min_size: Option<Size<Pixels>>,
        appearance: WindowAppearance,
        disable_direct_composition: bool,
    ) -> Result<Self> {
        let scale_factor = {
            let monitor_dpi = unsafe { GetDpiForWindow(hwnd) } as f32;
            monitor_dpi / USER_DEFAULT_SCREEN_DPI as f32
        };
        let origin = logical_point(window_params.x as f32, window_params.y as f32, scale_factor);
        let logical_size = {
            let physical_size = size(
                DevicePixels(window_params.cx),
                DevicePixels(window_params.cy),
            );
            physical_size.to_pixels(scale_factor)
        };
        let fullscreen_restore_bounds = Bounds {
            origin,
            size: logical_size,
        };
        let border_offset = WindowBorderOffset::default();
        let restore_from_minimized = None;
        let renderer = DirectXRenderer::new(hwnd, directx_devices, disable_direct_composition)
            .context("Creating DirectX renderer")?;
        let callbacks = Callbacks::default();
        let input_handler = None;
        let pending_surrogate = None;
        let last_reported_modifiers = None;
        let last_reported_capslock = None;
        let system_key_handled = false;
        let system_caret_created = false;
        let hovered = false;
        let click_state = ClickState::new();
        let nc_button_pressed = None;
        let fullscreen = None;
        let initial_placement = None;

        Ok(Self {
            origin,
            logical_size,
            fullscreen_restore_bounds,
            border_offset,
            appearance,
            scale_factor,
            restore_from_minimized,
            min_size,
            callbacks,
            input_handler,
            pending_surrogate,
            last_reported_modifiers,
            last_reported_capslock,
            system_key_handled,
            system_caret_created,
            hovered,
            renderer,
            click_state,
            current_cursor,
            nc_button_pressed,
            display,
            fullscreen,
            initial_placement,
            hwnd,
        })
    }

    #[inline]
    pub(crate) fn is_fullscreen(&self) -> bool {
        self.fullscreen.is_some()
    }

    pub(crate) fn is_maximized(&self) -> bool {
        !self.is_fullscreen() && unsafe { IsZoomed(self.hwnd) }.as_bool()
    }

    fn bounds(&self) -> Bounds<Pixels> {
        Bounds {
            origin: self.origin,
            size: self.logical_size,
        }
    }

    // Calculate the bounds used for saving and whether the window is maximized.
    fn calculate_window_bounds(&self) -> (Bounds<Pixels>, bool) {
        let placement = unsafe {
            let mut placement = WINDOWPLACEMENT {
                length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                ..Default::default()
            };
            GetWindowPlacement(self.hwnd, &mut placement)
                .context("failed to get window placement")
                .log_err();
            placement
        };
        (
            calculate_client_rect(
                placement.rcNormalPosition,
                self.border_offset,
                self.scale_factor,
            ),
            placement.showCmd == SW_SHOWMAXIMIZED.0 as u32,
        )
    }

    fn window_bounds(&self) -> WindowBounds {
        let (bounds, maximized) = self.calculate_window_bounds();

        if self.is_fullscreen() {
            WindowBounds::Fullscreen(self.fullscreen_restore_bounds)
        } else if maximized {
            WindowBounds::Maximized(bounds)
        } else {
            WindowBounds::Windowed(bounds)
        }
    }

    /// get the logical size of the app's drawable area.
    ///
    /// Currently, GPUI uses the logical size of the app to handle mouse interactions (such as
    /// whether the mouse collides with other elements of GPUI).
    fn content_size(&self) -> Size<Pixels> {
        self.logical_size
    }
}

impl WindowsWindowInner {
    pub(super) fn handle_start_external_paths_drag_msg(&self) -> Option<isize> {
        let Some(paths) = self.pending_external_paths_drag.borrow_mut().take() else {
            log::error!("received external paths drag message without a pending drag");
            return Some(0);
        };

        // SHDoDragDrop runs a nested Windows message loop. The queued paths must
        // be taken before entering it so no RefCell borrow survives re-entrancy.
        let result = windows_external_drag_completion(start_windows_external_paths_drag(
            self.hwnd, paths,
        ));

        let Some(mut callback) = self.state.borrow_mut().callbacks.input.take() else {
            log::error!("unable to report external paths drag completion without an input callback");
            return Some(0);
        };
        let callback_result = catch_unwind(AssertUnwindSafe(|| {
            callback(PlatformInput::ExternalPathsDragFinished(result));
        }));
        self.state.borrow_mut().callbacks.input = Some(callback);
        if let Err(payload) = callback_result {
            resume_unwind(payload);
        }

        Some(0)
    }

    pub(super) fn handle_start_deferred_external_paths_drop_msg(&self) -> Option<isize> {
        let Some(pending) = self
            .pending_deferred_external_paths_drops
            .borrow_mut()
            .pop_front()
        else {
            log::error!("received deferred external drop message without a pending operation");
            return Some(0);
        };
        start_pending_deferred_windows_external_drop(pending);
        Some(0)
    }

    pub(super) fn cancel_pending_deferred_external_paths_drops(&self) {
        let pending = self
            .pending_deferred_external_paths_drops
            .borrow_mut()
            .drain(..)
            .collect::<Vec<_>>();
        for pending in pending {
            pending.cancel(E_ABORT);
        }
    }

    pub(super) fn destroy_system_caret(&self) {
        if let Some(context) = self.tsf_caret_context.as_ref() {
            context.clear_caret();
        }
        let had_system_caret = std::mem::take(
            &mut self.state.borrow_mut().system_caret_created,
        );
        if had_system_caret
            && let Err(error) = unsafe { DestroyCaret() }
        {
            log::error!("failed to destroy system caret: {error}");
        }
    }

    fn new(context: &mut WindowCreateContext, hwnd: HWND, cs: &CREATESTRUCTW) -> Result<Rc<Self>> {
        let system_caret_bitmap = SystemCaretBitmap::new();
        let tsf_caret_context = TsfCaretContext::new(hwnd).log_err();
        let state = RefCell::new(WindowsWindowState::new(
            hwnd,
            &context.directx_devices,
            cs,
            context.current_cursor,
            context.display,
            context.min_size,
            context.appearance,
            context.disable_direct_composition,
        )?);

        Ok(Rc::new_cyclic(|this| Self {
            hwnd,
            this: this.clone(),
            drop_target_helper: context.drop_target_helper.clone(),
            state,
            pending_external_paths_drag: RefCell::new(PendingExternalPathsDrag::default()),
            pending_deferred_external_paths_drops: RefCell::new(VecDeque::new()),
            active_external_paths_drops: RefCell::new(Vec::new()),
            external_paths_drop_is_accepted: Cell::new(false),
            external_paths_drop_is_deferred: Cell::new(false),
            handle: context.handle,
            hide_title_bar: context.hide_title_bar,
            system_caret_bitmap,
            tsf_caret_context,
            is_movable: context.is_movable,
            executor: context.executor.clone(),
            windows_version: context.windows_version,
            validation_number: context.validation_number,
            main_receiver: context.main_receiver.clone(),
            platform_window_handle: context.platform_window_handle,
            system_settings: RefCell::new(WindowsSystemSettings::new(context.display)),
        }))
    }

    fn toggle_fullscreen(&self) {
        let Some(this) = self.this.upgrade() else {
            log::error!("Unable to toggle fullscreen: window has been dropped");
            return;
        };
        self.executor
            .spawn(async move {
                let mut lock = this.state.borrow_mut();
                let StyleAndBounds {
                    style,
                    x,
                    y,
                    cx,
                    cy,
                } = if let Some(state) = lock.fullscreen.take() {
                    state
                } else {
                    let (window_bounds, _) = lock.calculate_window_bounds();
                    lock.fullscreen_restore_bounds = window_bounds;
                    let style = WINDOW_STYLE(unsafe { get_window_long(this.hwnd, GWL_STYLE) } as _);
                    let mut rc = RECT::default();
                    unsafe { GetWindowRect(this.hwnd, &mut rc) }
                        .context("failed to get window rect")
                        .log_err();
                    let _ = lock.fullscreen.insert(StyleAndBounds {
                        style,
                        x: rc.left,
                        y: rc.top,
                        cx: rc.right - rc.left,
                        cy: rc.bottom - rc.top,
                    });
                    let style = style
                        & !(WS_THICKFRAME
                            | WS_SYSMENU
                            | WS_MAXIMIZEBOX
                            | WS_MINIMIZEBOX
                            | WS_CAPTION);
                    let physical_bounds = lock.display.physical_bounds();
                    StyleAndBounds {
                        style,
                        x: physical_bounds.left().0,
                        y: physical_bounds.top().0,
                        cx: physical_bounds.size.width.0,
                        cy: physical_bounds.size.height.0,
                    }
                };
                drop(lock);
                unsafe { set_window_long(this.hwnd, GWL_STYLE, style.0 as isize) };
                unsafe {
                    SetWindowPos(
                        this.hwnd,
                        None,
                        x,
                        y,
                        cx,
                        cy,
                        SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOZORDER,
                    )
                }
                .log_err();
            })
            .detach();
    }

    fn set_window_placement(&self) -> Result<()> {
        let Some(open_status) = self.state.borrow_mut().initial_placement.take() else {
            return Ok(());
        };
        match open_status.state {
            WindowOpenState::Maximized => unsafe {
                SetWindowPlacement(self.hwnd, &open_status.placement)
                    .context("failed to set window placement")?;
                ShowWindowAsync(self.hwnd, SW_MAXIMIZE).ok()?;
            },
            WindowOpenState::Fullscreen => {
                unsafe {
                    SetWindowPlacement(self.hwnd, &open_status.placement)
                        .context("failed to set window placement")?
                };
                self.toggle_fullscreen();
            }
            WindowOpenState::Windowed => unsafe {
                SetWindowPlacement(self.hwnd, &open_status.placement)
                    .context("failed to set window placement")?;
            },
        }
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct Callbacks {
    pub(crate) request_frame: Option<Box<dyn FnMut(RequestFrameOptions)>>,
    pub(crate) input: Option<Box<dyn FnMut(crate::PlatformInput) -> DispatchEventResult>>,
    pub(crate) active_status_change: Option<Box<dyn FnMut(bool)>>,
    pub(crate) hovered_status_change: Option<Box<dyn FnMut(bool)>>,
    pub(crate) resize: Option<Box<dyn FnMut(Size<Pixels>, f32)>>,
    pub(crate) moved: Option<Box<dyn FnMut()>>,
    pub(crate) should_close: Option<Box<dyn FnMut() -> bool>>,
    pub(crate) close: Option<Box<dyn FnOnce()>>,
    pub(crate) hit_test_window_control: Option<Box<dyn FnMut() -> Option<WindowControlArea>>>,
    pub(crate) appearance_changed: Option<Box<dyn FnMut()>>,
}

struct WindowCreateContext {
    inner: Option<Result<Rc<WindowsWindowInner>>>,
    handle: AnyWindowHandle,
    hide_title_bar: bool,
    display: WindowsDisplay,
    is_movable: bool,
    min_size: Option<Size<Pixels>>,
    executor: ForegroundExecutor,
    current_cursor: Option<HCURSOR>,
    windows_version: WindowsVersion,
    drop_target_helper: IDropTargetHelper,
    validation_number: usize,
    main_receiver: flume::Receiver<Runnable>,
    platform_window_handle: HWND,
    appearance: WindowAppearance,
    disable_direct_composition: bool,
    directx_devices: DirectXDevices,
}

impl WindowsWindow {
    pub(crate) fn new(
        handle: AnyWindowHandle,
        params: WindowParams,
        creation_info: WindowCreationInfo,
    ) -> Result<Self> {
        let WindowCreationInfo {
            icon,
            executor,
            current_cursor,
            windows_version,
            drop_target_helper,
            validation_number,
            main_receiver,
            platform_window_handle,
            disable_direct_composition,
            directx_devices,
        } = creation_info;
        register_window_class(icon);
        let hide_title_bar = params
            .titlebar
            .as_ref()
            .map(|titlebar| titlebar.appears_transparent)
            .unwrap_or(true);
        let window_name = HSTRING::from(
            params
                .titlebar
                .as_ref()
                .and_then(|titlebar| titlebar.title.as_ref())
                .map(|title| title.as_ref())
                .unwrap_or(""),
        );

        let (mut dwexstyle, dwstyle) = if params.kind == WindowKind::PopUp {
            (WS_EX_TOOLWINDOW, WINDOW_STYLE(0x0))
        } else {
            let mut dwstyle = WS_SYSMENU;

            if params.is_resizable {
                dwstyle |= WS_THICKFRAME | WS_MAXIMIZEBOX;
            }

            if params.is_minimizable {
                dwstyle |= WS_MINIMIZEBOX;
            }

            (WS_EX_APPWINDOW, dwstyle)
        };
        if !disable_direct_composition {
            dwexstyle |= WS_EX_NOREDIRECTIONBITMAP;
        }

        let hinstance = get_module_handle();
        let display = if let Some(display_id) = params.display_id {
            // if we obtain a display_id, then this ID must be valid.
            WindowsDisplay::new(display_id).unwrap()
        } else {
            WindowsDisplay::primary_monitor().unwrap()
        };
        let appearance = system_appearance().unwrap_or_default();
        let mut context = WindowCreateContext {
            inner: None,
            handle,
            hide_title_bar,
            display,
            is_movable: params.is_movable,
            min_size: params.window_min_size,
            executor,
            current_cursor,
            windows_version,
            drop_target_helper,
            validation_number,
            main_receiver,
            platform_window_handle,
            appearance,
            disable_direct_composition,
            directx_devices,
        };
        let creation_result = unsafe {
            CreateWindowExW(
                dwexstyle,
                WINDOW_CLASS_NAME,
                &window_name,
                dwstyle,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                Some(hinstance.into()),
                Some(&context as *const _ as *const _),
            )
        };

        // Failure to create a `WindowsWindowState` can cause window creation to fail,
        // so check the inner result first.
        let this = context.inner.take().unwrap()?;
        let hwnd = creation_result?;

        register_drag_drop(&this)?;
        configure_dwm_dark_mode(hwnd, appearance);
        this.state.borrow_mut().border_offset.update(hwnd)?;
        let placement = retrieve_window_placement(
            hwnd,
            display,
            params.bounds,
            this.state.borrow().scale_factor,
            this.state.borrow().border_offset,
        )?;
        if params.show {
            unsafe { SetWindowPlacement(hwnd, &placement)? };
        } else {
            this.state.borrow_mut().initial_placement = Some(WindowOpenStatus {
                placement,
                state: WindowOpenState::Windowed,
            });
        }

        let window = Self(this);
        if params.show && params.focus {
            window.activate();
        }

        Ok(window)
    }
}

impl rwh::HasWindowHandle for WindowsWindow {
    fn window_handle(&self) -> std::result::Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        let raw = rwh::Win32WindowHandle::new(unsafe {
            NonZeroIsize::new_unchecked(self.0.hwnd.0 as isize)
        })
        .into();
        Ok(unsafe { rwh::WindowHandle::borrow_raw(raw) })
    }
}

// todo(windows)
impl rwh::HasDisplayHandle for WindowsWindow {
    fn display_handle(&self) -> std::result::Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        unimplemented!()
    }
}

impl Drop for WindowsWindow {
    fn drop(&mut self) {
        // clone this `Rc` to prevent early release of the pointer
        let this = self.0.clone();
        self.0
            .executor
            .spawn(async move {
                let handle = this.hwnd;
                unsafe {
                    RevokeDragDrop(handle).log_err();
                    DestroyWindow(handle).log_err();
                }
            })
            .detach();
    }
}

impl PlatformWindow for WindowsWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        self.0.state.borrow().bounds()
    }

    fn is_maximized(&self) -> bool {
        self.0.state.borrow().is_maximized()
    }

    fn window_bounds(&self) -> WindowBounds {
        self.0.state.borrow().window_bounds()
    }

    /// get the logical size of the app's drawable area.
    ///
    /// Currently, GPUI uses the logical size of the app to handle mouse interactions (such as
    /// whether the mouse collides with other elements of GPUI).
    fn content_size(&self) -> Size<Pixels> {
        self.0.state.borrow().content_size()
    }

    fn resize(&mut self, size: Size<Pixels>) {
        let hwnd = self.0.hwnd;
        let bounds =
            crate::bounds(self.bounds().origin, size).to_device_pixels(self.scale_factor());
        let rect = calculate_window_rect(bounds, self.0.state.borrow().border_offset);

        self.0
            .executor
            .spawn(async move {
                unsafe {
                    SetWindowPos(
                        hwnd,
                        None,
                        bounds.origin.x.0,
                        bounds.origin.y.0,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        SWP_NOMOVE,
                    )
                    .context("unable to set window content size")
                    .log_err();
                }
            })
            .detach();
    }

    fn scale_factor(&self) -> f32 {
        self.0.state.borrow().scale_factor
    }

    fn appearance(&self) -> WindowAppearance {
        self.0.state.borrow().appearance
    }

    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        Some(Rc::new(self.0.state.borrow().display))
    }

    fn mouse_position(&self) -> Point<Pixels> {
        let scale_factor = self.scale_factor();
        let point = unsafe {
            let mut point: POINT = std::mem::zeroed();
            GetCursorPos(&mut point)
                .context("unable to get cursor position")
                .log_err();
            ScreenToClient(self.0.hwnd, &mut point).ok().log_err();
            point
        };
        logical_point(point.x as f32, point.y as f32, scale_factor)
    }

    fn modifiers(&self) -> Modifiers {
        current_modifiers()
    }

    fn capslock(&self) -> Capslock {
        current_capslock()
    }

    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        self.0.state.borrow_mut().input_handler = Some(input_handler);
    }

    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.0.state.borrow_mut().input_handler.take()
    }

    fn prompt(
        &self,
        level: PromptLevel,
        msg: &str,
        detail: Option<&str>,
        answers: &[PromptButton],
    ) -> Option<Receiver<usize>> {
        let (done_tx, done_rx) = oneshot::channel();
        let msg = msg.to_string();
        let detail_string = detail.map(|detail| detail.to_string());
        let handle = self.0.hwnd;
        let answers = answers.to_vec();
        self.0
            .executor
            .spawn(async move {
                unsafe {
                    let mut config = TASKDIALOGCONFIG::default();
                    config.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as _;
                    config.hwndParent = handle;
                    let title;
                    let main_icon;
                    match level {
                        crate::PromptLevel::Info => {
                            title = windows::core::w!("Info");
                            main_icon = TD_INFORMATION_ICON;
                        }
                        crate::PromptLevel::Warning => {
                            title = windows::core::w!("Warning");
                            main_icon = TD_WARNING_ICON;
                        }
                        crate::PromptLevel::Critical => {
                            title = windows::core::w!("Critical");
                            main_icon = TD_ERROR_ICON;
                        }
                    };
                    config.pszWindowTitle = title;
                    config.Anonymous1.pszMainIcon = main_icon;
                    let instruction = HSTRING::from(msg);
                    config.pszMainInstruction = PCWSTR::from_raw(instruction.as_ptr());
                    let hints_encoded;
                    if let Some(ref hints) = detail_string {
                        hints_encoded = HSTRING::from(hints);
                        config.pszContent = PCWSTR::from_raw(hints_encoded.as_ptr());
                    };
                    let mut button_id_map = Vec::with_capacity(answers.len());
                    let mut buttons = Vec::new();
                    let mut btn_encoded = Vec::new();
                    for (index, btn) in answers.iter().enumerate() {
                        let encoded = HSTRING::from(btn.label().as_ref());
                        let button_id = match btn {
                            PromptButton::Ok(_) => IDOK.0,
                            PromptButton::Cancel(_) => IDCANCEL.0,
                            // the first few low integer values are reserved for known buttons
                            // so for simplicity we just go backwards from -1
                            PromptButton::Other(_) => -(index as i32) - 1,
                        };
                        button_id_map.push(button_id);
                        buttons.push(TASKDIALOG_BUTTON {
                            nButtonID: button_id,
                            pszButtonText: PCWSTR::from_raw(encoded.as_ptr()),
                        });
                        btn_encoded.push(encoded);
                    }
                    config.cButtons = buttons.len() as _;
                    config.pButtons = buttons.as_ptr();

                    config.pfCallback = None;
                    let mut res = std::mem::zeroed();
                    let _ = TaskDialogIndirect(&config, Some(&mut res), None, None)
                        .context("unable to create task dialog")
                        .log_err();

                    if let Some(clicked) =
                        button_id_map.iter().position(|&button_id| button_id == res)
                    {
                        let _ = done_tx.send(clicked);
                    }
                }
            })
            .detach();

        Some(done_rx)
    }

    fn activate(&self) {
        let hwnd = self.0.hwnd;
        let this = self.0.clone();
        self.0
            .executor
            .spawn(async move {
                this.set_window_placement().log_err();

                unsafe {
                    // If the window is minimized, restore it.
                    if IsIconic(hwnd).as_bool() {
                        ShowWindowAsync(hwnd, SW_RESTORE).ok().log_err();
                    }

                    SetActiveWindow(hwnd).log_err();
                    SetFocus(Some(hwnd)).log_err();
                }

                // premium ragebait by windows, this is needed because the window
                // must have received an input event to be able to set itself to foreground
                // so let's just simulate user input as that seems to be the most reliable way
                // some more info: https://gist.github.com/Aetopia/1581b40f00cc0cadc93a0e8ccb65dc8c
                // bonus: this bug also doesn't manifest if you have vs attached to the process
                let inputs = [
                    INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: VK_MENU,
                                dwFlags: KEYBD_EVENT_FLAGS(0),
                                ..Default::default()
                            },
                        },
                    },
                    INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: VK_MENU,
                                dwFlags: KEYEVENTF_KEYUP,
                                ..Default::default()
                            },
                        },
                    },
                ];
                unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };

                // todo(windows)
                // crate `windows 0.56` reports true as Err
                unsafe { SetForegroundWindow(hwnd).as_bool() };
            })
            .detach();
    }

    fn is_active(&self) -> bool {
        self.0.hwnd == unsafe { GetActiveWindow() }
    }

    fn is_hovered(&self) -> bool {
        self.0.state.borrow().hovered
    }

    fn set_title(&mut self, title: &str) {
        unsafe { SetWindowTextW(self.0.hwnd, &HSTRING::from(title)) }
            .inspect_err(|e| log::error!("Set title failed: {e}"))
            .ok();
    }

    fn start_window_move(&self) {
        if !self.0.is_movable || self.0.state.borrow().is_fullscreen() {
            return;
        }

        unsafe {
            ReleaseCapture().log_err();
            SendMessageW(
                self.0.hwnd,
                WM_SYSCOMMAND,
                Some(WPARAM((SC_MOVE | HTCAPTION) as usize)),
                Some(LPARAM(0)),
            );
        }
    }

    fn set_background_appearance(&self, background_appearance: WindowBackgroundAppearance) {
        let hwnd = self.0.hwnd;

        match background_appearance {
            WindowBackgroundAppearance::Opaque => {
                // ACCENT_DISABLED
                set_window_composition_attribute(hwnd, None, 0);
            }
            WindowBackgroundAppearance::Transparent => {
                // Use ACCENT_ENABLE_TRANSPARENTGRADIENT for transparent background
                set_window_composition_attribute(hwnd, None, 2);
            }
            WindowBackgroundAppearance::Blurred => {
                // Enable acrylic blur
                // ACCENT_ENABLE_ACRYLICBLURBEHIND
                set_window_composition_attribute(hwnd, Some((0, 0, 0, 0)), 4);
            }
        }
    }

    fn minimize(&self) {
        unsafe { ShowWindowAsync(self.0.hwnd, SW_MINIMIZE).ok().log_err() };
    }

    fn zoom(&self) {
        unsafe {
            if IsWindowVisible(self.0.hwnd).as_bool() {
                ShowWindowAsync(self.0.hwnd, SW_MAXIMIZE).ok().log_err();
            } else if let Some(status) = self.0.state.borrow_mut().initial_placement.as_mut() {
                status.state = WindowOpenState::Maximized;
            }
        }
    }

    fn toggle_fullscreen(&self) {
        if unsafe { IsWindowVisible(self.0.hwnd).as_bool() } {
            self.0.toggle_fullscreen();
        } else if let Some(status) = self.0.state.borrow_mut().initial_placement.as_mut() {
            status.state = WindowOpenState::Fullscreen;
        }
    }

    fn is_fullscreen(&self) -> bool {
        self.0.state.borrow().is_fullscreen()
    }

    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.0.state.borrow_mut().callbacks.request_frame = Some(callback);
    }

    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {
        self.0.state.borrow_mut().callbacks.input = Some(callback);
    }

    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.state.borrow_mut().callbacks.active_status_change = Some(callback);
    }

    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.state.borrow_mut().callbacks.hovered_status_change = Some(callback);
    }

    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.0.state.borrow_mut().callbacks.resize = Some(callback);
    }

    fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.0.state.borrow_mut().callbacks.moved = Some(callback);
    }

    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.0.state.borrow_mut().callbacks.should_close = Some(callback);
    }

    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.0.state.borrow_mut().callbacks.close = Some(callback);
    }

    fn on_hit_test_window_control(&self, callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
        self.0.state.borrow_mut().callbacks.hit_test_window_control = Some(callback);
    }

    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.0.state.borrow_mut().callbacks.appearance_changed = Some(callback);
    }

    fn draw(&self, scene: &Scene) {
        self.0.state.borrow_mut().renderer.draw(scene).log_err();
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.0.state.borrow().renderer.sprite_atlas()
    }

    fn get_raw_handle(&self) -> HWND {
        self.0.hwnd
    }

    fn complete_external_paths_drop(&self, effect: u32) -> bool {
        complete_active_windows_external_drop(&self.0.active_external_paths_drops, effect)
    }

    fn complete_pending_windows_drop(
        &self,
        destination: &Path,
    ) -> oneshot::Receiver<std::result::Result<(), String>> {
        complete_active_deferred_windows_external_drop(
            &self.0.active_external_paths_drops,
            &self.0.pending_deferred_external_paths_drops,
            destination,
            self.0.hwnd,
        )
    }

    fn start_external_paths_drag(&self, paths: ExternalPaths) -> ExternalPathsDragStartResult {
        if paths.paths().is_empty() {
            return ExternalPathsDragStartResult::Failed;
        }

        let Ok(mut pending_drag) = self.0.pending_external_paths_drag.try_borrow_mut() else {
            log::error!("unable to queue external paths drag while the queue is borrowed");
            return ExternalPathsDragStartResult::Failed;
        };
        if !pending_drag.queue(paths) {
            log::error!("unable to queue external paths drag while another drag is pending");
            return ExternalPathsDragStartResult::Failed;
        }

        if let Err(error) = unsafe {
            PostMessageW(
                Some(self.0.hwnd),
                WM_GPUI_START_EXTERNAL_PATHS_DRAG,
                WPARAM(0),
                LPARAM(0),
            )
        } {
            pending_drag.cancel();
            log::error!("unable to post external paths drag message: {error}");
            return ExternalPathsDragStartResult::Failed;
        }

        ExternalPathsDragStartResult::Pending
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        self.0.state.borrow().renderer.gpu_specs().log_err()
    }

    fn update_ime_position(&self, _bounds: Bounds<Pixels>) {
        // There is no such thing on Windows.
    }

    fn update_system_caret(&self, bounds: Option<Bounds<Pixels>>) {
        let Some(bounds) = bounds.filter(|_| unsafe { GetFocus() } == self.0.hwnd) else {
            self.0.destroy_system_caret();
            return;
        };

        let scale_factor = self.0.state.borrow().scale_factor;
        let geometry = system_caret_geometry(bounds, scale_factor);
        let caret_created = self.0.state.borrow().system_caret_created;

        if !caret_created {
            let Some(bitmap) = self.0.system_caret_bitmap.as_ref() else {
                return;
            };
            if let Err(error) = unsafe { CreateCaret(self.0.hwnd, Some(bitmap.0), 0, 0) } {
                log::error!("failed to create system caret: {error}");
                return;
            }
            self.0.state.borrow_mut().system_caret_created = true;
        }

        if let Err(error) = unsafe { SetCaretPos(geometry.x, geometry.y) } {
            log::error!("failed to position system caret: {error}");
            self.0.destroy_system_caret();
            return;
        }

        if let Some(context) = self.0.tsf_caret_context.as_ref() {
            context.set_caret(geometry);
        }

        // ShowCaret is cumulative, so call it exactly once for each successful
        // CreateCaret. The bitmap above makes the shown caret transparent.
        if !caret_created
            && let Err(error) = unsafe { ShowCaret(Some(self.0.hwnd)) }
        {
            log::error!("failed to show transparent system caret: {error}");
            self.0.destroy_system_caret();
        }
    }
}

static PREFERRED_DROPEFFECT_FORMAT: LazyLock<u16> =
    LazyLock::new(|| register_shell_clipboard_format(CFSTR_PREFERREDDROPEFFECT));
static PERFORMED_DROPEFFECT_FORMAT: LazyLock<u16> =
    LazyLock::new(|| register_shell_clipboard_format(CFSTR_PERFORMEDDROPEFFECT));
static LOGICAL_PERFORMED_DROPEFFECT_FORMAT: LazyLock<u16> =
    LazyLock::new(|| register_shell_clipboard_format(CFSTR_LOGICALPERFORMEDDROPEFFECT));
static FILE_DESCRIPTOR_W_FORMAT: LazyLock<u16> =
    LazyLock::new(|| register_shell_clipboard_format(CFSTR_FILEDESCRIPTORW));
static FILE_CONTENTS_FORMAT: LazyLock<u16> =
    LazyLock::new(|| register_shell_clipboard_format(CFSTR_FILECONTENTS));
#[cfg(test)]
static TEST_SHELL_ID_LIST_FORMAT: LazyLock<u16> =
    LazyLock::new(|| register_shell_clipboard_format(CFSTR_SHELLIDLIST));

fn register_shell_clipboard_format(format: PCWSTR) -> u16 {
    let format = unsafe { RegisterClipboardFormatW(format) };
    if format == 0 {
        panic!(
            "Error when registering shell clipboard format: {}",
            std::io::Error::last_os_error()
        );
    }
    format as u16
}

#[implement(IDropSource)]
struct WindowsFileDragSource;

#[allow(non_snake_case)]
impl IDropSource_Impl for WindowsFileDragSource_Impl {
    fn QueryContinueDrag(
        &self,
        fescapepressed: BOOL,
        grfkeystate: MODIFIERKEYS_FLAGS,
    ) -> windows::core::HRESULT {
        if fescapepressed.as_bool() {
            return DRAGDROP_S_CANCEL;
        }

        if !grfkeystate.contains(MK_LBUTTON) {
            return DRAGDROP_S_DROP;
        }

        S_OK
    }

    fn GiveFeedback(&self, _: DROPEFFECT) -> windows::core::HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

#[cfg_attr(test, implement(IDataObject, IDataObjectAsyncCapability))]
#[cfg_attr(not(test), implement(IDataObject))]
struct WindowsFileDataObject {
    paths: Vec<PathBuf>,
    preferred_effect: DROPEFFECT,
    performed_effect: Rc<Cell<DROPEFFECT>>,
    logical_performed_effect: Rc<Cell<DROPEFFECT>>,
    #[cfg(test)]
    test_offer_hdrop: bool,
    #[cfg(test)]
    test_query_hdrop: bool,
    #[cfg(test)]
    test_shell_id_list: Option<Vec<u8>>,
    #[cfg(test)]
    test_fail_shell_get_data: bool,
    #[cfg(test)]
    test_fail_hdrop_get_data: bool,
    #[cfg(test)]
    test_hdrop_failures_remaining: Option<Rc<Cell<usize>>>,
    #[cfg(test)]
    test_async_mode: bool,
    #[cfg(test)]
    test_start_error: Option<HRESULT>,
    #[cfg(test)]
    test_call_order: Option<Rc<RefCell<Vec<&'static str>>>>,
    #[cfg(test)]
    test_start_count: Option<Rc<Cell<usize>>>,
    #[cfg(test)]
    test_end_events: Option<Rc<RefCell<Vec<(HRESULT, u32)>>>>,
    #[cfg(test)]
    test_virtual_files: Option<Vec<(String, Vec<u8>, u8)>>,
    #[cfg(test)]
    test_chromium_virtual_descriptor: bool,
    #[cfg(test)]
    test_virtual_directory: bool,
    #[cfg(test)]
    test_malformed_virtual_descriptors: bool,
}

#[allow(non_snake_case)]
impl IDataObject_Impl for WindowsFileDataObject_Impl {
    fn GetData(&self, pformatetcin: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        if self.QueryGetData(pformatetcin) != S_OK {
            return Err(DV_E_FORMATETC.into());
        }

        let requested = unsafe { pformatetcin.as_ref() }
            .ok_or_else(|| windows::core::Error::from(DV_E_FORMATETC))?;
        let format = requested.cfFormat;
        #[cfg(test)]
        if format == CF_HDROP.0 {
            if let Some(call_order) = &self.test_call_order {
                call_order.as_ref().borrow_mut().push("get_data");
            }
            if let Some(failures_remaining) = &self.test_hdrop_failures_remaining
                && failures_remaining.get() > 0
            {
                failures_remaining.set(failures_remaining.get() - 1);
                return Err(DV_E_FORMATETC.into());
            }
            if self.test_fail_hdrop_get_data {
                return Err(DV_E_FORMATETC.into());
            }
        }
        #[cfg(test)]
        if format == *FILE_DESCRIPTOR_W_FORMAT {
            let files = self.test_virtual_files.as_deref().ok_or(DV_E_FORMATETC)?;
            let payload = build_virtual_descriptor_payload(
                files,
                self.test_chromium_virtual_descriptor,
                self.test_virtual_directory,
                self.test_malformed_virtual_descriptors,
            );
            return Ok(STGMEDIUM {
                tymed: TYMED_HGLOBAL.0 as u32,
                u: STGMEDIUM_0 {
                    hGlobal: allocate_global_payload(&payload)?,
                },
                pUnkForRelease: Default::default(),
            });
        }
        #[cfg(test)]
        if format == *FILE_CONTENTS_FORMAT {
            let (_, bytes, medium_kind) = self
                .test_virtual_files
                .as_ref()
                .and_then(|files| files.get(requested.lindex as usize))
                .ok_or(DV_E_FORMATETC)?;
            if *medium_kind == 0 {
                let stream = unsafe { SHCreateMemStream(Some(bytes)) }.ok_or(E_OUTOFMEMORY)?;
                return Ok(STGMEDIUM {
                    tymed: TYMED_ISTREAM.0 as u32,
                    u: STGMEDIUM_0 {
                        pstm: mem::ManuallyDrop::new(Some(stream)),
                    },
                    pUnkForRelease: Default::default(),
                });
            }
            if *medium_kind == 1 {
                return Ok(STGMEDIUM {
                    tymed: TYMED_HGLOBAL.0 as u32,
                    u: STGMEDIUM_0 {
                        hGlobal: allocate_global_payload(bytes)?,
                    },
                    pUnkForRelease: Default::default(),
                });
            }
            return Ok(STGMEDIUM::default());
        }
        let hglobal = if format == CF_HDROP.0 {
            allocate_hdrop(self.paths.as_slice())?
        } else if format == *PREFERRED_DROPEFFECT_FORMAT {
            allocate_dropeffect(self.preferred_effect)?
        } else {
            #[cfg(test)]
            {
                if format == *TEST_SHELL_ID_LIST_FORMAT {
                    if self.test_fail_shell_get_data {
                        return Err(DV_E_FORMATETC.into());
                    }
                    allocate_global_payload(
                        self.test_shell_id_list
                            .as_deref()
                            .ok_or(DV_E_FORMATETC)?,
                    )?
                } else {
                    return Err(DV_E_FORMATETC.into());
                }
            }
            #[cfg(not(test))]
            return Err(DV_E_FORMATETC.into());
        };
        Ok(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: hglobal },
            pUnkForRelease: Default::default(),
        })
    }

    fn GetDataHere(
        &self,
        _pformatetc: *const FORMATETC,
        _pmedium: *mut STGMEDIUM,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn QueryGetData(&self, pformatetc: *const FORMATETC) -> windows::core::HRESULT {
        let hdrop_supported = is_hdrop_format(pformatetc)
            && {
                #[cfg(test)]
                {
                    self.test_query_hdrop
                }
                #[cfg(not(test))]
                {
                    true
                }
            };
        #[cfg(test)]
        let shell_id_list_supported = self.test_shell_id_list.is_some()
            && is_clipboard_hglobal_format(pformatetc, *TEST_SHELL_ID_LIST_FORMAT);
        #[cfg(test)]
        let virtual_file_supported = unsafe { pformatetc.as_ref() }.is_some_and(|format| {
            self.test_virtual_files.as_ref().is_some_and(|files| {
                (format.cfFormat == *FILE_DESCRIPTOR_W_FORMAT
                    && (format.tymed & TYMED_HGLOBAL.0 as u32) != 0)
                    || (format.cfFormat == *FILE_CONTENTS_FORMAT
                        && format.lindex >= 0
                        && (format.lindex as usize) < files.len()
                        && (format.tymed
                            & (TYMED_ISTREAM.0 as u32 | TYMED_HGLOBAL.0 as u32))
                            != 0)
            })
        });
        #[cfg(not(test))]
        let virtual_file_supported = false;
        #[cfg(not(test))]
        let shell_id_list_supported = false;

        if hdrop_supported
            || shell_id_list_supported
            || virtual_file_supported
            || is_dropeffect_format(pformatetc, *PREFERRED_DROPEFFECT_FORMAT)
        {
            S_OK
        } else {
            DV_E_FORMATETC
        }
    }

    fn GetCanonicalFormatEtc(
        &self,
        _pformatectin: *const FORMATETC,
        _pformatetcout: *mut FORMATETC,
    ) -> windows::core::HRESULT {
        E_NOTIMPL
    }

    fn SetData(
        &self,
        pformatetc: *const FORMATETC,
        pmedium: *const STGMEDIUM,
        frelease: BOOL,
    ) -> windows::core::Result<()> {
        if is_dropeffect_format(pformatetc, *PERFORMED_DROPEFFECT_FORMAT) {
            if let Some(effect) = read_dropeffect_from_medium(pmedium) {
                self.performed_effect.set(effect);
            }
        } else if is_dropeffect_format(pformatetc, *LOGICAL_PERFORMED_DROPEFFECT_FORMAT)
            && let Some(effect) = read_dropeffect_from_medium(pmedium)
        {
            self.logical_performed_effect.set(effect);
        }

        if frelease.as_bool() && !pmedium.is_null() {
            let mut medium = unsafe { std::ptr::read(pmedium) };
            unsafe { ReleaseStgMedium(&mut medium) };
        }

        Ok(())
    }

    fn EnumFormatEtc(&self, dwdirection: u32) -> windows::core::Result<IEnumFORMATETC> {
        if dwdirection == DATADIR_GET.0 as u32 {
            let mut formats = Vec::new();
            #[cfg(test)]
            if self.test_offer_hdrop {
                formats.push(hdrop_format_etc());
            }
            #[cfg(test)]
            if self.test_virtual_files.is_some() {
                formats.push(FORMATETC {
                    cfFormat: *FILE_DESCRIPTOR_W_FORMAT,
                    ptd: std::ptr::null_mut(),
                    dwAspect: DVASPECT_CONTENT.0,
                    lindex: -1,
                    tymed: TYMED_HGLOBAL.0 as u32,
                });
                formats.push(FORMATETC {
                    cfFormat: *FILE_CONTENTS_FORMAT,
                    ptd: std::ptr::null_mut(),
                    dwAspect: DVASPECT_CONTENT.0,
                    lindex: 0,
                    tymed: (TYMED_ISTREAM.0 | TYMED_HGLOBAL.0) as u32,
                });
            }
            #[cfg(not(test))]
            formats.push(hdrop_format_etc());
            formats.push(dropeffect_format_etc(*PREFERRED_DROPEFFECT_FORMAT));
            Ok(WindowsFormatEtcEnumerator::new(formats).into())
        } else {
            Err(E_NOTIMPL.into())
        }
    }

    fn DAdvise(
        &self,
        _pformatetc: *const FORMATETC,
        _advf: u32,
        _padvsink: windows::core::Ref<'_, IAdviseSink>,
    ) -> windows::core::Result<u32> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn DUnadvise(&self, _dwconnection: u32) -> windows::core::Result<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}

#[allow(non_snake_case)]
#[cfg(test)]
impl IDataObjectAsyncCapability_Impl for WindowsFileDataObject_Impl {
    fn SetAsyncMode(&self, _async_mode: BOOL) -> windows::core::Result<()> {
        Ok(())
    }

    fn GetAsyncMode(&self) -> windows::core::Result<BOOL> {
        if let Some(call_order) = &self.test_call_order {
            call_order.as_ref().borrow_mut().push("get_async_mode");
        }
        Ok(self.test_async_mode.into())
    }

    fn StartOperation(
        &self,
        _reserved: windows::core::Ref<'_, IBindCtx>,
    ) -> windows::core::Result<()> {
        if let Some(call_order) = &self.test_call_order {
            call_order.as_ref().borrow_mut().push("start_operation");
        }
        if let Some(start_count) = &self.test_start_count {
            start_count.set(start_count.get() + 1);
        }
        if let Some(error) = self.test_start_error {
            return Err(error.into());
        }
        Ok(())
    }

    fn InOperation(&self) -> windows::core::Result<BOOL> {
        Ok(false.into())
    }

    fn EndOperation(
        &self,
        result: windows::core::HRESULT,
        _reserved: windows::core::Ref<'_, IBindCtx>,
        effects: u32,
    ) -> windows::core::Result<()> {
        if let Some(call_order) = &self.test_call_order {
            call_order.as_ref().borrow_mut().push("end_operation");
        }
        if let Some(end_events) = &self.test_end_events {
            end_events.as_ref().borrow_mut().push((result, effects));
        }
        Ok(())
    }
}

#[implement(IEnumFORMATETC)]
struct WindowsFormatEtcEnumerator {
    next_index: Cell<usize>,
    formats: Vec<FORMATETC>,
}

impl WindowsFormatEtcEnumerator {
    fn new(formats: Vec<FORMATETC>) -> Self {
        Self {
            next_index: Cell::new(0),
            formats,
        }
    }
}

#[allow(non_snake_case)]
impl IEnumFORMATETC_Impl for WindowsFormatEtcEnumerator_Impl {
    fn Next(&self, celt: u32, rgelt: *mut FORMATETC, pceltfetched: *mut u32) -> windows::core::HRESULT {
        if rgelt.is_null() || (celt > 1 && pceltfetched.is_null()) {
            return E_INVALIDARG;
        }

        let mut fetched = 0;
        while fetched < celt && self.next_index.get() < self.formats.len() {
            let format = self.formats[self.next_index.get()];
            unsafe { rgelt.add(fetched as usize).write(format) };
            self.next_index.set(self.next_index.get() + 1);
            fetched += 1;
        }

        if !pceltfetched.is_null() {
            unsafe {
                pceltfetched.write(fetched);
            }
        }

        if fetched == celt { S_OK } else { S_FALSE }
    }

    fn Skip(&self, celt: u32) -> windows::core::Result<()> {
        let remaining = self.formats.len().saturating_sub(self.next_index.get());
        self.next_index
            .set((self.next_index.get() + celt as usize).min(self.formats.len()));
        if celt as usize <= remaining {
            Ok(())
        } else {
            Err(S_FALSE.into())
        }
    }

    fn Reset(&self) -> windows::core::Result<()> {
        self.next_index.set(0);
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IEnumFORMATETC> {
        Ok(WindowsFormatEtcEnumerator {
            next_index: Cell::new(self.next_index.get()),
            formats: self.formats.clone(),
        }
        .into())
    }
}

fn start_windows_external_paths_drag(hwnd: HWND, paths: ExternalPaths) -> ExternalPathsDragStartResult {
    let operations = paths.operations();
    let preferred_effect = preferred_dropeffect_for_operations(operations);
    let allowed_effects = allowed_dropeffects_for_operations(operations);
    let paths = paths
        .paths()
        .iter()
        .filter(|path| path.as_os_str().len() > 0)
        .cloned()
        .collect::<Vec<_>>();

    if paths.is_empty() {
        return ExternalPathsDragStartResult::Failed;
    }

    let performed_effect = Rc::new(Cell::new(DROPEFFECT_NONE));
    let logical_performed_effect = Rc::new(Cell::new(DROPEFFECT_NONE));
    let result = unsafe {
        let data_object: IDataObject = WindowsFileDataObject {
            paths,
            preferred_effect,
            performed_effect: performed_effect.clone(),
            logical_performed_effect: logical_performed_effect.clone(),
            #[cfg(test)]
            test_offer_hdrop: true,
            #[cfg(test)]
            test_query_hdrop: true,
            #[cfg(test)]
            test_shell_id_list: None,
            #[cfg(test)]
            test_fail_shell_get_data: false,
            #[cfg(test)]
            test_fail_hdrop_get_data: false,
            #[cfg(test)]
            test_hdrop_failures_remaining: None,
            #[cfg(test)]
            test_async_mode: false,
            #[cfg(test)]
            test_start_error: None,
            #[cfg(test)]
            test_call_order: None,
            #[cfg(test)]
            test_start_count: None,
            #[cfg(test)]
            test_end_events: None,
            #[cfg(test)]
            test_virtual_files: None,
            #[cfg(test)]
            test_chromium_virtual_descriptor: false,
            #[cfg(test)]
            test_virtual_directory: false,
            #[cfg(test)]
            test_malformed_virtual_descriptors: false,
        }
        .into();
        let drop_source: IDropSource = WindowsFileDragSource.into();
        SHDoDragDrop(Some(hwnd), &data_object, &drop_source, allowed_effects)
    };

    match result.log_err() {
        Some(effect) => ExternalPathsDragStartResult::Completed(windows_external_drag_result(
            effect,
            performed_effect.get(),
            logical_performed_effect.get(),
        )),
        None => ExternalPathsDragStartResult::Failed,
    }
}

#[derive(Default)]
struct PendingExternalPathsDrag {
    paths: Option<ExternalPaths>,
}

impl PendingExternalPathsDrag {
    fn queue(&mut self, paths: ExternalPaths) -> bool {
        if self.paths.is_some() {
            false
        } else {
            self.paths = Some(paths);
            true
        }
    }

    fn take(&mut self) -> Option<ExternalPaths> {
        self.paths.take()
    }

    fn cancel(&mut self) {
        self.paths = None;
    }
}

fn windows_external_drag_completion(
    result: ExternalPathsDragStartResult,
) -> ExternalPathsDragResult {
    match result {
        ExternalPathsDragStartResult::Completed(result) => result,
        ExternalPathsDragStartResult::Pending | ExternalPathsDragStartResult::Failed => {
            ExternalPathsDragResult::Cancelled
        }
    }
}

fn allowed_dropeffects_for_operations(operations: ExternalPathDragOperations) -> DROPEFFECT {
    let mut effect = DROPEFFECT_NONE;
    if operations.copy() {
        effect |= DROPEFFECT_COPY;
    }
    if operations.move_() {
        effect |= DROPEFFECT_MOVE;
    }
    if operations.link() {
        effect |= DROPEFFECT_LINK;
    }
    effect
}

fn preferred_dropeffect_for_operations(operations: ExternalPathDragOperations) -> DROPEFFECT {
    if operations.link() && !operations.copy() && !operations.move_() {
        DROPEFFECT_LINK
    } else if operations.move_() && !operations.copy() {
        DROPEFFECT_MOVE
    } else if operations.copy() {
        DROPEFFECT_COPY
    } else {
        DROPEFFECT_NONE
    }
}

fn windows_external_drag_result(
    drop_effect: DROPEFFECT,
    performed_effect: DROPEFFECT,
    logical_performed_effect: DROPEFFECT,
) -> ExternalPathsDragResult {
    if drop_effect == DROPEFFECT_LINK {
        ExternalPathsDragResult::link()
    } else if drop_effect == DROPEFFECT_MOVE {
        let cleanup_source = performed_effect == DROPEFFECT_MOVE
            || (performed_effect == DROPEFFECT_NONE && logical_performed_effect == DROPEFFECT_MOVE);
        ExternalPathsDragResult::move_(cleanup_source)
    } else if drop_effect == DROPEFFECT_COPY {
        ExternalPathsDragResult::copy()
    } else {
        ExternalPathsDragResult::Cancelled
    }
}

fn is_hdrop_format(format: *const FORMATETC) -> bool {
    let Some(format) = (unsafe { format.as_ref() }) else {
        return false;
    };

    format.cfFormat == CF_HDROP.0
        && format.dwAspect == DVASPECT_CONTENT.0
        && (format.tymed & TYMED_HGLOBAL.0 as u32) != 0
}

fn is_dropeffect_format(format: *const FORMATETC, expected_format: u16) -> bool {
    is_clipboard_hglobal_format(format, expected_format)
}

fn is_clipboard_hglobal_format(format: *const FORMATETC, expected_format: u16) -> bool {
    let Some(format) = (unsafe { format.as_ref() }) else {
        return false;
    };

    format.cfFormat == expected_format
        && format.dwAspect == DVASPECT_CONTENT.0
        && (format.tymed & TYMED_HGLOBAL.0 as u32) != 0
}

fn hdrop_format_etc() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn dropeffect_format_etc(format: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: format,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn build_hdrop_payload(paths: &[PathBuf]) -> Vec<u8> {
    let mut encoded_paths = Vec::<u16>::new();
    for path in paths {
        encoded_paths.extend(path.to_string_lossy().encode_utf16());
        encoded_paths.push(0);
    }
    encoded_paths.push(0);

    let header = DROPFILES {
        pFiles: mem::size_of::<DROPFILES>() as u32,
        pt: POINT { x: 0, y: 0 },
        fNC: BOOL(0),
        fWide: BOOL(1),
    };

    let header_bytes = unsafe {
        std::slice::from_raw_parts(
            std::ptr::addr_of!(header).cast::<u8>(),
            mem::size_of::<DROPFILES>(),
        )
    };
    let path_bytes = unsafe {
        std::slice::from_raw_parts(
            encoded_paths.as_ptr().cast::<u8>(),
            encoded_paths.len() * mem::size_of::<u16>(),
        )
    };

    let mut payload = Vec::with_capacity(header_bytes.len() + path_bytes.len());
    payload.extend_from_slice(header_bytes);
    payload.extend_from_slice(path_bytes);
    payload
}

fn allocate_dropeffect(effect: DROPEFFECT) -> windows::core::Result<HGLOBAL> {
    let effect = effect.0.to_ne_bytes();
    unsafe {
        let global = GlobalAlloc(GMEM_MOVEABLE, effect.len())?;
        let handle = GlobalLock(global);
        if handle.is_null() {
            return Err(windows::core::Error::from_win32());
        }
        std::ptr::copy_nonoverlapping(effect.as_ptr(), handle.cast::<u8>(), effect.len());
        let _ = GlobalUnlock(global);
        Ok(global)
    }
}

fn read_dropeffect_from_medium(medium: *const STGMEDIUM) -> Option<DROPEFFECT> {
    let medium = unsafe { medium.as_ref() }?;
    if (medium.tymed & TYMED_HGLOBAL.0 as u32) == 0 {
        return None;
    }

    let global = unsafe { medium.u.hGlobal };
    let size = unsafe { GlobalSize(global) };
    if size < mem::size_of::<u32>() {
        return None;
    }

    let handle = unsafe { GlobalLock(global) };
    if handle.is_null() {
        return None;
    }

    let effect = unsafe { std::ptr::read_unaligned(handle.cast::<u32>()) };
    let _ = unsafe { GlobalUnlock(global) };
    Some(DROPEFFECT(effect))
}

fn allocate_hdrop(paths: &[PathBuf]) -> windows::core::Result<HGLOBAL> {
    let payload = build_hdrop_payload(paths);
    allocate_global_payload(&payload)
}

fn allocate_global_payload(payload: &[u8]) -> windows::core::Result<HGLOBAL> {
    unsafe {
        let global = GlobalAlloc(GMEM_MOVEABLE, payload.len())?;
        let handle = GlobalLock(global);
        if handle.is_null() {
            return Err(windows::core::Error::from_win32());
        }
        std::ptr::copy_nonoverlapping(payload.as_ptr(), handle.cast::<u8>(), payload.len());
        let _ = GlobalUnlock(global);
        Ok(global)
    }
}

#[cfg(test)]
fn build_virtual_descriptor_payload(
    files: &[(String, Vec<u8>, u8)],
    chromium_style: bool,
    directory: bool,
    malformed: bool,
) -> Vec<u8> {
    let descriptor_size = mem::size_of::<FILEDESCRIPTORW>();
    let mut payload = vec![0_u8; mem::size_of::<u32>() + files.len() * descriptor_size];
    let advertised_count = if malformed {
        files.len().saturating_add(1)
    } else {
        files.len()
    } as u32;
    payload[..mem::size_of::<u32>()].copy_from_slice(&advertised_count.to_ne_bytes());
    for (index, (name, contents, _)) in files.iter().enumerate() {
        let mut descriptor = FILEDESCRIPTORW {
            dwFlags: if chromium_style {
                FD_LINKUI.0 as u32
            } else {
                (FD_ATTRIBUTES.0 | FD_FILESIZE.0) as u32
            },
            dwFileAttributes: if directory {
                FILE_ATTRIBUTE_DIRECTORY.0
            } else {
                0
            },
            nFileSizeHigh: if chromium_style {
                0
            } else {
                (contents.len() as u64 >> 32) as u32
            },
            nFileSizeLow: if chromium_style {
                0
            } else {
                contents.len() as u32
            },
            ..Default::default()
        };
        let mut file_name = [0_u16; 260];
        for (destination, source) in file_name.iter_mut().zip(name.encode_utf16().take(259)) {
            *destination = source;
        }
        descriptor.cFileName = file_name;
        unsafe {
            std::ptr::write_unaligned(
                payload
                    .as_mut_ptr()
                    .add(mem::size_of::<u32>() + index * descriptor_size)
                    .cast::<FILEDESCRIPTORW>(),
                descriptor,
            );
        }
    }
    payload
}

enum HdropPaths {
    Unavailable,
    Invalid,
    Deferred,
    Paths(SmallVec<[PathBuf; 2]>),
}

fn enabled_async_capability(data_object: &IDataObject) -> Option<IDataObjectAsyncCapability> {
    let capability = match data_object.cast::<IDataObjectAsyncCapability>() {
        Ok(capability) => capability,
        Err(error) => {
            log::trace!("external drop has no async capability: {error}");
            return None;
        }
    };
    match unsafe { capability.GetAsyncMode() } {
        Ok(enabled) if enabled.as_bool() => {
            log::trace!("external drop GetAsyncMode succeeded and is enabled");
            Some(capability)
        }
        Ok(_) => {
            log::trace!("external drop async capability is disabled");
            None
        }
        Err(error) => {
            log::trace!("external drop async mode query failed: {error}");
            None
        }
    }
}

fn data_object_advertises_hdrop(data_object: &IDataObject) -> bool {
    let format = hdrop_format_etc();
    let query_result = unsafe { data_object.QueryGetData(&format) };
    let mut enumerated_hdrop = false;
    let mut enumerated_formats = Vec::new();

    match unsafe { data_object.EnumFormatEtc(DATADIR_GET.0 as u32) } {
        Ok(enumerator) => loop {
            let mut format = FORMATETC::default();
            let mut fetched = 0;
            let result = unsafe {
                enumerator.Next(std::slice::from_mut(&mut format), Some(&mut fetched))
            };
            if fetched == 0 {
                break;
            }
            enumerated_formats.push((format.cfFormat, format.dwAspect, format.tymed));
            enumerated_hdrop |= format.cfFormat == CF_HDROP.0
                && format.dwAspect == DVASPECT_CONTENT.0
                && (format.tymed & TYMED_HGLOBAL.0 as u32) != 0;
            if !format.ptd.is_null() {
                unsafe { CoTaskMemFree(Some(format.ptd.cast())) };
            }
            if result != S_OK {
                break;
            }
        },
        Err(error) => log::trace!("external drop format enumeration failed: {error}"),
    }

    log::trace!(
        "external drop CF_HDROP discovery: query={query_result:?}, enumerated={enumerated_formats:?}"
    );
    enumerated_hdrop || query_result == S_OK
}

fn paths_from_hdrop_medium(medium: &STGMEDIUM) -> Option<SmallVec<[PathBuf; 2]>> {
    if (medium.tymed & TYMED_HGLOBAL.0 as u32) == 0 {
        return None;
    }

    unsafe {
        let global = medium.u.hGlobal;
        if global.is_invalid() {
            return None;
        }
        let locked = GlobalLock(global);
        if locked.is_null() {
            return None;
        }

        let hdrop = HDROP(locked);
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        let mut paths = SmallVec::<[PathBuf; 2]>::with_capacity(count as usize);
        let result = (count > 0).then(|| {
            for index in 0..count {
                let length = DragQueryFileW(hdrop, index, None) as usize;
                if length == 0 {
                    return None;
                }
                let mut buffer = vec![0_u16; length + 1];
                if DragQueryFileW(hdrop, index, Some(buffer.as_mut_slice())) as usize != length {
                    return None;
                }
                let path = PathBuf::from(String::from_utf16(&buffer[..length]).ok()?);
                if path.as_os_str().is_empty() {
                    return None;
                }
                paths.push(path);
            }
            Some(paths)
        });
        let _ = GlobalUnlock(global);
        result.flatten()
    }
}

fn hdrop_paths_from_data_object(data_object: &IDataObject) -> HdropPaths {
    if !data_object_advertises_hdrop(data_object) {
        return HdropPaths::Unavailable;
    }

    let mut medium = match unsafe { data_object.GetData(&hdrop_format_etc()) } {
        Ok(medium) => medium,
        Err(error) => {
            log::trace!("external drop CF_HDROP materialization failed: {error}");
            return HdropPaths::Deferred;
        }
    };
    if (medium.tymed & TYMED_HGLOBAL.0 as u32) == 0 {
        log::trace!(
            "external drop CF_HDROP is advertised but returned unavailable medium {}",
            medium.tymed
        );
        unsafe { ReleaseStgMedium(&mut medium) };
        return HdropPaths::Deferred;
    }
    let paths = paths_from_hdrop_medium(&medium);
    unsafe { ReleaseStgMedium(&mut medium) };
    paths.map_or(HdropPaths::Invalid, HdropPaths::Paths)
}

fn data_object_advertises_virtual_files(data_object: &IDataObject) -> bool {
    let descriptor = FORMATETC {
        cfFormat: *FILE_DESCRIPTOR_W_FORMAT,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let contents = FORMATETC {
        cfFormat: *FILE_CONTENTS_FORMAT,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: 0,
        tymed: (TYMED_ISTREAM.0 | TYMED_HGLOBAL.0) as u32,
    };
    let descriptor_result = unsafe { data_object.QueryGetData(&descriptor) };
    let contents_result = unsafe { data_object.QueryGetData(&contents) };
    log::trace!(
        "external drop virtual-file discovery: descriptor={descriptor_result:?}, contents={contents_result:?}"
    );
    descriptor_result == S_OK && contents_result == S_OK
}

fn shell_item_paths_from_data_object(
    data_object: &IDataObject,
) -> Option<SmallVec<[PathBuf; 2]>> {
    let items: IShellItemArray =
        unsafe { SHCreateShellItemArrayFromDataObject(data_object) }.ok()?;
    let count = unsafe { items.GetCount() }.ok()?;
    if count == 0 {
        return None;
    }

    let mut paths = SmallVec::<[PathBuf; 2]>::with_capacity(count as usize);
    for index in 0..count {
        let item = unsafe { items.GetItemAt(index) }.ok()?;
        let display_name = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }.ok()?;
        if display_name.0.is_null() {
            return None;
        }
        let path = unsafe { display_name.to_string() };
        unsafe { CoTaskMemFree(Some(display_name.0.cast())) };
        let path = PathBuf::from(path.ok()?);
        if path.as_os_str().is_empty() || !path.is_absolute() {
            return None;
        }
        paths.push(path);
    }
    Some(paths)
}

fn external_paths_from_data_object(data_object: &IDataObject) -> Option<ExternalPaths> {
    match hdrop_paths_from_data_object(data_object) {
        HdropPaths::Paths(paths) => Some(ExternalPaths::new(paths)),
        HdropPaths::Unavailable => shell_item_paths_from_data_object(data_object)
            .map(ExternalPaths::new)
            .or_else(|| {
                data_object_advertises_virtual_files(data_object)
                    .then(ExternalPaths::pending_windows_drop)
            }),
        HdropPaths::Deferred => shell_item_paths_from_data_object(data_object)
            .map(ExternalPaths::new)
            .or_else(|| Some(ExternalPaths::pending_windows_drop())),
        HdropPaths::Invalid => {
            shell_item_paths_from_data_object(data_object).map(ExternalPaths::new)
        }
    }
}

#[derive(Clone)]
struct DeferredWindowsExternalDrop {
    data_object: IDataObject,
    allowed_effects: DROPEFFECT,
}

struct MarshaledWindowsInterface(Option<IStream>);

// CoMarshalInterThreadInterfaceInStream explicitly creates a stream that may be handed to
// another apartment. The windows crate cannot express that conditional COM guarantee.
unsafe impl Send for MarshaledWindowsInterface {}

impl MarshaledWindowsInterface {
    fn new<T: Interface>(interface: &T) -> windows::core::Result<Self> {
        let unknown = interface.cast::<IUnknown>()?;
        let stream =
            unsafe { CoMarshalInterThreadInterfaceInStream(&T::IID, &unknown) }?;
        Ok(Self(Some(stream)))
    }

    fn unmarshal<T: Interface>(mut self) -> windows::core::Result<T> {
        let stream = self.0.take().ok_or(E_UNEXPECTED)?;
        let stream = mem::ManuallyDrop::new(stream);
        unsafe { CoGetInterfaceAndReleaseStream::<_, T>(&*stream) }
    }
}

impl Drop for MarshaledWindowsInterface {
    fn drop(&mut self) {
        if let Some(stream) = self.0.take() {
            unsafe { CoReleaseMarshalData(&stream).log_err() };
        }
    }
}

enum PendingWindowsExternalDropWork {
    LiveData {
        marshaled_data_object: MarshaledWindowsInterface,
        marshaled_async_capability: MarshaledWindowsInterface,
        async_capability: IDataObjectAsyncCapability,
    },
    StagedFiles(VirtualDropStaging),
}

struct PendingDeferredWindowsExternalDrop {
    work: PendingWindowsExternalDropWork,
    destination: PathBuf,
    owner: isize,
    completion: Option<oneshot::Sender<std::result::Result<(), String>>>,
}

impl PendingDeferredWindowsExternalDrop {
    fn cancel(mut self, result: HRESULT) {
        if let PendingWindowsExternalDropWork::LiveData {
            async_capability, ..
        } = &self.work
        {
            unsafe {
                async_capability
                    .EndOperation(result, None, DROPEFFECT_NONE.0 as u32)
                    .log_err();
            }
        }
        if let Some(completion) = self.completion.take() {
            let _ = completion.send(Err(format!(
                "The pending file transfer was cancelled ({result:?})."
            )));
        }
    }
}

struct ActiveWindowsExternalDrop {
    completed_effect: Cell<Option<DROPEFFECT>>,
    deferred: Option<DeferredWindowsExternalDrop>,
}

struct ActiveWindowsExternalDropGuard<'a> {
    active_drops: &'a RefCell<Vec<Rc<ActiveWindowsExternalDrop>>>,
}

impl Drop for ActiveWindowsExternalDropGuard<'_> {
    fn drop(&mut self) {
        self.active_drops.borrow_mut().pop();
    }
}

fn push_active_windows_external_drop<'a>(
    active_drops: &'a RefCell<Vec<Rc<ActiveWindowsExternalDrop>>>,
    drop: Rc<ActiveWindowsExternalDrop>,
) -> ActiveWindowsExternalDropGuard<'a> {
    active_drops.borrow_mut().push(drop);
    ActiveWindowsExternalDropGuard { active_drops }
}

fn completed_external_drop_effect(
    drop: &ActiveWindowsExternalDrop,
    default: DROPEFFECT,
) -> DROPEFFECT {
    drop.completed_effect.get().unwrap_or(default)
}

fn default_external_drop_effect(deferred: bool) -> DROPEFFECT {
    if deferred {
        DROPEFFECT_NONE
    } else {
        DROPEFFECT_COPY
    }
}

fn complete_active_windows_external_drop(
    active_drops: &RefCell<Vec<Rc<ActiveWindowsExternalDrop>>>,
    effect: u32,
) -> bool {
    let Some(drop) = active_drops.borrow().last().cloned() else {
        return false;
    };
    drop.completed_effect.set(Some(DROPEFFECT(effect)));
    true
}

fn shell_item_for_path(destination: &Path) -> windows::core::Result<IShellItem> {
    use std::os::windows::ffi::OsStrExt as _;

    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    unsafe { SHCreateItemFromParsingName(PCWSTR(destination.as_ptr()), None) }
}

fn materialize_deferred_hdrop_paths(
    data_object: &IDataObject,
) -> windows::core::Result<SmallVec<[PathBuf; 2]>> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let attempt_started = Instant::now();
        match materialize_hdrop_paths_once(data_object) {
            Ok(paths) => return Ok(paths),
            Err(error)
                if error.code() == DV_E_FORMATETC
                    && attempt_started.elapsed() < Duration::from_millis(100)
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

fn materialize_hdrop_paths_once(
    data_object: &IDataObject,
) -> windows::core::Result<SmallVec<[PathBuf; 2]>> {
    let mut medium = unsafe { data_object.GetData(&hdrop_format_etc()) }?;
    let paths = paths_from_hdrop_medium(&medium);
    unsafe { ReleaseStgMedium(&mut medium) };
    let paths = paths.ok_or(E_INVALIDARG)?;
    if paths
        .iter()
        .all(|path| path.is_absolute() && path.exists())
    {
        Ok(paths)
    } else {
        Err(E_INVALIDARG.into())
    }
}

static NEXT_VIRTUAL_DROP_STAGING_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct VirtualDropStaging {
    root: PathBuf,
    paths: Vec<PathBuf>,
}

impl VirtualDropStaging {
    fn new() -> windows::core::Result<Self> {
        for _ in 0..32 {
            let sequence = NEXT_VIRTUAL_DROP_STAGING_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "gpui-virtual-drop-{}-{sequence}",
                std::process::id()
            ));
            match std::fs::create_dir(&root) {
                Ok(()) => return Ok(Self { root, paths: Vec::new() }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    log::trace!("failed to create virtual-drop staging directory: {error}");
                    return Err(E_FAIL.into());
                }
            }
        }
        Err(E_FAIL.into())
    }
}

impl Drop for VirtualDropStaging {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.root)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!(
                "failed to remove virtual-drop staging directory {}: {error}",
                self.root.display()
            );
        }
    }
}

#[derive(Debug)]
struct VirtualFileDescriptor {
    name: String,
    expected_size: Option<u64>,
}

#[derive(Debug)]
struct PendingWindowsDropFailure {
    stage: String,
    error: windows::core::Error,
}

impl PendingWindowsDropFailure {
    fn new(stage: impl Into<String>, error: windows::core::Error) -> Self {
        Self {
            stage: stage.into(),
            error,
        }
    }

    fn code(&self) -> HRESULT {
        self.error.code()
    }

    fn user_message(&self) -> String {
        format!(
            "The file transfer failed while {}: {} ({:?}).",
            self.stage,
            self.error,
            self.error.code()
        )
    }
}

fn virtual_file_descriptors(
    data_object: &IDataObject,
) -> windows::core::Result<Vec<VirtualFileDescriptor>> {
    let format = FORMATETC {
        cfFormat: *FILE_DESCRIPTOR_W_FORMAT,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let mut medium = unsafe { data_object.GetData(&format) }?;
    let result = (|| {
        if (medium.tymed & TYMED_HGLOBAL.0 as u32) == 0 {
            return Err(DV_E_TYMED.into());
        }
        let global = unsafe { medium.u.hGlobal };
        let size = unsafe { GlobalSize(global) };
        if size < mem::size_of::<u32>() {
            return Err(E_INVALIDARG.into());
        }
        let locked = unsafe { GlobalLock(global) };
        if locked.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let parsed = (|| {
            let count = unsafe { std::ptr::read_unaligned(locked.cast::<u32>()) } as usize;
            if count == 0 || count > 4096 {
                return Err(E_INVALIDARG.into());
            }
            let descriptor_size = mem::size_of::<FILEDESCRIPTORW>();
            let required = mem::size_of::<u32>()
                .checked_add(count.checked_mul(descriptor_size).ok_or(E_INVALIDARG)?)
                .ok_or(E_INVALIDARG)?;
            if size < required {
                return Err(E_INVALIDARG.into());
            }
            let mut descriptors = Vec::with_capacity(count);
            let mut names = std::collections::HashSet::with_capacity(count);
            for index in 0..count {
                let descriptor = unsafe {
                    std::ptr::read_unaligned(
                        locked
                            .cast::<u8>()
                            .add(mem::size_of::<u32>() + index * descriptor_size)
                            .cast::<FILEDESCRIPTORW>(),
                    )
                };
                let flags = descriptor.dwFlags;
                let attributes = descriptor.dwFileAttributes;
                if (flags & FD_ATTRIBUTES.0 as u32) != 0
                    && (attributes & FILE_ATTRIBUTE_DIRECTORY.0) != 0
                {
                    return Err(E_INVALIDARG.into());
                }
                let file_name = descriptor.cFileName;
                let length = file_name
                    .iter()
                    .position(|character| *character == 0)
                    .ok_or(E_INVALIDARG)?;
                let name = String::from_utf16(&file_name[..length])
                    .map_err(|_| windows::core::Error::from(E_INVALIDARG))?;
                let path = Path::new(&name);
                let has_invalid_windows_character = name.chars().any(|character| {
                    character < ' '
                        || matches!(character, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
                });
                let safe_leaf = !name.is_empty()
                    && !has_invalid_windows_character
                    && !name.ends_with(' ')
                    && !name.ends_with('.')
                    && path.file_name().is_some_and(|leaf| leaf == path.as_os_str())
                    && path.components().count() == 1;
                if !safe_leaf || !names.insert(name.to_lowercase()) {
                    return Err(E_INVALIDARG.into());
                }
                let expected_size = ((flags & FD_FILESIZE.0 as u32) != 0).then_some(
                    ((descriptor.nFileSizeHigh as u64) << 32) | descriptor.nFileSizeLow as u64,
                );
                descriptors.push(VirtualFileDescriptor { name, expected_size });
            }
            Ok(descriptors)
        })();
        let _ = unsafe { GlobalUnlock(global) };
        parsed
    })();
    unsafe { ReleaseStgMedium(&mut medium) };
    result
}

fn write_virtual_file_contents_with_diagnostics(
    data_object: &IDataObject,
    index: usize,
    destination: &Path,
    name: &str,
) -> std::result::Result<u64, PendingWindowsDropFailure> {
    let format = FORMATETC {
        cfFormat: *FILE_CONTENTS_FORMAT,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: index.try_into().map_err(|_| {
            PendingWindowsDropFailure::new(
                format!("selecting CFSTR_FILECONTENTS item {index} ({name})"),
                E_INVALIDARG.into(),
            )
        })?,
        tymed: (TYMED_ISTREAM.0 | TYMED_HGLOBAL.0) as u32,
    };
    let mut medium = unsafe { data_object.GetData(&format) }.map_err(|error| {
        PendingWindowsDropFailure::new(
            format!("retrieving CFSTR_FILECONTENTS item {index} ({name})"),
            error,
        )
    })?;
    let result = (|| {
        let mut file = File::create(destination).map_err(|error| {
            PendingWindowsDropFailure::new(
                format!("creating the staging file for item {index} ({name})"),
                windows::core::Error::new(E_FAIL, error.to_string()),
            )
        })?;
        if (medium.tymed & TYMED_ISTREAM.0 as u32) != 0 {
            let stream = unsafe { (&*medium.u.pstm).as_ref().cloned() }.ok_or_else(|| {
                PendingWindowsDropFailure::new(
                    format!("opening the IStream for item {index} ({name})"),
                    DV_E_TYMED.into(),
                )
            })?;
            let mut total = 0_u64;
            loop {
                let mut buffer = [0_u8; 64 * 1024];
                let mut read = 0_u32;
                let result = unsafe {
                    stream.Read(
                        buffer.as_mut_ptr().cast(),
                        buffer.len() as u32,
                        Some(&mut read),
                    )
                };
                result.ok().map_err(|error| {
                    PendingWindowsDropFailure::new(
                        format!("reading the IStream for item {index} ({name})"),
                        error,
                    )
                })?;
                if read == 0 {
                    break;
                }
                file.write_all(&buffer[..read as usize]).map_err(|error| {
                    PendingWindowsDropFailure::new(
                        format!("writing the staged IStream for item {index} ({name})"),
                        windows::core::Error::new(E_FAIL, error.to_string()),
                    )
                })?;
                total = total.checked_add(read as u64).ok_or_else(|| {
                    PendingWindowsDropFailure::new(
                        format!("measuring the IStream for item {index} ({name})"),
                        E_FAIL.into(),
                    )
                })?;
            }
            file.flush().map_err(|error| {
                PendingWindowsDropFailure::new(
                    format!("flushing the staged IStream for item {index} ({name})"),
                    windows::core::Error::new(E_FAIL, error.to_string()),
                )
            })?;
            Ok(total)
        } else if (medium.tymed & TYMED_HGLOBAL.0 as u32) != 0 {
            let global = unsafe { medium.u.hGlobal };
            let size = unsafe { GlobalSize(global) };
            if size == 0 {
                return Ok(0);
            }
            let locked = unsafe { GlobalLock(global) };
            if locked.is_null() {
                return Err(PendingWindowsDropFailure::new(
                    format!("locking the HGLOBAL for item {index} ({name})"),
                    E_INVALIDARG.into(),
                ));
            }
            let bytes = unsafe { std::slice::from_raw_parts(locked.cast::<u8>(), size) };
            let write_result = file.write_all(bytes).and_then(|_| file.flush());
            let _ = unsafe { GlobalUnlock(global) };
            write_result.map_err(|error| {
                PendingWindowsDropFailure::new(
                    format!("writing the staged HGLOBAL for item {index} ({name})"),
                    windows::core::Error::new(E_FAIL, error.to_string()),
                )
            })?;
            Ok(size as u64)
        } else {
            Err(PendingWindowsDropFailure::new(
                format!("selecting supported storage media for item {index} ({name})"),
                DV_E_TYMED.into(),
            ))
        }
    })();
    unsafe { ReleaseStgMedium(&mut medium) };
    result
}

fn materialize_virtual_files(
    data_object: &IDataObject,
) -> windows::core::Result<VirtualDropStaging> {
    materialize_virtual_files_with_diagnostics(data_object).map_err(|failure| failure.error)
}

fn materialize_virtual_files_with_diagnostics(
    data_object: &IDataObject,
) -> std::result::Result<VirtualDropStaging, PendingWindowsDropFailure> {
    let descriptors = virtual_file_descriptors(data_object).map_err(|error| {
        PendingWindowsDropFailure::new("retrieving or parsing CFSTR_FILEDESCRIPTORW", error)
    })?;
    let mut staging = VirtualDropStaging::new().map_err(|error| {
        PendingWindowsDropFailure::new("creating the virtual-file staging directory", error)
    })?;
    for (index, descriptor) in descriptors.iter().enumerate() {
        let path = staging.root.join(&descriptor.name);
        let size = write_virtual_file_contents_with_diagnostics(
            data_object,
            index,
            &path,
            &descriptor.name,
        )?;
        if descriptor.expected_size.is_some_and(|expected| expected != size) {
            return Err(PendingWindowsDropFailure::new(
                format!(
                    "validating the size of CFSTR_FILECONTENTS item {index} ({})",
                    descriptor.name
                ),
                E_INVALIDARG.into(),
            ));
        }
        staging.paths.push(path);
    }
    log::trace!(
        "materialized {} virtual external-drop files in {}",
        staging.paths.len(),
        staging.root.display()
    );
    Ok(staging)
}

fn copy_deferred_hdrop_paths_with_shell(
    paths: &[PathBuf],
    destination: &Path,
    owner: HWND,
) -> windows::core::Result<()> {
    let destination = shell_item_for_path(destination)?;
    let sources = paths
        .iter()
        .map(|path| shell_item_for_path(path))
        .collect::<windows::core::Result<Vec<_>>>()?;
    let operation: IFileOperation = unsafe {
        CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)
    }?;
    if unsafe { IsWindow(Some(owner)).as_bool() } {
        unsafe { operation.SetOwnerWindow(owner)? };
    }
    for source in &sources {
        unsafe {
            operation.CopyItem(
                source,
                &destination,
                PCWSTR::null(),
                None::<&IFileOperationProgressSink>,
            )?;
        }
    }
    unsafe { operation.PerformOperations()? };
    if unsafe { operation.GetAnyOperationsAborted()? }.as_bool() {
        Err(E_ABORT.into())
    } else {
        Ok(())
    }
}

fn copy_paths_with_shell_diagnostics(
    paths: &[PathBuf],
    destination: &Path,
    owner: HWND,
) -> std::result::Result<(), PendingWindowsDropFailure> {
    let destination = shell_item_for_path(destination).map_err(|error| {
        PendingWindowsDropFailure::new("creating the destination Shell item", error)
    })?;
    let sources = paths
        .iter()
        .map(|path| {
            shell_item_for_path(path).map_err(|error| {
                PendingWindowsDropFailure::new(
                    format!("creating the source Shell item for {}", path.display()),
                    error,
                )
            })
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let operation: IFileOperation = unsafe {
        CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)
    }
    .map_err(|error| PendingWindowsDropFailure::new("creating IFileOperation", error))?;
    if unsafe { IsWindow(Some(owner)).as_bool() } {
        unsafe { operation.SetOwnerWindow(owner) }.map_err(|error| {
            PendingWindowsDropFailure::new("setting the IFileOperation owner window", error)
        })?;
    }
    for source in &sources {
        unsafe {
            operation.CopyItem(
                source,
                &destination,
                PCWSTR::null(),
                None::<&IFileOperationProgressSink>,
            )
        }
        .map_err(|error| {
            PendingWindowsDropFailure::new("queuing a source item in IFileOperation", error)
        })?;
    }
    unsafe { operation.PerformOperations() }.map_err(|error| {
        PendingWindowsDropFailure::new("performing IFileOperation", error)
    })?;
    let aborted = unsafe { operation.GetAnyOperationsAborted() }.map_err(|error| {
        PendingWindowsDropFailure::new("checking the IFileOperation result", error)
    })?;
    if aborted.as_bool() {
        Err(PendingWindowsDropFailure::new(
            "performing IFileOperation (the operation was cancelled)",
            E_ABORT.into(),
        ))
    } else {
        Ok(())
    }
}

enum PreparedSynchronousWindowsExternalDrop {
    Copied,
    Staged(VirtualDropStaging),
}

fn prepare_pending_windows_external_drop_synchronously(
    data_object: &IDataObject,
    destination: &Path,
    owner: HWND,
    allow_hdrop: bool,
) -> std::result::Result<
    PreparedSynchronousWindowsExternalDrop,
    PendingWindowsDropFailure,
> {
    if allow_hdrop && data_object_advertises_hdrop(data_object) {
        match materialize_hdrop_paths_once(data_object) {
            Ok(paths) => {
                log::debug!(
                    "pending external drop selected synchronous CF_HDROP pipeline ({} paths)",
                    paths.len()
                );
                copy_paths_with_shell_diagnostics(
                    &paths,
                    destination,
                    owner,
                )?;
                return Ok(PreparedSynchronousWindowsExternalDrop::Copied);
            }
            Err(error) => log::debug!(
                "synchronous CF_HDROP materialization failed ({error}); trying virtual files"
            ),
        }
    }

    log::debug!("pending external drop selected staged virtual-file pipeline");
    materialize_virtual_files_with_diagnostics(data_object)
        .map(PreparedSynchronousWindowsExternalDrop::Staged)
}

fn finish_immediate_windows_external_drop(
    active_drop: &ActiveWindowsExternalDrop,
    async_capability: Option<&IDataObjectAsyncCapability>,
    completion: oneshot::Sender<std::result::Result<(), String>>,
    operation: std::result::Result<(), PendingWindowsDropFailure>,
) {
    let (result, effect, user_result) = match operation {
        Ok(()) => (S_OK, DROPEFFECT_COPY, Ok(())),
        Err(failure) if failure.code() == E_ABORT => (E_ABORT, DROPEFFECT_NONE, Ok(())),
        Err(failure) => {
            let result = failure.code();
            (result, DROPEFFECT_NONE, Err(failure.user_message()))
        }
    };

    if let Some(capability) = async_capability {
        log::debug!("ending synchronously completed async drop with {result:?} and {effect:?}");
        unsafe {
            capability
                .EndOperation(result, None, effect.0 as u32)
                .log_err();
        }
    }
    if effect == DROPEFFECT_COPY {
        active_drop.completed_effect.set(Some(DROPEFFECT_COPY));
    }
    let _ = completion.send(user_result);
}

fn failed_pending_windows_drop_receiver(
    message: impl Into<String>,
) -> oneshot::Receiver<std::result::Result<(), String>> {
    let (completion, receiver) = oneshot::channel();
    let _ = completion.send(Err(message.into()));
    receiver
}

struct DeferredWindowsOperationCompletion {
    capability: IDataObjectAsyncCapability,
    completed: bool,
}

impl DeferredWindowsOperationCompletion {
    fn finish(mut self, result: HRESULT, effect: DROPEFFECT) {
        unsafe {
            self.capability
                .EndOperation(result, None, effect.0 as u32)
                .log_err();
        }
        self.completed = true;
    }
}

impl Drop for DeferredWindowsOperationCompletion {
    fn drop(&mut self) {
        if !self.completed {
            unsafe {
                self.capability
                    .EndOperation(E_FAIL, None, DROPEFFECT_NONE.0 as u32)
                    .log_err();
            }
        }
    }
}

fn run_deferred_windows_external_drop_worker(
    marshaled_data_object: MarshaledWindowsInterface,
    marshaled_async_capability: MarshaledWindowsInterface,
    destination: PathBuf,
    owner: HWND,
) -> windows::core::Result<()> {
    let capability =
        marshaled_async_capability.unmarshal::<IDataObjectAsyncCapability>()?;
    let completion = DeferredWindowsOperationCompletion {
        capability,
        completed: false,
    };
    let data_object = match marshaled_data_object.unmarshal::<IDataObject>() {
        Ok(data_object) => data_object,
        Err(error) => {
            completion.finish(error.code(), DROPEFFECT_NONE);
            return Err(error);
        }
    };
    let materialized_hdrop = materialize_deferred_hdrop_paths(&data_object);
    let operation = match materialized_hdrop {
        Ok(paths) => {
            log::trace!("pending external drop selected delayed CF_HDROP pipeline");
            copy_deferred_hdrop_paths_with_shell(&paths, &destination, owner)
        }
        Err(hdrop_error) => {
            log::trace!(
                "delayed CF_HDROP unavailable after Drop ({hdrop_error}); trying virtual files"
            );
            materialize_virtual_files(&data_object).and_then(|staging| {
                copy_deferred_hdrop_paths_with_shell(&staging.paths, &destination, owner)
            })
        }
    };
    match operation {
        Ok(()) => {
            log::trace!("pending external drop completed successfully");
            completion.finish(S_OK, DROPEFFECT_COPY);
            Ok(())
        }
        Err(error) => {
            log::trace!("pending external drop failed: {error}");
            let result = error.code();
            completion.finish(result, DROPEFFECT_NONE);
            Err(error)
        }
    }
}

fn start_pending_deferred_windows_external_drop(pending: PendingDeferredWindowsExternalDrop) {
    let PendingDeferredWindowsExternalDrop {
        work,
        destination,
        owner,
        completion,
    } = pending;
    match work {
        PendingWindowsExternalDropWork::LiveData {
            marshaled_data_object,
            marshaled_async_capability,
            async_capability,
        } => start_live_data_windows_external_drop(
            marshaled_data_object,
            marshaled_async_capability,
            async_capability,
            destination,
            owner,
            completion,
        ),
        PendingWindowsExternalDropWork::StagedFiles(staging) => {
            start_staged_windows_external_drop(staging, destination, owner, completion)
        }
    }
}

fn start_live_data_windows_external_drop(
    marshaled_data_object: MarshaledWindowsInterface,
    marshaled_async_capability: MarshaledWindowsInterface,
    async_capability: IDataObjectAsyncCapability,
    destination: PathBuf,
    owner: isize,
    completion: Option<oneshot::Sender<std::result::Result<(), String>>>,
) {
    let completion = Arc::new(Mutex::new(completion));
    let worker_completion = completion.clone();
    let result = std::thread::Builder::new()
        .name("gpui-deferred-file-drop".to_owned())
        .spawn(move || {
            let take_completion = || {
                worker_completion
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take()
            };
            let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            if let Err(error) = initialized.ok() {
                log::error!("failed to initialize deferred drop COM apartment: {error}");
                if let Some(completion) = take_completion() {
                    let _ = completion.send(Err(format!(
                        "Failed to initialize the file-transfer worker: {error} ({:?}).",
                        error.code()
                    )));
                }
                return;
            }
            let owner = HWND(owner as *mut _);
            let result = catch_unwind(AssertUnwindSafe(|| {
                run_deferred_windows_external_drop_worker(
                    marshaled_data_object,
                    marshaled_async_capability,
                    destination,
                    owner,
                )
            }));
            match result {
                Ok(Ok(())) => {
                    if let Some(completion) = take_completion() {
                        let _ = completion.send(Ok(()));
                    }
                }
                Ok(Err(error)) => {
                    log::error!("deferred external drop failed: {error}");
                    if let Some(completion) = take_completion() {
                        let outcome = if error.code() == E_ABORT {
                            Ok(())
                        } else {
                            Err(format!(
                                "The asynchronous file transfer failed: {error} ({:?}).",
                                error.code()
                            ))
                        };
                        let _ = completion.send(outcome);
                    }
                }
                Err(_) => {
                    log::error!("deferred external drop worker panicked");
                    if let Some(completion) = take_completion() {
                        let _ = completion.send(Err(
                            "The file-transfer worker stopped unexpectedly.".to_owned(),
                        ));
                    }
                }
            }
            unsafe { CoUninitialize() };
        });
    match result {
        Ok(_) => drop(async_capability),
        Err(error) => {
            log::error!("failed to start deferred external drop worker: {error}");
            unsafe {
                async_capability
                    .EndOperation(E_FAIL, None, DROPEFFECT_NONE.0 as u32)
                    .log_err();
            }
            if let Some(completion) = completion
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            {
                let _ = completion.send(Err(format!(
                    "Failed to start the file-transfer worker: {error}."
                )));
            }
        }
    }
}

fn start_staged_windows_external_drop(
    staging: VirtualDropStaging,
    destination: PathBuf,
    owner: isize,
    completion: Option<oneshot::Sender<std::result::Result<(), String>>>,
) {
    start_staged_windows_external_drop_with_spawn(
        staging,
        destination,
        owner,
        completion,
        |worker| {
            std::thread::Builder::new()
                .name("gpui-staged-file-drop".to_owned())
                .spawn(worker)
                .map(|_| ())
        },
    );
}

fn start_staged_windows_external_drop_with_spawn(
    staging: VirtualDropStaging,
    destination: PathBuf,
    owner: isize,
    completion: Option<oneshot::Sender<std::result::Result<(), String>>>,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<()>,
) {
    let completion = Arc::new(Mutex::new(completion));
    let worker_completion = completion.clone();
    let worker = Box::new(move || {
            let take_completion = || {
                worker_completion
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take()
            };
            let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            if let Err(error) = initialized.ok() {
                let failure = PendingWindowsDropFailure::new(
                    "initializing the staged-file Shell worker",
                    error,
                );
                log::error!("{}", failure.user_message());
                if let Some(completion) = take_completion() {
                    let _ = completion.send(Err(failure.user_message()));
                }
                return;
            }
            let owner = HWND(owner as *mut _);
            let operation = catch_unwind(AssertUnwindSafe(|| {
                copy_paths_with_shell_diagnostics(
                    &staging.paths,
                    &destination,
                    owner,
                )
            }));
            match operation {
                Ok(Ok(())) => {
                    if let Some(completion) = take_completion() {
                        let _ = completion.send(Ok(()));
                    }
                }
                Ok(Err(failure)) if failure.code() == E_ABORT => {
                    if let Some(completion) = take_completion() {
                        let _ = completion.send(Ok(()));
                    }
                }
                Ok(Err(failure)) => {
                    log::error!("{}", failure.user_message());
                    if let Some(completion) = take_completion() {
                        let _ = completion.send(Err(failure.user_message()));
                    }
                }
                Err(_) => {
                    log::error!("staged external drop worker panicked");
                    if let Some(completion) = take_completion() {
                        let _ = completion.send(Err(
                            "The staged file-transfer worker stopped unexpectedly.".to_owned(),
                        ));
                    }
                }
            }
            unsafe { CoUninitialize() };
        });
    let result = spawn(worker);
    if let Err(error) = result {
        let failure = PendingWindowsDropFailure::new(
            "starting the post-Drop staged-file worker",
            windows::core::Error::new(E_FAIL, error.to_string()),
        );
        log::error!("{}", failure.user_message());
        if let Some(completion) = completion
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = completion.send(Err(failure.user_message()));
        }
    }
}

fn queue_pending_windows_external_drop(
    active_drop: &ActiveWindowsExternalDrop,
    pending_drops: &RefCell<VecDeque<PendingDeferredWindowsExternalDrop>>,
    pending: PendingDeferredWindowsExternalDrop,
    hwnd: HWND,
    post: impl FnOnce(HWND) -> windows::core::Result<()>,
) -> std::result::Result<(), HRESULT> {
    pending_drops.borrow_mut().push_back(pending);
    if let Err(error) = post(hwnd) {
        let mut pending = pending_drops
            .borrow_mut()
            .pop_back()
            .expect("the pending drop was just queued");
        let result = error.code();
        if let PendingWindowsExternalDropWork::LiveData {
            async_capability, ..
        } = &pending.work
        {
            unsafe {
                async_capability
                    .EndOperation(result, None, DROPEFFECT_NONE.0 as u32)
                    .log_err();
            }
        }
        let failure = PendingWindowsDropFailure::new(
            "posting the post-Drop file-transfer worker",
            error,
        );
        if let Some(completion) = pending.completion.take() {
            let _ = completion.send(Err(failure.user_message()));
        }
        return Err(result);
    }
    active_drop.completed_effect.set(Some(DROPEFFECT_COPY));
    Ok(())
}

fn complete_prepared_synchronous_windows_external_drop(
    active_drop: &ActiveWindowsExternalDrop,
    pending_drops: &RefCell<VecDeque<PendingDeferredWindowsExternalDrop>>,
    destination: &Path,
    hwnd: HWND,
    completion: oneshot::Sender<std::result::Result<(), String>>,
    prepared: std::result::Result<
        PreparedSynchronousWindowsExternalDrop,
        PendingWindowsDropFailure,
    >,
    async_capability: Option<&IDataObjectAsyncCapability>,
    post: impl FnOnce(HWND) -> windows::core::Result<()>,
) {
    match prepared {
        Ok(PreparedSynchronousWindowsExternalDrop::Copied) => {
            finish_immediate_windows_external_drop(
                active_drop,
                async_capability,
                completion,
                Ok(()),
            );
        }
        Err(failure) => {
            finish_immediate_windows_external_drop(
                active_drop,
                async_capability,
                completion,
                Err(failure),
            );
        }
        Ok(PreparedSynchronousWindowsExternalDrop::Staged(staging)) => {
            let queued = queue_pending_windows_external_drop(
                active_drop,
                pending_drops,
                PendingDeferredWindowsExternalDrop {
                    work: PendingWindowsExternalDropWork::StagedFiles(staging),
                    destination: destination.to_path_buf(),
                    owner: hwnd.0 as isize,
                    completion: Some(completion),
                },
                hwnd,
                post,
            );
            if let Some(capability) = async_capability {
                let (result, effect) = match queued {
                    Ok(()) => (S_OK, DROPEFFECT_COPY),
                    Err(result) => (result, DROPEFFECT_NONE),
                };
                unsafe {
                    capability
                        .EndOperation(result, None, effect.0 as u32)
                        .log_err();
                }
            }
        }
    }
}

fn complete_active_deferred_windows_external_drop(
    active_drops: &RefCell<Vec<Rc<ActiveWindowsExternalDrop>>>,
    pending_drops: &RefCell<VecDeque<PendingDeferredWindowsExternalDrop>>,
    destination: &Path,
    hwnd: HWND,
) -> oneshot::Receiver<std::result::Result<(), String>> {
    complete_active_deferred_windows_external_drop_with_post(
        active_drops,
        pending_drops,
        destination,
        hwnd,
        |hwnd| unsafe {
            PostMessageW(
                Some(hwnd),
                WM_GPUI_START_DEFERRED_EXTERNAL_PATHS_DROP,
                WPARAM(0),
                LPARAM(0),
            )
        },
    )
}

fn complete_active_deferred_windows_external_drop_with_post(
    active_drops: &RefCell<Vec<Rc<ActiveWindowsExternalDrop>>>,
    pending_drops: &RefCell<VecDeque<PendingDeferredWindowsExternalDrop>>,
    destination: &Path,
    hwnd: HWND,
    post: impl FnOnce(HWND) -> windows::core::Result<()>,
) -> oneshot::Receiver<std::result::Result<(), String>> {
    let Some(active_drop) = active_drops.borrow().last().cloned() else {
        return failed_pending_windows_drop_receiver(
            "The pending file transfer was no longer active during Drop.",
        );
    };
    let Some(deferred) = active_drop.deferred.as_ref() else {
        return failed_pending_windows_drop_receiver(
            "The active drop did not contain a pending Windows file offer.",
        );
    };
    if !destination.is_absolute() || !destination.is_dir() {
        return failed_pending_windows_drop_receiver(format!(
            "The drop destination is not an absolute local directory: {}",
            destination.display()
        ));
    }
    if !deferred.allowed_effects.contains(DROPEFFECT_COPY) {
        return failed_pending_windows_drop_receiver(format!(
            "The source did not allow a copy operation ({:?}).",
            deferred.allowed_effects
        ));
    }

    let (completion_tx, completion_rx) = oneshot::channel();
    let Some(capability) = enabled_async_capability(&deferred.data_object) else {
        log::debug!("pending external drop is using synchronous materialization");
        let prepared = prepare_pending_windows_external_drop_synchronously(
            &deferred.data_object,
            destination,
            hwnd,
            true,
        );
        complete_prepared_synchronous_windows_external_drop(
            &active_drop,
            pending_drops,
            destination,
            hwnd,
            completion_tx,
            prepared,
            None,
            post,
        );
        return completion_rx;
    };
    if let Err(start_error) = unsafe { capability.StartOperation(None) } {
        log::debug!(
            "external drop StartOperation failed ({start_error}); using synchronous materialization"
        );
        let prepared = prepare_pending_windows_external_drop_synchronously(
            &deferred.data_object,
            destination,
            hwnd,
            true,
        );
        complete_prepared_synchronous_windows_external_drop(
            &active_drop,
            pending_drops,
            destination,
            hwnd,
            completion_tx,
            prepared,
            None,
            post,
        );
        return completion_rx;
    }
    log::trace!("external drop StartOperation succeeded");
    let marshaled_data_object = match MarshaledWindowsInterface::new(&deferred.data_object) {
        Ok(marshaled_data_object) => marshaled_data_object,
        Err(error) => {
            log::debug!(
                "failed to marshal pending drop data object ({error}); trying synchronous virtual files"
            );
            let prepared = prepare_pending_windows_external_drop_synchronously(
                &deferred.data_object,
                destination,
                hwnd,
                false,
            );
            complete_prepared_synchronous_windows_external_drop(
                &active_drop,
                pending_drops,
                destination,
                hwnd,
                completion_tx,
                prepared,
                Some(&capability),
                post,
            );
            return completion_rx;
        }
    };
    let marshaled_async_capability = match MarshaledWindowsInterface::new(&capability) {
        Ok(marshaled_async_capability) => marshaled_async_capability,
        Err(error) => {
            log::debug!(
                "failed to marshal pending drop async capability ({error}); trying synchronous virtual files"
            );
            let prepared = prepare_pending_windows_external_drop_synchronously(
                &deferred.data_object,
                destination,
                hwnd,
                false,
            );
            complete_prepared_synchronous_windows_external_drop(
                &active_drop,
                pending_drops,
                destination,
                hwnd,
                completion_tx,
                prepared,
                Some(&capability),
                post,
            );
            return completion_rx;
        }
    };
    let queued = queue_pending_windows_external_drop(
        &active_drop,
        pending_drops,
        PendingDeferredWindowsExternalDrop {
            work: PendingWindowsExternalDropWork::LiveData {
                marshaled_data_object,
                marshaled_async_capability,
                async_capability: capability,
            },
            destination: destination.to_path_buf(),
            owner: hwnd.0 as isize,
            completion: Some(completion_tx),
        },
        hwnd,
        post,
    );
    if queued.is_ok() {
        log::debug!("pending external drop was handed off to the asynchronous worker");
    }
    completion_rx
}

#[implement(IDropTarget)]
struct WindowsDragDropHandler(pub Rc<WindowsWindowInner>);

impl WindowsDragDropHandler {
    fn handle_drag_drop(&self, input: PlatformInput) {
        let mut lock = self.0.state.borrow_mut();
        if let Some(mut func) = lock.callbacks.input.take() {
            drop(lock);
            func(input);
            self.0.state.borrow_mut().callbacks.input = Some(func);
        }
    }
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for WindowsDragDropHandler_Impl {
    fn DragEnter(
        &self,
        pdataobj: windows::core::Ref<IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        unsafe {
            let idata_obj = pdataobj.ok()?;
            let cursor_position = POINT { x: pt.x, y: pt.y };
            if let Some(paths) = external_paths_from_data_object(idata_obj) {
                self.0.external_paths_drop_is_accepted.set(true);
                self.0
                    .external_paths_drop_is_deferred
                    .set(paths.is_pending_windows_drop());
                *pdweffect = DROPEFFECT_COPY;
                let mut cursor_position = cursor_position;
                ScreenToClient(self.0.hwnd, &mut cursor_position)
                    .ok()
                    .log_err();
                let scale_factor = self.0.state.borrow().scale_factor;
                let input = PlatformInput::FileDrop(FileDropEvent::Entered {
                    position: logical_point(
                        cursor_position.x as f32,
                        cursor_position.y as f32,
                        scale_factor,
                    ),
                    paths,
                });
                self.handle_drag_drop(input);
            } else {
                self.0.external_paths_drop_is_accepted.set(false);
                self.0.external_paths_drop_is_deferred.set(false);
                *pdweffect = DROPEFFECT_NONE;
            }
            self.0
                .drop_target_helper
                .DragEnter(self.0.hwnd, idata_obj, &cursor_position, *pdweffect)
                .log_err();
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let mut cursor_position = POINT { x: pt.x, y: pt.y };
        let accepted = self.0.external_paths_drop_is_accepted.get();
        unsafe {
            *pdweffect = if accepted { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
            self.0
                .drop_target_helper
                .DragOver(&cursor_position, *pdweffect)
                .log_err();
            ScreenToClient(self.0.hwnd, &mut cursor_position)
                .ok()
                .log_err();
        }
        if !accepted {
            return Ok(());
        }
        let scale_factor = self.0.state.borrow().scale_factor;
        let input = PlatformInput::FileDrop(FileDropEvent::Pending {
            position: logical_point(
                cursor_position.x as f32,
                cursor_position.y as f32,
                scale_factor,
            ),
        });
        self.handle_drag_drop(input);

        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        self.0.external_paths_drop_is_accepted.set(false);
        self.0.external_paths_drop_is_deferred.set(false);
        unsafe {
            self.0.drop_target_helper.DragLeave().log_err();
        }
        let input = PlatformInput::FileDrop(FileDropEvent::Exited);
        self.handle_drag_drop(input);

        Ok(())
    }

    fn Drop(
        &self,
        pdataobj: windows::core::Ref<IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let idata_obj = pdataobj.ok()?;
        let mut cursor_position = POINT { x: pt.x, y: pt.y };
        let accepted = self.0.external_paths_drop_is_accepted.replace(false);
        let deferred = self.0.external_paths_drop_is_deferred.replace(false);
        let allowed_effects = unsafe { *pdweffect };
        let default_effect = if accepted {
            default_external_drop_effect(deferred)
        } else {
            DROPEFFECT_NONE
        };
        unsafe {
            *pdweffect = default_effect;
            self.0
                .drop_target_helper
                .Drop(idata_obj, &cursor_position, *pdweffect)
                .log_err();
            ScreenToClient(self.0.hwnd, &mut cursor_position)
                .ok()
                .log_err();
        }
        let active_drop = Rc::new(ActiveWindowsExternalDrop {
            completed_effect: Cell::new(None),
            deferred: deferred.then(|| DeferredWindowsExternalDrop {
                data_object: idata_obj.clone(),
                allowed_effects,
            }),
        });
        let _active_drop_guard = push_active_windows_external_drop(
            &self.0.active_external_paths_drops,
            active_drop.clone(),
        );
        let scale_factor = self.0.state.borrow().scale_factor;
        let input = PlatformInput::FileDrop(FileDropEvent::Submit {
            position: logical_point(
                cursor_position.x as f32,
                cursor_position.y as f32,
                scale_factor,
            ),
        });
        self.handle_drag_drop(input);
        unsafe {
            *pdweffect = completed_external_drop_effect(&active_drop, default_effect);
        }

        Ok(())
    }
}

#[cfg(test)]
mod external_paths_drag_tests {
    use super::{
        ActiveWindowsExternalDrop, COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree,
        CoUninitialize, DROPFILES, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE,
        DROPEFFECT_NONE, DeferredWindowsExternalDrop, E_ABORT, E_FAIL, HWND, IDataObject,
        ILGetSize, PCWSTR, PendingExternalPathsDrag, SHParseDisplayName,
        WindowsFileDataObject, build_hdrop_payload,
        catch_windows_callback, complete_active_windows_external_drop,
        complete_active_deferred_windows_external_drop_with_post,
        copy_deferred_hdrop_paths_with_shell,
        completed_external_drop_effect, default_external_drop_effect,
        enabled_async_capability, external_paths_from_data_object,
        materialize_deferred_hdrop_paths, materialize_virtual_files,
        run_deferred_windows_external_drop_worker, MarshaledWindowsInterface,
        push_active_windows_external_drop, start_pending_deferred_windows_external_drop,
        windows_external_drag_completion, windows_external_drag_result,
    };
    use crate::{
        ExternalPathDragOperation, ExternalPaths, ExternalPathsDragResult,
        ExternalPathsDragStartResult,
    };
    use std::{
        cell::{Cell, RefCell},
        collections::VecDeque,
        ffi::OsStr,
        mem,
        path::{Path, PathBuf},
        rc::Rc,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestComApartment(bool);

    impl TestComApartment {
        fn new() -> Self {
            Self(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok())
        }
    }

    impl Drop for TestComApartment {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    struct TestTempDir(PathBuf);

    impl TestTempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "gpui-shell-drop-{}-{}",
                std::process::id(),
                NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestTempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[windows::core::implement(IDataObject)]
    struct DataObjectWithoutAsyncCapability {
        inner: IDataObject,
    }

    #[allow(non_snake_case)]
    impl super::IDataObject_Impl for DataObjectWithoutAsyncCapability_Impl {
        fn GetData(
            &self,
            format: *const super::FORMATETC,
        ) -> windows::core::Result<super::STGMEDIUM> {
            unsafe { self.inner.GetData(format) }
        }

        fn GetDataHere(
            &self,
            _format: *const super::FORMATETC,
            _medium: *mut super::STGMEDIUM,
        ) -> windows::core::Result<()> {
            Err(super::E_NOTIMPL.into())
        }

        fn QueryGetData(&self, format: *const super::FORMATETC) -> windows::core::HRESULT {
            unsafe { self.inner.QueryGetData(format) }
        }

        fn GetCanonicalFormatEtc(
            &self,
            _format_in: *const super::FORMATETC,
            _format_out: *mut super::FORMATETC,
        ) -> windows::core::HRESULT {
            super::E_NOTIMPL
        }

        fn SetData(
            &self,
            _format: *const super::FORMATETC,
            _medium: *const super::STGMEDIUM,
            _release: super::BOOL,
        ) -> windows::core::Result<()> {
            Err(super::E_NOTIMPL.into())
        }

        fn EnumFormatEtc(
            &self,
            direction: u32,
        ) -> windows::core::Result<super::IEnumFORMATETC> {
            unsafe { self.inner.EnumFormatEtc(direction) }
        }

        fn DAdvise(
            &self,
            _format: *const super::FORMATETC,
            _flags: u32,
            _sink: windows::core::Ref<'_, super::IAdviseSink>,
        ) -> windows::core::Result<u32> {
            Err(super::OLE_E_ADVISENOTSUPPORTED.into())
        }

        fn DUnadvise(&self, _connection: u32) -> windows::core::Result<()> {
            Err(super::OLE_E_ADVISENOTSUPPORTED.into())
        }

        fn EnumDAdvise(&self) -> windows::core::Result<super::IEnumSTATDATA> {
            Err(super::OLE_E_ADVISENOTSUPPORTED.into())
        }
    }

    fn without_async_capability(inner: IDataObject) -> IDataObject {
        DataObjectWithoutAsyncCapability { inner }.into()
    }

    fn start_queued_external_drop(
        pending: &RefCell<VecDeque<super::PendingDeferredWindowsExternalDrop>>,
    ) {
        let queued = pending
            .borrow_mut()
            .pop_front()
            .expect("a post-Drop operation should be queued");
        start_pending_deferred_windows_external_drop(queued);
    }

    fn pidl_bytes(parsing_name: &OsStr) -> Vec<u8> {
        use std::os::windows::ffi::OsStrExt;

        let parsing_name = parsing_name
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut pidl = std::ptr::null_mut();
        unsafe {
            SHParseDisplayName(PCWSTR(parsing_name.as_ptr()), None, &mut pidl, 0, None).unwrap();
            let size = ILGetSize(Some(pidl)) as usize;
            let bytes = std::slice::from_raw_parts(pidl.cast::<u8>(), size).to_vec();
            CoTaskMemFree(Some(pidl.cast()));
            bytes
        }
    }

    fn shell_id_list_payload(items: &[Vec<u8>]) -> Vec<u8> {
        let header_size = size_of::<u32>() * (items.len() + 2);
        let total_size = header_size + size_of::<u16>() + items.iter().map(Vec::len).sum::<usize>();
        let mut payload = vec![0; total_size];
        payload[..size_of::<u32>()].copy_from_slice(&(items.len() as u32).to_ne_bytes());

        let mut offset = header_size;
        payload[size_of::<u32>()..size_of::<u32>() * 2]
            .copy_from_slice(&(offset as u32).to_ne_bytes());
        offset += size_of::<u16>();
        for (index, item) in items.iter().enumerate() {
            let offset_start = size_of::<u32>() * (index + 2);
            payload[offset_start..offset_start + size_of::<u32>()]
                .copy_from_slice(&(offset as u32).to_ne_bytes());
            payload[offset..offset + item.len()].copy_from_slice(item);
            offset += item.len();
        }
        payload
    }

    fn test_data_object(
        hdrop_paths: Option<Vec<PathBuf>>,
        shell_id_list: Option<Vec<u8>>,
    ) -> IDataObject {
        let test_offer_hdrop = hdrop_paths.is_some();
        WindowsFileDataObject {
            paths: hdrop_paths.unwrap_or_default(),
            preferred_effect: DROPEFFECT_COPY,
            performed_effect: Rc::new(Cell::new(DROPEFFECT_NONE)),
            logical_performed_effect: Rc::new(Cell::new(DROPEFFECT_NONE)),
            test_offer_hdrop,
            test_query_hdrop: test_offer_hdrop,
            test_shell_id_list: shell_id_list,
            test_fail_shell_get_data: false,
            test_fail_hdrop_get_data: false,
            test_hdrop_failures_remaining: None,
            test_async_mode: false,
            test_start_error: None,
            test_call_order: None,
            test_start_count: None,
            test_end_events: None,
            test_virtual_files: None,
            test_chromium_virtual_descriptor: false,
            test_virtual_directory: false,
            test_malformed_virtual_descriptors: false,
        }
        .into()
    }

    fn delayed_hdrop_test_data_object_with(
        async_mode: bool,
        query_hdrop: bool,
        call_order: Option<Rc<RefCell<Vec<&'static str>>>>,
    ) -> IDataObject {
        WindowsFileDataObject {
            paths: Vec::new(),
            preferred_effect: DROPEFFECT_COPY,
            performed_effect: Rc::new(Cell::new(DROPEFFECT_NONE)),
            logical_performed_effect: Rc::new(Cell::new(DROPEFFECT_NONE)),
            test_offer_hdrop: true,
            test_query_hdrop: query_hdrop,
            test_shell_id_list: None,
            test_fail_shell_get_data: false,
            test_fail_hdrop_get_data: true,
            test_hdrop_failures_remaining: None,
            test_async_mode: async_mode,
            test_start_error: None,
            test_call_order: call_order,
            test_start_count: None,
            test_end_events: None,
            test_virtual_files: None,
            test_chromium_virtual_descriptor: false,
            test_virtual_directory: false,
            test_malformed_virtual_descriptors: false,
        }
        .into()
    }

    fn malformed_test_data_object() -> IDataObject {
        WindowsFileDataObject {
            paths: Vec::new(),
            preferred_effect: DROPEFFECT_COPY,
            performed_effect: Rc::new(Cell::new(DROPEFFECT_NONE)),
            logical_performed_effect: Rc::new(Cell::new(DROPEFFECT_NONE)),
            test_offer_hdrop: false,
            test_query_hdrop: false,
            test_shell_id_list: Some(Vec::new()),
            test_fail_shell_get_data: true,
            test_fail_hdrop_get_data: false,
            test_hdrop_failures_remaining: None,
            test_async_mode: false,
            test_start_error: None,
            test_call_order: None,
            test_start_count: None,
            test_end_events: None,
            test_virtual_files: None,
            test_chromium_virtual_descriptor: false,
            test_virtual_directory: false,
            test_malformed_virtual_descriptors: false,
        }
        .into()
    }

    fn lifecycle_delayed_hdrop_test_data_object(
        start_count: Rc<Cell<usize>>,
        end_events: Rc<RefCell<Vec<(windows::core::HRESULT, u32)>>>,
    ) -> IDataObject {
        WindowsFileDataObject {
            paths: Vec::new(),
            preferred_effect: DROPEFFECT_COPY,
            performed_effect: Rc::new(Cell::new(DROPEFFECT_NONE)),
            logical_performed_effect: Rc::new(Cell::new(DROPEFFECT_NONE)),
            test_offer_hdrop: true,
            test_query_hdrop: true,
            test_shell_id_list: None,
            test_fail_shell_get_data: false,
            test_fail_hdrop_get_data: true,
            test_hdrop_failures_remaining: None,
            test_async_mode: true,
            test_start_error: None,
            test_call_order: None,
            test_start_count: Some(start_count),
            test_end_events: Some(end_events),
            test_virtual_files: None,
            test_chromium_virtual_descriptor: false,
            test_virtual_directory: false,
            test_malformed_virtual_descriptors: false,
        }
        .into()
    }

    fn transient_hdrop_test_data_object(
        paths: Vec<PathBuf>,
        failures_remaining: Rc<Cell<usize>>,
    ) -> IDataObject {
        WindowsFileDataObject {
            paths,
            preferred_effect: DROPEFFECT_COPY,
            performed_effect: Rc::new(Cell::new(DROPEFFECT_NONE)),
            logical_performed_effect: Rc::new(Cell::new(DROPEFFECT_NONE)),
            test_offer_hdrop: true,
            test_query_hdrop: true,
            test_shell_id_list: None,
            test_fail_shell_get_data: false,
            test_fail_hdrop_get_data: false,
            test_hdrop_failures_remaining: Some(failures_remaining),
            test_async_mode: true,
            test_start_error: None,
            test_call_order: None,
            test_start_count: None,
            test_end_events: None,
            test_virtual_files: None,
            test_chromium_virtual_descriptor: false,
            test_virtual_directory: false,
            test_malformed_virtual_descriptors: false,
        }
        .into()
    }

    fn virtual_file_test_data_object(
        files: Vec<(String, Vec<u8>, u8)>,
        directory: bool,
        malformed: bool,
    ) -> IDataObject {
        virtual_file_test_data_object_with_async_mode(
            files, directory, malformed, false, true, None,
        )
    }

    fn virtual_file_test_data_object_with_async_mode(
        files: Vec<(String, Vec<u8>, u8)>,
        directory: bool,
        malformed: bool,
        chromium_style: bool,
        async_mode: bool,
        start_error: Option<windows::core::HRESULT>,
    ) -> IDataObject {
        WindowsFileDataObject {
            paths: Vec::new(),
            preferred_effect: DROPEFFECT_COPY,
            performed_effect: Rc::new(Cell::new(DROPEFFECT_NONE)),
            logical_performed_effect: Rc::new(Cell::new(DROPEFFECT_NONE)),
            test_offer_hdrop: false,
            test_query_hdrop: false,
            test_shell_id_list: None,
            test_fail_shell_get_data: false,
            test_fail_hdrop_get_data: false,
            test_hdrop_failures_remaining: None,
            test_async_mode: async_mode,
            test_start_error: start_error,
            test_call_order: None,
            test_start_count: None,
            test_end_events: None,
            test_virtual_files: Some(files),
            test_chromium_virtual_descriptor: chromium_style,
            test_virtual_directory: directory,
            test_malformed_virtual_descriptors: malformed,
        }
        .into()
    }

    fn virtual_file_lifecycle_test_data_object(
        files: Vec<(String, Vec<u8>, u8)>,
        start_error: Option<windows::core::HRESULT>,
        start_count: Rc<Cell<usize>>,
        end_events: Rc<RefCell<Vec<(windows::core::HRESULT, u32)>>>,
    ) -> IDataObject {
        WindowsFileDataObject {
            paths: Vec::new(),
            preferred_effect: DROPEFFECT_COPY,
            performed_effect: Rc::new(Cell::new(DROPEFFECT_NONE)),
            logical_performed_effect: Rc::new(Cell::new(DROPEFFECT_NONE)),
            test_offer_hdrop: false,
            test_query_hdrop: false,
            test_shell_id_list: None,
            test_fail_shell_get_data: false,
            test_fail_hdrop_get_data: false,
            test_hdrop_failures_remaining: None,
            test_async_mode: true,
            test_start_error: start_error,
            test_call_order: None,
            test_start_count: Some(start_count),
            test_end_events: Some(end_events),
            test_virtual_files: Some(files),
            test_chromium_virtual_descriptor: false,
            test_virtual_directory: false,
            test_malformed_virtual_descriptors: false,
        }
        .into()
    }

    fn ordered_hdrop_test_data_object(
        path: PathBuf,
        call_order: Rc<RefCell<Vec<&'static str>>>,
        end_events: Rc<RefCell<Vec<(windows::core::HRESULT, u32)>>>,
    ) -> IDataObject {
        WindowsFileDataObject {
            paths: vec![path],
            preferred_effect: DROPEFFECT_COPY,
            performed_effect: Rc::new(Cell::new(DROPEFFECT_NONE)),
            logical_performed_effect: Rc::new(Cell::new(DROPEFFECT_NONE)),
            test_offer_hdrop: true,
            test_query_hdrop: true,
            test_shell_id_list: None,
            test_fail_shell_get_data: false,
            test_fail_hdrop_get_data: false,
            test_hdrop_failures_remaining: None,
            test_async_mode: true,
            test_start_error: None,
            test_call_order: Some(call_order),
            test_start_count: None,
            test_end_events: Some(end_events),
            test_virtual_files: None,
            test_chromium_virtual_descriptor: false,
            test_virtual_directory: false,
            test_malformed_virtual_descriptors: false,
        }
        .into()
    }

    fn hdrop_paths_from_payload(payload: &[u8]) -> Vec<String> {
        let path_bytes = &payload[mem::size_of::<DROPFILES>()..];
        let path_words = path_bytes
            .chunks_exact(mem::size_of::<u16>())
            .map(|bytes| u16::from_ne_bytes([bytes[0], bytes[1]]))
            .collect::<Vec<_>>();

        path_words
            .split(|word| *word == 0)
            .take_while(|path| !path.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }

    #[test]
    fn hdrop_payload_encodes_single_path() {
        let payload = build_hdrop_payload(&[PathBuf::from(r"C:\Users\test\file.txt")]);

        let pfiles = u32::from_ne_bytes(payload[0..4].try_into().unwrap());
        assert_eq!(pfiles, mem::size_of::<DROPFILES>() as u32);
        assert_eq!(hdrop_paths_from_payload(&payload), [r"C:\Users\test\file.txt"]);
        assert_eq!(&payload[payload.len() - 4..], &[0, 0, 0, 0]);
    }

    #[test]
    fn hdrop_payload_encodes_multiple_paths() {
        let payload = build_hdrop_payload(&[
            PathBuf::from(r"C:\Users\test\one.txt"),
            PathBuf::from(r"C:\Users\test\two.txt"),
        ]);

        assert_eq!(
            hdrop_paths_from_payload(&payload),
            [r"C:\Users\test\one.txt", r"C:\Users\test\two.txt"]
        );
        assert_eq!(&payload[payload.len() - 4..], &[0, 0, 0, 0]);
    }

    #[test]
    fn pidl_only_filesystem_data_object_becomes_external_paths() {
        let _com = TestComApartment::new();
        let temp = TestTempDir::new();
        let first = temp.path().join("first.txt");
        let second = temp.path().join("second");
        std::fs::write(&first, b"first").unwrap();
        std::fs::create_dir(&second).unwrap();
        let payload = shell_id_list_payload(&[
            pidl_bytes(first.as_os_str()),
            pidl_bytes(second.as_os_str()),
        ]);
        let data_object = test_data_object(None, Some(payload));

        let paths = external_paths_from_data_object(&data_object).unwrap();
        assert_eq!(paths.paths(), [first, second]);
    }

    #[test]
    fn hdrop_takes_precedence_when_shell_id_list_is_also_available() {
        let _com = TestComApartment::new();
        let temp = TestTempDir::new();
        let shell_path = temp.path().join("shell.txt");
        std::fs::write(&shell_path, b"shell").unwrap();
        let hdrop_path = PathBuf::from(r"C:\preferred-hdrop.txt");
        let data_object = test_data_object(
            Some(vec![hdrop_path.clone()]),
            Some(shell_id_list_payload(&[pidl_bytes(shell_path.as_os_str())])),
        );

        let paths = external_paths_from_data_object(&data_object).unwrap();
        assert_eq!(paths.paths(), [hdrop_path]);
    }

    #[test]
    fn shell_filesystem_items_are_used_when_materialized_hdrop_is_invalid() {
        let _com = TestComApartment::new();
        let temp = TestTempDir::new();
        let shell_path = temp.path().join("shell-fallback.txt");
        std::fs::write(&shell_path, b"shell").unwrap();
        let data_object = test_data_object(
            Some(Vec::new()),
            Some(shell_id_list_payload(&[pidl_bytes(shell_path.as_os_str())])),
        );

        let paths = external_paths_from_data_object(&data_object).unwrap();
        assert_eq!(paths.paths(), [shell_path]);
    }

    #[test]
    fn advertised_unmaterialized_hdrop_is_pending_without_async_probe() {
        let _com = TestComApartment::new();
        let call_order = Rc::new(RefCell::new(Vec::new()));
        let pending = external_paths_from_data_object(&delayed_hdrop_test_data_object_with(
            false,
            true,
            Some(call_order.clone()),
        ))
        .expect("advertised delayed CF_HDROP should be accepted");

        assert!(pending.is_pending_windows_drop());
        assert!(pending.paths().is_empty());
        assert!(!pending.is_empty());
        assert_eq!(call_order.borrow().as_slice(), ["get_data"]);
    }

    #[test]
    fn enumerated_hdrop_can_be_deferred_when_query_get_data_disagrees() {
        let _com = TestComApartment::new();
        let deferred = external_paths_from_data_object(&delayed_hdrop_test_data_object_with(
            true, false, None,
        ))
        .expect("enumerated delayed CF_HDROP should be accepted");

        assert!(deferred.is_pending_windows_drop());
    }

    #[test]
    fn async_capability_is_not_queried_during_admission() {
        let _com = TestComApartment::new();
        let call_order = Rc::new(RefCell::new(Vec::new()));
        let deferred = external_paths_from_data_object(&delayed_hdrop_test_data_object_with(
            true,
            true,
            Some(call_order.clone()),
        ))
        .expect("async delayed CF_HDROP should be accepted");

        assert!(deferred.is_pending_windows_drop());
        assert_eq!(call_order.borrow().as_slice(), ["get_data"]);
    }

    #[test]
    fn successful_empty_hdrop_is_not_treated_as_deferred() {
        let _com = TestComApartment::new();
        let empty_hdrop = test_data_object(Some(Vec::new()), None);

        assert!(external_paths_from_data_object(&empty_hdrop).is_none());
    }

    #[test]
    fn deferred_completion_starts_and_queues_before_publishing_copy() {
        let _com = TestComApartment::new();
        let temp = TestTempDir::new();
        let start_count = Rc::new(Cell::new(0));
        let end_events = Rc::new(RefCell::new(Vec::new()));
        let active = RefCell::new(Vec::new());
        let pending = RefCell::new(VecDeque::new());
        let drop = Rc::new(ActiveWindowsExternalDrop {
            completed_effect: Cell::new(None),
            deferred: Some(DeferredWindowsExternalDrop {
                data_object: lifecycle_delayed_hdrop_test_data_object(
                    start_count.clone(),
                    end_events.clone(),
                ),
                allowed_effects: DROPEFFECT_COPY,
            }),
        });
        let _guard = push_active_windows_external_drop(&active, drop.clone());

        let completion = complete_active_deferred_windows_external_drop_with_post(
            &active,
            &pending,
            temp.path(),
            HWND::default(),
            |_| Ok(()),
        );
        assert_eq!(start_count.get(), 1);
        assert_eq!(
            completed_external_drop_effect(&drop, DROPEFFECT_NONE),
            DROPEFFECT_COPY
        );
        assert_eq!(pending.borrow().len(), 1);
        assert!(end_events.borrow().is_empty());

        pending.borrow_mut().pop_front().unwrap().cancel(E_ABORT);
        assert!(futures::executor::block_on(completion).unwrap().is_err());
        assert_eq!(
            end_events.borrow().as_slice(),
            [(E_ABORT, DROPEFFECT_NONE.0 as u32)]
        );
    }

    #[test]
    fn deferred_completion_cancels_when_posting_fails() {
        let _com = TestComApartment::new();
        let temp = TestTempDir::new();
        let start_count = Rc::new(Cell::new(0));
        let end_events = Rc::new(RefCell::new(Vec::new()));
        let active = RefCell::new(Vec::new());
        let pending = RefCell::new(VecDeque::new());
        let drop = Rc::new(ActiveWindowsExternalDrop {
            completed_effect: Cell::new(None),
            deferred: Some(DeferredWindowsExternalDrop {
                data_object: lifecycle_delayed_hdrop_test_data_object(
                    start_count.clone(),
                    end_events.clone(),
                ),
                allowed_effects: DROPEFFECT_COPY,
            }),
        });
        let _guard = push_active_windows_external_drop(&active, drop.clone());

        let completion = complete_active_deferred_windows_external_drop_with_post(
            &active,
            &pending,
            temp.path(),
            HWND::default(),
            |_| Err(E_FAIL.into()),
        );
        assert_eq!(start_count.get(), 1);
        assert_eq!(
            completed_external_drop_effect(&drop, DROPEFFECT_NONE),
            DROPEFFECT_NONE
        );
        assert!(pending.borrow().is_empty());
        let error = futures::executor::block_on(completion)
            .unwrap()
            .unwrap_err();
        assert!(
            error.contains("posting the post-Drop file-transfer worker"),
            "{error}"
        );
        assert_eq!(end_events.borrow().len(), 1);
        assert_ne!(end_events.borrow()[0].0, super::S_OK);
        assert_eq!(end_events.borrow()[0].1, DROPEFFECT_NONE.0 as u32);
    }

    #[test]
    fn virtual_files_without_async_capability_complete_synchronously() {
        let _com = TestComApartment::new();
        let destination = TestTempDir::new();
        let stream_bytes = b"synchronous stream image".to_vec();
        let global_bytes = b"synchronous global image".to_vec();
        let data_object = without_async_capability(virtual_file_test_data_object(
            vec![
                ("stream.png".to_owned(), stream_bytes.clone(), 0),
                ("global.jpg".to_owned(), global_bytes.clone(), 1),
            ],
            false,
            false,
        ));
        assert!(enabled_async_capability(&data_object).is_none());
        assert!(
            external_paths_from_data_object(&data_object)
                .unwrap()
                .is_pending_windows_drop()
        );

        let active = RefCell::new(Vec::new());
        let pending = RefCell::new(VecDeque::new());
        let drop = Rc::new(ActiveWindowsExternalDrop {
            completed_effect: Cell::new(None),
            deferred: Some(DeferredWindowsExternalDrop {
                data_object,
                allowed_effects: DROPEFFECT_COPY,
            }),
        });
        let _guard = push_active_windows_external_drop(&active, drop.clone());
        let completion = complete_active_deferred_windows_external_drop_with_post(
            &active,
            &pending,
            destination.path(),
            HWND::default(),
            |_| Ok(()),
        );

        assert_eq!(pending.borrow().len(), 1);
        assert!(!destination.path().join("stream.png").exists());
        assert_eq!(
            completed_external_drop_effect(&drop, DROPEFFECT_NONE),
            DROPEFFECT_COPY
        );
        start_queued_external_drop(&pending);
        futures::executor::block_on(completion).unwrap().unwrap();
        assert_eq!(
            std::fs::read(destination.path().join("stream.png")).unwrap(),
            stream_bytes
        );
        assert_eq!(
            std::fs::read(destination.path().join("global.jpg")).unwrap(),
            global_bytes
        );
    }

    #[test]
    fn chromium_virtual_file_is_staged_during_drop_and_copied_afterward() {
        let _com = TestComApartment::new();
        let destination = TestTempDir::new();
        let suggested_name = "chrome-suggested.png";
        let bytes = b"chromium indexed stream contents".to_vec();
        let data_object = without_async_capability(
            virtual_file_test_data_object_with_async_mode(
                vec![(suggested_name.to_owned(), bytes.clone(), 0)],
                false,
                false,
                true,
                false,
                None,
            ),
        );
        assert!(enabled_async_capability(&data_object).is_none());
        assert!(
            external_paths_from_data_object(&data_object)
                .unwrap()
                .is_pending_windows_drop()
        );

        let active = RefCell::new(Vec::new());
        let pending = RefCell::new(VecDeque::new());
        let drop = Rc::new(ActiveWindowsExternalDrop {
            completed_effect: Cell::new(None),
            deferred: Some(DeferredWindowsExternalDrop {
                data_object,
                allowed_effects: DROPEFFECT_COPY,
            }),
        });
        let _guard = push_active_windows_external_drop(&active, drop.clone());
        let completion = complete_active_deferred_windows_external_drop_with_post(
            &active,
            &pending,
            destination.path(),
            HWND::default(),
            |_| Ok(()),
        );

        assert_eq!(pending.borrow().len(), 1);
        assert!(!destination.path().join(suggested_name).exists());
        assert_eq!(
            completed_external_drop_effect(&drop, DROPEFFECT_NONE),
            DROPEFFECT_COPY
        );

        start_queued_external_drop(&pending);
        futures::executor::block_on(completion).unwrap().unwrap();
        assert_eq!(
            std::fs::read(destination.path().join(suggested_name)).unwrap(),
            bytes
        );
    }

    #[test]
    fn staged_worker_start_failure_reports_stage_and_cleans_up() {
        let _com = TestComApartment::new();
        let destination = TestTempDir::new();
        let data_object = virtual_file_test_data_object(
            vec![("staged.png".to_owned(), b"staged bytes".to_vec(), 0)],
            false,
            false,
        );
        let staging = materialize_virtual_files(&data_object).unwrap();
        let staging_root = staging.root.clone();
        let (completion, receiver) = futures::channel::oneshot::channel();

        super::start_staged_windows_external_drop_with_spawn(
            staging,
            destination.path().to_path_buf(),
            0,
            Some(completion),
            |_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "simulated worker startup failure",
                ))
            },
        );

        let error = futures::executor::block_on(receiver)
            .unwrap()
            .unwrap_err();
        assert!(
            error.contains("starting the post-Drop staged-file worker"),
            "{error}"
        );
        assert!(error.contains("HRESULT(0x80004005)"), "{error}");
        assert!(!staging_root.exists());
    }

    #[test]
    fn virtual_files_with_disabled_async_mode_complete_synchronously() {
        let _com = TestComApartment::new();
        let destination = TestTempDir::new();
        let bytes = b"disabled async image".to_vec();
        let data_object = virtual_file_test_data_object_with_async_mode(
            vec![("disabled.png".to_owned(), bytes.clone(), 0)],
            false,
            false,
            false,
            false,
            None,
        );
        assert!(enabled_async_capability(&data_object).is_none());

        let active = RefCell::new(Vec::new());
        let pending = RefCell::new(VecDeque::new());
        let drop = Rc::new(ActiveWindowsExternalDrop {
            completed_effect: Cell::new(None),
            deferred: Some(DeferredWindowsExternalDrop {
                data_object,
                allowed_effects: DROPEFFECT_COPY,
            }),
        });
        let _guard = push_active_windows_external_drop(&active, drop.clone());
        let completion = complete_active_deferred_windows_external_drop_with_post(
            &active,
            &pending,
            destination.path(),
            HWND::default(),
            |_| Ok(()),
        );

        assert_eq!(pending.borrow().len(), 1);
        assert!(!destination.path().join("disabled.png").exists());
        assert_eq!(
            completed_external_drop_effect(&drop, DROPEFFECT_NONE),
            DROPEFFECT_COPY
        );
        start_queued_external_drop(&pending);
        futures::executor::block_on(completion).unwrap().unwrap();
        assert_eq!(
            std::fs::read(destination.path().join("disabled.png")).unwrap(),
            bytes
        );
    }

    #[test]
    fn start_operation_failure_falls_back_without_end_operation() {
        let _com = TestComApartment::new();
        let destination = TestTempDir::new();
        let bytes = b"start failure fallback".to_vec();
        let start_count = Rc::new(Cell::new(0));
        let end_events = Rc::new(RefCell::new(Vec::new()));
        let data_object = virtual_file_lifecycle_test_data_object(
            vec![("fallback.png".to_owned(), bytes.clone(), 0)],
            Some(E_FAIL),
            start_count.clone(),
            end_events.clone(),
        );

        let active = RefCell::new(Vec::new());
        let pending = RefCell::new(VecDeque::new());
        let drop = Rc::new(ActiveWindowsExternalDrop {
            completed_effect: Cell::new(None),
            deferred: Some(DeferredWindowsExternalDrop {
                data_object,
                allowed_effects: DROPEFFECT_COPY,
            }),
        });
        let _guard = push_active_windows_external_drop(&active, drop.clone());
        let completion = complete_active_deferred_windows_external_drop_with_post(
            &active,
            &pending,
            destination.path(),
            HWND::default(),
            |_| Ok(()),
        );

        assert_eq!(start_count.get(), 1);
        assert!(end_events.borrow().is_empty());
        assert_eq!(pending.borrow().len(), 1);
        assert!(!destination.path().join("fallback.png").exists());
        assert_eq!(
            completed_external_drop_effect(&drop, DROPEFFECT_NONE),
            DROPEFFECT_COPY
        );
        start_queued_external_drop(&pending);
        futures::executor::block_on(completion).unwrap().unwrap();
        assert_eq!(
            std::fs::read(destination.path().join("fallback.png")).unwrap(),
            bytes
        );
    }

    #[test]
    fn posting_failure_reports_stage_and_ends_async_operation_once() {
        let _com = TestComApartment::new();
        let destination = TestTempDir::new();
        let bytes = b"post failure fallback".to_vec();
        let start_count = Rc::new(Cell::new(0));
        let end_events = Rc::new(RefCell::new(Vec::new()));
        let data_object = virtual_file_lifecycle_test_data_object(
            vec![("post-fallback.png".to_owned(), bytes.clone(), 0)],
            None,
            start_count.clone(),
            end_events.clone(),
        );

        let active = RefCell::new(Vec::new());
        let pending = RefCell::new(VecDeque::new());
        let drop = Rc::new(ActiveWindowsExternalDrop {
            completed_effect: Cell::new(None),
            deferred: Some(DeferredWindowsExternalDrop {
                data_object,
                allowed_effects: DROPEFFECT_COPY,
            }),
        });
        let _guard = push_active_windows_external_drop(&active, drop.clone());
        let completion = complete_active_deferred_windows_external_drop_with_post(
            &active,
            &pending,
            destination.path(),
            HWND::default(),
            |_| Err(E_FAIL.into()),
        );

        let error = futures::executor::block_on(completion)
            .unwrap()
            .unwrap_err();
        assert!(
            error.contains("posting the post-Drop file-transfer worker"),
            "{error}"
        );
        assert_eq!(start_count.get(), 1);
        assert!(pending.borrow().is_empty());
        assert_eq!(
            end_events.borrow().as_slice(),
            [(E_FAIL, DROPEFFECT_NONE.0 as u32)]
        );
        assert_eq!(
            completed_external_drop_effect(&drop, DROPEFFECT_NONE),
            DROPEFFECT_NONE
        );
        assert!(!destination.path().join("post-fallback.png").exists());
    }

    #[test]
    fn disabled_async_mode_materializes_hdrop_synchronously_before_virtual_files() {
        let _com = TestComApartment::new();
        let source_directory = TestTempDir::new();
        let destination = TestTempDir::new();
        let source = source_directory.path().join("source.png");
        std::fs::write(&source, b"synchronous hdrop").unwrap();
        let data_object = test_data_object(Some(vec![source]), None);

        let active = RefCell::new(Vec::new());
        let pending = RefCell::new(VecDeque::new());
        let drop = Rc::new(ActiveWindowsExternalDrop {
            completed_effect: Cell::new(None),
            deferred: Some(DeferredWindowsExternalDrop {
                data_object,
                allowed_effects: DROPEFFECT_COPY,
            }),
        });
        let _guard = push_active_windows_external_drop(&active, drop.clone());
        let completion = complete_active_deferred_windows_external_drop_with_post(
            &active,
            &pending,
            destination.path(),
            HWND::default(),
            |_| panic!("synchronous drop must not post a worker message"),
        );

        futures::executor::block_on(completion).unwrap().unwrap();
        assert_eq!(
            completed_external_drop_effect(&drop, DROPEFFECT_NONE),
            DROPEFFECT_COPY
        );
        assert_eq!(
            std::fs::read(destination.path().join("source.png")).unwrap(),
            b"synchronous hdrop"
        );
    }

    #[test]
    fn async_drop_orders_start_materialization_and_single_end_operation() {
        let _com = TestComApartment::new();
        let source_directory = TestTempDir::new();
        let destination = TestTempDir::new();
        let source = source_directory.path().join("ordered.png");
        std::fs::write(&source, b"ordered image").unwrap();
        let call_order = Rc::new(RefCell::new(Vec::new()));
        let end_events = Rc::new(RefCell::new(Vec::new()));
        let data_object = ordered_hdrop_test_data_object(
            source,
            call_order.clone(),
            end_events.clone(),
        );
        let capability = enabled_async_capability(&data_object).unwrap();
        unsafe { capability.StartOperation(None) }.unwrap();
        let marshaled_data_object = MarshaledWindowsInterface::new(&data_object).unwrap();
        let marshaled_capability = MarshaledWindowsInterface::new(&capability).unwrap();

        run_deferred_windows_external_drop_worker(
            marshaled_data_object,
            marshaled_capability,
            destination.path().to_path_buf(),
            HWND::default(),
        )
        .unwrap();

        assert_eq!(
            call_order.borrow().as_slice(),
            ["get_async_mode", "start_operation", "get_data", "end_operation"]
        );
        assert_eq!(end_events.borrow().as_slice(), [(super::S_OK, DROPEFFECT_COPY.0 as u32)]);
        assert_eq!(
            std::fs::read(destination.path().join("ordered.png")).unwrap(),
            b"ordered image"
        );
    }

    #[test]
    fn async_drop_worker_failure_ends_once_with_no_effect() {
        let _com = TestComApartment::new();
        let temp = TestTempDir::new();
        let missing = temp.path().join("missing.png");
        let call_order = Rc::new(RefCell::new(Vec::new()));
        let end_events = Rc::new(RefCell::new(Vec::new()));
        let data_object = ordered_hdrop_test_data_object(
            missing,
            call_order.clone(),
            end_events.clone(),
        );
        let capability = enabled_async_capability(&data_object).unwrap();
        unsafe { capability.StartOperation(None) }.unwrap();
        let marshaled_data_object = MarshaledWindowsInterface::new(&data_object).unwrap();
        let marshaled_capability = MarshaledWindowsInterface::new(&capability).unwrap();

        assert!(
            run_deferred_windows_external_drop_worker(
                marshaled_data_object,
                marshaled_capability,
                temp.path().to_path_buf(),
                HWND::default(),
            )
            .is_err()
        );

        assert_eq!(
            call_order.borrow().as_slice(),
            ["get_async_mode", "start_operation", "get_data", "end_operation"]
        );
        assert_eq!(end_events.borrow().len(), 1);
        assert_ne!(end_events.borrow()[0].0, super::S_OK);
        assert_eq!(end_events.borrow()[0].1, DROPEFFECT_NONE.0 as u32);
    }

    #[test]
    fn deferred_materialization_retries_transient_format_errors() {
        let _com = TestComApartment::new();
        let temp = TestTempDir::new();
        let source = temp.path().join("source.png");
        std::fs::write(&source, b"image").unwrap();
        let failures_remaining = Rc::new(Cell::new(2));
        let data_object = transient_hdrop_test_data_object(
            vec![source.clone()],
            failures_remaining.clone(),
        );

        let paths = materialize_deferred_hdrop_paths(&data_object).unwrap();
        assert_eq!(paths.as_slice(), [source]);
        assert_eq!(failures_remaining.get(), 0);
    }

    #[test]
    fn deferred_materialization_rejects_missing_paths() {
        let _com = TestComApartment::new();
        let data_object = test_data_object(Some(vec![PathBuf::from(r"C:\missing.png")]), None);

        assert!(materialize_deferred_hdrop_paths(&data_object).is_err());
    }

    #[test]
    fn deferred_shell_copy_copies_materialized_files() {
        let _com = TestComApartment::new();
        let source_dir = TestTempDir::new();
        let destination = TestTempDir::new();
        let source = source_dir.path().join("source.txt");
        std::fs::write(&source, b"shell-copy").unwrap();

        copy_deferred_hdrop_paths_with_shell(
            std::slice::from_ref(&source),
            destination.path(),
            HWND::default(),
        )
        .unwrap();

        assert_eq!(
            std::fs::read(destination.path().join("source.txt")).unwrap(),
            b"shell-copy"
        );
    }

    #[test]
    fn virtual_stream_and_hglobal_contents_are_materialized_byte_identically() {
        let _com = TestComApartment::new();
        let stream_bytes = b"stream image bytes\0\xff".to_vec();
        let global_bytes = b"global image bytes\0\xfe".to_vec();
        let data_object = virtual_file_test_data_object(
            vec![
                ("stream.png".to_owned(), stream_bytes.clone(), 0),
                ("global.jpg".to_owned(), global_bytes.clone(), 1),
            ],
            false,
            false,
        );

        let offered = external_paths_from_data_object(&data_object).unwrap();
        assert!(offered.is_pending_windows_drop());
        let staging = materialize_virtual_files(&data_object).unwrap();
        let root = staging.root.clone();
        assert_eq!(std::fs::read(&staging.paths[0]).unwrap(), stream_bytes);
        assert_eq!(std::fs::read(&staging.paths[1]).unwrap(), global_bytes);
        drop(staging);
        assert!(!root.exists());
    }

    #[test]
    fn virtual_files_reject_unsafe_names_directories_and_malformed_descriptors() {
        let _com = TestComApartment::new();
        for name in [r"..\escape.png", r"sub/file.png", r"C:\absolute.png", ".."] {
            let data_object = virtual_file_test_data_object(
                vec![(name.to_owned(), b"bytes".to_vec(), 0)],
                false,
                false,
            );
            assert!(materialize_virtual_files(&data_object).is_err(), "{name}");
        }
        let directory = virtual_file_test_data_object(
            vec![("folder".to_owned(), Vec::new(), 0)],
            true,
            false,
        );
        assert!(materialize_virtual_files(&directory).is_err());
        let malformed = virtual_file_test_data_object(
            vec![("image.png".to_owned(), b"bytes".to_vec(), 0)],
            false,
            true,
        );
        let failure = match super::materialize_virtual_files_with_diagnostics(&malformed) {
            Ok(_) => panic!("malformed descriptors must be rejected"),
            Err(failure) => failure,
        };
        assert_eq!(failure.code(), super::E_INVALIDARG);
        assert_eq!(
            failure.stage,
            "retrieving or parsing CFSTR_FILEDESCRIPTORW"
        );
    }

    #[test]
    fn virtual_files_reject_unsupported_storage_media_and_clean_up_staging() {
        let _com = TestComApartment::new();
        let data_object = virtual_file_test_data_object(
            vec![("image.png".to_owned(), b"bytes".to_vec(), 2)],
            false,
            false,
        );
        let failure = match super::materialize_virtual_files_with_diagnostics(&data_object) {
            Ok(_) => panic!("unsupported storage media must be rejected"),
            Err(failure) => failure,
        };
        assert_eq!(failure.code(), super::DV_E_TYMED);
        assert!(
            failure
                .stage
                .contains("selecting supported storage media for item 0 (image.png)"),
            "{}",
            failure.stage
        );
    }

    #[test]
    fn empty_virtual_partial_and_malformed_shell_batches_are_rejected() {
        let _com = TestComApartment::new();
        let empty = test_data_object(None, Some(shell_id_list_payload(&[])));
        assert!(external_paths_from_data_object(&empty).is_none());

        let virtual_pidl = pidl_bytes(OsStr::new(
            r"shell:::{26EE0668-A00A-44D7-9371-BEB064C98683}",
        ));
        let virtual_only = test_data_object(
            None,
            Some(shell_id_list_payload(std::slice::from_ref(&virtual_pidl))),
        );
        assert!(external_paths_from_data_object(&virtual_only).is_none());

        let temp = TestTempDir::new();
        let file = temp.path().join("real.txt");
        std::fs::write(&file, b"real").unwrap();
        let partial = test_data_object(
            None,
            Some(shell_id_list_payload(&[
                pidl_bytes(file.as_os_str()),
                virtual_pidl,
            ])),
        );
        assert!(external_paths_from_data_object(&partial).is_none());

        let malformed = malformed_test_data_object();
        assert!(external_paths_from_data_object(&malformed).is_none());
    }

    #[test]
    fn windows_drag_result_copies_without_source_cleanup() {
        assert_eq!(
            windows_external_drag_result(DROPEFFECT_COPY, DROPEFFECT_COPY, DROPEFFECT_NONE),
            ExternalPathsDragResult::Completed {
                operation: ExternalPathDragOperation::Copy,
                cleanup_source: false,
            }
        );
    }

    #[test]
    fn windows_drag_result_links_without_source_cleanup() {
        assert_eq!(
            windows_external_drag_result(DROPEFFECT_LINK, DROPEFFECT_LINK, DROPEFFECT_NONE),
            ExternalPathsDragResult::Completed {
                operation: ExternalPathDragOperation::Link,
                cleanup_source: false,
            }
        );
    }

    #[test]
    fn windows_drag_result_requires_cleanup_for_unoptimized_move() {
        assert_eq!(
            windows_external_drag_result(DROPEFFECT_MOVE, DROPEFFECT_MOVE, DROPEFFECT_NONE),
            ExternalPathsDragResult::Completed {
                operation: ExternalPathDragOperation::Move,
                cleanup_source: true,
            }
        );
    }

    #[test]
    fn windows_drag_result_preserves_optimized_move_sources() {
        assert_eq!(
            windows_external_drag_result(DROPEFFECT_MOVE, DROPEFFECT_NONE, DROPEFFECT_NONE),
            ExternalPathsDragResult::Completed {
                operation: ExternalPathDragOperation::Move,
                cleanup_source: false,
            }
        );
    }

    #[test]
    fn pending_drag_queue_rejects_duplicates_and_can_be_reused() {
        let mut pending = PendingExternalPathsDrag::default();
        assert!(pending.queue(ExternalPaths::new([PathBuf::from(
            r"C:\Users\test\one.txt",
        )])));
        assert!(!pending.queue(ExternalPaths::new([PathBuf::from(
            r"C:\Users\test\two.txt",
        )])));

        let paths = pending.take().unwrap();
        assert_eq!(paths.paths(), [PathBuf::from(r"C:\Users\test\one.txt")]);
        assert!(pending.queue(ExternalPaths::new([PathBuf::from(
            r"C:\Users\test\three.txt",
        )])));
        pending.cancel();
        assert!(pending.take().is_none());
    }

    #[test]
    fn deferred_drag_completion_preserves_success_and_cancels_failures() {
        let completed = ExternalPathsDragResult::copy();
        assert_eq!(
            windows_external_drag_completion(ExternalPathsDragStartResult::Completed(completed)),
            completed
        );
        assert_eq!(
            windows_external_drag_completion(ExternalPathsDragStartResult::Failed),
            ExternalPathsDragResult::Cancelled
        );
        assert_eq!(
            windows_external_drag_completion(ExternalPathsDragStartResult::Pending),
            ExternalPathsDragResult::Cancelled
        );
    }

    #[test]
    fn windows_callback_panics_use_the_fallback() {
        let fallback_called = Cell::new(false);
        let result = catch_windows_callback(
            || panic!("test callback panic"),
            |_| {
                fallback_called.set(true);
                42
            },
        );

        assert_eq!(result, 42);
        assert!(fallback_called.get());
    }

    #[test]
    fn external_drop_completion_is_scoped_reentrant_and_defaults_to_copy() {
        let active = RefCell::new(Vec::new());
        assert!(!complete_active_windows_external_drop(&active, DROPEFFECT_NONE.0));
        assert_eq!(default_external_drop_effect(false), DROPEFFECT_COPY);
        assert_eq!(default_external_drop_effect(true), DROPEFFECT_NONE);

        let outer = Rc::new(ActiveWindowsExternalDrop {
            completed_effect: Cell::new(None),
            deferred: None,
        });
        let outer_guard = push_active_windows_external_drop(&active, outer.clone());
        assert_eq!(completed_external_drop_effect(&outer, DROPEFFECT_COPY), DROPEFFECT_COPY);

        let inner = Rc::new(ActiveWindowsExternalDrop {
            completed_effect: Cell::new(None),
            deferred: None,
        });
        {
            let _inner_guard = push_active_windows_external_drop(&active, inner.clone());
            assert!(complete_active_windows_external_drop(&active, DROPEFFECT_NONE.0));
            assert_eq!(completed_external_drop_effect(&inner, DROPEFFECT_COPY), DROPEFFECT_NONE);
            assert_eq!(completed_external_drop_effect(&outer, DROPEFFECT_COPY), DROPEFFECT_COPY);
        }

        assert!(complete_active_windows_external_drop(&active, DROPEFFECT_MOVE.0));
        assert_eq!(completed_external_drop_effect(&outer, DROPEFFECT_COPY), DROPEFFECT_MOVE);
        drop(outer_guard);
        assert!(!complete_active_windows_external_drop(&active, DROPEFFECT_NONE.0));
    }

    #[test]
    fn external_drop_completion_scope_is_cleared_during_unwinding() {
        let active = RefCell::new(Vec::new());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let drop = Rc::new(ActiveWindowsExternalDrop {
                completed_effect: Cell::new(None),
                deferred: None,
            });
            let _guard = push_active_windows_external_drop(&active, drop);
            panic!("exercise drop-scope cleanup");
        }));

        assert!(result.is_err());
        assert!(active.borrow().is_empty());
        assert!(!complete_active_windows_external_drop(&active, DROPEFFECT_NONE.0));
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ClickState {
    button: MouseButton,
    last_click: Instant,
    last_position: Point<DevicePixels>,
    double_click_spatial_tolerance_width: i32,
    double_click_spatial_tolerance_height: i32,
    double_click_interval: Duration,
    pub(crate) current_count: usize,
}

impl ClickState {
    pub fn new() -> Self {
        let double_click_spatial_tolerance_width = unsafe { GetSystemMetrics(SM_CXDOUBLECLK) };
        let double_click_spatial_tolerance_height = unsafe { GetSystemMetrics(SM_CYDOUBLECLK) };
        let double_click_interval = Duration::from_millis(unsafe { GetDoubleClickTime() } as u64);

        ClickState {
            button: MouseButton::Left,
            last_click: Instant::now(),
            last_position: Point::default(),
            double_click_spatial_tolerance_width,
            double_click_spatial_tolerance_height,
            double_click_interval,
            current_count: 0,
        }
    }

    /// update self and return the needed click count
    pub fn update(&mut self, button: MouseButton, new_position: Point<DevicePixels>) -> usize {
        if self.button == button && self.is_double_click(new_position) {
            self.current_count += 1;
        } else {
            self.current_count = 1;
        }
        self.last_click = Instant::now();
        self.last_position = new_position;
        self.button = button;

        self.current_count
    }

    pub fn system_update(&mut self, wparam: usize) {
        match wparam {
            // SPI_SETDOUBLECLKWIDTH
            29 => {
                self.double_click_spatial_tolerance_width =
                    unsafe { GetSystemMetrics(SM_CXDOUBLECLK) }
            }
            // SPI_SETDOUBLECLKHEIGHT
            30 => {
                self.double_click_spatial_tolerance_height =
                    unsafe { GetSystemMetrics(SM_CYDOUBLECLK) }
            }
            // SPI_SETDOUBLECLICKTIME
            32 => {
                self.double_click_interval =
                    Duration::from_millis(unsafe { GetDoubleClickTime() } as u64)
            }
            _ => {}
        }
    }

    #[inline]
    fn is_double_click(&self, new_position: Point<DevicePixels>) -> bool {
        let diff = self.last_position - new_position;

        self.last_click.elapsed() < self.double_click_interval
            && diff.x.0.abs() <= self.double_click_spatial_tolerance_width
            && diff.y.0.abs() <= self.double_click_spatial_tolerance_height
    }
}

struct StyleAndBounds {
    style: WINDOW_STYLE,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
}

#[repr(C)]
struct WINDOWCOMPOSITIONATTRIBDATA {
    attrib: u32,
    pv_data: *mut std::ffi::c_void,
    cb_data: usize,
}

#[repr(C)]
struct AccentPolicy {
    accent_state: u32,
    accent_flags: u32,
    gradient_color: u32,
    animation_id: u32,
}

type Color = (u8, u8, u8, u8);

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct WindowBorderOffset {
    pub(crate) width_offset: i32,
    pub(crate) height_offset: i32,
}

impl WindowBorderOffset {
    pub(crate) fn update(&mut self, hwnd: HWND) -> anyhow::Result<()> {
        let window_rect = unsafe {
            let mut rect = std::mem::zeroed();
            GetWindowRect(hwnd, &mut rect)?;
            rect
        };
        let client_rect = unsafe {
            let mut rect = std::mem::zeroed();
            GetClientRect(hwnd, &mut rect)?;
            rect
        };
        self.width_offset =
            (window_rect.right - window_rect.left) - (client_rect.right - client_rect.left);
        self.height_offset =
            (window_rect.bottom - window_rect.top) - (client_rect.bottom - client_rect.top);
        Ok(())
    }
}

struct WindowOpenStatus {
    placement: WINDOWPLACEMENT,
    state: WindowOpenState,
}

enum WindowOpenState {
    Maximized,
    Fullscreen,
    Windowed,
}

const WINDOW_CLASS_NAME: PCWSTR = w!("Zed::Window");

fn register_window_class(icon_handle: HICON) {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_procedure),
            hIcon: icon_handle,
            lpszClassName: PCWSTR(WINDOW_CLASS_NAME.as_ptr()),
            style: CS_HREDRAW | CS_VREDRAW,
            hInstance: get_module_handle().into(),
            hbrBackground: unsafe { CreateSolidBrush(COLORREF(0x00000000)) },
            ..Default::default()
        };
        unsafe { RegisterClassW(&wc) };
    });
}

unsafe extern "system" fn window_procedure(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    catch_windows_callback(
        || window_procedure_inner(hwnd, msg, wparam, lparam),
        |payload| {
            let message = panic_payload_message(payload);
            log::error!("panic in Windows window procedure for message {msg:#x}: {message}");
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        },
    )
}

fn window_procedure_inner(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let window_params = lparam.0 as *const CREATESTRUCTW;
        let window_params = unsafe { &*window_params };
        let window_creation_context = window_params.lpCreateParams as *mut WindowCreateContext;
        let window_creation_context = unsafe { &mut *window_creation_context };
        return match WindowsWindowInner::new(window_creation_context, hwnd, window_params) {
            Ok(window_state) => {
                let weak = Box::new(Rc::downgrade(&window_state));
                unsafe { set_window_long(hwnd, GWLP_USERDATA, Box::into_raw(weak) as isize) };
                window_creation_context.inner = Some(Ok(window_state));
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            Err(error) => {
                window_creation_context.inner = Some(Err(error));
                LRESULT(0)
            }
        };
    }

    let ptr = unsafe { get_window_long(hwnd, GWLP_USERDATA) } as *mut Weak<WindowsWindowInner>;
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let inner = unsafe { &*ptr };
    let result = if let Some(inner) = inner.upgrade() {
        inner.handle_msg(hwnd, msg, wparam, lparam)
    } else {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    };

    if msg == WM_NCDESTROY {
        unsafe { set_window_long(hwnd, GWLP_USERDATA, 0) };
        unsafe { drop(Box::from_raw(ptr)) };
    }

    result
}

pub(super) fn catch_windows_callback<T>(
    callback: impl FnOnce() -> T,
    on_panic: impl FnOnce(&(dyn Any + Send)) -> T,
) -> T {
    match catch_unwind(AssertUnwindSafe(callback)) {
        Ok(result) => result,
        Err(payload) => on_panic(payload.as_ref()),
    }
}

pub(super) fn panic_payload_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic payload")
}

pub(crate) fn window_from_hwnd(hwnd: HWND) -> Option<Rc<WindowsWindowInner>> {
    if hwnd.is_invalid() {
        return None;
    }

    let ptr = unsafe { get_window_long(hwnd, GWLP_USERDATA) } as *mut Weak<WindowsWindowInner>;
    if !ptr.is_null() {
        let inner = unsafe { &*ptr };
        inner.upgrade()
    } else {
        None
    }
}

fn get_module_handle() -> HMODULE {
    unsafe {
        let mut h_module = std::mem::zeroed();
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            windows::core::w!("ZedModule"),
            &mut h_module,
        )
        .expect("Unable to get module handle"); // this should never fail

        h_module
    }
}

fn register_drag_drop(window: &Rc<WindowsWindowInner>) -> Result<()> {
    let window_handle = window.hwnd;
    let handler = WindowsDragDropHandler(window.clone());
    // The lifetime of `IDropTarget` is handled by Windows, it won't release until
    // we call `RevokeDragDrop`.
    // So, it's safe to drop it here.
    let drag_drop_handler: IDropTarget = handler.into();
    unsafe {
        RegisterDragDrop(window_handle, &drag_drop_handler)
            .context("unable to register drag-drop event")?;
    }
    Ok(())
}

fn calculate_window_rect(bounds: Bounds<DevicePixels>, border_offset: WindowBorderOffset) -> RECT {
    // NOTE:
    // The reason we're not using `AdjustWindowRectEx()` here is
    // that the size reported by this function is incorrect.
    // You can test it, and there are similar discussions online.
    // See: https://stackoverflow.com/questions/12423584/how-to-set-exact-client-size-for-overlapped-window-winapi
    //
    // So we manually calculate these values here.
    let mut rect = RECT {
        left: bounds.left().0,
        top: bounds.top().0,
        right: bounds.right().0,
        bottom: bounds.bottom().0,
    };
    let left_offset = border_offset.width_offset / 2;
    let top_offset = border_offset.height_offset / 2;
    let right_offset = border_offset.width_offset - left_offset;
    let bottom_offset = border_offset.height_offset - top_offset;
    rect.left -= left_offset;
    rect.top -= top_offset;
    rect.right += right_offset;
    rect.bottom += bottom_offset;
    rect
}

fn calculate_client_rect(
    rect: RECT,
    border_offset: WindowBorderOffset,
    scale_factor: f32,
) -> Bounds<Pixels> {
    let left_offset = border_offset.width_offset / 2;
    let top_offset = border_offset.height_offset / 2;
    let right_offset = border_offset.width_offset - left_offset;
    let bottom_offset = border_offset.height_offset - top_offset;
    let left = rect.left + left_offset;
    let top = rect.top + top_offset;
    let right = rect.right - right_offset;
    let bottom = rect.bottom - bottom_offset;
    let physical_size = size(DevicePixels(right - left), DevicePixels(bottom - top));
    Bounds {
        origin: logical_point(left as f32, top as f32, scale_factor),
        size: physical_size.to_pixels(scale_factor),
    }
}

fn retrieve_window_placement(
    hwnd: HWND,
    display: WindowsDisplay,
    initial_bounds: Bounds<Pixels>,
    scale_factor: f32,
    border_offset: WindowBorderOffset,
) -> Result<WINDOWPLACEMENT> {
    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    unsafe { GetWindowPlacement(hwnd, &mut placement)? };
    // the bounds may be not inside the display
    let bounds = if display.check_given_bounds(initial_bounds) {
        initial_bounds
    } else {
        display.default_bounds()
    };
    let bounds = bounds.to_device_pixels(scale_factor);
    placement.rcNormalPosition = calculate_window_rect(bounds, border_offset);
    Ok(placement)
}

fn set_window_composition_attribute(hwnd: HWND, color: Option<Color>, state: u32) {
    let mut version = unsafe { std::mem::zeroed() };
    let status = unsafe { windows::Wdk::System::SystemServices::RtlGetVersion(&mut version) };
    if !status.is_ok() || version.dwBuildNumber < 17763 {
        return;
    }

    unsafe {
        type SetWindowCompositionAttributeType =
            unsafe extern "system" fn(HWND, *mut WINDOWCOMPOSITIONATTRIBDATA) -> BOOL;
        let module_name = PCSTR::from_raw(c"user32.dll".as_ptr() as *const u8);
        if let Some(user32) = GetModuleHandleA(module_name)
            .context("Unable to get user32.dll handle")
            .log_err()
        {
            let func_name = PCSTR::from_raw(c"SetWindowCompositionAttribute".as_ptr() as *const u8);
            let set_window_composition_attribute: SetWindowCompositionAttributeType =
                std::mem::transmute(GetProcAddress(user32, func_name));
            let mut color = color.unwrap_or_default();
            let is_acrylic = state == 4;
            if is_acrylic && color.3 == 0 {
                color.3 = 1;
            }
            let accent = AccentPolicy {
                accent_state: state,
                accent_flags: if is_acrylic { 0 } else { 2 },
                gradient_color: (color.0 as u32)
                    | ((color.1 as u32) << 8)
                    | ((color.2 as u32) << 16)
                    | ((color.3 as u32) << 24),
                animation_id: 0,
            };
            let mut data = WINDOWCOMPOSITIONATTRIBDATA {
                attrib: 0x13,
                pv_data: &accent as *const _ as *mut _,
                cb_data: std::mem::size_of::<AccentPolicy>(),
            };
            let _ = set_window_composition_attribute(hwnd, &mut data as *mut _ as _);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ClickState, SystemCaretGeometry, system_caret_geometry};
    use crate::{Bounds, DevicePixels, MouseButton, UTF16Selection, point, px, size};
    use std::time::Duration;

    #[test]
    fn test_double_click_interval() {
        let mut state = ClickState::new();
        assert_eq!(
            state.update(MouseButton::Left, point(DevicePixels(0), DevicePixels(0))),
            1
        );
        assert_eq!(
            state.update(MouseButton::Right, point(DevicePixels(0), DevicePixels(0))),
            1
        );
        assert_eq!(
            state.update(MouseButton::Left, point(DevicePixels(0), DevicePixels(0))),
            1
        );
        assert_eq!(
            state.update(MouseButton::Left, point(DevicePixels(0), DevicePixels(0))),
            2
        );
        state.last_click -= Duration::from_millis(700);
        assert_eq!(
            state.update(MouseButton::Left, point(DevicePixels(0), DevicePixels(0))),
            1
        );
    }

    #[test]
    fn test_double_click_spatial_tolerance() {
        let mut state = ClickState::new();
        assert_eq!(
            state.update(MouseButton::Left, point(DevicePixels(-3), DevicePixels(0))),
            1
        );
        assert_eq!(
            state.update(MouseButton::Left, point(DevicePixels(0), DevicePixels(3))),
            2
        );
        assert_eq!(
            state.update(MouseButton::Right, point(DevicePixels(3), DevicePixels(2))),
            1
        );
        assert_eq!(
            state.update(MouseButton::Right, point(DevicePixels(10), DevicePixels(0))),
            1
        );
    }

    #[test]
    fn system_caret_geometry_scales_to_device_pixels() {
        let bounds = Bounds::new(point(px(10.25), px(20.5)), size(px(0.0), px(18.0)));

        assert_eq!(
            system_caret_geometry(bounds, 1.0),
            SystemCaretGeometry {
                x: 10,
                y: 21,
                width: 1,
                height: 18,
            }
        );
        assert_eq!(
            system_caret_geometry(bounds, 1.5),
            SystemCaretGeometry {
                x: 15,
                y: 31,
                width: 2,
                height: 27,
            }
        );
        assert_eq!(
            system_caret_geometry(bounds, 2.0),
            SystemCaretGeometry {
                x: 21,
                y: 41,
                width: 2,
                height: 36,
            }
        );
    }

    #[test]
    fn system_caret_geometry_has_nonzero_size() {
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(0.0), px(0.0)));

        assert_eq!(
            system_caret_geometry(bounds, 0.5),
            SystemCaretGeometry {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            }
        );
    }

    #[test]
    fn utf16_selection_head_uses_the_active_edge() {
        let forward = UTF16Selection {
            range: 4..9,
            reversed: false,
        };
        let reversed = UTF16Selection {
            range: 4..9,
            reversed: true,
        };

        assert_eq!(forward.head(), 9);
        assert_eq!(reversed.head(), 4);
    }
}
