use std::{
    fs, io,
    ops::{Deref, DerefMut, Range},
    path::{Path, PathBuf},
    time::Duration,
};

use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, GlobalElementId, IntoElement, LayoutId, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, Style, Task, TextRun,
    UTF16Selection, Window, fill, point, px, relative, rgb, size,
};

#[cfg(test)]
use crate::explorer::text_input::text_x_for_mouse_x;
use crate::explorer::{
    actions::{
        RenameBackspace, RenameBackspaceWord, RenameCancel, RenameCommit, RenameCopy, RenameCut,
        RenameDelete, RenameEnd, RenameHome, RenameLeft, RenameNoop, RenamePaste, RenameRight,
        RenameSelectAll, RenameSelectEnd, RenameSelectHome, RenameSelectLeft, RenameSelectRight,
        RenameSelectWordLeft, RenameSelectWordRight, RenameSelected, RenameWordLeft,
        RenameWordRight,
    },
    entry::FileEntry,
    selection::SelectionModifiers,
    text_input::{
        EDITABLE_TEXT_SELECTION_BACKGROUND, EditableTextState, editable_text_runs,
        scroll_offset_for_cursor,
    },
    view::ExplorerView,
};

const CLICK_RENAME_DELAY: Duration = Duration::from_millis(280);

#[derive(Clone)]
pub(super) struct RenameState {
    pub(super) original_path: PathBuf,
    original_name: String,
    hidden_suffix: Option<String>,
    text: EditableTextState,
    pub(super) focus_handle: Option<FocusHandle>,
}

pub(super) struct PendingClickRename {
    pub(super) path: PathBuf,
    pub(super) request_id: u64,
    pub(super) task: Task<()>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ActiveTextInput {
    Rename,
    Address,
    Search,
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
            text: EditableTextState::with_selection(content, selected_range),
            focus_handle,
        }
    }

    fn target_file_name(&self) -> String {
        match self.hidden_suffix.as_deref() {
            Some(suffix) => format!("{}{}", self.content, suffix),
            None => self.content.clone(),
        }
    }
}

impl Deref for RenameState {
    type Target = EditableTextState;

    fn deref(&self) -> &Self::Target {
        &self.text
    }
}

impl DerefMut for RenameState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.text
    }
}

impl ExplorerView {
    pub(super) fn handle_rename_selected(
        &mut self,
        _: &RenameSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_pending_click_rename();
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
        self.cancel_pending_click_rename();
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
                let offset = rename.previous_boundary(rename.cursor_offset());
                rename.select_to(offset);
            }
            replace_rename_text(rename, None, "");
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_rename_backspace_word(
        &mut self,
        _: &RenameBackspaceWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(rename) = self.active_rename.as_mut() {
            rename.delete_previous_word_or_selection();
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
                let offset = rename.next_boundary(rename.cursor_offset());
                rename.select_to(offset);
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
            let offset = if rename.selected_range.is_empty() {
                rename.previous_boundary(rename.cursor_offset())
            } else {
                rename.selected_range.start
            };
            rename.move_to(offset);
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
            let offset = if rename.selected_range.is_empty() {
                rename.next_boundary(rename.cursor_offset())
            } else {
                rename.selected_range.end
            };
            rename.move_to(offset);
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
            let offset = rename.previous_boundary(rename.cursor_offset());
            rename.select_to(offset);
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
            let offset = rename.next_boundary(rename.cursor_offset());
            rename.select_to(offset);
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
            let offset = rename.previous_word_boundary(rename.cursor_offset());
            rename.move_to(offset);
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
            let offset = rename.next_word_boundary(rename.cursor_offset());
            rename.move_to(offset);
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
            let offset = rename.previous_word_boundary(rename.cursor_offset());
            rename.select_to(offset);
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
            let offset = rename.next_word_boundary(rename.cursor_offset());
            rename.select_to(offset);
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
            let offset = rename.content.len();
            rename.move_to(offset);
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
            let offset = rename.content.len();
            rename.select_to(offset);
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

    fn active_text_input(&self) -> Option<ActiveTextInput> {
        if self.active_rename.is_some() {
            Some(ActiveTextInput::Rename)
        } else if self.active_address_bar.is_some() {
            Some(ActiveTextInput::Address)
        } else if self.search_is_editing() {
            Some(ActiveTextInput::Search)
        } else {
            None
        }
    }

    pub(super) fn has_active_text_input(&self) -> bool {
        self.active_text_input().is_some()
    }

    pub(super) fn active_text_input_is_selecting(&self) -> bool {
        self.active_rename
            .as_ref()
            .is_some_and(|rename| rename.is_selecting)
            || self
                .active_address_bar
                .as_ref()
                .is_some_and(|address| address.is_selecting)
            || self.search_is_editing() && self.search.is_selecting
    }

    pub(super) fn finish_active_input_for_pointer_interaction(
        &mut self,
        input: ActiveTextInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.active_text_input() != Some(input) || self.active_text_input_is_selecting() {
            return false;
        }

        if input == ActiveTextInput::Rename {
            self.finish_active_rename_on_focus_out(cx);
        } else if input == ActiveTextInput::Address {
            self.cancel_address_bar_edit();
        } else {
            self.finish_search_edit();
        }

        self.focus_explorer(window);
        true
    }

    pub(super) fn can_start_selected_rename(&self) -> bool {
        if self.selection.selected_indices.len() != 1 {
            return false;
        }
        let Some(entry) = self
            .selection
            .selected_indices
            .iter()
            .next()
            .and_then(|ix| self.entries.get(*ix))
        else {
            return false;
        };
        crate::explorer::explorer_fs::ExplorerFs::new().can_mutate(&entry.path)
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
            && self.entries.get(ix).is_some_and(|entry| {
                crate::explorer::explorer_fs::ExplorerFs::new().can_mutate(&entry.path)
            })
    }

    pub(super) fn cancel_pending_click_rename(&mut self) {
        if let Some(pending) = self.pending_click_rename.take() {
            drop(pending.task);
        }
    }

    fn schedule_click_rename_for_entry(
        &mut self,
        entry: FileEntry,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_pending_click_rename();

        let path = entry.path;
        let request_id = self.next_pending_click_rename_id;
        self.next_pending_click_rename_id = self.next_pending_click_rename_id.wrapping_add(1);

        let task_path = path.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(CLICK_RENAME_DELAY).await;

            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.complete_pending_click_rename(&task_path, request_id, window, cx) {
                        cx.notify();
                    }
                });
            });
        });

        self.pending_click_rename = Some(PendingClickRename {
            path,
            request_id,
            task,
        });
    }

    fn pending_click_rename_entry(&self, path: &Path, request_id: u64) -> Option<FileEntry> {
        let pending = self.pending_click_rename.as_ref()?;
        if pending.request_id != request_id || pending.path != path {
            return None;
        }

        if self.active_rename.is_some() {
            return None;
        }

        let ix = self.entry_index_by_path(path)?;
        if self.selection.selected_indices.len() != 1
            || !self.selection.selected_indices.contains(&ix)
        {
            return None;
        }

        self.entries.get(ix).cloned()
    }

    fn complete_pending_click_rename(
        &mut self,
        path: &Path,
        request_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let matches_pending = self
            .pending_click_rename
            .as_ref()
            .is_some_and(|pending| pending.request_id == request_id && pending.path == path);
        if !matches_pending {
            return false;
        }

        let entry = self.pending_click_rename_entry(path, request_id);
        self.cancel_pending_click_rename();

        let Some(entry) = entry else {
            return false;
        };

        self.start_rename_for_entry(entry, Some(cx.focus_handle()), window, cx);
        true
    }

    pub(super) fn handle_entry_name_click(
        &mut self,
        entry: &FileEntry,
        click_count: usize,
        modifiers: SelectionModifiers,
        directory_open_mode: crate::explorer::navigation::DirectoryOpenMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<crate::explorer::navigation::EntryAction> {
        if self.active_rename.is_some() && !self.rename_is_active_for_path(&entry.path) {
            self.finish_active_rename_on_focus_out(cx);
        }

        let mouse_down_selection = self.take_mouse_down_entry_selection(entry, modifiers);
        let was_selected_before_mouse_down = mouse_down_selection
            .as_ref()
            .is_some_and(|selection| selection.was_selected);
        let selection_was_applied_on_mouse_down = mouse_down_selection
            .as_ref()
            .is_some_and(|selection| selection.selection_applied);
        let had_mouse_down_selection = mouse_down_selection.is_some();

        let ix = self.entry_index_by_path(&entry.path);
        if let Some(ix) = ix
            && (!had_mouse_down_selection || was_selected_before_mouse_down)
            && self.can_start_rename_from_name_click(ix, click_count, modifiers)
        {
            self.schedule_click_rename_for_entry(entry.clone(), window, cx);
            return None;
        }

        if selection_was_applied_on_mouse_down {
            self.handle_entry_click_after_mouse_down_selection_with_watcher_and_directory_mode(
                entry,
                click_count,
                directory_open_mode,
                cx,
            )
        } else {
            self.handle_entry_click_with_watcher_and_directory_mode(
                entry,
                click_count,
                modifiers,
                directory_open_mode,
                cx,
            )
        }
    }

    pub(super) fn commit_active_rename_before_interaction(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.cancel_pending_click_rename();

        if self.active_rename.is_some() {
            self.finish_active_rename_on_focus_out(cx);
            true
        } else {
            true
        }
    }

    pub(super) fn start_rename_selected(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.cancel_pending_click_rename();

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

    #[cfg(test)]
    pub(super) fn start_test_rename_for_index(&mut self, ix: usize) -> bool {
        let Some(entry) = self.entries.get(ix).cloned() else {
            return false;
        };
        self.start_rename_for_entry_without_focus(entry);
        true
    }

    pub(super) fn start_rename_for_path_without_focus(&mut self, path: &Path) -> bool {
        self.cancel_pending_click_rename();

        let Some(entry) = self
            .entry_index_by_path(path)
            .and_then(|ix| self.entries.get(ix))
            .cloned()
        else {
            return false;
        };

        self.start_rename_for_entry_without_focus(entry);
        true
    }

    pub(super) fn start_rename_for_path(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.cancel_pending_click_rename();

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

    fn start_rename_for_entry_without_focus(&mut self, entry: FileEntry) {
        self.rename_focus_out = None;
        self.active_rename = Some(RenameState::new(
            &entry,
            self.show_file_name_extensions,
            None,
        ));
        self.clear_operation_notice();
    }

    fn start_rename_for_entry(
        &mut self,
        entry: FileEntry,
        focus_handle: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_search_edit();
        self.cancel_address_bar_edit();
        self.rename_focus_out = None;
        self.active_rename = Some(RenameState::new(
            &entry,
            self.show_file_name_extensions,
            focus_handle.clone(),
        ));
        self.clear_operation_notice();

        if let Some(focus_handle) = focus_handle {
            focus_handle.focus(window);
            let subscription = cx.on_focus_out(&focus_handle, window, |this, _, _, cx| {
                this.finish_active_rename_on_focus_out(cx);
                cx.notify();
            });
            self.rename_focus_out = Some(subscription);
        }
    }

    pub(super) fn cancel_active_rename(&mut self) {
        self.cancel_pending_click_rename();
        self.rename_focus_out = None;
        self.active_rename = None;
        self.clear_operation_notice();
    }

    fn commit_active_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let will_change_filesystem = self
            .active_rename
            .as_ref()
            .is_some_and(|rename| rename.target_file_name() != rename.original_name);
        let committed = self.apply_active_rename_commit_with_context(cx);
        if committed && will_change_filesystem {
            self.emit_filesystem_changed(cx);
        }
        if !committed {
            self.refocus_rename_input(window);
            cx.notify();
        }
        committed
    }

    fn finish_active_rename_on_focus_out(&mut self, cx: &mut Context<Self>) {
        let will_change_filesystem = self
            .active_rename
            .as_ref()
            .is_some_and(|rename| rename.target_file_name() != rename.original_name);
        let committed = self.apply_active_rename_commit_or_cancel_with_context(cx);
        if committed && will_change_filesystem {
            self.emit_filesystem_changed(cx);
        }
    }

    #[cfg(test)]
    fn apply_active_rename_commit_or_cancel(&mut self) -> bool {
        let committed = self.apply_active_rename_commit_inner(None, true);
        if !committed {
            self.cancel_active_rename();
        }
        committed
    }

    fn apply_active_rename_commit_or_cancel_with_context(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let committed = self.apply_active_rename_commit_inner(Some(cx), false);
        if !committed {
            self.cancel_active_rename();
        }
        committed
    }

    #[cfg(test)]
    fn apply_active_rename_commit(&mut self) -> bool {
        self.apply_active_rename_commit_inner(None, true)
    }

    fn apply_active_rename_commit_with_context(&mut self, cx: &mut Context<Self>) -> bool {
        self.apply_active_rename_commit_inner(Some(cx), true)
    }

    fn apply_active_rename_commit_inner(
        &mut self,
        cx: Option<&mut Context<Self>>,
        select_target_after_reload: bool,
    ) -> bool {
        let Some(rename) = self.active_rename.as_ref() else {
            return true;
        };

        let target_name = rename.target_file_name();
        if let Err(error) = validate_rename_text(&rename.content) {
            self.set_error_notice(error);
            return false;
        }

        if target_name == rename.original_name {
            self.rename_focus_out = None;
            self.active_rename = None;
            self.clear_operation_notice();
            return true;
        }

        let original_path = rename.original_path.clone();
        let target_path = original_path.with_file_name(&target_name);

        match rename_path(&original_path, &target_path) {
            Ok(()) => {
                self.rename_focus_out = None;
                self.active_rename = None;
                self.clear_operation_notice();
                self.remove_cut_paths(&[original_path]);
                if let Some(cx) = cx {
                    if select_target_after_reload {
                        self.reload_async_with_options(
                            crate::explorer::view::ReloadMode {
                                preserve_selection: true,
                                rebuild_sidebar: true,
                                preserve_context_menu: false,
                            },
                            vec![target_path],
                            true,
                            false,
                            false,
                            cx,
                        );
                    } else {
                        self.reload_async_with_options_preserving_live_selection(
                            crate::explorer::view::ReloadMode {
                                preserve_selection: true,
                                rebuild_sidebar: true,
                                preserve_context_menu: false,
                            },
                            Vec::new(),
                            true,
                            false,
                            false,
                            cx,
                        );
                    }
                } else {
                    self.reload();
                    self.select_single_path(&target_path);
                }
                true
            }
            Err(error) => {
                let source = original_path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| original_path.display().to_string());
                self.set_error_notice(format!(
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

    pub(super) fn focus_explorer(&self, window: &mut Window) {
        if let Some(focus_handle) = self.focus_handle.as_ref() {
            focus_handle.focus(window);
        }
    }

    fn selected_rename_text(&self) -> Option<String> {
        self.active_rename.as_ref()?.selected_text()
    }

    pub(super) fn on_rename_mouse_down(&mut self, event: &MouseDownEvent) {
        let Some(rename) = self.active_rename.as_mut() else {
            return;
        };

        rename.is_selecting = true;
        let offset = rename.index_for_mouse_position(event.position);
        if event.click_count >= 3 {
            rename.select_all();
        } else if event.click_count == 2 {
            rename.select_word_at(offset);
        } else if event.modifiers.shift {
            rename.select_to(offset);
        } else {
            rename.move_to(offset);
        }
    }

    pub(super) fn on_rename_mouse_move(&mut self, event: &MouseMoveEvent) {
        let Some(rename) = self.active_rename.as_mut() else {
            return;
        };

        if rename.is_selecting {
            let offset = rename.index_for_mouse_position(event.position);
            rename.select_to(offset);
        }
    }

    pub(super) fn on_rename_mouse_up(&mut self, _: &MouseUpEvent) {
        if let Some(rename) = self.active_rename.as_mut() {
            rename.is_selecting = false;
        }
    }

    fn update_rename_layout(&mut self, line: ShapedLine, bounds: Bounds<Pixels>) {
        if let Some(rename) = self.active_rename.as_mut() {
            rename.update_layout(line, bounds);
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
        if self.active_address_bar.is_some() {
            return self.address_text_for_range(range_utf16, actual_range);
        }
        if self.search_is_editing() {
            return self.search_text_for_range(range_utf16, actual_range);
        }

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
        if self.active_address_bar.is_some() {
            return self.selected_address_text_range();
        }
        if self.search_is_editing() {
            return Some(self.selected_search_text_range());
        }

        let rename = self.active_rename.as_ref()?;
        Some(rename.selected_text_range_utf16())
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        if self.active_address_bar.is_some() {
            return self.marked_address_text_range();
        }
        if self.search_is_editing() {
            return self.search.marked_text_range_utf16();
        }

        let rename = self.active_rename.as_ref()?;
        rename.marked_text_range_utf16()
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        if self.active_address_bar.is_some() {
            self.unmark_address_text();
            return;
        }
        if self.search_is_editing() {
            self.search.unmark_text();
            return;
        }

        if let Some(rename) = self.active_rename.as_mut() {
            rename.unmark_text();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_address_bar.is_some() {
            self.replace_address_text_in_range(range_utf16, text);
            cx.notify();
            return;
        }
        if self.search_is_editing() {
            self.replace_search_text_in_range(range_utf16, text, cx);
            cx.notify();
            return;
        }

        if let Some(rename) = self.active_rename.as_mut() {
            rename.replace_text_in_range_utf16(range_utf16, &text.replace(['\r', '\n'], " "));
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
        if self.active_address_bar.is_some() {
            self.replace_and_mark_address_text_in_range(
                range_utf16,
                new_text,
                new_selected_range_utf16,
            );
            cx.notify();
            return;
        }
        if self.search_is_editing() {
            self.replace_and_mark_search_text_in_range(
                range_utf16,
                new_text,
                new_selected_range_utf16,
                cx,
            );
            cx.notify();
            return;
        }

        if let Some(rename) = self.active_rename.as_mut() {
            let new_text = new_text.replace(['\r', '\n'], " ");
            rename.replace_and_mark_text_in_range_utf16(
                range_utf16,
                &new_text,
                new_selected_range_utf16,
            );
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
        if self.active_address_bar.is_some() {
            return self.address_bounds_for_range(range_utf16, bounds);
        }
        if self.search_is_editing() {
            return self.search.bounds_for_range(range_utf16, bounds);
        }

        let rename = self.active_rename.as_ref()?;
        rename.bounds_for_range(range_utf16, bounds)
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        if self.active_address_bar.is_some() {
            return self.address_character_index_for_point(point);
        }
        if self.search_is_editing() {
            return self.search.character_index_for_point(point);
        }

        let rename = self.active_rename.as_ref()?;
        rename.character_index_for_point(point)
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
        let runs = editable_text_runs(
            content.len(),
            run,
            &selected_range,
            rename.marked_range.as_ref(),
        );

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
                    rgb(EDITABLE_TEXT_SELECTION_BACKGROUND),
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
    rename.replace_text(range, new_text);
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
    rename_local_path(original_path, target_path)
}

fn rename_local_path(original_path: &Path, target_path: &Path) -> io::Result<()> {
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
        test_support::{TempDir, selected_names, test_view_entity, test_view_with_entries},
    };
    use gpui::{AppContext, ClipboardItem, MouseButton, TestAppContext};
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
    fn pending_click_rename_resolves_matching_selected_entry() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        view.select_single_index(0);
        let path = view.entries[0].path.clone();
        view.pending_click_rename = Some(PendingClickRename {
            path: path.clone(),
            request_id: 7,
            task: Task::ready(()),
        });

        let entry = view
            .pending_click_rename_entry(&path, 7)
            .expect("matching pending rename entry");

        assert_eq!(entry.path, path);
        assert!(view.active_rename.is_none());
    }

    #[test]
    fn pending_click_rename_rejects_stale_request_id_and_path() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        view.select_single_index(0);
        let path = view.entries[0].path.clone();
        let other_path = view.entries[1].path.clone();
        view.pending_click_rename = Some(PendingClickRename {
            path: path.clone(),
            request_id: 7,
            task: Task::ready(()),
        });

        assert!(view.pending_click_rename_entry(&path, 8).is_none());
        assert!(view.pending_click_rename_entry(&other_path, 7).is_none());
    }

    #[test]
    fn pending_click_rename_rejects_changed_selection() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        view.select_single_index(0);
        let path = view.entries[0].path.clone();
        view.pending_click_rename = Some(PendingClickRename {
            path: path.clone(),
            request_id: 7,
            task: Task::ready(()),
        });
        view.selection.selected_indices = std::collections::BTreeSet::from([1]);

        assert!(view.pending_click_rename_entry(&path, 7).is_none());
    }

    #[test]
    fn target_name_preserves_hidden_suffixes() {
        let shortcut = RenameState::new(
            &FileEntry::test("target.LNK", false, Some(1), None),
            true,
            None,
        );
        #[cfg(target_os = "macos")]
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

        #[cfg(target_os = "macos")]
        {
            let mut app = app;
            app.content = "Terminal".to_owned();
            assert_eq!(app.target_file_name(), "Terminal.app");
        }

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
    fn double_click_word_selection_selects_rename_word_at_offset() {
        let mut rename = RenameState::new(
            &FileEntry::test("alpha beta.txt", false, Some(1), None),
            true,
            None,
        );
        rename.content = "alpha beta.txt".to_owned();

        rename.select_word_at("al".len());
        assert_eq!(rename.selected_range, 0.."alpha".len());

        rename.select_word_at("alpha ".len());
        assert_eq!(rename.selected_range, "alpha ".len().."alpha beta".len());
    }

    #[test]
    fn triple_click_selection_selects_entire_rename_text() {
        let mut view = test_view_with_entries(&["alpha beta.txt"]);
        view.select_single_index(0);
        assert!(view.start_test_rename_for_index(0));

        view.on_rename_mouse_down(&MouseDownEvent {
            button: MouseButton::Left,
            click_count: 3,
            ..MouseDownEvent::default()
        });

        let rename = view.active_rename.as_ref().expect("rename edit");
        assert_eq!(rename.selected_range, 0..rename.content.len());
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
        let offset = rename.content.len();
        rename.select_to(offset);
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
        let offset = rename.next_word_boundary(rename.cursor_offset());
        rename.move_to(offset);
        assert_eq!(rename.selected_range, "alpha ".len().."alpha ".len());

        let offset = rename.next_word_boundary(rename.cursor_offset());
        rename.move_to(offset);
        assert_eq!(
            rename.selected_range,
            "alpha beta.".len().."alpha beta.".len()
        );

        let offset = rename.previous_word_boundary(rename.cursor_offset());
        rename.move_to(offset);
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
        let offset = rename.next_word_boundary(rename.cursor_offset());
        rename.select_to(offset);
        assert_eq!(rename.selected_range, 0.."alpha ".len());

        rename.move_to("alpha beta.".len());
        let offset = rename.previous_word_boundary(rename.cursor_offset());
        rename.select_to(offset);
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
        assert_eq!(text_x_for_mouse_x(px(60.0), px(20.0), px(80.0)), px(120.0));
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

    #[gpui::test]
    fn context_backed_rename_uses_async_reload_and_selects_new_path(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        fs::write(temp.path().join("a.txt"), b"data").expect("write file");
        let root = temp.path().to_path_buf();
        let original = root.join("a.txt");
        let target = root.join("b.txt");
        let (view, cx) = cx.add_window_view({
            let root = root.clone();
            move |window, cx| {
                let focus_handle = cx.focus_handle();
                focus_handle.focus(window);
                ExplorerView::new_with_focus_handle_for_test(root, focus_handle)
            }
        });

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                assert!(view.start_rename_for_path(&original, window, cx));
                view.active_rename.as_mut().unwrap().content = "b.txt".to_owned();

                assert!(view.apply_active_rename_commit_with_context(cx));

                assert!(target.exists());
                assert!(view.active_rename.is_none());
                assert_eq!(view.loading_path.as_deref(), Some(root.as_path()));
                assert!(view.directory_load_task.is_some());
            });
        });
        cx.run_until_parked();

        cx.read_entity(&view, |view, _| {
            assert!(view.directory_load_task.is_none());
            assert!(view.loading_path.is_none());
            assert_eq!(selected_names(view), vec!["b.txt"]);
        });
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
            view.operation_notice
                .as_ref()
                .is_some_and(|notice| notice.text.contains("Could not rename"))
        );
    }

    #[test]
    fn invalid_rename_submitted_explicitly_keeps_edit_active() {
        let temp = TempDir::new();
        fs::write(temp.path().join("a.txt"), b"data").expect("write file");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&temp.path().join("a.txt"));
        assert!(view.start_test_rename_for_index(0));
        view.active_rename.as_mut().unwrap().content.clear();

        assert!(!view.apply_active_rename_commit());

        assert!(temp.path().join("a.txt").exists());
        assert!(view.active_rename.is_some());
        assert_eq!(
            view.operation_notice
                .as_ref()
                .map(|notice| notice.text.as_str()),
            Some("The file name cannot be empty.")
        );
    }

    #[test]
    fn valid_rename_click_away_commits_and_allows_clicked_selection() {
        let temp = TempDir::new();
        fs::write(temp.path().join("a.txt"), b"data").expect("write source");
        fs::write(temp.path().join("c.txt"), b"data").expect("write target selection");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&temp.path().join("a.txt"));
        assert!(view.start_test_rename_for_index(0));
        view.active_rename.as_mut().unwrap().content = "b.txt".to_owned();
        let clicked_entry = view
            .entries
            .iter()
            .find(|entry| entry.path == temp.path().join("c.txt"))
            .unwrap()
            .clone();

        assert!(view.apply_active_rename_commit_or_cancel());
        view.handle_entry_click(&clicked_entry, 1, SelectionModifiers::default());

        assert!(temp.path().join("b.txt").exists());
        assert!(!temp.path().join("a.txt").exists());
        assert_eq!(selected_names(&view), vec!["c.txt"]);
        assert!(view.active_rename.is_none());
    }

    #[test]
    fn failed_rename_click_away_cancels_and_allows_clicked_selection() {
        let temp = TempDir::new();
        fs::write(temp.path().join("a.txt"), b"data").expect("write source");
        fs::write(temp.path().join("b.txt"), b"data").expect("write duplicate");
        fs::write(temp.path().join("c.txt"), b"data").expect("write target selection");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&temp.path().join("a.txt"));
        assert!(view.start_test_rename_for_index(0));
        view.active_rename.as_mut().unwrap().content = "b.txt".to_owned();
        let clicked_entry = view
            .entries
            .iter()
            .find(|entry| entry.path == temp.path().join("c.txt"))
            .unwrap()
            .clone();

        assert!(!view.apply_active_rename_commit_or_cancel());
        view.handle_entry_click(&clicked_entry, 1, SelectionModifiers::default());

        assert!(temp.path().join("a.txt").exists());
        assert!(temp.path().join("b.txt").exists());
        assert_eq!(selected_names(&view), vec!["c.txt"]);
        assert!(view.active_rename.is_none());
        assert!(view.operation_notice.is_none());
    }

    #[gpui::test]
    fn rename_action_handlers_edit_text_selection_clipboard_and_finish(cx: &mut TestAppContext) {
        let (temp, view, cx) = test_view_entity(cx, &["alpha beta.txt"]);
        let file = temp.path().join("alpha beta.txt");

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.select_single_path(&file);
                view.handle_rename_selected(&RenameSelected, window, cx);
                assert!(view.active_rename.is_some());

                set_rename_text(view, "alpha beta");
                view.handle_rename_left(&RenameLeft, window, cx);
                assert_eq!(
                    view.active_rename
                        .as_ref()
                        .expect("active rename")
                        .selected_range,
                    "alpha bet".len().."alpha bet".len()
                );
                view.handle_rename_right(&RenameRight, window, cx);
                assert_eq!(
                    view.active_rename
                        .as_ref()
                        .expect("active rename")
                        .selected_range,
                    "alpha beta".len().."alpha beta".len()
                );

                view.handle_rename_word_left(&RenameWordLeft, window, cx);
                assert_eq!(
                    view.active_rename
                        .as_ref()
                        .expect("active rename")
                        .selected_range,
                    "alpha ".len().."alpha ".len()
                );
                view.handle_rename_word_right(&RenameWordRight, window, cx);
                assert_eq!(
                    view.active_rename
                        .as_ref()
                        .expect("active rename")
                        .selected_range,
                    "alpha beta".len().."alpha beta".len()
                );

                set_rename_text(view, "alpha beta");
                view.handle_rename_select_left(&RenameSelectLeft, window, cx);
                assert_eq!(
                    view.active_rename
                        .as_ref()
                        .expect("active rename")
                        .selected_range,
                    "alpha bet".len().."alpha beta".len()
                );

                set_rename_text(view, "alpha beta");
                view.active_rename
                    .as_mut()
                    .expect("active rename")
                    .move_to(0);
                view.handle_rename_select_right(&RenameSelectRight, window, cx);
                assert_eq!(
                    view.active_rename
                        .as_ref()
                        .expect("active rename")
                        .selected_range,
                    0.."a".len()
                );

                set_rename_text(view, "alpha beta");
                view.handle_rename_select_word_left(&RenameSelectWordLeft, window, cx);
                assert_eq!(
                    view.active_rename
                        .as_ref()
                        .expect("active rename")
                        .selected_range,
                    "alpha ".len().."alpha beta".len()
                );

                set_rename_text(view, "alpha beta");
                view.active_rename
                    .as_mut()
                    .expect("active rename")
                    .move_to(0);
                view.handle_rename_select_word_right(&RenameSelectWordRight, window, cx);
                assert_eq!(
                    view.active_rename
                        .as_ref()
                        .expect("active rename")
                        .selected_range,
                    0.."alpha ".len()
                );

                view.handle_rename_home(&RenameHome, window, cx);
                assert_eq!(
                    view.active_rename
                        .as_ref()
                        .expect("active rename")
                        .selected_range,
                    0..0
                );
                view.handle_rename_end(&RenameEnd, window, cx);
                assert_eq!(
                    view.active_rename
                        .as_ref()
                        .expect("active rename")
                        .selected_range,
                    "alpha beta".len().."alpha beta".len()
                );
                view.handle_rename_select_home(&RenameSelectHome, window, cx);
                assert_eq!(
                    view.active_rename
                        .as_ref()
                        .expect("active rename")
                        .selected_range,
                    0.."alpha beta".len()
                );

                set_rename_text(view, "alpha beta");
                view.active_rename
                    .as_mut()
                    .expect("active rename")
                    .move_to(0);
                view.handle_rename_select_end(&RenameSelectEnd, window, cx);
                assert_eq!(
                    view.active_rename
                        .as_ref()
                        .expect("active rename")
                        .selected_range,
                    0.."alpha beta".len()
                );

                view.handle_rename_select_all(&RenameSelectAll, window, cx);
                assert_eq!(
                    view.active_rename
                        .as_ref()
                        .expect("active rename")
                        .selected_text()
                        .as_deref(),
                    Some("alpha beta")
                );

                set_rename_text(view, "alpha beta");
                view.active_rename
                    .as_mut()
                    .expect("active rename")
                    .selected_range = 0.."alpha".len();
                view.handle_rename_copy(&RenameCopy, window, cx);
                assert_eq!(
                    cx.read_from_clipboard().and_then(|item| item.text()),
                    Some("alpha".to_owned())
                );

                view.handle_rename_cut(&RenameCut, window, cx);
                assert_eq!(
                    view.active_rename.as_ref().expect("active rename").content,
                    " beta"
                );

                cx.write_to_clipboard(ClipboardItem::new_string("gamma\nname".to_owned()));
                view.handle_rename_paste(&RenamePaste, window, cx);
                assert_eq!(
                    view.active_rename.as_ref().expect("active rename").content,
                    "gamma name beta"
                );

                set_rename_text(view, "alpha");
                view.handle_rename_backspace(&RenameBackspace, window, cx);
                assert_eq!(
                    view.active_rename.as_ref().expect("active rename").content,
                    "alph"
                );

                set_rename_text(view, "alpha");
                view.active_rename
                    .as_mut()
                    .expect("active rename")
                    .move_to(0);
                view.handle_rename_delete(&RenameDelete, window, cx);
                assert_eq!(
                    view.active_rename.as_ref().expect("active rename").content,
                    "lpha"
                );

                set_rename_text(view, "alpha beta");
                view.handle_rename_backspace_word(&RenameBackspaceWord, window, cx);
                assert_eq!(
                    view.active_rename.as_ref().expect("active rename").content,
                    "alpha "
                );

                view.handle_rename_noop(&RenameNoop, window, cx);

                let original_name = view
                    .active_rename
                    .as_ref()
                    .expect("active rename")
                    .original_name
                    .clone();
                set_rename_text(view, &original_name);
                view.handle_rename_commit(&RenameCommit, window, cx);
                assert!(view.active_rename.is_none());

                view.select_single_path(&file);
                view.handle_rename_selected(&RenameSelected, window, cx);
                assert!(view.active_rename.is_some());
                view.handle_rename_cancel(&RenameCancel, window, cx);
                assert!(view.active_rename.is_none());
            });
        });
    }

    #[gpui::test]
    fn text_input_handler_routes_to_rename_address_and_search(cx: &mut TestAppContext) {
        let (_temp, view, cx) = test_view_entity(cx, &["alpha beta.txt", "notes.txt"]);

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.select_single_index(0);
                assert!(view.start_test_rename_for_index(0));

                let mut actual_range = None;
                assert_eq!(
                    <ExplorerView as EntityInputHandler>::text_for_range(
                        view,
                        0.."alpha".len(),
                        &mut actual_range,
                        window,
                        cx,
                    ),
                    Some("alpha".to_owned())
                );
                assert_eq!(actual_range, Some(0.."alpha".len()));

                <ExplorerView as EntityInputHandler>::replace_text_in_range(
                    view,
                    Some(0.."alpha".len()),
                    "omega\nname",
                    window,
                    cx,
                );
                assert!(
                    view.active_rename
                        .as_ref()
                        .expect("active rename")
                        .content
                        .starts_with("omega name")
                );

                <ExplorerView as EntityInputHandler>::replace_and_mark_text_in_range(
                    view,
                    Some(0.."omega".len()),
                    "delta",
                    Some(1..3),
                    window,
                    cx,
                );
                let rename = view.active_rename.as_ref().expect("active rename");
                assert_eq!(rename.marked_range, Some(0.."delta".len()));
                assert_eq!(rename.selected_range, 1..3);
                assert_eq!(
                    <ExplorerView as EntityInputHandler>::marked_text_range(view, window, cx),
                    Some(0.."delta".len())
                );
                <ExplorerView as EntityInputHandler>::unmark_text(view, window, cx);
                assert!(
                    view.active_rename
                        .as_ref()
                        .expect("active rename")
                        .marked_range
                        .is_none()
                );
                assert!(
                    <ExplorerView as EntityInputHandler>::bounds_for_range(
                        view,
                        0..1,
                        Bounds::default(),
                        window,
                        cx,
                    )
                    .is_none()
                );
                assert!(
                    <ExplorerView as EntityInputHandler>::character_index_for_point(
                        view,
                        point(px(0.0), px(0.0)),
                        window,
                        cx,
                    )
                    .is_none()
                );

                view.cancel_active_rename();
                assert!(view.start_address_bar_edit(window, cx));
                let address_len = view
                    .active_address_bar
                    .as_ref()
                    .expect("address edit")
                    .content
                    .len();
                <ExplorerView as EntityInputHandler>::replace_text_in_range(
                    view,
                    Some(0..address_len),
                    "folder\nname",
                    window,
                    cx,
                );
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .content,
                    "folder name"
                );
                let mut actual_range = None;
                assert_eq!(
                    <ExplorerView as EntityInputHandler>::text_for_range(
                        view,
                        0.."folder".len(),
                        &mut actual_range,
                        window,
                        cx,
                    ),
                    Some("folder".to_owned())
                );

                view.cancel_address_bar_edit();
                assert!(view.start_search_edit(window, cx));
                <ExplorerView as EntityInputHandler>::replace_text_in_range(
                    view,
                    Some(0..0),
                    "note\nquery",
                    window,
                    cx,
                );
                assert_eq!(view.search_query(), "note query");
                assert_eq!(view.entries.len(), 0);

                <ExplorerView as EntityInputHandler>::replace_and_mark_text_in_range(
                    view,
                    Some(0.."note".len()),
                    "notes",
                    Some(0..5),
                    window,
                    cx,
                );
                assert_eq!(view.search_query(), "notes query");
                assert_eq!(view.search.marked_range, Some(0.."notes".len()));
                assert_eq!(
                    <ExplorerView as EntityInputHandler>::selected_text_range(
                        view, false, window, cx,
                    )
                    .expect("selection")
                    .range,
                    0.."notes".len()
                );
            });
        });
    }

    fn set_rename_text(view: &mut ExplorerView, text: &str) {
        let rename = view.active_rename.as_mut().expect("active rename");
        rename.content = text.to_owned();
        rename.selected_range = text.len()..text.len();
        rename.selection_reversed = false;
        rename.marked_range = None;
    }
}
