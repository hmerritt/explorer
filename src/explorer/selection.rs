use std::path::{Path, PathBuf};

use gpui::ScrollStrategy;

use crate::explorer::{constants::ROW_HEIGHT, entry::FileEntry, view::ExplorerView};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct SelectionState {
    pub(super) anchor_index: Option<usize>,
    pub(super) focused_index: Option<usize>,
    pub(super) selected_range: Option<SelectionRange>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SelectionRange {
    pub(super) start: usize,
    pub(super) end: usize,
}

impl SelectionRange {
    pub(super) fn new(a: usize, b: usize) -> Self {
        Self {
            start: a.min(b),
            end: a.max(b),
        }
    }

    pub(super) fn contains(self, ix: usize) -> bool {
        ix >= self.start && ix <= self.end
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SelectionDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SelectionEdge {
    Home,
    End,
}

impl ExplorerView {
    pub(super) fn selected_paths(&self) -> Vec<PathBuf> {
        let Some(range) = self.selection.selected_range else {
            return Vec::new();
        };

        (range.start..=range.end)
            .filter_map(|ix| self.entries.get(ix).map(|entry| entry.path.clone()))
            .collect()
    }

    pub(super) fn restore_selection_from_paths(&mut self, paths: &[PathBuf]) {
        let mut indices = paths
            .iter()
            .filter_map(|path| self.entry_index_by_path(path))
            .collect::<Vec<_>>();

        indices.sort_unstable();
        indices.dedup();

        let Some(first) = indices.first().copied() else {
            self.clear_selection();
            return;
        };

        let last = indices.last().copied().unwrap_or(first);
        self.selection = SelectionState {
            anchor_index: Some(first),
            focused_index: Some(last),
            selected_range: Some(SelectionRange::new(first, last)),
        };
    }

    pub(super) fn entry_index_by_path(&self, path: &Path) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.path.as_path() == path)
    }

    pub(super) fn entry_is_selected(&self, ix: usize) -> bool {
        self.selection
            .selected_range
            .is_some_and(|range| range.contains(ix))
    }

    pub(super) fn focused_entry(&self) -> Option<&FileEntry> {
        self.selection
            .focused_index
            .and_then(|ix| self.entries.get(ix))
    }

    pub(super) fn select_single_index(&mut self, ix: usize) {
        if ix >= self.entries.len() {
            self.clear_selection();
            return;
        }

        self.selection = SelectionState {
            anchor_index: Some(ix),
            focused_index: Some(ix),
            selected_range: Some(SelectionRange::new(ix, ix)),
        };
        self.scroll_index_into_view(ix);
    }

    pub(super) fn select_single_path(&mut self, path: &Path) {
        if let Some(ix) = self.entry_index_by_path(path) {
            self.select_single_index(ix);
        } else {
            self.clear_selection();
        }
    }

    pub(super) fn extend_selection_to_index(&mut self, ix: usize) {
        if ix >= self.entries.len() {
            return;
        }

        let anchor = self
            .selection
            .anchor_index
            .or(self.selection.focused_index)
            .unwrap_or(ix);
        self.selection = SelectionState {
            anchor_index: Some(anchor),
            focused_index: Some(ix),
            selected_range: Some(SelectionRange::new(anchor, ix)),
        };
        self.scroll_index_into_view(ix);
    }

    pub(super) fn select_all_entries(&mut self) {
        if self.entries.is_empty() {
            self.clear_selection();
            return;
        }

        let last = self.entries.len() - 1;
        self.selection = SelectionState {
            anchor_index: Some(0),
            focused_index: Some(last),
            selected_range: Some(SelectionRange::new(0, last)),
        };
        self.scroll_index_into_view(last);
    }

    pub(super) fn scroll_index_into_view(&self, ix: usize) {
        let row_top = ix as f32 * ROW_HEIGHT;
        let row_bottom = row_top + ROW_HEIGHT;

        if let Some(metrics) = self.scrollbar_metrics() {
            let viewport_bottom = metrics.scroll_top + metrics.viewport_height;
            if row_top < metrics.scroll_top {
                self.set_scroll_offset(row_top);
            } else if row_bottom > viewport_bottom {
                self.set_scroll_offset(row_bottom - metrics.viewport_height);
            }
        } else {
            self.scroll_handle.scroll_to_item(ix, ScrollStrategy::Top);
        }
    }

    pub(super) fn clear_selection(&mut self) {
        self.selection = SelectionState::default();
    }

    pub(super) fn move_selection(&mut self, direction: SelectionDirection) {
        let Some(last) = self.entries.len().checked_sub(1) else {
            self.clear_selection();
            return;
        };

        let target = match (self.selection.focused_index, direction) {
            (Some(ix), SelectionDirection::Up) => ix.saturating_sub(1),
            (Some(ix), SelectionDirection::Down) => (ix + 1).min(last),
            (None, SelectionDirection::Up) => last,
            (None, SelectionDirection::Down) => 0,
        };

        self.select_single_index(target);
    }

    pub(super) fn extend_selection(&mut self, direction: SelectionDirection) {
        let Some(last) = self.entries.len().checked_sub(1) else {
            self.clear_selection();
            return;
        };

        let Some(focused) = self.selection.focused_index else {
            self.move_selection(direction);
            return;
        };

        let target = match direction {
            SelectionDirection::Up if focused > 0 => focused - 1,
            SelectionDirection::Down if focused < last => focused + 1,
            _ => return,
        };

        self.extend_selection_to_index(target);
    }

    pub(super) fn select_edge(&mut self, edge: SelectionEdge) {
        let Some(last) = self.entries.len().checked_sub(1) else {
            self.clear_selection();
            return;
        };

        let target = match edge {
            SelectionEdge::Home => 0,
            SelectionEdge::End => last,
        };
        self.select_single_index(target);
    }

    pub(super) fn extend_selection_to_edge(&mut self, edge: SelectionEdge) {
        let Some(last) = self.entries.len().checked_sub(1) else {
            self.clear_selection();
            return;
        };

        let target = match edge {
            SelectionEdge::Home => 0,
            SelectionEdge::End => last,
        };
        self.extend_selection_to_index(target);
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::explorer::{
        test_support::{TempDir, selected_names, test_view_with_entries},
        view::ExplorerView,
    };
    use std::fs;

    #[test]
    fn clear_selection_removes_selected_paths() {
        let temp = TempDir::new();
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_index(0);

        view.clear_selection();
        assert!(view.selected_paths().is_empty());

        view.clear_selection();
        assert!(view.selected_paths().is_empty());
    }

    #[test]
    fn up_down_selection_initializes_and_clamps_at_bounds() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);

        view.move_selection(SelectionDirection::Down);
        assert_eq!(selected_names(&view), vec!["a.txt"]);

        view.move_selection(SelectionDirection::Down);
        assert_eq!(selected_names(&view), vec!["b.txt"]);

        view.move_selection(SelectionDirection::Down);
        view.move_selection(SelectionDirection::Down);
        assert_eq!(selected_names(&view), vec!["c.txt"]);

        view.clear_selection();
        view.move_selection(SelectionDirection::Up);
        assert_eq!(selected_names(&view), vec!["c.txt"]);

        view.move_selection(SelectionDirection::Up);
        view.move_selection(SelectionDirection::Up);
        view.move_selection(SelectionDirection::Up);
        assert_eq!(selected_names(&view), vec!["a.txt"]);
    }

    #[test]
    fn shift_up_down_extends_selection_and_stops_at_bounds() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);

        view.select_single_index(1);
        view.extend_selection(SelectionDirection::Down);
        assert_eq!(selected_names(&view), vec!["b.txt", "c.txt"]);

        view.extend_selection(SelectionDirection::Down);
        assert_eq!(selected_names(&view), vec!["b.txt", "c.txt"]);

        view.select_single_index(1);
        view.extend_selection(SelectionDirection::Up);
        assert_eq!(selected_names(&view), vec!["a.txt", "b.txt"]);

        view.extend_selection(SelectionDirection::Up);
        assert_eq!(selected_names(&view), vec!["a.txt", "b.txt"]);
    }

    #[test]
    fn home_end_and_shift_home_end_update_selection_ranges() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt", "d.txt"]);

        view.select_edge(SelectionEdge::End);
        assert_eq!(selected_names(&view), vec!["d.txt"]);

        view.select_edge(SelectionEdge::Home);
        assert_eq!(selected_names(&view), vec!["a.txt"]);

        view.select_single_index(2);
        view.extend_selection_to_edge(SelectionEdge::Home);
        assert_eq!(selected_names(&view), vec!["a.txt", "b.txt", "c.txt"]);

        view.select_single_index(1);
        view.extend_selection_to_edge(SelectionEdge::End);
        assert_eq!(selected_names(&view), vec!["b.txt", "c.txt", "d.txt"]);
    }

    #[test]
    fn select_all_entries_selects_every_entry() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);

        view.select_all_entries();

        assert_eq!(selected_names(&view), vec!["a.txt", "b.txt", "c.txt"]);
    }

    #[test]
    fn reload_preserves_surviving_selected_paths() {
        let temp = TempDir::new();
        let a = temp.path().join("a.txt");
        let b = temp.path().join("b.txt");
        let c = temp.path().join("c.txt");
        fs::write(&a, b"a").expect("create a");
        fs::write(&b, b"b").expect("create b");
        fs::write(&c, b"c").expect("create c");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&b);
        view.extend_selection_to_index(view.entry_index_by_path(&c).expect("c entry"));
        fs::remove_file(&b).expect("remove b");

        view.reload();

        assert_eq!(view.selected_paths(), vec![c]);
    }
}
