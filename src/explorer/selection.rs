use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use gpui::{Modifiers, ScrollStrategy};

use crate::explorer::{constants::ROW_HEIGHT, entry::FileEntry, view::ExplorerView};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct SelectionState {
    pub(super) anchor_index: Option<usize>,
    pub(super) focused_index: Option<usize>,
    pub(super) selected_indices: BTreeSet<usize>,
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct SelectionModifiers {
    pub(super) toggle: bool,
    pub(super) extend: bool,
}

impl SelectionModifiers {
    pub(super) fn from_gpui(modifiers: Modifiers) -> Self {
        Self {
            toggle: modifiers.secondary(),
            extend: modifiers.shift,
        }
    }
}

impl ExplorerView {
    pub(super) fn selected_paths(&self) -> Vec<PathBuf> {
        self.selection
            .selected_indices
            .iter()
            .filter_map(|ix| self.entries.get(*ix).map(|entry| entry.path.clone()))
            .collect()
    }

    pub(super) fn restore_selection_from_paths(&mut self, paths: &[PathBuf]) {
        self.cancel_pending_click_rename();

        let mut indices = paths
            .iter()
            .filter_map(|path| self.entry_index_by_path(path))
            .collect::<Vec<_>>();

        indices.sort_unstable();
        indices.dedup();

        let Some(anchor) = indices.first().copied() else {
            self.clear_selection();
            return;
        };

        let focused = indices.last().copied().unwrap_or(anchor);
        self.selection = SelectionState {
            anchor_index: Some(anchor),
            focused_index: Some(focused),
            selected_indices: indices.into_iter().collect(),
        };
    }

    pub(super) fn entry_index_by_path(&self, path: &Path) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.path.as_path() == path)
    }

    pub(super) fn entry_is_selected(&self, ix: usize) -> bool {
        self.selection.selected_indices.contains(&ix)
    }

    pub(super) fn focused_entry(&self) -> Option<&FileEntry> {
        self.selection
            .focused_index
            .and_then(|ix| self.entries.get(ix))
    }

    pub(super) fn select_single_index(&mut self, ix: usize) {
        self.cancel_pending_click_rename();

        if ix >= self.entries.len() {
            self.clear_selection();
            return;
        }

        self.selection = SelectionState {
            anchor_index: Some(ix),
            focused_index: Some(ix),
            selected_indices: BTreeSet::from([ix]),
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
        self.cancel_pending_click_rename();

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
            selected_indices: indices_in_range(anchor, ix).collect(),
        };
        self.scroll_index_into_view(ix);
    }

    pub(super) fn toggle_selection_index(&mut self, ix: usize) {
        self.cancel_pending_click_rename();

        if ix >= self.entries.len() {
            return;
        }

        if !self.selection.selected_indices.remove(&ix) {
            self.selection.selected_indices.insert(ix);
        }
        self.selection.anchor_index = Some(ix);
        self.selection.focused_index = Some(ix);
        self.scroll_index_into_view(ix);
    }

    pub(super) fn apply_click_selection(&mut self, ix: usize, modifiers: SelectionModifiers) {
        if ix >= self.entries.len() {
            self.clear_selection();
            return;
        }

        match (modifiers.toggle, modifiers.extend) {
            (false, false) => self.select_single_index(ix),
            (true, false) => self.toggle_selection_index(ix),
            (false, true) => self.extend_selection_to_index(ix),
            (true, true) => {}
        }
    }

    pub(super) fn replace_selection_with_indices(&mut self, selected_indices: BTreeSet<usize>) {
        self.cancel_pending_click_rename();

        let Some(anchor) = selected_indices.first().copied() else {
            self.clear_selection();
            return;
        };

        let focused = selected_indices.last().copied().unwrap_or(anchor);
        self.selection = SelectionState {
            anchor_index: Some(anchor),
            focused_index: Some(focused),
            selected_indices,
        };
    }

    pub(super) fn select_all_entries(&mut self) {
        self.cancel_pending_click_rename();

        if self.entries.is_empty() {
            self.clear_selection();
            return;
        }

        let last = self.entries.len() - 1;
        self.selection = SelectionState {
            anchor_index: Some(0),
            focused_index: Some(last),
            selected_indices: (0..=last).collect(),
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
        self.cancel_pending_click_rename();
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

fn indices_in_range(a: usize, b: usize) -> impl Iterator<Item = usize> {
    let range = SelectionRange::new(a, b);
    range.start..=range.end
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::explorer::{
        entry::FileEntry,
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
    fn ctrl_click_toggles_discontiguous_selection() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);

        view.select_single_index(0);
        view.apply_click_selection(
            2,
            SelectionModifiers {
                toggle: true,
                extend: false,
            },
        );
        assert_eq!(selected_names(&view), vec!["a.txt", "c.txt"]);

        view.apply_click_selection(
            0,
            SelectionModifiers {
                toggle: true,
                extend: false,
            },
        );
        assert_eq!(selected_names(&view), vec!["c.txt"]);
        assert_eq!(view.selection.anchor_index, Some(0));
        assert_eq!(view.selection.focused_index, Some(0));
    }

    #[test]
    fn shift_click_replaces_with_anchor_range() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt", "d.txt"]);

        view.select_single_index(1);
        view.apply_click_selection(
            3,
            SelectionModifiers {
                toggle: false,
                extend: true,
            },
        );

        assert_eq!(selected_names(&view), vec!["b.txt", "c.txt", "d.txt"]);
        assert_eq!(view.selection.anchor_index, Some(1));
        assert_eq!(view.selection.focused_index, Some(3));
    }

    #[test]
    fn repeated_shift_click_shrinks_selection_to_anchor_range() {
        let mut view = test_view_with_entries(&[
            "a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "f.txt", "g.txt", "h.txt",
        ]);

        view.select_single_index(4);
        view.apply_click_selection(
            7,
            SelectionModifiers {
                toggle: false,
                extend: true,
            },
        );
        assert_eq!(
            selected_names(&view),
            vec!["e.txt", "f.txt", "g.txt", "h.txt"]
        );

        view.apply_click_selection(
            5,
            SelectionModifiers {
                toggle: false,
                extend: true,
            },
        );

        assert_eq!(selected_names(&view), vec!["e.txt", "f.txt"]);
        assert_eq!(view.selection.anchor_index, Some(4));
        assert_eq!(view.selection.focused_index, Some(5));
    }

    #[test]
    fn shift_click_below_selected_file_selects_inclusive_range() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"]);

        view.select_single_index(1);
        view.apply_click_selection(
            4,
            SelectionModifiers {
                toggle: false,
                extend: true,
            },
        );

        assert_eq!(
            selected_names(&view),
            vec!["b.txt", "c.txt", "d.txt", "e.txt"]
        );
        assert_eq!(view.selection.anchor_index, Some(1));
        assert_eq!(view.selection.focused_index, Some(4));
    }

    #[test]
    fn shift_click_above_selected_file_selects_inclusive_range() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"]);

        view.select_single_index(3);
        view.apply_click_selection(
            0,
            SelectionModifiers {
                toggle: false,
                extend: true,
            },
        );

        assert_eq!(
            selected_names(&view),
            vec!["a.txt", "b.txt", "c.txt", "d.txt"]
        );
        assert_eq!(view.selection.anchor_index, Some(3));
        assert_eq!(view.selection.focused_index, Some(0));
    }

    #[test]
    fn shift_click_from_selected_folder_selects_range_across_entry_types() {
        let mut view = test_view_with_entries(&[]);
        view.entries = vec![
            FileEntry::test("folder-a", true, None, None),
            FileEntry::test("file-b.txt", false, Some(1), None),
            FileEntry::test("folder-c", true, None, None),
            FileEntry::test("file-d.txt", false, Some(1), None),
        ];

        view.select_single_index(0);
        view.apply_click_selection(
            2,
            SelectionModifiers {
                toggle: false,
                extend: true,
            },
        );

        assert_eq!(
            selected_names(&view),
            vec!["folder-a", "file-b.txt", "folder-c"]
        );
        assert_eq!(view.selection.anchor_index, Some(0));
        assert_eq!(view.selection.focused_index, Some(2));
    }

    #[test]
    fn ctrl_shift_click_does_not_change_selection() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"]);

        view.select_single_index(1);
        view.apply_click_selection(
            4,
            SelectionModifiers {
                toggle: true,
                extend: false,
            },
        );
        let selection_before = view.selection.clone();

        view.apply_click_selection(
            3,
            SelectionModifiers {
                toggle: true,
                extend: true,
            },
        );

        assert_eq!(selected_names(&view), vec!["b.txt", "e.txt"]);
        assert_eq!(view.selection, selection_before);
        assert_eq!(view.selection.anchor_index, Some(4));
        assert_eq!(view.selection.focused_index, Some(4));
    }

    #[test]
    fn keyboard_move_replaces_discontiguous_selection() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt", "d.txt"]);

        view.select_single_index(0);
        view.apply_click_selection(
            3,
            SelectionModifiers {
                toggle: true,
                extend: false,
            },
        );
        view.move_selection(SelectionDirection::Up);

        assert_eq!(selected_names(&view), vec!["c.txt"]);
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
        view.select_single_path(&a);
        view.apply_click_selection(
            view.entry_index_by_path(&c).expect("c entry"),
            SelectionModifiers {
                toggle: true,
                extend: false,
            },
        );
        fs::remove_file(&b).expect("remove b");

        view.reload();

        assert_eq!(view.selected_paths(), vec![a, c]);
    }
}
