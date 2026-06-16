use std::fmt::Write as _;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    io::BufReader,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::{Instant, SystemTime},
};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use filetime::{FileTime, set_file_times};
use gpui::{
    AnyElement, AnyWindowHandle, App, ClickEvent, ClipboardItem, Context, FocusHandle, Focusable,
    Image, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit,
    Render, RenderImage, ScrollHandle, ScrollWheelEvent, SharedString, StyledImage, Task, TextRun,
    TitlebarOptions, WeakEntity, Window, WindowBounds, WindowDecorations, WindowKind,
    WindowOptions, canvas, div, point, prelude::*, px, rgb, size,
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
use crate::loaders::{LinearProgressStyle, linear_indeterminate};
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
const PROPERTIES_FRAME_LIST_GAP: f32 = 16.0;
const PROPERTIES_FRAME_LABEL_GAP: f32 = 4.0;
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
    Media,
    Video,
    Audio,
    Subtitles,
    Chapters,
    Camera,
    Exposure,
    Gps,
    Exifmeta,
    Misc,
    NonStandard,
}

impl PropertyDetailGroupKind {
    fn title(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Media => "media",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Subtitles => "subtitles",
            Self::Chapters => "chapters",
            Self::Camera => "camera",
            Self::Exposure => "exposure",
            Self::Gps => "gps",
            Self::Exifmeta => "exifmeta",
            Self::Misc => "misc",
            Self::NonStandard => "non-standard",
        }
    }
}

const PROPERTY_DETAIL_GROUP_ORDER: &[PropertyDetailGroupKind] = &[
    PropertyDetailGroupKind::File,
    PropertyDetailGroupKind::Media,
    PropertyDetailGroupKind::Video,
    PropertyDetailGroupKind::Audio,
    PropertyDetailGroupKind::Subtitles,
    PropertyDetailGroupKind::Chapters,
    PropertyDetailGroupKind::Camera,
    PropertyDetailGroupKind::Exposure,
    PropertyDetailGroupKind::Gps,
    PropertyDetailGroupKind::Exifmeta,
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
    Frames,
}

const PROPERTY_TABS: &[(PropertyTab, &str)] = &[
    (PropertyTab::General, "General"),
    (PropertyTab::Details, "Details"),
    (PropertyTab::Frames, "Frames"),
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

enum PropertyFramesState {
    NotStarted,
    Loading(Vec<PropertyFrameThumbnail>),
    Ready(Vec<PropertyFrameThumbnail>),
    Failed(String),
}

#[derive(Clone, Debug)]
struct PropertyFrameThumbnail {
    label: String,
    image: Arc<RenderImage>,
    aspect_ratio: f32,
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
    frames_state: PropertyFramesState,
    frames_generation: u64,
    frames_scroll_handle: ScrollHandle,
    frames_scrollbar_hovered: bool,
    frames_scrollbar_drag: Option<ScrollbarDrag>,
    snapshot_task: Option<Task<()>>,
    details_task: Option<Task<()>>,
    frames_task: Option<Task<()>>,
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
            frames_state: PropertyFramesState::NotStarted,
            frames_generation: 0,
            frames_scroll_handle: ScrollHandle::new(),
            frames_scrollbar_hovered: false,
            frames_scrollbar_drag: None,
            snapshot_task: None,
            details_task: None,
            frames_task: None,
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
        self.reset_frames_state();
        let target = self.target.clone();
        let date_format = self.date_format.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let path_count = target.paths.len();
                    let started = Instant::now();
                    let result = collect_property_snapshot_with_date_format(target, &date_format);
                    match &result {
                        Ok(snapshot) => crate::debug_options::log_property_timing(
                            started.elapsed(),
                            format_args!(
                                "general snapshot ready paths={} details={} title={:?}",
                                path_count,
                                detail_count(&snapshot.details),
                                snapshot.title
                            ),
                        ),
                        Err(error) => crate::debug_options::log_property_timing(
                            started.elapsed(),
                            format_args!(
                                "general snapshot failed paths={} error={:?}",
                                path_count, error
                            ),
                        ),
                    }
                    result
                })
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

    fn reset_frames_state(&mut self) {
        self.frames_generation = self.frames_generation.wrapping_add(1);
        self.frames_state = PropertyFramesState::NotStarted;
        self.frames_task = None;
        self.frames_scrollbar_drag = None;
        let offset = self.frames_scroll_handle.offset();
        self.frames_scroll_handle
            .set_offset(point(offset.x, px(0.0)));
    }

    fn set_ready_snapshot(&mut self, snapshot: PropertySnapshot, cx: &mut Context<Self>) {
        self.draft = EditablePropertyDraft::from_snapshot(&snapshot);
        self.snapshot_state = PropertySnapshotState::Ready(snapshot);
        self.reset_details_state();
        self.reset_frames_state();
        if let PropertySnapshotState::Ready(snapshot) = &self.snapshot_state {
            if !property_tab_is_visible(self.active_tab, Some(snapshot)) {
                self.active_tab = PropertyTab::General;
            }
        }
        match self.active_tab {
            PropertyTab::Details => self.start_details_task(cx),
            PropertyTab::Frames => self.start_frames_task(cx),
            PropertyTab::General => {}
        }
    }

    fn start_details_task(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.details_state, PropertyDetailsState::NotStarted) {
            return;
        }
        let PropertySnapshotState::Ready(snapshot) = &self.snapshot_state else {
            return;
        };
        if single_file_media_path(&snapshot.target, snapshot.item_kind).is_none() {
            self.details_state = PropertyDetailsState::Ready(Vec::new());
            return;
        }

        self.details_state = PropertyDetailsState::Loading;
        let target = snapshot.target.clone();
        let item_kind = snapshot.item_kind;
        let generation = self.details_generation;
        let task = cx.spawn(async move |this, cx| {
            let groups = cx
                .background_executor()
                .spawn(async move {
                    let path_count = target.paths.len();
                    let started = Instant::now();
                    let groups = collect_single_file_media_detail_groups(&target, item_kind);
                    crate::debug_options::log_property_timing(
                        started.elapsed(),
                        format_args!(
                            "details ready paths={} groups={} details={}",
                            path_count,
                            groups.len(),
                            detail_count(&groups)
                        ),
                    );
                    groups
                })
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

    fn start_frames_task(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.frames_state, PropertyFramesState::NotStarted) {
            return;
        }
        let PropertySnapshotState::Ready(snapshot) = &self.snapshot_state else {
            return;
        };
        let Some(path) =
            single_file_video_path(&snapshot.target, snapshot.item_kind).map(Path::to_path_buf)
        else {
            self.frames_state = PropertyFramesState::Failed(
                "Video frames are not available for this item.".to_owned(),
            );
            return;
        };

        self.frames_state = PropertyFramesState::Loading(Vec::new());
        let generation = self.frames_generation;
        let task = cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let requests = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move { prepare_video_frame_requests(&path) }
                })
                .await;

            let requests = match requests {
                Ok(requests) => requests,
                Err(error) => {
                    crate::debug_options::log_property_timing(
                        started.elapsed(),
                        format_args!(
                            "video frames failed path={} error={}",
                            path.display(),
                            error
                        ),
                    );
                    let _ = this.update(cx, |dialog, cx| {
                        if dialog.frames_generation == generation {
                            dialog.frames_task = None;
                            dialog.frames_state = PropertyFramesState::Failed(error);
                            cx.notify();
                        }
                    });
                    return;
                }
            };

            let mut errors = Vec::new();
            let mut frame_count = 0usize;
            for request in requests {
                let frame = cx
                    .background_executor()
                    .spawn({
                        let path = path.clone();
                        async move {
                            extract_video_frame_png(&path, request)
                                .and_then(prepare_video_frame_thumbnail)
                        }
                    })
                    .await;

                match frame {
                    Ok(thumbnail) => {
                        let should_continue = this
                            .update(cx, |dialog, cx| {
                                if dialog.frames_generation != generation {
                                    return false;
                                }
                                let PropertyFramesState::Loading(frames) = &mut dialog.frames_state
                                else {
                                    return false;
                                };
                                frames.push(thumbnail);
                                frame_count = frames.len();
                                cx.notify();
                                true
                            })
                            .unwrap_or(false);
                        if !should_continue {
                            return;
                        }
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(
                                VIDEO_FRAME_PUBLISH_INTERVAL_MS,
                            ))
                            .await;
                    }
                    Err(error) => errors.push(error),
                }
            }

            let failed = frame_count == 0;
            let error = failed.then(|| {
                format!(
                    "ffmpeg failed to extract video frames: {}",
                    frame_extraction_error_summary(&errors)
                )
            });
            if let Some(error) = error.as_ref() {
                crate::debug_options::log_property_timing(
                    started.elapsed(),
                    format_args!(
                        "video frames failed path={} error={}",
                        path.display(),
                        error
                    ),
                );
            } else {
                crate::debug_options::log_property_timing(
                    started.elapsed(),
                    format_args!(
                        "video frames ready path={} frames={}",
                        path.display(),
                        frame_count
                    ),
                );
            }

            let _ = this.update(cx, |dialog, cx| {
                if dialog.frames_generation == generation {
                    dialog.frames_task = None;
                    dialog.frames_state = if let Some(error) = error {
                        PropertyFramesState::Failed(error)
                    } else {
                        let frames = match std::mem::replace(
                            &mut dialog.frames_state,
                            PropertyFramesState::NotStarted,
                        ) {
                            PropertyFramesState::Loading(frames)
                            | PropertyFramesState::Ready(frames) => frames,
                            PropertyFramesState::NotStarted | PropertyFramesState::Failed(_) => {
                                Vec::new()
                            }
                        };
                        PropertyFramesState::Ready(frames)
                    };
                    cx.notify();
                }
            });
        });
        self.frames_task = Some(task);
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
        let snapshot = match &self.snapshot_state {
            PropertySnapshotState::Ready(snapshot) => Some(snapshot),
            PropertySnapshotState::Loading | PropertySnapshotState::Failed(_) => None,
        };
        if !property_tab_is_visible(tab, snapshot) {
            return;
        }
        if self.active_tab != tab {
            self.active_tab = tab;
            if tab == PropertyTab::Details {
                self.start_details_task(cx);
            } else if tab == PropertyTab::Frames {
                self.start_frames_task(cx);
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
        let snapshot = self.ready_snapshot();
        for (tab, label) in property_tabs_for_snapshot(snapshot) {
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
        let snapshot = self.ready_snapshot();
        for (tab, label) in property_tabs_for_snapshot(snapshot) {
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
                PropertyTab::Frames => self.render_frames(&snapshot, cx),
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
            .child(body)
            .into_any_element()
    }

    fn ready_snapshot(&self) -> Option<&PropertySnapshot> {
        match &self.snapshot_state {
            PropertySnapshotState::Ready(snapshot) => Some(snapshot),
            PropertySnapshotState::Loading | PropertySnapshotState::Failed(_) => None,
        }
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
            .p(px(PROPERTIES_PANEL_PADDING))
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
            .p(px(PROPERTIES_PANEL_PADDING))
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
                    .child("Loading media metadata..."),
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

    fn render_frames(&mut self, snapshot: &PropertySnapshot, cx: &mut Context<Self>) -> AnyElement {
        if single_file_video_path(&snapshot.target, snapshot.item_kind).is_none() {
            return centered_message("Video frames are not available for this item.");
        }

        let mut body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .w_full()
            .id("properties-frames-body")
            .overflow_y_scroll()
            .scrollbar_width(px(0.0))
            .p(px(PROPERTIES_PANEL_PADDING))
            .track_scroll(&self.frames_scroll_handle)
            .on_scroll_wheel(cx.listener(|_: &mut Self, _: &ScrollWheelEvent, _, cx| {
                cx.notify();
            }));

        let loading_frames = matches!(self.frames_state, PropertyFramesState::Loading(_));
        match &self.frames_state {
            PropertyFramesState::NotStarted => {}
            PropertyFramesState::Loading(frames) if frames.is_empty() => {}
            PropertyFramesState::Loading(frames) => {
                body = body.child(frame_thumbnail_list(frames));
            }
            PropertyFramesState::Failed(error) => {
                body = body.child(
                    div()
                        .min_w(px(0.0))
                        .w_full()
                        .text_color(rgb(PROPERTIES_MUTED_TEXT))
                        .child(SharedString::from(error.clone())),
                );
            }
            PropertyFramesState::Ready(frames) => {
                if frames.is_empty() {
                    body = body.child(
                        div()
                            .min_w(px(0.0))
                            .w_full()
                            .text_color(rgb(PROPERTIES_MUTED_TEXT))
                            .truncate()
                            .child("No video frames are available."),
                    );
                } else {
                    body = body.child(frame_thumbnail_list(frames));
                }
            }
        }

        let has_scrollbar = self.frames_scrollbar_metrics().is_some();
        let content = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .w_full()
            .overflow_hidden()
            .child(body)
            .when(has_scrollbar, |this| {
                this.child(self.render_frames_scrollbar(cx))
            });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .w_full()
            .overflow_hidden()
            .child(content)
            .when(loading_frames, |this| {
                this.child(linear_indeterminate(
                    "properties-frames-linear-progress",
                    LinearProgressStyle::explorer_copy_green(),
                ))
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

    fn frames_scrollbar_metrics(&self) -> Option<ScrollbarMetrics> {
        let viewport_height = f32::from(self.frames_scroll_handle.bounds().size.height);
        let scroll_max = f32::from(self.frames_scroll_handle.max_offset().height);
        let scroll_top = -f32::from(self.frames_scroll_handle.offset().y);
        frames_scrollbar_metrics_for_dimensions(viewport_height, scroll_max, scroll_top)
    }

    fn set_frames_scroll_top(&self, scroll_top: f32) {
        let scroll_top = self
            .frames_scrollbar_metrics()
            .map_or(0.0, |metrics| metrics.clamp_scroll_top(scroll_top));
        let offset = self.frames_scroll_handle.offset();
        self.frames_scroll_handle
            .set_offset(point(offset.x, px(-scroll_top)));
    }

    fn handle_frames_scrollbar_mouse_down(&mut self, local_y: f32, metrics: ScrollbarMetrics) {
        if local_y < SCROLLBAR_ARROW_HEIGHT {
            self.set_frames_scroll_top(metrics.scroll_by(-PROPERTIES_ROW_HEIGHT));
        } else if local_y > metrics.viewport_height - SCROLLBAR_ARROW_HEIGHT {
            self.set_frames_scroll_top(metrics.scroll_by(PROPERTIES_ROW_HEIGHT));
        } else if local_y >= metrics.thumb_top && local_y <= metrics.thumb_bottom() {
            self.frames_scrollbar_drag = Some(ScrollbarDrag {
                pointer_offset_from_thumb_top: local_y - metrics.thumb_top,
            });
        } else if local_y < metrics.thumb_top {
            self.set_frames_scroll_top(metrics.scroll_by(-metrics.viewport_height));
        } else {
            self.set_frames_scroll_top(metrics.scroll_by(metrics.viewport_height));
        }
    }

    fn handle_frames_scrollbar_drag(&mut self, local_y: f32, metrics: ScrollbarMetrics) {
        let Some(drag) = self.frames_scrollbar_drag else {
            return;
        };

        let thumb_top = local_y - drag.pointer_offset_from_thumb_top;
        self.set_frames_scroll_top(metrics.scroll_top_for_thumb_top(thumb_top));
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

    fn render_frames_scrollbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(metrics) = self.frames_scrollbar_metrics() else {
            return div().into_any_element();
        };

        let hovered_or_dragged =
            self.frames_scrollbar_hovered || self.frames_scrollbar_drag.is_some();
        let thumb_width = if hovered_or_dragged {
            SCROLLBAR_THUMB_HOVER_WIDTH
        } else {
            SCROLLBAR_THUMB_WIDTH
        };
        let thumb_right = (SCROLLBAR_GUTTER_WIDTH - thumb_width) / 2.0;
        let thumb_color = if self.frames_scrollbar_drag.is_some() {
            SCROLLBAR_THUMB_ACTIVE_BG
        } else if hovered_or_dragged {
            SCROLLBAR_THUMB_HOVER_BG
        } else {
            SCROLLBAR_THUMB_BG
        };
        let bottom_arrow_top = (metrics.viewport_height - SCROLLBAR_ARROW_HEIGHT).max(0.0);

        div()
            .id("properties-frames-scrollbar")
            .relative()
            .w(px(SCROLLBAR_GUTTER_WIDTH))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(SCROLLBAR_TRACK_BG))
            .cursor_default()
            .block_mouse_except_scroll()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.frames_scrollbar_hovered = *hovered;
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
            .child(self.render_frames_scrollbar_hit_layer(cx))
            .into_any_element()
    }

    fn render_frames_scrollbar_hit_layer(&self, cx: &mut Context<Self>) -> AnyElement {
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
                            if let Some(metrics) = this.frames_scrollbar_metrics() {
                                this.handle_frames_scrollbar_mouse_down(local_y, metrics);
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
                            if this.frames_scrollbar_drag.is_none() {
                                return;
                            }

                            if let Some(metrics) = this.frames_scrollbar_metrics() {
                                this.handle_frames_scrollbar_drag(local_y, metrics);
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
                        if this.frames_scrollbar_drag.take().is_some() {
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

fn detail_count(groups: &[PropertyDetailGroup]) -> usize {
    groups.iter().map(|group| group.details.len()).sum()
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

fn path_may_have_media_details(path: &Path) -> bool {
    path_may_have_exif(path) || path_may_have_video_metadata(path)
}

fn path_may_have_video_metadata(path: &Path) -> bool {
    if mime_guess::from_path(path)
        .first_raw()
        .is_some_and(|mime| mime.starts_with("video/"))
    {
        return true;
    }

    let Some(extension) = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
    else {
        return false;
    };

    VIDEO_METADATA_EXTENSIONS.contains(&extension.as_str())
}

const VIDEO_METADATA_EXTENSIONS: &[&str] = &[
    "webm", "mkv", "flv", "vob", "ogv", "ogg", "rrc", "gifv", "mng", "mov", "avi", "qt", "wmv",
    "yuv", "rm", "asf", "amv", "m2ts", "mts", "ts", "mp4", "m4p", "m4v", "mpg", "mp2", "mpeg",
    "mpe", "mpv", "m2v", "svi", "3gp", "3g2", "mxf", "roq", "nsv", "f4v", "f4p", "f4a", "f4b",
];
const VIDEO_FRAME_COUNT: usize = 20;
const VIDEO_FRAME_SHORT_THRESHOLD_SECONDS: f64 = 60.0;
const VIDEO_FRAME_LONG_THRESHOLD_SECONDS: f64 = 600.0;
const VIDEO_FRAME_MEDIUM_INSET_SECONDS: f64 = 1.0;
const VIDEO_FRAME_LONG_INSET_SECONDS: f64 = 5.0;
const VIDEO_FRAME_EOF_SEEK_INSET_SECONDS: f64 = 0.05;
const VIDEO_FRAME_PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
#[cfg(test)]
const VIDEO_FRAME_FALLBACK_ASPECT_RATIO: f32 = 16.0 / 9.0;
const VIDEO_FRAME_PUBLISH_INTERVAL_MS: u64 = 16;

fn single_file_video_path(target: &PropertyTarget, item_kind: PropertyItemKind) -> Option<&Path> {
    if !matches!(item_kind, PropertyItemKind::SingleFile) {
        return None;
    }
    let path = target.paths.first()?;
    path_may_have_video_metadata(path).then_some(path.as_path())
}

fn single_file_media_path(target: &PropertyTarget, item_kind: PropertyItemKind) -> Option<&Path> {
    if !matches!(item_kind, PropertyItemKind::SingleFile) {
        return None;
    }
    let path = target.paths.first()?;
    path_may_have_media_details(path).then_some(path.as_path())
}

fn collect_single_file_media_detail_groups(
    target: &PropertyTarget,
    item_kind: PropertyItemKind,
) -> Vec<PropertyDetailGroup> {
    let Some(path) = single_file_media_path(target, item_kind) else {
        return Vec::new();
    };
    media_details(path)
}

fn media_details(path: &Path) -> Vec<PropertyDetailGroup> {
    let mut groups = Vec::new();
    if path_may_have_exif(path) {
        groups.extend(exif_details(path));
    }
    if path_may_have_video_metadata(path) {
        groups.extend(video_details(path));
    }
    groups
}

const EXIF_VALUE_TOO_BIG_LABEL: &str = "<value too big to display>";
const EXIF_NON_STANDARD_VALUE_CHAR_LIMIT: usize = 1024;
const EXIF_STANDARD_VALUE_CHAR_LIMIT: usize = 5120;

fn video_details(path: &Path) -> Vec<PropertyDetailGroup> {
    let availability_started = Instant::now();
    let ffprobe_installed = ffmpeg_sidecar::ffprobe::ffprobe_is_installed();
    crate::debug_options::log_property_timing(
        availability_started.elapsed(),
        format_args!(
            "video ffprobe availability path={} installed={}",
            path.display(),
            ffprobe_installed
        ),
    );
    if !ffprobe_installed {
        return video_metadata_unavailable_groups(
            "ffprobe is not available. Install FFmpeg/ffprobe or place ffprobe beside Explorer.",
        );
    }

    let output = match ffprobe_json_output(path) {
        Ok(output) => output,
        Err(error) => return video_metadata_unavailable_groups(format!("ffprobe failed: {error}")),
    };
    let parse_started = Instant::now();
    let probe: serde_json::Value = match serde_json::from_slice(&output) {
        Ok(probe) => {
            crate::debug_options::log_property_timing(
                parse_started.elapsed(),
                format_args!(
                    "video ffprobe json parsed path={} stdout_bytes={}",
                    path.display(),
                    output.len()
                ),
            );
            probe
        }
        Err(error) => {
            crate::debug_options::log_property_timing(
                parse_started.elapsed(),
                format_args!(
                    "video ffprobe json parse failed path={} stdout_bytes={} error={}",
                    path.display(),
                    output.len(),
                    error
                ),
            );
            return video_metadata_unavailable_groups(format!(
                "ffprobe returned unreadable metadata: {error}"
            ));
        }
    };
    let grouping_started = Instant::now();
    let groups = video_detail_groups_from_probe(&probe);
    crate::debug_options::log_property_timing(
        grouping_started.elapsed(),
        format_args!(
            "video metadata grouped path={} groups={} details={}",
            path.display(),
            groups.len(),
            detail_count(&groups)
        ),
    );
    if groups.is_empty() {
        return video_metadata_unavailable_groups("No video metadata was reported.");
    }

    groups
}

fn video_metadata_unavailable_groups(message: impl Into<String>) -> Vec<PropertyDetailGroup> {
    vec![property_detail_group(
        PropertyDetailGroupKind::Media,
        vec![PropertyDetail {
            name: "Video metadata".to_owned(),
            value: message.into(),
        }],
    )]
}

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn ffprobe_json_output(path: &Path) -> Result<Vec<u8>, String> {
    let mut command = Command::new(ffmpeg_sidecar::ffprobe::ffprobe_path());
    command
        .arg("-v")
        .arg("quiet")
        .arg("-print_format")
        .arg("json")
        .arg("-show_format")
        .arg("-show_streams")
        .arg("-show_programs")
        .arg("-show_chapters")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let started = Instant::now();
    let output = match command.output() {
        Ok(output) => output,
        Err(error) => {
            crate::debug_options::log_property_timing(
                started.elapsed(),
                format_args!(
                    "video ffprobe command failed path={} error={}",
                    path.display(),
                    error
                ),
            );
            return Err(format!("could not start ffprobe: {error}"));
        }
    };
    if output.status.success() {
        crate::debug_options::log_property_timing(
            started.elapsed(),
            format_args!(
                "video ffprobe command succeeded path={} stdout_bytes={}",
                path.display(),
                output.stdout.len()
            ),
        );
        return Ok(output.stdout);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        let error = format!("exited with {}", output.status);
        crate::debug_options::log_property_timing(
            started.elapsed(),
            format_args!(
                "video ffprobe command failed path={} error={}",
                path.display(),
                error
            ),
        );
        Err(error)
    } else {
        let error = format!("exited with {}: {stderr}", output.status);
        crate::debug_options::log_property_timing(
            started.elapsed(),
            format_args!(
                "video ffprobe command failed path={} error={}",
                path.display(),
                error
            ),
        );
        Err(error)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct VideoFrameRequest {
    label_seconds: f64,
    seek_seconds: f64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct VideoFramePng {
    label: String,
    png: Vec<u8>,
}

fn prepare_video_frame_requests(path: &Path) -> Result<Vec<VideoFrameRequest>, String> {
    let ffprobe_installed = ffmpeg_sidecar::ffprobe::ffprobe_is_installed();
    if !ffprobe_installed {
        return Err(
            "ffprobe is not available. Install FFmpeg/ffprobe or place ffprobe beside Explorer."
                .to_owned(),
        );
    }
    let ffmpeg_installed = ffmpeg_sidecar::command::ffmpeg_is_installed();
    if !ffmpeg_installed {
        return Err(
            "ffmpeg is not available. Install FFmpeg/ffprobe or place ffmpeg beside Explorer."
                .to_owned(),
        );
    }

    let output = ffprobe_json_output(path).map_err(|error| format!("ffprobe failed: {error}"))?;
    let probe: serde_json::Value = serde_json::from_slice(&output)
        .map_err(|error| format!("ffprobe returned unreadable metadata: {error}"))?;
    let duration = ffprobe_duration_seconds_from_probe(&probe)
        .ok_or_else(|| "Video duration is not available.".to_owned())?;
    let requests = video_frame_requests(duration);
    if requests.is_empty() {
        return Err("Video duration is not long enough to extract frames.".to_owned());
    }

    Ok(requests)
}

fn extract_video_frame_png(
    path: &Path,
    request: VideoFrameRequest,
) -> Result<VideoFramePng, String> {
    let label = video_frame_timestamp_label(request.label_seconds);
    match ffmpeg_frame_png_output(path, request.seek_seconds) {
        Ok(png) if png.starts_with(VIDEO_FRAME_PNG_SIGNATURE) => Ok(VideoFramePng { label, png }),
        Ok(png) => Err(format!(
            "{label}: ffmpeg returned {} bytes, but not a PNG image",
            png.len()
        )),
        Err(error) => Err(format!("{label}: {error}")),
    }
}

fn prepare_video_frame_thumbnail(frame: VideoFramePng) -> Result<PropertyFrameThumbnail, String> {
    let mut image = image::load_from_memory_with_format(&frame.png, image::ImageFormat::Png)
        .map_err(|error| {
            format!(
                "{}: ffmpeg returned unreadable PNG data: {error}",
                frame.label
            )
        })?
        .into_rgba8();
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return Err(format!(
            "{}: ffmpeg returned a PNG image with no dimensions",
            frame.label
        ));
    }

    for pixel in image.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    Ok(PropertyFrameThumbnail {
        label: frame.label,
        image: Arc::new(RenderImage::new(vec![image::Frame::new(image)])),
        aspect_ratio: width as f32 / height as f32,
    })
}

#[cfg(test)]
fn video_frame_png_aspect_ratio(png: &[u8]) -> f32 {
    image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .ok()
        .and_then(|image| {
            let width = image.width();
            let height = image.height();
            (width > 0 && height > 0).then_some(width as f32 / height as f32)
        })
        .unwrap_or(VIDEO_FRAME_FALLBACK_ASPECT_RATIO)
}

fn ffprobe_duration_seconds_from_probe(probe: &serde_json::Value) -> Option<f64> {
    let format_duration = probe
        .get("format")
        .and_then(|format| format.as_object())
        .and_then(|format| format.get("duration"))
        .and_then(ffprobe_seconds_value);
    if format_duration.is_some() {
        return format_duration;
    }

    probe
        .get("streams")
        .and_then(|streams| streams.as_array())
        .and_then(|streams| {
            streams.iter().find_map(|stream| {
                let stream = stream.as_object()?;
                let codec_type = stream
                    .get("codec_type")
                    .and_then(ffprobe_scalar_value_label);
                if codec_type.as_deref() != Some("video") {
                    return None;
                }
                stream.get("duration").and_then(ffprobe_seconds_value)
            })
        })
}

fn ffprobe_seconds_value(value: &serde_json::Value) -> Option<f64> {
    ffprobe_scalar_value_label(value)
        .as_deref()
        .and_then(parse_positive_f64)
}

fn video_frame_requests(duration_seconds: f64) -> Vec<VideoFrameRequest> {
    if !duration_seconds.is_finite() || duration_seconds <= 0.0 {
        return Vec::new();
    }

    let inset = video_frame_inset_seconds(duration_seconds);
    let mut label_seconds = Vec::new();
    if inset <= 0.0 {
        label_seconds.extend(evenly_spaced_seconds(
            0.0,
            duration_seconds,
            VIDEO_FRAME_COUNT,
        ));
    } else {
        label_seconds.push(0.0);
        label_seconds.extend(evenly_spaced_seconds(
            inset,
            duration_seconds - inset,
            VIDEO_FRAME_COUNT,
        ));
        label_seconds.push(duration_seconds);
    }

    label_seconds
        .into_iter()
        .map(|label_seconds| VideoFrameRequest {
            label_seconds,
            seek_seconds: safe_video_frame_seek_seconds(label_seconds, duration_seconds),
        })
        .collect()
}

fn video_frame_inset_seconds(duration_seconds: f64) -> f64 {
    if duration_seconds < VIDEO_FRAME_SHORT_THRESHOLD_SECONDS {
        0.0
    } else if duration_seconds < VIDEO_FRAME_LONG_THRESHOLD_SECONDS {
        VIDEO_FRAME_MEDIUM_INSET_SECONDS
    } else {
        VIDEO_FRAME_LONG_INSET_SECONDS
    }
}

fn evenly_spaced_seconds(start: f64, end: f64, count: usize) -> Vec<f64> {
    match count {
        0 => Vec::new(),
        1 => vec![start],
        _ => {
            let step = (end - start) / (count - 1) as f64;
            (0..count)
                .map(|index| start + step * index as f64)
                .collect()
        }
    }
}

fn safe_video_frame_seek_seconds(label_seconds: f64, duration_seconds: f64) -> f64 {
    if !duration_seconds.is_finite() || duration_seconds <= 0.0 {
        return 0.0;
    }
    let max_seek = (duration_seconds - VIDEO_FRAME_EOF_SEEK_INSET_SECONDS).max(0.0);
    label_seconds.clamp(0.0, max_seek)
}

fn ffmpeg_frame_png_output(path: &Path, seek_seconds: f64) -> Result<Vec<u8>, String> {
    let mut command = Command::new(ffmpeg_sidecar::paths::ffmpeg_path());
    command
        .arg("-v")
        .arg("error")
        .arg("-nostdin")
        .arg("-ss")
        .arg(ffmpeg_seek_argument(seek_seconds))
        .arg("-i")
        .arg(path)
        .arg("-map")
        .arg("0:v:0")
        .arg("-frames:v")
        .arg("1")
        .arg("-f")
        .arg("image2pipe")
        .arg("-vcodec")
        .arg("png")
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let output = command
        .output()
        .map_err(|error| format!("could not start ffmpeg: {error}"))?;
    if output.status.success() {
        return Ok(output.stdout);
    }

    let stderr = command_error_output_label(&output.stderr);
    if stderr.is_empty() {
        Err(format!("ffmpeg exited with {}", output.status))
    } else {
        Err(format!("ffmpeg exited with {}: {stderr}", output.status))
    }
}

fn ffmpeg_seek_argument(seconds: f64) -> String {
    format!("{:.3}", seconds.max(0.0))
}

fn video_frame_timestamp_label(seconds: f64) -> String {
    let total_millis = (seconds.max(0.0) * 1000.0).round() as u64;
    let hours = total_millis / 3_600_000;
    let minutes = (total_millis % 3_600_000) / 60_000;
    let seconds = (total_millis % 60_000) / 1000;
    let millis = total_millis % 1000;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}.{millis:03}")
    } else {
        format!("{minutes}:{seconds:02}.{millis:03}")
    }
}

fn command_error_output_label(stderr: &[u8]) -> String {
    let label = String::from_utf8_lossy(stderr).trim().to_owned();
    if label.chars().count() <= 300 {
        label
    } else {
        let mut truncated: String = label.chars().take(300).collect();
        truncated.push_str("...");
        truncated
    }
}

fn frame_extraction_error_summary(errors: &[String]) -> String {
    match errors {
        [] => "no frame data was returned".to_owned(),
        [error] => error.clone(),
        [first, ..] => format!("{first} ({} frame attempts failed)", errors.len()),
    }
}

#[derive(Default)]
struct VideoDetailBuilder {
    groups: BTreeMap<PropertyDetailGroupKind, Vec<PropertyDetail>>,
    used_paths: BTreeSet<String>,
    stream_labels: BTreeMap<usize, String>,
}

impl VideoDetailBuilder {
    fn push_heading(&mut self, kind: PropertyDetailGroupKind, name: impl Into<String>) {
        self.groups.entry(kind).or_default().push(PropertyDetail {
            name: name.into(),
            value: String::new(),
        });
    }

    fn push(
        &mut self,
        kind: PropertyDetailGroupKind,
        name: impl Into<String>,
        value: Option<String>,
    ) {
        let Some(value) = value.and_then(non_empty_property_value) else {
            return;
        };
        self.groups.entry(kind).or_default().push(PropertyDetail {
            name: name.into(),
            value,
        });
    }

    fn scalar_field(
        &mut self,
        object: &serde_json::Map<String, serde_json::Value>,
        base_path: &str,
        key: &str,
    ) -> Option<String> {
        let path = format!("{base_path}.{key}");
        let value = object.get(key)?;
        self.used_paths.insert(path);
        ffprobe_scalar_value_label(value)
    }

    fn integer_field(
        &mut self,
        object: &serde_json::Map<String, serde_json::Value>,
        base_path: &str,
        key: &str,
    ) -> Option<u64> {
        let path = format!("{base_path}.{key}");
        let value = object.get(key)?;
        self.used_paths.insert(path);
        ffprobe_integer_value(value)
    }

    fn tag_field(
        &mut self,
        object: &serde_json::Map<String, serde_json::Value>,
        base_path: &str,
        key: &str,
    ) -> Option<String> {
        let path = format!("{base_path}.tags.{key}");
        let tags = object.get("tags")?.as_object()?;
        let value = tags.get(key)?;
        self.used_paths.insert(path);
        ffprobe_scalar_value_label(value)
    }

    fn into_groups(mut self) -> Vec<PropertyDetailGroup> {
        PROPERTY_DETAIL_GROUP_ORDER
            .iter()
            .filter_map(|kind| {
                let details = self.groups.remove(kind)?;
                (!details.is_empty()).then(|| property_detail_group(*kind, details))
            })
            .collect()
    }
}

fn video_detail_groups_from_probe(probe: &serde_json::Value) -> Vec<PropertyDetailGroup> {
    let mut builder = VideoDetailBuilder::default();
    add_format_details(&mut builder, probe);
    add_stream_details(&mut builder, probe);
    add_chapter_details(&mut builder, probe);
    add_unknown_ffprobe_details(&mut builder, probe);
    builder.into_groups()
}

fn add_format_details(builder: &mut VideoDetailBuilder, probe: &serde_json::Value) {
    let Some(format) = probe.get("format").and_then(|format| format.as_object()) else {
        return;
    };

    let format_name = builder.scalar_field(format, "format", "format_name");
    let format_long_name = builder.scalar_field(format, "format", "format_long_name");
    builder.push(
        PropertyDetailGroupKind::Media,
        "Format",
        metadata_name_label(format_long_name.clone(), format_name.clone()),
    );
    let duration = builder
        .scalar_field(format, "format", "duration")
        .as_deref()
        .and_then(format_duration_label);
    builder.push(PropertyDetailGroupKind::Media, "Duration", duration);
    let bit_rate = builder
        .scalar_field(format, "format", "bit_rate")
        .as_deref()
        .and_then(format_bit_rate_label);
    builder.push(PropertyDetailGroupKind::Media, "Bit rate", bit_rate);
    let embedded_title = builder.tag_field(format, "format", "title");
    builder.push(
        PropertyDetailGroupKind::Media,
        "Embedded title",
        embedded_title,
    );

    let counts = ffprobe_stream_counts(probe);
    if let Some(nb_streams) = format.get("nb_streams") {
        builder.used_paths.insert("format.nb_streams".to_owned());
        let _ = nb_streams;
    }
    builder.push(
        PropertyDetailGroupKind::Media,
        "Streams",
        stream_counts_summary_label(&counts),
    );
    builder.push(
        PropertyDetailGroupKind::Media,
        "Video tracks",
        (counts.video > 0).then(|| counts.video.to_string()),
    );
    builder.push(
        PropertyDetailGroupKind::Media,
        "Audio tracks",
        (counts.audio > 0).then(|| counts.audio.to_string()),
    );
    builder.push(
        PropertyDetailGroupKind::Media,
        "Subtitles",
        (counts.subtitles > 0).then(|| counts.subtitles.to_string()),
    );
    builder.push(
        PropertyDetailGroupKind::Media,
        "Chapters",
        (counts.chapters > 0).then(|| counts.chapters.to_string()),
    );
}

fn add_stream_details(builder: &mut VideoDetailBuilder, probe: &serde_json::Value) {
    let Some(streams) = probe.get("streams").and_then(|streams| streams.as_array()) else {
        return;
    };

    let counts = ffprobe_stream_counts(probe);
    let mut video_count = 0usize;
    let mut audio_count = 0usize;
    let mut subtitle_count = 0usize;
    let mut other_count = 0usize;
    for (index, stream_value) in streams.iter().enumerate() {
        let Some(stream) = stream_value.as_object() else {
            continue;
        };
        let base_path = format!("streams.{index}");
        let codec_type = builder
            .scalar_field(stream, &base_path, "codec_type")
            .unwrap_or_else(|| "stream".to_owned());
        match codec_type.as_str() {
            "video" => {
                video_count += 1;
                builder
                    .stream_labels
                    .insert(index, format!("Video {video_count}"));
                if counts.video > 1 {
                    builder.push_heading(PropertyDetailGroupKind::Video, format!("#{video_count}"));
                }
                add_video_stream_details(builder, stream, &base_path, None);
            }
            "audio" => {
                audio_count += 1;
                builder
                    .stream_labels
                    .insert(index, format!("Audio {audio_count}"));
                if counts.audio > 1 {
                    builder.push_heading(PropertyDetailGroupKind::Audio, format!("#{audio_count}"));
                }
                add_audio_stream_details(builder, stream, &base_path, None);
            }
            "subtitle" => {
                subtitle_count += 1;
                builder
                    .stream_labels
                    .insert(index, format!("Subtitle {subtitle_count}"));
                if counts.subtitles > 1 {
                    builder.push_heading(
                        PropertyDetailGroupKind::Subtitles,
                        format!("#{subtitle_count}"),
                    );
                }
                add_subtitle_stream_details(builder, stream, &base_path, None);
            }
            _ => {
                other_count += 1;
                let label = format!("Stream {other_count}");
                builder.stream_labels.insert(index, label.clone());
                add_basic_stream_details(
                    builder,
                    stream,
                    &base_path,
                    PropertyDetailGroupKind::Misc,
                    &label,
                );
            }
        }
    }
}

fn stream_detail_name(prefix: Option<&str>, name: &str) -> String {
    match prefix {
        Some(prefix) => format!("{prefix} {name}"),
        None => name.to_owned(),
    }
}

fn add_video_stream_details(
    builder: &mut VideoDetailBuilder,
    stream: &serde_json::Map<String, serde_json::Value>,
    base_path: &str,
    label: Option<&str>,
) {
    let codec_name = builder.scalar_field(stream, base_path, "codec_name");
    let codec_long_name = builder.scalar_field(stream, base_path, "codec_long_name");
    builder.push(
        PropertyDetailGroupKind::Video,
        stream_detail_name(label, "Codec"),
        metadata_name_label(codec_long_name, codec_name),
    );

    let width = builder.integer_field(stream, base_path, "width");
    let height = builder.integer_field(stream, base_path, "height");
    builder.push(
        PropertyDetailGroupKind::Video,
        stream_detail_name(label, "Resolution"),
        resolution_label(width, height),
    );

    let avg_frame_rate = builder.scalar_field(stream, base_path, "avg_frame_rate");
    let raw_frame_rate = builder.scalar_field(stream, base_path, "r_frame_rate");
    builder.push(
        PropertyDetailGroupKind::Video,
        stream_detail_name(label, "Frame rate"),
        avg_frame_rate
            .as_deref()
            .and_then(format_frame_rate_label)
            .or_else(|| raw_frame_rate.as_deref().and_then(format_frame_rate_label)),
    );

    let display_aspect_ratio = builder.scalar_field(stream, base_path, "display_aspect_ratio");
    let sample_aspect_ratio = builder.scalar_field(stream, base_path, "sample_aspect_ratio");
    builder.push(
        PropertyDetailGroupKind::Video,
        stream_detail_name(label, "Aspect ratio"),
        aspect_ratio_label(
            display_aspect_ratio.as_deref(),
            sample_aspect_ratio.as_deref(),
            width,
            height,
        ),
    );

    let bit_rate = builder
        .scalar_field(stream, base_path, "bit_rate")
        .as_deref()
        .and_then(format_bit_rate_label);
    builder.push(
        PropertyDetailGroupKind::Video,
        stream_detail_name(label, "Bit rate"),
        bit_rate,
    );
    let profile = builder.scalar_field(stream, base_path, "profile");
    builder.push(
        PropertyDetailGroupKind::Video,
        stream_detail_name(label, "Profile"),
        profile,
    );
    let level = builder.scalar_field(stream, base_path, "level");
    builder.push(
        PropertyDetailGroupKind::Video,
        stream_detail_name(label, "Level"),
        level,
    );
    let pixel_format = builder.scalar_field(stream, base_path, "pix_fmt");
    builder.push(
        PropertyDetailGroupKind::Video,
        stream_detail_name(label, "Pixel format"),
        pixel_format,
    );
    let chroma_location = builder.scalar_field(stream, base_path, "chroma_location");
    builder.push(
        PropertyDetailGroupKind::Video,
        stream_detail_name(label, "Chroma location"),
        chroma_location,
    );
    let color_range = builder.scalar_field(stream, base_path, "color_range");
    builder.push(
        PropertyDetailGroupKind::Video,
        stream_detail_name(label, "Color range"),
        color_range,
    );
    let color_matrix = builder.scalar_field(stream, base_path, "color_space");
    builder.push(
        PropertyDetailGroupKind::Video,
        stream_detail_name(label, "Color matrix"),
        color_matrix,
    );
    let color_primaries = builder.scalar_field(stream, base_path, "color_primaries");
    builder.push(
        PropertyDetailGroupKind::Video,
        stream_detail_name(label, "Color primaries"),
        color_primaries,
    );
    let color_transfer = builder.scalar_field(stream, base_path, "color_transfer");
    builder.push(
        PropertyDetailGroupKind::Video,
        stream_detail_name(label, "Color transfer"),
        color_transfer,
    );
    let duration = builder
        .scalar_field(stream, base_path, "duration")
        .as_deref()
        .and_then(format_duration_label);
    builder.push(
        PropertyDetailGroupKind::Video,
        stream_detail_name(label, "Duration"),
        duration,
    );
    let frames = builder.scalar_field(stream, base_path, "nb_frames");
    builder.push(
        PropertyDetailGroupKind::Video,
        stream_detail_name(label, "Frames"),
        frames,
    );
    let language = builder.tag_field(stream, base_path, "language");
    builder.push(
        PropertyDetailGroupKind::Video,
        stream_detail_name(label, "Language"),
        language,
    );
    let title = builder.tag_field(stream, base_path, "title");
    builder.push(
        PropertyDetailGroupKind::Video,
        stream_detail_name(label, "Title"),
        title,
    );
}

fn add_audio_stream_details(
    builder: &mut VideoDetailBuilder,
    stream: &serde_json::Map<String, serde_json::Value>,
    base_path: &str,
    label: Option<&str>,
) {
    let codec_name = builder.scalar_field(stream, base_path, "codec_name");
    let codec_long_name = builder.scalar_field(stream, base_path, "codec_long_name");
    builder.push(
        PropertyDetailGroupKind::Audio,
        stream_detail_name(label, "Codec"),
        metadata_name_label(codec_long_name, codec_name),
    );
    let channels = builder
        .integer_field(stream, base_path, "channels")
        .map(channels_label);
    builder.push(
        PropertyDetailGroupKind::Audio,
        stream_detail_name(label, "Channels"),
        channels,
    );
    let channel_layout = builder.scalar_field(stream, base_path, "channel_layout");
    builder.push(
        PropertyDetailGroupKind::Audio,
        stream_detail_name(label, "Channel layout"),
        channel_layout,
    );
    let sample_rate = builder
        .scalar_field(stream, base_path, "sample_rate")
        .as_deref()
        .and_then(format_sample_rate_label);
    builder.push(
        PropertyDetailGroupKind::Audio,
        stream_detail_name(label, "Sample rate"),
        sample_rate,
    );
    let bit_rate = builder
        .scalar_field(stream, base_path, "bit_rate")
        .as_deref()
        .and_then(format_bit_rate_label);
    builder.push(
        PropertyDetailGroupKind::Audio,
        stream_detail_name(label, "Bit rate"),
        bit_rate,
    );
    let duration = builder
        .scalar_field(stream, base_path, "duration")
        .as_deref()
        .and_then(format_duration_label);
    builder.push(
        PropertyDetailGroupKind::Audio,
        stream_detail_name(label, "Duration"),
        duration,
    );
    let language = builder.tag_field(stream, base_path, "language");
    builder.push(
        PropertyDetailGroupKind::Audio,
        stream_detail_name(label, "Language"),
        language,
    );
    let title = builder.tag_field(stream, base_path, "title");
    builder.push(
        PropertyDetailGroupKind::Audio,
        stream_detail_name(label, "Title"),
        title,
    );
}

fn add_subtitle_stream_details(
    builder: &mut VideoDetailBuilder,
    stream: &serde_json::Map<String, serde_json::Value>,
    base_path: &str,
    label: Option<&str>,
) {
    let codec_name = builder.scalar_field(stream, base_path, "codec_name");
    let codec_long_name = builder.scalar_field(stream, base_path, "codec_long_name");
    builder.push(
        PropertyDetailGroupKind::Subtitles,
        stream_detail_name(label, "Codec"),
        metadata_name_label(codec_long_name, codec_name),
    );
    let language = builder.tag_field(stream, base_path, "language");
    builder.push(
        PropertyDetailGroupKind::Subtitles,
        stream_detail_name(label, "Language"),
        language,
    );
    let title = builder.tag_field(stream, base_path, "title");
    builder.push(
        PropertyDetailGroupKind::Subtitles,
        stream_detail_name(label, "Title"),
        title,
    );
    let disposition = disposition_label(builder, stream, base_path);
    builder.push(
        PropertyDetailGroupKind::Subtitles,
        stream_detail_name(label, "Disposition"),
        disposition,
    );
}

fn add_basic_stream_details(
    builder: &mut VideoDetailBuilder,
    stream: &serde_json::Map<String, serde_json::Value>,
    base_path: &str,
    kind: PropertyDetailGroupKind,
    label: &str,
) {
    let codec_name = builder.scalar_field(stream, base_path, "codec_name");
    let codec_long_name = builder.scalar_field(stream, base_path, "codec_long_name");
    builder.push(
        kind,
        format!("{label} Codec"),
        metadata_name_label(codec_long_name, codec_name),
    );
    let language = builder.tag_field(stream, base_path, "language");
    builder.push(kind, format!("{label} Language"), language);
    let title = builder.tag_field(stream, base_path, "title");
    builder.push(kind, format!("{label} Title"), title);
}

fn add_chapter_details(builder: &mut VideoDetailBuilder, probe: &serde_json::Value) {
    let Some(chapters) = probe
        .get("chapters")
        .and_then(|chapters| chapters.as_array())
    else {
        return;
    };

    for (index, chapter) in chapters.iter().enumerate() {
        let Some(chapter) = chapter.as_object() else {
            continue;
        };
        let base_path = format!("chapters.{index}");
        let start = builder
            .scalar_field(chapter, &base_path, "start_time")
            .as_deref()
            .and_then(format_duration_label);
        let end = builder
            .scalar_field(chapter, &base_path, "end_time")
            .as_deref()
            .and_then(format_duration_label);
        let title = builder.tag_field(chapter, &base_path, "title");
        builder.push(
            PropertyDetailGroupKind::Chapters,
            format!("Chapter {}", index + 1),
            chapter_label(start, end, title),
        );
    }
}

fn add_unknown_ffprobe_details(builder: &mut VideoDetailBuilder, probe: &serde_json::Value) {
    if let Some(format) = probe.get("format") {
        flatten_unknown_ffprobe_value(
            builder,
            format,
            "format".to_owned(),
            "Format".to_owned(),
            PropertyDetailGroupKind::Misc,
        );
    }
    if let Some(streams) = probe.get("streams").and_then(|streams| streams.as_array()) {
        for (index, stream) in streams.iter().enumerate() {
            let label = builder
                .stream_labels
                .get(&index)
                .cloned()
                .unwrap_or_else(|| format!("Stream {}", index + 1));
            flatten_unknown_ffprobe_value(
                builder,
                stream,
                format!("streams.{index}"),
                label,
                PropertyDetailGroupKind::Misc,
            );
        }
    }
    if let Some(programs) = probe
        .get("programs")
        .and_then(|programs| programs.as_array())
    {
        for (index, program) in programs.iter().enumerate() {
            flatten_unknown_ffprobe_value(
                builder,
                program,
                format!("programs.{index}"),
                format!("Program {}", index + 1),
                PropertyDetailGroupKind::Misc,
            );
        }
    }
    if let Some(chapters) = probe
        .get("chapters")
        .and_then(|chapters| chapters.as_array())
    {
        for (index, chapter) in chapters.iter().enumerate() {
            flatten_unknown_ffprobe_value(
                builder,
                chapter,
                format!("chapters.{index}"),
                format!("Chapter {}", index + 1),
                PropertyDetailGroupKind::Chapters,
            );
        }
    }
    if let Some(object) = probe.as_object() {
        for (key, value) in object {
            if matches!(key.as_str(), "format" | "streams" | "programs" | "chapters") {
                continue;
            }
            flatten_unknown_ffprobe_value(
                builder,
                value,
                key.clone(),
                humanized_metadata_key(key),
                PropertyDetailGroupKind::Misc,
            );
        }
    }
}

fn flatten_unknown_ffprobe_value(
    builder: &mut VideoDetailBuilder,
    value: &serde_json::Value,
    path: String,
    label: String,
    kind: PropertyDetailGroupKind,
) {
    if builder.used_paths.contains(&path) {
        return;
    }
    if let Some(value) = ffprobe_scalar_value_label(value) {
        builder.push(kind, label, Some(value));
        return;
    }
    if let Some(object) = value.as_object() {
        for (key, value) in object {
            let child_path = format!("{path}.{key}");
            let child_label = metadata_child_label(&label, key);
            flatten_unknown_ffprobe_value(builder, value, child_path, child_label, kind);
        }
        return;
    }
    if let Some(array) = value.as_array() {
        for (index, value) in array.iter().enumerate() {
            flatten_unknown_ffprobe_value(
                builder,
                value,
                format!("{path}.{index}"),
                format!("{label} {}", index + 1),
                kind,
            );
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct FfprobeStreamCounts {
    video: usize,
    audio: usize,
    subtitles: usize,
    other: usize,
    chapters: usize,
}

fn ffprobe_stream_counts(probe: &serde_json::Value) -> FfprobeStreamCounts {
    let mut counts = FfprobeStreamCounts {
        chapters: probe
            .get("chapters")
            .and_then(|chapters| chapters.as_array())
            .map_or(0, Vec::len),
        ..FfprobeStreamCounts::default()
    };
    let Some(streams) = probe.get("streams").and_then(|streams| streams.as_array()) else {
        return counts;
    };

    for stream in streams {
        match stream
            .get("codec_type")
            .and_then(|codec_type| codec_type.as_str())
        {
            Some("video") => counts.video += 1,
            Some("audio") => counts.audio += 1,
            Some("subtitle") => counts.subtitles += 1,
            _ => counts.other += 1,
        }
    }
    counts
}

fn stream_counts_summary_label(counts: &FfprobeStreamCounts) -> Option<String> {
    let mut parts = Vec::new();
    push_count_part(&mut parts, counts.video, "Video", "Videos");
    push_count_part(&mut parts, counts.audio, "Audio", "Audio");
    push_count_part(&mut parts, counts.subtitles, "Subtitle", "Subtitles");
    push_count_part(&mut parts, counts.other, "Other", "Other");
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn push_count_part(parts: &mut Vec<String>, count: usize, singular: &str, plural: &str) {
    if count > 0 {
        parts.push(count_label(count, singular, plural));
    }
}

fn metadata_name_label(long_name: Option<String>, short_name: Option<String>) -> Option<String> {
    match (long_name, short_name) {
        (Some(long_name), Some(short_name)) if long_name != short_name => {
            Some(format!("{long_name} ({short_name})"))
        }
        (Some(name), _) | (_, Some(name)) => Some(name),
        (None, None) => None,
    }
}

fn resolution_label(width: Option<u64>, height: Option<u64>) -> Option<String> {
    Some(format!("{} x {}", width?, height?))
}

fn aspect_ratio_label(
    display_aspect_ratio: Option<&str>,
    sample_aspect_ratio: Option<&str>,
    width: Option<u64>,
    height: Option<u64>,
) -> Option<String> {
    if let Some(display_aspect_ratio) =
        display_aspect_ratio.filter(|ratio| valid_ratio_label(ratio))
    {
        return Some(display_aspect_ratio.to_owned());
    }
    if let (Some(width), Some(height)) = (width, height)
        && width > 0
        && height > 0
    {
        let divisor = gcd(width, height);
        return Some(format!("{}:{}", width / divisor, height / divisor));
    }
    sample_aspect_ratio
        .filter(|ratio| valid_ratio_label(ratio))
        .map(str::to_owned)
}

fn valid_ratio_label(value: &str) -> bool {
    !value.is_empty() && value != "N/A" && value != "0:0" && value != "0:1"
}

fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left.max(1)
}

fn format_frame_rate_label(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some((numerator, denominator)) = value.split_once('/') {
        let numerator = parse_positive_f64(numerator)?;
        let denominator = parse_positive_f64(denominator)?;
        let fps = numerator / denominator;
        if !fps.is_finite() || fps <= 0.0 {
            return None;
        }
        return Some(format!("{} fps ({value})", format_decimal(fps, 2)));
    }

    parse_positive_f64(value).map(|fps| format!("{} fps", format_decimal(fps, 2)))
}

fn format_bit_rate_label(value: &str) -> Option<String> {
    let bits_per_second = parse_positive_f64(value)?;
    let kilobits_per_second = bits_per_second / 1000.0;
    let megabits_per_second = bits_per_second / 1_000_000.0;
    Some(format!(
        "{} Mb/s ({} kb/s)",
        format!("{megabits_per_second:.2}"),
        format_decimal_with_commas(kilobits_per_second, 2)
    ))
}

fn format_duration_label(value: &str) -> Option<String> {
    let seconds = parse_positive_or_zero_f64(value)?;
    let total_millis = (seconds * 1000.0).round() as u64;
    let hours = total_millis / 3_600_000;
    let minutes = (total_millis % 3_600_000) / 60_000;
    let seconds = (total_millis % 60_000) / 1000;
    let millis = total_millis % 1000;
    Some(format!(
        "{hours}h {minutes:02}m ({hours}:{minutes:02}:{seconds:02}.{millis:03})"
    ))
}

fn format_sample_rate_label(value: &str) -> Option<String> {
    let sample_rate = value.trim().parse::<u64>().ok()?;
    Some(format!("{} Hz", sample_rate.separate_with_commas()))
}

fn channels_label(channels: u64) -> String {
    format!(
        "{channels} {}",
        if channels == 1 { "channel" } else { "channels" }
    )
}

fn disposition_label(
    builder: &mut VideoDetailBuilder,
    stream: &serde_json::Map<String, serde_json::Value>,
    base_path: &str,
) -> Option<String> {
    let dispositions = stream.get("disposition")?.as_object()?;
    let mut labels = Vec::new();
    for (key, value) in dispositions {
        builder
            .used_paths
            .insert(format!("{base_path}.disposition.{key}"));
        if ffprobe_integer_value(value) == Some(1) {
            labels.push(humanized_metadata_key(key));
        }
    }
    (!labels.is_empty()).then(|| labels.join(", "))
}

fn chapter_label(
    start: Option<String>,
    end: Option<String>,
    title: Option<String>,
) -> Option<String> {
    match (start, end, title) {
        (Some(start), Some(end), Some(title)) => Some(format!("{start} to {end} - {title}")),
        (Some(start), Some(end), None) => Some(format!("{start} to {end}")),
        (Some(start), None, Some(title)) => Some(format!("{start} - {title}")),
        (None, Some(end), Some(title)) => Some(format!("to {end} - {title}")),
        (Some(value), None, None) | (None, Some(value), None) | (None, None, Some(value)) => {
            Some(value)
        }
        (None, None, None) => None,
    }
}

fn ffprobe_scalar_value_label(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Bool(value) => Some(value.to_string()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        serde_json::Value::String(value) => {
            let value = value.trim();
            (!value.is_empty() && value != "N/A").then(|| value.to_owned())
        }
        serde_json::Value::Null | serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            None
        }
    }
}

fn ffprobe_integer_value(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(value) => value.as_u64(),
        serde_json::Value::String(value) => value.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn parse_positive_f64(value: &str) -> Option<f64> {
    let value = value.trim().parse::<f64>().ok()?;
    (value.is_finite() && value > 0.0).then_some(value)
}

fn parse_positive_or_zero_f64(value: &str) -> Option<f64> {
    let value = value.trim().parse::<f64>().ok()?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn format_decimal(value: f64, precision: usize) -> String {
    let mut value = format!("{value:.precision$}");
    if let Some(dot_index) = value.find('.') {
        while value.ends_with('0') {
            value.pop();
        }
        if value.len() == dot_index + 1 {
            value.pop();
        }
    }
    value
}

fn format_decimal_with_commas(value: f64, precision: usize) -> String {
    let value = format!("{value:.precision$}");
    let Some((whole, fraction)) = value.split_once('.') else {
        return value.separate_with_commas();
    };
    format!("{}.{}", whole.separate_with_commas(), fraction)
}

fn metadata_child_label(parent: &str, key: &str) -> String {
    let key = match key {
        "tags" => "Tag".to_owned(),
        "disposition" => "Disposition".to_owned(),
        _ => humanized_metadata_key(key),
    };
    if parent.is_empty() {
        key
    } else {
        format!("{parent} {key}")
    }
}

fn humanized_metadata_key(name: &str) -> String {
    name.split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| {
            let part = match part {
                "id" => "ID".to_owned(),
                "nb" => "Number".to_owned(),
                _ => humanized_exif_tag_name(part),
            };
            title_case_words(&part)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn title_case_words(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            if word.chars().all(|ch| ch.is_ascii_uppercase()) {
                return word.to_owned();
            }
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn exif_details(path: &Path) -> Vec<PropertyDetailGroup> {
    let open_started = Instant::now();
    let file = match fs::File::open(path) {
        Ok(file) => {
            crate::debug_options::log_property_timing(
                open_started.elapsed(),
                format_args!("exif file opened path={}", path.display()),
            );
            file
        }
        Err(error) => {
            crate::debug_options::log_property_timing(
                open_started.elapsed(),
                format_args!(
                    "exif file open failed path={} error={}",
                    path.display(),
                    error
                ),
            );
            return Vec::new();
        }
    };
    let mut reader = BufReader::new(file);
    let read_started = Instant::now();
    let exif = match exif::Reader::new()
        .continue_on_error(true)
        .read_from_container(&mut reader)
        .or_else(|error| error.distill_partial_result(|_| {}))
    {
        Ok(exif) => {
            crate::debug_options::log_property_timing(
                read_started.elapsed(),
                format_args!("exif container read path={}", path.display()),
            );
            exif
        }
        Err(error) => {
            crate::debug_options::log_property_timing(
                read_started.elapsed(),
                format_args!(
                    "exif container read failed path={} error={}",
                    path.display(),
                    error
                ),
            );
            return Vec::new();
        }
    };

    let grouping_started = Instant::now();
    let fields: Vec<_> = exif.fields().collect();
    let field_count = fields.len();
    let mut tag_counts = BTreeMap::new();
    for field in &fields {
        *tag_counts.entry(exif_tag_name(field.tag)).or_insert(0usize) += 1;
    }

    let mut used_names = BTreeMap::new();
    let mut groups: BTreeMap<PropertyDetailGroupKind, Vec<PropertyDetail>> = BTreeMap::new();
    for field in fields {
        if field.tag == exif::Tag::UserComment {
            if let Some(details) = exifmeta_details(field) {
                groups
                    .entry(PropertyDetailGroupKind::Exifmeta)
                    .or_default()
                    .extend(details);
                continue;
            }
        }

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
            value: exif_detail_value_label(field, &exif),
        });
    }

    let groups: Vec<_> = PROPERTY_DETAIL_GROUP_ORDER
        .iter()
        .filter_map(|kind| {
            let details = groups.remove(kind)?;
            (!details.is_empty()).then(|| property_detail_group(*kind, details))
        })
        .collect();
    crate::debug_options::log_property_timing(
        grouping_started.elapsed(),
        format_args!(
            "exif details grouped path={} fields={} groups={} details={}",
            path.display(),
            field_count,
            groups.len(),
            detail_count(&groups)
        ),
    );
    groups
}

fn exif_detail_value_label(field: &exif::Field, exif: &exif::Exif) -> String {
    let char_limit = exif_detail_value_char_limit(field.tag);
    match &field.value {
        exif::Value::Ascii(values) => exif_ascii_value_label(values, char_limit),
        _ => bounded_exif_value_label(field, exif, char_limit),
    }
}

fn exif_detail_value_char_limit(tag: exif::Tag) -> Option<usize> {
    if tag == exif::Tag::UserComment {
        None
    } else if tag.description().is_none() {
        Some(EXIF_NON_STANDARD_VALUE_CHAR_LIMIT)
    } else {
        Some(EXIF_STANDARD_VALUE_CHAR_LIMIT)
    }
}

fn exif_ascii_value_label(values: &[Vec<u8>], char_limit: Option<usize>) -> String {
    let mut label = BoundedExifValueString::new(char_limit);
    for bytes in values {
        let value = String::from_utf8_lossy(bytes);
        let value = value.trim_end_matches('\0');
        if value.is_empty() {
            continue;
        }

        if !label.is_empty() && label.write_str(", ").is_err() {
            return EXIF_VALUE_TOO_BIG_LABEL.to_owned();
        }
        if label.write_str(value).is_err() {
            return EXIF_VALUE_TOO_BIG_LABEL.to_owned();
        }
    }

    label.into_string()
}

fn bounded_exif_value_label(
    field: &exif::Field,
    exif: &exif::Exif,
    char_limit: Option<usize>,
) -> String {
    if let Some(char_limit) = char_limit {
        let mut label = BoundedExifValueString::new(Some(char_limit));
        if fmt::write(
            &mut label,
            format_args!("{}", field.display_value().with_unit(exif)),
        )
        .is_err()
        {
            return EXIF_VALUE_TOO_BIG_LABEL.to_owned();
        }
        label.into_string()
    } else {
        field.display_value().with_unit(exif).to_string()
    }
}

struct BoundedExifValueString {
    value: String,
    char_limit: Option<usize>,
    char_count: usize,
}

impl BoundedExifValueString {
    fn new(char_limit: Option<usize>) -> Self {
        Self {
            value: String::new(),
            char_limit,
            char_count: 0,
        }
    }

    fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    fn into_string(self) -> String {
        self.value
    }
}

impl fmt::Write for BoundedExifValueString {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let char_count = value.chars().count();
        if self
            .char_limit
            .is_some_and(|limit| self.char_count + char_count > limit)
        {
            return Err(fmt::Error);
        }

        self.value.push_str(value);
        self.char_count += char_count;
        Ok(())
    }
}

fn exifmeta_details(field: &exif::Field) -> Option<Vec<PropertyDetail>> {
    let comment = exif_user_comment_text(&field.value)?;
    let payload = exifmeta_json_payload(&comment)?;
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let object = value.as_object()?;
    let mut details: Vec<_> = object
        .iter()
        .filter_map(|(key, value)| {
            exifmeta_value_label(value).map(|value| PropertyDetail {
                name: humanized_exif_tag_name(key),
                value,
            })
        })
        .collect();
    details.sort_by(|left, right| left.name.cmp(&right.name));
    (!details.is_empty()).then_some(details)
}

fn exif_user_comment_text(value: &exif::Value) -> Option<String> {
    let bytes = match value {
        exif::Value::Ascii(values) => values.first()?.as_slice(),
        exif::Value::Undefined(bytes, _) => exif_user_comment_payload_bytes(bytes),
        _ => return None,
    };
    let text = String::from_utf8_lossy(bytes);
    Some(text.trim_matches('\0').trim().to_owned())
}

fn exif_user_comment_payload_bytes(bytes: &[u8]) -> &[u8] {
    const ASCII_PREFIX: &[u8; 8] = b"ASCII\0\0\0";
    const JIS_PREFIX: &[u8; 8] = b"JIS\0\0\0\0\0";
    const UNICODE_PREFIX: &[u8; 8] = b"UNICODE\0";
    const UNDEFINED_PREFIX: &[u8; 8] = b"\0\0\0\0\0\0\0\0";

    for prefix in [
        ASCII_PREFIX.as_slice(),
        JIS_PREFIX.as_slice(),
        UNICODE_PREFIX.as_slice(),
        UNDEFINED_PREFIX.as_slice(),
    ] {
        if let Some(payload) = bytes.strip_prefix(prefix) {
            return payload;
        }
    }

    bytes
}

fn exifmeta_json_payload(comment: &str) -> Option<&str> {
    let versioned_payload = comment.strip_prefix("exifmeta-v")?;
    let json_start = versioned_payload.find('{')?;
    Some(versioned_payload[json_start..].trim())
}

fn exifmeta_value_label(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => Some("null".to_owned()),
        serde_json::Value::Bool(value) => Some(value.to_string()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => None,
    }
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

fn property_tabs_for_snapshot(
    snapshot: Option<&PropertySnapshot>,
) -> Vec<(PropertyTab, &'static str)> {
    PROPERTY_TABS
        .iter()
        .copied()
        .filter(|(tab, _)| property_tab_is_visible(*tab, snapshot))
        .collect()
}

fn property_tab_is_visible(tab: PropertyTab, snapshot: Option<&PropertySnapshot>) -> bool {
    match tab {
        PropertyTab::General | PropertyTab::Details => true,
        PropertyTab::Frames => snapshot.is_some_and(snapshot_has_frames_tab),
    }
}

fn snapshot_has_frames_tab(snapshot: &PropertySnapshot) -> bool {
    single_file_video_path(&snapshot.target, snapshot.item_kind).is_some()
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

fn frames_scrollbar_metrics_for_dimensions(
    viewport_height: f32,
    scroll_max: f32,
    scroll_top: f32,
) -> Option<ScrollbarMetrics> {
    if scroll_max <= 0.0 {
        return None;
    }

    ScrollbarMetrics::new(viewport_height, viewport_height + scroll_max, scroll_top)
}

fn frame_thumbnail_list(frames: &[PropertyFrameThumbnail]) -> AnyElement {
    let mut list = div()
        .flex()
        .flex_col()
        .gap(px(PROPERTIES_FRAME_LIST_GAP))
        .w_full()
        .min_w(px(0.0));
    for (index, frame) in frames.iter().enumerate() {
        list = list.child(frame_thumbnail_tile(index, frame));
    }
    list.into_any_element()
}

fn frame_thumbnail_tile(index: usize, frame: &PropertyFrameThumbnail) -> AnyElement {
    let mut image = div()
        .w_full()
        .border_1()
        .border_color(rgb(PROPERTIES_BORDER))
        .bg(rgb(0xf6f6f6))
        .overflow_hidden()
        .child(
            gpui::img(frame.image.clone())
                .size_full()
                .object_fit(ObjectFit::Contain),
        );
    image.style().aspect_ratio = Some(frame.aspect_ratio);

    div()
        .id(frame_thumbnail_id(index))
        .w_full()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(px(PROPERTIES_FRAME_LABEL_GAP))
        .child(image)
        .child(
            div()
                .w_full()
                .min_w(px(0.0))
                .text_size(px(11.0))
                .text_color(rgb(PROPERTIES_MUTED_TEXT))
                .child(SharedString::from(frame_thumbnail_label(
                    index,
                    &frame.label,
                ))),
        )
        .into_any_element()
}

fn frame_thumbnail_label(_index: usize, timestamp: &str) -> String {
    timestamp.to_owned()
}

fn frame_thumbnail_id(index: usize) -> (&'static str, usize) {
    ("properties-frame-thumbnail", index)
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
        PropertyTab::Frames => "properties-tab-frames",
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
    use std::{collections::HashSet, io::Cursor, time::Duration};

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
    fn properties_dialog_defines_general_details_and_frames_tabs() {
        assert_eq!(
            PROPERTY_TABS,
            &[
                (PropertyTab::General, "General"),
                (PropertyTab::Details, "Details"),
                (PropertyTab::Frames, "Frames")
            ]
        );
    }

    #[test]
    fn frames_tab_is_visible_only_for_single_video_files() {
        let temp = TempDir::new();
        let video = temp.path().join("movie.mp4");
        let image = temp.path().join("photo.jpg");
        let folder = temp.path().join("folder");
        let other = temp.path().join("other.txt");
        let missing_path = temp.path().join("missing.mp4");
        fs::write(&video, b"not real video").unwrap();
        fs::write(&image, b"not real image").unwrap();
        fs::write(&other, b"other").unwrap();
        fs::create_dir(&folder).unwrap();

        let video = collect_property_snapshot(PropertyTarget { paths: vec![video] }).unwrap();
        let image = collect_property_snapshot(PropertyTarget { paths: vec![image] }).unwrap();
        let folder = collect_property_snapshot(PropertyTarget {
            paths: vec![folder],
        })
        .unwrap();
        let missing = collect_property_snapshot(PropertyTarget {
            paths: vec![missing_path],
        })
        .unwrap();
        let mixed = collect_property_snapshot(PropertyTarget {
            paths: vec![video.target.paths[0].clone(), other],
        })
        .unwrap();

        assert_eq!(
            property_tabs_for_snapshot(Some(&video)),
            vec![
                (PropertyTab::General, "General"),
                (PropertyTab::Details, "Details"),
                (PropertyTab::Frames, "Frames")
            ]
        );
        assert_eq!(
            property_tabs_for_snapshot(Some(&image)),
            vec![
                (PropertyTab::General, "General"),
                (PropertyTab::Details, "Details")
            ]
        );
        assert_eq!(
            property_tabs_for_snapshot(Some(&folder)),
            vec![
                (PropertyTab::General, "General"),
                (PropertyTab::Details, "Details")
            ]
        );
        assert_eq!(
            property_tabs_for_snapshot(Some(&missing)),
            vec![
                (PropertyTab::General, "General"),
                (PropertyTab::Details, "Details")
            ]
        );
        assert_eq!(
            property_tabs_for_snapshot(Some(&mixed)),
            vec![
                (PropertyTab::General, "General"),
                (PropertyTab::Details, "Details")
            ]
        );
    }

    #[test]
    fn non_standard_detail_group_renders_as_misc() {
        assert_eq!(PropertyDetailGroupKind::NonStandard.title(), "non-standard");
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
    fn frames_scrollbar_metrics_only_exist_for_overflow() {
        assert!(frames_scrollbar_metrics_for_dimensions(100.0, 0.0, 0.0).is_none());

        let metrics =
            frames_scrollbar_metrics_for_dimensions(100.0, 50.0, 500.0).expect("overflow metrics");
        assert_eq!(metrics.viewport_height, 100.0);
        assert_eq!(metrics.content_height, 150.0);
        assert_eq!(metrics.scroll_max, 50.0);
        assert_eq!(metrics.scroll_top, 50.0);
    }

    #[test]
    fn video_probe_details_are_grouped_and_formatted() {
        let groups = video_detail_groups_from_probe(&sample_ffprobe_json(
            "5025.678",
            "4898900",
            "Studio Cut",
            1920,
            1080,
        ));

        assert_detail_contains(
            &groups,
            PropertyDetailGroupKind::Media,
            "Format",
            "QuickTime / MOV",
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Media, "Container format"),
            None
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Media, "Duration"),
            Some("1h 23m (1:23:45.678)")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Media, "Bit rate"),
            Some("4.90 Mb/s (4,898.90 kb/s)")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Media, "Embedded title"),
            Some("Studio Cut")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Media, "Streams"),
            Some("1 Video, 1 Audio, 1 Subtitle")
        );

        assert_detail_contains(&groups, PropertyDetailGroupKind::Video, "Codec", "H.264");
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Video, "Resolution"),
            Some("1920 x 1080")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Video, "Frame rate"),
            Some("29.97 fps (30000/1001)")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Video, "Aspect ratio"),
            Some("16:9")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Video, "Level"),
            Some("31")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Video, "Chroma location"),
            Some("left")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Video, "Color matrix"),
            Some("bt709")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Video, "Color primaries"),
            Some("bt709")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Video, "Color transfer"),
            Some("bt709")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Video, "#1"),
            None
        );

        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Audio, "Sample rate"),
            Some("48,000 Hz")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Audio, "Channels"),
            Some("2 channels")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Subtitles, "Disposition"),
            Some("Default")
        );
        assert_detail_contains(
            &groups,
            PropertyDetailGroupKind::Chapters,
            "Chapter 1",
            "Opening",
        );
    }

    #[test]
    fn video_probe_unknown_scalars_are_preserved() {
        let groups = video_detail_groups_from_probe(&sample_ffprobe_json(
            "60.0", "1000000", "Clip", 1280, 720,
        ));

        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Misc, "Format Probe Score"),
            Some("100")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Misc, "Format Tag Encoder"),
            Some("Lavf60")
        );
        assert_eq!(
            detail_value(
                &groups,
                PropertyDetailGroupKind::Misc,
                "Video 1 Bits Per Raw Sample"
            ),
            Some("8")
        );
    }

    #[test]
    fn video_probe_multiple_streams_use_numbered_subgroups() {
        let groups = video_detail_groups_from_probe(&sample_multi_stream_ffprobe_json());

        assert_eq!(
            detail_rows(&groups, PropertyDetailGroupKind::Video),
            vec![
                ("#1", ""),
                ("Codec", "H.264"),
                ("Resolution", "1920 x 1080"),
                ("Aspect ratio", "16:9"),
                ("#2", ""),
                ("Codec", "H.265"),
                ("Resolution", "3840 x 2160"),
                ("Aspect ratio", "16:9"),
            ]
        );
        assert_eq!(
            detail_rows(&groups, PropertyDetailGroupKind::Audio),
            vec![
                ("#1", ""),
                ("Codec", "AAC"),
                ("Channels", "2 channels"),
                ("#2", ""),
                ("Codec", "Opus"),
                ("Channels", "6 channels"),
            ]
        );
        assert_eq!(
            detail_rows(&groups, PropertyDetailGroupKind::Subtitles),
            vec![
                ("#1", ""),
                ("Codec", "SubRip"),
                ("#2", ""),
                ("Codec", "ASS")
            ]
        );
        assert_eq!(
            detail_value(
                &groups,
                PropertyDetailGroupKind::Misc,
                "Video 1 Bits Per Raw Sample"
            ),
            Some("8")
        );
        assert_eq!(
            detail_value(
                &groups,
                PropertyDetailGroupKind::Misc,
                "Video 2 Bits Per Raw Sample"
            ),
            Some("10")
        );
    }

    #[test]
    fn unavailable_video_metadata_returns_nonfatal_group() {
        let groups = video_metadata_unavailable_groups("ffprobe missing");

        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Media, "Video metadata"),
            Some("ffprobe missing")
        );
    }

    #[test]
    fn video_metadata_detection_uses_video_mime_or_extension() {
        assert!(path_may_have_video_metadata(Path::new("movie.mp4")));
        assert!(path_may_have_video_metadata(Path::new("clip.mkv")));
        assert!(!path_may_have_video_metadata(Path::new("note.txt")));
    }

    #[test]
    fn video_frame_requests_use_no_inset_below_one_minute() {
        let requests = video_frame_requests(59.0);

        assert_eq!(requests.len(), 20);
        assert_seconds(requests[0].label_seconds, 0.0);
        assert_seconds(requests[0].seek_seconds, 0.0);
        assert_seconds(requests[19].label_seconds, 59.0);
        assert_seconds(requests[19].seek_seconds, 58.95);
    }

    #[test]
    fn video_frame_requests_add_boundary_frames_with_medium_inset() {
        let requests = video_frame_requests(60.0);

        assert_eq!(requests.len(), 22);
        assert_seconds(requests[0].label_seconds, 0.0);
        assert_seconds(requests[1].label_seconds, 1.0);
        assert_seconds(requests[20].label_seconds, 59.0);
        assert_seconds(requests[21].label_seconds, 60.0);
        assert_seconds(requests[21].seek_seconds, 59.95);
    }

    #[test]
    fn video_frame_requests_add_boundary_frames_with_long_inset() {
        let requests = video_frame_requests(600.0);

        assert_eq!(requests.len(), 22);
        assert_seconds(requests[0].label_seconds, 0.0);
        assert_seconds(requests[1].label_seconds, 5.0);
        assert_seconds(requests[20].label_seconds, 595.0);
        assert_seconds(requests[21].label_seconds, 600.0);
        assert_seconds(requests[21].seek_seconds, 599.95);
    }

    #[test]
    fn video_frame_duration_uses_format_duration_then_video_stream_duration() {
        assert_seconds(
            ffprobe_duration_seconds_from_probe(&sample_ffprobe_json(
                "5025.678",
                "4898900",
                "Studio Cut",
                1920,
                1080,
            ))
            .unwrap(),
            5025.678,
        );

        let probe = serde_json::json!({
            "format": {
                "format_name": "matroska"
            },
            "streams": [
                {
                    "codec_type": "audio",
                    "duration": "45.0"
                },
                {
                    "codec_type": "video",
                    "duration": "123.456"
                }
            ]
        });

        assert_seconds(
            ffprobe_duration_seconds_from_probe(&probe).unwrap(),
            123.456,
        );
    }

    #[test]
    fn final_frame_seek_is_clamped_before_eof() {
        assert_seconds(safe_video_frame_seek_seconds(10.0, 10.0), 9.95);
        assert_seconds(safe_video_frame_seek_seconds(3.0, 10.0), 3.0);
        assert_seconds(safe_video_frame_seek_seconds(1.0, 0.02), 0.0);
    }

    #[test]
    fn frame_thumbnail_labels_use_timestamp_only() {
        assert_eq!(frame_thumbnail_label(0, "0:00.000"), "0:00.000");
        assert_eq!(frame_thumbnail_label(11, "1:02:03.456"), "1:02:03.456");
    }

    #[test]
    fn video_frame_png_aspect_ratio_uses_png_dimensions() {
        let mut bytes = Vec::new();
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(4, 2));
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();

        assert_eq!(video_frame_png_aspect_ratio(&bytes), 2.0);
    }

    #[test]
    fn video_frame_thumbnail_preparation_preserves_png_dimensions() {
        let mut bytes = Vec::new();
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(4, 2));
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();

        let thumbnail = prepare_video_frame_thumbnail(VideoFramePng {
            label: "0:00.000".to_owned(),
            png: bytes,
        })
        .unwrap();

        let size = thumbnail.image.size(0);
        assert_eq!(thumbnail.label, "0:00.000");
        assert_eq!(thumbnail.aspect_ratio, 2.0);
        assert_eq!(size.width.0, 4);
        assert_eq!(size.height.0, 2);
    }

    #[test]
    fn video_frame_png_aspect_ratio_falls_back_for_invalid_png() {
        assert_eq!(
            video_frame_png_aspect_ratio(b"not a png"),
            VIDEO_FRAME_FALLBACK_ASPECT_RATIO
        );
    }

    #[test]
    fn video_frame_thumbnail_preparation_rejects_invalid_png() {
        let error = prepare_video_frame_thumbnail(VideoFramePng {
            label: "0:00.000".to_owned(),
            png: b"not a png".to_vec(),
        })
        .unwrap_err();

        assert!(error.contains("unreadable PNG data"));
    }

    #[test]
    fn video_metadata_formatters_match_properties_copy_text() {
        assert_eq!(
            format_duration_label("5025.678"),
            Some("1h 23m (1:23:45.678)".to_owned())
        );
        assert_eq!(
            format_frame_rate_label("30000/1001"),
            Some("29.97 fps (30000/1001)".to_owned())
        );
        assert_eq!(
            format_bit_rate_label("4898900"),
            Some("4.90 Mb/s (4,898.90 kb/s)".to_owned())
        );
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

        let exif_groups = exif_details(&file);

        assert_eq!(
            detail_value(&exif_groups, PropertyDetailGroupKind::Camera, "Make"),
            Some("Canon")
        );
        assert_eq!(
            detail_value(&exif_groups, PropertyDetailGroupKind::Camera, "Model"),
            Some("TestCam")
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

        let exif_groups = exif_details(&file);

        assert_eq!(
            detail_value(
                &exif_groups,
                PropertyDetailGroupKind::Camera,
                "Make (IFD 0)"
            ),
            Some("Canon")
        );
        assert_eq!(
            detail_value(
                &exif_groups,
                PropertyDetailGroupKind::Camera,
                "Make (IFD 1)"
            ),
            Some("Thumb")
        );
    }

    #[test]
    fn exif_ascii_values_render_without_quotes() {
        assert_eq!(
            exif_ascii_value_label(
                &[b"Canon\0".to_vec(), b"".to_vec(), b"TestCam\0\0".to_vec()],
                None
            ),
            "Canon, TestCam"
        );
        assert_eq!(exif_ascii_value_label(&[b"\0".to_vec()], None), "");
    }

    #[test]
    fn non_standard_exif_ascii_values_at_limit_render_normally() {
        let temp = TempDir::new();
        let file = temp.path().join("photo.jpg");
        let value = "A".repeat(EXIF_NON_STANDARD_VALUE_CHAR_LIMIT);
        fs::write(&file, jpeg_with_exif(&custom_ascii_tag_tiff(&value))).unwrap();

        let exif_groups = exif_details(&file);

        assert_eq!(
            detail_value(
                &exif_groups,
                PropertyDetailGroupKind::NonStandard,
                "Tag(Tiff, 0xFDE8)"
            ),
            Some(value.as_str())
        );
    }

    #[test]
    fn non_standard_exif_ascii_values_over_limit_are_replaced() {
        let temp = TempDir::new();
        let file = temp.path().join("photo.jpg");
        let value = "A".repeat(EXIF_NON_STANDARD_VALUE_CHAR_LIMIT + 1);
        fs::write(&file, jpeg_with_exif(&custom_ascii_tag_tiff(&value))).unwrap();

        let exif_groups = exif_details(&file);

        assert_eq!(
            detail_value(
                &exif_groups,
                PropertyDetailGroupKind::NonStandard,
                "Tag(Tiff, 0xFDE8)"
            ),
            Some(EXIF_VALUE_TOO_BIG_LABEL)
        );
    }

    #[test]
    fn standard_exif_ascii_values_over_limit_are_replaced() {
        let temp = TempDir::new();
        let file = temp.path().join("photo.jpg");
        let make = "C".repeat(EXIF_STANDARD_VALUE_CHAR_LIMIT + 1);
        fs::write(&file, jpeg_with_exif(&exif_tiff(&make, "TestCam", None))).unwrap();

        let exif_groups = exif_details(&file);

        assert_eq!(
            detail_value(&exif_groups, PropertyDetailGroupKind::Camera, "Make"),
            Some(EXIF_VALUE_TOO_BIG_LABEL)
        );
    }

    #[test]
    fn exif_details_are_grouped_by_standard_category() {
        let temp = TempDir::new();
        let file = temp.path().join("photo.jpg");
        fs::write(&file, jpeg_with_exif(&grouped_exif_tiff())).unwrap();

        let exif_groups = exif_details(&file);

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
    fn exifmeta_user_comment_renders_as_own_group() {
        let temp = TempDir::new();
        let file = temp.path().join("photo.jpg");
        fs::write(
            &file,
            jpeg_with_exif(&exifmeta_tiff(
                "exifmeta-v0.1.0\n{\"LocationExact\":\"The Yellow River in Jinan\",\"FilmRoll\":36,\"FilmMaker\":\"Kodak\",\"FilmName\":\"Vision 3 250D\",\"FilmFormat\":120,\"FilmColor\":true,\"FilmNegative\":true,\"FilmDevelopProcess\":\"ECN-2\",\"FilmDeveloper\":\"CD-3\",\"FilmProcessLab\":\"栗子胶片社 (Chestnut Film Studio)\",\"FilmProcessDate\":\"2026-05-22\",\"FilmScanner\":\"Hasselblad Flextight X5\"}",
            )),
        )
        .unwrap();

        let exif_groups = exif_details(&file);

        assert_detail_contains(
            &exif_groups,
            PropertyDetailGroupKind::Exifmeta,
            "Film Color",
            "true",
        );
        assert_detail_contains(
            &exif_groups,
            PropertyDetailGroupKind::Exifmeta,
            "Film Develop Process",
            "ECN-2",
        );
        assert_detail_contains(
            &exif_groups,
            PropertyDetailGroupKind::Exifmeta,
            "Film Format",
            "120",
        );
        assert_detail_contains(
            &exif_groups,
            PropertyDetailGroupKind::Exifmeta,
            "Film Process Lab",
            "栗子胶片社 (Chestnut Film Studio)",
        );
        assert_detail_contains(
            &exif_groups,
            PropertyDetailGroupKind::Exifmeta,
            "Location Exact",
            "The Yellow River in Jinan",
        );
        assert_eq!(
            detail_value(&exif_groups, PropertyDetailGroupKind::Misc, "User Comment"),
            None
        );

        let exifmeta_index = group_index(&exif_groups, PropertyDetailGroupKind::Exifmeta).unwrap();
        let misc_index = group_index(&exif_groups, PropertyDetailGroupKind::Misc).unwrap();
        assert!(exifmeta_index < misc_index);

        let exifmeta_group = detail_group(&exif_groups, PropertyDetailGroupKind::Exifmeta).unwrap();
        let names: Vec<_> = exifmeta_group
            .details
            .iter()
            .map(|detail| detail.name.as_str())
            .collect();
        let mut sorted_names = names.clone();
        sorted_names.sort_unstable();
        assert_eq!(names, sorted_names);
    }

    #[test]
    fn invalid_exifmeta_user_comment_stays_in_misc() {
        let temp = TempDir::new();
        let file = temp.path().join("photo.jpg");
        fs::write(
            &file,
            jpeg_with_exif(&exifmeta_tiff("exifmeta-v0.1.0\nnot json")),
        )
        .unwrap();

        let exif_groups = exif_details(&file);

        assert!(detail_group(&exif_groups, PropertyDetailGroupKind::Exifmeta).is_none());
        assert_group_has_detail(&exif_groups, PropertyDetailGroupKind::Misc, "User Comment");
    }

    #[test]
    fn long_exifmeta_user_comment_still_renders_as_own_group() {
        let temp = TempDir::new();
        let file = temp.path().join("photo.jpg");
        let location = "River ".repeat(EXIF_STANDARD_VALUE_CHAR_LIMIT + 1);
        fs::write(
            &file,
            jpeg_with_exif(&exifmeta_tiff(&format!(
                "exifmeta-v0.1.0\n{{\"FilmMaker\":\"Kodak\",\"LocationExact\":{}}}",
                serde_json::to_string(&location).unwrap()
            ))),
        )
        .unwrap();

        let exif_groups = exif_details(&file);

        assert_detail_contains(
            &exif_groups,
            PropertyDetailGroupKind::Exifmeta,
            "Film Maker",
            "Kodak",
        );
        assert_detail_contains(
            &exif_groups,
            PropertyDetailGroupKind::Exifmeta,
            "Location Exact",
            "River",
        );
        assert_eq!(
            detail_value(&exif_groups, PropertyDetailGroupKind::Misc, "User Comment"),
            None
        );
    }

    #[test]
    fn multiple_file_properties_do_not_collect_media_metadata() {
        let temp = TempDir::new();
        let first = temp.path().join("a.jpg");
        let second = temp.path().join("b.jpg");
        fs::write(&first, jpeg_with_exif(&exif_tiff("Canon", "A", None))).unwrap();
        fs::write(&second, jpeg_with_exif(&exif_tiff("Nikon", "B", None))).unwrap();

        let groups = collect_single_file_media_detail_groups(
            &PropertyTarget {
                paths: vec![first, second],
            },
            PropertyItemKind::MultipleFiles,
        );

        assert!(groups.is_empty());
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

    fn group_index(groups: &[PropertyDetailGroup], kind: PropertyDetailGroupKind) -> Option<usize> {
        groups.iter().position(|group| group.kind == kind)
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

    fn detail_rows(
        groups: &[PropertyDetailGroup],
        kind: PropertyDetailGroupKind,
    ) -> Vec<(&str, &str)> {
        detail_group(groups, kind)
            .unwrap_or_else(|| panic!("missing {kind:?} group"))
            .details
            .iter()
            .map(|detail| (detail.name.as_str(), detail.value.as_str()))
            .collect()
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

    fn assert_seconds(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 0.000_001,
            "expected {actual} to equal {expected}"
        );
    }

    fn sample_ffprobe_json(
        duration: &str,
        bit_rate: &str,
        title: &str,
        width: u64,
        height: u64,
    ) -> serde_json::Value {
        serde_json::json!({
            "format": {
                "filename": "movie.mp4",
                "nb_streams": 3,
                "format_name": "mov,mp4,m4a,3gp,3g2,mj2",
                "format_long_name": "QuickTime / MOV",
                "duration": duration,
                "size": "3072000000",
                "bit_rate": bit_rate,
                "probe_score": 100,
                "tags": {
                    "title": title,
                    "encoder": "Lavf60"
                }
            },
            "streams": [
                {
                    "index": 0,
                    "codec_name": "h264",
                    "codec_long_name": "H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10",
                    "profile": "High",
                    "codec_type": "video",
                    "width": width,
                    "height": height,
                    "sample_aspect_ratio": "1:1",
                    "display_aspect_ratio": "16:9",
                    "pix_fmt": "yuv420p",
                    "level": 31,
                    "color_range": "tv",
                    "color_space": "bt709",
                    "color_transfer": "bt709",
                    "color_primaries": "bt709",
                    "chroma_location": "left",
                    "r_frame_rate": "30000/1001",
                    "avg_frame_rate": "30000/1001",
                    "duration": duration,
                    "bit_rate": "4500000",
                    "nb_frames": "150620",
                    "bits_per_raw_sample": "8",
                    "tags": {
                        "language": "eng",
                        "title": "Main video"
                    }
                },
                {
                    "index": 1,
                    "codec_name": "aac",
                    "codec_long_name": "AAC (Advanced Audio Coding)",
                    "codec_type": "audio",
                    "sample_rate": "48000",
                    "channels": 2,
                    "channel_layout": "stereo",
                    "duration": duration,
                    "bit_rate": "192000",
                    "tags": {
                        "language": "eng",
                        "title": "Stereo"
                    }
                },
                {
                    "index": 2,
                    "codec_name": "subrip",
                    "codec_long_name": "SubRip subtitle",
                    "codec_type": "subtitle",
                    "disposition": {
                        "default": 1,
                        "forced": 0
                    },
                    "tags": {
                        "language": "eng",
                        "title": "English"
                    }
                }
            ],
            "programs": [],
            "chapters": [
                {
                    "id": 1,
                    "start_time": "0.000000",
                    "end_time": "60.000000",
                    "tags": {
                        "title": "Opening"
                    }
                }
            ]
        })
    }

    fn sample_multi_stream_ffprobe_json() -> serde_json::Value {
        serde_json::json!({
            "format": {
                "nb_streams": 6,
                "format_name": "matroska,webm",
                "format_long_name": "Matroska / WebM"
            },
            "streams": [
                {
                    "codec_type": "video",
                    "codec_name": "H.264",
                    "codec_long_name": "H.264",
                    "width": 1920,
                    "height": 1080,
                    "bits_per_raw_sample": "8"
                },
                {
                    "codec_type": "video",
                    "codec_name": "H.265",
                    "codec_long_name": "H.265",
                    "width": 3840,
                    "height": 2160,
                    "bits_per_raw_sample": "10"
                },
                {
                    "codec_type": "audio",
                    "codec_name": "AAC",
                    "codec_long_name": "AAC",
                    "channels": 2
                },
                {
                    "codec_type": "audio",
                    "codec_name": "Opus",
                    "codec_long_name": "Opus",
                    "channels": 6
                },
                {
                    "codec_type": "subtitle",
                    "codec_name": "SubRip",
                    "codec_long_name": "SubRip"
                },
                {
                    "codec_type": "subtitle",
                    "codec_name": "ASS",
                    "codec_long_name": "ASS"
                }
            ],
            "programs": [],
            "chapters": []
        })
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

    fn custom_ascii_tag_tiff(value: &str) -> Vec<u8> {
        let value = ascii_exif_value(value);
        let ifd0_entry_count = 1u16;
        let ifd0_start = 8usize;
        let ifd0_end = ifd0_start + 2 + usize::from(ifd0_entry_count) * 12 + 4;
        let value_offset = ifd0_end;

        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II");
        tiff.extend_from_slice(&42u16.to_le_bytes());
        tiff.extend_from_slice(&(ifd0_start as u32).to_le_bytes());
        tiff.extend_from_slice(&ifd0_entry_count.to_le_bytes());
        push_ifd_entry(
            &mut tiff,
            0xfde8,
            2,
            value.len() as u32,
            value_offset as u32,
        );
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.extend_from_slice(&value);

        tiff
    }

    fn exifmeta_tiff(comment: &str) -> Vec<u8> {
        let make = ascii_exif_value("Canon");
        let model = ascii_exif_value("TestCam");
        let comment = undefined_exif_value(comment);
        let ifd0_entry_count = 4u16;
        let ifd0_start = 8usize;
        let ifd0_end = ifd0_start + 2 + usize::from(ifd0_entry_count) * 12 + 4;
        let make_offset = ifd0_end;
        let model_offset = make_offset + make.len();
        let exif_ifd_offset = model_offset + model.len();
        let comment_offset = exif_ifd_offset + 2 + 12 + 4;

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
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.extend_from_slice(&make);
        tiff.extend_from_slice(&model);

        tiff.extend_from_slice(&1u16.to_le_bytes());
        push_ifd_entry(
            &mut tiff,
            0x9286,
            7,
            comment.len() as u32,
            comment_offset as u32,
        );
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.extend_from_slice(&comment);

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

    fn undefined_exif_value(value: &str) -> Vec<u8> {
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
