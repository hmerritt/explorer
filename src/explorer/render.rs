use std::{
    any::Any,
    collections::{BTreeSet, HashMap},
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, ClickEvent, ClipboardItem, Context,
    CursorStyle, Div, DragMoveEvent, Entity, ExternalPaths, FocusHandle, Focusable, Image,
    IntoElement, ListHorizontalSizingBehavior, ModifiersChangedEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, NavigationDirection, Pixels, Point, Render, ScrollWheelEvent,
    SharedString, TextAlign, TextRun, Window, canvas, div, list, prelude::*, px, rgb,
    transparent_black, uniform_list,
};

#[cfg(test)]
use crate::explorer::address_bar::format_address_path;
use crate::explorer::{
    DirectoryKind,
    address_bar::{
        ADDRESS_SUGGESTION_ROW_HEIGHT, ADDRESS_SUGGESTIONS_VERTICAL_PADDING, address_text_element,
    },
    app_icons::NativeIconSize,
    breadcrumb::{
        BreadcrumbSegment, VisibleBreadcrumb, directory_bar_available_width,
        visible_breadcrumb_for_path,
    },
    clipboard::clipboard_item_can_paste,
    codebase_summary::{CodebaseSummary, language_segment_widths},
    columns::file_column_label,
    constants::{
        COLUMN_NAME_MIN_WIDTH, DIRECTORY_BAR_COPY_BUTTON_GAP, DIRECTORY_BAR_COPY_BUTTON_SIZE,
        DIRECTORY_BAR_ELLIPSIS, DIRECTORY_BAR_HEIGHT, DIRECTORY_BAR_HORIZONTAL_PADDING,
        DIRECTORY_BAR_RADIUS, DIRECTORY_BAR_SEGMENT_HORIZONTAL_PADDING, DIRECTORY_BAR_SEPARATOR,
        DIRECTORY_BAR_TEXT_SIZE, EMPTY_FOLDER_MESSAGE, EMPTY_FOLDER_TEXT_SIZE,
        EMPTY_FOLDER_TOP_MARGIN, FILE_ICON_SLOT_HEIGHT, FILE_ICON_SLOT_WIDTH,
        FOLDER_LOADING_MESSAGE, HEADER_HEIGHT, LARGE_ICON_SIZE, LARGE_ICON_TEXT_LINE_HEIGHT,
        LARGE_ICON_TEXT_ROWS, LARGE_ICON_TEXT_SIZE, LARGE_ICON_TEXT_TOP_GAP,
        LARGE_ICON_TILE_HEIGHT, LARGE_ICON_TILE_WIDTH, NAV_BUTTON_ACTIVE_OPACITY,
        NAV_BUTTON_HOVER_BG, NAV_BUTTON_SIZE, NAV_ICON_DISABLED_COLOR, NAV_ICON_ENABLED_COLOR,
        NAV_ICON_TEXT_SIZE, NAVBAR_HEIGHT, NAVBAR_HORIZONTAL_PADDING, NAVBAR_ITEM_GAP,
        OPEN_ERROR_HORIZONTAL_PADDING, OPEN_ERROR_VERTICAL_PADDING, RECURSIVE_SEARCH_ROW_HEIGHT,
        ROW_HEIGHT, SCROLLBAR_ARROW_HEIGHT, SCROLLBAR_GUTTER_WIDTH, SCROLLBAR_THUMB_ACTIVE_BG,
        SCROLLBAR_THUMB_BG, SCROLLBAR_THUMB_HOVER_BG, SCROLLBAR_THUMB_HOVER_WIDTH,
        SCROLLBAR_THUMB_WIDTH, SCROLLBAR_TRACK_BG, SEARCH_BAR_MAX_WIDTH, SEARCH_BAR_MIN_WIDTH,
        SEARCH_NO_MATCHES_MESSAGE, SEARCH_WORKING_MESSAGE, SIDEBAR_HORIZONTAL_PADDING,
        SIDEBAR_ICON_TEXT_GAP, SIDEBAR_ROW_HEIGHT, SIDEBAR_TEXT_SIZE, STATUS_BAR_HEIGHT,
        STATUS_BAR_HORIZONTAL_PADDING, STATUS_BAR_SEPARATOR_COLOR, STATUS_BAR_TEXT_COLOR,
        STATUS_BAR_TEXT_SIZE, UTILITY_BAR_HEIGHT, UTILITY_BAR_HORIZONTAL_PADDING,
        UTILITY_BAR_ITEM_GAP, UTILITY_BUTTON_HEIGHT, UTILITY_ICON_BUTTON_SIZE,
        UTILITY_MENU_ROW_HEIGHT, UTILITY_MENU_WIDTH,
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
    filesystem::{drive_root_is_ejectable, wsl_distro_kind_for_path},
    formatting::{format_size, format_timestamp},
    git_status::{GitDivergence, GitRepositoryStatus},
    icons::{
        COPY_AS_PATH_ICON, COPY_ICON, CUT_ICON, DELETE_ICON, DETAILS_ICON, EJECT_ICON,
        EXTRACT_ICON, FAVORITE_PIN_REMOVE_ICON, GIT_BRANCH_ICON, GIT_ICON, HAMBURGER_ICON,
        LARGE_ICONS_ICON, NEW_ITEM_ICON, NEW_TAB_ICON, NavIcon, OPEN_WITH_ICON, PASTE_ICON,
        PROPERTIES_ICON, RENAME_ICON, SORT_CHEVRON_DOWN_ICON, SORT_CHEVRON_UP_ICON,
        directory_kind_icon, directory_kind_icon_sized, directory_shortcut_icon,
        directory_shortcut_icon_sized, drive_disc_icon, drive_disc_icon_sized, drive_icon,
        drive_windows_icon, drive_wsl_icon_for_path, drive_wsl_icon_sized_for_path,
        executable_icon_sized, file_icon, file_icon_for_path, file_icon_sized, folder_icon,
        folder_icon_sized, image_icon, large_file_icon_for_path_sized, nav_icon_font,
    },
    large_icons::{
        LargeIconLayout, LargeIconLayoutCacheKey, large_icon_filename_text_width,
        large_icon_max_tile_height,
    },
    mouse_selection::{local_point, selection_box_bounds, viewport_size},
    navigation::{DirectoryOpenMode, HistoryMode},
    recursive_search::RecursiveSearchProgressSnapshot,
    rename::{ActiveTextInput, rename_text_element},
    scrollbar::{
        ScrollbarArrow, scrollbar_arrow_button, scrollbar_corner, scrollbar_header_spacer,
    },
    search::search_text_element,
    selection::SelectionModifiers,
    sidebar::{SidebarItem, SidebarItemKind},
    tooltip::explorer_tooltip,
    view::{
        ExplorerContentBranch, ExplorerView, ExplorerViewEvent, FileColumnResizeResult,
        UtilityMenu, normalized_sidebar_width_f32,
    },
};
use crate::loaders::{LinearProgressStyle, linear_indeterminate};
use crate::settings::{
    FileColumnKind, FileSortColumn, FileSortSettings, FileViewMode, SettingsState, SortDirection,
};
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
const FILE_COLUMN_RESIZE_HIT_WIDTH: f32 = 6.0;
const FILE_COLUMN_HEADER_HOVER_BG: u32 = 0xd9ebf9;
const FILE_SORT_CHEVRON_ICON_SIZE: f32 = 11.0;
const FILE_SORT_CHEVRON_RIGHT_OFFSET: f32 = FILE_COLUMN_RESIZE_HIT_WIDTH + 2.0;
const FILE_SORT_CHEVRON_RESERVED_WIDTH: f32 =
    FILE_SORT_CHEVRON_RIGHT_OFFSET + FILE_SORT_CHEVRON_ICON_SIZE + 2.0;
const CODEBASE_MAKEUP_BAR_WIDTH: f32 = 120.0;
const CODEBASE_MAKEUP_BAR_HEIGHT: f32 = 8.0;
const CODEBASE_MAKEUP_BAR_RADIUS: f32 = 6.0;
const CODEBASE_MAKEUP_SEPARATOR_WIDTH: f32 = 2.0;
const CODEBASE_MAKEUP_SEPARATOR_COLOR: u32 = 0x3d444d;
const STATUS_BAR_GIT_ICON_SIZE: f32 = 14.0;
const STATUS_BAR_GIT_ITEM_GAP: f32 = 4.0;
const DIRECTORY_COPY_ADDRESS_FADE_MS: u64 = 50;

#[derive(Clone, Copy, Debug, PartialEq)]
struct CodebaseMakeupSegment {
    left: f32,
    width: f32,
    color: u32,
    round_left: bool,
    round_right: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileColumnHeaderDrag {
    kind: FileColumnKind,
}

struct FileColumnHeaderDragPreview {
    label: SharedString,
    width: f32,
}

impl Render for FileColumnHeaderDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_start()
            .h(px(HEADER_HEIGHT))
            .w(px(self.width))
            .pl(px(8.0))
            .pt(px(8.0))
            .border_1()
            .border_color(rgb(0x7aa7d9))
            .bg(rgb(0xf7fbff))
            .text_size(px(12.0))
            .text_color(rgb(0x1f4e79))
            .child(self.label.clone())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SidebarItemDrag {
    configured_index: usize,
    label: SharedString,
    path: PathBuf,
    kind: SidebarItemKind,
}

struct SidebarItemDragPreview {
    label: SharedString,
    path: PathBuf,
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
            .child(sidebar_item_kind_icon_for_path(self.kind, &self.path))
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
const UTILITY_SIDEBAR_TOGGLE_MENU_OFFSET: f32 =
    UTILITY_ICON_BUTTON_SIZE + UTILITY_SEPARATOR_OUTER_WIDTH + (UTILITY_BAR_ITEM_GAP * 2.0);
const UTILITY_ICON_CHEVRON_DOWN: &str = "\u{E70D}";
const UTILITY_ICON_CHECK: &str = "\u{E73E}";
const UTILITY_TEXT_BUTTON_ICON_SIZE: f32 = 16.0;
const SIDEBAR_AUTO_HIDE_MAX_WINDOW_FRACTION: f32 = 0.40;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryClickTarget {
    Row,
    Name,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryContextMenuTarget {
    WholeEntry,
    NameCell,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CurrentFolderClickTarget {
    Background,
    EmptyFolder,
}

fn sidebar_auto_hide_is_active(sidebar_width: f32, window_width: f32) -> bool {
    sidebar_width > window_width * SIDEBAR_AUTO_HIDE_MAX_WINDOW_FRACTION
}

fn effective_sidebar_is_visible(
    sidebar_width: f32,
    window_width: f32,
    sidebar_auto_hide_expanded: bool,
) -> bool {
    !sidebar_auto_hide_is_active(sidebar_width, window_width) || sidebar_auto_hide_expanded
}

fn effective_sidebar_layout_width(
    sidebar_width: f32,
    window_width: f32,
    sidebar_auto_hide_expanded: bool,
) -> f32 {
    if effective_sidebar_is_visible(sidebar_width, window_width, sidebar_auto_hide_expanded) {
        sidebar_width
    } else {
        0.0
    }
}

impl ExplorerView {
    pub(super) fn entry_row_height(&self) -> f32 {
        if self.view_mode == FileViewMode::LargeIcons {
            return LARGE_ICON_TILE_HEIGHT;
        }

        if self.recursive_search_results_active() {
            RECURSIVE_SEARCH_ROW_HEIGHT
        } else {
            ROW_HEIGHT
        }
    }

    fn list_viewport_width(&self, window: &Window) -> f32 {
        let window_width = f32::from(window.bounds().size.width);
        let sidebar_width = effective_sidebar_layout_width(
            normalized_sidebar_width_f32(self.sidebar_width),
            window_width,
            self.sidebar_auto_hide_expanded,
        );
        (window_width - sidebar_width).max(0.0)
    }

    fn name_column_width(&self, window: &Window) -> f32 {
        self.effective_name_column_width(self.list_viewport_width(window))
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
                "Back",
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
                "Forward",
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
                "Up",
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
                "Refresh",
                true,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.close_context_menu();
                    this.refresh_with_entry_metadata_and_search_resolution(cx);
                    cx.notify();
                }),
            ))
            .child(if self.address_bar_is_editing() {
                editable_directory_bar(self.active_address_focus_handle(), cx)
            } else {
                directory_bar(
                    breadcrumb,
                    self.directory_bar_hovered,
                    self.directory_bar_hover_generation,
                    cx,
                )
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
                "Search subfolders",
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

    fn render_utility_bar(
        &self,
        sidebar_auto_hide_active: bool,
        sidebar_visible: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let has_selection = !self.selection.selected_indices.is_empty();
        let can_rename = self.can_start_selected_rename();
        let can_extract = self.selected_archive_paths().is_some();
        let clipboard = cx.read_from_clipboard();
        let can_paste = clipboard_item_can_paste(clipboard.as_ref());

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
            .when(sidebar_auto_hide_active, |this| {
                this.child(utility_icon_button(
                    "utility-sidebar-toggle",
                    HAMBURGER_ICON.clone(),
                    if sidebar_visible {
                        "Hide navigation pane"
                    } else {
                        "Show navigation pane"
                    },
                    true,
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.close_context_menu();
                        this.cancel_pending_click_rename();
                        this.open_utility_menu = None;
                        this.sidebar_auto_hide_expanded = !this.sidebar_auto_hide_expanded;
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ))
                .child(utility_separator())
            })
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
                "Cut",
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
                "Copy",
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
                "Paste",
                can_paste,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.close_context_menu();
                    this.open_utility_menu = None;
                    if this.commit_active_rename_before_interaction(window, cx) {
                        this.paste_clipboard(window, cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            ))
            .child(utility_icon_button(
                "utility-rename",
                RENAME_ICON.clone(),
                "Rename",
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
                "Delete",
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

    fn render_utility_menu_overlay(
        &self,
        sidebar_auto_hide_active: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.open_utility_menu?;
        let left = utility_menu_left(menu, sidebar_auto_hide_active);

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
                .child(utility_menu_row(
                    "utility-large-icons",
                    Some(utility_menu_image_icon(LARGE_ICONS_ICON.clone())),
                    "Large Icons",
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.open_utility_menu = None;
                        if this.commit_active_rename_before_interaction(window, cx) {
                            crate::settings::set_view_mode(FileViewMode::LargeIcons, cx);
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ))
                .child(utility_menu_row(
                    "utility-details",
                    Some(utility_menu_image_icon(DETAILS_ICON.clone())),
                    "Details",
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.open_utility_menu = None;
                        if this.commit_active_rename_before_interaction(window, cx) {
                            crate::settings::set_view_mode(FileViewMode::Details, cx);
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ))
                .child(utility_menu_separator())
                .child(utility_checkbox_row(
                    "utility-hidden-files",
                    self.show_hidden_files,
                    "Hidden items",
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
                    "utility-folder-sizes",
                    self.show_folder_size,
                    "Folder sizes",
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.open_utility_menu = None;
                        if this.commit_active_rename_before_interaction(window, cx) {
                            crate::settings::set_show_folder_sizes(!this.show_folder_size, cx);
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ))
                .child(utility_checkbox_row(
                    "utility-file-name-extensions",
                    self.show_file_name_extensions,
                    "File name extensions",
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
            .and_then(|entry| self.native_icon_for_entry(entry, NativeIconSize::Details, cx));
        let native_path_icons = {
            let mut paths = Vec::new();
            collect_context_menu_native_paths(&self.context_menu.as_ref()?.items, &mut paths);
            paths
                .into_iter()
                .filter_map(|path| {
                    self.native_icon_for_path(&path, NativeIconSize::Details, cx)
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

    fn render_header(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let scroll_left = if self.content_branch() == ExplorerContentBranch::List {
            self.visible_horizontal_scroll_offset()
        } else {
            0.0
        };
        let active_sort = self.header_file_sort();
        let mut header_row = div()
            .relative()
            .left(px(-scroll_left))
            .flex()
            .flex_row()
            .h_full()
            .w_full()
            .min_w(px(self.minimum_file_columns_width()))
            .child(name_header_cell(
                self.name_column_width(window),
                self.name_column_is_manual_width(),
                cx.entity(),
                active_sort,
            ));

        for kind in self.file_columns.order.iter().copied() {
            header_row =
                header_row.child(self.render_file_column_header_cell(kind, active_sort, cx));
        }

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
                div()
                    .relative()
                    .flex_1()
                    .h_full()
                    .overflow_hidden()
                    .child(header_row),
            )
            .child(scrollbar_header_spacer())
    }

    fn render_file_column_header_cell(
        &self,
        kind: FileColumnKind,
        active_sort: Option<FileSortSettings>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entity = cx.entity();
        let width = self.file_column_width(kind);
        let label = file_column_label(kind);
        let sort_column = file_column_sort_column(kind);
        let mut cell = header_cell(label, width, sort_column, active_sort)
            .id(file_column_header_element_id(kind))
            .debug_selector(move || file_column_header_element_id(kind).to_owned());
        if let Some(sort_column) = sort_column {
            cell = add_header_sort_click(cell, sort_column, entity.clone());
        }

        cell.on_drag(FileColumnHeaderDrag { kind }, {
            let label = SharedString::from(label);
            move |_, _, _, cx| {
                cx.new(|_| FileColumnHeaderDragPreview {
                    label: label.clone(),
                    width,
                })
            }
        })
        .on_drag_move::<FileColumnHeaderDrag>({
            let entity = entity.clone();
            move |event: &DragMoveEvent<FileColumnHeaderDrag>, _, cx| {
                let left = f32::from(event.bounds.origin.x);
                let width = f32::from(event.bounds.size.width);
                let cursor_x = f32::from(event.event.position.x);
                let before = cursor_x < left + (width / 2.0);
                let dragged = event.drag(cx).kind;

                let _ = entity.update(cx, |this, cx| {
                    if this.reorder_file_column(dragged, kind, before) {
                        crate::settings::reorder_file_column(dragged, kind, before, cx);
                        cx.notify();
                    }
                });
            }
        })
        .child(file_column_resize_handle(kind, entity))
        .into_any_element()
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let sections = &self.sidebar_sections;
        let mut children = Vec::new();
        let has_user_directories = !sections.user_directories.is_empty();
        let sidebar_width = normalized_sidebar_width_f32(self.sidebar_width);

        for (index, item) in sections.user_directories.iter().cloned().enumerate() {
            children.push(
                self.render_sidebar_insertion_zone(
                    item.configured_index
                        .unwrap_or(self.sidebar_settings.items.len()),
                    index,
                    SIDEBAR_ITEM_GAP,
                    cx,
                ),
            );
            children.push(self.render_sidebar_row(index, item, cx));
        }
        let final_insertion_index = sections
            .user_directories
            .last()
            .and_then(|item| item.configured_index)
            .map(|index| index + 1)
            .unwrap_or(self.sidebar_settings.items.len());
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

        if !children.is_empty() && !sections.wsl_drives.is_empty() {
            children.push(sidebar_separator().into_any_element());
        }

        for (index, item) in sections.wsl_drives.iter().cloned().enumerate() {
            if index > 0 {
                children.push(sidebar_item_gap().into_any_element());
            }
            children.push(self.render_sidebar_row(index + 3_000, item, cx));
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
            .debug_selector(move || format!("explorer-sidebar-row-{id}"))
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
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                this.close_context_menu();
                if event.modifiers().control {
                    cx.emit(ExplorerViewEvent::OpenDirectoryInNewTab(click_path.clone()));
                } else {
                    this.navigate_to_sidebar_path_with_watcher(click_path.clone(), cx);
                }
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
            .child(sidebar_item_icon(&icon_item))
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

        let (context_path, context_configured_index, open_icon_kind, can_eject) =
            context_menu_target;
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
                    can_eject,
                    window,
                    cx,
                );
            }),
        );

        if let Some(configured_index) = configured_index {
            let drag_label = SharedString::from(item.label.clone());
            let drag_path = item.path.clone();
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
                        path: drag_path,
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
                                path: drag.path.clone(),
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
        let app_icon = self.native_icon_for_entry(&entry, NativeIconSize::Details, cx);
        let is_selected = self.entry_is_selected(ix);
        let context_menu_active = self.context_menu.is_some();
        let is_cut = self.entry_is_cut(&entry.path);
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
            .min_w(px(self.minimum_file_columns_width()))
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
            .when(is_cut, |this| this.opacity(CUT_ITEM_OPACITY));
        row = add_entry_primary_click(row, entry.clone(), EntryClickTarget::Row, cx);
        row = add_entry_context_menu(row, entry.clone(), EntryContextMenuTarget::WholeEntry, cx);
        row = add_entry_middle_click(row, entry.clone(), cx);
        row = add_drop_handlers(
            row,
            destination,
            entry.is_directory_like(),
            entity.clone(),
            cx,
        );
        row = add_selected_entry_drag(row, selected_drag_payload, entity.clone());

        let non_name_cells = self
            .file_columns
            .order
            .iter()
            .copied()
            .map(|kind| {
                let cell = file_column_cell(
                    kind,
                    &entry,
                    self.file_column_width(kind),
                    &self.date_format,
                    &self.font,
                    window,
                );
                if let Some(drag_payload) = individual_drag_payload.clone() {
                    add_item_drag(
                        cell,
                        (file_column_entry_drag_element_id(kind), ix),
                        drag_payload,
                        entity.clone(),
                    )
                } else {
                    cell.into_any_element()
                }
            })
            .collect::<Vec<_>>();

        let name_cell = if self.rename_is_active_for_path(&entry.path) {
            rename_name_cell(
                &entry,
                app_icon,
                self.active_rename_focus_handle(),
                self.name_column_width(window),
                self.name_column_is_manual_width(),
                cx,
            )
            .into_any_element()
        } else {
            let name_cell = name_cell(
                &entry,
                app_icon,
                self.show_file_name_extensions,
                self.recursive_search_results_active(),
                self.name_column_width(window),
                self.name_column_is_manual_width(),
                &self.font,
                window,
            )
            .id(("explorer-entry-name", ix));
            let name_cell =
                add_entry_primary_click(name_cell, entry.clone(), EntryClickTarget::Name, cx);
            let name_cell = add_entry_context_menu(
                name_cell,
                entry.clone(),
                EntryContextMenuTarget::NameCell,
                cx,
            );
            add_entry_middle_click(name_cell, entry.clone(), cx).into_any_element()
        };

        let mut row = row.child(name_cell);
        for cell in non_name_cells {
            row = row.child(cell);
        }
        row.into_any_element()
    }

    fn render_large_icon_row(
        &mut self,
        row_ix: usize,
        layout: LargeIconLayout,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(row_layout) = layout.row_bounds(row_ix) else {
            return div().into_any_element();
        };
        let start = row_ix * layout.columns;
        let end = (start + layout.columns).min(self.entries.len());
        let mut row = div()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(layout.column_gap))
            .h(px(row_layout.height))
            .w_full();

        for ix in start..end {
            let tile_height = layout
                .tile_height(ix)
                .unwrap_or_else(large_icon_max_tile_height);
            row = row.child(self.render_large_icon_tile(ix, tile_height, window, cx));
        }

        row.into_any_element()
    }

    fn render_large_icon_tile(
        &mut self,
        ix: usize,
        tile_height: f32,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entry = self.entries[ix].clone();
        let image_thumbnail = self.image_thumbnail_for_entry(&entry, cx);
        let app_icon = self.native_icon_for_entry(&entry, NativeIconSize::LargeIcons, cx);
        let is_selected = self.entry_is_selected(ix);
        let context_menu_active = self.context_menu.is_some();
        let is_cut = self.entry_is_cut(&entry.path);
        let name_click_entry = entry.clone();
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

        let name = if self.rename_is_active_for_path(&entry.path) {
            large_icon_rename_input(self.active_rename_focus_handle(), cx).into_any_element()
        } else {
            large_icon_filename(
                ix,
                &entry,
                self.show_file_name_extensions,
                &self.font,
                name_click_entry,
                cx,
            )
        };

        let mut tile = div()
            .id(("explorer-large-icon-entry", ix))
            .debug_selector(move || format!("explorer-large-icon-entry-{ix}"))
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .w(px(LARGE_ICON_TILE_WIDTH))
            .h(px(tile_height))
            .max_h(px(large_icon_max_tile_height()))
            .flex_shrink_0()
            .border_1()
            .border_color(if is_selected {
                rgb(0x000000)
            } else {
                rgb(0xffffff)
            })
            .bg(if is_selected {
                rgb(0xcce8ff)
            } else {
                rgb(0xffffff)
            })
            .when(
                entry_row_hover_enabled(is_selected, context_menu_active),
                |this| this.hover(|style| style.bg(rgb(0xe5f3ff))),
            )
            .cursor_default()
            .when(is_cut, |this| this.opacity(CUT_ITEM_OPACITY));
        tile = add_entry_primary_click(tile, entry.clone(), EntryClickTarget::Row, cx);
        tile = add_entry_context_menu(tile, entry.clone(), EntryContextMenuTarget::WholeEntry, cx);
        tile = add_entry_middle_click(tile, entry.clone(), cx);
        tile = add_drop_handlers(
            tile,
            destination,
            entry.is_directory_like(),
            entity.clone(),
            cx,
        );
        tile = if selected_drag_payload.is_some() {
            add_selected_entry_drag(tile, selected_drag_payload, entity.clone())
        } else {
            add_individual_entry_drag(tile, individual_drag_payload, entity.clone())
        };

        tile.child(large_entry_icon(&entry, image_thumbnail, app_icon))
            .child(div().mt(px(LARGE_ICON_TEXT_TOP_GAP)).child(name))
            .into_any_element()
    }

    fn render_large_icons(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        let viewport_width = (self.list_viewport_width(window) - SCROLLBAR_GUTTER_WIDTH).max(0.0);
        let layout_key = LargeIconLayoutCacheKey::new(
            &self.entries,
            viewport_width,
            self.show_file_name_extensions,
            &self.font,
        );

        if self.large_icon_layout_key.as_ref() != Some(&layout_key)
            || self.large_icon_layout.is_none()
        {
            let layout = LargeIconLayout::from_cache_key(&layout_key, cx);
            self.large_icon_list_state.reset(layout.row_count());
            self.large_icon_layout = Some(layout);
            self.large_icon_layout_key = Some(layout_key);
        }
        let layout = self
            .large_icon_layout
            .clone()
            .expect("large icon layout is initialized before rendering");

        div().flex().flex_col().size_full().overflow_hidden().child(
            div()
                .flex()
                .flex_row()
                .flex_1()
                .overflow_hidden()
                .child(
                    add_current_folder_drop_handlers(
                        div()
                            .id("explorer-list-background")
                            .relative()
                            .flex_1()
                            .h_full()
                            .overflow_hidden(),
                        CurrentFolderClickTarget::Background,
                        cx,
                    )
                    .child(self.render_mouse_selection_hit_layer(cx))
                    .child(
                        list(
                            self.large_icon_list_state.clone(),
                            cx.processor(move |this, row_ix: usize, window, cx| {
                                this.render_large_icon_row(row_ix, layout.clone(), window, cx)
                            }),
                        )
                        .size_full(),
                    )
                    .child(self.render_mouse_selection_box()),
                )
                .child(self.render_scrollbar(cx)),
        )
    }

    fn render_list(&mut self, cx: &mut Context<Self>) -> Div {
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
                        add_current_folder_drop_handlers(
                            div()
                                .id("explorer-list-background")
                                .relative()
                                .flex_1()
                                .h_full()
                                .overflow_hidden(),
                            CurrentFolderClickTarget::Background,
                            cx,
                        )
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
        add_current_folder_drop_handlers(
            div().id("explorer-empty-folder-drop-target").size_full(),
            CurrentFolderClickTarget::EmptyFolder,
            cx,
        )
        .child(render_empty_folder_message(message, detail))
        .into_any_element()
    }

    fn render_status_bar(&self) -> AnyElement {
        let summary = folder_status_summary(&self.entries, &self.selection.selected_indices);
        let codebase_summary = self
            .codebase_summary
            .as_ref()
            .filter(|summary| summary.total_code > 0 && !summary.languages.is_empty());
        let git_status = self.git_status.as_ref();

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
                this.child(status_bar_separator()).child(
                    div()
                        .min_w(px(0.0))
                        .truncate()
                        .child(SharedString::from(selection_info)),
                )
            })
            .child(div().flex_1().min_w(px(12.0)))
            .when_some(codebase_summary, |this, codebase_summary| {
                this.child(render_codebase_makeup_status(codebase_summary))
            })
            .when_some(git_status, |this, git_status| {
                this.when(codebase_summary.is_some(), |this| {
                    this.child(status_bar_separator())
                })
                .child(render_git_repository_status(git_status))
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
    tooltip: &'static str,
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
        .tooltip(explorer_tooltip(tooltip))
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
        let window_width = f32::from(window.bounds().size.width);
        let sidebar_width = normalized_sidebar_width_f32(self.sidebar_width);
        let sidebar_auto_hide_active = sidebar_auto_hide_is_active(sidebar_width, window_width);
        if !sidebar_auto_hide_active {
            self.sidebar_auto_hide_expanded = false;
        }
        let sidebar_visible = effective_sidebar_is_visible(
            sidebar_width,
            window_width,
            self.sidebar_auto_hide_expanded,
        );

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
            .on_action(cx.listener(Self::handle_open_selected_in_new_tab))
            .on_action(cx.listener(Self::handle_open_properties))
            .on_action(cx.listener(Self::handle_open_settings))
            .on_action(cx.listener(Self::handle_enter_selected))
            .on_action(cx.listener(Self::handle_enter_selected_in_new_tab))
            .on_action(cx.listener(Self::handle_refresh))
            .on_action(cx.listener(Self::handle_select_all))
            .on_action(cx.listener(Self::handle_copy_selected))
            .on_action(cx.listener(Self::handle_cut_selected))
            .on_action(cx.listener(Self::handle_paste_clipboard))
            .on_action(cx.listener(Self::handle_undo_file_operation))
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
            .child(self.render_utility_bar(sidebar_auto_hide_active, sidebar_visible, cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .when(sidebar_visible, |this| this.child(self.render_sidebar(cx)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .h_full()
                            .overflow_hidden()
                            .when(self.view_mode == FileViewMode::Details, |this| {
                                this.child(self.render_header(window, cx))
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
                                    ExplorerContentBranch::Loading => div().child(
                                        self.render_empty_folder(FOLDER_LOADING_MESSAGE, cx),
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
                                        if self.view_mode == FileViewMode::LargeIcons {
                                            div().child(self.render_large_icons(window, cx))
                                        } else {
                                            div().child(self.render_list(cx))
                                        }
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
            .when_some(
                self.render_utility_menu_overlay(sidebar_auto_hide_active, cx),
                |this, menu| this.child(menu),
            )
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
        .id("drop-indicator")
        .debug_selector(|| "drop-indicator".to_owned())
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
        .my(px(18.0))
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
) -> (PathBuf, Option<usize>, Option<DirectoryKind>, bool) {
    let open_icon_kind = match item.kind {
        SidebarItemKind::Directory(kind) => Some(kind),
        SidebarItemKind::CustomDirectory => crate::explorer::resolve_directory_kind(&item.path),
        SidebarItemKind::Drive => Some(DirectoryKind::Drive),
        SidebarItemKind::DriveWindows => Some(DirectoryKind::DriveWindows),
        SidebarItemKind::DriveWsl => Some(DirectoryKind::DriveWsl),
    };
    let can_eject =
        matches!(item.kind, SidebarItemKind::Drive) && drive_root_is_ejectable(&item.path);
    (
        item.path.clone(),
        item.configured_index,
        open_icon_kind,
        can_eject,
    )
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

fn sidebar_item_icon(item: &SidebarItem) -> AnyElement {
    sidebar_item_kind_icon_for_path(item.kind, &item.path)
}

fn sidebar_item_kind_icon_for_path(kind: SidebarItemKind, path: &Path) -> AnyElement {
    match kind {
        SidebarItemKind::Directory(kind) => directory_kind_icon(kind),
        SidebarItemKind::CustomDirectory => folder_icon().into_any_element(),
        SidebarItemKind::Drive if drive_root_is_ejectable(path) => drive_disc_icon(),
        SidebarItemKind::Drive => drive_icon().into_any_element(),
        SidebarItemKind::DriveWindows => drive_windows_icon().into_any_element(),
        SidebarItemKind::DriveWsl => drive_wsl_icon_for_path(path).into_any_element(),
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
    let can_paste = clipboard_item_can_paste(clipboard.as_ref());
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
    can_eject: bool,
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
        can_eject,
        window,
        cx,
    ) {
        cx.notify();
    }
    cx.stop_propagation();
}

fn add_entry_primary_click(
    element: gpui::Stateful<Div>,
    entry: FileEntry,
    target: EntryClickTarget,
    cx: &mut Context<ExplorerView>,
) -> gpui::Stateful<Div> {
    element.on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
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

        if target == EntryClickTarget::Row
            && !this.commit_active_rename_before_interaction(window, cx)
        {
            cx.stop_propagation();
            cx.notify();
            return;
        }

        let click_count = this.normalize_entry_click_count(&entry, event.click_count());
        if is_alt_entry_double_click(event, click_count) {
            this.handle_entry_properties_click(&entry, selection_modifiers_for_click(event));
            this.open_selected_properties(window, cx);
            cx.stop_propagation();
            cx.notify();
            return;
        }

        let directory_open_mode = directory_open_mode_for_entry_click(event, click_count);
        let action = match target {
            EntryClickTarget::Row => this.handle_entry_click_with_watcher_and_directory_mode(
                &entry,
                click_count,
                selection_modifiers_for_click(event),
                directory_open_mode,
                cx,
            ),
            EntryClickTarget::Name => this.handle_entry_name_click(
                &entry,
                click_count,
                selection_modifiers_for_click(event),
                directory_open_mode,
                window,
                cx,
            ),
        };
        if let Some(action) = action {
            this.perform_entry_action(action, window, cx);
        }
        cx.stop_propagation();
        cx.notify();
    }))
}

fn add_entry_context_menu(
    element: gpui::Stateful<Div>,
    entry: FileEntry,
    target: EntryContextMenuTarget,
    cx: &mut Context<ExplorerView>,
) -> gpui::Stateful<Div> {
    element.on_mouse_up(
        MouseButton::Right,
        cx.listener(move |this, event: &MouseUpEvent, window, cx| match target {
            EntryContextMenuTarget::WholeEntry => {
                open_entry_context_menu_from_event(this, event, &entry, window, cx);
            }
            EntryContextMenuTarget::NameCell => {
                let clicked_index = this.entry_index_by_path(&entry.path);
                if clicked_index.is_some_and(|ix| this.entry_is_selected(ix)) {
                    open_entry_context_menu_from_event(this, event, &entry, window, cx);
                } else {
                    open_current_folder_context_menu_from_event(this, event, window, cx);
                }
            }
        }),
    )
}

fn add_entry_middle_click(
    element: gpui::Stateful<Div>,
    entry: FileEntry,
    cx: &mut Context<ExplorerView>,
) -> gpui::Stateful<Div> {
    element.on_mouse_down(
        MouseButton::Middle,
        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
            if !this.commit_active_rename_before_interaction(window, cx) {
                cx.stop_propagation();
                cx.notify();
                return;
            }

            if let Some(path) = this
                .handle_entry_middle_click(&entry, SelectionModifiers::from_gpui(event.modifiers))
            {
                cx.emit(ExplorerViewEvent::OpenDirectoryInNewTab(path));
            }
            cx.stop_propagation();
            cx.notify();
        }),
    )
}

fn add_selected_entry_drag(
    element: gpui::Stateful<Div>,
    drag_payload: Option<DraggedEntries>,
    entity: Entity<ExplorerView>,
) -> gpui::Stateful<Div> {
    let Some(drag_payload) = drag_payload else {
        return element;
    };

    let external_paths = ExternalPaths::new(drag_payload.paths.clone());
    element.on_drag_with_external_paths(drag_payload, external_paths, {
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
    })
}

fn add_individual_entry_drag(
    element: gpui::Stateful<Div>,
    drag_payload: Option<DraggedEntries>,
    entity: Entity<ExplorerView>,
) -> gpui::Stateful<Div> {
    let Some(drag_payload) = drag_payload else {
        return element;
    };

    let external_paths = ExternalPaths::new(drag_payload.paths.clone());
    element.on_drag_with_external_paths(
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
}

fn add_drop_handlers(
    element: gpui::Stateful<Div>,
    destination: DropDestination,
    highlights_target: bool,
    entity: Entity<ExplorerView>,
    cx: &mut Context<ExplorerView>,
) -> gpui::Stateful<Div> {
    let element = element
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
        .can_drop({
            let destination = destination.clone();
            let entity = entity.clone();
            move |dragged_value, window, cx| {
                entity.update(cx, |this, _| {
                    this.can_drop_value(dragged_value, &destination, window.modifiers())
                })
            }
        });

    let element = if highlights_target {
        element
            .drag_over::<DraggedEntries>(|style, _, _, _| {
                style.bg(rgb(0xe5f3ff)).border_color(rgb(0x0078d7))
            })
            .drag_over::<ExternalPaths>(|style, _, _, _| {
                style.bg(rgb(0xe5f3ff)).border_color(rgb(0x0078d7))
            })
    } else {
        element
            .drag_over::<DraggedEntries>(|style, _, _, _| style.bg(rgb(0xf7fbff)))
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(0xf7fbff)))
    };

    element
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
}

fn add_current_folder_drop_handlers(
    element: gpui::Stateful<Div>,
    target: CurrentFolderClickTarget,
    cx: &mut Context<ExplorerView>,
) -> gpui::Stateful<Div> {
    let entity = cx.entity();
    add_drop_handlers(
        element,
        DropDestination::CurrentDirectory,
        false,
        entity,
        cx,
    )
    .on_mouse_up(
        MouseButton::Right,
        cx.listener(|this, event: &MouseUpEvent, window, cx| {
            open_current_folder_context_menu_from_event(this, event, window, cx);
        }),
    )
    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
        if !event.standard_click() {
            cx.stop_propagation();
            return;
        }

        match target {
            CurrentFolderClickTarget::Background => {
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
            }
            CurrentFolderClickTarget::EmptyFolder => {
                this.close_context_menu();
                if this.commit_active_rename_before_interaction(window, cx) {
                    this.clear_selection();
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }))
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

fn utility_menu_left(menu: UtilityMenu, sidebar_toggle_visible: bool) -> f32 {
    let left = match menu {
        UtilityMenu::New => UTILITY_NEW_MENU_LEFT,
        UtilityMenu::View => UTILITY_VIEW_MENU_LEFT,
    };

    if sidebar_toggle_visible {
        left + UTILITY_SIDEBAR_TOGGLE_MENU_OFFSET
    } else {
        left
    }
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
    tooltip: &'static str,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
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
        .tooltip(explorer_tooltip(tooltip))
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

fn utility_menu_separator() -> Div {
    div()
        .h(px(9.0))
        .mx(px(10.0))
        .flex()
        .items_center()
        .child(div().h(px(1.0)).w_full().bg(rgb(0xe5e5e5)))
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
        ContextMenuIcon::CopyAsPath => gpui::img(COPY_AS_PATH_ICON.clone())
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
        ContextMenuIcon::Properties => gpui::img(PROPERTIES_ICON.clone())
            .w(px(CONTEXT_MENU_ICON_SIZE))
            .h(px(CONTEXT_MENU_ICON_SIZE))
            .into_any_element(),
        ContextMenuIcon::Extract => gpui::img(EXTRACT_ICON.clone())
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
        ContextMenuIcon::FolderKindForPath { path, kind } => {
            if matches!(kind, Some(DirectoryKind::DriveWsl)) {
                drive_wsl_icon_sized_for_path(&path, CONTEXT_MENU_ICON_SIZE)
            } else if matches!(kind, Some(DirectoryKind::Drive)) && drive_root_is_ejectable(&path) {
                drive_disc_icon_sized(CONTEXT_MENU_ICON_SIZE)
            } else {
                kind.map(|kind| directory_kind_icon_sized(kind, CONTEXT_MENU_ICON_SIZE))
                    .unwrap_or_else(|| folder_icon_sized(CONTEXT_MENU_ICON_SIZE).into_any_element())
            }
        }
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
        ContextMenuIcon::Eject => gpui::img(EJECT_ICON.clone())
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
        .debug_selector(move || id.to_owned())
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

fn utility_menu_image_icon(icon: Arc<Image>) -> AnyElement {
    gpui::img(icon).w(px(16.0)).h(px(16.0)).into_any_element()
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
    tooltip: &'static str,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
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
        .tooltip(explorer_tooltip(tooltip))
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

fn directory_bar(
    breadcrumb: VisibleBreadcrumb,
    hovered: bool,
    hover_generation: usize,
    cx: &mut Context<ExplorerView>,
) -> AnyElement {
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
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            if this.directory_bar_hovered != *hovered {
                if *hovered {
                    this.directory_bar_hover_generation =
                        this.directory_bar_hover_generation.wrapping_add(1);
                }
                this.directory_bar_hovered = *hovered;
                cx.notify();
            }
        }))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .children(directory_bar_children(breadcrumb, cx))
                .child(div().flex_1().min_w(px(0.0))),
        )
        .child(directory_copy_address_button(hovered, hover_generation, cx))
        .into_any_element()
}

fn directory_copy_address_button(
    visible: bool,
    fade_generation: usize,
    cx: &mut Context<ExplorerView>,
) -> AnyElement {
    let button = div()
        .id("directory-copy-address")
        .debug_selector(|| "directory-copy-address".to_owned())
        .flex()
        .items_center()
        .justify_center()
        .w(px(DIRECTORY_BAR_COPY_BUTTON_SIZE))
        .h(px(DIRECTORY_BAR_COPY_BUTTON_SIZE))
        .ml(px(DIRECTORY_BAR_COPY_BUTTON_GAP))
        .flex_shrink_0()
        .rounded(px(4.0))
        .cursor_default()
        .hover(|style| style.bg(rgb(NAV_BUTTON_HOVER_BG)))
        .active(|style| style.opacity(NAV_BUTTON_ACTIVE_OPACITY))
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.close_context_menu();
            this.open_utility_menu = None;
            if this.commit_active_rename_before_interaction(window, cx) {
                cx.write_to_clipboard(ClipboardItem::new_string(
                    this.address_text_for_path(&this.path),
                ));
            }
            cx.stop_propagation();
            cx.notify();
        }))
        .tooltip(explorer_tooltip("Copy address"))
        .child(gpui::img(COPY_ICON.clone()).w(px(16.0)).h(px(16.0)));

    if visible {
        button
            .with_animation(
                ("directory-copy-address-fade", fade_generation),
                Animation::new(Duration::from_millis(DIRECTORY_COPY_ADDRESS_FADE_MS)),
                |button, delta| button.opacity(delta),
            )
            .into_any_element()
    } else {
        button.opacity(0.0).into_any_element()
    }
}

#[cfg(test)]
fn copied_directory_address(path: &std::path::Path) -> String {
    format_address_path(path, crate::settings::AddressSlash::Forward)
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

fn header_cell(
    label: &'static str,
    width: f32,
    sort_column: Option<FileSortColumn>,
    active_sort: Option<FileSortSettings>,
) -> Div {
    let cell = div()
        .relative()
        .flex()
        .items_start()
        .h_full()
        .w(px(width))
        .flex_shrink_0()
        .pl(px(8.0))
        .pr(px(FILE_SORT_CHEVRON_RESERVED_WIDTH))
        .pt(px(8.0))
        .border_r_1()
        .border_color(rgb(0xe7e7e7))
        .hover(|style| style.bg(rgb(FILE_COLUMN_HEADER_HOVER_BG)));

    let cell = if sort_column.is_some() {
        cell.cursor(CursorStyle::PointingHand)
    } else {
        cell
    };

    let cell = cell.child(header_label(label));
    match (
        sort_column,
        sort_indicator_direction(sort_column, active_sort),
    ) {
        (Some(column), Some(direction)) => cell.child(sort_indicator_element(column, direction)),
        _ => cell,
    }
}

fn add_header_sort_click(
    cell: gpui::Stateful<Div>,
    column: FileSortColumn,
    entity: Entity<ExplorerView>,
) -> gpui::Stateful<Div> {
    cell.on_click(move |_: &ClickEvent, _, cx| {
        let _ = entity.update(cx, |this, cx| {
            this.close_context_menu();
            let sort = this.sort_entries_from_header(column);
            crate::settings::set_file_sort(sort, cx);
            cx.notify();
        });
        cx.stop_propagation();
    })
}

fn sort_indicator_direction(
    sort_column: Option<FileSortColumn>,
    active_sort: Option<FileSortSettings>,
) -> Option<SortDirection> {
    let sort_column = sort_column?;
    let active_sort = active_sort?;
    (active_sort.column == sort_column).then_some(active_sort.direction)
}

fn sort_indicator_element(column: FileSortColumn, direction: SortDirection) -> AnyElement {
    let icon = match direction {
        SortDirection::Ascending => SORT_CHEVRON_UP_ICON.clone(),
        SortDirection::Descending => SORT_CHEVRON_DOWN_ICON.clone(),
    };

    div()
        .debug_selector(move || sort_indicator_debug_selector(column).to_owned())
        .absolute()
        .top(px(0.0))
        .right(px(FILE_SORT_CHEVRON_RIGHT_OFFSET))
        .h_full()
        .w(px(FILE_SORT_CHEVRON_ICON_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .child(image_icon(
            icon,
            FILE_SORT_CHEVRON_ICON_SIZE,
            FILE_SORT_CHEVRON_ICON_SIZE,
        ))
        .into_any_element()
}

fn header_label(label: &'static str) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .min_w(px(0.0))
        .child(label)
        .into_any_element()
}

fn sort_indicator_debug_selector(column: FileSortColumn) -> &'static str {
    match column {
        FileSortColumn::Name => "explorer-header-name-sort-chevron",
        FileSortColumn::DateModified => "explorer-header-date-modified-sort-chevron",
        FileSortColumn::Type => "explorer-header-type-sort-chevron",
        FileSortColumn::Size => "explorer-header-size-sort-chevron",
    }
}

fn file_column_sort_column(kind: FileColumnKind) -> Option<FileSortColumn> {
    match kind {
        FileColumnKind::DateModified => Some(FileSortColumn::DateModified),
        FileColumnKind::Type => Some(FileSortColumn::Type),
        FileColumnKind::Size => Some(FileSortColumn::Size),
    }
}

fn file_column_header_element_id(kind: FileColumnKind) -> &'static str {
    match kind {
        FileColumnKind::DateModified => "explorer-header-date-modified",
        FileColumnKind::Type => "explorer-header-type",
        FileColumnKind::Size => "explorer-header-size",
    }
}

fn file_column_entry_drag_element_id(kind: FileColumnKind) -> &'static str {
    match kind {
        FileColumnKind::DateModified => "explorer-entry-date-drag",
        FileColumnKind::Type => "explorer-entry-type-drag",
        FileColumnKind::Size => "explorer-entry-size-drag",
    }
}

fn file_column_resize_debug_selector(kind: FileColumnKind) -> &'static str {
    match kind {
        FileColumnKind::DateModified => "explorer-header-date-modified-resizer",
        FileColumnKind::Type => "explorer-header-type-resizer",
        FileColumnKind::Size => "explorer-header-size-resizer",
    }
}

fn file_column_resize_handle(kind: FileColumnKind, entity: Entity<ExplorerView>) -> AnyElement {
    div()
        .debug_selector(move || file_column_resize_debug_selector(kind).to_owned())
        .absolute()
        .top(px(0.0))
        .right(px(-(FILE_COLUMN_RESIZE_HIT_WIDTH / 2.0)))
        .w(px(FILE_COLUMN_RESIZE_HIT_WIDTH))
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
                                this.begin_file_column_resize(kind, f32::from(event.position.x));
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
                                if this.file_column_resize_drag.is_none() {
                                    return;
                                }

                                if this.update_file_column_resize(f32::from(event.position.x)) {
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
                                    let Some(result) = this.finish_file_column_resize() else {
                                        return;
                                    };

                                    match result {
                                        FileColumnResizeResult::Name(width) => {
                                            crate::settings::set_name_column_width(width, cx);
                                        }
                                        FileColumnResizeResult::Column(kind, width) => {
                                            crate::settings::set_file_column_width(kind, width, cx);
                                        }
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                });
                            }
                            MouseButton::Right | MouseButton::Middle
                                if bounds.contains(&event.position) =>
                            {
                                let _ = entity.update(cx, |this, cx| {
                                    this.close_context_menu();
                                    let (kind, width) = this.reset_file_column_width(kind);
                                    crate::settings::set_file_column_width(kind, width, cx);
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

fn name_header_cell(
    width: f32,
    manual_width: bool,
    entity: Entity<ExplorerView>,
    active_sort: Option<FileSortSettings>,
) -> gpui::Stateful<Div> {
    let cell = div()
        .id("explorer-header-name")
        .debug_selector(|| "explorer-header-name".to_owned())
        .relative()
        .flex()
        .items_start()
        .h_full()
        .min_w(px(COLUMN_NAME_MIN_WIDTH))
        .overflow_hidden()
        .pl(px(36.0))
        .pr(px(FILE_SORT_CHEVRON_RESERVED_WIDTH))
        .pt(px(8.0))
        .border_r_1()
        .border_color(rgb(0xe7e7e7))
        .hover(|style| style.bg(rgb(FILE_COLUMN_HEADER_HOVER_BG)))
        .cursor(CursorStyle::PointingHand);
    let cell = if manual_width {
        cell.w(px(width)).flex_shrink_0()
    } else {
        cell.flex_1()
    };

    let cell = cell.child(header_label("Name"));
    let cell = if let Some(direction) =
        sort_indicator_direction(Some(FileSortColumn::Name), active_sort)
    {
        cell.child(sort_indicator_element(FileSortColumn::Name, direction))
    } else {
        cell
    };

    add_header_sort_click(cell, FileSortColumn::Name, entity.clone())
        .child(name_column_resize_handle(width, entity))
}

fn name_column_resize_handle(width: f32, entity: Entity<ExplorerView>) -> AnyElement {
    div()
        .debug_selector(|| "explorer-header-name-resizer".to_owned())
        .absolute()
        .top(px(0.0))
        .right(px(-(FILE_COLUMN_RESIZE_HIT_WIDTH / 2.0)))
        .w(px(FILE_COLUMN_RESIZE_HIT_WIDTH))
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
                                this.begin_name_column_resize(f32::from(event.position.x), width);
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
                                if this.file_column_resize_drag.is_none() {
                                    return;
                                }

                                if this.update_file_column_resize(f32::from(event.position.x)) {
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
                                    let Some(result) = this.finish_file_column_resize() else {
                                        return;
                                    };

                                    match result {
                                        FileColumnResizeResult::Name(width) => {
                                            crate::settings::set_name_column_width(width, cx);
                                        }
                                        FileColumnResizeResult::Column(kind, width) => {
                                            crate::settings::set_file_column_width(kind, width, cx);
                                        }
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                });
                            }
                            MouseButton::Right | MouseButton::Middle
                                if bounds.contains(&event.position) =>
                            {
                                let _ = entity.update(cx, |this, cx| {
                                    this.close_context_menu();
                                    this.reset_name_column_width();
                                    crate::settings::clear_name_column_width(cx);
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

fn name_cell(
    entry: &FileEntry,
    app_icon: Option<Arc<Image>>,
    show_file_name_extensions: bool,
    show_full_path: bool,
    name_column_width: f32,
    manual_width: bool,
    font: &gpui::Font,
    window: &Window,
) -> Div {
    let text_width = if show_full_path {
        recursive_result_text_width(name_column_width)
    } else {
        available_filename_text_width(name_column_width)
    };
    let filename = truncated_text(
        entry.display_name_with_extensions(show_file_name_extensions),
        text_width,
        0x000000,
        font,
        window,
    );
    let cell = div()
        .flex()
        .items_center()
        .h_full()
        .min_w(px(COLUMN_NAME_MIN_WIDTH))
        .overflow_hidden()
        .pl(px(NAME_CELL_LEFT_PADDING));
    let cell = if manual_width {
        cell.w(px(name_column_width)).flex_shrink_0()
    } else {
        cell.flex_1()
    };

    cell.child(entry_icon(entry, app_icon))
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
    name_column_width: f32,
    manual_width: bool,
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

    let cell = div()
        .flex()
        .items_center()
        .h_full()
        .min_w(px(COLUMN_NAME_MIN_WIDTH))
        .overflow_hidden()
        .pl(px(NAME_CELL_LEFT_PADDING));
    let cell = if manual_width {
        cell.w(px(name_column_width)).flex_shrink_0()
    } else {
        cell.flex_1()
    };

    cell.child(entry_icon(entry, app_icon)).child(input)
}

fn entry_icon(entry: &FileEntry, app_icon: Option<Arc<Image>>) -> AnyElement {
    if entry.uses_directory_shortcut_icon() {
        return directory_shortcut_icon().into_any_element();
    }

    if let Some(app_icon) = app_icon {
        return image_icon(app_icon, FILE_ICON_SLOT_WIDTH, FILE_ICON_SLOT_HEIGHT);
    }

    if entry.is_directory_like() {
        if wsl_distro_kind_for_path(&entry.path).is_some() {
            return drive_wsl_icon_sized_for_path(&entry.path, FILE_ICON_SLOT_WIDTH);
        }

        folder_icon().into_any_element()
    } else {
        file_icon_for_path(&entry.path).into_any_element()
    }
}

fn large_entry_icon(
    entry: &FileEntry,
    image_thumbnail: Option<Arc<Image>>,
    app_icon: Option<Arc<Image>>,
) -> AnyElement {
    if let Some(image_thumbnail) = image_thumbnail {
        return image_icon(image_thumbnail, LARGE_ICON_SIZE, LARGE_ICON_SIZE);
    }

    if entry.uses_directory_shortcut_icon() {
        return directory_shortcut_icon_sized(LARGE_ICON_SIZE).into_any_element();
    }

    if let Some(app_icon) = app_icon {
        return image_icon(app_icon, LARGE_ICON_SIZE, LARGE_ICON_SIZE);
    }

    if entry.is_directory_like() {
        if wsl_distro_kind_for_path(&entry.path).is_some() {
            return drive_wsl_icon_sized_for_path(&entry.path, LARGE_ICON_SIZE);
        }

        folder_icon_sized(LARGE_ICON_SIZE).into_any_element()
    } else {
        large_file_icon_for_path_sized(&entry.path, LARGE_ICON_SIZE).into_any_element()
    }
}

fn large_icon_filename(
    ix: usize,
    entry: &FileEntry,
    show_file_name_extensions: bool,
    font: &gpui::Font,
    name_click_entry: FileEntry,
    cx: &mut Context<ExplorerView>,
) -> AnyElement {
    let name = div()
        .w(px(large_icon_filename_text_width()))
        .max_h(px(LARGE_ICON_TEXT_LINE_HEIGHT * LARGE_ICON_TEXT_ROWS as f32))
        .overflow_hidden()
        .text_center()
        .text_size(px(LARGE_ICON_TEXT_SIZE))
        .line_height(px(LARGE_ICON_TEXT_LINE_HEIGHT))
        .font(font.clone())
        .text_ellipsis()
        .line_clamp(LARGE_ICON_TEXT_ROWS)
        .child(
            entry
                .display_name_with_extensions(show_file_name_extensions)
                .to_owned(),
        )
        .id(("explorer-large-icon-name", ix));

    add_entry_primary_click(name, name_click_entry, EntryClickTarget::Name, cx).into_any_element()
}

fn large_icon_rename_input(
    focus_handle: Option<FocusHandle>,
    cx: &mut Context<ExplorerView>,
) -> AnyElement {
    let entity = cx.entity();
    div()
        .debug_selector(|| "large-icon-rename-input".to_owned())
        .key_context("ExplorerRenameInput")
        .w(px(LARGE_ICON_TILE_WIDTH - 16.0))
        .h(px(20.0))
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
        .child(rename_text_element(entity))
        .into_any_element()
}

fn filename_text_width(name_column_width: f32) -> f32 {
    (name_column_width - NAME_CELL_LEFT_PADDING - FILE_ICON_SLOT_WIDTH - NAME_ICON_TEXT_GAP)
        .max(0.0)
}

fn available_filename_text_width(viewport_width: f32) -> f32 {
    filename_text_width(viewport_width)
}

fn recursive_result_text_width(name_column_width: f32) -> f32 {
    available_filename_text_width(name_column_width)
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

fn file_column_cell(
    kind: FileColumnKind,
    entry: &FileEntry,
    width: f32,
    date_format: &str,
    font: &gpui::Font,
    window: &Window,
) -> Div {
    let (text, right) = match kind {
        FileColumnKind::DateModified => (format_timestamp(entry.modified, date_format), false),
        FileColumnKind::Type => (entry.type_label(), false),
        FileColumnKind::Size => (format_size(entry.size), true),
    };

    text_cell(text, width, right, font, window)
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

fn is_alt_entry_double_click(event: &ClickEvent, click_count: usize) -> bool {
    match event {
        ClickEvent::Mouse(event) => {
            event.down.button == MouseButton::Left && click_count == 2 && event.up.modifiers.alt
        }
        ClickEvent::Keyboard(_) => false,
    }
}

fn directory_open_mode_for_entry_click(
    event: &ClickEvent,
    click_count: usize,
) -> DirectoryOpenMode {
    if is_ctrl_entry_double_click(event, click_count) {
        DirectoryOpenMode::NewTab
    } else {
        DirectoryOpenMode::CurrentTab
    }
}

fn is_ctrl_entry_double_click(event: &ClickEvent, click_count: usize) -> bool {
    match event {
        ClickEvent::Mouse(event) => {
            event.down.button == MouseButton::Left && click_count == 2 && event.up.modifiers.control
        }
        ClickEvent::Keyboard(_) => false,
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

fn render_codebase_makeup_status(summary: &CodebaseSummary) -> AnyElement {
    let Some(dominant_language) = summary.languages.first() else {
        return div().into_any_element();
    };
    let total_code = summary.total_code.separate_with_commas();

    div()
        .flex()
        .flex_row()
        .items_center()
        .h_full()
        .flex_shrink_0()
        .child(render_codebase_makeup_bar(summary))
        .child(status_bar_separator())
        .child(
            div()
                .id("codebase-lines-of-code")
                .debug_selector(|| "codebase-lines-of-code".to_owned())
                .flex_shrink_0()
                .cursor_default()
                .tooltip(explorer_tooltip(lines_of_code_tooltip(summary.total_code)))
                .child(SharedString::from(total_code)),
        )
        .child(status_bar_separator())
        .child(div().flex_shrink_0().child(SharedString::from(format!(
            "{}% {}",
            dominant_language.percentage, dominant_language.name
        ))))
        .into_any_element()
}

fn render_codebase_makeup_bar(summary: &CodebaseSummary) -> Div {
    let separator_count = summary.languages.len().saturating_sub(1);
    let separator_width = CODEBASE_MAKEUP_SEPARATOR_WIDTH * separator_count as f32;
    let language_width = (CODEBASE_MAKEUP_BAR_WIDTH - separator_width).max(0.0);
    let widths = language_segment_widths(&summary.languages, summary.total_code, language_width);
    let colors = summary
        .languages
        .iter()
        .map(|language| language.color)
        .collect::<Vec<_>>();
    let segments = codebase_makeup_segments(&widths, &colors);

    div()
        .relative()
        .h(px(CODEBASE_MAKEUP_BAR_HEIGHT))
        .w(px(CODEBASE_MAKEUP_BAR_WIDTH))
        .flex_shrink_0()
        .overflow_hidden()
        .rounded(px(CODEBASE_MAKEUP_BAR_RADIUS))
        .bg(rgb(CODEBASE_MAKEUP_SEPARATOR_COLOR))
        .child(
            canvas(
                {
                    let segments = segments.clone();
                    move |_, _, _| segments
                },
                |bounds, segments, window, _| {
                    for segment in segments {
                        if segment.width <= 0.0 {
                            continue;
                        }

                        let segment_bounds = Bounds {
                            origin: gpui::point(
                                bounds.origin.x + px(segment.left),
                                bounds.origin.y,
                            ),
                            size: gpui::size(px(segment.width), bounds.size.height),
                        };
                        window.paint_quad(
                            gpui::fill(segment_bounds, rgb(segment.color)).corner_radii(
                                codebase_makeup_segment_corner_radii(&segment, bounds.size.height),
                            ),
                        );
                    }
                },
            )
            .size_full(),
        )
        .children(
            summary
                .languages
                .iter()
                .zip(segments)
                .enumerate()
                .filter_map(|(ix, (language, segment))| {
                    (segment.width > 0.0).then(|| {
                        div()
                            .id(("codebase-makeup-segment", ix))
                            .debug_selector(move || format!("codebase-makeup-segment-{ix}"))
                            .absolute()
                            .left(px(segment.left))
                            .top(px(0.0))
                            .h_full()
                            .w(px(segment.width))
                            .cursor_default()
                            .tooltip(explorer_tooltip(language.name.clone()))
                            .into_any_element()
                    })
                }),
        )
}

fn codebase_makeup_segments(widths: &[f32], colors: &[u32]) -> Vec<CodebaseMakeupSegment> {
    let segment_count = widths.len().min(colors.len());
    let mut left = 0.0;
    let mut segments = Vec::with_capacity(segment_count);

    for ix in 0..segment_count {
        let width = widths[ix].max(0.0).round();
        segments.push(CodebaseMakeupSegment {
            left,
            width,
            color: colors[ix],
            round_left: false,
            round_right: false,
        });

        left += width;
        if ix + 1 < segment_count {
            left += CODEBASE_MAKEUP_SEPARATOR_WIDTH;
        }
    }

    let first_visible = segments.iter().position(|segment| segment.width > 0.0);
    let last_visible = segments.iter().rposition(|segment| segment.width > 0.0);
    if let Some(ix) = first_visible {
        segments[ix].round_left = true;
    }
    if let Some(ix) = last_visible {
        segments[ix].round_right = true;
    }

    segments
}

fn codebase_makeup_segment_corner_radii(
    segment: &CodebaseMakeupSegment,
    height: Pixels,
) -> gpui::Corners<Pixels> {
    let radius = px(CODEBASE_MAKEUP_BAR_RADIUS);
    gpui::Corners {
        top_left: if segment.round_left { radius } else { px(0.0) },
        top_right: if segment.round_right { radius } else { px(0.0) },
        bottom_right: if segment.round_right { radius } else { px(0.0) },
        bottom_left: if segment.round_left { radius } else { px(0.0) },
    }
    .clamp_radii_for_quad_size(gpui::size(px(segment.width), height))
}

fn render_git_repository_status(status: &GitRepositoryStatus) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .h_full()
        .min_w(px(0.0))
        .flex_shrink()
        .overflow_hidden()
        .when_some(status.divergence, |this, divergence| {
            this.child(status_bar_icon_label(
                GIT_ICON.clone(),
                git_divergence_label(divergence),
                false,
                "git-divergence-status",
                Some(git_divergence_tooltip(divergence)),
            ))
            .child(status_bar_separator())
        })
        .child(status_bar_icon_label(
            GIT_BRANCH_ICON.clone(),
            status.branch.clone(),
            true,
            "git-branch-status",
            Some(git_branch_tooltip(&status.branch)),
        ))
        .into_any_element()
}

fn status_bar_icon_label(
    icon: Arc<Image>,
    label: String,
    flexible: bool,
    debug_selector: &'static str,
    tooltip: Option<SharedString>,
) -> AnyElement {
    div()
        .id(debug_selector)
        .debug_selector(move || debug_selector.to_owned())
        .flex()
        .flex_row()
        .items_center()
        .gap(px(STATUS_BAR_GIT_ITEM_GAP))
        .min_w(px(0.0))
        .when(!flexible, |this| this.flex_shrink_0())
        .when(flexible, |this| this.flex_shrink().overflow_hidden())
        .cursor_default()
        .child(image_icon(
            icon,
            STATUS_BAR_GIT_ICON_SIZE,
            STATUS_BAR_GIT_ICON_SIZE,
        ))
        .child(
            div()
                .min_w(px(0.0))
                .truncate()
                .child(SharedString::from(label)),
        )
        .when_some(tooltip, |this, tooltip| {
            this.tooltip(explorer_tooltip(tooltip))
        })
        .into_any_element()
}

fn git_divergence_label(divergence: GitDivergence) -> String {
    format!(
        "{} / {}",
        divergence.outgoing.separate_with_commas(),
        divergence.incoming.separate_with_commas()
    )
}

fn git_branch_tooltip(branch: &str) -> SharedString {
    SharedString::from(format!("Current Branch: {branch}"))
}

fn git_divergence_tooltip(divergence: GitDivergence) -> SharedString {
    SharedString::from(format!(
        "{} outgoing / {} incoming commits",
        divergence.outgoing.separate_with_commas(),
        divergence.incoming.separate_with_commas()
    ))
}

fn lines_of_code_tooltip(total_code: usize) -> SharedString {
    SharedString::from(format!(
        "{} Lines of Code",
        total_code.separate_with_commas()
    ))
}

fn status_bar_separator() -> Div {
    div()
        .h(px(14.0))
        .w(px(1.0))
        .mx(px(12.0))
        .flex_shrink_0()
        .bg(rgb(STATUS_BAR_SEPARATOR_COLOR))
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

    use std::{
        collections::BTreeSet,
        fs,
        path::{Path, PathBuf},
        time::Duration,
    };

    use gpui::{
        AppContext, ClickEvent, ClipboardItem, ExternalPaths, Image, ImageFormat,
        KeyboardClickEvent, Modifiers, MouseButton, MouseClickEvent, MouseDownEvent, MouseUpEvent,
        SharedString,
    };

    use crate::explorer::context_menu::{
        ContextMenuIconSlot, ContextMenuItem, ContextMenuSource, ContextMenuState,
    };
    use crate::explorer::{
        DirectoryKind,
        clipboard::{
            FileClipboard, FileClipboardOperation, clipboard_item_can_paste,
            clipboard_item_for_files,
        },
        codebase_summary::{CodebaseLanguageSummary, CodebaseSummary},
        constants::{
            COLUMN_NAME_MIN_WIDTH, COLUMN_TYPE_WIDTH, EMPTY_FOLDER_MESSAGE, EMPTY_FOLDER_TEXT_SIZE,
            EMPTY_FOLDER_TOP_MARGIN, EXPLORER_COPY_GREEN, FILE_ICON_SLOT_WIDTH, MB_BYTES,
            NAV_BUTTON_ACTIVE_OPACITY,
        },
        entry::FileEntry,
        git_status::{GitDivergence, GitRepositoryStatus},
        navigation::DirectoryOpenMode,
        selection::SelectionModifiers,
        sidebar::{SidebarItem, SidebarItemKind},
        test_support::{TempDir, test_view_entity_at_path},
        view::ExplorerView,
    };

    use super::{
        CODEBASE_MAKEUP_BAR_WIDTH, CODEBASE_MAKEUP_SEPARATOR_WIDTH, CONTEXT_MENU_MAX_WIDTH,
        CONTEXT_MENU_MIN_WIDTH, CUT_ITEM_OPACITY, CodebaseMakeupSegment,
        DROP_INDICATOR_TARGET_MAX_WIDTH, FILE_COLUMN_HEADER_HOVER_BG, FILE_SORT_CHEVRON_ICON_SIZE,
        NAME_CELL_LEFT_PADDING, NAME_ICON_TEXT_GAP, RecursiveSearchProgressSnapshot,
        UTILITY_TEXT_BUTTON_ICON_SIZE, UTILITY_TEXT_BUTTON_WIDTH, available_filename_text_width,
        codebase_makeup_segments, context_menu_action_width_for_text_width,
        context_menu_detail_width_for_text_widths, context_menu_text_width, context_menu_width,
        context_menu_width_for_natural_width, copied_directory_address,
        directory_open_mode_for_entry_click, drop_indicator_target_width,
        effective_sidebar_is_visible, effective_sidebar_layout_width, entry_row_hover_enabled,
        filename_text_width, folder_status_summary, format_address_path, git_branch_tooltip,
        git_divergence_label, git_divergence_tooltip, is_alt_entry_double_click,
        is_ctrl_entry_double_click, is_normal_entry_click, lines_of_code_tooltip,
        open_current_folder_context_menu_from_event, recursive_result_text_width,
        search_working_detail, selection_modifiers_for_click, sidebar_auto_hide_is_active,
        sidebar_context_menu_is_active, sidebar_context_menu_target, sidebar_item_is_dragging,
        sidebar_pin_path_from_value, sidebar_row_background_color, sort_indicator_direction,
        text_cell_width,
    };
    use crate::settings::{
        AddressSlash, FileSortColumn, FileSortSettings, SettingsState, SortDirection,
    };

    fn entry_names(view: &ExplorerView) -> Vec<String> {
        view.entries
            .iter()
            .map(|entry| entry.name.clone())
            .collect()
    }

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
    fn file_column_header_hover_and_sort_icon_size_match_design() {
        assert_eq!(FILE_COLUMN_HEADER_HOVER_BG, 0xd9ebf9);
        assert_eq!(FILE_SORT_CHEVRON_ICON_SIZE, 11.0);
    }

    #[test]
    fn sort_indicator_direction_only_matches_active_column() {
        let sort = Some(FileSortSettings {
            column: FileSortColumn::Name,
            direction: SortDirection::Descending,
        });

        assert_eq!(
            sort_indicator_direction(Some(FileSortColumn::Name), sort),
            Some(SortDirection::Descending)
        );
        assert_eq!(
            sort_indicator_direction(Some(FileSortColumn::DateModified), sort),
            None
        );
        assert_eq!(sort_indicator_direction(None, sort), None);
        assert_eq!(
            sort_indicator_direction(Some(FileSortColumn::Name), None),
            None
        );
    }

    #[test]
    fn codebase_makeup_segments_leave_whole_pixel_separator_gaps() {
        let segments =
            codebase_makeup_segments(&[80.0, 30.0, 6.0], &[0x111111, 0x222222, 0x333333]);

        assert_eq!(
            segments,
            vec![
                CodebaseMakeupSegment {
                    left: 0.0,
                    width: 80.0,
                    color: 0x111111,
                    round_left: true,
                    round_right: false,
                },
                CodebaseMakeupSegment {
                    left: 80.0 + CODEBASE_MAKEUP_SEPARATOR_WIDTH,
                    width: 30.0,
                    color: 0x222222,
                    round_left: false,
                    round_right: false,
                },
                CodebaseMakeupSegment {
                    left: 110.0 + (CODEBASE_MAKEUP_SEPARATOR_WIDTH * 2.0),
                    width: 6.0,
                    color: 0x333333,
                    round_left: false,
                    round_right: true,
                },
            ]
        );

        let last = segments.last().expect("last segment");
        assert_eq!(last.left + last.width, CODEBASE_MAKEUP_BAR_WIDTH);
        assert!(segments.iter().all(|segment| segment.left.fract() == 0.0));
        assert!(segments.iter().all(|segment| segment.width.fract() == 0.0));
    }

    #[test]
    fn codebase_makeup_segments_single_language_covers_full_bar() {
        let segments = codebase_makeup_segments(&[CODEBASE_MAKEUP_BAR_WIDTH], &[0x3178c6]);

        assert_eq!(
            segments,
            vec![CodebaseMakeupSegment {
                left: 0.0,
                width: CODEBASE_MAKEUP_BAR_WIDTH,
                color: 0x3178c6,
                round_left: true,
                round_right: true,
            }]
        );
    }

    #[test]
    fn codebase_makeup_segments_round_first_and_last_visible_segments() {
        let segments = codebase_makeup_segments(
            &[0.0, 60.0, 56.0, 0.0],
            &[0x111111, 0x222222, 0x333333, 0x444444],
        );

        assert!(!segments[0].round_left);
        assert!(!segments[0].round_right);
        assert!(segments[1].round_left);
        assert!(!segments[1].round_right);
        assert!(!segments[2].round_left);
        assert!(segments[2].round_right);
        assert!(!segments[3].round_left);
        assert!(!segments[3].round_right);
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
    fn paste_button_availability_accepts_file_and_image_clipboard_payloads() {
        let explorer_item = clipboard_item_for_files(&FileClipboard::new(
            FileClipboardOperation::Copy,
            vec![PathBuf::from("a.txt")],
        ))
        .expect("clipboard item");
        let image = Image::from_bytes(ImageFormat::Png, vec![1, 2, 3]);
        let image_item = ClipboardItem::new_image(&image);
        let plain_item = ClipboardItem::new_string("a.txt".to_owned());

        assert!(clipboard_item_can_paste(Some(&explorer_item)));
        assert!(clipboard_item_can_paste(Some(&image_item)));
        assert!(!clipboard_item_can_paste(Some(&plain_item)));
        assert!(!clipboard_item_can_paste(None));
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
            ExplorerView::new_with_settings_for_test(
                path,
                Some(focus_handle),
                &crate::settings::ExplorerSettings::default(),
            )
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
            ExplorerView::new_with_settings_for_test(
                path,
                Some(focus_handle),
                &crate::settings::ExplorerSettings::default(),
            )
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
        let wsl_drive = SidebarItem {
            label: "Ubuntu".to_owned(),
            path: PathBuf::from("\\\\wsl.localhost\\Ubuntu\\"),
            kind: SidebarItemKind::DriveWsl,
            configured_index: None,
        };

        assert_eq!(
            sidebar_context_menu_target(&custom),
            (path.clone(), Some(3), None, false)
        );
        assert_eq!(
            sidebar_context_menu_target(&builtin_configured),
            (
                path.join("downloads"),
                Some(1),
                Some(DirectoryKind::Downloads),
                false
            )
        );
        assert_eq!(
            sidebar_context_menu_target(&custom_unconfigured),
            (PathBuf::from("/other"), None, None, false)
        );
        assert_eq!(
            sidebar_context_menu_target(&drive),
            (PathBuf::from("/"), None, Some(DirectoryKind::Drive), false)
        );
        assert_eq!(
            sidebar_context_menu_target(&windows_drive),
            (
                PathBuf::from("C:\\"),
                None,
                Some(DirectoryKind::DriveWindows),
                false
            )
        );
        assert_eq!(
            sidebar_context_menu_target(&wsl_drive),
            (
                PathBuf::from("\\\\wsl.localhost\\Ubuntu\\"),
                None,
                Some(DirectoryKind::DriveWsl),
                false
            )
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn sidebar_context_menu_target_marks_visible_mounted_drives_ejectable() {
        let path = if cfg!(target_os = "macos") {
            PathBuf::from("/Volumes/Backup")
        } else {
            PathBuf::from("/media/alex/Backup")
        };
        let drive = SidebarItem {
            label: "Backup".to_owned(),
            path: path.clone(),
            kind: SidebarItemKind::Drive,
            configured_index: None,
        };

        assert_eq!(
            sidebar_context_menu_target(&drive),
            (path, None, Some(DirectoryKind::Drive), true)
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

    #[test]
    fn sidebar_auto_hide_helpers_use_strict_forty_percent_threshold() {
        assert!(!sidebar_auto_hide_is_active(320.0, 800.0));
        assert!(sidebar_auto_hide_is_active(320.0, 799.0));
        assert!(effective_sidebar_is_visible(320.0, 800.0, false));
        assert!(!effective_sidebar_is_visible(320.0, 799.0, false));
        assert!(effective_sidebar_is_visible(320.0, 799.0, true));
        assert_eq!(effective_sidebar_layout_width(320.0, 799.0, false), 0.0);
        assert_eq!(effective_sidebar_layout_width(320.0, 799.0, true), 320.0);
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
    fn sidebar_stays_visible_at_exactly_forty_percent(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            let mut view = ExplorerView::new_with_focus_handle_for_test(path, focus_handle);
            view.sidebar_width = 320.0;
            view
        });

        cx.simulate_resize(gpui::size(gpui::px(800.0), gpui::px(600.0)));
        cx.run_until_parked();

        assert!(cx.debug_bounds("explorer-sidebar").is_some());
        assert!(cx.debug_bounds("explorer-sidebar-resizer").is_some());
        assert!(cx.debug_bounds("utility-sidebar-toggle").is_none());
    }

    #[gpui::test]
    fn sidebar_auto_hides_when_width_exceeds_forty_percent(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            let mut view = ExplorerView::new_with_focus_handle_for_test(path, focus_handle);
            view.sidebar_width = 320.0;
            view
        });

        cx.simulate_resize(gpui::size(gpui::px(799.0), gpui::px(600.0)));
        cx.run_until_parked();

        assert!(cx.debug_bounds("explorer-sidebar").is_none());
        assert!(cx.debug_bounds("explorer-sidebar-resizer").is_none());
        assert!(cx.debug_bounds("utility-sidebar-toggle").is_some());
    }

    #[gpui::test]
    fn hamburger_toggles_sidebar_in_auto_hide_mode(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            let mut view = ExplorerView::new_with_focus_handle_for_test(path, focus_handle);
            view.sidebar_width = 320.0;
            view
        });

        cx.simulate_resize(gpui::size(gpui::px(799.0), gpui::px(600.0)));
        cx.run_until_parked();
        let toggle_position = cx
            .debug_bounds("utility-sidebar-toggle")
            .expect("sidebar toggle bounds")
            .center();

        cx.simulate_mouse_down(toggle_position, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(toggle_position, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();

        assert!(cx.debug_bounds("explorer-sidebar").is_some());
        assert!(cx.debug_bounds("explorer-sidebar-resizer").is_some());
        cx.read_entity(&view, |view, _| assert!(view.sidebar_auto_hide_expanded));

        let toggle_position = cx
            .debug_bounds("utility-sidebar-toggle")
            .expect("sidebar toggle bounds after expanding")
            .center();
        cx.simulate_mouse_down(toggle_position, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(toggle_position, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();

        cx.read_entity(&view, |view, _| assert!(!view.sidebar_auto_hide_expanded));
    }

    #[gpui::test]
    fn resizing_wide_resets_sidebar_auto_hide_override(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            let mut view = ExplorerView::new_with_focus_handle_for_test(path, focus_handle);
            view.sidebar_width = 320.0;
            view
        });

        let window_size = |width: f32| gpui::size(gpui::px(width), gpui::px(600.0));

        cx.simulate_resize(window_size(799.0));
        cx.run_until_parked();
        let toggle_position = cx
            .debug_bounds("utility-sidebar-toggle")
            .expect("sidebar toggle bounds")
            .center();
        cx.simulate_mouse_down(toggle_position, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(toggle_position, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();

        assert!(cx.debug_bounds("explorer-sidebar").is_some());
        cx.read_entity(&view, |view, _| assert!(view.sidebar_auto_hide_expanded));

        cx.simulate_resize(window_size(900.0));
        cx.run_until_parked();

        assert!(cx.debug_bounds("explorer-sidebar").is_some());
        cx.read_entity(&view, |view, _| assert!(!view.sidebar_auto_hide_expanded));

        cx.simulate_resize(window_size(799.0));
        cx.run_until_parked();

        assert!(cx.debug_bounds("utility-sidebar-toggle").is_some());
        cx.read_entity(&view, |view, _| assert!(!view.sidebar_auto_hide_expanded));
    }

    #[gpui::test]
    fn utility_menus_open_when_sidebar_toggle_is_visible(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            let mut view = ExplorerView::new_with_focus_handle_for_test(path, focus_handle);
            view.sidebar_width = 320.0;
            view
        });

        cx.simulate_resize(gpui::size(gpui::px(799.0), gpui::px(600.0)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("utility-sidebar-toggle").is_some());

        let new_position = cx
            .debug_bounds("utility-new")
            .expect("new utility button bounds")
            .center();
        cx.simulate_mouse_down(new_position, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(new_position, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("utility-new-folder").is_some());

        let dismiss_position = gpui::point(gpui::px(650.0), gpui::px(500.0));
        cx.simulate_mouse_down(dismiss_position, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(dismiss_position, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();

        let view_position = cx
            .debug_bounds("utility-view")
            .expect("view utility button bounds")
            .center();
        cx.simulate_mouse_down(view_position, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(view_position, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("utility-large-icons").is_some());
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

    #[gpui::test]
    fn right_clicking_file_column_resizer_restores_and_persists_default_width(
        cx: &mut gpui::TestAppContext,
    ) {
        let mut settings = crate::settings::ExplorerSettings::default();
        settings
            .view
            .file_columns
            .widths
            .insert(crate::settings::FileColumnKind::Type, 312);
        cx.set_global(crate::settings::SettingsState::for_test(settings));
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            let mut view = ExplorerView::new_with_focus_handle_for_test(path, focus_handle);
            view.file_columns
                .widths
                .insert(crate::settings::FileColumnKind::Type, 312);
            view.begin_file_column_resize(crate::settings::FileColumnKind::Type, 312.0);
            view
        });

        cx.run_until_parked();
        let position = cx
            .debug_bounds("explorer-header-type-resizer")
            .expect("type column resizer bounds")
            .center();
        cx.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
        cx.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());

        let default_width =
            crate::settings::default_file_column_width(crate::settings::FileColumnKind::Type);
        cx.read_entity(&view, |view, _| {
            assert_eq!(
                view.file_columns.widths[&crate::settings::FileColumnKind::Type],
                default_width
            );
            assert_eq!(view.file_column_resize_drag, None);
        });
        assert_eq!(
            cx.read(|cx| cx
                .global::<crate::settings::SettingsState>()
                .value
                .view
                .file_columns
                .widths[&crate::settings::FileColumnKind::Type]),
            default_width
        );
    }

    #[gpui::test]
    fn default_name_sort_is_ascending_and_name_header_toggles(cx: &mut gpui::TestAppContext) {
        cx.set_global(SettingsState::for_test(
            crate::settings::ExplorerSettings::default(),
        ));
        let temp = TempDir::new();
        fs::write(temp.path().join("a.txt"), b"a").unwrap();
        fs::write(temp.path().join("b.txt"), b"b").unwrap();
        fs::write(temp.path().join("c.txt"), b"c").unwrap();
        let selected = temp.path().join("a.txt");
        let path = temp.path().to_path_buf();
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_settings_for_test(
                path,
                Some(focus_handle),
                &crate::settings::ExplorerSettings::default(),
            )
        });
        cx.run_until_parked();

        cx.read_entity(&view, |view, _| {
            assert_eq!(entry_names(view), vec!["a.txt", "b.txt", "c.txt"]);
            assert_eq!(
                view.header_file_sort(),
                Some(FileSortSettings {
                    column: FileSortColumn::Name,
                    direction: SortDirection::Ascending,
                })
            );
        });
        let header_bounds = cx
            .debug_bounds("explorer-header-name")
            .expect("name header bounds");
        let chevron_bounds = cx
            .debug_bounds("explorer-header-name-sort-chevron")
            .expect("name sort chevron bounds");
        let resizer_bounds = cx
            .debug_bounds("explorer-header-name-resizer")
            .expect("name header resizer bounds");
        assert!(chevron_bounds.right() <= resizer_bounds.left());
        assert!(chevron_bounds.right() > header_bounds.right() - gpui::px(24.0));
        assert!(
            cx.debug_bounds("explorer-header-size-sort-chevron")
                .is_none()
        );
        cx.update(|_, app| {
            view.update(app, |view, _| view.select_single_path(&selected));
        });

        let position = cx
            .debug_bounds("explorer-header-name")
            .expect("name header bounds")
            .center();
        cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());

        cx.read_entity(&view, |view, _| {
            assert_eq!(entry_names(view), vec!["c.txt", "b.txt", "a.txt"]);
            assert_eq!(view.selected_paths(), vec![selected]);
        });
        assert_eq!(
            cx.read(|cx| cx.global::<SettingsState>().value.view.sort),
            FileSortSettings {
                column: FileSortColumn::Name,
                direction: SortDirection::Descending,
            }
        );
    }

    #[gpui::test]
    fn clicking_size_header_sorts_size_ascending(cx: &mut gpui::TestAppContext) {
        cx.set_global(SettingsState::for_test(
            crate::settings::ExplorerSettings::default(),
        ));
        let temp = TempDir::new();
        fs::write(temp.path().join("small.txt"), b"1").unwrap();
        fs::write(temp.path().join("large.txt"), b"12345").unwrap();
        fs::write(temp.path().join("middle.txt"), b"123").unwrap();
        let path = temp.path().to_path_buf();
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_settings_for_test(
                path,
                Some(focus_handle),
                &crate::settings::ExplorerSettings::default(),
            )
        });
        cx.run_until_parked();

        let position = cx
            .debug_bounds("explorer-header-size")
            .expect("size header bounds")
            .center();
        cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());

        cx.read_entity(&view, |view, _| {
            assert_eq!(
                entry_names(view),
                vec!["small.txt", "middle.txt", "large.txt"]
            );
        });
        assert_eq!(
            cx.read(|cx| cx.global::<SettingsState>().value.view.sort),
            FileSortSettings {
                column: FileSortColumn::Size,
                direction: SortDirection::Ascending,
            }
        );
    }

    #[gpui::test]
    fn clicking_date_header_sorts_date_ascending(cx: &mut gpui::TestAppContext) {
        cx.set_global(SettingsState::for_test(
            crate::settings::ExplorerSettings::default(),
        ));
        let temp = TempDir::new();
        let old = temp.path().join("old.txt");
        let new = temp.path().join("new.txt");
        fs::write(&old, b"old").unwrap();
        fs::write(&new, b"new").unwrap();
        filetime::set_file_mtime(&old, filetime::FileTime::from_unix_time(10, 0)).unwrap();
        filetime::set_file_mtime(&new, filetime::FileTime::from_unix_time(20, 0)).unwrap();
        let path = temp.path().to_path_buf();
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_settings_for_test(
                path,
                Some(focus_handle),
                &crate::settings::ExplorerSettings::default(),
            )
        });
        cx.run_until_parked();

        let position = cx
            .debug_bounds("explorer-header-date-modified")
            .expect("date header bounds")
            .center();
        cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());

        cx.read_entity(&view, |view, _| {
            assert_eq!(entry_names(view), vec!["old.txt", "new.txt"]);
        });
        assert_eq!(
            cx.read(|cx| cx.global::<SettingsState>().value.view.sort),
            FileSortSettings {
                column: FileSortColumn::DateModified,
                direction: SortDirection::Ascending,
            }
        );
    }

    #[gpui::test]
    fn clicking_type_header_sorts_type_and_toggles(cx: &mut gpui::TestAppContext) {
        cx.set_global(SettingsState::for_test(
            crate::settings::ExplorerSettings::default(),
        ));
        let temp = TempDir::new();
        fs::write(temp.path().join("a.md"), b"a").unwrap();
        fs::write(temp.path().join("b.txt"), b"b").unwrap();
        fs::write(temp.path().join("c.exe"), b"c").unwrap();
        let selected = temp.path().join("c.exe");
        let path = temp.path().to_path_buf();
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_settings_for_test(
                path,
                Some(focus_handle),
                &crate::settings::ExplorerSettings::default(),
            )
        });
        cx.run_until_parked();
        cx.update(|_, app| {
            view.update(app, |view, _| view.select_single_path(&selected));
        });

        let position = cx
            .debug_bounds("explorer-header-type")
            .expect("type header bounds")
            .center();
        cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());

        assert!(
            cx.debug_bounds("explorer-header-type-sort-chevron")
                .is_some()
        );
        cx.read_entity(&view, |view, _| {
            assert_eq!(entry_names(view), vec!["c.exe", "a.md", "b.txt"]);
            assert_eq!(view.selected_paths(), vec![selected.clone()]);
            assert_eq!(
                view.header_file_sort(),
                Some(FileSortSettings {
                    column: FileSortColumn::Type,
                    direction: SortDirection::Ascending,
                })
            );
        });
        assert_eq!(
            cx.read(|cx| cx.global::<SettingsState>().value.view.sort),
            FileSortSettings {
                column: FileSortColumn::Type,
                direction: SortDirection::Ascending,
            }
        );

        cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());

        cx.read_entity(&view, |view, _| {
            assert_eq!(entry_names(view), vec!["b.txt", "a.md", "c.exe"]);
            assert_eq!(view.selected_paths(), vec![selected]);
        });
        assert_eq!(
            cx.read(|cx| cx.global::<SettingsState>().value.view.sort),
            FileSortSettings {
                column: FileSortColumn::Type,
                direction: SortDirection::Descending,
            }
        );
    }

    #[gpui::test]
    fn dragging_name_column_resizer_sets_and_persists_manual_width(cx: &mut gpui::TestAppContext) {
        cx.set_global(crate::settings::SettingsState::for_test(
            crate::settings::ExplorerSettings::default(),
        ));
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_focus_handle_for_test(path, focus_handle)
        });

        cx.run_until_parked();
        let start = cx
            .debug_bounds("explorer-header-name-resizer")
            .expect("name column resizer bounds")
            .center();
        let end = gpui::point(start.x + gpui::px(80.0), start.y);
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());

        cx.read_entity(&view, |view, _| {
            assert!(view.file_columns.name_width.is_some_and(|width| {
                width > crate::explorer::constants::COLUMN_NAME_MIN_WIDTH as u32
            }));
            assert_eq!(view.file_column_resize_drag, None);
        });
        assert!(cx.read(|cx| {
            cx.global::<crate::settings::SettingsState>()
                .value
                .view
                .file_columns
                .name_width
                .is_some_and(|width| {
                    width > crate::explorer::constants::COLUMN_NAME_MIN_WIDTH as u32
                })
        }));
    }

    #[gpui::test]
    fn right_clicking_name_column_resizer_restores_auto_width(cx: &mut gpui::TestAppContext) {
        reset_name_column_resizer_with_button(cx, MouseButton::Right);
    }

    #[gpui::test]
    fn middle_clicking_name_column_resizer_restores_auto_width(cx: &mut gpui::TestAppContext) {
        reset_name_column_resizer_with_button(cx, MouseButton::Middle);
    }

    fn reset_name_column_resizer_with_button(cx: &mut gpui::TestAppContext, button: MouseButton) {
        let mut settings = crate::settings::ExplorerSettings::default();
        settings.view.file_columns.name_width = Some(360);
        cx.set_global(crate::settings::SettingsState::for_test(settings));
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            let mut view = ExplorerView::new_with_focus_handle_for_test(path, focus_handle);
            view.file_columns.name_width = Some(360);
            view.begin_name_column_resize(360.0, 360.0);
            view
        });

        cx.run_until_parked();
        let position = cx
            .debug_bounds("explorer-header-name-resizer")
            .expect("name column resizer bounds")
            .center();
        cx.simulate_mouse_down(position, button, Modifiers::default());
        cx.simulate_mouse_up(position, button, Modifiers::default());

        cx.read_entity(&view, |view, _| {
            assert_eq!(view.file_columns.name_width, None);
            assert_eq!(view.file_column_resize_drag, None);
        });
        assert_eq!(
            cx.read(|cx| cx
                .global::<crate::settings::SettingsState>()
                .value
                .view
                .file_columns
                .name_width),
            None
        );
    }

    #[gpui::test]
    fn middle_clicking_file_column_resizers_restores_and_persists_default_widths(
        cx: &mut gpui::TestAppContext,
    ) {
        let custom_widths = [
            (crate::settings::FileColumnKind::DateModified, 333),
            (crate::settings::FileColumnKind::Type, 312),
            (crate::settings::FileColumnKind::Size, 222),
        ];
        let mut settings = crate::settings::ExplorerSettings::default();
        for (kind, width) in custom_widths {
            settings.view.file_columns.widths.insert(kind, width);
        }
        cx.set_global(crate::settings::SettingsState::for_test(settings));
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            let mut view = ExplorerView::new_with_focus_handle_for_test(path, focus_handle);
            for (kind, width) in custom_widths {
                view.file_columns.widths.insert(kind, width);
            }
            view
        });

        cx.run_until_parked();
        for selector in [
            "explorer-header-date-modified-resizer",
            "explorer-header-type-resizer",
            "explorer-header-size-resizer",
        ] {
            let position = cx
                .debug_bounds(selector)
                .expect("file column resizer bounds")
                .center();
            cx.simulate_mouse_down(position, MouseButton::Middle, Modifiers::default());
            cx.simulate_mouse_up(position, MouseButton::Middle, Modifiers::default());
        }

        cx.read_entity(&view, |view, _| {
            for (kind, _) in custom_widths {
                assert_eq!(
                    view.file_columns.widths[&kind],
                    crate::settings::default_file_column_width(kind)
                );
            }
            assert_eq!(view.file_column_resize_drag, None);
        });
        for (kind, _) in custom_widths {
            assert_eq!(
                cx.read(|cx| cx
                    .global::<crate::settings::SettingsState>()
                    .value
                    .view
                    .file_columns
                    .widths[&kind]),
                crate::settings::default_file_column_width(kind)
            );
        }
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

    #[gpui::test]
    fn disabled_icon_button_still_shows_tooltip(cx: &mut gpui::TestAppContext) {
        cx.set_global(crate::settings::SettingsState::for_test(
            crate::settings::ExplorerSettings::default(),
        ));
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_focus_handle_for_test(path, focus_handle)
        });

        cx.run_until_parked();
        let position = cx
            .debug_bounds("back")
            .expect("back nav button bounds")
            .center();
        cx.simulate_mouse_move(position, Option::<MouseButton>::None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(280));
        cx.run_until_parked();

        assert!(cx.debug_bounds("explorer-tooltip").is_some());
    }

    #[gpui::test]
    fn icon_button_tooltip_appears_below_and_right_of_cursor(cx: &mut gpui::TestAppContext) {
        cx.set_global(crate::settings::SettingsState::for_test(
            crate::settings::ExplorerSettings::default(),
        ));
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_focus_handle_for_test(path, focus_handle)
        });

        cx.run_until_parked();
        let position = cx
            .debug_bounds("utility-paste")
            .expect("paste utility button bounds")
            .center();
        cx.simulate_mouse_move(position, Option::<MouseButton>::None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(280));
        cx.run_until_parked();

        let tooltip = cx
            .debug_bounds("explorer-tooltip")
            .expect("tooltip bounds after hover delay");
        assert!(tooltip.origin.x >= position.x + gpui::px(12.0));
        assert!(tooltip.origin.y >= position.y + gpui::px(18.0));
    }

    #[test]
    fn copied_directory_address_formats_platform_path() {
        #[cfg(target_os = "windows")]
        {
            assert_eq!(
                copied_directory_address(Path::new(r"C:\Users\Ada\Documents")),
                "C:/Users/Ada/Documents"
            );
            assert_eq!(
                copied_directory_address(Path::new(r"\\server\share\dir")),
                "//server/share/dir"
            );
        }

        #[cfg(not(target_os = "windows"))]
        {
            assert_eq!(
                copied_directory_address(Path::new("/Users/Ada/Documents")),
                "/Users/Ada/Documents"
            );
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn address_path_formatting_supports_backslashes_on_windows() {
        assert_eq!(
            format_address_path(Path::new(r"C:/Users/Ada/Documents"), AddressSlash::Back),
            r"C:\Users\Ada\Documents"
        );
        assert_eq!(
            format_address_path(Path::new(r"//server/share/dir"), AddressSlash::Back),
            r"\\server\share\dir"
        );
    }

    #[gpui::test]
    fn directory_copy_address_button_copies_current_path_without_editing(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let expected = copied_directory_address(&path);
        let (view, cx) = test_view_entity_at_path(cx, path);

        cx.run_until_parked();
        let position = cx
            .debug_bounds("directory-copy-address")
            .expect("directory copy address button bounds")
            .center();
        cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();

        assert_eq!(
            cx.read(|cx| cx.read_from_clipboard().and_then(|item| item.text())),
            Some(expected)
        );
        cx.read_entity(&view, |view, _| assert!(!view.address_bar_is_editing()));
        assert!(cx.debug_bounds("directory-bar").is_some());
        assert!(cx.debug_bounds("directory-bar-input").is_none());
    }

    #[gpui::test]
    fn directory_copy_address_button_fade_restarts_on_address_bar_hover(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (view, cx) = test_view_entity_at_path(cx, path);

        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(!view.directory_bar_hovered);
            assert_eq!(view.directory_bar_hover_generation, 0);
        });

        let directory_position = cx
            .debug_bounds("directory-bar")
            .expect("directory bar bounds")
            .center();
        cx.simulate_mouse_move(
            directory_position,
            Option::<MouseButton>::None,
            Modifiers::default(),
        );
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(view.directory_bar_hovered);
            assert_eq!(view.directory_bar_hover_generation, 1);
        });

        let outside_position = cx
            .debug_bounds("back")
            .expect("back button bounds")
            .center();
        cx.simulate_mouse_move(
            outside_position,
            Option::<MouseButton>::None,
            Modifiers::default(),
        );
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(!view.directory_bar_hovered);
            assert_eq!(view.directory_bar_hover_generation, 1);
        });

        cx.simulate_mouse_move(
            directory_position,
            Option::<MouseButton>::None,
            Modifiers::default(),
        );
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(view.directory_bar_hovered);
            assert_eq!(view.directory_bar_hover_generation, 2);
        });
    }

    #[cfg(target_os = "windows")]
    #[gpui::test]
    fn directory_copy_address_button_uses_configured_backslashes_on_windows(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let expected = format_address_path(&path, AddressSlash::Back);
        let (view, cx) = test_view_entity_at_path(cx, path);

        cx.update(|_, app| {
            view.update(app, |view, _| {
                view.address_slash = AddressSlash::Back;
            });
        });
        cx.run_until_parked();
        let position = cx
            .debug_bounds("directory-copy-address")
            .expect("directory copy address button bounds")
            .center();
        cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();

        assert_eq!(
            cx.read(|cx| cx.read_from_clipboard().and_then(|item| item.text())),
            Some(expected)
        );
        cx.read_entity(&view, |view, _| assert!(!view.address_bar_is_editing()));
    }

    #[gpui::test]
    fn directory_copy_address_button_is_hidden_while_editing(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (view, cx) = test_view_entity_at_path(cx, path);

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                assert!(view.start_address_bar_edit(window, cx));
                cx.notify();
            });
        });
        cx.run_until_parked();

        cx.read_entity(&view, |view, _| assert!(view.address_bar_is_editing()));
        assert!(cx.debug_bounds("directory-copy-address").is_none());
        assert!(cx.debug_bounds("directory-bar-input").is_some());
    }

    #[gpui::test]
    fn git_branch_status_shows_tooltip(cx: &mut gpui::TestAppContext) {
        assert_status_bar_tooltip(cx, "git-branch-status");
    }

    #[gpui::test]
    fn git_divergence_status_shows_tooltip(cx: &mut gpui::TestAppContext) {
        assert_status_bar_tooltip(cx, "git-divergence-status");
    }

    #[gpui::test]
    fn codebase_lines_of_code_status_shows_tooltip(cx: &mut gpui::TestAppContext) {
        assert_status_bar_tooltip(cx, "codebase-lines-of-code");
    }

    #[gpui::test]
    fn codebase_first_language_segment_shows_tooltip(cx: &mut gpui::TestAppContext) {
        assert_status_bar_tooltip(cx, "codebase-makeup-segment-0");
    }

    #[gpui::test]
    fn codebase_second_language_segment_shows_tooltip(cx: &mut gpui::TestAppContext) {
        assert_status_bar_tooltip(cx, "codebase-makeup-segment-1");
    }

    #[gpui::test]
    fn view_menu_large_icons_updates_global_view_mode(cx: &mut gpui::TestAppContext) {
        cx.set_global(crate::settings::SettingsState::for_test(
            crate::settings::ExplorerSettings::default(),
        ));
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_focus_handle_for_test(path, focus_handle)
        });

        cx.run_until_parked();
        let view_position = cx
            .debug_bounds("utility-view")
            .expect("view utility button bounds")
            .center();
        cx.simulate_mouse_down(view_position, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(view_position, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();

        let large_icons_position = cx
            .debug_bounds("utility-large-icons")
            .expect("large icons menu row bounds")
            .center();
        cx.simulate_mouse_down(
            large_icons_position,
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_up(
            large_icons_position,
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.run_until_parked();

        assert_eq!(
            cx.read(|cx| cx
                .global::<crate::settings::SettingsState>()
                .value
                .view
                .mode),
            crate::settings::FileViewMode::LargeIcons
        );
    }

    #[gpui::test]
    fn view_menu_folder_sizes_updates_global_setting(cx: &mut gpui::TestAppContext) {
        cx.set_global(crate::settings::SettingsState::for_test(
            crate::settings::ExplorerSettings::default(),
        ));
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_focus_handle_for_test(path, focus_handle)
        });

        cx.run_until_parked();
        let view_position = cx
            .debug_bounds("utility-view")
            .expect("view utility button bounds")
            .center();
        cx.simulate_mouse_down(view_position, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(view_position, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();

        let extensions_bounds = cx
            .debug_bounds("utility-file-name-extensions")
            .expect("file name extensions menu row bounds");
        let folder_sizes_bounds = cx
            .debug_bounds("utility-folder-sizes")
            .expect("folder sizes menu row bounds");
        assert!(folder_sizes_bounds.origin.y < extensions_bounds.origin.y);

        let folder_sizes_position = folder_sizes_bounds.center();
        cx.simulate_mouse_down(
            folder_sizes_position,
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_up(
            folder_sizes_position,
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.run_until_parked();

        assert!(cx.read(|cx| {
            cx.global::<crate::settings::SettingsState>()
                .value
                .view
                .show_folder_sizes
        }));
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
    fn name_text_width_uses_name_column_width() {
        let name_column_width = 400.0;

        assert_eq!(
            available_filename_text_width(name_column_width),
            name_column_width - NAME_CELL_LEFT_PADDING - FILE_ICON_SLOT_WIDTH - NAME_ICON_TEXT_GAP
        );
    }

    #[test]
    fn name_text_width_respects_name_column_minimum() {
        assert_eq!(
            available_filename_text_width(COLUMN_NAME_MIN_WIDTH),
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
    fn alt_entry_double_click_detects_left_mouse_double_click() {
        let event = ClickEvent::Mouse(MouseClickEvent {
            down: MouseDownEvent {
                button: MouseButton::Left,
                ..MouseDownEvent::default()
            },
            up: MouseUpEvent {
                modifiers: Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
                ..MouseUpEvent::default()
            },
        });

        assert!(is_alt_entry_double_click(&event, 2));
    }

    #[test]
    fn alt_entry_double_click_rejects_plain_double_click() {
        let event = ClickEvent::Mouse(MouseClickEvent {
            down: MouseDownEvent {
                button: MouseButton::Left,
                ..MouseDownEvent::default()
            },
            up: MouseUpEvent::default(),
        });

        assert!(!is_alt_entry_double_click(&event, 2));
    }

    #[test]
    fn alt_entry_double_click_rejects_single_click() {
        let event = ClickEvent::Mouse(MouseClickEvent {
            down: MouseDownEvent {
                button: MouseButton::Left,
                ..MouseDownEvent::default()
            },
            up: MouseUpEvent {
                modifiers: Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
                ..MouseUpEvent::default()
            },
        });

        assert!(!is_alt_entry_double_click(&event, 1));
    }

    #[test]
    fn alt_entry_double_click_rejects_non_left_clicks() {
        for button in [MouseButton::Middle, MouseButton::Right] {
            let event = ClickEvent::Mouse(MouseClickEvent {
                down: MouseDownEvent {
                    button,
                    ..MouseDownEvent::default()
                },
                up: MouseUpEvent {
                    modifiers: Modifiers {
                        alt: true,
                        ..Modifiers::default()
                    },
                    ..MouseUpEvent::default()
                },
            });

            assert!(!is_alt_entry_double_click(&event, 2));
        }
    }

    #[test]
    fn ctrl_entry_double_click_detects_left_mouse_double_click() {
        let event = ClickEvent::Mouse(MouseClickEvent {
            down: MouseDownEvent {
                button: MouseButton::Left,
                ..MouseDownEvent::default()
            },
            up: MouseUpEvent {
                modifiers: Modifiers {
                    control: true,
                    ..Modifiers::default()
                },
                ..MouseUpEvent::default()
            },
        });

        assert!(is_ctrl_entry_double_click(&event, 2));
        assert_eq!(
            directory_open_mode_for_entry_click(&event, 2),
            DirectoryOpenMode::NewTab
        );
    }

    #[test]
    fn ctrl_entry_double_click_rejects_plain_or_single_clicks() {
        let plain = ClickEvent::Mouse(MouseClickEvent {
            down: MouseDownEvent {
                button: MouseButton::Left,
                ..MouseDownEvent::default()
            },
            up: MouseUpEvent::default(),
        });
        let ctrl = ClickEvent::Mouse(MouseClickEvent {
            down: MouseDownEvent {
                button: MouseButton::Left,
                ..MouseDownEvent::default()
            },
            up: MouseUpEvent {
                modifiers: Modifiers {
                    control: true,
                    ..Modifiers::default()
                },
                ..MouseUpEvent::default()
            },
        });

        assert!(!is_ctrl_entry_double_click(&plain, 2));
        assert_eq!(
            directory_open_mode_for_entry_click(&plain, 2),
            DirectoryOpenMode::CurrentTab
        );
        assert!(!is_ctrl_entry_double_click(&ctrl, 1));
        assert_eq!(
            directory_open_mode_for_entry_click(&ctrl, 1),
            DirectoryOpenMode::CurrentTab
        );
    }

    #[test]
    fn ctrl_entry_double_click_rejects_non_left_clicks() {
        for button in [MouseButton::Middle, MouseButton::Right] {
            let event = ClickEvent::Mouse(MouseClickEvent {
                down: MouseDownEvent {
                    button,
                    ..MouseDownEvent::default()
                },
                up: MouseUpEvent {
                    modifiers: Modifiers {
                        control: true,
                        ..Modifiers::default()
                    },
                    ..MouseUpEvent::default()
                },
            });

            assert!(!is_ctrl_entry_double_click(&event, 2));
        }
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

    #[test]
    fn git_divergence_label_separates_outgoing_and_incoming_counts() {
        assert_eq!(
            git_divergence_label(GitDivergence {
                outgoing: 1_234,
                incoming: 56,
            }),
            "1,234 / 56"
        );
    }

    #[test]
    fn status_bar_tooltip_labels_use_requested_wording_and_count_formatting() {
        assert_eq!(
            git_branch_tooltip("main"),
            SharedString::from("Current Branch: main")
        );
        assert_eq!(
            git_divergence_tooltip(GitDivergence {
                outgoing: 1_234,
                incoming: 56,
            }),
            SharedString::from("1,234 outgoing / 56 incoming commits")
        );
        assert_eq!(
            lines_of_code_tooltip(12_345),
            SharedString::from("12,345 Lines of Code")
        );
    }

    fn assert_status_bar_tooltip(cx: &mut gpui::TestAppContext, selector: &'static str) {
        cx.set_global(crate::settings::SettingsState::for_test(
            crate::settings::ExplorerSettings::default(),
        ));
        let temp = TempDir::new();
        let path = temp.path().to_path_buf();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            let mut view = ExplorerView::new_with_focus_handle_for_test(path.clone(), focus_handle);
            view.git_status = Some(GitRepositoryStatus {
                repo_root: path.clone(),
                branch: "main".to_owned(),
                divergence: Some(GitDivergence {
                    outgoing: 12,
                    incoming: 3,
                }),
            });
            view.codebase_summary = Some(CodebaseSummary {
                repo_root: path,
                total_code: 12_345,
                languages: vec![
                    CodebaseLanguageSummary {
                        name: "Rust".to_owned(),
                        code: 10_000,
                        percentage: 81,
                        color: 0xde3c10,
                    },
                    CodebaseLanguageSummary {
                        name: "TOML".to_owned(),
                        code: 2_345,
                        percentage: 19,
                        color: 0x9c4221,
                    },
                ],
            });
            view
        });

        cx.run_until_parked();
        hover_selector_until_tooltip(cx, selector);
    }

    fn hover_selector_until_tooltip(cx: &mut gpui::VisualTestContext, selector: &'static str) {
        assert!(
            cx.debug_bounds("explorer-tooltip").is_none(),
            "tooltip should be hidden before hovering {selector}"
        );
        let position = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} bounds"))
            .center();
        cx.simulate_mouse_move(position, Option::<MouseButton>::None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(280));
        cx.run_until_parked();

        assert!(
            cx.debug_bounds("explorer-tooltip").is_some(),
            "{selector} should show tooltip after hover delay"
        );
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
