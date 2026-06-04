use std::{
    fs, io,
    ops::Range,
    path::{Path, PathBuf},
};

use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, GlobalElementId, IntoElement, LayoutId, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, Style, TextRun,
    UTF16Selection, UnderlineStyle, Window, fill, point, px, relative, rgb, size,
};

use crate::explorer::{
    actions::{
        RenameBackspace, RenameCancel, RenameCommit, RenameCopy, RenameCut, RenameDelete,
        RenameEnd, RenameHome, RenameLeft, RenameNoop, RenamePaste, RenameRight, RenameSelectAll,
        RenameSelectEnd, RenameSelectHome, RenameSelectLeft, RenameSelectRight,
        RenameSelectWordLeft, RenameSelectWordRight, RenameSelected, RenameWordLeft,
        RenameWordRight,
    },
    entry::FileEntry,
    selection::SelectionModifiers,
    view::ExplorerView,
};

#[derive(Clone)]
pub(super) struct RenameState {
    pub(super) original_path: PathBuf,
    original_name: String,
    hidden_suffix: Option<String>,
    pub(super) content: String,
    pub(super) selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    scroll_offset: Pixels,
    is_selecting: bool,
    pub(super) focus_handle: Option<FocusHandle>,
}

impl RenameState {
    fn new(
        entry: &FileEntry,
        show_file_name_extensions: bool,
        focus_handle: Option<FocusHandle>,
    ) -> Self {
        let content = entry
            .display_name_with_extensions(show_file_name_extensions)
            .to_owned();
        let hidden_suffix = hidden_rename_suffix(entry, show_file_name_extensions);
        let selected_range = initial_rename_selection(entry, &content, hidden_suffix.is_some());

        Self {
            original_path: entry.path.clone(),
            original_name: entry.name.clone(),
            hidden_suffix,
            content,
            selected_range,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            scroll_offset: px(0.0),
            is_selecting: false,
            focus_handle,
        }
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn move_to(&mut self, offset: usize) {
        let offset = self.clamp_to_boundary(offset);
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.scroll_cursor_into_view();
    }

    fn select_to(&mut self, offset: usize) {
        let offset = self.clamp_to_boundary(offset);
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }

        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.scroll_cursor_into_view();
    }

    fn select_all(&mut self) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        self.scroll_cursor_into_view();
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .char_indices()
            .rev()
            .find_map(|(ix, _)| (ix < offset).then_some(ix))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .char_indices()
            .find_map(|(ix, _)| (ix > offset).then_some(ix))
            .unwrap_or(self.content.len())
    }

    fn previous_word_boundary(&self, offset: usize) -> usize {
        let mut offset = self.clamp_to_boundary(offset);

        while let Some((previous_offset, ch)) = self.previous_char(offset) {
            if ch.is_alphanumeric() {
                break;
            }
            offset = previous_offset;
        }

        while let Some((previous_offset, ch)) = self.previous_char(offset) {
            if !ch.is_alphanumeric() {
                break;
            }
            offset = previous_offset;
        }

        offset
    }

    fn next_word_boundary(&self, offset: usize) -> usize {
        let mut offset = self.clamp_to_boundary(offset);

        while let Some((next_offset, ch)) = self.next_char(offset) {
            if !ch.is_alphanumeric() {
                break;
            }
            offset = next_offset;
        }

        while let Some((next_offset, ch)) = self.next_char(offset) {
            if ch.is_alphanumeric() {
                break;
            }
            offset = next_offset;
        }

        offset
    }

    fn previous_char(&self, offset: usize) -> Option<(usize, char)> {
        self.content
            .get(..offset)?
            .char_indices()
            .next_back()
            .map(|(ix, ch)| (ix, ch))
    }

    fn next_char(&self, offset: usize) -> Option<(usize, char)> {
        let ch = self.content.get(offset..)?.chars().next()?;
        Some((offset + ch.len_utf8(), ch))
    }

    fn clamp_to_boundary(&self, offset: usize) -> usize {
        if offset >= self.content.len() {
            return self.content.len();
        }

        if self.content.is_char_boundary(offset) {
            offset
        } else {
            self.previous_boundary(offset)
        }
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;

        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }

        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;

        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }

        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }

        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };

        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }

        self.clamp_to_boundary(line.closest_index_for_x(rename_text_x_for_mouse_x(
            position.x,
            bounds.left(),
            self.scroll_offset,
        )))
    }

    fn target_file_name(&self) -> String {
        match self.hidden_suffix.as_deref() {
            Some(suffix) => format!("{}{}", self.content, suffix),
            None => self.content.clone(),
        }
    }

    fn scroll_cursor_into_view(&mut self) {
        let (Some(line), Some(bounds)) = (self.last_layout.as_ref(), self.last_bounds.as_ref())
        else {
            return;
        };

        self.scroll_offset = scroll_offset_for_cursor(
            self.scroll_offset,
            line.x_for_index(self.cursor_offset()),
            line.width,
            bounds.right() - bounds.left(),
        );
    }
}

impl ExplorerView {
    pub(super) fn handle_rename_selected(
        &mut self,
        _: &RenameSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_rename_selected(window, cx);
        cx.notify();
    }

    pub(super) fn handle_rename_commit(
        &mut self,
        _: &RenameCommit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.commit_active_rename(window, cx) {
            self.focus_explorer(window);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_cancel(
        &mut self,
        _: &RenameCancel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_active_rename();
        self.focus_explorer(window);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_backspace(
        &mut self,
        _: &RenameBackspace,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            if rename.selected_range.is_empty() {
                rename.select_to(rename.previous_boundary(rename.cursor_offset()));
            }
            replace_rename_text(rename, None, "");
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_delete(
        &mut self,
        _: &RenameDelete,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            if rename.selected_range.is_empty() {
                rename.select_to(rename.next_boundary(rename.cursor_offset()));
            }
            replace_rename_text(rename, None, "");
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_left(
        &mut self,
        _: &RenameLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            if rename.selected_range.is_empty() {
                rename.move_to(rename.previous_boundary(rename.cursor_offset()));
            } else {
                rename.move_to(rename.selected_range.start);
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_right(
        &mut self,
        _: &RenameRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            if rename.selected_range.is_empty() {
                rename.move_to(rename.next_boundary(rename.cursor_offset()));
            } else {
                rename.move_to(rename.selected_range.end);
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_select_left(
        &mut self,
        _: &RenameSelectLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            rename.select_to(rename.previous_boundary(rename.cursor_offset()));
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_select_right(
        &mut self,
        _: &RenameSelectRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            rename.select_to(rename.next_boundary(rename.cursor_offset()));
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_word_left(
        &mut self,
        _: &RenameWordLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            rename.move_to(rename.previous_word_boundary(rename.cursor_offset()));
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_word_right(
        &mut self,
        _: &RenameWordRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            rename.move_to(rename.next_word_boundary(rename.cursor_offset()));
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_select_word_left(
        &mut self,
        _: &RenameSelectWordLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            rename.select_to(rename.previous_word_boundary(rename.cursor_offset()));
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_select_word_right(
        &mut self,
        _: &RenameSelectWordRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            rename.select_to(rename.next_word_boundary(rename.cursor_offset()));
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_home(
        &mut self,
        _: &RenameHome,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            rename.move_to(0);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_end(
        &mut self,
        _: &RenameEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            rename.move_to(rename.content.len());
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_select_home(
        &mut self,
        _: &RenameSelectHome,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            rename.select_to(0);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_select_end(
        &mut self,
        _: &RenameSelectEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            rename.select_to(rename.content.len());
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_select_all(
        &mut self,
        _: &RenameSelectAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            rename.select_all();
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_copy(
        &mut self,
        _: &RenameCopy,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = self.selected_rename_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_cut(
        &mut self,
        _: &RenameCut,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = self.selected_rename_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            if let Some(rename) = self.active_rename.as_mut() {
                replace_rename_text(rename, None, "");
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_paste(
        &mut self,
        _: &RenamePaste,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
            && let Some(rename) = self.active_rename.as_mut()
        {
            replace_rename_text(rename, None, &text.replace(['\r', '\n'], " "));
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_noop(
        &mut self,
        _: &RenameNoop,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
    }

    pub(super) fn rename_is_active_for_path(&self, path: &Path) -> bool {
        self.active_rename
            .as_ref()
            .is_some_and(|rename| rename.original_path == path)
    }

    pub(super) fn active_rename_focus_handle(&self) -> Option<FocusHandle> {
        self.active_rename
            .as_ref()
            .and_then(|rename| rename.focus_handle.clone())
    }

    pub(super) fn can_start_selected_rename(&self) -> bool {
        self.selection.selected_indices.len() == 1
    }

    pub(super) fn can_start_rename_from_name_click(
        &self,
        ix: usize,
        click_count: usize,
        modifiers: SelectionModifiers,
    ) -> bool {
        click_count == 1
            && !modifiers.toggle
            && !modifiers.extend
            && self.selection.selected_indices.len() == 1
            && self.selection.selected_indices.contains(&ix)
    }

    pub(super) fn handle_entry_name_click(
        &mut self,
        entry: &FileEntry,
        click_count: usize,
        modifiers: SelectionModifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<crate::explorer::navigation::EntryAction> {
        if self.active_rename.is_some() && !self.rename_is_active_for_path(&entry.path) {
            if !self.commit_active_rename(window, cx) {
                return None;
            }
        }

        let ix = self.entry_index_by_path(&entry.path);
        if let Some(ix) = ix
            && self.can_start_rename_from_name_click(ix, click_count, modifiers)
        {
            self.start_rename_for_entry(entry.clone(), Some(cx.focus_handle()), window, cx);
            return None;
        }

        self.handle_entry_click(entry, click_count, modifiers)
    }

    pub(super) fn commit_active_rename_before_interaction(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.active_rename.is_some() {
            self.commit_active_rename(window, cx)
        } else {
            true
        }
    }

    pub(super) fn start_rename_selected(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(ix) = self.selection.selected_indices.iter().next().copied() else {
            return false;
        };

        if self.selection.selected_indices.len() != 1 {
            return false;
        }

        let Some(entry) = self.entries.get(ix).cloned() else {
            return false;
        };

        self.start_rename_for_entry(entry, Some(cx.focus_handle()), window, cx);
        true
    }

    pub(super) fn start_rename_for_path(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(entry) = self
            .entry_index_by_path(path)
            .and_then(|ix| self.entries.get(ix))
            .cloned()
        else {
            return false;
        };

        self.start_rename_for_entry(entry, Some(cx.focus_handle()), window, cx);
        true
    }

    #[cfg(test)]
    pub(super) fn start_test_rename_for_index(&mut self, ix: usize) -> bool {
        let Some(entry) = self.entries.get(ix).cloned() else {
            return false;
        };
        self.start_rename_for_entry_without_focus(entry);
        true
    }

    #[cfg(test)]
    fn start_rename_for_entry_without_focus(&mut self, entry: FileEntry) {
        self.rename_focus_out = None;
        self.active_rename = Some(RenameState::new(
            &entry,
            self.show_file_name_extensions,
            None,
        ));
        self.open_error = None;
    }

    fn start_rename_for_entry(
        &mut self,
        entry: FileEntry,
        focus_handle: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rename_focus_out = None;
        self.active_rename = Some(RenameState::new(
            &entry,
            self.show_file_name_extensions,
            focus_handle.clone(),
        ));
        self.open_error = None;

        if let Some(focus_handle) = focus_handle {
            focus_handle.focus(window);
            let subscription = cx.on_focus_out(&focus_handle, window, |this, _, window, cx| {
                this.commit_active_rename(window, cx);
                cx.notify();
            });
            self.rename_focus_out = Some(subscription);
        }
    }

    pub(super) fn cancel_active_rename(&mut self) {
        self.rename_focus_out = None;
        self.active_rename = None;
        self.open_error = None;
    }

    fn commit_active_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let will_change_filesystem = self
            .active_rename
            .as_ref()
            .is_some_and(|rename| rename.target_file_name() != rename.original_name);
        let committed = self.apply_active_rename_commit();
        if committed && will_change_filesystem {
            self.emit_filesystem_changed(cx);
        }
        if !committed {
            self.refocus_rename_input(window);
            cx.notify();
        }
        committed
    }

    fn apply_active_rename_commit(&mut self) -> bool {
        let Some(rename) = self.active_rename.as_ref() else {
            return true;
        };

        let target_name = rename.target_file_name();
        if let Err(error) = validate_rename_text(&rename.content) {
            self.open_error = Some(error);
            return false;
        }

        if target_name == rename.original_name {
            self.rename_focus_out = None;
            self.active_rename = None;
            self.open_error = None;
            return true;
        }

        let original_path = rename.original_path.clone();
        let target_path = original_path.with_file_name(&target_name);

        match rename_path(&original_path, &target_path) {
            Ok(()) => {
                self.rename_focus_out = None;
                self.active_rename = None;
                self.open_error = None;
                self.remove_cut_paths(&[original_path]);
                self.reload();
                self.select_single_path(&target_path);
                true
            }
            Err(error) => {
                let source = original_path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| original_path.display().to_string());
                self.open_error = Some(format!(
                    "Could not rename \"{source}\" to \"{target_name}\": {error}"
                ));
                false
            }
        }
    }

    fn refocus_rename_input(&self, window: &mut Window) {
        if let Some(focus_handle) = self.active_rename_focus_handle() {
            focus_handle.focus(window);
        }
    }

    fn focus_explorer(&self, window: &mut Window) {
        if let Some(focus_handle) = self.focus_handle.as_ref() {
            focus_handle.focus(window);
        }
    }

    fn selected_rename_text(&self) -> Option<String> {
        let rename = self.active_rename.as_ref()?;
        (!rename.selected_range.is_empty())
            .then(|| rename.content[rename.selected_range.clone()].to_owned())
    }

    pub(super) fn on_rename_mouse_down(&mut self, event: &MouseDownEvent) {
        let Some(rename) = self.active_rename.as_mut() else {
            return;
        };

        rename.is_selecting = true;
        if event.modifiers.shift {
            rename.select_to(rename.index_for_mouse_position(event.position));
        } else {
            rename.move_to(rename.index_for_mouse_position(event.position));
        }
    }

    pub(super) fn on_rename_mouse_move(&mut self, event: &MouseMoveEvent) {
        let Some(rename) = self.active_rename.as_mut() else {
            return;
        };

        if rename.is_selecting {
            rename.select_to(rename.index_for_mouse_position(event.position));
        }
    }

    pub(super) fn on_rename_mouse_up(&mut self, _: &MouseUpEvent) {
        if let Some(rename) = self.active_rename.as_mut() {
            rename.is_selecting = false;
        }
    }

    fn update_rename_layout(&mut self, line: ShapedLine, bounds: Bounds<Pixels>) {
        if let Some(rename) = self.active_rename.as_mut() {
            rename.last_layout = Some(line);
            rename.last_bounds = Some(bounds);
            rename.scroll_cursor_into_view();
        }
    }
}

impl EntityInputHandler for ExplorerView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let rename = self.active_rename.as_ref()?;
        let range = rename.range_from_utf16(&range_utf16);
        actual_range.replace(rename.range_to_utf16(&range));
        Some(rename.content[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let rename = self.active_rename.as_ref()?;
        Some(UTF16Selection {
            range: rename.range_to_utf16(&rename.selected_range),
            reversed: rename.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let rename = self.active_rename.as_ref()?;
        rename
            .marked_range
            .as_ref()
            .map(|range| rename.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        if let Some(rename) = self.active_rename.as_mut() {
            rename.marked_range = None;
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            let range = range_utf16
                .as_ref()
                .map(|range_utf16| rename.range_from_utf16(range_utf16))
                .or(rename.marked_range.clone());
            replace_rename_text(rename, range, &text.replace(['\r', '\n'], " "));
            cx.notify();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            let range = range_utf16
                .as_ref()
                .map(|range_utf16| rename.range_from_utf16(range_utf16))
                .or(rename.marked_range.clone())
                .unwrap_or_else(|| rename.selected_range.clone());
            let new_text = new_text.replace(['\r', '\n'], " ");
            rename
                .content
                .replace_range(range.clone(), new_text.as_str());
            if new_text.is_empty() {
                rename.marked_range = None;
            } else {
                rename.marked_range = Some(range.start..range.start + new_text.len());
            }
            rename.selected_range = new_selected_range_utf16
                .as_ref()
                .map(|range_utf16| rename.range_from_utf16(range_utf16))
                .map(|new_range| new_range.start + range.start..new_range.end + range.start)
                .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
            rename.selection_reversed = false;
            rename.scroll_cursor_into_view();
            cx.notify();
        }
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let rename = self.active_rename.as_ref()?;
        let line = rename.last_layout.as_ref()?;
        let range = rename.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + line.x_for_index(range.start) - rename.scroll_offset,
                bounds.top(),
            ),
            point(
                bounds.left() + line.x_for_index(range.end) - rename.scroll_offset,
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let rename = self.active_rename.as_ref()?;
        let line_point = rename.last_bounds?.localize(&point)?;
        let line = rename.last_layout.as_ref()?;
        let utf8_index = line.index_for_x(point.x - line_point.x)?;
        Some(rename.offset_to_utf16(rename.clamp_to_boundary(utf8_index)))
    }
}

pub(super) struct RenameTextElement {
    entity: Entity<ExplorerView>,
}

pub(super) fn rename_text_element(entity: Entity<ExplorerView>) -> RenameTextElement {
    RenameTextElement { entity }
}

pub(super) struct RenamePrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    scroll_offset: Pixels,
}

impl IntoElement for RenameTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for RenameTextElement {
    type RequestLayoutState = ();
    type PrepaintState = RenamePrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let view = self.entity.read(cx);
        let rename = view
            .active_rename
            .as_ref()
            .expect("rename text element is only rendered during rename");
        let content = gpui::SharedString::from(rename.content.clone());
        let selected_range = rename.selected_range.clone();
        let cursor = rename.cursor_offset();
        let style = window.text_style();
        let run = TextRun {
            len: content.len(),
            font: style.font(),
            color: style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = rename.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: content.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect::<Vec<_>>()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(content, font_size, &runs, None);
        let scroll_offset = scroll_offset_for_cursor(
            rename.scroll_offset,
            line.x_for_index(cursor),
            line.width,
            bounds.right() - bounds.left(),
        );
        let cursor_pos = line.x_for_index(cursor);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos - scroll_offset, bounds.top()),
                        size(px(1.0), bounds.bottom() - bounds.top()),
                    ),
                    gpui::blue(),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected_range.start) - scroll_offset,
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end) - scroll_offset,
                            bounds.bottom(),
                        ),
                    ),
                    rgb(0x0078d7),
                )),
                None,
            )
        };

        RenamePrepaintState {
            line: Some(line),
            cursor,
            selection,
            scroll_offset,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(focus_handle) = self.entity.read(cx).active_rename_focus_handle() {
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, self.entity.clone()),
                cx,
            );
        }

        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        let line = prepaint.line.take().expect("rename text line");
        line.paint(
            point(bounds.origin.x - prepaint.scroll_offset, bounds.origin.y),
            window.line_height(),
            window,
            cx,
        )
        .expect("paint rename text");

        if let Some(focus_handle) = self.entity.read(cx).active_rename_focus_handle()
            && focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.entity.update(cx, |view, _| {
            view.update_rename_layout(line, bounds);
        });
    }
}

fn replace_rename_text(rename: &mut RenameState, range: Option<Range<usize>>, new_text: &str) {
    let range = range
        .or(rename.marked_range.clone())
        .unwrap_or_else(|| rename.selected_range.clone());
    rename.content.replace_range(range.clone(), new_text);
    let cursor = range.start + new_text.len();
    rename.selected_range = cursor..cursor;
    rename.selection_reversed = false;
    rename.marked_range = None;
    rename.scroll_cursor_into_view();
}

fn scroll_offset_for_cursor(
    current_offset: Pixels,
    cursor_x: Pixels,
    content_width: Pixels,
    viewport_width: Pixels,
) -> Pixels {
    let margin = px(4.0);
    if content_width <= viewport_width {
        return px(0.0);
    }

    let max_offset = (content_width - viewport_width + margin).max(px(0.0));
    let left_edge = current_offset + margin;
    let right_edge = current_offset + viewport_width - margin;

    if cursor_x < left_edge {
        (cursor_x - margin).max(px(0.0))
    } else if cursor_x > right_edge {
        (cursor_x - viewport_width + margin).clamp(px(0.0), max_offset)
    } else {
        current_offset.clamp(px(0.0), max_offset)
    }
}

fn rename_text_x_for_mouse_x(
    mouse_x: Pixels,
    bounds_left: Pixels,
    scroll_offset: Pixels,
) -> Pixels {
    mouse_x - bounds_left + scroll_offset
}

fn hidden_rename_suffix(entry: &FileEntry, show_file_name_extensions: bool) -> Option<String> {
    if let Some(suffix_start) = entry.name.len().checked_sub(4)
        && let Some(suffix) = entry.name.get(suffix_start..)
        && (suffix.eq_ignore_ascii_case(".lnk")
            || (entry.is_app_bundle() && suffix.eq_ignore_ascii_case(".app")))
    {
        return Some(suffix.to_owned());
    }

    if !show_file_name_extensions && !entry.is_directory_like() {
        match entry.name.rfind('.') {
            Some(0) | None => None,
            Some(dot) => Some(entry.name[dot..].to_owned()),
        }
    } else {
        None
    }
}

fn initial_rename_selection(
    entry: &FileEntry,
    display_name: &str,
    has_hidden_suffix: bool,
) -> Range<usize> {
    if entry.is_directory_like() || has_hidden_suffix {
        return 0..display_name.len();
    }

    match display_name.rfind('.') {
        Some(0) | None => 0..display_name.len(),
        Some(dot) => 0..dot,
    }
}

fn validate_rename_text(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Err("The file name cannot be empty.".to_owned());
    }

    if matches!(text, "." | "..") {
        return Err("The file name is not valid.".to_owned());
    }

    if text.chars().any(|ch| {
        matches!(
            ch,
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\0'
        )
    }) {
        return Err("The file name contains invalid characters.".to_owned());
    }

    Ok(())
}

fn rename_path(original_path: &Path, target_path: &Path) -> io::Result<()> {
    if destination_conflicts_with_existing_file(original_path, target_path) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "an item with this name already exists",
        ));
    }

    fs::rename(original_path, target_path)
}

fn destination_conflicts_with_existing_file(original_path: &Path, target_path: &Path) -> bool {
    if !target_path.exists() {
        return false;
    }

    let same_file = fs::canonicalize(original_path)
        .ok()
        .zip(fs::canonicalize(target_path).ok())
        .is_some_and(|(original, target)| original == target);
    !same_file
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::{
        entry::FileEntry,
        test_support::{TempDir, selected_names, test_view_with_entries},
    };
    use std::fs;

    #[test]
    fn selected_rename_requires_exactly_one_selected_entry() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);

        assert!(!view.can_start_selected_rename());

        view.select_single_index(0);
        assert!(view.can_start_selected_rename());

        view.select_all_entries();
        assert!(!view.can_start_selected_rename());
    }

    #[test]
    fn name_click_rename_requires_already_single_selected_entry() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        let plain_click = SelectionModifiers::default();

        assert!(!view.can_start_rename_from_name_click(0, 1, plain_click));

        view.select_single_index(0);
        assert!(view.can_start_rename_from_name_click(0, 1, plain_click));
        assert!(!view.can_start_rename_from_name_click(1, 1, plain_click));
        assert!(!view.can_start_rename_from_name_click(0, 2, plain_click));
        assert!(!view.can_start_rename_from_name_click(
            0,
            1,
            SelectionModifiers {
                toggle: true,
                extend: false,
            },
        ));
    }

    #[test]
    fn target_name_preserves_hidden_suffixes() {
        let shortcut = RenameState::new(
            &FileEntry::test("target.LNK", false, Some(1), None),
            true,
            None,
        );
        let app = RenameState::new(
            &FileEntry::test("Preview.app", true, None, None),
            true,
            None,
        );
        let file = RenameState::new(
            &FileEntry::test("readme.md", false, Some(1), None),
            true,
            None,
        );

        let mut shortcut = shortcut;
        shortcut.content = "renamed".to_owned();
        assert_eq!(shortcut.target_file_name(), "renamed.LNK");

        let mut app = app;
        app.content = "Terminal".to_owned();
        assert_eq!(app.target_file_name(), "Terminal.app");

        let mut file = file;
        file.content = "notes.txt".to_owned();
        assert_eq!(file.target_file_name(), "notes.txt");
    }

    #[test]
    fn target_name_preserves_normal_extension_when_extensions_are_hidden() {
        let mut file = RenameState::new(
            &FileEntry::test("readme.md", false, Some(1), None),
            false,
            None,
        );
        file.content = "notes".to_owned();

        let mut short_extension =
            RenameState::new(&FileEntry::test("a.b", false, Some(1), None), false, None);
        short_extension.content = "c".to_owned();

        let dotfile = RenameState::new(
            &FileEntry::test(".gitignore", false, Some(1), None),
            false,
            None,
        );

        assert_eq!(file.content, "notes");
        assert_eq!(file.target_file_name(), "notes.md");
        assert_eq!(short_extension.target_file_name(), "c.b");
        assert_eq!(dotfile.content, ".gitignore");
        assert_eq!(dotfile.target_file_name(), ".gitignore");
    }

    #[test]
    fn initial_selection_selects_stem_for_files_and_all_for_hidden_suffixes() {
        let file = RenameState::new(
            &FileEntry::test("archive.tar.gz", false, Some(1), None),
            true,
            None,
        );
        let folder = RenameState::new(&FileEntry::test("folder", true, None, None), true, None);
        let extensionless =
            RenameState::new(&FileEntry::test("README", false, Some(1), None), true, None);
        let dotfile = RenameState::new(
            &FileEntry::test(".gitignore", false, Some(1), None),
            true,
            None,
        );
        let shortcut = RenameState::new(
            &FileEntry::test("target.lnk", false, Some(1), None),
            true,
            None,
        );

        assert_eq!(file.selected_range, 0.."archive.tar".len());
        assert_eq!(folder.selected_range, 0.."folder".len());
        assert_eq!(extensionless.selected_range, 0.."README".len());
        assert_eq!(dotfile.selected_range, 0..".gitignore".len());
        assert_eq!(shortcut.selected_range, 0.."target".len());
    }

    #[test]
    fn word_boundaries_skip_spaces_punctuation_and_extensions() {
        let mut rename = RenameState::new(
            &FileEntry::test("hello world.txt", false, Some(1), None),
            true,
            None,
        );
        rename.content = "hello world.txt".to_owned();

        assert_eq!(rename.next_word_boundary(0), "hello ".len());
        assert_eq!(
            rename.next_word_boundary("hello ".len()),
            "hello world.".len()
        );
        assert_eq!(
            rename.previous_word_boundary("hello world.txt".len()),
            "hello world.".len()
        );
        assert_eq!(
            rename.previous_word_boundary("hello world.".len()),
            "hello ".len()
        );
    }

    #[test]
    fn word_boundaries_handle_punctuation_and_unicode_safely() {
        let mut rename = RenameState::new(
            &FileEntry::test("file-name café.txt", false, Some(1), None),
            true,
            None,
        );
        rename.content = "file-name café.txt".to_owned();

        assert_eq!(rename.next_word_boundary(0), "file-".len());
        assert_eq!(rename.next_word_boundary("file-".len()), "file-name ".len());
        assert_eq!(
            rename.next_word_boundary("file-name ".len()),
            "file-name café.".len()
        );
        assert_eq!(
            rename.previous_word_boundary("file-name café.".len()),
            "file-name ".len()
        );

        let inside_multi_byte = "file-name ca".len() + 1;
        assert_eq!(
            rename.previous_word_boundary(inside_multi_byte),
            "file-name ".len()
        );
    }

    #[test]
    fn shift_home_and_end_select_rename_text_without_changing_file_selection() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        view.select_single_index(0);
        assert!(view.start_test_rename_for_index(0));

        let rename = view.active_rename.as_mut().unwrap();
        rename.content = "alpha beta.txt".to_owned();
        rename.move_to("alpha ".len());
        rename.select_to(0);
        assert_eq!(rename.selected_range, 0.."alpha ".len());
        assert_eq!(selected_names(&view), vec!["a.txt"]);

        let rename = view.active_rename.as_mut().unwrap();
        rename.move_to("alpha ".len());
        rename.select_to(rename.content.len());
        assert_eq!(
            rename.selected_range,
            "alpha ".len().."alpha beta.txt".len()
        );
        assert_eq!(selected_names(&view), vec!["a.txt"]);
    }

    #[test]
    fn ctrl_left_and_right_move_by_word() {
        let mut rename = RenameState::new(
            &FileEntry::test("alpha beta.txt", false, Some(1), None),
            true,
            None,
        );
        rename.content = "alpha beta.txt".to_owned();

        rename.move_to(0);
        rename.move_to(rename.next_word_boundary(rename.cursor_offset()));
        assert_eq!(rename.selected_range, "alpha ".len().."alpha ".len());

        rename.move_to(rename.next_word_boundary(rename.cursor_offset()));
        assert_eq!(
            rename.selected_range,
            "alpha beta.".len().."alpha beta.".len()
        );

        rename.move_to(rename.previous_word_boundary(rename.cursor_offset()));
        assert_eq!(rename.selected_range, "alpha ".len().."alpha ".len());
    }

    #[test]
    fn ctrl_shift_left_and_right_extend_selection_by_word() {
        let mut rename = RenameState::new(
            &FileEntry::test("alpha beta.txt", false, Some(1), None),
            true,
            None,
        );
        rename.content = "alpha beta.txt".to_owned();

        rename.move_to(0);
        rename.select_to(rename.next_word_boundary(rename.cursor_offset()));
        assert_eq!(rename.selected_range, 0.."alpha ".len());

        rename.move_to("alpha beta.".len());
        rename.select_to(rename.previous_word_boundary(rename.cursor_offset()));
        assert_eq!(rename.selected_range, "alpha ".len().."alpha beta.".len());
    }

    #[test]
    fn rename_scroll_does_not_scroll_when_text_fits() {
        assert_eq!(
            scroll_offset_for_cursor(px(12.0), px(40.0), px(90.0), px(100.0)),
            px(0.0)
        );
    }

    #[test]
    fn rename_scroll_moves_right_to_keep_cursor_visible() {
        assert_eq!(
            scroll_offset_for_cursor(px(0.0), px(140.0), px(200.0), px(100.0)),
            px(44.0)
        );
    }

    #[test]
    fn rename_scroll_moves_left_to_keep_cursor_visible() {
        assert_eq!(
            scroll_offset_for_cursor(px(80.0), px(20.0), px(200.0), px(100.0)),
            px(16.0)
        );
    }

    #[test]
    fn rename_scroll_clamps_at_zero_and_maximum() {
        assert_eq!(
            scroll_offset_for_cursor(px(40.0), px(2.0), px(200.0), px(100.0)),
            px(0.0)
        );
        assert_eq!(
            scroll_offset_for_cursor(px(0.0), px(220.0), px(200.0), px(100.0)),
            px(104.0)
        );
    }

    #[test]
    fn rename_scroll_uses_active_selection_end() {
        let mut rename = RenameState::new(
            &FileEntry::test("alpha beta gamma.txt", false, Some(1), None),
            true,
            None,
        );
        rename.content = "alpha beta gamma.txt".to_owned();
        rename.selected_range = 0.."alpha beta gamma".len();
        rename.selection_reversed = false;
        assert_eq!(rename.cursor_offset(), "alpha beta gamma".len());

        rename.selection_reversed = true;
        assert_eq!(rename.cursor_offset(), 0);
    }

    #[test]
    fn mouse_text_x_includes_scroll_offset() {
        assert_eq!(
            rename_text_x_for_mouse_x(px(60.0), px(20.0), px(80.0)),
            px(120.0)
        );
    }

    #[test]
    fn validation_rejects_invalid_names() {
        for name in ["", ".", "..", "a/b", "a\\b", "a:b", "a*b", "a?b", "a|b"] {
            assert!(validate_rename_text(name).is_err(), "{name:?}");
        }

        assert!(validate_rename_text("valid name.txt").is_ok());
    }

    #[test]
    fn successful_rename_reloads_and_selects_new_path() {
        let temp = TempDir::new();
        fs::write(temp.path().join("a.txt"), b"data").expect("write file");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&temp.path().join("a.txt"));
        assert!(view.start_test_rename_for_index(0));
        view.active_rename.as_mut().unwrap().content = "b.txt".to_owned();

        assert!(view.apply_active_rename_commit());

        assert!(temp.path().join("b.txt").exists());
        assert_eq!(selected_names(&view), vec!["b.txt"]);
        assert!(view.active_rename.is_none());
    }

    #[test]
    fn duplicate_rename_keeps_edit_active_and_reports_error() {
        let temp = TempDir::new();
        fs::write(temp.path().join("a.txt"), b"data").expect("write source");
        fs::write(temp.path().join("b.txt"), b"data").expect("write duplicate");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&temp.path().join("a.txt"));
        let ix = view
            .entry_index_by_path(&temp.path().join("a.txt"))
            .unwrap();
        assert!(view.start_test_rename_for_index(ix));
        view.active_rename.as_mut().unwrap().content = "b.txt".to_owned();

        assert!(!view.apply_active_rename_commit());

        assert!(temp.path().join("a.txt").exists());
        assert!(temp.path().join("b.txt").exists());
        assert!(view.active_rename.is_some());
        assert!(
            view.open_error
                .as_ref()
                .is_some_and(|error| error.contains("Could not rename"))
        );
    }
}
