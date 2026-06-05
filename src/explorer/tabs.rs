use std::{path::PathBuf, time::Duration};

use gpui::{
    AnyElement, App, Bounds, ClickEvent, Context, DragMoveEvent, Entity, ExternalPaths,
    FileDropEvent, FocusHandle, Focusable, IntoElement, Modifiers, MouseButton, MouseDownEvent,
    ParentElement, Pixels, Point, Render, ScrollHandle, SharedString, Styled, Task, Window, div,
    font, prelude::*, px, rgb,
};

use crate::explorer::{
    CloseTab, NewTab, SelectNextTab, SelectPreviousTab, SelectTabByIndex,
    constants::{NAV_BUTTON_ACTIVE_OPACITY, NAV_BUTTON_HOVER_BG},
    default_start_path,
    drag_drop::{DraggedEntries, DropDestination},
    icons::folder_icon,
    render::render_drop_indicator,
    view::{ExplorerView, ExplorerViewEvent},
};

const TAB_BAR_HEIGHT: f32 = 36.0;
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
const TAB_FILE_DRAG_HOVER_DELAY: Duration = Duration::from_millis(0);
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
    is_active: bool,
}

struct TabDragPreview {
    label: SharedString,
    is_active: bool,
    scale_factor: f32,
}

struct PendingTabFileDragHover {
    tab_id: TabId,
    request_id: u64,
    bounds: Bounds<Pixels>,
    latest_position: Point<Pixels>,
    _task: Option<Task<()>>,
}

pub struct ExplorerTabs {
    tabs: Vec<ExplorerTab>,
    active_tab: TabId,
    next_tab_id: u64,
    background_operation_tabs: Vec<Entity<ExplorerView>>,
    dragging_tab: Option<TabId>,
    pending_file_drag_hover: Option<PendingTabFileDragHover>,
    next_file_drag_hover_request_id: u64,
    tab_scroll_handle: ScrollHandle,
}

impl ExplorerTabs {
    pub fn new(initial_path: PathBuf, focus_handle: FocusHandle, cx: &mut Context<Self>) -> Self {
        let first_id = TabId(1);
        let view = cx
            .new(|cx| ExplorerView::new_watched_with_focus_handle(initial_path, focus_handle, cx));
        observe_tab_view(&view, cx);

        Self {
            tabs: vec![ExplorerTab { id: first_id, view }],
            active_tab: first_id,
            next_tab_id: 2,
            background_operation_tabs: Vec::new(),
            dragging_tab: None,
            pending_file_drag_hover: None,
            next_file_drag_hover_request_id: 0,
            tab_scroll_handle: ScrollHandle::new(),
        }
    }

    fn active_tab_index(&self) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == self.active_tab)
    }

    fn active_tab(&self) -> Option<&ExplorerTab> {
        self.tabs.iter().find(|tab| tab.id == self.active_tab)
    }

    fn tab_view(&self, id: TabId) -> Option<Entity<ExplorerView>> {
        self.tabs
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| tab.view.clone())
    }

    fn add_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.add_foreground_tab(default_start_path(), window, cx);
    }

    fn add_foreground_tab(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let id = TabId(self.next_tab_id);
        self.next_tab_id += 1;

        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        let view = cx.new(|cx| ExplorerView::new_watched_with_focus_handle(path, focus_handle, cx));
        observe_tab_view(&view, cx);

        self.tabs.push(ExplorerTab { id, view });
        self.active_tab = id;
        self.scroll_active_tab_into_view();
    }

    fn add_background_tab(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let id = TabId(self.next_tab_id);
        self.next_tab_id += 1;

        let focus_handle = cx.focus_handle();
        let view = cx.new(|cx| ExplorerView::new_watched_with_focus_handle(path, focus_handle, cx));
        observe_tab_view(&view, cx);

        self.tabs.push(ExplorerTab { id, view });
    }

    fn activate_tab(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_tab == id || !self.tabs.iter().any(|tab| tab.id == id) {
            return;
        }

        self.active_tab = id;
        self.scroll_active_tab_into_view();
        self.focus_active_tab(window, cx);
    }

    fn schedule_file_drag_hover_tab_activation(
        &mut self,
        id: TabId,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !tab_can_schedule_file_drag_hover(self.active_tab, id, &self.tabs) {
            self.cancel_file_drag_hover_tab_activation();
            return;
        }

        if let Some(pending) = self.pending_file_drag_hover.as_mut()
            && pending.tab_id == id
        {
            pending.bounds = bounds;
            pending.latest_position = position;
            return;
        }

        let request_id = self.next_file_drag_hover_request_id;

        let task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(TAB_FILE_DRAG_HOVER_DELAY)
                .await;

            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.complete_file_drag_hover_tab_activation(id, request_id, window, cx) {
                        cx.notify();
                    }
                });
            });
        });

        store_pending_file_drag_hover(
            &mut self.pending_file_drag_hover,
            &mut self.next_file_drag_hover_request_id,
            id,
            bounds,
            position,
            Some(task),
        );
    }

    fn update_file_drag_hover_position(&mut self, position: Point<Pixels>) -> bool {
        let Some(pending) = self.pending_file_drag_hover.as_mut() else {
            return false;
        };

        if pending.latest_position == position {
            return false;
        }

        pending.latest_position = position;
        false
    }

    fn cancel_file_drag_hover_tab_activation(&mut self) -> bool {
        clear_pending_file_drag_hover(&mut self.pending_file_drag_hover)
    }

    fn complete_file_drag_hover_tab_activation(
        &mut self,
        id: TabId,
        request_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !pending_file_drag_hover_can_activate(
            self.pending_file_drag_hover.as_ref(),
            id,
            request_id,
        ) {
            return false;
        }

        self.pending_file_drag_hover = None;
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
        self.cancel_file_drag_hover_tab_activation();
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
        self.cancel_file_drag_hover_tab_activation();
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
        let has_active_operation = closing.view.read(cx).has_active_file_operation();
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
        self.active_tab = self.tabs[next_index].id;
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

    fn reload_all_tabs(&mut self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            let _ = tab.view.update(cx, |view, cx| {
                view.reload();
                cx.notify();
            });
        }
    }

    fn cleanup_completed_background_operations(&mut self, cx: &mut Context<Self>) {
        let mut still_running = Vec::new();

        for view in std::mem::take(&mut self.background_operation_tabs) {
            if view.read(cx).has_active_file_operation() {
                still_running.push(view);
            }
        }

        self.background_operation_tabs = still_running;
    }

    fn handle_new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.add_new_tab(window, cx);
        cx.notify();
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
        let scale_factor = window.scale_factor();
        let can_close = self.tabs.len() > 1;
        let can_drag = can_drag_tab(self.tabs.len());
        let mut tab_children = self
            .tabs
            .iter()
            .map(|tab| {
                self.render_tab(tab, can_close, can_drag, scale_factor, cx)
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        tab_children.push(new_tab_button(cx));

        div()
            .id("explorer-tab-bar")
            .flex()
            .flex_row()
            .items_end()
            .h(px(TAB_BAR_HEIGHT))
            .w_full()
            .flex_shrink_0()
            .overflow_hidden()
            .bg(rgb(0xe8e8e8))
            .child(
                div()
                    .id("explorer-tab-scroll")
                    .flex()
                    .flex_row()
                    .items_end()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .overflow_x_scroll()
                    .track_scroll(&self.tab_scroll_handle)
                    .children(tab_children),
            )
            .into_any_element()
    }

    fn render_tab(
        &self,
        tab: &ExplorerTab,
        can_close: bool,
        can_drag: bool,
        scale_factor: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_active = tab.id == self.active_tab;
        let label = SharedString::from(tab.view.read(cx).tab_label());
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
                scale_factor,
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
                        this.schedule_file_drag_hover_tab_activation(
                            tab_id,
                            event.bounds,
                            event.event.position,
                            window,
                            cx,
                        );
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
                        this.schedule_file_drag_hover_tab_activation(
                            tab_id,
                            event.bounds,
                            event.event.position,
                            window,
                            cx,
                        );
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
            rendered_tab = rendered_tab
                .on_drag(
                    TabDrag {
                        id: tab_id,
                        label: drag_label,
                        is_active,
                    },
                    move |drag, _, window, cx| {
                        let scale_factor = window.scale_factor();
                        let _ = entity.update(cx, |this, cx| {
                            this.start_tab_drag(drag.id);
                            cx.notify();
                        });
                        cx.new(|_| TabDragPreview {
                            label: drag.label.clone(),
                            is_active: drag.is_active,
                            scale_factor,
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
        let active_view = self.active_tab().map(|tab| tab.view.clone());
        let drop_exit_view = active_view.clone();
        let drop_exit_tabs = cx.entity();
        let active_drop_indicator = active_view
            .as_ref()
            .and_then(|view| view.read(cx).active_drop_indicator());

        div()
            .key_context("ExplorerTabs")
            .on_action(cx.listener(Self::handle_new_tab))
            .on_action(cx.listener(Self::handle_close_tab))
            .on_action(cx.listener(Self::handle_select_next_tab))
            .on_action(cx.listener(Self::handle_select_previous_tab))
            .on_action(cx.listener(Self::handle_select_tab_by_index))
            .on_file_drop(move |event, _, cx| {
                if let FileDropEvent::Exited = event {
                    if let Some(active_view) = &drop_exit_view {
                        active_view.update(cx, |view, cx| {
                            if view.clear_drop_indicator() {
                                cx.notify();
                            }
                        });
                    }

                    let _ = drop_exit_tabs.update(cx, |this, cx| {
                        if this.cancel_file_drag_hover_tab_activation() {
                            cx.notify();
                        }
                    });
                }
            })
            .on_drag_move::<DraggedEntries>({
                let entity = cx.entity();
                move |event: &DragMoveEvent<DraggedEntries>, _, cx| {
                    let _ = entity.update(cx, |this, cx| {
                        if this.update_file_drag_hover_position(event.event.position) {
                            cx.notify();
                        }
                    });
                }
            })
            .on_drag_move::<ExternalPaths>({
                let entity = cx.entity();
                move |event: &DragMoveEvent<ExternalPaths>, _, cx| {
                    let _ = entity.update(cx, |this, cx| {
                        if this.update_file_drag_hover_position(event.event.position) {
                            cx.notify();
                        }
                    });
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
                this.child(render_drop_indicator(indicator, window))
            })
    }
}

impl Render for TabDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        tab_preview_visual(self.label.clone(), self.is_active, self.scale_factor)
    }
}

fn tab_preview_visual(label: SharedString, is_active: bool, scale_factor: f32) -> impl IntoElement {
    div()
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
            scale_factor,
            Some(close_tab_glyph_visual().into_any_element()),
        ))
}

fn tab_inner_contents(
    label: SharedString,
    scale_factor: f32,
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
        .child(folder_icon(scale_factor))
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

fn observe_tab_view(view: &Entity<ExplorerView>, cx: &mut Context<ExplorerTabs>) {
    cx.observe(view, |this, _, cx| {
        this.cleanup_completed_background_operations(cx);
        cx.notify();
    })
    .detach();

    cx.subscribe(view, |this, _, event, cx| match event {
        ExplorerViewEvent::FilesystemChanged => {
            this.reload_all_tabs(cx);
            cx.notify();
        }
        ExplorerViewEvent::OpenDirectoryInNewTab(path) => {
            this.add_background_tab(path.clone(), cx);
            cx.notify();
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

fn tab_can_schedule_file_drag_hover(
    active_tab: TabId,
    target_tab: TabId,
    tabs: &[ExplorerTab],
) -> bool {
    target_tab != active_tab && tabs.iter().any(|tab| tab.id == target_tab)
}

#[cfg(test)]
fn tab_can_schedule_file_drag_hover_from_ids(
    active_tab: TabId,
    target_tab: TabId,
    tabs: &[TabId],
) -> bool {
    target_tab != active_tab && tabs.contains(&target_tab)
}

fn store_pending_file_drag_hover(
    pending: &mut Option<PendingTabFileDragHover>,
    next_request_id: &mut u64,
    tab_id: TabId,
    bounds: Bounds<Pixels>,
    latest_position: Point<Pixels>,
    task: Option<Task<()>>,
) -> u64 {
    let request_id = *next_request_id;
    *next_request_id = next_request_id.wrapping_add(1);
    *pending = Some(PendingTabFileDragHover {
        tab_id,
        request_id,
        bounds,
        latest_position,
        _task: task,
    });
    request_id
}

fn clear_pending_file_drag_hover(pending: &mut Option<PendingTabFileDragHover>) -> bool {
    pending.take().is_some()
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

fn pending_file_drag_hover_can_activate(
    pending: Option<&PendingTabFileDragHover>,
    id: TabId,
    request_id: u64,
) -> bool {
    pending.is_some_and(|pending| {
        pending.tab_id == id
            && pending.request_id == request_id
            && pending.bounds.contains(&pending.latest_position)
    })
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
    use crate::explorer::view::tab_label_for_path;
    use gpui::{bounds, point, size};

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
    fn pending_file_drag_hover_replaces_prior_tab() {
        let mut pending = None;
        let mut next_request_id = 7;
        let first_bounds = test_tab_bounds(0.0);
        let second_bounds = test_tab_bounds(240.0);

        let first_request_id = store_pending_file_drag_hover(
            &mut pending,
            &mut next_request_id,
            TabId(2),
            first_bounds,
            point(px(20.0), px(18.0)),
            None,
        );
        let second_request_id = store_pending_file_drag_hover(
            &mut pending,
            &mut next_request_id,
            TabId(3),
            second_bounds,
            point(px(260.0), px(18.0)),
            None,
        );

        let pending = pending.as_ref().expect("pending drag hover");
        assert_eq!(first_request_id, 7);
        assert_eq!(second_request_id, 8);
        assert_eq!(next_request_id, 9);
        assert_eq!(pending.tab_id, TabId(3));
        assert_eq!(pending.request_id, second_request_id);
        assert_eq!(pending.bounds, second_bounds);
        assert_eq!(pending.latest_position, point(px(260.0), px(18.0)));
    }

    #[test]
    fn pending_file_drag_hover_ignores_active_or_missing_tab() {
        let tabs = [TabId(1), TabId(2), TabId(3)];

        assert!(!tab_can_schedule_file_drag_hover_from_ids(
            TabId(2),
            TabId(2),
            &tabs
        ));
        assert!(!tab_can_schedule_file_drag_hover_from_ids(
            TabId(2),
            TabId(4),
            &tabs
        ));
        assert!(tab_can_schedule_file_drag_hover_from_ids(
            TabId(2),
            TabId(3),
            &tabs
        ));
    }

    #[test]
    fn pending_file_drag_hover_activation_validates_request_and_bounds() {
        let bounds = test_tab_bounds(0.0);
        let pending = PendingTabFileDragHover {
            tab_id: TabId(2),
            request_id: 42,
            bounds,
            latest_position: point(px(80.0), px(18.0)),
            _task: None,
        };

        assert!(pending_file_drag_hover_can_activate(
            Some(&pending),
            TabId(2),
            42
        ));
        assert!(!pending_file_drag_hover_can_activate(
            Some(&pending),
            TabId(3),
            42
        ));
        assert!(!pending_file_drag_hover_can_activate(
            Some(&pending),
            TabId(2),
            43
        ));

        let outside_pending = PendingTabFileDragHover {
            latest_position: point(px(260.0), px(18.0)),
            ..pending
        };
        assert!(!pending_file_drag_hover_can_activate(
            Some(&outside_pending),
            TabId(2),
            42
        ));
    }

    #[test]
    fn pending_file_drag_hover_clears() {
        let mut pending = Some(PendingTabFileDragHover {
            tab_id: TabId(2),
            request_id: 42,
            bounds: test_tab_bounds(0.0),
            latest_position: point(px(80.0), px(18.0)),
            _task: None,
        });

        assert!(clear_pending_file_drag_hover(&mut pending));
        assert!(pending.is_none());
        assert!(!clear_pending_file_drag_hover(&mut pending));
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

    fn test_tab_bounds(left: f32) -> Bounds<Pixels> {
        bounds(
            point(px(left), px(0.0)),
            size(px(TAB_WIDTH), px(TAB_BAR_HEIGHT)),
        )
    }
}
