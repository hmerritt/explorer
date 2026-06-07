use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

use gpui::{
    AnyWindowHandle, Context, EventEmitter, FocusHandle, Subscription, Task,
    UniformListScrollHandle,
};

use crate::explorer::{
    address_bar::AddressBarState,
    app_icons::AppIconCache,
    drag_drop::DropIndicator,
    entry::FileEntry,
    filesystem::{FileConflictBatch, FileOperationProgress, load_entries},
    mouse_selection::MouseSelectionDrag,
    rename::{PendingClickRename, RenameState},
    scrollbar::ScrollbarDrag,
    search::SearchState,
    selection::SelectionState,
    watcher::DirectoryWatcher,
};

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
    pub(super) directory_watcher: Option<DirectoryWatcher>,
}

pub(super) struct FileOperationState {
    pub(super) progress: FileOperationProgress,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) task: Option<Task<()>>,
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
        let mut view = Self::new_inner(initial_path, Some(focus_handle));
        view.restart_directory_watcher(cx);
        view
    }

    #[cfg(test)]
    pub(super) fn new_with_focus_handle_for_test(
        initial_path: PathBuf,
        focus_handle: FocusHandle,
    ) -> Self {
        Self::new_inner(initial_path, Some(focus_handle))
    }

    fn new_inner(initial_path: PathBuf, focus_handle: Option<FocusHandle>) -> Self {
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
            show_hidden_files: true,
            show_file_name_extensions: true,
            open_utility_menu: None,
            directory_watcher: None,
        };
        view.reload();
        view
    }

    pub fn reload(&mut self) {
        self.open_error = None;
        let selected_paths = self.selected_paths();

        match load_entries(&self.path, self.show_hidden_files) {
            Ok(entries) => {
                self.all_entries = entries;
                self.read_error = None;
                self.apply_search_filter_preserving_selection(&selected_paths);
            }
            Err(error) => {
                self.all_entries.clear();
                self.entries.clear();
                self.clear_selection();
                self.read_error = Some(error.to_string());
            }
        }
    }

    pub(super) fn emit_filesystem_changed(&self, cx: &mut Context<Self>) {
        cx.emit(ExplorerViewEvent::FilesystemChanged);
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn restart_directory_watcher(&mut self, cx: &mut Context<Self>) {
        self.directory_watcher = DirectoryWatcher::start(self.path.clone(), cx);
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

#[cfg(test)]
mod tests {

    use super::*;
    use crate::explorer::entry::FileEntry;
    use std::path::PathBuf;

    #[test]
    fn empty_directory_without_error_shows_empty_folder_message() {
        let mut view = ExplorerView::new(PathBuf::from("empty"));
        view.entries.clear();
        view.read_error = None;

        assert!(view.should_show_empty_folder_message());
    }

    #[test]
    fn view_options_default_to_showing_hidden_files_and_extensions() {
        let view = ExplorerView::new(PathBuf::from("defaults"));

        assert!(view.show_hidden_files);
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
}
