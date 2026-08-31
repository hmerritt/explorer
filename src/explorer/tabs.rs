use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use gpui::{
    AnyElement, App, Bounds, ClickEvent, Context, CursorStyle, DragMoveEvent, Entity,
    ExternalPaths, FileDropEvent, FocusHandle, Focusable, IntoElement, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render,
    ScrollHandle, SharedString, Styled, Window, canvas, div, font, prelude::*, px, relative, rgb,
    rgba,
};

use crate::explorer::{
    CloseTab, FocusPaneDown, FocusPaneLeft, FocusPaneRight, FocusPaneUp, MovePaneDown,
    MovePaneLeft, MovePaneRight, MovePaneUp, NewTab, NewWindow, SelectNextTab, SelectPreviousTab,
    SelectTabByIndex, SplitPaneDown, SplitPaneLeft, SplitPaneRight, SplitPaneUp,
    constants::{NAV_BUTTON_ACTIVE_OPACITY, NAV_BUTTON_HOVER_BG},
    drag_drop::{DraggedEntries, DropDestination},
    icons::{
        drive_wsl_icon, drives_group_icon, folder_icon, network_group_icon, pinned_group_icon,
    },
    render::render_drop_indicator,
    view::{ExplorerView, ExplorerViewEvent},
};
use crate::settings::{SettingsState, SidebarGroupKind};
use crate::window_chrome::{
    MAC_TRAFFIC_LIGHT_PADDING, TITLEBAR_HEIGHT, WindowDragState, render_platform_window_frame,
    render_titlebar_drag_region, render_window_controls,
};

const TAB_BAR_HEIGHT: f32 = TITLEBAR_HEIGHT;
const TAB_WIDTH: f32 = 225.0;
const TAB_MIN_WIDTH: f32 = 160.0;
const TAB_HORIZONTAL_PADDING: f32 = 10.0;
const TAB_ICON_GAP: f32 = 8.0;
const TAB_CLOSE_SIZE: f32 = 22.0;
const TAB_TEXT_SIZE: f32 = 12.0;
const TAB_ACTIVE_BG: u32 = 0xf8f8f8;
const TAB_INACTIVE_BG: u32 = 0xe8e8e8;
const TAB_BORDER: u32 = 0xe7e7e7;
const TAB_HOVER_BG: u32 = 0xf3f3f3;
const TAB_TEXT_COLOR: u32 = 0x1f1f1f;
const TAB_ICON_TEXT_SIZE: f32 = 11.0;
const TAB_REORDER_VERTICAL_TOLERANCE: f32 = 100.0;
const PANE_MIN_WIDTH: f32 = 160.0;
const PANE_MIN_HEIGHT: f32 = 120.0;
const SPLIT_DIVIDER_SIZE: f32 = 1.0;
const SPLIT_DIVIDER_HIT_SIZE: f32 = 7.0;
const SPLIT_BORDER: u32 = 0xd9d9d9;
const SPLIT_FOCUS_BLUE: u32 = 0x4b91d1;
const CLOSE_GLYPH: &str = "\u{E711}";
const NEW_TAB_GLYPH: &str = "\u{E710}";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TabId(u64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct PaneId(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SplitDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct NormalizedPaneRect {
    pane: PaneId,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl NormalizedPaneRect {
    fn horizontal_center(self) -> f32 {
        (self.left + self.right) / 2.0
    }

    fn vertical_center(self) -> f32 {
        (self.top + self.bottom) / 2.0
    }
}

impl SplitDirection {
    fn axis(self) -> SplitAxis {
        match self {
            Self::Left | Self::Right => SplitAxis::Horizontal,
            Self::Up | Self::Down => SplitAxis::Vertical,
        }
    }

    fn increasing(self) -> bool {
        matches!(self, Self::Right | Self::Down)
    }

    fn opposite(self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Down => Self::Up,
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum PaneNode {
    Leaf(PaneId),
    Split {
        id: u64,
        axis: SplitAxis,
        ratio: f32,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
}

impl PaneNode {
    fn pane_ids(&self, ids: &mut Vec<PaneId>) {
        match self {
            Self::Leaf(id) => ids.push(*id),
            Self::Split { first, second, .. } => {
                first.pane_ids(ids);
                second.pane_ids(ids);
            }
        }
    }

    fn pane_count(&self) -> usize {
        match self {
            Self::Leaf(_) => 1,
            Self::Split { first, second, .. } => first.pane_count() + second.pane_count(),
        }
    }

    fn split_ids(&self, ids: &mut HashSet<u64>) {
        if let Self::Split {
            id, first, second, ..
        } = self
        {
            ids.insert(*id);
            first.split_ids(ids);
            second.split_ids(ids);
        }
    }

    fn contains(&self, pane_id: PaneId) -> bool {
        match self {
            Self::Leaf(id) => *id == pane_id,
            Self::Split { first, second, .. } => {
                first.contains(pane_id) || second.contains(pane_id)
            }
        }
    }

    fn insert_split(
        &mut self,
        target: PaneId,
        inserted: PaneId,
        direction: SplitDirection,
        split_id: u64,
    ) -> bool {
        match self {
            Self::Leaf(id) if *id == target => {
                let target_node = Self::Leaf(target);
                let inserted_node = Self::Leaf(inserted);
                let (first, second) = if direction.increasing() {
                    (target_node, inserted_node)
                } else {
                    (inserted_node, target_node)
                };
                *self = Self::Split {
                    id: split_id,
                    axis: direction.axis(),
                    ratio: 0.5,
                    first: Box::new(first),
                    second: Box::new(second),
                };
                true
            }
            Self::Leaf(_) => false,
            Self::Split { first, second, .. } => {
                first.insert_split(target, inserted, direction, split_id)
                    || second.insert_split(target, inserted, direction, split_id)
            }
        }
    }

    fn remove(&mut self, pane_id: PaneId) -> bool {
        match self {
            Self::Leaf(_) => false,
            Self::Split { first, second, .. } => {
                if matches!(first.as_ref(), Self::Leaf(id) if *id == pane_id) {
                    *self = *std::mem::replace(second, Box::new(Self::Leaf(pane_id)));
                    return true;
                }
                if matches!(second.as_ref(), Self::Leaf(id) if *id == pane_id) {
                    *self = *std::mem::replace(first, Box::new(Self::Leaf(pane_id)));
                    return true;
                }
                first.remove(pane_id) || second.remove(pane_id)
            }
        }
    }

    fn first_pane(&self) -> PaneId {
        match self {
            Self::Leaf(id) => *id,
            Self::Split { first, .. } => first.first_pane(),
        }
    }

    fn set_ratio(&mut self, split_id: u64, ratio: f32) -> bool {
        match self {
            Self::Leaf(_) => false,
            Self::Split {
                id,
                ratio: current,
                first,
                second,
                ..
            } => {
                if *id == split_id {
                    *current = ratio;
                    true
                } else {
                    first.set_ratio(split_id, ratio) || second.set_ratio(split_id, ratio)
                }
            }
        }
    }

    fn split_ratio(&self, split_id: u64) -> Option<(SplitAxis, f32)> {
        match self {
            Self::Leaf(_) => None,
            Self::Split {
                id,
                axis,
                ratio,
                first,
                second,
            } => {
                if *id == split_id {
                    Some((*axis, *ratio))
                } else {
                    first
                        .split_ratio(split_id)
                        .or_else(|| second.split_ratio(split_id))
                }
            }
        }
    }

    fn normalized_rects(&self) -> Vec<NormalizedPaneRect> {
        let mut rects = Vec::with_capacity(self.pane_count());
        self.collect_normalized_rects(0.0, 0.0, 1.0, 1.0, &mut rects);
        rects
    }

    fn collect_normalized_rects(
        &self,
        left: f32,
        top: f32,
        right: f32,
        bottom: f32,
        rects: &mut Vec<NormalizedPaneRect>,
    ) {
        match self {
            Self::Leaf(pane) => rects.push(NormalizedPaneRect {
                pane: *pane,
                left,
                top,
                right,
                bottom,
            }),
            Self::Split {
                axis,
                ratio,
                first,
                second,
                ..
            } => match axis {
                SplitAxis::Horizontal => {
                    let split = left + ((right - left) * ratio);
                    first.collect_normalized_rects(left, top, split, bottom, rects);
                    second.collect_normalized_rects(split, top, right, bottom, rects);
                }
                SplitAxis::Vertical => {
                    let split = top + ((bottom - top) * ratio);
                    first.collect_normalized_rects(left, top, right, split, rects);
                    second.collect_normalized_rects(left, split, right, bottom, rects);
                }
            },
        }
    }

    fn adjacent_pane(&self, pane: PaneId, direction: SplitDirection) -> Option<PaneId> {
        const EPSILON: f32 = 0.000_001;

        let rects = self.normalized_rects();
        let current = rects.iter().find(|rect| rect.pane == pane).copied()?;
        rects
            .iter()
            .enumerate()
            .filter(|(_, candidate)| candidate.pane != pane)
            .filter_map(|(order, candidate)| {
                let perpendicular_overlap = match direction {
                    SplitDirection::Left | SplitDirection::Right => {
                        current.bottom.min(candidate.bottom) - current.top.max(candidate.top)
                    }
                    SplitDirection::Up | SplitDirection::Down => {
                        current.right.min(candidate.right) - current.left.max(candidate.left)
                    }
                };
                if perpendicular_overlap <= EPSILON {
                    return None;
                }

                let (in_direction, boundary_gap, center_gap) = match direction {
                    SplitDirection::Left => (
                        candidate.right <= current.left + EPSILON,
                        current.left - candidate.right,
                        (current.vertical_center() - candidate.vertical_center()).abs(),
                    ),
                    SplitDirection::Right => (
                        candidate.left + EPSILON >= current.right,
                        candidate.left - current.right,
                        (current.vertical_center() - candidate.vertical_center()).abs(),
                    ),
                    SplitDirection::Up => (
                        candidate.bottom <= current.top + EPSILON,
                        current.top - candidate.bottom,
                        (current.horizontal_center() - candidate.horizontal_center()).abs(),
                    ),
                    SplitDirection::Down => (
                        candidate.top + EPSILON >= current.bottom,
                        candidate.top - current.bottom,
                        (current.horizontal_center() - candidate.horizontal_center()).abs(),
                    ),
                };
                in_direction.then_some((candidate.pane, boundary_gap.max(0.0), center_gap, order))
            })
            .min_by(|a, b| {
                a.1.total_cmp(&b.1)
                    .then_with(|| a.2.total_cmp(&b.2))
                    .then_with(|| a.3.cmp(&b.3))
            })
            .map(|(pane, _, _, _)| pane)
    }

    fn swap_panes(&mut self, first_pane: PaneId, second_pane: PaneId) -> bool {
        if first_pane == second_pane || !self.contains(first_pane) || !self.contains(second_pane) {
            return false;
        }

        self.swap_pane_ids(first_pane, second_pane);
        true
    }

    fn swap_pane_ids(&mut self, first_pane: PaneId, second_pane: PaneId) {
        match self {
            Self::Leaf(pane) if *pane == first_pane => *pane = second_pane,
            Self::Leaf(pane) if *pane == second_pane => *pane = first_pane,
            Self::Leaf(_) => {}
            Self::Split { first, second, .. } => {
                first.swap_pane_ids(first_pane, second_pane);
                second.swap_pane_ids(first_pane, second_pane);
            }
        }
    }

    fn is_outer_leaf(&self, pane: PaneId, direction: SplitDirection) -> bool {
        let Self::Split {
            axis,
            first,
            second,
            ..
        } = self
        else {
            return false;
        };
        if *axis != direction.axis() {
            return false;
        }

        let outer = if direction.increasing() {
            second
        } else {
            first
        };
        matches!(outer.as_ref(), Self::Leaf(id) if *id == pane)
    }

    fn move_pane_to_outer_edge(
        &mut self,
        pane: PaneId,
        direction: SplitDirection,
        split_id: u64,
    ) -> bool {
        if self.is_outer_leaf(pane, direction) || !self.remove(pane) {
            return false;
        }

        let remaining = std::mem::replace(self, Self::Leaf(pane));
        let moved = Self::Leaf(pane);
        let (first, second) = if direction.increasing() {
            (remaining, moved)
        } else {
            (moved, remaining)
        };
        *self = Self::Split {
            id: split_id,
            axis: direction.axis(),
            ratio: 0.5,
            first: Box::new(first),
            second: Box::new(second),
        };
        true
    }
}

#[derive(Clone)]
struct ExplorerPane {
    id: PaneId,
    view: Entity<ExplorerView>,
}

#[derive(Clone)]
struct ExplorerTab {
    id: TabId,
    // Kept as the focused view for existing integrations and test helpers.
    view: Entity<ExplorerView>,
    panes: Vec<ExplorerPane>,
    layout: PaneNode,
    active_pane: PaneId,
}

impl ExplorerTab {
    fn single(id: TabId, pane_id: PaneId, view: Entity<ExplorerView>) -> Self {
        Self {
            id,
            view: view.clone(),
            panes: vec![ExplorerPane { id: pane_id, view }],
            layout: PaneNode::Leaf(pane_id),
            active_pane: pane_id,
        }
    }

    fn pane(&self, pane_id: PaneId) -> Option<&ExplorerPane> {
        self.panes.iter().find(|pane| pane.id == pane_id)
    }

    fn active_view(&self) -> Entity<ExplorerView> {
        self.view.clone()
    }

    fn activate_pane(&mut self, pane_id: PaneId) -> bool {
        let Some(view) = self.pane(pane_id).map(|pane| pane.view.clone()) else {
            return false;
        };
        let changed = self.active_pane != pane_id;
        self.active_pane = pane_id;
        self.view = view;
        changed
    }

    fn is_split(&self) -> bool {
        self.panes.len() > 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct DockTarget {
    workspace_tab: TabId,
    pane: PaneId,
    direction: SplitDirection,
}

#[derive(Clone, Copy, Debug)]
struct SplitResizeDrag {
    workspace_tab: TabId,
    split_id: u64,
    axis: SplitAxis,
    start_pointer: f32,
    start_ratio: f32,
}

#[derive(Clone, Copy, Debug)]
struct TabContextMenu {
    tab: TabId,
    position: Point<Pixels>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TabDrag {
    id: TabId,
    label: SharedString,
    path: PathBuf,
    is_active: bool,
    dockable: bool,
}

struct TabDragPreview {
    label: SharedString,
    path: PathBuf,
    is_active: bool,
    pane_count: usize,
    font: gpui::Font,
}

pub struct ExplorerTabs {
    tabs: Vec<ExplorerTab>,
    active_tab: TabId,
    next_tab_id: u64,
    next_pane_id: u64,
    next_split_id: u64,
    background_operation_tabs: Vec<Entity<ExplorerView>>,
    dragging_tab: Option<TabId>,
    dock_target: Option<DockTarget>,
    pane_bounds: HashMap<PaneId, Bounds<Pixels>>,
    split_bounds: HashMap<u64, Bounds<Pixels>>,
    split_resize_drag: Option<SplitResizeDrag>,
    tab_context_menu: Option<TabContextMenu>,
    tab_scroll_handle: ScrollHandle,
    should_move_window: bool,
}

impl WindowDragState for ExplorerTabs {
    fn set_window_drag_pending(&mut self, pending: bool) {
        self.should_move_window = pending;
    }

    fn take_window_drag_pending(&mut self) -> bool {
        let pending = self.should_move_window;
        self.should_move_window = false;
        pending
    }
}

impl ExplorerTabs {
    pub fn new(
        initial_path: PathBuf,
        focus_handle: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let first_id = TabId(1);
        let view = cx.new(|cx| {
            let mut view = ExplorerView::new_watched_with_focus_handle(
                initial_path,
                focus_handle,
                Some(window),
                cx,
            );
            view.set_shared_chrome_hosted(true);
            view
        });
        observe_tab_view(&view, window, cx);
        observe_settings(cx);
        observe_window_activation(window, cx);
        crate::explorer::clipboard::refresh_clipboard_summary(cx);

        Self {
            tabs: vec![ExplorerTab::single(first_id, PaneId(1), view)],
            active_tab: first_id,
            next_tab_id: 2,
            next_pane_id: 2,
            next_split_id: 1,
            background_operation_tabs: Vec::new(),
            dragging_tab: None,
            dock_target: None,
            pane_bounds: HashMap::new(),
            split_bounds: HashMap::new(),
            split_resize_drag: None,
            tab_context_menu: None,
            tab_scroll_handle: ScrollHandle::new(),
            should_move_window: false,
        }
    }

    #[cfg(test)]
    fn new_for_test(
        initial_path: PathBuf,
        focus_handle: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let first_id = TabId(1);
        let view = cx.new(|_| {
            let mut view = ExplorerView::new_with_focus_handle_for_test(initial_path, focus_handle);
            view.set_shared_chrome_hosted(true);
            view
        });

        Self {
            tabs: vec![ExplorerTab::single(first_id, PaneId(1), view)],
            active_tab: first_id,
            next_tab_id: 2,
            next_pane_id: 2,
            next_split_id: 1,
            background_operation_tabs: Vec::new(),
            dragging_tab: None,
            dock_target: None,
            pane_bounds: HashMap::new(),
            split_bounds: HashMap::new(),
            split_resize_drag: None,
            tab_context_menu: None,
            tab_scroll_handle: ScrollHandle::new(),
            should_move_window: false,
        }
    }

    fn active_tab_index(&self) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == self.active_tab)
    }

    fn active_tab(&self) -> Option<&ExplorerTab> {
        self.tabs.iter().find(|tab| tab.id == self.active_tab)
    }

    pub(crate) fn active_path(&self, cx: &App) -> Option<PathBuf> {
        self.active_tab()
            .map(|tab| tab.active_view().read(cx).path().to_path_buf())
    }

    fn tab_view(&self, id: TabId) -> Option<Entity<ExplorerView>> {
        self.tabs
            .iter()
            .find(|tab| tab.id == id)
            .map(ExplorerTab::active_view)
    }

    fn add_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = cx.global::<SettingsState>().startup_path();
        self.add_foreground_tab(path, window, cx);
    }

    fn create_pane(
        &mut self,
        path: PathBuf,
        focus_handle: FocusHandle,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> ExplorerPane {
        let pane_id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        let view = cx.new(|cx| {
            let mut view =
                ExplorerView::new_watched_with_focus_handle(path, focus_handle, Some(window), cx);
            view.set_shared_chrome_hosted(true);
            view
        });
        observe_tab_view(&view, window, cx);
        ExplorerPane { id: pane_id, view }
    }

    fn add_foreground_tab(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let id = TabId(self.next_tab_id);
        self.next_tab_id += 1;
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        let pane = self.create_pane(path, focus_handle, window, cx);

        self.tabs.push(ExplorerTab::single(id, pane.id, pane.view));
        self.cancel_active_tab_thumbnail_extraction(cx);
        self.active_tab = id;
        self.scroll_active_tab_into_view();
    }

    fn add_background_tab(&mut self, path: PathBuf, window: &Window, cx: &mut Context<Self>) {
        let id = TabId(self.next_tab_id);
        self.next_tab_id += 1;
        let focus_handle = cx.focus_handle();
        let pane = self.create_pane(path, focus_handle, window, cx);

        self.tabs.push(ExplorerTab::single(id, pane.id, pane.view));
    }

    fn add_configured_tab(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if cx.global::<SettingsState>().value.tabs.focus_new {
            self.add_foreground_tab(path, window, cx);
        } else {
            self.add_background_tab(path, window, cx);
        }
    }

    fn activate_tab(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_tab == id || !self.tabs.iter().any(|tab| tab.id == id) {
            return;
        }

        self.cancel_active_tab_thumbnail_extraction(cx);
        self.active_tab = id;
        self.scroll_active_tab_into_view();
        self.focus_active_tab(window, cx);
    }

    fn activate_tab_for_file_drag_hover(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !tab_can_activate_for_file_drag_hover(self.active_tab, id, &self.tabs) {
            return false;
        }

        let was_active = self.active_tab;
        self.activate_tab(id, window, cx);
        self.active_tab != was_active
    }

    fn can_drop_on_tab(
        &self,
        id: TabId,
        dragged_value: &dyn std::any::Any,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(view) = self.tab_view(id) else {
            return false;
        };

        view.update(cx, |view, _| {
            view.can_drop_value_nonblocking(
                dragged_value,
                &DropDestination::CurrentDirectory,
                modifiers,
            )
        })
    }

    fn drop_internal_entries_on_tab(
        &mut self,
        id: TabId,
        dragged: &DraggedEntries,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self.tab_view(id) {
            let _ = view.update(cx, |view, cx| {
                view.clear_drop_indicator();
                view.drop_internal_entries_and_open_dialog(
                    dragged,
                    DropDestination::CurrentDirectory,
                    window.modifiers(),
                    cx,
                );
                cx.notify();
            });
        }
    }

    fn drop_external_paths_on_tab(
        &mut self,
        id: TabId,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self.tab_view(id) {
            let _ = view.update(cx, |view, cx| {
                view.clear_drop_indicator();
                view.drop_external_paths_and_open_dialog(
                    paths.paths(),
                    DropDestination::CurrentDirectory,
                    window.modifiers(),
                    window,
                    cx,
                );
                cx.notify();
            });
        }
    }

    fn focus_active_tab(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab() {
            let focus_handle = tab.active_view().read(cx).focus_handle(cx);
            focus_handle.focus(window);
        }
    }

    fn activate_pane(
        &mut self,
        workspace_tab: TabId,
        pane_id: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_index) = self.tabs.iter().position(|tab| tab.id == workspace_tab) else {
            return;
        };
        if !self.tabs[tab_index].layout.contains(pane_id) {
            return;
        }

        if self.active_tab == workspace_tab && self.tabs[tab_index].active_pane == pane_id {
            return;
        }

        if let Some(previous) = self.active_tab().map(ExplorerTab::active_view) {
            let _ = previous.update(cx, |view, cx| {
                view.finish_search_edit();
                view.cancel_address_bar_edit();
                view.close_context_menu();
                view.open_utility_menu = None;
                cx.notify();
            });
        }

        self.active_tab = workspace_tab;
        self.tabs[tab_index].activate_pane(pane_id);
        self.tab_context_menu = None;
        self.focus_active_tab(window, cx);
    }

    fn close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_tab = self.active_tab;
        if self.active_tab().is_some_and(|tab| tab.panes.len() > 1) {
            self.close_focused_pane(active_tab, window, cx);
        } else {
            self.close_tab(active_tab, window, cx);
        }
    }

    fn close_tab(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        let closes_only_split = self.tabs.len() == 1 && self.tabs[index].is_split();
        if self.tabs.len() == 1 && !closes_only_split {
            return;
        }

        let closing = self.tabs.remove(index);
        for pane in closing.panes {
            self.prepare_closed_view(pane.view, cx);
        }

        if closes_only_split {
            let path = cx.global::<SettingsState>().startup_path();
            self.add_foreground_tab(path, window, cx);
        } else if self.active_tab == id {
            if let Some(next_active) = active_id_after_close_from_removed(&self.tabs, index) {
                self.active_tab = next_active;
            }
            self.scroll_active_tab_into_view();
            self.focus_active_tab(window, cx);
        } else {
            self.scroll_active_tab_into_view();
        }
        self.tab_context_menu = None;
        self.dock_target = None;
        self.clear_obsolete_layout_state();
    }

    fn prepare_closed_view(&mut self, view: Entity<ExplorerView>, cx: &mut Context<Self>) {
        let has_active_operation = view.read(cx).has_background_operation();
        let _ = view.update(cx, |view, cx| {
            view.prepare_for_tab_close(cx);
            cx.notify();
        });
        if has_active_operation {
            self.background_operation_tabs.push(view);
        }
    }

    fn close_focused_pane(
        &mut self,
        workspace_tab: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_index) = self.tabs.iter().position(|tab| tab.id == workspace_tab) else {
            return;
        };
        let pane_id = self.tabs[tab_index].active_pane;
        if self.tabs[tab_index].panes.len() <= 1 {
            self.close_tab(workspace_tab, window, cx);
            return;
        }

        let removed = {
            let tab = &mut self.tabs[tab_index];
            if !tab.layout.remove(pane_id) {
                return;
            }
            let Some(pane_index) = tab.panes.iter().position(|pane| pane.id == pane_id) else {
                return;
            };
            let removed = tab.panes.remove(pane_index);
            let next_pane = tab.layout.first_pane();
            tab.activate_pane(next_pane);
            removed
        };
        self.prepare_closed_view(removed.view, cx);
        self.clear_obsolete_layout_state();
        self.focus_active_tab(window, cx);
        self.dock_target = None;
    }

    fn split_tab_into_pane(
        &mut self,
        source_tab: TabId,
        target_tab: TabId,
        target_pane: PaneId,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if source_tab == target_tab {
            return false;
        }
        let Some(source_index) = self.tabs.iter().position(|tab| tab.id == source_tab) else {
            return false;
        };
        if self.tabs[source_index].panes.len() != 1 {
            return false;
        }
        let Some(target_index_before) = self.tabs.iter().position(|tab| tab.id == target_tab)
        else {
            return false;
        };
        if !self.tabs[target_index_before].layout.contains(target_pane) {
            return false;
        }

        let source = self.tabs.remove(source_index);
        let target_index = if source_index < target_index_before {
            target_index_before - 1
        } else {
            target_index_before
        };
        let mut source_panes = source.panes;
        let source_pane = source_panes.remove(0);
        let split_id = self.next_split_id;
        self.next_split_id += 1;
        let target = &mut self.tabs[target_index];
        let inserted = target
            .layout
            .insert_split(target_pane, source_pane.id, direction, split_id);
        debug_assert!(inserted, "validated target pane must accept split");
        if !inserted {
            return false;
        }
        target.panes.push(source_pane);
        let inserted_id = target.panes.last().expect("inserted pane").id;
        target.activate_pane(inserted_id);
        self.active_tab = target.id;
        self.dock_target = None;
        self.clear_obsolete_layout_state();
        self.scroll_active_tab_into_view();
        self.focus_active_tab(window, cx);
        true
    }

    fn self_dock_active_tab(
        &mut self,
        workspace_tab: TabId,
        target_pane: PaneId,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target) = self.tabs.iter().find(|tab| tab.id == workspace_tab) else {
            return false;
        };
        if self.active_tab != workspace_tab
            || target.is_split()
            || target.active_pane != target_pane
            || !matches!(target.layout, PaneNode::Leaf(id) if id == target_pane)
        {
            return false;
        }

        let helper_pane_id = PaneId(self.next_pane_id);
        let split_id = self.next_split_id;
        let mut layout = target.layout.clone();
        if !layout.insert_split(target_pane, helper_pane_id, direction.opposite(), split_id) {
            return false;
        }

        let path = cx.global::<SettingsState>().startup_path();
        let focus_handle = cx.focus_handle();
        let helper_pane = self.create_pane(path, focus_handle, window, cx);
        debug_assert_eq!(helper_pane.id, helper_pane_id);
        self.next_split_id += 1;

        let target_index = self
            .tabs
            .iter()
            .position(|tab| tab.id == workspace_tab)
            .expect("validated target tab must remain present");
        let target = &mut self.tabs[target_index];
        target.layout = layout;
        target.panes.push(helper_pane);
        target.activate_pane(target_pane);
        self.active_tab = workspace_tab;
        self.dock_target = None;
        self.clear_obsolete_layout_state();
        self.scroll_active_tab_into_view();
        self.focus_active_tab(window, cx);
        true
    }

    fn split_active_pane(
        &mut self,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab_index) = self.active_tab_index() else {
            return false;
        };
        let target_pane = self.tabs[tab_index].active_pane;
        let Some(bounds) = self.pane_bounds.get(&target_pane).copied() else {
            return false;
        };
        if !pane_bounds_allow_split(bounds, direction) {
            return false;
        }

        let inserted_pane = PaneId(self.next_pane_id);
        let split_id = self.next_split_id;
        let mut layout = self.tabs[tab_index].layout.clone();
        if !layout.insert_split(target_pane, inserted_pane, direction, split_id) {
            return false;
        }

        let path = cx.global::<SettingsState>().startup_path();
        let focus_handle = cx.focus_handle();
        let pane = self.create_pane(path, focus_handle, window, cx);
        debug_assert_eq!(pane.id, inserted_pane);
        self.next_split_id += 1;

        let target = &mut self.tabs[tab_index];
        target.layout = layout;
        target.panes.push(pane);
        target.activate_pane(inserted_pane);
        self.dock_target = None;
        self.clear_obsolete_layout_state();
        self.focus_active_tab(window, cx);
        true
    }

    fn move_active_pane(
        &mut self,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab_index) = self.active_tab_index() else {
            return false;
        };
        let active_pane = self.tabs[tab_index].active_pane;
        let mut layout = self.tabs[tab_index].layout.clone();

        let changed = if let Some(adjacent) = layout.adjacent_pane(active_pane, direction) {
            layout.swap_panes(active_pane, adjacent)
        } else {
            let moved = layout.move_pane_to_outer_edge(active_pane, direction, self.next_split_id);
            if moved {
                self.next_split_id += 1;
            }
            moved
        };
        if !changed {
            return false;
        }

        self.tabs[tab_index].layout = layout;
        self.dock_target = None;
        self.clear_obsolete_layout_state();
        self.focus_active_tab(window, cx);
        true
    }

    fn focus_adjacent_pane(
        &mut self,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((workspace_tab, adjacent_pane)) = self
            .active_tab()
            .filter(|tab| tab.is_split())
            .and_then(|tab| {
                tab.layout
                    .adjacent_pane(tab.active_pane, direction)
                    .map(|pane| (tab.id, pane))
            })
        else {
            return false;
        };

        self.activate_pane(workspace_tab, adjacent_pane, window, cx);
        true
    }

    fn clear_obsolete_layout_state(&mut self) {
        let mut split_ids = HashSet::new();
        for tab in &self.tabs {
            tab.layout.split_ids(&mut split_ids);
        }

        self.pane_bounds.clear();
        self.split_bounds
            .retain(|split, _| split_ids.contains(split));
        if self
            .split_resize_drag
            .is_some_and(|drag| !split_ids.contains(&drag.split_id))
        {
            self.split_resize_drag = None;
        }
    }

    fn unsplit_tab(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return false;
        };
        if !self.tabs[index].is_split() {
            return false;
        }
        let was_active = self.active_tab == id;
        let split = self.tabs.remove(index);
        let focused_pane = split.active_pane;
        let mut order = Vec::new();
        split.layout.pane_ids(&mut order);
        let mut panes = split.panes;
        let mut restored = Vec::with_capacity(order.len());
        let mut focused_tab = None;
        for pane_id in order {
            let pane_index = panes
                .iter()
                .position(|pane| pane.id == pane_id)
                .expect("split layout pane must exist");
            let pane = panes.remove(pane_index);
            let tab_id = TabId(self.next_tab_id);
            self.next_tab_id += 1;
            if pane.id == focused_pane {
                focused_tab = Some(tab_id);
            }
            restored.push(ExplorerTab::single(tab_id, pane.id, pane.view));
        }
        self.tabs.splice(index..index, restored);
        if was_active {
            self.active_tab = focused_tab.expect("focused split pane must be restored");
            self.scroll_active_tab_into_view();
            self.focus_active_tab(window, cx);
        }
        self.tab_context_menu = None;
        self.clear_obsolete_layout_state();
        true
    }

    fn select_adjacent_tab(
        &mut self,
        direction: TabDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(active_index) = self.active_tab_index() else {
            return;
        };
        let next_index = adjacent_tab_index(active_index, self.tabs.len(), direction);
        let next_tab = self.tabs[next_index].id;
        if next_tab == self.active_tab {
            return;
        }

        self.cancel_active_tab_thumbnail_extraction(cx);
        self.active_tab = next_tab;
        self.scroll_active_tab_into_view();
        self.focus_active_tab(window, cx);
    }

    fn select_tab_by_index(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target_id) = selectable_tab_id_by_index(&self.tabs, self.active_tab, index) else {
            return false;
        };

        self.cancel_active_tab_thumbnail_extraction(cx);
        self.active_tab = target_id;
        self.scroll_active_tab_into_view();
        self.focus_active_tab(window, cx);
        true
    }

    fn reorder_dragged_tab(&mut self, dragged_id: TabId, target_id: TabId, before: bool) -> bool {
        reorder_tabs(&mut self.tabs, dragged_id, target_id, before)
    }

    fn start_tab_drag(&mut self, id: TabId) {
        start_dragging_tab(&mut self.dragging_tab, id);
    }

    fn clear_tab_drag(&mut self) -> bool {
        clear_dragging_tab(&mut self.dragging_tab)
    }

    fn scroll_active_tab_into_view(&self) {
        if let Some(index) = self.active_tab_index() {
            self.tab_scroll_handle.scroll_to_item(index);
        }
    }

    fn cancel_active_tab_thumbnail_extraction(&self, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab() {
            let _ = tab.active_view().update(cx, |view, cx| {
                view.cancel_image_thumbnail_extraction(cx);
                view.cancel_video_hover_preview(cx);
                view.cancel_text_hover_preview();
            });
        }
    }

    fn reload_tabs_except(&mut self, source_view: &Entity<ExplorerView>, cx: &mut Context<Self>) {
        let source_view_id = source_view.entity_id();
        for tab in &self.tabs {
            for pane in &tab.panes {
                if pane.view.entity_id() == source_view_id {
                    continue;
                }
                let _ = pane.view.update(cx, |view, cx| {
                    view.reload_async_with_entry_metadata_resolution(cx);
                    cx.notify();
                });
            }
        }
    }

    fn redirect_tabs_after_mounted_volume_ejected(
        &mut self,
        ejected_root: &Path,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut redirected = false;
        for tab in &self.tabs {
            for pane in &tab.panes {
                let _ = pane.view.update(cx, |view, cx| {
                    if view.redirect_after_mounted_volume_ejected_with_watcher(ejected_root, cx) {
                        redirected = true;
                        cx.notify();
                    }
                });
            }
        }
        redirected
    }

    fn apply_settings_to_all_tabs(&mut self, cx: &mut Context<Self>) {
        let settings = cx.global::<SettingsState>().value.clone();
        for tab in &self.tabs {
            for pane in &tab.panes {
                let _ = pane
                    .view
                    .update(cx, |view, cx| view.apply_settings(&settings, cx));
            }
        }
        cx.notify();
    }

    fn cleanup_completed_background_operations(&mut self, cx: &mut Context<Self>) {
        let mut still_running = Vec::new();

        for view in std::mem::take(&mut self.background_operation_tabs) {
            if view.read(cx).has_background_operation() {
                still_running.push(view);
            }
        }

        self.background_operation_tabs = still_running;
    }

    fn handle_new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.add_new_tab(window, cx);
        cx.notify();
    }

    fn handle_new_window(&mut self, _: &NewWindow, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.active_path(cx) {
            crate::app::open_new_explorer_window(path, window.window_bounds(), cx);
        }
    }

    fn handle_close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_active_tab(window, cx);
        cx.notify();
    }

    fn handle_select_next_tab(
        &mut self,
        _: &SelectNextTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_adjacent_tab(TabDirection::Next, window, cx);
        cx.notify();
    }

    fn handle_select_previous_tab(
        &mut self,
        _: &SelectPreviousTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_adjacent_tab(TabDirection::Previous, window, cx);
        cx.notify();
    }

    fn handle_select_tab_by_index(
        &mut self,
        action: &SelectTabByIndex,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.select_tab_by_index(action.index, window, cx) {
            cx.notify();
        }
    }

    fn handle_split_pane_left(
        &mut self,
        _: &SplitPaneLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.split_active_pane(SplitDirection::Left, window, cx) {
            cx.notify();
        }
    }

    fn handle_split_pane_right(
        &mut self,
        _: &SplitPaneRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.split_active_pane(SplitDirection::Right, window, cx) {
            cx.notify();
        }
    }

    fn handle_split_pane_up(
        &mut self,
        _: &SplitPaneUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.split_active_pane(SplitDirection::Up, window, cx) {
            cx.notify();
        }
    }

    fn handle_split_pane_down(
        &mut self,
        _: &SplitPaneDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.split_active_pane(SplitDirection::Down, window, cx) {
            cx.notify();
        }
    }

    fn handle_focus_pane_left(
        &mut self,
        _: &FocusPaneLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.focus_adjacent_pane(SplitDirection::Left, window, cx) {
            cx.notify();
        }
    }

    fn handle_focus_pane_right(
        &mut self,
        _: &FocusPaneRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.focus_adjacent_pane(SplitDirection::Right, window, cx) {
            cx.notify();
        }
    }

    fn handle_focus_pane_up(
        &mut self,
        _: &FocusPaneUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.focus_adjacent_pane(SplitDirection::Up, window, cx) {
            cx.notify();
        }
    }

    fn handle_focus_pane_down(
        &mut self,
        _: &FocusPaneDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.focus_adjacent_pane(SplitDirection::Down, window, cx) {
            cx.notify();
        }
    }

    fn handle_move_pane_left(
        &mut self,
        _: &MovePaneLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.move_active_pane(SplitDirection::Left, window, cx) {
            cx.notify();
        }
    }

    fn handle_move_pane_right(
        &mut self,
        _: &MovePaneRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.move_active_pane(SplitDirection::Right, window, cx) {
            cx.notify();
        }
    }

    fn handle_move_pane_up(&mut self, _: &MovePaneUp, window: &mut Window, cx: &mut Context<Self>) {
        if self.move_active_pane(SplitDirection::Up, window, cx) {
            cx.notify();
        }
    }

    fn handle_move_pane_down(
        &mut self,
        _: &MovePaneDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.move_active_pane(SplitDirection::Down, window, cx) {
            cx.notify();
        }
    }

    fn render_tab_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let decorations = window.window_decorations();
        let tab_scroll_width = tab_strip_width(self.tabs.len());
        let mut tab_children = self
            .tabs
            .iter()
            .map(|tab| {
                self.render_tab(
                    tab,
                    self.tabs.len() > 1 || tab.is_split(),
                    can_drag_tab(self.tabs.len(), tab.is_split()),
                    cx,
                )
                .into_any_element()
            })
            .collect::<Vec<_>>();
        tab_children.push(new_tab_button(cx));

        div()
            .id("explorer-tab-bar")
            .flex()
            .flex_row()
            .items_end()
            .relative()
            .h(px(TAB_BAR_HEIGHT))
            .w_full()
            .flex_shrink_0()
            .overflow_hidden()
            .bg(rgb(0xe8e8e8))
            .when(
                cfg!(target_os = "macos") && !window.is_fullscreen(),
                |this| {
                    this.child(
                        div()
                            .id("explorer-macos-traffic-light-space")
                            .h_full()
                            .w(px(MAC_TRAFFIC_LIGHT_PADDING))
                            .flex_none()
                            .occlude(),
                    )
                },
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_end()
                    .w(px(tab_scroll_width))
                    .flex_shrink()
                    .min_w(px(0.0))
                    .h_full()
                    .overflow_hidden()
                    .child(
                        div()
                            .id("explorer-tab-scroll")
                            .flex()
                            .flex_row()
                            .items_end()
                            .w_full()
                            .h_full()
                            .overflow_x_scroll()
                            .track_scroll(&self.tab_scroll_handle)
                            .children(tab_children),
                    ),
            )
            .child(render_titlebar_drag_region(
                "explorer-titlebar-drag-region",
                decorations,
                cx,
            ))
            .children(render_window_controls(window))
            .into_any_element()
    }

    fn render_tab(
        &self,
        tab: &ExplorerTab,
        can_close: bool,
        can_drag: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_active = tab.id == self.active_tab;
        let view = tab.active_view();
        let view = view.read(cx);
        let label = SharedString::from(view.tab_label());
        let path = view.path().to_path_buf();
        let sidebar_group = view.active_sidebar_group();
        let pane_count = tab.layout.pane_count();
        let tab_id = tab.id;
        let is_dragging = self.dragging_tab == Some(tab_id);
        let entity = cx.entity();

        let mut rendered_tab = div()
            .id(("explorer-tab", tab.id.0))
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .h_full()
            .w(px(TAB_WIDTH))
            .min_w(px(TAB_MIN_WIDTH))
            .max_w(px(TAB_WIDTH))
            .px(px(TAB_HORIZONTAL_PADDING))
            .gap(px(TAB_ICON_GAP))
            .flex_shrink()
            .overflow_hidden()
            .cursor_default()
            .block_mouse_except_scroll()
            .bg(if is_active {
                rgb(TAB_ACTIVE_BG)
            } else {
                rgb(TAB_INACTIVE_BG)
            })
            .border_r_1()
            .border_color(rgb(TAB_BORDER))
            .when(is_dragging, |this| this.opacity(0.4))
            .when(!is_active, |this| {
                this.hover(|style| style.bg(rgb(TAB_HOVER_BG)))
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.activate_tab(tab_id, window, cx);
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.close_tab(tab_id, window, cx);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.tab_context_menu = Some(TabContextMenu {
                        tab: tab_id,
                        position: event.position,
                    });
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| {
                    if this.clear_tab_drag() {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| {
                    if this.clear_tab_drag() {
                        cx.notify();
                    }
                }),
            )
            .child(tab_inner_contents(
                label.clone(),
                Some(&path),
                sidebar_group,
                pane_count,
                can_close.then(|| close_tab_button(tab_id, cx)),
            ))
            .can_drop({
                let entity = entity.clone();
                move |dragged_value, window, cx| {
                    let modifiers = window.modifiers();
                    entity.update(cx, |this, cx| {
                        this.can_drop_on_tab(tab_id, dragged_value, modifiers, cx)
                    })
                }
            })
            .drag_over::<DraggedEntries>(|style, _, _, _| style.bg(rgb(TAB_HOVER_BG)))
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(TAB_HOVER_BG)))
            .on_drag_move::<DraggedEntries>({
                let entity = entity.clone();
                move |event: &DragMoveEvent<DraggedEntries>, window, cx| {
                    if !event.bounds.contains(&event.event.position) {
                        return;
                    }

                    let _ = entity.update(cx, |this, cx| {
                        if this.activate_tab_for_file_drag_hover(tab_id, window, cx) {
                            cx.notify();
                        }
                    });
                }
            })
            .on_drag_move::<ExternalPaths>({
                let entity = entity.clone();
                move |event: &DragMoveEvent<ExternalPaths>, window, cx| {
                    if !event.bounds.contains(&event.event.position) {
                        return;
                    }

                    let _ = entity.update(cx, |this, cx| {
                        if this.activate_tab_for_file_drag_hover(tab_id, window, cx) {
                            cx.notify();
                        }
                    });
                }
            })
            .on_drop(
                cx.listener(move |this, dragged: &DraggedEntries, window, cx| {
                    this.drop_internal_entries_on_tab(tab_id, dragged, window, cx);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, window, cx| {
                this.drop_external_paths_on_tab(tab_id, paths, window, cx);
                cx.stop_propagation();
                cx.notify();
            }));

        if can_drag {
            let drag_label = label.clone();
            let drag_path = path.clone();
            rendered_tab = rendered_tab
                .on_drag(
                    TabDrag {
                        id: tab_id,
                        label: drag_label,
                        path: drag_path,
                        is_active,
                        dockable: pane_count == 1,
                    },
                    move |drag, _, _, cx| {
                        let font = crate::settings::current_app_font(cx);
                        let _ = entity.update(cx, |this, cx| {
                            this.start_tab_drag(drag.id);
                            cx.notify();
                        });
                        cx.new(|_| TabDragPreview {
                            label: drag.label.clone(),
                            path: drag.path.clone(),
                            is_active: drag.is_active,
                            pane_count,
                            font,
                        })
                    },
                )
                .on_drag_move::<TabDrag>({
                    let entity = cx.entity();
                    move |event: &DragMoveEvent<TabDrag>, _: &mut Window, cx: &mut App| {
                        let left = f32::from(event.bounds.origin.x);
                        let top = f32::from(event.bounds.origin.y);
                        let width = f32::from(event.bounds.size.width);
                        let height = f32::from(event.bounds.size.height);
                        let cursor_x = f32::from(event.event.position.x);
                        let cursor_y = f32::from(event.event.position.y);

                        if !tab_reorder_hit_test(left, top, width, height, cursor_x, cursor_y) {
                            return;
                        }

                        let before = cursor_x < left + (width / 2.0);
                        let dragged_id = event.drag(cx).id;

                        let _ = entity.update(cx, |this, cx| {
                            if this.reorder_dragged_tab(dragged_id, tab_id, before) {
                                cx.notify();
                            }
                        });
                    }
                });
        }

        rendered_tab.into_any_element()
    }

    fn update_dock_target(
        &mut self,
        workspace_tab: TabId,
        pane: PaneId,
        dragged: &TabDrag,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
    ) -> bool {
        if !bounds.contains(&position) {
            let owns_target = self
                .dock_target
                .is_some_and(|target| target.workspace_tab == workspace_tab && target.pane == pane);
            if owns_target {
                self.dock_target = None;
                return true;
            }
            return false;
        }

        let target = self.tabs.iter().find(|tab| tab.id == workspace_tab);
        let ordinary_drop = dragged.id != workspace_tab && target.is_some();
        let self_drop = dragged.id == workspace_tab
            && self.active_tab == workspace_tab
            && target.is_some_and(|tab| {
                !tab.is_split() && tab.active_pane == pane && tab.layout.contains(pane)
            });
        let eligible = dragged.dockable && (ordinary_drop || self_drop);
        let direction = eligible
            .then(|| split_direction_for_position(bounds, position))
            .flatten();
        let next = direction.map(|direction| DockTarget {
            workspace_tab,
            pane,
            direction,
        });
        if self.dock_target == next {
            false
        } else {
            self.dock_target = next;
            true
        }
    }

    fn begin_split_resize(&mut self, workspace_tab: TabId, split_id: u64, pointer: Point<Pixels>) {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == workspace_tab) else {
            return;
        };
        let Some((axis, ratio)) = tab.layout.split_ratio(split_id) else {
            return;
        };
        let start_pointer = match axis {
            SplitAxis::Horizontal => f32::from(pointer.x),
            SplitAxis::Vertical => f32::from(pointer.y),
        };
        self.split_resize_drag = Some(SplitResizeDrag {
            workspace_tab,
            split_id,
            axis,
            start_pointer,
            start_ratio: ratio,
        });
    }

    fn update_split_resize(&mut self, pointer: Point<Pixels>) -> bool {
        let Some(drag) = self.split_resize_drag else {
            return false;
        };
        let Some(bounds) = self.split_bounds.get(&drag.split_id).copied() else {
            return false;
        };
        let (current, total, min_size) = match drag.axis {
            SplitAxis::Horizontal => (
                f32::from(pointer.x),
                f32::from(bounds.size.width),
                PANE_MIN_WIDTH,
            ),
            SplitAxis::Vertical => (
                f32::from(pointer.y),
                f32::from(bounds.size.height),
                PANE_MIN_HEIGHT,
            ),
        };
        if total <= 0.0 {
            return false;
        }
        let min_ratio = (min_size / total).min(0.49);
        let ratio = (drag.start_ratio + (current - drag.start_pointer) / total)
            .clamp(min_ratio, 1.0 - min_ratio);
        self.tabs
            .iter_mut()
            .find(|tab| tab.id == drag.workspace_tab)
            .is_some_and(|tab| tab.layout.set_ratio(drag.split_id, ratio))
    }

    fn render_active_layout(
        &self,
        tab: &ExplorerTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_pane_node(tab.id, tab.active_pane, &tab.layout, window, cx)
    }

    fn render_pane_node(
        &self,
        workspace_tab: TabId,
        active_pane: PaneId,
        node: &PaneNode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            PaneNode::Leaf(pane_id) => {
                let Some(tab) = self.tabs.iter().find(|tab| tab.id == workspace_tab) else {
                    return div().into_any_element();
                };
                let Some(pane) = tab.pane(*pane_id) else {
                    return div().into_any_element();
                };
                let pane_id = *pane_id;
                let view = pane.view.clone();
                let is_active = pane_focus_outline_visible(
                    pane_id == active_pane,
                    tab.is_split(),
                    cx.try_global::<SettingsState>()
                        .is_some_and(|settings| settings.value.tabs.highlight_focused),
                );
                let dock_target = self
                    .dock_target
                    .filter(|_| self.dragging_tab.is_some())
                    .filter(|target| {
                        target.workspace_tab == workspace_tab && target.pane == pane_id
                    });
                let entity = cx.entity();

                div()
                    .on_children_prepainted({
                        let entity = entity.clone();
                        move |bounds, _, cx| {
                            let Some(first) = bounds.first().copied() else {
                                return;
                            };
                            let combined = bounds
                                .iter()
                                .copied()
                                .skip(1)
                                .fold(first, |combined, bounds| combined.union(&bounds));
                            let _ = entity.update(cx, |this, _| {
                                this.pane_bounds.insert(pane_id, combined);
                            });
                        }
                    })
                    .id(("explorer-pane", pane_id.0))
                    .debug_selector(move || format!("explorer-pane-{}", pane_id.0))
                    .relative()
                    .flex()
                    .flex_1()
                    .size_full()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .bg(rgb(0xffffff))
                    .capture_any_mouse_down({
                        let entity = entity.clone();
                        move |event, window, cx| {
                            if matches!(event.button, MouseButton::Left | MouseButton::Right) {
                                let _ = entity.update(cx, |this, cx| {
                                    this.activate_pane(workspace_tab, pane_id, window, cx);
                                    cx.notify();
                                });
                            }
                        }
                    })
                    .on_drag_move::<TabDrag>({
                        let entity = entity.clone();
                        move |event: &DragMoveEvent<TabDrag>, _, cx| {
                            let dragged = event.drag(cx).clone();
                            let _ = entity.update(cx, |this, cx| {
                                if this.update_dock_target(
                                    workspace_tab,
                                    pane_id,
                                    &dragged,
                                    event.bounds,
                                    event.event.position,
                                ) {
                                    cx.notify();
                                }
                            });
                        }
                    })
                    .on_drop(cx.listener(move |this, dragged: &TabDrag, window, cx| {
                        let target = this.dock_target;
                        if dragged.dockable
                            && target.is_some_and(|target| {
                                target.workspace_tab == workspace_tab && target.pane == pane_id
                            })
                        {
                            let target = target.expect("checked dock target");
                            if dragged.id == workspace_tab {
                                this.self_dock_active_tab(
                                    workspace_tab,
                                    pane_id,
                                    target.direction,
                                    window,
                                    cx,
                                );
                            } else {
                                this.split_tab_into_pane(
                                    dragged.id,
                                    workspace_tab,
                                    pane_id,
                                    target.direction,
                                    window,
                                    cx,
                                );
                            }
                        }
                        this.clear_tab_drag();
                        this.dock_target = None;
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .when(is_active, |this| {
                        this.border_1().border_color(rgb(SPLIT_FOCUS_BLUE))
                    })
                    .child(view)
                    .when_some(dock_target, |this, target| {
                        this.child(render_split_drop_preview(target.direction))
                    })
                    .into_any_element()
            }
            PaneNode::Split {
                id,
                axis,
                ratio,
                first,
                second,
            } => {
                let split_id = *id;
                let axis = *axis;
                let ratio = *ratio;
                let entity = cx.entity();
                let first = self.render_pane_node(workspace_tab, active_pane, first, window, cx);
                let second = self.render_pane_node(workspace_tab, active_pane, second, window, cx);
                let divider = self.render_split_divider(workspace_tab, split_id, axis, cx);

                div()
                    .on_children_prepainted(move |bounds, _, cx| {
                        let Some(first) = bounds.first().copied() else {
                            return;
                        };
                        let combined = bounds
                            .iter()
                            .copied()
                            .skip(1)
                            .fold(first, |combined, bounds| combined.union(&bounds));
                        let _ = entity.update(cx, |this, _| {
                            this.split_bounds.insert(split_id, combined);
                        });
                    })
                    .id(("explorer-split", split_id))
                    .relative()
                    .flex()
                    .when(axis == SplitAxis::Horizontal, |this| this.flex_row())
                    .when(axis == SplitAxis::Vertical, |this| this.flex_col())
                    .size_full()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .flex_shrink_0()
                            .when(axis == SplitAxis::Horizontal, |this| {
                                this.w(relative(ratio)).h_full()
                            })
                            .when(axis == SplitAxis::Vertical, |this| {
                                this.h(relative(ratio)).w_full()
                            })
                            .child(first),
                    )
                    .child(divider)
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .child(second),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_split_divider(
        &self,
        workspace_tab: TabId,
        split_id: u64,
        axis: SplitAxis,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entity = cx.entity();
        let hit_offset = -(SPLIT_DIVIDER_HIT_SIZE - SPLIT_DIVIDER_SIZE) / 2.0;
        div()
            .id(("explorer-split-divider", split_id))
            .relative()
            .flex_none()
            .bg(rgb(SPLIT_BORDER))
            .when(axis == SplitAxis::Horizontal, |this| {
                this.w(px(SPLIT_DIVIDER_SIZE)).h_full()
            })
            .when(axis == SplitAxis::Vertical, |this| {
                this.h(px(SPLIT_DIVIDER_SIZE)).w_full()
            })
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        window.on_mouse_event({
                            let entity = entity.clone();
                            move |event: &MouseDownEvent, _, _, cx| {
                                if event.button != MouseButton::Left
                                    || !bounds.contains(&event.position)
                                {
                                    return;
                                }
                                let _ = entity.update(cx, |this, cx| {
                                    this.begin_split_resize(
                                        workspace_tab,
                                        split_id,
                                        event.position,
                                    );
                                    cx.stop_propagation();
                                });
                            }
                        });
                        window.on_mouse_event({
                            let entity = entity.clone();
                            move |event: &MouseMoveEvent, _, _, cx| {
                                if event.pressed_button != Some(MouseButton::Left) {
                                    return;
                                }
                                let _ = entity.update(cx, |this, cx| {
                                    if this.update_split_resize(event.position) {
                                        cx.notify();
                                    }
                                });
                            }
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, _, _, cx| {
                            if event.button == MouseButton::Left {
                                let _ = entity.update(cx, |this, _| {
                                    this.split_resize_drag = None;
                                });
                            }
                        });
                    },
                )
                .absolute()
                .when(axis == SplitAxis::Horizontal, |this| {
                    this.left(px(hit_offset))
                        .top_0()
                        .w(px(SPLIT_DIVIDER_HIT_SIZE))
                        .h_full()
                        .cursor(CursorStyle::ResizeColumn)
                })
                .when(axis == SplitAxis::Vertical, |this| {
                    this.top(px(hit_offset))
                        .left_0()
                        .h(px(SPLIT_DIVIDER_HIT_SIZE))
                        .w_full()
                        .cursor(CursorStyle::ResizeRow)
                }),
            )
            .into_any_element()
    }

    fn render_tab_context_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.tab_context_menu?;
        let tab = self.tabs.iter().find(|tab| tab.id == menu.tab)?;
        let is_split = tab.is_split();
        let close_enabled = self.tabs.len() > 1 || is_split;
        Some(
            div()
                .absolute()
                .inset_0()
                .child(div().absolute().inset_0().on_any_mouse_down(cx.listener(
                    |this, _, _, cx| {
                        this.tab_context_menu = None;
                        cx.notify();
                    },
                )))
                .child(
                    div()
                        .id("explorer-tab-context-menu")
                        .absolute()
                        .left(menu.position.x)
                        .top(menu.position.y)
                        .w(px(168.0))
                        .p(px(4.0))
                        .rounded(px(4.0))
                        .bg(rgb(0xffffff))
                        .border_1()
                        .border_color(rgb(0xd8d8d8))
                        .shadow_md()
                        .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                        .children(is_split.then(|| {
                            tab_context_menu_row(
                                "tab-context-unsplit",
                                "Unsplit",
                                true,
                                cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    this.unsplit_tab(menu.tab, window, cx);
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            )
                        }))
                        .child(tab_context_menu_row(
                            "tab-context-close",
                            "Close",
                            close_enabled,
                            cx.listener(move |this, _: &ClickEvent, window, cx| {
                                if close_enabled {
                                    this.close_tab(menu.tab, window, cx);
                                }
                                this.tab_context_menu = None;
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )),
                )
                .into_any_element(),
        )
    }
}

fn split_direction_for_position(
    bounds: Bounds<Pixels>,
    position: Point<Pixels>,
) -> Option<SplitDirection> {
    if !bounds.contains(&position) {
        return None;
    }
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let x = f32::from(position.x - bounds.origin.x);
    let y = f32::from(position.y - bounds.origin.y);
    let horizontal_threshold = width / 3.0;
    let vertical_threshold = height / 3.0;
    let mut candidates = Vec::with_capacity(4);
    if width >= PANE_MIN_WIDTH * 2.0 + SPLIT_DIVIDER_SIZE {
        if x <= horizontal_threshold {
            candidates.push((x, SplitDirection::Left));
        }
        if width - x <= horizontal_threshold {
            candidates.push((width - x, SplitDirection::Right));
        }
    }
    if height >= PANE_MIN_HEIGHT * 2.0 + SPLIT_DIVIDER_SIZE {
        if y <= vertical_threshold {
            candidates.push((y, SplitDirection::Up));
        }
        if height - y <= vertical_threshold {
            candidates.push((height - y, SplitDirection::Down));
        }
    }
    candidates
        .into_iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, direction)| direction)
}

fn pane_bounds_allow_split(bounds: Bounds<Pixels>, direction: SplitDirection) -> bool {
    match direction.axis() {
        SplitAxis::Horizontal => {
            f32::from(bounds.size.width) >= PANE_MIN_WIDTH * 2.0 + SPLIT_DIVIDER_SIZE
        }
        SplitAxis::Vertical => {
            f32::from(bounds.size.height) >= PANE_MIN_HEIGHT * 2.0 + SPLIT_DIVIDER_SIZE
        }
    }
}

fn pane_focus_outline_visible(
    pane_is_active: bool,
    workspace_is_split: bool,
    highlight_focused: bool,
) -> bool {
    pane_is_active && workspace_is_split && highlight_focused
}

fn render_split_drop_preview(direction: SplitDirection) -> AnyElement {
    div()
        .absolute()
        .when(direction == SplitDirection::Left, |this| {
            this.left_0().top_0().bottom_0().w(relative(0.5))
        })
        .when(direction == SplitDirection::Right, |this| {
            this.right_0().top_0().bottom_0().w(relative(0.5))
        })
        .when(direction == SplitDirection::Up, |this| {
            this.left_0().right_0().top_0().h(relative(0.5))
        })
        .when(direction == SplitDirection::Down, |this| {
            this.left_0().right_0().bottom_0().h(relative(0.5))
        })
        .bg(rgba(0x0078d429))
        .border_1()
        .border_color(rgba(0x0078d4b8))
        .into_any_element()
}

fn tab_context_menu_row(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(30.0))
        .px(px(10.0))
        .rounded(px(3.0))
        .text_size(px(12.0))
        .text_color(rgb(if enabled { 0x1f1f1f } else { 0x9a9a9a }))
        .when(enabled, |this| {
            this.hover(|style| style.bg(rgb(0xf0f0f0)))
                .on_click(on_click)
        })
        .child(label)
        .into_any_element()
}

impl Render for ExplorerTabs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.cleanup_completed_background_operations(cx);
        let app_font = crate::settings::current_app_font(cx);
        let active_workspace = self.active_tab().cloned();
        let split_key_context = active_workspace
            .as_ref()
            .is_some_and(ExplorerTab::is_split)
            .then_some("ExplorerTabs split = true")
            .unwrap_or("ExplorerTabs");
        let active_view = active_workspace.as_ref().map(ExplorerTab::active_view);
        let drop_exit_view = active_view.clone();
        let input_mouse_down_view = active_view.clone();
        let active_drop_indicator = active_workspace.as_ref().and_then(|tab| {
            tab.panes
                .iter()
                .find_map(|pane| pane.view.read(cx).active_drop_indicator())
        });
        let shared_chrome = active_view.as_ref().map(|view| {
            view.update(cx, |view, view_cx| {
                view.render_shared_chrome(window, view_cx)
            })
        });
        let (navbar, utility_bar, sidebar, chrome_overlays) = match shared_chrome {
            Some(chrome) => (
                Some(chrome.navbar),
                Some(chrome.utility_bar),
                chrome.sidebar,
                chrome.overlays,
            ),
            None => (None, None, None, Vec::new()),
        };
        let active_layout = active_workspace
            .as_ref()
            .map(|tab| self.render_active_layout(tab, window, cx));
        let tab_context_menu = self.render_tab_context_menu(cx);

        let mut content = div()
            .font(app_font.clone())
            .key_context(split_key_context)
            .on_action(cx.listener(Self::handle_new_tab))
            .on_action(cx.listener(Self::handle_new_window))
            .on_action(cx.listener(Self::handle_close_tab))
            .on_action(cx.listener(Self::handle_select_next_tab))
            .on_action(cx.listener(Self::handle_select_previous_tab))
            .on_action(cx.listener(Self::handle_select_tab_by_index))
            .on_action(cx.listener(Self::handle_split_pane_left))
            .on_action(cx.listener(Self::handle_split_pane_right))
            .on_action(cx.listener(Self::handle_split_pane_up))
            .on_action(cx.listener(Self::handle_split_pane_down))
            .on_action(cx.listener(Self::handle_focus_pane_left))
            .on_action(cx.listener(Self::handle_focus_pane_right))
            .on_action(cx.listener(Self::handle_focus_pane_up))
            .on_action(cx.listener(Self::handle_focus_pane_down))
            .on_action(cx.listener(Self::handle_move_pane_left))
            .on_action(cx.listener(Self::handle_move_pane_right))
            .on_action(cx.listener(Self::handle_move_pane_up))
            .on_action(cx.listener(Self::handle_move_pane_down))
            .capture_any_mouse_down(move |event, window, cx| {
                if event.button == MouseButton::Left
                    && input_mouse_down_view
                        .as_ref()
                        .is_some_and(|view| view.read(cx).has_active_text_input())
                {
                    window.prevent_default();
                }
            })
            .on_file_drop(move |event, _, cx| {
                if let FileDropEvent::Exited = event {
                    if let Some(active_view) = &drop_exit_view {
                        active_view.update(cx, |view, cx| {
                            if view.clear_drop_indicator() {
                                cx.notify();
                            }
                        });
                    }
                }
            })
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(0xffffff))
            .child(self.render_tab_bar(window, cx))
            .when_some(navbar, |this, navbar| this.child(navbar))
            .when_some(utility_bar, |this, utility_bar| this.child(utility_bar))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_hidden()
                    .when_some(sidebar, |this, sidebar| this.child(sidebar))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .h_full()
                            .overflow_hidden()
                            .when_some(active_layout, |this, layout| this.child(layout)),
                    ),
            );

        content = content.children(chrome_overlays);

        let content = content
            .when_some(tab_context_menu, |this, menu| this.child(menu))
            .when_some(active_drop_indicator, |this, indicator| {
                this.child(render_drop_indicator(indicator, &app_font, window))
            })
            .into_any_element();

        render_platform_window_frame(content, window)
    }
}

impl Render for TabDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        tab_preview_visual(
            self.label.clone(),
            &self.path,
            self.is_active,
            self.pane_count,
            self.font.clone(),
        )
    }
}

fn tab_preview_visual(
    label: SharedString,
    path: &Path,
    is_active: bool,
    pane_count: usize,
    font: gpui::Font,
) -> impl IntoElement {
    div()
        .font(font)
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .h(px(TAB_BAR_HEIGHT))
        .w(px(TAB_WIDTH))
        .px(px(TAB_HORIZONTAL_PADDING))
        .gap(px(TAB_ICON_GAP))
        .overflow_hidden()
        .bg(if is_active {
            rgb(TAB_ACTIVE_BG)
        } else {
            rgb(TAB_INACTIVE_BG)
        })
        .border_1()
        .border_color(rgb(TAB_BORDER))
        .shadow_md()
        .child(tab_inner_contents(
            label,
            Some(path),
            None,
            pane_count,
            Some(close_tab_glyph_visual().into_any_element()),
        ))
}

fn tab_inner_contents(
    label: SharedString,
    path: Option<&Path>,
    sidebar_group: Option<SidebarGroupKind>,
    pane_count: usize,
    close_glyph: Option<AnyElement>,
) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_w(px(0.0))
        .gap(px(TAB_ICON_GAP))
        .overflow_hidden()
        .child(tab_icon(path, sidebar_group))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(TAB_TEXT_SIZE))
                .text_color(rgb(TAB_TEXT_COLOR))
                .child(label),
        )
        .when(pane_count > 1, |this| {
            this.child(
                div()
                    .debug_selector(move || format!("explorer-tab-split-count-{pane_count}"))
                    .flex()
                    .items_center()
                    .justify_center()
                    .min_w(px(20.0))
                    .h(px(18.0))
                    .px(px(5.0))
                    .rounded(px(3.0))
                    .bg(rgb(0xe5f3ff))
                    .text_size(px(10.0))
                    .text_color(rgb(0x005a9e))
                    .child(format!("{pane_count}")),
            )
        })
        .when_some(close_glyph, |this, close_glyph| this.child(close_glyph))
        .into_any_element()
}

fn tab_icon(path: Option<&Path>, sidebar_group: Option<SidebarGroupKind>) -> AnyElement {
    if let Some(group) = sidebar_group {
        return match group {
            SidebarGroupKind::Pinned => pinned_group_icon(),
            SidebarGroupKind::Drives => drives_group_icon(),
            SidebarGroupKind::Network => network_group_icon(),
            SidebarGroupKind::Wsl => drive_wsl_icon(),
        };
    }

    let Some(path) = path else {
        return folder_icon().into_any_element();
    };

    if let Some(kind) = crate::explorer::resolve_directory_kind(path) {
        if kind == crate::explorer::DirectoryKind::DriveWsl {
            return crate::explorer::icons::drive_wsl_icon_for_path(path);
        }
        if kind == crate::explorer::DirectoryKind::Drive
            && crate::explorer::filesystem::drive_root_is_ejectable(path)
        {
            return crate::explorer::icons::drive_disc_icon_for_path(path);
        }

        return crate::explorer::icons::directory_kind_icon(kind);
    }

    folder_icon().into_any_element()
}

fn close_tab_glyph_visual() -> gpui::Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(TAB_CLOSE_SIZE))
        .h(px(TAB_CLOSE_SIZE))
        .flex_shrink_0()
        .rounded(px(3.0))
        .font(tab_icon_font())
        .text_size(px(TAB_ICON_TEXT_SIZE))
        .text_color(rgb(0x404040))
        .child(CLOSE_GLYPH)
}

fn observe_tab_view(view: &Entity<ExplorerView>, window: &Window, cx: &mut Context<ExplorerTabs>) {
    cx.observe(view, |this, _, cx| {
        this.cleanup_completed_background_operations(cx);
        cx.notify();
    })
    .detach();

    cx.subscribe_in(
        view,
        window,
        |this, source_view, event, window, cx| match event {
            ExplorerViewEvent::FilesystemChanged => {
                this.reload_tabs_except(source_view, cx);
                cx.notify();
            }
            ExplorerViewEvent::MountedVolumeEjected(path) => {
                if this.redirect_tabs_after_mounted_volume_ejected(path, cx) {
                    cx.notify();
                }
            }
            ExplorerViewEvent::OpenDirectoryInNewTab(path) => {
                this.add_configured_tab(path.clone(), window, cx);
                cx.notify();
            }
        },
    )
    .detach();
}

fn observe_settings(cx: &mut Context<ExplorerTabs>) {
    cx.observe_global::<SettingsState>(|this, cx| this.apply_settings_to_all_tabs(cx))
        .detach();
}

fn observe_window_activation(window: &mut Window, cx: &mut Context<ExplorerTabs>) {
    cx.observe_window_activation(window, |this, window, cx| {
        if window.is_window_active() {
            if let Some(tab) = this.active_tab() {
                tab.view
                    .read(cx)
                    .restore_focus_after_window_activation(window);
            }
            crate::explorer::clipboard::refresh_clipboard_summary(cx);
        }
    })
    .detach();
}

fn close_tab_button(tab_id: TabId, cx: &mut Context<ExplorerTabs>) -> AnyElement {
    div()
        .id(("explorer-tab-close", tab_id.0))
        .flex()
        .items_center()
        .justify_center()
        .w(px(TAB_CLOSE_SIZE))
        .h(px(TAB_CLOSE_SIZE))
        .flex_shrink_0()
        .rounded(px(3.0))
        .font(tab_icon_font())
        .text_size(px(TAB_ICON_TEXT_SIZE))
        .text_color(rgb(0x404040))
        .hover(|style| style.bg(rgb(NAV_BUTTON_HOVER_BG)))
        .active(|style| style.opacity(NAV_BUTTON_ACTIVE_OPACITY))
        .child(CLOSE_GLYPH)
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.close_tab(tab_id, window, cx);
            cx.stop_propagation();
            cx.notify();
        }))
        .into_any_element()
}

fn new_tab_button(cx: &mut Context<ExplorerTabs>) -> AnyElement {
    div()
        .id("explorer-new-tab")
        .flex()
        .items_center()
        .justify_center()
        .w(px(TAB_BAR_HEIGHT))
        .h_full()
        .flex_shrink_0()
        .block_mouse_except_scroll()
        .font(tab_icon_font())
        .text_size(px(13.0))
        .text_color(rgb(0x404040))
        .hover(|style| style.bg(rgb(NAV_BUTTON_HOVER_BG)))
        .active(|style| style.opacity(NAV_BUTTON_ACTIVE_OPACITY))
        .child(NEW_TAB_GLYPH)
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.add_new_tab(window, cx);
            cx.stop_propagation();
            cx.notify();
        }))
        .into_any_element()
}

fn tab_icon_font() -> gpui::Font {
    font("Segoe Fluent Icons")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TabDirection {
    Next,
    Previous,
}

fn adjacent_tab_index(active_index: usize, len: usize, direction: TabDirection) -> usize {
    debug_assert!(len > 0);
    match direction {
        TabDirection::Next => (active_index + 1) % len,
        TabDirection::Previous => active_index.checked_sub(1).unwrap_or(len - 1),
    }
}

fn selectable_tab_id_by_index(
    tabs: &[ExplorerTab],
    active_tab: TabId,
    index: usize,
) -> Option<TabId> {
    if tabs.len() <= 1 {
        return None;
    }

    let target_id = tabs.get(index)?.id;
    (target_id != active_tab).then_some(target_id)
}

#[cfg(test)]
fn selectable_tab_id_by_index_from_ids(
    tab_ids: &[TabId],
    active_tab: TabId,
    index: usize,
) -> Option<TabId> {
    if tab_ids.len() <= 1 {
        return None;
    }

    let target_id = *tab_ids.get(index)?;
    (target_id != active_tab).then_some(target_id)
}

#[cfg(test)]
fn can_close_tab(tab_count: usize) -> bool {
    tab_count > 1
}

fn can_drag_tab(tab_count: usize, is_split: bool) -> bool {
    tab_count > 1 || (tab_count == 1 && !is_split)
}

fn tab_strip_width(tab_count: usize) -> f32 {
    (tab_count as f32 * TAB_WIDTH) + TAB_BAR_HEIGHT
}

fn tab_can_activate_for_file_drag_hover(
    active_tab: TabId,
    target_tab: TabId,
    tabs: &[ExplorerTab],
) -> bool {
    target_tab != active_tab && tabs.iter().any(|tab| tab.id == target_tab)
}

#[cfg(test)]
fn activate_tab_id_for_file_drag_hover(
    active_tab: TabId,
    target_tab: TabId,
    tabs: &[TabId],
) -> Option<TabId> {
    (target_tab != active_tab && tabs.contains(&target_tab)).then_some(target_tab)
}

fn start_dragging_tab(dragging_tab: &mut Option<TabId>, id: TabId) {
    *dragging_tab = Some(id);
}

fn clear_dragging_tab(dragging_tab: &mut Option<TabId>) -> bool {
    dragging_tab.take().is_some()
}

fn tab_reorder_hit_test(
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    cursor_x: f32,
    cursor_y: f32,
) -> bool {
    let right = left + width;
    let bottom = top + height;

    cursor_x >= left
        && cursor_x <= right
        && cursor_y >= top - TAB_REORDER_VERTICAL_TOLERANCE
        && cursor_y <= bottom + TAB_REORDER_VERTICAL_TOLERANCE
}

fn active_id_after_close_from_removed(tabs: &[ExplorerTab], removed_index: usize) -> Option<TabId> {
    active_id_after_close_from_removed_ids(
        &tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
        removed_index,
    )
}

fn active_id_after_close_from_removed_ids(tabs: &[TabId], removed_index: usize) -> Option<TabId> {
    let next_index = removed_index.min(tabs.len().checked_sub(1)?);
    Some(tabs[next_index])
}

fn reorder_tabs(
    tabs: &mut Vec<ExplorerTab>,
    dragged_id: TabId,
    target_id: TabId,
    before: bool,
) -> bool {
    if dragged_id == target_id {
        return false;
    }

    let Some(dragged_index) = tabs.iter().position(|tab| tab.id == dragged_id) else {
        return false;
    };
    let Some(target_index) = tabs.iter().position(|tab| tab.id == target_id) else {
        return false;
    };

    let insert_index = tab_reorder_insert_index(dragged_index, target_index, before);
    let dragged = tabs.remove(dragged_index);
    tabs.insert(insert_index, dragged);
    true
}

fn tab_reorder_insert_index(dragged_index: usize, mut target_index: usize, before: bool) -> usize {
    if dragged_index < target_index {
        target_index -= 1;
    }

    if before {
        target_index
    } else {
        target_index + 1
    }
}

#[cfg(test)]
fn reorder_tab_ids(
    tabs: &mut Vec<TabId>,
    dragged_id: TabId,
    target_id: TabId,
    before: bool,
) -> bool {
    if dragged_id == target_id {
        return false;
    }

    let Some(dragged_index) = tabs.iter().position(|id| *id == dragged_id) else {
        return false;
    };
    let Some(target_index) = tabs.iter().position(|id| *id == target_id) else {
        return false;
    };

    let insert_index = tab_reorder_insert_index(dragged_index, target_index, before);
    let dragged = tabs.remove(dragged_index);
    tabs.insert(insert_index, dragged);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::{
        actions::{
            CreateNewFile, CreateNewFolder, EnterSelectedInNewTab, MoveDown, OpenSelectedInNewTab,
            PasteClipboard, RecursiveSearchEdit, RenameCancel, RenameCommit, RenameSelected,
            SearchCommit, SearchEdit,
        },
        address_bar::folder_suggestions_for_input,
        clipboard::{FileClipboard, FileClipboardOperation, file_clipboard_from_item},
        test_support::{TempDir, selected_names},
        view::{PendingPermanentDelete, PendingTrash, tab_label_for_path},
    };
    use crate::settings::{ExplorerSettings, SettingsState};
    use git2::Repository;
    use gpui::{
        AppContext, ClipboardItem, Image, ImageFormat, Modifiers, MouseButton, MouseDownEvent,
        MouseUpEvent, ScrollDelta, ScrollWheelEvent, TestAppContext,
    };
    use std::{fs, io::Write};

    #[test]
    fn tab_icon_font_remains_dedicated() {
        assert_eq!(tab_icon_font().family, "Segoe Fluent Icons");
    }

    fn test_tabs_with_files<'a>(
        cx: &'a mut TestAppContext,
        names: &[&str],
    ) -> (
        TempDir,
        Entity<ExplorerTabs>,
        &'a mut gpui::VisualTestContext,
    ) {
        let temp = TempDir::new();
        for name in names {
            fs::write(temp.path().join(name), b"file").expect("write test file");
        }
        let path = temp.path().to_path_buf();
        let (tabs, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerTabs::new_for_test(path, focus_handle, cx)
        });
        (temp, tabs, cx)
    }

    fn test_tabs_with_two_files<'a>(
        cx: &'a mut TestAppContext,
    ) -> (
        TempDir,
        Entity<ExplorerTabs>,
        &'a mut gpui::VisualTestContext,
    ) {
        test_tabs_with_files(cx, &["a.txt", "b.txt"])
    }

    fn test_tabs_at_path<'a>(
        cx: &'a mut TestAppContext,
        path: PathBuf,
    ) -> (Entity<ExplorerTabs>, &'a mut gpui::VisualTestContext) {
        cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerTabs::new_for_test(path, focus_handle, cx)
        })
    }

    fn test_tabs_with_directories<'a>(
        cx: &'a mut TestAppContext,
        names: &[&str],
    ) -> (
        TempDir,
        Entity<ExplorerTabs>,
        &'a mut gpui::VisualTestContext,
    ) {
        let temp = TempDir::new();
        for name in names {
            fs::create_dir(temp.path().join(name)).expect("create test directory");
        }
        let path = temp.path().to_path_buf();
        let (tabs, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerTabs::new_for_test(path, focus_handle, cx)
        });
        (temp, tabs, cx)
    }

    fn test_tabs_with_directories_and_files<'a>(
        cx: &'a mut TestAppContext,
        directory_names: &[&str],
        file_names: &[&str],
    ) -> (
        TempDir,
        Entity<ExplorerTabs>,
        &'a mut gpui::VisualTestContext,
    ) {
        let temp = TempDir::new();
        for name in directory_names {
            fs::create_dir(temp.path().join(name)).expect("create test directory");
        }
        for name in file_names {
            fs::write(temp.path().join(name), b"file").expect("write test file");
        }
        let path = temp.path().to_path_buf();
        let (tabs, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerTabs::new_for_test(path, focus_handle, cx)
        });
        (temp, tabs, cx)
    }

    fn create_zip_archive(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).expect("create zip archive");
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::FileOptions::default();
        for (name, contents) in entries {
            writer.start_file(*name, options).expect("start zip file");
            writer.write_all(contents).expect("write zip file");
        }
        writer.finish().expect("finish zip archive");
    }

    fn active_test_view(
        tabs: &Entity<ExplorerTabs>,
        cx: &gpui::VisualTestContext,
    ) -> Entity<ExplorerView> {
        cx.read_entity(tabs, |tabs, _| tabs.active_tab().unwrap().view.clone())
    }

    fn observe_active_test_view(
        tabs: &Entity<ExplorerTabs>,
        cx: &mut gpui::VisualTestContext,
    ) -> Entity<ExplorerView> {
        let view = active_test_view(tabs, cx);
        cx.update(|window, app| {
            tabs.update(app, |_, cx| observe_tab_view(&view, window, cx));
        });
        view
    }

    fn assert_active_tab_focused(tabs: &Entity<ExplorerTabs>, cx: &mut gpui::VisualTestContext) {
        cx.update(|window, app| {
            let active_view = tabs.read(app).active_tab().unwrap().view.clone();
            assert!(active_view.read(app).focus_handle(app).is_focused(window));
        });
    }

    fn click_selector(cx: &mut gpui::VisualTestContext, selector: &'static str) {
        let bounds = cx.debug_bounds(selector).expect("element bounds");
        cx.simulate_click(bounds.center(), Modifiers::default());
    }

    fn left_click_position(
        cx: &mut gpui::VisualTestContext,
        position: gpui::Point<gpui::Pixels>,
        click_count: usize,
        modifiers: Modifiers,
    ) {
        cx.simulate_event(MouseDownEvent {
            position,
            modifiers,
            button: MouseButton::Left,
            click_count,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            position,
            modifiers,
            button: MouseButton::Left,
            click_count,
        });
    }

    fn right_click_selector(cx: &mut gpui::VisualTestContext, selector: &'static str) {
        let bounds = cx.debug_bounds(selector).expect("element bounds");
        right_click_position(cx, bounds.center());
    }

    fn right_click_entry_name(cx: &mut gpui::VisualTestContext, selector: &'static str) {
        let position = entry_name_hit_position(cx, selector);
        right_click_position(cx, position);
    }

    fn right_click_entry_other_column(cx: &mut gpui::VisualTestContext, selector: &'static str) {
        let position = entry_other_column_position(cx, selector);
        right_click_position(cx, position);
    }

    fn right_click_position(cx: &mut gpui::VisualTestContext, position: gpui::Point<gpui::Pixels>) {
        cx.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
        cx.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    }

    fn entry_name_position(
        cx: &mut gpui::VisualTestContext,
        selector: &'static str,
    ) -> gpui::Point<gpui::Pixels> {
        let bounds = cx.debug_bounds(selector).expect("entry bounds");
        gpui::point(bounds.left() + gpui::px(10.0), bounds.center().y)
    }

    fn entry_name_hit_position(
        cx: &mut gpui::VisualTestContext,
        selector: &'static str,
    ) -> gpui::Point<gpui::Pixels> {
        let bounds = cx.debug_bounds(selector).expect("entry bounds");
        gpui::point(bounds.left() + gpui::px(24.0), bounds.center().y)
    }

    fn entry_other_column_position(
        cx: &mut gpui::VisualTestContext,
        selector: &'static str,
    ) -> gpui::Point<gpui::Pixels> {
        let bounds = cx.debug_bounds(selector).expect("entry bounds");
        gpui::point(bounds.right() - gpui::px(10.0), bounds.center().y)
    }

    fn click_second_entry(cx: &mut gpui::VisualTestContext) {
        click_selector(cx, "explorer-entry-name-hit-1");
    }

    #[gpui::test]
    fn render_drop_indicator_shows_copy_to_overlay_once(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_files(cx, &["file.txt"]);
        let view = active_test_view(&tabs, cx);
        let source = temp.path().join("file.txt");
        let mouse_position = gpui::point(gpui::px(96.0), gpui::px(120.0));

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.select_single_path(&source);
                let ix = view
                    .entries
                    .iter()
                    .position(|entry| entry.path == source)
                    .expect("source entry");
                let dragged = view
                    .test_dragged_entries_for_index(ix)
                    .expect("dragged row");
                view.active_drop_indicator = view.drop_indicator_for_value(
                    &dragged,
                    &DropDestination::CurrentDirectory,
                    Modifiers::secondary_key(),
                    mouse_position,
                );
                assert!(view.active_drop_indicator.is_some());
                cx.notify();
            });
        });
        cx.run_until_parked();

        let indicator_bounds = cx
            .debug_bounds("drop-indicator")
            .expect("drop indicator bounds");
        assert!(indicator_bounds.origin.y > mouse_position.y);
    }

    #[gpui::test]
    fn settings_changes_apply_to_existing_and_future_tabs(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (tabs, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerTabs::new(path, focus_handle, window, cx)
        });

        cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                tabs.add_background_tab(temp.path().to_path_buf(), window, cx);
            });
        });
        cx.update_global::<SettingsState, _>(|state, _| {
            state.value.view.date_format = "%d %B %Y".to_owned();
            state.value.view.show_hidden = true;
            state.value.view.show_extensions = false;
            state.value.view.show_folder_sizes = true;
            state.value.view.font = "Inter".to_owned();
            state.value.view.search_mode = crate::settings::SearchMode::Compact;
            state.value.tabs.highlight_focused = true;
        });
        cx.run_until_parked();

        cx.update(|_, app| {
            assert!(app.global::<SettingsState>().value.tabs.highlight_focused);
            assert!(pane_focus_outline_visible(
                true,
                true,
                app.global::<SettingsState>().value.tabs.highlight_focused
            ));
        });

        let existing_views = cx.read_entity(&tabs, |tabs, _| {
            tabs.tabs
                .iter()
                .map(|tab| tab.view.clone())
                .collect::<Vec<_>>()
        });
        for view in existing_views {
            cx.read_entity(&view, |view, _| {
                assert!(view.show_hidden_files);
                assert!(!view.show_file_name_extensions);
                assert!(view.show_folder_size);
                assert_eq!(view.date_format, "%d %B %Y");
                assert_eq!(view.font.family, "Inter");
                assert_eq!(view.search_mode, crate::settings::SearchMode::Compact);
            });
        }

        cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                tabs.add_background_tab(temp.path().to_path_buf(), window, cx);
            });
        });
        let future_view = cx.read_entity(&tabs, |tabs, _| tabs.tabs.last().unwrap().view.clone());
        cx.read_entity(&future_view, |view, _| {
            assert!(view.show_hidden_files);
            assert!(!view.show_file_name_extensions);
            assert!(view.show_folder_size);
            assert_eq!(view.date_format, "%d %B %Y");
            assert_eq!(view.font.family, "Inter");
            assert_eq!(view.search_mode, crate::settings::SearchMode::Compact);
        });
    }

    #[gpui::test]
    fn window_reactivation_restores_rename_and_paste_keyboard_target(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let temp = TempDir::new();
        let selected = temp.path().join("selected.txt");
        fs::write(&selected, b"selected").expect("create selected file");
        let path = temp.path().to_path_buf();
        let (tabs, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerTabs::new(path, focus_handle, window, cx)
        });
        let view = active_test_view(&tabs, cx);

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.select_single_path(&selected);
                cx.notify();
            });
        });
        cx.run_until_parked();

        cx.deactivate_window();
        cx.update(|window, _| window.blur());
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();

        assert_active_tab_focused(&tabs, cx);
        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["selected.txt"]);
        });

        cx.dispatch_action(RenameSelected);
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(view.rename_is_active_for_path(&selected));
        });
        cx.dispatch_action(RenameCancel);
        cx.run_until_parked();

        cx.update(|_, app| {
            app.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
                ImageFormat::Png,
                vec![1, 2, 3, 4],
            )));
        });
        cx.deactivate_window();
        cx.update(|window, _| window.blur());
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();

        assert_active_tab_focused(&tabs, cx);
        cx.dispatch_action(PasteClipboard);
        cx.run_until_parked();
        assert_eq!(
            fs::read(temp.path().join("image.png")).unwrap(),
            vec![1, 2, 3, 4]
        );
    }

    #[gpui::test]
    fn mounted_volume_ejected_event_redirects_all_affected_tabs(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let temp = TempDir::new();
        let outside = temp.path().join("outside");
        let ejected_root = temp.path().join("drive");
        let affected_one = ejected_root.join("one");
        let affected_two = ejected_root.clone();
        let history_one = temp.path().join("history-one");
        let history_two = temp.path().join("history-two");
        let ejected_history = ejected_root.join("old");
        fs::create_dir_all(&outside).expect("create outside tab path");
        fs::create_dir_all(&affected_one).expect("create affected tab path");
        fs::create_dir_all(&history_one).expect("create first history path");
        fs::create_dir_all(&history_two).expect("create second history path");
        fs::create_dir_all(&ejected_history).expect("create ejected history path");

        let (tabs, cx) = cx.add_window_view({
            let outside = outside.clone();
            move |window, cx| {
                let focus_handle = cx.focus_handle();
                focus_handle.focus(window);
                ExplorerTabs::new_for_test(outside, focus_handle, cx)
            }
        });
        let emitter = active_test_view(&tabs, cx);

        let affected_views = cx.update(|window, app| {
            let mut affected_views = Vec::new();
            tabs.update(app, |tabs, cx| {
                observe_tab_view(&emitter, window, cx);

                let focus_one = cx.focus_handle();
                let view_one_path = affected_one.clone();
                let view_one = cx.new(|_| {
                    ExplorerView::new_with_focus_handle_for_test(view_one_path, focus_one)
                });
                view_one.update(cx, |view, _| {
                    view.back_stack =
                        vec![ejected_history.clone().into(), history_one.clone().into()];
                    view.forward_stack = vec![ejected_root.join("forward-one").into()];
                });

                let focus_two = cx.focus_handle();
                let view_two_path = affected_two.clone();
                let view_two = cx.new(|_| {
                    ExplorerView::new_with_focus_handle_for_test(view_two_path, focus_two)
                });
                view_two.update(cx, |view, _| {
                    view.back_stack = vec![history_two.clone().into()];
                    view.forward_stack = vec![ejected_root.join("forward-two").into()];
                });

                tabs.tabs
                    .push(ExplorerTab::single(TabId(2), PaneId(2), view_one.clone()));
                tabs.tabs
                    .push(ExplorerTab::single(TabId(3), PaneId(3), view_two.clone()));
                tabs.next_tab_id = 4;
                tabs.next_pane_id = 4;
                affected_views.push(view_one);
                affected_views.push(view_two);
            });

            emitter.update(app, |_, cx| {
                cx.emit(ExplorerViewEvent::MountedVolumeEjected(
                    ejected_root.clone(),
                ));
            });
            affected_views
        });
        cx.run_until_parked();

        cx.read_entity(&emitter, |view, _| {
            assert_eq!(view.path, outside);
        });
        cx.read_entity(&affected_views[0], |view, _| {
            assert_eq!(view.path, history_one);
            assert!(view.back_stack.is_empty());
            assert!(view.forward_stack.iter().all(|location| match location {
                crate::explorer::view::NavigationLocation::Directory(path) => {
                    !path.starts_with(&ejected_root)
                }
                crate::explorer::view::NavigationLocation::SidebarGroup(_) => true,
            }));
        });
        cx.read_entity(&affected_views[1], |view, _| {
            assert_eq!(view.path, history_two);
            assert!(view.back_stack.is_empty());
            assert!(view.forward_stack.iter().all(|location| match location {
                crate::explorer::view::NavigationLocation::Directory(path) => {
                    !path.starts_with(&ejected_root)
                }
                crate::explorer::view::NavigationLocation::SidebarGroup(_) => true,
            }));
        });
    }

    #[gpui::test]
    fn explicit_new_tab_method_and_action_focus_with_default_settings(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (_temp, tabs, cx) = test_tabs_with_files(cx, &[]);

        cx.update(|window, app| {
            tabs.update(app, |tabs, cx| tabs.add_new_tab(window, cx));
        });
        cx.run_until_parked();

        cx.read_entity(&tabs, |tabs, _| {
            assert_eq!(tabs.tabs.len(), 2);
            assert_eq!(tabs.active_tab, tabs.tabs[1].id);
        });
        assert_active_tab_focused(&tabs, cx);

        cx.dispatch_action(NewTab);
        cx.run_until_parked();

        cx.read_entity(&tabs, |tabs, _| {
            assert_eq!(tabs.tabs.len(), 3);
            assert_eq!(tabs.active_tab, tabs.tabs[2].id);
        });
        assert_active_tab_focused(&tabs, cx);
    }

    #[gpui::test]
    fn open_directory_in_new_tab_stays_in_background_by_default(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a"]);
        let view = active_test_view(&tabs, cx);
        cx.update(|window, app| {
            tabs.update(app, |_, cx| observe_tab_view(&view, window, cx));
            view.update(app, |_, cx| {
                cx.emit(ExplorerViewEvent::OpenDirectoryInNewTab(
                    temp.path().join("a"),
                ));
            });
        });
        cx.run_until_parked();

        cx.read_entity(&tabs, |tabs, _| {
            assert_eq!(tabs.tabs.len(), 2);
            assert_eq!(tabs.active_tab, tabs.tabs[0].id);
        });
        assert_active_tab_focused(&tabs, cx);
    }

    #[gpui::test]
    fn middle_clicking_breadcrumb_opens_ancestor_in_new_background_tab(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let temp = TempDir::new();
        let parent = temp.path().join("parent");
        let current_path = parent.join("child");
        fs::create_dir_all(&current_path).expect("create nested test directory");
        let (tabs, cx) = test_tabs_at_path(cx, current_path.clone());
        let view = active_test_view(&tabs, cx);
        cx.update(|window, app| {
            tabs.update(app, |_, cx| observe_tab_view(&view, window, cx));
        });
        cx.run_until_parked();

        let parent_position = cx
            .debug_bounds("breadcrumb-segment-parent")
            .expect("ancestor breadcrumb bounds")
            .center();

        cx.simulate_mouse_down(parent_position, MouseButton::Middle, Modifiers::default());
        cx.simulate_mouse_up(parent_position, MouseButton::Middle, Modifiers::default());
        cx.run_until_parked();

        let new_tab_view = cx.read_entity(&tabs, |tabs, _| {
            assert_eq!(tabs.tabs.len(), 2);
            assert_eq!(tabs.active_tab, tabs.tabs[0].id);
            tabs.tabs[1].view.clone()
        });
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.path, current_path);
        });
        cx.read_entity(&new_tab_view, |view, _| {
            assert_eq!(view.path, parent);
        });
    }

    #[gpui::test]
    fn configured_new_tab_focus_activates_and_focuses_last_created_tab(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a", "b"]);
        let view = active_test_view(&tabs, cx);
        cx.update_global::<SettingsState, _>(|state, _| {
            state.value.tabs.focus_new = true;
        });
        cx.update(|window, app| {
            tabs.update(app, |_, cx| observe_tab_view(&view, window, cx));
            view.update(app, |_, cx| {
                cx.emit(ExplorerViewEvent::OpenDirectoryInNewTab(
                    temp.path().join("a"),
                ));
                cx.emit(ExplorerViewEvent::OpenDirectoryInNewTab(
                    temp.path().join("b"),
                ));
            });
        });
        cx.run_until_parked();

        let active_view = cx.read_entity(&tabs, |tabs, _| {
            assert_eq!(tabs.tabs.len(), 3);
            assert_eq!(tabs.active_tab, tabs.tabs[2].id);
            tabs.active_tab().unwrap().view.clone()
        });
        cx.read_entity(&active_view, |view, _| {
            assert_eq!(view.path, temp.path().join("b"));
        });
        assert_active_tab_focused(&tabs, cx);
    }

    #[gpui::test]
    fn ctrl_enter_folder_open_opens_directory_in_new_tab(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a"]);
        let view = active_test_view(&tabs, cx);
        let folder = temp.path().join("a");
        cx.update(|window, app| {
            tabs.update(app, |_, cx| observe_tab_view(&view, window, cx));
            view.update(app, |view, _| view.select_single_path(&folder));
        });

        cx.dispatch_action(EnterSelectedInNewTab);
        cx.run_until_parked();

        let new_tab_view = cx.read_entity(&tabs, |tabs, _| {
            assert_eq!(tabs.tabs.len(), 2);
            assert_eq!(tabs.active_tab, tabs.tabs[0].id);
            tabs.tabs[1].view.clone()
        });
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.path, temp.path());
        });
        cx.read_entity(&new_tab_view, |view, _| {
            assert_eq!(view.path, folder);
        });
    }

    #[gpui::test]
    fn ctrl_right_folder_open_opens_directory_in_new_tab(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a"]);
        let view = active_test_view(&tabs, cx);
        let folder = temp.path().join("a");
        cx.update(|window, app| {
            tabs.update(app, |_, cx| observe_tab_view(&view, window, cx));
            view.update(app, |view, _| view.select_single_path(&folder));
        });

        cx.dispatch_action(OpenSelectedInNewTab);
        cx.run_until_parked();

        let new_tab_view = cx.read_entity(&tabs, |tabs, _| {
            assert_eq!(tabs.tabs.len(), 2);
            assert_eq!(tabs.active_tab, tabs.tabs[0].id);
            tabs.tabs[1].view.clone()
        });
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.path, temp.path());
        });
        cx.read_entity(&new_tab_view, |view, _| {
            assert_eq!(view.path, folder);
        });
    }

    #[gpui::test]
    fn ctrl_double_click_folder_opens_directory_in_new_tab(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a"]);
        let view = active_test_view(&tabs, cx);
        cx.update(|window, app| {
            tabs.update(app, |_, cx| observe_tab_view(&view, window, cx));
        });
        let position = entry_other_column_position(cx, "explorer-entry-0");
        let ctrl = Modifiers {
            control: true,
            ..Modifiers::default()
        };

        left_click_position(cx, position, 1, ctrl);
        left_click_position(cx, position, 2, ctrl);
        cx.run_until_parked();

        let new_tab_view = cx.read_entity(&tabs, |tabs, _| {
            assert_eq!(tabs.tabs.len(), 2);
            assert_eq!(tabs.active_tab, tabs.tabs[0].id);
            tabs.tabs[1].view.clone()
        });
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.path, temp.path());
        });
        cx.read_entity(&new_tab_view, |view, _| {
            assert_eq!(view.path, temp.path().join("a"));
        });
    }

    #[gpui::test]
    fn ctrl_click_sidebar_item_opens_directory_in_new_tab(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a"]);
        let view = active_test_view(&tabs, cx);
        let sidebar_path = temp.path().join("a");
        cx.update(|window, app| {
            tabs.update(app, |_, cx| observe_tab_view(&view, window, cx));
            view.update(app, |view, _| {
                view.sidebar_settings.items = vec![sidebar_path.clone()];
                view.sidebar_sections = crate::explorer::sidebar::sidebar_sections(
                    &view.sidebar_settings,
                    &view.filesystem_name,
                );
            });
        });
        cx.run_until_parked();
        let row = cx
            .debug_bounds("explorer-sidebar-row-0")
            .expect("sidebar row bounds");

        cx.simulate_click(
            row.center(),
            Modifiers {
                control: true,
                ..Modifiers::default()
            },
        );
        cx.run_until_parked();

        let new_tab_view = cx.read_entity(&tabs, |tabs, _| {
            assert_eq!(tabs.tabs.len(), 2);
            assert_eq!(tabs.active_tab, tabs.tabs[0].id);
            tabs.tabs[1].view.clone()
        });
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.path, temp.path());
        });
        cx.read_entity(&new_tab_view, |view, _| {
            assert_eq!(view.path, sidebar_path);
        });
    }

    #[gpui::test]
    fn visual_test_click_selects_entry(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);

        click_second_entry(cx);

        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["b.txt"]);
        });
    }

    #[gpui::test]
    fn single_click_name_cell_whitespace_selects_entry_without_rubber_band(
        cx: &mut TestAppContext,
    ) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        let position = entry_name_position(cx, "explorer-entry-1");

        left_click_position(cx, position, 1, Modifiers::default());
        cx.run_until_parked();

        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["b.txt"]);
            assert!(view.mouse_selection_drag.is_none());
            assert!(view.pending_click_rename.is_none());
        });
    }

    #[gpui::test]
    fn double_click_name_cell_whitespace_opens_directory(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a"]);
        let view = active_test_view(&tabs, cx);
        let position = entry_name_position(cx, "explorer-entry-0");

        left_click_position(cx, position, 1, Modifiers::default());
        left_click_position(cx, position, 2, Modifiers::default());
        cx.run_until_parked();

        cx.read_entity(&view, |view, _| {
            assert_eq!(view.path, temp.path().join("a"));
        });
    }

    #[gpui::test]
    fn dragging_name_cell_whitespace_starts_rubber_band_and_suppresses_row_click(
        cx: &mut TestAppContext,
    ) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        let start = entry_name_position(cx, "explorer-entry-1");
        let end = gpui::point(start.x + gpui::px(20.0), start.y);

        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();

        cx.read_entity(&view, |view, _| {
            let drag = view
                .mouse_selection_drag
                .as_ref()
                .expect("rubber-band drag");
            assert!(drag.active);
            assert!(drag.visible);
            assert!(selected_names(view).is_empty());
        });

        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();

        cx.read_entity(&view, |view, _| {
            assert!(view.mouse_selection_drag.is_none());
            assert!(selected_names(view).is_empty());
        });
    }

    #[gpui::test]
    fn selected_name_cell_whitespace_mouse_down_preserves_selection_without_rubber_band(
        cx: &mut TestAppContext,
    ) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        click_selector(cx, "explorer-entry-name-hit-1");
        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["b.txt"]);
        });

        let position = entry_name_position(cx, "explorer-entry-1");
        cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();

        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["b.txt"]);
            assert!(view.mouse_selection_drag.is_none());
        });

        cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
    }

    #[gpui::test]
    fn selected_name_cell_whitespace_drag_preserves_multi_selection_without_rubber_band(
        cx: &mut TestAppContext,
    ) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.select_single_index(0);
                view.toggle_selection_index(1);
                cx.notify();
            });
        });
        cx.run_until_parked();
        let start = entry_name_position(cx, "explorer-entry-1");
        let end = gpui::point(start.x + gpui::px(20.0), start.y);

        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();

        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["a.txt", "b.txt"]);
            assert!(view.mouse_selection_drag.is_none());
            let dragged = view
                .test_dragged_entries_for_index(1)
                .expect("selected drag payload");
            let dragged_names = dragged
                .paths
                .iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
                .collect::<Vec<_>>();
            assert_eq!(dragged_names, vec!["a.txt", "b.txt"]);
        });

        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    }

    #[gpui::test]
    fn right_click_unselected_name_cell_opens_current_folder_context_menu_and_clears_selection(
        cx: &mut TestAppContext,
    ) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        let first_position = entry_name_position(cx, "explorer-entry-0");
        let second_position = entry_name_position(cx, "explorer-entry-1");

        cx.simulate_click(second_position, Modifiers::default());
        cx.simulate_mouse_down(first_position, MouseButton::Right, Modifiers::default());
        assert!(cx.debug_bounds("mouse-selection-box").is_some());
        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_none());
            assert!(
                !view
                    .mouse_selection_drag
                    .as_ref()
                    .expect("selection drag")
                    .active
            );
        });
        cx.simulate_mouse_up(first_position, MouseButton::Right, Modifiers::default());
        let first_menu_origin = cx
            .debug_bounds("context-menu")
            .expect("context menu")
            .origin;
        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_some());
            assert!(view.mouse_selection_drag.is_none());
            assert_eq!(first_menu_origin, first_position);
            assert_eq!(selected_names(view), Vec::<String>::new());
        });

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.close_context_menu();
                cx.notify();
            });
        });
        cx.run_until_parked();

        cx.simulate_click(first_position, Modifiers::default());
        cx.simulate_mouse_down(second_position, MouseButton::Right, Modifiers::default());
        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_none());
        });
        cx.simulate_mouse_up(second_position, MouseButton::Right, Modifiers::default());
        let second_menu_origin = cx
            .debug_bounds("context-menu")
            .expect("context menu")
            .origin;
        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_some());
            assert_eq!(second_menu_origin, second_position);
            assert_eq!(selected_names(view), Vec::<String>::new());
        });
    }

    #[gpui::test]
    fn right_click_unselected_name_hit_selects_file_and_opens_entry_menu(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);

        right_click_entry_name(cx, "explorer-entry-1");

        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["b.txt"]);
            let menu = view.context_menu.as_ref().expect("entry context menu");
            assert_eq!(
                menu.native_icon_entry
                    .as_ref()
                    .map(|entry| entry.name.as_str()),
                Some("b.txt")
            );
            assert!(matches!(
                menu.items.first(),
                Some(crate::explorer::context_menu::ContextMenuItem::Action {
                    icon: Some(crate::explorer::context_menu::ContextMenuIcon::NativeFile),
                    command: crate::explorer::context_menu::ContextMenuCommand::OpenSelectedFiles,
                    ..
                })
            ));
        });
        assert!(cx.debug_bounds("context-menu-entry-cut").is_some());
        assert!(cx.debug_bounds("context-menu-paste").is_none());
    }

    #[gpui::test]
    fn right_click_selected_name_cell_whitespace_preserves_selection_and_opens_entry_menu(
        cx: &mut TestAppContext,
    ) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        click_selector(cx, "explorer-entry-name-hit-1");
        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["b.txt"]);
        });

        let position = entry_name_position(cx, "explorer-entry-1");
        right_click_position(cx, position);

        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["b.txt"]);
            let menu = view.context_menu.as_ref().expect("entry context menu");
            assert_eq!(
                menu.native_icon_entry
                    .as_ref()
                    .map(|entry| entry.name.as_str()),
                Some("b.txt")
            );
            assert!(matches!(
                menu.items.first(),
                Some(crate::explorer::context_menu::ContextMenuItem::Action {
                    icon: Some(crate::explorer::context_menu::ContextMenuIcon::NativeFile),
                    command: crate::explorer::context_menu::ContextMenuCommand::OpenSelectedFiles,
                    ..
                })
            ));
        });
        assert!(cx.debug_bounds("context-menu-entry-cut").is_some());
        assert!(cx.debug_bounds("context-menu-paste").is_none());
    }

    #[gpui::test]
    fn right_click_selected_name_cell_whitespace_preserves_multi_selection_and_opens_selected_menu(
        cx: &mut TestAppContext,
    ) {
        let (_temp, tabs, cx) = test_tabs_with_directories(cx, &["a", "b"]);
        let view = active_test_view(&tabs, cx);
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.select_single_index(0);
                view.toggle_selection_index(1);
                cx.notify();
            });
        });
        cx.run_until_parked();

        let position = entry_name_position(cx, "explorer-entry-1");
        right_click_position(cx, position);

        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["a", "b"]);
            let menu = view.context_menu.as_ref().expect("entry context menu");
            assert!(!menu.items.iter().any(|item| matches!(
                item,
                crate::explorer::context_menu::ContextMenuItem::Action {
                    command:
                        crate::explorer::context_menu::ContextMenuCommand::OpenDirectory { .. }
                            | crate::explorer::context_menu::ContextMenuCommand::OpenSelectedFiles,
                    ..
                }
            )));
            assert!(matches!(
                menu.items.first(),
                Some(crate::explorer::context_menu::ContextMenuItem::Action {
                    label,
                    command:
                        crate::explorer::context_menu::ContextMenuCommand::OpenSelectedDirectoriesInNewTabs,
                    ..
                }) if label == "Open new tabs (2)"
            ));
        });
        assert!(cx.debug_bounds("context-menu-entry-copy-path").is_none());
    }

    #[gpui::test]
    fn right_click_unselected_other_column_selects_file_and_opens_entry_menu(
        cx: &mut TestAppContext,
    ) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);

        right_click_entry_other_column(cx, "explorer-entry-1");

        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["b.txt"]);
            let menu = view.context_menu.as_ref().expect("entry context menu");
            assert_eq!(
                menu.native_icon_entry
                    .as_ref()
                    .map(|entry| entry.name.as_str()),
                Some("b.txt")
            );
            assert!(matches!(
                menu.items.first(),
                Some(crate::explorer::context_menu::ContextMenuItem::Action {
                    icon: Some(crate::explorer::context_menu::ContextMenuIcon::NativeFile),
                    command: crate::explorer::context_menu::ContextMenuCommand::OpenSelectedFiles,
                    ..
                })
            ));
        });
        assert!(cx.debug_bounds("context-menu-entry-cut").is_some());
        assert!(cx.debug_bounds("context-menu-paste").is_none());
    }

    #[gpui::test]
    fn right_button_rubber_band_opens_context_menu_for_new_selection(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        let first = cx
            .debug_bounds("explorer-entry-0")
            .expect("first entry bounds");
        let second = cx
            .debug_bounds("explorer-entry-1")
            .expect("second entry bounds");
        let start = gpui::point(
            first.left() + gpui::px(10.0),
            second.bottom() + gpui::px(20.0),
        );
        let end = gpui::point(first.left() + gpui::px(100.0), first.top() + gpui::px(2.0));

        cx.simulate_mouse_down(start, MouseButton::Right, Modifiers::default());
        let initial_box = cx
            .debug_bounds("mouse-selection-box")
            .expect("right-button selection box");
        assert!(initial_box.size.width > gpui::px(0.0));
        assert!(initial_box.size.height > gpui::px(0.0));
        cx.read_entity(&view, |view, _| {
            let drag = view.mouse_selection_drag.as_ref().expect("selection drag");
            assert!(drag.visible);
            assert!(!drag.active);
        });

        cx.simulate_mouse_move(end, MouseButton::Right, Modifiers::default());
        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_none());
            assert_eq!(selected_names(view), vec!["a.txt", "b.txt"]);
        });

        cx.simulate_mouse_up(end, MouseButton::Right, Modifiers::default());

        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_some());
            assert_eq!(selected_names(view), vec!["a.txt", "b.txt"]);
        });
        assert!(cx.debug_bounds("context-menu-entry-cut").is_some());
    }

    #[gpui::test]
    fn right_button_rubber_band_with_empty_selection_opens_folder_context_menu(
        cx: &mut TestAppContext,
    ) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        let second = cx
            .debug_bounds("explorer-entry-1")
            .expect("second entry bounds");
        let start = gpui::point(
            second.left() + gpui::px(10.0),
            second.bottom() + gpui::px(20.0),
        );
        let end = gpui::point(
            second.left() + gpui::px(100.0),
            second.bottom() + gpui::px(40.0),
        );

        cx.simulate_mouse_down(start, MouseButton::Right, Modifiers::default());
        cx.simulate_mouse_move(end, MouseButton::Right, Modifiers::default());
        cx.simulate_mouse_up(end, MouseButton::Right, Modifiers::default());

        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_some());
            assert!(selected_names(view).is_empty());
        });
        assert!(cx.debug_bounds("context-menu-paste").is_some());
        assert!(cx.debug_bounds("context-menu-entry-cut").is_none());
    }

    #[gpui::test]
    fn right_button_down_restarts_rubber_band_behind_active_context_menu(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        let first = cx
            .debug_bounds("explorer-entry-0")
            .expect("first entry bounds");
        let second = cx
            .debug_bounds("explorer-entry-1")
            .expect("second entry bounds");
        let start = gpui::point(
            first.left() + gpui::px(10.0),
            second.bottom() + gpui::px(20.0),
        );
        let end = gpui::point(first.left() + gpui::px(100.0), first.top() + gpui::px(2.0));

        right_click_selector(cx, "explorer-entry-0");
        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_some());
        });

        cx.simulate_mouse_down(start, MouseButton::Right, Modifiers::default());

        assert!(cx.debug_bounds("context-menu").is_none());
        assert!(cx.debug_bounds("mouse-selection-box").is_some());
        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_none());
            let drag = view.mouse_selection_drag.as_ref().expect("selection drag");
            assert!(drag.visible);
            assert!(!drag.active);
        });

        cx.simulate_mouse_move(end, MouseButton::Right, Modifiers::default());
        cx.simulate_mouse_up(end, MouseButton::Right, Modifiers::default());

        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_some());
            assert_eq!(selected_names(view), vec!["a.txt", "b.txt"]);
        });
        assert!(cx.debug_bounds("context-menu-entry-cut").is_some());
    }

    #[gpui::test]
    fn right_button_down_inside_context_menu_is_contained(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);

        right_click_entry_other_column(cx, "explorer-entry-0");
        let menu_position = cx
            .debug_bounds("context-menu-entry-cut")
            .expect("context menu row")
            .center();

        cx.simulate_mouse_down(menu_position, MouseButton::Right, Modifiers::default());

        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_some());
            assert!(view.mouse_selection_drag.is_none());
        });
        assert!(cx.debug_bounds("context-menu").is_some());
        assert!(cx.debug_bounds("mouse-selection-box").is_none());
    }

    #[gpui::test]
    fn opening_sidebar_context_menu_clears_entry_selection(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        let sidebar_path = temp.path().to_path_buf();

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.select_single_index(1);
                assert!(view.open_sidebar_context_menu(
                    gpui::point(gpui::px(20.0), gpui::px(20.0)),
                    sidebar_path,
                    42,
                    None,
                    None,
                    None,
                    false,
                    window,
                    cx,
                ));
                cx.notify();
            });
        });

        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_some());
            assert!(selected_names(view).is_empty());
        });
    }

    #[gpui::test]
    fn right_click_unselected_folder_other_column_selects_it_and_opens_entry_menu(
        cx: &mut TestAppContext,
    ) {
        let (_temp, tabs, cx) = test_tabs_with_directories(cx, &["a", "b"]);
        let view = active_test_view(&tabs, cx);

        right_click_entry_other_column(cx, "explorer-entry-1");

        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["b"]);
            let menu = view.context_menu.as_ref().expect("entry context menu");
            assert!(matches!(
                menu.items.first(),
                Some(crate::explorer::context_menu::ContextMenuItem::Action {
                    command: crate::explorer::context_menu::ContextMenuCommand::OpenDirectory {
                        ..
                    },
                    ..
                })
            ));
        });
    }

    #[gpui::test]
    fn right_click_selected_folder_preserves_multi_selection_and_omits_primary_open_and_rename(
        cx: &mut TestAppContext,
    ) {
        let (_temp, tabs, cx) = test_tabs_with_directories(cx, &["a", "b"]);
        let view = active_test_view(&tabs, cx);
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.select_single_index(0);
                view.toggle_selection_index(1);
                cx.notify();
            });
        });
        cx.run_until_parked();

        right_click_entry_name(cx, "explorer-entry-1");

        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["a", "b"]);
            let menu = view.context_menu.as_ref().expect("entry context menu");
            assert!(!menu.items.iter().any(|item| matches!(
                item,
                crate::explorer::context_menu::ContextMenuItem::Action {
                    command:
                        crate::explorer::context_menu::ContextMenuCommand::OpenDirectory { .. }
                            | crate::explorer::context_menu::ContextMenuCommand::OpenSelectedFiles,
                    ..
                }
            )));
            assert!(matches!(
                menu.items.first(),
                Some(crate::explorer::context_menu::ContextMenuItem::Action {
                    label,
                    command:
                        crate::explorer::context_menu::ContextMenuCommand::OpenSelectedDirectoriesInNewTabs,
                    ..
                }) if label == "Open new tabs (2)"
            ));
            assert!(!menu.items.iter().any(|item| matches!(
                item,
                crate::explorer::context_menu::ContextMenuItem::Action {
                    command: crate::explorer::context_menu::ContextMenuCommand::RenameSelected,
                    ..
                }
            )));
        });
        assert!(cx.debug_bounds("context-menu-entry-copy-path").is_none());
    }

    #[gpui::test]
    fn folder_context_menu_cut_preserves_selection_and_marks_folder_cut(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a"]);
        let view = active_test_view(&tabs, cx);
        let path = temp.path().join("a");

        right_click_entry_other_column(cx, "explorer-entry-0");
        click_selector(cx, "context-menu-entry-cut");

        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["a"]);
            assert!(view.entry_is_cut(&path));
            assert!(view.context_menu.is_none());
        });
    }

    #[gpui::test]
    fn folder_context_menu_copy_preserves_selection_and_copies_folder(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a"]);
        let view = active_test_view(&tabs, cx);
        let path = temp.path().join("a");

        right_click_entry_other_column(cx, "explorer-entry-0");
        click_selector(cx, "context-menu-entry-copy");

        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["a"]);
            assert!(view.context_menu.is_none());
        });
        cx.update(|_, app| {
            let clipboard = app
                .read_from_clipboard()
                .as_ref()
                .and_then(file_clipboard_from_item);
            assert_eq!(
                clipboard,
                Some(FileClipboard::new(FileClipboardOperation::Copy, vec![path]))
            );
        });
    }

    #[gpui::test]
    fn file_context_menu_copy_path_copies_selected_file_path(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        let path = temp.path().join("a.txt");
        let expected = cx.read_entity(&view, |view, _| view.address_text_for_path(&path));

        right_click_entry_other_column(cx, "explorer-entry-0");
        click_selector(cx, "context-menu-entry-copy-path");

        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["a.txt"]);
            assert!(view.context_menu.is_none());
        });
        cx.update(|_, app| {
            assert_eq!(
                app.read_from_clipboard().and_then(|item| item.text()),
                Some(expected)
            );
        });
    }

    #[gpui::test]
    fn file_context_menu_copy_relative_repo_path_copies_selected_file_repo_path(
        cx: &mut TestAppContext,
    ) {
        let temp = TempDir::new();
        Repository::init(temp.path()).expect("init repo");
        let source_dir = temp.path().join("src");
        fs::create_dir(&source_dir).expect("create source directory");
        fs::write(source_dir.join("a.txt"), b"file").expect("write test file");
        let (tabs, cx) = test_tabs_at_path(cx, source_dir);
        let view = active_test_view(&tabs, cx);

        right_click_entry_other_column(cx, "explorer-entry-0");
        click_selector(cx, "context-menu-entry-copy-relative-repo-path");

        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["a.txt"]);
            assert!(view.context_menu.is_none());
        });
        cx.update(|_, app| {
            assert_eq!(
                app.read_from_clipboard().and_then(|item| item.text()),
                Some("src/a.txt".to_owned())
            );
        });
    }

    #[gpui::test]
    fn archive_context_menu_extract_extracts_selected_archive(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_files(cx, &[]);
        let view = active_test_view(&tabs, cx);
        let archive = temp.path().join("archive.zip");
        create_zip_archive(&archive, &[("inside.txt", b"archive contents")]);

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.reload_with_entry_metadata_resolution(cx);
                cx.notify();
            });
        });
        cx.run_until_parked();

        right_click_entry_other_column(cx, "explorer-entry-0");
        click_selector(cx, "context-menu-entry-extract");
        cx.run_until_parked();

        assert_eq!(
            fs::read(temp.path().join("inside.txt")).unwrap(),
            b"archive contents"
        );
        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["archive.zip"]);
            assert!(view.context_menu.is_none());
        });
    }

    #[gpui::test]
    fn current_folder_context_menu_copy_path_copies_current_folder_path(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        let expected = cx.read_entity(&view, |view, _| view.address_text_for_path(temp.path()));
        let second = cx
            .debug_bounds("explorer-entry-1")
            .expect("second entry bounds");
        let start = gpui::point(
            second.left() + gpui::px(10.0),
            second.bottom() + gpui::px(20.0),
        );
        let end = gpui::point(
            second.left() + gpui::px(100.0),
            second.bottom() + gpui::px(40.0),
        );

        cx.simulate_mouse_down(start, MouseButton::Right, Modifiers::default());
        cx.simulate_mouse_move(end, MouseButton::Right, Modifiers::default());
        cx.simulate_mouse_up(end, MouseButton::Right, Modifiers::default());
        click_selector(cx, "context-menu-folder-copy-path");

        cx.read_entity(&view, |view, _| {
            assert!(selected_names(view).is_empty());
            assert!(view.context_menu.is_none());
        });
        cx.update(|_, app| {
            assert_eq!(
                app.read_from_clipboard().and_then(|item| item.text()),
                Some(expected)
            );
        });
    }

    #[gpui::test]
    fn current_folder_context_menu_copy_relative_repo_path_copies_current_folder_repo_path(
        cx: &mut TestAppContext,
    ) {
        let temp = TempDir::new();
        Repository::init(temp.path()).expect("init repo");
        let source_dir = temp.path().join("src");
        fs::create_dir(&source_dir).expect("create source directory");
        fs::write(source_dir.join("a.txt"), b"file").expect("write first file");
        fs::write(source_dir.join("b.txt"), b"file").expect("write second file");
        let (tabs, cx) = test_tabs_at_path(cx, source_dir);
        let view = active_test_view(&tabs, cx);
        let second = cx
            .debug_bounds("explorer-entry-1")
            .expect("second entry bounds");
        let start = gpui::point(
            second.left() + gpui::px(10.0),
            second.bottom() + gpui::px(20.0),
        );
        let end = gpui::point(
            second.left() + gpui::px(100.0),
            second.bottom() + gpui::px(40.0),
        );

        cx.simulate_mouse_down(start, MouseButton::Right, Modifiers::default());
        cx.simulate_mouse_move(end, MouseButton::Right, Modifiers::default());
        cx.simulate_mouse_up(end, MouseButton::Right, Modifiers::default());
        click_selector(cx, "context-menu-folder-copy-relative-repo-path");

        cx.read_entity(&view, |view, _| {
            assert!(selected_names(view).is_empty());
            assert!(view.context_menu.is_none());
        });
        cx.update(|_, app| {
            assert_eq!(
                app.read_from_clipboard().and_then(|item| item.text()),
                Some("src".to_owned())
            );
        });
    }

    #[gpui::test]
    fn paste_clipboard_image_saves_file_selects_it_and_starts_rename(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_files(cx, &[]);
        let view = observe_active_test_view(&tabs, cx);
        let image = Image::from_bytes(ImageFormat::Png, vec![1, 2, 3, 4]);

        cx.update(|_, app| app.write_to_clipboard(ClipboardItem::new_image(&image)));
        cx.dispatch_action(PasteClipboard);
        cx.run_until_parked();

        let path = temp.path().join("image.png");
        assert_eq!(fs::read(&path).unwrap(), vec![1, 2, 3, 4]);
        cx.update(|window, app| {
            view.update(app, |view, _| {
                let rename_focus = view
                    .active_rename_focus_handle()
                    .expect("pasted image rename focus");
                assert!(rename_focus.is_focused(window));
            });
        });
        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["image.png"]);
            assert!(view.rename_is_active_for_path(&path));
        });
    }

    #[gpui::test]
    fn paste_clipboard_image_uses_first_free_image_name(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_files(cx, &["image.png"]);
        let view = active_test_view(&tabs, cx);
        let image = Image::from_bytes(ImageFormat::Png, vec![5, 6, 7]);

        cx.update(|_, app| app.write_to_clipboard(ClipboardItem::new_image(&image)));
        cx.dispatch_action(PasteClipboard);
        cx.run_until_parked();

        let path = temp.path().join("image (2).png");
        assert_eq!(fs::read(&path).unwrap(), vec![5, 6, 7]);
        assert_eq!(fs::read(temp.path().join("image.png")).unwrap(), b"file");
        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["image (2).png"]);
            assert!(view.rename_is_active_for_path(&path));
        });
    }

    #[gpui::test]
    fn paste_materializes_supported_text_formats_and_starts_rename(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_files(cx, &[]);
        let view = observe_active_test_view(&tabs, cx);
        let cases = [
            (
                ClipboardItem::new_string("{\"ok\": true}".to_owned()),
                "data.json",
                "{\"ok\": true}",
            ),
            (
                ClipboardItem::new_string("a\tb\n1\t2".to_owned()),
                "table.csv",
                "a,b\r\n1,2",
            ),
            (
                ClipboardItem::new_string_with_markdown(
                    "Heading".to_owned(),
                    "# Heading".to_owned(),
                ),
                "document.md",
                "# Heading",
            ),
            (
                ClipboardItem::new_string("<svg viewBox=\"0 0 1 1\"></svg>".to_owned()),
                "vector.svg",
                "<svg viewBox=\"0 0 1 1\"></svg>",
            ),
            (
                ClipboardItem::new_string("<p>plain fallback</p>".to_owned()),
                "text.txt",
                "<p>plain fallback</p>",
            ),
        ];

        for (item, file_name, expected) in cases {
            cx.update(|_, app| app.write_to_clipboard(item));
            cx.dispatch_action(PasteClipboard);
            cx.run_until_parked();

            let path = temp.path().join(file_name);
            assert_eq!(fs::read_to_string(&path).unwrap(), expected);
            cx.update(|window, app| {
                view.update(app, |view, _| {
                    assert!(view.rename_is_active_for_path(&path));
                    assert!(
                        view.active_rename_focus_handle()
                            .expect("materialized rename focus")
                            .is_focused(window)
                    );
                });
            });
            cx.dispatch_action(RenameCancel);
            cx.run_until_parked();
        }
    }

    #[gpui::test]
    fn paste_single_token_clipboard_text_does_nothing(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_files(cx, &[]);
        let view = observe_active_test_view(&tabs, cx);

        cx.update(|_, app| {
            app.write_to_clipboard(ClipboardItem::new_string("password".to_owned()));
        });
        cx.dispatch_action(PasteClipboard);
        cx.run_until_parked();

        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
        cx.read_entity(&view, |view, _| {
            assert!(view.operation_notice.is_none());
            assert!(selected_names(view).is_empty());
            assert!(view.active_rename_focus_handle().is_none());
        });
    }

    #[gpui::test]
    fn observed_new_folder_starts_focused_rename_and_refreshes_peer_tab(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_files(cx, &[]);
        let view = observe_active_test_view(&tabs, cx);
        let peer_view = cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                tabs.add_background_tab(temp.path().to_path_buf(), window, cx);
                tabs.tabs.last().unwrap().view.clone()
            })
        });
        cx.run_until_parked();

        cx.dispatch_action(CreateNewFolder);
        cx.run_until_parked();

        let folder_path = temp.path().join("New folder");
        assert!(folder_path.is_dir());
        cx.update(|window, app| {
            view.update(app, |view, _| {
                assert_eq!(selected_names(view), vec!["New folder"]);
                assert!(view.rename_is_active_for_path(&folder_path));
                let rename_focus = view
                    .active_rename_focus_handle()
                    .expect("new folder rename focus");
                assert!(rename_focus.is_focused(window));
            });
        });
        cx.read_entity(&peer_view, |view, _| {
            assert!(view.entries.iter().any(|entry| entry.path == folder_path));
        });
    }

    #[gpui::test]
    fn observed_new_file_starts_focused_rename(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_files(cx, &[]);
        let view = observe_active_test_view(&tabs, cx);

        cx.dispatch_action(CreateNewFile);
        cx.run_until_parked();

        let file_path = temp.path().join("New file");
        assert!(file_path.is_file());
        cx.update(|window, app| {
            view.update(app, |view, _| {
                assert_eq!(selected_names(view), vec!["New file"]);
                assert!(view.rename_is_active_for_path(&file_path));
                let rename_focus = view
                    .active_rename_focus_handle()
                    .expect("new file rename focus");
                assert!(rename_focus.is_focused(window));
            });
        });
    }

    #[gpui::test]
    fn new_folder_focused_rename_commits_on_click_away(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_files(cx, &["z.txt"]);
        let view = active_test_view(&tabs, cx);

        cx.dispatch_action(CreateNewFolder);
        cx.run_until_parked();

        let folder_path = temp.path().join("New folder");
        cx.update(|window, app| {
            view.update(app, |view, _| {
                assert_eq!(selected_names(view), vec!["New folder"]);
                assert!(view.rename_is_active_for_path(&folder_path));
                let rename_focus = view
                    .active_rename_focus_handle()
                    .expect("new folder rename focus");
                assert!(rename_focus.is_focused(window));
                view.active_rename.as_mut().unwrap().content = "Renamed folder".to_owned();
            });
        });

        click_selector(cx, "explorer-entry-name-hit-1");

        assert!(temp.path().join("Renamed folder").is_dir());
        cx.read_entity(&view, |view, _| {
            assert!(!view.has_active_text_input());
            assert_eq!(selected_names(view), vec!["z.txt"]);
        });
    }

    #[gpui::test]
    fn folder_context_menu_delete_removes_selected_folder(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a", "b"]);
        let view = active_test_view(&tabs, cx);
        let path = temp.path().join("a");

        right_click_entry_other_column(cx, "explorer-entry-0");
        click_selector(cx, "context-menu-entry-delete");
        cx.run_until_parked();

        assert!(!path.exists());
        assert!(temp.path().join("b").exists());
        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["b"]);
            assert!(view.context_menu.is_none());
        });
    }

    #[gpui::test]
    fn confirmed_trash_selects_the_next_surviving_item(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_files(cx, &["a.txt", "b.txt", "c.txt"]);
        let view = active_test_view(&tabs, cx);
        let paths = vec![temp.path().join("a.txt"), temp.path().join("b.txt")];

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.restore_selection_from_paths(&paths);
                view.mark_cut_paths(&paths);
                view.pending_trash = Some(PendingTrash {
                    paths: paths.clone(),
                });
                view.confirm_pending_trash(cx);
                assert!(view.pending_trash.is_none());
                assert!(view.pending_trash_task.is_some());
                assert_eq!(view.pending_deleted_paths, paths);
                assert!(paths.iter().all(|path| path.exists()));
                assert_eq!(selected_names(view), vec!["c.txt"]);
                assert_eq!(
                    view.entries
                        .iter()
                        .map(|entry| entry.name.as_str())
                        .collect::<Vec<_>>(),
                    vec!["c.txt"]
                );
                assert!(view.has_background_operation());
                assert!(view.file_operation_undo_stack.is_empty());
            });
        });
        cx.run_until_parked();

        assert!(!paths[0].exists());
        assert!(!paths[1].exists());
        assert!(temp.path().join("c.txt").exists());
        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["c.txt"]);
            assert!(view.pending_trash_task.is_none());
            assert!(view.pending_deleted_paths.is_empty());
            assert!(!view.has_background_operation());
            assert_eq!(view.file_operation_undo_stack.len(), 1);
            assert!(paths.iter().all(|path| !view.entry_is_cut(path)));
        });
    }

    #[gpui::test]
    fn confirmed_permanent_delete_selects_the_next_surviving_item(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_files(cx, &["a.txt", "b.txt", "c.txt"]);
        let view = active_test_view(&tabs, cx);
        let paths = vec![temp.path().join("a.txt"), temp.path().join("b.txt")];

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.restore_selection_from_paths(&paths);
                view.pending_permanent_delete = Some(PendingPermanentDelete {
                    paths: paths.clone(),
                });
                view.confirm_pending_permanent_delete(cx);
            });
        });
        cx.run_until_parked();

        assert!(!paths[0].exists());
        assert!(!paths[1].exists());
        assert!(temp.path().join("c.txt").exists());
        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["c.txt"]);
        });
    }

    #[gpui::test]
    fn folder_context_menu_rename_preserves_selection_and_starts_rename(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a"]);
        let view = active_test_view(&tabs, cx);
        let path = temp.path().join("a");

        right_click_entry_other_column(cx, "explorer-entry-0");
        click_selector(cx, "context-menu-entry-rename");

        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["a"]);
            assert!(view.rename_is_active_for_path(&path));
            assert!(view.context_menu.is_none());
        });
    }

    #[gpui::test]
    fn folder_context_menu_open_navigates_active_tab(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a"]);
        let view = active_test_view(&tabs, cx);
        let target = temp.path().join("a");

        right_click_entry_other_column(cx, "explorer-entry-0");
        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.execute_context_menu_command(
                    crate::explorer::context_menu::ContextMenuCommand::OpenDirectory {
                        path: target.clone(),
                    },
                    window,
                    cx,
                );
            });
        });

        cx.read_entity(&view, |view, _| {
            assert_eq!(view.path, target);
            assert!(view.context_menu.is_none());
        });
    }

    #[gpui::test]
    fn folder_context_menu_open_in_new_tab_opens_single_selected_folder(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a"]);
        let target = temp.path().join("a");
        let view = active_test_view(&tabs, cx);
        cx.update(|window, app| {
            tabs.update(app, |_, cx| observe_tab_view(&view, window, cx));
        });

        right_click_entry_other_column(cx, "explorer-entry-0");
        cx.read_entity(&view, |view, _| {
            let menu = view.context_menu.as_ref().expect("entry context menu");
            assert!(matches!(
                menu.items.first(),
                Some(crate::explorer::context_menu::ContextMenuItem::Action {
                    command: crate::explorer::context_menu::ContextMenuCommand::OpenDirectory {
                        ..
                    },
                    ..
                })
            ));
            assert!(matches!(
                menu.items.get(1),
                Some(crate::explorer::context_menu::ContextMenuItem::Action {
                    label,
                    command:
                        crate::explorer::context_menu::ContextMenuCommand::OpenSelectedDirectoriesInNewTabs,
                    ..
                }) if label == "Open in new tab"
            ));
        });
        click_selector(cx, "context-menu-entry-open-new-tab");
        cx.run_until_parked();

        let new_tab_view = cx.read_entity(&tabs, |tabs, _| {
            assert_eq!(tabs.tabs.len(), 2);
            tabs.tabs[1].view.clone()
        });
        cx.read_entity(&new_tab_view, |view, _| {
            assert_eq!(view.path, target);
        });
    }

    #[gpui::test]
    fn folder_context_menu_open_in_new_tabs_ignores_files_and_preserves_folder_display_order(
        cx: &mut TestAppContext,
    ) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories_and_files(cx, &["a", "b"], &["file.txt"]);
        let view = active_test_view(&tabs, cx);
        cx.update(|window, app| {
            tabs.update(app, |_, cx| observe_tab_view(&view, window, cx));
            view.update(app, |view, cx| {
                view.select_all_entries();
                cx.notify();
            });
        });
        cx.run_until_parked();

        right_click_entry_name(cx, "explorer-entry-1");
        cx.read_entity(&view, |view, _| {
            let menu = view.context_menu.as_ref().expect("entry context menu");
            assert!(matches!(
                menu.items.first(),
                Some(crate::explorer::context_menu::ContextMenuItem::Action {
                    label,
                    command: crate::explorer::context_menu::ContextMenuCommand::OpenSelectedFiles,
                    ..
                }) if label == "Open files (1)"
            ));
            assert!(matches!(
                menu.items.get(1),
                Some(crate::explorer::context_menu::ContextMenuItem::Action {
                    label,
                    command:
                        crate::explorer::context_menu::ContextMenuCommand::OpenSelectedDirectoriesInNewTabs,
                    ..
                }) if label == "Open new tabs (2)"
            ));
        });
        click_selector(cx, "context-menu-entry-open-new-tab");
        cx.run_until_parked();

        let new_tab_views = cx.read_entity(&tabs, |tabs, _| {
            assert_eq!(tabs.tabs.len(), 3);
            tabs.tabs[1..]
                .iter()
                .map(|tab| tab.view.clone())
                .collect::<Vec<_>>()
        });
        let new_tab_paths = new_tab_views
            .iter()
            .map(|view| cx.read_entity(view, |view, _| view.path.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            new_tab_paths,
            vec![temp.path().join("a"), temp.path().join("b")]
        );
    }

    #[gpui::test]
    fn folder_context_menu_open_in_new_tab_ignores_selected_files(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories_and_files(cx, &["folder"], &["file.txt"]);
        let view = active_test_view(&tabs, cx);
        cx.update(|window, app| {
            tabs.update(app, |_, cx| observe_tab_view(&view, window, cx));
            view.update(app, |view, cx| {
                view.select_all_entries();
                cx.notify();
            });
        });
        cx.run_until_parked();

        right_click_entry_name(cx, "explorer-entry-0");
        cx.read_entity(&view, |view, _| {
            let menu = view.context_menu.as_ref().expect("entry context menu");
            assert!(matches!(
                menu.items.first(),
                Some(crate::explorer::context_menu::ContextMenuItem::Action {
                    label,
                    command: crate::explorer::context_menu::ContextMenuCommand::OpenSelectedFiles,
                    ..
                }) if label == "Open files (1)"
            ));
            assert!(matches!(
                menu.items.get(1),
                Some(crate::explorer::context_menu::ContextMenuItem::Action {
                    label,
                    command:
                        crate::explorer::context_menu::ContextMenuCommand::OpenSelectedDirectoriesInNewTabs,
                    ..
                }) if label == "Open in new tab"
            ));
        });
        click_selector(cx, "context-menu-entry-open-new-tab");
        cx.run_until_parked();

        let new_tab_view = cx.read_entity(&tabs, |tabs, _| {
            assert_eq!(tabs.tabs.len(), 2);
            tabs.tabs[1].view.clone()
        });
        cx.read_entity(&new_tab_view, |view, _| {
            assert_eq!(view.path, temp.path().join("folder"));
        });
    }

    #[gpui::test]
    fn clicking_entry_closes_context_menu_and_selects_with_one_click(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_files(
            cx,
            &[
                "a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "f.txt", "g.txt", "h.txt", "i.txt",
                "j.txt",
            ],
        );
        let view = active_test_view(&tabs, cx);

        right_click_selector(cx, "explorer-entry-0");
        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_some());
        });

        click_selector(cx, "explorer-entry-name-hit-9");

        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_none());
            assert_eq!(selected_names(view), vec!["j.txt"]);
        });
    }

    #[gpui::test]
    fn clicking_sidebar_closes_context_menu(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);

        right_click_selector(cx, "explorer-entry-0");
        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_some());
        });

        click_selector(cx, "explorer-sidebar");

        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_none());
        });
    }

    #[gpui::test]
    fn clicking_address_or_search_closes_context_menu(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);

        right_click_selector(cx, "explorer-entry-0");
        click_selector(cx, "directory-bar");
        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_none());
        });

        right_click_selector(cx, "explorer-entry-0");
        click_selector(cx, "search-bar");
        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_none());
        });
    }

    #[gpui::test]
    fn unmodified_typing_starts_search_and_enters_text_once(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);

        cx.simulate_input("b");

        cx.read_entity(&view, |view, _| {
            assert!(view.search_is_editing());
            assert_eq!(view.search_query(), "b");
            assert_eq!(view.entries.len(), 1);
            assert_eq!(view.entries[0].name, "b.txt");
        });

        cx.simulate_input("a");

        cx.read_entity(&view, |view, _| assert_eq!(view.search_query(), "ba"));
    }

    #[gpui::test]
    fn type_to_search_replaces_an_inactive_query(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);

        cx.simulate_input("a");
        cx.dispatch_action(SearchCommit);
        cx.simulate_input("b");

        cx.read_entity(&view, |view, _| {
            assert!(view.search_is_editing());
            assert_eq!(view.search_query(), "b");
        });
    }

    #[gpui::test]
    fn search_commit_opens_focused_entry_after_arrow_navigation(cx: &mut TestAppContext) {
        let (temp, tabs, cx) =
            test_tabs_with_directories_and_files(cx, &["target-folder"], &["other.txt"]);
        let view = active_test_view(&tabs, cx);

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                assert!(view.start_search_edit(window, cx));
                view.set_search_query("target".to_owned());
                cx.notify();
            });
        });
        cx.dispatch_action(MoveDown);

        cx.read_entity(&view, |view, _| {
            assert!(view.search_is_editing());
            assert_eq!(selected_names(view), vec!["target-folder"]);
        });

        cx.dispatch_action(SearchCommit);

        cx.read_entity(&view, |view, _| {
            assert!(!view.search_is_editing());
            assert_eq!(view.path, temp.path().join("target-folder"));
        });
    }

    #[gpui::test]
    fn ctrl_f_action_forces_regular_search(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.search.recursive_enabled = true;
                view.set_search_query("a".to_owned());
                cx.notify();
            });
        });
        cx.dispatch_action(SearchEdit);

        cx.read_entity(&view, |view, _| {
            assert!(view.search_is_editing());
            assert!(!view.recursive_search_is_enabled());
            assert_eq!(view.search_query(), "a");
            assert_eq!(view.entries.len(), 1);
            assert_eq!(view.entries[0].name, "a.txt");
        });
    }

    #[gpui::test]
    fn recursive_search_action_forces_recursive_search(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);

        cx.dispatch_action(RecursiveSearchEdit);

        cx.read_entity(&view, |view, _| {
            assert!(view.search_is_editing());
            assert!(view.recursive_search_is_enabled());
        });
    }

    #[gpui::test]
    fn recursive_search_action_is_not_a_toggle(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);

        cx.dispatch_action(RecursiveSearchEdit);
        cx.dispatch_action(RecursiveSearchEdit);

        cx.read_entity(&view, |view, _| {
            assert!(view.search_is_editing());
            assert!(view.recursive_search_is_enabled());
        });
    }

    #[gpui::test]
    fn modified_and_non_printable_keys_do_not_start_search(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);

        cx.simulate_keystrokes("shift-z ctrl-z alt-z win-z fn-z left");

        cx.read_entity(&view, |view, _| {
            assert!(!view.search_is_editing());
            assert_eq!(view.search_query(), "");
        });
    }

    #[gpui::test]
    fn active_address_and_rename_inputs_are_not_hijacked_by_typing(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                assert!(view.start_address_bar_edit(window, cx));
                cx.notify();
            });
        });
        cx.simulate_input("z");
        cx.read_entity(&view, |view, _| {
            assert!(view.address_bar_is_editing());
            assert_eq!(view.search_query(), "");
        });

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.cancel_address_bar_edit();
                view.focus_explorer(window);
                view.select_single_path(&temp.path().join("a.txt"));
                assert!(view.start_rename_selected(window, cx));
                cx.notify();
            });
        });
        cx.simulate_input("z");
        cx.read_entity(&view, |view, _| {
            assert!(view.has_active_text_input());
            assert_eq!(view.search_query(), "");
        });
    }

    #[gpui::test]
    fn search_click_away_selects_entry_with_same_click(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        cx.update(|window, app| {
            view.update(app, |view, cx| {
                assert!(view.start_search_edit(window, cx));
                view.set_search_query(".txt".to_owned());
                cx.notify();
            });
        });

        let bounds = cx
            .debug_bounds("explorer-entry-name-hit-1")
            .expect("second entry name hit bounds");
        cx.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(bounds.center(), MouseButton::Left, Modifiers::default());

        cx.read_entity(&view, |view, _| {
            assert!(!view.search_is_editing());
            assert_eq!(view.search_query(), ".txt");
            assert_eq!(selected_names(view), vec!["b.txt"]);
        });
    }

    #[gpui::test]
    fn address_click_away_selects_entry_with_same_click(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        cx.update(|window, app| {
            view.update(app, |view, cx| {
                assert!(view.start_address_bar_edit(window, cx));
                cx.notify();
            });
        });

        click_second_entry(cx);

        cx.read_entity(&view, |view, _| {
            assert!(!view.address_bar_is_editing());
            assert_eq!(selected_names(view), vec!["b.txt"]);
        });
    }

    #[gpui::test]
    fn rename_click_away_selects_entry_with_same_click(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.select_single_path(&temp.path().join("a.txt"));
                assert!(view.start_rename_selected(window, cx));
                view.active_rename.as_mut().unwrap().content = "c.txt".to_owned();
                cx.notify();
            });
        });

        click_second_entry(cx);

        assert!(temp.path().join("c.txt").exists());
        cx.read_entity(&view, |view, _| {
            assert!(!view.has_active_text_input());
            assert_eq!(selected_names(view), vec!["b.txt"]);
        });
    }

    #[gpui::test]
    fn conflicting_rename_click_away_cancels_and_selects_entry_with_same_click(
        cx: &mut TestAppContext,
    ) {
        let (temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.select_single_path(&temp.path().join("a.txt"));
                assert!(view.start_rename_selected(window, cx));
                view.active_rename.as_mut().unwrap().content = "b.txt".to_owned();
                cx.notify();
            });
        });

        click_second_entry(cx);

        assert!(temp.path().join("a.txt").exists());
        assert!(temp.path().join("b.txt").exists());
        cx.read_entity(&view, |view, _| {
            assert!(!view.has_active_text_input());
            assert!(view.operation_notice.is_none());
            assert_eq!(selected_names(view), vec!["b.txt"]);
        });
    }

    #[gpui::test]
    fn invalid_rename_click_away_cancels_and_selects_entry_with_same_click(
        cx: &mut TestAppContext,
    ) {
        let (temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.select_single_path(&temp.path().join("a.txt"));
                assert!(view.start_rename_selected(window, cx));
                let rename = view.active_rename.as_mut().unwrap();
                rename.content.clear();
                rename.selected_range = 0..0;
                cx.notify();
            });
        });

        click_second_entry(cx);

        assert!(temp.path().join("a.txt").exists());
        cx.read_entity(&view, |view, _| {
            assert!(!view.has_active_text_input());
            assert!(view.operation_notice.is_none());
            assert_eq!(selected_names(view), vec!["b.txt"]);
        });
    }

    #[gpui::test]
    fn clicking_inside_search_keeps_it_active(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        cx.update(|window, app| {
            view.update(app, |view, cx| {
                assert!(view.start_search_edit(window, cx));
                cx.notify();
            });
        });

        click_selector(cx, "search-bar");

        cx.read_entity(&view, |view, _| assert!(view.search_is_editing()));
    }

    #[gpui::test]
    fn clicking_inside_address_keeps_it_active(cx: &mut TestAppContext) {
        let (_temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        cx.update(|window, app| {
            view.update(app, |view, cx| {
                assert!(view.start_address_bar_edit(window, cx));
                cx.notify();
            });
        });

        click_selector(cx, "directory-bar-input");

        cx.read_entity(&view, |view, _| assert!(view.address_bar_is_editing()));
    }

    #[gpui::test]
    fn clicking_inside_rename_keeps_it_active(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.select_single_path(&temp.path().join("a.txt"));
                assert!(view.start_rename_selected(window, cx));
                cx.notify();
            });
        });

        click_selector(cx, "rename-input");

        cx.read_entity(&view, |view, _| assert!(view.active_rename.is_some()));
    }

    #[gpui::test]
    fn invalid_rename_submitted_with_enter_stays_active_and_reports_error(cx: &mut TestAppContext) {
        let (temp, tabs, cx) = test_tabs_with_two_files(cx);
        let view = active_test_view(&tabs, cx);
        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.select_single_path(&temp.path().join("a.txt"));
                assert!(view.start_rename_selected(window, cx));
                let rename = view.active_rename.as_mut().unwrap();
                rename.content.clear();
                rename.selected_range = 0..0;
                cx.notify();
            });
        });

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.handle_rename_commit(&RenameCommit, window, cx);
            });
        });

        assert!(temp.path().join("a.txt").exists());
        cx.read_entity(&view, |view, _| {
            assert!(view.active_rename.is_some());
            assert_eq!(
                view.operation_notice
                    .as_ref()
                    .map(|notice| notice.text.as_str()),
                Some("The file name cannot be empty.")
            );
        });
    }

    #[test]
    fn pane_tree_inserts_on_each_requested_edge() {
        let cases = [
            (
                SplitDirection::Left,
                SplitAxis::Horizontal,
                vec![PaneId(2), PaneId(1)],
            ),
            (
                SplitDirection::Right,
                SplitAxis::Horizontal,
                vec![PaneId(1), PaneId(2)],
            ),
            (
                SplitDirection::Up,
                SplitAxis::Vertical,
                vec![PaneId(2), PaneId(1)],
            ),
            (
                SplitDirection::Down,
                SplitAxis::Vertical,
                vec![PaneId(1), PaneId(2)],
            ),
        ];

        for (direction, expected_axis, expected_order) in cases {
            let mut tree = PaneNode::Leaf(PaneId(1));
            assert!(tree.insert_split(PaneId(1), PaneId(2), direction, 11));
            assert_eq!(tree.split_ratio(11), Some((expected_axis, 0.5)));
            let mut order = Vec::new();
            tree.pane_ids(&mut order);
            assert_eq!(order, expected_order);
        }
    }

    #[test]
    fn pane_tree_swaps_leaf_ids_without_changing_split_ratios() {
        let mut tree = PaneNode::Leaf(PaneId(1));
        assert!(tree.insert_split(PaneId(1), PaneId(2), SplitDirection::Right, 1));
        assert!(tree.insert_split(PaneId(2), PaneId(3), SplitDirection::Down, 2));
        assert!(tree.set_ratio(1, 0.35));
        assert!(tree.set_ratio(2, 0.7));

        let adjacent = tree
            .adjacent_pane(PaneId(1), SplitDirection::Right)
            .expect("right-hand pane");
        assert_eq!(adjacent, PaneId(2));
        assert!(tree.swap_panes(PaneId(1), adjacent));
        assert_eq!(tree.split_ratio(1), Some((SplitAxis::Horizontal, 0.35)));
        assert_eq!(tree.split_ratio(2), Some((SplitAxis::Vertical, 0.7)));

        let mut order = Vec::new();
        tree.pane_ids(&mut order);
        assert_eq!(order, vec![PaneId(2), PaneId(1), PaneId(3)]);
    }

    #[test]
    fn pane_tree_moves_a_leaf_to_a_new_outer_edge_and_preserves_the_remaining_subtree() {
        let mut tree = PaneNode::Leaf(PaneId(1));
        assert!(tree.insert_split(PaneId(1), PaneId(2), SplitDirection::Right, 1));
        assert!(tree.insert_split(PaneId(2), PaneId(3), SplitDirection::Down, 2));
        assert!(tree.set_ratio(2, 0.3));

        assert!(tree.move_pane_to_outer_edge(PaneId(1), SplitDirection::Down, 3));
        assert_eq!(tree.split_ratio(3), Some((SplitAxis::Vertical, 0.5)));
        assert_eq!(tree.split_ratio(2), Some((SplitAxis::Vertical, 0.3)));
        assert_eq!(tree.split_ratio(1), None);

        let mut order = Vec::new();
        tree.pane_ids(&mut order);
        assert_eq!(order, vec![PaneId(2), PaneId(3), PaneId(1)]);
        assert!(!tree.move_pane_to_outer_edge(PaneId(1), SplitDirection::Down, 4));
    }

    #[test]
    fn keyboard_split_size_check_matches_the_drag_split_minimums() {
        let bounds = Bounds {
            origin: gpui::point(px(0.0), px(0.0)),
            size: gpui::size(px(321.0), px(241.0)),
        };
        assert!(pane_bounds_allow_split(bounds, SplitDirection::Left));
        assert!(pane_bounds_allow_split(bounds, SplitDirection::Down));

        let too_narrow = Bounds {
            size: gpui::size(px(320.0), bounds.size.height),
            ..bounds
        };
        let too_short = Bounds {
            size: gpui::size(bounds.size.width, px(240.0)),
            ..bounds
        };
        assert!(!pane_bounds_allow_split(too_narrow, SplitDirection::Right));
        assert!(!pane_bounds_allow_split(too_short, SplitDirection::Up));
    }

    #[test]
    fn self_dock_keeps_the_current_pane_on_the_previewed_edge() {
        let cases = [
            (
                SplitDirection::Left,
                SplitAxis::Horizontal,
                vec![PaneId(1), PaneId(2)],
            ),
            (
                SplitDirection::Right,
                SplitAxis::Horizontal,
                vec![PaneId(2), PaneId(1)],
            ),
            (
                SplitDirection::Up,
                SplitAxis::Vertical,
                vec![PaneId(1), PaneId(2)],
            ),
            (
                SplitDirection::Down,
                SplitAxis::Vertical,
                vec![PaneId(2), PaneId(1)],
            ),
        ];

        for (direction, expected_axis, expected_order) in cases {
            let mut tree = PaneNode::Leaf(PaneId(1));
            assert!(tree.insert_split(PaneId(1), PaneId(2), direction.opposite(), 1));
            let mut order = Vec::new();
            tree.pane_ids(&mut order);
            assert_eq!(order, expected_order);
            assert_eq!(tree.split_ratio(1), Some((expected_axis, 0.5)));
        }
    }

    #[test]
    fn nested_pane_tree_preserves_spatial_order_ratios_and_collapses() {
        let mut tree = PaneNode::Leaf(PaneId(1));
        assert!(tree.insert_split(PaneId(1), PaneId(2), SplitDirection::Right, 1));
        assert!(tree.insert_split(PaneId(1), PaneId(3), SplitDirection::Down, 2));
        assert!(tree.insert_split(PaneId(2), PaneId(4), SplitDirection::Down, 3));
        assert_eq!(tree.pane_count(), 4);

        let mut order = Vec::new();
        tree.pane_ids(&mut order);
        assert_eq!(order, vec![PaneId(1), PaneId(3), PaneId(2), PaneId(4)]);

        assert!(tree.set_ratio(2, 0.35));
        assert_eq!(tree.split_ratio(2), Some((SplitAxis::Vertical, 0.35)));
        assert!(!tree.set_ratio(99, 0.7));

        assert!(tree.remove(PaneId(3)));
        assert_eq!(tree.pane_count(), 3);
        assert!(!tree.contains(PaneId(3)));
        assert!(tree.remove(PaneId(1)));
        assert_eq!(tree.pane_count(), 2);
        let mut collapsed_order = Vec::new();
        tree.pane_ids(&mut collapsed_order);
        assert_eq!(collapsed_order, vec![PaneId(2), PaneId(4)]);
    }

    #[test]
    fn deeply_nested_pane_tree_has_no_fixed_count_limit_and_collapses() {
        const PANE_COUNT: u64 = 16;
        let mut tree = PaneNode::Leaf(PaneId(1));
        for id in 2..=PANE_COUNT {
            assert!(tree.insert_split(PaneId(1), PaneId(id), SplitDirection::Right, id - 1,));
        }

        assert_eq!(tree.pane_count(), PANE_COUNT as usize);
        let mut order = Vec::new();
        tree.pane_ids(&mut order);
        let expected = std::iter::once(PaneId(1))
            .chain((2..=PANE_COUNT).rev().map(PaneId))
            .collect::<Vec<_>>();
        assert_eq!(order, expected);
        assert_eq!(tree.normalized_rects().len(), PANE_COUNT as usize);
        assert_eq!(
            tree.adjacent_pane(PaneId(1), SplitDirection::Right),
            Some(PaneId(PANE_COUNT))
        );

        for id in 2..=PANE_COUNT {
            assert!(tree.remove(PaneId(id)));
        }
        assert_eq!(tree, PaneNode::Leaf(PaneId(1)));
    }

    #[test]
    fn directional_pane_navigation_follows_spatial_geometry_without_wrapping() {
        let mut quarters = PaneNode::Leaf(PaneId(1));
        assert!(quarters.insert_split(PaneId(1), PaneId(2), SplitDirection::Right, 1));
        assert!(quarters.insert_split(PaneId(1), PaneId(3), SplitDirection::Down, 2));
        assert!(quarters.insert_split(PaneId(2), PaneId(4), SplitDirection::Down, 3));

        assert_eq!(
            quarters.adjacent_pane(PaneId(1), SplitDirection::Right),
            Some(PaneId(2))
        );
        assert_eq!(
            quarters.adjacent_pane(PaneId(1), SplitDirection::Down),
            Some(PaneId(3))
        );
        assert_eq!(
            quarters.adjacent_pane(PaneId(3), SplitDirection::Right),
            Some(PaneId(4))
        );
        assert_eq!(
            quarters.adjacent_pane(PaneId(4), SplitDirection::Up),
            Some(PaneId(2))
        );
        assert_eq!(
            quarters.adjacent_pane(PaneId(1), SplitDirection::Left),
            None
        );
        assert_eq!(quarters.adjacent_pane(PaneId(2), SplitDirection::Up), None);
    }

    #[test]
    fn directional_pane_navigation_prefers_nearest_perpendicular_center_then_visual_order() {
        let mut uneven = PaneNode::Leaf(PaneId(1));
        assert!(uneven.insert_split(PaneId(1), PaneId(2), SplitDirection::Right, 1));
        assert!(uneven.insert_split(PaneId(2), PaneId(3), SplitDirection::Down, 2));
        assert!(uneven.set_ratio(1, 0.35));
        assert!(uneven.set_ratio(2, 0.25));

        assert_eq!(
            uneven.adjacent_pane(PaneId(1), SplitDirection::Right),
            Some(PaneId(3))
        );

        assert!(uneven.set_ratio(2, 0.5));
        assert_eq!(
            uneven.adjacent_pane(PaneId(1), SplitDirection::Right),
            Some(PaneId(2))
        );
    }

    #[test]
    fn focused_pane_outline_requires_the_opt_in_setting() {
        assert!(!pane_focus_outline_visible(true, true, false));
        assert!(pane_focus_outline_visible(true, true, true));
        assert!(!pane_focus_outline_visible(false, true, true));
        assert!(!pane_focus_outline_visible(true, false, true));
    }

    #[test]
    fn split_drop_detection_is_edge_only_and_respects_minimum_size() {
        let bounds = Bounds {
            origin: gpui::point(px(100.0), px(50.0)),
            size: gpui::size(px(640.0), px(480.0)),
        };
        assert_eq!(
            split_direction_for_position(bounds, gpui::point(px(105.0), px(250.0))),
            Some(SplitDirection::Left)
        );
        assert_eq!(
            split_direction_for_position(bounds, gpui::point(px(735.0), px(250.0))),
            Some(SplitDirection::Right)
        );
        assert_eq!(
            split_direction_for_position(bounds, gpui::point(px(400.0), px(55.0))),
            Some(SplitDirection::Up)
        );
        assert_eq!(
            split_direction_for_position(bounds, gpui::point(px(400.0), px(525.0))),
            Some(SplitDirection::Down)
        );
        assert_eq!(split_direction_for_position(bounds, bounds.center()), None);

        let too_narrow = Bounds {
            origin: gpui::point(px(0.0), px(0.0)),
            size: gpui::size(px(319.0), px(480.0)),
        };
        assert_eq!(
            split_direction_for_position(too_narrow, gpui::point(px(1.0), px(240.0))),
            None
        );
        assert_eq!(
            split_direction_for_position(too_narrow, gpui::point(px(160.0), px(1.0))),
            Some(SplitDirection::Up)
        );
    }

    #[test]
    fn split_drop_detection_uses_one_third_axis_specific_bands_without_a_cap() {
        let bounds = Bounds {
            origin: gpui::point(px(0.0), px(0.0)),
            size: gpui::size(px(900.0), px(600.0)),
        };
        assert_eq!(
            split_direction_for_position(bounds, gpui::point(px(300.0), px(300.0))),
            Some(SplitDirection::Left)
        );
        assert_eq!(
            split_direction_for_position(bounds, gpui::point(px(300.1), px(300.0))),
            None
        );
        assert_eq!(
            split_direction_for_position(bounds, gpui::point(px(450.0), px(200.0))),
            Some(SplitDirection::Up)
        );
        assert_eq!(
            split_direction_for_position(bounds, gpui::point(px(450.0), px(200.1))),
            None
        );
        assert_eq!(
            split_direction_for_position(bounds, gpui::point(px(100.0), px(50.0))),
            Some(SplitDirection::Up)
        );

        let wide = Bounds {
            origin: gpui::point(px(0.0), px(0.0)),
            size: gpui::size(px(3000.0), px(600.0)),
        };
        assert_eq!(
            split_direction_for_position(wide, gpui::point(px(1000.0), px(300.0))),
            Some(SplitDirection::Left)
        );
        assert_eq!(
            split_direction_for_position(wide, gpui::point(px(1000.1), px(300.0))),
            None
        );

        let tall = Bounds {
            origin: gpui::point(px(0.0), px(0.0)),
            size: gpui::size(px(600.0), px(3000.0)),
        };
        assert_eq!(
            split_direction_for_position(tall, gpui::point(px(300.0), px(1000.0))),
            Some(SplitDirection::Up)
        );
        assert_eq!(
            split_direction_for_position(tall, gpui::point(px(300.0), px(1000.1))),
            None
        );

        let exact_minimum = Bounds {
            origin: gpui::point(px(0.0), px(0.0)),
            size: gpui::size(px(321.0), px(241.0)),
        };
        assert_eq!(
            split_direction_for_position(exact_minimum, gpui::point(px(107.0), px(120.5))),
            Some(SplitDirection::Left)
        );
        assert_eq!(
            split_direction_for_position(exact_minimum, gpui::point(px(160.5), px(80.0))),
            Some(SplitDirection::Up)
        );

        let too_short = Bounds {
            origin: gpui::point(px(0.0), px(0.0)),
            size: gpui::size(px(640.0), px(240.0)),
        };
        assert_eq!(
            split_direction_for_position(too_short, gpui::point(px(320.0), px(1.0))),
            None
        );
    }

    #[gpui::test]
    fn split_dock_target_survives_later_out_of_bounds_capture_listeners(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a", "b"]);
        let dragged_path = temp.path().join("b");

        let (workspace_tab, first_pane, second_pane, dragged_tab) = cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                tabs.add_background_tab(temp.path().join("a"), window, cx);
                tabs.add_background_tab(dragged_path.clone(), window, cx);
                let workspace_tab = tabs.tabs[0].id;
                let first_pane = tabs.tabs[0].active_pane;
                let second_pane = tabs.tabs[1].active_pane;
                let split_source = tabs.tabs[1].id;
                let dragged_tab = tabs.tabs[2].id;
                assert!(tabs.split_tab_into_pane(
                    split_source,
                    workspace_tab,
                    first_pane,
                    SplitDirection::Right,
                    window,
                    cx,
                ));
                (workspace_tab, first_pane, second_pane, dragged_tab)
            })
        });
        let drag = TabDrag {
            id: dragged_tab,
            label: "Dragged".into(),
            path: dragged_path,
            is_active: false,
            dockable: true,
        };
        let left = Bounds {
            origin: gpui::point(px(0.0), px(0.0)),
            size: gpui::size(px(400.0), px(600.0)),
        };
        let right = Bounds {
            origin: gpui::point(px(401.0), px(0.0)),
            size: gpui::size(px(400.0), px(600.0)),
        };
        let upper = Bounds {
            origin: gpui::point(px(0.0), px(0.0)),
            size: gpui::size(px(800.0), px(300.0)),
        };
        let lower = Bounds {
            origin: gpui::point(px(0.0), px(301.0)),
            size: gpui::size(px(800.0), px(300.0)),
        };

        cx.update(|_, app| {
            tabs.update(app, |tabs, _| {
                let left_edge = gpui::point(px(1.0), px(300.0));
                assert!(
                    tabs.update_dock_target(workspace_tab, first_pane, &drag, left, left_edge,)
                );
                assert!(!tabs.update_dock_target(
                    workspace_tab,
                    second_pane,
                    &drag,
                    right,
                    left_edge,
                ));
                assert_eq!(
                    tabs.dock_target,
                    Some(DockTarget {
                        workspace_tab,
                        pane: first_pane,
                        direction: SplitDirection::Left,
                    })
                );

                let right_edge = gpui::point(px(800.0), px(300.0));
                assert!(tabs.update_dock_target(
                    workspace_tab,
                    first_pane,
                    &drag,
                    left,
                    right_edge,
                ));
                assert!(tabs.update_dock_target(
                    workspace_tab,
                    second_pane,
                    &drag,
                    right,
                    right_edge,
                ));
                assert_eq!(
                    tabs.dock_target,
                    Some(DockTarget {
                        workspace_tab,
                        pane: second_pane,
                        direction: SplitDirection::Right,
                    })
                );

                let upper_edge = gpui::point(px(400.0), px(1.0));
                assert!(tabs.update_dock_target(
                    workspace_tab,
                    first_pane,
                    &drag,
                    upper,
                    upper_edge,
                ));
                assert!(!tabs.update_dock_target(
                    workspace_tab,
                    second_pane,
                    &drag,
                    lower,
                    upper_edge,
                ));
                assert_eq!(
                    tabs.dock_target,
                    Some(DockTarget {
                        workspace_tab,
                        pane: first_pane,
                        direction: SplitDirection::Up,
                    })
                );

                let lower_edge = gpui::point(px(400.0), px(600.0));
                assert!(tabs.update_dock_target(
                    workspace_tab,
                    first_pane,
                    &drag,
                    upper,
                    lower_edge,
                ));
                assert!(tabs.update_dock_target(
                    workspace_tab,
                    second_pane,
                    &drag,
                    lower,
                    lower_edge,
                ));
                assert_eq!(
                    tabs.dock_target,
                    Some(DockTarget {
                        workspace_tab,
                        pane: second_pane,
                        direction: SplitDirection::Down,
                    })
                );

                let center = upper.center();
                assert!(tabs.update_dock_target(workspace_tab, first_pane, &drag, upper, center,));
                assert!(
                    !tabs.update_dock_target(workspace_tab, second_pane, &drag, lower, center,)
                );
                assert!(tabs.dock_target.is_none());

                let outside = gpui::point(px(900.0), px(700.0));
                tabs.dock_target = Some(DockTarget {
                    workspace_tab,
                    pane: first_pane,
                    direction: SplitDirection::Left,
                });
                assert!(tabs.update_dock_target(workspace_tab, first_pane, &drag, left, outside,));
                assert!(!tabs.update_dock_target(
                    workspace_tab,
                    second_pane,
                    &drag,
                    right,
                    outside,
                ));
                assert!(tabs.dock_target.is_none());
            });
        });
    }

    #[gpui::test]
    fn nested_split_dock_target_is_owned_by_the_containing_leaf(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a", "b", "c"]);
        let dragged_path = temp.path().join("c");

        let (workspace_tab, top_left, bottom_left, right, dragged_tab) =
            cx.update(|window, app| {
                tabs.update(app, |tabs, cx| {
                    for name in ["a", "b", "c"] {
                        tabs.add_background_tab(temp.path().join(name), window, cx);
                    }
                    let workspace_tab = tabs.tabs[0].id;
                    let top_left = tabs.tabs[0].active_pane;
                    let right = tabs.tabs[1].active_pane;
                    let bottom_left = tabs.tabs[2].active_pane;
                    let right_source = tabs.tabs[1].id;
                    let bottom_source = tabs.tabs[2].id;
                    let dragged_tab = tabs.tabs[3].id;
                    assert!(tabs.split_tab_into_pane(
                        right_source,
                        workspace_tab,
                        top_left,
                        SplitDirection::Right,
                        window,
                        cx,
                    ));
                    assert!(tabs.split_tab_into_pane(
                        bottom_source,
                        workspace_tab,
                        top_left,
                        SplitDirection::Down,
                        window,
                        cx,
                    ));
                    (workspace_tab, top_left, bottom_left, right, dragged_tab)
                })
            });
        let drag = TabDrag {
            id: dragged_tab,
            label: "Dragged".into(),
            path: dragged_path,
            is_active: false,
            dockable: true,
        };
        let leaves = [
            (
                top_left,
                Bounds {
                    origin: gpui::point(px(0.0), px(0.0)),
                    size: gpui::size(px(400.0), px(300.0)),
                },
            ),
            (
                bottom_left,
                Bounds {
                    origin: gpui::point(px(0.0), px(301.0)),
                    size: gpui::size(px(400.0), px(300.0)),
                },
            ),
            (
                right,
                Bounds {
                    origin: gpui::point(px(401.0), px(0.0)),
                    size: gpui::size(px(400.0), px(601.0)),
                },
            ),
        ];

        cx.update(|_, app| {
            tabs.update(app, |tabs, _| {
                for (expected_pane, position, expected_direction) in [
                    (
                        top_left,
                        gpui::point(px(1.0), px(150.0)),
                        SplitDirection::Left,
                    ),
                    (
                        bottom_left,
                        gpui::point(px(200.0), px(600.0)),
                        SplitDirection::Down,
                    ),
                    (
                        right,
                        gpui::point(px(800.0), px(300.0)),
                        SplitDirection::Right,
                    ),
                ] {
                    for (pane, bounds) in leaves {
                        tabs.update_dock_target(workspace_tab, pane, &drag, bounds, position);
                    }
                    assert_eq!(
                        tabs.dock_target,
                        Some(DockTarget {
                            workspace_tab,
                            pane: expected_pane,
                            direction: expected_direction,
                        })
                    );
                }
            });
        });
    }

    #[gpui::test]
    fn standalone_tabs_combine_and_focus_the_dropped_pane(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a"]);
        let dropped_path = temp.path().join("a");

        cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                tabs.add_background_tab(dropped_path.clone(), window, cx);
                let target = tabs.tabs[0].id;
                let target_pane = tabs.tabs[0].active_pane;
                let source = tabs.tabs[1].id;
                assert!(tabs.split_tab_into_pane(
                    source,
                    target,
                    target_pane,
                    SplitDirection::Right,
                    window,
                    cx,
                ));
                cx.notify();
            });
        });
        cx.run_until_parked();

        cx.read_entity(&tabs, |tabs, cx| {
            assert_eq!(tabs.tabs.len(), 1);
            let tab = &tabs.tabs[0];
            assert_eq!(tab.layout.pane_count(), 2);
            assert_eq!(tab.active_view().read(cx).path(), dropped_path.as_path());
            assert!(tab.is_split());
        });
        assert!(cx.debug_bounds("explorer-tab-split-count-2").is_some());

        let target_position = cx
            .debug_bounds("explorer-pane-1")
            .expect("target pane bounds")
            .center();
        cx.simulate_click(target_position, Modifiers::default());
        cx.read_entity(&tabs, |tabs, cx| {
            assert_eq!(
                tabs.active_tab().unwrap().active_view().read(cx).path(),
                temp.path()
            );
        });
    }

    #[gpui::test]
    fn keyboard_split_creates_a_focused_startup_pane_and_preserves_other_tabs(
        cx: &mut TestAppContext,
    ) {
        let temp = TempDir::new();
        let current_path = temp.path().join("current");
        let startup_path = temp.path().join("startup");
        let other_path = temp.path().join("other");
        for path in [&current_path, &startup_path, &other_path] {
            fs::create_dir(path).expect("create test path");
        }
        let mut settings = ExplorerSettings::default();
        settings.app.start = startup_path.clone();
        cx.set_global(SettingsState::for_test(settings));
        let (tabs, cx) = test_tabs_at_path(cx, current_path.clone());

        let (workspace_tab, current_pane, inserted_pane, other_tab, other_entity, next_tab_id) = cx
            .update(|window, app| {
                tabs.update(app, |tabs, cx| {
                    tabs.add_background_tab(other_path.clone(), window, cx);
                    let workspace_tab = tabs.tabs[0].id;
                    let current_pane = tabs.tabs[0].active_pane;
                    let inserted_pane = PaneId(tabs.next_pane_id);
                    let other_tab = tabs.tabs[1].id;
                    let other_entity = tabs.tabs[1].active_view().entity_id();
                    let next_tab_id = tabs.next_tab_id;
                    tabs.pane_bounds.insert(
                        current_pane,
                        Bounds {
                            origin: gpui::point(px(0.0), px(0.0)),
                            size: gpui::size(px(640.0), px(480.0)),
                        },
                    );
                    assert!(tabs.split_active_pane(SplitDirection::Left, window, cx,));
                    (
                        workspace_tab,
                        current_pane,
                        inserted_pane,
                        other_tab,
                        other_entity,
                        next_tab_id,
                    )
                })
            });
        cx.run_until_parked();

        cx.read_entity(&tabs, |tabs, cx| {
            assert_eq!(tabs.tabs.len(), 2);
            assert_eq!(tabs.next_tab_id, next_tab_id);
            assert_eq!(tabs.active_tab, workspace_tab);
            let workspace = &tabs.tabs[0];
            assert_eq!(workspace.active_pane, inserted_pane);
            assert_eq!(
                workspace.layout.split_ratio(1),
                Some((SplitAxis::Horizontal, 0.5))
            );
            let mut order = Vec::new();
            workspace.layout.pane_ids(&mut order);
            assert_eq!(order, vec![inserted_pane, current_pane]);
            assert_eq!(workspace.active_view().read(cx).path(), startup_path);
            assert!(
                workspace
                    .panes
                    .iter()
                    .any(|pane| pane.view.read(cx).path() == current_path)
            );

            assert_eq!(tabs.tabs[1].id, other_tab);
            assert_eq!(tabs.tabs[1].active_view().entity_id(), other_entity);
            assert_eq!(tabs.tabs[1].active_view().read(cx).path(), other_path);
        });
        assert_active_tab_focused(&tabs, cx);
    }

    #[gpui::test]
    fn keyboard_split_rejects_a_pane_below_the_minimum_size(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &[]);

        cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                let pane = tabs.active_tab().unwrap().active_pane;
                tabs.pane_bounds.insert(
                    pane,
                    Bounds {
                        origin: gpui::point(px(0.0), px(0.0)),
                        size: gpui::size(px(320.0), px(480.0)),
                    },
                );
                assert!(!tabs.split_active_pane(SplitDirection::Right, window, cx,));
            });
        });

        cx.read_entity(&tabs, |tabs, cx| {
            assert_eq!(tabs.tabs.len(), 1);
            assert_eq!(tabs.tabs[0].layout.pane_count(), 1);
            assert_eq!(tabs.tabs[0].active_view().read(cx).path(), temp.path());
        });
    }

    #[gpui::test]
    fn self_docking_a_single_tab_uses_startup_pane_and_keeps_current_focused(
        cx: &mut TestAppContext,
    ) {
        let temp = TempDir::new();
        let current_path = temp.path().join("current");
        let startup_path = temp.path().join("startup");
        fs::create_dir(&current_path).expect("create current path");
        fs::create_dir(&startup_path).expect("create startup path");
        let mut settings = ExplorerSettings::default();
        settings.app.start = startup_path.clone();
        cx.set_global(SettingsState::for_test(settings));
        let (tabs, cx) = test_tabs_at_path(cx, current_path.clone());

        let (workspace_tab, current_pane, next_tab_id, helper_pane_id) =
            cx.read_entity(&tabs, |tabs, _| {
                let tab = tabs.active_tab().unwrap();
                (
                    tab.id,
                    tab.active_pane,
                    tabs.next_tab_id,
                    PaneId(tabs.next_pane_id),
                )
            });
        cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                let drag = TabDrag {
                    id: workspace_tab,
                    label: "Current".into(),
                    path: current_path.clone(),
                    is_active: true,
                    dockable: true,
                };
                let bounds = Bounds {
                    origin: gpui::point(px(0.0), px(0.0)),
                    size: gpui::size(px(640.0), px(480.0)),
                };
                assert!(!tabs.update_dock_target(
                    workspace_tab,
                    current_pane,
                    &drag,
                    bounds,
                    bounds.center(),
                ));
                assert!(tabs.dock_target.is_none());
                assert!(tabs.update_dock_target(
                    workspace_tab,
                    current_pane,
                    &drag,
                    bounds,
                    gpui::point(px(1.0), px(240.0)),
                ));
                assert_eq!(
                    tabs.dock_target.map(|target| target.direction),
                    Some(SplitDirection::Left)
                );
                assert!(tabs.self_dock_active_tab(
                    workspace_tab,
                    current_pane,
                    SplitDirection::Left,
                    window,
                    cx,
                ));
                cx.notify();
            });
        });
        cx.run_until_parked();

        cx.read_entity(&tabs, |tabs, cx| {
            assert_eq!(tabs.tabs.len(), 1);
            assert_eq!(tabs.next_tab_id, next_tab_id);
            let tab = tabs.active_tab().unwrap();
            assert_eq!(tab.id, workspace_tab);
            assert_eq!(tab.active_pane, current_pane);
            assert_eq!(tab.layout.pane_count(), 2);
            assert_eq!(
                tab.layout.split_ratio(1),
                Some((SplitAxis::Horizontal, 0.5))
            );
            let mut order = Vec::new();
            tab.layout.pane_ids(&mut order);
            assert_eq!(order, vec![current_pane, helper_pane_id]);
            assert_ne!(current_pane, helper_pane_id);
            let paths = tab
                .panes
                .iter()
                .map(|pane| pane.view.read(cx).path().to_path_buf())
                .collect::<Vec<_>>();
            assert!(paths.contains(&current_path));
            assert!(paths.contains(&startup_path));
            assert_eq!(tab.active_view().read(cx).path(), current_path);
        });
        assert_active_tab_focused(&tabs, cx);
        assert!(cx.debug_bounds("explorer-tab-split-count-2").is_some());
    }

    #[gpui::test]
    fn self_docking_last_tab_creates_startup_pane_and_preserves_left_tab(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let left_path = temp.path().join("left");
        let current_path = temp.path().join("current");
        let startup_path = temp.path().join("startup");
        for path in [&left_path, &current_path, &startup_path] {
            fs::create_dir(path).expect("create test path");
        }
        let mut settings = ExplorerSettings::default();
        settings.app.start = startup_path.clone();
        cx.set_global(SettingsState::for_test(settings));
        let (tabs, cx) = test_tabs_at_path(cx, left_path.clone());

        let (left_tab, left_entity, workspace_tab, current_pane, next_tab_id) =
            cx.update(|window, app| {
                tabs.update(app, |tabs, cx| {
                    let left_tab = tabs.tabs[0].id;
                    let left_entity = tabs.tabs[0].active_view().entity_id();
                    tabs.add_background_tab(current_path.clone(), window, cx);
                    let workspace_tab = tabs.tabs[1].id;
                    let current_pane = tabs.tabs[1].active_pane;
                    let next_tab_id = tabs.next_tab_id;
                    tabs.activate_tab(workspace_tab, window, cx);
                    assert!(tabs.self_dock_active_tab(
                        workspace_tab,
                        current_pane,
                        SplitDirection::Right,
                        window,
                        cx,
                    ));
                    (
                        left_tab,
                        left_entity,
                        workspace_tab,
                        current_pane,
                        next_tab_id,
                    )
                })
            });
        cx.run_until_parked();

        cx.read_entity(&tabs, |tabs, cx| {
            assert_eq!(tabs.tabs.len(), 2);
            assert_eq!(tabs.next_tab_id, next_tab_id);
            assert_eq!(tabs.tabs[0].id, left_tab);
            assert_eq!(tabs.tabs[0].active_view().entity_id(), left_entity);
            assert_eq!(tabs.tabs[0].active_view().read(cx).path(), left_path);
            let tab = &tabs.tabs[1];
            assert_eq!(tab.id, workspace_tab);
            assert_eq!(tab.active_pane, current_pane);
            let mut order = Vec::new();
            tab.layout.pane_ids(&mut order);
            assert_eq!(order.last(), Some(&current_pane));
            assert!(
                tab.panes
                    .iter()
                    .any(|pane| pane.view.read(cx).path() == startup_path)
            );
            assert!(
                !tab.panes
                    .iter()
                    .any(|pane| pane.view.entity_id() == left_entity)
            );
            assert_eq!(tab.active_view().read(cx).path(), current_path);
            assert!(tabs.background_operation_tabs.is_empty());
        });
        assert_active_tab_focused(&tabs, cx);
    }

    #[gpui::test]
    fn first_tab_self_dock_uses_startup_and_leaves_right_tab_untouched(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let current_path = temp.path().join("current");
        let right_path = temp.path().join("right");
        let startup_path = temp.path().join("startup");
        for path in [&current_path, &right_path, &startup_path] {
            fs::create_dir(path).expect("create test path");
        }
        let mut settings = ExplorerSettings::default();
        settings.app.start = startup_path.clone();
        cx.set_global(SettingsState::for_test(settings));
        let (tabs, cx) = test_tabs_at_path(cx, current_path.clone());

        let (workspace_tab, right_tab, right_entity, next_tab_id) = cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                tabs.add_background_tab(right_path.clone(), window, cx);
                let workspace_tab = tabs.tabs[0].id;
                let current_pane = tabs.tabs[0].active_pane;
                let right_tab = tabs.tabs[1].id;
                let right_entity = tabs.tabs[1].active_view().entity_id();
                let next_tab_id = tabs.next_tab_id;
                assert!(tabs.self_dock_active_tab(
                    workspace_tab,
                    current_pane,
                    SplitDirection::Down,
                    window,
                    cx,
                ));
                (workspace_tab, right_tab, right_entity, next_tab_id)
            })
        });

        cx.read_entity(&tabs, |tabs, cx| {
            assert_eq!(tabs.tabs.len(), 2);
            assert_eq!(tabs.next_tab_id, next_tab_id);
            assert_eq!(tabs.tabs[0].id, workspace_tab);
            assert!(tabs.tabs[0].is_split());
            assert!(
                tabs.tabs[0]
                    .panes
                    .iter()
                    .any(|pane| pane.view.read(cx).path() == startup_path)
            );
            assert_eq!(tabs.tabs[1].id, right_tab);
            assert_eq!(tabs.tabs[1].active_view().entity_id(), right_entity);
            assert_eq!(tabs.tabs[1].active_view().read(cx).path(), right_path);
        });
    }

    #[gpui::test]
    fn middle_tab_self_dock_uses_startup_and_preserves_surrounding_tabs(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let far_left_path = temp.path().join("far-left");
        let composite_path = temp.path().join("composite");
        let composite_helper_path = temp.path().join("composite-helper");
        let current_path = temp.path().join("current");
        let right_path = temp.path().join("right");
        let startup_path = temp.path().join("startup");
        for path in [
            &far_left_path,
            &composite_path,
            &composite_helper_path,
            &current_path,
            &right_path,
            &startup_path,
        ] {
            fs::create_dir(path).expect("create test path");
        }
        let mut settings = ExplorerSettings::default();
        settings.app.start = startup_path.clone();
        cx.set_global(SettingsState::for_test(settings));
        let (tabs, cx) = test_tabs_at_path(cx, far_left_path.clone());

        let (
            far_left_tab,
            far_left_entity,
            composite_tab,
            composite_entities,
            workspace_tab,
            right_tab,
            right_entity,
            next_tab_id,
        ) = cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                tabs.add_background_tab(composite_path.clone(), window, cx);
                tabs.add_background_tab(composite_helper_path.clone(), window, cx);
                tabs.add_background_tab(current_path.clone(), window, cx);
                tabs.add_background_tab(right_path.clone(), window, cx);
                let far_left_tab = tabs.tabs[0].id;
                let far_left_entity = tabs.tabs[0].active_view().entity_id();
                let composite_tab = tabs.tabs[1].id;
                let composite_pane = tabs.tabs[1].active_pane;
                let composite_helper_tab = tabs.tabs[2].id;
                let workspace_tab = tabs.tabs[3].id;
                let right_tab = tabs.tabs[4].id;
                let right_entity = tabs.tabs[4].active_view().entity_id();
                assert!(tabs.split_tab_into_pane(
                    composite_helper_tab,
                    composite_tab,
                    composite_pane,
                    SplitDirection::Right,
                    window,
                    cx,
                ));
                let composite_entities = tabs.tabs[1]
                    .panes
                    .iter()
                    .map(|pane| pane.view.entity_id())
                    .collect::<Vec<_>>();
                let next_tab_id = tabs.next_tab_id;
                tabs.activate_tab(workspace_tab, window, cx);
                let current_pane = tabs.active_tab().unwrap().active_pane;
                assert!(tabs.self_dock_active_tab(
                    workspace_tab,
                    current_pane,
                    SplitDirection::Up,
                    window,
                    cx,
                ));
                (
                    far_left_tab,
                    far_left_entity,
                    composite_tab,
                    composite_entities,
                    workspace_tab,
                    right_tab,
                    right_entity,
                    next_tab_id,
                )
            })
        });

        cx.read_entity(&tabs, |tabs, cx| {
            assert_eq!(tabs.tabs.len(), 4);
            assert_eq!(tabs.next_tab_id, next_tab_id);
            assert_eq!(tabs.tabs[0].id, far_left_tab);
            assert_eq!(tabs.tabs[0].active_view().entity_id(), far_left_entity);
            assert!(!tabs.tabs[0].is_split());
            assert_eq!(tabs.tabs[0].active_view().read(cx).path(), far_left_path);
            assert_eq!(tabs.tabs[1].id, composite_tab);
            assert_eq!(tabs.tabs[1].layout.pane_count(), 2);
            assert_eq!(
                tabs.tabs[1]
                    .panes
                    .iter()
                    .map(|pane| pane.view.entity_id())
                    .collect::<Vec<_>>(),
                composite_entities
            );
            assert_eq!(tabs.tabs[2].id, workspace_tab);
            assert_eq!(tabs.tabs[2].layout.pane_count(), 2);
            assert!(
                tabs.tabs[2]
                    .panes
                    .iter()
                    .any(|pane| pane.view.read(cx).path() == startup_path)
            );
            assert_eq!(tabs.tabs[3].id, right_tab);
            assert_eq!(tabs.tabs[3].active_view().entity_id(), right_entity);
            assert_eq!(tabs.tabs[3].active_view().read(cx).path(), right_path);
        });
    }

    #[gpui::test]
    fn hosted_address_suggestions_render_below_the_shared_address_input(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["child"]);
        let view = active_test_view(&tabs, cx);

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                assert!(view.start_address_bar_edit(window, cx));
                view.active_address_bar
                    .as_mut()
                    .expect("active address edit")
                    .suggestions = folder_suggestions_for_input("", temp.path(), true);
                cx.notify();
            });
        });
        cx.run_until_parked();

        let input = cx
            .debug_bounds("directory-bar-input")
            .expect("shared address input bounds");
        let suggestion = cx
            .debug_bounds("address-suggestion-0")
            .expect("shared address suggestion bounds");
        assert!(suggestion.origin.y >= input.bottom());
    }

    #[gpui::test]
    fn pane_move_actions_swap_then_reorient_and_keep_the_same_pane_focused(
        cx: &mut TestAppContext,
    ) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a"]);

        let (left_pane, right_pane) = cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                tabs.add_background_tab(temp.path().join("a"), window, cx);
                let target = tabs.tabs[0].id;
                let left_pane = tabs.tabs[0].active_pane;
                let source = tabs.tabs[1].id;
                let right_pane = tabs.tabs[1].active_pane;
                assert!(tabs.split_tab_into_pane(
                    source,
                    target,
                    left_pane,
                    SplitDirection::Right,
                    window,
                    cx,
                ));
                (left_pane, right_pane)
            })
        });
        cx.run_until_parked();

        cx.dispatch_action(MovePaneLeft);
        cx.run_until_parked();
        cx.read_entity(&tabs, |tabs, _| {
            let tab = tabs.active_tab().unwrap();
            assert_eq!(tab.active_pane, right_pane);
            let mut order = Vec::new();
            tab.layout.pane_ids(&mut order);
            assert_eq!(order, vec![right_pane, left_pane]);
        });

        cx.dispatch_action(MovePaneLeft);
        cx.run_until_parked();
        cx.read_entity(&tabs, |tabs, _| {
            let tab = tabs.active_tab().unwrap();
            let mut order = Vec::new();
            tab.layout.pane_ids(&mut order);
            assert_eq!(order, vec![right_pane, left_pane]);
        });

        cx.dispatch_action(MovePaneDown);
        cx.run_until_parked();
        cx.read_entity(&tabs, |tabs, _| {
            let tab = tabs.active_tab().unwrap();
            assert_eq!(tab.active_pane, right_pane);
            assert_eq!(tab.layout.split_ratio(2), Some((SplitAxis::Vertical, 0.5)));
            let mut order = Vec::new();
            tab.layout.pane_ids(&mut order);
            assert_eq!(order, vec![left_pane, right_pane]);
        });
        assert_active_tab_focused(&tabs, cx);
    }

    #[gpui::test]
    fn pane_focus_actions_move_spatially_without_changing_layout_and_stop_at_outer_edges(
        cx: &mut TestAppContext,
    ) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a"]);

        let (left_pane, right_pane, original_layout) = cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                tabs.add_background_tab(temp.path().join("a"), window, cx);
                let target = tabs.tabs[0].id;
                let left_pane = tabs.tabs[0].active_pane;
                let source = tabs.tabs[1].id;
                let right_pane = tabs.tabs[1].active_pane;
                assert!(tabs.split_tab_into_pane(
                    source,
                    target,
                    left_pane,
                    SplitDirection::Right,
                    window,
                    cx,
                ));
                (left_pane, right_pane, tabs.tabs[0].layout.clone())
            })
        });
        cx.run_until_parked();

        cx.dispatch_action(FocusPaneLeft);
        cx.run_until_parked();
        cx.read_entity(&tabs, |tabs, _| {
            let tab = tabs.active_tab().unwrap();
            assert_eq!(tab.active_pane, left_pane);
            assert_eq!(tab.layout, original_layout);
        });

        cx.dispatch_action(FocusPaneLeft);
        cx.run_until_parked();
        cx.read_entity(&tabs, |tabs, _| {
            let tab = tabs.active_tab().unwrap();
            assert_eq!(tab.active_pane, left_pane);
            assert_eq!(tab.layout, original_layout);
        });

        cx.dispatch_action(FocusPaneRight);
        cx.run_until_parked();
        cx.read_entity(&tabs, |tabs, _| {
            let tab = tabs.active_tab().unwrap();
            assert_eq!(tab.active_pane, right_pane);
            assert_eq!(tab.layout, original_layout);
        });
        assert_active_tab_focused(&tabs, cx);
    }

    #[gpui::test]
    fn split_workspace_accepts_many_unique_panes_and_unsplits_in_visual_order(
        cx: &mut TestAppContext,
    ) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &names);
        let paths = names.map(|name| temp.path().join(name));

        let (target, expected_paths, focused_path) = cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                for path in &paths {
                    tabs.add_background_tab(path.clone(), window, cx);
                }
                let target = tabs.tabs[0].id;
                let root_pane = tabs.tabs[0].active_pane;
                let sources = tabs.tabs[1..]
                    .iter()
                    .map(|tab| {
                        (
                            tab.id,
                            tab.active_pane,
                            tab.active_view().read(cx).path().to_path_buf(),
                        )
                    })
                    .collect::<Vec<_>>();
                let directions = [
                    SplitDirection::Right,
                    SplitDirection::Down,
                    SplitDirection::Left,
                    SplitDirection::Up,
                ];

                for (index, source) in sources.iter().enumerate() {
                    if index == 3 {
                        let drag = TabDrag {
                            id: source.0,
                            label: "Fifth pane".into(),
                            path: source.2.clone(),
                            is_active: false,
                            dockable: true,
                        };
                        let bounds = Bounds {
                            origin: gpui::point(px(0.0), px(0.0)),
                            size: gpui::size(px(640.0), px(480.0)),
                        };
                        assert!(tabs.update_dock_target(
                            target,
                            root_pane,
                            &drag,
                            bounds,
                            gpui::point(px(1.0), px(240.0)),
                        ));
                        assert_eq!(
                            tabs.dock_target,
                            Some(DockTarget {
                                workspace_tab: target,
                                pane: root_pane,
                                direction: SplitDirection::Left,
                            })
                        );
                    }

                    assert!(tabs.split_tab_into_pane(
                        source.0,
                        target,
                        root_pane,
                        directions[index % directions.len()],
                        window,
                        cx,
                    ));
                }

                let split = tabs.tabs.iter().find(|tab| tab.id == target).unwrap();
                assert_eq!(split.layout.pane_count(), paths.len() + 1);
                let mut order = Vec::new();
                split.layout.pane_ids(&mut order);
                assert_eq!(
                    order,
                    vec![
                        sources[2].1,
                        sources[3].1,
                        sources[6].1,
                        sources[7].1,
                        root_pane,
                        sources[5].1,
                        sources[4].1,
                        sources[1].1,
                        sources[0].1,
                    ]
                );
                let mut unique = order.clone();
                unique.sort_by_key(|id| id.0);
                unique.dedup();
                assert_eq!(unique.len(), paths.len() + 1);
                for split_id in 1..=paths.len() as u64 {
                    assert_eq!(split.layout.split_ratio(split_id).unwrap().1, 0.5);
                }
                assert_eq!(tabs.tabs.len(), 1);

                let expected_paths = order
                    .iter()
                    .map(|pane_id| {
                        split
                            .pane(*pane_id)
                            .unwrap()
                            .view
                            .read(cx)
                            .path()
                            .to_path_buf()
                    })
                    .collect::<Vec<_>>();
                let focused_path = split.active_view().read(cx).path().to_path_buf();
                cx.notify();
                (target, expected_paths, focused_path)
            })
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("explorer-tab-split-count-9").is_some());

        cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                assert!(tabs.unsplit_tab(target, window, cx));
            });
        });
        cx.read_entity(&tabs, |tabs, cx| {
            assert_eq!(tabs.tabs.len(), expected_paths.len());
            let restored_paths = tabs
                .tabs
                .iter()
                .map(|tab| tab.active_view().read(cx).path().to_path_buf())
                .collect::<Vec<_>>();
            assert_eq!(restored_paths, expected_paths);
            assert_eq!(
                tabs.active_tab().unwrap().active_view().read(cx).path(),
                focused_path
            );
            assert!(tabs.tabs.iter().all(|tab| !tab.is_split()));
        });
    }

    #[gpui::test]
    fn composite_source_cannot_merge_into_another_split_workspace(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a", "b", "c"]);

        cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                for name in ["a", "b", "c"] {
                    tabs.add_background_tab(temp.path().join(name), window, cx);
                }

                let target = tabs.tabs[0].id;
                let target_pane = tabs.tabs[0].active_pane;
                let target_source = tabs.tabs[1].id;
                assert!(tabs.split_tab_into_pane(
                    target_source,
                    target,
                    target_pane,
                    SplitDirection::Right,
                    window,
                    cx,
                ));

                let composite_source = tabs.tabs[1].id;
                let composite_source_pane = tabs.tabs[1].active_pane;
                let source_helper = tabs.tabs[2].id;
                assert!(tabs.split_tab_into_pane(
                    source_helper,
                    composite_source,
                    composite_source_pane,
                    SplitDirection::Down,
                    window,
                    cx,
                ));

                let tab_ids = tabs.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
                let target_layout = tabs.tabs[0].layout.clone();
                let source_layout = tabs.tabs[1].layout.clone();
                assert!(!tabs.split_tab_into_pane(
                    composite_source,
                    target,
                    target_pane,
                    SplitDirection::Left,
                    window,
                    cx,
                ));

                assert_eq!(
                    tabs.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
                    tab_ids
                );
                assert_eq!(tabs.tabs[0].layout, target_layout);
                assert_eq!(tabs.tabs[1].layout, source_layout);
                assert_eq!(tabs.tabs[0].layout.pane_count(), 2);
                assert_eq!(tabs.tabs[1].layout.pane_count(), 2);
            });
        });
    }

    #[gpui::test]
    fn divider_resize_clamps_to_minimum_pane_dimensions(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a"]);

        cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                tabs.add_background_tab(temp.path().join("a"), window, cx);
                let target = tabs.tabs[0].id;
                let target_pane = tabs.tabs[0].active_pane;
                let source = tabs.tabs[1].id;
                assert!(tabs.split_tab_into_pane(
                    source,
                    target,
                    target_pane,
                    SplitDirection::Right,
                    window,
                    cx,
                ));
                tabs.split_bounds.insert(
                    1,
                    Bounds {
                        origin: gpui::point(px(0.0), px(0.0)),
                        size: gpui::size(px(640.0), px(480.0)),
                    },
                );
                tabs.begin_split_resize(target, 1, gpui::point(px(320.0), px(0.0)));
                assert!(tabs.update_split_resize(gpui::point(px(0.0), px(0.0))));
                assert_eq!(tabs.tabs[0].layout.split_ratio(1).unwrap().1, 0.25);
                assert!(tabs.update_split_resize(gpui::point(px(640.0), px(0.0))));
                assert_eq!(tabs.tabs[0].layout.split_ratio(1).unwrap().1, 0.75);
            });
        });
    }

    #[gpui::test]
    fn unsplit_restores_visual_order_and_the_focused_pane(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a", "b"]);
        let root_path = temp.path().to_path_buf();
        let a_path = temp.path().join("a");
        let b_path = temp.path().join("b");

        cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                tabs.add_background_tab(a_path.clone(), window, cx);
                tabs.add_background_tab(b_path.clone(), window, cx);
                let target = tabs.tabs[0].id;
                let root_pane = tabs.tabs[0].active_pane;
                let a_tab = tabs.tabs[1].id;
                let a_pane = tabs.tabs[1].active_pane;
                let b_tab = tabs.tabs[2].id;
                assert!(tabs.split_tab_into_pane(
                    a_tab,
                    target,
                    root_pane,
                    SplitDirection::Right,
                    window,
                    cx,
                ));
                assert!(tabs.split_tab_into_pane(
                    b_tab,
                    target,
                    a_pane,
                    SplitDirection::Down,
                    window,
                    cx,
                ));
                assert!(tabs.unsplit_tab(target, window, cx));
            });
        });

        cx.read_entity(&tabs, |tabs, cx| {
            assert_eq!(tabs.tabs.len(), 3);
            let paths = tabs
                .tabs
                .iter()
                .map(|tab| tab.active_view().read(cx).path().to_path_buf())
                .collect::<Vec<_>>();
            assert_eq!(paths, vec![root_path, a_path, b_path.clone()]);
            assert_eq!(
                tabs.active_tab().unwrap().active_view().read(cx).path(),
                b_path
            );
            assert!(tabs.tabs.iter().all(|tab| !tab.is_split()));
        });
    }

    #[gpui::test]
    fn focused_pane_close_collapses_a_two_pane_split_to_an_ordinary_tab(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a"]);
        let remaining_path = temp.path().to_path_buf();

        cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                tabs.add_background_tab(temp.path().join("a"), window, cx);
                let target = tabs.tabs[0].id;
                let target_pane = tabs.tabs[0].active_pane;
                let source = tabs.tabs[1].id;
                assert!(tabs.split_tab_into_pane(
                    source,
                    target,
                    target_pane,
                    SplitDirection::Right,
                    window,
                    cx,
                ));
                tabs.close_focused_pane(target, window, cx);
            });
        });

        cx.read_entity(&tabs, |tabs, cx| {
            assert_eq!(tabs.tabs.len(), 1);
            assert!(!tabs.tabs[0].is_split());
            assert_eq!(tabs.tabs[0].active_view().read(cx).path(), remaining_path);
        });
    }

    #[gpui::test]
    fn tab_switching_restores_split_ratio_and_focused_pane(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a", "b"]);
        let focused_path = temp.path().join("a");

        cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                tabs.add_background_tab(focused_path.clone(), window, cx);
                tabs.add_background_tab(temp.path().join("b"), window, cx);
                let target = tabs.tabs[0].id;
                let target_pane = tabs.tabs[0].active_pane;
                let source = tabs.tabs[1].id;
                let other = tabs.tabs[2].id;
                assert!(tabs.split_tab_into_pane(
                    source,
                    target,
                    target_pane,
                    SplitDirection::Right,
                    window,
                    cx,
                ));
                assert!(tabs.tabs[0].layout.set_ratio(1, 0.37));
                let focused_pane = tabs.tabs[0].active_pane;

                tabs.activate_tab(other, window, cx);
                tabs.activate_tab(target, window, cx);

                let restored = tabs.active_tab().unwrap();
                assert_eq!(restored.active_pane, focused_pane);
                assert_eq!(restored.layout.split_ratio(1).unwrap().1, 0.37);
                assert_eq!(restored.active_view().read(cx).path(), focused_path);
            });
        });
    }

    #[gpui::test]
    fn closing_the_only_composite_creates_one_fresh_ordinary_tab(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (temp, tabs, cx) = test_tabs_with_directories(cx, &["a"]);

        cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                tabs.add_background_tab(temp.path().join("a"), window, cx);
                let target = tabs.tabs[0].id;
                let target_pane = tabs.tabs[0].active_pane;
                let source = tabs.tabs[1].id;
                assert!(tabs.split_tab_into_pane(
                    source,
                    target,
                    target_pane,
                    SplitDirection::Right,
                    window,
                    cx,
                ));
                assert_eq!(tabs.tabs.len(), 1);

                tabs.close_tab(target, window, cx);

                assert_eq!(tabs.tabs.len(), 1);
                assert!(!tabs.tabs[0].is_split());
                assert_ne!(tabs.tabs[0].id, target);
            });
        });
    }

    #[test]
    fn tab_strip_width_reserves_only_tabs_and_new_tab_button() {
        assert_eq!(tab_strip_width(0), TAB_BAR_HEIGHT);
        assert_eq!(tab_strip_width(1), TAB_WIDTH + TAB_BAR_HEIGHT);
        assert_eq!(tab_strip_width(3), (3.0 * TAB_WIDTH) + TAB_BAR_HEIGHT);
    }

    #[gpui::test]
    fn overflowing_tab_strip_scrolls_active_tab_into_view(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (_temp, tabs, cx) = test_tabs_with_files(cx, &[]);
        cx.simulate_resize(gpui::size(px(700.0), px(600.0)));

        cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                let path = tabs
                    .active_tab()
                    .unwrap()
                    .view
                    .read(cx)
                    .path()
                    .to_path_buf();
                for _ in 0..8 {
                    tabs.add_background_tab(path.clone(), window, cx);
                }
                cx.notify();
            });
        });
        cx.run_until_parked();

        let initial_offset = cx.read_entity(&tabs, |tabs, _| {
            assert!(tabs.tab_scroll_handle.max_offset().width > px(0.0));
            tabs.tab_scroll_handle.offset().x
        });

        let first_tab_position = cx.read_entity(&tabs, |tabs, _| {
            tabs.tab_scroll_handle
                .bounds_for_item(0)
                .expect("first tab bounds")
                .center()
        });
        cx.simulate_event(ScrollWheelEvent {
            position: first_tab_position,
            delta: ScrollDelta::Lines(gpui::point(0.0, -3.0)),
            ..Default::default()
        });

        cx.read_entity(&tabs, |tabs, _| {
            assert!(tabs.tab_scroll_handle.offset().x < initial_offset);
        });

        cx.update(|window, app| {
            tabs.update(app, |tabs, cx| {
                let last_index = tabs.tabs.len() - 1;
                assert!(tabs.select_tab_by_index(last_index, window, cx));
                cx.notify();
            });
        });
        cx.run_until_parked();

        cx.read_entity(&tabs, |tabs, _| {
            let handle = &tabs.tab_scroll_handle;
            let viewport = handle.bounds();
            let last_tab = handle
                .bounds_for_item(tabs.tabs.len() - 1)
                .expect("last tab bounds");

            assert!(handle.offset().x < initial_offset);
            assert!(last_tab.left() + handle.offset().x >= viewport.left());
            assert!(last_tab.right() + handle.offset().x <= viewport.right());
        });
    }

    #[test]
    fn tab_label_uses_last_path_component() {
        assert_eq!(
            tab_label_for_path(&PathBuf::from("/home/user/Downloads")),
            "Downloads"
        );
    }

    #[test]
    fn tab_label_falls_back_for_root_path() {
        let label = tab_label_for_path(&PathBuf::from("/"));

        assert!(!label.is_empty());
    }

    #[test]
    fn adjacent_tab_selection_wraps() {
        assert_eq!(adjacent_tab_index(2, 3, TabDirection::Next), 0);
        assert_eq!(adjacent_tab_index(0, 3, TabDirection::Previous), 2);
    }

    #[test]
    fn indexed_tab_selection_uses_direct_position() {
        let ids = [TabId(1), TabId(2), TabId(3), TabId(4), TabId(5)];

        assert_eq!(
            selectable_tab_id_by_index_from_ids(&ids, TabId(5), 0),
            Some(TabId(1))
        );
        assert_eq!(
            selectable_tab_id_by_index_from_ids(&ids, TabId(1), 3),
            Some(TabId(4))
        );
    }

    #[test]
    fn indexed_tab_selection_no_ops_for_active_or_missing_tab() {
        let ids = [TabId(1), TabId(2), TabId(3)];

        assert_eq!(selectable_tab_id_by_index_from_ids(&ids, TabId(2), 1), None);
        assert_eq!(selectable_tab_id_by_index_from_ids(&ids, TabId(1), 3), None);
    }

    #[test]
    fn indexed_tab_selection_no_ops_for_single_tab() {
        let ids = [TabId(1)];

        assert_eq!(selectable_tab_id_by_index_from_ids(&ids, TabId(1), 0), None);
    }

    #[test]
    fn last_tab_cannot_be_closed() {
        assert!(!can_close_tab(0));
        assert!(!can_close_tab(1));
        assert!(can_close_tab(2));
    }

    #[test]
    fn lone_standalone_tab_can_drag_but_lone_composite_cannot() {
        assert!(!can_drag_tab(0, false));
        assert!(can_drag_tab(1, false));
        assert!(!can_drag_tab(1, true));
        assert!(can_drag_tab(2, false));
        assert!(can_drag_tab(2, true));
    }

    #[test]
    fn tab_min_width_keeps_labels_readable_before_overflow() {
        assert_eq!(TAB_MIN_WIDTH, 160.0);
        assert!(TAB_MIN_WIDTH < TAB_WIDTH);
    }

    #[test]
    fn dragging_tab_state_sets_and_clears() {
        let mut dragging_tab = None;

        start_dragging_tab(&mut dragging_tab, TabId(2));
        assert_eq!(dragging_tab, Some(TabId(2)));
        assert!(clear_dragging_tab(&mut dragging_tab));
        assert_eq!(dragging_tab, None);
        assert!(!clear_dragging_tab(&mut dragging_tab));
    }

    #[test]
    fn file_drag_hover_ignores_active_or_missing_tab() {
        let tabs = [TabId(1), TabId(2), TabId(3)];

        assert_eq!(
            activate_tab_id_for_file_drag_hover(TabId(2), TabId(2), &tabs),
            None
        );
        assert_eq!(
            activate_tab_id_for_file_drag_hover(TabId(2), TabId(4), &tabs),
            None
        );
    }

    #[test]
    fn file_drag_hover_activates_inactive_existing_tab() {
        let tabs = [TabId(1), TabId(2), TabId(3)];

        assert_eq!(
            activate_tab_id_for_file_drag_hover(TabId(2), TabId(3), &tabs),
            Some(TabId(3))
        );
        assert_eq!(
            activate_tab_id_for_file_drag_hover(TabId(3), TabId(1), &tabs),
            Some(TabId(1))
        );
    }

    #[test]
    fn file_drag_hover_activation_requires_multiple_tabs() {
        let tabs = [TabId(1)];

        assert_eq!(
            activate_tab_id_for_file_drag_hover(TabId(1), TabId(1), &tabs),
            None
        );
        assert_eq!(
            activate_tab_id_for_file_drag_hover(TabId(1), TabId(2), &tabs),
            None
        );
    }

    #[test]
    fn file_drag_hover_activation_uses_direct_tab_id() {
        let tabs = [TabId(5), TabId(9), TabId(2)];

        assert_eq!(
            activate_tab_id_for_file_drag_hover(TabId(5), TabId(2), &tabs),
            Some(TabId(2))
        );
    }

    #[test]
    fn tab_reorder_hit_test_allows_vertical_tolerance() {
        let left = 20.0;
        let top = 10.0;
        let width = 200.0;
        let height = 36.0;
        let cursor_x = left + (width / 2.0);

        assert!(tab_reorder_hit_test(
            left, top, width, height, cursor_x, top
        ));
        assert!(tab_reorder_hit_test(
            left,
            top,
            width,
            height,
            cursor_x,
            top - TAB_REORDER_VERTICAL_TOLERANCE
        ));
        assert!(tab_reorder_hit_test(
            left,
            top,
            width,
            height,
            cursor_x,
            top + height + TAB_REORDER_VERTICAL_TOLERANCE
        ));
    }

    #[test]
    fn tab_reorder_hit_test_rejects_outside_tolerance_or_horizontal_bounds() {
        let left = 20.0;
        let top = 10.0;
        let width = 200.0;
        let height = 36.0;
        let cursor_x = left + (width / 2.0);
        let cursor_y = top + (height / 2.0);

        assert!(!tab_reorder_hit_test(
            left,
            top,
            width,
            height,
            cursor_x,
            top - TAB_REORDER_VERTICAL_TOLERANCE - 1.0
        ));
        assert!(!tab_reorder_hit_test(
            left,
            top,
            width,
            height,
            cursor_x,
            top + height + TAB_REORDER_VERTICAL_TOLERANCE + 1.0
        ));
        assert!(!tab_reorder_hit_test(
            left,
            top,
            width,
            height,
            left - 1.0,
            cursor_y
        ));
        assert!(!tab_reorder_hit_test(
            left,
            top,
            width,
            height,
            left + width + 1.0,
            cursor_y
        ));
    }

    #[test]
    fn active_tab_after_close_uses_next_tab_or_previous_tail() {
        assert_eq!(
            active_id_after_close_from_removed_ids(&[TabId(1), TabId(3)], 1),
            Some(TabId(3))
        );
        assert_eq!(
            active_id_after_close_from_removed_ids(&[TabId(1)], 1),
            Some(TabId(1))
        );
        assert_eq!(active_id_after_close_from_removed_ids(&[], 0), None);
    }

    #[test]
    fn reordering_tabs_moves_before_or_after_target() {
        let mut ids = vec![TabId(1), TabId(2), TabId(3), TabId(4)];

        assert!(reorder_tab_ids(&mut ids, TabId(4), TabId(2), true));
        assert_eq!(ids, vec![TabId(1), TabId(4), TabId(2), TabId(3)]);

        assert!(reorder_tab_ids(&mut ids, TabId(1), TabId(3), false));
        assert_eq!(ids, vec![TabId(4), TabId(2), TabId(3), TabId(1)]);
    }

    #[test]
    fn reordering_same_or_missing_tab_is_no_op() {
        let mut ids = vec![TabId(1), TabId(2)];

        assert!(!reorder_tab_ids(&mut ids, TabId(1), TabId(1), true));
        assert!(!reorder_tab_ids(&mut ids, TabId(3), TabId(1), true));
        assert_eq!(ids, vec![TabId(1), TabId(2)]);
    }
}
