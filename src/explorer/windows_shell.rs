use std::{ffi::OsStr, io, path::Path};

use gpui::Window;
use windows::{
    Win32::{
        Foundation::HWND,
        UI::{
            Shell::{SEE_MASK_CLASSNAME, SEE_MASK_FLAG_NO_UI, SHELLEXECUTEINFOW, ShellExecuteExW},
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
    core::PCWSTR,
};

pub(super) const WINDOWS_ERROR_CANCELLED: u32 = 1223;

pub(super) fn parent_hwnd(window: &Window) -> Option<HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut _)),
        _ => None,
    }
}

pub(super) fn null_terminated_wide(value: &OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    value.encode_wide().chain(std::iter::once(0)).collect()
}

pub(super) fn shell_execute_result(result: windows::core::Result<()>) -> io::Result<bool> {
    match result {
        Ok(()) => Ok(true),
        Err(error)
            if error.code() == windows::core::HRESULT::from_win32(WINDOWS_ERROR_CANCELLED) =>
        {
            Ok(false)
        }
        Err(error) => Err(io::Error::other(error)),
    }
}

pub(super) struct ShellExecuteRequest {
    _verb: Vec<u16>,
    _class: Option<Vec<u16>>,
    _file: Vec<u16>,
    execute_info: SHELLEXECUTEINFOW,
}

impl ShellExecuteRequest {
    #[cfg(test)]
    pub(super) fn execute_info(&self) -> &SHELLEXECUTEINFOW {
        &self.execute_info
    }

    fn execute_info_mut(&mut self) -> &mut SHELLEXECUTEINFOW {
        &mut self.execute_info
    }
}

pub(super) fn shell_execute_file_request(
    path: &Path,
    verb: &OsStr,
    class: Option<&OsStr>,
    no_ui: bool,
    parent: Option<HWND>,
) -> ShellExecuteRequest {
    use std::mem::size_of;

    let verb = null_terminated_wide(verb);
    let class = class.map(null_terminated_wide);
    let file = null_terminated_wide(path.as_os_str());
    let mut execute_info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        hwnd: parent.unwrap_or_default(),
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    if let Some(class) = class.as_ref() {
        execute_info.fMask |= SEE_MASK_CLASSNAME;
        execute_info.lpClass = PCWSTR(class.as_ptr());
    }
    if no_ui {
        execute_info.fMask |= SEE_MASK_FLAG_NO_UI;
    }

    ShellExecuteRequest {
        _verb: verb,
        _class: class,
        _file: file,
        execute_info,
    }
}

pub(super) fn execute_shell_request(request: &mut ShellExecuteRequest) -> io::Result<bool> {
    shell_execute_result(unsafe { ShellExecuteExW(request.execute_info_mut()) })
}
