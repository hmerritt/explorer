use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

use gpui::{
    AnyWindowHandle, Context, EventEmitter, FocusHandle, Pixels, Point, Subscription, Task,
    UniformListScrollHandle, point, px,
};

use crate::explorer::sidebar::{SidebarSections, sidebar_sections};
use crate::explorer::{
    address_bar::AddressBarState,
    app_icons::AppIconCache,
    context_menu::ContextMenuState,
    drag_drop::DropIndicator,
    entry::{FileEntry, ShellShortcutTargetKind, resolve_shell_shortcut_target_kind},
    filesystem::{FileConflictBatch, FileOperationProgress, load_entries},
    mouse_selection::MouseSelectionDrag,
    rename::{PendingClickRename, RenameState},
    scrollbar::ScrollbarDrag,
    search::SearchState,
    selection::SelectionState,
    watcher::DirectoryWatcher,
};
use crate::settings::{ExplorerSettings, SidebarLocation};

pub struct ExplorerView {
    pub(super) path: PathBuf,
    pub(super) entries: Vec<FileEntry>,
    pub(super) all_entries: Vec<FileEntry>,
    pub(super) selection: SelectionState,
    pub(super) read_error: Option<String>,
    pub(super) open_error: Option<String>,
    pub(super) back_stack: Vec<PathBuf>,
    pub(super) forward_stack: Vec<PathBuf>,
    pub(super) scroll_handle: UniformListScrollHandle,
    pub(super) focus_handle: Option<FocusHandle>,
    pub(super) scrollbar_hovered: bool,
    pub(super) scrollbar_drag: Option<ScrollbarDrag>,
    pub(super) mouse_selection_drag: Option<MouseSelectionDrag>,
    pub(super) suppress_next_click: bool,
    pub(super) cut_paths: BTreeSet<PathBuf>,
    pub(super) active_drop_indicator: Option<DropIndicator>,
    pub(super) dragging_sidebar_item: Option<usize>,
    pub(super) app_icon_cache: AppIconCache,
    pub(super) pending_permanent_delete: Option<PendingPermanentDelete>,
    pub(super) pending_trash: Option<PendingTrash>,
    pub(super) pending_file_conflict: Option<FileConflictBatch>,
    pub(super) active_file_operation: Option<FileOperationState>,
    pub(super) active_dialog_window: Option<AnyWindowHandle>,
    pub(super) active_rename: Option<RenameState>,
    pub(super) rename_focus_out: Option<Subscription>,
    pub(super) active_address_bar: Option<AddressBarState>,
    pub(super) search: SearchState,
    pub(super) pending_click_rename: Option<PendingClickRename>,
    pub(super) next_pending_click_rename_id: u64,
    pub(super) show_hidden_files: bool,
    pub(super) show_file_name_extensions: bool,
    pub(super) open_utility_menu: Option<UtilityMenu>,
    pub(super) context_menu: Option<ContextMenuState>,
    pub(super) view_origin: Point<Pixels>,
    pub(super) directory_watcher: Option<DirectoryWatcher>,
    pub(super) sidebar_items: Vec<SidebarLocation>,
    pub(super) sidebar_sections: SidebarSections,
    pub(super) shell_shortcut_resolution_generation: u64,
    pub(super) shell_shortcut_resolution_task: Option<Task<()>>,
}

pub(super) struct FileOperationState {
    pub(super) progress: FileOperationProgress,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) task: Option<Task<()>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ShellShortcutResolution {
    pub(super) path: PathBuf,
    pub(super) target_kind: ShellShortcutTargetKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ExplorerViewEvent {
    FilesystemChanged,
    OpenDirectoryInNewTab(PathBuf),
}

impl EventEmitter<ExplorerViewEvent> for ExplorerView {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PendingPermanentDelete {
    pub(super) paths: Vec<PathBuf>,
    pub(super) fallback_index: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PendingTrash {
    pub(super) paths: Vec<PathBuf>,
    pub(super) fallback_index: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExplorerContentBranch {
    Error,
    Empty,
    SearchWorking,
    NoSearchMatches,
    List,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum UtilityMenu {
    New,
    View,
}

impl ExplorerView {
    #[cfg(test)]
    pub fn new(initial_path: PathBuf) -> Self {
        Self::new_inner(initial_path, None)
    }

    pub fn new_watched_with_focus_handle(
        initial_path: PathBuf,
        focus_handle: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = cx.global::<crate::settings::SettingsState>().value.clone();
        let mut view = Self::new_inner_with_settings(initial_path, Some(focus_handle), &settings);
        view.restart_directory_watcher(cx);
        view.schedule_pending_shell_shortcut_resolution(cx);
        view
    }

    #[cfg(test)]
    pub(super) fn new_with_focus_handle_for_test(
        initial_path: PathBuf,
        focus_handle: FocusHandle,
    ) -> Self {
        Self::new_inner(initial_path, Some(focus_handle))
    }

    #[cfg(test)]
    fn new_inner(initial_path: PathBuf, focus_handle: Option<FocusHandle>) -> Self {
        Self::new_inner_with_settings(initial_path, focus_handle, &ExplorerSettings::default())
    }

    fn new_inner_with_settings(
        initial_path: PathBuf,
        focus_handle: Option<FocusHandle>,
        settings: &ExplorerSettings,
    ) -> Self {
        let mut view = Self {
            path: initial_path,
            entries: Vec::new(),
            all_entries: Vec::new(),
            selection: SelectionState::default(),
            read_error: None,
            open_error: None,
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            scroll_handle: UniformListScrollHandle::new(),
            focus_handle,
            scrollbar_hovered: false,
            scrollbar_drag: None,
            mouse_selection_drag: None,
            suppress_next_click: false,
            cut_paths: BTreeSet::new(),
            active_drop_indicator: None,
            dragging_sidebar_item: None,
            app_icon_cache: AppIconCache::default(),
            pending_permanent_delete: None,
            pending_trash: None,
            pending_file_conflict: None,
            active_file_operation: None,
            active_dialog_window: None,
            active_rename: None,
            rename_focus_out: None,
            active_address_bar: None,
            search: SearchState::default(),
            pending_click_rename: None,
            next_pending_click_rename_id: 0,
            show_hidden_files: settings.show_hidden_files,
            show_file_name_extensions: settings.show_file_name_extensions,
            open_utility_menu: None,
            context_menu: None,
            view_origin: point(px(0.0), px(0.0)),
            directory_watcher: None,
            sidebar_items: settings.sidebar_items.clone(),
            sidebar_sections: SidebarSections::default(),
            shell_shortcut_resolution_generation: 0,
            shell_shortcut_resolution_task: None,
        };
        view.reload();
        view
    }

    pub(super) fn apply_settings(&mut self, settings: &ExplorerSettings, cx: &mut Context<Self>) {
        let hidden_changed = self.show_hidden_files != settings.show_hidden_files;
        self.show_hidden_files = settings.show_hidden_files;
        self.show_file_name_extensions = settings.show_file_name_extensions;

        self.sidebar_items = settings.sidebar_items.clone();

        if hidden_changed {
            self.invalidate_recursive_search_cache();
            self.reload();
            self.schedule_pending_shell_shortcut_resolution(cx);
            self.refresh_search_after_external_change(cx);
        } else {
            self.sidebar_sections = sidebar_sections(&self.sidebar_items);
        }
        cx.notify();
    }

    pub fn reload(&mut self) {
        let _timing_batch = crate::debug_options::NavTimingBatch::start();
        self.reload_inner(ReloadMode {
            preserve_selection: true,
            rebuild_sidebar: true,
        });
    }

    pub(super) fn reload_for_navigation(&mut self) {
        self.reload_inner(ReloadMode {
            preserve_selection: false,
            rebuild_sidebar: false,
        });
    }

    fn reload_inner(&mut self, mode: ReloadMode) {
        let total_started = Instant::now();
        self.context_menu = None;
        self.open_error = None;
        let selected_paths_started = Instant::now();
        let selected_paths = if mode.preserve_selection {
            self.selected_paths()
        } else {
            Vec::new()
        };
        crate::debug_options::log_nav_timing(
            selected_paths_started.elapsed(),
            format_args!(
                "reload.selected_paths path={:?} selected={}",
                self.path,
                selected_paths.len()
            ),
        );

        if mode.rebuild_sidebar {
            let sidebar_started = Instant::now();
            self.sidebar_sections = sidebar_sections(&self.sidebar_items);
            crate::debug_options::log_nav_timing(
                sidebar_started.elapsed(),
                format_args!("reload.sidebar_sections path={:?}", self.path),
            );
        }

        let load_started = Instant::now();
        match load_entries(&self.path, self.show_hidden_files) {
            Ok(entries) => {
                crate::debug_options::log_nav_timing(
                    load_started.elapsed(),
                    format_args!(
                        "reload.load_entries path={:?} ok=true entries={}",
                        self.path,
                        entries.len()
                    ),
                );
                self.read_error = None;
                let filter_started = Instant::now();
                if self.search_is_active() {
                    self.all_entries = entries;
                    self.apply_search_filter_preserving_selection(&selected_paths);
                } else {
                    self.entries = entries.clone();
                    self.all_entries = entries;
                    if mode.preserve_selection {
                        self.restore_selection_from_paths(&selected_paths);
                    } else {
                        self.selection = SelectionState::default();
                    }
                }
                crate::debug_options::log_nav_timing(
                    filter_started.elapsed(),
                    format_args!(
                        "reload.search_filter path={:?} query={:?} visible={} selected={}",
                        self.path,
                        self.search_query(),
                        self.entries.len(),
                        self.selection.selected_indices.len()
                    ),
                );
            }
            Err(error) => {
                crate::debug_options::log_nav_timing(
                    load_started.elapsed(),
                    format_args!(
                        "reload.load_entries path={:?} ok=false error={error}",
                        self.path
                    ),
                );
                self.all_entries.clear();
                self.entries.clear();
                self.clear_selection();
                self.read_error = Some(error.to_string());
            }
        }
        crate::debug_options::log_nav_timing(
            total_started.elapsed(),
            format_args!(
                "reload.total path={:?} entries={} all_entries={} read_error={}",
                self.path,
                self.entries.len(),
                self.all_entries.len(),
                self.read_error.is_some()
            ),
        );
    }

    pub(super) fn reload_with_shell_shortcut_resolution(&mut self, cx: &mut Context<Self>) {
        self.reload();
        self.schedule_pending_shell_shortcut_resolution(cx);
    }

    pub(super) fn schedule_pending_shell_shortcut_resolution(&mut self, cx: &mut Context<Self>) {
        self.shell_shortcut_resolution_generation =
            self.shell_shortcut_resolution_generation.wrapping_add(1);
        let generation = self.shell_shortcut_resolution_generation;
        let path = self.path.clone();
        let pending_targets = self.pending_shell_shortcut_targets();

        if pending_targets.is_empty() {
            self.shell_shortcut_resolution_task = None;
            return;
        }

        let task = cx.spawn(async move |this, cx| {
            let output_task = cx.background_executor().spawn(async move {
                pending_targets
                    .into_iter()
                    .map(|(path, target)| ShellShortcutResolution {
                        path,
                        target_kind: resolve_shell_shortcut_target_kind(&target),
                    })
                    .collect::<Vec<_>>()
            });
            let resolutions = output_task.await;

            let _ = this.update(cx, |explorer, cx| {
                if explorer.apply_shell_shortcut_resolutions(&path, generation, resolutions) {
                    cx.notify();
                }
            });
        });
        self.shell_shortcut_resolution_task = Some(task);
    }

    fn pending_shell_shortcut_targets(&self) -> Vec<(PathBuf, PathBuf)> {
        self.all_entries
            .iter()
            .filter_map(FileEntry::pending_shell_shortcut_target)
            .collect()
    }

    pub(super) fn apply_shell_shortcut_resolutions(
        &mut self,
        path: &Path,
        generation: u64,
        resolutions: Vec<ShellShortcutResolution>,
    ) -> bool {
        if self.path != path || self.shell_shortcut_resolution_generation != generation {
            return false;
        }

        let selected_paths = self.selected_paths();
        let mut changed = false;
        for resolution in resolutions {
            changed |=
                apply_shell_shortcut_resolution_to_entries(&mut self.all_entries, &resolution);
            changed |= apply_shell_shortcut_resolution_to_entries(&mut self.entries, &resolution);
        }

        if changed {
            self.restore_selection_from_paths(&selected_paths);
        }

        changed
    }

    pub(super) fn emit_filesystem_changed(&self, cx: &mut Context<Self>) {
        cx.emit(ExplorerViewEvent::FilesystemChanged);
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn restart_directory_watcher(&mut self, cx: &mut Context<Self>) -> bool {
        let started = Instant::now();
        self.directory_watcher = DirectoryWatcher::start(self.path.clone(), cx);
        let ok = self.directory_watcher.is_some();
        crate::debug_options::log_nav_timing(
            started.elapsed(),
            format_args!("watcher.restart path={:?} ok={ok}", self.path),
        );
        ok
    }

    pub(super) fn tab_label(&self) -> String {
        tab_label_for_path(&self.path)
    }

    pub(super) fn has_active_file_operation(&self) -> bool {
        self.active_file_operation.is_some()
    }

    pub(super) fn active_drop_indicator(&self) -> Option<DropIndicator> {
        self.active_drop_indicator.clone()
    }

    pub(super) fn prepare_for_tab_close(&mut self, cx: &mut Context<Self>) {
        self.cancel_active_rename();
        self.cancel_address_bar_edit();
        self.finish_search_edit();
        self.close_context_menu();
        self.cancel_mouse_selection_drag();
        self.clear_drop_indicator();
        self.pending_permanent_delete = None;
        self.pending_trash = None;
        self.pending_file_conflict = None;

        if self.active_file_operation.is_none()
            && let Some(handle) = self.active_dialog_window.take()
        {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }

        self.directory_watcher = None;
    }
}

#[derive(Clone, Copy)]
struct ReloadMode {
    preserve_selection: bool,
    rebuild_sidebar: bool,
}

pub(super) fn tab_label_for_path(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let display = path.display().to_string();
            if display.is_empty() {
                ".".to_owned()
            } else {
                display
            }
        })
}

impl ExplorerView {
    pub(super) fn should_show_empty_folder_message(&self) -> bool {
        self.all_entries.is_empty() && self.read_error.is_none()
    }

    pub(super) fn content_branch(&self) -> ExplorerContentBranch {
        if self.read_error.is_some() {
            ExplorerContentBranch::Error
        } else if self.recursive_search_is_working() {
            ExplorerContentBranch::SearchWorking
        } else if self.should_show_empty_folder_message() {
            ExplorerContentBranch::Empty
        } else if self.entries.is_empty() && self.search_is_active() {
            ExplorerContentBranch::NoSearchMatches
        } else {
            ExplorerContentBranch::List
        }
    }
}

fn apply_shell_shortcut_resolution_to_entries(
    entries: &mut [FileEntry],
    resolution: &ShellShortcutResolution,
) -> bool {
    let mut changed = false;
    for entry in entries
        .iter_mut()
        .filter(|entry| entry.path == resolution.path)
    {
        entry.resolve_shell_shortcut_target_kind(resolution.target_kind);
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::explorer::entry::{DirectoryLinkKind, EntryKind, FileEntry};
    use std::path::PathBuf;

    fn test_pending_shell_shortcut(path: &str, target: &str) -> FileEntry {
        FileEntry {
            path: PathBuf::from(path),
            name: path.to_owned(),
            kind: EntryKind::DirectoryLink(DirectoryLinkKind::ShellShortcut {
                target: PathBuf::from(target),
                target_kind: ShellShortcutTargetKind::Pending,
            }),
            modified: None,
            size: Some(1),
        }
    }

    #[test]
    fn empty_directory_without_error_shows_empty_folder_message() {
        let mut view = ExplorerView::new(PathBuf::from("empty"));
        view.entries.clear();
        view.read_error = None;

        assert!(view.should_show_empty_folder_message());
    }

    #[test]
    fn view_options_use_settings_defaults() {
        let view = ExplorerView::new(PathBuf::from("defaults"));

        assert!(!view.show_hidden_files);
        assert!(view.show_file_name_extensions);
        assert_eq!(view.open_utility_menu, None);
        assert!(view.directory_watcher.is_none());
    }

    #[test]
    fn read_error_suppresses_empty_folder_message() {
        let mut view = ExplorerView::new(PathBuf::from("missing"));
        view.entries.clear();
        view.read_error = Some("missing".to_owned());

        assert!(!view.should_show_empty_folder_message());
    }

    #[test]
    fn non_empty_directory_suppresses_empty_folder_message() {
        let mut view = ExplorerView::new(PathBuf::from("non-empty"));
        view.all_entries = vec![FileEntry::test("file.txt", false, Some(1), None)];
        view.entries = view.all_entries.clone();
        view.read_error = None;

        assert!(!view.should_show_empty_folder_message());
    }

    #[test]
    fn content_branch_prioritizes_error_empty_then_list() {
        let mut view = ExplorerView::new(PathBuf::from("branch"));

        view.entries.clear();
        view.read_error = Some("error".to_owned());
        assert_eq!(view.content_branch(), ExplorerContentBranch::Error);

        view.read_error = None;
        assert_eq!(view.content_branch(), ExplorerContentBranch::Empty);

        view.all_entries = vec![FileEntry::test("file.txt", false, Some(1), None)];
        view.entries = view.all_entries.clone();
        assert_eq!(view.content_branch(), ExplorerContentBranch::List);

        view.set_search_query("missing".to_owned());
        assert_eq!(
            view.content_branch(),
            ExplorerContentBranch::NoSearchMatches
        );
    }

    #[test]
    fn shell_shortcut_resolution_updates_entries_and_preserves_selection() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.path = PathBuf::from("root");
        view.shell_shortcut_resolution_generation = 7;
        view.all_entries = vec![test_pending_shell_shortcut("shortcut.lnk", "target")];
        view.entries = view.all_entries.clone();
        view.select_single_path(Path::new("shortcut.lnk"));

        assert!(view.apply_shell_shortcut_resolutions(
            Path::new("root"),
            7,
            vec![ShellShortcutResolution {
                path: PathBuf::from("shortcut.lnk"),
                target_kind: ShellShortcutTargetKind::Directory,
            }],
        ));

        assert!(view.all_entries[0].is_directory_like());
        assert!(view.entries[0].is_directory_like());
        assert_eq!(view.entries[0].navigation_path(), Path::new("target"));
        assert_eq!(view.selected_paths(), vec![PathBuf::from("shortcut.lnk")]);
    }

    #[test]
    fn stale_shell_shortcut_resolution_is_ignored() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.path = PathBuf::from("root");
        view.shell_shortcut_resolution_generation = 2;
        view.all_entries = vec![test_pending_shell_shortcut("shortcut.lnk", "target")];
        view.entries = view.all_entries.clone();

        assert!(!view.apply_shell_shortcut_resolutions(
            Path::new("root"),
            1,
            vec![ShellShortcutResolution {
                path: PathBuf::from("shortcut.lnk"),
                target_kind: ShellShortcutTargetKind::Directory,
            }],
        ));
        assert!(!view.entries[0].is_directory_like());

        assert!(!view.apply_shell_shortcut_resolutions(
            Path::new("other"),
            2,
            vec![ShellShortcutResolution {
                path: PathBuf::from("shortcut.lnk"),
                target_kind: ShellShortcutTargetKind::Directory,
            }],
        ));
        assert!(!view.entries[0].is_directory_like());
    }
}
