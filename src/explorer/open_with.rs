use std::{
    io,
    path::{Path, PathBuf},
};

use gpui::{Context, Window};

use crate::explorer::{
    context_menu::{ContextMenuCommand, ContextMenuIcon, ContextMenuItem},
    filesystem::format_open_error,
    view::ExplorerView,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum OpenFileIntent {
    Default,
    ChooseApplication,
    #[cfg(target_os = "macos")]
    SpecificApplication(PathBuf),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OpenWithOutcome {
    Opened,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DefaultAppChangeOutcome {
    Changed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DefaultApplication {
    pub(super) name: String,
    pub(super) path: Option<PathBuf>,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct LinuxDefaultApplicationChoice {
    pub(super) name: String,
    pub(super) desktop_id: String,
    pub(super) compatible: bool,
    pub(super) current_default: bool,
}

#[cfg(target_os = "linux")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct LinuxDefaultApplicationChoices {
    pub(super) mime_type: String,
    pub(super) choices: Vec<LinuxDefaultApplicationChoice>,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Debug, Eq, PartialEq)]
struct LinuxDesktopEntryInfo {
    name: String,
    desktop_id: String,
    type_name: Option<String>,
    hidden: bool,
    no_display: bool,
    terminal: bool,
    has_exec: bool,
    dbus_activatable: bool,
    mime_types: Vec<String>,
}

pub(super) fn context_menu_item(path: &Path) -> ContextMenuItem {
    #[cfg(target_os = "macos")]
    {
        let mut children = mac_compatible_applications(path)
            .into_iter()
            .enumerate()
            .map(|(index, application)| ContextMenuItem::Action {
                id: format!("context-menu-entry-open-with-application-{index}"),
                icon: Some(ContextMenuIcon::NativePathOptional(
                    application.path.clone(),
                )),
                label: application.name,
                command: ContextMenuCommand::OpenWithApplication {
                    target: path.to_path_buf(),
                    application: application.path,
                },
                enabled: true,
            })
            .collect::<Vec<_>>();
        if !children.is_empty() {
            children.push(ContextMenuItem::Separator);
        }
        children.push(ContextMenuItem::Action {
            id: "context-menu-entry-open-with-other".to_owned(),
            icon: None,
            label: "Other...".to_owned(),
            command: ContextMenuCommand::ChooseApplication {
                path: path.to_path_buf(),
            },
            enabled: true,
        });

        ContextMenuItem::Submenu {
            id: "context-menu-entry-open-with".to_owned(),
            icon: Some(ContextMenuIcon::OpenWith),
            label: "Open with".to_owned(),
            children,
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        ContextMenuItem::Action {
            id: "context-menu-entry-open-with".to_owned(),
            icon: Some(ContextMenuIcon::OpenWith),
            label: "Open with".to_owned(),
            command: ContextMenuCommand::ChooseApplication {
                path: path.to_path_buf(),
            },
            enabled: true,
        }
    }
}

pub(super) fn default_application_for_file(path: &Path) -> Option<DefaultApplication> {
    #[cfg(target_os = "windows")]
    {
        return windows_default_application_for_file(path);
    }

    #[cfg(target_os = "macos")]
    {
        return mac_default_application_for_file(path);
    }

    #[cfg(target_os = "linux")]
    {
        return linux_default_application_for_file(path);
    }

    #[allow(unreachable_code)]
    None
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(super) fn change_default_application_for_file(
    path: &Path,
    window: &Window,
) -> io::Result<DefaultAppChangeOutcome> {
    #[cfg(target_os = "windows")]
    {
        return windows_change_default_application_for_file(path, window);
    }

    #[cfg(target_os = "macos")]
    {
        return mac_change_default_application_for_file(path, window);
    }
}

impl ExplorerView {
    pub(super) fn open_file_with_default_app(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_files_with_intent(
            vec![path.to_path_buf()],
            OpenFileIntent::Default,
            window,
            cx,
        );
    }

    pub(super) fn choose_application_for_file(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_files_with_intent(vec![path], OpenFileIntent::ChooseApplication, window, cx);
    }

    #[cfg(target_os = "macos")]
    pub(super) fn open_file_with_application(
        &mut self,
        path: PathBuf,
        application: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_files_with_intent(
            vec![path],
            OpenFileIntent::SpecificApplication(application),
            window,
            cx,
        );
    }

    pub(super) fn open_files_with_default_app(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_files_with_intent(paths, OpenFileIntent::Default, window, cx);
    }

    fn open_files_with_intent(
        &mut self,
        paths: Vec<PathBuf>,
        intent: OpenFileIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() || self.open_with_task.is_some() {
            return;
        }

        #[cfg(target_os = "linux")]
        {
            use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

            let window_handle = HasWindowHandle::window_handle(window)
                .ok()
                .map(|handle| handle.as_raw());
            let display_handle = HasDisplayHandle::display_handle(window)
                .ok()
                .map(|handle| handle.as_raw());
            let task = cx.spawn(async move |this, cx| {
                let mut result = Ok(OpenWithOutcome::Opened);
                let mut result_path = paths.first().cloned();
                for path in &paths {
                    result_path = Some(path.clone());
                    result = linux_open_file(
                        path,
                        &intent,
                        window_handle.as_ref(),
                        display_handle.as_ref(),
                    )
                    .await;
                    if !matches!(result, Ok(OpenWithOutcome::Opened)) {
                        break;
                    }
                }
                let result = (result_path, result);

                let _ = this.update(cx, |explorer, cx| {
                    explorer.open_with_task = None;
                    if let (Some(path), result) = result {
                        explorer.handle_open_with_result(&path, result);
                    }
                    cx.notify();
                });
            });
            self.open_with_task = Some(task);
        }

        #[cfg(target_os = "windows")]
        {
            let parent = windows_parent_hwnd(window);
            let task = cx.spawn(async move |this, cx| {
                let results = open_paths_until_not_opened(paths, |path| {
                    windows_open_file(path, &intent, parent)
                });

                let _ = this.update(cx, |explorer, cx| {
                    explorer.open_with_task = None;
                    for (path, result) in results {
                        explorer.handle_open_with_result(&path, result);
                    }
                    cx.notify();
                });
            });
            self.open_with_task = Some(task);
        }

        #[cfg(target_os = "macos")]
        {
            let _ = window;
            let _ = cx;
            for path in paths {
                let result = mac_open_file(&path, &intent);
                let completed = matches!(result, Ok(OpenWithOutcome::Opened));
                self.handle_open_with_result(&path, result);
                if !completed {
                    break;
                }
            }
        }
    }

    pub(super) fn handle_open_with_result(
        &mut self,
        path: &Path,
        result: io::Result<OpenWithOutcome>,
    ) {
        match result {
            Ok(OpenWithOutcome::Opened) => self.open_error = None,
            Ok(OpenWithOutcome::Cancelled) => {}
            Err(error) => self.open_error = Some(format_open_error(path, &error)),
        }
    }
}

#[cfg(any(target_os = "windows", test))]
fn open_paths_until_not_opened(
    paths: Vec<PathBuf>,
    mut open_path: impl FnMut(&Path) -> io::Result<OpenWithOutcome>,
) -> Vec<(PathBuf, io::Result<OpenWithOutcome>)> {
    let mut results = Vec::new();
    for path in paths {
        let result = open_path(&path);
        let completed = matches!(result, Ok(OpenWithOutcome::Opened));
        results.push((path, result));
        if !completed {
            break;
        }
    }
    results
}

#[cfg(target_os = "windows")]
fn windows_parent_hwnd(window: &Window) -> Option<windows::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;

    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut _)),
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn windows_open_file(
    path: &Path,
    intent: &OpenFileIntent,
    parent: Option<windows::Win32::Foundation::HWND>,
) -> io::Result<OpenWithOutcome> {
    match intent {
        OpenFileIntent::Default => match open::that_detached(path) {
            Ok(()) => Ok(OpenWithOutcome::Opened),
            Err(error) if windows_error_is_no_association(&error) => {
                windows_choose_application(path, parent, false)
            }
            Err(error) => Err(error),
        },
        OpenFileIntent::ChooseApplication => windows_choose_application(path, parent, false),
    }
}

#[cfg(any(target_os = "windows", test))]
fn windows_error_is_no_association(error: &io::Error) -> bool {
    error.raw_os_error() == Some(1155)
}

#[cfg(any(target_os = "windows", test))]
const WINDOWS_OAIF_ALLOW_REGISTRATION: i32 = 1;
#[cfg(any(target_os = "windows", test))]
const WINDOWS_OAIF_REGISTER_EXT: i32 = 2;
#[cfg(any(target_os = "windows", test))]
const WINDOWS_OAIF_EXEC: i32 = 4;
#[cfg(any(target_os = "windows", test))]
const WINDOWS_OAIF_FORCE_REGISTRATION: i32 = 8;

#[cfg(any(target_os = "windows", test))]
fn windows_open_with_dialog_flag_bits(execute: bool, register_default: bool) -> i32 {
    let mut flags = 0;
    if execute {
        flags |= WINDOWS_OAIF_EXEC;
    }
    if register_default {
        flags |= WINDOWS_OAIF_ALLOW_REGISTRATION
            | WINDOWS_OAIF_REGISTER_EXT
            | WINDOWS_OAIF_FORCE_REGISTRATION;
    }
    flags
}

#[cfg(target_os = "windows")]
fn windows_open_with_dialog_flags(
    execute: bool,
    register_default: bool,
) -> windows::Win32::UI::Shell::OPEN_AS_INFO_FLAGS {
    windows::Win32::UI::Shell::OPEN_AS_INFO_FLAGS(windows_open_with_dialog_flag_bits(
        execute,
        register_default,
    ))
}

#[cfg(target_os = "windows")]
fn windows_show_open_with_dialog(
    path: &Path,
    parent: Option<windows::Win32::Foundation::HWND>,
    execute: bool,
    register_default: bool,
) -> io::Result<bool> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::{
            Foundation::ERROR_CANCELLED,
            UI::Shell::{OPENASINFO, SHOpenWithDialog},
        },
        core::{HRESULT, PCWSTR},
    };

    let path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let info = OPENASINFO {
        pcszFile: PCWSTR::from_raw(path.as_ptr()),
        pcszClass: PCWSTR::null(),
        oaifInFlags: windows_open_with_dialog_flags(execute, register_default),
    };

    match unsafe { SHOpenWithDialog(parent, &info) } {
        Ok(()) => Ok(true),
        Err(error) if error.code() == HRESULT::from_win32(ERROR_CANCELLED.0) => Ok(false),
        Err(error) => Err(io::Error::other(error)),
    }
}

#[cfg(target_os = "windows")]
fn windows_choose_application(
    path: &Path,
    parent: Option<windows::Win32::Foundation::HWND>,
    register_default: bool,
) -> io::Result<OpenWithOutcome> {
    if windows_show_open_with_dialog(path, parent, true, register_default)? {
        Ok(OpenWithOutcome::Opened)
    } else {
        Ok(OpenWithOutcome::Cancelled)
    }
}

#[cfg(target_os = "windows")]
fn windows_change_default_application_for_file(
    path: &Path,
    window: &Window,
) -> io::Result<DefaultAppChangeOutcome> {
    if windows_show_open_with_dialog(path, windows_parent_hwnd(window), false, true)? {
        Ok(DefaultAppChangeOutcome::Changed)
    } else {
        Ok(DefaultAppChangeOutcome::Cancelled)
    }
}

#[cfg(target_os = "windows")]
fn windows_default_application_for_file(path: &Path) -> Option<DefaultApplication> {
    let association = windows_file_association_query(path)?;
    let name = windows_assoc_query_string(&association, windows_assoc_friendly_app_name())
        .or_else(|| {
            windows_assoc_query_string(&association, windows_assoc_executable())
                .and_then(|path| windows_executable_display_name(Path::new(&path)))
        })?;
    let path =
        windows_assoc_query_string(&association, windows_assoc_executable()).map(PathBuf::from);

    Some(DefaultApplication { name, path })
}

#[cfg(any(target_os = "windows", test))]
fn windows_file_association_query(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    if name.starts_with('.') && !name[1..].contains('.') {
        return Some(name.into_owned());
    }

    path.extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| !extension.is_empty())
        .map(|extension| format!(".{extension}"))
}

#[cfg(target_os = "windows")]
fn windows_assoc_friendly_app_name() -> windows::Win32::UI::Shell::ASSOCSTR {
    windows::Win32::UI::Shell::ASSOCSTR_FRIENDLYAPPNAME
}

#[cfg(target_os = "windows")]
fn windows_assoc_executable() -> windows::Win32::UI::Shell::ASSOCSTR {
    windows::Win32::UI::Shell::ASSOCSTR_EXECUTABLE
}

#[cfg(target_os = "windows")]
fn windows_assoc_query_string(
    association: &str,
    query: windows::Win32::UI::Shell::ASSOCSTR,
) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::UI::Shell::{ASSOCF_INIT_IGNOREUNKNOWN, AssocQueryStringW},
        core::{PCWSTR, PWSTR},
    };

    let association = std::ffi::OsStr::new(association)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut len = 0u32;
    unsafe {
        let _ = AssocQueryStringW(
            ASSOCF_INIT_IGNOREUNKNOWN,
            query,
            PCWSTR::from_raw(association.as_ptr()),
            PCWSTR::null(),
            None,
            &mut len,
        );
    }
    if len == 0 {
        return None;
    }

    let mut output = vec![0u16; len as usize];
    let result = unsafe {
        AssocQueryStringW(
            ASSOCF_INIT_IGNOREUNKNOWN,
            query,
            PCWSTR::from_raw(association.as_ptr()),
            PCWSTR::null(),
            Some(PWSTR::from_raw(output.as_mut_ptr())),
            &mut len,
        )
    };
    if result.is_err() {
        return None;
    }

    output.truncate(
        output
            .iter()
            .position(|ch| *ch == 0)
            .unwrap_or(output.len()),
    );
    String::from_utf16(&output)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(target_os = "windows")]
fn windows_executable_display_name(path: &Path) -> Option<String> {
    path.file_stem()
        .or_else(|| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
}

#[cfg(target_os = "linux")]
async fn linux_open_file(
    path: &Path,
    intent: &OpenFileIntent,
    window_handle: Option<&raw_window_handle::RawWindowHandle>,
    display_handle: Option<&raw_window_handle::RawDisplayHandle>,
) -> io::Result<OpenWithOutcome> {
    use ashpd::{
        Error, WindowIdentifier,
        desktop::{ResponseError, open_uri::OpenFileRequest},
    };
    use std::fs::File;

    let ask = match intent {
        OpenFileIntent::ChooseApplication => true,
        OpenFileIntent::Default => {
            linux_should_ask_for_default(linux_has_default_application(path))
        }
    };
    let identifier = match window_handle {
        Some(window_handle) => {
            WindowIdentifier::from_raw_handle(window_handle, display_handle).await
        }
        None => None,
    };
    let file = File::open(path)?;
    let result = OpenFileRequest::default()
        .identifier(identifier)
        .ask(ask)
        .send_file(&file)
        .await
        .and_then(|request| request.response());

    match result {
        Ok(()) => Ok(OpenWithOutcome::Opened),
        Err(Error::Response(ResponseError::Cancelled)) => Ok(OpenWithOutcome::Cancelled),
        Err(error) if matches!(intent, OpenFileIntent::Default) => open::that_detached(path)
            .map(|_| OpenWithOutcome::Opened)
            .map_err(|fallback| {
                io::Error::other(format!(
                    "desktop portal failed ({error}); fallback opener failed ({fallback})"
                ))
            }),
        Err(error) => Err(io::Error::other(format!(
            "desktop Open With picker is unavailable: {error}"
        ))),
    }
}

#[cfg(target_os = "linux")]
fn linux_should_ask_for_default(has_default: Option<bool>) -> bool {
    matches!(has_default, Some(false))
}

#[cfg(target_os = "linux")]
fn linux_has_default_application(path: &Path) -> Option<bool> {
    use std::process::Command;

    let mime = Command::new("xdg-mime")
        .args(["query", "filetype"])
        .arg(path)
        .output()
        .ok()?;
    if !mime.status.success() {
        return None;
    }
    let mime = String::from_utf8_lossy(&mime.stdout).trim().to_owned();
    if mime.is_empty() {
        return None;
    }

    let default = Command::new("xdg-mime")
        .args(["query", "default", &mime])
        .output()
        .ok()?;
    default
        .status
        .success()
        .then(|| !String::from_utf8_lossy(&default.stdout).trim().is_empty())
}

#[cfg(target_os = "linux")]
fn linux_default_application_for_file(path: &Path) -> Option<DefaultApplication> {
    let mime = linux_file_mime_type(path)?;
    let default_id = linux_default_desktop_id_for_mime(&mime)?;
    let entries = freedesktop_desktop_entry::desktop_entries(&[]);
    let app_id = default_id.strip_suffix(".desktop").unwrap_or(&default_id);
    let entry = entries
        .iter()
        .find(|entry| entry.id() == app_id || format!("{}.desktop", entry.id()) == default_id);
    let name = entry
        .and_then(|entry| entry.full_name::<&str>(&[]))
        .or_else(|| entry.and_then(|entry| entry.name::<&str>(&[])))
        .map(|name| name.into_owned())
        .unwrap_or_else(|| default_id.clone());

    Some(DefaultApplication { name, path: None })
}

#[cfg(target_os = "linux")]
fn linux_file_mime_type(path: &Path) -> Option<String> {
    use std::process::Command;

    let output = Command::new("xdg-mime")
        .args(["query", "filetype"])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

#[cfg(target_os = "linux")]
fn linux_default_desktop_id_for_mime(mime: &str) -> Option<String> {
    use std::process::Command;

    let output = Command::new("xdg-mime")
        .args(["query", "default", mime])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

#[cfg(target_os = "linux")]
pub(super) fn linux_default_app_choices_for_file(
    path: &Path,
) -> io::Result<LinuxDefaultApplicationChoices> {
    let mime_type = linux_file_mime_type(path).ok_or_else(|| {
        io::Error::other(format!(
            "could not determine file type for {}",
            path.display()
        ))
    })?;
    let current_default = linux_default_desktop_id_for_mime(&mime_type);
    let entries = freedesktop_desktop_entry::desktop_entries(&[])
        .into_iter()
        .map(linux_desktop_entry_info)
        .collect::<Vec<_>>();
    let choices =
        linux_default_app_choices_from_entries(&mime_type, current_default.as_deref(), entries);

    Ok(LinuxDefaultApplicationChoices { mime_type, choices })
}

#[cfg(target_os = "linux")]
fn linux_desktop_entry_info(
    entry: freedesktop_desktop_entry::DesktopEntry,
) -> LinuxDesktopEntryInfo {
    let desktop_id = linux_desktop_id_with_suffix(entry.id());
    let name = entry
        .full_name::<&str>(&[])
        .or_else(|| entry.name::<&str>(&[]))
        .map(|name| name.into_owned())
        .unwrap_or_else(|| desktop_id.clone());
    let type_name = entry.desktop_entry("Type").map(str::to_owned);
    let has_exec = entry.exec().is_some() || entry.try_exec().is_some();
    let mime_types = entry
        .mime_type()
        .unwrap_or_default()
        .into_iter()
        .filter(|mime| !mime.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();

    LinuxDesktopEntryInfo {
        name,
        desktop_id,
        type_name,
        hidden: entry.hidden(),
        no_display: entry.no_display(),
        terminal: entry.terminal(),
        has_exec,
        dbus_activatable: entry.dbus_activatable(),
        mime_types,
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_desktop_id_with_suffix(desktop_id: &str) -> String {
    if desktop_id.ends_with(".desktop") {
        desktop_id.to_owned()
    } else {
        format!("{desktop_id}.desktop")
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_default_desktop_ids_match(left: &str, right: &str) -> bool {
    left.strip_suffix(".desktop").unwrap_or(left) == right.strip_suffix(".desktop").unwrap_or(right)
}

#[cfg(any(target_os = "linux", test))]
fn linux_default_app_choices_from_entries(
    mime_type: &str,
    current_default: Option<&str>,
    entries: Vec<LinuxDesktopEntryInfo>,
) -> Vec<LinuxDefaultApplicationChoice> {
    let mut seen = std::collections::HashSet::new();
    let mut choices = entries
        .into_iter()
        .filter(linux_desktop_entry_is_default_app_candidate)
        .filter(|entry| seen.insert(entry.desktop_id.clone()))
        .map(|entry| {
            let compatible = entry.mime_types.iter().any(|mime| mime == mime_type);
            let current_default = current_default
                .is_some_and(|default| linux_default_desktop_ids_match(default, &entry.desktop_id));
            LinuxDefaultApplicationChoice {
                name: entry.name,
                desktop_id: entry.desktop_id,
                compatible,
                current_default,
            }
        })
        .collect::<Vec<_>>();

    choices.sort_by(|left, right| {
        right
            .compatible
            .cmp(&left.compatible)
            .then_with(|| right.current_default.cmp(&left.current_default))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.desktop_id.cmp(&right.desktop_id))
    });
    choices
}

#[cfg(any(target_os = "linux", test))]
fn linux_desktop_entry_is_default_app_candidate(entry: &LinuxDesktopEntryInfo) -> bool {
    entry.type_name.as_deref() == Some("Application")
        && !entry.hidden
        && !entry.no_display
        && !entry.terminal
        && (entry.has_exec || entry.dbus_activatable)
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn linux_default_app_initial_selection(
    choices: &[LinuxDefaultApplicationChoice],
) -> Option<usize> {
    choices
        .iter()
        .position(|choice| choice.current_default)
        .or_else(|| choices.iter().position(|choice| choice.compatible))
        .or_else(|| (!choices.is_empty()).then_some(0))
}

#[cfg(any(target_os = "linux", test))]
fn linux_xdg_mime_default_args(desktop_id: &str, mime_type: &str) -> [String; 3] {
    [
        "default".to_owned(),
        linux_desktop_id_with_suffix(desktop_id),
        mime_type.to_owned(),
    ]
}

#[cfg(target_os = "linux")]
pub(super) fn linux_change_default_application(
    mime_type: &str,
    desktop_id: &str,
) -> io::Result<DefaultAppChangeOutcome> {
    use std::process::Command;

    let args = linux_xdg_mime_default_args(desktop_id, mime_type);
    let output = Command::new("xdg-mime").args(args.iter()).output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(io::Error::other(if stderr.is_empty() {
            format!("xdg-mime failed to set {desktop_id} as the default for {mime_type}")
        } else {
            stderr
        }));
    }

    let verified = linux_default_desktop_id_for_mime(mime_type).ok_or_else(|| {
        io::Error::other(format!(
            "could not verify the default application for {mime_type}"
        ))
    })?;
    if !linux_default_desktop_ids_match(&verified, desktop_id) {
        return Err(io::Error::other(format!(
            "the default application for {mime_type} is still {verified}"
        )));
    }

    Ok(DefaultAppChangeOutcome::Changed)
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Eq, PartialEq)]
struct MacApplication {
    name: String,
    path: PathBuf,
}

#[cfg(target_os = "macos")]
fn mac_change_default_application_for_file(
    path: &Path,
    _: &Window,
) -> io::Result<DefaultAppChangeOutcome> {
    let Some(application) = mac_choose_application()? else {
        return Ok(DefaultAppChangeOutcome::Cancelled);
    };
    mac_set_default_application_for_file_type(path, &application)?;
    Ok(DefaultAppChangeOutcome::Changed)
}

#[cfg(target_os = "macos")]
fn mac_open_file(path: &Path, intent: &OpenFileIntent) -> io::Result<OpenWithOutcome> {
    match intent {
        OpenFileIntent::Default
            if mac_is_application_bundle(path) || mac_has_default_application(path) =>
        {
            open::that_detached(path).map(|_| OpenWithOutcome::Opened)
        }
        OpenFileIntent::Default | OpenFileIntent::ChooseApplication => {
            let Some(application) = mac_choose_application()? else {
                return Ok(OpenWithOutcome::Cancelled);
            };
            mac_open_with_application(path, &application)
        }
        OpenFileIntent::SpecificApplication(application) => {
            mac_open_with_application(path, application)
        }
    }
}

#[cfg(target_os = "macos")]
const MAC_LS_ROLES_ALL: u32 = u32::MAX;

#[cfg(target_os = "macos")]
#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn LSSetDefaultRoleHandlerForContentType(
        in_content_type: cocoa::base::id,
        in_role: u32,
        in_handler_bundle_id: cocoa::base::id,
    ) -> i32;
}

#[cfg(target_os = "macos")]
fn mac_set_default_application_for_file_type(path: &Path, application: &Path) -> io::Result<()> {
    use cocoa::base::{id, nil};
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let pool: id = msg_send![class!(NSAutoreleasePool), new];
        let result = (|| {
            let content_type = mac_content_type_for_file(path)?;
            let application_url = mac_file_url(application)
                .ok_or_else(|| io::Error::other("could not create application URL"))?;
            let bundle: id = msg_send![class!(NSBundle), bundleWithURL: application_url];
            if bundle == nil {
                return Err(io::Error::other("selected application has no bundle"));
            }
            let bundle_id: id = msg_send![bundle, bundleIdentifier];
            if bundle_id == nil {
                return Err(io::Error::other(
                    "selected application has no bundle identifier",
                ));
            }

            let status =
                LSSetDefaultRoleHandlerForContentType(content_type, MAC_LS_ROLES_ALL, bundle_id);
            if status == 0 {
                Ok(())
            } else {
                Err(io::Error::other(format!(
                    "LaunchServices failed with status {status}"
                )))
            }
        })();
        let _: () = msg_send![pool, drain];
        result
    }
}

#[cfg(target_os = "macos")]
unsafe fn mac_content_type_for_file(path: &Path) -> io::Result<cocoa::base::id> {
    use cocoa::{
        base::{id, nil},
        foundation::NSString,
    };
    use objc::{class, msg_send, sel, sel_impl};

    let path = path
        .to_str()
        .ok_or_else(|| io::Error::other("file path is not valid UTF-8"))?;
    let ns_path = NSString::alloc(nil).init_str(path);
    let _: id = msg_send![ns_path, autorelease];
    let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
    let content_type: id = msg_send![workspace, typeOfFile: ns_path error: nil];
    if content_type == nil {
        Err(io::Error::other(
            "could not determine the file content type",
        ))
    } else {
        Ok(content_type)
    }
}

#[cfg(target_os = "macos")]
fn mac_open_with_application(path: &Path, application: &Path) -> io::Result<OpenWithOutcome> {
    use cocoa::{
        base::{id, nil},
        foundation::NSArray,
    };
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let pool: id = msg_send![class!(NSAutoreleasePool), new];
        let result = (|| {
            let file_url =
                mac_file_url(path).ok_or_else(|| io::Error::other("could not create file URL"))?;
            let application_url = mac_file_url(application)
                .ok_or_else(|| io::Error::other("could not create application URL"))?;
            let urls = NSArray::arrayWithObject(nil, file_url);
            let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
            let configuration: id = msg_send![class!(NSWorkspaceOpenConfiguration), configuration];
            if workspace == nil || configuration == nil {
                return Err(io::Error::other("NSWorkspace is unavailable"));
            }

            let _: () = msg_send![
                workspace,
                openURLs: urls
                withApplicationAtURL: application_url
                configuration: configuration
                completionHandler: nil
            ];
            Ok(OpenWithOutcome::Opened)
        })();
        let _: () = msg_send![pool, drain];
        result
    }
}

#[cfg(target_os = "macos")]
fn mac_is_application_bundle(path: &Path) -> bool {
    path.is_dir()
        && path
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
}

#[cfg(target_os = "macos")]
fn mac_has_default_application(path: &Path) -> bool {
    use cocoa::base::{id, nil};
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let pool: id = msg_send![class!(NSAutoreleasePool), new];
        let has_default = mac_file_url(path).is_some_and(|url| {
            let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
            let application: id = msg_send![workspace, URLForApplicationToOpenURL: url];
            application != nil
        });
        let _: () = msg_send![pool, drain];
        has_default
    }
}

#[cfg(target_os = "macos")]
fn mac_default_application_for_file(path: &Path) -> Option<DefaultApplication> {
    use cocoa::base::{id, nil};
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let pool: id = msg_send![class!(NSAutoreleasePool), new];
        let default = (|| {
            let url = mac_file_url(path)?;
            let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
            let application: id = msg_send![workspace, URLForApplicationToOpenURL: url];
            if application == nil {
                return None;
            }
            let path = mac_path_from_url(application)?;
            let name = path
                .file_stem()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned();
            Some(DefaultApplication {
                name,
                path: Some(path),
            })
        })();
        let _: () = msg_send![pool, drain];
        default
    }
}

#[cfg(target_os = "macos")]
fn mac_compatible_applications(path: &Path) -> Vec<MacApplication> {
    use cocoa::{
        base::{id, nil},
        foundation::NSArray,
    };
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let pool: id = msg_send![class!(NSAutoreleasePool), new];
        let applications = (|| {
            let url = mac_file_url(path)?;
            let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
            let urls: id = msg_send![workspace, URLsForApplicationsToOpenURL: url];
            if urls == nil {
                return None;
            }

            let mut applications = Vec::new();
            for index in 0..urls.count() {
                let url = urls.objectAtIndex(index);
                let Some(path) = mac_path_from_url(url) else {
                    continue;
                };
                let name = path
                    .file_stem()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned();
                applications.push(MacApplication { name, path });
            }
            Some(deduplicate_mac_applications(applications))
        })()
        .unwrap_or_default();
        let _: () = msg_send![pool, drain];
        applications
    }
}

#[cfg(target_os = "macos")]
fn deduplicate_mac_applications(applications: Vec<MacApplication>) -> Vec<MacApplication> {
    let mut seen = std::collections::HashSet::new();
    applications
        .into_iter()
        .filter(|application| seen.insert(application.path.clone()))
        .collect()
}

#[cfg(target_os = "macos")]
fn mac_choose_application() -> io::Result<Option<PathBuf>> {
    use cocoa::{
        appkit::{NSModalResponse, NSOpenPanel, NSSavePanel},
        base::{NO, YES, id, nil},
        foundation::{NSArray, NSString},
    };
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let pool: id = msg_send![class!(NSAutoreleasePool), new];
        let result = (|| {
            let panel = NSOpenPanel::openPanel(nil);
            panel.setCanChooseFiles_(YES);
            panel.setCanChooseDirectories_(NO);
            panel.setAllowsMultipleSelection_(NO);
            panel.setResolvesAliases_(YES);

            let applications_url = mac_file_url(Path::new("/Applications"))
                .ok_or_else(|| io::Error::other("could not create Applications URL"))?;
            panel.setDirectoryURL(applications_url);

            let app_type = NSString::alloc(nil).init_str("app");
            let _: id = msg_send![app_type, autorelease];
            let allowed_types = NSArray::arrayWithObject(nil, app_type);
            let _: () = msg_send![panel, setAllowedFileTypes: allowed_types];

            if panel.runModal() != NSModalResponse::NSModalResponseOk {
                return Ok(None);
            }

            let application = mac_path_from_url(panel.URL())
                .ok_or_else(|| io::Error::other("selected application path is unavailable"))?;
            if !mac_is_application_bundle(&application) {
                return Err(io::Error::other("selected item is not an application"));
            }
            Ok(Some(application))
        })();
        let _: () = msg_send![pool, drain];
        result
    }
}

#[cfg(target_os = "macos")]
unsafe fn mac_file_url(path: &Path) -> Option<cocoa::base::id> {
    use cocoa::{
        base::{id, nil},
        foundation::{NSString, NSURL},
    };
    use objc::{class, msg_send, sel, sel_impl};

    let path = path.to_str()?;
    let ns_path = NSString::alloc(nil).init_str(path);
    let _: id = msg_send![ns_path, autorelease];
    Some(NSURL::fileURLWithPath_(nil, ns_path))
}

#[cfg(target_os = "macos")]
unsafe fn mac_path_from_url(url: cocoa::base::id) -> Option<PathBuf> {
    use objc::{msg_send, sel, sel_impl};
    use std::{ffi::CStr, os::unix::ffi::OsStrExt};

    if url == cocoa::base::nil {
        return None;
    }
    let path: *const std::ffi::c_char = msg_send![url, fileSystemRepresentation];
    (!path.is_null())
        .then(|| PathBuf::from(std::ffi::OsStr::from_bytes(CStr::from_ptr(path).to_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_result_name(result: &io::Result<OpenWithOutcome>) -> &'static str {
        match result {
            Ok(OpenWithOutcome::Opened) => "opened",
            Ok(OpenWithOutcome::Cancelled) => "cancelled",
            Err(_) => "error",
        }
    }

    fn attempted_path_names(results: &[(PathBuf, io::Result<OpenWithOutcome>)]) -> Vec<String> {
        results
            .iter()
            .map(|(path, _)| path.to_string_lossy().into_owned())
            .collect()
    }

    fn open_result_names(results: &[(PathBuf, io::Result<OpenWithOutcome>)]) -> Vec<&'static str> {
        results
            .iter()
            .map(|(_, result)| open_result_name(result))
            .collect()
    }

    #[cfg(any(target_os = "linux", test))]
    fn linux_entry(name: &str, desktop_id: &str, mime_types: &[&str]) -> LinuxDesktopEntryInfo {
        LinuxDesktopEntryInfo {
            name: name.to_owned(),
            desktop_id: linux_desktop_id_with_suffix(desktop_id),
            type_name: Some("Application".to_owned()),
            hidden: false,
            no_display: false,
            terminal: false,
            has_exec: true,
            dbus_activatable: false,
            mime_types: mime_types.iter().map(|mime| (*mime).to_owned()).collect(),
        }
    }

    #[test]
    fn open_paths_until_not_opened_records_all_successful_attempts() {
        let results = open_paths_until_not_opened(
            vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")],
            |_| Ok(OpenWithOutcome::Opened),
        );

        assert_eq!(attempted_path_names(&results), vec!["a.txt", "b.txt"]);
        assert_eq!(open_result_names(&results), vec!["opened", "opened"]);
    }

    #[test]
    fn open_paths_until_not_opened_stops_after_first_error() {
        let results = open_paths_until_not_opened(
            vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")],
            |path| {
                if path == Path::new("a.txt") {
                    Err(io::Error::new(io::ErrorKind::NotFound, "missing"))
                } else {
                    Ok(OpenWithOutcome::Opened)
                }
            },
        );

        assert_eq!(attempted_path_names(&results), vec!["a.txt"]);
        assert_eq!(open_result_names(&results), vec!["error"]);
    }

    #[test]
    fn open_paths_until_not_opened_stops_after_later_error() {
        let results = open_paths_until_not_opened(
            vec![
                PathBuf::from("a.txt"),
                PathBuf::from("b.txt"),
                PathBuf::from("c.txt"),
            ],
            |path| {
                if path == Path::new("b.txt") {
                    Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
                } else {
                    Ok(OpenWithOutcome::Opened)
                }
            },
        );

        assert_eq!(attempted_path_names(&results), vec!["a.txt", "b.txt"]);
        assert_eq!(open_result_names(&results), vec!["opened", "error"]);
    }

    #[test]
    fn open_paths_until_not_opened_stops_after_cancelled_result() {
        let results = open_paths_until_not_opened(
            vec![
                PathBuf::from("a.txt"),
                PathBuf::from("b.txt"),
                PathBuf::from("c.txt"),
            ],
            |path| {
                if path == Path::new("b.txt") {
                    Ok(OpenWithOutcome::Cancelled)
                } else {
                    Ok(OpenWithOutcome::Opened)
                }
            },
        );

        assert_eq!(attempted_path_names(&results), vec!["a.txt", "b.txt"]);
        assert_eq!(open_result_names(&results), vec!["opened", "cancelled"]);
    }

    #[test]
    fn cancelled_open_does_not_replace_existing_error() {
        let mut view = ExplorerView::new(PathBuf::from("."));
        view.open_error = Some("existing".to_owned());

        view.handle_open_with_result(Path::new("file.txt"), Ok(OpenWithOutcome::Cancelled));

        assert_eq!(view.open_error.as_deref(), Some("existing"));
    }

    #[test]
    fn successful_open_clears_existing_error() {
        let mut view = ExplorerView::new(PathBuf::from("."));
        view.open_error = Some("existing".to_owned());

        view.handle_open_with_result(Path::new("file.txt"), Ok(OpenWithOutcome::Opened));

        assert_eq!(view.open_error, None);
    }

    #[test]
    fn failed_open_sets_existing_error_message() {
        let mut view = ExplorerView::new(PathBuf::from("."));

        view.handle_open_with_result(
            Path::new("file.txt"),
            Err(io::Error::new(io::ErrorKind::NotFound, "missing")),
        );

        assert_eq!(
            view.open_error.as_deref(),
            Some("Could not open file.txt: missing")
        );
    }

    #[test]
    fn windows_no_association_error_matches_only_platform_code() {
        assert!(windows_error_is_no_association(
            &io::Error::from_raw_os_error(1155)
        ));
        assert!(!windows_error_is_no_association(
            &io::Error::from_raw_os_error(2)
        ));
    }

    #[test]
    fn windows_association_query_treats_leading_dot_names_as_extensions() {
        assert_eq!(
            windows_file_association_query(Path::new(".gitignore")).as_deref(),
            Some(".gitignore")
        );
        assert_eq!(
            windows_file_association_query(Path::new("notes.txt")).as_deref(),
            Some(".txt")
        );
        assert_eq!(windows_file_association_query(Path::new("Makefile")), None);
    }

    #[test]
    fn windows_open_with_flags_for_file_open_execute_without_registration() {
        let flags = windows_open_with_dialog_flag_bits(true, false);

        assert_ne!(flags & WINDOWS_OAIF_EXEC, 0);
        assert_eq!(flags & WINDOWS_OAIF_ALLOW_REGISTRATION, 0);
        assert_eq!(flags & WINDOWS_OAIF_REGISTER_EXT, 0);
        assert_eq!(flags & WINDOWS_OAIF_FORCE_REGISTRATION, 0);
    }

    #[test]
    fn windows_default_app_change_flags_register_without_execute() {
        let flags = windows_open_with_dialog_flag_bits(false, true);

        assert_eq!(flags & WINDOWS_OAIF_EXEC, 0);
        assert_ne!(flags & WINDOWS_OAIF_ALLOW_REGISTRATION, 0);
        assert_ne!(flags & WINDOWS_OAIF_REGISTER_EXT, 0);
        assert_ne!(flags & WINDOWS_OAIF_FORCE_REGISTRATION, 0);
    }

    #[test]
    fn linux_default_app_choices_include_visible_apps_and_sort_compatible_first() {
        let mut hidden = linux_entry("Hidden App", "hidden", &["text/plain"]);
        hidden.hidden = true;
        let mut no_display = linux_entry("No Display", "no-display", &["text/plain"]);
        no_display.no_display = true;
        let mut terminal = linux_entry("Terminal App", "terminal", &["text/plain"]);
        terminal.terminal = true;
        let mut not_app = linux_entry("Link", "link", &["text/plain"]);
        not_app.type_name = Some("Link".to_owned());
        let mut unavailable = linux_entry("Unavailable", "unavailable", &["text/plain"]);
        unavailable.has_exec = false;
        unavailable.dbus_activatable = false;

        let choices = linux_default_app_choices_from_entries(
            "text/plain",
            Some("org.other.desktop"),
            vec![
                linux_entry("Other App", "org.other", &["image/png"]),
                hidden,
                no_display,
                terminal,
                not_app,
                unavailable,
                linux_entry("Text Editor", "org.editor", &["text/plain"]),
                linux_entry("Archive Manager", "org.archive", &[]),
            ],
        );

        assert_eq!(
            choices
                .iter()
                .map(|choice| choice.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Text Editor", "Other App", "Archive Manager"]
        );
        assert!(choices[0].compatible);
        assert!(choices[1].current_default);
        assert_eq!(linux_default_app_initial_selection(&choices), Some(1));
    }

    #[test]
    fn linux_xdg_mime_default_args_include_desktop_suffix_and_mime_type() {
        assert_eq!(
            linux_xdg_mime_default_args("org.editor", "text/plain"),
            [
                "default".to_owned(),
                "org.editor.desktop".to_owned(),
                "text/plain".to_owned()
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_default_open_asks_only_when_default_is_known_missing() {
        assert!(!linux_should_ask_for_default(Some(true)));
        assert!(linux_should_ask_for_default(Some(false)));
        assert!(!linux_should_ask_for_default(None));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_application_deduplication_preserves_first_seen_order() {
        let applications = vec![
            MacApplication {
                name: "First".to_owned(),
                path: PathBuf::from("/Applications/First.app"),
            },
            MacApplication {
                name: "Duplicate".to_owned(),
                path: PathBuf::from("/Applications/First.app"),
            },
            MacApplication {
                name: "Second".to_owned(),
                path: PathBuf::from("/Applications/Second.app"),
            },
        ];

        assert_eq!(
            deduplicate_mac_applications(applications)
                .into_iter()
                .map(|application| application.name)
                .collect::<Vec<_>>(),
            vec!["First", "Second"]
        );
    }
}
