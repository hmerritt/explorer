use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, App, ClickEvent, Context, DragMoveEvent, Entity, ExternalPaths, FileDropEvent,
    FocusHandle, Focusable, IntoElement, Modifiers, MouseButton, MouseDownEvent, ParentElement,
    Render, ScrollHandle, SharedString, Styled, Window, div, font, prelude::*, px, rgb,
};

use crate::explorer::{
    CloseTab, NewTab, NewWindow, SelectNextTab, SelectPreviousTab, SelectTabByIndex,
    constants::{NAV_BUTTON_ACTIVE_OPACITY, NAV_BUTTON_HOVER_BG},
    drag_drop::{DraggedEntries, DropDestination},
    icons::folder_icon,
    render::render_drop_indicator,
    view::{ExplorerView, ExplorerViewEvent},
};
use crate::settings::SettingsState;
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
const CLOSE_GLYPH: &str = "\u{E711}";
const NEW_TAB_GLYPH: &str = "\u{E710}";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TabId(u64);

#[derive(Clone)]
struct ExplorerTab {
    id: TabId,
    view: Entity<ExplorerView>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TabDrag {
    id: TabId,
    label: SharedString,
    path: PathBuf,
    is_active: bool,
}

struct TabDragPreview {
    label: SharedString,
    path: PathBuf,
    is_active: bool,
    font: gpui::Font,
}

pub struct ExplorerTabs {
    tabs: Vec<ExplorerTab>,
    active_tab: TabId,
    next_tab_id: u64,
    background_operation_tabs: Vec<Entity<ExplorerView>>,
    dragging_tab: Option<TabId>,
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
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let first_id = TabId(1);
        let view = cx.new(|cx| {
            ExplorerView::new_watched_with_focus_handle(
                initial_path,
                focus_handle,
                Some(window),
                cx,
            )
        });
        observe_tab_view(&view, window, cx);
        observe_settings(cx);

        Self {
            tabs: vec![ExplorerTab { id: first_id, view }],
            active_tab: first_id,
            next_tab_id: 2,
            background_operation_tabs: Vec::new(),
            dragging_tab: None,
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
        let view =
            cx.new(|_| ExplorerView::new_with_focus_handle_for_test(initial_path, focus_handle));

        Self {
            tabs: vec![ExplorerTab { id: first_id, view }],
            active_tab: first_id,
            next_tab_id: 2,
            background_operation_tabs: Vec::new(),
            dragging_tab: None,
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
            .map(|tab| tab.view.read(cx).path().to_path_buf())
    }

    fn tab_view(&self, id: TabId) -> Option<Entity<ExplorerView>> {
        self.tabs
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| tab.view.clone())
    }

    fn add_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = cx.global::<SettingsState>().startup_path();
        self.add_foreground_tab(path, window, cx);
    }

    fn add_foreground_tab(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let id = TabId(self.next_tab_id);
        self.next_tab_id += 1;

        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        let view = cx.new(|cx| {
            ExplorerView::new_watched_with_focus_handle(path, focus_handle, Some(window), cx)
        });
        observe_tab_view(&view, window, cx);

        self.tabs.push(ExplorerTab { id, view });
        self.cancel_active_tab_thumbnail_extraction(cx);
        self.active_tab = id;
        self.scroll_active_tab_into_view();
    }

    fn add_background_tab(&mut self, path: PathBuf, window: &Window, cx: &mut Context<Self>) {
        let id = TabId(self.next_tab_id);
        self.next_tab_id += 1;

        let focus_handle = cx.focus_handle();
        let view = cx.new(|cx| {
            ExplorerView::new_watched_with_focus_handle(path, focus_handle, Some(window), cx)
        });
        observe_tab_view(&view, window, cx);

        self.tabs.push(ExplorerTab { id, view });
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
            view.can_drop_value(dragged_value, &DropDestination::CurrentDirectory, modifiers)
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
                    cx,
                );
                cx.notify();
            });
        }
    }

    fn focus_active_tab(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab() {
            let focus_handle = tab.view.read(cx).focus_handle(cx);
            focus_handle.focus(window);
        }
    }

    fn close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_tab = self.active_tab;
        self.close_tab(active_tab, window, cx);
    }

    fn close_tab(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        if !can_close_tab(self.tabs.len()) {
            return;
        }

        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };

        let closing = self.tabs.remove(index);
        let has_active_operation = closing.view.read(cx).has_background_operation();
        let _ = closing.view.update(cx, |view, cx| {
            view.prepare_for_tab_close(cx);
            cx.notify();
        });

        if has_active_operation {
            self.background_operation_tabs.push(closing.view);
        }

        if self.active_tab == id {
            if let Some(next_active) = active_id_after_close_from_removed(&self.tabs, index) {
                self.active_tab = next_active;
            }
            self.scroll_active_tab_into_view();
            self.focus_active_tab(window, cx);
        } else {
            self.scroll_active_tab_into_view();
        }
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
            let _ = tab.view.update(cx, |view, cx| {
                view.cancel_image_thumbnail_extraction(cx);
                view.cancel_video_hover_preview(cx);
            });
        }
    }

    fn reload_tabs_except(&mut self, source_view: &Entity<ExplorerView>, cx: &mut Context<Self>) {
        let source_view_id = source_view.entity_id();
        for tab in &self.tabs {
            if tab.view.entity_id() == source_view_id {
                continue;
            }
            let _ = tab.view.update(cx, |view, cx| {
                view.reload_async_with_entry_metadata_resolution(cx);
                cx.notify();
            });
        }
    }

    fn redirect_tabs_after_mounted_volume_ejected(
        &mut self,
        ejected_root: &Path,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut redirected = false;
        for tab in &self.tabs {
            let _ = tab.view.update(cx, |view, cx| {
                if view.redirect_after_mounted_volume_ejected_with_watcher(ejected_root, cx) {
                    redirected = true;
                    cx.notify();
                }
            });
        }
        redirected
    }

    fn apply_settings_to_all_tabs(&mut self, cx: &mut Context<Self>) {
        let settings = cx.global::<SettingsState>().value.clone();
        for tab in &self.tabs {
            let _ = tab
                .view
                .update(cx, |view, cx| view.apply_settings(&settings, cx));
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

    fn render_tab_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let can_close = self.tabs.len() > 1;
        let can_drag = can_drag_tab(self.tabs.len());
        let decorations = window.window_decorations();
        let tab_scroll_width = tab_strip_width(self.tabs.len());
        let mut tab_children = self
            .tabs
            .iter()
            .map(|tab| {
                self.render_tab(tab, can_close, can_drag, cx)
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
        let view = tab.view.read(cx);
        let label = SharedString::from(view.tab_label());
        let path = view.path().to_path_buf();
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
}

impl Render for ExplorerTabs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.cleanup_completed_background_operations(cx);
        let app_font = crate::settings::current_app_font(cx);
        let active_view = self.active_tab().map(|tab| tab.view.clone());
        let drop_exit_view = active_view.clone();
        let input_mouse_down_view = active_view.clone();
        let active_drop_indicator = active_view
            .as_ref()
            .and_then(|view| view.read(cx).active_drop_indicator());

        let content = div()
            .font(app_font.clone())
            .key_context("ExplorerTabs")
            .on_action(cx.listener(Self::handle_new_tab))
            .on_action(cx.listener(Self::handle_new_window))
            .on_action(cx.listener(Self::handle_close_tab))
            .on_action(cx.listener(Self::handle_select_next_tab))
            .on_action(cx.listener(Self::handle_select_previous_tab))
            .on_action(cx.listener(Self::handle_select_tab_by_index))
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
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_hidden()
                    .when_some(active_view, |this, view| this.child(view)),
            )
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
            self.font.clone(),
        )
    }
}

fn tab_preview_visual(
    label: SharedString,
    path: &Path,
    is_active: bool,
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
            Some(close_tab_glyph_visual().into_any_element()),
        ))
}

fn tab_inner_contents(
    label: SharedString,
    path: Option<&Path>,
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
        .child(tab_icon(path))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(TAB_TEXT_SIZE))
                .text_color(rgb(TAB_TEXT_COLOR))
                .child(label),
        )
        .when_some(close_glyph, |this, close_glyph| this.child(close_glyph))
        .into_any_element()
}

fn tab_icon(path: Option<&Path>) -> AnyElement {
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

fn can_close_tab(tab_count: usize) -> bool {
    tab_count > 1
}

fn can_drag_tab(tab_count: usize) -> bool {
    tab_count > 1
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
            PasteClipboard, RecursiveSearchEdit, RenameCommit, SearchCommit, SearchEdit,
        },
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
        });
        cx.run_until_parked();

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
                    view.back_stack = vec![ejected_history.clone(), history_one.clone()];
                    view.forward_stack = vec![ejected_root.join("forward-one")];
                });

                let focus_two = cx.focus_handle();
                let view_two_path = affected_two.clone();
                let view_two = cx.new(|_| {
                    ExplorerView::new_with_focus_handle_for_test(view_two_path, focus_two)
                });
                view_two.update(cx, |view, _| {
                    view.back_stack = vec![history_two.clone()];
                    view.forward_stack = vec![ejected_root.join("forward-two")];
                });

                tabs.tabs.push(ExplorerTab {
                    id: TabId(2),
                    view: view_one.clone(),
                });
                tabs.tabs.push(ExplorerTab {
                    id: TabId(3),
                    view: view_two.clone(),
                });
                tabs.next_tab_id = 4;
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
            assert!(
                view.forward_stack
                    .iter()
                    .all(|path| !path.starts_with(&ejected_root))
            );
        });
        cx.read_entity(&affected_views[1], |view, _| {
            assert_eq!(view.path, history_two);
            assert!(view.back_stack.is_empty());
            assert!(
                view.forward_stack
                    .iter()
                    .all(|path| !path.starts_with(&ejected_root))
            );
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
                view.pending_trash = Some(PendingTrash {
                    paths: paths.clone(),
                });
                view.confirm_pending_trash(cx);
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
    fn single_tab_cannot_be_dragged() {
        assert!(!can_drag_tab(0));
        assert!(!can_drag_tab(1));
        assert!(can_drag_tab(2));
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
