#![deny(unsafe_op_in_unsafe_fn)]

use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::WindowsAndMessaging::{HTCAPTION, PostMessageW, SC_MOVE, WM_SYSCOMMAND},
};

pub(super) fn queue_window_move(hwnd: HWND) -> windows::core::Result<()> {
    // SendMessageW enters the native move loop before the GPUI input callback
    // releases its app borrow. Tasks dispatched by that loop would reborrow it.
    // Posting lets the callback return before Windows starts moving the window.
    unsafe {
        PostMessageW(
            Some(hwnd),
            WM_SYSCOMMAND,
            WPARAM((SC_MOVE | HTCAPTION) as usize),
            LPARAM(0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use windows::{
        Win32::{
            Foundation::{HINSTANCE, LRESULT},
            System::LibraryLoader::GetModuleHandleW,
            UI::WindowsAndMessaging::*,
        },
        core::{HSTRING, PCWSTR},
    };

    #[derive(Default)]
    struct MoveProbe {
        app: RefCell<()>,
        calls: Cell<usize>,
        borrow_conflict: Cell<bool>,
        command: Cell<usize>,
        parameter: Cell<isize>,
    }

    unsafe extern "system" fn probe_window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // Record failures for assertions outside this callback: Rust panics must
        // never unwind across Win32. Consume the command instead of entering a
        // real interactive move loop on the hidden test window.
        unsafe {
            if message == WM_NCCREATE {
                let creation = &*(lparam.0 as *const CREATESTRUCTW);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, creation.lpCreateParams as isize);
                return LRESULT(1);
            }
            if message == WM_SYSCOMMAND {
                let probe = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const MoveProbe;
                if !probe.is_null() {
                    let probe = &*probe;
                    probe.calls.set(probe.calls.get().saturating_add(1));
                    probe.command.set(wparam.0);
                    probe.parameter.set(lparam.0);
                    if probe.app.try_borrow_mut().is_err() {
                        probe.borrow_conflict.set(true);
                    }
                }
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
    }

    struct HiddenWindow {
        hwnd: HWND,
        class_name: HSTRING,
        instance: HINSTANCE,
    }

    impl HiddenWindow {
        fn new(probe: &MoveProbe) -> Self {
            let class_name =
                HSTRING::from(format!("GPUIQueuedMoveRegression-{}", std::process::id()));
            let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
            let class = WNDCLASSW {
                lpfnWndProc: Some(probe_window_proc),
                hInstance: instance,
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            assert_ne!(unsafe { RegisterClassW(&class) }, 0);
            let mut window = Self {
                hwnd: HWND::default(),
                class_name,
                instance,
            };
            window.hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    PCWSTR(window.class_name.as_ptr()),
                    PCWSTR::null(),
                    WS_OVERLAPPED,
                    0,
                    0,
                    100,
                    100,
                    None,
                    None,
                    Some(instance),
                    Some(std::ptr::from_ref(probe).cast()),
                )
            }
            .unwrap();
            window
        }
    }

    impl Drop for HiddenWindow {
        fn drop(&mut self) {
            unsafe {
                if !self.hwnd.is_invalid() {
                    let _ = DestroyWindow(self.hwnd);
                }
                let _ = UnregisterClassW(PCWSTR(self.class_name.as_ptr()), Some(self.instance));
            }
        }
    }

    #[test]
    fn native_window_move_is_queued() {
        // Keep the probe alive until after DestroyWindow has finished callbacks.
        let probe = MoveProbe::default();
        let window = HiddenWindow::new(&probe);
        let app_borrow = probe.app.borrow_mut();

        queue_window_move(window.hwnd).unwrap();
        assert_eq!(probe.calls.get(), 0, "the callback must return first");
        assert!(!probe.borrow_conflict.get());
        drop(app_borrow);

        let mut message = MSG::default();
        assert!(
            unsafe {
                PeekMessageW(
                    &mut message,
                    Some(window.hwnd),
                    WM_SYSCOMMAND,
                    WM_SYSCOMMAND,
                    PM_REMOVE,
                )
            }
            .as_bool()
        );
        assert_eq!(message.hwnd, window.hwnd);
        assert_eq!(message.message, WM_SYSCOMMAND);
        assert_eq!(message.wParam.0, (SC_MOVE | HTCAPTION) as usize);
        assert_eq!(message.lParam.0, 0);
        assert_eq!(probe.calls.get(), 0, "dispatch must deliver the command");

        unsafe { DispatchMessageW(&message) };
        assert_eq!(probe.calls.get(), 1);
        assert!(!probe.borrow_conflict.get(), "nested app borrow detected");
        assert_eq!(probe.command.get(), (SC_MOVE | HTCAPTION) as usize);
        assert_eq!(probe.parameter.get(), 0);
        assert!(
            !unsafe {
                PeekMessageW(
                    &mut message,
                    Some(window.hwnd),
                    WM_SYSCOMMAND,
                    WM_SYSCOMMAND,
                    PM_REMOVE,
                )
            }
            .as_bool()
        );
        assert_eq!(probe.calls.get(), 1, "deliver exactly one move command");
    }
}
