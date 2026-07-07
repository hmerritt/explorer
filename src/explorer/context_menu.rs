use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

use git2::Repository;
use gpui::{ClipboardItem, Context, Pixels, Point, Window};

use crate::explorer::{
    DirectoryKind,
    entry::FileEntry,
    filesystem::archive_path_is_supported,
    filesystem::format_open_error,
    filesystem::mountable_image_path_is_supported,
    formatting::format_timestamp,
    navigation::{HistoryMode, directory_new_tab_target},
    view::{ExplorerView, ExplorerViewEvent},
};
use crate::settings::{
    ContextMenuConfiguredIcon, ContextMenuOnlyFilter, CustomContextMenuItem,
    resolve_context_menu_only_filter,
};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ContextMenuState {
    pub(super) origin: Point<Pixels>,
    pub(super) items: Vec<ContextMenuItem>,
    pub(super) hovered_path: Vec<usize>,
    pub(super) source: Option<ContextMenuSource>,
    pub(super) native_icon_entry: Option<FileEntry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ContextMenuSource {
    SidebarItem { row_id: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum ContextMenuItem {
    Action {
        id: String,
        icon: Option<ContextMenuIcon>,
        label: String,
        command: ContextMenuCommand,
        enabled: bool,
    },
    Submenu {
        id: String,
        icon: Option<ContextMenuIcon>,
        label: String,
        children: Vec<ContextMenuItem>,
    },
    Separator,
    Detail {
        label: &'static str,
        value: String,
        icon_slot: ContextMenuIconSlot,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ContextMenuIconSlot {
    Reserve,
    Collapse,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ContextMenuIcon {
    Cut,
    Copy,
    CopyAsPath,
    Paste,
    Delete,
    Rename,
    New,
    Properties,
    RunElevated,
    Extract,
    Mount,
    File,
    NativeFile,
    Folder,
    FolderKind(Option<DirectoryKind>),
    FolderKindForPath {
        path: PathBuf,
        kind: Option<DirectoryKind>,
    },
    ImagePath(PathBuf),
    ImagePathWithExecutableFallback(PathBuf),
    ImageUrl(String),
    ImageUrlWithExecutableFallback(String),
    NativePath(PathBuf),
    NativePathOptional(PathBuf),
    NewTab,
    OpenWith,
    Eject,
    Unpin,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ContextMenuCommand {
    OpenDirectory {
        path: PathBuf,
    },
    OpenDirectoryInNewTab {
        path: PathBuf,
    },
    OpenSelectedFiles,
    ChooseApplication {
        path: PathBuf,
    },
    #[cfg(target_os = "macos")]
    OpenWithApplication {
        target: PathBuf,
        application: PathBuf,
    },
    OpenSelectedDirectoriesInNewTabs,
    CutSelected,
    CopySelected,
    CopyPath {
        path: PathBuf,
    },
    CopyRepoRelativePath {
        relative_path: String,
    },
    Paste,
    ExtractSelectedArchives,
    MountSelectedImage,
    DeleteSelected,
    RenameSelected,
    PropertiesSelected,
    PropertiesForPath {
        path: PathBuf,
    },
    RunSelectedElevated {
        paths: Vec<PathBuf>,
    },
    NewFile,
    NewFolder,
    RunCustom {
        executable: PathBuf,
        args: Vec<String>,
        targets: Vec<PathBuf>,
    },
    EjectMountedVolume {
        path: PathBuf,
    },
    UnpinSidebar {
        configured_index: usize,
    },
}

impl ContextMenuState {
    pub(super) fn new(origin: Point<Pixels>, items: Vec<ContextMenuItem>) -> Self {
        Self {
            origin,
            items,
            hovered_path: Vec::new(),
            source: None,
            native_icon_entry: None,
        }
    }

    pub(super) fn new_with_native_icon_entry(
        origin: Point<Pixels>,
        items: Vec<ContextMenuItem>,
        native_icon_entry: Option<FileEntry>,
    ) -> Self {
        Self {
            origin,
            items,
            hovered_path: Vec::new(),
            source: None,
            native_icon_entry,
        }
    }

    pub(super) fn new_with_source(
        origin: Point<Pixels>,
        items: Vec<ContextMenuItem>,
        source: ContextMenuSource,
    ) -> Self {
        Self {
            origin,
            items,
            hovered_path: Vec::new(),
            source: Some(source),
            native_icon_entry: None,
        }
    }
}

impl ExplorerView {
    pub(super) fn open_folder_context_menu(
        &mut self,
        origin: Point<Pixels>,
        can_paste: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.commit_active_rename_before_interaction(window, cx) {
            return false;
        }

        self.clear_selection();
        self.finish_search_edit();
        self.cancel_address_bar_edit();
        self.cancel_pending_click_rename();
        self.open_utility_menu = None;
        let custom_items = cx
            .try_global::<crate::settings::SettingsState>()
            .map(|settings| settings.value.contextmenu.items.clone())
            .unwrap_or_default();
        self.context_menu = Some(ContextMenuState::new(
            origin,
            folder_context_menu_items_with_custom(
                &self.path,
                can_paste,
                &self.date_format,
                &custom_items,
            ),
        ));
        true
    }

    pub(super) fn open_sidebar_context_menu(
        &mut self,
        origin: Point<Pixels>,
        path: PathBuf,
        row_id: usize,
        configured_index: Option<usize>,
        open_icon_kind: Option<DirectoryKind>,
        can_eject: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.commit_active_rename_before_interaction(window, cx) {
            return false;
        }

        self.clear_selection();
        self.finish_search_edit();
        self.cancel_address_bar_edit();
        self.cancel_pending_click_rename();
        self.open_utility_menu = None;
        self.context_menu = Some(ContextMenuState::new_with_source(
            origin,
            sidebar_context_menu_items(path, configured_index, open_icon_kind, can_eject),
            ContextMenuSource::SidebarItem { row_id },
        ));
        true
    }

    pub(super) fn open_entry_context_menu(
        &mut self,
        origin: Point<Pixels>,
        entry: &FileEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.commit_active_rename_before_interaction(window, cx) {
            return false;
        }

        let Some(ix) = self.entry_index_by_path(&entry.path) else {
            return false;
        };
        if !self.entry_is_selected(ix) {
            self.select_single_index(ix);
        }

        self.finish_search_edit();
        self.cancel_address_bar_edit();
        self.cancel_pending_click_rename();
        self.open_utility_menu = None;
        let selected_context = self.selected_entry_context();
        let custom_items = cx
            .try_global::<crate::settings::SettingsState>()
            .map(|settings| settings.value.contextmenu.items.clone())
            .unwrap_or_default();
        let targets = self.selected_paths();
        let selected_entries = self.selected_entries();
        self.context_menu = Some(ContextMenuState::new_with_native_icon_entry(
            origin,
            entry_context_menu_items_with_custom(
                selected_context.single_directory_open_target,
                selected_context.selected_count,
                selected_context.file_open_count,
                selected_context.directory_new_tab_count,
                self.can_start_selected_rename(),
                selected_context.native_icon_entry.is_some(),
                &custom_items,
                &targets,
                &selected_entries,
            ),
            selected_context.native_icon_entry,
        ));
        true
    }

    pub(super) fn open_selected_entries_context_menu(
        &mut self,
        origin: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.commit_active_rename_before_interaction(window, cx) {
            return false;
        }

        if self.focused_entry().is_none() {
            return false;
        }

        self.finish_search_edit();
        self.cancel_address_bar_edit();
        self.cancel_pending_click_rename();
        self.open_utility_menu = None;
        let selected_context = self.selected_entry_context();
        let custom_items = cx
            .try_global::<crate::settings::SettingsState>()
            .map(|settings| settings.value.contextmenu.items.clone())
            .unwrap_or_default();
        let targets = self.selected_paths();
        let selected_entries = self.selected_entries();
        self.context_menu = Some(ContextMenuState::new_with_native_icon_entry(
            origin,
            entry_context_menu_items_with_custom(
                selected_context.single_directory_open_target,
                selected_context.selected_count,
                selected_context.file_open_count,
                selected_context.directory_new_tab_count,
                self.can_start_selected_rename(),
                selected_context.native_icon_entry.is_some(),
                &custom_items,
                &targets,
                &selected_entries,
            ),
            selected_context.native_icon_entry,
        ));
        true
    }

    pub(super) fn close_context_menu(&mut self) -> bool {
        self.context_menu.take().is_some()
    }

    pub(super) fn set_context_menu_hovered_path(&mut self, path: Vec<usize>) {
        if let Some(menu) = self.context_menu.as_mut() {
            menu.hovered_path = path;
        }
    }

    pub(super) fn execute_context_menu_command(
        &mut self,
        command: ContextMenuCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_context_menu();
        self.open_utility_menu = None;

        if !self.commit_active_rename_before_interaction(window, cx) {
            return;
        }

        match command {
            ContextMenuCommand::OpenDirectory { path } => {
                self.navigate_to_directory_with_watcher(path, HistoryMode::Record, cx);
            }
            ContextMenuCommand::OpenDirectoryInNewTab { path } => {
                cx.emit(crate::explorer::view::ExplorerViewEvent::OpenDirectoryInNewTab(path));
            }
            ContextMenuCommand::OpenSelectedFiles => {
                self.open_selected_files_with_default_app(window, cx);
            }
            ContextMenuCommand::ChooseApplication { path } => {
                self.choose_application_for_file(path, window, cx);
            }
            #[cfg(target_os = "macos")]
            ContextMenuCommand::OpenWithApplication {
                target,
                application,
            } => {
                self.open_file_with_application(target, application, window, cx);
            }
            ContextMenuCommand::OpenSelectedDirectoriesInNewTabs => {
                for path in self.selected_directory_new_tab_targets() {
                    cx.emit(crate::explorer::view::ExplorerViewEvent::OpenDirectoryInNewTab(path));
                }
            }
            ContextMenuCommand::CutSelected => self.cut_selected_to_clipboard(cx),
            ContextMenuCommand::CopySelected => self.copy_selected_to_clipboard(cx),
            ContextMenuCommand::CopyPath { path } => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.address_text_for_path(&path)));
                self.cut_paths.clear();
                self.clear_operation_notice();
            }
            ContextMenuCommand::CopyRepoRelativePath { relative_path } => {
                cx.write_to_clipboard(ClipboardItem::new_string(relative_path));
                self.cut_paths.clear();
                self.clear_operation_notice();
            }
            ContextMenuCommand::Paste => self.paste_clipboard(window, cx),
            ContextMenuCommand::ExtractSelectedArchives => self.extract_selected_archives(cx),
            ContextMenuCommand::MountSelectedImage => self.mount_selected_image(cx),
            ContextMenuCommand::DeleteSelected => self.trash_selected_paths(cx),
            ContextMenuCommand::RenameSelected => {
                self.start_rename_selected(window, cx);
            }
            ContextMenuCommand::PropertiesSelected => {
                self.open_selected_properties(window, cx);
            }
            ContextMenuCommand::PropertiesForPath { path } => {
                self.open_properties_for_paths(vec![path], window, cx);
            }
            ContextMenuCommand::RunSelectedElevated { paths } => {
                self.run_selected_paths_elevated(paths, window, cx);
            }
            ContextMenuCommand::NewFile => self.create_new_file(window, cx),
            ContextMenuCommand::NewFolder => self.create_new_folder(window, cx),
            ContextMenuCommand::RunCustom {
                executable,
                args,
                targets,
            } => {
                self.handle_custom_command_result(
                    &executable,
                    run_custom_command(&executable, &args, &targets),
                );
            }
            ContextMenuCommand::EjectMountedVolume { path } => {
                self.eject_mounted_volume(path, cx);
            }
            ContextMenuCommand::UnpinSidebar { configured_index } => {
                crate::settings::unpin_sidebar_item(configured_index, cx);
            }
        }
    }

    fn run_selected_paths_elevated(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() || self.run_elevated_task.is_some() {
            return;
        }

        #[cfg(target_os = "windows")]
        {
            let parent = crate::explorer::windows_shell::parent_hwnd(window);
            self.clear_operation_notice();
            let task = cx.spawn(async move |this, cx| {
                let results = run_elevated_paths_until_not_launched(paths, |path| {
                    windows_run_elevated_path(path, parent)
                });

                let _ = this.update(cx, |explorer, cx| {
                    explorer.run_elevated_task = None;
                    explorer.handle_run_elevated_results(results);
                    cx.notify();
                });
            });
            self.run_elevated_task = Some(task);
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = window;
            let _ = cx;
        }
    }

    fn handle_run_elevated_results(&mut self, results: Vec<(PathBuf, io::Result<bool>)>) {
        for (path, result) in results {
            match result {
                Ok(true) => self.clear_operation_notice(),
                Ok(false) => break,
                Err(error) => {
                    self.set_error_notice(format_open_error(&path, &error));
                    break;
                }
            }
        }
    }

    fn handle_custom_command_result(&mut self, executable: &Path, result: std::io::Result<()>) {
        match result {
            Ok(()) => self.clear_operation_notice(),
            Err(error) => {
                self.set_error_notice(format!(
                    "Could not run {}: {error}",
                    executable
                        .file_name()
                        .unwrap_or(executable.as_os_str())
                        .to_string_lossy()
                ));
            }
        }
    }

    fn eject_mounted_volume(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.volume_eject_task.is_some() {
            return;
        }

        self.clear_operation_notice();
        let task_path = path.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { eject_mounted_volume_path(&task_path) })
                .await;

            let _ = this.update(cx, |explorer, cx| {
                explorer.volume_eject_task = None;
                explorer.handle_mounted_volume_eject_result(&path, result, cx);
                cx.notify();
            });
        });
        self.volume_eject_task = Some(task);
    }

    fn handle_mounted_volume_eject_result(
        &mut self,
        path: &Path,
        result: io::Result<()>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(()) => {
                cx.emit(ExplorerViewEvent::MountedVolumeEjected(path.to_path_buf()));
            }
            Err(error) => {
                self.set_error_notice(format!(
                    "Could not eject {}: {error}",
                    mounted_volume_error_name(path)
                ));
            }
        }
    }

    pub(super) fn mount_selected_image(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected_mountable_image_path() else {
            return;
        };
        if self.image_mount_task.is_some() {
            return;
        }

        self.clear_operation_notice();
        let task_path = path.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { mount_image_path(&task_path) })
                .await;

            let _ = this.update(cx, |explorer, cx| {
                explorer.image_mount_task = None;
                explorer.handle_image_mount_result(&path, result, cx);
                cx.notify();
            });
        });
        self.image_mount_task = Some(task);
    }

    fn handle_image_mount_result(
        &mut self,
        path: &Path,
        result: io::Result<()>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(()) => {
                self.clear_operation_notice();
                self.refresh_with_entry_metadata_resolution(cx);
            }
            Err(error) => {
                self.set_error_notice(format!(
                    "Could not mount {}: {error}",
                    mounted_volume_error_name(path)
                ));
            }
        }
    }

    fn selected_entry_context(&self) -> SelectedEntryContext {
        let selected_count = self.selection.selected_indices.len();
        let file_open_targets = self.selected_file_open_targets();
        let directory_new_tab_targets = self.selected_directory_new_tab_targets();
        let single_directory_open_target = if selected_count == 1 && file_open_targets.is_empty() {
            directory_new_tab_targets.first().cloned()
        } else {
            None
        };
        let native_icon_entry = if selected_count == 1 {
            self.selection
                .selected_indices
                .iter()
                .next()
                .and_then(|ix| self.entries.get(*ix))
                .filter(|entry| entry_is_file_open_target(entry))
                .cloned()
        } else {
            None
        };

        SelectedEntryContext {
            selected_count,
            file_open_count: file_open_targets.len(),
            directory_new_tab_count: directory_new_tab_targets.len(),
            single_directory_open_target,
            native_icon_entry,
        }
    }

    fn selected_file_open_targets(&self) -> Vec<PathBuf> {
        self.selection
            .selected_indices
            .iter()
            .filter_map(|ix| self.entries.get(*ix))
            .filter(|entry| entry_is_file_open_target(entry))
            .map(|entry| entry.path.clone())
            .collect()
    }

    fn selected_directory_new_tab_targets(&self) -> Vec<PathBuf> {
        self.selection
            .selected_indices
            .iter()
            .filter_map(|ix| self.entries.get(*ix))
            .filter_map(directory_new_tab_target)
            .collect()
    }

    fn selected_entries(&self) -> Vec<FileEntry> {
        self.selection
            .selected_indices
            .iter()
            .filter_map(|ix| self.entries.get(*ix).cloned())
            .collect()
    }

    fn open_selected_files_with_default_app(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_files_with_default_app(self.selected_file_open_targets(), window, cx);
    }

    #[cfg(test)]
    fn open_selected_files_with(&mut self, mut open: impl FnMut(&Path) -> std::io::Result<()>) {
        for path in self.selected_file_open_targets() {
            let result = open(&path);
            self.handle_open_file_result(&path, result);
        }
    }
}

fn run_custom_command(
    executable: &Path,
    args: &[String],
    targets: &[PathBuf],
) -> std::io::Result<()> {
    run_custom_command_with(executable, args, targets, |executable, arguments| {
        Command::new(executable).args(arguments).spawn().map(|_| ())
    })
}

#[cfg(any(target_os = "windows", test))]
fn run_elevated_paths_until_not_launched(
    paths: Vec<PathBuf>,
    mut run_path: impl FnMut(&Path) -> io::Result<bool>,
) -> Vec<(PathBuf, io::Result<bool>)> {
    let mut results = Vec::new();
    for path in paths {
        let result = run_path(&path);
        let launched = result.as_ref().is_ok_and(|launched| *launched);
        results.push((path, result));
        if !launched {
            break;
        }
    }
    results
}

#[cfg(target_os = "windows")]
fn windows_run_elevated_path(
    path: &Path,
    parent: Option<windows::Win32::Foundation::HWND>,
) -> io::Result<bool> {
    use std::ffi::OsStr;

    let mut request = crate::explorer::windows_shell::shell_execute_file_request(
        path,
        OsStr::new("runas"),
        None,
        false,
        parent,
    );
    crate::explorer::windows_shell::execute_shell_request(&mut request)
}

fn run_custom_command_with(
    executable: &Path,
    args: &[String],
    targets: &[PathBuf],
    spawn: impl FnOnce(&Path, &[OsString]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let arguments = custom_command_arguments(args, targets);
    spawn(executable, &arguments)
}

fn custom_command_arguments(args: &[String], targets: &[PathBuf]) -> Vec<OsString> {
    let mut arguments = Vec::new();
    let mut expanded_placeholder = false;

    for arg in args {
        match arg.as_str() {
            "{path}" => {
                expanded_placeholder = true;
                if let Some(target) = targets.first() {
                    arguments.push(target.as_os_str().to_os_string());
                }
            }
            "{paths}" => {
                expanded_placeholder = true;
                arguments.extend(
                    targets
                        .iter()
                        .map(|target| target.as_os_str().to_os_string()),
                );
            }
            _ => arguments.push(OsString::from(arg)),
        }
    }

    if !expanded_placeholder {
        arguments.extend(
            targets
                .iter()
                .map(|target| target.as_os_str().to_os_string()),
        );
    }

    arguments
}

struct SelectedEntryContext {
    selected_count: usize,
    file_open_count: usize,
    directory_new_tab_count: usize,
    single_directory_open_target: Option<PathBuf>,
    native_icon_entry: Option<FileEntry>,
}

pub(super) fn sidebar_context_menu_items(
    path: PathBuf,
    configured_index: Option<usize>,
    open_icon_kind: Option<DirectoryKind>,
    can_eject: bool,
) -> Vec<ContextMenuItem> {
    let mut items = vec![
        ContextMenuItem::Action {
            id: "context-menu-sidebar-open".to_owned(),
            icon: Some(ContextMenuIcon::FolderKindForPath {
                path: path.clone(),
                kind: open_icon_kind,
            }),
            label: "Open".to_owned(),
            command: ContextMenuCommand::OpenDirectory { path: path.clone() },
            enabled: true,
        },
        ContextMenuItem::Action {
            id: "context-menu-sidebar-open-new-tab".to_owned(),
            icon: Some(ContextMenuIcon::NewTab),
            label: "Open in new tab".to_owned(),
            command: ContextMenuCommand::OpenDirectoryInNewTab { path: path.clone() },
            enabled: true,
        },
    ];
    if can_eject {
        items.push(ContextMenuItem::Separator);
        items.push(ContextMenuItem::Action {
            id: "context-menu-sidebar-eject".to_owned(),
            icon: Some(ContextMenuIcon::Eject),
            label: "Eject".to_owned(),
            command: ContextMenuCommand::EjectMountedVolume { path: path.clone() },
            enabled: true,
        });
    }
    if let Some(configured_index) = configured_index {
        items.push(ContextMenuItem::Separator);
        items.push(ContextMenuItem::Action {
            id: "context-menu-sidebar-unpin".to_owned(),
            icon: Some(ContextMenuIcon::Unpin),
            label: "Unpin".to_owned(),
            command: ContextMenuCommand::UnpinSidebar { configured_index },
            enabled: true,
        });
    }
    items
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PlatformCommand {
    executable: OsString,
    args: Vec<OsString>,
}

fn eject_mounted_volume_path(path: &Path) -> io::Result<()> {
    let Some(command) = mounted_volume_eject_command(path) else {
        return Err(io::Error::other("eject is not supported on this platform"));
    };
    run_platform_command(&command)
}

fn mount_image_path(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        return windows_mount_image_path(path);
    }

    #[cfg(target_os = "macos")]
    {
        return run_platform_command(&macos_mount_image_command(path));
    }

    #[cfg(target_os = "linux")]
    {
        return linux_mount_image_path(path);
    }

    #[allow(unreachable_code)]
    Err(io::Error::other("mount is not supported on this platform"))
}

fn run_platform_command(command: &PlatformCommand) -> io::Result<()> {
    let output = Command::new(&command.executable)
        .args(&command.args)
        .output()?;
    if output.status.success() {
        return Ok(());
    }

    Err(io::Error::other(platform_command_error_message(&output)))
}

fn platform_command_error_message(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return stderr.to_owned();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout = stdout.trim();
    if !stdout.is_empty() {
        return stdout.to_owned();
    }

    output.status.to_string()
}

fn mounted_volume_error_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

fn mounted_volume_eject_command(path: &Path) -> Option<PlatformCommand> {
    #[cfg(target_os = "windows")]
    {
        return Some(windows_mounted_volume_eject_command(path));
    }

    #[cfg(target_os = "macos")]
    {
        return Some(macos_mounted_volume_eject_command(path));
    }

    #[cfg(target_os = "linux")]
    {
        return Some(linux_mounted_volume_eject_command(path));
    }

    #[allow(unreachable_code)]
    None
}

#[cfg(target_os = "windows")]
fn windows_mount_image_path(path: &Path) -> io::Result<()> {
    if windows_mount_image_uses_shell(path) {
        run_platform_command(&windows_shell_mount_image_command(path))
    } else {
        run_platform_command(&windows_mount_disk_image_command(path))
    }
}

#[cfg(any(target_os = "windows", test))]
fn windows_mount_image_uses_shell(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("img"))
}

#[cfg(any(target_os = "windows", test))]
fn windows_mount_disk_image_command(path: &Path) -> PlatformCommand {
    let path_literal = powershell_single_quoted_literal(&path.display().to_string());
    PlatformCommand {
        executable: OsString::from("powershell.exe"),
        args: vec![
            OsString::from("-NoProfile"),
            OsString::from("-NonInteractive"),
            OsString::from("-ExecutionPolicy"),
            OsString::from("Bypass"),
            OsString::from("-Command"),
            OsString::from(format!(
                "Mount-DiskImage -ImagePath {path_literal} -ErrorAction Stop",
            )),
        ],
    }
}

#[cfg(any(target_os = "windows", test))]
fn windows_shell_mount_image_command(path: &Path) -> PlatformCommand {
    let parent_literal = powershell_single_quoted_literal(
        &path
            .parent()
            .map(|parent| parent.display().to_string())
            .unwrap_or_else(|| ".".to_owned()),
    );
    let file_name_literal = powershell_single_quoted_literal(
        &path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
    );
    PlatformCommand {
        executable: OsString::from("powershell.exe"),
        args: vec![
            OsString::from("-NoProfile"),
            OsString::from("-NonInteractive"),
            OsString::from("-ExecutionPolicy"),
            OsString::from("Bypass"),
            OsString::from("-Command"),
            OsString::from(format!(
                "$folder = (New-Object -ComObject Shell.Application).Namespace({parent_literal}); \
                 if ($null -eq $folder) {{ throw \"Folder not found\" }}; \
                 $item = $folder.ParseName({file_name_literal}); \
                 if ($null -eq $item) {{ throw \"Image not found\" }}; \
                 $item.InvokeVerb('Mount')",
            )),
        ],
    }
}

#[cfg(any(target_os = "windows", test))]
fn windows_mounted_volume_eject_command(path: &Path) -> PlatformCommand {
    let path_literal = powershell_single_quoted_literal(&path.display().to_string());
    PlatformCommand {
        executable: OsString::from("powershell.exe"),
        args: vec![
            OsString::from("-NoProfile"),
            OsString::from("-NonInteractive"),
            OsString::from("-ExecutionPolicy"),
            OsString::from("Bypass"),
            OsString::from("-Command"),
            OsString::from(format!(
                "$drive = [System.IO.Path]::GetPathRoot({path_literal}).TrimEnd('\\'); \
                 $item = (New-Object -ComObject Shell.Application).Namespace(17).ParseName($drive); \
                 if ($null -eq $item) {{ throw \"Drive not found: $drive\" }}; \
                 $item.InvokeVerb('Eject')",
            )),
        ],
    }
}

#[cfg(any(target_os = "windows", test))]
fn powershell_single_quoted_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(any(target_os = "macos", test))]
fn macos_mounted_volume_eject_command(path: &Path) -> PlatformCommand {
    PlatformCommand {
        executable: OsString::from("/usr/sbin/diskutil"),
        args: vec![OsString::from("eject"), path.as_os_str().to_os_string()],
    }
}

#[cfg(any(target_os = "macos", test))]
fn macos_mount_image_command(path: &Path) -> PlatformCommand {
    PlatformCommand {
        executable: OsString::from("/usr/bin/hdiutil"),
        args: vec![OsString::from("attach"), path.as_os_str().to_os_string()],
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_mounted_volume_eject_command(path: &Path) -> PlatformCommand {
    PlatformCommand {
        executable: OsString::from("gio"),
        args: vec![
            OsString::from("mount"),
            OsString::from("--eject"),
            path.as_os_str().to_os_string(),
        ],
    }
}

#[cfg(target_os = "linux")]
fn linux_mount_image_path(path: &Path) -> io::Result<()> {
    let loop_output = run_platform_command_output(&linux_loop_setup_command(path))?;
    let loop_device = linux_loop_device_from_loop_setup_output(&loop_output).ok_or_else(|| {
        io::Error::other("udisksctl did not report a loop device for the mounted image")
    })?;

    match run_platform_command(&linux_mount_loop_device_command(&loop_device)) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = run_platform_command(&linux_loop_delete_command(&loop_device));
            Err(error)
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_loop_setup_command(path: &Path) -> PlatformCommand {
    PlatformCommand {
        executable: OsString::from("udisksctl"),
        args: vec![
            OsString::from("loop-setup"),
            OsString::from("--read-only"),
            OsString::from("--file"),
            path.as_os_str().to_os_string(),
        ],
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_mount_loop_device_command(loop_device: &Path) -> PlatformCommand {
    PlatformCommand {
        executable: OsString::from("udisksctl"),
        args: vec![
            OsString::from("mount"),
            OsString::from("--block-device"),
            loop_device.as_os_str().to_os_string(),
        ],
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_loop_delete_command(loop_device: &Path) -> PlatformCommand {
    PlatformCommand {
        executable: OsString::from("udisksctl"),
        args: vec![
            OsString::from("loop-delete"),
            OsString::from("--block-device"),
            loop_device.as_os_str().to_os_string(),
        ],
    }
}

#[cfg(target_os = "linux")]
fn run_platform_command_output(command: &PlatformCommand) -> io::Result<std::process::Output> {
    let output = Command::new(&command.executable)
        .args(&command.args)
        .output()?;
    if output.status.success() {
        return Ok(output);
    }

    Err(io::Error::other(platform_command_error_message(&output)))
}

#[cfg(target_os = "linux")]
fn linux_loop_device_from_loop_setup_output(output: &std::process::Output) -> Option<PathBuf> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    linux_loop_device_from_loop_setup_stdout(&stdout)
}

#[cfg(any(target_os = "linux", test))]
fn linux_loop_device_from_loop_setup_stdout(stdout: &str) -> Option<PathBuf> {
    stdout
        .split_whitespace()
        .map(|word| {
            word.trim_matches(|character: char| {
                character == '.' || character == ',' || character == ';'
            })
        })
        .find(|word| word.starts_with("/dev/loop"))
        .map(PathBuf::from)
}

#[cfg(test)]
pub(super) fn entry_context_menu_items(
    single_directory_open_target: Option<PathBuf>,
    selected_count: usize,
    selected_file_count: usize,
    selected_directory_count: usize,
    can_rename: bool,
    use_native_file_icon: bool,
) -> Vec<ContextMenuItem> {
    let selected_entries = test_context_menu_entries(
        &single_directory_open_target,
        selected_count,
        selected_file_count,
        selected_directory_count,
    );
    let targets = selected_entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    entry_context_menu_items_with_custom(
        single_directory_open_target,
        selected_count,
        selected_file_count,
        selected_directory_count,
        can_rename,
        use_native_file_icon,
        &[],
        &targets,
        &selected_entries,
    )
}

#[cfg(test)]
fn test_context_menu_entries(
    single_directory_open_target: &Option<PathBuf>,
    selected_count: usize,
    selected_file_count: usize,
    selected_directory_count: usize,
) -> Vec<FileEntry> {
    let mut entries = Vec::new();

    for index in 0..selected_file_count.min(selected_count) {
        entries.push(test_context_menu_entry(
            PathBuf::from(format!("file-{}.txt", index + 1)),
            false,
        ));
    }

    for index in 0..selected_directory_count.min(selected_count.saturating_sub(entries.len())) {
        let path = if index == 0 {
            single_directory_open_target
                .clone()
                .unwrap_or_else(|| PathBuf::from("folder-1"))
        } else {
            PathBuf::from(format!("folder-{}", index + 1))
        };
        entries.push(test_context_menu_entry(path, true));
    }

    entries
}

#[cfg(test)]
fn test_context_menu_entry(path: PathBuf, is_dir: bool) -> FileEntry {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(if is_dir { "folder" } else { "file.txt" })
        .to_owned();
    FileEntry {
        path,
        name,
        kind: if is_dir {
            crate::explorer::entry::EntryKind::Directory
        } else {
            crate::explorer::entry::EntryKind::File
        },
        modified: None,
        size: (!is_dir).then_some(1),
    }
}

fn entry_context_menu_items_with_custom(
    single_directory_open_target: Option<PathBuf>,
    selected_count: usize,
    selected_file_count: usize,
    selected_directory_count: usize,
    can_rename: bool,
    use_native_file_icon: bool,
    custom_items: &[CustomContextMenuItem],
    targets: &[PathBuf],
    selected_entries: &[FileEntry],
) -> Vec<ContextMenuItem> {
    let mut items = Vec::new();

    if selected_count == 1 {
        let command = match single_directory_open_target {
            Some(path) => ContextMenuCommand::OpenDirectory { path },
            None => ContextMenuCommand::OpenSelectedFiles,
        };
        let icon = if selected_file_count > 0 {
            if use_native_file_icon {
                ContextMenuIcon::NativeFile
            } else {
                ContextMenuIcon::File
            }
        } else {
            ContextMenuIcon::FolderKind(None)
        };
        items.push(ContextMenuItem::Action {
            id: "context-menu-entry-open".to_owned(),
            icon: Some(icon),
            label: "Open".to_owned(),
            command,
            enabled: true,
        });

        if selected_entries_are_supported_mountable_images(selected_entries) {
            items.push(ContextMenuItem::Action {
                id: "context-menu-entry-mount".to_owned(),
                icon: Some(ContextMenuIcon::Mount),
                label: "Mount".to_owned(),
                command: ContextMenuCommand::MountSelectedImage,
                enabled: true,
            });
        }

        #[cfg(target_os = "windows")]
        if selected_entries_are_run_elevated_targets(selected_entries) {
            items.push(run_elevated_context_menu_item(targets));
        }

        if let Some(entry) = selected_entries
            .first()
            .filter(|entry| entry_is_open_with_context_menu_target(entry))
            .filter(|entry| {
                let _ = entry;
                true
            })
        {
            items.push(crate::explorer::open_with::context_menu_item(&entry.path));
        }
    }

    if selected_count > 1 && selected_file_count > 0 {
        items.push(ContextMenuItem::Action {
            id: "context-menu-entry-open".to_owned(),
            icon: Some(ContextMenuIcon::File),
            label: format!("Open files ({selected_file_count})"),
            command: ContextMenuCommand::OpenSelectedFiles,
            enabled: true,
        });
        #[cfg(target_os = "windows")]
        if selected_entries_are_run_elevated_targets(selected_entries) {
            items.push(run_elevated_context_menu_item(targets));
        }
    }

    if selected_directory_count > 0 {
        items.push(ContextMenuItem::Action {
            id: "context-menu-entry-open-new-tab".to_owned(),
            icon: Some(ContextMenuIcon::NewTab),
            label: if selected_directory_count > 1 {
                format!("Open new tabs ({selected_directory_count})")
            } else {
                "Open in new tab".to_owned()
            },
            command: ContextMenuCommand::OpenSelectedDirectoriesInNewTabs,
            enabled: true,
        });
    }

    if selected_entries_are_supported_archives(selected_entries) {
        items.push(ContextMenuItem::Action {
            id: "context-menu-entry-extract".to_owned(),
            icon: Some(ContextMenuIcon::Extract),
            label: "Extract".to_owned(),
            command: ContextMenuCommand::ExtractSelectedArchives,
            enabled: true,
        });
    }

    if !items.is_empty() {
        items.push(ContextMenuItem::Separator);
    }
    insert_custom_items_after_first_separator(
        &mut items,
        custom_items,
        targets,
        CustomContextMenuTarget::Entries(selected_entries),
    );

    items.extend([
        ContextMenuItem::Action {
            id: "context-menu-entry-cut".to_owned(),
            icon: Some(ContextMenuIcon::Cut),
            label: "Cut".to_owned(),
            command: ContextMenuCommand::CutSelected,
            enabled: true,
        },
        ContextMenuItem::Action {
            id: "context-menu-entry-copy".to_owned(),
            icon: Some(ContextMenuIcon::Copy),
            label: "Copy".to_owned(),
            command: ContextMenuCommand::CopySelected,
            enabled: true,
        },
    ]);
    if selected_count == 1
        && let Some(entry) = selected_entries.first()
    {
        let is_folder = directory_new_tab_target(entry).is_some();
        items.push(copy_path_context_menu_item(
            "context-menu-entry-copy-path",
            &entry.path,
            is_folder,
        ));
        if let Some(item) = copy_repo_relative_path_context_menu_item(
            "context-menu-entry-copy-relative-repo-path",
            &entry.path,
            is_folder,
        ) {
            items.push(item);
        }
    }
    items.extend([
        ContextMenuItem::Separator,
        ContextMenuItem::Action {
            id: "context-menu-entry-delete".to_owned(),
            icon: Some(ContextMenuIcon::Delete),
            label: "Delete".to_owned(),
            command: ContextMenuCommand::DeleteSelected,
            enabled: true,
        },
    ]);
    if selected_count == 1 {
        items.push(ContextMenuItem::Action {
            id: "context-menu-entry-rename".to_owned(),
            icon: Some(ContextMenuIcon::Rename),
            label: "Rename".to_owned(),
            command: ContextMenuCommand::RenameSelected,
            enabled: can_rename,
        });
    }
    items.extend([
        ContextMenuItem::Separator,
        ContextMenuItem::Action {
            id: "context-menu-entry-properties".to_owned(),
            icon: Some(ContextMenuIcon::Properties),
            label: "Properties".to_owned(),
            command: ContextMenuCommand::PropertiesSelected,
            enabled: selected_count > 0,
        },
    ]);
    items
}

fn entry_is_file_open_target(entry: &FileEntry) -> bool {
    !entry.is_directory_like() || entry.is_app_bundle()
}

fn entry_is_open_with_context_menu_target(entry: &FileEntry) -> bool {
    entry.is_open_with_target() && !path_is_windows_open_with_context_menu_excluded(&entry.path)
}

fn path_is_windows_open_with_context_menu_excluded(path: &Path) -> bool {
    cfg!(target_os = "windows")
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

fn selected_entries_are_supported_archives(selected_entries: &[FileEntry]) -> bool {
    !selected_entries.is_empty()
        && selected_entries
            .iter()
            .all(|entry| entry.is_open_with_target() && archive_path_is_supported(&entry.path))
}

fn selected_entries_are_supported_mountable_images(selected_entries: &[FileEntry]) -> bool {
    let [entry] = selected_entries else {
        return false;
    };
    entry.is_open_with_target() && mountable_image_path_is_supported(&entry.path)
}

fn selected_entries_are_run_elevated_targets(selected_entries: &[FileEntry]) -> bool {
    !selected_entries.is_empty() && selected_entries.iter().all(entry_is_run_elevated_target)
}

fn entry_is_run_elevated_target(entry: &FileEntry) -> bool {
    entry.is_open_with_target() && path_is_run_elevated_target(&entry.path)
}

fn path_is_run_elevated_target(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("exe") || extension.eq_ignore_ascii_case("bat")
        })
}

fn run_elevated_context_menu_item(targets: &[PathBuf]) -> ContextMenuItem {
    ContextMenuItem::Action {
        id: "context-menu-entry-run-elevated".to_owned(),
        icon: Some(ContextMenuIcon::RunElevated),
        label: "Run as administrator".to_owned(),
        command: ContextMenuCommand::RunSelectedElevated {
            paths: targets.to_vec(),
        },
        enabled: true,
    }
}

fn folder_context_menu_items_with_custom(
    path: &Path,
    can_paste: bool,
    date_format: &str,
    custom_items: &[CustomContextMenuItem],
) -> Vec<ContextMenuItem> {
    let path_is_transfer_path = false;

    let (created, modified) = if path_is_transfer_path {
        (None, None)
    } else {
        fs::metadata(path)
            .map(|metadata| (metadata.created().ok(), metadata.modified().ok()))
            .unwrap_or((None, None))
    };

    let mut items = folder_context_menu_items_from_times_with_format(
        path,
        can_paste,
        created,
        modified,
        date_format,
    );
    if !path_is_transfer_path {
        insert_custom_items_after_first_separator(
            &mut items,
            custom_items,
            &[path.to_path_buf()],
            CustomContextMenuTarget::Directory,
        );
    }
    items
}

#[cfg(test)]
pub(super) fn folder_context_menu_items_from_times(
    can_paste: bool,
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
) -> Vec<ContextMenuItem> {
    folder_context_menu_items_from_times_with_format(
        Path::new("folder"),
        can_paste,
        created,
        modified,
        crate::settings::DEFAULT_DATE_FORMAT,
    )
}

fn folder_context_menu_items_from_times_with_format(
    path: &Path,
    can_paste: bool,
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
    date_format: &str,
) -> Vec<ContextMenuItem> {
    let path_is_transfer_path = false;

    let mut items = vec![
        ContextMenuItem::Action {
            id: "context-menu-paste".to_owned(),
            icon: Some(ContextMenuIcon::Paste),
            label: "Paste".to_owned(),
            command: ContextMenuCommand::Paste,
            enabled: can_paste,
        },
        ContextMenuItem::Submenu {
            id: "context-menu-new".to_owned(),
            icon: Some(ContextMenuIcon::New),
            label: "New".to_owned(),
            children: vec![
                ContextMenuItem::Action {
                    id: "context-menu-new-file".to_owned(),
                    icon: Some(ContextMenuIcon::File),
                    label: "File".to_owned(),
                    command: ContextMenuCommand::NewFile,
                    enabled: true,
                },
                ContextMenuItem::Action {
                    id: "context-menu-new-folder".to_owned(),
                    icon: Some(ContextMenuIcon::Folder),
                    label: "Folder".to_owned(),
                    command: ContextMenuCommand::NewFolder,
                    enabled: true,
                },
            ],
        },
        copy_path_context_menu_item("context-menu-folder-copy-path", path, true),
    ];
    if !path_is_transfer_path
        && let Some(relative_path) = repo_relative_path_text(path, true).filter(|path| path != ".")
    {
        items.push(
            copy_repo_relative_path_context_menu_item_with_relative_path(
                "context-menu-folder-copy-relative-repo-path",
                true,
                relative_path,
            ),
        );
    }
    items.extend([
        ContextMenuItem::Separator,
        ContextMenuItem::Detail {
            label: "Created",
            value: format_timestamp(created, date_format),
            icon_slot: ContextMenuIconSlot::Collapse,
        },
        ContextMenuItem::Detail {
            label: "Modified",
            value: format_timestamp(modified, date_format),
            icon_slot: ContextMenuIconSlot::Collapse,
        },
        ContextMenuItem::Separator,
        ContextMenuItem::Action {
            id: "context-menu-folder-properties".to_owned(),
            icon: Some(ContextMenuIcon::Properties),
            label: "Properties".to_owned(),
            command: ContextMenuCommand::PropertiesForPath {
                path: path.to_path_buf(),
            },
            enabled: true,
        },
    ]);
    items
}

fn copy_path_context_menu_item(id: &str, path: &Path, is_folder: bool) -> ContextMenuItem {
    ContextMenuItem::Action {
        id: id.to_owned(),
        icon: Some(ContextMenuIcon::CopyAsPath),
        label: if is_folder {
            "Copy folder path".to_owned()
        } else {
            "Copy file path".to_owned()
        },
        command: ContextMenuCommand::CopyPath {
            path: path.to_path_buf(),
        },
        enabled: true,
    }
}

fn copy_repo_relative_path_context_menu_item(
    id: &str,
    path: &Path,
    is_folder: bool,
) -> Option<ContextMenuItem> {
    let relative_path = repo_relative_path_text(path, is_folder)?;
    Some(copy_repo_relative_path_context_menu_item_with_relative_path(id, is_folder, relative_path))
}

fn copy_repo_relative_path_context_menu_item_with_relative_path(
    id: &str,
    is_folder: bool,
    relative_path: String,
) -> ContextMenuItem {
    ContextMenuItem::Action {
        id: id.to_owned(),
        icon: Some(ContextMenuIcon::CopyAsPath),
        label: if is_folder {
            "Copy relative folder path".to_owned()
        } else {
            "Copy relative file path".to_owned()
        },
        command: ContextMenuCommand::CopyRepoRelativePath { relative_path },
        enabled: true,
    }
}

fn repo_relative_path_text(path: &Path, is_folder: bool) -> Option<String> {
    let discover_start = if is_folder { path } else { path.parent()? };
    let repo = Repository::discover(discover_start).ok()?;
    let repo_root = repo.workdir()?;
    let relative = path.strip_prefix(repo_root).ok()?;
    Some(format_repo_relative_path(relative))
}

fn format_repo_relative_path(path: &Path) -> String {
    let relative = path
        .iter()
        .map(|component| component.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if relative.is_empty() {
        ".".to_owned()
    } else {
        relative
    }
}

fn insert_custom_items_after_first_separator(
    items: &mut Vec<ContextMenuItem>,
    configured: &[CustomContextMenuItem],
    targets: &[PathBuf],
    target: CustomContextMenuTarget<'_>,
) {
    let custom = configured
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            configured_context_menu_item(item, targets, target, &index.to_string())
        })
        .collect::<Vec<_>>();
    if custom.is_empty() {
        return;
    }

    let insertion = items
        .iter()
        .position(|item| matches!(item, ContextMenuItem::Separator))
        .map_or(items.len(), |index| index + 1);
    items.splice(
        insertion..insertion,
        custom
            .into_iter()
            .chain(std::iter::once(ContextMenuItem::Separator)),
    );
}

fn configured_context_menu_item(
    item: &CustomContextMenuItem,
    targets: &[PathBuf],
    target: CustomContextMenuTarget<'_>,
    id_suffix: &str,
) -> Option<ContextMenuItem> {
    match item {
        CustomContextMenuItem::Item {
            label, args, only, ..
        } => {
            if !context_menu_item_matches_only(only, target) {
                return None;
            }
            let executable = item.resolved_executable()?;
            let icon = match item.resolved_icon() {
                Some(ContextMenuConfiguredIcon::Image(path)) => {
                    ContextMenuIcon::ImagePathWithExecutableFallback(path)
                }
                Some(ContextMenuConfiguredIcon::Url(url)) => {
                    ContextMenuIcon::ImageUrlWithExecutableFallback(url)
                }
                Some(ContextMenuConfiguredIcon::NativePath(path)) => {
                    ContextMenuIcon::NativePath(path)
                }
                None => {
                    ContextMenuIcon::NativePath(item.resolved_executable_icon_path(&executable))
                }
            };
            Some(ContextMenuItem::Action {
                id: format!("context-menu-custom-{id_suffix}"),
                icon: Some(icon),
                label: label.clone(),
                command: ContextMenuCommand::RunCustom {
                    executable,
                    args: args.clone(),
                    targets: targets.to_vec(),
                },
                enabled: true,
            })
        }
        CustomContextMenuItem::Submenu { label, items, .. } => {
            let children = items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    configured_context_menu_item(
                        item,
                        targets,
                        target,
                        &format!("{id_suffix}-{index}"),
                    )
                })
                .collect::<Vec<_>>();
            let icon = match item.resolved_icon() {
                Some(ContextMenuConfiguredIcon::Image(path)) => {
                    Some(ContextMenuIcon::ImagePath(path))
                }
                Some(ContextMenuConfiguredIcon::Url(url)) => Some(ContextMenuIcon::ImageUrl(url)),
                Some(ContextMenuConfiguredIcon::NativePath(path)) => {
                    Some(ContextMenuIcon::NativePathOptional(path))
                }
                None => None,
            };
            (!children.is_empty()).then(|| ContextMenuItem::Submenu {
                id: format!("context-menu-custom-{id_suffix}"),
                icon,
                label: label.clone(),
                children,
            })
        }
    }
}

#[derive(Clone, Copy)]
enum CustomContextMenuTarget<'a> {
    Directory,
    Entries(&'a [FileEntry]),
}

fn context_menu_item_matches_only(only: &[String], target: CustomContextMenuTarget<'_>) -> bool {
    let filters = only
        .iter()
        .filter_map(|extension| resolve_context_menu_only_filter(extension))
        .collect::<Vec<_>>();

    match target {
        CustomContextMenuTarget::Directory => filters
            .iter()
            .any(|filter| matches!(filter, ContextMenuOnlyFilter::Directory)),
        CustomContextMenuTarget::Entries(selected_entries) => {
            if only.is_empty() {
                return true;
            }
            if selected_entries.is_empty() || filters.is_empty() {
                return false;
            }
            selected_entries
                .iter()
                .all(|entry| context_menu_entry_matches_any_filter(entry, &filters))
        }
    }
}

fn context_menu_entry_matches_any_filter(
    entry: &FileEntry,
    filters: &[ContextMenuOnlyFilter],
) -> bool {
    filters
        .iter()
        .any(|filter| context_menu_entry_matches_filter(entry, filter))
}

fn context_menu_entry_matches_filter(entry: &FileEntry, filter: &ContextMenuOnlyFilter) -> bool {
    match filter {
        ContextMenuOnlyFilter::Directory => false,
        ContextMenuOnlyFilter::File => entry_is_file_open_target(entry),
        ContextMenuOnlyFilter::Folder => directory_new_tab_target(entry).is_some(),
        ContextMenuOnlyFilter::Extension(candidate) => {
            entry_file_extension(entry).is_some_and(|extension| candidate == &extension)
        }
        ContextMenuOnlyFilter::Alias(candidates) => entry_file_extension(entry)
            .is_some_and(|extension| candidates.contains(&extension.as_str())),
    }
}

fn entry_file_extension(entry: &FileEntry) -> Option<String> {
    entry_is_file_open_target(entry).then(|| {
        entry
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
    })?
}

pub(super) fn context_menu_path_is_active(hovered_path: &[usize], path: &[usize]) -> bool {
    hovered_path.len() >= path.len() && hovered_path[..path.len()] == *path
}

pub(super) fn context_menu_item_is_persistently_active(
    item: &ContextMenuItem,
    hovered_path: &[usize],
    path: &[usize],
) -> bool {
    matches!(item, ContextMenuItem::Submenu { .. })
        && context_menu_path_is_active(hovered_path, path)
}

pub(super) fn context_menu_height(
    items: &[ContextMenuItem],
    row_height: f32,
    row_gap: f32,
    separator_height: f32,
) -> f32 {
    let content_height: f32 = items
        .iter()
        .map(|item| match item {
            ContextMenuItem::Separator => separator_height,
            ContextMenuItem::Action { .. }
            | ContextMenuItem::Submenu { .. }
            | ContextMenuItem::Detail { .. } => row_height + row_gap,
        })
        .sum();

    content_height + 8.0
}

pub(super) fn context_menu_item_top(
    items: &[ContextMenuItem],
    index: usize,
    row_height: f32,
    row_gap: f32,
    separator_height: f32,
) -> f32 {
    4.0 + items[..index]
        .iter()
        .map(|item| match item {
            ContextMenuItem::Separator => separator_height,
            ContextMenuItem::Action { .. }
            | ContextMenuItem::Submenu { .. }
            | ContextMenuItem::Detail { .. } => row_height + row_gap,
        })
        .sum::<f32>()
}

pub(super) fn clamped_context_menu_origin(
    origin: (f32, f32),
    menu_size: (f32, f32),
    window_size: (f32, f32),
) -> (f32, f32) {
    let max_x = (window_size.0 - menu_size.0).max(0.0);
    let max_y = (window_size.1 - menu_size.1).max(0.0);

    (origin.0.clamp(0.0, max_x), origin.1.clamp(0.0, max_y))
}

pub(super) fn context_menu_pointer_tip_origin(
    pointer: (f32, f32),
    menu_size: (f32, f32),
    window_size: (f32, f32),
) -> (f32, f32) {
    clamped_context_menu_origin(pointer, menu_size, window_size)
}

pub(super) fn context_submenu_left(
    parent_left: f32,
    parent_width: f32,
    child_width: f32,
    overlap: f32,
    window_width: f32,
) -> f32 {
    let right_left = parent_left + parent_width - overlap;
    if right_left + child_width <= window_width {
        right_left
    } else {
        parent_left - child_width + overlap
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::CustomContextMenuItem;
    use std::time::UNIX_EPOCH;

    fn configured_executable_path() -> PathBuf {
        let dir = unique_temp_dir("configured-executable");
        fs::create_dir_all(&dir).unwrap();
        let executable = dir.join(if cfg!(target_os = "windows") {
            "inspect.exe"
        } else {
            "inspect"
        });
        fs::write(&executable, "").unwrap();
        executable
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("explorer-context-menu-{name}-{nanos}"))
    }

    fn action_index(items: &[ContextMenuItem], expected_id: &str) -> usize {
        items
            .iter()
            .position(|item| {
                matches!(
                    item,
                    ContextMenuItem::Action { id, .. } if id == expected_id
                )
            })
            .expect("context menu action")
    }

    fn menu_has_label(items: &[ContextMenuItem], expected: &str) -> bool {
        items.iter().any(|item| match item {
            ContextMenuItem::Action { label, .. } | ContextMenuItem::Submenu { label, .. } => {
                label == expected
            }
            ContextMenuItem::Separator | ContextMenuItem::Detail { .. } => false,
        })
    }

    fn menu_extract_count(items: &[ContextMenuItem]) -> usize {
        items
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    ContextMenuItem::Action {
                        command: ContextMenuCommand::ExtractSelectedArchives,
                        ..
                    }
                )
            })
            .count()
    }

    fn menu_mount_count(items: &[ContextMenuItem]) -> usize {
        items
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    ContextMenuItem::Action {
                        command: ContextMenuCommand::MountSelectedImage,
                        ..
                    }
                )
            })
            .count()
    }

    fn attempted_run_elevated_paths(results: &[(PathBuf, io::Result<bool>)]) -> Vec<String> {
        results
            .iter()
            .map(|(path, _)| path.to_string_lossy().into_owned())
            .collect()
    }

    fn entry_menu_for_selected_entries(selected_entries: Vec<FileEntry>) -> Vec<ContextMenuItem> {
        let targets = selected_entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let selected_file_count = selected_entries
            .iter()
            .filter(|entry| entry_is_file_open_target(entry))
            .count();
        let selected_directory_count = selected_entries
            .iter()
            .filter(|entry| directory_new_tab_target(entry).is_some())
            .count();
        let single_directory_open_target = (selected_entries.len() == 1)
            .then(|| directory_new_tab_target(&selected_entries[0]))
            .flatten();

        entry_context_menu_items_with_custom(
            single_directory_open_target,
            selected_entries.len(),
            selected_file_count,
            selected_directory_count,
            false,
            false,
            &[],
            &targets,
            &selected_entries,
        )
    }

    fn custom_menu_for_entries(
        label: &str,
        only: &[&str],
        selected_entries: Vec<FileEntry>,
    ) -> Vec<ContextMenuItem> {
        let executable = configured_executable_path();
        let targets = selected_entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let selected_file_count = selected_entries
            .iter()
            .filter(|entry| entry_is_file_open_target(entry))
            .count();
        let selected_directory_count = selected_entries
            .iter()
            .filter(|entry| directory_new_tab_target(entry).is_some())
            .count();
        let single_directory_open_target = (selected_entries.len() == 1)
            .then(|| directory_new_tab_target(&selected_entries[0]))
            .flatten();
        let configured = vec![CustomContextMenuItem::Item {
            label: label.to_owned(),
            exe: executable,
            icon: None,
            args: Vec::new(),
            only: only.iter().map(|value| (*value).to_owned()).collect(),
        }];

        entry_context_menu_items_with_custom(
            single_directory_open_target,
            selected_entries.len(),
            selected_file_count,
            selected_directory_count,
            false,
            false,
            &configured,
            &targets,
            &selected_entries,
        )
    }

    use chrono::{Local, TimeZone};

    #[test]
    fn hovered_path_matches_active_branch() {
        let hovered = vec![1, 0, 2];

        assert!(context_menu_path_is_active(&hovered, &[1]));
        assert!(context_menu_path_is_active(&hovered, &[1, 0]));
        assert!(context_menu_path_is_active(&hovered, &[1, 0, 2]));
        assert!(!context_menu_path_is_active(&hovered, &[0]));
        assert!(!context_menu_path_is_active(&hovered, &[1, 1]));
        assert!(!context_menu_path_is_active(&hovered, &[1, 0, 2, 0]));
    }

    #[test]
    fn only_active_submenu_parents_are_persistently_active() {
        let action = ContextMenuItem::Action {
            id: "action".to_owned(),
            icon: None,
            label: "Action".to_owned(),
            command: ContextMenuCommand::Paste,
            enabled: true,
        };
        let detail = ContextMenuItem::Detail {
            label: "Created",
            value: "Today".to_owned(),
            icon_slot: ContextMenuIconSlot::Collapse,
        };
        let submenu = ContextMenuItem::Submenu {
            id: "submenu".to_owned(),
            icon: None,
            label: "New".to_owned(),
            children: Vec::new(),
        };
        let hovered = vec![1, 0];

        assert!(!context_menu_item_is_persistently_active(
            &action,
            &hovered,
            &[1, 0]
        ));
        assert!(!context_menu_item_is_persistently_active(
            &detail,
            &hovered,
            &[1, 0]
        ));
        assert!(context_menu_item_is_persistently_active(
            &submenu,
            &hovered,
            &[1]
        ));
        assert!(!context_menu_item_is_persistently_active(
            &submenu,
            &hovered,
            &[0]
        ));
    }

    #[test]
    fn root_menu_position_clamps_inside_window() {
        assert_eq!(
            context_menu_pointer_tip_origin((120.0, 90.0), (220.0, 180.0), (800.0, 600.0)),
            (120.0, 90.0)
        );
        assert_eq!(
            context_menu_pointer_tip_origin((780.0, 580.0), (220.0, 180.0), (800.0, 600.0)),
            (580.0, 420.0)
        );
        assert_eq!(
            context_menu_pointer_tip_origin((-20.0, -10.0), (220.0, 180.0), (800.0, 600.0)),
            (0.0, 0.0)
        );
    }

    #[test]
    fn submenu_position_overlaps_parent_border() {
        assert_eq!(context_submenu_left(100.0, 250.0, 250.0, 1.0, 800.0), 349.0);
        assert_eq!(context_submenu_left(500.0, 250.0, 250.0, 1.0, 800.0), 251.0);
    }

    #[test]
    fn submenu_position_uses_child_width_for_edge_fit() {
        assert_eq!(context_submenu_left(500.0, 180.0, 120.0, 1.0, 800.0), 679.0);
        assert_eq!(context_submenu_left(500.0, 180.0, 280.0, 1.0, 800.0), 221.0);
    }

    #[test]
    fn menu_height_includes_item_gaps() {
        let items = vec![
            ContextMenuItem::Action {
                id: "context-menu-paste".to_owned(),
                icon: Some(ContextMenuIcon::Paste),
                label: "Paste".to_owned(),
                command: ContextMenuCommand::Paste,
                enabled: true,
            },
            ContextMenuItem::Separator,
            ContextMenuItem::Detail {
                label: "Created",
                value: String::new(),
                icon_slot: ContextMenuIconSlot::Collapse,
            },
        ];

        assert_eq!(context_menu_height(&items, 28.0, 4.0, 9.0), 81.0);
    }

    #[test]
    fn item_top_includes_prior_item_gaps() {
        let items = vec![
            ContextMenuItem::Action {
                id: "context-menu-paste".to_owned(),
                icon: Some(ContextMenuIcon::Paste),
                label: "Paste".to_owned(),
                command: ContextMenuCommand::Paste,
                enabled: true,
            },
            ContextMenuItem::Submenu {
                id: "context-menu-new".to_owned(),
                icon: Some(ContextMenuIcon::New),
                label: "New".to_owned(),
                children: Vec::new(),
            },
            ContextMenuItem::Separator,
            ContextMenuItem::Detail {
                label: "Created",
                value: String::new(),
                icon_slot: ContextMenuIconSlot::Collapse,
            },
        ];

        assert_eq!(context_menu_item_top(&items, 0, 28.0, 4.0, 9.0), 4.0);
        assert_eq!(context_menu_item_top(&items, 1, 28.0, 4.0, 9.0), 36.0);
        assert_eq!(context_menu_item_top(&items, 3, 28.0, 4.0, 9.0), 77.0);
    }

    #[test]
    fn new_state_replaces_origin_for_reopen() {
        let first = ContextMenuState::new(
            Point {
                x: gpui::px(10.0),
                y: gpui::px(20.0),
            },
            Vec::new(),
        );
        let second = ContextMenuState::new(
            Point {
                x: gpui::px(70.0),
                y: gpui::px(80.0),
            },
            Vec::new(),
        );

        assert_ne!(first.origin, second.origin);
        assert_eq!(second.hovered_path, Vec::<usize>::new());
        assert_eq!(second.source, None);
        assert_eq!(second.native_icon_entry, None);
    }

    #[test]
    fn native_icon_state_retains_single_file_entry() {
        let entry = FileEntry::test("report.txt", false, Some(1), None);
        let state = ContextMenuState::new_with_native_icon_entry(
            Point {
                x: gpui::px(10.0),
                y: gpui::px(20.0),
            },
            Vec::new(),
            Some(entry.clone()),
        );

        assert_eq!(state.native_icon_entry, Some(entry));
        assert_eq!(state.source, None);
    }

    #[test]
    fn state_records_and_replaces_menu_source() {
        let first = ContextMenuState::new_with_source(
            Point {
                x: gpui::px(10.0),
                y: gpui::px(20.0),
            },
            Vec::new(),
            ContextMenuSource::SidebarItem { row_id: 1 },
        );
        let second = ContextMenuState::new_with_source(
            Point {
                x: gpui::px(70.0),
                y: gpui::px(80.0),
            },
            Vec::new(),
            ContextMenuSource::SidebarItem { row_id: 4 },
        );

        assert_eq!(
            first.source,
            Some(ContextMenuSource::SidebarItem { row_id: 1 })
        );
        assert_eq!(
            second.source,
            Some(ContextMenuSource::SidebarItem { row_id: 4 })
        );
    }

    #[test]
    fn folder_menu_contains_expected_items_and_icons() {
        let items = folder_context_menu_items_from_times(false, None, None);

        assert_eq!(items.len(), 8);
        assert_eq!(
            items[0],
            ContextMenuItem::Action {
                id: "context-menu-paste".to_owned(),
                icon: Some(ContextMenuIcon::Paste),
                label: "Paste".to_owned(),
                command: ContextMenuCommand::Paste,
                enabled: false,
            }
        );

        let ContextMenuItem::Submenu {
            icon,
            label,
            children,
            ..
        } = &items[1]
        else {
            panic!("expected New submenu");
        };
        assert_eq!(*icon, Some(ContextMenuIcon::New));
        assert_eq!(*label, "New");
        assert_eq!(children.len(), 2);
        assert_eq!(
            items[2],
            ContextMenuItem::Action {
                id: "context-menu-folder-copy-path".to_owned(),
                icon: Some(ContextMenuIcon::CopyAsPath),
                label: "Copy folder path".to_owned(),
                command: ContextMenuCommand::CopyPath {
                    path: PathBuf::from("folder")
                },
                enabled: true,
            }
        );
        assert!(matches!(items[3], ContextMenuItem::Separator));
        assert!(matches!(items[6], ContextMenuItem::Separator));
        assert_eq!(
            items[7],
            ContextMenuItem::Action {
                id: "context-menu-folder-properties".to_owned(),
                icon: Some(ContextMenuIcon::Properties),
                label: "Properties".to_owned(),
                command: ContextMenuCommand::PropertiesForPath {
                    path: PathBuf::from("folder")
                },
                enabled: true,
            }
        );
    }

    #[test]
    fn configured_sidebar_menu_contains_expected_items_icons_and_commands() {
        let path = PathBuf::from("/tmp/custom");
        let items = sidebar_context_menu_items(
            path.clone(),
            Some(2),
            Some(DirectoryKind::Downloads),
            false,
        );

        assert_eq!(items.len(), 4);
        assert_eq!(
            items[0],
            ContextMenuItem::Action {
                id: "context-menu-sidebar-open".to_owned(),
                icon: Some(ContextMenuIcon::FolderKindForPath {
                    path: path.clone(),
                    kind: Some(DirectoryKind::Downloads),
                }),
                label: "Open".to_owned(),
                command: ContextMenuCommand::OpenDirectory { path: path.clone() },
                enabled: true,
            }
        );
        assert_eq!(
            items[1],
            ContextMenuItem::Action {
                id: "context-menu-sidebar-open-new-tab".to_owned(),
                icon: Some(ContextMenuIcon::NewTab),
                label: "Open in new tab".to_owned(),
                command: ContextMenuCommand::OpenDirectoryInNewTab { path },
                enabled: true,
            }
        );
        assert!(matches!(items[2], ContextMenuItem::Separator));
        assert_eq!(
            items[3],
            ContextMenuItem::Action {
                id: "context-menu-sidebar-unpin".to_owned(),
                icon: Some(ContextMenuIcon::Unpin),
                label: "Unpin".to_owned(),
                command: ContextMenuCommand::UnpinSidebar {
                    configured_index: 2
                },
                enabled: true,
            }
        );
    }

    #[test]
    fn unconfigured_sidebar_menu_omits_separator_and_unpin() {
        let path = PathBuf::from("/tmp/drive");
        let items =
            sidebar_context_menu_items(path.clone(), None, Some(DirectoryKind::Drive), false);

        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0],
            ContextMenuItem::Action {
                id: "context-menu-sidebar-open".to_owned(),
                icon: Some(ContextMenuIcon::FolderKindForPath {
                    path: path.clone(),
                    kind: Some(DirectoryKind::Drive),
                }),
                label: "Open".to_owned(),
                command: ContextMenuCommand::OpenDirectory { path: path.clone() },
                enabled: true,
            }
        );
        assert_eq!(
            items[1],
            ContextMenuItem::Action {
                id: "context-menu-sidebar-open-new-tab".to_owned(),
                icon: Some(ContextMenuIcon::NewTab),
                label: "Open in new tab".to_owned(),
                command: ContextMenuCommand::OpenDirectoryInNewTab { path },
                enabled: true,
            }
        );
    }

    #[test]
    fn unconfigured_wsl_sidebar_menu_uses_wsl_icon_kind() {
        let path = PathBuf::from("\\\\wsl.localhost\\Ubuntu-24.04\\");
        let items =
            sidebar_context_menu_items(path.clone(), None, Some(DirectoryKind::DriveWsl), false);

        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0],
            ContextMenuItem::Action {
                id: "context-menu-sidebar-open".to_owned(),
                icon: Some(ContextMenuIcon::FolderKindForPath {
                    path: path.clone(),
                    kind: Some(DirectoryKind::DriveWsl),
                }),
                label: "Open".to_owned(),
                command: ContextMenuCommand::OpenDirectory { path: path.clone() },
                enabled: true,
            }
        );
        assert_eq!(
            items[1],
            ContextMenuItem::Action {
                id: "context-menu-sidebar-open-new-tab".to_owned(),
                icon: Some(ContextMenuIcon::NewTab),
                label: "Open in new tab".to_owned(),
                command: ContextMenuCommand::OpenDirectoryInNewTab { path },
                enabled: true,
            }
        );
    }

    #[test]
    fn ejectable_sidebar_drive_menu_contains_eject() {
        let path = PathBuf::from("/Volumes/Backup");
        let items =
            sidebar_context_menu_items(path.clone(), None, Some(DirectoryKind::Drive), true);

        assert_eq!(items.len(), 4);
        assert!(matches!(items[2], ContextMenuItem::Separator));
        assert_eq!(
            items[3],
            ContextMenuItem::Action {
                id: "context-menu-sidebar-eject".to_owned(),
                icon: Some(ContextMenuIcon::Eject),
                label: "Eject".to_owned(),
                command: ContextMenuCommand::EjectMountedVolume { path },
                enabled: true,
            }
        );
    }

    #[test]
    fn entry_menu_for_single_folder_contains_open_actions_and_rename_state() {
        let path = PathBuf::from("/tmp/folder-target");
        let items = entry_context_menu_items(Some(path.clone()), 1, 0, 1, false, false);

        assert_eq!(
            items,
            vec![
                ContextMenuItem::Action {
                    id: "context-menu-entry-open".to_owned(),
                    icon: Some(ContextMenuIcon::FolderKind(None)),
                    label: "Open".to_owned(),
                    command: ContextMenuCommand::OpenDirectory { path: path.clone() },
                    enabled: true,
                },
                ContextMenuItem::Action {
                    id: "context-menu-entry-open-new-tab".to_owned(),
                    icon: Some(ContextMenuIcon::NewTab),
                    label: "Open in new tab".to_owned(),
                    command: ContextMenuCommand::OpenSelectedDirectoriesInNewTabs,
                    enabled: true,
                },
                ContextMenuItem::Separator,
                ContextMenuItem::Action {
                    id: "context-menu-entry-cut".to_owned(),
                    icon: Some(ContextMenuIcon::Cut),
                    label: "Cut".to_owned(),
                    command: ContextMenuCommand::CutSelected,
                    enabled: true,
                },
                ContextMenuItem::Action {
                    id: "context-menu-entry-copy".to_owned(),
                    icon: Some(ContextMenuIcon::Copy),
                    label: "Copy".to_owned(),
                    command: ContextMenuCommand::CopySelected,
                    enabled: true,
                },
                ContextMenuItem::Action {
                    id: "context-menu-entry-copy-path".to_owned(),
                    icon: Some(ContextMenuIcon::CopyAsPath),
                    label: "Copy folder path".to_owned(),
                    command: ContextMenuCommand::CopyPath { path: path.clone() },
                    enabled: true,
                },
                ContextMenuItem::Separator,
                ContextMenuItem::Action {
                    id: "context-menu-entry-delete".to_owned(),
                    icon: Some(ContextMenuIcon::Delete),
                    label: "Delete".to_owned(),
                    command: ContextMenuCommand::DeleteSelected,
                    enabled: true,
                },
                ContextMenuItem::Action {
                    id: "context-menu-entry-rename".to_owned(),
                    icon: Some(ContextMenuIcon::Rename),
                    label: "Rename".to_owned(),
                    command: ContextMenuCommand::RenameSelected,
                    enabled: false,
                },
                ContextMenuItem::Separator,
                ContextMenuItem::Action {
                    id: "context-menu-entry-properties".to_owned(),
                    icon: Some(ContextMenuIcon::Properties),
                    label: "Properties".to_owned(),
                    command: ContextMenuCommand::PropertiesSelected,
                    enabled: true,
                },
            ]
        );

        let enabled_items =
            entry_context_menu_items(Some(PathBuf::from("/tmp/folder")), 1, 0, 1, true, false);
        assert!(matches!(
            enabled_items.iter().find(|item| matches!(
                item,
                ContextMenuItem::Action {
                    command: ContextMenuCommand::RenameSelected,
                    ..
                }
            )),
            Some(ContextMenuItem::Action {
                command: ContextMenuCommand::RenameSelected,
                enabled: true,
                ..
            })
        ));
    }

    #[test]
    fn entry_menu_for_single_file_opens_selected_file_and_can_rename() {
        let items = entry_context_menu_items(None, 1, 1, 0, true, true);

        assert_eq!(
            items.first(),
            Some(&ContextMenuItem::Action {
                id: "context-menu-entry-open".to_owned(),
                icon: Some(ContextMenuIcon::NativeFile),
                label: "Open".to_owned(),
                command: ContextMenuCommand::OpenSelectedFiles,
                enabled: true,
            })
        );
        assert!(matches!(
            items.iter().find(|item| matches!(
                item,
                ContextMenuItem::Action {
                    command: ContextMenuCommand::RenameSelected,
                    ..
                }
            )),
            Some(ContextMenuItem::Action {
                command: ContextMenuCommand::RenameSelected,
                enabled: true,
                ..
            })
        ));
        assert!(matches!(
            items.iter().find(|item| matches!(
                item,
                ContextMenuItem::Action {
                    command: ContextMenuCommand::CopyPath { .. },
                    ..
                }
            )),
            Some(ContextMenuItem::Action {
                id,
                icon: Some(ContextMenuIcon::CopyAsPath),
                label,
                command: ContextMenuCommand::CopyPath { path },
                enabled: true,
            }) if id == "context-menu-entry-copy-path"
                && label == "Copy file path"
                && path == Path::new("file-1.txt")
        ));
    }

    #[test]
    fn entry_menu_for_file_in_repo_shows_relative_repo_path_after_copy_path() {
        let repo = unique_temp_dir("file-relative-path-repo");
        fs::create_dir_all(repo.join("src").join("explorer")).unwrap();
        Repository::init(&repo).expect("init repo");
        let path = repo.join("src").join("explorer").join("context_menu.rs");
        fs::write(&path, b"file").unwrap();
        let entry = test_context_menu_entry(path.clone(), false);
        let items = entry_menu_for_selected_entries(vec![entry]);
        let copy_path_index = action_index(&items, "context-menu-entry-copy-path");

        assert_eq!(
            items.get(copy_path_index + 1),
            Some(&ContextMenuItem::Action {
                id: "context-menu-entry-copy-relative-repo-path".to_owned(),
                icon: Some(ContextMenuIcon::CopyAsPath),
                label: "Copy relative file path".to_owned(),
                command: ContextMenuCommand::CopyRepoRelativePath {
                    relative_path: "src/explorer/context_menu.rs".to_owned()
                },
                enabled: true,
            })
        );
    }

    #[test]
    fn folder_menu_for_folder_in_repo_shows_relative_repo_path_after_copy_path() {
        let repo = unique_temp_dir("folder-relative-path-repo");
        let path = repo.join("src").join("explorer");
        fs::create_dir_all(&path).unwrap();
        Repository::init(&repo).expect("init repo");
        let items = folder_context_menu_items_with_custom(
            &path,
            false,
            crate::settings::DEFAULT_DATE_FORMAT,
            &[],
        );
        let copy_path_index = action_index(&items, "context-menu-folder-copy-path");

        assert_eq!(
            items.get(copy_path_index + 1),
            Some(&ContextMenuItem::Action {
                id: "context-menu-folder-copy-relative-repo-path".to_owned(),
                icon: Some(ContextMenuIcon::CopyAsPath),
                label: "Copy relative folder path".to_owned(),
                command: ContextMenuCommand::CopyRepoRelativePath {
                    relative_path: "src/explorer".to_owned()
                },
                enabled: true,
            })
        );
    }

    #[test]
    fn context_menus_omit_relative_repo_path_outside_repositories() {
        let dir = unique_temp_dir("outside-relative-path-repo");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("file.txt");
        fs::write(&file, b"file").unwrap();
        let entry_items =
            entry_menu_for_selected_entries(vec![test_context_menu_entry(file.clone(), false)]);
        let folder_items = folder_context_menu_items_with_custom(
            &dir,
            false,
            crate::settings::DEFAULT_DATE_FORMAT,
            &[],
        );

        assert!(!entry_items.iter().any(|item| matches!(
            item,
            ContextMenuItem::Action {
                command: ContextMenuCommand::CopyRepoRelativePath { .. },
                ..
            }
        )));
        assert!(!folder_items.iter().any(|item| matches!(
            item,
            ContextMenuItem::Action {
                command: ContextMenuCommand::CopyRepoRelativePath { .. },
                ..
            }
        )));
    }

    #[test]
    fn folder_menu_for_repo_root_omits_relative_repo_path() {
        let repo = unique_temp_dir("root-relative-path-repo");
        fs::create_dir_all(&repo).unwrap();
        Repository::init(&repo).expect("init repo");
        let items = folder_context_menu_items_with_custom(
            &repo,
            false,
            crate::settings::DEFAULT_DATE_FORMAT,
            &[],
        );
        let copy_path_index = action_index(&items, "context-menu-folder-copy-path");

        assert!(matches!(
            items.get(copy_path_index + 1),
            Some(ContextMenuItem::Separator)
        ));
        assert!(!items.iter().any(|item| matches!(
            item,
            ContextMenuItem::Action {
                id,
                command: ContextMenuCommand::CopyRepoRelativePath { .. },
                ..
            } if id == "context-menu-folder-copy-relative-repo-path"
        )));
    }

    #[test]
    fn entry_menu_shows_open_with_only_for_one_ordinary_file() {
        let file = FileEntry::test("file.txt", false, Some(1), None);
        let items = entry_context_menu_items_with_custom(
            None,
            1,
            1,
            0,
            true,
            true,
            &[],
            std::slice::from_ref(&file.path),
            std::slice::from_ref(&file),
        );

        assert!(
            matches!(
                items.get(1),
                Some(ContextMenuItem::Action {
                    icon: Some(ContextMenuIcon::OpenWith),
                    label,
                    command: ContextMenuCommand::ChooseApplication { path },
                    ..
                }) if label == "Open with" && path == Path::new("file.txt")
            ) || matches!(
                items.get(1),
                Some(ContextMenuItem::Submenu {
                    icon: Some(ContextMenuIcon::OpenWith),
                    label,
                    ..
                }) if label == "Open with"
            )
        );

        let folder = FileEntry::test("folder", true, None, None);
        let folder_items = entry_context_menu_items_with_custom(
            Some(folder.path.clone()),
            1,
            0,
            1,
            true,
            false,
            &[],
            std::slice::from_ref(&folder.path),
            std::slice::from_ref(&folder),
        );
        assert!(!menu_has_label(&folder_items, "Open with"));

        let shortcut = FileEntry::test_directory_link(
            "linked",
            crate::explorer::entry::DirectoryLinkKind::FilesystemLink,
        );
        let shortcut_items = entry_context_menu_items_with_custom(
            None,
            1,
            0,
            1,
            true,
            false,
            &[],
            std::slice::from_ref(&shortcut.path),
            std::slice::from_ref(&shortcut),
        );
        assert!(!menu_has_label(&shortcut_items, "Open with"));

        let file_shortcut = FileEntry::test_directory_link(
            "file.lnk",
            crate::explorer::entry::DirectoryLinkKind::ShellShortcut {
                target: PathBuf::from("file.txt"),
                target_kind: crate::explorer::entry::ShellShortcutTargetKind::NonDirectory,
            },
        );
        let file_shortcut_items = entry_context_menu_items_with_custom(
            None,
            1,
            1,
            0,
            true,
            false,
            &[],
            std::slice::from_ref(&file_shortcut.path),
            std::slice::from_ref(&file_shortcut),
        );
        assert!(!menu_has_label(&file_shortcut_items, "Open with"));

        #[cfg(target_os = "macos")]
        {
            let app = FileEntry::test("Preview.app", true, None, None);
            let app_items = entry_context_menu_items_with_custom(
                None,
                1,
                1,
                0,
                true,
                true,
                &[],
                std::slice::from_ref(&app.path),
                std::slice::from_ref(&app),
            );
            assert!(!menu_has_label(&app_items, "Open with"));
        }

        let multi_items = entry_context_menu_items_with_custom(
            None,
            2,
            2,
            0,
            false,
            false,
            &[],
            &[PathBuf::from("first.txt"), PathBuf::from("second.txt")],
            &[
                FileEntry::test("first.txt", false, Some(1), None),
                FileEntry::test("second.txt", false, Some(1), None),
            ],
        );
        assert!(!menu_has_label(&multi_items, "Open with"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_entry_menu_omits_open_with_for_exe_case_insensitively() {
        for name in ["setup.exe", "SETUP.EXE"] {
            let items =
                entry_menu_for_selected_entries(vec![FileEntry::test(name, false, Some(1), None)]);

            assert!(menu_has_label(&items, "Open"));
            assert!(menu_has_label(&items, "Run as administrator"));
            assert!(!menu_has_label(&items, "Open with"));
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_entry_menu_shows_open_with_for_exe_files() {
        let items = entry_menu_for_selected_entries(vec![FileEntry::test(
            "setup.exe",
            false,
            Some(1),
            None,
        )]);

        assert!(menu_has_label(&items, "Open with"));
    }

    #[test]
    fn entry_menu_for_single_archive_shows_extract_before_first_separator() {
        let archive = FileEntry::test("archive.zip", false, Some(1), None);
        let items = entry_menu_for_selected_entries(vec![archive]);

        let extract_index = items
            .iter()
            .position(|item| {
                matches!(
                    item,
                    ContextMenuItem::Action {
                        command: ContextMenuCommand::ExtractSelectedArchives,
                        ..
                    }
                )
            })
            .expect("extract action");
        let first_separator = items
            .iter()
            .position(|item| matches!(item, ContextMenuItem::Separator))
            .expect("first separator");

        assert!(extract_index < first_separator);
        assert!(matches!(
            items.get(extract_index),
            Some(ContextMenuItem::Action {
                id,
                icon: Some(ContextMenuIcon::Extract),
                label,
                command: ContextMenuCommand::ExtractSelectedArchives,
                enabled: true,
            }) if id == "context-menu-entry-extract" && label == "Extract"
        ));
    }

    #[test]
    fn entry_menu_for_single_mountable_image_shows_mount_below_open() {
        let image = FileEntry::test("installer.iso", false, Some(1), None);
        let items = entry_menu_for_selected_entries(vec![image]);

        let open_index = action_index(&items, "context-menu-entry-open");
        let mount_index = action_index(&items, "context-menu-entry-mount");
        let first_separator = items
            .iter()
            .position(|item| matches!(item, ContextMenuItem::Separator))
            .expect("first separator");

        assert_eq!(mount_index, open_index + 1);
        assert!(mount_index < first_separator);
        assert!(matches!(
            items.get(mount_index),
            Some(ContextMenuItem::Action {
                id,
                icon: Some(ContextMenuIcon::Mount),
                label,
                command: ContextMenuCommand::MountSelectedImage,
                enabled: true,
            }) if id == "context-menu-entry-mount" && label == "Mount"
        ));
    }

    #[test]
    fn entry_menu_omits_mount_for_unsupported_multi_selection_and_folders() {
        let image = FileEntry::test("installer.iso", false, Some(1), None);
        let other_image = FileEntry::test("rescue.img", false, Some(1), None);
        let text = FileEntry::test("notes.txt", false, Some(1), None);
        let folder = FileEntry::test("folder.iso", true, None, None);

        assert_eq!(
            menu_mount_count(&entry_menu_for_selected_entries(vec![
                image.clone(),
                other_image
            ])),
            0
        );
        assert_eq!(
            menu_mount_count(&entry_menu_for_selected_entries(vec![text])),
            0
        );
        assert_eq!(
            menu_mount_count(&entry_menu_for_selected_entries(vec![folder])),
            0
        );
    }

    #[test]
    fn entry_menu_for_multiple_archives_shows_one_extract_action() {
        let items = entry_menu_for_selected_entries(vec![
            FileEntry::test("archive.zip", false, Some(1), None),
            FileEntry::test("package.tar.gz", false, Some(1), None),
        ]);

        assert_eq!(menu_extract_count(&items), 1);
        assert!(matches!(
            items.get(1),
            Some(ContextMenuItem::Action {
                id,
                icon: Some(ContextMenuIcon::Extract),
                label,
                command: ContextMenuCommand::ExtractSelectedArchives,
                enabled: true,
            }) if id == "context-menu-entry-extract" && label == "Extract"
        ));
    }

    #[test]
    fn entry_menu_omits_extract_for_non_archive_mixed_folder_and_empty_selection() {
        let archive = FileEntry::test("archive.zip", false, Some(1), None);
        let text = FileEntry::test("notes.txt", false, Some(1), None);
        let folder = FileEntry::test("folder.zip", true, None, None);

        assert_eq!(
            menu_extract_count(&entry_menu_for_selected_entries(vec![
                archive.clone(),
                text
            ])),
            0
        );
        assert_eq!(
            menu_extract_count(&entry_menu_for_selected_entries(vec![archive, folder])),
            0
        );
        assert_eq!(
            menu_extract_count(&entry_menu_for_selected_entries(Vec::new())),
            0
        );
    }

    #[test]
    fn run_elevated_target_detection_accepts_exe_and_bat_case_insensitively() {
        assert!(entry_is_run_elevated_target(&FileEntry::test(
            "setup.exe",
            false,
            Some(1),
            None
        )));
        assert!(entry_is_run_elevated_target(&FileEntry::test(
            "SCRIPT.BAT",
            false,
            Some(1),
            None
        )));
        assert!(selected_entries_are_run_elevated_targets(&[
            FileEntry::test("setup.EXE", false, Some(1), None),
            FileEntry::test("script.bat", false, Some(1), None),
        ]));
    }

    #[test]
    fn run_elevated_target_detection_rejects_empty_mixed_and_non_files() {
        assert!(!selected_entries_are_run_elevated_targets(&[]));
        assert!(!selected_entries_are_run_elevated_targets(&[
            FileEntry::test("setup.exe", false, Some(1), None),
            FileEntry::test("note.txt", false, Some(1), None),
        ]));
        assert!(!entry_is_run_elevated_target(&FileEntry::test(
            "folder.bat",
            true,
            None,
            None
        )));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn entry_menu_shows_run_as_administrator_for_all_elevatable_files() {
        let items = entry_menu_for_selected_entries(vec![
            FileEntry::test("setup.exe", false, Some(1), None),
            FileEntry::test("script.bat", false, Some(1), None),
        ]);

        assert!(matches!(
            items.get(1),
            Some(ContextMenuItem::Action {
                id,
                icon: Some(ContextMenuIcon::RunElevated),
                label,
                command: ContextMenuCommand::RunSelectedElevated { paths },
                enabled: true,
            }) if id == "context-menu-entry-run-elevated"
                && label == "Run as administrator"
                && paths == &vec![PathBuf::from("setup.exe"), PathBuf::from("script.bat")]
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn entry_menu_omits_run_as_administrator_for_ineligible_selection() {
        let items = entry_menu_for_selected_entries(vec![
            FileEntry::test("setup.exe", false, Some(1), None),
            FileEntry::test("note.txt", false, Some(1), None),
        ]);

        assert!(!menu_has_label(&items, "Run as administrator"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_open_with_submenu_always_ends_with_other() {
        let item = crate::explorer::open_with::context_menu_item(Path::new("file.txt"));
        let ContextMenuItem::Submenu { children, .. } = item else {
            panic!("expected Open with submenu");
        };

        assert!(matches!(
            children.last(),
            Some(ContextMenuItem::Action {
                label,
                command: ContextMenuCommand::ChooseApplication { .. },
                ..
            }) if label == "Other..."
        ));
    }

    #[test]
    fn entry_menu_for_files_only_multi_selection_opens_files_and_omits_rename() {
        let items = entry_context_menu_items(None, 2, 2, 0, false, false);

        assert_eq!(
            items,
            vec![
                ContextMenuItem::Action {
                    id: "context-menu-entry-open".to_owned(),
                    icon: Some(ContextMenuIcon::File),
                    label: "Open files (2)".to_owned(),
                    command: ContextMenuCommand::OpenSelectedFiles,
                    enabled: true,
                },
                ContextMenuItem::Separator,
                ContextMenuItem::Action {
                    id: "context-menu-entry-cut".to_owned(),
                    icon: Some(ContextMenuIcon::Cut),
                    label: "Cut".to_owned(),
                    command: ContextMenuCommand::CutSelected,
                    enabled: true,
                },
                ContextMenuItem::Action {
                    id: "context-menu-entry-copy".to_owned(),
                    icon: Some(ContextMenuIcon::Copy),
                    label: "Copy".to_owned(),
                    command: ContextMenuCommand::CopySelected,
                    enabled: true,
                },
                ContextMenuItem::Separator,
                ContextMenuItem::Action {
                    id: "context-menu-entry-delete".to_owned(),
                    icon: Some(ContextMenuIcon::Delete),
                    label: "Delete".to_owned(),
                    command: ContextMenuCommand::DeleteSelected,
                    enabled: true,
                },
                ContextMenuItem::Separator,
                ContextMenuItem::Action {
                    id: "context-menu-entry-properties".to_owned(),
                    icon: Some(ContextMenuIcon::Properties),
                    label: "Properties".to_owned(),
                    command: ContextMenuCommand::PropertiesSelected,
                    enabled: true,
                },
            ]
        );
    }

    #[test]
    fn entry_menu_for_folders_only_multi_selection_opens_new_tabs_and_omits_rename() {
        let items = entry_context_menu_items(None, 2, 0, 2, false, false);

        assert_eq!(
            items,
            vec![
                ContextMenuItem::Action {
                    id: "context-menu-entry-open-new-tab".to_owned(),
                    icon: Some(ContextMenuIcon::NewTab),
                    label: "Open new tabs (2)".to_owned(),
                    command: ContextMenuCommand::OpenSelectedDirectoriesInNewTabs,
                    enabled: true,
                },
                ContextMenuItem::Separator,
                ContextMenuItem::Action {
                    id: "context-menu-entry-cut".to_owned(),
                    icon: Some(ContextMenuIcon::Cut),
                    label: "Cut".to_owned(),
                    command: ContextMenuCommand::CutSelected,
                    enabled: true,
                },
                ContextMenuItem::Action {
                    id: "context-menu-entry-copy".to_owned(),
                    icon: Some(ContextMenuIcon::Copy),
                    label: "Copy".to_owned(),
                    command: ContextMenuCommand::CopySelected,
                    enabled: true,
                },
                ContextMenuItem::Separator,
                ContextMenuItem::Action {
                    id: "context-menu-entry-delete".to_owned(),
                    icon: Some(ContextMenuIcon::Delete),
                    label: "Delete".to_owned(),
                    command: ContextMenuCommand::DeleteSelected,
                    enabled: true,
                },
                ContextMenuItem::Separator,
                ContextMenuItem::Action {
                    id: "context-menu-entry-properties".to_owned(),
                    icon: Some(ContextMenuIcon::Properties),
                    label: "Properties".to_owned(),
                    command: ContextMenuCommand::PropertiesSelected,
                    enabled: true,
                },
            ]
        );
    }

    #[test]
    fn entry_menu_for_any_selection_ends_with_properties() {
        let items = entry_context_menu_items(None, 2, 1, 1, false, false);

        assert!(matches!(
            items.last(),
            Some(ContextMenuItem::Action {
                id,
                icon: Some(ContextMenuIcon::Properties),
                label,
                command: ContextMenuCommand::PropertiesSelected,
                enabled: true,
            }) if id == "context-menu-entry-properties" && label == "Properties"
        ));
    }

    #[test]
    fn entry_menu_for_mixed_multi_selection_keeps_file_and_folder_open_actions() {
        let items = entry_context_menu_items(None, 3, 1, 2, false, false);

        assert!(matches!(
            items.first(),
            Some(ContextMenuItem::Action {
                label,
                icon: Some(ContextMenuIcon::File),
                command: ContextMenuCommand::OpenSelectedFiles,
                ..
            }) if label == "Open files (1)"
        ));
        assert!(matches!(
            items.get(1),
            Some(ContextMenuItem::Action {
                label,
                command: ContextMenuCommand::OpenSelectedDirectoriesInNewTabs,
                ..
            }) if label == "Open new tabs (2)"
        ));
        assert!(matches!(items.get(2), Some(ContextMenuItem::Separator)));
        assert!(!items.iter().any(|item| matches!(
            item,
            ContextMenuItem::Action {
                command: ContextMenuCommand::RenameSelected,
                ..
            }
        )));
        assert!(!items.iter().any(|item| matches!(
            item,
            ContextMenuItem::Action {
                command: ContextMenuCommand::CopyPath { .. },
                ..
            }
        )));
    }

    #[test]
    fn entry_menu_for_mixed_selection_uses_singular_new_tab_label() {
        let items = entry_context_menu_items(None, 3, 2, 1, false, false);

        assert!(matches!(
            items.get(1),
            Some(ContextMenuItem::Action {
                label,
                command: ContextMenuCommand::OpenSelectedDirectoriesInNewTabs,
                ..
            }) if label == "Open in new tab"
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn single_app_bundle_uses_native_file_icon_context() {
        let entry = FileEntry::test("Preview.app", true, None, None);
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.entries = vec![entry.clone()];
        view.select_single_index(0);

        let context = view.selected_entry_context();

        assert_eq!(context.file_open_count, 1);
        assert_eq!(context.native_icon_entry, Some(entry));
        assert_eq!(context.single_directory_open_target, None);
    }

    #[test]
    fn opening_selected_files_attempts_files_in_selection_order_and_ignores_folders() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.entries = vec![
            FileEntry::test("folder", true, None, None),
            FileEntry::test("a.txt", false, Some(1), None),
            FileEntry::test("b.txt", false, Some(1), None),
        ];
        view.select_all_entries();

        let mut opened = Vec::new();
        view.open_selected_files_with(|path| {
            opened.push(path.to_path_buf());
            Ok(())
        });

        assert_eq!(opened, vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")]);
    }

    #[test]
    fn run_elevated_paths_stop_after_cancel_or_error() {
        let cancelled = run_elevated_paths_until_not_launched(
            vec![PathBuf::from("a.exe"), PathBuf::from("b.bat")],
            |_| Ok(false),
        );
        assert_eq!(attempted_run_elevated_paths(&cancelled), vec!["a.exe"]);

        let mut calls = 0;
        let errored = run_elevated_paths_until_not_launched(
            vec![
                PathBuf::from("a.exe"),
                PathBuf::from("b.bat"),
                PathBuf::from("c.exe"),
            ],
            |_| {
                calls += 1;
                if calls == 2 {
                    Err(io::Error::other("denied"))
                } else {
                    Ok(true)
                }
            },
        );
        assert_eq!(
            attempted_run_elevated_paths(&errored),
            vec!["a.exe", "b.bat"]
        );
    }

    #[test]
    fn configured_entry_items_are_inserted_after_first_separator_with_selected_targets() {
        let executable = configured_executable_path();
        let targets = vec![PathBuf::from("a.txt"), PathBuf::from("folder")];
        let configured = vec![
            CustomContextMenuItem::Item {
                label: "Inspect".to_owned(),
                exe: executable.clone(),
                icon: None,
                args: vec!["--inspect".to_owned()],
                only: Vec::new(),
            },
            CustomContextMenuItem::Submenu {
                label: "Tools".to_owned(),
                icon: None,
                items: vec![CustomContextMenuItem::Item {
                    label: "Deep inspect".to_owned(),
                    exe: executable.clone(),
                    icon: None,
                    args: Vec::new(),
                    only: Vec::new(),
                }],
            },
        ];

        let items = entry_context_menu_items_with_custom(
            None,
            2,
            1,
            1,
            false,
            false,
            &configured,
            &targets,
            &[],
        );

        assert!(matches!(items[2], ContextMenuItem::Separator));
        assert!(matches!(
            &items[3],
            ContextMenuItem::Action {
                label,
                icon: Some(ContextMenuIcon::NativePath(path)),
                command: ContextMenuCommand::RunCustom {
                    executable: command,
                    args,
                    targets: command_targets,
                },
                ..
            } if label == "Inspect"
                && path == &executable
                && command == &executable
                && args == &vec!["--inspect".to_owned()]
                && command_targets == &targets
        ));
        assert!(matches!(
            &items[4],
            ContextMenuItem::Submenu { label, children, .. }
                if label == "Tools" && matches!(
                    children.first(),
                    Some(ContextMenuItem::Action {
                        command: ContextMenuCommand::RunCustom { targets: child_targets, .. },
                        ..
                    }) if child_targets == &targets
                )
        ));
        assert!(matches!(items[5], ContextMenuItem::Separator));
        assert!(matches!(
            items[6],
            ContextMenuItem::Action {
                command: ContextMenuCommand::CutSelected,
                ..
            }
        ));
    }

    #[test]
    fn configured_entry_items_use_explicit_icons_before_executable_icons() {
        let command = configured_executable_path();
        let icon_executable = configured_executable_path();
        let image_icon = unique_temp_dir("custom-image-icon").join("tool.svg");
        let targets = vec![PathBuf::from("a.txt")];
        let configured = vec![
            CustomContextMenuItem::Item {
                label: "Image icon".to_owned(),
                exe: command.clone(),
                icon: Some(image_icon.clone()),
                args: Vec::new(),
                only: Vec::new(),
            },
            CustomContextMenuItem::Item {
                label: "Executable icon".to_owned(),
                exe: command.clone(),
                icon: Some(icon_executable.clone()),
                args: Vec::new(),
                only: Vec::new(),
            },
        ];

        let items = entry_context_menu_items_with_custom(
            None,
            1,
            1,
            0,
            false,
            false,
            &configured,
            &targets,
            &[],
        );

        assert!(matches!(
            &items[2],
            ContextMenuItem::Action {
                icon: Some(ContextMenuIcon::ImagePathWithExecutableFallback(path)),
                command: ContextMenuCommand::RunCustom { executable, .. },
                ..
            } if path == &image_icon && executable == &command
        ));
        assert!(matches!(
            &items[3],
            ContextMenuItem::Action {
                icon: Some(ContextMenuIcon::NativePath(path)),
                command: ContextMenuCommand::RunCustom { executable, .. },
                ..
            } if path == &icon_executable && executable == &command
        ));
    }

    #[test]
    fn configured_submenus_include_explicit_image_and_executable_icons() {
        let executable = configured_executable_path();
        let icon_executable = configured_executable_path();
        let image_icon = unique_temp_dir("custom-submenu-image-icon").join("tools.ico");
        let targets = vec![PathBuf::from("a.txt")];
        let child = CustomContextMenuItem::Item {
            label: "Inspect".to_owned(),
            exe: executable,
            icon: None,
            args: Vec::new(),
            only: Vec::new(),
        };
        let configured = vec![
            CustomContextMenuItem::Submenu {
                label: "Image tools".to_owned(),
                icon: Some(image_icon.clone()),
                items: vec![child.clone()],
            },
            CustomContextMenuItem::Submenu {
                label: "Executable tools".to_owned(),
                icon: Some(icon_executable.clone()),
                items: vec![child],
            },
        ];

        let items = entry_context_menu_items_with_custom(
            None,
            1,
            1,
            0,
            false,
            false,
            &configured,
            &targets,
            &[],
        );

        assert!(matches!(
            &items[2],
            ContextMenuItem::Submenu {
                icon: Some(ContextMenuIcon::ImagePath(path)),
                ..
            } if path == &image_icon
        ));
        assert!(matches!(
            &items[3],
            ContextMenuItem::Submenu {
                icon: Some(ContextMenuIcon::NativePathOptional(path)),
                ..
            } if path == &icon_executable
        ));
    }

    #[test]
    fn configured_items_and_submenus_include_explicit_url_icons() {
        let executable = configured_executable_path();
        let action_url = "https://example.com/action.svg";
        let submenu_url = "https://example.com/submenu.ico";
        let child = CustomContextMenuItem::Item {
            label: "Inspect".to_owned(),
            exe: executable.clone(),
            icon: None,
            args: Vec::new(),
            only: Vec::new(),
        };
        let configured = vec![
            CustomContextMenuItem::Item {
                label: "URL action".to_owned(),
                exe: executable,
                icon: Some(PathBuf::from(action_url)),
                args: Vec::new(),
                only: Vec::new(),
            },
            CustomContextMenuItem::Submenu {
                label: "URL submenu".to_owned(),
                icon: Some(PathBuf::from(submenu_url)),
                items: vec![child],
            },
        ];

        let items = entry_context_menu_items_with_custom(
            None,
            1,
            1,
            0,
            false,
            false,
            &configured,
            &[PathBuf::from("a.txt")],
            &[],
        );

        assert!(matches!(
            &items[2],
            ContextMenuItem::Action {
                icon: Some(ContextMenuIcon::ImageUrlWithExecutableFallback(url)),
                ..
            } if url == action_url
        ));
        assert!(matches!(
            &items[3],
            ContextMenuItem::Submenu {
                icon: Some(ContextMenuIcon::ImageUrl(url)),
                ..
            } if url == submenu_url
        ));
    }

    #[test]
    fn configured_entry_items_skip_only_missing_executables() {
        let executable = configured_executable_path();
        let missing = executable.with_file_name("missing-inspect");
        let targets = vec![PathBuf::from("a.txt")];
        let configured = vec![
            CustomContextMenuItem::Item {
                label: "Missing".to_owned(),
                exe: missing.clone(),
                icon: None,
                args: Vec::new(),
                only: Vec::new(),
            },
            CustomContextMenuItem::Item {
                label: "Inspect".to_owned(),
                exe: executable.clone(),
                icon: None,
                args: Vec::new(),
                only: Vec::new(),
            },
            CustomContextMenuItem::Submenu {
                label: "Tools".to_owned(),
                icon: None,
                items: vec![
                    CustomContextMenuItem::Item {
                        label: "Missing child".to_owned(),
                        exe: missing.clone(),
                        icon: None,
                        args: Vec::new(),
                        only: Vec::new(),
                    },
                    CustomContextMenuItem::Item {
                        label: "Deep inspect".to_owned(),
                        exe: executable.clone(),
                        icon: None,
                        args: Vec::new(),
                        only: Vec::new(),
                    },
                ],
            },
            CustomContextMenuItem::Submenu {
                label: "Empty".to_owned(),
                icon: None,
                items: vec![CustomContextMenuItem::Item {
                    label: "Only missing".to_owned(),
                    exe: missing,
                    icon: None,
                    args: Vec::new(),
                    only: Vec::new(),
                }],
            },
        ];

        let items = entry_context_menu_items_with_custom(
            None,
            1,
            1,
            0,
            false,
            false,
            &configured,
            &targets,
            &[],
        );

        assert!(matches!(
            &items[2],
            ContextMenuItem::Action { label, .. } if label == "Inspect"
        ));
        assert!(matches!(
            &items[3],
            ContextMenuItem::Submenu { label, children, .. }
                if label == "Tools"
                    && children.len() == 1
                    && matches!(
                        &children[0],
                        ContextMenuItem::Action { label, .. } if label == "Deep inspect"
                    )
        ));
        assert!(matches!(items[4], ContextMenuItem::Separator));
        assert!(!items.iter().any(|item| matches!(
            item,
            ContextMenuItem::Action { label, .. }
            | ContextMenuItem::Submenu { label, .. }
                if label == "Missing" || label == "Empty"
        )));
    }

    #[test]
    fn configured_entry_items_filter_by_only_extensions() {
        let executable = configured_executable_path();
        let targets = vec![PathBuf::from("README.TXT")];
        let selected_entries = vec![FileEntry::test("README.TXT", false, Some(1), None)];
        let configured = vec![
            CustomContextMenuItem::Item {
                label: "Text tool".to_owned(),
                exe: executable.clone(),
                icon: None,
                args: Vec::new(),
                only: vec!["txt".to_owned(), ".MD".to_owned()],
            },
            CustomContextMenuItem::Item {
                label: "Image tool".to_owned(),
                exe: executable.clone(),
                icon: None,
                args: Vec::new(),
                only: vec!["png".to_owned()],
            },
            CustomContextMenuItem::Item {
                label: "Any tool".to_owned(),
                exe: executable,
                icon: None,
                args: Vec::new(),
                only: Vec::new(),
            },
        ];

        let items = entry_context_menu_items_with_custom(
            None,
            1,
            1,
            0,
            false,
            false,
            &configured,
            &targets,
            &selected_entries,
        );

        assert!(menu_has_label(&items, "Text tool"));
        assert!(menu_has_label(&items, "Any tool"));
        assert!(!menu_has_label(&items, "Image tool"));
    }

    #[test]
    fn configured_entry_items_ignore_directory_only_filter_and_match_additive_extensions() {
        let directory_only = custom_menu_for_entries(
            "Directory only",
            &["*directory"],
            vec![FileEntry::test("photo.png", false, Some(1), None)],
        );
        assert!(!menu_has_label(&directory_only, "Directory only"));

        let png_or_directory = custom_menu_for_entries(
            "Png or directory",
            &[".png", "*directory"],
            vec![FileEntry::test("photo.PNG", false, Some(1), None)],
        );
        assert!(menu_has_label(&png_or_directory, "Png or directory"));

        let non_png = custom_menu_for_entries(
            "Png or directory",
            &[".png", "*directory"],
            vec![FileEntry::test("report.txt", false, Some(1), None)],
        );
        assert!(!menu_has_label(&non_png, "Png or directory"));
    }

    #[test]
    fn configured_entry_items_filter_by_only_media_aliases() {
        let image_items = custom_menu_for_entries(
            "Image tool",
            &["*image"],
            vec![
                FileEntry::test("photo.JPG", false, Some(1), None),
                FileEntry::test("design.svg", false, Some(1), None),
            ],
        );
        assert!(menu_has_label(&image_items, "Image tool"));

        let photo_items = custom_menu_for_entries(
            "Photo tool",
            &["*photo"],
            vec![
                FileEntry::test("photo.JPG", false, Some(1), None),
                FileEntry::test("design.svg", false, Some(1), None),
            ],
        );
        assert!(menu_has_label(&photo_items, "Photo tool"));

        let audio_items = custom_menu_for_entries(
            "Audio tool",
            &["*audio"],
            vec![
                FileEntry::test("song.M4A", false, Some(1), None),
                FileEntry::test("mix.OPUS", false, Some(1), None),
            ],
        );
        assert!(menu_has_label(&audio_items, "Audio tool"));

        let video_items = custom_menu_for_entries(
            "Video tool",
            &["*video"],
            vec![
                FileEntry::test("clip.MP4", false, Some(1), None),
                FileEntry::test("movie.mkv", false, Some(1), None),
            ],
        );
        assert!(menu_has_label(&video_items, "Video tool"));

        let mixed_items = custom_menu_for_entries(
            "Image tool",
            &["*image"],
            vec![
                FileEntry::test("photo.JPG", false, Some(1), None),
                FileEntry::test("song.mp3", false, Some(1), None),
            ],
        );
        assert!(!menu_has_label(&mixed_items, "Image tool"));

        let combined_items = custom_menu_for_entries(
            "Image or PDF tool",
            &["*image", "pdf"],
            vec![
                FileEntry::test("photo.JPG", false, Some(1), None),
                FileEntry::test("report.PDF", false, Some(1), None),
            ],
        );
        assert!(menu_has_label(&combined_items, "Image or PDF tool"));
    }

    #[test]
    fn configured_entry_items_filter_by_only_file_and_folder_aliases_additively() {
        let file_items = custom_menu_for_entries(
            "Files tool",
            &["*file"],
            vec![
                FileEntry::test("readme.txt", false, Some(1), None),
                FileEntry::test("archive", false, Some(1), None),
            ],
        );
        assert!(menu_has_label(&file_items, "Files tool"));

        let files_with_extension_override = custom_menu_for_entries(
            "Files override",
            &["*files", "png"],
            vec![FileEntry::test("readme.txt", false, Some(1), None)],
        );
        assert!(menu_has_label(
            &files_with_extension_override,
            "Files override"
        ));

        let folder_items = custom_menu_for_entries(
            "Folders tool",
            &["*folder"],
            vec![
                FileEntry::test("src", true, None, None),
                FileEntry::test("target", true, None, None),
            ],
        );
        assert!(menu_has_label(&folder_items, "Folders tool"));

        let folders_with_extension_override = custom_menu_for_entries(
            "Folders override",
            &["*folders", "txt"],
            vec![FileEntry::test("src", true, None, None)],
        );
        assert!(menu_has_label(
            &folders_with_extension_override,
            "Folders override"
        ));

        let file_for_folders = custom_menu_for_entries(
            "Folders tool",
            &["*folder"],
            vec![FileEntry::test("readme.txt", false, Some(1), None)],
        );
        assert!(!menu_has_label(&file_for_folders, "Folders tool"));

        let folder_for_files = custom_menu_for_entries(
            "Files tool",
            &["*file"],
            vec![FileEntry::test("src", true, None, None)],
        );
        assert!(!menu_has_label(&folder_for_files, "Files tool"));

        let mixed_for_files = custom_menu_for_entries(
            "Files tool",
            &["*file"],
            vec![
                FileEntry::test("readme.txt", false, Some(1), None),
                FileEntry::test("src", true, None, None),
            ],
        );
        assert!(!menu_has_label(&mixed_for_files, "Files tool"));

        let mixed_for_folders = custom_menu_for_entries(
            "Folders tool",
            &["*folder"],
            vec![
                FileEntry::test("readme.txt", false, Some(1), None),
                FileEntry::test("src", true, None, None),
            ],
        );
        assert!(!menu_has_label(&mixed_for_folders, "Folders tool"));

        let both_for_files = custom_menu_for_entries(
            "Any item kind",
            &["*file", "*folder"],
            vec![FileEntry::test("readme.txt", false, Some(1), None)],
        );
        assert!(menu_has_label(&both_for_files, "Any item kind"));

        let both_for_folders = custom_menu_for_entries(
            "Any item kind",
            &["*file", "*folder"],
            vec![FileEntry::test("src", true, None, None)],
        );
        assert!(menu_has_label(&both_for_folders, "Any item kind"));

        let both_for_mixed = custom_menu_for_entries(
            "Any item kind",
            &["*file", "*folder"],
            vec![
                FileEntry::test("readme.txt", false, Some(1), None),
                FileEntry::test("src", true, None, None),
            ],
        );
        assert!(menu_has_label(&both_for_mixed, "Any item kind"));
    }

    #[test]
    fn configured_entry_only_filters_require_every_selected_file_to_match() {
        let executable = configured_executable_path();
        let configured = vec![CustomContextMenuItem::Item {
            label: "Code tool".to_owned(),
            exe: executable,
            icon: None,
            args: Vec::new(),
            only: vec!["rs".to_owned(), ".toml".to_owned()],
        }];
        let matching_targets = vec![PathBuf::from("main.rs"), PathBuf::from("Cargo.TOML")];
        let matching_entries = vec![
            FileEntry::test("main.rs", false, Some(1), None),
            FileEntry::test("Cargo.TOML", false, Some(1), None),
        ];
        let mixed_entries = vec![
            FileEntry::test("main.rs", false, Some(1), None),
            FileEntry::test("README.md", false, Some(1), None),
        ];
        let file_and_folder_entries = vec![
            FileEntry::test("main.rs", false, Some(1), None),
            FileEntry::test("src", true, None, None),
        ];

        let matching_items = entry_context_menu_items_with_custom(
            None,
            2,
            2,
            0,
            false,
            false,
            &configured,
            &matching_targets,
            &matching_entries,
        );
        assert!(menu_has_label(&matching_items, "Code tool"));

        let mixed_items = entry_context_menu_items_with_custom(
            None,
            2,
            2,
            0,
            false,
            false,
            &configured,
            &matching_targets,
            &mixed_entries,
        );
        assert!(!menu_has_label(&mixed_items, "Code tool"));

        let file_and_folder_items = entry_context_menu_items_with_custom(
            None,
            2,
            1,
            1,
            false,
            false,
            &configured,
            &matching_targets,
            &file_and_folder_entries,
        );
        assert!(!menu_has_label(&file_and_folder_items, "Code tool"));
    }

    #[test]
    fn configured_entry_only_filters_omit_empty_submenus() {
        let executable = configured_executable_path();
        let targets = vec![PathBuf::from("README.txt")];
        let selected_entries = vec![FileEntry::test("README.txt", false, Some(1), None)];
        let configured = vec![CustomContextMenuItem::Submenu {
            label: "Tools".to_owned(),
            icon: None,
            items: vec![CustomContextMenuItem::Item {
                label: "Image tool".to_owned(),
                exe: executable,
                icon: None,
                args: Vec::new(),
                only: vec!["png".to_owned()],
            }],
        }];

        let items = entry_context_menu_items_with_custom(
            None,
            1,
            1,
            0,
            false,
            false,
            &configured,
            &targets,
            &selected_entries,
        );

        assert!(!menu_has_label(&items, "Tools"));
    }

    #[test]
    fn configured_directory_items_require_directory_only_filter_and_receive_current_directory() {
        let directory = PathBuf::from("current");
        let executable = configured_executable_path();
        let configured = vec![
            CustomContextMenuItem::Submenu {
                label: "Empty".to_owned(),
                icon: None,
                items: Vec::new(),
            },
            CustomContextMenuItem::Item {
                label: "Implicit entry-only".to_owned(),
                exe: executable.clone(),
                icon: None,
                args: Vec::new(),
                only: Vec::new(),
            },
            CustomContextMenuItem::Item {
                label: "Inspect directory".to_owned(),
                exe: executable.clone(),
                icon: None,
                args: Vec::new(),
                only: vec!["*directory".to_owned()],
            },
            CustomContextMenuItem::Item {
                label: "Png or directory".to_owned(),
                exe: executable.clone(),
                icon: None,
                args: Vec::new(),
                only: vec![".png".to_owned(), "*directory".to_owned()],
            },
            CustomContextMenuItem::Item {
                label: "Text-only directory".to_owned(),
                exe: executable.clone(),
                icon: None,
                args: Vec::new(),
                only: vec!["txt".to_owned()],
            },
            CustomContextMenuItem::Item {
                label: "Folder-only directory".to_owned(),
                exe: executable,
                icon: None,
                args: Vec::new(),
                only: vec!["*folder".to_owned()],
            },
        ];

        let items = folder_context_menu_items_with_custom(
            &directory,
            false,
            crate::settings::DEFAULT_DATE_FORMAT,
            &configured,
        );

        assert!(matches!(
            items[2],
            ContextMenuItem::Action {
                command: ContextMenuCommand::CopyPath { .. },
                ..
            }
        ));
        assert!(matches!(items[3], ContextMenuItem::Separator));
        assert!(matches!(
            &items[4],
            ContextMenuItem::Action {
                command: ContextMenuCommand::RunCustom { targets, .. },
                ..
            } if targets == &[directory]
        ));
        assert!(menu_has_label(&items, "Png or directory"));
        assert!(matches!(items[6], ContextMenuItem::Separator));
        assert!(!menu_has_label(&items, "Implicit entry-only"));
        assert!(!menu_has_label(&items, "Text-only directory"));
        assert!(!menu_has_label(&items, "Folder-only directory"));
        assert!(!items.iter().any(
            |item| matches!(item, ContextMenuItem::Submenu { label, .. } if label == "Empty")
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn configured_item_can_use_scoop_shim_target_for_icon_only() {
        let dir = unique_temp_dir("scoop-shim-icon");
        let shim_dir = dir.join("shims");
        let app_dir = dir.join("apps").join("zed").join("current").join("bin");
        let executable = shim_dir.join("zed.exe");
        let icon_target = app_dir.join("zed.exe");
        fs::create_dir_all(&shim_dir).unwrap();
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(&executable, "").unwrap();
        fs::write(&icon_target, "").unwrap();
        fs::write(
            shim_dir.join("zed.shim"),
            format!("path = \"{}\"\n", icon_target.display()),
        )
        .unwrap();
        let targets = vec![PathBuf::from("a.txt")];
        let configured = vec![CustomContextMenuItem::Item {
            label: "Open in Zed".to_owned(),
            exe: executable.clone(),
            icon: None,
            args: Vec::new(),
            only: Vec::new(),
        }];

        let items = entry_context_menu_items_with_custom(
            None,
            1,
            1,
            0,
            false,
            false,
            &configured,
            &targets,
            &[],
        );

        assert!(matches!(
            &items[2],
            ContextMenuItem::Action {
                icon: Some(ContextMenuIcon::NativePath(path)),
                command: ContextMenuCommand::RunCustom { executable: command, .. },
                ..
            } if path == &icon_target && command == &executable
        ));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn custom_command_result_reports_and_clears_spawn_errors() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        let executable = PathBuf::from("missing-tool");

        view.handle_custom_command_result(
            &executable,
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing")),
        );
        assert_eq!(
            view.operation_notice
                .as_ref()
                .map(|notice| notice.text.as_str()),
            Some("Could not run missing-tool: missing")
        );

        view.handle_custom_command_result(&executable, Ok(()));
        assert_eq!(view.operation_notice, None);
    }

    #[test]
    fn mounted_volume_eject_commands_use_platform_tools() {
        let path = PathBuf::from("/Volumes/Backup Disk");

        assert_eq!(
            macos_mounted_volume_eject_command(&path),
            PlatformCommand {
                executable: OsString::from("/usr/sbin/diskutil"),
                args: vec![OsString::from("eject"), path.as_os_str().to_os_string()],
            }
        );
        assert_eq!(
            linux_mounted_volume_eject_command(&path),
            PlatformCommand {
                executable: OsString::from("gio"),
                args: vec![
                    OsString::from("mount"),
                    OsString::from("--eject"),
                    path.as_os_str().to_os_string(),
                ],
            }
        );

        let windows_path = PathBuf::from("E:\\");
        let windows_command = windows_mounted_volume_eject_command(&windows_path);
        assert_eq!(windows_command.executable, OsString::from("powershell.exe"));
        assert!(windows_command.args.iter().any(|arg| arg == "-NoProfile"));
        assert_eq!(windows_command.args.len(), 6);
        let script = windows_command.args.last().unwrap().to_string_lossy();
        assert!(script.contains("[System.IO.Path]::GetPathRoot('E:\\')"));
        assert!(script.contains("$item.InvokeVerb('Eject')"));
    }

    #[test]
    fn mount_image_commands_use_platform_tools() {
        let image = PathBuf::from("/images/Installer.iso");

        assert_eq!(
            macos_mount_image_command(&image),
            PlatformCommand {
                executable: OsString::from("/usr/bin/hdiutil"),
                args: vec![OsString::from("attach"), image.as_os_str().to_os_string()],
            }
        );
        assert_eq!(
            linux_loop_setup_command(&image),
            PlatformCommand {
                executable: OsString::from("udisksctl"),
                args: vec![
                    OsString::from("loop-setup"),
                    OsString::from("--read-only"),
                    OsString::from("--file"),
                    image.as_os_str().to_os_string(),
                ],
            }
        );

        let loop_device = PathBuf::from("/dev/loop7");
        assert_eq!(
            linux_mount_loop_device_command(&loop_device),
            PlatformCommand {
                executable: OsString::from("udisksctl"),
                args: vec![
                    OsString::from("mount"),
                    OsString::from("--block-device"),
                    loop_device.as_os_str().to_os_string(),
                ],
            }
        );
        assert_eq!(
            linux_loop_delete_command(&loop_device),
            PlatformCommand {
                executable: OsString::from("udisksctl"),
                args: vec![
                    OsString::from("loop-delete"),
                    OsString::from("--block-device"),
                    loop_device.as_os_str().to_os_string(),
                ],
            }
        );
        assert_eq!(
            linux_loop_device_from_loop_setup_stdout("Mapped file image.iso as /dev/loop7.\n"),
            Some(loop_device)
        );

        let windows_command = windows_mount_disk_image_command(&PathBuf::from("/images/Win's.iso"));
        assert_eq!(windows_command.executable, OsString::from("powershell.exe"));
        assert!(windows_command.args.iter().any(|arg| arg == "-NoProfile"));
        let script = windows_command.args.last().unwrap().to_string_lossy();
        assert!(script.contains("Mount-DiskImage -ImagePath '/images/Win''s.iso'"));

        let shell_image = PathBuf::from("/images/Disk.img");
        assert!(windows_mount_image_uses_shell(&shell_image));
        let shell_command = windows_shell_mount_image_command(&shell_image);
        let script = shell_command.args.last().unwrap().to_string_lossy();
        assert!(script.contains("Namespace('/images')"));
        assert!(script.contains("ParseName('Disk.img')"));
        assert!(script.contains("$item.InvokeVerb('Mount')"));
    }

    #[test]
    fn powershell_single_quoted_literal_escapes_apostrophes() {
        assert_eq!(powershell_single_quoted_literal(r"E:\"), r"'E:\'");
        assert_eq!(
            powershell_single_quoted_literal(r"C:\Bob's Drive"),
            r"'C:\Bob''s Drive'"
        );
    }

    #[test]
    fn mounted_volume_error_name_prefers_last_path_component() {
        assert_eq!(
            mounted_volume_error_name(Path::new("/Volumes/Backup Disk")),
            "Backup Disk"
        );
        assert_eq!(mounted_volume_error_name(Path::new("/")), "/");
    }

    #[test]
    fn custom_command_launches_once_with_all_targets_in_order() {
        let executable = configured_executable_path();
        let args = vec!["--inspect".to_owned()];
        let targets = vec![PathBuf::from("a.txt"), PathBuf::from("folder")];
        let mut calls = Vec::new();

        run_custom_command_with(
            &executable,
            &args,
            &targets,
            |actual_executable, actual_arguments| {
                calls.push((actual_executable.to_path_buf(), actual_arguments.to_vec()));
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(
            calls,
            vec![(
                executable,
                vec![
                    OsString::from("--inspect"),
                    OsString::from("a.txt"),
                    OsString::from("folder")
                ]
            )]
        );
    }

    #[test]
    fn custom_command_arguments_expand_exact_path_placeholder_to_first_target() {
        let args = vec![
            "--open".to_owned(),
            "{path}".to_owned(),
            "--literal={path}".to_owned(),
        ];
        let targets = vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")];

        assert_eq!(
            custom_command_arguments(&args, &targets),
            vec![
                OsString::from("--open"),
                OsString::from("a.txt"),
                OsString::from("--literal={path}")
            ]
        );
    }

    #[test]
    fn custom_command_arguments_expand_paths_placeholder_to_all_targets_without_append() {
        let args = vec!["--open".to_owned(), "{paths}".to_owned()];
        let targets = vec![PathBuf::from("a.txt"), PathBuf::from("folder")];

        assert_eq!(
            custom_command_arguments(&args, &targets),
            vec![
                OsString::from("--open"),
                OsString::from("a.txt"),
                OsString::from("folder")
            ]
        );
    }

    #[test]
    fn detail_rows_have_no_icons_and_blank_unsupported_dates() {
        let items = folder_context_menu_items_from_times(true, None, None);

        assert_eq!(
            items[4],
            ContextMenuItem::Detail {
                label: "Created",
                value: String::new(),
                icon_slot: ContextMenuIconSlot::Collapse,
            }
        );
        assert_eq!(
            items[5],
            ContextMenuItem::Detail {
                label: "Modified",
                value: String::new(),
                icon_slot: ContextMenuIconSlot::Collapse,
            }
        );
    }

    #[test]
    fn detail_rows_format_supported_dates() {
        let created = Local.with_ymd_and_hms(2026, 6, 1, 9, 15, 0).unwrap();
        let modified = Local.with_ymd_and_hms(2026, 6, 2, 10, 30, 0).unwrap();
        let items =
            folder_context_menu_items_from_times(true, Some(created.into()), Some(modified.into()));

        assert_eq!(
            items[4],
            ContextMenuItem::Detail {
                label: "Created",
                value: "2026/06/01 09:15".to_owned(),
                icon_slot: ContextMenuIconSlot::Collapse,
            }
        );
        assert_eq!(
            items[5],
            ContextMenuItem::Detail {
                label: "Modified",
                value: "2026/06/02 10:30".to_owned(),
                icon_slot: ContextMenuIconSlot::Collapse,
            }
        );
    }

    #[test]
    fn detail_rows_use_configured_date_format() {
        let timestamp = Local.with_ymd_and_hms(2026, 2, 5, 9, 15, 0).unwrap();
        let items = folder_context_menu_items_from_times_with_format(
            Path::new("folder"),
            true,
            Some(timestamp.into()),
            Some(timestamp.into()),
            "%d %B %Y",
        );

        for item in &items[4..=5] {
            assert!(matches!(
                item,
                ContextMenuItem::Detail { value, .. } if value == "05 February 2026"
            ));
        }
    }
}
