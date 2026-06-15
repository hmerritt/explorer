use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use filetime::{FileTime, set_file_times};
use gpui::{
    AnyElement, AnyWindowHandle, App, ClickEvent, Context, FocusHandle, Focusable, IntoElement,
    Render, SharedString, Task, TitlebarOptions, WeakEntity, Window, WindowBounds,
    WindowDecorations, WindowKind, WindowOptions, div, prelude::*, px, rgb, size,
};

use crate::explorer::{
    DialogCancel, DialogConfirm,
    entry::{DirectoryLinkKind, EntryKind},
    folder_size::calculate_folder_size,
    formatting::{format_size, format_timestamp},
    view::ExplorerView,
};
use crate::settings::SettingsState;

const PROPERTIES_WIDTH: f32 = 430.0;
const PROPERTIES_HEIGHT: f32 = 560.0;
const PROPERTIES_PADDING: f32 = 14.0;
const PROPERTIES_TAB_HEIGHT: f32 = 28.0;
const PROPERTIES_ROW_HEIGHT: f32 = 24.0;
const PROPERTIES_BUTTON_HEIGHT: f32 = 28.0;
const PROPERTIES_BUTTON_MIN_WIDTH: f32 = 78.0;
const PROPERTIES_LABEL_WIDTH: f32 = 122.0;
const PROPERTIES_BORDER: u32 = 0xd0d0d0;
const PROPERTIES_MUTED_TEXT: u32 = 0x666666;
const PROPERTIES_LINK_BLUE: u32 = 0x0067c0;

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
    pub(super) created: MixedValue<SystemTime>,
    pub(super) modified: MixedValue<SystemTime>,
    pub(super) accessed: MixedValue<SystemTime>,
    pub(super) attributes: PropertyAttributes,
    pub(super) owner: MixedValue<String>,
    pub(super) group: MixedValue<String>,
    pub(super) unix_mode: MixedValue<u32>,
    pub(super) permission_summary: MixedValue<String>,
    pub(super) shortcut: Option<ShortcutDetails>,
    pub(super) details: Vec<PropertyDetail>,
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
pub(super) struct ShortcutDetails {
    pub(super) target: String,
    pub(super) target_type: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PropertyDetail {
    pub(super) name: String,
    pub(super) value: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PropertyTab {
    General,
    Shortcut,
    Security,
    Details,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PropertySnapshotState {
    Loading,
    Ready(PropertySnapshot),
    Failed(String),
}

pub(super) struct PropertiesDialog {
    target: PropertyTarget,
    explorer: WeakEntity<ExplorerView>,
    date_format: String,
    font: gpui::Font,
    focus_handle: FocusHandle,
    active_tab: PropertyTab,
    snapshot_state: PropertySnapshotState,
    snapshot_task: Option<Task<()>>,
    apply_task: Option<Task<()>>,
    draft: EditablePropertyDraft,
    apply_error: Option<String>,
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
            snapshot_task: None,
            apply_task: None,
            draft: EditablePropertyDraft::default(),
            apply_error: None,
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
        let target = self.target.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { collect_property_snapshot(target) })
                .await;

            let _ = this.update(cx, |dialog, cx| {
                dialog.snapshot_state = match result {
                    Ok(snapshot) => {
                        dialog.draft = EditablePropertyDraft::from_snapshot(&snapshot);
                        PropertySnapshotState::Ready(snapshot)
                    }
                    Err(error) => PropertySnapshotState::Failed(error),
                };
                cx.notify();
            });
        });
        self.snapshot_task = Some(task);
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
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let outcome = apply_property_draft(&target.paths, &plan);
                    let snapshot = collect_property_snapshot(target).ok();
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
                    dialog.draft = EditablePropertyDraft::from_snapshot(&snapshot);
                    dialog.snapshot_state = PropertySnapshotState::Ready(snapshot);
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

    fn set_active_tab(&mut self, tab: PropertyTab, cx: &mut Context<Self>) {
        if self.active_tab != tab {
            self.active_tab = tab;
            cx.notify();
        }
    }

    fn set_timestamp_now(&mut self, which: TimestampField, cx: &mut Context<Self>) {
        let now = SystemTime::now();
        match which {
            TimestampField::Modified => self.draft.modified = Some(now),
            TimestampField::Accessed => self.draft.accessed = Some(now),
        }
        cx.notify();
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

    #[cfg(unix)]
    fn toggle_mode_bit(&mut self, bit: u32, cx: &mut Context<Self>) {
        let Some(current) = self
            .draft
            .unix_mode
            .or_else(|| snapshot_unix_mode(&self.snapshot_state))
        else {
            return;
        };
        self.draft.unix_mode = Some(current ^ bit);
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
            .bg(rgb(0xffffff))
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
                    .child(self.render_tabs(cx))
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
    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let show_shortcut = matches!(
            self.snapshot_state,
            PropertySnapshotState::Ready(PropertySnapshot {
                shortcut: Some(_),
                ..
            })
        );
        div()
            .flex()
            .flex_row()
            .h(px(PROPERTIES_TAB_HEIGHT))
            .border_b_1()
            .border_color(rgb(PROPERTIES_BORDER))
            .child(tab_button(
                "General",
                PropertyTab::General,
                self.active_tab,
                cx,
            ))
            .when(show_shortcut, |this| {
                this.child(tab_button(
                    "Shortcut",
                    PropertyTab::Shortcut,
                    self.active_tab,
                    cx,
                ))
            })
            .child(tab_button(
                "Security",
                PropertyTab::Security,
                self.active_tab,
                cx,
            ))
            .child(tab_button(
                "Details",
                PropertyTab::Details,
                self.active_tab,
                cx,
            ))
            .into_any_element()
    }

    fn render_body(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        match &self.snapshot_state {
            PropertySnapshotState::Loading => centered_message("Loading properties..."),
            PropertySnapshotState::Failed(error) => centered_message(error),
            PropertySnapshotState::Ready(snapshot) => match self.active_tab {
                PropertyTab::General => self.render_general(snapshot, window, cx),
                PropertyTab::Shortcut => self.render_shortcut(snapshot),
                PropertyTab::Security => self.render_security(snapshot, cx),
                PropertyTab::Details => self.render_details(snapshot),
            },
        }
    }

    fn render_general(
        &self,
        snapshot: &PropertySnapshot,
        _: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let created = mixed_time_label(&snapshot.created, &self.date_format);
        let modified = self
            .draft
            .modified
            .map(|time| format_timestamp(Some(time), &self.date_format))
            .unwrap_or_else(|| mixed_time_label(&snapshot.modified, &self.date_format));
        let accessed = self
            .draft
            .accessed
            .map(|time| format_timestamp(Some(time), &self.date_format))
            .unwrap_or_else(|| mixed_time_label(&snapshot.accessed, &self.date_format));

        div()
            .flex()
            .flex_col()
            .flex_1()
            .id("properties-general-body")
            .overflow_y_scroll()
            .pt(px(12.0))
            .child(title_row(&snapshot.title, snapshot.item_kind))
            .child(separator())
            .child(property_row(
                "Type",
                mixed_string_label(&snapshot.type_label),
            ))
            .child(property_row(
                "Location",
                mixed_string_label(&snapshot.location),
            ))
            .child(property_row("Size", format_size(Some(snapshot.size))))
            .child(property_row(
                "Size on disk",
                format_size(Some(snapshot.size_on_disk)),
            ))
            .when_some(snapshot.contains.as_ref(), |this, contains| {
                this.child(property_row(
                    "Contains",
                    format!("{} Files, {} Folders", contains.files, contains.folders),
                ))
            })
            .child(separator())
            .child(property_row("Created", created))
            .child(timestamp_row(
                "Modified",
                modified,
                TimestampField::Modified,
                cx,
            ))
            .child(timestamp_row(
                "Accessed",
                accessed,
                TimestampField::Accessed,
                cx,
            ))
            .child(separator())
            .child(attribute_row(
                "Read-only",
                self.draft
                    .readonly
                    .or(mixed_bool_value(&snapshot.attributes.readonly)),
                cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_readonly(cx)),
            ))
            .child(attribute_row(
                "Hidden",
                self.draft
                    .hidden
                    .or(mixed_bool_value(&snapshot.attributes.hidden)),
                cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_hidden(cx)),
            ))
            .when_some(self.apply_error.as_ref(), |this, error| {
                this.child(
                    div()
                        .mt(px(10.0))
                        .p(px(8.0))
                        .border_1()
                        .border_color(rgb(0xe81123))
                        .text_color(rgb(0x9b0000))
                        .child(SharedString::from(error.clone())),
                )
            })
            .into_any_element()
    }

    fn render_shortcut(&self, snapshot: &PropertySnapshot) -> AnyElement {
        let Some(shortcut) = snapshot.shortcut.as_ref() else {
            return centered_message("No shortcut details are available for this selection.");
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .id("properties-shortcut-body")
            .overflow_y_scroll()
            .pt(px(12.0))
            .child(property_row("Target type", shortcut.target_type.clone()))
            .child(property_row("Target", shortcut.target.clone()))
            .child(property_row(
                "Status",
                if Path::new(&shortcut.target).exists() {
                    "Available".to_owned()
                } else {
                    "Target not found".to_owned()
                },
            ))
            .into_any_element()
    }

    fn render_security(&self, snapshot: &PropertySnapshot, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .id("properties-security-body")
            .overflow_y_scroll()
            .pt(px(12.0))
            .child(property_row("Owner", mixed_string_label(&snapshot.owner)))
            .child(property_row("Group", mixed_string_label(&snapshot.group)))
            .child(property_row(
                "Permissions",
                mixed_string_label(&snapshot.permission_summary),
            ))
            .child(separator())
            .child(security_permissions_element(
                self.draft
                    .unix_mode
                    .or_else(|| mixed_u32_value(&snapshot.unix_mode)),
                cx,
            ))
            .into_any_element()
    }

    fn render_details(&self, snapshot: &PropertySnapshot) -> AnyElement {
        let mut body = div()
            .flex()
            .flex_col()
            .flex_1()
            .id("properties-details-body")
            .overflow_y_scroll()
            .pt(px(12.0));

        for detail in &snapshot.details {
            body = body.child(property_row(&detail.name, detail.value.clone()));
        }

        if snapshot.details.is_empty() {
            body.child(
                div()
                    .text_color(rgb(PROPERTIES_MUTED_TEXT))
                    .child("No additional metadata is available."),
            )
            .into_any_element()
        } else {
            body.into_any_element()
        }
    }

    fn render_buttons(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let has_changes = self.has_changes();
        div()
            .flex()
            .flex_row()
            .justify_end()
            .gap(px(8.0))
            .pt(px(12.0))
            .border_t_1()
            .border_color(rgb(0xe5e5e5))
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimestampField {
    Modified,
    Accessed,
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
        is_minimizable: true,
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

fn collect_property_snapshot(target: PropertyTarget) -> Result<PropertySnapshot, String> {
    if target.paths.is_empty() {
        return Err("No items selected.".to_owned());
    }

    let mut items = Vec::new();
    for path in &target.paths {
        items.push(collect_property_item(path));
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
    let contains = contains_summary(&items);
    let shortcut = (items.len() == 1)
        .then(|| items[0].shortcut.clone())
        .flatten();
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
        created,
        modified,
        accessed,
        attributes: PropertyAttributes { readonly, hidden },
        owner,
        group,
        unix_mode,
        permission_summary,
        shortcut,
        details,
    })
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
    details: Vec<PropertyDetail>,
}

fn collect_property_item(path: &Path) -> PropertyItem {
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
    let size = if is_dir {
        calculate_folder_size(path, &std::sync::atomic::AtomicBool::new(false)).ok()
    } else {
        metadata.as_ref().map(|metadata| metadata.len())
    };
    let size_on_disk = metadata.as_ref().map(|metadata| {
        size_on_disk(path, metadata).unwrap_or_else(|| size.unwrap_or(metadata.len()))
    });
    let contains = if is_dir {
        count_directory_children(path)
    } else {
        None
    };
    let readonly = metadata
        .as_ref()
        .map(|metadata| metadata.permissions().readonly());
    let hidden = Some(path_is_hidden(path, metadata.as_ref()));
    let shortcut = shortcut_details(path, entry.as_ref());
    let details = metadata_details(path, entry.as_ref(), metadata.as_ref());

    PropertyItem {
        path: path.to_path_buf(),
        exists,
        is_dir,
        type_label: entry.as_ref().map(|entry| entry.type_label()),
        location: path.parent().map(|parent| parent.display().to_string()),
        size,
        size_on_disk,
        contains,
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

fn contains_summary(items: &[PropertyItem]) -> Option<PropertyContains> {
    let mut files = 0;
    let mut folders = 0;
    let mut has_directory = false;
    for item in items {
        if let Some(contains) = &item.contains {
            has_directory = true;
            files += contains.files;
            folders += contains.folders;
        }
    }
    has_directory.then_some(PropertyContains { files, folders })
}

fn count_directory_children(path: &Path) -> Option<PropertyContains> {
    let entries = fs::read_dir(path).ok()?;
    let mut files = 0;
    let mut folders = 0;
    for entry in entries.flatten() {
        match entry.file_type() {
            Ok(file_type) if file_type.is_dir() => folders += 1,
            Ok(_) => files += 1,
            Err(_) => {}
        }
    }
    Some(PropertyContains { files, folders })
}

fn merged_details(items: &[PropertyItem]) -> Vec<PropertyDetail> {
    let mut values: BTreeMap<String, MixedValue<String>> = BTreeMap::new();
    for item in items {
        for detail in &item.details {
            let entry = values.remove(&detail.name).unwrap_or(MixedValue::None);
            values.insert(
                detail.name.clone(),
                mix_value(entry, Some(detail.value.clone())),
            );
        }
    }
    values
        .into_iter()
        .map(|(name, value)| PropertyDetail {
            name,
            value: mixed_string_label(&value),
        })
        .collect()
}

fn metadata_details(
    path: &Path,
    entry: Option<&crate::explorer::FileEntry>,
    metadata: Option<&fs::Metadata>,
) -> Vec<PropertyDetail> {
    let mut details = Vec::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        details.push(PropertyDetail {
            name: "Extension".to_owned(),
            value: extension.to_owned(),
        });
    }
    if let Some(entry) = entry {
        details.push(PropertyDetail {
            name: "Item type".to_owned(),
            value: entry.type_label(),
        });
    }
    if let Some(metadata) = metadata {
        details.push(PropertyDetail {
            name: "Bytes".to_owned(),
            value: metadata.len().to_string(),
        });
    }
    if let Ok((width, height)) = image::image_dimensions(path) {
        details.push(PropertyDetail {
            name: "Dimensions".to_owned(),
            value: format!("{width} x {height}"),
        });
    }
    details
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

fn size_on_disk(_: &Path, metadata: &fs::Metadata) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        return Some(metadata.blocks().saturating_mul(512));
    }

    #[cfg(not(unix))]
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

#[cfg(unix)]
fn snapshot_unix_mode(state: &PropertySnapshotState) -> Option<u32> {
    match state {
        PropertySnapshotState::Ready(snapshot) => mixed_u32_value(&snapshot.unix_mode),
        _ => None,
    }
}

fn tab_button(
    label: &'static str,
    tab: PropertyTab,
    active: PropertyTab,
    cx: &mut Context<PropertiesDialog>,
) -> AnyElement {
    let id = match tab {
        PropertyTab::General => "properties-tab-general",
        PropertyTab::Shortcut => "properties-tab-shortcut",
        PropertyTab::Security => "properties-tab-security",
        PropertyTab::Details => "properties-tab-details",
    };
    div()
        .id(id)
        .h(px(PROPERTIES_TAB_HEIGHT))
        .px(px(12.0))
        .flex()
        .items_center()
        .border_1()
        .border_color(rgb(if active == tab {
            PROPERTIES_BORDER
        } else {
            0xffffff
        }))
        .bg(rgb(if active == tab { 0xffffff } else { 0xf4f4f4 }))
        .cursor_default()
        .child(label)
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_active_tab(tab, cx)))
        .into_any_element()
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

fn title_row(title: &str, kind: PropertyItemKind) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .pb(px(8.0))
        .child(
            div()
                .w(px(32.0))
                .h(px(32.0))
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(rgb(PROPERTIES_BORDER))
                .child(match kind {
                    PropertyItemKind::SingleFolder | PropertyItemKind::MultipleFolders => "[]",
                    PropertyItemKind::SingleShortcut => "->",
                    _ => "*",
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(14.0))
                .child(SharedString::from(title.to_owned())),
        )
        .into_any_element()
}

fn property_row(label: impl Into<String>, value: impl Into<String>) -> AnyElement {
    let label = label.into();
    let value = value.into();
    div()
        .flex()
        .flex_row()
        .items_center()
        .min_h(px(PROPERTIES_ROW_HEIGHT))
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

fn timestamp_row(
    label: &'static str,
    value: String,
    which: TimestampField,
    cx: &mut Context<PropertiesDialog>,
) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .min_h(px(PROPERTIES_ROW_HEIGHT))
        .child(
            div()
                .w(px(PROPERTIES_LABEL_WIDTH))
                .flex_shrink_0()
                .text_color(rgb(PROPERTIES_MUTED_TEXT))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .child(SharedString::from(value)),
        )
        .child(mini_button("Set to now").on_click(
            cx.listener(move |this, _: &ClickEvent, _, cx| this.set_timestamp_now(which, cx)),
        ))
        .into_any_element()
}

fn attribute_row(
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
        .min_h(px(PROPERTIES_ROW_HEIGHT))
        .cursor_default()
        .child(
            div()
                .w(px(PROPERTIES_LABEL_WIDTH))
                .flex_shrink_0()
                .text_color(rgb(PROPERTIES_MUTED_TEXT))
                .child(label),
        )
        .child(check_box(value))
        .child(div().ml(px(8.0)).child(match value {
            Some(true) => "On",
            Some(false) => "Off",
            None => "Mixed",
        }))
        .on_click(on_click)
        .into_any_element()
}

fn check_box(value: Option<bool>) -> AnyElement {
    div()
        .w(px(14.0))
        .h(px(14.0))
        .border_1()
        .border_color(rgb(0x707070))
        .flex()
        .items_center()
        .justify_center()
        .child(match value {
            Some(true) => "x",
            Some(false) => "",
            None => "-",
        })
        .into_any_element()
}

#[cfg(unix)]
fn security_permissions_element(
    mode: Option<u32>,
    cx: &mut Context<PropertiesDialog>,
) -> AnyElement {
    permission_matrix(mode, cx)
}

#[cfg(not(unix))]
fn security_permissions_element(_: Option<u32>, _: &mut Context<PropertiesDialog>) -> AnyElement {
    div()
        .text_color(rgb(PROPERTIES_MUTED_TEXT))
        .child("Security details are read-only in this version.")
        .into_any_element()
}

#[cfg(unix)]
fn permission_matrix(mode: Option<u32>, cx: &mut Context<PropertiesDialog>) -> AnyElement {
    let mode = mode.unwrap_or(0);
    div()
        .id(("properties-permission", label, bit))
        .flex()
        .flex_col()
        .gap(px(5.0))
        .child(property_row("Mode", format!("{mode:o}")))
        .child(permission_row("Owner", mode, [0o400, 0o200, 0o100], cx))
        .child(permission_row("Group", mode, [0o040, 0o020, 0o010], cx))
        .child(permission_row("Everyone", mode, [0o004, 0o002, 0o001], cx))
        .into_any_element()
}

#[cfg(unix)]
fn permission_row(
    label: &'static str,
    mode: u32,
    bits: [u32; 3],
    cx: &mut Context<PropertiesDialog>,
) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .min_h(px(PROPERTIES_ROW_HEIGHT))
        .child(
            div()
                .w(px(PROPERTIES_LABEL_WIDTH))
                .flex_shrink_0()
                .text_color(rgb(PROPERTIES_MUTED_TEXT))
                .child(label),
        )
        .child(permission_cell("Read", mode, bits[0], cx))
        .child(permission_cell("Write", mode, bits[1], cx))
        .child(permission_cell("Execute", mode, bits[2], cx))
        .into_any_element()
}

#[cfg(unix)]
fn permission_cell(
    label: &'static str,
    mode: u32,
    bit: u32,
    cx: &mut Context<PropertiesDialog>,
) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .mr(px(8.0))
        .cursor_default()
        .child(check_box(Some(mode & bit != 0)))
        .child(div().ml(px(4.0)).child(label))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_mode_bit(bit, cx)))
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

fn mini_button(label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id("properties-mini-button")
        .h(px(22.0))
        .px(px(8.0))
        .border_1()
        .border_color(rgb(PROPERTIES_BORDER))
        .bg(rgb(0xfdfdfd))
        .hover(|style| style.bg(rgb(0xe5f3ff)))
        .active(|style| style.bg(rgb(0xcce4f7)))
        .flex()
        .items_center()
        .justify_center()
        .cursor_default()
        .text_color(rgb(PROPERTIES_LINK_BLUE))
        .child(label)
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
    use std::time::Duration;

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
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("a.txt"), b"a").unwrap();
        fs::create_dir(folder.join("child")).unwrap();

        let snapshot = collect_property_snapshot(PropertyTarget {
            paths: vec![folder],
        })
        .unwrap();

        assert_eq!(snapshot.item_kind, PropertyItemKind::SingleFolder);
        assert_eq!(
            snapshot.contains,
            Some(PropertyContains {
                files: 1,
                folders: 1
            })
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
}
