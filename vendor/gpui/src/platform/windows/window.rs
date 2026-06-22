#![deny(unsafe_op_in_unsafe_fn)]

use std::{
    cell::{Cell, RefCell},
    mem,
    num::NonZeroIsize,
    path::PathBuf,
    rc::{Rc, Weak},
    str::FromStr,
    sync::{Arc, LazyLock, Once},
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
        System::{
            Com::*,
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

pub(crate) struct WindowsWindow(pub Rc<WindowsWindowInner>);

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
    pub(crate) system_settings: RefCell<WindowsSystemSettings>,
    pub(crate) handle: AnyWindowHandle,
    pub(crate) hide_title_bar: bool,
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
    fn new(context: &mut WindowCreateContext, hwnd: HWND, cs: &CREATESTRUCTW) -> Result<Rc<Self>> {
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
            handle: context.handle,
            hide_title_bar: context.hide_title_bar,
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

        Ok(Self(this))
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

    fn start_external_paths_drag(&self, paths: ExternalPaths) -> ExternalPathsDragStartResult {
        start_windows_external_paths_drag(self.0.hwnd, paths)
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        self.0.state.borrow().renderer.gpu_specs().log_err()
    }

    fn update_ime_position(&self, _bounds: Bounds<Pixels>) {
        // There is no such thing on Windows.
    }
}

static PREFERRED_DROPEFFECT_FORMAT: LazyLock<u16> =
    LazyLock::new(|| register_shell_clipboard_format(CFSTR_PREFERREDDROPEFFECT));
static PERFORMED_DROPEFFECT_FORMAT: LazyLock<u16> =
    LazyLock::new(|| register_shell_clipboard_format(CFSTR_PERFORMEDDROPEFFECT));
static LOGICAL_PERFORMED_DROPEFFECT_FORMAT: LazyLock<u16> =
    LazyLock::new(|| register_shell_clipboard_format(CFSTR_LOGICALPERFORMEDDROPEFFECT));

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

#[implement(IDataObject)]
struct WindowsFileDataObject {
    paths: Vec<PathBuf>,
    preferred_effect: DROPEFFECT,
    performed_effect: Rc<Cell<DROPEFFECT>>,
    logical_performed_effect: Rc<Cell<DROPEFFECT>>,
}

#[allow(non_snake_case)]
impl IDataObject_Impl for WindowsFileDataObject_Impl {
    fn GetData(&self, pformatetcin: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        if self.QueryGetData(pformatetcin) != S_OK {
            return Err(DV_E_FORMATETC.into());
        }

        let format = unsafe { pformatetcin.as_ref() }
            .map(|format| format.cfFormat)
            .ok_or_else(|| windows::core::Error::from(DV_E_FORMATETC))?;
        let hglobal = if format == CF_HDROP.0 {
            allocate_hdrop(self.paths.as_slice())?
        } else if format == *PREFERRED_DROPEFFECT_FORMAT {
            allocate_dropeffect(self.preferred_effect)?
        } else {
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
        if is_hdrop_format(pformatetc) || is_dropeffect_format(pformatetc, *PREFERRED_DROPEFFECT_FORMAT) {
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
            Ok(WindowsFormatEtcEnumerator::new().into())
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

#[implement(IEnumFORMATETC)]
struct WindowsFormatEtcEnumerator {
    next_index: Cell<usize>,
}

impl WindowsFormatEtcEnumerator {
    fn new() -> Self {
        Self {
            next_index: Cell::new(0),
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
        while fetched < celt && self.next_index.get() < 2 {
            let format = match self.next_index.get() {
                0 => hdrop_format_etc(),
                1 => dropeffect_format_etc(*PREFERRED_DROPEFFECT_FORMAT),
                _ => unreachable!(),
            };
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
        let remaining = 2usize.saturating_sub(self.next_index.get());
        self.next_index
            .set((self.next_index.get() + celt as usize).min(2));
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

fn allowed_dropeffects_for_operations(operations: ExternalPathDragOperations) -> DROPEFFECT {
    let mut effect = DROPEFFECT_NONE;
    if operations.copy() {
        effect |= DROPEFFECT_COPY;
    }
    if operations.move_() {
        effect |= DROPEFFECT_MOVE;
    }
    effect
}

fn preferred_dropeffect_for_operations(operations: ExternalPathDragOperations) -> DROPEFFECT {
    if operations.move_() && !operations.copy() {
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
    if drop_effect == DROPEFFECT_MOVE {
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
            let config = FORMATETC {
                cfFormat: CF_HDROP.0,
                ptd: std::ptr::null_mut() as _,
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as _,
            };
            let cursor_position = POINT { x: pt.x, y: pt.y };
            if idata_obj.QueryGetData(&config as _) == S_OK {
                *pdweffect = DROPEFFECT_COPY;
                let Some(mut idata) = idata_obj.GetData(&config as _).log_err() else {
                    return Ok(());
                };
                if idata.u.hGlobal.is_invalid() {
                    return Ok(());
                }
                let hdrop = idata.u.hGlobal.0 as *mut HDROP;
                let mut paths = SmallVec::<[PathBuf; 2]>::new();
                with_file_names(*hdrop, |file_name| {
                    if let Some(path) = PathBuf::from_str(&file_name).log_err() {
                        paths.push(path);
                    }
                });
                ReleaseStgMedium(&mut idata);
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
                    paths: ExternalPaths::new(paths),
                });
                self.handle_drag_drop(input);
            } else {
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
        unsafe {
            *pdweffect = DROPEFFECT_COPY;
            self.0
                .drop_target_helper
                .DragOver(&cursor_position, *pdweffect)
                .log_err();
            ScreenToClient(self.0.hwnd, &mut cursor_position)
                .ok()
                .log_err();
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
        unsafe {
            *pdweffect = DROPEFFECT_COPY;
            self.0
                .drop_target_helper
                .Drop(idata_obj, &cursor_position, *pdweffect)
                .log_err();
            ScreenToClient(self.0.hwnd, &mut cursor_position)
                .ok()
                .log_err();
        }
        let scale_factor = self.0.state.borrow().scale_factor;
        let input = PlatformInput::FileDrop(FileDropEvent::Submit {
            position: logical_point(
                cursor_position.x as f32,
                cursor_position.y as f32,
                scale_factor,
            ),
        });
        self.handle_drag_drop(input);

        Ok(())
    }
}

#[cfg(test)]
mod external_paths_drag_tests {
    use super::{
        build_hdrop_payload, windows_external_drag_result, DROPFILES, DROPEFFECT_COPY,
        DROPEFFECT_MOVE, DROPEFFECT_NONE,
    };
    use crate::{ExternalPathDragOperation, ExternalPathsDragResult};
    use std::{mem, path::PathBuf};

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
    use super::ClickState;
    use crate::{DevicePixels, MouseButton, point};
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
}
