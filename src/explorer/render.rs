use std::ops::Range;

use gpui::{
    AnyElement, App, ClickEvent, Context, Div, DragMoveEvent, Entity, ExternalPaths, FocusHandle,
    Focusable, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    NavigationDirection, Render, ScrollWheelEvent, SharedString, TextRun, Window, canvas, div,
    font, prelude::*, px, rgb, uniform_list,
};

use crate::explorer::{
    breadcrumb::{
        BreadcrumbSegment, VisibleBreadcrumb, directory_bar_available_width,
        visible_breadcrumb_for_path,
    },
    constants::{
        COLUMN_DATE_WIDTH, COLUMN_NAME_MIN_WIDTH, COLUMN_SIZE_WIDTH, COLUMN_TYPE_WIDTH,
        DIRECTORY_BAR_ELLIPSIS, DIRECTORY_BAR_HEIGHT, DIRECTORY_BAR_HORIZONTAL_PADDING,
        DIRECTORY_BAR_RADIUS, DIRECTORY_BAR_SEGMENT_HORIZONTAL_PADDING, DIRECTORY_BAR_SEPARATOR,
        DIRECTORY_BAR_TEXT_SIZE, EMPTY_FOLDER_MESSAGE, EMPTY_FOLDER_TEXT_SIZE,
        EMPTY_FOLDER_TOP_MARGIN, FILE_ICON_SLOT_WIDTH_PHYSICAL, HEADER_HEIGHT,
        NAV_BUTTON_ACTIVE_OPACITY, NAV_BUTTON_HOVER_BG, NAV_BUTTON_SIZE, NAV_ICON_DISABLED_COLOR,
        NAV_ICON_ENABLED_COLOR, NAV_ICON_SIZE_PHYSICAL, NAVBAR_HEIGHT, NAVBAR_HORIZONTAL_PADDING,
        NAVBAR_ITEM_GAP, OPEN_ERROR_HORIZONTAL_PADDING, OPEN_ERROR_VERTICAL_PADDING, ROW_HEIGHT,
        SCROLLBAR_GUTTER_WIDTH, SIDEBAR_HORIZONTAL_PADDING, SIDEBAR_ICON_TEXT_GAP_PHYSICAL,
        SIDEBAR_ROW_HEIGHT, SIDEBAR_TEXT_SIZE, SIDEBAR_WIDTH,
    },
    drag_drop::{DragPreview, DraggedEntries, DropDestination},
    entry::FileEntry,
    formatting::{format_modified, format_size},
    icons::{
        NavIcon, desktop_folder_icon, device_px, device_px_value, directory_shortcut_icon,
        downloads_folder_icon, drive_icon, file_icon, folder_icon, nav_icon_font,
    },
    mouse_selection::{local_point, selection_box_bounds, viewport_size},
    navigation::{EntryAction, HistoryMode},
    scrollbar::scrollbar_header_spacer,
    selection::SelectionModifiers,
    sidebar::{SidebarItem, SidebarItemKind, UserDirectoryKind, sidebar_sections},
    view::{ExplorerContentBranch, ExplorerView},
};

const NAME_CELL_LEFT_PADDING: f32 = 16.0;
const NAME_ICON_TEXT_GAP_PHYSICAL: f32 = 8.0;
const NAME_TEXT_SIZE: f32 = 12.0;
const TEXT_CELL_HORIZONTAL_PADDING: f32 = 8.0;
const TEXT_CELL_TEXT_COLOR: u32 = 0x595959;
const NAME_TRUNCATION_SUFFIX: &str = "…";

impl ExplorerView {
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

    fn render_sidebar(&self, scale_factor: f32, cx: &mut Context<Self>) -> AnyElement {
        let sections = sidebar_sections();
        let mut children = Vec::new();

        for (index, item) in sections.user_directories.into_iter().enumerate() {
            children.push(self.render_sidebar_row(index, item, scale_factor, cx));
        }

        if !children.is_empty() && !sections.drives.is_empty() {
            children.push(sidebar_separator().into_any_element());
        }

        for (index, item) in sections.drives.into_iter().enumerate() {
            children.push(self.render_sidebar_row(index + 1_000, item, scale_factor, cx));
        }

        div()
            .id("explorer-sidebar")
            .flex()
            .flex_col()
            .h_full()
            .w(px(SIDEBAR_WIDTH))
            .flex_shrink_0()
            .bg(rgb(0xffffff))
            .border_r_1()
            .border_color(rgb(0xe7e7e7))
            .pt(px(8.0))
            .overflow_hidden()
            .children(children)
            .into_any_element()
    }

    fn render_sidebar_row(
        &self,
        id: usize,
        item: SidebarItem,
        scale_factor: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_current = item.path == self.path;
        let label = item.label.clone();
        let path = item.path.clone();
        let icon_item = item.clone();

        div()
            .id(("explorer-sidebar-row", id))
            .flex()
            .flex_row()
            .items_center()
            .h(px(SIDEBAR_ROW_HEIGHT))
            .mx(px(4.0))
            .px(px(SIDEBAR_HORIZONTAL_PADDING))
            .rounded(px(4.0))
            .cursor_default()
            .bg(if is_current {
                rgb(0xcce8ff)
            } else {
                rgb(0xffffff)
            })
            .when(!is_current, |this| {
                this.hover(|style| style.bg(rgb(0xe5f3ff)))
            })
            .active(|style| style.opacity(NAV_BUTTON_ACTIVE_OPACITY))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.navigate_to_sidebar_path(path.clone());
                cx.stop_propagation();
                cx.notify();
            }))
            .child(sidebar_item_icon(icon_item, scale_factor))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .ml(device_px(SIDEBAR_ICON_TEXT_GAP_PHYSICAL, scale_factor))
                    .truncate()
                    .text_size(px(SIDEBAR_TEXT_SIZE))
                    .text_color(rgb(0x1f1f1f))
                    .child(SharedString::from(label)),
            )
            .into_any_element()
    }

    fn render_row(
        &self,
        ix: usize,
        scale_factor: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entry = self.entries[ix].clone();
        let is_selected = self.entry_is_selected(ix);
        let clicked_entry = entry.clone();
        let drag_payload = self
            .can_start_item_drag_for_index(ix)
            .then(|| self.dragged_entries_for_index(ix))
            .flatten();
        let destination = DropDestination::Directory {
            item_path: entry.path.clone(),
            target_path: entry.drop_target_path().to_path_buf(),
        };
        let entity = cx.entity();

        let mut row = div()
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
                if this.suppress_next_click() {
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }

                if let Some(EntryAction::OpenFile(path)) = this.handle_entry_click(
                    &clicked_entry,
                    event.click_count(),
                    selection_modifiers_for_click(event),
                ) {
                    this.open_file_with_default_app(&path);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_drag_move::<DraggedEntries>({
                let destination = destination.clone();
                let entity = entity.clone();
                move |event: &DragMoveEvent<DraggedEntries>, window, cx| {
                    update_drag_cursor_if_hovered(&entity, event, &destination, window, cx);
                }
            })
            .on_drag_move::<ExternalPaths>({
                let destination = destination.clone();
                let entity = entity.clone();
                move |event: &DragMoveEvent<ExternalPaths>, window, cx| {
                    update_drag_cursor_if_hovered(&entity, event, &destination, window, cx);
                }
            });

        if let Some(drag_payload) = drag_payload {
            row = row.on_drag(drag_payload, {
                let entity = entity.clone();
                move |dragged: &DraggedEntries, cursor_offset, _, cx| {
                    entity.update(cx, |this, _| {
                        if this.mouse_selection_drag.is_none() {
                            this.cancel_mouse_selection_drag();
                        }
                    });
                    cx.new(|_| DragPreview::new(dragged, cursor_offset))
                }
            });
        }

        if entry.is_directory_like() {
            row = row
                .can_drop({
                    let destination = destination.clone();
                    let entity = entity.clone();
                    move |dragged_value, window, cx| {
                        entity.update(cx, |this, _| {
                            this.can_drop_value(dragged_value, &destination, window.modifiers())
                        })
                    }
                })
                .drag_over::<DraggedEntries>(|style, _, _, _| {
                    style.bg(rgb(0xe5f3ff)).border_color(rgb(0x0078d7))
                })
                .drag_over::<ExternalPaths>(|style, _, _, _| {
                    style.bg(rgb(0xe5f3ff)).border_color(rgb(0x0078d7))
                })
                .on_drop(cx.listener({
                    let destination = destination.clone();
                    move |this, dragged: &DraggedEntries, window, cx| {
                        this.drop_internal_entries(
                            dragged,
                            destination.clone(),
                            window.modifiers(),
                        );
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .on_drop(cx.listener({
                    let destination = destination.clone();
                    move |this, paths: &ExternalPaths, window, cx| {
                        this.drop_external_paths(
                            paths.paths(),
                            destination.clone(),
                            window.modifiers(),
                        );
                        cx.stop_propagation();
                        cx.notify();
                    }
                }));
        } else {
            row = row
                .can_drop(|dragged_value, _, _| {
                    dragged_value.is::<DraggedEntries>() || dragged_value.is::<ExternalPaths>()
                })
                .on_drop(
                    cx.listener(|_: &mut Self, _: &DraggedEntries, _: &mut Window, cx| {
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .on_drop(
                    cx.listener(|_: &mut Self, _: &ExternalPaths, _: &mut Window, cx| {
                        cx.stop_propagation();
                        cx.notify();
                    }),
                );
        }

        row.child(name_cell(&entry, scale_factor, window))
            .child(text_cell(
                format_modified(entry.modified),
                COLUMN_DATE_WIDTH,
                false,
                window,
            ))
            .child(text_cell(
                entry.type_label(),
                COLUMN_TYPE_WIDTH,
                false,
                window,
            ))
            .child(text_cell(
                format_size(entry.size),
                COLUMN_SIZE_WIDTH,
                true,
                window,
            ))
            .into_any_element()
    }

    fn render_list(&mut self, cx: &mut Context<Self>) -> Div {
        let entity = cx.entity();
        let current_directory = DropDestination::CurrentDirectory;

        div()
            .flex()
            .flex_row()
            .size_full()
            .overflow_hidden()
            .child(
                div()
                    .id("explorer-list-background")
                    .relative()
                    .flex_1()
                    .h_full()
                    .overflow_hidden()
                    .can_drop({
                        let current_directory = current_directory.clone();
                        let entity = entity.clone();
                        move |dragged_value, window, cx| {
                            entity.update(cx, |this, _| {
                                this.can_drop_value(
                                    dragged_value,
                                    &current_directory,
                                    window.modifiers(),
                                )
                            })
                        }
                    })
                    .drag_over::<DraggedEntries>(|style, _, _, _| style.bg(rgb(0xf7fbff)))
                    .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(0xf7fbff)))
                    .on_drag_move::<DraggedEntries>({
                        let current_directory = current_directory.clone();
                        let entity = entity.clone();
                        move |event: &DragMoveEvent<DraggedEntries>, window, cx| {
                            update_drag_cursor_if_hovered(
                                &entity,
                                event,
                                &current_directory,
                                window,
                                cx,
                            );
                        }
                    })
                    .on_drag_move::<ExternalPaths>({
                        let current_directory = current_directory.clone();
                        let entity = entity.clone();
                        move |event: &DragMoveEvent<ExternalPaths>, window, cx| {
                            update_drag_cursor_if_hovered(
                                &entity,
                                event,
                                &current_directory,
                                window,
                                cx,
                            );
                        }
                    })
                    .on_drop(cx.listener({
                        let current_directory = current_directory.clone();
                        move |this, dragged: &DraggedEntries, window, cx| {
                            this.drop_internal_entries(
                                dragged,
                                current_directory.clone(),
                                window.modifiers(),
                            );
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .on_drop(cx.listener({
                        let current_directory = current_directory.clone();
                        move |this, paths: &ExternalPaths, window, cx| {
                            this.drop_external_paths(
                                paths.paths(),
                                current_directory.clone(),
                                window.modifiers(),
                            );
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        if this.suppress_next_click() {
                            cx.stop_propagation();
                            cx.notify();
                            return;
                        }

                        this.clear_selection();
                        cx.notify();
                    }))
                    .child(self.render_mouse_selection_hit_layer(cx))
                    .child(
                        uniform_list(
                            "explorer-entries",
                            self.entries.len(),
                            cx.processor(|this, range: Range<usize>, window, cx| {
                                let scale_factor = window.scale_factor();
                                let mut rows = Vec::with_capacity(range.end - range.start);
                                for ix in range {
                                    rows.push(this.render_row(ix, scale_factor, window, cx));
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
                    )
                    .child(self.render_mouse_selection_box()),
            )
            .child(self.render_scrollbar(cx))
    }

    fn render_empty_folder(&self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let current_directory = DropDestination::CurrentDirectory;

        div()
            .id("explorer-empty-folder-drop-target")
            .size_full()
            .can_drop({
                let current_directory = current_directory.clone();
                let entity = entity.clone();
                move |dragged_value, window, cx| {
                    entity.update(cx, |this, _| {
                        this.can_drop_value(dragged_value, &current_directory, window.modifiers())
                    })
                }
            })
            .drag_over::<DraggedEntries>(|style, _, _, _| style.bg(rgb(0xf7fbff)))
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(0xf7fbff)))
            .on_drag_move::<DraggedEntries>({
                let current_directory = current_directory.clone();
                let entity = entity.clone();
                move |event: &DragMoveEvent<DraggedEntries>, window, cx| {
                    update_drag_cursor_if_hovered(&entity, event, &current_directory, window, cx);
                }
            })
            .on_drag_move::<ExternalPaths>({
                let current_directory = current_directory.clone();
                let entity = entity.clone();
                move |event: &DragMoveEvent<ExternalPaths>, window, cx| {
                    update_drag_cursor_if_hovered(&entity, event, &current_directory, window, cx);
                }
            })
            .on_drop(cx.listener({
                let current_directory = current_directory.clone();
                move |this, dragged: &DraggedEntries, window, cx| {
                    this.drop_internal_entries(
                        dragged,
                        current_directory.clone(),
                        window.modifiers(),
                    );
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_drop(cx.listener({
                let current_directory = current_directory.clone();
                move |this, paths: &ExternalPaths, window, cx| {
                    this.drop_external_paths(
                        paths.paths(),
                        current_directory.clone(),
                        window.modifiers(),
                    );
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(render_empty_folder_message())
            .into_any_element()
    }

    fn render_mouse_selection_box(&self) -> AnyElement {
        let Some(selection_box) = self.active_selection_box() else {
            return div().into_any_element();
        };

        let bounds = selection_box_bounds(selection_box);
        div()
            .absolute()
            .left(bounds.origin.x)
            .top(bounds.origin.y)
            .w(bounds.size.width)
            .h(bounds.size.height)
            .bg(rgb(0x2B80D5))
            .opacity(0.5)
            .border_2()
            .border_color(rgb(0x0078d7))
            .into_any_element()
    }

    fn render_mouse_selection_hit_layer(&self, cx: &mut Context<Self>) -> AnyElement {
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

                        let local_position = local_point(event.position, &bounds);
                        let modifiers = SelectionModifiers::from_gpui(event.modifiers);
                        let _ = entity.update(cx, |this, cx| {
                            if this.begin_mouse_selection_drag_for_intent(local_position, modifiers)
                            {
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

                        let local_position = local_point(event.position, &bounds);
                        let viewport_size = viewport_size(&bounds);
                        let _ = entity.update(cx, |this, cx| {
                            if this.mouse_selection_drag.is_none() {
                                return;
                            }

                            this.update_mouse_selection_drag(local_position, viewport_size);
                            cx.notify();
                        });
                    }
                });

                window.on_mouse_event(move |event: &MouseUpEvent, _, _, cx| {
                    if event.button != MouseButton::Left {
                        return;
                    }

                    let _ = entity.update(cx, |this, cx| {
                        this.end_mouse_selection_drag();
                        cx.notify();
                    });
                });
            },
        )
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .w_full()
        .h_full()
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
            .on_action(cx.listener(Self::handle_cancel_drag))
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
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .child(self.render_sidebar(scale_factor, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .h_full()
                            .overflow_hidden()
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
                                    ExplorerContentBranch::Empty => {
                                        div().child(self.render_empty_folder(cx))
                                    }
                                    ExplorerContentBranch::List => {
                                        div().child(self.render_list(cx))
                                    }
                                }
                                .id("explorer-scroll")
                                .flex_1()
                                .w_full()
                                .overflow_hidden(),
                            ),
                    ),
            )
    }
}

fn sidebar_separator() -> Div {
    div()
        .h(px(1.0))
        .mx(px(12.0))
        .my(px(12.0))
        .bg(rgb(0xe5e5e5))
        .flex_shrink_0()
}

fn sidebar_item_icon(item: SidebarItem, scale_factor: f32) -> AnyElement {
    match item.kind {
        SidebarItemKind::UserDirectory(UserDirectoryKind::Desktop) => {
            desktop_folder_icon(item.path, scale_factor)
        }
        SidebarItemKind::UserDirectory(UserDirectoryKind::Downloads) => {
            downloads_folder_icon(item.path, scale_factor)
        }
        SidebarItemKind::UserDirectory(UserDirectoryKind::Home) => {
            folder_icon(scale_factor).into_any_element()
        }
        SidebarItemKind::Drive => drive_icon(scale_factor).into_any_element(),
    }
}

impl Focusable for ExplorerView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle
            .clone()
            .expect("ExplorerView must be constructed with a FocusHandle before rendering")
    }
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
    let navigation_target = target.clone();
    let destination = DropDestination::Directory {
        item_path: target.clone(),
        target_path: target,
    };
    let entity = cx.entity();

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
        .can_drop({
            let destination = destination.clone();
            let entity = entity.clone();
            move |dragged_value, window, cx| {
                entity.update(cx, |this, _| {
                    this.can_drop_value(dragged_value, &destination, window.modifiers())
                })
            }
        })
        .drag_over::<DraggedEntries>(|style, _, _, _| {
            style.bg(rgb(0xe5f3ff)).border_color(rgb(0x0078d7))
        })
        .drag_over::<ExternalPaths>(|style, _, _, _| {
            style.bg(rgb(0xe5f3ff)).border_color(rgb(0x0078d7))
        })
        .on_drag_move::<DraggedEntries>({
            let destination = destination.clone();
            let entity = entity.clone();
            move |event: &DragMoveEvent<DraggedEntries>, window, cx| {
                update_drag_cursor_if_hovered(&entity, event, &destination, window, cx);
            }
        })
        .on_drag_move::<ExternalPaths>({
            let destination = destination.clone();
            let entity = entity.clone();
            move |event: &DragMoveEvent<ExternalPaths>, window, cx| {
                update_drag_cursor_if_hovered(&entity, event, &destination, window, cx);
            }
        })
        .on_drop(cx.listener({
            let destination = destination.clone();
            move |this, dragged: &DraggedEntries, window, cx| {
                this.drop_internal_entries(dragged, destination.clone(), window.modifiers());
                cx.stop_propagation();
                cx.notify();
            }
        }))
        .on_drop(cx.listener({
            let destination = destination.clone();
            move |this, paths: &ExternalPaths, window, cx| {
                this.drop_external_paths(paths.paths(), destination.clone(), window.modifiers());
                cx.stop_propagation();
                cx.notify();
            }
        }))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.navigate_to_directory(navigation_target.clone(), HistoryMode::Record);
            cx.stop_propagation();
            cx.notify();
        }))
        .child(SharedString::from(segment.label))
        .flex_shrink_0()
        .into_any_element()
}

fn update_drag_cursor_if_hovered<T: 'static>(
    entity: &Entity<ExplorerView>,
    event: &DragMoveEvent<T>,
    destination: &DropDestination,
    window: &mut Window,
    cx: &mut App,
) {
    if !event.bounds.contains(&event.event.position) {
        return;
    }

    let cursor = entity.update(cx, |this, _| {
        this.drag_cursor_for_value(event.dragged_item(), destination, window.modifiers())
    });
    cx.set_active_drag_cursor_style(cursor, window);
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

fn name_cell(entry: &FileEntry, scale_factor: f32, window: &Window) -> Div {
    let list_viewport_width = (f32::from(window.bounds().size.width) - SIDEBAR_WIDTH).max(0.0);
    let text_width = available_filename_text_width(list_viewport_width, scale_factor);
    let filename = truncated_text(&entry.name, text_width, 0x000000, window);

    div()
        .flex()
        .items_center()
        .h_full()
        .flex_1()
        .min_w(px(COLUMN_NAME_MIN_WIDTH))
        .overflow_hidden()
        .pl(px(NAME_CELL_LEFT_PADDING))
        .child(if entry.uses_directory_shortcut_icon() {
            directory_shortcut_icon(scale_factor)
        } else if entry.is_directory_like() {
            folder_icon(scale_factor)
        } else {
            file_icon(scale_factor)
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .ml(device_px(NAME_ICON_TEXT_GAP_PHYSICAL, scale_factor))
                .truncate()
                .text_size(px(NAME_TEXT_SIZE))
                .child(filename),
        )
}

fn effective_name_column_width(viewport_width: f32) -> f32 {
    let fixed_columns_width =
        COLUMN_DATE_WIDTH + COLUMN_TYPE_WIDTH + COLUMN_SIZE_WIDTH + SCROLLBAR_GUTTER_WIDTH;

    (viewport_width - fixed_columns_width).max(COLUMN_NAME_MIN_WIDTH)
}

fn filename_text_width(name_column_width: f32, scale_factor: f32) -> f32 {
    let icon_width = device_px_value(FILE_ICON_SLOT_WIDTH_PHYSICAL, scale_factor);
    let gap_width = device_px_value(NAME_ICON_TEXT_GAP_PHYSICAL, scale_factor);

    (name_column_width - NAME_CELL_LEFT_PADDING - icon_width - gap_width).max(0.0)
}

fn available_filename_text_width(viewport_width: f32, scale_factor: f32) -> f32 {
    filename_text_width(effective_name_column_width(viewport_width), scale_factor)
}

fn truncated_text(
    text: &str,
    available_width: f32,
    text_color: u32,
    window: &Window,
) -> SharedString {
    let name_font = font(".SystemUIFont");
    let mut runs = vec![TextRun {
        len: text.len(),
        font: name_font.clone(),
        color: rgb(text_color).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }];

    window
        .text_system()
        .line_wrapper(name_font, px(NAME_TEXT_SIZE))
        .truncate_line(
            SharedString::from(text.to_owned()),
            px(available_width),
            NAME_TRUNCATION_SUFFIX,
            &mut runs,
        )
}

fn text_cell_width(column_width: f32) -> f32 {
    (column_width - TEXT_CELL_HORIZONTAL_PADDING * 2.0).max(0.0)
}

fn selection_modifiers_for_click(event: &ClickEvent) -> SelectionModifiers {
    match event {
        ClickEvent::Mouse(event) => SelectionModifiers::from_gpui(event.down.modifiers),
        ClickEvent::Keyboard(_) => SelectionModifiers::default(),
    }
}

fn text_cell(text: String, width: f32, right: bool, window: &Window) -> Div {
    let text = truncated_text(&text, text_cell_width(width), TEXT_CELL_TEXT_COLOR, window);

    let cell = div()
        .flex()
        .items_center()
        .h_full()
        .w(px(width))
        .flex_shrink_0()
        .overflow_hidden()
        .px(px(TEXT_CELL_HORIZONTAL_PADDING))
        .text_size(px(12.0))
        .text_color(rgb(TEXT_CELL_TEXT_COLOR))
        .child(text);

    if right {
        cell.justify_end()
    } else {
        cell.justify_start()
    }
}

#[cfg(test)]
mod tests {

    use gpui::{
        ClickEvent, KeyboardClickEvent, Modifiers, MouseClickEvent, MouseDownEvent, MouseUpEvent,
    };

    use crate::explorer::{
        constants::{
            COLUMN_DATE_WIDTH, COLUMN_NAME_MIN_WIDTH, COLUMN_SIZE_WIDTH, COLUMN_TYPE_WIDTH,
            EMPTY_FOLDER_MESSAGE, EMPTY_FOLDER_TEXT_SIZE, EMPTY_FOLDER_TOP_MARGIN,
            FILE_ICON_SLOT_WIDTH_PHYSICAL, NAV_BUTTON_ACTIVE_OPACITY, SCROLLBAR_GUTTER_WIDTH,
        },
        selection::SelectionModifiers,
    };

    use super::{
        NAME_CELL_LEFT_PADDING, NAME_ICON_TEXT_GAP_PHYSICAL, available_filename_text_width,
        filename_text_width, selection_modifiers_for_click, text_cell_width,
    };

    #[test]
    fn nav_button_active_opacity_dims_button() {
        assert_eq!(NAV_BUTTON_ACTIVE_OPACITY, 0.7);
        assert!(NAV_BUTTON_ACTIVE_OPACITY < 1.0);
    }

    #[test]
    fn empty_folder_message_uses_compact_text() {
        assert_eq!(EMPTY_FOLDER_TEXT_SIZE, 12.0);
        assert_eq!(EMPTY_FOLDER_TOP_MARGIN, 20.0);
        assert_eq!(EMPTY_FOLDER_MESSAGE, "This folder is empty.");
    }

    #[test]
    fn name_text_width_uses_remaining_viewport_width() {
        let name_column_width = 400.0;
        let viewport_width = name_column_width
            + COLUMN_DATE_WIDTH
            + COLUMN_TYPE_WIDTH
            + COLUMN_SIZE_WIDTH
            + SCROLLBAR_GUTTER_WIDTH;

        assert_eq!(
            available_filename_text_width(viewport_width, 1.0),
            name_column_width
                - NAME_CELL_LEFT_PADDING
                - FILE_ICON_SLOT_WIDTH_PHYSICAL
                - NAME_ICON_TEXT_GAP_PHYSICAL
        );
    }

    #[test]
    fn name_text_width_respects_name_column_minimum() {
        assert_eq!(
            available_filename_text_width(100.0, 1.0),
            COLUMN_NAME_MIN_WIDTH
                - NAME_CELL_LEFT_PADDING
                - FILE_ICON_SLOT_WIDTH_PHYSICAL
                - NAME_ICON_TEXT_GAP_PHYSICAL
        );
    }

    #[test]
    fn name_text_width_clamps_when_chrome_consumes_column() {
        assert_eq!(filename_text_width(10.0, 1.0), 0.0);
    }

    #[test]
    fn text_cell_width_subtracts_horizontal_padding() {
        assert_eq!(text_cell_width(COLUMN_TYPE_WIDTH), COLUMN_TYPE_WIDTH - 16.0);
    }

    #[test]
    fn text_cell_width_clamps_when_padding_consumes_column() {
        assert_eq!(text_cell_width(10.0), 0.0);
    }

    #[test]
    fn click_selection_modifiers_use_mouse_down_shift() {
        let event = ClickEvent::Mouse(MouseClickEvent {
            down: MouseDownEvent {
                modifiers: Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
                ..MouseDownEvent::default()
            },
            up: MouseUpEvent::default(),
        });

        assert_eq!(
            selection_modifiers_for_click(&event),
            SelectionModifiers {
                toggle: false,
                extend: true,
            }
        );
    }

    #[test]
    fn click_selection_modifiers_use_mouse_down_secondary_modifier() {
        let event = ClickEvent::Mouse(MouseClickEvent {
            down: MouseDownEvent {
                modifiers: Modifiers {
                    control: true,
                    platform: cfg!(target_os = "macos"),
                    ..Modifiers::default()
                },
                ..MouseDownEvent::default()
            },
            up: MouseUpEvent::default(),
        });

        assert_eq!(
            selection_modifiers_for_click(&event),
            SelectionModifiers {
                toggle: true,
                extend: false,
            }
        );
    }

    #[test]
    fn keyboard_click_selection_modifiers_are_default() {
        let event = ClickEvent::Keyboard(KeyboardClickEvent::default());

        assert_eq!(
            selection_modifiers_for_click(&event),
            SelectionModifiers::default()
        );
    }
}
