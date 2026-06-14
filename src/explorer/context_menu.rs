use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

use gpui::{Context, Pixels, Point, Window};

use crate::explorer::{
    DirectoryKind,
    entry::FileEntry,
    formatting::format_timestamp,
    navigation::{HistoryMode, directory_new_tab_target},
    view::ExplorerView,
};
use crate::settings::{
    ContextMenuOnlyFilter, CustomContextMenuItem, resolve_context_menu_only_filter,
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
    Paste,
    Delete,
    Rename,
    New,
    File,
    NativeFile,
    Folder,
    FolderKind(Option<DirectoryKind>),
    NativePath(PathBuf),
    NewTab,
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
    OpenSelectedDirectoriesInNewTabs,
    CutSelected,
    CopySelected,
    Paste,
    DeleteSelected,
    RenameSelected,
    NewFile,
    NewFolder,
    RunCustom {
        executable: PathBuf,
        args: Vec<String>,
        targets: Vec<PathBuf>,
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
            sidebar_context_menu_items(path, configured_index, open_icon_kind),
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
                self.open_selected_files_with_default_app();
            }
            ContextMenuCommand::OpenSelectedDirectoriesInNewTabs => {
                for path in self.selected_directory_new_tab_targets() {
                    cx.emit(crate::explorer::view::ExplorerViewEvent::OpenDirectoryInNewTab(path));
                }
            }
            ContextMenuCommand::CutSelected => self.cut_selected_to_clipboard(cx),
            ContextMenuCommand::CopySelected => self.copy_selected_to_clipboard(cx),
            ContextMenuCommand::Paste => self.paste_clipboard_files(cx),
            ContextMenuCommand::DeleteSelected => self.trash_selected_paths(cx),
            ContextMenuCommand::RenameSelected => {
                self.start_rename_selected(window, cx);
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
            ContextMenuCommand::UnpinSidebar { configured_index } => {
                crate::settings::unpin_sidebar_item(configured_index, cx);
            }
        }
    }

    fn handle_custom_command_result(&mut self, executable: &Path, result: std::io::Result<()>) {
        match result {
            Ok(()) => self.open_error = None,
            Err(error) => {
                self.open_error = Some(format!(
                    "Could not run {}: {error}",
                    executable
                        .file_name()
                        .unwrap_or(executable.as_os_str())
                        .to_string_lossy()
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

    fn open_selected_files_with_default_app(&mut self) {
        for path in self.selected_file_open_targets() {
            self.open_file_with_default_app(&path);
        }
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
) -> Vec<ContextMenuItem> {
    let mut items = vec![
        ContextMenuItem::Action {
            id: "context-menu-sidebar-open".to_owned(),
            icon: Some(ContextMenuIcon::FolderKind(open_icon_kind)),
            label: "Open".to_owned(),
            command: ContextMenuCommand::OpenDirectory { path: path.clone() },
            enabled: true,
        },
        ContextMenuItem::Action {
            id: "context-menu-sidebar-open-new-tab".to_owned(),
            icon: Some(ContextMenuIcon::NewTab),
            label: "Open in new tab".to_owned(),
            command: ContextMenuCommand::OpenDirectoryInNewTab { path },
            enabled: true,
        },
    ];
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

#[cfg(test)]
pub(super) fn entry_context_menu_items(
    single_directory_open_target: Option<PathBuf>,
    selected_count: usize,
    selected_file_count: usize,
    selected_directory_count: usize,
    can_rename: bool,
    use_native_file_icon: bool,
) -> Vec<ContextMenuItem> {
    entry_context_menu_items_with_custom(
        single_directory_open_target,
        selected_count,
        selected_file_count,
        selected_directory_count,
        can_rename,
        use_native_file_icon,
        &[],
        &[],
        &[],
    )
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
    }

    if selected_count > 1 && selected_file_count > 0 {
        items.push(ContextMenuItem::Action {
            id: "context-menu-entry-open".to_owned(),
            icon: Some(ContextMenuIcon::File),
            label: format!("Open files ({selected_file_count})"),
            command: ContextMenuCommand::OpenSelectedFiles,
            enabled: true,
        });
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
    items
}

fn entry_is_file_open_target(entry: &FileEntry) -> bool {
    !entry.is_directory_like() || entry.is_app_bundle()
}

fn folder_context_menu_items_with_custom(
    path: &Path,
    can_paste: bool,
    date_format: &str,
    custom_items: &[CustomContextMenuItem],
) -> Vec<ContextMenuItem> {
    let (created, modified) = fs::metadata(path)
        .map(|metadata| (metadata.created().ok(), metadata.modified().ok()))
        .unwrap_or((None, None));

    let mut items =
        folder_context_menu_items_from_times_with_format(can_paste, created, modified, date_format);
    insert_custom_items_after_first_separator(
        &mut items,
        custom_items,
        &[path.to_path_buf()],
        CustomContextMenuTarget::Directory,
    );
    items
}

#[cfg(test)]
pub(super) fn folder_context_menu_items_from_times(
    can_paste: bool,
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
) -> Vec<ContextMenuItem> {
    folder_context_menu_items_from_times_with_format(
        can_paste,
        created,
        modified,
        crate::settings::DEFAULT_DATE_FORMAT,
    )
}

fn folder_context_menu_items_from_times_with_format(
    can_paste: bool,
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
    date_format: &str,
) -> Vec<ContextMenuItem> {
    vec![
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
    ]
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
            let icon_path = item.resolved_icon_path(&executable);
            Some(ContextMenuItem::Action {
                id: format!("context-menu-custom-{id_suffix}"),
                icon: Some(ContextMenuIcon::NativePath(icon_path)),
                label: label.clone(),
                command: ContextMenuCommand::RunCustom {
                    executable,
                    args: args.clone(),
                    targets: targets.to_vec(),
                },
                enabled: true,
            })
        }
        CustomContextMenuItem::Submenu { label, items } => {
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
            (!children.is_empty()).then(|| ContextMenuItem::Submenu {
                id: format!("context-menu-custom-{id_suffix}"),
                icon: None,
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

    fn menu_has_label(items: &[ContextMenuItem], expected: &str) -> bool {
        items.iter().any(|item| match item {
            ContextMenuItem::Action { label, .. } | ContextMenuItem::Submenu { label, .. } => {
                label == expected
            }
            ContextMenuItem::Separator | ContextMenuItem::Detail { .. } => false,
        })
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

        assert_eq!(items.len(), 5);
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
        assert!(matches!(items[2], ContextMenuItem::Separator));
    }

    #[test]
    fn configured_sidebar_menu_contains_expected_items_icons_and_commands() {
        let path = PathBuf::from("/tmp/custom");
        let items =
            sidebar_context_menu_items(path.clone(), Some(2), Some(DirectoryKind::Downloads));

        assert_eq!(items.len(), 4);
        assert_eq!(
            items[0],
            ContextMenuItem::Action {
                id: "context-menu-sidebar-open".to_owned(),
                icon: Some(ContextMenuIcon::FolderKind(Some(DirectoryKind::Downloads))),
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
        let items = sidebar_context_menu_items(path.clone(), None, Some(DirectoryKind::Drive));

        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0],
            ContextMenuItem::Action {
                id: "context-menu-sidebar-open".to_owned(),
                icon: Some(ContextMenuIcon::FolderKind(Some(DirectoryKind::Drive))),
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
            ]
        );

        let enabled_items =
            entry_context_menu_items(Some(PathBuf::from("/tmp/folder")), 1, 0, 1, true, false);
        assert!(matches!(
            enabled_items.last(),
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
            items.last(),
            Some(ContextMenuItem::Action {
                command: ContextMenuCommand::RenameSelected,
                enabled: true,
                ..
            })
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
            ]
        );
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
    fn configured_entry_items_are_inserted_after_first_separator_with_selected_targets() {
        let executable = configured_executable_path();
        let targets = vec![PathBuf::from("a.txt"), PathBuf::from("folder")];
        let configured = vec![
            CustomContextMenuItem::Item {
                label: "Inspect".to_owned(),
                exe: executable.clone(),
                args: vec!["--inspect".to_owned()],
                only: Vec::new(),
            },
            CustomContextMenuItem::Submenu {
                label: "Tools".to_owned(),
                items: vec![CustomContextMenuItem::Item {
                    label: "Deep inspect".to_owned(),
                    exe: executable.clone(),
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
    fn configured_entry_items_skip_only_missing_executables() {
        let executable = configured_executable_path();
        let missing = executable.with_file_name("missing-inspect");
        let targets = vec![PathBuf::from("a.txt")];
        let configured = vec![
            CustomContextMenuItem::Item {
                label: "Missing".to_owned(),
                exe: missing.clone(),
                args: Vec::new(),
                only: Vec::new(),
            },
            CustomContextMenuItem::Item {
                label: "Inspect".to_owned(),
                exe: executable.clone(),
                args: Vec::new(),
                only: Vec::new(),
            },
            CustomContextMenuItem::Submenu {
                label: "Tools".to_owned(),
                items: vec![
                    CustomContextMenuItem::Item {
                        label: "Missing child".to_owned(),
                        exe: missing.clone(),
                        args: Vec::new(),
                        only: Vec::new(),
                    },
                    CustomContextMenuItem::Item {
                        label: "Deep inspect".to_owned(),
                        exe: executable.clone(),
                        args: Vec::new(),
                        only: Vec::new(),
                    },
                ],
            },
            CustomContextMenuItem::Submenu {
                label: "Empty".to_owned(),
                items: vec![CustomContextMenuItem::Item {
                    label: "Only missing".to_owned(),
                    exe: missing,
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
                args: Vec::new(),
                only: vec!["txt".to_owned(), ".MD".to_owned()],
            },
            CustomContextMenuItem::Item {
                label: "Image tool".to_owned(),
                exe: executable.clone(),
                args: Vec::new(),
                only: vec!["png".to_owned()],
            },
            CustomContextMenuItem::Item {
                label: "Any tool".to_owned(),
                exe: executable,
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
            items: vec![CustomContextMenuItem::Item {
                label: "Image tool".to_owned(),
                exe: executable,
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
                items: Vec::new(),
            },
            CustomContextMenuItem::Item {
                label: "Implicit entry-only".to_owned(),
                exe: executable.clone(),
                args: Vec::new(),
                only: Vec::new(),
            },
            CustomContextMenuItem::Item {
                label: "Inspect directory".to_owned(),
                exe: executable.clone(),
                args: Vec::new(),
                only: vec!["*directory".to_owned()],
            },
            CustomContextMenuItem::Item {
                label: "Png or directory".to_owned(),
                exe: executable.clone(),
                args: Vec::new(),
                only: vec![".png".to_owned(), "*directory".to_owned()],
            },
            CustomContextMenuItem::Item {
                label: "Text-only directory".to_owned(),
                exe: executable.clone(),
                args: Vec::new(),
                only: vec!["txt".to_owned()],
            },
            CustomContextMenuItem::Item {
                label: "Folder-only directory".to_owned(),
                exe: executable,
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

        assert!(matches!(items[2], ContextMenuItem::Separator));
        assert!(matches!(
            &items[3],
            ContextMenuItem::Action {
                command: ContextMenuCommand::RunCustom { targets, .. },
                ..
            } if targets == &[directory]
        ));
        assert!(menu_has_label(&items, "Png or directory"));
        assert!(matches!(items[5], ContextMenuItem::Separator));
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
            view.open_error.as_deref(),
            Some("Could not run missing-tool: missing")
        );

        view.handle_custom_command_result(&executable, Ok(()));
        assert_eq!(view.open_error, None);
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
            items[3],
            ContextMenuItem::Detail {
                label: "Created",
                value: String::new(),
                icon_slot: ContextMenuIconSlot::Collapse,
            }
        );
        assert_eq!(
            items[4],
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
            items[3],
            ContextMenuItem::Detail {
                label: "Created",
                value: "2026/06/01 09:15".to_owned(),
                icon_slot: ContextMenuIconSlot::Collapse,
            }
        );
        assert_eq!(
            items[4],
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
            true,
            Some(timestamp.into()),
            Some(timestamp.into()),
            "%d %B %Y",
        );

        for item in &items[3..=4] {
            assert!(matches!(
                item,
                ContextMenuItem::Detail { value, .. } if value == "05 February 2026"
            ));
        }
    }
}
