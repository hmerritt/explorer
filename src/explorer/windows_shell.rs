use std::{
    ffi::OsStr,
    io,
    mem::size_of,
    path::{Path, PathBuf},
    sync::atomic::{Ordering, compiler_fence},
};

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
                SEE_MASK_CLASSKEY, SEE_MASK_CLASSNAME, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC,
                SHELLEXECUTEINFOW, ShellExecuteExW,
            },
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
    core::{PCWSTR, w},
};

pub(super) const WINDOWS_ERROR_CANCELLED: u32 = 1223;
const LEGACY_SHELL_MAX_PATH: usize = 260;
const WINSCP_MAPPING_NAME: PCWSTR = w!("WinSCPDragExtMapping");
const WINSCP_MUTEX_NAME: PCWSTR = w!("WinSCPDragExtMutex");
const WINSCP_MAPPING_SIZE: usize = 528;
const WINSCP_VERSION_OFFSET: usize = 0;
const WINSCP_DRAGGING_OFFSET: usize = 4;
const WINSCP_DROP_DEST_OFFSET: usize = 6;
const WINSCP_DROP_DEST_WORDS: usize = 260;
const WINSCP_PROTOCOL_VERSION: i32 = 1;
const WINSCP_MUTEX_TIMEOUT_MS: u32 = 1_000;
const WINSCP_MUTEX_ACCESS: SYNCHRONIZATION_ACCESS_RIGHTS =
    SYNCHRONIZATION_ACCESS_RIGHTS(0x0010_0000 | MUTEX_MODIFY_STATE.0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WinScpDropBridgeResult {
    NotApplicable,
    FailedBeforeCommit,
    Committed,
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

fn winscp_bridge_candidate<'a>(
    paths: &'a [PathBuf],
    destination: &Path,
) -> Option<(&'a Path, Vec<u16>)> {
    use std::os::windows::ffi::OsStrExt;

    let [source] = paths else {
        return None;
    };
    if !source.is_dir() {
        return None;
    }

    let file_name = source.file_name()?;
    let file_name_wide = file_name.encode_wide().collect::<Vec<_>>();
    if !file_name_wide.starts_with(&[b's' as u16, b'c' as u16, b'p' as u16]) {
        return None;
    }

    let target = legacy_shell_path_wide(&destination.join(file_name))?;
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
                .expect("validated WinSCP mapping version field"),
        ) == WINSCP_PROTOCOL_VERSION
        && mapping[WINSCP_DRAGGING_OFFSET] != 0
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

fn write_winscp_destination(
    mapping: &mut [u8],
    target: &[u16],
    before_publish: impl FnOnce(&[u8]),
) {
    let destination = &mut mapping[WINSCP_DROP_DEST_OFFSET
        ..WINSCP_DROP_DEST_OFFSET + WINSCP_DROP_DEST_WORDS * size_of::<u16>()];
    destination.fill(0);
    for (index, word) in target.iter().enumerate() {
        destination[index * size_of::<u16>()..(index + 1) * size_of::<u16>()]
            .copy_from_slice(&word.to_ne_bytes());
    }

    before_publish(mapping);
    compiler_fence(Ordering::Release);
    unsafe { std::ptr::write_volatile(mapping.as_mut_ptr().add(WINSCP_DRAGGING_OFFSET), 0) };
}

fn commit_winscp_mapping(mapping: &mut [u8], source: &Path, target: &[u16]) -> bool {
    if !winscp_mapping_is_active(mapping) || target.len() >= WINSCP_DROP_DEST_WORDS {
        return false;
    }
    let Some(mapped_source) = winscp_mapping_drop_dest(mapping) else {
        return false;
    };
    if !winscp_source_matches(source, &mapped_source) {
        return false;
    }

    // WinSCP polls this byte as the commit marker, so publish it after the complete destination.
    write_winscp_destination(mapping, target, |_| {});
    true
}

pub(super) fn bridge_winscp_fake_directory_drop(
    paths: &[PathBuf],
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
    if !commit_winscp_mapping(mapping, source, &target) {
        return WinScpDropBridgeResult::FailedBeforeCommit;
    }

    WinScpDropBridgeResult::Committed
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

    fn mapping_bytes(version: i32, dragging: bool, source: &[u16]) -> Vec<u8> {
        let mut mapping = vec![0; WINSCP_MAPPING_SIZE];
        mapping[WINSCP_VERSION_OFFSET..WINSCP_VERSION_OFFSET + size_of::<i32>()]
            .copy_from_slice(&version.to_ne_bytes());
        mapping[WINSCP_DRAGGING_OFFSET] = u8::from(dragging);
        for (index, word) in source.iter().enumerate() {
            mapping[WINSCP_DROP_DEST_OFFSET + index * size_of::<u16>()
                ..WINSCP_DROP_DEST_OFFSET + (index + 1) * size_of::<u16>()]
                .copy_from_slice(&word.to_ne_bytes());
        }
        mapping
    }

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

    #[test]
    fn winscp_protocol_layout_matches_the_528_byte_shared_structure() {
        #[repr(C)]
        struct ProtocolLayout {
            version: i32,
            dragging: u8,
            drop_dest: [u16; WINSCP_DROP_DEST_WORDS],
        }

        assert_eq!(size_of::<ProtocolLayout>(), WINSCP_MAPPING_SIZE);
        assert_eq!(
            std::mem::offset_of!(ProtocolLayout, version),
            WINSCP_VERSION_OFFSET
        );
        assert_eq!(
            std::mem::offset_of!(ProtocolLayout, dragging),
            WINSCP_DRAGGING_OFFSET
        );
        assert_eq!(
            std::mem::offset_of!(ProtocolLayout, drop_dest),
            WINSCP_DROP_DEST_OFFSET
        );
    }

    #[test]
    fn winscp_candidate_requires_exactly_one_existing_scp_directory() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("scp1234");
        let uppercase = temp.path().join("SCP1234");
        let ordinary = temp.path().join("ordinary");
        let file = temp.path().join("scp-file");
        let destination = temp.path().join("destination");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&ordinary).unwrap();
        std::fs::write(&file, b"not a directory").unwrap();
        std::fs::create_dir(&destination).unwrap();

        assert!(winscp_bridge_candidate(std::slice::from_ref(&source), &destination).is_some());
        assert!(winscp_bridge_candidate(std::slice::from_ref(&uppercase), &destination).is_none());
        assert!(winscp_bridge_candidate(std::slice::from_ref(&ordinary), &destination).is_none());
        assert!(winscp_bridge_candidate(std::slice::from_ref(&file), &destination).is_none());
        assert!(winscp_bridge_candidate(&[source.clone(), ordinary], &destination).is_none());
    }

    #[test]
    fn winscp_source_matching_is_case_insensitive_and_supports_short_paths() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("scp Long Source Name");
        std::fs::create_dir(&source).unwrap();
        let source_wide = legacy_shell_path_wide(&source).unwrap();
        assert!(winscp_source_matches(&source, &source_wide));

        let differently_cased = source
            .to_string_lossy()
            .to_ascii_uppercase()
            .encode_utf16()
            .collect::<Vec<_>>();
        assert!(winscp_source_matches(&source, &differently_cased));

        let short = short_path_wide(&source_wide).unwrap();
        assert!(winscp_source_matches(&source, &short));
    }

    #[test]
    fn winscp_mapping_rejects_inactive_future_mismatched_and_unterminated_sources_unchanged() {
        use std::os::windows::ffi::OsStrExt;

        let source = Path::new(r"C:\shell-test\scp-source");
        let source_wide = source.as_os_str().encode_wide().collect::<Vec<_>>();
        let target = Path::new(r"C:\shell-test\destination\scp-source")
            .as_os_str()
            .encode_wide()
            .collect::<Vec<_>>();

        let cases = [
            mapping_bytes(2, true, &source_wide),
            mapping_bytes(WINSCP_PROTOCOL_VERSION, false, &source_wide),
            mapping_bytes(
                WINSCP_PROTOCOL_VERSION,
                true,
                &Path::new(r"C:\shell-test\scp-other")
                    .as_os_str()
                    .encode_wide()
                    .collect::<Vec<_>>(),
            ),
        ];
        for mut mapping in cases {
            let original = mapping.clone();
            assert!(!commit_winscp_mapping(&mut mapping, source, &target));
            assert_eq!(mapping, original);
        }

        let mut unterminated = mapping_bytes(
            WINSCP_PROTOCOL_VERSION,
            true,
            &vec![b'x' as u16; WINSCP_DROP_DEST_WORDS],
        );
        let original = unterminated.clone();
        assert!(!commit_winscp_mapping(&mut unterminated, source, &target));
        assert_eq!(unterminated, original);
    }

    #[test]
    fn winscp_destination_is_encoded_cleared_and_published_last() {
        use std::{cell::Cell, os::windows::ffi::OsStrExt};

        let source = Path::new(r"C:\shell-test\scp-source");
        let source_wide = source.as_os_str().encode_wide().collect::<Vec<_>>();
        let target = Path::new(r"C:\shell-test\destination\scp-source")
            .as_os_str()
            .encode_wide()
            .collect::<Vec<_>>();
        let mut mapping = mapping_bytes(WINSCP_PROTOCOL_VERSION, true, &source_wide);
        mapping[WINSCP_DROP_DEST_OFFSET + (source_wide.len() + 1) * size_of::<u16>()..].fill(0xaa);

        let observed_before_publish = Cell::new(false);
        write_winscp_destination(&mut mapping, &target, |mapping| {
            assert_eq!(mapping[WINSCP_DRAGGING_OFFSET], 1);
            assert_eq!(winscp_mapping_drop_dest(mapping), Some(target.clone()));
            observed_before_publish.set(true);
        });

        assert!(observed_before_publish.get());
        assert_eq!(mapping[WINSCP_DRAGGING_OFFSET], 0);
        assert_eq!(winscp_mapping_drop_dest(&mapping), Some(target.clone()));
        let cleared_from = WINSCP_DROP_DEST_OFFSET + (target.len() + 1) * size_of::<u16>();
        assert!(
            mapping[cleared_from..WINSCP_MAPPING_SIZE - 2]
                .iter()
                .all(|byte| *byte == 0)
        );
    }
}
