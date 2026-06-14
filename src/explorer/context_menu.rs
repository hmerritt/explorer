use std::{
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
use crate::settings::CustomContextMenuItem;

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
            .map(|settings| settings.value.contextmenu.directory.clone())
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
            .map(|settings| settings.value.contextmenu.file_folder.clone())
            .unwrap_or_default();
        let targets = self.selected_paths();
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
            .map(|settings| settings.value.contextmenu.file_folder.clone())
            .unwrap_or_default();
        let targets = self.selected_paths();
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
                targets,
            } => {
                self.handle_custom_command_result(
                    &executable,
                    run_custom_command(&executable, &targets),
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

fn run_custom_command(executable: &Path, targets: &[PathBuf]) -> std::io::Result<()> {
    run_custom_command_with(executable, targets, |executable, targets| {
        Command::new(executable).args(targets).spawn().map(|_| ())
    })
}

fn run_custom_command_with(
    executable: &Path,
    targets: &[PathBuf],
    spawn: impl FnOnce(&Path, &[PathBuf]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    spawn(executable, targets)
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
    insert_custom_items_after_first_separator(&mut items, custom_items, targets);

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
    insert_custom_items_after_first_separator(&mut items, custom_items, &[path.to_path_buf()]);
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
) {
    let custom = configured
        .iter()
        .enumerate()
        .filter_map(|(index, item)| configured_context_menu_item(item, targets, &index.to_string()))
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
    id_suffix: &str,
) -> Option<ContextMenuItem> {
    match item {
        CustomContextMenuItem::Item { label, .. } => {
            let executable = item.resolved_executable()?;
            Some(ContextMenuItem::Action {
                id: format!("context-menu-custom-{id_suffix}"),
                icon: Some(ContextMenuIcon::NativePath(executable.clone())),
                label: label.clone(),
                command: ContextMenuCommand::RunCustom {
                    executable,
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
                    configured_context_menu_item(item, targets, &format!("{id_suffix}-{index}"))
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

    fn configured_executable_path() -> PathBuf {
        if cfg!(target_os = "windows") {
            PathBuf::from(r"C:\Tools\inspect.exe")
        } else {
            PathBuf::from("/usr/bin/inspect")
        }
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
                executable: executable.clone(),
            },
            CustomContextMenuItem::Submenu {
                label: "Tools".to_owned(),
                items: vec![CustomContextMenuItem::Item {
                    label: "Deep inspect".to_owned(),
                    executable: executable.clone(),
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
        );

        assert!(matches!(items[2], ContextMenuItem::Separator));
        assert!(matches!(
            &items[3],
            ContextMenuItem::Action {
                label,
                icon: Some(ContextMenuIcon::NativePath(path)),
                command: ContextMenuCommand::RunCustom {
                    executable: command,
                    targets: command_targets,
                },
                ..
            } if label == "Inspect"
                && path == &executable
                && command == &executable
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
    fn configured_directory_items_receive_current_directory_and_empty_submenus_are_omitted() {
        let directory = PathBuf::from("current");
        let executable = configured_executable_path();
        let configured = vec![
            CustomContextMenuItem::Submenu {
                label: "Empty".to_owned(),
                items: Vec::new(),
            },
            CustomContextMenuItem::Item {
                label: "Inspect directory".to_owned(),
                executable: executable.clone(),
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
        assert!(matches!(items[4], ContextMenuItem::Separator));
        assert!(!items.iter().any(
            |item| matches!(item, ContextMenuItem::Submenu { label, .. } if label == "Empty")
        ));
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
        let targets = vec![PathBuf::from("a.txt"), PathBuf::from("folder")];
        let mut calls = Vec::new();

        run_custom_command_with(
            &executable,
            &targets,
            |actual_executable, actual_targets| {
                calls.push((actual_executable.to_path_buf(), actual_targets.to_vec()));
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(calls, vec![(executable, targets)]);
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
