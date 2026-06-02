use std::{collections::BTreeSet, path::PathBuf};

use gpui::{FocusHandle, UniformListScrollHandle};

use crate::explorer::filesystem::FileConflictBatch;
use crate::explorer::{
    entry::FileEntry, filesystem::load_entries, mouse_selection::MouseSelectionDrag,
    scrollbar::ScrollbarDrag, selection::SelectionState,
};

pub struct ExplorerView {
    pub(super) path: PathBuf,
    pub(super) entries: Vec<FileEntry>,
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
    pub(super) pending_permanent_delete: Option<PendingPermanentDelete>,
    pub(super) pending_file_conflict: Option<FileConflictBatch>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PendingPermanentDelete {
    pub(super) paths: Vec<PathBuf>,
    pub(super) fallback_index: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExplorerContentBranch {
    Error,
    Empty,
    List,
}

impl ExplorerView {
    #[cfg(test)]
    pub fn new(initial_path: PathBuf) -> Self {
        Self::new_inner(initial_path, None)
    }

    pub fn new_with_focus_handle(initial_path: PathBuf, focus_handle: FocusHandle) -> Self {
        Self::new_inner(initial_path, Some(focus_handle))
    }

    fn new_inner(initial_path: PathBuf, focus_handle: Option<FocusHandle>) -> Self {
        let mut view = Self {
            path: initial_path,
            entries: Vec::new(),
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
            pending_permanent_delete: None,
            pending_file_conflict: None,
        };
        view.reload();
        view
    }

    pub fn reload(&mut self) {
        self.open_error = None;
        let selected_paths = self.selected_paths();

        match load_entries(&self.path) {
            Ok(entries) => {
                self.entries = entries;
                self.read_error = None;
                self.restore_selection_from_paths(&selected_paths);
            }
            Err(error) => {
                self.entries.clear();
                self.clear_selection();
                self.read_error = Some(error.to_string());
            }
        }
    }
}

impl ExplorerView {
    pub(super) fn should_show_empty_folder_message(&self) -> bool {
        self.entries.is_empty() && self.read_error.is_none()
    }

    pub(super) fn content_branch(&self) -> ExplorerContentBranch {
        if self.read_error.is_some() {
            ExplorerContentBranch::Error
        } else if self.should_show_empty_folder_message() {
            ExplorerContentBranch::Empty
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
    fn read_error_suppresses_empty_folder_message() {
        let mut view = ExplorerView::new(PathBuf::from("missing"));
        view.entries.clear();
        view.read_error = Some("missing".to_owned());

        assert!(!view.should_show_empty_folder_message());
    }

    #[test]
    fn non_empty_directory_suppresses_empty_folder_message() {
        let mut view = ExplorerView::new(PathBuf::from("non-empty"));
        view.entries = vec![FileEntry::test("file.txt", false, Some(1), None)];
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

        view.entries = vec![FileEntry::test("file.txt", false, Some(1), None)];
        assert_eq!(view.content_branch(), ExplorerContentBranch::List);
    }
}
