use std::ops::Range;

use gpui::{
    AnyElement, App, ClickEvent, Context, Div, FocusHandle, Focusable, IntoElement, MouseButton,
    MouseDownEvent, NavigationDirection, Render, ScrollWheelEvent, SharedString, TextRun, Window,
    div, font, prelude::*, px, rgb, uniform_list,
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
        SCROLLBAR_GUTTER_WIDTH,
    },
    entry::FileEntry,
    formatting::{format_modified, format_size},
    icons::{NavIcon, device_px, device_px_value, file_icon, folder_icon, nav_icon_font},
    navigation::{EntryAction, HistoryMode},
    scrollbar::scrollbar_header_spacer,
    view::{ExplorerContentBranch, ExplorerView},
};

const NAME_CELL_LEFT_PADDING: f32 = 16.0;
const NAME_ICON_TEXT_GAP_PHYSICAL: f32 = 8.0;
const NAME_TEXT_SIZE: f32 = 12.0;
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
            .child(name_cell(&entry, scale_factor, window))
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
                    ),
            )
            .child(self.render_scrollbar(cx))
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

fn name_cell(entry: &FileEntry, scale_factor: f32, window: &Window) -> Div {
    let text_width =
        available_filename_text_width(f32::from(window.bounds().size.width), scale_factor);
    let filename = truncated_filename(&entry.name, text_width, window);

    div()
        .flex()
        .items_center()
        .h_full()
        .flex_1()
        .min_w(px(COLUMN_NAME_MIN_WIDTH))
        .overflow_hidden()
        .pl(px(NAME_CELL_LEFT_PADDING))
        .child(if entry.is_dir {
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

fn truncated_filename(name: &str, available_width: f32, window: &Window) -> SharedString {
    let name_font = font(".SystemUIFont");
    let mut runs = vec![TextRun {
        len: name.len(),
        font: name_font.clone(),
        color: rgb(0x000000).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }];

    window
        .text_system()
        .line_wrapper(name_font, px(NAME_TEXT_SIZE))
        .truncate_line(
            SharedString::from(name.to_owned()),
            px(available_width),
            NAME_TRUNCATION_SUFFIX,
            &mut runs,
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

#[cfg(test)]
mod tests {

    use crate::explorer::constants::{
        COLUMN_DATE_WIDTH, COLUMN_NAME_MIN_WIDTH, COLUMN_SIZE_WIDTH, COLUMN_TYPE_WIDTH,
        EMPTY_FOLDER_MESSAGE, EMPTY_FOLDER_TEXT_SIZE, EMPTY_FOLDER_TOP_MARGIN,
        FILE_ICON_SLOT_WIDTH_PHYSICAL, NAV_BUTTON_ACTIVE_OPACITY, SCROLLBAR_GUTTER_WIDTH,
    };

    use super::{
        NAME_CELL_LEFT_PADDING, NAME_ICON_TEXT_GAP_PHYSICAL, available_filename_text_width,
        filename_text_width,
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
}
