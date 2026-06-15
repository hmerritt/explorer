use std::{
    any::Any,
    collections::{BTreeSet, HashMap},
    ops::Range,
    path::PathBuf,
    sync::Arc,
};

use gpui::{
    AnyElement, App, Bounds, ClickEvent, ClipboardItem, Context, CursorStyle, Div, DragMoveEvent,
    Entity, ExternalPaths, FocusHandle, Focusable, Image, IntoElement,
    ListHorizontalSizingBehavior, ModifiersChangedEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, NavigationDirection, Pixels, Point, Render, ScrollWheelEvent,
    SharedString, TextAlign, TextRun, Window, canvas, div, prelude::*, px, rgb, transparent_black,
    uniform_list,
};

use crate::explorer::{
    DirectoryKind,
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
        EMPTY_FOLDER_TOP_MARGIN, FILE_ICON_SLOT_HEIGHT, FILE_ICON_SLOT_WIDTH, HEADER_HEIGHT,
        NAV_BUTTON_ACTIVE_OPACITY, NAV_BUTTON_HOVER_BG, NAV_BUTTON_SIZE, NAV_ICON_DISABLED_COLOR,
        NAV_ICON_ENABLED_COLOR, NAV_ICON_TEXT_SIZE, NAVBAR_HEIGHT, NAVBAR_HORIZONTAL_PADDING,
        NAVBAR_ITEM_GAP, OPEN_ERROR_HORIZONTAL_PADDING, OPEN_ERROR_VERTICAL_PADDING,
        RECURSIVE_SEARCH_ROW_HEIGHT, ROW_HEIGHT, SCROLLBAR_ARROW_HEIGHT, SCROLLBAR_GUTTER_WIDTH,
        SCROLLBAR_THUMB_ACTIVE_BG, SCROLLBAR_THUMB_BG, SCROLLBAR_THUMB_HOVER_BG,
        SCROLLBAR_THUMB_HOVER_WIDTH, SCROLLBAR_THUMB_WIDTH, SCROLLBAR_TRACK_BG,
        SEARCH_BAR_MAX_WIDTH, SEARCH_BAR_MIN_WIDTH, SEARCH_NO_MATCHES_MESSAGE,
        SEARCH_WORKING_MESSAGE, SIDEBAR_HORIZONTAL_PADDING, SIDEBAR_ICON_TEXT_GAP,
        SIDEBAR_ROW_HEIGHT, SIDEBAR_TEXT_SIZE, STATUS_BAR_HEIGHT, STATUS_BAR_HORIZONTAL_PADDING,
        STATUS_BAR_SEPARATOR_COLOR, STATUS_BAR_TEXT_COLOR, STATUS_BAR_TEXT_SIZE,
        UTILITY_BAR_HEIGHT, UTILITY_BAR_HORIZONTAL_PADDING, UTILITY_BAR_ITEM_GAP,
        UTILITY_BUTTON_HEIGHT, UTILITY_ICON_BUTTON_SIZE, UTILITY_MENU_ROW_HEIGHT,
        UTILITY_MENU_WIDTH, effective_name_column_width, minimum_file_columns_width,
    },
    context_menu::{
        ContextMenuCommand, ContextMenuIcon, ContextMenuIconSlot, ContextMenuItem,
        ContextMenuSource, clamped_context_menu_origin, context_menu_height,
        context_menu_item_is_persistently_active, context_menu_item_top,
        context_menu_path_is_active, context_menu_pointer_tip_origin, context_submenu_left,
    },
    drag_drop::{
        DragPreview, DraggedEntries, DropDestination, DropIndicator, FileOperationKind,
        drop_indicator_origin, row_drop_destination_for_entry,
    },
    entry::FileEntry,
    formatting::{format_size, format_timestamp},
    icons::{
        COPY_ICON, CUT_ICON, DELETE_ICON, DETAILS_ICON, EXTRACT_ICON, FAVORITE_PIN_REMOVE_ICON,
        NEW_ITEM_ICON, NEW_TAB_ICON, NavIcon, OPEN_WITH_ICON, PASTE_ICON, RENAME_ICON,
        directory_kind_icon, directory_kind_icon_sized, directory_shortcut_icon, drive_icon,
        drive_windows_icon, executable_icon_sized, file_icon, file_icon_for_path, file_icon_sized,
        folder_icon, folder_icon_sized, image_icon, nav_icon_font,
    },
    mouse_selection::{local_point, selection_box_bounds, viewport_size},
    navigation::{EntryAction, HistoryMode},
    recursive_search::RecursiveSearchProgressSnapshot,
    rename::{ActiveTextInput, rename_text_element},
    scrollbar::{
        ScrollbarArrow, scrollbar_arrow_button, scrollbar_corner, scrollbar_header_spacer,
    },
    search::search_text_element,
    selection::SelectionModifiers,
    sidebar::{SidebarItem, SidebarItemKind},
    view::{
        ExplorerContentBranch, ExplorerView, ExplorerViewEvent, UtilityMenu,
        normalized_sidebar_width_f32,
    },
};
use crate::loaders::{LinearProgressStyle, linear_indeterminate};
use crate::settings::SettingsState;
use thousands::Separable;

const NAME_CELL_LEFT_PADDING: f32 = 16.0;
const NAME_ICON_TEXT_GAP: f32 = 8.0;
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
const SIDEBAR_ITEM_GAP: f32 = 4.0;
const SIDEBAR_ROW_BG: u32 = 0xffffff;
const SIDEBAR_ROW_CURRENT_BG: u32 = 0xcce8ff;
const SIDEBAR_ROW_HOVER_BG: u32 = 0xe5f3ff;
const SIDEBAR_RESIZE_HIT_WIDTH: f32 = 6.0;

#[derive(Clone, Debug, Eq, PartialEq)]
struct SidebarItemDrag {
    configured_index: usize,
    label: SharedString,
    kind: SidebarItemKind,
}

struct SidebarItemDragPreview {
    label: SharedString,
    kind: SidebarItemKind,
    width: f32,
    font: gpui::Font,
}

impl Render for SidebarItemDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .font(self.font.clone())
            .flex()
            .flex_row()
            .items_center()
            .h(px(SIDEBAR_ROW_HEIGHT))
            .w(px((self.width - 16.0).max(0.0)))
            .px(px(SIDEBAR_HORIZONTAL_PADDING))
            .rounded(px(4.0))
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(0x8a8a8a))
            .shadow_md()
            .child(sidebar_item_kind_icon(self.kind))
            .child(
                div()
                    .min_w(px(0.0))
                    .ml(px(SIDEBAR_ICON_TEXT_GAP))
                    .truncate()
                    .text_size(px(SIDEBAR_TEXT_SIZE))
                    .child(self.label.clone()),
            )
    }
}
const UTILITY_SEPARATOR_OUTER_WIDTH: f32 = 17.0;
const UTILITY_NEW_MENU_LEFT: f32 = UTILITY_BAR_HORIZONTAL_PADDING;
const UTILITY_VIEW_MENU_LEFT: f32 = UTILITY_BAR_HORIZONTAL_PADDING
    + UTILITY_TEXT_BUTTON_WIDTH
    + UTILITY_SEPARATOR_OUTER_WIDTH
    + (UTILITY_ICON_BUTTON_SIZE * 5.0)
    + UTILITY_SEPARATOR_OUTER_WIDTH
    + (UTILITY_BAR_ITEM_GAP * 8.0);
const UTILITY_ICON_CHEVRON_DOWN: &str = "\u{E70D}";
const UTILITY_ICON_CHECK: &str = "\u{E73E}";
const UTILITY_TEXT_BUTTON_ICON_SIZE: f32 = 16.0;
const CONTEXT_MENU_MIN_WIDTH: f32 = 170.0;
const CONTEXT_MENU_MAX_WIDTH: f32 = 280.0;
const CONTEXT_MENU_BORDER_WIDTH: f32 = 1.0;
const CONTEXT_MENU_ROW_HEIGHT: f32 = 30.0;
const CONTEXT_MENU_ITEM_VERTICAL_GAP: f32 = 6.0;
const CONTEXT_MENU_SEPARATOR_HEIGHT: f32 = 10.0;
const CONTEXT_MENU_ICON_SLOT_SIZE: f32 = 14.0;
const CONTEXT_MENU_ICON_SIZE: f32 = 14.0;
const CONTEXT_MENU_SUBMENU_OVERLAP: f32 = 1.0;
const CONTEXT_MENU_CHEVRON: &str = "\u{E76C}";
const CONTEXT_MENU_TEXT_SIZE: f32 = 11.0;
const CONTEXT_MENU_ROW_OUTER_HORIZONTAL_PADDING: f32 = 0.0;
const CONTEXT_MENU_ROW_INNER_HORIZONTAL_PADDING: f32 = 18.0;
const CONTEXT_MENU_ROW_CHILD_GAP: f32 = 10.0;
const CONTEXT_MENU_TRAILING_SLOT_WIDTH: f32 = 14.0;
const CONTEXT_MENU_DETAIL_VALUE_LEFT_MARGIN: f32 = 16.0;
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
            &self.font,
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
                    this.close_context_menu();
                    this.navigate_back_with_watcher(cx);
                    cx.notify();
                }),
            ))
            .child(nav_button(
                "forward",
                NavIcon::Forward,
                self.can_go_forward(),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.close_context_menu();
                    this.navigate_forward_with_watcher(cx);
                    cx.notify();
                }),
            ))
            .child(nav_button(
                "up",
                NavIcon::Up,
                self.can_go_up(),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.close_context_menu();
                    this.navigate_up_with_watcher(cx);
                    cx.notify();
                }),
            ))
            .child(nav_button(
                "refresh",
                NavIcon::Refresh,
                true,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.close_context_menu();
                    this.refresh_with_entry_metadata_resolution(cx);
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
        let can_extract = self.selected_archive_paths().is_some();
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
                    this.close_context_menu();
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
                CUT_ICON.clone(),
                has_selection,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.close_context_menu();
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
                COPY_ICON.clone(),
                has_selection,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.close_context_menu();
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
                PASTE_ICON.clone(),
                can_paste,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.close_context_menu();
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
                RENAME_ICON.clone(),
                can_rename,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.close_context_menu();
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
                DELETE_ICON.clone(),
                has_selection,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.close_context_menu();
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
                    this.close_context_menu();
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
            .when(can_extract, |this| {
                this.child(utility_separator()).child(utility_action_button(
                    "utility-extract",
                    Some(
                        gpui::img(EXTRACT_ICON.clone())
                            .w(px(UTILITY_TEXT_BUTTON_ICON_SIZE))
                            .h(px(UTILITY_TEXT_BUTTON_ICON_SIZE))
                            .into_any_element(),
                    ),
                    "Extract",
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.close_context_menu();
                        this.open_utility_menu = None;
                        if this.commit_active_rename_before_interaction(window, cx) {
                            this.extract_selected_archives(cx);
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ))
            })
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
                    Some(folder_icon().into_any_element()),
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
                    Some(file_icon().into_any_element()),
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
                            crate::settings::set_show_hidden(!this.show_hidden_files, cx);
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
                            crate::settings::set_show_extensions(
                                !this.show_file_name_extensions,
                                cx,
                            );
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

    fn render_context_menu_overlay(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let native_icon_entry = self.context_menu.as_ref()?.native_icon_entry.clone();
        let native_file_icon = native_icon_entry
            .as_ref()
            .and_then(|entry| self.native_icon_for_entry(entry, cx));
        let native_path_icons = {
            let mut paths = Vec::new();
            collect_context_menu_native_paths(&self.context_menu.as_ref()?.items, &mut paths);
            paths
                .into_iter()
                .filter_map(|path| {
                    self.native_icon_for_path(&path, cx)
                        .map(|icon| (path, icon))
                })
                .collect::<HashMap<_, _>>()
        };
        let url_icon_paths = {
            let mut urls = Vec::new();
            collect_context_menu_url_icons(&self.context_menu.as_ref()?.items, &mut urls);
            urls.into_iter()
                .filter_map(|url| self.cached_url_icon_path(&url, cx).map(|path| (url, path)))
                .collect::<HashMap<_, _>>()
        };
        let menu = self.context_menu.as_ref()?;
        let window_width = f32::from(window.bounds().size.width);
        let window_height = f32::from(window.bounds().size.height);
        let root_height = context_menu_height(
            &menu.items,
            CONTEXT_MENU_ROW_HEIGHT,
            CONTEXT_MENU_ITEM_VERTICAL_GAP,
            CONTEXT_MENU_SEPARATOR_HEIGHT,
        );
        let root_width = context_menu_width(&menu.items, window);
        let (left, top) = context_menu_pointer_tip_origin(
            (f32::from(menu.origin.x), f32::from(menu.origin.y)),
            (root_width, root_height),
            (window_width, window_height),
        );
        let mut menu_elements = Vec::new();

        render_context_menu_level(
            &menu.items,
            &menu.hovered_path,
            Vec::new(),
            Point {
                x: px(left),
                y: px(top),
            },
            (window_width, window_height),
            window,
            cx,
            &mut menu_elements,
            native_file_icon.as_ref(),
            &native_path_icons,
            &url_icon_paths,
        );

        let mut overlay = div().absolute().left(px(0.0)).top(px(0.0)).size_full();

        for menu in menu_elements {
            overlay = overlay.child(menu);
        }

        Some(overlay.into_any_element())
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
        let scroll_left = if self.content_branch() == ExplorerContentBranch::List {
            self.visible_horizontal_scroll_offset()
        } else {
            0.0
        };

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
            .child(
                div().relative().flex_1().h_full().overflow_hidden().child(
                    div()
                        .relative()
                        .left(px(-scroll_left))
                        .flex()
                        .flex_row()
                        .h_full()
                        .w_full()
                        .min_w(px(minimum_file_columns_width()))
                        .child(name_header_cell())
                        .child(header_cell("Date modified", COLUMN_DATE_WIDTH, false))
                        .child(header_cell("Type", COLUMN_TYPE_WIDTH, false))
                        .child(header_cell("Size", COLUMN_SIZE_WIDTH, false)),
                ),
            )
            .child(scrollbar_header_spacer())
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let sections = &self.sidebar_sections;
        let mut children = Vec::new();
        let has_user_directories = !sections.user_directories.is_empty();
        let sidebar_width = normalized_sidebar_width_f32(self.sidebar_width);

        for (index, item) in sections.user_directories.iter().cloned().enumerate() {
            children.push(self.render_sidebar_insertion_zone(
                item.configured_index.unwrap_or(self.sidebar_items.len()),
                index,
                SIDEBAR_ITEM_GAP,
                cx,
            ));
            children.push(self.render_sidebar_row(index, item, cx));
        }
        let final_insertion_index = sections
            .user_directories
            .last()
            .and_then(|item| item.configured_index)
            .map(|index| index + 1)
            .unwrap_or(self.sidebar_items.len());
        children.push(self.render_sidebar_insertion_zone(
            final_insertion_index,
            sections.user_directories.len(),
            if has_user_directories {
                SIDEBAR_ITEM_GAP
            } else {
                SIDEBAR_ROW_HEIGHT
            },
            cx,
        ));

        if has_user_directories && !sections.macos_system_locations.is_empty() {
            children.push(sidebar_separator().into_any_element());
        }

        for (index, item) in sections.macos_system_locations.iter().cloned().enumerate() {
            if index > 0 {
                children.push(sidebar_item_gap().into_any_element());
            }
            children.push(self.render_sidebar_row(index + 1_000, item, cx));
        }

        if !children.is_empty() && !sections.drives.is_empty() {
            children.push(sidebar_separator().into_any_element());
        }

        for (index, item) in sections.drives.iter().cloned().enumerate() {
            if index > 0 {
                children.push(sidebar_item_gap().into_any_element());
            }
            children.push(self.render_sidebar_row(index + 2_000, item, cx));
        }

        div()
            .id("explorer-sidebar")
            .relative()
            .flex()
            .flex_col()
            .h_full()
            .w(px(sidebar_width))
            .flex_shrink_0()
            .bg(rgb(0xffffff))
            .border_r_1()
            .border_color(rgb(0xe7e7e7))
            .pt(px(8.0))
            .overflow_hidden()
            .debug_selector(|| "explorer-sidebar".to_owned())
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if this.close_context_menu() {
                    cx.notify();
                }
            }))
            .children(children)
            .child(self.render_sidebar_resize_handle(cx))
            .into_any_element()
    }

    fn render_sidebar_resize_handle(&self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();

        div()
            .id("explorer-sidebar-resizer")
            .debug_selector(|| "explorer-sidebar-resizer".to_owned())
            .absolute()
            .top(px(0.0))
            .right(px(-(SIDEBAR_RESIZE_HIT_WIDTH / 2.0)))
            .w(px(SIDEBAR_RESIZE_HIT_WIDTH))
            .h_full()
            .cursor(CursorStyle::ResizeColumn)
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
                                    this.close_context_menu();
                                    this.begin_sidebar_resize(f32::from(event.position.x));
                                    cx.stop_propagation();
                                    cx.notify();
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
                                    if this.sidebar_resize_drag.is_none() {
                                        return;
                                    }

                                    if this.update_sidebar_resize(f32::from(event.position.x)) {
                                        cx.notify();
                                    }
                                    cx.stop_propagation();
                                });
                            }
                        });

                        window.on_mouse_event(move |event: &MouseUpEvent, _, _, cx| {
                            match event.button {
                                MouseButton::Left => {
                                    let _ = entity.update(cx, |this, cx| {
                                        let Some(width) = this.finish_sidebar_resize() else {
                                            return;
                                        };

                                        crate::settings::set_sidebar_width(width, cx);
                                        cx.stop_propagation();
                                        cx.notify();
                                    });
                                }
                                MouseButton::Right if bounds.contains(&event.position) => {
                                    let _ = entity.update(cx, |this, cx| {
                                        this.close_context_menu();
                                        let width = this.reset_sidebar_width();
                                        crate::settings::set_sidebar_width(width, cx);
                                        cx.stop_propagation();
                                        cx.notify();
                                    });
                                }
                                _ => {}
                            }
                        });
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }

    fn render_sidebar_insertion_zone(
        &self,
        insertion_index: usize,
        id: usize,
        height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(("explorer-sidebar-insertion", id))
            .h(px(height))
            .mx(px(8.0))
            .flex_shrink_0()
            .can_drop(move |dragged_value, _, cx| {
                let Some(path) = sidebar_pin_path_from_value(dragged_value) else {
                    return false;
                };
                cx.try_global::<SettingsState>()
                    .is_some_and(|state| crate::settings::can_pin_sidebar_path(&path, &state.value))
            })
            .drag_over::<DraggedEntries>(|style, _, _, _| style.bg(rgb(0x0078d7)))
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(0x0078d7)))
            .on_drop(cx.listener(move |this, dragged: &DraggedEntries, _, cx| {
                this.clear_drop_indicator();
                if let Some(path) = sidebar_pin_path_from_value(dragged) {
                    crate::settings::pin_sidebar_path(path, insertion_index, cx);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                this.clear_drop_indicator();
                if let Some(path) = sidebar_pin_path_from_value(paths) {
                    crate::settings::pin_sidebar_path(path, insertion_index, cx);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_sidebar_row(
        &self,
        id: usize,
        item: SidebarItem,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_current = item.path == self.path;
        let label = item.label.clone();
        let path = item.path.clone();
        let icon_item = item.clone();
        let configured_index = item.configured_index;
        let is_dragging = sidebar_item_is_dragging(configured_index, self.dragging_sidebar_item);
        let is_user_directory = matches!(
            item.kind,
            SidebarItemKind::Directory(_) | SidebarItemKind::CustomDirectory
        );
        let is_bin = matches!(item.kind, SidebarItemKind::Directory(DirectoryKind::Bin));
        let destination = DropDestination::Directory {
            item_path: path.clone(),
            target_path: path.clone(),
        };
        let entity = cx.entity();

        let click_path = path.clone();
        let middle_click_path = path.clone();
        let context_menu_target = sidebar_context_menu_target(&item);
        let context_menu_active = sidebar_context_menu_is_active(self.context_menu.as_ref(), id);
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
            .when(is_dragging, |this| this.opacity(0.4))
            .bg(rgb(sidebar_row_background_color(
                is_current,
                context_menu_active,
            )))
            .when(!is_current && !context_menu_active, |this| {
                this.hover(|style| style.bg(rgb(SIDEBAR_ROW_HOVER_BG)))
            })
            .active(|style| style.opacity(NAV_BUTTON_ACTIVE_OPACITY))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.close_context_menu();
                this.navigate_to_sidebar_path_with_watcher(click_path.clone(), cx);
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |_, _: &MouseDownEvent, _, cx| {
                    cx.emit(ExplorerViewEvent::OpenDirectoryInNewTab(
                        middle_click_path.clone(),
                    ));
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(sidebar_item_icon(icon_item))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .ml(px(SIDEBAR_ICON_TEXT_GAP))
                    .truncate()
                    .text_size(px(SIDEBAR_TEXT_SIZE))
                    .text_color(rgb(0x1f1f1f))
                    .child(SharedString::from(label)),
            );

        let (context_path, context_configured_index, open_icon_kind) = context_menu_target;
        row = row.on_mouse_up(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                open_sidebar_context_menu_from_event(
                    this,
                    event,
                    context_path.clone(),
                    id,
                    context_configured_index,
                    open_icon_kind,
                    window,
                    cx,
                );
            }),
        );

        if let Some(configured_index) = configured_index {
            let drag_label = SharedString::from(item.label.clone());
            let drag_kind = item.kind;
            row = row
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                        if this.dragging_sidebar_item.take().is_some() {
                            cx.notify();
                        }
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                        if this.dragging_sidebar_item.take().is_some() {
                            cx.notify();
                        }
                    }),
                )
                .on_drag(
                    SidebarItemDrag {
                        configured_index,
                        label: drag_label,
                        kind: drag_kind,
                    },
                    {
                        let entity = entity.clone();
                        move |drag, _, _, cx| {
                            let width = entity.update(cx, |this, cx| {
                                this.dragging_sidebar_item = Some(drag.configured_index);
                                cx.notify();
                                this.sidebar_width
                            });
                            let font = entity.read(cx).font.clone();
                            cx.new(move |_| SidebarItemDragPreview {
                                label: drag.label.clone(),
                                kind: drag.kind,
                                width,
                                font,
                            })
                        }
                    },
                )
                .on_drag_move::<SidebarItemDrag>({
                    let entity = entity.clone();
                    move |event: &DragMoveEvent<SidebarItemDrag>, _, cx| {
                        if !event.bounds.contains(&event.event.position) {
                            return;
                        }
                        let top = f32::from(event.bounds.origin.y);
                        let height = f32::from(event.bounds.size.height);
                        let cursor_y = f32::from(event.event.position.y);
                        let before = cursor_y < top + (height / 2.0);
                        let fallback_source = event.drag(cx).configured_index;

                        let _ = entity.update(cx, |this, cx| {
                            let source_index =
                                this.dragging_sidebar_item.unwrap_or(fallback_source);
                            if let Some(new_index) = crate::settings::reorder_sidebar_item(
                                source_index,
                                configured_index,
                                before,
                                cx,
                            ) {
                                this.dragging_sidebar_item = Some(new_index);
                                cx.notify();
                            }
                        });
                    }
                });
        }

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

    fn render_row(&mut self, ix: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let entry = self.entries[ix].clone();
        let app_icon = self.native_icon_for_entry(&entry, cx);
        let is_selected = self.entry_is_selected(ix);
        let context_menu_active = self.context_menu.is_some();
        let is_cut = self.entry_is_cut(&entry.path);
        let clicked_entry = entry.clone();
        let context_clicked_entry = entry.clone();
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
            .when(
                entry_row_hover_enabled(is_selected, context_menu_active),
                |this| this.hover(|style| style.bg(rgb(0xe5f3ff))),
            )
            .border_1()
            .border_color(rgb(0xffffff))
            // .border_color(rgb(0x949494))
            .cursor_default()
            .when(is_cut, |this| this.opacity(CUT_ITEM_OPACITY))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if !is_normal_entry_click(event) {
                    cx.stop_propagation();
                    return;
                }

                this.close_context_menu();
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

                let click_count =
                    this.normalize_entry_click_count(&clicked_entry, event.click_count());
                if let Some(EntryAction::OpenFile(path)) = this.handle_entry_click_with_watcher(
                    &clicked_entry,
                    click_count,
                    selection_modifiers_for_click(event),
                    cx,
                ) {
                    this.open_file_with_default_app(&path, window, cx);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                    open_entry_context_menu_from_event(
                        this,
                        event,
                        &context_clicked_entry,
                        window,
                        cx,
                    );
                }),
            )
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
                    let font = entity.read(cx).font.clone();
                    cx.new(|_| DragPreview::new(dragged, cursor_offset, font))
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
            format_timestamp(entry.modified, &self.date_format),
            COLUMN_DATE_WIDTH,
            false,
            &self.font,
            window,
        );
        let type_cell = text_cell(
            entry.type_label(),
            COLUMN_TYPE_WIDTH,
            false,
            &self.font,
            window,
        );
        let size_cell = text_cell(
            format_size(entry.size),
            COLUMN_SIZE_WIDTH,
            true,
            &self.font,
            window,
        );

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
            rename_name_cell(&entry, app_icon, self.active_rename_focus_handle(), cx)
                .into_any_element()
        } else {
            let name_click_entry = entry.clone();
            let name_context_clicked_entry = entry.clone();
            let name_middle_clicked_entry = entry.clone();
            name_cell(
                &entry,
                app_icon,
                self.show_file_name_extensions,
                self.recursive_search_results_active(),
                self.sidebar_width,
                &self.font,
                window,
            )
            .id(("explorer-entry-name", ix))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if !is_normal_entry_click(event) {
                    cx.stop_propagation();
                    return;
                }

                this.close_context_menu();
                if this.suppress_next_click() {
                    this.cancel_pending_click_rename();
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }

                let click_count =
                    this.normalize_entry_click_count(&name_click_entry, event.click_count());
                if let Some(EntryAction::OpenFile(path)) = this.handle_entry_name_click(
                    &name_click_entry,
                    click_count,
                    selection_modifiers_for_click(event),
                    window,
                    cx,
                ) {
                    this.open_file_with_default_app(&path, window, cx);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                    let clicked_index = this.entry_index_by_path(&name_context_clicked_entry.path);
                    if clicked_index.is_some_and(|ix| this.entry_is_selected(ix)) {
                        open_entry_context_menu_from_event(
                            this,
                            event,
                            &name_context_clicked_entry,
                            window,
                            cx,
                        );
                    } else {
                        open_current_folder_context_menu_from_event(this, event, window, cx);
                    }
                }),
            )
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
        let has_horizontal_scrollbar = self.horizontal_scrollbar_metrics().is_some();

        div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
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
                            .on_mouse_up(
                                MouseButton::Right,
                                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                                    open_current_folder_context_menu_from_event(
                                        this, event, window, cx,
                                    );
                                }),
                            )
                            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                                if !event.standard_click() {
                                    cx.stop_propagation();
                                    return;
                                }

                                this.close_context_menu();
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
                                this.close_context_menu();
                                cx.notify();
                            }))
                            .child(self.render_mouse_selection_hit_layer(cx))
                            .child(
                                uniform_list(
                                    "explorer-entries",
                                    self.entries.len(),
                                    cx.processor(|this, range: Range<usize>, window, cx| {
                                        let mut rows = Vec::with_capacity(range.end - range.start);
                                        for ix in range {
                                            rows.push(this.render_row(ix, window, cx));
                                        }
                                        rows
                                    }),
                                )
                                .with_horizontal_sizing_behavior(
                                    ListHorizontalSizingBehavior::Unconstrained,
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
                    .child(self.render_scrollbar(cx)),
            )
            .when(has_horizontal_scrollbar, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_row()
                        .w_full()
                        .h(px(SCROLLBAR_GUTTER_WIDTH))
                        .child(self.render_horizontal_scrollbar(cx))
                        .child(scrollbar_corner()),
                )
            })
    }

    fn render_empty_folder(&self, message: &'static str, cx: &mut Context<Self>) -> AnyElement {
        self.render_empty_folder_with_detail(Some(message), None, cx)
    }

    fn render_empty_folder_with_detail(
        &self,
        message: Option<&'static str>,
        detail: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    open_current_folder_context_menu_from_event(this, event, window, cx);
                }),
            )
            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                if !event.standard_click() {
                    cx.stop_propagation();
                    return;
                }

                this.close_context_menu();
                if this.commit_active_rename_before_interaction(window, cx) {
                    this.clear_selection();
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .child(render_empty_folder_message(message, detail))
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
        let Some(selection_box) = self.visible_mouse_selection_box() else {
            return div().into_any_element();
        };

        let bounds = selection_box_bounds(selection_box);
        div()
            .debug_selector(|| "mouse-selection-box".to_owned())
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
                    move |event: &MouseDownEvent, _, window, cx| {
                        if !matches!(event.button, MouseButton::Left | MouseButton::Right)
                            || !bounds.contains(&event.position)
                        {
                            return;
                        }

                        let local_position = local_point(event.position, &bounds);
                        let viewport_size = viewport_size(&bounds);
                        let modifiers = SelectionModifiers::from_gpui(event.modifiers);
                        let _ = entity.update(cx, |this, cx| {
                            if context_menu_contains_window_position(this, event.position, window) {
                                return;
                            }

                            let outcome = this.begin_mouse_selection_drag_after_menu_dismissal(
                                event.button,
                                local_position,
                                viewport_size,
                                modifiers,
                            );
                            if outcome.menu_closed || outcome.selection_started {
                                cx.notify();
                            }
                        });
                    }
                });

                window.on_mouse_event({
                    let entity = entity.clone();
                    move |event: &MouseMoveEvent, _, _, cx| {
                        if !matches!(
                            event.pressed_button,
                            Some(MouseButton::Left | MouseButton::Right)
                        ) {
                            return;
                        }

                        let local_position = local_point(event.position, &bounds);
                        let viewport_size = viewport_size(&bounds);
                        let _ = entity.update(cx, |this, cx| {
                            if this
                                .mouse_selection_drag
                                .as_ref()
                                .is_none_or(|drag| Some(drag.button) != event.pressed_button)
                            {
                                return;
                            }

                            this.update_mouse_selection_drag(local_position, viewport_size);
                            cx.notify();
                        });
                    }
                });

                window.on_mouse_event(move |event: &MouseUpEvent, _, window, cx| {
                    if !matches!(event.button, MouseButton::Left | MouseButton::Right) {
                        return;
                    }

                    let _ = entity.update(cx, |this, cx| {
                        if this
                            .mouse_selection_drag
                            .as_ref()
                            .is_some_and(|drag| drag.button == event.button)
                        {
                            this.update_mouse_selection_drag(
                                local_point(event.position, &bounds),
                                viewport_size(&bounds),
                            );
                        }
                        let activated = this.end_mouse_selection_drag(event.button);
                        if activated && event.button == MouseButton::Right {
                            if this.selection.selected_indices.is_empty() {
                                open_current_folder_context_menu_from_event(
                                    this, event, window, cx,
                                );
                            } else {
                                open_selected_entries_context_menu_from_event(
                                    this, event, window, cx,
                                );
                            }
                        }
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
        let focus_handle = self.focus_handle(cx);
        let entity = cx.entity();

        div()
            .key_context("Explorer")
            .track_focus(&focus_handle)
            .on_key_down(cx.listener(Self::handle_type_to_search))
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
            .on_action(cx.listener(Self::handle_open_properties))
            .on_action(cx.listener(Self::handle_open_settings))
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
            .on_action(cx.listener(Self::handle_recursive_search_edit))
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
            .on_children_prepainted(move |child_bounds, _, cx| {
                let _ = entity.update(cx, |this, _| {
                    this.update_view_origin_from_child_bounds(&child_bounds);
                });
            })
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
                    .child(self.render_sidebar(cx))
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
                                    ExplorerContentBranch::SearchWorking => {
                                        div().child(self.render_empty_folder_with_detail(
                                            None,
                                            Some(search_working_detail(
                                                self.recursive_search_progress(),
                                            )),
                                            cx,
                                        ))
                                    }
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
                            .when(self.recursive_search_is_working(), |this| {
                                this.child(linear_indeterminate(
                                    "recursive-search-linear-progress",
                                    LinearProgressStyle::explorer_copy_green(),
                                ))
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
            .when_some(
                self.render_context_menu_overlay(window, cx),
                |this, menu| this.child(menu),
            )
    }
}

pub(super) fn render_drop_indicator(
    indicator: DropIndicator,
    font: &gpui::Font,
    window: &Window,
) -> AnyElement {
    let origin = drop_indicator_origin(indicator.mouse_position);
    let (icon, action_label) = match indicator.operation {
        FileOperationKind::Move => (NavIcon::Forward.glyph(), "Move to"),
        FileOperationKind::Copy => ("\u{E710}", "Copy to"),
    };
    let target_width = drop_indicator_target_width(measure_drop_indicator_target_text(
        &indicator.target_label,
        font,
        window,
    ));
    let target_label =
        truncated_drop_indicator_target_label(&indicator.target_label, target_width, font, window);

    div()
        .font(font.clone())
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

fn measure_drop_indicator_target_text(text: &str, font: &gpui::Font, window: &Window) -> f32 {
    if text.is_empty() {
        return 0.0;
    }

    let run = TextRun {
        len: text.len(),
        font: font.clone(),
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
    target_font: &gpui::Font,
    window: &Window,
) -> SharedString {
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
        .line_wrapper(target_font.clone(), px(DROP_INDICATOR_TEXT_SIZE))
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

fn sidebar_item_gap() -> Div {
    div().h(px(SIDEBAR_ITEM_GAP)).flex_shrink_0()
}

fn sidebar_pin_path_from_value(dragged_value: &dyn Any) -> Option<PathBuf> {
    let paths = if let Some(dragged) = dragged_value.downcast_ref::<DraggedEntries>() {
        &dragged.paths
    } else if let Some(paths) = dragged_value.downcast_ref::<ExternalPaths>() {
        paths.paths()
    } else {
        return None;
    };

    let [path] = paths else {
        return None;
    };
    path.is_dir().then(|| path.clone())
}

fn sidebar_item_is_dragging(
    configured_index: Option<usize>,
    dragging_index: Option<usize>,
) -> bool {
    configured_index.is_some() && configured_index == dragging_index
}

fn sidebar_context_menu_target(
    item: &SidebarItem,
) -> (PathBuf, Option<usize>, Option<DirectoryKind>) {
    let open_icon_kind = match item.kind {
        SidebarItemKind::Directory(kind) => Some(kind),
        SidebarItemKind::CustomDirectory => crate::explorer::resolve_directory_kind(&item.path),
        SidebarItemKind::Drive => Some(DirectoryKind::Drive),
        SidebarItemKind::DriveWindows => Some(DirectoryKind::DriveWindows),
    };
    (item.path.clone(), item.configured_index, open_icon_kind)
}

fn sidebar_context_menu_is_active(
    context_menu: Option<&crate::explorer::context_menu::ContextMenuState>,
    row_id: usize,
) -> bool {
    matches!(
        context_menu.and_then(|menu| menu.source),
        Some(ContextMenuSource::SidebarItem { row_id: active_id }) if active_id == row_id
    )
}

fn sidebar_row_background_color(is_current: bool, context_menu_active: bool) -> u32 {
    if is_current {
        SIDEBAR_ROW_CURRENT_BG
    } else if context_menu_active {
        SIDEBAR_ROW_HOVER_BG
    } else {
        SIDEBAR_ROW_BG
    }
}

fn entry_row_hover_enabled(is_selected: bool, context_menu_active: bool) -> bool {
    !is_selected && !context_menu_active
}

fn sidebar_item_icon(item: SidebarItem) -> AnyElement {
    sidebar_item_kind_icon(item.kind)
}

fn sidebar_item_kind_icon(kind: SidebarItemKind) -> AnyElement {
    match kind {
        SidebarItemKind::Directory(kind) => directory_kind_icon(kind),
        SidebarItemKind::CustomDirectory => folder_icon().into_any_element(),
        SidebarItemKind::Drive => drive_icon().into_any_element(),
        SidebarItemKind::DriveWindows => drive_windows_icon().into_any_element(),
    }
}

impl Focusable for ExplorerView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle
            .clone()
            .expect("ExplorerView must be constructed with a FocusHandle before rendering")
    }
}

fn render_empty_folder_message(message: Option<&'static str>, detail: Option<String>) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .mt(px(EMPTY_FOLDER_TOP_MARGIN))
        .text_center()
        .text_size(px(EMPTY_FOLDER_TEXT_SIZE))
        .text_color(rgb(0x9a9a9a))
        .when_some(message, |this, message| this.child(message))
        .when_some(detail, |this, detail| {
            this.child(div().child(SharedString::from(detail)))
        })
}

fn search_working_detail(progress: RecursiveSearchProgressSnapshot) -> String {
    match progress {
        RecursiveSearchProgressSnapshot::Scanning(count) => {
            format!("Scanning {}...", count.separate_with_commas())
        }
        RecursiveSearchProgressSnapshot::Searching(Some(count)) => {
            format!("Searching {}...", count.separate_with_commas())
        }
        RecursiveSearchProgressSnapshot::Searching(None) => SEARCH_WORKING_MESSAGE.to_owned(),
    }
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

fn local_context_menu_origin(
    window_position: Point<Pixels>,
    view_origin: Point<Pixels>,
) -> Point<Pixels> {
    window_position - view_origin
}

fn context_menu_contains_window_position(
    this: &ExplorerView,
    window_position: Point<Pixels>,
    window: &Window,
) -> bool {
    let Some(menu) = this.context_menu.as_ref() else {
        return false;
    };

    let window_size = (
        f32::from(window.bounds().size.width),
        f32::from(window.bounds().size.height),
    );
    let root_width = context_menu_width(&menu.items, window);
    let root_height = context_menu_height(
        &menu.items,
        CONTEXT_MENU_ROW_HEIGHT,
        CONTEXT_MENU_ITEM_VERTICAL_GAP,
        CONTEXT_MENU_SEPARATOR_HEIGHT,
    );
    let (left, top) = context_menu_pointer_tip_origin(
        (f32::from(menu.origin.x), f32::from(menu.origin.y)),
        (root_width, root_height),
        window_size,
    );
    let position = local_context_menu_origin(window_position, this.view_origin);

    context_menu_level_contains_position(
        &menu.items,
        &menu.hovered_path,
        &[],
        (left, top),
        window_size,
        (f32::from(position.x), f32::from(position.y)),
        window,
    )
}

fn context_menu_level_contains_position(
    items: &[ContextMenuItem],
    hovered_path: &[usize],
    path_prefix: &[usize],
    origin: (f32, f32),
    window_size: (f32, f32),
    position: (f32, f32),
    window: &Window,
) -> bool {
    let menu_width = context_menu_width(items, window);
    let menu_height = context_menu_height(
        items,
        CONTEXT_MENU_ROW_HEIGHT,
        CONTEXT_MENU_ITEM_VERTICAL_GAP,
        CONTEXT_MENU_SEPARATOR_HEIGHT,
    );
    let (left, top) = clamped_context_menu_origin(origin, (menu_width, menu_height), window_size);
    if position.0 >= left
        && position.0 <= left + menu_width
        && position.1 >= top
        && position.1 <= top + menu_height
    {
        return true;
    }

    for (index, item) in items.iter().enumerate() {
        let mut path = path_prefix.to_vec();
        path.push(index);
        let ContextMenuItem::Submenu { children, .. } = item else {
            continue;
        };
        if !context_menu_path_is_active(hovered_path, &path) {
            continue;
        }

        let child_width = context_menu_width(children, window);
        let child_height = context_menu_height(
            children,
            CONTEXT_MENU_ROW_HEIGHT,
            CONTEXT_MENU_ITEM_VERTICAL_GAP,
            CONTEXT_MENU_SEPARATOR_HEIGHT,
        );
        let child_left = context_submenu_left(
            left,
            menu_width,
            child_width,
            CONTEXT_MENU_SUBMENU_OVERLAP,
            window_size.0,
        );
        let row_top = context_menu_item_top(
            items,
            index,
            CONTEXT_MENU_ROW_HEIGHT,
            CONTEXT_MENU_ITEM_VERTICAL_GAP,
            CONTEXT_MENU_SEPARATOR_HEIGHT,
        );
        let (_, child_top) = clamped_context_menu_origin(
            (child_left, top + row_top - 10.0),
            (child_width, child_height),
            window_size,
        );
        return context_menu_level_contains_position(
            children,
            hovered_path,
            &path,
            (child_left, child_top),
            window_size,
            position,
            window,
        );
    }

    false
}

impl ExplorerView {
    fn update_view_origin_from_child_bounds(&mut self, child_bounds: &[Bounds<Pixels>]) {
        if self.context_menu.is_some() {
            return;
        }

        if let Some(first_child) = child_bounds.first() {
            self.view_origin = first_child.origin;
        }
    }
}

fn open_current_folder_context_menu_from_event(
    this: &mut ExplorerView,
    event: &MouseUpEvent,
    window: &mut Window,
    cx: &mut Context<ExplorerView>,
) {
    let clipboard = cx.read_from_clipboard();
    let can_paste = clipboard_has_file_clipboard(clipboard.as_ref());
    let origin = local_context_menu_origin(event.position, this.view_origin);
    if this.open_folder_context_menu(origin, can_paste, window, cx) {
        cx.notify();
    }
    cx.stop_propagation();
}

fn open_entry_context_menu_from_event(
    this: &mut ExplorerView,
    event: &MouseUpEvent,
    entry: &FileEntry,
    window: &mut Window,
    cx: &mut Context<ExplorerView>,
) {
    let origin = local_context_menu_origin(event.position, this.view_origin);
    if this.open_entry_context_menu(origin, entry, window, cx) {
        cx.notify();
    }
    cx.stop_propagation();
}

fn open_selected_entries_context_menu_from_event(
    this: &mut ExplorerView,
    event: &MouseUpEvent,
    window: &mut Window,
    cx: &mut Context<ExplorerView>,
) {
    let origin = local_context_menu_origin(event.position, this.view_origin);
    if this.open_selected_entries_context_menu(origin, window, cx) {
        cx.notify();
    }
    cx.stop_propagation();
}

fn open_sidebar_context_menu_from_event(
    this: &mut ExplorerView,
    event: &MouseUpEvent,
    path: PathBuf,
    row_id: usize,
    configured_index: Option<usize>,
    open_icon_kind: Option<DirectoryKind>,
    window: &mut Window,
    cx: &mut Context<ExplorerView>,
) {
    let origin = local_context_menu_origin(event.position, this.view_origin);
    if this.open_sidebar_context_menu(
        origin,
        path,
        row_id,
        configured_index,
        open_icon_kind,
        window,
        cx,
    ) {
        cx.notify();
    }
    cx.stop_propagation();
}

fn utility_text_button(
    id: &'static str,
    left_icon: Option<AnyElement>,
    label: &'static str,
    is_open: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    utility_text_button_base(id, left_icon, label, is_open, true, on_click)
}

fn utility_action_button(
    id: &'static str,
    left_icon: Option<AnyElement>,
    label: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    utility_text_button_base(id, left_icon, label, false, false, on_click)
}

fn utility_text_button_base(
    id: &'static str,
    left_icon: Option<AnyElement>,
    label: &'static str,
    is_open: bool,
    show_chevron: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
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
        .when(show_chevron, |this| {
            this.child(
                div()
                    .font(nav_icon_font())
                    .text_size(px(7.0))
                    .mt(px(2.0))
                    .text_color(rgb(0x505050))
                    .child(UTILITY_ICON_CHEVRON_DOWN),
            )
        })
        .into_any_element()
}

fn utility_new_icon() -> gpui::Img {
    gpui::img(NEW_ITEM_ICON.clone())
        .w(px(UTILITY_TEXT_BUTTON_ICON_SIZE))
        .h(px(UTILITY_TEXT_BUTTON_ICON_SIZE))
}

fn utility_view_icon() -> gpui::Img {
    gpui::img(DETAILS_ICON.clone())
        .w(px(UTILITY_TEXT_BUTTON_ICON_SIZE))
        .h(px(UTILITY_TEXT_BUTTON_ICON_SIZE))
}

fn utility_icon_button(
    id: &'static str,
    icon: Arc<Image>,
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
            gpui::img(icon)
                .w(px(16.0))
                .h(px(16.0))
                .when(!enabled, |this| this.opacity(0.4)),
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

fn context_menu_dropdown(width: f32) -> Div {
    div()
        .debug_selector(|| "context-menu".to_owned())
        .w(px(width))
        .rounded(px(6.0))
        .bg(rgb(0xffffff))
        .border_1()
        .border_color(rgb(0xd8d8d8))
        .shadow_md()
        .occlude()
        .on_any_mouse_down(|_, _, cx| {
            cx.stop_propagation();
        })
}

fn render_context_menu_level(
    items: &[ContextMenuItem],
    hovered_path: &[usize],
    path_prefix: Vec<usize>,
    origin: Point<Pixels>,
    window_size: (f32, f32),
    window: &Window,
    cx: &mut Context<ExplorerView>,
    elements: &mut Vec<AnyElement>,
    native_file_icon: Option<&Arc<Image>>,
    native_path_icons: &HashMap<PathBuf, Arc<Image>>,
    url_icon_paths: &HashMap<String, PathBuf>,
) {
    let menu_width = context_menu_width(items, window);
    let menu_height = context_menu_height(
        items,
        CONTEXT_MENU_ROW_HEIGHT,
        CONTEXT_MENU_ITEM_VERTICAL_GAP,
        CONTEXT_MENU_SEPARATOR_HEIGHT,
    );
    let (left, top) = clamped_context_menu_origin(
        (f32::from(origin.x), f32::from(origin.y)),
        (menu_width, menu_height),
        window_size,
    );
    let mut menu = context_menu_dropdown(menu_width)
        .absolute()
        .left(px(left))
        .top(px(top));
    let mut active_submenu: Option<(&[ContextMenuItem], Vec<usize>, f32)> = None;

    for (index, item) in items.iter().enumerate() {
        let mut path = path_prefix.clone();
        path.push(index);
        let row_top = context_menu_item_top(
            items,
            index,
            CONTEXT_MENU_ROW_HEIGHT,
            CONTEXT_MENU_ITEM_VERTICAL_GAP,
            CONTEXT_MENU_SEPARATOR_HEIGHT,
        );

        if let ContextMenuItem::Submenu { children, .. } = item
            && context_menu_path_is_active(hovered_path, &path)
        {
            active_submenu = Some((children.as_slice(), path.clone(), row_top));
        }

        menu = menu.child(render_context_menu_item(
            item,
            path,
            hovered_path,
            cx,
            native_file_icon,
            native_path_icons,
            url_icon_paths,
        ));
    }

    elements.push(menu.into_any_element());

    if let Some((children, child_path, row_top)) = active_submenu {
        let child_width = context_menu_width(children, window);
        let child_height = context_menu_height(
            children,
            CONTEXT_MENU_ROW_HEIGHT,
            CONTEXT_MENU_ITEM_VERTICAL_GAP,
            CONTEXT_MENU_SEPARATOR_HEIGHT,
        );
        let child_left = context_submenu_left(
            left,
            menu_width,
            child_width,
            CONTEXT_MENU_SUBMENU_OVERLAP,
            window_size.0,
        );
        let submenu_y_middle_offset = 10.0;
        let (_, child_top) = clamped_context_menu_origin(
            (child_left, top + row_top - submenu_y_middle_offset),
            (child_width, child_height),
            window_size,
        );

        render_context_menu_level(
            children,
            hovered_path,
            child_path,
            Point {
                x: px(child_left),
                y: px(child_top),
            },
            window_size,
            window,
            cx,
            elements,
            native_file_icon,
            native_path_icons,
            url_icon_paths,
        );
    }
}

fn render_context_menu_item(
    item: &ContextMenuItem,
    path: Vec<usize>,
    hovered_path: &[usize],
    cx: &mut Context<ExplorerView>,
    native_file_icon: Option<&Arc<Image>>,
    native_path_icons: &HashMap<PathBuf, Arc<Image>>,
    url_icon_paths: &HashMap<String, PathBuf>,
) -> AnyElement {
    let active = context_menu_item_is_persistently_active(item, hovered_path, &path);

    match item {
        ContextMenuItem::Action {
            id,
            icon,
            label,
            command,
            enabled,
        } => context_menu_action_row(
            id,
            icon.clone(),
            label,
            command.clone(),
            *enabled,
            path,
            active,
            cx,
            native_file_icon,
            native_path_icons,
            url_icon_paths,
        ),
        ContextMenuItem::Submenu {
            id, icon, label, ..
        } => context_menu_submenu_row(
            id,
            icon.clone(),
            label,
            path,
            active,
            cx,
            native_file_icon,
            native_path_icons,
            url_icon_paths,
        ),
        ContextMenuItem::Separator => context_menu_separator().into_any_element(),
        ContextMenuItem::Detail {
            label,
            value,
            icon_slot,
        } => context_menu_detail_row(
            label,
            value,
            *icon_slot,
            path,
            active,
            cx,
            native_path_icons,
            url_icon_paths,
        ),
    }
}

fn context_menu_action_row(
    id: &str,
    icon: Option<ContextMenuIcon>,
    label: &str,
    command: ContextMenuCommand,
    enabled: bool,
    path: Vec<usize>,
    active: bool,
    cx: &mut Context<ExplorerView>,
    native_file_icon: Option<&Arc<Image>>,
    native_path_icons: &HashMap<PathBuf, Arc<Image>>,
    url_icon_paths: &HashMap<String, PathBuf>,
) -> AnyElement {
    context_menu_row_base(
        id,
        icon,
        ContextMenuIconSlot::Reserve,
        path,
        active,
        cx,
        native_file_icon,
        native_path_icons,
        url_icon_paths,
    )
    .child(context_menu_label(label, true))
    .when(!enabled, |this| this.opacity(0.45))
    .when(enabled, |this| {
        this.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.execute_context_menu_command(command.clone(), window, cx);
            cx.stop_propagation();
            cx.notify();
        }))
    })
    .child(context_menu_trailing_slot(None))
    .into_any_element()
}

fn context_menu_submenu_row(
    id: &str,
    icon: Option<ContextMenuIcon>,
    label: &str,
    path: Vec<usize>,
    active: bool,
    cx: &mut Context<ExplorerView>,
    native_file_icon: Option<&Arc<Image>>,
    native_path_icons: &HashMap<PathBuf, Arc<Image>>,
    url_icon_paths: &HashMap<String, PathBuf>,
) -> AnyElement {
    context_menu_row_base(
        id,
        icon,
        ContextMenuIconSlot::Reserve,
        path,
        active,
        cx,
        native_file_icon,
        native_path_icons,
        url_icon_paths,
    )
    .child(context_menu_label(label, true))
    .child(context_menu_trailing_slot(Some(CONTEXT_MENU_CHEVRON)))
    .into_any_element()
}

fn context_menu_detail_row(
    label: &'static str,
    value: &str,
    icon_slot: ContextMenuIconSlot,
    path: Vec<usize>,
    active: bool,
    cx: &mut Context<ExplorerView>,
    native_path_icons: &HashMap<PathBuf, Arc<Image>>,
    url_icon_paths: &HashMap<String, PathBuf>,
) -> AnyElement {
    let id = match label {
        "Created" => "context-menu-created",
        "Modified" => "context-menu-modified",
        _ => "context-menu-detail",
    };

    context_menu_row_base(
        id,
        None,
        icon_slot,
        path,
        active,
        cx,
        None,
        native_path_icons,
        url_icon_paths,
    )
    .child(context_menu_label(label, false))
    .child(
        div()
            .debug_selector(|| "context-menu-detail-value".to_owned())
            .ml(px(CONTEXT_MENU_DETAIL_VALUE_LEFT_MARGIN))
            .min_w(px(0.0))
            .flex_1()
            .truncate()
            .text_align(TextAlign::Right)
            .text_size(px(CONTEXT_MENU_TEXT_SIZE))
            .text_color(rgb(0x595959))
            .opacity(0.8)
            .child(SharedString::from(value.to_owned())),
    )
    .into_any_element()
}

fn context_menu_row_base(
    id: &str,
    icon: Option<ContextMenuIcon>,
    icon_slot: ContextMenuIconSlot,
    path: Vec<usize>,
    active: bool,
    cx: &mut Context<ExplorerView>,
    native_file_icon: Option<&Arc<Image>>,
    native_path_icons: &HashMap<PathBuf, Arc<Image>>,
    url_icon_paths: &HashMap<String, PathBuf>,
) -> gpui::Stateful<Div> {
    let id = id.to_owned();
    div()
        .id(SharedString::from(id.clone()))
        .debug_selector(move || id.clone())
        .flex()
        .flex_row()
        .items_center()
        .h(px(CONTEXT_MENU_ROW_HEIGHT))
        .mx(px(CONTEXT_MENU_ROW_OUTER_HORIZONTAL_PADDING / 2.0))
        .px(px(CONTEXT_MENU_ROW_INNER_HORIZONTAL_PADDING / 2.0))
        .gap(px(CONTEXT_MENU_ROW_CHILD_GAP))
        .cursor_default()
        .when(active, |this| this.bg(rgb(0xe5f3ff)))
        .hover(|style| style.bg(rgb(0xe5f3ff)))
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            if *hovered {
                this.set_context_menu_hovered_path(path.clone());
                cx.notify();
            }
        }))
        .when(icon_slot == ContextMenuIconSlot::Reserve, |this| {
            this.child(context_menu_icon_slot(
                icon,
                native_file_icon,
                native_path_icons,
                url_icon_paths,
            ))
        })
}

fn context_menu_label(label: &str, flexible: bool) -> Div {
    div()
        .when(flexible, |this| this.flex_1().min_w(px(0.0)))
        .when(!flexible, |this| this.flex_shrink_0())
        .truncate()
        .text_size(px(CONTEXT_MENU_TEXT_SIZE))
        .text_color(rgb(0x1f1f1f))
        .child(SharedString::from(label.to_owned()))
}

fn context_menu_width(items: &[ContextMenuItem], window: &Window) -> f32 {
    let natural_width = items
        .iter()
        .map(|item| context_menu_item_width(item, window))
        .fold(0.0, f32::max);

    context_menu_width_for_natural_width(natural_width)
}

fn context_menu_width_for_natural_width(natural_width: f32) -> f32 {
    natural_width
        .max(CONTEXT_MENU_MIN_WIDTH)
        .min(CONTEXT_MENU_MAX_WIDTH)
}

fn context_menu_item_width(item: &ContextMenuItem, window: &Window) -> f32 {
    match item {
        ContextMenuItem::Action { label, .. } => {
            context_menu_action_width_for_text_width(context_menu_text_width(label, window))
        }
        ContextMenuItem::Submenu { label, .. } => {
            context_menu_action_width_for_text_width(context_menu_text_width(label, window))
        }
        ContextMenuItem::Detail {
            label,
            value,
            icon_slot,
        } => context_menu_detail_width_for_text_widths(
            context_menu_text_width(label, window),
            context_menu_text_width(value, window),
            *icon_slot,
        ),
        ContextMenuItem::Separator => 0.0,
    }
}

fn context_menu_action_width_for_text_width(text_width: f32) -> f32 {
    context_menu_row_horizontal_chrome(ContextMenuIconSlot::Reserve, 2)
        + text_width
        + CONTEXT_MENU_TRAILING_SLOT_WIDTH
}

fn context_menu_detail_width_for_text_widths(
    label_width: f32,
    value_width: f32,
    icon_slot: ContextMenuIconSlot,
) -> f32 {
    context_menu_row_horizontal_chrome(icon_slot, 2)
        + label_width
        + CONTEXT_MENU_DETAIL_VALUE_LEFT_MARGIN
        + value_width
}

fn context_menu_row_horizontal_chrome(icon_slot: ContextMenuIconSlot, child_count: usize) -> f32 {
    let (icon_width, total_child_count) = match icon_slot {
        ContextMenuIconSlot::Reserve => (CONTEXT_MENU_ICON_SLOT_SIZE, child_count + 1),
        ContextMenuIconSlot::Collapse => (0.0, child_count),
    };
    let child_gaps = total_child_count.saturating_sub(1) as f32 * CONTEXT_MENU_ROW_CHILD_GAP;

    CONTEXT_MENU_ROW_OUTER_HORIZONTAL_PADDING
        + CONTEXT_MENU_ROW_INNER_HORIZONTAL_PADDING
        + CONTEXT_MENU_BORDER_WIDTH * 2.0
        + child_gaps
        + icon_width
}

fn context_menu_text_width(text: &str, window: &Window) -> f32 {
    if text.is_empty() {
        return 0.0;
    }

    let style = window.text_style();
    let run = TextRun {
        len: text.len(),
        font: style.font(),
        color: style.color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };

    f32::from(
        window
            .text_system()
            .layout_line(text, px(CONTEXT_MENU_TEXT_SIZE), &[run], None)
            .width
            .ceil(),
    )
}

fn context_menu_separator() -> Div {
    div()
        .h(px(CONTEXT_MENU_SEPARATOR_HEIGHT))
        .mx(px(10.0))
        .flex()
        .items_center()
        .child(div().h(px(1.0)).w_full().bg(rgb(0xe5e5e5)))
}

fn context_menu_icon_slot(
    icon: Option<ContextMenuIcon>,
    native_file_icon: Option<&Arc<Image>>,
    native_path_icons: &HashMap<PathBuf, Arc<Image>>,
    url_icon_paths: &HashMap<String, PathBuf>,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(CONTEXT_MENU_ICON_SLOT_SIZE))
        .h(px(CONTEXT_MENU_ICON_SLOT_SIZE))
        .flex_shrink_0()
        .when_some(
            icon.and_then(|icon| {
                context_menu_icon_element(icon, native_file_icon, native_path_icons, url_icon_paths)
            }),
            |this, icon| this.child(icon),
        )
}

fn context_menu_icon_element(
    icon: ContextMenuIcon,
    native_file_icon: Option<&Arc<Image>>,
    native_path_icons: &HashMap<PathBuf, Arc<Image>>,
    url_icon_paths: &HashMap<String, PathBuf>,
) -> Option<AnyElement> {
    Some(match icon {
        ContextMenuIcon::Cut => gpui::img(CUT_ICON.clone())
            .w(px(CONTEXT_MENU_ICON_SIZE))
            .h(px(CONTEXT_MENU_ICON_SIZE))
            .into_any_element(),
        ContextMenuIcon::Copy => gpui::img(COPY_ICON.clone())
            .w(px(CONTEXT_MENU_ICON_SIZE))
            .h(px(CONTEXT_MENU_ICON_SIZE))
            .into_any_element(),
        ContextMenuIcon::Paste => gpui::img(PASTE_ICON.clone())
            .w(px(CONTEXT_MENU_ICON_SIZE))
            .h(px(CONTEXT_MENU_ICON_SIZE))
            .into_any_element(),
        ContextMenuIcon::Delete => gpui::img(DELETE_ICON.clone())
            .w(px(CONTEXT_MENU_ICON_SIZE))
            .h(px(CONTEXT_MENU_ICON_SIZE))
            .into_any_element(),
        ContextMenuIcon::Rename => gpui::img(RENAME_ICON.clone())
            .w(px(CONTEXT_MENU_ICON_SIZE))
            .h(px(CONTEXT_MENU_ICON_SIZE))
            .into_any_element(),
        ContextMenuIcon::New => gpui::img(NEW_ITEM_ICON.clone())
            .w(px(CONTEXT_MENU_ICON_SIZE))
            .h(px(CONTEXT_MENU_ICON_SIZE))
            .into_any_element(),
        ContextMenuIcon::File => file_icon_sized(CONTEXT_MENU_ICON_SIZE).into_any_element(),
        ContextMenuIcon::NativeFile => native_file_icon
            .map(|icon| image_icon(icon.clone(), CONTEXT_MENU_ICON_SIZE, CONTEXT_MENU_ICON_SIZE))
            .unwrap_or_else(|| file_icon_sized(CONTEXT_MENU_ICON_SIZE).into_any_element()),
        ContextMenuIcon::Folder => folder_icon_sized(CONTEXT_MENU_ICON_SIZE).into_any_element(),
        ContextMenuIcon::FolderKind(kind) => kind
            .map(|kind| directory_kind_icon_sized(kind, CONTEXT_MENU_ICON_SIZE))
            .unwrap_or_else(|| folder_icon_sized(CONTEXT_MENU_ICON_SIZE).into_any_element()),
        ContextMenuIcon::ImagePath(path) => {
            context_menu_image_path_icon(path, ContextMenuIconImageFallback::None)
        }
        ContextMenuIcon::ImagePathWithExecutableFallback(path) => {
            context_menu_image_path_icon(path, ContextMenuIconImageFallback::Executable)
        }
        ContextMenuIcon::ImageUrl(url) => url_icon_paths
            .get(&url)
            .map(|path| {
                context_menu_image_path_icon(path.clone(), ContextMenuIconImageFallback::None)
            })
            .unwrap_or_else(|| div().into_any_element()),
        ContextMenuIcon::ImageUrlWithExecutableFallback(url) => url_icon_paths
            .get(&url)
            .map(|path| {
                context_menu_image_path_icon(path.clone(), ContextMenuIconImageFallback::Executable)
            })
            .unwrap_or_else(|| executable_icon_sized(CONTEXT_MENU_ICON_SIZE).into_any_element()),
        ContextMenuIcon::NativePath(path) => native_path_icons
            .get(&path)
            .map(|icon| image_icon(icon.clone(), CONTEXT_MENU_ICON_SIZE, CONTEXT_MENU_ICON_SIZE))
            .unwrap_or_else(|| executable_icon_sized(CONTEXT_MENU_ICON_SIZE).into_any_element()),
        ContextMenuIcon::NativePathOptional(path) => native_path_icons
            .get(&path)
            .map(|icon| image_icon(icon.clone(), CONTEXT_MENU_ICON_SIZE, CONTEXT_MENU_ICON_SIZE))
            .unwrap_or_else(|| div().into_any_element()),
        ContextMenuIcon::NewTab => gpui::img(NEW_TAB_ICON.clone())
            .w(px(CONTEXT_MENU_ICON_SIZE))
            .h(px(CONTEXT_MENU_ICON_SIZE))
            .into_any_element(),
        ContextMenuIcon::OpenWith => gpui::img(OPEN_WITH_ICON.clone())
            .w(px(CONTEXT_MENU_ICON_SIZE))
            .h(px(CONTEXT_MENU_ICON_SIZE))
            .into_any_element(),
        ContextMenuIcon::Unpin => gpui::img(FAVORITE_PIN_REMOVE_ICON.clone())
            .w(px(CONTEXT_MENU_ICON_SIZE))
            .h(px(CONTEXT_MENU_ICON_SIZE))
            .into_any_element(),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContextMenuIconImageFallback {
    None,
    Executable,
}

fn context_menu_image_path_icon(
    path: PathBuf,
    fallback: ContextMenuIconImageFallback,
) -> AnyElement {
    gpui::img(path)
        .w(px(CONTEXT_MENU_ICON_SIZE))
        .h(px(CONTEXT_MENU_ICON_SIZE))
        .with_fallback(move || match fallback {
            ContextMenuIconImageFallback::None => div().into_any_element(),
            ContextMenuIconImageFallback::Executable => {
                executable_icon_sized(CONTEXT_MENU_ICON_SIZE).into_any_element()
            }
        })
        .into_any_element()
}

fn collect_context_menu_native_paths(items: &[ContextMenuItem], paths: &mut Vec<PathBuf>) {
    for item in items {
        match item {
            ContextMenuItem::Action {
                icon:
                    Some(
                        ContextMenuIcon::NativePath(path)
                        | ContextMenuIcon::NativePathOptional(path),
                    ),
                ..
            } => paths.push(path.clone()),
            ContextMenuItem::Submenu { icon, children, .. } => {
                if let Some(
                    ContextMenuIcon::NativePath(path) | ContextMenuIcon::NativePathOptional(path),
                ) = icon
                {
                    paths.push(path.clone());
                }
                collect_context_menu_native_paths(children, paths);
            }
            _ => {}
        }
    }
}

fn collect_context_menu_url_icons(items: &[ContextMenuItem], urls: &mut Vec<String>) {
    for item in items {
        match item {
            ContextMenuItem::Action {
                icon:
                    Some(
                        ContextMenuIcon::ImageUrl(url)
                        | ContextMenuIcon::ImageUrlWithExecutableFallback(url),
                    ),
                ..
            } => urls.push(url.clone()),
            ContextMenuItem::Submenu { icon, children, .. } => {
                if let Some(
                    ContextMenuIcon::ImageUrl(url)
                    | ContextMenuIcon::ImageUrlWithExecutableFallback(url),
                ) = icon
                {
                    urls.push(url.clone());
                }
                collect_context_menu_url_icons(children, urls);
            }
            _ => {}
        }
    }
}

fn context_menu_trailing_slot(glyph: Option<&'static str>) -> Div {
    div()
        .ml_auto()
        .flex()
        .items_center()
        .justify_center()
        .w(px(16.0))
        .h(px(16.0))
        .font(nav_icon_font())
        .text_size(px(12.0))
        .text_color(rgb(0x1f1f1f))
        .when_some(glyph, |this, glyph| this.child(glyph))
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
        .debug_selector(|| "directory-bar".to_owned())
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
    show_file_name_extensions: bool,
    show_full_path: bool,
    sidebar_width: f32,
    font: &gpui::Font,
    window: &Window,
) -> Div {
    let list_viewport_width = (f32::from(window.bounds().size.width)
        - normalized_sidebar_width_f32(sidebar_width))
    .max(0.0);
    let text_width = if show_full_path {
        recursive_result_text_width(list_viewport_width)
    } else {
        available_filename_text_width(list_viewport_width)
    };
    let filename = truncated_text(
        entry.display_name_with_extensions(show_file_name_extensions),
        text_width,
        0x000000,
        font,
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
        .child(entry_icon(entry, app_icon))
        .child(if show_full_path {
            let full_path = truncated_text_with_size(
                &entry.path.display().to_string(),
                text_width,
                RECURSIVE_SEARCH_PATH_TEXT_SIZE,
                RECURSIVE_SEARCH_PATH_TEXT_COLOR,
                font,
                window,
            );

            div()
                .flex()
                .flex_col()
                .justify_center()
                .flex_1()
                .min_w(px(0.0))
                .ml(px(NAME_ICON_TEXT_GAP))
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
                .ml(px(NAME_ICON_TEXT_GAP))
                .truncate()
                .text_size(px(NAME_TEXT_SIZE))
                .child(filename)
        })
}

fn rename_name_cell(
    entry: &FileEntry,
    app_icon: Option<Arc<Image>>,
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
        .ml(px(NAME_ICON_TEXT_GAP))
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
        .child(entry_icon(entry, app_icon))
        .child(input)
}

fn entry_icon(entry: &FileEntry, app_icon: Option<Arc<Image>>) -> AnyElement {
    if let Some(app_icon) = app_icon {
        return image_icon(app_icon, FILE_ICON_SLOT_WIDTH, FILE_ICON_SLOT_HEIGHT);
    }

    if entry.uses_directory_shortcut_icon() {
        directory_shortcut_icon().into_any_element()
    } else if entry.is_directory_like() {
        folder_icon().into_any_element()
    } else {
        file_icon_for_path(&entry.path).into_any_element()
    }
}

fn filename_text_width(name_column_width: f32) -> f32 {
    (name_column_width - NAME_CELL_LEFT_PADDING - FILE_ICON_SLOT_WIDTH - NAME_ICON_TEXT_GAP)
        .max(0.0)
}

fn available_filename_text_width(viewport_width: f32) -> f32 {
    filename_text_width(effective_name_column_width(viewport_width))
}

fn recursive_result_text_width(viewport_width: f32) -> f32 {
    available_filename_text_width(viewport_width)
}

fn truncated_text(
    text: &str,
    available_width: f32,
    text_color: u32,
    font: &gpui::Font,
    window: &Window,
) -> SharedString {
    truncated_text_with_size(
        text,
        available_width,
        NAME_TEXT_SIZE,
        text_color,
        font,
        window,
    )
}

fn truncated_text_with_size(
    text: &str,
    available_width: f32,
    text_size: f32,
    text_color: u32,
    name_font: &gpui::Font,
    window: &Window,
) -> SharedString {
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
        .line_wrapper(name_font.clone(), px(text_size))
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
                let font = entity.read(cx).font.clone();
                cx.new(|_| DragPreview::new(dragged, cursor_offset, font))
            },
        )
        .into_any_element()
}

fn text_cell(text: String, width: f32, right: bool, font: &gpui::Font, window: &Window) -> Div {
    let text = truncated_text(
        &text,
        text_cell_width(width),
        TEXT_CELL_TEXT_COLOR,
        font,
        window,
    );

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
    let count_friendly = count.separate_with_commas();
    format!("{count_friendly} {name}")
}

#[cfg(test)]
mod tests {

    use std::{collections::BTreeSet, fs, path::PathBuf};

    use gpui::{
        AppContext, ClickEvent, ClipboardItem, ExternalPaths, KeyboardClickEvent, Modifiers,
        MouseButton, MouseClickEvent, MouseDownEvent, MouseUpEvent,
    };

    use crate::explorer::context_menu::{
        ContextMenuIconSlot, ContextMenuItem, ContextMenuSource, ContextMenuState,
    };
    use crate::explorer::{
        DirectoryKind,
        clipboard::{FileClipboard, FileClipboardOperation, clipboard_item_for_files},
        constants::{
            COLUMN_DATE_WIDTH, COLUMN_NAME_MIN_WIDTH, COLUMN_SIZE_WIDTH, COLUMN_TYPE_WIDTH,
            EMPTY_FOLDER_MESSAGE, EMPTY_FOLDER_TEXT_SIZE, EMPTY_FOLDER_TOP_MARGIN,
            EXPLORER_COPY_GREEN, FILE_ICON_SLOT_WIDTH, MB_BYTES, NAV_BUTTON_ACTIVE_OPACITY,
            SCROLLBAR_GUTTER_WIDTH,
        },
        entry::FileEntry,
        selection::SelectionModifiers,
        sidebar::{SidebarItem, SidebarItemKind},
        test_support::TempDir,
        view::ExplorerView,
    };

    use super::{
        CONTEXT_MENU_MAX_WIDTH, CONTEXT_MENU_MIN_WIDTH, CUT_ITEM_OPACITY,
        DROP_INDICATOR_TARGET_MAX_WIDTH, NAME_CELL_LEFT_PADDING, NAME_ICON_TEXT_GAP,
        RecursiveSearchProgressSnapshot, UTILITY_TEXT_BUTTON_ICON_SIZE, UTILITY_TEXT_BUTTON_WIDTH,
        available_filename_text_width, clipboard_has_file_clipboard,
        context_menu_action_width_for_text_width, context_menu_detail_width_for_text_widths,
        context_menu_text_width, context_menu_width, context_menu_width_for_natural_width,
        drop_indicator_target_width, entry_row_hover_enabled, filename_text_width,
        folder_status_summary, is_normal_entry_click, open_current_folder_context_menu_from_event,
        recursive_result_text_width, search_working_detail, selection_modifiers_for_click,
        sidebar_context_menu_is_active, sidebar_context_menu_target, sidebar_item_is_dragging,
        sidebar_pin_path_from_value, sidebar_row_background_color, text_cell_width,
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
    fn context_menu_width_clamps_to_minimum() {
        assert_eq!(context_menu_width_for_natural_width(120.0), 170.0);
    }

    #[test]
    fn context_menu_width_preserves_natural_width_inside_bounds() {
        assert_eq!(context_menu_width_for_natural_width(220.0), 220.0);
    }

    #[test]
    fn context_menu_width_clamps_to_maximum() {
        assert_eq!(context_menu_width_for_natural_width(320.0), 280.0);
    }

    #[test]
    fn context_menu_action_and_submenu_width_account_for_all_row_chrome() {
        assert_eq!(context_menu_action_width_for_text_width(80.0), 148.0);
    }

    #[test]
    fn context_menu_collapsed_detail_width_accounts_for_label_and_value() {
        assert_eq!(
            context_menu_detail_width_for_text_widths(40.0, 100.0, ContextMenuIconSlot::Collapse),
            186.0
        );
    }

    #[test]
    fn context_menu_reserved_detail_width_accounts_for_icon_slot() {
        assert_eq!(
            context_menu_detail_width_for_text_widths(40.0, 100.0, ContextMenuIconSlot::Reserve),
            210.0
        );
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

    #[gpui::test]
    fn context_menu_opener_uses_mouse_up_event_position(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let event_position = gpui::point(gpui::px(123.0), gpui::px(45.0));
        let event = MouseUpEvent {
            button: MouseButton::Right,
            position: event_position,
            ..MouseUpEvent::default()
        };
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_focus_handle_for_test(path, focus_handle)
        });

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                open_current_folder_context_menu_from_event(view, &event, window, cx);
            });
        });

        cx.read_entity(&view, |view, _| {
            let menu = view.context_menu.as_ref().expect("context menu");
            assert_eq!(menu.origin, event_position);
        });
    }

    #[gpui::test]
    fn context_menu_detail_value_receives_its_natural_width(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let value = "January 2, 2026, 10:30 AM";
        let items = vec![ContextMenuItem::Detail {
            label: "Created",
            value: value.to_owned(),
            icon_slot: ContextMenuIconSlot::Collapse,
        }];
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_focus_handle_for_test(path, focus_handle)
        });
        let expected_value_width = cx.update(|window, _| context_menu_text_width(value, window));

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.context_menu = Some(ContextMenuState::new(
                    gpui::point(gpui::px(20.0), gpui::px(20.0)),
                    items,
                ));
                cx.notify();
            });
        });
        cx.run_until_parked();

        let menu_width = f32::from(
            cx.debug_bounds("context-menu")
                .expect("context menu bounds")
                .size
                .width,
        );
        let value_width = f32::from(
            cx.debug_bounds("context-menu-detail-value")
                .expect("detail value bounds")
                .size
                .width,
        );

        assert!(menu_width > CONTEXT_MENU_MIN_WIDTH);
        assert!(menu_width < CONTEXT_MENU_MAX_WIDTH);
        assert_eq!(value_width, expected_value_width);
    }

    #[gpui::test]
    fn context_menu_levels_calculate_widths_independently(cx: &mut gpui::TestAppContext) {
        let child_items = vec![ContextMenuItem::Action {
            id: "context-menu-child".to_owned(),
            icon: None,
            label: "A significantly wider submenu action".to_owned(),
            command: crate::explorer::context_menu::ContextMenuCommand::Paste,
            enabled: true,
        }];
        let root_items = vec![ContextMenuItem::Submenu {
            id: "context-menu-root".to_owned(),
            icon: None,
            label: "New".to_owned(),
            children: child_items.clone(),
        }];
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_focus_handle_for_test(path, focus_handle)
        });

        let (root_width, child_width) = cx.update(|window, _| {
            (
                context_menu_width(&root_items, window),
                context_menu_width(&child_items, window),
            )
        });

        assert_eq!(root_width, CONTEXT_MENU_MIN_WIDTH);
        assert!(child_width > root_width);
        assert!(child_width <= CONTEXT_MENU_MAX_WIDTH);
    }

    #[test]
    fn sidebar_pin_drop_accepts_exactly_one_directory() {
        let temp = TempDir::new();
        let directory = temp.path().join("directory");
        let file = temp.path().join("file.txt");
        fs::create_dir(&directory).unwrap();
        fs::write(&file, "file").unwrap();

        assert_eq!(
            sidebar_pin_path_from_value(&ExternalPaths::new(vec![directory.clone()])),
            Some(directory.clone())
        );
        assert_eq!(
            sidebar_pin_path_from_value(&ExternalPaths::new(vec![file])),
            None
        );
        assert_eq!(
            sidebar_pin_path_from_value(&ExternalPaths::new(vec![
                directory.clone(),
                directory.clone(),
            ])),
            None
        );
        assert_eq!(
            sidebar_pin_path_from_value(&ExternalPaths::new(Vec::new())),
            None
        );
    }

    #[test]
    fn only_configured_sidebar_item_matching_active_drag_is_dimmed() {
        assert!(sidebar_item_is_dragging(Some(2), Some(2)));
        assert!(!sidebar_item_is_dragging(Some(2), Some(1)));
        assert!(!sidebar_item_is_dragging(Some(2), None));
        assert!(!sidebar_item_is_dragging(None, None));
        assert!(!sidebar_item_is_dragging(None, Some(2)));
    }

    #[test]
    fn sidebar_context_menu_target_supports_all_sidebar_item_kinds() {
        let path = PathBuf::from("/custom");
        let custom = SidebarItem {
            label: "Custom".to_owned(),
            path: path.clone(),
            kind: SidebarItemKind::CustomDirectory,
            configured_index: Some(3),
        };
        let builtin_configured = SidebarItem {
            label: "Downloads".to_owned(),
            path: path.join("downloads"),
            kind: SidebarItemKind::Directory(DirectoryKind::Downloads),
            configured_index: Some(1),
        };
        let custom_unconfigured = SidebarItem {
            label: "Custom".to_owned(),
            path: PathBuf::from("/other"),
            kind: SidebarItemKind::CustomDirectory,
            configured_index: None,
        };
        let drive = SidebarItem {
            label: "Drive".to_owned(),
            path: PathBuf::from("/"),
            kind: SidebarItemKind::Drive,
            configured_index: None,
        };
        let windows_drive = SidebarItem {
            label: "Windows".to_owned(),
            path: PathBuf::from("C:\\"),
            kind: SidebarItemKind::DriveWindows,
            configured_index: None,
        };

        assert_eq!(
            sidebar_context_menu_target(&custom),
            (path.clone(), Some(3), None)
        );
        assert_eq!(
            sidebar_context_menu_target(&builtin_configured),
            (
                path.join("downloads"),
                Some(1),
                Some(DirectoryKind::Downloads)
            )
        );
        assert_eq!(
            sidebar_context_menu_target(&custom_unconfigured),
            (PathBuf::from("/other"), None, None)
        );
        assert_eq!(
            sidebar_context_menu_target(&drive),
            (PathBuf::from("/"), None, Some(DirectoryKind::Drive))
        );
        assert_eq!(
            sidebar_context_menu_target(&windows_drive),
            (
                PathBuf::from("C:\\"),
                None,
                Some(DirectoryKind::DriveWindows)
            )
        );
    }

    #[test]
    fn sidebar_context_menu_active_matches_row_source() {
        let menu = ContextMenuState::new_with_source(
            gpui::point(gpui::px(0.0), gpui::px(0.0)),
            Vec::new(),
            ContextMenuSource::SidebarItem { row_id: 2_000 },
        );

        assert!(sidebar_context_menu_is_active(Some(&menu), 2_000));
        assert!(!sidebar_context_menu_is_active(Some(&menu), 1_000));
        assert!(!sidebar_context_menu_is_active(None, 2_000));
    }

    #[test]
    fn sidebar_context_menu_background_matches_windows_hover_precedence() {
        assert_eq!(sidebar_row_background_color(false, false), 0xffffff);
        assert_eq!(sidebar_row_background_color(false, true), 0xe5f3ff);
        assert_eq!(sidebar_row_background_color(true, false), 0xcce8ff);
        assert_eq!(sidebar_row_background_color(true, true), 0xcce8ff);
    }

    #[gpui::test]
    fn sidebar_render_uses_configured_width(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            let mut view = ExplorerView::new_with_focus_handle_for_test(path, focus_handle);
            view.sidebar_width = 312.0;
            view
        });

        cx.run_until_parked();

        let sidebar_width = f32::from(
            cx.debug_bounds("explorer-sidebar")
                .expect("sidebar bounds")
                .size
                .width,
        );
        assert_eq!(sidebar_width, 312.0);
        assert!(cx.debug_bounds("explorer-sidebar-resizer").is_some());
    }

    #[gpui::test]
    fn right_clicking_sidebar_resizer_restores_and_persists_default_width(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.set_global(crate::settings::SettingsState::for_test(
            crate::settings::ExplorerSettings {
                sidebar: crate::settings::SidebarSettings {
                    width: 312,
                    ..crate::settings::SidebarSettings::default()
                },
                ..crate::settings::ExplorerSettings::default()
            },
        ));
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            let mut view = ExplorerView::new_with_focus_handle_for_test(path, focus_handle);
            view.sidebar_width = 312.0;
            view.begin_sidebar_resize(312.0);
            view
        });

        cx.run_until_parked();
        let position = cx
            .debug_bounds("explorer-sidebar-resizer")
            .expect("sidebar resizer bounds")
            .center();
        cx.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
        cx.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());

        let sidebar_width = f32::from(
            cx.debug_bounds("explorer-sidebar")
                .expect("sidebar bounds")
                .size
                .width,
        );
        assert_eq!(sidebar_width, crate::settings::SIDEBAR_DEFAULT_WIDTH as f32);
        cx.read_entity(&view, |view, _| {
            assert_eq!(
                view.sidebar_width,
                crate::settings::SIDEBAR_DEFAULT_WIDTH as f32
            );
            assert_eq!(view.sidebar_resize_drag, None);
        });
        assert_eq!(
            cx.read(|cx| cx
                .global::<crate::settings::SettingsState>()
                .value
                .sidebar
                .width),
            crate::settings::SIDEBAR_DEFAULT_WIDTH
        );
    }

    #[test]
    fn entry_hover_is_only_enabled_for_unselected_rows_without_a_context_menu() {
        assert!(entry_row_hover_enabled(false, false));
        assert!(!entry_row_hover_enabled(true, false));
        assert!(!entry_row_hover_enabled(false, true));
        assert!(!entry_row_hover_enabled(true, true));
    }

    #[test]
    fn utility_text_button_icon_geometry_fits_button() {
        assert_eq!(UTILITY_TEXT_BUTTON_ICON_SIZE, 16.0);
        assert!(UTILITY_TEXT_BUTTON_WIDTH >= 92.0);
    }

    #[test]
    fn explorer_copy_green_matches_dialog_copy_green() {
        assert_eq!(EXPLORER_COPY_GREEN, 0x36a646);
    }

    #[test]
    fn empty_folder_message_uses_compact_text() {
        assert_eq!(EMPTY_FOLDER_TEXT_SIZE, 12.0);
        assert_eq!(EMPTY_FOLDER_TOP_MARGIN, 20.0);
        assert_eq!(EMPTY_FOLDER_MESSAGE, "This folder is empty.");
    }

    #[test]
    fn recursive_search_working_detail_formats_live_count() {
        assert_eq!(
            search_working_detail(RecursiveSearchProgressSnapshot::Searching(None)),
            "Searching..."
        );
        assert_eq!(
            search_working_detail(RecursiveSearchProgressSnapshot::Scanning(1_234)),
            "Scanning 1,234..."
        );
        assert_eq!(
            search_working_detail(RecursiveSearchProgressSnapshot::Searching(Some(1_234))),
            "Searching 1,234..."
        );
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
            available_filename_text_width(viewport_width),
            name_column_width - NAME_CELL_LEFT_PADDING - FILE_ICON_SLOT_WIDTH - NAME_ICON_TEXT_GAP
        );
    }

    #[test]
    fn name_text_width_respects_name_column_minimum() {
        assert_eq!(
            available_filename_text_width(100.0),
            COLUMN_NAME_MIN_WIDTH
                - NAME_CELL_LEFT_PADDING
                - FILE_ICON_SLOT_WIDTH
                - NAME_ICON_TEXT_GAP
        );
    }

    #[test]
    fn recursive_result_text_width_matches_name_text_width() {
        for viewport_width in [100.0, 900.0] {
            let recursive_width = recursive_result_text_width(viewport_width);

            assert_eq!(
                recursive_width,
                available_filename_text_width(viewport_width)
            );
            assert!(recursive_width > 0.0);
        }
    }

    #[test]
    fn name_text_width_clamps_when_chrome_consumes_column() {
        assert_eq!(filename_text_width(10.0), 0.0);
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
