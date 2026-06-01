use std::path::{Path, PathBuf};

use crate::explorer::{
    entry::FileEntry,
    filesystem::{format_open_error, open_path_with_default_app},
    selection::SelectionModifiers,
    view::ExplorerView,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HistoryMode {
    Record,
    Preserve,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum EntryAction {
    OpenFile(PathBuf),
}

impl ExplorerView {
    pub(super) fn navigate_to_directory(&mut self, path: PathBuf, history_mode: HistoryMode) {
        if path == self.path {
            self.reload();
            return;
        }

        let select_entry_after_reload =
            (self.path.parent() == Some(path.as_path())).then(|| self.path.clone());

        if matches!(history_mode, HistoryMode::Record) {
            self.back_stack.push(self.path.clone());
            self.forward_stack.clear();
        }

        self.path = path;
        self.clear_selection();
        self.read_error = None;
        self.open_error = None;
        self.scroll_to_top();
        self.reload();

        if let Some(path) = select_entry_after_reload {
            self.select_single_path(&path);
        }
    }

    pub(super) fn navigate_back(&mut self) {
        if let Some(path) = self.back_stack.pop() {
            self.forward_stack.push(self.path.clone());
            self.navigate_to_directory(path, HistoryMode::Preserve);
        }
    }

    pub(super) fn navigate_forward(&mut self) {
        if let Some(path) = self.forward_stack.pop() {
            self.back_stack.push(self.path.clone());
            self.navigate_to_directory(path, HistoryMode::Preserve);
        }
    }

    pub(super) fn navigate_up(&mut self) {
        if let Some(parent) = self.path.parent().map(Path::to_path_buf) {
            self.navigate_to_directory(parent, HistoryMode::Record);
        }
    }

    pub(super) fn can_go_back(&self) -> bool {
        !self.back_stack.is_empty()
    }

    pub(super) fn can_go_forward(&self) -> bool {
        !self.forward_stack.is_empty()
    }

    pub(super) fn can_go_up(&self) -> bool {
        self.path.parent().is_some()
    }

    pub(super) fn handle_entry_click(
        &mut self,
        entry: &FileEntry,
        click_count: usize,
        modifiers: SelectionModifiers,
    ) -> Option<EntryAction> {
        if let Some(ix) = self.entry_index_by_path(&entry.path) {
            self.apply_click_selection(ix, modifiers);
        } else {
            self.clear_selection();
        }
        self.open_error = None;

        if click_count < 2 {
            return None;
        }

        if entry.is_dir {
            self.navigate_to_directory(entry.path.clone(), HistoryMode::Record);
            None
        } else {
            Some(EntryAction::OpenFile(entry.path.clone()))
        }
    }

    pub(super) fn activate_focused_entry(&mut self, open_files: bool) -> Option<EntryAction> {
        let entry = self.focused_entry()?.clone();
        self.open_error = None;

        if entry.is_dir {
            self.navigate_to_directory(entry.path, HistoryMode::Record);
            None
        } else if open_files {
            Some(EntryAction::OpenFile(entry.path))
        } else {
            None
        }
    }

    pub(super) fn open_file_with_default_app(&mut self, path: &Path) {
        let result = open_path_with_default_app(path);
        self.handle_open_file_result(path, result);
    }

    pub(super) fn handle_open_file_result(&mut self, path: &Path, result: std::io::Result<()>) {
        match result {
            Ok(()) => self.open_error = None,
            Err(error) => {
                self.open_error = Some(format_open_error(path, &error));
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::explorer::{entry::FileEntry, test_support::TempDir, view::ExplorerView};
    use std::{fs, path::PathBuf};

    #[test]
    fn navigating_to_valid_directory_updates_path_and_clears_selection() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir_all(&child).expect("create child directory");
        fs::write(child.join("inside.txt"), b"data").expect("create child file");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&child);
        view.open_error = Some("stale error".to_owned());

        view.navigate_to_directory(child.clone(), HistoryMode::Record);

        assert_eq!(view.path, child);
        assert!(view.selected_paths().is_empty());
        assert_eq!(view.read_error, None);
        assert_eq!(view.open_error, None);
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
        assert!(view.forward_stack.is_empty());
        assert_eq!(view.entries.len(), 1);
        assert_eq!(view.entries[0].name, "inside.txt");
    }

    #[test]
    fn navigating_to_missing_directory_sets_read_error_and_empty_entries() {
        let temp = TempDir::new();
        let missing = temp.path().join("missing");
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_index(0);

        view.navigate_to_directory(missing.clone(), HistoryMode::Record);

        assert_eq!(view.path, missing);
        assert!(view.selected_paths().is_empty());
        assert!(view.read_error.is_some());
        assert!(view.entries.is_empty());
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn single_click_selects_without_navigating() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir_all(&child).expect("create child directory");
        let entry = FileEntry {
            path: child.clone(),
            name: "child".to_owned(),
            is_dir: true,
            modified: None,
            size: None,
        };
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.open_error = Some("stale error".to_owned());

        let action = view.handle_entry_click(&entry, 1, SelectionModifiers::default());

        assert_eq!(action, None);
        assert_eq!(view.path, temp.path());
        assert_eq!(view.selected_paths(), vec![child]);
        assert_eq!(view.open_error, None);
        assert!(view.back_stack.is_empty());
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn double_click_opens_files_and_navigates_directories() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        let file = temp.path().join("file.txt");
        fs::create_dir_all(&child).expect("create child directory");
        fs::write(&file, b"data").expect("create file");

        let file_entry = FileEntry {
            path: file.clone(),
            name: "file.txt".to_owned(),
            is_dir: false,
            modified: None,
            size: Some(4),
        };
        let dir_entry = FileEntry {
            path: child.clone(),
            name: "child".to_owned(),
            is_dir: true,
            modified: None,
            size: None,
        };
        let mut view = ExplorerView::new(temp.path().to_path_buf());

        let action = view.handle_entry_click(&file_entry, 2, SelectionModifiers::default());
        assert_eq!(action, Some(EntryAction::OpenFile(file.clone())));
        assert_eq!(view.path, temp.path());
        assert_eq!(view.selected_paths(), vec![file.clone()]);
        assert!(view.back_stack.is_empty());
        assert!(view.forward_stack.is_empty());

        let action = view.handle_entry_click(&dir_entry, 2, SelectionModifiers::default());
        assert_eq!(action, None);
        assert_eq!(view.path, child);
        assert!(view.selected_paths().is_empty());
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn open_file_result_sets_and_clears_open_error() {
        let temp = TempDir::new();
        let file = temp.path().join("file.txt");
        let mut view = ExplorerView::new(temp.path().to_path_buf());

        view.handle_open_file_result(
            &file,
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing")),
        );

        assert_eq!(
            view.open_error,
            Some("Could not open file.txt: missing".to_owned())
        );

        view.handle_open_file_result(&file, Ok(()));

        assert_eq!(view.open_error, None);
    }

    #[test]
    fn refresh_clears_open_error() {
        let temp = TempDir::new();
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.open_error = Some("stale error".to_owned());

        view.reload();

        assert_eq!(view.open_error, None);
    }

    #[test]
    fn focused_activation_enters_directories_and_opens_files_on_enter() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.entries = vec![
            FileEntry::test("folder", true, None, None),
            FileEntry::test("file.txt", false, Some(4), None),
        ];

        view.select_single_index(0);
        assert_eq!(view.activate_focused_entry(true), None);
        assert_eq!(view.path, PathBuf::from("folder"));

        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.entries = vec![FileEntry::test("file.txt", false, Some(4), None)];
        view.select_single_index(0);

        assert_eq!(
            view.activate_focused_entry(true),
            Some(EntryAction::OpenFile(PathBuf::from("file.txt")))
        );
        assert_eq!(view.path, PathBuf::from("root"));
    }

    #[test]
    fn right_arrow_activation_ignores_files() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.entries = vec![FileEntry::test("file.txt", false, Some(4), None)];
        view.select_single_index(0);

        assert_eq!(view.activate_focused_entry(false), None);
        assert_eq!(view.path, PathBuf::from("root"));
    }

    #[test]
    fn folder_navigation_records_back_and_clears_forward() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir_all(&child).expect("create child directory");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.forward_stack.push(temp.path().join("forward"));

        view.navigate_to_directory(child.clone(), HistoryMode::Record);

        assert_eq!(view.path, child);
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn back_and_forward_move_between_paths() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir_all(&child).expect("create child directory");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.navigate_to_directory(child.clone(), HistoryMode::Record);

        view.navigate_back();
        assert_eq!(view.path, temp.path());
        assert_eq!(view.selected_paths(), vec![child.clone()]);
        assert!(view.back_stack.is_empty());
        assert_eq!(view.forward_stack, vec![child.clone()]);

        view.navigate_forward();
        assert_eq!(view.path, child);
        assert!(view.selected_paths().is_empty());
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn up_navigates_to_parent_selects_origin_and_records_history() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        let grandchild = child.join("grandchild");
        fs::create_dir_all(&grandchild).expect("create nested directories");

        let mut view = ExplorerView::new(grandchild.clone());

        view.navigate_up();

        assert_eq!(view.path, child);
        assert_eq!(view.selected_paths(), vec![grandchild.clone()]);
        assert_eq!(view.back_stack, vec![grandchild]);
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn refresh_preserves_path_and_history() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir_all(&child).expect("create child directory");
        let back = temp.path().join("back");
        let forward = temp.path().join("forward");

        let mut view = ExplorerView::new(child.clone());
        view.back_stack.push(back.clone());
        view.forward_stack.push(forward.clone());

        view.reload();

        assert_eq!(view.path, child);
        assert_eq!(view.back_stack, vec![back]);
        assert_eq!(view.forward_stack, vec![forward]);
    }
}
