use std::{
    io,
    path::{Path, PathBuf},
};

use gpui::{Context, Window};

#[cfg(target_os = "macos")]
use crate::explorer::context_menu::ContextMenuIcon;
use crate::explorer::{
    context_menu::{ContextMenuCommand, ContextMenuItem},
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
            icon: None,
            label: "Open with".to_owned(),
            children,
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        ContextMenuItem::Action {
            id: "context-menu-entry-open-with".to_owned(),
            icon: None,
            label: "Open with".to_owned(),
            command: ContextMenuCommand::ChooseApplication {
                path: path.to_path_buf(),
            },
            enabled: true,
        }
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

        #[cfg(not(target_os = "linux"))]
        let _ = cx;

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
            for path in paths {
                let result = windows_open_file(&path, &intent, parent);
                let completed = matches!(result, Ok(OpenWithOutcome::Opened));
                self.handle_open_with_result(&path, result);
                if !completed {
                    break;
                }
            }
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
                windows_choose_application(path, parent)
            }
            Err(error) => Err(error),
        },
        OpenFileIntent::ChooseApplication => windows_choose_application(path, parent),
    }
}

#[cfg(any(target_os = "windows", test))]
fn windows_error_is_no_association(error: &io::Error) -> bool {
    error.raw_os_error() == Some(1155)
}

#[cfg(target_os = "windows")]
fn windows_choose_application(
    path: &Path,
    parent: Option<windows::Win32::Foundation::HWND>,
) -> io::Result<OpenWithOutcome> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::{
            Foundation::ERROR_CANCELLED,
            UI::Shell::{OAIF_EXEC, OPENASINFO, SHOpenWithDialog},
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
        oaifInFlags: OAIF_EXEC,
    };

    match unsafe { SHOpenWithDialog(parent, &info) } {
        Ok(()) => Ok(OpenWithOutcome::Opened),
        Err(error) if error.code() == HRESULT::from_win32(ERROR_CANCELLED.0) => {
            Ok(OpenWithOutcome::Cancelled)
        }
        Err(error) => Err(io::Error::other(error)),
    }
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

#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Eq, PartialEq)]
struct MacApplication {
    name: String,
    path: PathBuf,
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
