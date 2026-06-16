use std::{
    collections::BTreeMap,
    fs,
    io::BufReader,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use filetime::{FileTime, set_file_times};
use gpui::{
    AnyElement, AnyWindowHandle, App, ClickEvent, ClipboardItem, Context, FocusHandle, Focusable,
    Image, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Render,
    ScrollHandle, ScrollWheelEvent, SharedString, Task, TextRun, TitlebarOptions, WeakEntity,
    Window, WindowBounds, WindowDecorations, WindowKind, WindowOptions, canvas, div, point,
    prelude::*, px, rgb, size,
};
use thousands::Separable;

#[cfg(not(target_os = "windows"))]
use crate::explorer::open_with::OpenWithOutcome;
use crate::explorer::{
    DialogCancel, DialogConfirm,
    constants::{
        SCROLLBAR_ARROW_HEIGHT, SCROLLBAR_GUTTER_WIDTH, SCROLLBAR_THUMB_ACTIVE_BG,
        SCROLLBAR_THUMB_BG, SCROLLBAR_THUMB_HOVER_BG, SCROLLBAR_THUMB_HOVER_WIDTH,
        SCROLLBAR_THUMB_WIDTH, SCROLLBAR_TRACK_BG,
    },
    entry::{DirectoryLinkKind, EntryKind},
    formatting::{format_size, format_timestamp},
    icons::{
        copy_file_dialog_icon_sized, directory_shortcut_icon_sized, file_icon_for_path_sized,
        folder_icon_sized, image_icon,
    },
    open_with::{DefaultApplication, default_application_for_file},
    scrollbar::{ScrollbarArrow, ScrollbarDrag, ScrollbarMetrics, scrollbar_arrow_button},
    view::ExplorerView,
};
use crate::settings::SettingsState;

const PROPERTIES_WIDTH: f32 = 408.0;
const PROPERTIES_HEIGHT: f32 = 520.0;
const PROPERTIES_PADDING: f32 = 10.0;
const PROPERTIES_PANEL_PADDING: f32 = 20.0;
const PROPERTIES_TAB_HEIGHT: f32 = 22.0;
const PROPERTIES_TAB_HORIZONTAL_PADDING: f32 = 12.0;
const PROPERTIES_BORDER_WIDTH: f32 = 1.0;
const PROPERTIES_ROW_HEIGHT: f32 = 24.0;
const PROPERTIES_BUTTON_HEIGHT: f32 = 28.0;
const PROPERTIES_BUTTON_MIN_WIDTH: f32 = 78.0;
const PROPERTIES_LABEL_WIDTH: f32 = 108.0;
const PROPERTIES_ITEM_ICON_SIZE: f32 = 32.0;
const PROPERTIES_OPEN_WITH_ICON_SIZE: f32 = 20.0;
const PROPERTIES_BORDER: u32 = 0xe5e5e5;
const PROPERTIES_MUTED_TEXT: u32 = 0x666666;
const PROPERTIES_GROUP_TITLE: u32 = 0x003399;
const PROPERTIES_ROW_TYPE_ID: &str = "properties-property-row-type";
const PROPERTIES_ROW_LOCATION_ID: &str = "properties-property-row-location";
const PROPERTIES_ROW_SIZE_ID: &str = "properties-property-row-size";
const PROPERTIES_ROW_SIZE_ON_DISK_ID: &str = "properties-property-row-size-on-disk";
const PROPERTIES_ROW_CONTAINS_ID: &str = "properties-property-row-contains";
const PROPERTIES_ROW_CREATED_ID: &str = "properties-property-row-created";
const PROPERTIES_ROW_MODIFIED_ID: &str = "properties-property-row-modified";
const PROPERTIES_ROW_ACCESSED_ID: &str = "properties-property-row-accessed";
#[cfg(test)]
const PROPERTIES_GENERAL_PROPERTY_ROW_IDS: &[&str] = &[
    PROPERTIES_ROW_TYPE_ID,
    PROPERTIES_ROW_LOCATION_ID,
    PROPERTIES_ROW_SIZE_ID,
    PROPERTIES_ROW_SIZE_ON_DISK_ID,
    PROPERTIES_ROW_CONTAINS_ID,
    PROPERTIES_ROW_CREATED_ID,
    PROPERTIES_ROW_MODIFIED_ID,
    PROPERTIES_ROW_ACCESSED_ID,
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PropertyTarget {
    pub(super) paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PropertySnapshot {
    pub(super) target: PropertyTarget,
    pub(super) title: String,
    pub(super) item_count: usize,
    pub(super) item_kind: PropertyItemKind,
    pub(super) type_label: MixedValue<String>,
    pub(super) location: MixedValue<String>,
    pub(super) size: u64,
    pub(super) size_on_disk: u64,
    pub(super) contains: Option<PropertyContains>,
    pub(super) selection_counts: Option<PropertyContains>,
    pub(super) created: MixedValue<SystemTime>,
    pub(super) modified: MixedValue<SystemTime>,
    pub(super) accessed: MixedValue<SystemTime>,
    pub(super) attributes: PropertyAttributes,
    pub(super) owner: MixedValue<String>,
    pub(super) group: MixedValue<String>,
    pub(super) unix_mode: MixedValue<u32>,
    pub(super) permission_summary: MixedValue<String>,
    pub(super) default_app: Option<PropertyDefaultApp>,
    pub(super) shortcut: Option<ShortcutDetails>,
    pub(super) details: Vec<PropertyDetailGroup>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct EditablePropertyDraft {
    pub(super) modified: Option<SystemTime>,
    pub(super) accessed: Option<SystemTime>,
    pub(super) readonly: Option<bool>,
    pub(super) hidden: Option<bool>,
    pub(super) unix_mode: Option<u32>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct PropertyApplyOutcome {
    pub(super) changed: usize,
    pub(super) errors: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum MixedValue<T> {
    None,
    Single(T),
    Mixed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PropertyItemKind {
    SingleFile,
    SingleFolder,
    SingleShortcut,
    MultipleFiles,
    MultipleFolders,
    MultipleItems,
    Missing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PropertyContains {
    pub(super) files: usize,
    pub(super) folders: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PropertyAttributes {
    pub(super) readonly: MixedValue<bool>,
    pub(super) hidden: MixedValue<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PropertyDefaultApp {
    pub(super) name: String,
    pub(super) path: Option<PathBuf>,
}

impl From<DefaultApplication> for PropertyDefaultApp {
    fn from(value: DefaultApplication) -> Self {
        Self {
            name: value.name,
            path: value.path,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ShortcutDetails {
    pub(super) target: String,
    pub(super) target_type: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PropertyDetailGroup {
    pub(super) kind: PropertyDetailGroupKind,
    pub(super) title: String,
    pub(super) details: Vec<PropertyDetail>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum PropertyDetailGroupKind {
    File,
    Camera,
    Exposure,
    Gps,
    Misc,
    NonStandard,
}

impl PropertyDetailGroupKind {
    fn title(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Camera => "camera",
            Self::Exposure => "exposure",
            Self::Gps => "gps",
            Self::Misc => "misc",
            Self::NonStandard => "non-standard",
        }
    }
}

const PROPERTY_DETAIL_GROUP_ORDER: &[PropertyDetailGroupKind] = &[
    PropertyDetailGroupKind::File,
    PropertyDetailGroupKind::Camera,
    PropertyDetailGroupKind::Exposure,
    PropertyDetailGroupKind::Gps,
    PropertyDetailGroupKind::Misc,
    PropertyDetailGroupKind::NonStandard,
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PropertyDetail {
    pub(super) name: String,
    pub(super) value: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PropertyTab {
    General,
    Details,
}

const PROPERTY_TABS: &[(PropertyTab, &str)] = &[
    (PropertyTab::General, "General"),
    (PropertyTab::Details, "Details"),
];

#[derive(Clone, Debug, Eq, PartialEq)]
enum PropertySnapshotState {
    Loading,
    Ready(PropertySnapshot),
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PropertyDetailsState {
    NotStarted,
    Loading,
    Ready(Vec<PropertyDetailGroup>),
}

pub(super) struct PropertiesDialog {
    target: PropertyTarget,
    explorer: WeakEntity<ExplorerView>,
    date_format: String,
    font: gpui::Font,
    focus_handle: FocusHandle,
    active_tab: PropertyTab,
    snapshot_state: PropertySnapshotState,
    details_state: PropertyDetailsState,
    details_generation: u64,
    details_scroll_handle: ScrollHandle,
    details_scrollbar_hovered: bool,
    details_scrollbar_drag: Option<ScrollbarDrag>,
    snapshot_task: Option<Task<()>>,
    details_task: Option<Task<()>>,
    apply_task: Option<Task<()>>,
    #[cfg(not(target_os = "windows"))]
    default_app_task: Option<Task<()>>,
    draft: EditablePropertyDraft,
    apply_error: Option<String>,
    #[cfg(not(target_os = "windows"))]
    default_app_error: Option<String>,
    completed: bool,
}

impl ExplorerView {
    pub(super) fn open_selected_properties(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.commit_active_rename_before_interaction(window, cx) {
            return;
        }

        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }

        self.close_context_menu();
        self.open_utility_menu = None;
        match open_properties_window(
            PropertyTarget { paths },
            cx.entity(),
            self.date_format.clone(),
            cx,
        ) {
            Ok(_) => self.open_error = None,
            Err(error) => self.open_error = Some(format!("Failed to open Properties: {error}")),
        }
    }

    pub(super) fn handle_open_properties(
        &mut self,
        _: &crate::explorer::OpenProperties,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_selected_properties(window, cx);
        cx.notify();
    }
}

impl PropertiesDialog {
    fn new(
        target: PropertyTarget,
        explorer: WeakEntity<ExplorerView>,
        date_format: String,
        focus_handle: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let font = crate::settings::current_app_font(cx);
        let mut dialog = Self {
            target,
            explorer,
            date_format,
            font,
            focus_handle,
            active_tab: PropertyTab::General,
            snapshot_state: PropertySnapshotState::Loading,
            details_state: PropertyDetailsState::NotStarted,
            details_generation: 0,
            details_scroll_handle: ScrollHandle::new(),
            details_scrollbar_hovered: false,
            details_scrollbar_drag: None,
            snapshot_task: None,
            details_task: None,
            apply_task: None,
            #[cfg(not(target_os = "windows"))]
            default_app_task: None,
            draft: EditablePropertyDraft::default(),
            apply_error: None,
            #[cfg(not(target_os = "windows"))]
            default_app_error: None,
            completed: false,
        };
        dialog.start_snapshot_task(cx);
        cx.observe_global::<SettingsState>(|this, cx| {
            this.font = crate::settings::current_app_font(cx);
            cx.notify();
        })
        .detach();
        dialog
    }

    fn start_snapshot_task(&mut self, cx: &mut Context<Self>) {
        self.snapshot_state = PropertySnapshotState::Loading;
        self.reset_details_state();
        let target = self.target.clone();
        let date_format = self.date_format.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(
                    async move { collect_property_snapshot_with_date_format(target, &date_format) },
                )
                .await;

            let _ = this.update(cx, |dialog, cx| {
                match result {
                    Ok(snapshot) => {
                        dialog.set_ready_snapshot(snapshot, cx);
                    }
                    Err(error) => dialog.snapshot_state = PropertySnapshotState::Failed(error),
                }
                cx.notify();
            });
        });
        self.snapshot_task = Some(task);
    }

    fn reset_details_state(&mut self) {
        self.details_generation = self.details_generation.wrapping_add(1);
        self.details_state = PropertyDetailsState::NotStarted;
        self.details_task = None;
        self.details_scrollbar_drag = None;
        self.set_details_scroll_top(0.0);
    }

    fn set_ready_snapshot(&mut self, snapshot: PropertySnapshot, cx: &mut Context<Self>) {
        self.draft = EditablePropertyDraft::from_snapshot(&snapshot);
        self.snapshot_state = PropertySnapshotState::Ready(snapshot);
        self.reset_details_state();
        if self.active_tab == PropertyTab::Details {
            self.start_details_task(cx);
        }
    }

    fn start_details_task(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.details_state, PropertyDetailsState::NotStarted) {
            return;
        }
        let PropertySnapshotState::Ready(snapshot) = &self.snapshot_state else {
            return;
        };
        if !snapshot
            .target
            .paths
            .iter()
            .any(|path| path_may_have_exif(path))
        {
            self.details_state = PropertyDetailsState::Ready(Vec::new());
            return;
        }

        self.details_state = PropertyDetailsState::Loading;
        let target = snapshot.target.clone();
        let generation = self.details_generation;
        let task = cx.spawn(async move |this, cx| {
            let groups = cx
                .background_executor()
                .spawn(async move { collect_exif_detail_groups(&target) })
                .await;

            let _ = this.update(cx, |dialog, cx| {
                if dialog.details_generation == generation {
                    dialog.details_task = None;
                    dialog.details_state = PropertyDetailsState::Ready(groups);
                    cx.notify();
                }
            });
        });
        self.details_task = Some(task);
    }

    fn handle_cancel(&mut self, _: &DialogCancel, window: &mut Window, cx: &mut Context<Self>) {
        self.close(window, cx);
    }

    fn handle_confirm(&mut self, _: &DialogConfirm, window: &mut Window, cx: &mut Context<Self>) {
        if self.has_changes() {
            self.apply_changes(false, Some(window.window_handle()), cx);
        } else {
            self.close(window, cx);
        }
    }

    fn close(&mut self, window: &mut Window, _: &mut Context<Self>) {
        self.completed = true;
        window.remove_window();
    }

    fn release(&mut self, _: &mut App) {
        self.completed = true;
    }

    fn has_changes(&self) -> bool {
        let PropertySnapshotState::Ready(snapshot) = &self.snapshot_state else {
            return false;
        };
        self.draft != EditablePropertyDraft::from_snapshot(snapshot)
    }

    fn apply_changes(
        &mut self,
        close_on_success: bool,
        window_handle: Option<AnyWindowHandle>,
        cx: &mut Context<Self>,
    ) {
        let PropertySnapshotState::Ready(snapshot) = &self.snapshot_state else {
            return;
        };
        let plan = property_apply_plan(snapshot, &self.draft);
        if property_apply_plan_is_empty(&plan) {
            return;
        }

        self.apply_error = None;
        let target = snapshot.target.clone();
        let explorer = self.explorer.clone();
        let date_format = self.date_format.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let outcome = apply_property_draft(&target.paths, &plan);
                    let snapshot =
                        collect_property_snapshot_with_date_format(target, &date_format).ok();
                    (outcome, snapshot)
                })
                .await;

            let _ = this.update(cx, |dialog, cx| {
                let (outcome, snapshot) = result;
                dialog.apply_task = None;
                if outcome.errors.is_empty() {
                    dialog.apply_error = None;
                } else {
                    dialog.apply_error = Some(outcome.errors.join("\n"));
                }
                if let Some(snapshot) = snapshot {
                    dialog.set_ready_snapshot(snapshot, cx);
                }
                let _ = explorer.update(cx, |explorer, cx| {
                    explorer.refresh_with_entry_metadata_resolution(cx);
                    cx.notify();
                });
                if close_on_success && outcome.errors.is_empty() {
                    dialog.completed = true;
                    if let Some(window_handle) = window_handle {
                        let _ = window_handle.update(cx, |_, window, _| window.remove_window());
                    }
                }
                cx.notify();
            });
        });
        self.apply_task = Some(task);
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    fn change_default_app(
        &mut self,
        snapshot: &PropertySnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.default_app_task.is_some() {
            return;
        }
        let Some(path) = single_file_default_app_path(snapshot).map(Path::to_path_buf) else {
            return;
        };

        self.default_app_error = None;
        let before = snapshot.default_app.clone();
        let result = crate::explorer::open_with::choose_default_application_for_file(&path, window);
        self.refresh_after_default_app_change(path, before, result, cx);
    }

    #[cfg(target_os = "linux")]
    fn change_default_app(
        &mut self,
        snapshot: &PropertySnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.default_app_task.is_some() {
            return;
        }
        let Some(path) = single_file_default_app_path(snapshot).map(Path::to_path_buf) else {
            return;
        };

        use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

        self.default_app_error = None;
        let before = snapshot.default_app.clone();
        let target = snapshot.target.clone();
        let date_format = self.date_format.clone();
        let window_handle = HasWindowHandle::window_handle(window)
            .ok()
            .map(|handle| handle.as_raw());
        let display_handle = HasDisplayHandle::display_handle(window)
            .ok()
            .map(|handle| handle.as_raw());
        let path_for_result = path.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = crate::explorer::open_with::choose_default_application_for_file(
                &path,
                window_handle.as_ref(),
                display_handle.as_ref(),
            )
            .await;
            let snapshot = cx
                .background_executor()
                .spawn(
                    async move { collect_property_snapshot_with_date_format(target, &date_format) },
                )
                .await
                .ok();

            let _ = this.update(cx, |dialog, cx| {
                dialog.default_app_task = None;
                dialog.default_app_error =
                    default_app_change_error(&path_for_result, &before, &result, snapshot.as_ref());
                if let Some(snapshot) = snapshot {
                    dialog.set_ready_snapshot(snapshot, cx);
                }
                cx.notify();
            });
        });
        self.default_app_task = Some(task);
        cx.notify();
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    fn refresh_after_default_app_change(
        &mut self,
        path: PathBuf,
        before: Option<PropertyDefaultApp>,
        result: std::io::Result<OpenWithOutcome>,
        cx: &mut Context<Self>,
    ) {
        let target = self.target.clone();
        let date_format = self.date_format.clone();
        let task = cx.spawn(async move |this, cx| {
            let snapshot = cx
                .background_executor()
                .spawn(
                    async move { collect_property_snapshot_with_date_format(target, &date_format) },
                )
                .await
                .ok();

            let _ = this.update(cx, |dialog, cx| {
                dialog.default_app_task = None;
                dialog.default_app_error =
                    default_app_change_error(&path, &before, &result, snapshot.as_ref());
                if let Some(snapshot) = snapshot {
                    dialog.set_ready_snapshot(snapshot, cx);
                }
                cx.notify();
            });
        });
        self.default_app_task = Some(task);
        cx.notify();
    }

    fn set_active_tab(&mut self, tab: PropertyTab, cx: &mut Context<Self>) {
        if self.active_tab != tab {
            self.active_tab = tab;
            if tab == PropertyTab::Details {
                self.start_details_task(cx);
            }
            cx.notify();
        }
    }

    fn toggle_readonly(&mut self, cx: &mut Context<Self>) {
        let current = self.draft.readonly.unwrap_or(false);
        self.draft.readonly = Some(!current);
        cx.notify();
    }

    fn toggle_hidden(&mut self, cx: &mut Context<Self>) {
        let current = self.draft.hidden.unwrap_or(false);
        self.draft.hidden = Some(!current);
        cx.notify();
    }
}

impl Render for PropertiesDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .font(self.font.clone())
            .key_context("ExplorerDialog")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(rgb(0xf3f3f3))
            .cursor_default()
            .text_size(px(12.0))
            .text_color(rgb(0x000000))
            .on_action(cx.listener(Self::handle_cancel))
            .on_action(cx.listener(Self::handle_confirm))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .p(px(PROPERTIES_PADDING))
                    .child(self.render_tabs(window, cx))
                    .child(self.render_tab_panel_border(window))
                    .child(self.render_body(window, cx))
                    .child(self.render_buttons(window, cx)),
            )
    }
}

impl Focusable for PropertiesDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl PropertiesDialog {
    fn render_tabs(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let mut tabs = div().flex().flex_row().h(px(PROPERTIES_TAB_HEIGHT));
        for &(tab, label) in PROPERTY_TABS {
            tabs = tabs.child(tab_button(
                label,
                tab,
                self.active_tab,
                property_tab_width(label, &self.font, window),
                cx,
            ));
        }
        tabs.into_any_element()
    }

    fn render_tab_panel_border(&self, window: &Window) -> AnyElement {
        let mut border = div().flex().flex_row().h(px(PROPERTIES_BORDER_WIDTH));
        for &(tab, label) in PROPERTY_TABS {
            let color = if self.active_tab == tab {
                0xffffff
            } else {
                PROPERTIES_BORDER
            };
            border = border.child(
                div()
                    .w(px(property_tab_width(label, &self.font, window)))
                    .h(px(PROPERTIES_BORDER_WIDTH))
                    .bg(rgb(color)),
            );
        }
        border
            .child(
                div()
                    .flex_1()
                    .h(px(PROPERTIES_BORDER_WIDTH))
                    .bg(rgb(PROPERTIES_BORDER)),
            )
            .into_any_element()
    }

    fn render_body(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let body = match self.snapshot_state.clone() {
            PropertySnapshotState::Loading => centered_message("Loading properties..."),
            PropertySnapshotState::Failed(error) => centered_message(error),
            PropertySnapshotState::Ready(snapshot) => match self.active_tab {
                PropertyTab::General => self.render_general(&snapshot, window, cx),
                PropertyTab::Details => self.render_details(&snapshot, cx),
            },
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .border_1()
            .border_t_0()
            .border_color(rgb(PROPERTIES_BORDER))
            .bg(rgb(0xffffff))
            .min_w(px(0.0))
            .overflow_hidden()
            .p(px(PROPERTIES_PANEL_PADDING))
            .child(body)
            .into_any_element()
    }

    fn render_general(
        &self,
        snapshot: &PropertySnapshot,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let type_label = type_of_file_label(snapshot);
        let location = location_label(snapshot);
        let created = mixed_time_label(&snapshot.created, &self.date_format);
        let modified = mixed_time_label(&snapshot.modified, &self.date_format);
        let accessed = mixed_time_label(&snapshot.accessed, &self.date_format);
        let has_dates = non_empty_property_value(created.clone()).is_some()
            || non_empty_property_value(modified.clone()).is_some()
            || non_empty_property_value(accessed.clone()).is_some();

        let mut body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .id("properties-general-body")
            .overflow_y_scroll()
            .child(self.render_title_row(snapshot, cx))
            .child(separator());

        if let Some(type_label) = non_empty_property_value(type_label) {
            body = body.child(property_row(
                PROPERTIES_ROW_TYPE_ID,
                "Type:",
                type_label,
                cx,
            ));
        }
        if single_file_default_app_path(snapshot).is_some() {
            body = body.child(self.render_open_with_row(snapshot, window, cx));
        }

        body = body.child(separator());
        if let Some(location) = location {
            body = body.child(property_row(
                PROPERTIES_ROW_LOCATION_ID,
                "Location:",
                location,
                cx,
            ));
        }
        body = body
            .child(property_row(
                PROPERTIES_ROW_SIZE_ID,
                "Size:",
                property_size_label(snapshot.size),
                cx,
            ))
            .child(property_row(
                PROPERTIES_ROW_SIZE_ON_DISK_ID,
                "Size on disk:",
                property_size_label(snapshot.size_on_disk),
                cx,
            ));
        if let Some(contains) = snapshot.contains.as_ref() {
            body = body.child(property_row(
                PROPERTIES_ROW_CONTAINS_ID,
                "Contains:",
                contains_label(contains),
                cx,
            ));
        }

        if has_dates {
            body = body.child(separator());
            if let Some(created) = non_empty_property_value(created) {
                body = body.child(property_row(
                    PROPERTIES_ROW_CREATED_ID,
                    "Created:",
                    created,
                    cx,
                ));
            }
            if let Some(modified) = non_empty_property_value(modified) {
                body = body.child(property_row(
                    PROPERTIES_ROW_MODIFIED_ID,
                    "Modified:",
                    modified,
                    cx,
                ));
            }
            if let Some(accessed) = non_empty_property_value(accessed) {
                body = body.child(property_row(
                    PROPERTIES_ROW_ACCESSED_ID,
                    "Accessed:",
                    accessed,
                    cx,
                ));
            }
        }

        body = body
            .child(separator())
            .child(self.render_attributes_row(snapshot, cx));
        #[cfg(not(target_os = "windows"))]
        if let Some(error) = self.default_app_error.as_ref() {
            body = body.child(error_message(error));
        }
        if let Some(error) = self.apply_error.as_ref() {
            body = body.child(error_message(error));
        }

        body.into_any_element()
    }

    fn render_title_row(&self, snapshot: &PropertySnapshot, cx: &mut Context<Self>) -> AnyElement {
        let title_text = if let Some(counts) = snapshot.selection_counts.as_ref() {
            selection_count_label(counts)
        } else {
            snapshot.title.clone()
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .pb(px(8.0))
            .child(
                div()
                    .w(px(PROPERTIES_LABEL_WIDTH))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .child(self.render_item_icon(snapshot, cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h(px(34.0))
                    .flex()
                    .items_center()
                    .truncate()
                    .text_size(px(12.0))
                    .child(SharedString::from(title_text))
                    .into_any_element(),
            )
            .into_any_element()
    }

    fn render_item_icon(&self, snapshot: &PropertySnapshot, cx: &mut Context<Self>) -> AnyElement {
        if let PropertyIconSource::Single(path) = property_icon_source(snapshot) {
            if let Some(icon) = self.native_icon_for_path(&path, cx) {
                return image_icon(icon, PROPERTIES_ITEM_ICON_SIZE, PROPERTIES_ITEM_ICON_SIZE);
            }
            return fallback_property_icon(snapshot.item_kind, &path, PROPERTIES_ITEM_ICON_SIZE);
        }

        copy_file_dialog_icon_sized(PROPERTIES_ITEM_ICON_SIZE).into_any_element()
    }

    fn render_open_with_row(
        &self,
        snapshot: &PropertySnapshot,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        #[cfg(target_os = "windows")]
        let _ = window;

        let default_app_label = snapshot
            .default_app
            .as_ref()
            .map(|default_app| default_app.name.clone())
            .unwrap_or_else(|| "Unknown application".to_owned());
        let copied_default_app_label = default_app_label.clone();
        let row = div()
            .id("properties-open-with-row")
            .flex()
            .flex_row()
            .items_center()
            .min_h(px(34.0))
            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                copy_property_to_clipboard("Opens with:", &copied_default_app_label, cx);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .w(px(PROPERTIES_LABEL_WIDTH))
                    .flex_shrink_0()
                    .child("Opens with:"),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .min_w(px(0.0))
                    .flex_1()
                    .gap(px(8.0))
                    .when_some(snapshot.default_app.as_ref(), |this, default_app| {
                        this.child(self.render_default_app_icon(default_app, cx))
                    })
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .child(SharedString::from(default_app_label)),
                    ),
            );

        #[cfg(not(target_os = "windows"))]
        let row = {
            let snapshot_for_click = snapshot.clone();
            let enabled = self.default_app_task.is_none();
            row.child(
                property_button(
                    "properties-change-default-app",
                    "Change...",
                    enabled,
                    window.scale_factor(),
                )
                .when(enabled, |this| {
                    this.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.change_default_app(&snapshot_for_click, window, cx);
                        cx.stop_propagation();
                    }))
                }),
            )
        };

        row.into_any_element()
    }

    fn render_default_app_icon(
        &self,
        default_app: &PropertyDefaultApp,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(path) = default_app.path.as_ref() else {
            return div()
                .w(px(PROPERTIES_OPEN_WITH_ICON_SIZE))
                .h(px(PROPERTIES_OPEN_WITH_ICON_SIZE))
                .into_any_element();
        };

        self.native_icon_for_path(path, cx)
            .map(|icon| {
                image_icon(
                    icon,
                    PROPERTIES_OPEN_WITH_ICON_SIZE,
                    PROPERTIES_OPEN_WITH_ICON_SIZE,
                )
            })
            .unwrap_or_else(|| {
                file_icon_for_path_sized(path, PROPERTIES_OPEN_WITH_ICON_SIZE).into_any_element()
            })
    }

    fn native_icon_for_path(&self, path: &Path, cx: &mut Context<Self>) -> Option<Arc<Image>> {
        self.explorer
            .update(cx, |explorer, cx| explorer.native_icon_for_path(path, cx))
            .ok()
            .flatten()
    }

    fn render_attributes_row(
        &self,
        snapshot: &PropertySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let attributes_label = property_attributes_label(snapshot, &self.draft);
        let copied_attributes_label = attributes_label.clone();
        div()
            .id("properties-attributes-row")
            .flex()
            .flex_row()
            .items_center()
            .min_h(px(PROPERTIES_ROW_HEIGHT))
            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                copy_property_to_clipboard("Attributes:", &copied_attributes_label, cx);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .w(px(PROPERTIES_LABEL_WIDTH))
                    .flex_shrink_0()
                    .child("Attributes:"),
            )
            .child(attribute_inline(
                "Read-only",
                self.draft
                    .readonly
                    .or(mixed_bool_value(&snapshot.attributes.readonly)),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.toggle_readonly(cx);
                    cx.stop_propagation();
                }),
            ))
            .child(attribute_inline(
                "Hidden",
                self.draft
                    .hidden
                    .or(mixed_bool_value(&snapshot.attributes.hidden)),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.toggle_hidden(cx);
                    cx.stop_propagation();
                }),
            ))
            .into_any_element()
    }

    fn render_details(
        &mut self,
        snapshot: &PropertySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let groups = detail_groups_for_render(snapshot, &self.details_state);
        let loading_details = matches!(self.details_state, PropertyDetailsState::Loading);
        let mut body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .w_full()
            .id("properties-details-body")
            .overflow_y_scroll()
            .scrollbar_width(px(0.0))
            .track_scroll(&self.details_scroll_handle)
            .on_scroll_wheel(cx.listener(|_: &mut Self, _: &ScrollWheelEvent, _, cx| {
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .w_full()
                    .min_w(px(0.0))
                    .min_h(px(26.0))
                    .border_b_1()
                    .border_color(rgb(PROPERTIES_BORDER))
                    .text_color(rgb(PROPERTIES_MUTED_TEXT))
                    .child(div().w(px(154.0)).flex_shrink_0().child("Property"))
                    .child(div().flex_1().min_w(px(0.0)).child("Value")),
            );

        let mut detail_row_index = 0;
        for group in &groups {
            body = body.child(detail_group_header(&group.title));
            for detail in &group.details {
                body = body.child(detail_row(
                    detail_row_index,
                    &detail.name,
                    &detail.value,
                    cx,
                ));
                detail_row_index += 1;
            }
        }

        if loading_details {
            body = body.child(
                div()
                    .min_w(px(0.0))
                    .w_full()
                    .pt(px(12.0))
                    .text_color(rgb(PROPERTIES_MUTED_TEXT))
                    .truncate()
                    .child("Loading image metadata..."),
            );
        }

        if groups.is_empty() && !loading_details {
            body = body.child(
                div()
                    .min_w(px(0.0))
                    .w_full()
                    .pt(px(12.0))
                    .text_color(rgb(PROPERTIES_MUTED_TEXT))
                    .truncate()
                    .child("No additional metadata is available."),
            );
        }

        let has_scrollbar = self.details_scrollbar_metrics().is_some();
        div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .w_full()
            .overflow_hidden()
            .child(body)
            .when(has_scrollbar, |this| {
                this.child(self.render_details_scrollbar(cx))
            })
            .into_any_element()
    }

    fn details_scrollbar_metrics(&self) -> Option<ScrollbarMetrics> {
        let viewport_height = f32::from(self.details_scroll_handle.bounds().size.height);
        let scroll_max = f32::from(self.details_scroll_handle.max_offset().height);
        let scroll_top = -f32::from(self.details_scroll_handle.offset().y);
        details_scrollbar_metrics_for_dimensions(viewport_height, scroll_max, scroll_top)
    }

    fn set_details_scroll_top(&self, scroll_top: f32) {
        let scroll_top = self
            .details_scrollbar_metrics()
            .map_or(0.0, |metrics| metrics.clamp_scroll_top(scroll_top));
        let offset = self.details_scroll_handle.offset();
        self.details_scroll_handle
            .set_offset(point(offset.x, px(-scroll_top)));
    }

    fn handle_details_scrollbar_mouse_down(&mut self, local_y: f32, metrics: ScrollbarMetrics) {
        if local_y < SCROLLBAR_ARROW_HEIGHT {
            self.set_details_scroll_top(metrics.scroll_by(-PROPERTIES_ROW_HEIGHT));
        } else if local_y > metrics.viewport_height - SCROLLBAR_ARROW_HEIGHT {
            self.set_details_scroll_top(metrics.scroll_by(PROPERTIES_ROW_HEIGHT));
        } else if local_y >= metrics.thumb_top && local_y <= metrics.thumb_bottom() {
            self.details_scrollbar_drag = Some(ScrollbarDrag {
                pointer_offset_from_thumb_top: local_y - metrics.thumb_top,
            });
        } else if local_y < metrics.thumb_top {
            self.set_details_scroll_top(metrics.scroll_by(-metrics.viewport_height));
        } else {
            self.set_details_scroll_top(metrics.scroll_by(metrics.viewport_height));
        }
    }

    fn handle_details_scrollbar_drag(&mut self, local_y: f32, metrics: ScrollbarMetrics) {
        let Some(drag) = self.details_scrollbar_drag else {
            return;
        };

        let thumb_top = local_y - drag.pointer_offset_from_thumb_top;
        self.set_details_scroll_top(metrics.scroll_top_for_thumb_top(thumb_top));
    }

    fn render_details_scrollbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(metrics) = self.details_scrollbar_metrics() else {
            return div().into_any_element();
        };

        let hovered_or_dragged =
            self.details_scrollbar_hovered || self.details_scrollbar_drag.is_some();
        let thumb_width = if hovered_or_dragged {
            SCROLLBAR_THUMB_HOVER_WIDTH
        } else {
            SCROLLBAR_THUMB_WIDTH
        };
        let thumb_right = (SCROLLBAR_GUTTER_WIDTH - thumb_width) / 2.0;
        let thumb_color = if self.details_scrollbar_drag.is_some() {
            SCROLLBAR_THUMB_ACTIVE_BG
        } else if hovered_or_dragged {
            SCROLLBAR_THUMB_HOVER_BG
        } else {
            SCROLLBAR_THUMB_BG
        };
        let bottom_arrow_top = (metrics.viewport_height - SCROLLBAR_ARROW_HEIGHT).max(0.0);

        div()
            .id("properties-details-scrollbar")
            .relative()
            .w(px(SCROLLBAR_GUTTER_WIDTH))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(SCROLLBAR_TRACK_BG))
            .cursor_default()
            .block_mouse_except_scroll()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.details_scrollbar_hovered = *hovered;
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
            .child(self.render_details_scrollbar_hit_layer(cx))
            .into_any_element()
    }

    fn render_details_scrollbar_hit_layer(&self, cx: &mut Context<Self>) -> AnyElement {
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
                            if let Some(metrics) = this.details_scrollbar_metrics() {
                                this.handle_details_scrollbar_mouse_down(local_y, metrics);
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
                            if this.details_scrollbar_drag.is_none() {
                                return;
                            }

                            if let Some(metrics) = this.details_scrollbar_metrics() {
                                this.handle_details_scrollbar_drag(local_y, metrics);
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
                        if this.details_scrollbar_drag.take().is_some() {
                            cx.notify();
                        }
                    });
                });
            },
        )
        .size_full()
        .into_any_element()
    }

    fn render_buttons(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let has_changes = self.has_changes();
        div()
            .flex()
            .flex_row()
            .justify_end()
            .gap(px(8.0))
            .pt(px(12.0))
            .child(
                property_button("properties-ok", "OK", true, window.scale_factor()).on_click(
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        if this.has_changes() {
                            this.apply_changes(true, Some(window.window_handle()), cx);
                        } else {
                            this.close(window, cx);
                        }
                        cx.stop_propagation();
                    }),
                ),
            )
            .child(
                property_button("properties-cancel", "Cancel", true, window.scale_factor())
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.close(window, cx);
                        cx.stop_propagation();
                    })),
            )
            .child(
                property_button(
                    "properties-apply",
                    "Apply",
                    has_changes,
                    window.scale_factor(),
                )
                .when(has_changes, |this| {
                    this.on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.apply_changes(false, Some(window.window_handle()), cx);
                        cx.stop_propagation();
                    }))
                }),
            )
            .into_any_element()
    }
}

impl Default for EditablePropertyDraft {
    fn default() -> Self {
        Self {
            modified: None,
            accessed: None,
            readonly: None,
            hidden: None,
            unix_mode: None,
        }
    }
}

impl EditablePropertyDraft {
    fn from_snapshot(snapshot: &PropertySnapshot) -> Self {
        Self {
            modified: None,
            accessed: None,
            readonly: mixed_bool_value(&snapshot.attributes.readonly),
            hidden: mixed_bool_value(&snapshot.attributes.hidden),
            unix_mode: mixed_u32_value(&snapshot.unix_mode),
        }
    }
}

fn open_properties_window(
    target: PropertyTarget,
    explorer: gpui::Entity<ExplorerView>,
    date_format: String,
    cx: &mut Context<ExplorerView>,
) -> Result<AnyWindowHandle, String> {
    let title = properties_window_title(&target.paths);
    let options = properties_window_options(title, cx);
    let handle = cx
        .open_window(options, |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            cx.new(|cx| {
                cx.on_release(|dialog: &mut PropertiesDialog, cx| dialog.release(cx))
                    .detach();
                PropertiesDialog::new(target, explorer.downgrade(), date_format, focus_handle, cx)
            })
        })
        .map_err(|error| error.to_string())?;

    Ok(handle.into())
}

fn properties_window_options(title: String, cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::centered(
            size(px(PROPERTIES_WIDTH), px(PROPERTIES_HEIGHT)),
            cx,
        )),
        window_min_size: Some(size(px(PROPERTIES_WIDTH), px(PROPERTIES_HEIGHT))),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from(title)),
            ..Default::default()
        }),
        kind: WindowKind::Floating,
        is_movable: true,
        is_resizable: true,
        is_minimizable: false,
        window_decorations: Some(WindowDecorations::Server),
        ..Default::default()
    }
}

fn properties_window_title(paths: &[PathBuf]) -> String {
    if paths.len() == 1 {
        let name = paths[0]
            .file_name()
            .unwrap_or(paths[0].as_os_str())
            .to_string_lossy();
        format!("{name} Properties")
    } else {
        format!("{} Items Properties", paths.len())
    }
}

#[cfg(test)]
fn collect_property_snapshot(target: PropertyTarget) -> Result<PropertySnapshot, String> {
    collect_property_snapshot_with_date_format(target, crate::settings::DEFAULT_DATE_FORMAT)
}

fn collect_property_snapshot_with_date_format(
    target: PropertyTarget,
    date_format: &str,
) -> Result<PropertySnapshot, String> {
    if target.paths.is_empty() {
        return Err("No items selected.".to_owned());
    }

    let mut items = Vec::new();
    for path in &target.paths {
        items.push(collect_property_item(path, date_format));
    }

    let title = property_title(&target.paths);
    let item_kind = property_item_kind(&items);
    let type_label = mixed_from_iter(items.iter().map(|item| item.type_label.clone()));
    let location = mixed_from_iter(items.iter().map(|item| item.location.clone()));
    let created = mixed_from_iter(items.iter().map(|item| item.created));
    let modified = mixed_from_iter(items.iter().map(|item| item.modified));
    let accessed = mixed_from_iter(items.iter().map(|item| item.accessed));
    let readonly = mixed_from_iter(items.iter().map(|item| item.readonly));
    let hidden = mixed_from_iter(items.iter().map(|item| item.hidden));
    let owner = mixed_from_iter(items.iter().map(|item| item.owner.clone()));
    let group = mixed_from_iter(items.iter().map(|item| item.group.clone()));
    let unix_mode = mixed_from_iter(items.iter().map(|item| item.unix_mode));
    let permission_summary =
        mixed_from_iter(items.iter().map(|item| item.permission_summary.clone()));
    let size = items.iter().map(|item| item.size.unwrap_or(0)).sum();
    let size_on_disk = items
        .iter()
        .map(|item| item.size_on_disk.unwrap_or(0))
        .sum();
    let contains = if items.len() == 1 {
        items[0].contains.clone()
    } else {
        None
    };
    let selection_counts = (items.len() > 1)
        .then(|| selection_counts_summary(&items))
        .flatten();
    let shortcut = (items.len() == 1)
        .then(|| items[0].shortcut.clone())
        .flatten();
    let default_app = single_file_default_app(&items);
    let details = merged_details(&items);

    Ok(PropertySnapshot {
        target,
        title,
        item_count: items.len(),
        item_kind,
        type_label,
        location,
        size,
        size_on_disk,
        contains,
        selection_counts,
        created,
        modified,
        accessed,
        attributes: PropertyAttributes { readonly, hidden },
        owner,
        group,
        unix_mode,
        permission_summary,
        default_app,
        shortcut,
        details,
    })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PropertyTreeSummary {
    files: usize,
    folders: usize,
    size: u64,
    size_on_disk: u64,
}

impl PropertyTreeSummary {
    fn add(&mut self, other: Self) {
        self.files += other.files;
        self.folders += other.folders;
        self.size = self.size.saturating_add(other.size);
        self.size_on_disk = self.size_on_disk.saturating_add(other.size_on_disk);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PropertyItem {
    path: PathBuf,
    exists: bool,
    is_dir: bool,
    type_label: Option<String>,
    location: Option<String>,
    size: Option<u64>,
    size_on_disk: Option<u64>,
    contains: Option<PropertyContains>,
    selection_counts: Option<PropertyContains>,
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
    accessed: Option<SystemTime>,
    readonly: Option<bool>,
    hidden: Option<bool>,
    owner: Option<String>,
    group: Option<String>,
    unix_mode: Option<u32>,
    permission_summary: Option<String>,
    shortcut: Option<ShortcutDetails>,
    details: Vec<PropertyDetailGroup>,
}

fn collect_property_item(path: &Path, date_format: &str) -> PropertyItem {
    let link_metadata = fs::symlink_metadata(path).ok();
    let metadata = fs::metadata(path).ok().or_else(|| link_metadata.clone());
    let is_dir = metadata.as_ref().is_some_and(|metadata| metadata.is_dir());
    let exists = metadata.is_some();
    let entry = link_metadata.as_ref().and_then(|metadata| {
        crate::explorer::FileEntry::from_path_with_link_metadata(
            path.to_path_buf(),
            metadata.clone(),
        )
    });
    let tree_summary = collect_property_tree_summary(path);
    let size = tree_summary
        .map(|summary| summary.size)
        .or_else(|| metadata.as_ref().map(|metadata| metadata.len()));
    let size_on_disk = tree_summary
        .map(|summary| summary.size_on_disk)
        .or_else(|| {
            metadata
                .as_ref()
                .map(|metadata| size_on_disk(path, metadata).unwrap_or_else(|| metadata.len()))
        });
    let contains = if is_dir {
        tree_summary.map(|summary| PropertyContains {
            files: summary.files,
            folders: summary.folders.saturating_sub(1),
        })
    } else {
        None
    };
    let selection_counts = tree_summary.map(|summary| PropertyContains {
        files: summary.files,
        folders: summary.folders,
    });
    let readonly = metadata
        .as_ref()
        .map(|metadata| metadata.permissions().readonly());
    let hidden = Some(path_is_hidden(path, metadata.as_ref()));
    let shortcut = shortcut_details(path, entry.as_ref());
    let details = metadata_details(
        path,
        entry.as_ref(),
        metadata.as_ref(),
        size,
        size_on_disk,
        date_format,
    );

    PropertyItem {
        path: path.to_path_buf(),
        exists,
        is_dir,
        type_label: entry.as_ref().map(|entry| entry.type_label()),
        location: path.parent().map(|parent| parent.display().to_string()),
        size,
        size_on_disk,
        contains,
        selection_counts,
        created: metadata
            .as_ref()
            .and_then(|metadata| metadata.created().ok()),
        modified: metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok()),
        accessed: metadata
            .as_ref()
            .and_then(|metadata| metadata.accessed().ok()),
        readonly,
        hidden,
        owner: owner_name(metadata.as_ref()),
        group: group_name(metadata.as_ref()),
        unix_mode: unix_mode(metadata.as_ref()),
        permission_summary: permission_summary(metadata.as_ref()),
        shortcut,
        details,
    }
}

fn property_title(paths: &[PathBuf]) -> String {
    if paths.len() == 1 {
        paths[0]
            .file_name()
            .unwrap_or(paths[0].as_os_str())
            .to_string_lossy()
            .into_owned()
    } else {
        format!("{} items", paths.len())
    }
}

fn property_item_kind(items: &[PropertyItem]) -> PropertyItemKind {
    if items.iter().any(|item| !item.exists) {
        return PropertyItemKind::Missing;
    }
    if items.len() == 1 {
        if items[0].shortcut.is_some() {
            return PropertyItemKind::SingleShortcut;
        }
        return if items[0].is_dir {
            PropertyItemKind::SingleFolder
        } else {
            PropertyItemKind::SingleFile
        };
    }

    let directories = items.iter().filter(|item| item.is_dir).count();
    match (directories, items.len() - directories) {
        (0, _) => PropertyItemKind::MultipleFiles,
        (_, 0) => PropertyItemKind::MultipleFolders,
        _ => PropertyItemKind::MultipleItems,
    }
}

fn selection_counts_summary(items: &[PropertyItem]) -> Option<PropertyContains> {
    let mut files = 0;
    let mut folders = 0;
    let mut has_counts = false;
    for item in items {
        if let Some(counts) = &item.selection_counts {
            has_counts = true;
            files += counts.files;
            folders += counts.folders;
        }
    }
    has_counts.then_some(PropertyContains { files, folders })
}

fn single_file_default_app(items: &[PropertyItem]) -> Option<PropertyDefaultApp> {
    let [item] = items else {
        return None;
    };
    if !item.exists || item.is_dir {
        return None;
    }

    default_application_for_file(&item.path).map(PropertyDefaultApp::from)
}

fn collect_property_tree_summary(path: &Path) -> Option<PropertyTreeSummary> {
    let link_metadata = fs::symlink_metadata(path).ok()?;
    let metadata = fs::metadata(path)
        .ok()
        .unwrap_or_else(|| link_metadata.clone());
    Some(property_tree_summary_from_metadata(
        path,
        &link_metadata,
        &metadata,
    ))
}

fn property_tree_summary_from_metadata(
    path: &Path,
    link_metadata: &fs::Metadata,
    metadata: &fs::Metadata,
) -> PropertyTreeSummary {
    let mut summary = PropertyTreeSummary::default();
    let is_dir = metadata.is_dir();
    let is_directory_link = metadata_is_directory_link(link_metadata);

    if is_dir {
        summary.folders += 1;
        if !is_directory_link {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    if let Some(child) = collect_property_tree_summary(&entry.path()) {
                        summary.add(child);
                    }
                }
            }
            return summary;
        }
    } else {
        summary.files += 1;
    }

    let size_metadata = if is_directory_link {
        link_metadata
    } else {
        metadata
    };
    summary.size = size_metadata.len();
    summary.size_on_disk = size_on_disk(path, size_metadata).unwrap_or_else(|| size_metadata.len());
    summary
}

fn merged_details(items: &[PropertyItem]) -> Vec<PropertyDetailGroup> {
    merge_detail_groups(items.iter().map(|item| item.details.as_slice()))
}

fn merge_detail_groups<'a>(
    groups_by_item: impl IntoIterator<Item = &'a [PropertyDetailGroup]>,
) -> Vec<PropertyDetailGroup> {
    let mut values: BTreeMap<PropertyDetailGroupKind, Vec<(String, MixedValue<String>)>> =
        BTreeMap::new();

    for groups in groups_by_item {
        for group in groups {
            let group_values = values.entry(group.kind).or_default();
            for detail in &group.details {
                if let Some((_, value)) = group_values
                    .iter_mut()
                    .find(|(name, _)| name == &detail.name)
                {
                    let current = std::mem::replace(value, MixedValue::None);
                    *value = mix_value(current, Some(detail.value.clone()));
                } else {
                    group_values.push((
                        detail.name.clone(),
                        mix_value(MixedValue::None, Some(detail.value.clone())),
                    ));
                }
            }
        }
    }

    PROPERTY_DETAIL_GROUP_ORDER
        .iter()
        .filter_map(|kind| {
            let details: Vec<_> = values
                .remove(kind)?
                .into_iter()
                .map(|(name, value)| PropertyDetail {
                    name,
                    value: mixed_string_label(&value),
                })
                .collect();
            (!details.is_empty()).then(|| property_detail_group(*kind, details))
        })
        .collect()
}

fn property_detail_group(
    kind: PropertyDetailGroupKind,
    details: Vec<PropertyDetail>,
) -> PropertyDetailGroup {
    PropertyDetailGroup {
        kind,
        title: kind.title().to_owned(),
        details,
    }
}

fn metadata_details(
    path: &Path,
    entry: Option<&crate::explorer::FileEntry>,
    metadata: Option<&fs::Metadata>,
    size: Option<u64>,
    size_on_disk: Option<u64>,
    date_format: &str,
) -> Vec<PropertyDetailGroup> {
    let mut details = Vec::new();
    details.push(PropertyDetail {
        name: "Name".to_owned(),
        value: path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned(),
    });
    if let Some(size) = size {
        details.push(PropertyDetail {
            name: "Size".to_owned(),
            value: property_size_label(size),
        });
    }
    if let Some(size_on_disk) = size_on_disk {
        details.push(PropertyDetail {
            name: "Size on disk".to_owned(),
            value: property_size_label(size_on_disk),
        });
    }
    if metadata.is_some_and(|metadata| metadata.is_file()) {
        details.push(PropertyDetail {
            name: "MIME Type".to_owned(),
            value: mime_type_label(path),
        });
    }
    if let Some(entry) = entry {
        details.push(PropertyDetail {
            name: "Item type".to_owned(),
            value: entry.type_label(),
        });
    }
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        details.push(PropertyDetail {
            name: "Extension".to_owned(),
            value: extension.to_owned(),
        });
    }
    if let Some(permissions) = permission_summary(metadata) {
        details.push(PropertyDetail {
            name: "Permissions".to_owned(),
            value: permissions,
        });
    }
    if let Some(parent) = path.parent() {
        details.push(PropertyDetail {
            name: "Directory".to_owned(),
            value: parent.display().to_string(),
        });
    }
    push_time_detail(
        &mut details,
        "Accessed",
        metadata,
        date_format,
        |metadata| metadata.accessed().ok(),
    );
    push_time_detail(
        &mut details,
        "Modified",
        metadata,
        date_format,
        |metadata| metadata.modified().ok(),
    );
    push_time_detail(&mut details, "Created", metadata, date_format, |metadata| {
        metadata.created().ok()
    });
    if let Some(owner) = owner_name(metadata) {
        details.push(PropertyDetail {
            name: "Owner".to_owned(),
            value: owner,
        });
    }
    if let Some(group) = group_name(metadata) {
        details.push(PropertyDetail {
            name: "Group".to_owned(),
            value: group,
        });
    }
    if let Some(mode) = unix_mode(metadata) {
        details.push(PropertyDetail {
            name: "Mode".to_owned(),
            value: unix_mode_detail_label(mode),
        });
    }
    if let Ok((width, height)) = image::image_dimensions(path) {
        details.push(PropertyDetail {
            name: "Dimensions".to_owned(),
            value: format!("{width} x {height}"),
        });
    }

    vec![property_detail_group(
        PropertyDetailGroupKind::File,
        details,
    )]
}

fn push_time_detail(
    details: &mut Vec<PropertyDetail>,
    name: &'static str,
    metadata: Option<&fs::Metadata>,
    date_format: &str,
    timestamp: impl FnOnce(&fs::Metadata) -> Option<SystemTime>,
) {
    let Some(value) = metadata
        .and_then(timestamp)
        .map(|timestamp| format_timestamp(Some(timestamp), date_format))
        .and_then(non_empty_property_value)
    else {
        return;
    };

    details.push(PropertyDetail {
        name: name.to_owned(),
        value,
    });
}

fn mime_type_label(path: &Path) -> String {
    mime_guess::from_path(path)
        .first_raw()
        .unwrap_or("application/octet-stream")
        .to_owned()
}

fn path_may_have_exif(path: &Path) -> bool {
    mime_guess::from_path(path)
        .first_raw()
        .is_some_and(|mime| mime.starts_with("image/"))
}

fn collect_exif_detail_groups(target: &PropertyTarget) -> Vec<PropertyDetailGroup> {
    let groups_by_item: Vec<_> = target
        .paths
        .iter()
        .filter(|path| path_may_have_exif(path))
        .map(|path| exif_details(path))
        .collect();
    merge_detail_groups(groups_by_item.iter().map(Vec::as_slice))
}

fn exif_details(path: &Path) -> Vec<PropertyDetailGroup> {
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    let mut reader = BufReader::new(file);
    let Ok(exif) = exif::Reader::new()
        .continue_on_error(true)
        .read_from_container(&mut reader)
        .or_else(|error| error.distill_partial_result(|_| {}))
    else {
        return Vec::new();
    };

    let fields: Vec<_> = exif.fields().collect();
    let mut tag_counts = BTreeMap::new();
    for field in &fields {
        *tag_counts.entry(exif_tag_name(field.tag)).or_insert(0usize) += 1;
    }

    let mut used_names = BTreeMap::new();
    let mut groups: BTreeMap<PropertyDetailGroupKind, Vec<PropertyDetail>> = BTreeMap::new();
    for field in fields {
        let kind = exif_detail_group_kind(field);
        let tag = exif_tag_name(field.tag);
        let base_name = if tag_counts.get(&tag).copied().unwrap_or(0) > 1 {
            format!("{tag} (IFD {})", field.ifd_num.index())
        } else {
            tag
        };
        let occurrence = used_names
            .entry((kind, base_name.clone()))
            .or_insert(0usize);
        *occurrence += 1;
        let name = if *occurrence == 1 {
            base_name
        } else {
            format!("{base_name} #{}", *occurrence)
        };

        groups.entry(kind).or_default().push(PropertyDetail {
            name,
            value: field.display_value().with_unit(&exif).to_string(),
        });
    }

    PROPERTY_DETAIL_GROUP_ORDER
        .iter()
        .filter_map(|kind| {
            let details = groups.remove(kind)?;
            (!details.is_empty()).then(|| property_detail_group(*kind, details))
        })
        .collect()
}

fn exif_tag_name(tag: exif::Tag) -> String {
    if tag.description().is_none() {
        return format!("Tag({:?}, 0x{:04X})", tag.context(), tag.number());
    }

    named_exif_tag_label(tag)
        .map(str::to_owned)
        .unwrap_or_else(|| humanized_exif_tag_name(&tag.to_string()))
}

fn named_exif_tag_label(tag: exif::Tag) -> Option<&'static str> {
    [
        (exif::Tag::FileSource, "File Source"),
        (exif::Tag::FocalLength, "Focal Length"),
        (
            exif::Tag::FocalLengthIn35mmFilm,
            "Focal Length In 35mm Film",
        ),
        (exif::Tag::LensSpecification, "Lens Specification"),
        (exif::Tag::LensMake, "Lens Make"),
        (exif::Tag::LensModel, "Lens Model"),
        (exif::Tag::LensSerialNumber, "Lens Serial Number"),
        (exif::Tag::ExposureTime, "Exposure Time"),
        (exif::Tag::FNumber, "F Number"),
        (
            exif::Tag::PhotographicSensitivity,
            "Photographic Sensitivity",
        ),
        (exif::Tag::ISOSpeed, "ISO Speed"),
        (exif::Tag::MaxApertureValue, "Max Aperture Value"),
        (exif::Tag::ExposureProgram, "Exposure Program"),
        (exif::Tag::ShutterSpeedValue, "Shutter Speed Value"),
        (exif::Tag::ApertureValue, "Aperture Value"),
        (exif::Tag::BrightnessValue, "Brightness Value"),
        (exif::Tag::ExposureBiasValue, "Exposure Bias Value"),
        (exif::Tag::SubjectDistance, "Subject Distance"),
        (exif::Tag::MeteringMode, "Metering Mode"),
        (exif::Tag::LightSource, "Light Source"),
        (exif::Tag::ExposureMode, "Exposure Mode"),
        (exif::Tag::WhiteBalance, "White Balance"),
        (exif::Tag::DigitalZoomRatio, "Digital Zoom Ratio"),
        (exif::Tag::SceneCaptureType, "Scene Capture Type"),
        (exif::Tag::SubjectDistanceRange, "Subject Distance Range"),
    ]
    .into_iter()
    .find_map(|(candidate, label)| (candidate == tag).then_some(label))
}

fn humanized_exif_tag_name(name: &str) -> String {
    let chars: Vec<_> = name.chars().collect();
    let mut text = String::with_capacity(name.len() + name.len() / 4);
    for (ix, ch) in chars.iter().copied().enumerate() {
        if ix > 0 && exif_tag_name_boundary(chars[ix - 1], ch, chars.get(ix + 1).copied()) {
            text.push(' ');
        }
        text.push(ch);
    }
    text
}

fn exif_tag_name_boundary(previous: char, current: char, next: Option<char>) -> bool {
    if current.is_ascii_digit() {
        return !previous.is_ascii_digit();
    }
    if !current.is_ascii_uppercase() {
        return false;
    }
    previous.is_ascii_lowercase()
        || previous.is_ascii_digit()
        || previous.is_ascii_uppercase() && next.is_some_and(|next| next.is_ascii_lowercase())
}

fn exif_detail_group_kind(field: &exif::Field) -> PropertyDetailGroupKind {
    if field.tag.context() == exif::Context::Gps {
        PropertyDetailGroupKind::Gps
    } else if field.tag.description().is_none() {
        PropertyDetailGroupKind::NonStandard
    } else if camera_exif_tag(field.tag) {
        PropertyDetailGroupKind::Camera
    } else if exposure_exif_tag(field.tag) {
        PropertyDetailGroupKind::Exposure
    } else {
        PropertyDetailGroupKind::Misc
    }
}

fn camera_exif_tag(tag: exif::Tag) -> bool {
    [
        exif::Tag::FileSource,
        exif::Tag::FocalLength,
        exif::Tag::FocalLengthIn35mmFilm,
        exif::Tag::LensSpecification,
        exif::Tag::LensMake,
        exif::Tag::LensModel,
        exif::Tag::LensSerialNumber,
        exif::Tag::Make,
        exif::Tag::Model,
    ]
    .contains(&tag)
}

fn exposure_exif_tag(tag: exif::Tag) -> bool {
    [
        exif::Tag::ExposureTime,
        exif::Tag::FNumber,
        exif::Tag::PhotographicSensitivity,
        exif::Tag::ISOSpeed,
        exif::Tag::MaxApertureValue,
        exif::Tag::ExposureProgram,
        exif::Tag::ShutterSpeedValue,
        exif::Tag::ApertureValue,
        exif::Tag::BrightnessValue,
        exif::Tag::ExposureBiasValue,
        exif::Tag::SubjectDistance,
        exif::Tag::MeteringMode,
        exif::Tag::LightSource,
        exif::Tag::Flash,
        exif::Tag::ExposureMode,
        exif::Tag::WhiteBalance,
        exif::Tag::DigitalZoomRatio,
        exif::Tag::SceneCaptureType,
        exif::Tag::Contrast,
        exif::Tag::Saturation,
        exif::Tag::Sharpness,
        exif::Tag::SubjectDistanceRange,
    ]
    .contains(&tag)
}

fn shortcut_details(
    path: &Path,
    entry: Option<&crate::explorer::FileEntry>,
) -> Option<ShortcutDetails> {
    if let Ok(target) = fs::read_link(path) {
        return Some(ShortcutDetails {
            target: target.display().to_string(),
            target_type: if target.is_dir() {
                "File folder".to_owned()
            } else {
                "File".to_owned()
            },
        });
    }

    match entry.map(|entry| &entry.kind) {
        Some(EntryKind::DirectoryLink(DirectoryLinkKind::ShellShortcut {
            target,
            target_kind,
        })) => Some(ShortcutDetails {
            target: target.display().to_string(),
            target_type: format!("{target_kind:?}"),
        }),
        _ => None,
    }
}

fn path_is_hidden(path: &Path, metadata: Option<&fs::Metadata>) -> bool {
    #[cfg(target_os = "windows")]
    {
        let _ = path;
        use std::os::windows::fs::MetadataExt;
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN;
        return metadata
            .is_some_and(|metadata| metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN.0 != 0);
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = metadata;
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('.'))
    }
}

#[cfg(not(target_os = "windows"))]
fn metadata_is_directory_link(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(target_os = "windows")]
fn metadata_is_directory_link(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
}

fn size_on_disk(path: &Path, metadata: &fs::Metadata) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        return Some(metadata.blocks().saturating_mul(512));
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Foundation::{ERROR_SUCCESS, GetLastError, SetLastError};
        use windows::Win32::Storage::FileSystem::{GetCompressedFileSizeW, INVALID_FILE_SIZE};
        use windows::core::PCWSTR;

        let mut high = 0;
        let encoded = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        unsafe {
            SetLastError(ERROR_SUCCESS);
            let low = GetCompressedFileSizeW(PCWSTR::from_raw(encoded.as_ptr()), Some(&mut high));
            if low == INVALID_FILE_SIZE && GetLastError() != ERROR_SUCCESS {
                return Some(metadata.len());
            }
            return Some(((high as u64) << 32) | low as u64);
        }
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    {
        Some(metadata.len())
    }
}

#[cfg(unix)]
fn owner_name(metadata: Option<&fs::Metadata>) -> Option<String> {
    use std::ffi::CStr;
    use std::os::unix::fs::MetadataExt;

    let uid = metadata?.uid();
    unsafe {
        let passwd = libc::getpwuid(uid);
        if passwd.is_null() {
            return Some(uid.to_string());
        }
        Some(
            CStr::from_ptr((*passwd).pw_name)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

#[cfg(not(unix))]
fn owner_name(_: Option<&fs::Metadata>) -> Option<String> {
    None
}

#[cfg(unix)]
fn group_name(metadata: Option<&fs::Metadata>) -> Option<String> {
    use std::ffi::CStr;
    use std::os::unix::fs::MetadataExt;

    let gid = metadata?.gid();
    unsafe {
        let group = libc::getgrgid(gid);
        if group.is_null() {
            return Some(gid.to_string());
        }
        Some(
            CStr::from_ptr((*group).gr_name)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

#[cfg(not(unix))]
fn group_name(_: Option<&fs::Metadata>) -> Option<String> {
    None
}

#[cfg(unix)]
fn unix_mode(metadata: Option<&fs::Metadata>) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    metadata.map(|metadata| metadata.mode() & 0o777)
}

#[cfg(not(unix))]
fn unix_mode(_: Option<&fs::Metadata>) -> Option<u32> {
    None
}

#[cfg(unix)]
fn unix_mode_detail_label(mode: u32) -> String {
    format!("{mode:o} ({})", unix_mode_string(mode))
}

#[cfg(not(unix))]
fn unix_mode_detail_label(mode: u32) -> String {
    mode.to_string()
}

#[cfg(unix)]
fn permission_summary(metadata: Option<&fs::Metadata>) -> Option<String> {
    unix_mode(metadata).map(|mode| format!("{mode:o} ({})", unix_mode_string(mode)))
}

#[cfg(not(unix))]
fn permission_summary(metadata: Option<&fs::Metadata>) -> Option<String> {
    metadata.map(|metadata| {
        if metadata.permissions().readonly() {
            "Read-only".to_owned()
        } else {
            "Writable".to_owned()
        }
    })
}

#[cfg(unix)]
fn unix_mode_string(mode: u32) -> String {
    let mut text = String::with_capacity(9);
    for bit in [
        0o400, 0o200, 0o100, 0o040, 0o020, 0o010, 0o004, 0o002, 0o001,
    ] {
        text.push(if mode & bit != 0 {
            match bit {
                0o400 | 0o040 | 0o004 => 'r',
                0o200 | 0o020 | 0o002 => 'w',
                _ => 'x',
            }
        } else {
            '-'
        });
    }
    text
}

fn mixed_from_iter<T: Eq>(values: impl IntoIterator<Item = Option<T>>) -> MixedValue<T> {
    values
        .into_iter()
        .fold(MixedValue::None, |current, value| mix_value(current, value))
}

fn mix_value<T: Eq>(current: MixedValue<T>, value: Option<T>) -> MixedValue<T> {
    match (current, value) {
        (MixedValue::Mixed, _) => MixedValue::Mixed,
        (MixedValue::None, None) => MixedValue::None,
        (MixedValue::None, Some(value)) => MixedValue::Single(value),
        (MixedValue::Single(current), Some(value)) if current == value => {
            MixedValue::Single(current)
        }
        (MixedValue::Single(_), _) => MixedValue::Mixed,
    }
}

fn mixed_string_label(value: &MixedValue<String>) -> String {
    match value {
        MixedValue::None => String::new(),
        MixedValue::Single(value) => value.clone(),
        MixedValue::Mixed => String::new(),
    }
}

fn mixed_time_label(value: &MixedValue<SystemTime>, date_format: &str) -> String {
    match value {
        MixedValue::None => String::new(),
        MixedValue::Single(value) => format_timestamp(Some(*value), date_format),
        MixedValue::Mixed => String::new(),
    }
}

fn mixed_bool_value(value: &MixedValue<bool>) -> Option<bool> {
    match value {
        MixedValue::Single(value) => Some(*value),
        MixedValue::None | MixedValue::Mixed => None,
    }
}

fn mixed_u32_value(value: &MixedValue<u32>) -> Option<u32> {
    match value {
        MixedValue::Single(value) => Some(*value),
        MixedValue::None | MixedValue::Mixed => None,
    }
}

fn property_apply_plan(
    snapshot: &PropertySnapshot,
    draft: &EditablePropertyDraft,
) -> EditablePropertyDraft {
    let baseline = EditablePropertyDraft::from_snapshot(snapshot);
    EditablePropertyDraft {
        modified: draft.modified,
        accessed: draft.accessed,
        readonly: (draft.readonly != baseline.readonly)
            .then_some(draft.readonly)
            .flatten(),
        hidden: (draft.hidden != baseline.hidden)
            .then_some(draft.hidden)
            .flatten(),
        unix_mode: (draft.unix_mode != baseline.unix_mode)
            .then_some(draft.unix_mode)
            .flatten(),
    }
}

fn property_apply_plan_is_empty(plan: &EditablePropertyDraft) -> bool {
    plan.modified.is_none()
        && plan.accessed.is_none()
        && plan.readonly.is_none()
        && plan.hidden.is_none()
        && plan.unix_mode.is_none()
}

pub(super) fn apply_property_draft(
    paths: &[PathBuf],
    draft: &EditablePropertyDraft,
) -> PropertyApplyOutcome {
    let mut outcome = PropertyApplyOutcome::default();
    for path in paths {
        match apply_property_draft_to_path(path, draft) {
            Ok(changed) => {
                if changed {
                    outcome.changed += 1;
                }
            }
            Err(error) => outcome.errors.push(format!("{}: {error}", path.display())),
        }
    }
    outcome
}

fn apply_property_draft_to_path(
    path: &Path,
    draft: &EditablePropertyDraft,
) -> Result<bool, String> {
    let mut changed = false;
    if draft.modified.is_some() || draft.accessed.is_some() {
        let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
        let accessed = draft
            .accessed
            .or_else(|| metadata.accessed().ok())
            .map(FileTime::from_system_time)
            .unwrap_or_else(FileTime::zero);
        let modified = draft
            .modified
            .or_else(|| metadata.modified().ok())
            .map(FileTime::from_system_time)
            .unwrap_or_else(FileTime::zero);
        set_file_times(path, accessed, modified).map_err(|error| error.to_string())?;
        changed = true;
    }
    if let Some(readonly) = draft.readonly {
        let mut permissions = fs::metadata(path)
            .map_err(|error| error.to_string())?
            .permissions();
        permissions.set_readonly(readonly);
        fs::set_permissions(path, permissions).map_err(|error| error.to_string())?;
        changed = true;
    }
    if let Some(hidden) = draft.hidden {
        apply_hidden_attribute(path, hidden)?;
        changed = true;
    }
    if let Some(mode) = draft.unix_mode {
        apply_unix_mode(path, mode)?;
        changed = true;
    }
    Ok(changed)
}

#[cfg(unix)]
fn apply_unix_mode(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn apply_unix_mode(_: &Path, _: u32) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "windows")]
fn apply_hidden_attribute(path: &Path, hidden: bool) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_HIDDEN, FILE_FLAGS_AND_ATTRIBUTES, SetFileAttributesW,
    };
    use windows::core::PCWSTR;

    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let mut attributes = metadata.file_attributes();
    if hidden {
        attributes |= FILE_ATTRIBUTE_HIDDEN.0;
    } else {
        attributes &= !FILE_ATTRIBUTE_HIDDEN.0;
    }
    let mut encoded = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    unsafe {
        SetFileAttributesW(
            PCWSTR::from_raw(encoded.as_mut_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(attributes),
        )
        .map_err(|error| error.to_string())
    }
}

#[cfg(not(target_os = "windows"))]
fn apply_hidden_attribute(_: &Path, _: bool) -> Result<(), String> {
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PropertyIconSource {
    Single(PathBuf),
    Multiple,
}

fn property_icon_source(snapshot: &PropertySnapshot) -> PropertyIconSource {
    if snapshot.target.paths.len() == 1 {
        PropertyIconSource::Single(snapshot.target.paths[0].clone())
    } else {
        PropertyIconSource::Multiple
    }
}

fn fallback_property_icon(kind: PropertyItemKind, path: &Path, size: f32) -> AnyElement {
    match kind {
        PropertyItemKind::SingleFolder => folder_icon_sized(size).into_any_element(),
        PropertyItemKind::SingleShortcut => directory_shortcut_icon_sized(size).into_any_element(),
        _ => file_icon_for_path_sized(path, size).into_any_element(),
    }
}

fn single_file_default_app_path(snapshot: &PropertySnapshot) -> Option<&Path> {
    matches!(snapshot.item_kind, PropertyItemKind::SingleFile)
        .then(|| snapshot.target.paths.first().map(PathBuf::as_path))
        .flatten()
}

#[cfg(not(target_os = "windows"))]
fn default_app_change_error(
    path: &Path,
    before: &Option<PropertyDefaultApp>,
    result: &std::io::Result<OpenWithOutcome>,
    snapshot: Option<&PropertySnapshot>,
) -> Option<String> {
    match result {
        Err(error) => Some(format!(
            "Could not change the default app for {}: {error}",
            property_path_display_name(path)
        )),
        Ok(OpenWithOutcome::Cancelled) => None,
        Ok(OpenWithOutcome::Opened) => {
            let Some(snapshot) = snapshot else {
                return Some(format!(
                    "The default app for {} could not be verified.",
                    property_path_display_name(path)
                ));
            };
            if &snapshot.default_app != before {
                None
            } else if snapshot.default_app.is_none() {
                Some(format!(
                    "No default app for {} could be verified.",
                    property_path_display_name(path)
                ))
            } else {
                Some(format!(
                    "The selected app was opened, but the default app for {} did not appear to change.",
                    property_path_display_name(path)
                ))
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn property_path_display_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn type_of_file_label(snapshot: &PropertySnapshot) -> String {
    if snapshot.item_count > 1 && matches!(snapshot.type_label, MixedValue::Mixed) {
        return "Multiple Types".to_owned();
    }
    let label = mixed_string_label(&snapshot.type_label);
    if matches!(snapshot.item_kind, PropertyItemKind::SingleFile) {
        if let Some(path) = snapshot.target.paths.first() {
            return single_file_type_label(path, &label);
        }
    }
    label
}

fn single_file_type_label(path: &Path, base_label: &str) -> String {
    let base_label = if base_label.is_empty() {
        "File"
    } else {
        base_label
    };
    let Some(suffix) = file_type_suffix(path) else {
        return base_label.to_owned();
    };
    let label = if base_label == "File" {
        let type_name = suffix.trim_start_matches('.').to_ascii_uppercase();
        if type_name.is_empty() {
            base_label.to_owned()
        } else {
            format!("{type_name} File")
        }
    } else {
        base_label.to_owned()
    };

    format!("{label} ({suffix})")
}

fn file_type_suffix(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    if name.starts_with('.') && !name[1..].contains('.') {
        return Some(name.into_owned());
    }

    path.extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| !extension.is_empty())
        .map(|extension| format!(".{extension}"))
}

fn location_label(snapshot: &PropertySnapshot) -> Option<String> {
    match &snapshot.location {
        MixedValue::Single(location) if !location.is_empty() && snapshot.item_count > 1 => {
            Some(format!("All in {location}"))
        }
        MixedValue::Single(location) if !location.is_empty() => Some(location.clone()),
        MixedValue::None | MixedValue::Mixed | MixedValue::Single(_) => None,
    }
}

fn selection_count_label(counts: &PropertyContains) -> String {
    match (counts.files, counts.folders) {
        (0, 0) => count_label(0, "File", "Files"),
        (0, folders) => count_label(folders, "Folder", "Folders"),
        (files, 0) => count_label(files, "File", "Files"),
        _ => contains_label(counts),
    }
}

fn contains_label(contains: &PropertyContains) -> String {
    format!(
        "{}, {}",
        count_label(contains.files, "File", "Files"),
        count_label(contains.folders, "Folder", "Folders")
    )
}

fn count_label(count: usize, singular: &str, plural: &str) -> String {
    format!(
        "{} {}",
        count.separate_with_commas(),
        if count == 1 { singular } else { plural }
    )
}

fn non_empty_property_value(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn property_size_label(size: u64) -> String {
    let label = format_size(Some(size));
    if label.ends_with(" bytes") {
        label
    } else {
        format!("{label} ({} bytes)", size.separate_with_commas())
    }
}

fn detail_groups_for_render(
    snapshot: &PropertySnapshot,
    details_state: &PropertyDetailsState,
) -> Vec<PropertyDetailGroup> {
    let mut groups = snapshot.details.clone();
    if let PropertyDetailsState::Ready(exif_groups) = details_state {
        groups.extend(exif_groups.iter().cloned());
    }
    groups.sort_by_key(|group| group.kind);
    groups
}

fn details_scrollbar_metrics_for_dimensions(
    viewport_height: f32,
    scroll_max: f32,
    scroll_top: f32,
) -> Option<ScrollbarMetrics> {
    if scroll_max <= 0.0 {
        return None;
    }

    ScrollbarMetrics::new(viewport_height, viewport_height + scroll_max, scroll_top)
}

fn tab_button(
    label: &'static str,
    tab: PropertyTab,
    active: PropertyTab,
    width: f32,
    cx: &mut Context<PropertiesDialog>,
) -> AnyElement {
    let id = match tab {
        PropertyTab::General => "properties-tab-general",
        PropertyTab::Details => "properties-tab-details",
    };
    div()
        .id(id)
        .w(px(width))
        .h(px(PROPERTIES_TAB_HEIGHT))
        .px(px(PROPERTIES_TAB_HORIZONTAL_PADDING))
        .flex()
        .items_center()
        .border_1()
        .border_color(rgb(PROPERTIES_BORDER))
        .border_b_0()
        .bg(rgb(if active == tab { 0xffffff } else { 0xf3f3f3 }))
        .when(active != tab, |this| {
            this.hover(|style| style.bg(rgb(0xededed)))
        })
        .cursor_default()
        .child(label)
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_active_tab(tab, cx)))
        .into_any_element()
}

fn property_tab_width(label: &str, font: &gpui::Font, window: &Window) -> f32 {
    let run = TextRun {
        len: label.len(),
        font: font.clone(),
        color: rgb(0x000000).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };

    f32::from(
        window
            .text_system()
            .layout_line(label, px(12.0), &[run], None)
            .width,
    ) + PROPERTIES_TAB_HORIZONTAL_PADDING * 2.0
        + PROPERTIES_BORDER_WIDTH * 2.0
}

fn centered_message(message: impl Into<String>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .flex_1()
        .text_color(rgb(PROPERTIES_MUTED_TEXT))
        .child(SharedString::from(message.into()))
        .into_any_element()
}

fn property_copy_text(label: &str, value: &str) -> String {
    let label = label.trim().trim_end_matches(':').trim_end();
    format!("{label}: {value}")
}

fn copy_property_to_clipboard(label: &str, value: &str, cx: &mut Context<PropertiesDialog>) {
    cx.write_to_clipboard(ClipboardItem::new_string(property_copy_text(label, value)));
}

fn property_row(
    id: &'static str,
    label: impl Into<String>,
    value: impl Into<String>,
    cx: &mut Context<PropertiesDialog>,
) -> AnyElement {
    let label = label.into();
    let value = value.into();
    let copied_label = label.clone();
    let copied_value = value.clone();
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_w(px(0.0))
        .min_h(px(PROPERTIES_ROW_HEIGHT))
        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
            copy_property_to_clipboard(&copied_label, &copied_value, cx);
            cx.stop_propagation();
        }))
        .child(
            div()
                .w(px(PROPERTIES_LABEL_WIDTH))
                .flex_shrink_0()
                .text_color(rgb(PROPERTIES_MUTED_TEXT))
                .child(SharedString::from(label)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .child(SharedString::from(value)),
        )
        .into_any_element()
}

fn detail_row(
    index: usize,
    label: &str,
    value: &str,
    cx: &mut Context<PropertiesDialog>,
) -> AnyElement {
    let copied_label = label.to_owned();
    let copied_value = value.to_owned();
    div()
        .id(detail_row_id(index))
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_w(px(0.0))
        .min_h(px(PROPERTIES_ROW_HEIGHT))
        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
            copy_property_to_clipboard(&copied_label, &copied_value, cx);
            cx.stop_propagation();
        }))
        .child(
            div()
                .w(px(154.0))
                .flex_shrink_0()
                .text_color(rgb(PROPERTIES_MUTED_TEXT))
                .truncate()
                .child(SharedString::from(label.to_owned())),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .child(SharedString::from(value.to_owned())),
        )
        .into_any_element()
}

fn detail_row_id(index: usize) -> (&'static str, usize) {
    ("properties-detail-row", index)
}

fn property_attributes_label(snapshot: &PropertySnapshot, draft: &EditablePropertyDraft) -> String {
    let readonly = draft
        .readonly
        .or(mixed_bool_value(&snapshot.attributes.readonly));
    let hidden = draft
        .hidden
        .or(mixed_bool_value(&snapshot.attributes.hidden));
    let mut labels = Vec::new();
    push_attribute_copy_label(&mut labels, "Read-only", readonly);
    push_attribute_copy_label(&mut labels, "Hidden", hidden);

    if labels.is_empty() {
        "None".to_owned()
    } else {
        labels.join(", ")
    }
}

fn push_attribute_copy_label(labels: &mut Vec<String>, label: &'static str, value: Option<bool>) {
    match value {
        Some(true) => labels.push(label.to_owned()),
        Some(false) => {}
        None => labels.push(format!("{label}: Mixed")),
    }
}

fn detail_group_header(title: &str) -> AnyElement {
    div()
        .mt(px(10.0))
        .w_full()
        .min_w(px(0.0))
        .overflow_hidden()
        .min_h(px(PROPERTIES_ROW_HEIGHT))
        .flex()
        .flex_row()
        .items_center()
        .child(
            div()
                .mr(px(8.0))
                .flex_shrink_0()
                .text_color(rgb(PROPERTIES_GROUP_TITLE))
                .child(SharedString::from(title.to_owned())),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .h(px(1.0))
                .bg(rgb(PROPERTIES_GROUP_TITLE)),
        )
        .into_any_element()
}

fn attribute_inline(
    label: &'static str,
    value: Option<bool>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let id = match label {
        "Read-only" => "properties-attribute-readonly",
        "Hidden" => "properties-attribute-hidden",
        _ => "properties-attribute",
    };
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .mr(px(20.0))
        .cursor_default()
        .child(check_box(value))
        .child(div().ml(px(6.0)).child(label))
        .on_click(on_click)
        .into_any_element()
}

fn check_box(value: Option<bool>) -> AnyElement {
    div()
        .w(px(16.0))
        .h(px(16.0))
        .border_1()
        .border_color(rgb(0x707070))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.0))
        .child(match value {
            Some(true) => "x",
            Some(false) => "",
            None => "-",
        })
        .into_any_element()
}

fn separator() -> AnyElement {
    div()
        .h(px(12.0))
        .flex()
        .items_center()
        .child(div().h(px(1.0)).w_full().bg(rgb(0xe5e5e5)))
        .into_any_element()
}

fn error_message(message: &str) -> AnyElement {
    div()
        .mt(px(10.0))
        .p(px(8.0))
        .border_1()
        .border_color(rgb(0xe81123))
        .text_color(rgb(0x9b0000))
        .child(SharedString::from(message.to_owned()))
        .into_any_element()
}

fn property_button(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    _: f32,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .min_w(px(PROPERTIES_BUTTON_MIN_WIDTH))
        .h(px(PROPERTIES_BUTTON_HEIGHT))
        .px(px(10.0))
        .border_1()
        .border_color(rgb(PROPERTIES_BORDER))
        .bg(rgb(0xfdfdfd))
        .when(!enabled, |this| this.opacity(0.45))
        .when(enabled, |this| {
            this.hover(|style| style.bg(rgb(0xe5f3ff)))
                .active(|style| style.bg(rgb(0xcce4f7)))
        })
        .flex()
        .items_center()
        .justify_center()
        .cursor_default()
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;
    use std::{collections::HashSet, time::Duration};

    #[test]
    fn snapshot_formats_single_file_core_fields() {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        fs::write(&file, b"abc").unwrap();

        let snapshot = collect_property_snapshot(PropertyTarget {
            paths: vec![file.clone()],
        })
        .unwrap();

        assert_eq!(snapshot.item_kind, PropertyItemKind::SingleFile);
        assert_eq!(snapshot.title, "a.txt");
        assert_eq!(snapshot.size, 3);
        assert!(snapshot.size_on_disk >= snapshot.size);
        assert!(matches!(snapshot.type_label, MixedValue::Single(_)));
        assert!(snapshot.contains.is_none());
    }

    #[test]
    fn snapshot_includes_folder_contains_count() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        let child = folder.join("child");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("a.txt"), b"a").unwrap();
        fs::create_dir(&child).unwrap();
        fs::write(child.join("b.txt"), b"bb").unwrap();
        fs::create_dir(child.join("grandchild")).unwrap();

        let snapshot = collect_property_snapshot(PropertyTarget {
            paths: vec![folder],
        })
        .unwrap();

        assert_eq!(snapshot.item_kind, PropertyItemKind::SingleFolder);
        assert_eq!(
            snapshot.contains,
            Some(PropertyContains {
                files: 2,
                folders: 2
            })
        );
        assert_eq!(snapshot.size, 3);
        assert!(snapshot.size_on_disk >= snapshot.size);
        assert!(snapshot.selection_counts.is_none());
    }

    #[test]
    fn multiselect_counts_selected_roots_and_descendants() {
        let temp = TempDir::new();
        let file = temp.path().join("root.txt");
        let folder = temp.path().join("folder");
        let nested = folder.join("nested");
        fs::write(&file, b"a").unwrap();
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("inside.txt"), b"bb").unwrap();
        fs::create_dir(&nested).unwrap();

        let snapshot = collect_property_snapshot(PropertyTarget {
            paths: vec![file, folder],
        })
        .unwrap();

        assert_eq!(snapshot.item_kind, PropertyItemKind::MultipleItems);
        assert_eq!(
            snapshot.selection_counts,
            Some(PropertyContains {
                files: 2,
                folders: 2
            })
        );
        assert!(snapshot.contains.is_none());
        assert_eq!(snapshot.size, 3);
        assert!(snapshot.size_on_disk >= snapshot.size);
        assert_eq!(
            selection_count_label(snapshot.selection_counts.as_ref().unwrap()),
            "2 Files, 2 Folders"
        );
        assert_eq!(type_of_file_label(&snapshot), "Multiple Types");
        assert_eq!(
            location_label(&snapshot),
            Some(format!("All in {}", temp.path().display()))
        );
    }

    #[test]
    fn multiselect_mixed_values_are_blankable() {
        let temp = TempDir::new();
        let first = temp.path().join("a.txt");
        let second = temp.path().join("b.md");
        fs::write(&first, b"a").unwrap();
        fs::write(&second, b"b").unwrap();

        let snapshot = collect_property_snapshot(PropertyTarget {
            paths: vec![first, second],
        })
        .unwrap();

        assert_eq!(snapshot.item_kind, PropertyItemKind::MultipleFiles);
        assert_eq!(snapshot.type_label, MixedValue::Mixed);
        assert_eq!(mixed_string_label(&snapshot.type_label), "");
    }

    #[test]
    fn blank_property_values_are_omitted() {
        assert_eq!(non_empty_property_value(String::new()), None);
        assert_eq!(
            non_empty_property_value(mixed_time_label(&MixedValue::Mixed, "")),
            None
        );
        assert_eq!(
            non_empty_property_value("Modified today".to_owned()),
            Some("Modified today".to_owned())
        );
    }

    #[test]
    fn properties_dialog_exposes_only_general_and_details_tabs() {
        assert_eq!(
            PROPERTY_TABS,
            &[
                (PropertyTab::General, "General"),
                (PropertyTab::Details, "Details")
            ]
        );
    }

    #[test]
    fn property_icon_source_uses_single_path_or_multi_copy_icon() {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        let folder = temp.path().join("folder");
        fs::write(&file, b"a").unwrap();
        fs::create_dir(&folder).unwrap();

        let single_file = collect_property_snapshot(PropertyTarget {
            paths: vec![file.clone()],
        })
        .unwrap();
        let single_folder = collect_property_snapshot(PropertyTarget {
            paths: vec![folder.clone()],
        })
        .unwrap();
        let mixed = collect_property_snapshot(PropertyTarget {
            paths: vec![file.clone(), folder],
        })
        .unwrap();

        assert_eq!(
            property_icon_source(&single_file),
            PropertyIconSource::Single(file)
        );
        assert_eq!(
            property_icon_source(&single_folder),
            PropertyIconSource::Single(single_folder.target.paths[0].clone())
        );
        assert_eq!(property_icon_source(&mixed), PropertyIconSource::Multiple);
    }

    #[test]
    fn type_of_file_label_matches_explorer_suffix_style() {
        let temp = TempDir::new();
        let gitignore = temp.path().join(".gitignore");
        let text = temp.path().join("note.txt");
        let no_extension = temp.path().join("Makefile");
        let folder = temp.path().join("folder");
        fs::write(&gitignore, b"target").unwrap();
        fs::write(&text, b"text").unwrap();
        fs::write(&no_extension, b"build").unwrap();
        fs::create_dir(&folder).unwrap();

        let gitignore = collect_property_snapshot(PropertyTarget {
            paths: vec![gitignore],
        })
        .unwrap();
        let text = collect_property_snapshot(PropertyTarget { paths: vec![text] }).unwrap();
        let no_extension = collect_property_snapshot(PropertyTarget {
            paths: vec![no_extension],
        })
        .unwrap();
        let folder = collect_property_snapshot(PropertyTarget {
            paths: vec![folder],
        })
        .unwrap();

        assert_eq!(
            type_of_file_label(&gitignore),
            "GITIGNORE File (.gitignore)"
        );
        assert_eq!(type_of_file_label(&text), "TXT File (.txt)");
        assert_eq!(type_of_file_label(&no_extension), "File");
        assert_eq!(type_of_file_label(&folder), "File folder");
    }

    #[test]
    fn size_label_includes_raw_bytes_for_scaled_units() {
        assert_eq!(property_size_label(99), "99 bytes");
        assert_eq!(property_size_label(2048), "2.0 KB (2,048 bytes)");
    }

    #[test]
    fn property_copy_text_formats_key_value_rows() {
        assert_eq!(property_copy_text("Size:", "2 KB"), "Size: 2 KB");
        assert_eq!(
            property_copy_text("Dimensions", "1920 x 1080"),
            "Dimensions: 1920 x 1080"
        );
    }

    #[test]
    fn general_property_row_ids_are_unique() {
        let unique_ids: HashSet<_> = PROPERTIES_GENERAL_PROPERTY_ROW_IDS
            .iter()
            .copied()
            .collect();
        assert_eq!(unique_ids.len(), PROPERTIES_GENERAL_PROPERTY_ROW_IDS.len());
    }

    #[test]
    fn detail_row_ids_are_unique_for_repeated_names() {
        let ids: HashSet<_> = (0..4).map(detail_row_id).collect();
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn details_scrollbar_metrics_only_exist_for_overflow() {
        assert!(details_scrollbar_metrics_for_dimensions(100.0, 0.0, 0.0).is_none());

        let metrics =
            details_scrollbar_metrics_for_dimensions(100.0, 50.0, 500.0).expect("overflow metrics");
        assert_eq!(metrics.viewport_height, 100.0);
        assert_eq!(metrics.content_height, 150.0);
        assert_eq!(metrics.scroll_max, 50.0);
        assert_eq!(metrics.scroll_top, 50.0);
    }

    #[test]
    fn file_group_is_created_for_all_file_types() {
        let temp = TempDir::new();
        let file = temp.path().join("note.txt");
        let image = temp.path().join("photo.jpg");
        let folder = temp.path().join("folder");
        fs::write(&file, b"not an image").unwrap();
        fs::write(&image, jpeg_with_exif(&exif_tiff("Canon", "TestCam", None))).unwrap();
        fs::create_dir(&folder).unwrap();

        for path in [file, image, folder] {
            let snapshot = collect_property_snapshot(PropertyTarget { paths: vec![path] }).unwrap();
            assert!(detail_group(&snapshot.details, PropertyDetailGroupKind::File).is_some());
        }
    }

    #[test]
    fn snapshot_excludes_exif_until_details_load() {
        let temp = TempDir::new();
        let file = temp.path().join("photo.jpg");
        fs::write(&file, jpeg_with_exif(&exif_tiff("Canon", "TestCam", None))).unwrap();

        let snapshot = collect_property_snapshot(PropertyTarget {
            paths: vec![file.clone()],
        })
        .unwrap();
        assert_eq!(
            detail_value(&snapshot.details, PropertyDetailGroupKind::Camera, "Make"),
            None
        );

        let exif_groups = collect_exif_detail_groups(&PropertyTarget { paths: vec![file] });

        assert_detail_contains(
            &exif_groups,
            PropertyDetailGroupKind::Camera,
            "Make",
            "Canon",
        );
        assert_detail_contains(
            &exif_groups,
            PropertyDetailGroupKind::Camera,
            "Model",
            "TestCam",
        );
        assert_group_has_detail(&exif_groups, PropertyDetailGroupKind::Misc, "Orientation");
    }

    #[test]
    fn duplicate_exif_tags_include_ifd_in_detail_name() {
        let temp = TempDir::new();
        let file = temp.path().join("photo.jpg");
        fs::write(
            &file,
            jpeg_with_exif(&exif_tiff("Canon", "TestCam", Some("Thumb"))),
        )
        .unwrap();

        let exif_groups = collect_exif_detail_groups(&PropertyTarget { paths: vec![file] });

        assert_detail_contains(
            &exif_groups,
            PropertyDetailGroupKind::Camera,
            "Make (IFD 0)",
            "Canon",
        );
        assert_detail_contains(
            &exif_groups,
            PropertyDetailGroupKind::Camera,
            "Make (IFD 1)",
            "Thumb",
        );
    }

    #[test]
    fn exif_details_are_grouped_by_standard_category() {
        let temp = TempDir::new();
        let file = temp.path().join("photo.jpg");
        fs::write(&file, jpeg_with_exif(&grouped_exif_tiff())).unwrap();

        let exif_groups = collect_exif_detail_groups(&PropertyTarget { paths: vec![file] });

        assert_detail_contains(
            &exif_groups,
            PropertyDetailGroupKind::Camera,
            "Make",
            "Canon",
        );
        assert_group_has_detail(&exif_groups, PropertyDetailGroupKind::Exposure, "ISO Speed");
        assert_group_has_detail(
            &exif_groups,
            PropertyDetailGroupKind::Gps,
            "GPS Latitude Ref",
        );
        assert_group_has_detail(&exif_groups, PropertyDetailGroupKind::Misc, "Orientation");
        assert_group_has_detail(
            &exif_groups,
            PropertyDetailGroupKind::NonStandard,
            "Tag(Tiff, 0xFDE8)",
        );
    }

    #[test]
    fn non_image_details_do_not_include_exif_fields() {
        let temp = TempDir::new();
        let file = temp.path().join("note.txt");
        fs::write(&file, b"not an image").unwrap();

        let snapshot = collect_property_snapshot(PropertyTarget { paths: vec![file] }).unwrap();

        assert_eq!(
            detail_value(&snapshot.details, PropertyDetailGroupKind::Camera, "Make"),
            None
        );
        assert_detail_contains(
            &snapshot.details,
            PropertyDetailGroupKind::File,
            "Size",
            "12 bytes",
        );
    }

    #[test]
    fn multiselect_exif_details_use_existing_mixed_value_behavior() {
        let temp = TempDir::new();
        let first = temp.path().join("a.jpg");
        let second = temp.path().join("b.jpg");
        fs::write(&first, jpeg_with_exif(&exif_tiff("Canon", "Same", None))).unwrap();
        fs::write(&second, jpeg_with_exif(&exif_tiff("Nikon", "Same", None))).unwrap();

        let exif_groups = collect_exif_detail_groups(&PropertyTarget {
            paths: vec![first, second],
        });

        assert_eq!(
            detail_value(&exif_groups, PropertyDetailGroupKind::Camera, "Make"),
            Some("")
        );
        assert_detail_contains(
            &exif_groups,
            PropertyDetailGroupKind::Camera,
            "Model",
            "Same",
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn default_app_change_error_reports_unverified_changes() {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        fs::write(&file, b"a").unwrap();
        let mut snapshot = collect_property_snapshot(PropertyTarget {
            paths: vec![file.clone()],
        })
        .unwrap();
        let before = Some(PropertyDefaultApp {
            name: "Old".to_owned(),
            path: None,
        });
        snapshot.default_app = before.clone();

        assert_eq!(
            default_app_change_error(
                &file,
                &before,
                &Ok(OpenWithOutcome::Cancelled),
                Some(&snapshot)
            ),
            None
        );
        assert!(
            default_app_change_error(
                &file,
                &before,
                &Ok(OpenWithOutcome::Opened),
                Some(&snapshot)
            )
            .unwrap()
            .contains("did not appear to change")
        );

        snapshot.default_app = Some(PropertyDefaultApp {
            name: "New".to_owned(),
            path: None,
        });
        assert_eq!(
            default_app_change_error(
                &file,
                &before,
                &Ok(OpenWithOutcome::Opened),
                Some(&snapshot)
            ),
            None
        );
    }

    #[test]
    fn apply_plan_omits_unchanged_fields() {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        fs::write(&file, b"a").unwrap();
        let snapshot = collect_property_snapshot(PropertyTarget { paths: vec![file] }).unwrap();
        let draft = EditablePropertyDraft::from_snapshot(&snapshot);

        let plan = property_apply_plan(&snapshot, &draft);

        assert!(property_apply_plan_is_empty(&plan));
    }

    #[test]
    fn apply_timestamp_changes_modified_time() {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        fs::write(&file, b"a").unwrap();
        let target = SystemTime::now() - Duration::from_secs(3600);

        let outcome = apply_property_draft(
            &[file.clone()],
            &EditablePropertyDraft {
                modified: Some(target),
                ..EditablePropertyDraft::default()
            },
        );

        assert!(outcome.errors.is_empty());
        let modified = fs::metadata(file).unwrap().modified().unwrap();
        assert!(
            modified
                .duration_since(target)
                .or_else(|_| target.duration_since(modified))
                .unwrap()
                < Duration::from_secs(3)
        );
    }

    #[test]
    fn apply_readonly_attribute_changes_permissions() {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        fs::write(&file, b"a").unwrap();

        let outcome = apply_property_draft(
            &[file.clone()],
            &EditablePropertyDraft {
                readonly: Some(true),
                ..EditablePropertyDraft::default()
            },
        );

        assert!(outcome.errors.is_empty());
        assert!(fs::metadata(&file).unwrap().permissions().readonly());

        let _ = apply_property_draft(
            &[file],
            &EditablePropertyDraft {
                readonly: Some(false),
                ..EditablePropertyDraft::default()
            },
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_mode_snapshot_and_apply_use_mode_bits() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new();
        let file = temp.path().join("script.sh");
        fs::write(&file, b"echo ok").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();

        let snapshot = collect_property_snapshot(PropertyTarget {
            paths: vec![file.clone()],
        })
        .unwrap();
        assert_eq!(snapshot.unix_mode, MixedValue::Single(0o644));

        let outcome = apply_property_draft(
            &[file.clone()],
            &EditablePropertyDraft {
                unix_mode: Some(0o755),
                ..EditablePropertyDraft::default()
            },
        );

        assert!(outcome.errors.is_empty());
        assert_eq!(
            fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    fn detail_group(
        groups: &[PropertyDetailGroup],
        kind: PropertyDetailGroupKind,
    ) -> Option<&PropertyDetailGroup> {
        groups.iter().find(|group| group.kind == kind)
    }

    fn detail_value<'a>(
        groups: &'a [PropertyDetailGroup],
        kind: PropertyDetailGroupKind,
        name: &str,
    ) -> Option<&'a str> {
        detail_group(groups, kind)?
            .details
            .iter()
            .find(|detail| detail.name == name)
            .map(|detail| detail.value.as_str())
    }

    fn assert_group_has_detail(
        groups: &[PropertyDetailGroup],
        kind: PropertyDetailGroupKind,
        name: &str,
    ) {
        let _ = detail_value(groups, kind, name)
            .unwrap_or_else(|| panic!("missing {kind:?} detail {name}"));
    }

    fn assert_detail_contains(
        groups: &[PropertyDetailGroup],
        kind: PropertyDetailGroupKind,
        name: &str,
        needle: &str,
    ) {
        let value = detail_value(groups, kind, name)
            .unwrap_or_else(|| panic!("missing {kind:?} detail {name}"));
        assert!(
            value.contains(needle),
            "expected {name} value {value:?} to contain {needle:?}"
        );
    }

    fn jpeg_with_exif(tiff: &[u8]) -> Vec<u8> {
        let app1_len = 2 + 6 + tiff.len();
        let mut jpeg = Vec::new();
        jpeg.extend_from_slice(&[0xff, 0xd8, 0xff, 0xe1]);
        jpeg.extend_from_slice(&(app1_len as u16).to_be_bytes());
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(tiff);
        jpeg.extend_from_slice(&[0xff, 0xd9]);
        jpeg
    }

    fn exif_tiff(make: &str, model: &str, thumbnail_make: Option<&str>) -> Vec<u8> {
        let make = ascii_exif_value(make);
        let model = ascii_exif_value(model);
        let thumbnail_make = thumbnail_make.map(ascii_exif_value);
        let ifd0_entry_count = 3u16;
        let ifd0_start = 8usize;
        let ifd0_end = ifd0_start + 2 + usize::from(ifd0_entry_count) * 12 + 4;
        let make_offset = ifd0_end;
        let model_offset = make_offset + make.len();
        let ifd1_offset = model_offset + model.len();

        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II");
        tiff.extend_from_slice(&42u16.to_le_bytes());
        tiff.extend_from_slice(&(ifd0_start as u32).to_le_bytes());
        tiff.extend_from_slice(&ifd0_entry_count.to_le_bytes());
        push_ifd_entry(&mut tiff, 0x010f, 2, make.len() as u32, make_offset as u32);
        push_ifd_entry(
            &mut tiff,
            0x0110,
            2,
            model.len() as u32,
            model_offset as u32,
        );
        push_ifd_entry(&mut tiff, 0x0112, 3, 1, 1);
        tiff.extend_from_slice(
            &thumbnail_make
                .as_ref()
                .map(|_| ifd1_offset as u32)
                .unwrap_or(0)
                .to_le_bytes(),
        );
        tiff.extend_from_slice(&make);
        tiff.extend_from_slice(&model);

        if let Some(thumbnail_make) = thumbnail_make {
            let thumbnail_value_offset = ifd1_offset + 2 + 12 + 4;
            tiff.extend_from_slice(&1u16.to_le_bytes());
            push_ifd_entry(
                &mut tiff,
                0x010f,
                2,
                thumbnail_make.len() as u32,
                thumbnail_value_offset as u32,
            );
            tiff.extend_from_slice(&0u32.to_le_bytes());
            tiff.extend_from_slice(&thumbnail_make);
        }

        tiff
    }

    fn grouped_exif_tiff() -> Vec<u8> {
        let make = ascii_exif_value("Canon");
        let model = ascii_exif_value("TestCam");
        let ifd0_entry_count = 6u16;
        let ifd0_start = 8usize;
        let ifd0_end = ifd0_start + 2 + usize::from(ifd0_entry_count) * 12 + 4;
        let make_offset = ifd0_end;
        let model_offset = make_offset + make.len();
        let exif_ifd_offset = model_offset + model.len();
        let gps_ifd_offset = exif_ifd_offset + 2 + 12 + 4;

        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II");
        tiff.extend_from_slice(&42u16.to_le_bytes());
        tiff.extend_from_slice(&(ifd0_start as u32).to_le_bytes());
        tiff.extend_from_slice(&ifd0_entry_count.to_le_bytes());
        push_ifd_entry(&mut tiff, 0x010f, 2, make.len() as u32, make_offset as u32);
        push_ifd_entry(
            &mut tiff,
            0x0110,
            2,
            model.len() as u32,
            model_offset as u32,
        );
        push_ifd_entry(&mut tiff, 0x0112, 3, 1, 1);
        push_ifd_entry(&mut tiff, 0x8769, 4, 1, exif_ifd_offset as u32);
        push_ifd_entry(&mut tiff, 0x8825, 4, 1, gps_ifd_offset as u32);
        push_ifd_entry(&mut tiff, 0xfde8, 3, 1, 42);
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.extend_from_slice(&make);
        tiff.extend_from_slice(&model);

        tiff.extend_from_slice(&1u16.to_le_bytes());
        push_ifd_entry(&mut tiff, 0x8833, 3, 1, 200);
        tiff.extend_from_slice(&0u32.to_le_bytes());

        tiff.extend_from_slice(&1u16.to_le_bytes());
        push_ifd_entry(&mut tiff, 0x0001, 2, 2, u32::from_le_bytes([b'N', 0, 0, 0]));
        tiff.extend_from_slice(&0u32.to_le_bytes());

        tiff
    }

    fn ascii_exif_value(value: &str) -> Vec<u8> {
        let mut bytes = value.as_bytes().to_vec();
        bytes.push(0);
        bytes
    }

    fn push_ifd_entry(tiff: &mut Vec<u8>, tag: u16, field_type: u16, count: u32, value: u32) {
        tiff.extend_from_slice(&tag.to_le_bytes());
        tiff.extend_from_slice(&field_type.to_le_bytes());
        tiff.extend_from_slice(&count.to_le_bytes());
        tiff.extend_from_slice(&value.to_le_bytes());
    }
}
