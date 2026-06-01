use std::{
    cmp::Ordering,
    ffi::OsStr,
    fs,
    ops::Range,
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, Local};
use gpui::{
    AnyElement, App, ClickEvent, Context, Div, FocusHandle, Focusable, FontFallbacks, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection, Pixels, Render,
    ScrollStrategy, ScrollWheelEvent, SharedString, Styled, TextRun, UniformListScrollHandle,
    Window, actions, canvas, div, font, point, prelude::*, px, rgb, uniform_list,
};

actions!(
    explorer,
    [
        MoveUp,
        MoveDown,
        ExtendUp,
        ExtendDown,
        MoveHome,
        MoveEnd,
        ExtendHome,
        ExtendEnd,
        GoBack,
        GoForward,
        GoUp,
        OpenSelected,
        EnterSelected,
        Refresh,
        SelectAll
    ]
);

const COLUMN_NAME_MIN_WIDTH: f32 = 250.0;
const COLUMN_DATE_WIDTH: f32 = 180.0;
const COLUMN_TYPE_WIDTH: f32 = 202.0;
const COLUMN_SIZE_WIDTH: f32 = 124.0;
const NAVBAR_HEIGHT: f32 = 52.0;
const NAV_ICON_SIZE_PHYSICAL: f32 = 18.0;
const NAV_ICON_ENABLED_COLOR: u32 = 0x1f1f1f;
const NAV_ICON_DISABLED_COLOR: u32 = 0x9a9a9a;
const NAV_BUTTON_HOVER_BG: u32 = 0xefefef;
const NAV_BUTTON_ACTIVE_OPACITY: f32 = 0.7;
const NAVBAR_HORIZONTAL_PADDING: f32 = 10.0;
const NAVBAR_ITEM_GAP: f32 = 10.0;
const NAV_BUTTON_SIZE: f32 = 34.0;
const DIRECTORY_BAR_HEIGHT: f32 = 34.0;
const DIRECTORY_BAR_RADIUS: f32 = 6.0;
const DIRECTORY_BAR_HORIZONTAL_PADDING: f32 = 16.0;
const DIRECTORY_BAR_SEGMENT_HORIZONTAL_PADDING: f32 = 4.0;
const DIRECTORY_BAR_TEXT_SIZE: f32 = 15.0;
const DIRECTORY_BAR_SEPARATOR: &str = " / ";
const DIRECTORY_BAR_ELLIPSIS: &str = "...";
const HEADER_HEIGHT: f32 = 32.0;
const ROW_HEIGHT: f32 = 28.0;
const FILE_ICON_SLOT_WIDTH_PHYSICAL: f32 = 22.0;
const FILE_ICON_SLOT_HEIGHT_PHYSICAL: f32 = 20.0;
const FILE_ICON_PAGE_WIDTH_PHYSICAL: f32 = 16.0;
const FILE_ICON_PAGE_HEIGHT_PHYSICAL: f32 = 20.0;
const FILE_ICON_PAGE_LEFT_PHYSICAL: f32 =
    (FILE_ICON_SLOT_WIDTH_PHYSICAL - FILE_ICON_PAGE_WIDTH_PHYSICAL) / 2.0;
const FILE_ICON_FOLD_SIZE_PHYSICAL: f32 = 5.0;
const EMPTY_FOLDER_TEXT_SIZE: f32 = 12.0;
const EMPTY_FOLDER_TOP_MARGIN: f32 = 20.0;
const EMPTY_FOLDER_MESSAGE: &str = "This folder is empty.";
const OPEN_ERROR_VERTICAL_PADDING: f32 = 8.0;
const OPEN_ERROR_HORIZONTAL_PADDING: f32 = 16.0;
const SCROLLBAR_GUTTER_WIDTH: f32 = 18.0;
const SCROLLBAR_THUMB_WIDTH: f32 = 4.0;
const SCROLLBAR_THUMB_HOVER_WIDTH: f32 = 6.0;
const SCROLLBAR_ARROW_HEIGHT: f32 = 16.0;
const SCROLLBAR_MIN_THUMB_HEIGHT: f32 = 32.0;
const SCROLLBAR_TRACK_BG: u32 = 0xf8f8f8;
const SCROLLBAR_THUMB_BG: u32 = 0x8a8a8a;
const SCROLLBAR_THUMB_HOVER_BG: u32 = 0x707070;
const SCROLLBAR_THUMB_ACTIVE_BG: u32 = 0x5f5f5f;
const SCROLLBAR_ARROW_COLOR: u32 = 0x606060;
const SCROLLBAR_ARROW_HOVER_BG: u32 = 0xe8e8e8;
const KB_BYTES: u64 = 1024;
const MB_BYTES: u64 = KB_BYTES * 1024;
const GB_BYTES: u64 = MB_BYTES * 1024;
const TB_BYTES: u64 = GB_BYTES * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEntry {
    path: PathBuf,
    name: String,
    is_dir: bool,
    modified: Option<SystemTime>,
    size: Option<u64>,
}

impl FileEntry {
    fn from_path(path: PathBuf) -> Option<Self> {
        let metadata = fs::metadata(&path).ok()?;
        let name = path.file_name()?.to_string_lossy().into_owned();
        let is_dir = metadata.is_dir();

        Some(Self {
            path,
            name,
            is_dir,
            modified: metadata.modified().ok(),
            size: (!is_dir).then_some(metadata.len()),
        })
    }

    #[cfg(test)]
    fn test(name: &str, is_dir: bool, size: Option<u64>, modified: Option<SystemTime>) -> Self {
        Self {
            path: PathBuf::from(name),
            name: name.to_owned(),
            is_dir,
            modified,
            size,
        }
    }

    fn type_label(&self) -> String {
        if self.is_dir {
            return "File folder".to_owned();
        }

        let Some(extension) = self.path.extension().and_then(OsStr::to_str) else {
            return "File".to_owned();
        };

        format!("{} File", extension.to_uppercase())
    }
}

pub struct ExplorerView {
    path: PathBuf,
    entries: Vec<FileEntry>,
    selection: SelectionState,
    read_error: Option<String>,
    open_error: Option<String>,
    back_stack: Vec<PathBuf>,
    forward_stack: Vec<PathBuf>,
    scroll_handle: UniformListScrollHandle,
    focus_handle: Option<FocusHandle>,
    scrollbar_hovered: bool,
    scrollbar_drag: Option<ScrollbarDrag>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SelectionState {
    anchor_index: Option<usize>,
    focused_index: Option<usize>,
    selected_range: Option<SelectionRange>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SelectionRange {
    start: usize,
    end: usize,
}

impl SelectionRange {
    fn new(a: usize, b: usize) -> Self {
        Self {
            start: a.min(b),
            end: a.max(b),
        }
    }

    fn contains(self, ix: usize) -> bool {
        ix >= self.start && ix <= self.end
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HistoryMode {
    Record,
    Preserve,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SelectionDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SelectionEdge {
    Home,
    End,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum EntryAction {
    OpenFile(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ScrollbarDrag {
    pointer_offset_from_thumb_top: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ScrollbarMetrics {
    viewport_height: f32,
    content_height: f32,
    scroll_top: f32,
    scroll_max: f32,
    track_top: f32,
    track_height: f32,
    thumb_top: f32,
    thumb_height: f32,
}

impl ScrollbarMetrics {
    fn new(viewport_height: f32, content_height: f32, scroll_top: f32) -> Option<Self> {
        if viewport_height <= 0.0 || content_height <= viewport_height {
            return None;
        }

        let track_top = SCROLLBAR_ARROW_HEIGHT;
        let track_height = viewport_height - (SCROLLBAR_ARROW_HEIGHT * 2.0);
        if track_height <= 0.0 {
            return None;
        }

        let scroll_max = content_height - viewport_height;
        let scroll_top = scroll_top.clamp(0.0, scroll_max);
        let thumb_height = (track_height * viewport_height / content_height)
            .clamp(SCROLLBAR_MIN_THUMB_HEIGHT.min(track_height), track_height);
        let thumb_travel = track_height - thumb_height;
        let thumb_top = if thumb_travel <= 0.0 {
            track_top
        } else {
            track_top + (scroll_top / scroll_max) * thumb_travel
        };

        Some(Self {
            viewport_height,
            content_height,
            scroll_top,
            scroll_max,
            track_top,
            track_height,
            thumb_top,
            thumb_height,
        })
    }

    fn thumb_bottom(self) -> f32 {
        self.thumb_top + self.thumb_height
    }

    fn clamp_scroll_top(self, scroll_top: f32) -> f32 {
        scroll_top.clamp(0.0, self.scroll_max)
    }

    fn scroll_by(self, delta: f32) -> f32 {
        self.clamp_scroll_top(self.scroll_top + delta)
    }

    fn scroll_top_for_thumb_top(self, thumb_top: f32) -> f32 {
        let thumb_travel = self.track_height - self.thumb_height;
        if thumb_travel <= 0.0 {
            return 0.0;
        }

        let thumb_top = thumb_top.clamp(self.track_top, self.track_top + thumb_travel);
        ((thumb_top - self.track_top) / thumb_travel) * self.scroll_max
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NavIcon {
    Back,
    Forward,
    Up,
    Refresh,
}

impl NavIcon {
    fn glyph(self) -> &'static str {
        match self {
            Self::Back => "\u{E72B}",
            Self::Forward => "\u{E72A}",
            Self::Up => "\u{E74A}",
            Self::Refresh => "\u{E72C}",
        }
    }
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

    fn selected_paths(&self) -> Vec<PathBuf> {
        let Some(range) = self.selection.selected_range else {
            return Vec::new();
        };

        (range.start..=range.end)
            .filter_map(|ix| self.entries.get(ix).map(|entry| entry.path.clone()))
            .collect()
    }

    fn restore_selection_from_paths(&mut self, paths: &[PathBuf]) {
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

    fn entry_index_by_path(&self, path: &Path) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.path.as_path() == path)
    }

    fn entry_is_selected(&self, ix: usize) -> bool {
        self.selection
            .selected_range
            .is_some_and(|range| range.contains(ix))
    }

    fn focused_entry(&self) -> Option<&FileEntry> {
        self.selection
            .focused_index
            .and_then(|ix| self.entries.get(ix))
    }

    fn select_single_index(&mut self, ix: usize) {
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

    fn select_single_path(&mut self, path: &Path) {
        if let Some(ix) = self.entry_index_by_path(path) {
            self.select_single_index(ix);
        } else {
            self.clear_selection();
        }
    }

    fn extend_selection_to_index(&mut self, ix: usize) {
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

    fn select_all_entries(&mut self) {
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

    fn scroll_index_into_view(&self, ix: usize) {
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

    fn navigate_to_directory(&mut self, path: PathBuf, history_mode: HistoryMode) {
        if path == self.path {
            self.reload();
            return;
        }

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
    }

    fn navigate_back(&mut self) {
        if let Some(path) = self.back_stack.pop() {
            self.forward_stack.push(self.path.clone());
            self.navigate_to_directory(path, HistoryMode::Preserve);
        }
    }

    fn navigate_forward(&mut self) {
        if let Some(path) = self.forward_stack.pop() {
            self.back_stack.push(self.path.clone());
            self.navigate_to_directory(path, HistoryMode::Preserve);
        }
    }

    fn navigate_up(&mut self) {
        if let Some(parent) = self.path.parent().map(Path::to_path_buf) {
            self.navigate_to_directory(parent, HistoryMode::Record);
        }
    }

    fn can_go_back(&self) -> bool {
        !self.back_stack.is_empty()
    }

    fn can_go_forward(&self) -> bool {
        !self.forward_stack.is_empty()
    }

    fn can_go_up(&self) -> bool {
        self.path.parent().is_some()
    }

    fn handle_entry_click(&mut self, entry: &FileEntry, click_count: usize) -> Option<EntryAction> {
        self.select_single_path(&entry.path);
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

    fn clear_selection(&mut self) {
        self.selection = SelectionState::default();
    }

    fn move_selection(&mut self, direction: SelectionDirection) {
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

    fn extend_selection(&mut self, direction: SelectionDirection) {
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

    fn select_edge(&mut self, edge: SelectionEdge) {
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

    fn extend_selection_to_edge(&mut self, edge: SelectionEdge) {
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

    fn activate_focused_entry(&mut self, open_files: bool) -> Option<EntryAction> {
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

    fn handle_move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(SelectionDirection::Up);
        cx.notify();
    }

    fn handle_move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(SelectionDirection::Down);
        cx.notify();
    }

    fn handle_extend_up(&mut self, _: &ExtendUp, _: &mut Window, cx: &mut Context<Self>) {
        self.extend_selection(SelectionDirection::Up);
        cx.notify();
    }

    fn handle_extend_down(&mut self, _: &ExtendDown, _: &mut Window, cx: &mut Context<Self>) {
        self.extend_selection(SelectionDirection::Down);
        cx.notify();
    }

    fn handle_move_home(&mut self, _: &MoveHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_edge(SelectionEdge::Home);
        cx.notify();
    }

    fn handle_move_end(&mut self, _: &MoveEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_edge(SelectionEdge::End);
        cx.notify();
    }

    fn handle_extend_home(&mut self, _: &ExtendHome, _: &mut Window, cx: &mut Context<Self>) {
        self.extend_selection_to_edge(SelectionEdge::Home);
        cx.notify();
    }

    fn handle_extend_end(&mut self, _: &ExtendEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.extend_selection_to_edge(SelectionEdge::End);
        cx.notify();
    }

    fn handle_go_back(&mut self, _: &GoBack, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate_back();
        cx.notify();
    }

    fn handle_go_forward(&mut self, _: &GoForward, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate_forward();
        cx.notify();
    }

    fn handle_go_up(&mut self, _: &GoUp, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate_up();
        cx.notify();
    }

    fn handle_open_selected(&mut self, _: &OpenSelected, _: &mut Window, cx: &mut Context<Self>) {
        let _ = self.activate_focused_entry(false);
        cx.notify();
    }

    fn handle_enter_selected(&mut self, _: &EnterSelected, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(EntryAction::OpenFile(path)) = self.activate_focused_entry(true) {
            self.open_file_with_default_app(&path);
        }
        cx.notify();
    }

    fn handle_refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        self.reload();
        cx.notify();
    }

    fn handle_select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all_entries();
        cx.notify();
    }

    fn open_file_with_default_app(&mut self, path: &Path) {
        let result = open_path_with_default_app(path);
        self.handle_open_file_result(path, result);
    }

    fn handle_open_file_result(&mut self, path: &Path, result: std::io::Result<()>) {
        match result {
            Ok(()) => self.open_error = None,
            Err(error) => {
                self.open_error = Some(format_open_error(path, &error));
            }
        }
    }

    fn should_show_empty_folder_message(&self) -> bool {
        self.entries.is_empty() && self.read_error.is_none()
    }

    fn scroll_to_top(&self) {
        self.set_scroll_offset(0.0);
    }

    fn set_scroll_offset(&self, scroll_top: f32) {
        let scroll_handle = self.scroll_handle.0.borrow().base_handle.clone();
        let offset = scroll_handle.offset();
        scroll_handle.set_offset(point(offset.x, px(-scroll_top.max(0.0))));
    }

    fn scrollbar_metrics(&self) -> Option<ScrollbarMetrics> {
        let scroll_state = self.scroll_handle.0.borrow();
        let item_size = scroll_state.last_item_size?;
        let viewport_height = f32::from(item_size.item.height);
        let content_height = f32::from(item_size.contents.height);
        let scroll_top = -f32::from(scroll_state.base_handle.offset().y);

        ScrollbarMetrics::new(viewport_height, content_height, scroll_top)
    }

    fn scrollbar_is_hovered_or_dragged(&self) -> bool {
        self.scrollbar_hovered || self.scrollbar_drag.is_some()
    }

    fn handle_scrollbar_mouse_down(&mut self, local_y: f32, metrics: ScrollbarMetrics) {
        if local_y < SCROLLBAR_ARROW_HEIGHT {
            self.set_scroll_offset(metrics.scroll_by(-ROW_HEIGHT));
        } else if local_y > metrics.viewport_height - SCROLLBAR_ARROW_HEIGHT {
            self.set_scroll_offset(metrics.scroll_by(ROW_HEIGHT));
        } else if local_y >= metrics.thumb_top && local_y <= metrics.thumb_bottom() {
            self.scrollbar_drag = Some(ScrollbarDrag {
                pointer_offset_from_thumb_top: local_y - metrics.thumb_top,
            });
        } else if local_y < metrics.thumb_top {
            self.set_scroll_offset(metrics.scroll_by(-metrics.viewport_height));
        } else {
            self.set_scroll_offset(metrics.scroll_by(metrics.viewport_height));
        }
    }

    fn handle_scrollbar_drag(&mut self, local_y: f32, metrics: ScrollbarMetrics) {
        let Some(drag) = self.scrollbar_drag else {
            return;
        };

        let thumb_top = local_y - drag.pointer_offset_from_thumb_top;
        self.set_scroll_offset(metrics.scroll_top_for_thumb_top(thumb_top));
    }

    fn content_branch(&self) -> ExplorerContentBranch {
        if self.read_error.is_some() {
            ExplorerContentBranch::Error
        } else if self.should_show_empty_folder_message() {
            ExplorerContentBranch::Empty
        } else {
            ExplorerContentBranch::List
        }
    }

    fn render_navbar(&self, window: &Window, scale_factor: f32, cx: &mut Context<Self>) -> Div {
        let breadcrumb = visible_breadcrumb_for_path(
            &self.path,
            directory_bar_available_width(f32::from(window.bounds().size.width)),
            window,
        );

        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(NAVBAR_HEIGHT))
            .w_full()
            .bg(rgb(0xf8f8f8))
            .px(px(NAVBAR_HORIZONTAL_PADDING))
            .gap(px(NAVBAR_ITEM_GAP))
            .child(nav_button(
                "back",
                NavIcon::Back,
                self.can_go_back(),
                scale_factor,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.navigate_back();
                    cx.notify();
                }),
            ))
            .child(nav_button(
                "forward",
                NavIcon::Forward,
                self.can_go_forward(),
                scale_factor,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.navigate_forward();
                    cx.notify();
                }),
            ))
            .child(nav_button(
                "up",
                NavIcon::Up,
                self.can_go_up(),
                scale_factor,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.navigate_up();
                    cx.notify();
                }),
            ))
            .child(nav_button(
                "refresh",
                NavIcon::Refresh,
                true,
                scale_factor,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.reload();
                    cx.notify();
                }),
            ))
            .child(directory_bar(breadcrumb, cx))
    }

    fn render_header(&self) -> Div {
        div()
            .flex()
            .flex_row()
            .h(px(HEADER_HEIGHT))
            .w_full()
            .bg(rgb(0xffffff))
            .border_b_1()
            .border_color(rgb(0xf2f2f2))
            .text_size(px(12.0))
            .text_color(rgb(0x1f4e79))
            .child(name_header_cell())
            .child(header_cell("Date modified", COLUMN_DATE_WIDTH, false))
            .child(header_cell("Type", COLUMN_TYPE_WIDTH, false))
            .child(header_cell("Size", COLUMN_SIZE_WIDTH, false))
            .child(scrollbar_header_spacer())
    }

    fn render_row(&self, ix: usize, scale_factor: f32, cx: &mut Context<Self>) -> AnyElement {
        let entry = self.entries[ix].clone();
        let is_selected = self.entry_is_selected(ix);
        let clicked_entry = entry.clone();

        div()
            .id(("explorer-entry", ix))
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .h(px(ROW_HEIGHT))
            .w_full()
            .bg(if is_selected {
                rgb(0xcce8ff)
            } else {
                rgb(0xffffff)
            })
            .when(!is_selected, |this| {
                this.hover(|style| style.bg(rgb(0xe5f3ff)))
            })
            .border_1()
            .border_color(rgb(0xffffff))
            .cursor_default()
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if let Some(EntryAction::OpenFile(path)) =
                    this.handle_entry_click(&clicked_entry, event.click_count())
                {
                    this.open_file_with_default_app(&path);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .child(name_cell(&entry, scale_factor))
            .child(text_cell(
                format_modified(entry.modified),
                COLUMN_DATE_WIDTH,
                false,
            ))
            .child(text_cell(entry.type_label(), COLUMN_TYPE_WIDTH, false))
            .child(text_cell(format_size(entry.size), COLUMN_SIZE_WIDTH, true))
            .into_any_element()
    }

    fn render_list(&mut self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_row()
            .size_full()
            .overflow_hidden()
            .child(
                div()
                    .id("explorer-list-background")
                    .flex_1()
                    .h_full()
                    .overflow_hidden()
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.clear_selection();
                        cx.notify();
                    }))
                    .child(
                        uniform_list(
                            "explorer-entries",
                            self.entries.len(),
                            cx.processor(|this, range: Range<usize>, window, cx| {
                                let scale_factor = window.scale_factor();
                                let mut rows = Vec::with_capacity(range.end - range.start);
                                for ix in range {
                                    rows.push(this.render_row(ix, scale_factor, cx));
                                }
                                rows
                            }),
                        )
                        .size_full()
                        .track_scroll(self.scroll_handle.clone())
                        .on_scroll_wheel(cx.listener(
                            |_: &mut Self, _: &ScrollWheelEvent, _, cx| {
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(self.render_scrollbar(cx))
    }

    fn render_scrollbar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(metrics) = self.scrollbar_metrics() else {
            return div()
                .id("explorer-scrollbar")
                .w(px(SCROLLBAR_GUTTER_WIDTH))
                .h_full()
                .flex_shrink_0()
                .into_any_element();
        };

        let hovered_or_dragged = self.scrollbar_is_hovered_or_dragged();
        let thumb_width = if hovered_or_dragged {
            SCROLLBAR_THUMB_HOVER_WIDTH
        } else {
            SCROLLBAR_THUMB_WIDTH
        };
        let thumb_right = (SCROLLBAR_GUTTER_WIDTH - thumb_width) / 2.0;
        let thumb_color = if self.scrollbar_drag.is_some() {
            SCROLLBAR_THUMB_ACTIVE_BG
        } else if hovered_or_dragged {
            SCROLLBAR_THUMB_HOVER_BG
        } else {
            SCROLLBAR_THUMB_BG
        };
        let bottom_arrow_top = (metrics.viewport_height - SCROLLBAR_ARROW_HEIGHT).max(0.0);

        div()
            .id("explorer-scrollbar")
            .relative()
            .w(px(SCROLLBAR_GUTTER_WIDTH))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(SCROLLBAR_TRACK_BG))
            .cursor_default()
            .block_mouse_except_scroll()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.scrollbar_hovered = *hovered;
                cx.notify();
            }))
            .when(hovered_or_dragged, |this| {
                this.child(scrollbar_arrow_button(0.0, ScrollbarArrow::Up))
                    .child(scrollbar_arrow_button(
                        bottom_arrow_top,
                        ScrollbarArrow::Down,
                    ))
            })
            .child(
                div()
                    .absolute()
                    .top(px(metrics.thumb_top))
                    .right(px(thumb_right))
                    .w(px(thumb_width))
                    .h(px(metrics.thumb_height))
                    .rounded(px(thumb_width / 2.0))
                    .bg(rgb(thumb_color)),
            )
            .child(self.render_scrollbar_hit_layer(cx))
            .into_any_element()
    }

    fn render_scrollbar_hit_layer(&self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();

        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                window.on_mouse_event({
                    let entity = entity.clone();
                    move |event: &MouseDownEvent, _, _, cx| {
                        if event.button != MouseButton::Left || !bounds.contains(&event.position) {
                            return;
                        }

                        let local_y = f32::from(event.position.y - bounds.origin.y);
                        let _ = entity.update(cx, |this, cx| {
                            if let Some(metrics) = this.scrollbar_metrics() {
                                this.handle_scrollbar_mouse_down(local_y, metrics);
                                cx.notify();
                            }
                        });
                    }
                });

                window.on_mouse_event({
                    let entity = entity.clone();
                    move |event: &MouseMoveEvent, _, _, cx| {
                        if !event.dragging() {
                            return;
                        }

                        let local_y = f32::from(event.position.y - bounds.origin.y);
                        let _ = entity.update(cx, |this, cx| {
                            if this.scrollbar_drag.is_none() {
                                return;
                            }

                            if let Some(metrics) = this.scrollbar_metrics() {
                                this.handle_scrollbar_drag(local_y, metrics);
                                cx.notify();
                            }
                        });
                    }
                });

                window.on_mouse_event(move |event: &MouseUpEvent, _, _, cx| {
                    if event.button != MouseButton::Left {
                        return;
                    }

                    let _ = entity.update(cx, |this, cx| {
                        if this.scrollbar_drag.take().is_some() {
                            cx.notify();
                        }
                    });
                });
            },
        )
        .size_full()
        .into_any_element()
    }
}

impl Render for ExplorerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale_factor = window.scale_factor();
        let focus_handle = self.focus_handle(cx);

        div()
            .key_context("Explorer")
            .track_focus(&focus_handle)
            .on_action(cx.listener(Self::handle_move_up))
            .on_action(cx.listener(Self::handle_move_down))
            .on_action(cx.listener(Self::handle_extend_up))
            .on_action(cx.listener(Self::handle_extend_down))
            .on_action(cx.listener(Self::handle_move_home))
            .on_action(cx.listener(Self::handle_move_end))
            .on_action(cx.listener(Self::handle_extend_home))
            .on_action(cx.listener(Self::handle_extend_end))
            .on_action(cx.listener(Self::handle_go_back))
            .on_action(cx.listener(Self::handle_go_forward))
            .on_action(cx.listener(Self::handle_go_up))
            .on_action(cx.listener(Self::handle_open_selected))
            .on_action(cx.listener(Self::handle_enter_selected))
            .on_action(cx.listener(Self::handle_refresh))
            .on_action(cx.listener(Self::handle_select_all))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.navigate_back();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.navigate_forward();
                    cx.notify();
                }),
            )
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x000000))
            .overflow_hidden()
            .child(self.render_navbar(window, scale_factor, cx))
            .child(self.render_header())
            .when_some(self.open_error.clone(), |this, error| {
                this.child(render_open_error(&error))
            })
            .child(
                match self.content_branch() {
                    ExplorerContentBranch::Error => div().child(
                        div()
                            .p_4()
                            .text_size(px(14.0))
                            .text_color(rgb(0x6f1d1d))
                            .child(self.read_error.clone().unwrap_or_default()),
                    ),
                    ExplorerContentBranch::Empty => div().child(render_empty_folder_message()),
                    ExplorerContentBranch::List => div().child(self.render_list(cx)),
                }
                .id("explorer-scroll")
                .flex_1()
                .w_full()
                .overflow_hidden(),
            )
    }
}

impl Focusable for ExplorerView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle
            .clone()
            .expect("ExplorerView must be constructed with a FocusHandle before rendering")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScrollbarArrow {
    Up,
    Down,
}

impl ScrollbarArrow {
    fn glyph(self) -> &'static str {
        match self {
            Self::Up => "\u{E70E}",
            Self::Down => "\u{E70D}",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExplorerContentBranch {
    Error,
    Empty,
    List,
}

pub fn default_start_path() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct VisibleBreadcrumb {
    show_ellipsis: bool,
    segments: Vec<BreadcrumbSegment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BreadcrumbSegment {
    label: String,
    target: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BreadcrumbVisibility {
    start_index: usize,
    show_ellipsis: bool,
}

fn path_breadcrumb_segments(path: &Path) -> Vec<BreadcrumbSegment> {
    let mut segments = Vec::new();
    let mut saw_prefix = false;
    let mut target = PathBuf::new();
    let components = path.components().collect::<Vec<_>>();

    for (index, component) in components.iter().copied().enumerate() {
        match component {
            Component::Prefix(prefix) => {
                saw_prefix = true;
                target.push(prefix.as_os_str());
                let prefix = prefix.as_os_str().to_string_lossy().into_owned();
                if !prefix.is_empty() {
                    let mut segment_target = target.clone();
                    if matches!(components.get(index + 1), Some(Component::RootDir)) {
                        segment_target.push(Component::RootDir.as_os_str());
                    }
                    segments.push(BreadcrumbSegment {
                        label: prefix,
                        target: segment_target,
                    });
                }
            }
            Component::RootDir => {
                target.push(component.as_os_str());
                if !saw_prefix {
                    segments.push(BreadcrumbSegment {
                        label: "/".to_owned(),
                        target: target.clone(),
                    });
                }
            }
            Component::CurDir => {
                target.push(component.as_os_str());
                segments.push(BreadcrumbSegment {
                    label: ".".to_owned(),
                    target: target.clone(),
                });
            }
            Component::ParentDir => {
                target.push(component.as_os_str());
                segments.push(BreadcrumbSegment {
                    label: "..".to_owned(),
                    target: target.clone(),
                });
            }
            Component::Normal(component) => {
                target.push(component);
                segments.push(BreadcrumbSegment {
                    label: component.to_string_lossy().into_owned(),
                    target: target.clone(),
                });
            }
        }
    }

    if segments.is_empty() {
        let fallback = path.display().to_string();
        let target = if path.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            path.to_path_buf()
        };
        segments.push(BreadcrumbSegment {
            label: if fallback.is_empty() {
                ".".to_owned()
            } else {
                fallback
            },
            target,
        });
    }

    segments
}

#[cfg(test)]
fn breadcrumb_labels(segments: &[BreadcrumbSegment]) -> Vec<String> {
    segments
        .iter()
        .map(|segment| segment.label.clone())
        .collect()
}

fn directory_bar_available_width(window_width: f32) -> f32 {
    let navbar_content_width =
        window_width - (NAVBAR_HORIZONTAL_PADDING * 2.0) - (NAV_BUTTON_SIZE * 4.0);
    let directory_bar_width = navbar_content_width - (NAVBAR_ITEM_GAP * 4.0);
    (directory_bar_width - (DIRECTORY_BAR_HORIZONTAL_PADDING * 2.0)).max(0.0)
}

fn visible_breadcrumb_for_path(
    path: &Path,
    available_width: f32,
    window: &Window,
) -> VisibleBreadcrumb {
    let segments = path_breadcrumb_segments(path);
    let segment_widths = segments
        .iter()
        .map(|segment| {
            measure_directory_bar_text(&segment.label, window)
                + DIRECTORY_BAR_SEGMENT_HORIZONTAL_PADDING * 2.0
        })
        .collect::<Vec<_>>();
    let separator_width = measure_directory_bar_text(DIRECTORY_BAR_SEPARATOR, window);
    let ellipsis_width = measure_directory_bar_text(DIRECTORY_BAR_ELLIPSIS, window);
    let visibility = choose_visible_breadcrumb(
        &segment_widths,
        separator_width,
        ellipsis_width,
        available_width,
    );

    VisibleBreadcrumb {
        show_ellipsis: visibility.show_ellipsis,
        segments: segments[visibility.start_index..].to_vec(),
    }
}

fn measure_directory_bar_text(text: &str, window: &Window) -> f32 {
    if text.is_empty() {
        return 0.0;
    }

    let run = TextRun {
        len: text.len(),
        font: font(".SystemUIFont"),
        color: rgb(0x1f1f1f).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };

    f32::from(
        window
            .text_system()
            .layout_line(text, px(DIRECTORY_BAR_TEXT_SIZE), &[run], None)
            .width,
    )
}

fn choose_visible_breadcrumb(
    segment_widths: &[f32],
    separator_width: f32,
    ellipsis_width: f32,
    available_width: f32,
) -> BreadcrumbVisibility {
    if segment_widths.is_empty() {
        return BreadcrumbVisibility {
            start_index: 0,
            show_ellipsis: false,
        };
    }

    if breadcrumb_width(segment_widths, separator_width) <= available_width {
        return BreadcrumbVisibility {
            start_index: 0,
            show_ellipsis: false,
        };
    }

    for start_index in 1..segment_widths.len() {
        let width = ellipsis_width
            + separator_width
            + breadcrumb_width(&segment_widths[start_index..], separator_width);
        if width <= available_width {
            return BreadcrumbVisibility {
                start_index,
                show_ellipsis: true,
            };
        }
    }

    BreadcrumbVisibility {
        start_index: segment_widths.len() - 1,
        show_ellipsis: segment_widths.len() > 1,
    }
}

fn breadcrumb_width(segment_widths: &[f32], separator_width: f32) -> f32 {
    if segment_widths.is_empty() {
        return 0.0;
    }

    segment_widths.iter().sum::<f32>() + separator_width * (segment_widths.len() - 1) as f32
}

fn load_entries(path: &Path) -> std::io::Result<Vec<FileEntry>> {
    let mut entries = fs::read_dir(path)?
        .filter_map(Result::ok)
        .filter_map(|entry| FileEntry::from_path(entry.path()))
        .collect::<Vec<_>>();

    sort_entries(&mut entries);
    Ok(entries)
}

fn open_path_with_default_app(path: &Path) -> std::io::Result<()> {
    open::that_detached(path)
}

fn format_open_error(path: &Path, error: &std::io::Error) -> String {
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned());

    format!("Could not open {name}: {error}")
}

fn sort_entries(entries: &mut [FileEntry]) {
    entries.sort_by(compare_entries);
}

fn compare_entries(a: &FileEntry, b: &FileEntry) -> Ordering {
    match (a.is_dir, b.is_dir) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => compare_names(&a.name, &b.name),
    }
}

#[cfg(target_os = "windows")]
fn compare_names(a: &str, b: &str) -> Ordering {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::StrCmpLogicalW;

    let a = OsStr::new(a)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let b = OsStr::new(b)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe { StrCmpLogicalW(a.as_ptr(), b.as_ptr()) };

    result.cmp(&0)
}

#[cfg(not(target_os = "windows"))]
fn compare_names(a: &str, b: &str) -> Ordering {
    natural_key(a).cmp(&natural_key(b)).then_with(|| a.cmp(b))
}

#[cfg(not(target_os = "windows"))]
#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
enum NaturalPart {
    Text(String),
    Number(u64),
}

#[cfg(not(target_os = "windows"))]
fn natural_key(value: &str) -> Vec<NaturalPart> {
    let mut parts = Vec::new();
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.peek().copied() {
        if ch.is_ascii_digit() {
            let mut digits = String::new();
            while let Some(next) = chars.peek().copied() {
                if next.is_ascii_digit() {
                    digits.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            parts.push(NaturalPart::Number(digits.parse().unwrap_or(u64::MAX)));
        } else {
            let mut text = String::new();
            while let Some(next) = chars.peek().copied() {
                if next.is_ascii_digit() {
                    break;
                }
                text.extend(next.to_lowercase());
                chars.next();
            }
            parts.push(NaturalPart::Text(text));
        }
    }

    parts
}

fn format_modified(modified: Option<SystemTime>) -> String {
    let Some(modified) = modified else {
        return String::new();
    };

    let local: DateTime<Local> = modified.into();
    local.format("%d/%m/%Y %H:%M").to_string()
}

fn format_size(size: Option<u64>) -> String {
    let Some(size) = size else {
        return String::new();
    };

    if size < KB_BYTES {
        return format!("{} bytes", format_u64_with_commas(size));
    }

    let (value, precision, unit) = if size < MB_BYTES {
        (size as f64 / KB_BYTES as f64, 1, "KB")
    } else if size < GB_BYTES {
        (size as f64 / MB_BYTES as f64, 2, "MB")
    } else if size < TB_BYTES {
        (size as f64 / GB_BYTES as f64, 2, "GB")
    } else {
        (size as f64 / TB_BYTES as f64, 2, "TB")
    };

    format!("{} {unit}", format_decimal_with_commas(value, precision))
}

fn format_decimal_with_commas(value: f64, precision: usize) -> String {
    let formatted = format!("{value:.precision$}");
    let Some((integer, fraction)) = formatted.split_once('.') else {
        return format_integer_string_with_commas(&formatted);
    };

    format!(
        "{}.{}",
        format_integer_string_with_commas(integer),
        fraction
    )
}

fn format_u64_with_commas(value: u64) -> String {
    format_integer_string_with_commas(&value.to_string())
}

fn format_integer_string_with_commas(value: &str) -> String {
    let mut formatted = String::with_capacity(value.len() + value.len() / 3);

    for (ix, ch) in value.chars().rev().enumerate() {
        if ix > 0 && ix % 3 == 0 {
            formatted.push(',');
        }
        formatted.push(ch);
    }

    formatted.chars().rev().collect()
}

fn render_empty_folder_message() -> Div {
    div()
        .w_full()
        .mt(px(EMPTY_FOLDER_TOP_MARGIN))
        .text_center()
        .text_size(px(EMPTY_FOLDER_TEXT_SIZE))
        .text_color(rgb(0x9a9a9a))
        .child(EMPTY_FOLDER_MESSAGE)
}

fn render_open_error(error: &str) -> Div {
    div()
        .w_full()
        .py(px(OPEN_ERROR_VERTICAL_PADDING))
        .px(px(OPEN_ERROR_HORIZONTAL_PADDING))
        .bg(rgb(0xfff4f4))
        .border_b_1()
        .border_color(rgb(0xf1c7c7))
        .text_size(px(12.0))
        .text_color(rgb(0x6f1d1d))
        .child(SharedString::from(error.to_owned()))
}

fn device_px(value: f32, scale_factor: f32) -> Pixels {
    px(device_px_value(value, scale_factor))
}

fn device_px_value(value: f32, scale_factor: f32) -> f32 {
    if scale_factor <= 0.0 {
        value
    } else {
        value / scale_factor
    }
}

fn nav_button(
    id: &'static str,
    icon: NavIcon,
    enabled: bool,
    scale_factor: f32,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(NAV_BUTTON_SIZE))
        .h(px(NAV_BUTTON_SIZE))
        .rounded(px(4.0))
        .cursor_default()
        .when(enabled, |this| {
            this.hover(|style| style.bg(rgb(NAV_BUTTON_HOVER_BG)))
                .active(|style| style.opacity(NAV_BUTTON_ACTIVE_OPACITY))
                .on_click(on_click)
        })
        .child(
            div()
                .font(nav_icon_font())
                .text_size(device_px(NAV_ICON_SIZE_PHYSICAL, scale_factor))
                .text_color(if enabled {
                    rgb(NAV_ICON_ENABLED_COLOR)
                } else {
                    rgb(NAV_ICON_DISABLED_COLOR)
                })
                .child(icon.glyph()),
        )
        .into_any_element()
}

fn scrollbar_arrow_button(top: f32, arrow: ScrollbarArrow) -> Div {
    div()
        .absolute()
        .top(px(top))
        .right(px(0.0))
        .flex()
        .items_center()
        .justify_center()
        .w(px(SCROLLBAR_GUTTER_WIDTH))
        .h(px(SCROLLBAR_ARROW_HEIGHT))
        .font(nav_icon_font())
        .text_size(px(8.0))
        .text_color(rgb(SCROLLBAR_ARROW_COLOR))
        .hover(|style| style.bg(rgb(SCROLLBAR_ARROW_HOVER_BG)))
        .child(arrow.glyph())
}

fn scrollbar_header_spacer() -> Div {
    div()
        .h_full()
        .w(px(SCROLLBAR_GUTTER_WIDTH))
        .flex_shrink_0()
        .bg(rgb(0xffffff))
}

fn nav_icon_font() -> gpui::Font {
    let mut font = font("Segoe Fluent Icons");
    font.fallbacks = Some(FontFallbacks::from_fonts(vec![
        "Segoe MDL2 Assets".to_owned(),
    ]));
    font
}

fn directory_bar(breadcrumb: VisibleBreadcrumb, cx: &mut Context<ExplorerView>) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .h(px(DIRECTORY_BAR_HEIGHT))
        .flex_1()
        .overflow_hidden()
        .rounded(px(DIRECTORY_BAR_RADIUS))
        .bg(rgb(0xfdfdfd))
        .px(px(DIRECTORY_BAR_HORIZONTAL_PADDING))
        .text_size(px(DIRECTORY_BAR_TEXT_SIZE))
        .text_color(rgb(0x1f1f1f))
        .children(directory_bar_children(breadcrumb, cx))
}

fn directory_bar_children(
    breadcrumb: VisibleBreadcrumb,
    cx: &mut Context<ExplorerView>,
) -> Vec<AnyElement> {
    let mut children = Vec::new();

    if breadcrumb.show_ellipsis {
        children.push(directory_bar_fixed_label(DIRECTORY_BAR_ELLIPSIS).into_any_element());
        if !breadcrumb.segments.is_empty() {
            children.push(directory_bar_separator().into_any_element());
        }
    }

    let segment_count = breadcrumb.segments.len();
    for (index, segment) in breadcrumb.segments.into_iter().enumerate() {
        let is_last = index + 1 == segment_count;
        children.push(directory_bar_label(segment, index, cx));
        if !is_last {
            children.push(directory_bar_separator().into_any_element());
        }
    }

    children
}

fn directory_bar_fixed_label(label: &'static str) -> Div {
    div()
        .flex_shrink_0()
        .whitespace_nowrap()
        .text_size(px(DIRECTORY_BAR_TEXT_SIZE))
        .text_color(rgb(0x1f1f1f))
        .child(label)
}

fn directory_bar_label(
    segment: BreadcrumbSegment,
    index: usize,
    cx: &mut Context<ExplorerView>,
) -> AnyElement {
    let target = segment.target;
    div()
        .id(("breadcrumb-segment", index))
        .min_w(px(0.0))
        .whitespace_nowrap()
        .text_size(px(DIRECTORY_BAR_TEXT_SIZE))
        .text_color(rgb(0x1f1f1f))
        .px(px(DIRECTORY_BAR_SEGMENT_HORIZONTAL_PADDING))
        .rounded(px(6.0))
        .cursor_default()
        .hover(|style| style.bg(rgb(NAV_BUTTON_HOVER_BG)))
        .active(|style| style.opacity(NAV_BUTTON_ACTIVE_OPACITY))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.navigate_to_directory(target.clone(), HistoryMode::Record);
            cx.stop_propagation();
            cx.notify();
        }))
        .child(SharedString::from(segment.label))
        .flex_shrink_0()
        .into_any_element()
}

fn directory_bar_separator() -> Div {
    div()
        .flex_shrink_0()
        .whitespace_nowrap()
        .text_size(px(DIRECTORY_BAR_TEXT_SIZE))
        .text_color(rgb(0x707070))
        .child(DIRECTORY_BAR_SEPARATOR)
}

fn header_cell(label: &'static str, width: f32, first: bool) -> Div {
    div()
        .relative()
        .flex()
        .items_start()
        .h_full()
        .w(px(width))
        .flex_shrink_0()
        .pl(px(if first { 36.0 } else { 8.0 }))
        .pt(px(8.0))
        .border_r_1()
        .border_color(rgb(0xe7e7e7))
        .child(label)
}

fn name_header_cell() -> Div {
    div()
        .relative()
        .flex()
        .items_start()
        .h_full()
        .flex_1()
        .min_w(px(COLUMN_NAME_MIN_WIDTH))
        .overflow_hidden()
        .pl(px(36.0))
        .pt(px(8.0))
        .border_r_1()
        .border_color(rgb(0xe7e7e7))
        .child("Name")
}

fn name_cell(entry: &FileEntry, scale_factor: f32) -> Div {
    div()
        .flex()
        .items_center()
        .h_full()
        .flex_1()
        .min_w(px(COLUMN_NAME_MIN_WIDTH))
        .overflow_hidden()
        .pl(px(16.0))
        .child(if entry.is_dir {
            folder_icon(scale_factor)
        } else {
            file_icon(scale_factor)
        })
        .child(
            div()
                .flex_1()
                .ml(device_px(8.0, scale_factor))
                .truncate()
                .text_size(px(12.0))
                .child(SharedString::from(entry.name.clone())),
        )
}

fn text_cell(text: String, width: f32, right: bool) -> Div {
    let cell = div()
        .flex()
        .items_center()
        .h_full()
        .w(px(width))
        .flex_shrink_0()
        .overflow_hidden()
        .px(px(8.0))
        .text_size(px(12.0))
        .text_color(rgb(0x595959))
        .child(SharedString::from(text));

    if right {
        cell.justify_end()
    } else {
        cell.justify_start()
    }
}

fn folder_icon(scale_factor: f32) -> Div {
    div()
        .relative()
        .w(device_px(22.0, scale_factor))
        .h(device_px(17.0, scale_factor))
        .flex_shrink_0()
        .child(
            div()
                .absolute()
                .left(device_px(1.0, scale_factor))
                .top(device_px(0.0, scale_factor))
                .w(device_px(9.0, scale_factor))
                .h(device_px(5.0, scale_factor))
                .bg(rgb(0xf5c242)),
        )
        .child(
            div()
                .absolute()
                .left(device_px(0.0, scale_factor))
                .top(device_px(4.0, scale_factor))
                .w(device_px(22.0, scale_factor))
                .h(device_px(13.0, scale_factor))
                .bg(rgb(0xffcc4d)),
        )
        .child(
            div()
                .absolute()
                .left(device_px(0.0, scale_factor))
                .top(device_px(14.0, scale_factor))
                .w(device_px(22.0, scale_factor))
                .h(device_px(3.0, scale_factor))
                .bg(rgb(0xf3b839)),
        )
}

fn file_icon(scale_factor: f32) -> Div {
    div()
        .relative()
        .w(device_px(FILE_ICON_SLOT_WIDTH_PHYSICAL, scale_factor))
        .h(device_px(FILE_ICON_SLOT_HEIGHT_PHYSICAL, scale_factor))
        .flex_shrink_0()
        .child(
            div()
                .relative()
                .absolute()
                .left(device_px(FILE_ICON_PAGE_LEFT_PHYSICAL, scale_factor))
                .top(device_px(0.0, scale_factor))
                .w(device_px(FILE_ICON_PAGE_WIDTH_PHYSICAL, scale_factor))
                .h(device_px(FILE_ICON_PAGE_HEIGHT_PHYSICAL, scale_factor))
                .border_1()
                .border_color(rgb(0x9a9a9a))
                .bg(rgb(0xffffff))
                .child(
                    div()
                        .absolute()
                        .right(device_px(0.0, scale_factor))
                        .top(device_px(0.0, scale_factor))
                        .w(device_px(FILE_ICON_FOLD_SIZE_PHYSICAL, scale_factor))
                        .h(device_px(FILE_ICON_FOLD_SIZE_PHYSICAL, scale_factor))
                        .border_l_1()
                        .border_b_1()
                        .border_color(rgb(0xc8c8c8))
                        .bg(rgb(0xf4f4f4)),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    static TEST_DIR_ID: AtomicU64 = AtomicU64::new(0);

    fn assert_approx_eq(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.000_1,
            "expected {actual} to approximately equal {expected}",
        );
    }

    fn selected_names(view: &ExplorerView) -> Vec<String> {
        view.selected_paths()
            .iter()
            .filter_map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect()
    }

    fn test_view_with_entries(names: &[&str]) -> ExplorerView {
        let mut view = ExplorerView::new(PathBuf::from("selection"));
        view.entries = names
            .iter()
            .map(|name| FileEntry::test(name, false, Some(1), None))
            .collect();
        view.read_error = None;
        view.clear_selection();
        view
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let id = TEST_DIR_ID.fetch_add(1, AtomicOrdering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "universal-explorer-test-{}-{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp test directory");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn sorts_directories_before_files() {
        let mut entries = vec![
            FileEntry::test("b.txt", false, Some(1), None),
            FileEntry::test("c", true, None, None),
            FileEntry::test("a.txt", false, Some(1), None),
            FileEntry::test("a", true, None, None),
        ];

        sort_entries(&mut entries);

        let names = entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["a", "c", "a.txt", "b.txt"]);
    }

    #[test]
    fn sorts_names_naturally() {
        let mut entries = vec![
            FileEntry::test("file10.txt", false, Some(1), None),
            FileEntry::test("file2.txt", false, Some(1), None),
            FileEntry::test("file1.txt", false, Some(1), None),
        ];

        sort_entries(&mut entries);

        let names = entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["file1.txt", "file2.txt", "file10.txt"]);
    }

    #[test]
    fn sorting_is_deterministic_for_case_differences() {
        let mut entries = vec![
            FileEntry::test("Readme.md", false, Some(1), None),
            FileEntry::test("README.md", false, Some(1), None),
        ];

        sort_entries(&mut entries);

        let names = entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        #[cfg(target_os = "windows")]
        assert_eq!(names, vec!["Readme.md", "README.md"]);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(names, vec!["README.md", "Readme.md"]);
    }

    #[test]
    fn folders_render_blank_size() {
        assert_eq!(format_size(None), "");
    }

    #[test]
    fn files_below_kilobytes_render_as_bytes() {
        assert_eq!(format_size(Some(0)), "0 bytes");
        assert_eq!(format_size(Some(350)), "350 bytes");
        assert_eq!(format_size(Some(1023)), "1,023 bytes");
    }

    #[test]
    fn kilobytes_render_with_one_decimal_place() {
        assert_eq!(format_size(Some(KB_BYTES)), "1.0 KB");
        assert_eq!(format_size(Some(KB_BYTES + 512)), "1.5 KB");
        assert_eq!(format_size(Some(MB_BYTES - 1)), "1,024.0 KB");
    }

    #[test]
    fn megabytes_gigabytes_and_terabytes_render_with_two_decimal_places() {
        assert_eq!(format_size(Some(MB_BYTES)), "1.00 MB");
        assert_eq!(format_size(Some(MB_BYTES + 512 * KB_BYTES)), "1.50 MB");
        assert_eq!(format_size(Some(GB_BYTES)), "1.00 GB");
        assert_eq!(format_size(Some(GB_BYTES + 512 * MB_BYTES)), "1.50 GB");
        assert_eq!(format_size(Some(TB_BYTES)), "1.00 TB");
        assert_eq!(format_size(Some(TB_BYTES + 512 * GB_BYTES)), "1.50 TB");
    }

    #[test]
    fn large_file_sizes_include_commas_and_stay_capped_at_terabytes() {
        assert_eq!(format_size(Some(1024 * MB_BYTES)), "1.00 GB");
        assert_eq!(format_size(Some(1024 * GB_BYTES)), "1.00 TB");
        assert_eq!(format_size(Some(1024 * TB_BYTES)), "1,024.00 TB");
    }

    #[test]
    fn device_pixel_values_convert_to_logical_pixels() {
        assert_eq!(device_px_value(22.0, 1.0), 22.0);
        assert!((device_px_value(22.0, 1.5) - 14.666_667).abs() < 0.000_01);
        assert!((device_px_value(17.0, 1.5) - 11.333_333).abs() < 0.000_01);
    }

    #[test]
    fn default_file_icon_uses_portrait_page_in_fixed_slot() {
        assert!(FILE_ICON_PAGE_HEIGHT_PHYSICAL > FILE_ICON_PAGE_WIDTH_PHYSICAL);
        assert_eq!(FILE_ICON_SLOT_WIDTH_PHYSICAL, 22.0);
        assert_eq!(
            FILE_ICON_PAGE_HEIGHT_PHYSICAL,
            FILE_ICON_SLOT_HEIGHT_PHYSICAL
        );
        assert_eq!(
            FILE_ICON_PAGE_LEFT_PHYSICAL,
            (FILE_ICON_SLOT_WIDTH_PHYSICAL - FILE_ICON_PAGE_WIDTH_PHYSICAL) / 2.0
        );
    }

    #[test]
    fn device_pixel_conversion_handles_invalid_scale() {
        assert_eq!(device_px_value(22.0, 0.0), 22.0);
        assert_eq!(device_px_value(22.0, -1.0), 22.0);
    }

    #[test]
    fn scrollbar_metrics_hide_without_overflow() {
        assert!(ScrollbarMetrics::new(200.0, 200.0, 0.0).is_none());
        assert!(ScrollbarMetrics::new(200.0, 180.0, 0.0).is_none());
    }

    #[test]
    fn scrollbar_thumb_is_proportional_and_respects_minimum_height() {
        let proportional = ScrollbarMetrics::new(200.0, 400.0, 0.0).unwrap();
        assert_approx_eq(proportional.thumb_height, 84.0);

        let minimum = ScrollbarMetrics::new(100.0, 10_000.0, 0.0).unwrap();
        assert_approx_eq(minimum.thumb_height, SCROLLBAR_MIN_THUMB_HEIGHT);
    }

    #[test]
    fn scrollbar_thumb_top_clamps_to_scroll_bounds() {
        let top = ScrollbarMetrics::new(200.0, 1_000.0, -50.0).unwrap();
        assert_approx_eq(top.scroll_top, 0.0);
        assert_approx_eq(top.thumb_top, SCROLLBAR_ARROW_HEIGHT);

        let bottom = ScrollbarMetrics::new(200.0, 1_000.0, 900.0).unwrap();
        assert_approx_eq(bottom.scroll_top, 800.0);
        assert_approx_eq(
            bottom.thumb_bottom(),
            SCROLLBAR_ARROW_HEIGHT + bottom.track_height,
        );
    }

    #[test]
    fn scrollbar_drag_positions_map_to_scroll_offsets() {
        let metrics = ScrollbarMetrics::new(200.0, 1_000.0, 0.0).unwrap();
        let bottom_thumb_top = metrics.track_top + metrics.track_height - metrics.thumb_height;
        let middle_thumb_top = metrics.track_top + (bottom_thumb_top - metrics.track_top) / 2.0;

        assert_approx_eq(metrics.scroll_top_for_thumb_top(metrics.track_top), 0.0);
        assert_approx_eq(
            metrics.scroll_top_for_thumb_top(middle_thumb_top),
            metrics.scroll_max / 2.0,
        );
        assert_approx_eq(
            metrics.scroll_top_for_thumb_top(bottom_thumb_top),
            metrics.scroll_max,
        );
    }

    #[test]
    fn scrollbar_line_and_page_deltas_clamp_at_bounds() {
        let top = ScrollbarMetrics::new(200.0, 1_000.0, 0.0).unwrap();
        assert_approx_eq(top.scroll_by(-ROW_HEIGHT), 0.0);
        assert_approx_eq(top.scroll_by(200.0), 200.0);

        let bottom = ScrollbarMetrics::new(200.0, 1_000.0, 800.0).unwrap();
        assert_approx_eq(bottom.scroll_by(ROW_HEIGHT), bottom.scroll_max);
        assert_approx_eq(bottom.scroll_by(-200.0), 600.0);
    }

    #[test]
    fn scrollbar_widths_match_reserved_layout_behavior() {
        assert_eq!(SCROLLBAR_THUMB_WIDTH, 6.0);
        assert_eq!(SCROLLBAR_THUMB_HOVER_WIDTH, 8.0);
        assert!(SCROLLBAR_THUMB_HOVER_WIDTH > SCROLLBAR_THUMB_WIDTH);
        assert_eq!(SCROLLBAR_GUTTER_WIDTH, 16.0);
        assert!(SCROLLBAR_GUTTER_WIDTH > SCROLLBAR_THUMB_HOVER_WIDTH);
    }

    #[test]
    fn nav_icons_use_windows_explorer_glyphs() {
        assert_eq!(NavIcon::Back.glyph(), "\u{E72B}");
        assert_eq!(NavIcon::Forward.glyph(), "\u{E72A}");
        assert_eq!(NavIcon::Up.glyph(), "\u{E74A}");
        assert_eq!(NavIcon::Refresh.glyph(), "\u{E72C}");
    }

    #[test]
    fn nav_icon_size_converts_from_physical_pixels() {
        assert_eq!(device_px_value(NAV_ICON_SIZE_PHYSICAL, 1.0), 18.0);
        assert_eq!(device_px_value(NAV_ICON_SIZE_PHYSICAL, 1.5), 12.0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_paths_render_drive_as_first_breadcrumb_segment() {
        let segments = path_breadcrumb_segments(Path::new(r"C:\Users\Ada\Documents"));

        assert_eq!(
            breadcrumb_labels(&segments),
            vec!["C:", "Users", "Ada", "Documents"]
        );
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.target.clone())
                .collect::<Vec<_>>(),
            vec![
                PathBuf::from("C:\\"),
                PathBuf::from(r"C:\Users"),
                PathBuf::from(r"C:\Users\Ada"),
                PathBuf::from(r"C:\Users\Ada\Documents"),
            ]
        );
    }

    #[test]
    fn absolute_paths_render_root_as_breadcrumb_segment() {
        let segments = path_breadcrumb_segments(Path::new("/usr/local/bin"));

        assert_eq!(
            breadcrumb_labels(&segments),
            vec!["/", "usr", "local", "bin"]
        );
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.target.clone())
                .collect::<Vec<_>>(),
            vec![
                PathBuf::from("/"),
                PathBuf::from("/usr"),
                PathBuf::from("/usr/local"),
                PathBuf::from("/usr/local/bin"),
            ]
        );
    }

    #[test]
    fn relative_paths_keep_relative_breadcrumb_components() {
        let segments = path_breadcrumb_segments(Path::new("../project/src"));

        assert_eq!(breadcrumb_labels(&segments), vec!["..", "project", "src"]);
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.target.clone())
                .collect::<Vec<_>>(),
            vec![
                PathBuf::from(".."),
                PathBuf::from("../project"),
                PathBuf::from("../project/src"),
            ]
        );

        let current_dir_segments = path_breadcrumb_segments(Path::new("."));
        assert_eq!(breadcrumb_labels(&current_dir_segments), vec!["."]);
        assert_eq!(current_dir_segments[0].target, PathBuf::from("."));
    }

    #[test]
    fn empty_paths_fall_back_to_current_directory_breadcrumb() {
        let segments = path_breadcrumb_segments(Path::new(""));

        assert_eq!(breadcrumb_labels(&segments), vec!["."]);
        assert_eq!(segments[0].target, PathBuf::from("."));
    }

    #[test]
    fn breadcrumb_visibility_keeps_full_path_when_it_fits() {
        assert_eq!(
            choose_visible_breadcrumb(&[10.0, 10.0, 10.0], 2.0, 3.0, 34.0),
            BreadcrumbVisibility {
                start_index: 0,
                show_ellipsis: false
            }
        );
    }

    #[test]
    fn breadcrumb_visibility_removes_leading_items_until_tail_fits() {
        assert_eq!(
            choose_visible_breadcrumb(&[20.0, 20.0, 20.0, 20.0], 2.0, 3.0, 47.0),
            BreadcrumbVisibility {
                start_index: 2,
                show_ellipsis: true
            }
        );
    }

    #[test]
    fn breadcrumb_visibility_preserves_final_segment_when_nothing_fits() {
        assert_eq!(
            choose_visible_breadcrumb(&[50.0, 50.0, 50.0], 5.0, 10.0, 1.0),
            BreadcrumbVisibility {
                start_index: 2,
                show_ellipsis: true
            }
        );
    }

    #[test]
    fn nav_button_active_opacity_dims_button() {
        assert_eq!(NAV_BUTTON_ACTIVE_OPACITY, 0.7);
        assert!(NAV_BUTTON_ACTIVE_OPACITY < 1.0);
    }

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

    #[test]
    fn empty_folder_message_uses_compact_text() {
        assert_eq!(EMPTY_FOLDER_TEXT_SIZE, 12.0);
        assert_eq!(EMPTY_FOLDER_TOP_MARGIN, 20.0);
        assert_eq!(EMPTY_FOLDER_MESSAGE, "This folder is empty.");
    }

    #[test]
    fn modified_time_uses_local_explorer_format() {
        let local = Local.with_ymd_and_hms(2026, 5, 31, 21, 48, 12).unwrap();
        assert_eq!(format_modified(Some(local.into())), "31/05/2026 21:48");
    }

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

        let action = view.handle_entry_click(&entry, 1);

        assert_eq!(action, None);
        assert_eq!(view.path, temp.path());
        assert_eq!(view.selected_paths(), vec![child]);
        assert_eq!(view.open_error, None);
        assert!(view.back_stack.is_empty());
        assert!(view.forward_stack.is_empty());
    }

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

        let action = view.handle_entry_click(&file_entry, 2);
        assert_eq!(action, Some(EntryAction::OpenFile(file.clone())));
        assert_eq!(view.path, temp.path());
        assert_eq!(view.selected_paths(), vec![file.clone()]);
        assert!(view.back_stack.is_empty());
        assert!(view.forward_stack.is_empty());

        let action = view.handle_entry_click(&dir_entry, 2);
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
        assert!(view.back_stack.is_empty());
        assert_eq!(view.forward_stack, vec![child.clone()]);

        view.navigate_forward();
        assert_eq!(view.path, child);
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn up_navigates_to_parent_and_records_history() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        let grandchild = child.join("grandchild");
        fs::create_dir_all(&grandchild).expect("create nested directories");

        let mut view = ExplorerView::new(grandchild.clone());

        view.navigate_up();

        assert_eq!(view.path, child);
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
