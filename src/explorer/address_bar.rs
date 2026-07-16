use std::{
    env,
    ffi::OsString,
    fs,
    ops::{Deref, DerefMut, Range},
    path::{Path, PathBuf},
};

use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler, Entity,
    FocusHandle, GlobalElementId, IntoElement, LayoutId, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, Style, Subscription, TextRun,
    UTF16Selection, Window, fill, point, px, relative, rgb, size,
};

use crate::explorer::{
    actions::{
        AddressAcceptSuggestion, AddressBackspace, AddressBackspaceWord, AddressCancel,
        AddressCommit, AddressCopy, AddressCut, AddressDelete, AddressEdit, AddressEnd,
        AddressHome, AddressLeft, AddressPaste, AddressRight, AddressSelectAll, AddressSelectEnd,
        AddressSelectHome, AddressSelectLeft, AddressSelectRight, AddressSelectWordLeft,
        AddressSelectWordRight, AddressSuggestionDown, AddressSuggestionUp, AddressWordLeft,
        AddressWordRight,
    },
    filesystem::should_hide_directory_entry,
    navigation::HistoryMode,
    scrollbar::{ScrollbarDrag, ScrollbarMetrics},
    text_input::{
        EDITABLE_TEXT_SELECTION_BACKGROUND, EditableTextEditKind, EditableTextState,
        editable_text_runs, scroll_offset_for_cursor,
    },
    view::ExplorerView,
};
use crate::settings::AddressSlash;

pub(super) const ADDRESS_SUGGESTION_ROW_HEIGHT: f32 = 30.0;
pub(super) const ADDRESS_SUGGESTION_VISIBLE_ROWS: usize = 10;
pub(super) const ADDRESS_SUGGESTIONS_VERTICAL_PADDING: f32 = 4.0;

pub(super) struct AddressBarState {
    text: EditableTextState,
    pub(super) focus_handle: Option<FocusHandle>,
    pub(super) focus_out: Option<Subscription>,
    pub(super) suggestions: Vec<AddressBarSuggestion>,
    pub(super) highlighted_suggestion: Option<usize>,
    pub(super) suggestions_scroll_top: f32,
    pub(super) suggestions_scrollbar_hovered: bool,
    pub(super) suggestions_scrollbar_drag: Option<ScrollbarDrag>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AddressBarSuggestion {
    pub(super) label: String,
    pub(super) path: PathBuf,
}

impl AddressBarState {
    fn new(content: String, focus_handle: Option<FocusHandle>) -> Self {
        Self {
            text: EditableTextState::new(content),
            focus_handle,
            focus_out: None,
            suggestions: Vec::new(),
            highlighted_suggestion: None,
            suggestions_scroll_top: 0.0,
            suggestions_scrollbar_hovered: false,
            suggestions_scrollbar_drag: None,
        }
    }

    pub(super) fn suggestions_viewport_height(&self) -> f32 {
        self.suggestions.len().min(ADDRESS_SUGGESTION_VISIBLE_ROWS) as f32
            * ADDRESS_SUGGESTION_ROW_HEIGHT
    }

    pub(super) fn suggestions_content_height(&self) -> f32 {
        self.suggestions.len() as f32 * ADDRESS_SUGGESTION_ROW_HEIGHT
    }

    pub(super) fn suggestions_scrollbar_metrics(&self) -> Option<ScrollbarMetrics> {
        ScrollbarMetrics::new(
            0.0,
            ADDRESS_SUGGESTION_VISIBLE_ROWS as f32 * ADDRESS_SUGGESTION_ROW_HEIGHT,
            self.suggestions_content_height(),
            self.suggestions_scroll_top,
        )
    }

    pub(super) fn set_suggestions_scroll_top(&mut self, scroll_top: f32) {
        self.suggestions_scroll_top = self
            .suggestions_scrollbar_metrics()
            .map_or(0.0, |metrics| metrics.clamp_scroll_top(scroll_top));
    }

    pub(super) fn scroll_suggestions_by(&mut self, delta: f32) {
        self.set_suggestions_scroll_top(self.suggestions_scroll_top + delta);
    }

    pub(super) fn clamp_suggestions_scroll(&mut self) {
        self.set_suggestions_scroll_top(self.suggestions_scroll_top);
        if self.suggestions_scrollbar_metrics().is_none() {
            self.suggestions_scrollbar_drag = None;
        }
    }

    pub(super) fn scroll_highlighted_suggestion_into_view(&mut self) {
        let Some(index) = self.highlighted_suggestion else {
            return;
        };

        let row_top = index as f32 * ADDRESS_SUGGESTION_ROW_HEIGHT;
        let row_bottom = row_top + ADDRESS_SUGGESTION_ROW_HEIGHT;
        let viewport_height =
            ADDRESS_SUGGESTION_VISIBLE_ROWS as f32 * ADDRESS_SUGGESTION_ROW_HEIGHT;
        let viewport_bottom = self.suggestions_scroll_top + viewport_height;

        if row_top < self.suggestions_scroll_top {
            self.set_suggestions_scroll_top(row_top);
        } else if row_bottom > viewport_bottom {
            self.set_suggestions_scroll_top(row_bottom - viewport_height);
        }
    }

    pub(super) fn handle_suggestions_scrollbar_mouse_down(
        &mut self,
        local_y: f32,
        metrics: ScrollbarMetrics,
    ) {
        use crate::explorer::constants::SCROLLBAR_ARROW_HEIGHT;

        if local_y < SCROLLBAR_ARROW_HEIGHT {
            self.set_suggestions_scroll_top(metrics.scroll_by(-ADDRESS_SUGGESTION_ROW_HEIGHT));
        } else if local_y > metrics.viewport_height - SCROLLBAR_ARROW_HEIGHT {
            self.set_suggestions_scroll_top(metrics.scroll_by(ADDRESS_SUGGESTION_ROW_HEIGHT));
        } else if local_y >= metrics.thumb_top && local_y <= metrics.thumb_bottom() {
            self.suggestions_scrollbar_drag = Some(ScrollbarDrag {
                pointer_offset_from_thumb_top: local_y - metrics.thumb_top,
            });
        } else if local_y < metrics.thumb_top {
            self.set_suggestions_scroll_top(metrics.scroll_by(-metrics.viewport_height));
        } else {
            self.set_suggestions_scroll_top(metrics.scroll_by(metrics.viewport_height));
        }
    }

    pub(super) fn handle_suggestions_scrollbar_drag(
        &mut self,
        local_y: f32,
        metrics: ScrollbarMetrics,
    ) {
        let Some(drag) = self.suggestions_scrollbar_drag else {
            return;
        };

        let thumb_top = local_y - drag.pointer_offset_from_thumb_top;
        self.set_suggestions_scroll_top(metrics.scroll_top_for_thumb_top(thumb_top));
    }
}

impl Deref for AddressBarState {
    type Target = EditableTextState;

    fn deref(&self) -> &Self::Target {
        &self.text
    }
}

impl DerefMut for AddressBarState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.text
    }
}

impl ExplorerView {
    pub(super) fn handle_address_edit(
        &mut self,
        _: &AddressEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_address_bar_edit(window, cx);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_commit(
        &mut self,
        _: &AddressCommit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_address_bar_edit(window, cx);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_cancel(
        &mut self,
        _: &AddressCancel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_address_bar_edit();
        self.focus_explorer(window);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_backspace(
        &mut self,
        _: &AddressBackspace,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(address) = self.active_address_bar.as_mut() {
            address.delete_backward();
            self.refresh_address_suggestions();
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_backspace_word(
        &mut self,
        _: &AddressBackspaceWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_previous_address_word_or_selection();
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_delete(
        &mut self,
        _: &AddressDelete,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(address) = self.active_address_bar.as_mut() {
            address.delete_forward();
            self.refresh_address_suggestions();
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_left(
        &mut self,
        _: &AddressLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = self.highlighted_address_suggestion_path() {
            self.navigate_to_address_suggestion_inline(path, cx);
        } else {
            self.move_address_cursor_left();
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_right(
        &mut self,
        _: &AddressRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = self.highlighted_address_suggestion_path() {
            self.navigate_to_address_suggestion_inline(path, cx);
        } else {
            self.move_address_cursor_right();
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_select_left(
        &mut self,
        _: &AddressSelectLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(address) = self.active_address_bar.as_mut() {
            let offset = address.previous_boundary(address.cursor_offset());
            address.select_to(offset);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_select_right(
        &mut self,
        _: &AddressSelectRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(address) = self.active_address_bar.as_mut() {
            let offset = address.next_boundary(address.cursor_offset());
            address.select_to(offset);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_word_left(
        &mut self,
        _: &AddressWordLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(address) = self.active_address_bar.as_mut() {
            let offset = address.previous_word_boundary(address.cursor_offset());
            address.move_to(offset);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_word_right(
        &mut self,
        _: &AddressWordRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(address) = self.active_address_bar.as_mut() {
            let offset = address.next_word_boundary(address.cursor_offset());
            address.move_to(offset);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_select_word_left(
        &mut self,
        _: &AddressSelectWordLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(address) = self.active_address_bar.as_mut() {
            let offset = address.previous_word_boundary(address.cursor_offset());
            address.select_to(offset);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_select_word_right(
        &mut self,
        _: &AddressSelectWordRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(address) = self.active_address_bar.as_mut() {
            let offset = address.next_word_boundary(address.cursor_offset());
            address.select_to(offset);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_home(
        &mut self,
        _: &AddressHome,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(address) = self.active_address_bar.as_mut() {
            address.move_to(0);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_end(
        &mut self,
        _: &AddressEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(address) = self.active_address_bar.as_mut() {
            let offset = address.content.len();
            address.move_to(offset);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_select_home(
        &mut self,
        _: &AddressSelectHome,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(address) = self.active_address_bar.as_mut() {
            address.select_to(0);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_select_end(
        &mut self,
        _: &AddressSelectEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(address) = self.active_address_bar.as_mut() {
            let offset = address.content.len();
            address.select_to(offset);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_select_all(
        &mut self,
        _: &AddressSelectAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(address) = self.active_address_bar.as_mut() {
            address.select_all();
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_copy(
        &mut self,
        _: &AddressCopy,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = self.selected_address_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_cut(
        &mut self,
        _: &AddressCut,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = self.selected_address_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            if let Some(address) = self.active_address_bar.as_mut() {
                replace_address_text(address, None, "", EditableTextEditKind::Cut);
                self.refresh_address_suggestions();
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_paste(
        &mut self,
        _: &AddressPaste,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
            && let Some(address) = self.active_address_bar.as_mut()
        {
            replace_address_text(
                address,
                None,
                &text.replace(['\r', '\n'], " "),
                EditableTextEditKind::Paste,
            );
            self.refresh_address_suggestions();
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_suggestion_up(
        &mut self,
        _: &AddressSuggestionUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(address) = self.active_address_bar.as_mut() {
            address.highlighted_suggestion = previous_suggestion_index(
                address.highlighted_suggestion,
                address.suggestions.len(),
            );
            address.scroll_highlighted_suggestion_into_view();
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_suggestion_down(
        &mut self,
        _: &AddressSuggestionDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(address) = self.active_address_bar.as_mut() {
            address.highlighted_suggestion =
                next_suggestion_index(address.highlighted_suggestion, address.suggestions.len());
            address.scroll_highlighted_suggestion_into_view();
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_address_accept_suggestion(
        &mut self,
        _: &AddressAcceptSuggestion,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.accept_address_suggestion();
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn start_address_bar_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.cancel_pending_click_rename();
        self.close_context_menu();
        self.open_utility_menu = None;
        self.finish_search_edit();

        if !self.commit_active_rename_before_interaction(window, cx) {
            return false;
        }

        let focus_handle = cx.focus_handle();
        let mut address = AddressBarState::new(
            self.address_text_for_path(&self.path),
            Some(focus_handle.clone()),
        );
        address.suggestions =
            folder_suggestions_for_input(&address.content, &self.path, self.entry_visibility());

        focus_handle.focus(window);
        let subscription = cx.on_focus_out(&focus_handle, window, |this, _, _, cx| {
            this.cancel_address_bar_edit();
            cx.notify();
        });
        address.focus_out = Some(subscription);
        self.active_address_bar = Some(address);
        self.clear_operation_notice();
        true
    }

    pub(super) fn cancel_address_bar_edit(&mut self) {
        if let Some(mut address) = self.active_address_bar.take() {
            address.focus_out = None;
        }
    }

    pub(super) fn address_text_for_path(&self, path: &Path) -> String {
        #[cfg(target_os = "windows")]
        {
            format_address_path(path, self.address_slash)
        }

        #[cfg(not(target_os = "windows"))]
        {
            format_address_path(path, AddressSlash::Forward)
        }
    }

    pub(super) fn commit_address_bar_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(path) = self.address_commit_target() else {
            return false;
        };

        self.finish_address_navigation(path, window, cx);
        true
    }

    pub(super) fn navigate_to_address_suggestion_path(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.finish_address_navigation(path, window, cx);
        true
    }

    fn navigate_to_address_suggestion_inline(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let address_text = self.address_text_for_path(&path);
        self.navigate_to_directory_with_watcher(path.clone(), HistoryMode::Record, cx);
        if let Some(address) = self.active_address_bar.as_mut() {
            address.content = address_text;
            address.selected_range = address.content.len()..address.content.len();
            address.selection_reversed = false;
            address.marked_range = None;
            address.reset_history();
            address.highlighted_suggestion = None;
            self.refresh_address_suggestions();
        }
    }

    fn move_address_cursor_left(&mut self) {
        if let Some(address) = self.active_address_bar.as_mut() {
            let offset = if address.selected_range.is_empty() {
                address.previous_boundary(address.cursor_offset())
            } else {
                address.selected_range.start
            };
            address.move_to(offset);
        }
    }

    fn move_address_cursor_right(&mut self) {
        if let Some(address) = self.active_address_bar.as_mut() {
            let offset = if address.selected_range.is_empty() {
                address.next_boundary(address.cursor_offset())
            } else {
                address.selected_range.end
            };
            address.move_to(offset);
        }
    }

    fn finish_address_navigation(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_address_bar_edit();
        self.navigate_to_directory_with_watcher(path, HistoryMode::Record, cx);
        self.focus_explorer(window);
    }

    fn address_commit_target(&mut self) -> Option<PathBuf> {
        if let Some(path) = self.highlighted_address_suggestion_path() {
            return Some(path);
        }

        let input = self.active_address_bar.as_ref()?.content.clone();
        match resolve_address_input(&input, &self.path) {
            Ok(path) => {
                self.clear_operation_notice();
                Some(path)
            }
            Err(error) => {
                self.set_error_notice(error);
                self.select_active_address_text();
                None
            }
        }
    }

    fn highlighted_address_suggestion_path(&self) -> Option<PathBuf> {
        let address = self.active_address_bar.as_ref()?;
        address
            .highlighted_suggestion
            .and_then(|index| address.suggestions.get(index))
            .map(|suggestion| suggestion.path.clone())
    }

    fn accept_address_suggestion(&mut self) -> bool {
        let Some(path) = self.active_address_bar.as_ref().and_then(|address| {
            let index = address.highlighted_suggestion.or(Some(0));
            index
                .and_then(|index| address.suggestions.get(index))
                .map(|suggestion| suggestion.path.clone())
        }) else {
            return false;
        };
        let address_text = self.address_text_for_path(&path);

        let Some(address) = self.active_address_bar.as_mut() else {
            return false;
        };
        address.content = address_text;
        address.selected_range = address.content.len()..address.content.len();
        address.selection_reversed = false;
        address.marked_range = None;
        address.reset_history();
        address.highlighted_suggestion = None;
        self.refresh_address_suggestions();
        true
    }

    pub(super) fn active_address_focus_handle(&self) -> Option<FocusHandle> {
        self.active_address_bar
            .as_ref()
            .and_then(|address| address.focus_handle.clone())
    }

    pub(super) fn address_bar_is_editing(&self) -> bool {
        self.active_address_bar.is_some()
    }

    pub(super) fn selected_address_text(&self) -> Option<String> {
        self.active_address_bar.as_ref()?.selected_text()
    }

    pub(super) fn on_address_mouse_down(&mut self, event: &MouseDownEvent) {
        let Some(address) = self.active_address_bar.as_mut() else {
            return;
        };

        address.is_selecting = true;
        let offset = address.index_for_mouse_position(event.position);
        if event.click_count >= 3 {
            address.select_all();
        } else if event.click_count == 2 {
            address.select_word_at(offset);
        } else if event.modifiers.shift {
            address.select_to(offset);
        } else {
            address.move_to(offset);
        }
    }

    pub(super) fn on_address_mouse_move(&mut self, event: &MouseMoveEvent) {
        let Some(address) = self.active_address_bar.as_mut() else {
            return;
        };

        if address.is_selecting {
            let offset = address.index_for_mouse_position(event.position);
            address.select_to(offset);
        }
    }

    pub(super) fn on_address_mouse_up(&mut self, _: &MouseUpEvent) {
        if let Some(address) = self.active_address_bar.as_mut() {
            address.is_selecting = false;
        }
    }

    pub(super) fn update_address_layout(&mut self, line: ShapedLine, bounds: Bounds<Pixels>) {
        if let Some(address) = self.active_address_bar.as_mut() {
            address.update_layout(line, bounds);
        }
    }

    pub(super) fn address_text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
    ) -> Option<String> {
        let address = self.active_address_bar.as_ref()?;
        let range = address.range_from_utf16(&range_utf16);
        actual_range.replace(address.range_to_utf16(&range));
        Some(address.content[range].to_owned())
    }

    pub(super) fn selected_address_text_range(&mut self) -> Option<UTF16Selection> {
        let address = self.active_address_bar.as_ref()?;
        Some(address.selected_text_range_utf16())
    }

    pub(super) fn marked_address_text_range(&self) -> Option<Range<usize>> {
        let address = self.active_address_bar.as_ref()?;
        address.marked_text_range_utf16()
    }

    pub(super) fn unmark_address_text(&mut self) {
        if let Some(address) = self.active_address_bar.as_mut() {
            address.unmark_text();
        }
    }

    pub(super) fn replace_address_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
    ) {
        if let Some(address) = self.active_address_bar.as_mut() {
            address.replace_text_in_range_utf16(range_utf16, &text.replace(['\r', '\n'], " "));
            self.refresh_address_suggestions();
        }
    }

    pub(super) fn replace_and_mark_address_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
    ) {
        if let Some(address) = self.active_address_bar.as_mut() {
            let new_text = new_text.replace(['\r', '\n'], " ");
            address.replace_and_mark_text_in_range_utf16(
                range_utf16,
                &new_text,
                new_selected_range_utf16,
            );
            self.refresh_address_suggestions();
        }
    }

    pub(super) fn undo_address_text(&mut self) {
        if self
            .active_address_bar
            .as_mut()
            .is_some_and(|address| address.undo())
        {
            self.refresh_address_suggestions();
        }
    }

    pub(super) fn redo_address_text(&mut self) {
        if self
            .active_address_bar
            .as_mut()
            .is_some_and(|address| address.redo())
        {
            self.refresh_address_suggestions();
        }
    }

    pub(super) fn address_bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
    ) -> Option<Bounds<Pixels>> {
        let address = self.active_address_bar.as_ref()?;
        address.bounds_for_range(range_utf16, bounds)
    }

    pub(super) fn address_character_index_for_point(
        &mut self,
        point: Point<Pixels>,
    ) -> Option<usize> {
        let address = self.active_address_bar.as_ref()?;
        address.character_index_for_point(point)
    }

    fn refresh_address_suggestions(&mut self) {
        let current_path = self.path.clone();
        let visibility = self.entry_visibility();
        if let Some(address) = self.active_address_bar.as_mut() {
            address.suggestions =
                folder_suggestions_for_input(&address.content, &current_path, visibility);
            if address
                .highlighted_suggestion
                .is_some_and(|index| index >= address.suggestions.len())
            {
                address.highlighted_suggestion = None;
                address.suggestions_scroll_top = 0.0;
            }
            if address.highlighted_suggestion.is_none() {
                address.suggestions_scroll_top = 0.0;
            }
            address.clamp_suggestions_scroll();
        }
    }

    fn select_active_address_text(&mut self) {
        if let Some(address) = self.active_address_bar.as_mut() {
            address.select_all();
        }
    }

    fn delete_previous_address_word_or_selection(&mut self) {
        if let Some(address) = self.active_address_bar.as_mut() {
            address.delete_previous_word_or_selection();
            self.refresh_address_suggestions();
        }
    }
}

pub(super) fn format_address_path(path: &Path, slash: AddressSlash) -> String {
    let address = path.display().to_string();

    #[cfg(target_os = "windows")]
    {
        match slash {
            AddressSlash::Forward => address.replace('\\', "/"),
            AddressSlash::Back => address.replace('/', "\\"),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = slash;
        address
    }
}

pub(super) fn resolve_address_input(input: &str, current_path: &Path) -> Result<PathBuf, String> {
    resolve_address_input_with_env(input, current_path, |name| env::var_os(name))
}

fn resolve_address_input_with_env(
    input: &str,
    current_path: &Path,
    env_var: impl FnMut(&str) -> Option<OsString>,
) -> Result<PathBuf, String> {
    let cleaned = cleaned_address_input(input);
    if cleaned.is_empty() {
        return Err("The address is empty.".to_owned());
    }

    let expanded = expand_address_environment_variables_with(&cleaned, env_var);
    let typed_path = Path::new(&expanded);
    let candidate = if typed_path.is_absolute() {
        typed_path.to_path_buf()
    } else {
        current_path.join(typed_path)
    };

    if !candidate.exists() {
        return Err(format!("Could not find {}.", candidate.display()));
    }

    if !candidate.is_dir() {
        return Err(format!("{} is not a folder.", candidate.display()));
    }

    Ok(fs::canonicalize(&candidate)
        .map(explorer_visible_address_path)
        .unwrap_or(candidate))
}

#[cfg(windows)]
fn explorer_visible_address_path(path: PathBuf) -> PathBuf {
    use std::path::{Component, Prefix};

    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return path;
    };

    let mut visible = match prefix.kind() {
        Prefix::VerbatimDisk(letter) => {
            PathBuf::from(format!("{}:\\", char::from(letter).to_ascii_uppercase()))
        }
        Prefix::VerbatimUNC(server, share) => PathBuf::from(format!(
            r"\\{}\{}\",
            server.to_string_lossy(),
            share.to_string_lossy()
        )),
        _ => return path,
    };

    for component in components {
        if !matches!(component, Component::RootDir) {
            visible.push(component.as_os_str());
        }
    }

    visible
}

#[cfg(not(windows))]
fn explorer_visible_address_path(path: PathBuf) -> PathBuf {
    path
}

pub(super) fn folder_suggestions_for_input(
    input: &str,
    current_path: &Path,
    visibility: impl Into<crate::explorer::filesystem::EntryVisibility>,
) -> Vec<AddressBarSuggestion> {
    folder_suggestions_for_input_with_env(input, current_path, visibility, |name| env::var_os(name))
}

fn folder_suggestions_for_input_with_env(
    input: &str,
    current_path: &Path,
    visibility: impl Into<crate::explorer::filesystem::EntryVisibility>,
    env_var: impl FnMut(&str) -> Option<OsString>,
) -> Vec<AddressBarSuggestion> {
    let visibility = visibility.into();
    let cleaned = cleaned_address_input(input);
    let expanded = expand_address_environment_variables_with(&cleaned, env_var);
    let (parent, prefix) = suggestion_parent_and_prefix(
        Path::new(&expanded),
        has_trailing_separator(&cleaned),
        current_path,
    );

    if !parent.is_dir() {
        return Vec::new();
    }

    let prefix_lower = prefix.to_lowercase();
    let Ok(entries) = fs::read_dir(&parent) else {
        return Vec::new();
    };

    let mut suggestions = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            if should_hide_directory_entry(&entry, visibility) {
                return None;
            }

            let path = entry.path();
            if !path.is_dir() || paths_match_for_address_suggestions(&path, current_path) {
                return None;
            }

            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.to_lowercase().starts_with(&prefix_lower) {
                return None;
            }

            Some(AddressBarSuggestion { label: name, path })
        })
        .collect::<Vec<_>>();

    suggestions.sort_by(|left, right| {
        left.label
            .to_lowercase()
            .cmp(&right.label.to_lowercase())
            .then_with(|| left.label.cmp(&right.label))
    });
    suggestions
}

fn expand_address_environment_variables_with(
    input: &str,
    mut env_var: impl FnMut(&str) -> Option<OsString>,
) -> OsString {
    let mut expanded = OsString::new();
    let mut remainder = input;
    let mut env_was_expanded = false;

    loop {
        let Some(start) = remainder.find('%') else {
            push_address_literal_segment(&mut expanded, remainder, env_was_expanded);
            return expanded;
        };

        push_address_literal_segment(&mut expanded, &remainder[..start], env_was_expanded);
        let after_start = &remainder[start + 1..];
        let Some(end) = after_start.find('%') else {
            push_address_literal_segment(&mut expanded, &remainder[start..], env_was_expanded);
            return expanded;
        };

        let name = &after_start[..end];
        let token_end = start + 1 + end + 1;
        if !name.is_empty()
            && let Some(value) = env_var(name)
        {
            expanded.push(value);
            env_was_expanded = true;
        } else {
            push_address_literal_segment(
                &mut expanded,
                &remainder[start..token_end],
                env_was_expanded,
            );
        }

        remainder = &remainder[token_end..];
    }
}

fn push_address_literal_segment(target: &mut OsString, segment: &str, normalize_backslashes: bool) {
    #[cfg(windows)]
    {
        let _ = normalize_backslashes;
        target.push(segment);
    }

    #[cfg(not(windows))]
    {
        if normalize_backslashes {
            let separator = std::path::MAIN_SEPARATOR.to_string();
            target.push(segment.replace('\\', &separator));
        } else {
            target.push(segment);
        }
    }
}

fn paths_match_for_address_suggestions(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn suggestion_parent_and_prefix(
    typed_path: &Path,
    input_has_trailing_separator: bool,
    current_path: &Path,
) -> (PathBuf, String) {
    if typed_path.as_os_str().is_empty() {
        return (current_path.to_path_buf(), String::new());
    }

    let candidate = if typed_path.is_absolute() {
        typed_path.to_path_buf()
    } else {
        current_path.join(typed_path)
    };
    if candidate.is_dir() && paths_match_for_address_suggestions(&candidate, current_path) {
        return (current_path.to_path_buf(), String::new());
    }

    let (parent, prefix) = if input_has_trailing_separator {
        (typed_path.to_path_buf(), String::new())
    } else {
        (
            typed_path
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .to_path_buf(),
            typed_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    };

    let parent = if parent.as_os_str().is_empty() {
        current_path.to_path_buf()
    } else if parent.is_absolute() {
        parent
    } else {
        current_path.join(parent)
    };

    (parent, prefix)
}

fn cleaned_address_input(input: &str) -> String {
    let trimmed = input.trim();
    let cleaned = strip_address_outer_quotes(trimmed)
        .unwrap_or(trimmed)
        .trim();

    normalize_windows_address_separators(cleaned)
}

fn strip_address_outer_quotes(input: &str) -> Option<&str> {
    let bytes = input.as_bytes();
    if input.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return Some(&input[1..input.len() - 1]);
        }
    }

    if input.len() >= 4 {
        let first = bytes[0];
        let second = bytes[1];
        let penultimate = bytes[bytes.len() - 2];
        let last = bytes[bytes.len() - 1];
        if first == b'\\'
            && penultimate == b'\\'
            && ((second == b'"' && last == b'"') || (second == b'\'' && last == b'\''))
        {
            return Some(&input[2..input.len() - 2]);
        }
    }

    None
}

fn normalize_windows_address_separators(input: &str) -> String {
    if !looks_like_windows_address_path(input) {
        return input.to_owned();
    }

    let mut normalized = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    if starts_with_address_separator_pair(input) {
        let separator = chars.next().expect("separator pair has first separator");
        normalized.push(separator);
        normalized.push(separator);
        while chars
            .peek()
            .is_some_and(|character| is_address_separator(*character))
        {
            chars.next();
        }
    }

    while let Some(character) = chars.next() {
        normalized.push(character);
        if is_address_separator(character) {
            while chars
                .peek()
                .is_some_and(|character| is_address_separator(*character))
            {
                chars.next();
            }
        }
    }

    normalized
}

fn looks_like_windows_address_path(input: &str) -> bool {
    let mut chars = input.chars();
    match (chars.next(), chars.next()) {
        (Some(first), Some(':')) if first.is_ascii_alphabetic() => true,
        (Some(first), Some(second))
            if is_address_separator(first) && is_address_separator(second) =>
        {
            true
        }
        _ => false,
    }
}

fn starts_with_address_separator_pair(input: &str) -> bool {
    let mut chars = input.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(first), Some(second))
            if is_address_separator(first) && is_address_separator(second)
    )
}

fn is_address_separator(character: char) -> bool {
    character == '\\' || character == '/'
}

fn has_trailing_separator(input: &str) -> bool {
    input.ends_with('/') || input.ends_with('\\')
}

fn next_suggestion_index(current: Option<usize>, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }

    Some(match current {
        Some(index) => (index + 1) % len,
        None => 0,
    })
}

fn previous_suggestion_index(current: Option<usize>, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }

    Some(match current {
        Some(0) | None => len - 1,
        Some(index) => index - 1,
    })
}

fn replace_address_text(
    address: &mut AddressBarState,
    range: Option<Range<usize>>,
    new_text: &str,
    kind: EditableTextEditKind,
) {
    address.replace_text_with_kind(range, new_text, kind);
}

pub(super) struct AddressTextElement {
    entity: Entity<ExplorerView>,
}

pub(super) fn address_text_element(entity: Entity<ExplorerView>) -> AddressTextElement {
    AddressTextElement { entity }
}

pub(super) struct AddressPrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    scroll_offset: Pixels,
}

impl IntoElement for AddressTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for AddressTextElement {
    type RequestLayoutState = ();
    type PrepaintState = AddressPrepaintState;

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
        let address = view
            .active_address_bar
            .as_ref()
            .expect("address text element is only rendered while editing the address");
        let content = gpui::SharedString::from(address.content.clone());
        let selected_range = address.selected_range.clone();
        let cursor = address.cursor_offset();
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
            address.marked_range.as_ref(),
        );

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(content, font_size, &runs, None);
        let scroll_offset = scroll_offset_for_cursor(
            address.scroll_offset,
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

        AddressPrepaintState {
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
        if let Some(focus_handle) = self.entity.read(cx).active_address_focus_handle() {
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, self.entity.clone()),
                cx,
            );
        }

        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        let line = prepaint.line.take().expect("address text line");
        line.paint(
            point(bounds.origin.x - prepaint.scroll_offset, bounds.origin.y),
            window.line_height(),
            window,
            cx,
        )
        .expect("paint address text");

        if let Some(focus_handle) = self.entity.read(cx).active_address_focus_handle()
            && focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.entity.update(cx, |view, _| {
            view.update_address_layout(line, bounds);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::{
        test_support::{TempDir, test_view_entity_at_path},
        view::ExplorerView,
    };
    use crate::settings::AddressSlash;
    use gpui::{ClipboardItem, Modifiers, MouseButton, TestAppContext};
    use std::{ffi::OsString, fs};

    fn env_path<'a>(
        name: &'static str,
        path: &'a std::path::Path,
    ) -> impl FnMut(&str) -> Option<OsString> + 'a {
        move |requested| (requested == name).then(|| path.as_os_str().to_os_string())
    }

    #[test]
    fn cleaned_address_input_normalizes_quotes_and_windows_separators() {
        assert_eq!(
            cleaned_address_input(r#" "C:\Users\Ada" "#),
            r"C:\Users\Ada"
        );
        assert_eq!(
            cleaned_address_input(r#" \"C:\Users\Ada\" "#),
            r"C:\Users\Ada"
        );
        assert_eq!(cleaned_address_input(r"C:\\Users\\Ada"), r"C:\Users\Ada");
        assert_eq!(
            cleaned_address_input(r"\\\\server\\share\\folder"),
            r"\\server\share\folder"
        );
        assert_eq!(cleaned_address_input(r#"C:\Users"Ada"#), r#"C:\Users"Ada"#);
    }

    #[test]
    fn resolve_address_accepts_absolute_and_relative_directories() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir(&child).expect("create child");

        assert_eq!(
            resolve_address_input(&child.display().to_string(), temp.path()).unwrap(),
            explorer_visible_address_path(fs::canonicalize(&child).unwrap())
        );
        assert_eq!(
            resolve_address_input("child", temp.path()).unwrap(),
            explorer_visible_address_path(fs::canonicalize(&child).unwrap())
        );
    }

    #[test]
    fn resolve_address_accepts_dot_dot_and_quotes() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir(&child).expect("create child");

        assert_eq!(
            resolve_address_input(".", &child).unwrap(),
            explorer_visible_address_path(fs::canonicalize(&child).unwrap())
        );
        assert_eq!(
            resolve_address_input("..", &child).unwrap(),
            explorer_visible_address_path(fs::canonicalize(temp.path()).unwrap())
        );
        assert_eq!(
            resolve_address_input(&format!(" \"{}\" ", child.display()), temp.path()).unwrap(),
            explorer_visible_address_path(fs::canonicalize(&child).unwrap())
        );
    }

    #[cfg(windows)]
    #[test]
    fn resolve_address_accepts_windows_escaped_path_forms() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir(&child).expect("create child");

        let expected = explorer_visible_address_path(fs::canonicalize(&child).unwrap());
        let doubled = child.display().to_string().replace('\\', r"\\");
        let forward = child.display().to_string().replace('\\', "/");

        assert_eq!(
            resolve_address_input(&doubled, temp.path()).unwrap(),
            expected
        );
        assert_eq!(
            resolve_address_input(&format!("\"{forward}\""), temp.path()).unwrap(),
            expected
        );
        assert_eq!(
            resolve_address_input(&format!("\"{doubled}\""), temp.path()).unwrap(),
            expected
        );
        assert_eq!(
            resolve_address_input(&format!(r#"\"{doubled}\""#), temp.path()).unwrap(),
            expected
        );
    }

    #[test]
    fn resolve_address_expands_environment_variable_to_directory() {
        let temp = TempDir::new();
        let appdata = temp.path().join("AppData").join("Roaming");
        fs::create_dir_all(&appdata).expect("create appdata");

        assert_eq!(
            resolve_address_input_with_env("%APPDATA%", temp.path(), env_path("APPDATA", &appdata))
                .unwrap(),
            explorer_visible_address_path(fs::canonicalize(&appdata).unwrap())
        );
    }

    #[test]
    fn resolve_address_expands_environment_variable_suffixes() {
        let temp = TempDir::new();
        let appdata = temp.path().join("AppData").join("Roaming");
        let child = appdata.join("Child");
        fs::create_dir_all(&child).expect("create child");

        assert_eq!(
            resolve_address_input_with_env(
                r"%APPDATA%\Child",
                temp.path(),
                env_path("APPDATA", &appdata),
            )
            .unwrap(),
            explorer_visible_address_path(fs::canonicalize(&child).unwrap())
        );
        assert_eq!(
            resolve_address_input_with_env(
                "%APPDATA%/Child",
                temp.path(),
                env_path("APPDATA", &appdata),
            )
            .unwrap(),
            explorer_visible_address_path(fs::canonicalize(&child).unwrap())
        );
    }

    #[test]
    fn resolve_address_expands_quoted_environment_variable() {
        let temp = TempDir::new();
        let appdata = temp.path().join("AppData").join("Roaming");
        fs::create_dir_all(&appdata).expect("create appdata");

        assert_eq!(
            resolve_address_input_with_env(
                "\"%APPDATA%\"",
                temp.path(),
                env_path("APPDATA", &appdata),
            )
            .unwrap(),
            explorer_visible_address_path(fs::canonicalize(&appdata).unwrap())
        );
    }

    #[test]
    fn address_environment_expansion_preserves_unresolved_tokens() {
        assert_eq!(
            expand_address_environment_variables_with("%MISSING%", |_| None),
            OsString::from("%MISSING%")
        );
        assert_eq!(
            expand_address_environment_variables_with("%%", |_| Some(OsString::from("ignored"))),
            OsString::from("%%")
        );
        assert_eq!(
            expand_address_environment_variables_with("%APPDATA", |name| {
                (name == "APPDATA").then(|| OsString::from("expanded"))
            }),
            OsString::from("%APPDATA")
        );
    }

    #[test]
    fn resolve_address_unknown_environment_variable_stays_literal() {
        let temp = TempDir::new();
        let error = resolve_address_input_with_env("%MISSING%", temp.path(), |_| None).unwrap_err();

        assert_eq!(
            error,
            format!(
                "Could not find {}.",
                temp.path().join("%MISSING%").display()
            )
        );
    }

    #[test]
    fn resolve_address_rejects_missing_paths_and_files() {
        let temp = TempDir::new();
        fs::write(temp.path().join("file.txt"), b"data").expect("write file");

        assert!(resolve_address_input("missing", temp.path()).is_err());
        assert!(resolve_address_input("file.txt", temp.path()).is_err());
        assert!(resolve_address_input("", temp.path()).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn explorer_visible_address_path_strips_verbatim_disk_prefix() {
        assert_eq!(
            explorer_visible_address_path(PathBuf::from(r"\\?\C:\Users\Ada\Documents")),
            PathBuf::from(r"C:\Users\Ada\Documents")
        );
    }

    #[cfg(windows)]
    #[test]
    fn explorer_visible_address_path_strips_verbatim_unc_prefix() {
        assert_eq!(
            explorer_visible_address_path(PathBuf::from(
                r"\\?\UNC\server\share\Users\Ada\Documents"
            )),
            PathBuf::from(r"\\server\share\Users\Ada\Documents")
        );
    }

    #[test]
    fn folder_suggestions_match_folders_only_case_insensitively() {
        let temp = TempDir::new();
        fs::create_dir(temp.path().join("Alpha")).expect("create alpha");
        fs::create_dir(temp.path().join("apricot")).expect("create apricot");
        fs::write(temp.path().join("apple.txt"), b"data").expect("write file");

        let suggestions = folder_suggestions_for_input("a", temp.path(), true);
        let labels = suggestions
            .iter()
            .map(|suggestion| suggestion.label.as_str())
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["Alpha", "apricot"]);
    }

    #[test]
    fn folder_suggestions_expand_environment_variable_parent() {
        let temp = TempDir::new();
        let appdata = temp.path().join("AppData").join("Roaming");
        fs::create_dir_all(appdata.join("Microsoft")).expect("create microsoft");
        fs::create_dir(appdata.join("Mozilla")).expect("create mozilla");
        fs::create_dir(appdata.join("Other")).expect("create other");

        let suggestions = folder_suggestions_for_input_with_env(
            r"%APPDATA%\M",
            temp.path(),
            true,
            env_path("APPDATA", &appdata),
        );

        assert_eq!(
            suggestions
                .iter()
                .map(|suggestion| suggestion.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Microsoft", "Mozilla"]
        );
        assert_eq!(suggestions[0].path, appdata.join("Microsoft"));
        assert_eq!(suggestions[1].path, appdata.join("Mozilla"));
    }

    #[cfg(windows)]
    #[test]
    fn folder_suggestions_normalize_escaped_windows_parent_path() {
        let temp = TempDir::new();
        let parent = temp.path().join("parent");
        fs::create_dir(&parent).expect("create parent");
        fs::create_dir(parent.join("child-a")).expect("create child a");
        fs::create_dir(parent.join("child-b")).expect("create child b");

        let doubled_parent = parent.display().to_string().replace('\\', r"\\");
        let suggestions = folder_suggestions_for_input(
            &format!(r#"\"{doubled_parent}\\child\""#),
            temp.path(),
            true,
        );

        assert_eq!(
            suggestions
                .iter()
                .map(|suggestion| suggestion.label.as_str())
                .collect::<Vec<_>>(),
            vec!["child-a", "child-b"]
        );
    }

    #[test]
    fn folder_suggestions_use_typed_parent_and_trailing_separator() {
        let temp = TempDir::new();
        let parent = temp.path().join("parent");
        fs::create_dir(&parent).expect("create parent");
        fs::create_dir(parent.join("child-a")).expect("create child a");
        fs::create_dir(parent.join("child-b")).expect("create child b");

        let suggestions = folder_suggestions_for_input(
            &format!("parent{}", std::path::MAIN_SEPARATOR),
            temp.path(),
            true,
        );

        assert_eq!(suggestions.len(), 2);
        assert_eq!(suggestions[0].label, "child-a");
        assert_eq!(suggestions[1].label, "child-b");
    }

    #[test]
    fn folder_suggestions_for_current_path_show_children_without_trailing_separator() {
        let temp = TempDir::new();
        fs::create_dir(temp.path().join("child-a")).expect("create child a");
        fs::create_dir(temp.path().join("child-b")).expect("create child b");

        let suggestions =
            folder_suggestions_for_input(&temp.path().display().to_string(), temp.path(), true);
        let labels = suggestions
            .iter()
            .map(|suggestion| suggestion.label.as_str())
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["child-a", "child-b"]);
    }

    #[test]
    fn folder_suggestions_for_current_path_show_children_with_trailing_separator() {
        let temp = TempDir::new();
        fs::create_dir(temp.path().join("child-a")).expect("create child a");
        fs::create_dir(temp.path().join("child-b")).expect("create child b");

        let suggestions = folder_suggestions_for_input(
            &format!("{}{}", temp.path().display(), std::path::MAIN_SEPARATOR),
            temp.path(),
            true,
        );
        let labels = suggestions
            .iter()
            .map(|suggestion| suggestion.label.as_str())
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["child-a", "child-b"]);
    }

    #[test]
    fn folder_suggestions_keep_partial_folder_name_as_sibling_prefix_match() {
        let temp = TempDir::new();
        let alpha = temp.path().join("alpha");
        fs::create_dir(&alpha).expect("create alpha");
        fs::create_dir(alpha.join("inside")).expect("create inside");
        fs::create_dir(temp.path().join("apricot")).expect("create apricot");

        let suggestions = folder_suggestions_for_input("a", temp.path(), true);
        let labels = suggestions
            .iter()
            .map(|suggestion| suggestion.label.as_str())
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["alpha", "apricot"]);
    }

    #[test]
    fn folder_suggestions_follow_show_hidden_files_setting() {
        let temp = TempDir::new();
        fs::create_dir(temp.path().join(".hidden")).expect("create hidden");
        fs::create_dir(temp.path().join("visible")).expect("create visible");

        let hidden_off = folder_suggestions_for_input("", temp.path(), false);
        let hidden_off_labels = hidden_off
            .iter()
            .map(|suggestion| suggestion.label.as_str())
            .collect::<Vec<_>>();

        let hidden_on = folder_suggestions_for_input("", temp.path(), true);
        let hidden_on_labels = hidden_on
            .iter()
            .map(|suggestion| suggestion.label.as_str())
            .collect::<Vec<_>>();

        assert_eq!(hidden_off_labels, vec!["visible"]);
        assert_eq!(hidden_on_labels, vec![".hidden", "visible"]);
    }

    #[test]
    fn folder_suggestions_follow_dot_items_setting_independently() {
        let temp = TempDir::new();
        fs::create_dir(temp.path().join(".dot-folder")).expect("create dot folder");
        fs::create_dir(temp.path().join("visible")).expect("create visible folder");

        let dot_off = folder_suggestions_for_input(
            "",
            temp.path(),
            crate::explorer::filesystem::EntryVisibility::new(false, true),
        );
        let dot_on = folder_suggestions_for_input(
            "",
            temp.path(),
            crate::explorer::filesystem::EntryVisibility::new(true, false),
        );

        assert_eq!(
            dot_off
                .iter()
                .map(|suggestion| suggestion.label.as_str())
                .collect::<Vec<_>>(),
            vec!["visible"]
        );
        assert_eq!(
            dot_on
                .iter()
                .map(|suggestion| suggestion.label.as_str())
                .collect::<Vec<_>>(),
            vec![".dot-folder", "visible"]
        );
    }

    #[test]
    fn folder_suggestions_always_omit_metadata_entries() {
        let temp = TempDir::new();
        fs::create_dir(temp.path().join(".localized")).expect("create localized");
        fs::create_dir(temp.path().join("visible")).expect("create visible");

        let suggestions = folder_suggestions_for_input("", temp.path(), true);
        let labels = suggestions
            .iter()
            .map(|suggestion| suggestion.label.as_str())
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["visible"]);
    }

    #[test]
    fn folder_suggestions_return_all_results_deterministically() {
        let temp = TempDir::new();
        for index in 0..12 {
            fs::create_dir(temp.path().join(format!("folder-{index:02}"))).expect("create folder");
        }

        let suggestions = folder_suggestions_for_input("folder", temp.path(), true);

        assert_eq!(suggestions.len(), 12);
        assert_eq!(suggestions[0].label, "folder-00");
        assert_eq!(suggestions[11].label, "folder-11");
    }

    #[test]
    fn folder_suggestions_omit_current_folder_from_child_matches() {
        let temp = TempDir::new();
        let current = temp.path().join("current");
        fs::create_dir(&current).expect("create current");
        fs::create_dir(current.join("child")).expect("create child");

        let suggestions =
            folder_suggestions_for_input(&current.display().to_string(), &current, false);
        let labels = suggestions
            .iter()
            .map(|suggestion| suggestion.label.as_str())
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["child"]);
    }

    #[test]
    fn folder_suggestions_do_not_include_parent_directory_row() {
        let temp = TempDir::new();
        let current = temp.path().join("current");
        fs::create_dir(&current).expect("create current");
        fs::create_dir(current.join("child")).expect("create child");

        let suggestions = folder_suggestions_for_input("ch", &current, true);

        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].label, "child");
        assert_eq!(suggestions[0].path, current.join("child"));
    }

    #[test]
    fn address_suggestions_scrollbar_appears_only_after_visible_rows_overflow() {
        let mut address = AddressBarState::new(String::new(), None);
        address.suggestions = address_suggestions_for_test(ADDRESS_SUGGESTION_VISIBLE_ROWS);
        assert!(address.suggestions_scrollbar_metrics().is_none());

        address.suggestions = address_suggestions_for_test(ADDRESS_SUGGESTION_VISIBLE_ROWS + 1);
        assert!(address.suggestions_scrollbar_metrics().is_some());
    }

    #[test]
    fn address_suggestions_scroll_top_clamps_to_overflow_bounds() {
        let mut address = AddressBarState::new(String::new(), None);
        address.suggestions = address_suggestions_for_test(ADDRESS_SUGGESTION_VISIBLE_ROWS + 1);

        address.set_suggestions_scroll_top(1000.0);
        assert_eq!(
            address.suggestions_scroll_top,
            ADDRESS_SUGGESTION_ROW_HEIGHT
        );

        address.set_suggestions_scroll_top(-1000.0);
        assert_eq!(address.suggestions_scroll_top, 0.0);
    }

    #[test]
    fn highlighted_address_suggestion_scrolls_into_view() {
        let mut address = AddressBarState::new(String::new(), None);
        address.suggestions = address_suggestions_for_test(20);

        address.highlighted_suggestion = Some(11);
        address.scroll_highlighted_suggestion_into_view();
        assert_eq!(
            address.suggestions_scroll_top,
            ADDRESS_SUGGESTION_ROW_HEIGHT * 2.0
        );

        address.highlighted_suggestion = Some(1);
        address.scroll_highlighted_suggestion_into_view();
        assert_eq!(
            address.suggestions_scroll_top,
            ADDRESS_SUGGESTION_ROW_HEIGHT
        );
    }

    #[test]
    fn address_start_selects_full_current_path() {
        let temp = TempDir::new();
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.active_address_bar = Some(AddressBarState::new(
            view.address_text_for_path(temp.path()),
            None,
        ));

        let address = view.active_address_bar.as_ref().unwrap();
        assert_eq!(
            address.content,
            format_address_path(temp.path(), AddressSlash::Forward)
        );
        assert_eq!(address.selected_range, 0..address.content.len());
    }

    #[cfg(target_os = "windows")]
    #[gpui::test]
    fn address_start_uses_configured_backslashes_on_windows(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (view, cx) = test_view_entity_at_path(cx, path.clone());

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.address_slash = AddressSlash::Back;
                assert!(view.start_address_bar_edit(window, cx));
                let address = view.active_address_bar.as_ref().expect("address edit");
                assert_eq!(
                    address.content,
                    format_address_path(&path, AddressSlash::Back)
                );
            });
        });
    }

    #[test]
    fn address_commit_target_preserves_current_path_on_failure() {
        let temp = TempDir::new();
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.active_address_bar = Some(AddressBarState::new("missing".to_owned(), None));

        assert_eq!(view.address_commit_target(), None);
        assert_eq!(view.path, temp.path());
        assert!(view.active_address_bar.is_some());
        assert!(view.operation_notice.is_some());
    }

    #[test]
    fn highlighted_address_suggestion_path_requires_highlight() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir(&child).expect("create child");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        let mut address = AddressBarState::new("ch".to_owned(), None);
        address.suggestions = folder_suggestions_for_input(&address.content, temp.path(), true);
        view.active_address_bar = Some(address);

        assert_eq!(view.highlighted_address_suggestion_path(), None);

        view.active_address_bar
            .as_mut()
            .expect("address edit")
            .highlighted_suggestion = Some(0);

        assert_eq!(view.highlighted_address_suggestion_path(), Some(child));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn accept_address_suggestion_uses_configured_backslashes_on_windows() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir(&child).expect("create child");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.address_slash = AddressSlash::Back;
        let mut address = AddressBarState::new("ch".to_owned(), None);
        address.suggestions = folder_suggestions_for_input(&address.content, temp.path(), true);
        view.active_address_bar = Some(address);

        assert!(view.accept_address_suggestion());

        let address = view.active_address_bar.as_ref().expect("address edit");
        assert_eq!(
            address.content,
            format_address_path(&child, AddressSlash::Back)
        );
    }

    #[test]
    fn address_left_without_highlight_moves_text_cursor() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        let mut address = AddressBarState::new("alpha".to_owned(), None);
        address.move_to(1);
        view.active_address_bar = Some(address);

        assert_eq!(view.highlighted_address_suggestion_path(), None);

        view.move_address_cursor_left();

        let address = view.active_address_bar.as_ref().expect("address edit");
        assert_eq!(address.selected_range, 0..0);
    }

    #[test]
    fn address_right_without_highlight_moves_text_cursor() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        let mut address = AddressBarState::new("alpha".to_owned(), None);
        address.move_to(0);
        view.active_address_bar = Some(address);

        assert_eq!(view.highlighted_address_suggestion_path(), None);

        view.move_address_cursor_right();

        let address = view.active_address_bar.as_ref().expect("address edit");
        assert_eq!(address.selected_range, 1..1);
    }

    #[test]
    fn captured_address_suggestion_path_can_navigate_after_edit_clears() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir(&child).expect("create child");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        let mut address = AddressBarState::new("ch".to_owned(), None);
        address.suggestions = folder_suggestions_for_input(&address.content, temp.path(), true);
        let captured_path = address.suggestions[0].path.clone();
        view.active_address_bar = Some(address);

        view.cancel_address_bar_edit();
        view.navigate_to_directory(captured_path.clone(), HistoryMode::Record);

        assert_eq!(view.path, captured_path);
        assert!(view.active_address_bar.is_none());
    }

    #[cfg(target_os = "windows")]
    #[gpui::test]
    fn inline_address_suggestion_uses_configured_backslashes_on_windows(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir(&child).expect("create child");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.address_slash = AddressSlash::Back;
                view.active_address_bar = Some(AddressBarState::new("ch".to_owned(), None));

                view.navigate_to_address_suggestion_inline(child.clone(), cx);

                assert_eq!(view.path, child);
                let address = view.active_address_bar.as_ref().expect("address edit");
                assert_eq!(
                    address.content,
                    format_address_path(&child, AddressSlash::Back)
                );
            });
        });
    }

    fn address_suggestions_for_test(count: usize) -> Vec<AddressBarSuggestion> {
        (0..count)
            .map(|index| AddressBarSuggestion {
                label: format!("folder-{index:02}"),
                path: PathBuf::from(format!("folder-{index:02}")),
            })
            .collect()
    }

    #[test]
    fn address_mouse_down_without_shift_collapses_selection_to_click_position() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.active_address_bar = Some(AddressBarState::new("alpha beta".to_owned(), None));

        view.on_address_mouse_down(&MouseDownEvent {
            button: MouseButton::Left,
            ..MouseDownEvent::default()
        });

        let address = view.active_address_bar.as_ref().expect("address edit");
        assert_eq!(address.selected_range, 0..0);
        assert!(!address.selection_reversed);
    }

    #[test]
    fn address_shift_mouse_down_extends_selection_to_click_position() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        let mut address = AddressBarState::new("alpha beta".to_owned(), None);
        let offset = address.content.len();
        address.move_to(offset);
        view.active_address_bar = Some(address);

        view.on_address_mouse_down(&MouseDownEvent {
            button: MouseButton::Left,
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
            ..MouseDownEvent::default()
        });

        let address = view.active_address_bar.as_ref().expect("address edit");
        assert_eq!(address.selected_range, 0.."alpha beta".len());
        assert!(address.selection_reversed);
    }

    #[test]
    fn double_click_word_selection_selects_address_word_at_offset() {
        let mut address = AddressBarState::new("alpha/beta gamma".to_owned(), None);

        address.select_word_at("al".len());
        assert_eq!(address.selected_range, 0.."alpha".len());

        address.select_word_at("alpha/".len());
        assert_eq!(address.selected_range, "alpha/".len().."alpha/beta".len());
    }

    #[test]
    fn ctrl_backspace_refreshes_address_suggestions() {
        let temp = TempDir::new();
        fs::create_dir(temp.path().join("child")).expect("create child");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        let mut address = AddressBarState::new("child missing".to_owned(), None);
        let offset = address.content.len();
        address.move_to(offset);
        view.active_address_bar = Some(address);

        view.delete_previous_address_word_or_selection();

        let address = view.active_address_bar.as_ref().expect("address edit");
        assert_eq!(address.content, "child ");
        assert_eq!(
            address
                .suggestions
                .iter()
                .map(|suggestion| suggestion.label.as_str())
                .collect::<Vec<_>>(),
            vec!["child"]
        );
    }

    #[test]
    fn address_undo_and_redo_refresh_suggestions() {
        let temp = TempDir::new();
        fs::create_dir(temp.path().join("child")).expect("create child");
        fs::create_dir(temp.path().join("other")).expect("create other");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.active_address_bar = Some(AddressBarState::new(String::new(), None));

        view.replace_address_text_in_range(None, "ch");
        let address = view.active_address_bar.as_ref().expect("address edit");
        assert_eq!(address.content, "ch");
        assert_eq!(
            address
                .suggestions
                .iter()
                .map(|suggestion| suggestion.label.as_str())
                .collect::<Vec<_>>(),
            vec!["child"]
        );

        view.undo_address_text();
        let address = view.active_address_bar.as_ref().expect("address edit");
        assert_eq!(address.content, "");
        assert_eq!(
            address
                .suggestions
                .iter()
                .map(|suggestion| suggestion.label.as_str())
                .collect::<Vec<_>>(),
            vec!["child", "other"]
        );

        view.redo_address_text();
        let address = view.active_address_bar.as_ref().expect("address edit");
        assert_eq!(address.content, "ch");
        assert_eq!(
            address
                .suggestions
                .iter()
                .map(|suggestion| suggestion.label.as_str())
                .collect::<Vec<_>>(),
            vec!["child"]
        );
    }

    #[test]
    fn triple_click_selection_selects_entire_address_text() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.active_address_bar = Some(AddressBarState::new("alpha/beta gamma".to_owned(), None));

        view.on_address_mouse_down(&MouseDownEvent {
            button: MouseButton::Left,
            click_count: 3,
            ..MouseDownEvent::default()
        });

        let address = view.active_address_bar.as_ref().expect("address edit");
        assert_eq!(address.selected_range, 0..address.content.len());
    }

    #[gpui::test]
    fn address_action_handlers_edit_text_clipboard_suggestions_and_navigation(
        cx: &mut TestAppContext,
    ) {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir(&child).expect("create child");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.handle_address_edit(&AddressEdit, window, cx);
                assert!(view.address_bar_is_editing());
                let address = view.active_address_bar.as_ref().expect("address edit");
                assert_eq!(address.selected_range, 0..address.content.len());

                view.handle_address_select_all(&AddressSelectAll, window, cx);
                view.handle_address_copy(&AddressCopy, window, cx);
                assert_eq!(
                    cx.read_from_clipboard().and_then(|item| item.text()),
                    Some(format_address_path(temp.path(), AddressSlash::Forward))
                );

                view.handle_address_cut(&AddressCut, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .content,
                    ""
                );

                cx.write_to_clipboard(ClipboardItem::new_string("child\n".to_owned()));
                view.handle_address_paste(&AddressPaste, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .content,
                    "child "
                );

                set_active_address(view, "alpha beta");
                view.handle_address_left(&AddressLeft, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .selected_range,
                    "alpha bet".len().."alpha bet".len()
                );
                view.handle_address_right(&AddressRight, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .selected_range,
                    "alpha beta".len().."alpha beta".len()
                );

                view.handle_address_word_left(&AddressWordLeft, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .selected_range,
                    "alpha ".len().."alpha ".len()
                );
                view.handle_address_word_right(&AddressWordRight, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .selected_range,
                    "alpha beta".len().."alpha beta".len()
                );

                set_active_address(view, "alpha beta");
                view.handle_address_select_left(&AddressSelectLeft, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .selected_range,
                    "alpha bet".len().."alpha beta".len()
                );

                set_active_address(view, "alpha beta");
                view.active_address_bar
                    .as_mut()
                    .expect("address edit")
                    .move_to(0);
                view.handle_address_select_right(&AddressSelectRight, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .selected_range,
                    0.."a".len()
                );

                set_active_address(view, "alpha beta");
                view.handle_address_select_word_left(&AddressSelectWordLeft, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .selected_range,
                    "alpha ".len().."alpha beta".len()
                );

                set_active_address(view, "alpha beta");
                view.active_address_bar
                    .as_mut()
                    .expect("address edit")
                    .move_to(0);
                view.handle_address_select_word_right(&AddressSelectWordRight, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .selected_range,
                    0.."alpha ".len()
                );

                view.handle_address_home(&AddressHome, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .selected_range,
                    0..0
                );
                view.handle_address_end(&AddressEnd, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .selected_range,
                    "alpha beta".len().."alpha beta".len()
                );
                view.handle_address_select_home(&AddressSelectHome, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .selected_range,
                    0.."alpha beta".len()
                );

                set_active_address(view, "alpha beta");
                view.active_address_bar
                    .as_mut()
                    .expect("address edit")
                    .move_to(0);
                view.handle_address_select_end(&AddressSelectEnd, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .selected_range,
                    0.."alpha beta".len()
                );

                set_active_address(view, "alpha");
                view.handle_address_backspace(&AddressBackspace, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .content,
                    "alph"
                );

                set_active_address(view, "alpha");
                view.active_address_bar
                    .as_mut()
                    .expect("address edit")
                    .move_to(0);
                view.handle_address_delete(&AddressDelete, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .content,
                    "lpha"
                );

                set_active_address(view, "alpha beta");
                view.handle_address_backspace_word(&AddressBackspaceWord, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .content,
                    "alpha "
                );

                set_active_address(view, "ch");
                view.refresh_address_suggestions();
                view.handle_address_suggestion_down(&AddressSuggestionDown, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .highlighted_suggestion,
                    Some(0)
                );
                view.handle_address_suggestion_up(&AddressSuggestionUp, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .highlighted_suggestion,
                    Some(0)
                );

                view.handle_address_accept_suggestion(&AddressAcceptSuggestion, window, cx);
                assert_eq!(
                    view.active_address_bar
                        .as_ref()
                        .expect("address edit")
                        .content,
                    format_address_path(&child, AddressSlash::Forward)
                );

                view.handle_address_commit(&AddressCommit, window, cx);
                assert_eq!(
                    view.path,
                    explorer_visible_address_path(fs::canonicalize(&child).unwrap())
                );
                assert!(!view.address_bar_is_editing());

                view.navigate_to_directory(temp.path().to_path_buf(), HistoryMode::Record);
                set_active_address(view, "ch");
                view.refresh_address_suggestions();
                view.active_address_bar
                    .as_mut()
                    .expect("address edit")
                    .highlighted_suggestion = Some(0);
                view.handle_address_right(&AddressRight, window, cx);
                assert_eq!(view.path, child);
                assert!(view.address_bar_is_editing());

                set_active_address(view, "missing");
                view.handle_address_commit(&AddressCommit, window, cx);
                assert!(view.address_bar_is_editing());
                assert!(view.operation_notice.is_some());

                view.handle_address_cancel(&AddressCancel, window, cx);
                assert!(!view.address_bar_is_editing());
            });
        });
    }

    fn set_active_address(view: &mut ExplorerView, text: &str) {
        let mut address = AddressBarState::new(text.to_owned(), None);
        address.move_to(text.len());
        view.active_address_bar = Some(address);
        view.refresh_address_suggestions();
    }
}
