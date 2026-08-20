use std::{ffi::OsStr, io, path::Path};

use gpui::Window;
use windows::{
    Win32::{
        Foundation::HWND,
        UI::{
            Shell::{
                SEE_MASK_CLASSKEY, SEE_MASK_CLASSNAME, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC,
                SHELLEXECUTEINFOW, ShellExecuteExW,
            },
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

fn null_terminated_shell_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .map(|unit| {
            if unit == b'/' as u16 {
                b'\\' as u16
            } else {
                unit
            }
        })
        .chain(std::iter::once(0))
        .collect()
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
    no_async: bool,
    parent: Option<HWND>,
) -> ShellExecuteRequest {
    use std::mem::size_of;

    let verb = null_terminated_wide(verb);
    let class = class.map(null_terminated_wide);
    let file = null_terminated_shell_path(path);
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
    if no_async {
        execute_info.fMask |= SEE_MASK_NOASYNC;
    }

    ShellExecuteRequest {
        _verb: verb,
        _class: class,
        _file: file,
        execute_info,
    }
}

pub(super) fn shell_execute_file_with_class_key_request(
    path: &Path,
    verb: &OsStr,
    class_key: windows::Win32::System::Registry::HKEY,
    no_ui: bool,
    no_async: bool,
    parent: Option<HWND>,
) -> ShellExecuteRequest {
    let mut request = shell_execute_file_request(path, verb, None, no_ui, no_async, parent);
    request.execute_info.fMask |= SEE_MASK_CLASSKEY;
    request.execute_info.hkeyClass = class_key;
    request
}

pub(super) fn execute_shell_request(request: &mut ShellExecuteRequest) -> io::Result<bool> {
    shell_execute_result(unsafe { ShellExecuteExW(request.execute_info_mut()) })
}

pub(super) fn execute_shell_request_with_com(
    request: &mut ShellExecuteRequest,
) -> io::Result<bool> {
    use windows::Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
    };

    let initialization = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if initialization.is_err() && initialization != RPC_E_CHANGED_MODE {
        return Err(io::Error::other(windows::core::Error::from_hresult(
            initialization,
        )));
    }

    let result = execute_shell_request(request);
    if initialization.is_ok() {
        unsafe { CoUninitialize() };
    }
    result
}

pub(super) fn create_shell_shortcut(shortcut: &Path, target: &Path) -> io::Result<()> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize, IPersistFile,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::Interface;

    unsafe {
        let initialized_com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let result = (|| -> windows::core::Result<()> {
            let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            let target_path = null_terminated_wide(target.as_os_str());
            shell_link.SetPath(PCWSTR::from_raw(target_path.as_ptr()))?;

            let persist_file: IPersistFile = shell_link.cast()?;
            let shortcut_path = null_terminated_wide(shortcut.as_os_str());
            persist_file.Save(PCWSTR::from_raw(shortcut_path.as_ptr()), true)
        })();
        if initialized_com {
            CoUninitialize();
        }
        result.map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_path_normalization_preserves_non_separator_utf16_units() {
        use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf};

        let path = PathBuf::from(OsString::from_wide(&[
            b'C' as u16,
            b':' as u16,
            b'/' as u16,
            0xd800,
            b'/' as u16,
            b'x' as u16,
        ]));

        assert_eq!(
            null_terminated_shell_path(&path),
            vec![
                b'C' as u16,
                b':' as u16,
                b'\\' as u16,
                0xd800,
                b'\\' as u16,
                b'x' as u16,
                0,
            ]
        );
    }
}
