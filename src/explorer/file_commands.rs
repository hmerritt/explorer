use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use gpui::Context;

use crate::explorer::{
    clipboard::{
        FileClipboard, FileClipboardOperation, clipboard_item_for_files, file_clipboard_from_item,
    },
    filesystem::{
        ConflictChoice, FileOperationOutcome, FileOperationSummary,
        copy_paths_to_directory_for_paste, move_paths_to_directory, remove_paths_permanently,
        resolve_file_conflicts, trash_paths,
    },
    view::{ExplorerView, PendingPermanentDelete},
};

impl ExplorerView {
    pub(super) fn copy_selected_to_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(clipboard) = self.selected_file_clipboard(FileClipboardOperation::Copy) else {
            return;
        };

        match clipboard_item_for_files(&clipboard) {
            Ok(item) => {
                cx.write_to_clipboard(item);
                self.cut_paths.clear();
                self.open_error = None;
            }
            Err(error) => self.open_error = Some(error),
        }
    }

    pub(super) fn cut_selected_to_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(clipboard) = self.selected_file_clipboard(FileClipboardOperation::Cut) else {
            return;
        };

        match clipboard_item_for_files(&clipboard) {
            Ok(item) => {
                cx.write_to_clipboard(item);
                self.mark_cut_paths(&clipboard.paths);
                self.open_error = None;
            }
            Err(error) => self.open_error = Some(error),
        }
    }

    pub(super) fn paste_clipboard_files(&mut self, cx: &mut Context<Self>) {
        let Some(clipboard) = cx
            .read_from_clipboard()
            .as_ref()
            .and_then(file_clipboard_from_item)
        else {
            return;
        };

        match clipboard.operation {
            FileClipboardOperation::Copy => {
                self.handle_file_command_result_and_open_dialog(
                    copy_paths_to_directory_for_paste(&clipboard.paths, &self.path),
                    cx,
                );
            }
            FileClipboardOperation::Cut => {
                let result = move_paths_to_directory(&clipboard.paths, &self.path);
                self.handle_file_command_result_and_open_dialog(result, cx);
            }
        }
    }

    pub(super) fn trash_selected_paths(&mut self) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }

        let fallback_index = self.selection_fallback_index_for_delete();
        match trash_paths(&paths) {
            Ok(()) => {
                self.remove_cut_paths(&paths);
                self.reload();
                self.select_fallback_index(fallback_index);
                self.open_error = None;
            }
            Err(error) => {
                self.open_error = Some(error);
                self.reload();
            }
        }
    }

    pub(super) fn request_permanent_delete_selected(&mut self, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }

        self.pending_permanent_delete = Some(PendingPermanentDelete {
            paths,
            fallback_index: self.selection_fallback_index_for_delete(),
        });
        self.open_error = None;
        self.open_pending_dialog_window(cx);
    }

    pub(super) fn confirm_pending_permanent_delete(&mut self) {
        let Some(pending) = self.pending_permanent_delete.take() else {
            return;
        };

        match remove_paths_permanently(&pending.paths) {
            Ok(()) => {
                self.remove_cut_paths(&pending.paths);
                self.reload();
                self.select_fallback_index(pending.fallback_index);
                self.open_error = None;
            }
            Err(error) => {
                self.open_error = Some(error);
                self.reload();
            }
        }
    }

    pub(super) fn cancel_pending_permanent_delete(&mut self) {
        self.pending_permanent_delete = None;
    }

    pub(super) fn replace_pending_file_conflicts(&mut self) {
        self.resolve_pending_file_conflicts(ConflictChoice::Replace);
    }

    pub(super) fn skip_pending_file_conflicts(&mut self) {
        self.resolve_pending_file_conflicts(ConflictChoice::Skip);
    }

    pub(super) fn selected_file_clipboard(
        &self,
        operation: FileClipboardOperation,
    ) -> Option<FileClipboard> {
        let paths = self.selected_paths();
        (!paths.is_empty()).then(|| FileClipboard::new(operation, paths))
    }

    pub(super) fn mark_cut_paths(&mut self, paths: &[PathBuf]) {
        self.cut_paths = paths.iter().cloned().collect();
    }

    #[cfg(test)]
    pub(super) fn clear_cut_paths(&mut self) {
        self.cut_paths.clear();
    }

    pub(super) fn remove_cut_paths(&mut self, paths: &[PathBuf]) {
        let paths = paths.iter().collect::<BTreeSet<_>>();
        self.cut_paths.retain(|path| !paths.contains(path));
    }

    pub(super) fn entry_is_cut(&self, path: &Path) -> bool {
        self.cut_paths.contains(path)
    }

    pub(super) fn handle_file_command_result(
        &mut self,
        result: Result<FileOperationOutcome, String>,
    ) {
        match result {
            Ok(FileOperationOutcome::Finished(summary)) => {
                self.finish_file_operation(summary);
            }
            Ok(FileOperationOutcome::Conflicts(conflicts)) => {
                self.pending_file_conflict = Some(conflicts);
                self.open_error = None;
            }
            Err(error) => {
                self.open_error = Some(error);
                self.reload();
            }
        }
    }

    pub(super) fn handle_file_command_result_and_open_dialog(
        &mut self,
        result: Result<FileOperationOutcome, String>,
        cx: &mut Context<Self>,
    ) {
        self.handle_file_command_result(result);
        self.open_pending_dialog_window(cx);
    }

    fn resolve_pending_file_conflicts(&mut self, choice: ConflictChoice) {
        let Some(conflicts) = self.pending_file_conflict.take() else {
            return;
        };

        match resolve_file_conflicts(conflicts, choice) {
            Ok(summary) => self.finish_file_operation(summary),
            Err(error) => {
                self.open_error = Some(error);
                self.reload();
            }
        }
    }

    fn finish_file_operation(&mut self, summary: FileOperationSummary) {
        self.open_error = None;
        self.remove_cut_paths(&summary.moved_source_paths);
        self.reload();
        self.restore_selection_from_paths(&summary.destination_paths);
    }

    fn selection_fallback_index_for_delete(&self) -> Option<usize> {
        self.selection.selected_indices.first().copied()
    }

    fn select_fallback_index(&mut self, fallback_index: Option<usize>) {
        let Some(last) = self.entries.len().checked_sub(1) else {
            self.clear_selection();
            return;
        };

        let ix = fallback_index.unwrap_or(0).min(last);
        self.select_single_index(ix);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::{
        clipboard::FileClipboardOperation,
        selection::SelectionModifiers,
        test_support::{TempDir, selected_names, test_view_with_entries},
    };
    use std::fs;

    #[test]
    fn selected_file_clipboard_is_empty_without_selection() {
        let view = test_view_with_entries(&["a.txt"]);

        assert_eq!(
            view.selected_file_clipboard(FileClipboardOperation::Copy),
            None
        );
    }

    #[test]
    fn selected_file_clipboard_includes_single_selection() {
        let mut view = test_view_with_entries(&["a.txt"]);
        view.select_single_index(0);

        let clipboard = view
            .selected_file_clipboard(FileClipboardOperation::Copy)
            .expect("clipboard");

        assert_eq!(clipboard.operation, FileClipboardOperation::Copy);
        assert_eq!(clipboard.paths, vec![PathBuf::from("a.txt")]);
    }

    #[test]
    fn selected_file_clipboard_includes_multi_selection() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);
        view.select_single_index(0);
        view.apply_click_selection(
            2,
            SelectionModifiers {
                toggle: true,
                extend: false,
            },
        );

        let clipboard = view
            .selected_file_clipboard(FileClipboardOperation::Cut)
            .expect("clipboard");

        assert_eq!(clipboard.operation, FileClipboardOperation::Cut);
        assert_eq!(
            clipboard.paths,
            vec![PathBuf::from("a.txt"), PathBuf::from("c.txt")]
        );
    }

    #[test]
    fn only_cut_paths_are_dimmed() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);

        view.mark_cut_paths(&[PathBuf::from("b.txt")]);

        assert!(!view.entry_is_cut(Path::new("a.txt")));
        assert!(view.entry_is_cut(Path::new("b.txt")));
        view.clear_cut_paths();
        assert!(!view.entry_is_cut(Path::new("b.txt")));
    }

    #[test]
    fn successful_cut_paste_moves_files_and_clears_cut_state() {
        let temp = TempDir::new();
        let source_dir = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir_all(&source_dir).expect("create source");
        fs::create_dir(&destination).expect("create destination");
        let source = source_dir.join("file.txt");
        fs::write(&source, b"data").expect("create source file");

        let mut view = ExplorerView::new(destination.clone());
        view.mark_cut_paths(std::slice::from_ref(&source));
        let result = move_paths_to_directory(std::slice::from_ref(&source), &view.path);
        view.handle_file_command_result(result);

        assert!(!source.exists());
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"data");
        assert!(view.cut_paths.is_empty());
        assert_eq!(selected_names(&view), vec!["file.txt"]);
    }

    #[test]
    fn delete_fallback_selects_next_surviving_row() {
        let temp = TempDir::new();
        let a = temp.path().join("a.txt");
        let b = temp.path().join("b.txt");
        let c = temp.path().join("c.txt");
        fs::write(&a, b"a").expect("create a");
        fs::write(&b, b"b").expect("create b");
        fs::write(&c, b"c").expect("create c");
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&b);

        let fallback = view.selection_fallback_index_for_delete();
        remove_paths_permanently(std::slice::from_ref(&b)).expect("delete b");
        view.reload();
        view.select_fallback_index(fallback);

        assert_eq!(selected_names(&view), vec!["c.txt"]);
    }
}
