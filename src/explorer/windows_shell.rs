use std::{ffi::OsStr, io, mem::size_of, path::Path};

use gpui::Window;
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, WAIT_ABANDONED, WAIT_OBJECT_0},
        Globalization::{CSTR_EQUAL, CompareStringOrdinal},
        Storage::FileSystem::GetShortPathNameW,
        System::{
            Memory::{
                FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW,
                UnmapViewOfFile,
            },
            Threading::{
                MUTEX_MODIFY_STATE, OpenMutexW, ReleaseMutex, SYNCHRONIZATION_ACCESS_RIGHTS,
                WaitForSingleObject,
            },
        },
        UI::{
            Shell::{
                FO_COPY, FO_MOVE, FOF_ALLOWUNDO, SEE_MASK_CLASSNAME, SEE_MASK_FLAG_NO_UI,
                SHELLEXECUTEINFOW, SHFILEOPSTRUCTW, SHFileOperationW, ShellExecuteExW,
            },
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
    core::{PCWSTR, w},
};

pub(super) const WINDOWS_ERROR_CANCELLED: u32 = 1223;
const DE_OPCANCELLED: i32 = 0x75;
const LEGACY_SHELL_MAX_PATH: usize = 260;
const WINSCP_MAPPING_NAME: PCWSTR = w!("WinSCPDragExtMapping");
const WINSCP_MUTEX_NAME: PCWSTR = w!("WinSCPDragExtMutex");
const WINSCP_MAPPING_SIZE: usize = 528;
const WINSCP_VERSION_OFFSET: usize = 0;
const WINSCP_DRAGGING_OFFSET: usize = 4;
const WINSCP_DROP_DEST_OFFSET: usize = 6;
const WINSCP_DROP_DEST_WORDS: usize = 260;
const WINSCP_MUTEX_ACCESS: SYNCHRONIZATION_ACCESS_RIGHTS =
    SYNCHRONIZATION_ACCESS_RIGHTS(0x0010_0000 | MUTEX_MODIFY_STATE.0);
const WINSCP_PROTOCOL_VERSION: i32 = 1;
const WINSCP_MUTEX_TIMEOUT_MS: u32 = 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ShellFileOperation {
    Copy,
    Move,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ShellFileOperationResult {
    Completed,
    Aborted,
    Failed(i32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WinScpDropBridgeResult {
    NotApplicable,
    FailedBeforeCommit,
    Committed,
}

const DROPEFFECT_COPY_VALUE: u32 = 1;
const DROPEFFECT_MOVE_VALUE: u32 = 2;
const DROPEFFECT_LINK_VALUE: u32 = 4;
const MK_SHIFT_VALUE: u32 = 0x0004;
const MK_CONTROL_VALUE: u32 = 0x0008;
const MK_ALT_VALUE: u32 = 0x0020;

pub(super) const fn shell_file_operation_effect(operation: ShellFileOperation) -> u32 {
    match operation {
        ShellFileOperation::Copy => DROPEFFECT_COPY_VALUE,
        ShellFileOperation::Move => DROPEFFECT_MOVE_VALUE,
    }
}

pub(super) fn resolve_native_shell_file_operation(
    context: Option<gpui::WindowsExternalDropContext>,
    fallback: ShellFileOperation,
) -> Option<ShellFileOperation> {
    let Some(context) = context else {
        return Some(fallback);
    };
    let allowed = context.allowed_effects;
    let key_state = context.key_state;
    let control = key_state & MK_CONTROL_VALUE != 0;
    let shift = key_state & MK_SHIFT_VALUE != 0;

    if key_state & MK_ALT_VALUE != 0 || (control && shift) {
        return None;
    }
    if control {
        return (allowed & DROPEFFECT_COPY_VALUE != 0).then_some(ShellFileOperation::Copy);
    }
    if shift {
        return (allowed & DROPEFFECT_MOVE_VALUE != 0).then_some(ShellFileOperation::Move);
    }
    if let Some(preferred) = context.preferred_effect {
        if preferred & DROPEFFECT_COPY_VALUE != 0 && allowed & DROPEFFECT_COPY_VALUE != 0 {
            return Some(ShellFileOperation::Copy);
        }
        if preferred & DROPEFFECT_MOVE_VALUE != 0 && allowed & DROPEFFECT_MOVE_VALUE != 0 {
            return Some(ShellFileOperation::Move);
        }
        if preferred & DROPEFFECT_LINK_VALUE != 0 && allowed & DROPEFFECT_LINK_VALUE != 0 {
            return None;
        }
    }

    let fallback_effect = shell_file_operation_effect(fallback);
    (allowed & fallback_effect != 0).then_some(fallback)
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) }.ok();
    }
}

struct MappedView(MEMORY_MAPPED_VIEW_ADDRESS);

impl Drop for MappedView {
    fn drop(&mut self) {
        unsafe { UnmapViewOfFile(self.0) }.ok();
    }
}

struct MutexGuard(HANDLE);

impl Drop for MutexGuard {
    fn drop(&mut self) {
        unsafe { ReleaseMutex(self.0) }.ok();
    }
}

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

fn double_null_terminated_wide_paths<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Vec<u16> {
    let mut encoded = Vec::new();
    for path in paths {
        encoded.extend(
            legacy_shell_path_wide(path)
                .expect("shell file operation paths must be validated before encoding"),
        );
        encoded.push(0);
    }
    encoded.push(0);
    encoded
}

fn legacy_shell_path_wide(path: &Path) -> Option<Vec<u16>> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    if !path.is_absolute() {
        return None;
    }

    let mut encoded = path.as_os_str().encode_wide().collect::<Vec<_>>();
    const VERBATIM_PREFIX: &[u16] = &[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    const VERBATIM_UNC_PREFIX: &[u16] = &[
        b'\\' as u16,
        b'\\' as u16,
        b'?' as u16,
        b'\\' as u16,
        b'U' as u16,
        b'N' as u16,
        b'C' as u16,
        b'\\' as u16,
    ];

    if encoded.starts_with(VERBATIM_PREFIX) {
        let is_verbatim_unc = encoded.len() >= VERBATIM_UNC_PREFIX.len()
            && encoded[..VERBATIM_UNC_PREFIX.len()]
                .iter()
                .zip(VERBATIM_UNC_PREFIX)
                .all(|(actual, expected)| {
                    actual == expected
                        || ((b'A' as u16..=b'Z' as u16).contains(expected)
                            && *actual == *expected + (b'a' - b'A') as u16)
                });
        if is_verbatim_unc {
            encoded.splice(..VERBATIM_UNC_PREFIX.len(), [b'\\' as u16, b'\\' as u16]);
        } else if encoded.len() >= 7 && encoded[5] == b':' as u16 && encoded[6] == b'\\' as u16 {
            encoded.drain(..VERBATIM_PREFIX.len());
        } else {
            return None;
        }
    }

    if encoded.is_empty()
        || encoded.len() >= LEGACY_SHELL_MAX_PATH
        || encoded.contains(&0)
        || !Path::new(&std::ffi::OsString::from_wide(&encoded)).is_absolute()
    {
        None
    } else {
        Some(encoded)
    }
}

fn legacy_shell_path_is_supported(path: &Path) -> bool {
    legacy_shell_path_wide(path).is_some()
}

pub(super) fn shell_file_operation_paths_supported(
    paths: &[std::path::PathBuf],
    destination: &Path,
) -> bool {
    !paths.is_empty()
        && paths
            .iter()
            .all(|path| legacy_shell_path_is_supported(path))
        && legacy_shell_path_is_supported(destination)
}

fn winscp_bridge_candidate<'a>(
    paths: &'a [std::path::PathBuf],
    destination: &Path,
) -> Option<(&'a Path, Vec<u16>)> {
    let [source] = paths else {
        return None;
    };
    if !source.is_dir() {
        return None;
    }
    let file_name = source.file_name()?;
    use std::os::windows::ffi::OsStrExt;
    let file_name_wide = file_name.encode_wide().collect::<Vec<_>>();
    if file_name_wide.len() < 3
        || file_name_wide[0] != b's' as u16
        || file_name_wide[1] != b'c' as u16
        || file_name_wide[2] != b'p' as u16
    {
        return None;
    }

    let target = destination.join(file_name);
    let target = legacy_shell_path_wide(&target)?;
    (target.len() < WINSCP_DROP_DEST_WORDS).then_some((source.as_path(), target))
}

fn winscp_mapping_drop_dest(mapping: &[u8]) -> Option<Vec<u16>> {
    if mapping.len() < WINSCP_MAPPING_SIZE {
        return None;
    }
    let words = mapping[WINSCP_DROP_DEST_OFFSET
        ..WINSCP_DROP_DEST_OFFSET + WINSCP_DROP_DEST_WORDS * size_of::<u16>()]
        .chunks_exact(size_of::<u16>())
        .map(|bytes| u16::from_ne_bytes([bytes[0], bytes[1]]));
    let mut value = Vec::new();
    for word in words {
        if word == 0 {
            return Some(value);
        }
        value.push(word);
    }
    None
}

fn winscp_mapping_is_active(mapping: &[u8]) -> bool {
    mapping.len() >= WINSCP_MAPPING_SIZE
        && i32::from_ne_bytes(
            mapping[WINSCP_VERSION_OFFSET..WINSCP_VERSION_OFFSET + size_of::<i32>()]
                .try_into()
                .unwrap(),
        ) == WINSCP_PROTOCOL_VERSION
        && mapping[WINSCP_DRAGGING_OFFSET] != 0
}

fn commit_winscp_mapping(mapping: &mut [u8], target: &[u16]) -> bool {
    if !winscp_mapping_is_active(mapping) || target.len() >= WINSCP_DROP_DEST_WORDS {
        return false;
    }

    let destination = &mut mapping[WINSCP_DROP_DEST_OFFSET
        ..WINSCP_DROP_DEST_OFFSET + WINSCP_DROP_DEST_WORDS * size_of::<u16>()];
    destination.fill(0);
    for (index, word) in target.iter().enumerate() {
        destination[index * 2..index * 2 + 2].copy_from_slice(&word.to_ne_bytes());
    }
    // Publish the completed destination last, while both processes hold the named mutex.
    mapping[WINSCP_DRAGGING_OFFSET] = 0;
    true
}

fn wide_paths_equal_ignore_case(left: &[u16], right: &[u16]) -> bool {
    (unsafe { CompareStringOrdinal(left, right, true) }) == CSTR_EQUAL
}

fn short_path_wide(path: &[u16]) -> Option<Vec<u16>> {
    let mut terminated = path.to_vec();
    terminated.push(0);
    let mut short = vec![0; LEGACY_SHELL_MAX_PATH];
    let length =
        unsafe { GetShortPathNameW(PCWSTR(terminated.as_ptr()), Some(&mut short)) } as usize;
    if length == 0 || length >= short.len() {
        None
    } else {
        short.truncate(length);
        Some(short)
    }
}

fn winscp_source_matches(source: &Path, mapped_source: &[u16]) -> bool {
    let Some(source) = legacy_shell_path_wide(source) else {
        return false;
    };
    wide_paths_equal_ignore_case(&source, mapped_source)
        || short_path_wide(&source)
            .zip(short_path_wide(mapped_source))
            .is_some_and(|(source, mapped)| wide_paths_equal_ignore_case(&source, &mapped))
}

fn commit_winscp_mapping_for_drop(mapping: &mut [u8], source: &Path, target: &[u16]) -> bool {
    if !winscp_mapping_is_active(mapping) {
        return false;
    }
    let Some(mapped_source) = winscp_mapping_drop_dest(mapping) else {
        return false;
    };
    winscp_source_matches(source, &mapped_source) && commit_winscp_mapping(mapping, target)
}

pub(super) fn bridge_winscp_fake_directory_drop(
    paths: &[std::path::PathBuf],
    destination: &Path,
) -> WinScpDropBridgeResult {
    let Some((source, target)) = winscp_bridge_candidate(paths, destination) else {
        return WinScpDropBridgeResult::NotApplicable;
    };

    let mapping =
        match unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, WINSCP_MAPPING_NAME) } {
            Ok(mapping) => OwnedHandle(mapping),
            Err(_) => return WinScpDropBridgeResult::NotApplicable,
        };
    let mutex = match unsafe { OpenMutexW(WINSCP_MUTEX_ACCESS, false, WINSCP_MUTEX_NAME) } {
        Ok(mutex) => OwnedHandle(mutex),
        Err(_) => return WinScpDropBridgeResult::FailedBeforeCommit,
    };
    let wait = unsafe { WaitForSingleObject(mutex.0, WINSCP_MUTEX_TIMEOUT_MS) };
    if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED {
        return WinScpDropBridgeResult::FailedBeforeCommit;
    }
    let _mutex_guard = MutexGuard(mutex.0);

    let view = unsafe { MapViewOfFile(mapping.0, FILE_MAP_ALL_ACCESS, 0, 0, WINSCP_MAPPING_SIZE) };
    if view.Value.is_null() {
        return WinScpDropBridgeResult::FailedBeforeCommit;
    }
    let view = MappedView(view);
    let mapping =
        unsafe { std::slice::from_raw_parts_mut(view.0.Value.cast::<u8>(), WINSCP_MAPPING_SIZE) };
    if !commit_winscp_mapping_for_drop(mapping, source, &target) {
        return WinScpDropBridgeResult::FailedBeforeCommit;
    }

    WinScpDropBridgeResult::Committed
}

struct ShellFileOperationRequest {
    _sources: Vec<u16>,
    _destination: Vec<u16>,
    operation: SHFILEOPSTRUCTW,
}

impl ShellFileOperationRequest {
    fn new(
        operation: ShellFileOperation,
        paths: &[std::path::PathBuf],
        destination: &Path,
        parent: Option<HWND>,
    ) -> Self {
        let sources = double_null_terminated_wide_paths(paths.iter().map(|path| path.as_path()));
        let destination = double_null_terminated_wide_paths(std::iter::once(destination));
        let operation = SHFILEOPSTRUCTW {
            hwnd: parent.unwrap_or_default(),
            wFunc: match operation {
                ShellFileOperation::Copy => FO_COPY,
                ShellFileOperation::Move => FO_MOVE,
            },
            pFrom: PCWSTR(sources.as_ptr()),
            pTo: PCWSTR(destination.as_ptr()),
            fFlags: FOF_ALLOWUNDO.0 as u16,
            ..Default::default()
        };

        Self {
            _sources: sources,
            _destination: destination,
            operation,
        }
    }

    #[cfg(test)]
    fn sources(&self) -> &[u16] {
        &self._sources
    }

    #[cfg(test)]
    fn destination(&self) -> &[u16] {
        &self._destination
    }
}

fn shell_file_operation_result(result: i32, operations_aborted: bool) -> ShellFileOperationResult {
    if operations_aborted || result == DE_OPCANCELLED || result == WINDOWS_ERROR_CANCELLED as i32 {
        ShellFileOperationResult::Aborted
    } else if result == 0 {
        ShellFileOperationResult::Completed
    } else {
        ShellFileOperationResult::Failed(result)
    }
}

pub(super) fn perform_shell_file_operation(
    operation: ShellFileOperation,
    paths: &[std::path::PathBuf],
    destination: &Path,
    parent: Option<HWND>,
) -> ShellFileOperationResult {
    debug_assert!(shell_file_operation_paths_supported(paths, destination));

    let mut request = ShellFileOperationRequest::new(operation, paths, destination, parent);
    let result = unsafe { SHFileOperationW(&mut request.operation) };
    shell_file_operation_result(result, request.operation.fAnyOperationsAborted.as_bool())
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
    use std::{os::windows::ffi::OsStrExt, path::PathBuf};

    fn absolute_test_path(name: &str) -> PathBuf {
        PathBuf::from(format!(r"C:\shell-test\{name}"))
    }

    #[test]
    fn shell_file_operation_request_encodes_multistrings_and_flags() {
        let sources = vec![absolute_test_path("first"), absolute_test_path("second")];
        let destination = absolute_test_path("destination");
        let request =
            ShellFileOperationRequest::new(ShellFileOperation::Move, &sources, &destination, None);

        let expected_sources =
            double_null_terminated_wide_paths(sources.iter().map(|path| path.as_path()));
        let expected_destination =
            double_null_terminated_wide_paths(std::iter::once(destination.as_path()));
        assert_eq!(request.sources(), expected_sources);
        assert_eq!(request.destination(), expected_destination);
        assert!(request.sources().ends_with(&[0, 0]));
        assert!(request.destination().ends_with(&[0, 0]));
        assert_eq!(request.operation.wFunc, FO_MOVE);
        assert_eq!(request.operation.fFlags, FOF_ALLOWUNDO.0 as u16);
    }

    #[test]
    fn legacy_shell_paths_require_absolute_short_paths_and_normalize_verbatim_prefixes() {
        assert!(legacy_shell_path_is_supported(&absolute_test_path(
            "source"
        )));
        assert!(!legacy_shell_path_is_supported(Path::new("relative")));
        assert_eq!(
            legacy_shell_path_wide(Path::new(r"\\?\C:\shell-test\source")),
            Some(
                Path::new(r"C:\shell-test\source")
                    .as_os_str()
                    .encode_wide()
                    .collect()
            )
        );
        assert_eq!(
            legacy_shell_path_wide(Path::new(r"\\?\UNC\server\share\source")),
            Some(
                Path::new(r"\\server\share\source")
                    .as_os_str()
                    .encode_wide()
                    .collect()
            )
        );
        assert!(!legacy_shell_path_is_supported(Path::new(
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\source"
        )));

        let too_long = PathBuf::from(format!(r"C:\{}", "a".repeat(257)));
        assert_eq!(too_long.as_os_str().encode_wide().count(), 260);
        assert!(!legacy_shell_path_is_supported(&too_long));
    }

    #[test]
    fn shell_file_operation_result_distinguishes_completion_abort_and_failure() {
        assert_eq!(
            shell_file_operation_result(0, false),
            ShellFileOperationResult::Completed
        );
        assert_eq!(
            shell_file_operation_result(0, true),
            ShellFileOperationResult::Aborted
        );
        assert_eq!(
            shell_file_operation_result(DE_OPCANCELLED, false),
            ShellFileOperationResult::Aborted
        );
        assert_eq!(
            shell_file_operation_result(WINDOWS_ERROR_CANCELLED as i32, false),
            ShellFileOperationResult::Aborted
        );
        assert_eq!(
            shell_file_operation_result(0x71, false),
            ShellFileOperationResult::Failed(0x71)
        );
    }

    fn drop_context(
        allowed_effects: u32,
        preferred_effect: Option<u32>,
        key_state: u32,
    ) -> gpui::WindowsExternalDropContext {
        gpui::WindowsExternalDropContext {
            allowed_effects,
            preferred_effect,
            key_state,
        }
    }

    #[test]
    fn native_shell_operation_honors_modifiers_then_source_preference() {
        assert_eq!(
            resolve_native_shell_file_operation(
                Some(drop_context(3, Some(DROPEFFECT_COPY_VALUE), 0)),
                ShellFileOperation::Move,
            ),
            Some(ShellFileOperation::Copy)
        );
        assert_eq!(
            resolve_native_shell_file_operation(
                Some(drop_context(3, Some(DROPEFFECT_MOVE_VALUE), 0)),
                ShellFileOperation::Copy,
            ),
            Some(ShellFileOperation::Move)
        );
        assert_eq!(
            resolve_native_shell_file_operation(
                Some(drop_context(3, Some(DROPEFFECT_COPY_VALUE), MK_SHIFT_VALUE)),
                ShellFileOperation::Copy,
            ),
            Some(ShellFileOperation::Move)
        );
        assert_eq!(
            resolve_native_shell_file_operation(
                Some(drop_context(
                    3,
                    Some(DROPEFFECT_MOVE_VALUE),
                    MK_CONTROL_VALUE
                )),
                ShellFileOperation::Move,
            ),
            Some(ShellFileOperation::Copy)
        );
        assert_eq!(
            resolve_native_shell_file_operation(
                Some(drop_context(
                    7,
                    Some(DROPEFFECT_COPY_VALUE),
                    MK_CONTROL_VALUE | MK_SHIFT_VALUE
                )),
                ShellFileOperation::Copy,
            ),
            None
        );
        assert_eq!(
            resolve_native_shell_file_operation(
                Some(drop_context(7, Some(DROPEFFECT_LINK_VALUE), 0)),
                ShellFileOperation::Copy,
            ),
            None
        );
        assert_eq!(
            resolve_native_shell_file_operation(
                Some(drop_context(
                    DROPEFFECT_MOVE_VALUE,
                    Some(DROPEFFECT_COPY_VALUE),
                    0,
                )),
                ShellFileOperation::Move,
            ),
            Some(ShellFileOperation::Move)
        );
        assert_eq!(
            resolve_native_shell_file_operation(None, ShellFileOperation::Move),
            Some(ShellFileOperation::Move)
        );
    }

    fn mapping_bytes(version: i32, dragging: bool, source: &[u16]) -> Vec<u8> {
        let mut mapping = vec![0; WINSCP_MAPPING_SIZE];
        mapping[WINSCP_VERSION_OFFSET..WINSCP_VERSION_OFFSET + 4]
            .copy_from_slice(&version.to_ne_bytes());
        mapping[WINSCP_DRAGGING_OFFSET] = u8::from(dragging);
        for (index, word) in source.iter().enumerate() {
            mapping[WINSCP_DROP_DEST_OFFSET + index * 2..WINSCP_DROP_DEST_OFFSET + index * 2 + 2]
                .copy_from_slice(&word.to_ne_bytes());
        }
        mapping
    }

    #[test]
    fn winscp_mapping_layout_is_528_bytes_and_commits_destination_last() {
        let source = absolute_test_path("scp-source")
            .as_os_str()
            .encode_wide()
            .collect::<Vec<_>>();
        let target = absolute_test_path("destination\\scp-source")
            .as_os_str()
            .encode_wide()
            .collect::<Vec<_>>();
        let mut mapping = mapping_bytes(WINSCP_PROTOCOL_VERSION, true, &source);

        assert_eq!(mapping.len(), 528);
        assert_eq!(winscp_mapping_drop_dest(&mapping), Some(source));
        assert!(commit_winscp_mapping(&mut mapping, &target));
        assert_eq!(mapping[WINSCP_DRAGGING_OFFSET], 0);
        assert_eq!(winscp_mapping_drop_dest(&mapping), Some(target));
    }

    #[test]
    fn winscp_mapping_rejects_inactive_unknown_and_overlong_protocol_data() {
        let source = absolute_test_path("scp-source")
            .as_os_str()
            .encode_wide()
            .collect::<Vec<_>>();
        let target = absolute_test_path("destination\\scp-source")
            .as_os_str()
            .encode_wide()
            .collect::<Vec<_>>();

        assert!(!commit_winscp_mapping(
            &mut mapping_bytes(2, true, &source),
            &target,
        ));
        assert!(!commit_winscp_mapping(
            &mut mapping_bytes(WINSCP_PROTOCOL_VERSION, false, &source),
            &target,
        ));
        assert!(!commit_winscp_mapping(
            &mut mapping_bytes(WINSCP_PROTOCOL_VERSION, true, &source),
            &vec![b'x' as u16; WINSCP_DROP_DEST_WORDS],
        ));
    }

    #[test]
    fn winscp_mapping_commits_only_for_the_matching_source() {
        let source = Path::new(r"C:\shell-test\scp-source");
        let mapped_source = source.as_os_str().encode_wide().collect::<Vec<_>>();
        let target = Path::new(r"C:\shell-test\destination\scp-source")
            .as_os_str()
            .encode_wide()
            .collect::<Vec<_>>();
        let mut matching = mapping_bytes(WINSCP_PROTOCOL_VERSION, true, &mapped_source);

        assert!(commit_winscp_mapping_for_drop(
            &mut matching,
            Path::new(r"c:\SHELL-TEST\SCP-SOURCE"),
            &target,
        ));
        assert_eq!(matching[WINSCP_DRAGGING_OFFSET], 0);

        let mut mismatching = mapping_bytes(WINSCP_PROTOCOL_VERSION, true, &mapped_source);
        let original = mismatching.clone();
        assert!(!commit_winscp_mapping_for_drop(
            &mut mismatching,
            Path::new(r"C:\shell-test\scp-other"),
            &target,
        ));
        assert_eq!(mismatching, original);
    }

    #[test]
    fn winscp_bridge_candidate_requires_one_scp_directory() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("scp1234");
        let other = temp.path().join("ordinary");
        let destination = temp.path().join("destination");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&other).unwrap();
        std::fs::create_dir(&destination).unwrap();

        assert!(winscp_bridge_candidate(std::slice::from_ref(&source), &destination).is_some());
        assert!(winscp_bridge_candidate(std::slice::from_ref(&other), &destination).is_none());
        assert!(winscp_bridge_candidate(&[source.clone(), other], &destination).is_none());
    }
}
