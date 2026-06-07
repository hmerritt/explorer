use std::{
    ops::{Deref, DerefMut, Range},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[cfg(debug_assertions)]
use std::time::Instant;

#[cfg(debug_assertions)]
macro_rules! recursive_search_timing {
    ($generation:expr, $elapsed:expr, $($message:tt)*) => {
        eprintln!(
            "[recursive-search:{}] {:<10.3?} {}",
            $generation,
            $elapsed,
            format_args!($($message)*)
        );
    };
}

use globset::{GlobBuilder, GlobMatcher};
use gpui::{
    App, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler, Entity,
    FocusHandle, GlobalElementId, IntoElement, KeyDownEvent, LayoutId, Modifiers, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, ShapedLine, Style, Subscription, Task,
    TextRun, UTF16Selection, Window, fill, point, px, relative, rgb, size,
};

use crate::explorer::{
    actions::{
        SearchBackspace, SearchBackspaceWord, SearchCancel, SearchCommit, SearchCopy, SearchCut,
        SearchDelete, SearchEdit, SearchEnd, SearchHome, SearchLeft, SearchPaste, SearchRight,
        SearchSelectAll, SearchSelectEnd, SearchSelectHome, SearchSelectLeft, SearchSelectRight,
        SearchSelectWordLeft, SearchSelectWordRight, SearchWordLeft, SearchWordRight,
    },
    entry::FileEntry,
    recursive_search::{
        RecursiveSearchCache, RecursiveSearchOutput, RecursiveSearchProgress,
        RecursiveSearchProgressSnapshot, recursive_search_entries,
    },
    text_input::{
        EDITABLE_TEXT_SELECTION_BACKGROUND, EditableTextState, editable_text_runs,
        scroll_offset_for_cursor,
    },
    view::ExplorerView,
};

const MIN_RECURSIVE_SEARCH_QUERY_CHARS: usize = 2;

pub(super) struct SearchState {
    text: EditableTextState,
    pub(super) focus_handle: Option<FocusHandle>,
    pub(super) focus_out: Option<Subscription>,
    pub(super) recursive_enabled: bool,
    pub(super) recursive_generation: u64,
    pub(super) recursive_status: RecursiveSearchStatus,
    pub(super) recursive_progress: RecursiveSearchProgressSnapshot,
    pub(super) recursive_results_active: bool,
    pub(super) recursive_cache: Option<RecursiveSearchCache>,
    pub(super) recursive_cancel: Option<Arc<AtomicBool>>,
    pub(super) recursive_task: Option<Task<()>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum RecursiveSearchStatus {
    #[default]
    Idle,
    Searching,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            text: EditableTextState::with_selection(String::new(), 0..0),
            focus_handle: None,
            focus_out: None,
            recursive_enabled: false,
            recursive_generation: 0,
            recursive_status: RecursiveSearchStatus::Idle,
            recursive_progress: RecursiveSearchProgressSnapshot::Searching(None),
            recursive_results_active: false,
            recursive_cache: None,
            recursive_cancel: None,
            recursive_task: None,
        }
    }
}

impl Deref for SearchState {
    type Target = EditableTextState;

    fn deref(&self) -> &Self::Target {
        &self.text
    }
}

impl DerefMut for SearchState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.text
    }
}

enum SearchMatcher {
    All,
    Substring(String),
    Glob(Option<GlobMatcher>),
}

impl SearchMatcher {
    fn new(query: &str) -> Self {
        if query.is_empty() {
            return Self::All;
        }

        if query.chars().any(|ch| matches!(ch, '*' | '?' | '[' | ']')) {
            let matcher = GlobBuilder::new(query)
                .case_insensitive(true)
                .literal_separator(true)
                .build()
                .ok()
                .map(|glob| glob.compile_matcher());
            Self::Glob(matcher)
        } else {
            Self::Substring(query.to_lowercase())
        }
    }

    fn matches(&self, name: &str) -> bool {
        match self {
            Self::All => true,
            Self::Substring(query) => name.to_lowercase().contains(query),
            Self::Glob(Some(matcher)) => matcher.is_match(name),
            Self::Glob(None) => false,
        }
    }
}

pub(super) fn filtered_entries(entries: &[FileEntry], query: &str) -> Vec<FileEntry> {
    let matcher = SearchMatcher::new(query);
    entries
        .iter()
        .filter(|entry| matcher.matches(&entry.name))
        .cloned()
        .collect()
}

fn recursive_search_query_is_ready(query: &str) -> bool {
    query.chars().count() >= MIN_RECURSIVE_SEARCH_QUERY_CHARS
}

impl ExplorerView {
    pub(super) fn handle_type_to_search(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.has_active_text_input() || event.keystroke.modifiers != Modifiers::none() {
            return;
        }

        let Some(text) = event.keystroke.key_char.as_deref().filter(|text| {
            !text.is_empty() && text.chars().all(|character| !character.is_control())
        }) else {
            return;
        };

        if self.start_search_edit(window, cx) {
            self.replace_search_text(None, text, cx);
            cx.stop_propagation();
            cx.notify();
        }
    }

    pub(super) fn search_query(&self) -> &str {
        &self.search.content
    }

    pub(super) fn search_is_active(&self) -> bool {
        !self.search.content.is_empty()
    }

    pub(super) fn recursive_search_is_enabled(&self) -> bool {
        self.search.recursive_enabled
    }

    pub(super) fn recursive_search_is_working(&self) -> bool {
        self.search.recursive_status == RecursiveSearchStatus::Searching
    }

    pub(super) fn recursive_search_results_active(&self) -> bool {
        self.search.recursive_results_active
    }

    pub(super) fn recursive_search_progress(&self) -> RecursiveSearchProgressSnapshot {
        self.search.recursive_progress
    }

    pub(super) fn search_is_editing(&self) -> bool {
        self.search.focus_handle.is_some()
    }

    pub(super) fn active_search_focus_handle(&self) -> Option<FocusHandle> {
        self.search.focus_handle.clone()
    }

    pub(super) fn search_placeholder(&self) -> String {
        let folder = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("this folder");
        format!("Search {folder}")
    }

    pub(super) fn set_search_query(&mut self, query: String) {
        let selected_paths = self.selected_paths();
        self.search.content = query;
        let end = self.search.content.len();
        self.search.selected_range = end..end;
        self.search.selection_reversed = false;
        self.search.marked_range = None;
        self.cancel_recursive_search();
        self.apply_search_filter_preserving_selection(&selected_paths);
        self.scroll_to_top();
    }

    pub(super) fn clear_search(&mut self) {
        self.set_search_query(String::new());
    }

    pub(super) fn apply_search_filter_preserving_selection(
        &mut self,
        selected_paths: &[std::path::PathBuf],
    ) {
        self.search.recursive_results_active = false;
        self.entries = filtered_entries(&self.all_entries, &self.search.content);
        self.restore_selection_from_paths(selected_paths);
    }

    fn restore_normal_entries_preserving_selection(&mut self, selected_paths: &[PathBuf]) {
        self.search.recursive_results_active = false;
        self.entries = self.all_entries.clone();
        self.restore_selection_from_paths(selected_paths);
    }

    fn reset_recursive_search_below_minimum(&mut self, selected_paths: &[PathBuf]) {
        self.cancel_recursive_search();
        self.restore_normal_entries_preserving_selection(selected_paths);
    }

    pub(super) fn invalidate_recursive_search_cache(&mut self) {
        self.search.recursive_cache = None;
        self.cancel_recursive_search();
    }

    pub(super) fn toggle_recursive_search(&mut self, cx: &mut Context<Self>) {
        let selected_paths = self.selected_paths();
        self.search.recursive_enabled = !self.search.recursive_enabled;
        self.search.recursive_generation = self.search.recursive_generation.wrapping_add(1);
        self.search.recursive_cache = None;
        self.refresh_search_filter_with_selection(&selected_paths, cx);
    }

    pub(super) fn refresh_search_after_external_change(&mut self, cx: &mut Context<Self>) {
        let selected_paths = self.selected_paths();
        self.refresh_search_filter_with_selection(&selected_paths, cx);
    }

    pub(super) fn start_search_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.cancel_pending_click_rename();
        self.open_utility_menu = None;
        self.cancel_address_bar_edit();

        if !self.commit_active_rename_before_interaction(window, cx) {
            return false;
        }

        if let Some(focus_handle) = self.search.focus_handle.clone() {
            self.search.select_all();
            focus_handle.focus(window);
            return true;
        }

        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        let subscription = cx.on_focus_out(&focus_handle, window, |this, _, _, cx| {
            this.finish_search_edit();
            cx.notify();
        });
        self.search.focus_handle = Some(focus_handle);
        self.search.focus_out = Some(subscription);
        self.search.select_all();
        self.open_error = None;
        true
    }

    pub(super) fn finish_search_edit(&mut self) {
        self.search.focus_out = None;
        self.search.focus_handle = None;
        self.search.is_selecting = false;
        self.search.marked_range = None;
    }

    pub(super) fn handle_search_edit(
        &mut self,
        _: &SearchEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_search_edit(window, cx);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_commit(
        &mut self,
        _: &SearchCommit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_search_edit();
        self.focus_explorer(window);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_cancel(
        &mut self,
        _: &SearchCancel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_search();
        self.finish_search_edit();
        self.focus_explorer(window);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_backspace(
        &mut self,
        _: &SearchBackspace,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search.selected_range.is_empty() {
            let offset = self.search.previous_boundary(self.search.cursor_offset());
            self.search.select_to(offset);
        }
        self.replace_search_text(None, "", cx);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_backspace_word(
        &mut self,
        _: &SearchBackspaceWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search.delete_previous_word_or_selection();
        self.refresh_search_filter(cx);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_delete(
        &mut self,
        _: &SearchDelete,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search.selected_range.is_empty() {
            let offset = self.search.next_boundary(self.search.cursor_offset());
            self.search.select_to(offset);
        }
        self.replace_search_text(None, "", cx);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_left(
        &mut self,
        _: &SearchLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = if self.search.selected_range.is_empty() {
            self.search.previous_boundary(self.search.cursor_offset())
        } else {
            self.search.selected_range.start
        };
        self.search.move_to(offset);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_right(
        &mut self,
        _: &SearchRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = if self.search.selected_range.is_empty() {
            self.search.next_boundary(self.search.cursor_offset())
        } else {
            self.search.selected_range.end
        };
        self.search.move_to(offset);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_select_left(
        &mut self,
        _: &SearchSelectLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.search.previous_boundary(self.search.cursor_offset());
        self.search.select_to(offset);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_select_right(
        &mut self,
        _: &SearchSelectRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.search.next_boundary(self.search.cursor_offset());
        self.search.select_to(offset);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_word_left(
        &mut self,
        _: &SearchWordLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self
            .search
            .previous_word_boundary(self.search.cursor_offset());
        self.search.move_to(offset);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_word_right(
        &mut self,
        _: &SearchWordRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.search.next_word_boundary(self.search.cursor_offset());
        self.search.move_to(offset);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_select_word_left(
        &mut self,
        _: &SearchSelectWordLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self
            .search
            .previous_word_boundary(self.search.cursor_offset());
        self.search.select_to(offset);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_select_word_right(
        &mut self,
        _: &SearchSelectWordRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.search.next_word_boundary(self.search.cursor_offset());
        self.search.select_to(offset);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_home(
        &mut self,
        _: &SearchHome,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search.move_to(0);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_end(
        &mut self,
        _: &SearchEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let end = self.search.content.len();
        self.search.move_to(end);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_select_home(
        &mut self,
        _: &SearchSelectHome,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search.select_to(0);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_select_end(
        &mut self,
        _: &SearchSelectEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let end = self.search.content.len();
        self.search.select_to(end);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_select_all(
        &mut self,
        _: &SearchSelectAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search.select_all();
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_copy(
        &mut self,
        _: &SearchCopy,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = self.search.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_cut(
        &mut self,
        _: &SearchCut,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = self.search.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.replace_search_text(None, "", cx);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn handle_search_paste(
        &mut self,
        _: &SearchPaste,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_search_text(None, &text.replace(['\r', '\n'], " "), cx);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn on_search_mouse_down(&mut self, event: &MouseDownEvent) {
        self.search.is_selecting = true;
        let offset = self.search.index_for_mouse_position(event.position);
        if event.click_count >= 3 {
            self.search.select_all();
        } else if event.click_count == 2 {
            self.search.select_word_at(offset);
        } else if event.modifiers.shift {
            self.search.select_to(offset);
        } else {
            self.search.move_to(offset);
        }
    }

    pub(super) fn on_search_mouse_move(&mut self, event: &MouseMoveEvent) {
        if self.search.is_selecting {
            let offset = self.search.index_for_mouse_position(event.position);
            self.search.select_to(offset);
        }
    }

    pub(super) fn on_search_mouse_up(&mut self, _: &MouseUpEvent) {
        self.search.is_selecting = false;
    }

    pub(super) fn update_search_layout(&mut self, line: ShapedLine, bounds: Bounds<Pixels>) {
        self.search.update_layout(line, bounds);
    }

    pub(super) fn search_text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
    ) -> Option<String> {
        let range = self.search.range_from_utf16(&range_utf16);
        actual_range.replace(self.search.range_to_utf16(&range));
        Some(self.search.content[range].to_owned())
    }

    pub(super) fn selected_search_text_range(&self) -> UTF16Selection {
        self.search.selected_text_range_utf16()
    }

    pub(super) fn replace_search_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        self.search
            .replace_text_in_range_utf16(range_utf16, &text.replace(['\r', '\n'], " "));
        self.refresh_search_filter(cx);
    }

    pub(super) fn replace_and_mark_search_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        selected_range_utf16: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        self.search.replace_and_mark_text_in_range_utf16(
            range_utf16,
            &text.replace(['\r', '\n'], " "),
            selected_range_utf16,
        );
        self.refresh_search_filter(cx);
    }

    fn replace_search_text(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        self.search.replace_text(range, text);
        self.refresh_search_filter(cx);
    }

    fn refresh_search_filter(&mut self, cx: &mut Context<Self>) {
        let selected_paths = self.selected_paths();
        self.refresh_search_filter_with_selection(&selected_paths, cx);
        self.scroll_to_top();
    }

    fn refresh_search_filter_with_selection(
        &mut self,
        selected_paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) {
        if self.search.recursive_enabled {
            if !recursive_search_query_is_ready(&self.search.content) {
                self.reset_recursive_search_below_minimum(selected_paths);
                return;
            };
            self.schedule_recursive_search(cx);
        } else {
            self.cancel_recursive_search();
            self.apply_search_filter_preserving_selection(selected_paths);
        }
    }

    fn schedule_recursive_search(&mut self, cx: &mut Context<Self>) {
        self.cancel_recursive_search();
        self.search.recursive_generation = self.search.recursive_generation.wrapping_add(1);
        self.search.recursive_status = RecursiveSearchStatus::Searching;
        self.search.recursive_progress = RecursiveSearchProgressSnapshot::Searching(None);
        self.search.recursive_results_active = true;
        self.entries.clear();
        self.clear_selection();

        let generation = self.search.recursive_generation;
        let root = self.path.clone();
        let query = self.search.content.clone();
        let show_hidden_files = self.show_hidden_files;
        #[cfg(debug_assertions)]
        let cache_clone_started = Instant::now();
        let cached_search = self
            .search
            .recursive_cache
            .as_ref()
            .filter(|cache| cache.root == root && cache.show_hidden_files == show_hidden_files)
            .cloned();
        self.search.recursive_progress = RecursiveSearchProgressSnapshot::Searching(
            cached_search
                .as_ref()
                .map(|cache| cache.paths.len())
                .filter(|count| *count > 0),
        );
        #[cfg(debug_assertions)]
        recursive_search_timing!(
            generation,
            cache_clone_started.elapsed(),
            "schedule.cache_clone cache_hit={} paths={}",
            cached_search.is_some(),
            cached_search.as_ref().map_or(0, |cache| cache.paths.len())
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(RecursiveSearchProgress::default());
        self.search.recursive_cancel = Some(cancel.clone());

        #[cfg(debug_assertions)]
        let schedule_started = Instant::now();
        let task = cx.spawn(async move |this, cx| {
            #[cfg(debug_assertions)]
            let debounce_started = Instant::now();
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            #[cfg(debug_assertions)]
            recursive_search_timing!(generation, debounce_started.elapsed(), "schedule.debounce");
            if cancel.load(Ordering::Relaxed) {
                #[cfg(debug_assertions)]
                recursive_search_timing!(
                    generation,
                    schedule_started.elapsed(),
                    "schedule.cancelled_before_work cancelled=true"
                );
                return;
            }

            let finished = Arc::new(AtomicBool::new(false));
            let output_task = cx.background_executor().spawn({
                let finished = finished.clone();
                let progress = progress.clone();
                {
                    let cancel = cancel.clone();
                    async move {
                        let output = recursive_search_entries(
                            generation,
                            root,
                            query,
                            show_hidden_files,
                            cached_search,
                            cancel,
                            progress,
                        );
                        finished.store(true, Ordering::Relaxed);
                        output
                    }
                }
            });

            while !finished.load(Ordering::Relaxed) {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let progress = progress.snapshot();
                let _ = this.update(cx, |explorer, cx| {
                    if explorer.update_recursive_search_progress(generation, progress) {
                        cx.notify();
                    }
                });
            }

            let output = output_task.await;

            let _ = this.update(cx, |explorer, cx| {
                explorer.apply_recursive_search_output(output);
                cx.notify();
            });
        });
        self.search.recursive_task = Some(task);
    }

    fn apply_recursive_search_output(&mut self, output: RecursiveSearchOutput) {
        if !self.search.recursive_enabled
            || self.search.recursive_generation != output.generation
            || self.path != output.root
            || self.search.content != output.query
            || self.show_hidden_files != output.show_hidden_files
        {
            return;
        }

        let selected_paths = self.selected_paths();
        self.search.recursive_status = RecursiveSearchStatus::Idle;
        self.search.recursive_progress = RecursiveSearchProgressSnapshot::Searching(None);
        self.search.recursive_cancel = None;
        self.search.recursive_task = None;
        self.search.recursive_results_active = true;
        self.search.recursive_cache = Some(RecursiveSearchCache {
            root: output.root,
            show_hidden_files: output.show_hidden_files,
            paths: output.scanned_paths,
        });
        self.entries = output.entries;
        self.restore_selection_from_paths(&selected_paths);
        self.scroll_to_top();
    }

    fn cancel_recursive_search(&mut self) {
        if let Some(cancel) = self.search.recursive_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
            #[cfg(debug_assertions)]
            eprintln!(
                "[recursive-search:{}] {:<10} schedule.cancel_requested cancelled=true",
                self.search.recursive_generation, "-"
            );
        }
        self.search.recursive_task = None;
        self.search.recursive_status = RecursiveSearchStatus::Idle;
        self.search.recursive_progress = RecursiveSearchProgressSnapshot::Searching(None);
        self.search.recursive_results_active = false;
    }

    fn update_recursive_search_progress(
        &mut self,
        generation: u64,
        progress: RecursiveSearchProgressSnapshot,
    ) -> bool {
        if self.search.recursive_generation != generation
            || self.search.recursive_status != RecursiveSearchStatus::Searching
            || self.search.recursive_progress == progress
        {
            return false;
        }

        self.search.recursive_progress = progress;
        true
    }
}

pub(super) struct SearchTextElement {
    entity: Entity<ExplorerView>,
}

pub(super) fn search_text_element(entity: Entity<ExplorerView>) -> SearchTextElement {
    SearchTextElement { entity }
}

pub(super) struct SearchPrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    scroll_offset: Pixels,
}

impl IntoElement for SearchTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SearchTextElement {
    type RequestLayoutState = ();
    type PrepaintState = SearchPrepaintState;

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
        let search = &self.entity.read(cx).search;
        let content = gpui::SharedString::from(search.content.clone());
        let selected_range = search.selected_range.clone();
        let cursor = search.cursor_offset();
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
            search.marked_range.as_ref(),
        );
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(content, font_size, &runs, None);
        let scroll_offset = scroll_offset_for_cursor(
            search.scroll_offset,
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

        SearchPrepaintState {
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
        if let Some(focus_handle) = self.entity.read(cx).active_search_focus_handle() {
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, self.entity.clone()),
                cx,
            );
        }
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        let line = prepaint.line.take().expect("search text line");
        line.paint(
            point(bounds.origin.x - prepaint.scroll_offset, bounds.origin.y),
            window.line_height(),
            window,
            cx,
        )
        .expect("paint search text");
        if self
            .entity
            .read(cx)
            .active_search_focus_handle()
            .is_some_and(|focus| focus.is_focused(window))
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.entity
            .update(cx, |view, _| view.update_search_layout(line, bounds));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::{
        entry::FileEntry,
        navigation::HistoryMode,
        test_support::{TempDir, test_view_with_entries},
    };
    use std::fs;

    fn names(entries: &[FileEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.name.as_str()).collect()
    }

    #[test]
    fn plain_text_search_is_case_insensitive_substring_match() {
        let entries = vec![
            FileEntry::test("Annual Report.txt", false, Some(1), None),
            FileEntry::test("notes.txt", false, Some(1), None),
        ];
        assert_eq!(
            names(&filtered_entries(&entries, "REPORT")),
            vec!["Annual Report.txt"]
        );
    }

    #[test]
    fn glob_search_supports_question_star_and_character_ranges() {
        let entries = vec![
            FileEntry::test("file1.txt", false, Some(1), None),
            FileEntry::test("fileA.txt", false, Some(1), None),
            FileEntry::test("file10.txt", false, Some(1), None),
            FileEntry::test("image.png", false, Some(1), None),
        ];
        assert_eq!(
            names(&filtered_entries(&entries, "file?.txt")),
            vec!["file1.txt", "fileA.txt"]
        );
        assert_eq!(
            names(&filtered_entries(&entries, "*.txt")),
            vec!["file1.txt", "fileA.txt", "file10.txt"]
        );
        assert_eq!(
            names(&filtered_entries(&entries, "file[0-9].txt")),
            vec!["file1.txt"]
        );
    }

    #[test]
    fn invalid_glob_has_no_matches() {
        let entries = vec![FileEntry::test("file.txt", false, Some(1), None)];
        assert!(filtered_entries(&entries, "file[.txt").is_empty());
    }

    #[test]
    fn empty_query_matches_files_and_directories() {
        let entries = vec![
            FileEntry::test("folder", true, None, None),
            FileEntry::test("file.txt", false, Some(1), None),
        ];
        assert_eq!(
            names(&filtered_entries(&entries, "")),
            vec!["folder", "file.txt"]
        );
    }

    #[test]
    fn filtering_preserves_order_and_only_visible_selection() {
        let mut view = test_view_with_entries(&["a.txt", "b.png", "c.txt"]);
        view.select_all_entries();
        view.set_search_query("*.txt".to_owned());
        assert_eq!(names(&view.entries), vec!["a.txt", "c.txt"]);
        assert_eq!(view.selected_paths().len(), 2);

        view.clear_search();
        assert_eq!(names(&view.entries), vec!["a.txt", "b.png", "c.txt"]);
        assert_eq!(view.selected_paths().len(), 2);
    }

    #[test]
    fn reload_reapplies_active_filter() {
        let temp = TempDir::new();
        fs::write(temp.path().join("a.txt"), b"a").expect("create txt");
        fs::write(temp.path().join("b.png"), b"b").expect("create png");
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.set_search_query("*.txt".to_owned());

        fs::write(temp.path().join("c.txt"), b"c").expect("create second txt");
        view.reload();

        assert_eq!(view.search_query(), "*.txt");
        assert_eq!(names(&view.entries), vec!["a.txt", "c.txt"]);
        assert_eq!(view.all_entries.len(), 3);
    }

    #[test]
    fn navigation_clears_active_filter_but_same_folder_reload_keeps_it() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir(&child).expect("create child");
        fs::write(temp.path().join("a.txt"), b"a").expect("create file");
        fs::write(child.join("b.png"), b"b").expect("create child file");
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.set_search_query("*.txt".to_owned());

        view.navigate_to_directory(temp.path().to_path_buf(), HistoryMode::Record);
        assert_eq!(view.search_query(), "*.txt");

        view.navigate_to_directory(child, HistoryMode::Record);
        assert_eq!(view.search_query(), "");
        assert_eq!(names(&view.entries), vec!["b.png"]);
    }

    #[test]
    fn empty_recursive_query_shows_current_folder_entries() {
        let mut view = test_view_with_entries(&["a.txt", "b.png"]);
        view.search.recursive_enabled = true;

        view.set_search_query(String::new());

        assert_eq!(names(&view.entries), vec!["a.txt", "b.png"]);
        assert!(!view.recursive_search_results_active());
    }

    #[test]
    fn recursive_search_query_requires_at_least_two_characters() {
        assert!(!recursive_search_query_is_ready(""));
        assert!(!recursive_search_query_is_ready("a"));
        assert!(recursive_search_query_is_ready("ab"));
        assert!(!recursive_search_query_is_ready("文"));
        assert!(recursive_search_query_is_ready("文件"));
    }

    #[test]
    fn recursive_query_below_minimum_restores_normal_entries() {
        let mut view = test_view_with_entries(&["a.txt", "b.png"]);
        view.search.recursive_enabled = true;
        view.search.recursive_results_active = true;
        view.search.recursive_status = RecursiveSearchStatus::Searching;
        view.search.content = "a".to_owned();
        view.entries = vec![FileEntry::test("stale-recursive.txt", false, Some(1), None)];

        view.reset_recursive_search_below_minimum(&[]);

        assert_eq!(view.search_query(), "a");
        assert_eq!(names(&view.entries), vec!["a.txt", "b.png"]);
        assert_eq!(view.search.recursive_status, RecursiveSearchStatus::Idle);
        assert!(!view.recursive_search_results_active());
    }

    #[test]
    fn stale_recursive_search_output_is_ignored() {
        let mut view = test_view_with_entries(&["current.txt"]);
        view.search.recursive_enabled = true;
        view.search.recursive_generation = 2;
        view.search.recursive_status = RecursiveSearchStatus::Searching;
        view.search.content = "stale".to_owned();

        view.apply_recursive_search_output(RecursiveSearchOutput {
            generation: 1,
            root: view.path.clone(),
            query: "stale".to_owned(),
            show_hidden_files: view.show_hidden_files,
            scanned_paths: Arc::new(Vec::new()),
            entries: vec![FileEntry::test("stale.txt", false, Some(1), None)],
        });

        assert_eq!(names(&view.entries), vec!["current.txt"]);
        assert_eq!(
            view.search.recursive_status,
            RecursiveSearchStatus::Searching
        );
        assert!(view.search.recursive_cache.is_none());
    }

    #[test]
    fn stale_recursive_search_progress_is_ignored() {
        let mut view = test_view_with_entries(&["current.txt"]);
        view.search.recursive_generation = 2;
        view.search.recursive_status = RecursiveSearchStatus::Searching;

        assert!(
            !view
                .update_recursive_search_progress(1, RecursiveSearchProgressSnapshot::Scanning(42))
        );
        assert_eq!(
            view.search.recursive_progress,
            RecursiveSearchProgressSnapshot::Searching(None)
        );
    }

    #[test]
    fn recursive_search_progress_updates_and_clears_on_cancel() {
        let mut view = test_view_with_entries(&["current.txt"]);
        view.search.recursive_generation = 2;
        view.search.recursive_status = RecursiveSearchStatus::Searching;

        assert!(
            view.update_recursive_search_progress(2, RecursiveSearchProgressSnapshot::Scanning(42))
        );
        assert_eq!(
            view.search.recursive_progress,
            RecursiveSearchProgressSnapshot::Scanning(42)
        );

        view.cancel_recursive_search();
        assert_eq!(
            view.search.recursive_progress,
            RecursiveSearchProgressSnapshot::Searching(None)
        );
    }
}
