use std::{collections::BTreeSet, ops::Range, path::PathBuf, sync::Arc};

use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, CursorStyle, Div, DragMoveEvent, Entity,
    ExternalPaths, FocusHandle, Focusable, Image, IntoElement, ModifiersChangedEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection, Pixels, Point, Render,
    ScrollWheelEvent, SharedString, TextRun, Window, canvas, div, font, prelude::*, px, rgb,
    transparent_black, uniform_list,
};

use crate::explorer::{
    address_bar::{
        ADDRESS_SUGGESTION_ROW_HEIGHT, ADDRESS_SUGGESTIONS_VERTICAL_PADDING, address_text_element,
    },
    breadcrumb::{
        BreadcrumbSegment, VisibleBreadcrumb, directory_bar_available_width,
        visible_breadcrumb_for_path,
    },
    clipboard::file_clipboard_from_item,
    constants::{
        COLUMN_DATE_WIDTH, COLUMN_NAME_MIN_WIDTH, COLUMN_SIZE_WIDTH, COLUMN_TYPE_WIDTH,
        DIRECTORY_BAR_ELLIPSIS, DIRECTORY_BAR_HEIGHT, DIRECTORY_BAR_HORIZONTAL_PADDING,
        DIRECTORY_BAR_RADIUS, DIRECTORY_BAR_SEGMENT_HORIZONTAL_PADDING, DIRECTORY_BAR_SEPARATOR,
        DIRECTORY_BAR_TEXT_SIZE, EMPTY_FOLDER_MESSAGE, EMPTY_FOLDER_TEXT_SIZE,
        EMPTY_FOLDER_TOP_MARGIN, FILE_ICON_SLOT_HEIGHT_PHYSICAL, FILE_ICON_SLOT_WIDTH_PHYSICAL,
        HEADER_HEIGHT, NAV_BUTTON_ACTIVE_OPACITY, NAV_BUTTON_HOVER_BG, NAV_BUTTON_SIZE,
        NAV_ICON_DISABLED_COLOR, NAV_ICON_ENABLED_COLOR, NAV_ICON_TEXT_SIZE, NAVBAR_HEIGHT,
        NAVBAR_HORIZONTAL_PADDING, NAVBAR_ITEM_GAP, OPEN_ERROR_HORIZONTAL_PADDING,
        OPEN_ERROR_VERTICAL_PADDING, RECURSIVE_SEARCH_ROW_HEIGHT, ROW_HEIGHT,
        SCROLLBAR_ARROW_HEIGHT, SCROLLBAR_GUTTER_WIDTH, SCROLLBAR_THUMB_ACTIVE_BG,
        SCROLLBAR_THUMB_BG, SCROLLBAR_THUMB_HOVER_BG, SCROLLBAR_THUMB_HOVER_WIDTH,
        SCROLLBAR_THUMB_WIDTH, SCROLLBAR_TRACK_BG, SEARCH_BAR_MAX_WIDTH, SEARCH_BAR_MIN_WIDTH,
        SEARCH_NO_MATCHES_MESSAGE, SEARCH_WORKING_MESSAGE, SIDEBAR_HORIZONTAL_PADDING,
        SIDEBAR_ICON_TEXT_GAP_PHYSICAL, SIDEBAR_ROW_HEIGHT, SIDEBAR_TEXT_SIZE, SIDEBAR_WIDTH,
        STATUS_BAR_HEIGHT, STATUS_BAR_HORIZONTAL_PADDING, STATUS_BAR_SEPARATOR_COLOR,
        STATUS_BAR_TEXT_COLOR, STATUS_BAR_TEXT_SIZE, UTILITY_BAR_HEIGHT,
        UTILITY_BAR_HORIZONTAL_PADDING, UTILITY_BAR_ITEM_GAP, UTILITY_BUTTON_HEIGHT,
        UTILITY_ICON_BUTTON_SIZE, UTILITY_MENU_ROW_HEIGHT, UTILITY_MENU_WIDTH,
        effective_name_column_width,
    },
    drag_drop::{
        DragPreview, DraggedEntries, DropDestination, DropIndicator, FileOperationKind,
        drop_indicator_origin, row_drop_destination_for_entry,
    },
    entry::FileEntry,
    formatting::{format_modified, format_size},
    icons::{
        NavIcon, applications_sidebar_icon, bin_sidebar_icon, desktop_folder_icon, device_px,
        device_px_value, directory_shortcut_icon, documents_folder_icon, downloads_folder_icon,
        drive_icon, file_icon, folder_icon, image_icon, nav_icon_font,
    },
    mouse_selection::{local_point, selection_box_bounds, viewport_size},
    navigation::{EntryAction, HistoryMode},
    rename::{ActiveTextInput, rename_text_element},
    scrollbar::{ScrollbarArrow, scrollbar_arrow_button, scrollbar_header_spacer},
    search::search_text_element,
    selection::SelectionModifiers,
    sidebar::{
        MacosSystemLocationKind, SidebarItem, SidebarItemKind, UserDirectoryKind, sidebar_sections,
    },
    view::{ExplorerContentBranch, ExplorerView, ExplorerViewEvent, UtilityMenu},
};

const NAME_CELL_LEFT_PADDING: f32 = 16.0;
const NAME_ICON_TEXT_GAP_PHYSICAL: f32 = 8.0;
const NAME_TEXT_SIZE: f32 = 12.0;
const CUT_ITEM_OPACITY: f32 = 0.7;
const TEXT_CELL_HORIZONTAL_PADDING: f32 = 8.0;
const TEXT_CELL_TEXT_COLOR: u32 = 0x595959;
const NAME_TRUNCATION_SUFFIX: &str = "…";
const DROP_INDICATOR_TEXT_SIZE: f32 = 12.0;
const DROP_INDICATOR_BLUE: u32 = 0x0078d7;
const DROP_INDICATOR_TEXT_COLOR: u32 = 0x1f1f1f;
const DROP_INDICATOR_TARGET_MAX_WIDTH: f32 = 180.0;
const UTILITY_TEXT_BUTTON_WIDTH: f32 = 92.0;
const UTILITY_SEPARATOR_OUTER_WIDTH: f32 = 17.0;
const UTILITY_NEW_MENU_LEFT: f32 = UTILITY_BAR_HORIZONTAL_PADDING;
const UTILITY_VIEW_MENU_LEFT: f32 = UTILITY_BAR_HORIZONTAL_PADDING
    + UTILITY_TEXT_BUTTON_WIDTH
    + UTILITY_SEPARATOR_OUTER_WIDTH
    + (UTILITY_ICON_BUTTON_SIZE * 5.0)
    + UTILITY_SEPARATOR_OUTER_WIDTH
    + (UTILITY_BAR_ITEM_GAP * 8.0);
const UTILITY_ICON_CUT: &str = "\u{E8C6}";
const UTILITY_ICON_COPY: &str = "\u{E8C8}";
const UTILITY_ICON_PASTE: &str = "\u{E77F}";
const UTILITY_ICON_RENAME: &str = "\u{E8AC}";
const UTILITY_ICON_DELETE: &str = "\u{E74D}";
const UTILITY_ICON_FILE: &str = "\u{E8A5}";
const UTILITY_ICON_CHEVRON_DOWN: &str = "\u{E70D}";
const UTILITY_ICON_CHECK: &str = "\u{E73E}";
const UTILITY_TEXT_BUTTON_ICON_SIZE: f32 = 16.0;
const UTILITY_NEW_ICON_CIRCLE_SIZE: f32 = 14.0;
const UTILITY_NEW_ICON_PLUS_SIZE: f32 = 8.0;
const UTILITY_NEW_ICON_PLUS_THICKNESS: f32 = 2.0;
const UTILITY_NEW_ICON_PLUS_CENTER_OFFSET: f32 =
    (UTILITY_NEW_ICON_PLUS_SIZE - UTILITY_NEW_ICON_PLUS_THICKNESS) / 2.0;
const UTILITY_NEW_ICON_BLUE: u32 = 0x0078d4;
const UTILITY_NEW_ICON_BLACK: u32 = 0x555555;
const UTILITY_VIEW_ICON_LINE_COLOR: u32 = 0x555555;
const UTILITY_VIEW_ICON_LINE_TOPS: [f32; 4] = [3.5, 6.5, 9.5, 12.5];
const RECURSIVE_SEARCH_ICON: &str = "\u{E8B7}";
const RECURSIVE_SEARCH_PATH_TEXT_SIZE: f32 = 11.0;
const RECURSIVE_SEARCH_PATH_TEXT_COLOR: u32 = 0x6f6f6f;

impl ExplorerView {
    pub(super) fn entry_row_height(&self) -> f32 {
        if self.recursive_search_results_active() {
            RECURSIVE_SEARCH_ROW_HEIGHT
        } else {
            ROW_HEIGHT
        }
    }

    fn render_navbar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
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
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.navigate_back_with_watcher(cx);
                    cx.notify();
                }),
            ))
            .child(nav_button(
                "forward",
                NavIcon::Forward,
                self.can_go_forward(),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.navigate_forward_with_watcher(cx);
                    cx.notify();
                }),
            ))
            .child(nav_button(
                "up",
                NavIcon::Up,
                self.can_go_up(),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.navigate_up_with_watcher(cx);
                    cx.notify();
                }),
            ))
            .child(nav_button(
                "refresh",
                NavIcon::Refresh,
                true,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.reload();
                    this.refresh_search_after_external_change(cx);
                    cx.notify();
                }),
            ))
            .child(if self.address_bar_is_editing() {
                editable_directory_bar(self.active_address_focus_handle(), cx)
            } else {
                directory_bar(breadcrumb, cx)
            })
            .child(self.render_search_bar(cx))
    }

    fn render_search_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let editing = self.search_is_editing();
        let text = if self.search_query().is_empty() {
            self.search_placeholder()
        } else {
            self.search_query().to_owned()
        };

        div()
            .id("search-bar")
            .debug_selector(|| "search-bar".to_owned())
            .key_context("ExplorerSearchInput")
            .flex()
            .flex_row()
            .items_center()
            .h(px(DIRECTORY_BAR_HEIGHT))
            .w(px(SEARCH_BAR_MAX_WIDTH))
            .min_w(px(SEARCH_BAR_MIN_WIDTH))
            .overflow_hidden()
            .rounded(px(DIRECTORY_BAR_RADIUS))
            .border_b_2()
            .border_color(rgb(if editing { 0x0078d7 } else { 0xf8f8f8 }))
            .bg(rgb(0xffffff))
            .pl(px(12.0))
            .pr(px(10.0))
            .gap(px(8.0))
            .cursor(CursorStyle::IBeam)
            .text_size(px(DIRECTORY_BAR_TEXT_SIZE))
            .line_height(px(20.0))
            .text_color(rgb(if self.search_query().is_empty() && !editing {
                0x767676
            } else {
                0x1f1f1f
            }))
            .when_some(self.active_search_focus_handle().as_ref(), |this, focus| {
                this.track_focus(focus)
            })
            .when(!editing, |this| {
                this.on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.start_search_edit(window, cx);
                    cx.stop_propagation();
                    cx.notify();
                }))
            })
            .when(editing, |this| {
                this.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.on_search_mouse_down(event);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    this.on_search_mouse_move(event);
                    cx.stop_propagation();
                    cx.notify();
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseUpEvent, _, cx| {
                        this.on_search_mouse_up(event);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseUpEvent, window, cx| {
                        if this.active_text_input_is_selecting() {
                            this.on_search_mouse_up(event);
                            cx.stop_propagation();
                            cx.notify();
                        } else if this.finish_active_input_for_pointer_interaction(
                            ActiveTextInput::Search,
                            window,
                            cx,
                        ) {
                            cx.notify();
                        }
                    }),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .when(editing, |this| this.child(search_text_element(entity)))
                    .when(!editing, |this| {
                        this.truncate().child(SharedString::from(text))
                    }),
            )
            .child(search_bar_icon_button(
                "recursive-search-toggle",
                RECURSIVE_SEARCH_ICON,
                self.recursive_search_is_enabled(),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.toggle_recursive_search(cx);
                    cx.stop_propagation();
                    cx.notify();
                }),
            ))
            .child(
                div()
                    .flex_shrink_0()
                    .font(nav_icon_font())
                    .text_size(px(13.0))
                    .text_color(rgb(0x5f5f5f))
                    .child("\u{E721}"),
            )
            .into_any_element()
    }

    fn render_utility_bar(&self, cx: &mut Context<Self>) -> Div {
        let has_selection = !self.selection.selected_indices.is_empty();
        let can_rename = self.can_start_selected_rename();
        let clipboard = cx.read_from_clipboard();
        let can_paste = clipboard_has_file_clipboard(clipboard.as_ref());

        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(UTILITY_BAR_HEIGHT))
            .w_full()
            .flex_shrink_0()
            .bg(rgb(0xf8f8f8))
            .border_t_1()
            .border_b_1()
            .border_color(rgb(0xe9e9e9))
            .px(px(UTILITY_BAR_HORIZONTAL_PADDING))
            .gap(px(UTILITY_BAR_ITEM_GAP))
            .child(utility_text_button(
                "utility-new",
                Some(utility_new_icon().into_any_element()),
                "New",
                self.open_utility_menu == Some(UtilityMenu::New),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.cancel_pending_click_rename();
                    this.open_utility_menu = if this.open_utility_menu == Some(UtilityMenu::New) {
                        None
                    } else {
                        Some(UtilityMenu::New)
                    };
                    cx.stop_propagation();
                    cx.notify();
                }),
            ))
            .child(utility_separator())
            .child(utility_icon_button(
                "utility-cut",
                UTILITY_ICON_CUT,
                has_selection,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_utility_menu = None;
                    if this.commit_active_rename_before_interaction(window, cx) {
                        this.cut_selected_to_clipboard(cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            ))
            .child(utility_icon_button(
                "utility-copy",
                UTILITY_ICON_COPY,
                has_selection,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_utility_menu = None;
                    if this.commit_active_rename_before_interaction(window, cx) {
                        this.copy_selected_to_clipboard(cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            ))
            .child(utility_icon_button(
                "utility-paste",
                UTILITY_ICON_PASTE,
                can_paste,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_utility_menu = None;
                    if this.commit_active_rename_before_interaction(window, cx) {
                        this.paste_clipboard_files(cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            ))
            .child(utility_icon_button(
                "utility-rename",
                UTILITY_ICON_RENAME,
                can_rename,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_utility_menu = None;
                    if this.commit_active_rename_before_interaction(window, cx) {
                        this.start_rename_selected(window, cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            ))
            .child(utility_icon_button(
                "utility-delete",
                UTILITY_ICON_DELETE,
                has_selection,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_utility_menu = None;
                    if this.commit_active_rename_before_interaction(window, cx) {
                        this.trash_selected_paths(cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            ))
            .child(utility_separator())
            .child(utility_text_button(
                "utility-view",
                Some(utility_view_icon().into_any_element()),
                "View",
                self.open_utility_menu == Some(UtilityMenu::View),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.cancel_pending_click_rename();
                    this.open_utility_menu = if this.open_utility_menu == Some(UtilityMenu::View) {
                        None
                    } else {
                        Some(UtilityMenu::View)
                    };
                    cx.stop_propagation();
                    cx.notify();
                }),
            ))
    }

    fn render_utility_menu_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.open_utility_menu?;
        let left = match menu {
            UtilityMenu::New => UTILITY_NEW_MENU_LEFT,
            UtilityMenu::View => UTILITY_VIEW_MENU_LEFT,
        };

        let menu = match menu {
            UtilityMenu::New => utility_dropdown()
                .child(utility_menu_row(
                    "utility-new-folder",
                    Some(folder_icon(1.4).into_any_element()),
                    "Folder",
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.open_utility_menu = None;
                        if this.commit_active_rename_before_interaction(window, cx) {
                            this.create_new_folder(window, cx);
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ))
                .child(utility_menu_row(
                    "utility-new-file",
                    Some(utility_menu_glyph_icon(UTILITY_ICON_FILE)),
                    "File",
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.open_utility_menu = None;
                        if this.commit_active_rename_before_interaction(window, cx) {
                            this.create_new_file(window, cx);
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )),
            UtilityMenu::View => utility_dropdown()
                .child(utility_checkbox_row(
                    "utility-hidden-files",
                    self.show_hidden_files,
                    "Hidden files",
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.open_utility_menu = None;
                        if this.commit_active_rename_before_interaction(window, cx) {
                            this.show_hidden_files = !this.show_hidden_files;
                            this.invalidate_recursive_search_cache();
                            this.reload();
                            this.refresh_search_after_external_change(cx);
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ))
                .child(utility_checkbox_row(
                    "utility-file-name-extensions",
                    self.show_file_name_extensions,
                    "File Name extensions",
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.open_utility_menu = None;
                        if this.commit_active_rename_before_interaction(window, cx) {
                            this.show_file_name_extensions = !this.show_file_name_extensions;
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )),
        };

        let click_catcher = div()
            .id("utility-menu-click-catcher")
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .size_full()
            .cursor_default()
            .bg(transparent_black())
            .occlude()
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.cancel_pending_click_rename();
                this.open_utility_menu = None;
                cx.stop_propagation();
                cx.notify();
            }));

        Some(
            div()
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .size_full()
                .child(click_catcher)
                .child(
                    menu.absolute()
                        .left(px(left))
                        .top(px(NAVBAR_HEIGHT + UTILITY_BAR_HEIGHT - 2.0)),
                )
                .into_any_element(),
        )
    }

    fn render_address_suggestions_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let address = self.active_address_bar.as_ref()?;
        if address.suggestions.is_empty() {
            return None;
        }

        let left = NAVBAR_HORIZONTAL_PADDING + (NAV_BUTTON_SIZE * 4.0) + (NAVBAR_ITEM_GAP * 4.0);
        let width =
            (f32::from(window.bounds().size.width) - left - NAVBAR_HORIZONTAL_PADDING).max(0.0);
        let top = ((NAVBAR_HEIGHT - DIRECTORY_BAR_HEIGHT) / 2.0) + DIRECTORY_BAR_HEIGHT;

        let mut dropdown = div()
            .w(px(width))
            .py(px(ADDRESS_SUGGESTIONS_VERTICAL_PADDING))
            .rounded(px(6.0))
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(0xd8d8d8))
            .shadow_md()
            .occlude();

        let viewport_height = address.suggestions_viewport_height();
        let scroll_top = address.suggestions_scroll_top;
        let has_scrollbar = address.suggestions_scrollbar_metrics().is_some();

        let mut rows = div()
            .absolute()
            .top(px(-scroll_top))
            .left(px(0.0))
            .right(px(0.0));
        for (index, suggestion) in address.suggestions.iter().enumerate() {
            let highlighted = address.highlighted_suggestion == Some(index);
            rows = rows.child(address_suggestion_row(
                index,
                suggestion.label.clone(),
                suggestion.path.clone(),
                highlighted,
                cx,
            ));
        }

        let viewport = div()
            .relative()
            .flex_1()
            .h(px(viewport_height))
            .min_w(px(0.0))
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let delta_y = event.delta.pixel_delta(px(ADDRESS_SUGGESTION_ROW_HEIGHT)).y;
                if let Some(address) = this.active_address_bar.as_mut() {
                    address.scroll_suggestions_by(-f32::from(delta_y));
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(rows);

        dropdown = dropdown.child(
            div()
                .flex()
                .flex_row()
                .h(px(viewport_height))
                .child(viewport)
                .when(has_scrollbar, |this| {
                    this.child(self.render_address_suggestions_scrollbar(cx))
                }),
        );

        Some(
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .child(dropdown)
                .into_any_element(),
        )
    }

    fn render_address_suggestions_scrollbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(address) = self.active_address_bar.as_ref() else {
            return div().into_any_element();
        };
        let Some(metrics) = address.suggestions_scrollbar_metrics() else {
            return div().into_any_element();
        };

        let hovered_or_dragged =
            address.suggestions_scrollbar_hovered || address.suggestions_scrollbar_drag.is_some();
        let thumb_width = if hovered_or_dragged {
            SCROLLBAR_THUMB_HOVER_WIDTH
        } else {
            SCROLLBAR_THUMB_WIDTH
        };
        let thumb_right = (SCROLLBAR_GUTTER_WIDTH - thumb_width) / 2.0;
        let thumb_color = if address.suggestions_scrollbar_drag.is_some() {
            SCROLLBAR_THUMB_ACTIVE_BG
        } else if hovered_or_dragged {
            SCROLLBAR_THUMB_HOVER_BG
        } else {
            SCROLLBAR_THUMB_BG
        };
        let bottom_arrow_top = (metrics.viewport_height - SCROLLBAR_ARROW_HEIGHT).max(0.0);

        div()
            .id("address-suggestions-scrollbar")
            .relative()
            .w(px(SCROLLBAR_GUTTER_WIDTH))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(SCROLLBAR_TRACK_BG))
            .cursor_default()
            .block_mouse_except_scroll()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if let Some(address) = this.active_address_bar.as_mut() {
                    address.suggestions_scrollbar_hovered = *hovered;
                    cx.notify();
                }
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
            .child(self.render_address_suggestions_scrollbar_hit_layer(cx))
            .into_any_element()
    }

    fn address_suggestions_contain_position(
        &self,
        position: Point<Pixels>,
        window: &Window,
    ) -> bool {
        let Some(address) = self.active_address_bar.as_ref() else {
            return false;
        };
        if address.suggestions.is_empty() {
            return false;
        }

        let left = NAVBAR_HORIZONTAL_PADDING + (NAV_BUTTON_SIZE * 4.0) + (NAVBAR_ITEM_GAP * 4.0);
        let right = f32::from(window.bounds().size.width) - NAVBAR_HORIZONTAL_PADDING;
        let top = ((NAVBAR_HEIGHT - DIRECTORY_BAR_HEIGHT) / 2.0) + DIRECTORY_BAR_HEIGHT;
        let bottom = top
            + address.suggestions_viewport_height()
            + (ADDRESS_SUGGESTIONS_VERTICAL_PADDING * 2.0);
        let x = f32::from(position.x);
        let y = f32::from(position.y);
        x >= left && x <= right && y >= top && y <= bottom
    }

    fn render_address_suggestions_scrollbar_hit_layer(&self, cx: &mut Context<Self>) -> AnyElement {
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
                            let Some(address) = this.active_address_bar.as_mut() else {
                                return;
                            };
                            if let Some(metrics) = address.suggestions_scrollbar_metrics() {
                                address.handle_suggestions_scrollbar_mouse_down(local_y, metrics);
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
                            let Some(address) = this.active_address_bar.as_mut() else {
                                return;
                            };
                            if address.suggestions_scrollbar_drag.is_none() {
                                return;
                            }

                            if let Some(metrics) = address.suggestions_scrollbar_metrics() {
                                address.handle_suggestions_scrollbar_drag(local_y, metrics);
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
                        if let Some(address) = this.active_address_bar.as_mut()
                            && address.suggestions_scrollbar_drag.take().is_some()
                        {
                            cx.notify();
                        }
                    });
                });
            },
        )
        .size_full()
        .into_any_element()
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

        if !children.is_empty() && !sections.macos_system_locations.is_empty() {
            children.push(sidebar_separator().into_any_element());
        }

        for (index, item) in sections.macos_system_locations.into_iter().enumerate() {
            children.push(self.render_sidebar_row(index + 1_000, item, scale_factor, cx));
        }

        if !children.is_empty() && !sections.drives.is_empty() {
            children.push(sidebar_separator().into_any_element());
        }

        for (index, item) in sections.drives.into_iter().enumerate() {
            children.push(self.render_sidebar_row(index + 2_000, item, scale_factor, cx));
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
        let is_user_directory = matches!(item.kind, SidebarItemKind::UserDirectory(_));
        let is_bin = matches!(
            item.kind,
            SidebarItemKind::MacosSystemLocation(MacosSystemLocationKind::Bin)
        );
        let destination = DropDestination::Directory {
            item_path: path.clone(),
            target_path: path.clone(),
        };
        let entity = cx.entity();

        let mut row = div()
            .id(("explorer-sidebar-row", id))
            .flex()
            .flex_row()
            .items_center()
            .h(px(SIDEBAR_ROW_HEIGHT))
            .mx(px(8.0))
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
                this.navigate_to_sidebar_path_with_watcher(path.clone(), cx);
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
            );

        if is_user_directory {
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
                        this.clear_drop_indicator();
                        this.drop_internal_entries_and_open_dialog(
                            dragged,
                            destination.clone(),
                            window.modifiers(),
                            cx,
                        );
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .on_drop(cx.listener({
                    let destination = destination.clone();
                    move |this, paths: &ExternalPaths, window, cx| {
                        this.clear_drop_indicator();
                        this.drop_external_paths_and_open_dialog(
                            paths.paths(),
                            destination.clone(),
                            window.modifiers(),
                            cx,
                        );
                        cx.stop_propagation();
                        cx.notify();
                    }
                }));
        }

        if is_bin {
            row = row
                .can_drop({
                    let entity = entity.clone();
                    move |dragged_value, _, cx| {
                        entity.update(cx, |this, _| this.can_trash_drop_value(dragged_value))
                    }
                })
                .drag_over::<DraggedEntries>(|style, _, _, _| {
                    style.bg(rgb(0xe5f3ff)).border_color(rgb(0x0078d7))
                })
                .drag_over::<ExternalPaths>(|style, _, _, _| {
                    style.bg(rgb(0xe5f3ff)).border_color(rgb(0x0078d7))
                })
                .on_drop(cx.listener(|this, dragged: &DraggedEntries, _, cx| {
                    this.clear_drop_indicator();
                    this.request_trash_paths_with_confirmation(dragged.paths.clone(), cx);
                    cx.stop_propagation();
                    cx.notify();
                }))
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                    this.clear_drop_indicator();
                    this.request_trash_paths_with_confirmation(paths.paths().to_vec(), cx);
                    cx.stop_propagation();
                    cx.notify();
                }));
        }

        row.into_any_element()
    }

    fn render_row(
        &mut self,
        ix: usize,
        scale_factor: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entry = self.entries[ix].clone();
        let app_icon = self.app_icon_for_entry(&entry, cx);
        let is_selected = self.entry_is_selected(ix);
        let is_cut = self.entry_is_cut(&entry.path);
        let clicked_entry = entry.clone();
        let middle_clicked_entry = entry.clone();
        let selected_drag_payload = self
            .can_start_item_drag_for_index(ix)
            .then(|| self.dragged_entries_for_index(ix))
            .flatten();
        let individual_drag_payload = self
            .can_start_individual_item_drag_for_index(ix)
            .then(|| self.dragged_entry_for_index(ix))
            .flatten();
        let destination = row_drop_destination_for_entry(&entry);
        let entity = cx.entity();

        let mut row = div()
            .id(("explorer-entry", ix))
            .debug_selector(move || format!("explorer-entry-{ix}"))
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .h(px(self.entry_row_height()))
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
            .when(is_cut, |this| this.opacity(CUT_ITEM_OPACITY))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if !is_normal_entry_click(event) {
                    cx.stop_propagation();
                    return;
                }

                if this.suppress_next_click() {
                    this.cancel_pending_click_rename();
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }

                if !this.commit_active_rename_before_interaction(window, cx) {
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }

                if let Some(EntryAction::OpenFile(path)) = this.handle_entry_click_with_watcher(
                    &clicked_entry,
                    event.click_count(),
                    selection_modifiers_for_click(event),
                    cx,
                ) {
                    this.open_file_with_default_app(&path);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if !this.commit_active_rename_before_interaction(window, cx) {
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }

                    if let Some(path) = this.handle_entry_middle_click(
                        &middle_clicked_entry,
                        SelectionModifiers::from_gpui(event.modifiers),
                    ) {
                        cx.emit(ExplorerViewEvent::OpenDirectoryInNewTab(path));
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
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

        if let Some(drag_payload) = selected_drag_payload {
            let external_paths = ExternalPaths::new(drag_payload.paths.clone());
            row = row.on_drag_with_external_paths(drag_payload, external_paths, {
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
                        this.clear_drop_indicator();
                        this.drop_internal_entries_and_open_dialog(
                            dragged,
                            destination.clone(),
                            window.modifiers(),
                            cx,
                        );
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .on_drop(cx.listener({
                    let destination = destination.clone();
                    move |this, paths: &ExternalPaths, window, cx| {
                        this.clear_drop_indicator();
                        this.drop_external_paths_and_open_dialog(
                            paths.paths(),
                            destination.clone(),
                            window.modifiers(),
                            cx,
                        );
                        cx.stop_propagation();
                        cx.notify();
                    }
                }));
        } else {
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
                .drag_over::<DraggedEntries>(|style, _, _, _| style.bg(rgb(0xf7fbff)))
                .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(0xf7fbff)))
                .on_drop(cx.listener({
                    let destination = destination.clone();
                    move |this, dragged: &DraggedEntries, window, cx| {
                        this.clear_drop_indicator();
                        this.drop_internal_entries_and_open_dialog(
                            dragged,
                            destination.clone(),
                            window.modifiers(),
                            cx,
                        );
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .on_drop(cx.listener({
                    let destination = destination.clone();
                    move |this, paths: &ExternalPaths, window, cx| {
                        this.clear_drop_indicator();
                        this.drop_external_paths_and_open_dialog(
                            paths.paths(),
                            destination.clone(),
                            window.modifiers(),
                            cx,
                        );
                        cx.stop_propagation();
                        cx.notify();
                    }
                }));
        }

        let date_cell = text_cell(
            format_modified(entry.modified),
            COLUMN_DATE_WIDTH,
            false,
            window,
        );
        let type_cell = text_cell(entry.type_label(), COLUMN_TYPE_WIDTH, false, window);
        let size_cell = text_cell(format_size(entry.size), COLUMN_SIZE_WIDTH, true, window);

        let (date_cell, type_cell, size_cell) = if let Some(drag_payload) = individual_drag_payload
        {
            (
                add_item_drag(
                    date_cell,
                    ("explorer-entry-date-drag", ix),
                    drag_payload.clone(),
                    entity.clone(),
                ),
                add_item_drag(
                    type_cell,
                    ("explorer-entry-type-drag", ix),
                    drag_payload.clone(),
                    entity.clone(),
                ),
                add_item_drag(
                    size_cell,
                    ("explorer-entry-size-drag", ix),
                    drag_payload,
                    entity.clone(),
                ),
            )
        } else {
            (
                date_cell.into_any_element(),
                type_cell.into_any_element(),
                size_cell.into_any_element(),
            )
        };

        let name_cell = if self.rename_is_active_for_path(&entry.path) {
            rename_name_cell(
                &entry,
                app_icon,
                scale_factor,
                self.active_rename_focus_handle(),
                cx,
            )
            .into_any_element()
        } else {
            let name_click_entry = entry.clone();
            let name_middle_clicked_entry = entry.clone();
            name_cell(
                &entry,
                app_icon,
                scale_factor,
                self.show_file_name_extensions,
                self.recursive_search_results_active(),
                window,
            )
            .id(("explorer-entry-name", ix))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if !is_normal_entry_click(event) {
                    cx.stop_propagation();
                    return;
                }

                if this.suppress_next_click() {
                    this.cancel_pending_click_rename();
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }

                if let Some(EntryAction::OpenFile(path)) = this.handle_entry_name_click(
                    &name_click_entry,
                    event.click_count(),
                    selection_modifiers_for_click(event),
                    window,
                    cx,
                ) {
                    this.open_file_with_default_app(&path);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if !this.commit_active_rename_before_interaction(window, cx) {
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }

                    if let Some(path) = this.handle_entry_middle_click(
                        &name_middle_clicked_entry,
                        SelectionModifiers::from_gpui(event.modifiers),
                    ) {
                        cx.emit(ExplorerViewEvent::OpenDirectoryInNewTab(path));
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .into_any_element()
        };

        row.child(name_cell)
            .child(date_cell)
            .child(type_cell)
            .child(size_cell)
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
                            this.clear_drop_indicator();
                            this.drop_internal_entries_and_open_dialog(
                                dragged,
                                current_directory.clone(),
                                window.modifiers(),
                                cx,
                            );
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .on_drop(cx.listener({
                        let current_directory = current_directory.clone();
                        move |this, paths: &ExternalPaths, window, cx| {
                            this.clear_drop_indicator();
                            this.drop_external_paths_and_open_dialog(
                                paths.paths(),
                                current_directory.clone(),
                                window.modifiers(),
                                cx,
                            );
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        if this.suppress_next_click() {
                            this.cancel_pending_click_rename();
                            cx.stop_propagation();
                            cx.notify();
                            return;
                        }

                        if !this.commit_active_rename_before_interaction(window, cx) {
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

    fn render_empty_folder(&self, message: &'static str, cx: &mut Context<Self>) -> AnyElement {
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
                    this.clear_drop_indicator();
                    this.drop_internal_entries_and_open_dialog(
                        dragged,
                        current_directory.clone(),
                        window.modifiers(),
                        cx,
                    );
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_drop(cx.listener({
                let current_directory = current_directory.clone();
                move |this, paths: &ExternalPaths, window, cx| {
                    this.clear_drop_indicator();
                    this.drop_external_paths_and_open_dialog(
                        paths.paths(),
                        current_directory.clone(),
                        window.modifiers(),
                        cx,
                    );
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(render_empty_folder_message(message))
            .into_any_element()
    }

    fn render_status_bar(&self) -> AnyElement {
        let summary = folder_status_summary(&self.entries, &self.selection.selected_indices);

        div()
            .id("explorer-status-bar")
            .flex()
            .flex_row()
            .items_center()
            .h(px(STATUS_BAR_HEIGHT))
            .w_full()
            .flex_shrink_0()
            .overflow_hidden()
            .bg(rgb(0xffffff))
            .px(px(STATUS_BAR_HORIZONTAL_PADDING))
            .text_size(px(STATUS_BAR_TEXT_SIZE))
            .text_color(rgb(STATUS_BAR_TEXT_COLOR))
            .child(
                div()
                    .flex_shrink_0()
                    .child(SharedString::from(summary.total_items)),
            )
            .when_some(summary.selection_info, |this, selection_info| {
                this.child(
                    div()
                        .h(px(14.0))
                        .w(px(1.0))
                        .mx(px(12.0))
                        .flex_shrink_0()
                        .bg(rgb(STATUS_BAR_SEPARATOR_COLOR)),
                )
                .child(
                    div()
                        .min_w(px(0.0))
                        .truncate()
                        .child(SharedString::from(selection_info)),
                )
            })
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
                        let viewport_size = viewport_size(&bounds);
                        let modifiers = SelectionModifiers::from_gpui(event.modifiers);
                        let _ = entity.update(cx, |this, cx| {
                            if this.begin_mouse_selection_drag_for_intent(
                                local_position,
                                viewport_size,
                                modifiers,
                            ) {
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

fn search_bar_icon_button(
    id: &'static str,
    icon: &'static str,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .w(px(22.0))
        .h(px(22.0))
        .rounded(px(4.0))
        .cursor_default()
        .when(active, |this| this.bg(rgb(0xe5f3ff)))
        .hover(|style| style.bg(rgb(NAV_BUTTON_HOVER_BG)))
        .active(|style| style.opacity(NAV_BUTTON_ACTIVE_OPACITY))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_click(on_click)
        .child(
            div()
                .font(nav_icon_font())
                .text_size(px(13.0))
                .text_color(rgb(if active { 0x0078d7 } else { 0x5f5f5f }))
                .child(icon),
        )
        .into_any_element()
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
            .on_action(cx.listener(Self::handle_copy_selected))
            .on_action(cx.listener(Self::handle_cut_selected))
            .on_action(cx.listener(Self::handle_paste_clipboard))
            .on_action(cx.listener(Self::handle_trash_selected))
            .on_action(cx.listener(Self::handle_permanently_delete_selected))
            .on_action(cx.listener(Self::handle_create_new_folder))
            .on_action(cx.listener(Self::handle_create_new_file))
            .on_action(cx.listener(Self::handle_rename_selected))
            .on_action(cx.listener(Self::handle_rename_commit))
            .on_action(cx.listener(Self::handle_rename_cancel))
            .on_action(cx.listener(Self::handle_rename_backspace))
            .on_action(cx.listener(Self::handle_rename_backspace_word))
            .on_action(cx.listener(Self::handle_rename_delete))
            .on_action(cx.listener(Self::handle_rename_left))
            .on_action(cx.listener(Self::handle_rename_right))
            .on_action(cx.listener(Self::handle_rename_select_left))
            .on_action(cx.listener(Self::handle_rename_select_right))
            .on_action(cx.listener(Self::handle_rename_word_left))
            .on_action(cx.listener(Self::handle_rename_word_right))
            .on_action(cx.listener(Self::handle_rename_select_word_left))
            .on_action(cx.listener(Self::handle_rename_select_word_right))
            .on_action(cx.listener(Self::handle_rename_home))
            .on_action(cx.listener(Self::handle_rename_end))
            .on_action(cx.listener(Self::handle_rename_select_home))
            .on_action(cx.listener(Self::handle_rename_select_end))
            .on_action(cx.listener(Self::handle_rename_select_all))
            .on_action(cx.listener(Self::handle_rename_copy))
            .on_action(cx.listener(Self::handle_rename_cut))
            .on_action(cx.listener(Self::handle_rename_paste))
            .on_action(cx.listener(Self::handle_rename_noop))
            .on_action(cx.listener(Self::handle_address_edit))
            .on_action(cx.listener(Self::handle_address_commit))
            .on_action(cx.listener(Self::handle_address_cancel))
            .on_action(cx.listener(Self::handle_address_backspace))
            .on_action(cx.listener(Self::handle_address_backspace_word))
            .on_action(cx.listener(Self::handle_address_delete))
            .on_action(cx.listener(Self::handle_address_left))
            .on_action(cx.listener(Self::handle_address_right))
            .on_action(cx.listener(Self::handle_address_select_left))
            .on_action(cx.listener(Self::handle_address_select_right))
            .on_action(cx.listener(Self::handle_address_word_left))
            .on_action(cx.listener(Self::handle_address_word_right))
            .on_action(cx.listener(Self::handle_address_select_word_left))
            .on_action(cx.listener(Self::handle_address_select_word_right))
            .on_action(cx.listener(Self::handle_address_home))
            .on_action(cx.listener(Self::handle_address_end))
            .on_action(cx.listener(Self::handle_address_select_home))
            .on_action(cx.listener(Self::handle_address_select_end))
            .on_action(cx.listener(Self::handle_address_select_all))
            .on_action(cx.listener(Self::handle_address_copy))
            .on_action(cx.listener(Self::handle_address_cut))
            .on_action(cx.listener(Self::handle_address_paste))
            .on_action(cx.listener(Self::handle_address_suggestion_up))
            .on_action(cx.listener(Self::handle_address_suggestion_down))
            .on_action(cx.listener(Self::handle_address_accept_suggestion))
            .on_action(cx.listener(Self::handle_search_edit))
            .on_action(cx.listener(Self::handle_search_commit))
            .on_action(cx.listener(Self::handle_search_cancel))
            .on_action(cx.listener(Self::handle_search_backspace))
            .on_action(cx.listener(Self::handle_search_backspace_word))
            .on_action(cx.listener(Self::handle_search_delete))
            .on_action(cx.listener(Self::handle_search_left))
            .on_action(cx.listener(Self::handle_search_right))
            .on_action(cx.listener(Self::handle_search_select_left))
            .on_action(cx.listener(Self::handle_search_select_right))
            .on_action(cx.listener(Self::handle_search_word_left))
            .on_action(cx.listener(Self::handle_search_word_right))
            .on_action(cx.listener(Self::handle_search_select_word_left))
            .on_action(cx.listener(Self::handle_search_select_word_right))
            .on_action(cx.listener(Self::handle_search_home))
            .on_action(cx.listener(Self::handle_search_end))
            .on_action(cx.listener(Self::handle_search_select_home))
            .on_action(cx.listener(Self::handle_search_select_end))
            .on_action(cx.listener(Self::handle_search_select_all))
            .on_action(cx.listener(Self::handle_search_copy))
            .on_action(cx.listener(Self::handle_search_cut))
            .on_action(cx.listener(Self::handle_search_paste))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.navigate_back_with_watcher(cx);
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.navigate_forward_with_watcher(cx);
                    cx.notify();
                }),
            )
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                if this.update_drop_indicator_modifiers(event.modifiers) {
                    cx.notify();
                }
            }))
            .on_drag_move::<DraggedEntries>({
                let entity = cx.entity();
                move |event: &DragMoveEvent<DraggedEntries>, _, cx| {
                    clear_stale_drop_indicator(&entity, event, cx);
                }
            })
            .on_drag_move::<ExternalPaths>({
                let entity = cx.entity();
                move |event: &DragMoveEvent<ExternalPaths>, _, cx| {
                    clear_stale_drop_indicator(&entity, event, cx);
                }
            })
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x000000))
            .overflow_hidden()
            .child(self.render_navbar(window, cx))
            .child(self.render_utility_bar(cx))
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
                            .child(
                                match self.content_branch() {
                                    ExplorerContentBranch::Error => div().child(
                                        div()
                                            .p_4()
                                            .text_size(px(14.0))
                                            .text_color(rgb(0x6f1d1d))
                                            .child(self.read_error.clone().unwrap_or_default()),
                                    ),
                                    ExplorerContentBranch::Empty => div()
                                        .child(self.render_empty_folder(EMPTY_FOLDER_MESSAGE, cx)),
                                    ExplorerContentBranch::SearchWorking => div().child(
                                        self.render_empty_folder(SEARCH_WORKING_MESSAGE, cx),
                                    ),
                                    ExplorerContentBranch::NoSearchMatches => div().child(
                                        self.render_empty_folder(SEARCH_NO_MATCHES_MESSAGE, cx),
                                    ),
                                    ExplorerContentBranch::List => {
                                        div().child(self.render_list(cx))
                                    }
                                }
                                .id("explorer-scroll")
                                .flex_1()
                                .w_full()
                                .overflow_hidden(),
                            )
                            .when_some(self.open_error.clone(), |this, error| {
                                this.child(render_open_error(&error))
                            })
                            .child(self.render_status_bar()),
                    ),
            )
            .when_some(self.render_utility_menu_overlay(cx), |this, menu| {
                this.child(menu)
            })
            .when_some(
                self.render_address_suggestions_overlay(window, cx),
                |this, menu| this.child(menu),
            )
    }
}

pub(super) fn render_drop_indicator(indicator: DropIndicator, window: &Window) -> AnyElement {
    let origin = drop_indicator_origin(indicator.mouse_position);
    let (icon, action_label) = match indicator.operation {
        FileOperationKind::Move => (NavIcon::Forward.glyph(), "Move to"),
        FileOperationKind::Copy => ("\u{E710}", "Copy to"),
    };
    let target_width = drop_indicator_target_width(measure_drop_indicator_target_text(
        &indicator.target_label,
        window,
    ));
    let target_label =
        truncated_drop_indicator_target_label(&indicator.target_label, target_width, window);

    div()
        .absolute()
        .left(px(origin.0))
        .top(px(origin.1))
        .flex()
        .items_center()
        .h(px(26.0))
        .px(px(8.0))
        .gap(px(4.0))
        .rounded(px(3.0))
        .bg(rgb(0xffffff))
        .border_1()
        .border_color(rgb(0x8a8a8a))
        .shadow_md()
        .text_size(px(DROP_INDICATOR_TEXT_SIZE))
        .child(
            div()
                .font(nav_icon_font())
                .text_size(px(DROP_INDICATOR_TEXT_SIZE))
                .text_color(rgb(DROP_INDICATOR_BLUE))
                .child(icon),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_color(rgb(DROP_INDICATOR_BLUE))
                .child(action_label),
        )
        .child(
            div()
                .w(px(target_width))
                .min_w(px(0.0))
                .truncate()
                .text_color(rgb(DROP_INDICATOR_TEXT_COLOR))
                .child(target_label),
        )
        .into_any_element()
}

fn measure_drop_indicator_target_text(text: &str, window: &Window) -> f32 {
    if text.is_empty() {
        return 0.0;
    }

    let run = TextRun {
        len: text.len(),
        font: font(".SystemUIFont"),
        color: rgb(DROP_INDICATOR_TEXT_COLOR).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };

    f32::from(
        window
            .text_system()
            .layout_line(text, px(DROP_INDICATOR_TEXT_SIZE), &[run], None)
            .width,
    )
}

fn drop_indicator_target_width(natural_width: f32) -> f32 {
    natural_width.min(DROP_INDICATOR_TARGET_MAX_WIDTH).max(0.0)
}

fn truncated_drop_indicator_target_label(
    text: &str,
    available_width: f32,
    window: &Window,
) -> SharedString {
    let target_font = font(".SystemUIFont");
    let mut runs = vec![TextRun {
        len: text.len(),
        font: target_font.clone(),
        color: rgb(DROP_INDICATOR_TEXT_COLOR).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }];

    window
        .text_system()
        .line_wrapper(target_font, px(DROP_INDICATOR_TEXT_SIZE))
        .truncate_line(
            SharedString::from(text.to_owned()),
            px(available_width),
            NAME_TRUNCATION_SUFFIX,
            &mut runs,
        )
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
            desktop_folder_icon(scale_factor)
        }
        SidebarItemKind::UserDirectory(UserDirectoryKind::Documents) => {
            documents_folder_icon(scale_factor)
        }
        SidebarItemKind::UserDirectory(UserDirectoryKind::Downloads) => {
            downloads_folder_icon(scale_factor)
        }
        SidebarItemKind::UserDirectory(UserDirectoryKind::Home) => {
            folder_icon(scale_factor).into_any_element()
        }
        SidebarItemKind::MacosSystemLocation(MacosSystemLocationKind::Applications) => {
            applications_sidebar_icon(scale_factor)
        }
        SidebarItemKind::MacosSystemLocation(MacosSystemLocationKind::Bin) => {
            bin_sidebar_icon(scale_factor)
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

fn render_empty_folder_message(message: &'static str) -> Div {
    div()
        .w_full()
        .mt(px(EMPTY_FOLDER_TOP_MARGIN))
        .text_center()
        .text_size(px(EMPTY_FOLDER_TEXT_SIZE))
        .text_color(rgb(0x9a9a9a))
        .child(message)
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

fn clipboard_has_file_clipboard(item: Option<&ClipboardItem>) -> bool {
    item.and_then(file_clipboard_from_item).is_some()
}

fn utility_text_button(
    id: &'static str,
    left_icon: Option<AnyElement>,
    label: &'static str,
    is_open: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .px(px(8.0))
        .h(px(UTILITY_BUTTON_HEIGHT))
        .gap(px(6.0))
        .rounded(px(4.0))
        .cursor_default()
        .bg(if is_open {
            rgb(0xe5f3ff)
        } else {
            rgb(0xf8f8f8)
        })
        .hover(|style| style.bg(rgb(NAV_BUTTON_HOVER_BG)))
        .active(|style| style.opacity(NAV_BUTTON_ACTIVE_OPACITY))
        .on_click(on_click)
        .when_some(left_icon, |this, icon| this.child(icon))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(0x1f1f1f))
                .child(label),
        )
        .child(
            div()
                .font(nav_icon_font())
                .text_size(px(7.0))
                .mt(px(2.0))
                .text_color(rgb(0x505050))
                .child(UTILITY_ICON_CHEVRON_DOWN),
        )
        .into_any_element()
}

fn utility_new_icon() -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(UTILITY_TEXT_BUTTON_ICON_SIZE))
        .h(px(UTILITY_TEXT_BUTTON_ICON_SIZE))
        .flex_shrink_0()
        .child(
            div()
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .w(px(UTILITY_NEW_ICON_CIRCLE_SIZE))
                .h(px(UTILITY_NEW_ICON_CIRCLE_SIZE))
                .rounded(px(UTILITY_NEW_ICON_CIRCLE_SIZE / 2.0))
                .border_1()
                .border_color(rgb(UTILITY_NEW_ICON_BLACK))
                .child(
                    div()
                        .relative()
                        .w(px(UTILITY_NEW_ICON_PLUS_SIZE))
                        .h(px(UTILITY_NEW_ICON_PLUS_SIZE))
                        .child(
                            div()
                                .absolute()
                                .left(px(0.0))
                                .top(px(UTILITY_NEW_ICON_PLUS_CENTER_OFFSET))
                                .w(px(UTILITY_NEW_ICON_PLUS_SIZE))
                                .h(px(UTILITY_NEW_ICON_PLUS_THICKNESS))
                                .bg(rgb(UTILITY_NEW_ICON_BLUE)),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(UTILITY_NEW_ICON_PLUS_CENTER_OFFSET))
                                .top(px(0.0))
                                .w(px(UTILITY_NEW_ICON_PLUS_THICKNESS))
                                .h(px(UTILITY_NEW_ICON_PLUS_SIZE))
                                .bg(rgb(UTILITY_NEW_ICON_BLUE)),
                        ),
                ),
        )
}

fn utility_view_icon() -> Div {
    div()
        .relative()
        .w(px(UTILITY_TEXT_BUTTON_ICON_SIZE))
        .h(px(UTILITY_TEXT_BUTTON_ICON_SIZE))
        .flex_shrink_0()
        .child(utility_view_icon_line(UTILITY_VIEW_ICON_LINE_TOPS[0]))
        .child(utility_view_icon_line(UTILITY_VIEW_ICON_LINE_TOPS[1]))
        .child(utility_view_icon_line(UTILITY_VIEW_ICON_LINE_TOPS[2]))
        .child(utility_view_icon_line(UTILITY_VIEW_ICON_LINE_TOPS[3]))
}

fn utility_view_icon_line(top: f32) -> Div {
    div()
        .absolute()
        .left(px(1.0))
        .top(px(top))
        .w(px(14.0))
        .h(px(1.0))
        .bg(rgb(UTILITY_VIEW_ICON_LINE_COLOR))
}

fn utility_icon_button(
    id: &'static str,
    icon: &'static str,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(UTILITY_ICON_BUTTON_SIZE))
        .h(px(UTILITY_ICON_BUTTON_SIZE))
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
                .text_size(px(14.0))
                .text_color(if enabled {
                    rgb(NAV_ICON_ENABLED_COLOR)
                } else {
                    rgb(NAV_ICON_DISABLED_COLOR)
                })
                .child(icon),
        )
        .into_any_element()
}

fn utility_separator() -> Div {
    div()
        .h(px(22.0))
        .w(px(1.0))
        .mx(px(0.0))
        .flex_shrink_0()
        .bg(rgb(0xd8d8d8))
}

fn utility_dropdown() -> Div {
    div()
        .w(px(UTILITY_MENU_WIDTH))
        .py(px(4.0))
        .rounded(px(6.0))
        .bg(rgb(0xffffff))
        .border_1()
        .border_color(rgb(0xd8d8d8))
        .shadow_md()
        .occlude()
}

fn utility_menu_row(
    id: &'static str,
    icon: Option<AnyElement>,
    label: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .h(px(UTILITY_MENU_ROW_HEIGHT))
        .mx(px(4.0))
        .px(px(8.0))
        .gap(px(8.0))
        .rounded(px(4.0))
        .cursor_default()
        .hover(|style| style.bg(rgb(0xe5f3ff)))
        .active(|style| style.opacity(NAV_BUTTON_ACTIVE_OPACITY))
        .on_click(on_click)
        .child(utility_menu_icon_slot(icon))
        .child(
            div()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(12.0))
                .text_color(rgb(0x1f1f1f))
                .child(label),
        )
        .into_any_element()
}

fn address_suggestion_row(
    index: usize,
    label: String,
    path: PathBuf,
    highlighted: bool,
    cx: &mut Context<ExplorerView>,
) -> AnyElement {
    div()
        .id(("address-suggestion", index))
        .flex()
        .flex_row()
        .items_center()
        .h(px(30.0))
        .mx(px(4.0))
        .px(px(8.0))
        .gap(px(10.0))
        .rounded(px(4.0))
        .cursor_default()
        .bg(if highlighted {
            rgb(0xcce8ff)
        } else {
            rgb(0xffffff)
        })
        .when(!highlighted, |this| {
            this.hover(|style| style.bg(rgb(0xe5f3ff)))
        })
        .active(|style| style.opacity(NAV_BUTTON_ACTIVE_OPACITY))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                this.navigate_to_address_suggestion_path(path.clone(), window, cx);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(12.0))
                .text_color(rgb(0x1f1f1f))
                .child(SharedString::from(label)),
        )
        .into_any_element()
}

fn utility_checkbox_row(
    id: &'static str,
    checked: bool,
    label: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    utility_menu_row(
        id,
        Some(if checked {
            utility_menu_glyph_icon(UTILITY_ICON_CHECK)
        } else {
            div()
                .w(px(16.0))
                .h(px(16.0))
                .rounded(px(2.0))
                .into_any_element()
        }),
        label,
        on_click,
    )
}

fn utility_menu_icon_slot(icon: Option<AnyElement>) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(22.0))
        .h(px(22.0))
        .flex_shrink_0()
        .when_some(icon, |this, icon| this.child(icon))
}

fn utility_menu_glyph_icon(icon: &'static str) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(16.0))
        .h(px(16.0))
        .font(nav_icon_font())
        .text_size(px(13.0))
        .text_color(rgb(0x1f1f1f))
        .child(icon)
        .into_any_element()
}

fn nav_button(
    id: &'static str,
    icon: NavIcon,
    enabled: bool,
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
                .text_size(px(NAV_ICON_TEXT_SIZE))
                .text_color(if enabled {
                    rgb(NAV_ICON_ENABLED_COLOR)
                } else {
                    rgb(NAV_ICON_DISABLED_COLOR)
                })
                .child(icon.glyph()),
        )
        .into_any_element()
}

fn directory_bar(breadcrumb: VisibleBreadcrumb, cx: &mut Context<ExplorerView>) -> AnyElement {
    div()
        .id("directory-bar")
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
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.start_address_bar_edit(window, cx);
            cx.stop_propagation();
            cx.notify();
        }))
        .children(directory_bar_children(breadcrumb, cx))
        .into_any_element()
}

fn editable_directory_bar(
    focus_handle: Option<FocusHandle>,
    cx: &mut Context<ExplorerView>,
) -> AnyElement {
    let entity = cx.entity();

    div()
        .id("directory-bar-input")
        .debug_selector(|| "directory-bar-input".to_owned())
        .key_context("ExplorerAddressInput")
        .flex()
        .flex_row()
        .items_center()
        .h(px(DIRECTORY_BAR_HEIGHT))
        .flex_1()
        .overflow_hidden()
        .rounded(px(DIRECTORY_BAR_RADIUS))
        .border_b_2()
        .border_color(rgb(0x0078d7))
        .bg(rgb(0xffffff))
        .px(px(DIRECTORY_BAR_HORIZONTAL_PADDING))
        .cursor(CursorStyle::IBeam)
        .text_size(px(DIRECTORY_BAR_TEXT_SIZE))
        .line_height(px(20.0))
        .text_color(rgb(0x1f1f1f))
        .when_some(focus_handle.as_ref(), |this, focus_handle| {
            this.track_focus(focus_handle)
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                this.on_address_mouse_down(event);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
            this.on_address_mouse_move(event);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, event: &MouseUpEvent, _, cx| {
                this.on_address_mouse_up(event);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, event: &MouseUpEvent, window, cx| {
                if this.active_text_input_is_selecting() {
                    this.on_address_mouse_up(event);
                    cx.stop_propagation();
                    cx.notify();
                } else if this.address_suggestions_contain_position(event.position, window) {
                    return;
                } else if this.finish_active_input_for_pointer_interaction(
                    ActiveTextInput::Address,
                    window,
                    cx,
                ) {
                    cx.notify();
                }
            }),
        )
        .child(address_text_element(entity))
        .into_any_element()
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
                this.clear_drop_indicator();
                this.drop_internal_entries_and_open_dialog(
                    dragged,
                    destination.clone(),
                    window.modifiers(),
                    cx,
                );
                cx.stop_propagation();
                cx.notify();
            }
        }))
        .on_drop(cx.listener({
            let destination = destination.clone();
            move |this, paths: &ExternalPaths, window, cx| {
                this.clear_drop_indicator();
                this.drop_external_paths_and_open_dialog(
                    paths.paths(),
                    destination.clone(),
                    window.modifiers(),
                    cx,
                );
                cx.stop_propagation();
                cx.notify();
            }
        }))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.navigate_to_directory_with_watcher(
                navigation_target.clone(),
                HistoryMode::Record,
                cx,
            );
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

    entity.update(cx, |this, cx| {
        let indicator = this.drop_indicator_for_value(
            event.dragged_item(),
            destination,
            window.modifiers(),
            event.event.position,
        );
        if this.active_drop_indicator != indicator {
            this.active_drop_indicator = indicator;
            cx.notify();
        }
    });
}

fn clear_stale_drop_indicator<T: 'static>(
    entity: &Entity<ExplorerView>,
    event: &DragMoveEvent<T>,
    cx: &mut App,
) {
    entity.update(cx, |this, cx| {
        if this.clear_stale_drop_indicator(event.event.position) {
            cx.notify();
        }
    });
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

fn name_cell(
    entry: &FileEntry,
    app_icon: Option<Arc<Image>>,
    scale_factor: f32,
    show_file_name_extensions: bool,
    show_full_path: bool,
    window: &Window,
) -> Div {
    let list_viewport_width = (f32::from(window.bounds().size.width) - SIDEBAR_WIDTH).max(0.0);
    let text_width = if show_full_path {
        recursive_result_text_width(list_viewport_width, scale_factor)
    } else {
        available_filename_text_width(list_viewport_width, scale_factor)
    };
    let filename = truncated_text(
        entry.display_name_with_extensions(show_file_name_extensions),
        text_width,
        0x000000,
        window,
    );
    div()
        .flex()
        .items_center()
        .h_full()
        .flex_1()
        .min_w(px(COLUMN_NAME_MIN_WIDTH))
        .overflow_hidden()
        .pl(px(NAME_CELL_LEFT_PADDING))
        .child(entry_icon(entry, app_icon, scale_factor))
        .child(if show_full_path {
            let full_path = truncated_text_with_size(
                &entry.path.display().to_string(),
                text_width,
                RECURSIVE_SEARCH_PATH_TEXT_SIZE,
                RECURSIVE_SEARCH_PATH_TEXT_COLOR,
                window,
            );

            div()
                .flex()
                .flex_col()
                .justify_center()
                .flex_1()
                .min_w(px(0.0))
                .ml(device_px(NAME_ICON_TEXT_GAP_PHYSICAL, scale_factor))
                .text_size(px(NAME_TEXT_SIZE))
                .child(
                    div()
                        .w(px(text_width))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(filename),
                )
                .child(
                    div()
                        .w(px(text_width))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(RECURSIVE_SEARCH_PATH_TEXT_SIZE))
                        .text_color(rgb(RECURSIVE_SEARCH_PATH_TEXT_COLOR))
                        .child(full_path),
                )
        } else {
            div()
                .flex_1()
                .min_w(px(0.0))
                .ml(device_px(NAME_ICON_TEXT_GAP_PHYSICAL, scale_factor))
                .truncate()
                .text_size(px(NAME_TEXT_SIZE))
                .child(filename)
        })
}

fn rename_name_cell(
    entry: &FileEntry,
    app_icon: Option<Arc<Image>>,
    scale_factor: f32,
    focus_handle: Option<FocusHandle>,
    cx: &mut Context<ExplorerView>,
) -> Div {
    let entity = cx.entity();
    let input = div()
        .debug_selector(|| "rename-input".to_owned())
        .key_context("ExplorerRenameInput")
        .flex_1()
        .min_w(px(0.0))
        .h(px(20.0))
        .ml(device_px(NAME_ICON_TEXT_GAP_PHYSICAL, scale_factor))
        .px(px(2.0))
        .border_1()
        .border_color(rgb(0x0078d7))
        .bg(rgb(0xffffff))
        .cursor(CursorStyle::IBeam)
        .text_size(px(NAME_TEXT_SIZE))
        .line_height(px(16.0))
        .overflow_hidden()
        .when_some(focus_handle.as_ref(), |this, focus_handle| {
            this.track_focus(focus_handle)
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                this.on_rename_mouse_down(event);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
            this.on_rename_mouse_move(event);
            cx.stop_propagation();
            cx.notify();
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, event: &MouseUpEvent, _, cx| {
                this.on_rename_mouse_up(event);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, event: &MouseUpEvent, window, cx| {
                if this.active_text_input_is_selecting() {
                    this.on_rename_mouse_up(event);
                    cx.stop_propagation();
                    cx.notify();
                } else if this.finish_active_input_for_pointer_interaction(
                    ActiveTextInput::Rename,
                    window,
                    cx,
                ) {
                    cx.notify();
                }
            }),
        )
        .child(rename_text_element(entity));

    div()
        .flex()
        .items_center()
        .h_full()
        .flex_1()
        .min_w(px(COLUMN_NAME_MIN_WIDTH))
        .overflow_hidden()
        .pl(px(NAME_CELL_LEFT_PADDING))
        .child(entry_icon(entry, app_icon, scale_factor))
        .child(input)
}

fn entry_icon(entry: &FileEntry, app_icon: Option<Arc<Image>>, scale_factor: f32) -> AnyElement {
    if let Some(app_icon) = app_icon {
        return image_icon(
            app_icon,
            FILE_ICON_SLOT_WIDTH_PHYSICAL,
            FILE_ICON_SLOT_HEIGHT_PHYSICAL,
            scale_factor,
        );
    }

    if entry.uses_directory_shortcut_icon() {
        directory_shortcut_icon(scale_factor).into_any_element()
    } else if entry.is_directory_like() {
        folder_icon(scale_factor).into_any_element()
    } else {
        file_icon(scale_factor).into_any_element()
    }
}

fn filename_text_width(name_column_width: f32, scale_factor: f32) -> f32 {
    let icon_width = device_px_value(FILE_ICON_SLOT_WIDTH_PHYSICAL, scale_factor);
    let gap_width = device_px_value(NAME_ICON_TEXT_GAP_PHYSICAL, scale_factor);

    (name_column_width - NAME_CELL_LEFT_PADDING - icon_width - gap_width).max(0.0)
}

fn available_filename_text_width(viewport_width: f32, scale_factor: f32) -> f32 {
    filename_text_width(effective_name_column_width(viewport_width), scale_factor)
}

fn recursive_result_text_width(viewport_width: f32, scale_factor: f32) -> f32 {
    available_filename_text_width(viewport_width, scale_factor)
}

fn truncated_text(
    text: &str,
    available_width: f32,
    text_color: u32,
    window: &Window,
) -> SharedString {
    truncated_text_with_size(text, available_width, NAME_TEXT_SIZE, text_color, window)
}

fn truncated_text_with_size(
    text: &str,
    available_width: f32,
    text_size: f32,
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
        .line_wrapper(name_font, px(text_size))
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
    SelectionModifiers::from_gpui(event.modifiers())
}

fn is_normal_entry_click(event: &ClickEvent) -> bool {
    match event {
        ClickEvent::Mouse(event) => event.down.button == MouseButton::Left,
        ClickEvent::Keyboard(_) => true,
    }
}

fn add_item_drag(
    cell: Div,
    id: impl Into<gpui::ElementId>,
    drag_payload: DraggedEntries,
    entity: Entity<ExplorerView>,
) -> AnyElement {
    let external_paths = ExternalPaths::new(drag_payload.paths.clone());

    cell.id(id)
        .on_drag_with_external_paths(
            drag_payload,
            external_paths,
            move |dragged: &DraggedEntries, cursor_offset, _, cx| {
                entity.update(cx, |this, _| {
                    this.begin_individual_item_drag(dragged);
                });
                cx.new(|_| DragPreview::new(dragged, cursor_offset))
            },
        )
        .into_any_element()
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct FolderStatusSummary {
    total_items: String,
    selection_info: Option<String>,
}

fn folder_status_summary(
    entries: &[FileEntry],
    selected_indices: &BTreeSet<usize>,
) -> FolderStatusSummary {
    let total_items = count_label(entries.len(), "item", "items");
    let mut folder_count = 0;
    let mut file_count = 0;
    let mut file_size = 0;

    for ix in selected_indices {
        let Some(entry) = entries.get(*ix) else {
            continue;
        };

        if entry.is_directory_like() {
            folder_count += 1;
        } else {
            file_count += 1;
            file_size += entry.size.unwrap_or(0);
        }
    }

    let selection_info = match (folder_count, file_count) {
        (0, 0) => None,
        (0, files) => Some(format!(
            "{} selected  {}",
            count_label(files, "file", "files"),
            format_size(Some(file_size))
        )),
        (folders, 0) => Some(format!(
            "{} selected",
            count_label(folders, "folder", "folders")
        )),
        (folders, files) => Some(format!(
            "{}, {} selected",
            count_label(folders, "folder", "folders"),
            count_label(files, "file", "files")
        )),
    };

    FolderStatusSummary {
        total_items,
        selection_info,
    }
}

fn count_label(count: usize, singular: &str, plural: &str) -> String {
    let name = if count == 1 { singular } else { plural };
    format!("{count} {name}")
}

#[cfg(test)]
mod tests {

    use std::{collections::BTreeSet, path::PathBuf};

    use gpui::{
        ClickEvent, ClipboardItem, KeyboardClickEvent, Modifiers, MouseButton, MouseClickEvent,
        MouseDownEvent, MouseUpEvent,
    };

    use crate::explorer::{
        clipboard::{FileClipboard, FileClipboardOperation, clipboard_item_for_files},
        constants::{
            COLUMN_DATE_WIDTH, COLUMN_NAME_MIN_WIDTH, COLUMN_SIZE_WIDTH, COLUMN_TYPE_WIDTH,
            EMPTY_FOLDER_MESSAGE, EMPTY_FOLDER_TEXT_SIZE, EMPTY_FOLDER_TOP_MARGIN,
            FILE_ICON_SLOT_WIDTH_PHYSICAL, MB_BYTES, NAV_BUTTON_ACTIVE_OPACITY,
            SCROLLBAR_GUTTER_WIDTH,
        },
        entry::FileEntry,
        selection::SelectionModifiers,
    };

    use super::{
        CUT_ITEM_OPACITY, DROP_INDICATOR_TARGET_MAX_WIDTH, NAME_CELL_LEFT_PADDING,
        NAME_ICON_TEXT_GAP_PHYSICAL, UTILITY_NEW_ICON_BLACK, UTILITY_NEW_ICON_BLUE,
        UTILITY_NEW_ICON_CIRCLE_SIZE, UTILITY_NEW_ICON_PLUS_CENTER_OFFSET,
        UTILITY_NEW_ICON_PLUS_SIZE, UTILITY_NEW_ICON_PLUS_THICKNESS, UTILITY_TEXT_BUTTON_ICON_SIZE,
        UTILITY_TEXT_BUTTON_WIDTH, UTILITY_VIEW_ICON_LINE_COLOR, UTILITY_VIEW_ICON_LINE_TOPS,
        available_filename_text_width, clipboard_has_file_clipboard, drop_indicator_target_width,
        filename_text_width, folder_status_summary, is_normal_entry_click,
        recursive_result_text_width, selection_modifiers_for_click, text_cell_width,
    };

    #[test]
    fn nav_button_active_opacity_dims_button() {
        assert_eq!(NAV_BUTTON_ACTIVE_OPACITY, 0.7);
        assert!(NAV_BUTTON_ACTIVE_OPACITY < 1.0);
    }

    #[test]
    fn cut_item_opacity_dims_rows() {
        assert_eq!(CUT_ITEM_OPACITY, 0.7);
        assert!(CUT_ITEM_OPACITY < 1.0);
    }

    #[test]
    fn paste_button_availability_requires_explorer_file_clipboard() {
        let explorer_item = clipboard_item_for_files(&FileClipboard::new(
            FileClipboardOperation::Copy,
            vec![PathBuf::from("a.txt")],
        ))
        .expect("clipboard item");
        let plain_item = ClipboardItem::new_string("a.txt".to_owned());

        assert!(clipboard_has_file_clipboard(Some(&explorer_item)));
        assert!(!clipboard_has_file_clipboard(Some(&plain_item)));
        assert!(!clipboard_has_file_clipboard(None));
    }

    #[test]
    fn utility_text_button_icon_geometry_fits_button() {
        assert_eq!(UTILITY_TEXT_BUTTON_ICON_SIZE, 16.0);
        assert!(UTILITY_TEXT_BUTTON_WIDTH >= 92.0);
        assert_eq!(UTILITY_NEW_ICON_CIRCLE_SIZE, 14.0);
        assert_eq!(UTILITY_NEW_ICON_PLUS_SIZE, 8.0);
        assert_eq!(UTILITY_NEW_ICON_PLUS_THICKNESS, 2.0);
        assert_eq!(UTILITY_NEW_ICON_PLUS_CENTER_OFFSET, 3.0);
        assert_eq!(UTILITY_NEW_ICON_BLUE, 0x0078d4);
        assert_eq!(UTILITY_NEW_ICON_BLACK, 0x555555);
        assert_eq!(UTILITY_VIEW_ICON_LINE_COLOR, 0x555555);
        assert_eq!(UTILITY_VIEW_ICON_LINE_TOPS, [3.5, 6.5, 9.5, 12.5]);
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
    fn recursive_result_text_width_matches_name_text_width() {
        for viewport_width in [100.0, 900.0] {
            let recursive_width = recursive_result_text_width(viewport_width, 1.0);

            assert_eq!(
                recursive_width,
                available_filename_text_width(viewport_width, 1.0)
            );
            assert!(recursive_width > 0.0);
        }
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
    fn drop_indicator_target_width_uses_natural_width_under_max() {
        assert_eq!(drop_indicator_target_width(72.0), 72.0);
    }

    #[test]
    fn drop_indicator_target_width_caps_at_max() {
        assert_eq!(
            drop_indicator_target_width(DROP_INDICATOR_TARGET_MAX_WIDTH + 20.0),
            DROP_INDICATOR_TARGET_MAX_WIDTH
        );
    }

    #[test]
    fn drop_indicator_target_width_clamps_empty_width() {
        assert_eq!(drop_indicator_target_width(0.0), 0.0);
    }

    #[test]
    fn click_selection_modifiers_use_click_shift() {
        let event = ClickEvent::Mouse(MouseClickEvent {
            down: MouseDownEvent::default(),
            up: MouseUpEvent {
                modifiers: Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
                ..MouseUpEvent::default()
            },
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
    fn click_selection_modifiers_use_click_secondary_modifier() {
        let event = ClickEvent::Mouse(MouseClickEvent {
            down: MouseDownEvent::default(),
            up: MouseUpEvent {
                modifiers: Modifiers {
                    control: true,
                    platform: cfg!(target_os = "macos"),
                    ..Modifiers::default()
                },
                ..MouseUpEvent::default()
            },
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

    #[test]
    fn normal_entry_click_accepts_left_mouse_and_keyboard_clicks() {
        let left = ClickEvent::Mouse(MouseClickEvent {
            down: MouseDownEvent {
                button: MouseButton::Left,
                ..MouseDownEvent::default()
            },
            up: MouseUpEvent::default(),
        });
        let keyboard = ClickEvent::Keyboard(KeyboardClickEvent::default());

        assert!(is_normal_entry_click(&left));
        assert!(is_normal_entry_click(&keyboard));
    }

    #[test]
    fn normal_entry_click_rejects_middle_mouse_clicks() {
        let middle = ClickEvent::Mouse(MouseClickEvent {
            down: MouseDownEvent {
                button: MouseButton::Middle,
                ..MouseDownEvent::default()
            },
            up: MouseUpEvent::default(),
        });

        assert!(!is_normal_entry_click(&middle));
    }

    #[test]
    fn status_summary_shows_total_items_without_selection() {
        let entries = status_entries(13, 0);
        let summary = folder_status_summary(&entries, &BTreeSet::new());

        assert_eq!(summary.total_items, "13 items");
        assert_eq!(summary.selection_info, None);
    }

    #[test]
    fn status_summary_shows_selected_files_and_total_size() {
        let mut entries = status_entries(13, 0);
        entries[0] = FileEntry::test("a.txt", false, Some(10 * MB_BYTES), None);
        entries[1] = FileEntry::test("b.txt", false, Some(15 * MB_BYTES + 818_000), None);
        let selected = BTreeSet::from([0, 1]);

        let summary = folder_status_summary(&entries, &selected);

        assert_eq!(summary.total_items, "13 items");
        assert_eq!(
            summary.selection_info,
            Some("2 files selected  25.78 MB".to_owned())
        );
    }

    #[test]
    fn status_summary_omits_size_for_mixed_folder_and_file_selection() {
        let entries = status_entries(10, 3);
        let selected = BTreeSet::from([0, 1, 10, 11, 12]);

        let summary = folder_status_summary(&entries, &selected);

        assert_eq!(summary.total_items, "13 items");
        assert_eq!(
            summary.selection_info,
            Some("3 folders, 2 files selected".to_owned())
        );
    }

    #[test]
    fn status_summary_shows_only_selected_folders() {
        let entries = status_entries(10, 3);
        let selected = BTreeSet::from([10, 11, 12]);

        let summary = folder_status_summary(&entries, &selected);

        assert_eq!(summary.total_items, "13 items");
        assert_eq!(
            summary.selection_info,
            Some("3 folders selected".to_owned())
        );
    }

    #[test]
    fn status_summary_uses_singular_labels() {
        let entries = vec![FileEntry::test("a.txt", false, Some(1), None)];
        let selected = BTreeSet::from([0]);

        let summary = folder_status_summary(&entries, &selected);

        assert_eq!(summary.total_items, "1 item");
        assert_eq!(
            summary.selection_info,
            Some("1 file selected  1 bytes".to_owned())
        );

        let entries = vec![FileEntry::test("folder", true, None, None)];
        let selected = BTreeSet::from([0]);
        let summary = folder_status_summary(&entries, &selected);

        assert_eq!(summary.total_items, "1 item");
        assert_eq!(summary.selection_info, Some("1 folder selected".to_owned()));
    }

    fn status_entries(file_count: usize, folder_count: usize) -> Vec<FileEntry> {
        let mut entries = Vec::with_capacity(file_count + folder_count);
        for ix in 0..file_count {
            entries.push(FileEntry::test(
                &format!("file-{ix}.txt"),
                false,
                Some(1),
                None,
            ));
        }
        for ix in 0..folder_count {
            entries.push(FileEntry::test(&format!("folder-{ix}"), true, None, None));
        }
        entries
    }
}
