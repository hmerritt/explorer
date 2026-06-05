use std::{
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
        AddressAcceptSuggestion, AddressBackspace, AddressCancel, AddressCommit, AddressCopy,
        AddressCut, AddressDelete, AddressEdit, AddressEnd, AddressHome, AddressLeft, AddressPaste,
        AddressRight, AddressSelectAll, AddressSelectEnd, AddressSelectHome, AddressSelectLeft,
        AddressSelectRight, AddressSelectWordLeft, AddressSelectWordRight, AddressSuggestionDown,
        AddressSuggestionUp, AddressWordLeft, AddressWordRight,
    },
    navigation::HistoryMode,
    text_input::{
        EDITABLE_TEXT_SELECTION_BACKGROUND, EditableTextState, editable_text_runs,
        scroll_offset_for_cursor,
    },
    view::ExplorerView,
};

const MAX_ADDRESS_SUGGESTIONS: usize = 8;

pub(super) struct AddressBarState {
    text: EditableTextState,
    pub(super) focus_handle: Option<FocusHandle>,
    pub(super) focus_out: Option<Subscription>,
    pub(super) suggestions: Vec<AddressBarSuggestion>,
    pub(super) highlighted_suggestion: Option<usize>,
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
        }
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
            if address.selected_range.is_empty() {
                let offset = address.previous_boundary(address.cursor_offset());
                address.select_to(offset);
            }
            replace_address_text(address, None, "");
            self.refresh_address_suggestions();
        }
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
            if address.selected_range.is_empty() {
                let offset = address.next_boundary(address.cursor_offset());
                address.select_to(offset);
            }
            replace_address_text(address, None, "");
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
        if let Some(address) = self.active_address_bar.as_mut() {
            let offset = if address.selected_range.is_empty() {
                address.previous_boundary(address.cursor_offset())
            } else {
                address.selected_range.start
            };
            address.move_to(offset);
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
        if let Some(address) = self.active_address_bar.as_mut() {
            let offset = if address.selected_range.is_empty() {
                address.next_boundary(address.cursor_offset())
            } else {
                address.selected_range.end
            };
            address.move_to(offset);
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
                replace_address_text(address, None, "");
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
            replace_address_text(address, None, &text.replace(['\r', '\n'], " "));
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
        self.open_utility_menu = None;

        if !self.commit_active_rename_before_interaction(window, cx) {
            return false;
        }

        let focus_handle = cx.focus_handle();
        let mut address =
            AddressBarState::new(self.path.display().to_string(), Some(focus_handle.clone()));
        address.suggestions = folder_suggestions_for_input(&address.content, &self.path);

        focus_handle.focus(window);
        let subscription = cx.on_focus_out(&focus_handle, window, |this, _, window, cx| {
            this.cancel_address_bar_edit();
            this.focus_explorer(window);
            cx.notify();
        });
        address.focus_out = Some(subscription);
        self.active_address_bar = Some(address);
        self.open_error = None;
        true
    }

    pub(super) fn cancel_address_bar_edit(&mut self) {
        if let Some(mut address) = self.active_address_bar.take() {
            address.focus_out = None;
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

    pub(super) fn navigate_to_address_suggestion(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(path) = self
            .active_address_bar
            .as_ref()
            .and_then(|address| address.suggestions.get(index))
            .map(|suggestion| suggestion.path.clone())
        else {
            return false;
        };

        self.finish_address_navigation(path, window, cx);
        true
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
                self.open_error = None;
                Some(path)
            }
            Err(error) => {
                self.open_error = Some(error);
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
        let Some(address) = self.active_address_bar.as_mut() else {
            return false;
        };
        let index = address.highlighted_suggestion.or(Some(0));
        let Some(suggestion) = index.and_then(|index| address.suggestions.get(index)) else {
            return false;
        };

        address.content = suggestion.path.display().to_string();
        address.selected_range = address.content.len()..address.content.len();
        address.selection_reversed = false;
        address.marked_range = None;
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
        if let Some(address) = self.active_address_bar.as_mut() {
            address.suggestions = folder_suggestions_for_input(&address.content, &current_path);
            if address
                .highlighted_suggestion
                .is_some_and(|index| index >= address.suggestions.len())
            {
                address.highlighted_suggestion = None;
            }
        }
    }

    fn select_active_address_text(&mut self) {
        if let Some(address) = self.active_address_bar.as_mut() {
            address.select_all();
        }
    }
}

pub(super) fn resolve_address_input(input: &str, current_path: &Path) -> Result<PathBuf, String> {
    let cleaned = cleaned_address_input(input);
    if cleaned.is_empty() {
        return Err("The address is empty.".to_owned());
    }

    let typed_path = Path::new(&cleaned);
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

    Ok(fs::canonicalize(&candidate).unwrap_or(candidate))
}

pub(super) fn folder_suggestions_for_input(
    input: &str,
    current_path: &Path,
) -> Vec<AddressBarSuggestion> {
    let cleaned = cleaned_address_input(input);
    let (parent, prefix) = suggestion_parent_and_prefix(&cleaned, current_path);
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
            let path = entry.path();
            if !path.is_dir() {
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
    suggestions.truncate(MAX_ADDRESS_SUGGESTIONS);
    suggestions
}

fn suggestion_parent_and_prefix(input: &str, current_path: &Path) -> (PathBuf, String) {
    if input.is_empty() {
        return (current_path.to_path_buf(), String::new());
    }

    let typed_path = Path::new(input);
    let (parent, prefix) = if has_trailing_separator(input) {
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
    if trimmed.len() >= 2 {
        let bytes = trimmed.as_bytes();
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return trimmed[1..trimmed.len() - 1].trim().to_owned();
        }
    }

    trimmed.to_owned()
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
) {
    address.replace_text(range, new_text);
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
    use crate::explorer::{test_support::TempDir, view::ExplorerView};
    use gpui::{Modifiers, MouseButton};
    use std::fs;

    #[test]
    fn resolve_address_accepts_absolute_and_relative_directories() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir(&child).expect("create child");

        assert_eq!(
            resolve_address_input(&child.display().to_string(), temp.path()).unwrap(),
            fs::canonicalize(&child).unwrap()
        );
        assert_eq!(
            resolve_address_input("child", temp.path()).unwrap(),
            fs::canonicalize(&child).unwrap()
        );
    }

    #[test]
    fn resolve_address_accepts_dot_dot_and_quotes() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir(&child).expect("create child");

        assert_eq!(
            resolve_address_input(".", &child).unwrap(),
            fs::canonicalize(&child).unwrap()
        );
        assert_eq!(
            resolve_address_input("..", &child).unwrap(),
            fs::canonicalize(temp.path()).unwrap()
        );
        assert_eq!(
            resolve_address_input(&format!(" \"{}\" ", child.display()), temp.path()).unwrap(),
            fs::canonicalize(&child).unwrap()
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

    #[test]
    fn folder_suggestions_match_folders_only_case_insensitively() {
        let temp = TempDir::new();
        fs::create_dir(temp.path().join("Alpha")).expect("create alpha");
        fs::create_dir(temp.path().join("apricot")).expect("create apricot");
        fs::write(temp.path().join("apple.txt"), b"data").expect("write file");

        let suggestions = folder_suggestions_for_input("a", temp.path());
        let labels = suggestions
            .iter()
            .map(|suggestion| suggestion.label.as_str())
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["Alpha", "apricot"]);
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
        );

        assert_eq!(suggestions.len(), 2);
        assert_eq!(suggestions[0].label, "child-a");
        assert_eq!(suggestions[1].label, "child-b");
    }

    #[test]
    fn folder_suggestions_limit_results_deterministically() {
        let temp = TempDir::new();
        for index in 0..12 {
            fs::create_dir(temp.path().join(format!("folder-{index:02}"))).expect("create folder");
        }

        let suggestions = folder_suggestions_for_input("folder", temp.path());

        assert_eq!(suggestions.len(), MAX_ADDRESS_SUGGESTIONS);
        assert_eq!(suggestions[0].label, "folder-00");
        assert_eq!(suggestions[7].label, "folder-07");
    }

    #[test]
    fn address_start_selects_full_current_path() {
        let temp = TempDir::new();
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.active_address_bar = Some(AddressBarState::new(
            temp.path().display().to_string(),
            None,
        ));

        let address = view.active_address_bar.as_ref().unwrap();
        assert_eq!(address.selected_range, 0..address.content.len());
    }

    #[test]
    fn address_commit_target_preserves_current_path_on_failure() {
        let temp = TempDir::new();
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.active_address_bar = Some(AddressBarState::new("missing".to_owned(), None));

        assert_eq!(view.address_commit_target(), None);
        assert_eq!(view.path, temp.path());
        assert!(view.active_address_bar.is_some());
        assert!(view.open_error.is_some());
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
}
