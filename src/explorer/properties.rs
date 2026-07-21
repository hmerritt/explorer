use std::fmt::Write as _;
use std::{
    cell::RefCell,
    cmp::Ordering as CmpOrdering,
    collections::{BTreeMap, BTreeSet, HashMap},
    ffi::OsString,
    fmt, fs,
    io::{self, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use filetime::{FileTime, set_file_times};
use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, ClickEvent, ClipboardItem, Context, Div, Entity,
    FocusHandle, Focusable, Global, Image, ImageFormat, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels, Render, RenderImage, ScrollHandle,
    ScrollWheelEvent, SharedString, StyledImage, Task, TextRun, TitlebarOptions, WeakEntity,
    Window, WindowBounds, WindowDecorations, WindowKind, WindowOptions, canvas, div, point,
    prelude::*, px, rgb, size,
};
use image::ImageEncoder;
use jwalk::WalkDirGeneric;
use sha2::{Digest, Sha256};
use thousands::Separable;

#[cfg(test)]
use crate::explorer::image_preview::load_property_image_preview;
#[cfg(test)]
use crate::explorer::image_preview::svg_raster_dimensions;
use crate::explorer::{
    DialogCancel, DialogConfirm, PropertiesOpenNext, PropertiesOpenPrevious, SelectNextTab,
    SelectPreviousTab,
    app_icons::NativeIconSize,
    codebase_summary::{
        CodebaseLanguageSummary, CodebaseSummary, direct_git_repository_root,
        language_segment_widths, scan_direct_codebase_summary,
    },
    constants::{
        NAV_BUTTON_ACTIVE_OPACITY, NAV_BUTTON_HOVER_BG, NAV_ICON_DISABLED_COLOR,
        NAV_ICON_ENABLED_COLOR, NAV_ICON_TEXT_SIZE, SCROLLBAR_ARROW_HEIGHT, SCROLLBAR_GUTTER_WIDTH,
        SCROLLBAR_THUMB_ACTIVE_BG, SCROLLBAR_THUMB_BG, SCROLLBAR_THUMB_HOVER_BG,
        SCROLLBAR_THUMB_HOVER_WIDTH, SCROLLBAR_THUMB_WIDTH, SCROLLBAR_TRACK_BG,
        UTILITY_ICON_BUTTON_SIZE,
    },
    context_menu::clamped_context_menu_origin,
    entry::{DirectoryLinkKind, EntryKind, FileEntry},
    formatting::{format_size, format_timestamp},
    git_status::{GitDivergence, GitRepositoryCodeInfo, scan_git_repository_code_info},
    icons::{
        COPY_ICON, NavIcon, copy_file_dialog_icon_sized, directory_shortcut_icon_sized,
        file_icon_for_path_sized, folder_icon_sized, nav_icon_font, render_image_icon,
    },
    image_preview::{PropertyImagePreview, path_may_have_image_preview},
    open_with::{DefaultAppChangeOutcome, DefaultApplication, default_application_for_file},
    scrollbar::{ScrollbarArrow, ScrollbarDrag, ScrollbarMetrics, scrollbar_arrow_button},
    tooltip::explorer_tooltip,
    video::{
        ffmpeg_executable_path, ffmpeg_is_installed, ffmpeg_seek_argument, ffprobe_executable_path,
        ffprobe_is_installed, ffprobe_scalar_value_label, parse_positive_f64,
        path_may_have_audio_metadata, path_may_have_video_metadata, probe_video_duration_seconds,
        safe_video_frame_seek_seconds, video_frame_inset_seconds, video_frame_timestamp_label,
    },
    video_thumbnails::{VideoFrameRgba, extract_video_frame_batch},
    view::ExplorerView,
};
use crate::image_viewer::{ImageViewerEvent, ImageViewerSurface, new_embedded_image_viewer};
use crate::loaders::{LinearProgressStyle, linear_indeterminate};
use crate::settings::SettingsState;

#[cfg(test)]
use crate::explorer::video::ffprobe_duration_seconds_from_probe;

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
const PROPERTIES_CODE_MAKEUP_BAR_WIDTH: f32 = 320.0;
const PROPERTIES_CODE_MAKEUP_BAR_HEIGHT: f32 = 10.0;
const PROPERTIES_CODE_MAKEUP_BAR_RADIUS: f32 = 5.0;
const PROPERTIES_CODE_MAKEUP_SEPARATOR_WIDTH: f32 = 1.0;
const PROPERTIES_CODE_MAKEUP_SEPARATOR_COLOR: u32 = 0xffffff;
const PROPERTIES_CODE_LANGUAGE_LABEL_WIDTH: f32 = 178.0;
const PROPERTIES_CODE_LANGUAGE_LOC_WIDTH: f32 = 86.0;
const PROPERTIES_CODE_LANGUAGE_SWATCH_SIZE: f32 = 10.0;
const PROPERTIES_SPECTRUM_INITIAL_TIME_BINS: usize = 512;
const PROPERTIES_SPECTRUM_FREQUENCY_BINS: usize = PROPERTIES_SPECTRUM_FFT_SIZE / 2 + 1;
const PROPERTIES_SPECTRUM_MAX_TIME_BINS: usize = 4096;
const PROPERTIES_SPECTRUM_FFT_SIZE: usize = 2048;
const PROPERTIES_SPECTRUM_RESIZE_DEBOUNCE_MS: u64 = 300;
const PROPERTIES_SPECTRUM_DEFAULT_LOW_DB: f32 = -120.0;
const PROPERTIES_SPECTRUM_DEFAULT_HIGH_DB: f32 = -20.0;
const PROPERTIES_SPECTRUM_MIN_DB: f32 = -160.0;
const PROPERTIES_SPECTRUM_MAX_DB: f32 = 0.0;
const PROPERTIES_SPECTRUM_MIN_RANGE_DB: f32 = 10.0;
const PROPERTIES_SPECTRUM_RANGE_STEP_DB: f32 = 10.0;
const PROPERTIES_SPECTRUM_HEADER_HEIGHT: f32 = 18.0;
const PROPERTIES_SPECTRUM_TIME_RULER_HEIGHT: f32 = 16.0;
const PROPERTIES_SPECTRUM_AXIS_GAP: f32 = 4.0;
const PROPERTIES_SPECTRUM_FREQUENCY_RULER_WIDTH: f32 = 42.0;
const PROPERTIES_SPECTRUM_DB_LEGEND_WIDTH: f32 = 10.0;
const PROPERTIES_SPECTRUM_DB_LABEL_WIDTH: f32 = 44.0;
const PROPERTIES_SPECTRUM_DB_RULER_WIDTH: f32 = PROPERTIES_SPECTRUM_DB_LEGEND_WIDTH
    + PROPERTIES_SPECTRUM_AXIS_GAP
    + PROPERTIES_SPECTRUM_DB_LABEL_WIDTH;
const PROPERTIES_SPECTRUM_CONTROL_HEIGHT: f32 = 34.0;
const PROPERTIES_SPECTRUM_CONTROL_BUTTON_SIZE: f32 = 20.0;
const PROPERTIES_SPECTRUM_AXIS_TEXT: u32 = 0xffffff;
const PROPERTIES_SPECTRUM_PANEL_BG: u32 = 0x000000;
const PROPERTIES_SPECTRUM_BORDER: u32 = 0xffffff;
const PROPERTIES_SPECTRUM_CONTROL_BORDER: u32 = 0x4c4c4c;
const PROPERTIES_FRAME_LIST_GAP: f32 = 16.0;
const PROPERTIES_FRAME_LABEL_GAP: f32 = 4.0;
const PROPERTIES_IMAGE_CONTEXT_MENU_WIDTH: f32 = 170.0;
const PROPERTIES_IMAGE_CONTEXT_MENU_ROW_HEIGHT: f32 = 30.0;
const PROPERTIES_IMAGE_CONTEXT_MENU_ROW_GAP: f32 = 6.0;
const PROPERTIES_IMAGE_CONTEXT_MENU_ICON_SIZE: f32 = 14.0;
const PROPERTIES_IMAGE_CONTEXT_MENU_ICON_SLOT_SIZE: f32 = 14.0;
const PROPERTIES_IMAGE_CONTEXT_MENU_TEXT_SIZE: f32 = 11.0;
const PROPERTIES_IMAGE_CONTEXT_MENU_HORIZONTAL_PADDING: f32 = 18.0;
const PROPERTIES_IMAGE_CONTEXT_MENU_CHILD_GAP: f32 = 10.0;
const PROPERTIES_COVER_NAVIGATION_HEIGHT: f32 = 44.0;
const PROPERTIES_BUTTON_ROW_TOP_PADDING: f32 = 12.0;
const PROPERTIES_BORDER: u32 = 0xe5e5e5;
const PROPERTIES_MUTED_TEXT: u32 = 0x666666;
const PROPERTIES_GROUP_TITLE: u32 = 0x003399;
const PROPERTIES_CLIPBOARD_IMAGE_PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const PROPERTIES_ROW_TYPE_ID: &str = "properties-property-row-type";
const PROPERTIES_ROW_LOCATION_ID: &str = "properties-property-row-location";
const PROPERTIES_ROW_SIZE_ID: &str = "properties-property-row-size";
const PROPERTIES_ROW_SIZE_ON_DISK_ID: &str = "properties-property-row-size-on-disk";
const PROPERTIES_ROW_CONTAINS_ID: &str = "properties-property-row-contains";
const PROPERTIES_ROW_CREATED_ID: &str = "properties-property-row-created";
const PROPERTIES_ROW_MODIFIED_ID: &str = "properties-property-row-modified";
const PROPERTIES_ROW_ACCESSED_ID: &str = "properties-property-row-accessed";
const PROPERTIES_CALCULATING_LABEL: &str = "Calculating...";
const PROPERTY_TREE_CANCELLATION_CHECK_INTERVAL: usize = 4096;
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
    pub(super) size: PropertyValue<u64>,
    pub(super) size_on_disk: PropertyValue<u64>,
    pub(super) contains: Option<PropertyValue<PropertyContains>>,
    pub(super) selection_counts: Option<PropertyValue<PropertyContains>>,
    pub(super) created: MixedValue<SystemTime>,
    pub(super) modified: MixedValue<SystemTime>,
    pub(super) accessed: MixedValue<SystemTime>,
    pub(super) attributes: PropertyAttributes,
    pub(super) owner: MixedValue<String>,
    pub(super) group: MixedValue<String>,
    pub(super) unix_mode: MixedValue<u32>,
    pub(super) permission_summary: MixedValue<String>,
    pub(super) default_app: Option<PropertyDefaultApp>,
    pub(super) run_as_admin: MixedValue<bool>,
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
    pub(super) run_as_admin: Option<bool>,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum PropertyValue<T> {
    Loading,
    Ready(T),
}

impl<T> PropertyValue<T> {
    fn ready(value: T) -> Self {
        Self::Ready(value)
    }

    fn as_ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            Self::Loading => None,
        }
    }

    fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }
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
    Tags,
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
            Self::Tags => "tags",
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
    PropertyDetailGroupKind::Tags,
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
    Cover,
    Spectrum,
    Code,
    Image,
    Frames,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PropertyNavigationDirection {
    Previous,
    Next,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PropertyTabDirection {
    Previous,
    Next,
}

const PROPERTY_TABS: &[(PropertyTab, &str)] = &[
    (PropertyTab::General, "General"),
    (PropertyTab::Details, "Details"),
    (PropertyTab::Cover, "Cover"),
    (PropertyTab::Spectrum, "Spectrum"),
    (PropertyTab::Code, "Code"),
    (PropertyTab::Image, "Image"),
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

#[derive(Clone, Debug, Eq, PartialEq)]
enum PropertyChecksumState {
    NotRequested,
    Loading,
    Ready(FileChecksums),
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PropertyCodeState {
    NotStarted,
    Loading,
    Ready(PropertyCodeSummary),
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PropertyCodeSummary {
    git: GitRepositoryCodeInfo,
    codebase: CodebaseSummary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PropertyDetailsRenderCacheKey {
    snapshot_generation: u64,
    extra_details_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PropertyScrollbarTarget {
    Details,
    Frames,
}

impl PropertyScrollbarTarget {
    fn scrollbar_id(self) -> &'static str {
        match self {
            Self::Details => "properties-details-scrollbar",
            Self::Frames => "properties-frames-scrollbar",
        }
    }
}

enum PropertyFramesState {
    NotStarted,
    Loading(Vec<PropertyFrameThumbnail>),
    Ready(Vec<PropertyFrameThumbnail>),
    Failed(String),
}

enum PropertyCoverState {
    NotStarted,
    Loading,
    Ready(Vec<PropertyCoverImage>),
    Failed(String),
}

enum PropertySpectrumState {
    NotStarted,
    Loading,
    Ready(PropertySpectrumAnalysis),
    Failed(String),
}

fn property_frames_state_label(state: &PropertyFramesState) -> &'static str {
    match state {
        PropertyFramesState::NotStarted => "not-started",
        PropertyFramesState::Loading(_) => "loading",
        PropertyFramesState::Ready(_) => "ready",
        PropertyFramesState::Failed(_) => "failed",
    }
}

fn property_spectrum_state_label(state: &PropertySpectrumState) -> &'static str {
    match state {
        PropertySpectrumState::NotStarted => "not-started",
        PropertySpectrumState::Loading => "loading",
        PropertySpectrumState::Ready(_) => "ready",
        PropertySpectrumState::Failed(_) => "failed",
    }
}

#[derive(Clone, Debug)]
struct PropertyFrameThumbnail {
    label: String,
    image: Arc<RenderImage>,
    width: u32,
    height: u32,
    aspect_ratio: f32,
}

#[derive(Default)]
struct VideoFrameBatchShared {
    pending: Mutex<Vec<PropertyFrameThumbnail>>,
    error: Mutex<Option<String>>,
    finished: AtomicBool,
}

#[derive(Clone, Debug)]
struct PropertyCoverImage {
    label: String,
    preview: PropertyImagePreview,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PropertySpectrumRange {
    low_db: f32,
    high_db: f32,
}

impl Default for PropertySpectrumRange {
    fn default() -> Self {
        Self {
            low_db: PROPERTIES_SPECTRUM_DEFAULT_LOW_DB,
            high_db: PROPERTIES_SPECTRUM_DEFAULT_HIGH_DB,
        }
    }
}

#[derive(Clone, Debug)]
struct PropertySpectrumMetadata {
    header: String,
    sample_rate: u32,
    duration_seconds: f64,
    bit_rate: Option<u64>,
    bit_depth: Option<u32>,
    channels: u32,
}

#[derive(Clone, Debug)]
struct PropertySpectrumAnalysis {
    metadata: PropertySpectrumMetadata,
    db_values: Vec<f32>,
    image: Arc<RenderImage>,
    target: PropertySpectrumTarget,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PropertySpectrumTarget {
    time_bins: usize,
    frequency_bins: usize,
    fft_size: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PropertySpectrumRenderSize {
    width: u32,
    height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PropertySpectrumRenderCacheKey {
    generation: u64,
    source_width: u32,
    source_height: u32,
    target: PropertySpectrumRenderSize,
    low_db_bits: u32,
    high_db_bits: u32,
}

#[derive(Clone)]
struct PropertySpectrumRenderCache {
    key: PropertySpectrumRenderCacheKey,
    image: Arc<RenderImage>,
}

struct PropertySpectrumRefinementState {
    generation: u64,
    pending_target: Option<PropertySpectrumTarget>,
    task: Option<Task<()>>,
    cancel: Option<Arc<AtomicBool>>,
}

impl PropertySpectrumTarget {
    fn initial() -> Self {
        Self {
            time_bins: PROPERTIES_SPECTRUM_INITIAL_TIME_BINS,
            frequency_bins: PROPERTIES_SPECTRUM_FREQUENCY_BINS,
            fft_size: PROPERTIES_SPECTRUM_FFT_SIZE,
        }
    }

    fn from_render_size(size: PropertySpectrumRenderSize) -> Self {
        Self {
            time_bins: (size.width as usize).clamp(1, PROPERTIES_SPECTRUM_MAX_TIME_BINS),
            frequency_bins: PROPERTIES_SPECTRUM_FREQUENCY_BINS,
            fft_size: PROPERTIES_SPECTRUM_FFT_SIZE,
        }
    }

    fn width(self) -> u32 {
        self.time_bins as u32
    }

    fn height(self) -> u32 {
        self.frequency_bins as u32
    }
}

impl PropertySpectrumRenderSize {
    fn from_bounds(bounds: Bounds<Pixels>, scale_factor: f32) -> Option<Self> {
        fn dimension(logical_pixels: Pixels, scale_factor: f32) -> Option<u32> {
            let device_pixels = f32::from(logical_pixels) * scale_factor;
            if !device_pixels.is_finite() || device_pixels <= 0.0 {
                return None;
            }
            Some(device_pixels.round().clamp(1.0, u32::MAX as f32) as u32)
        }

        Some(Self {
            width: dimension(bounds.size.width, scale_factor)?,
            height: dimension(bounds.size.height, scale_factor)?,
        })
    }
}

impl PropertySpectrumRenderCacheKey {
    fn new(
        generation: u64,
        source_width: u32,
        source_height: u32,
        target: PropertySpectrumRenderSize,
        range: PropertySpectrumRange,
    ) -> Self {
        Self {
            generation,
            source_width,
            source_height,
            target,
            low_db_bits: range.low_db.to_bits(),
            high_db_bits: range.high_db.to_bits(),
        }
    }
}

impl Default for PropertySpectrumRefinementState {
    fn default() -> Self {
        Self {
            generation: 0,
            pending_target: None,
            task: None,
            cancel: None,
        }
    }
}

#[derive(Clone)]
struct PropertyImageCopyPayload {
    image: Arc<RenderImage>,
    width: u32,
    height: u32,
}

#[derive(Clone)]
struct PropertyImageContextMenu {
    origin: gpui::Point<gpui::Pixels>,
    payload: PropertyImageCopyPayload,
}

impl PropertyFrameThumbnail {
    fn copy_payload(&self) -> PropertyImageCopyPayload {
        PropertyImageCopyPayload {
            image: self.image.clone(),
            width: self.width,
            height: self.height,
        }
    }
}

fn property_image_preview_copy_payload(preview: &PropertyImagePreview) -> PropertyImageCopyPayload {
    PropertyImageCopyPayload {
        image: preview.image.clone(),
        width: preview.width,
        height: preview.height,
    }
}

pub(super) struct PropertiesDialog {
    target: PropertyTarget,
    explorer: WeakEntity<ExplorerView>,
    date_format: String,
    font: gpui::Font,
    focus_handle: FocusHandle,
    active_tab: PropertyTab,
    snapshot_state: PropertySnapshotState,
    snapshot_generation: u64,
    tree_summary_generation: u64,
    details_state: PropertyDetailsState,
    details_generation: u64,
    checksum_state: PropertyChecksumState,
    checksum_generation: u64,
    code_state: PropertyCodeState,
    code_generation: u64,
    details_render_cache_key: Option<PropertyDetailsRenderCacheKey>,
    details_render_cache: Vec<PropertyDetailGroup>,
    details_scroll_handle: ScrollHandle,
    details_scrollbar_hovered: bool,
    details_scrollbar_drag: Option<ScrollbarDrag>,
    image_viewer: Option<Entity<ImageViewerSurface>>,
    image_viewer_path: Option<PathBuf>,
    cover_state: PropertyCoverState,
    cover_generation: u64,
    cover_index: usize,
    spectrum_state: PropertySpectrumState,
    spectrum_generation: u64,
    spectrum_range: PropertySpectrumRange,
    spectrum_render_size: Option<PropertySpectrumRenderSize>,
    spectrum_render_cache: Option<PropertySpectrumRenderCache>,
    spectrum_refinement: PropertySpectrumRefinementState,
    frames_state: PropertyFramesState,
    frames_generation: u64,
    frames_scroll_handle: ScrollHandle,
    frames_scrollbar_hovered: bool,
    frames_scrollbar_drag: Option<ScrollbarDrag>,
    image_copy_context_menu: Option<PropertyImageContextMenu>,
    snapshot_task: Option<Task<()>>,
    tree_summary_task: Option<Task<()>>,
    tree_summary_cancel: Option<Arc<AtomicBool>>,
    details_task: Option<Task<()>>,
    checksum_task: Option<Task<()>>,
    checksum_cancel: Option<Arc<AtomicBool>>,
    code_task: Option<Task<()>>,
    cover_task: Option<Task<()>>,
    spectrum_task: Option<Task<()>>,
    spectrum_cancel: Option<Arc<AtomicBool>>,
    frames_task: Option<Task<()>>,
    frames_cancel: Option<Arc<AtomicBool>>,
    apply_task: Option<Task<()>>,
    default_app_task: Option<Task<()>>,
    #[cfg(target_os = "linux")]
    default_app_picker: Option<AnyWindowHandle>,
    draft: EditablePropertyDraft,
    apply_error: Option<String>,
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

        self.open_properties_for_paths(paths, window, cx);
    }

    pub(super) fn handle_open_properties(
        &mut self,
        _: &crate::explorer::OpenProperties,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.commit_active_rename_before_interaction(window, cx) {
            return;
        }

        let paths = self.selected_or_current_property_paths();
        self.open_properties_for_paths(paths, window, cx);
        cx.notify();
    }

    fn selected_or_current_property_paths(&self) -> Vec<PathBuf> {
        let paths = self.selected_paths();
        if paths.is_empty() {
            vec![self.path.clone()]
        } else {
            paths
        }
    }

    pub(super) fn open_properties_for_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.close_context_menu();
        self.open_utility_menu = None;
        match open_properties_window(
            PropertyTarget { paths },
            cx.entity(),
            self.date_format.clone(),
            window,
            cx,
        ) {
            Ok(_) => self.clear_operation_notice(),
            Err(error) => self.set_error_notice(format!("Failed to open Properties: {error}")),
        }
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
            snapshot_generation: 0,
            tree_summary_generation: 0,
            details_state: PropertyDetailsState::NotStarted,
            details_generation: 0,
            checksum_state: PropertyChecksumState::NotRequested,
            checksum_generation: 0,
            code_state: PropertyCodeState::NotStarted,
            code_generation: 0,
            details_render_cache_key: None,
            details_render_cache: Vec::new(),
            details_scroll_handle: ScrollHandle::new(),
            details_scrollbar_hovered: false,
            details_scrollbar_drag: None,
            image_viewer: None,
            image_viewer_path: None,
            cover_state: PropertyCoverState::NotStarted,
            cover_generation: 0,
            cover_index: 0,
            spectrum_state: PropertySpectrumState::NotStarted,
            spectrum_generation: 0,
            spectrum_range: PropertySpectrumRange::default(),
            spectrum_render_size: None,
            spectrum_render_cache: None,
            spectrum_refinement: PropertySpectrumRefinementState::default(),
            frames_state: PropertyFramesState::NotStarted,
            frames_generation: 0,
            frames_scroll_handle: ScrollHandle::new(),
            frames_scrollbar_hovered: false,
            frames_scrollbar_drag: None,
            image_copy_context_menu: None,
            snapshot_task: None,
            tree_summary_task: None,
            tree_summary_cancel: None,
            details_task: None,
            checksum_task: None,
            checksum_cancel: None,
            code_task: None,
            cover_task: None,
            spectrum_task: None,
            spectrum_cancel: None,
            frames_task: None,
            frames_cancel: None,
            apply_task: None,
            default_app_task: None,
            #[cfg(target_os = "linux")]
            default_app_picker: None,
            draft: EditablePropertyDraft::default(),
            apply_error: None,
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
        self.cancel_tree_summary_task();
        self.snapshot_state = PropertySnapshotState::Loading;
        self.reset_details_state();
        self.reset_checksum_state();
        self.reset_code_state();
        self.reset_image_state();
        self.reset_cover_state();
        self.reset_spectrum_state();
        self.reset_frames_state();
        let target = self.target.clone();
        let date_format = self.date_format.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let path_count = target.paths.len();
                    let started = Instant::now();
                    let result =
                        collect_property_snapshot_fast_with_date_format(target, &date_format);
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
        self.clear_details_render_cache();
        self.details_task = None;
        self.details_scrollbar_drag = None;
        self.set_details_scroll_top(0.0);
    }

    fn reset_checksum_state(&mut self) {
        self.cancel_checksum_task();
        self.checksum_state = PropertyChecksumState::NotRequested;
    }

    fn reset_code_state(&mut self) {
        self.code_generation = self.code_generation.wrapping_add(1);
        self.code_state = PropertyCodeState::NotStarted;
        self.code_task = None;
    }

    fn reset_image_state(&mut self) {
        self.image_viewer = None;
        self.image_viewer_path = None;
        self.image_copy_context_menu = None;
    }

    fn reset_cover_state(&mut self) {
        self.cover_generation = self.cover_generation.wrapping_add(1);
        self.cover_state = PropertyCoverState::NotStarted;
        self.cover_index = 0;
        self.cover_task = None;
        self.image_copy_context_menu = None;
    }

    fn reset_spectrum_state(&mut self) {
        self.cancel_spectrum_task();
        self.cancel_spectrum_refinement_task();
        self.spectrum_generation = self.spectrum_generation.wrapping_add(1);
        self.spectrum_state = PropertySpectrumState::NotStarted;
        self.spectrum_range = PropertySpectrumRange::default();
        self.spectrum_render_size = None;
        self.clear_spectrum_render_cache();
    }

    fn cancel_spectrum_task(&mut self) {
        if let Some(cancel) = self.spectrum_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.spectrum_task = None;
    }

    fn cancel_spectrum_refinement_task(&mut self) {
        self.spectrum_refinement.generation = self.spectrum_refinement.generation.wrapping_add(1);
        if let Some(cancel) = self.spectrum_refinement.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.spectrum_refinement.pending_target = None;
        self.spectrum_refinement.task = None;
    }

    fn clear_spectrum_render_cache(&mut self) {
        self.spectrum_render_cache = None;
    }

    fn reset_frames_state(&mut self) {
        self.cancel_frames_task();
        self.frames_generation = self.frames_generation.wrapping_add(1);
        self.frames_state = PropertyFramesState::NotStarted;
        self.frames_scrollbar_drag = None;
        self.image_copy_context_menu = None;
        let offset = self.frames_scroll_handle.offset();
        self.frames_scroll_handle
            .set_offset(point(offset.x, px(0.0)));
    }

    fn cancel_frames_task(&mut self) {
        if let Some(cancel) = self.frames_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.frames_task = None;
    }

    fn set_ready_snapshot(&mut self, snapshot: PropertySnapshot, cx: &mut Context<Self>) {
        self.cancel_tree_summary_task();
        self.snapshot_generation = self.snapshot_generation.wrapping_add(1);
        self.draft = EditablePropertyDraft::from_snapshot(&snapshot);
        self.snapshot_state = PropertySnapshotState::Ready(snapshot);
        self.reset_details_state();
        self.reset_checksum_state();
        self.reset_code_state();
        self.reset_image_state();
        self.reset_cover_state();
        self.reset_spectrum_state();
        self.reset_frames_state();
        if let PropertySnapshotState::Ready(snapshot) = &self.snapshot_state {
            if !property_tab_is_visible(self.active_tab, Some(snapshot)) {
                self.active_tab = PropertyTab::General;
            }
        }
        match self.active_tab {
            PropertyTab::Details => self.start_details_task(cx),
            PropertyTab::Code => self.start_code_task(cx),
            PropertyTab::Cover => self.start_cover_task(cx),
            PropertyTab::Spectrum => self.start_spectrum_task(cx),
            PropertyTab::Frames => self.start_frames_task(cx),
            PropertyTab::General | PropertyTab::Image => {}
        }
        self.start_tree_summary_task(cx);
    }

    fn start_tree_summary_task(&mut self, cx: &mut Context<Self>) {
        let PropertySnapshotState::Ready(snapshot) = &self.snapshot_state else {
            return;
        };
        if !snapshot_needs_tree_summary(snapshot) {
            return;
        }

        self.tree_summary_generation = self.tree_summary_generation.wrapping_add(1);
        let generation = self.tree_summary_generation;
        let target = snapshot.target.clone();
        let date_format = self.date_format.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.tree_summary_cancel = Some(cancel.clone());
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let cancel = cancel.clone();
                    async move {
                        let path_count = target.paths.len();
                        let started = Instant::now();
                        let result = collect_property_snapshot_full_with_date_format(
                            target,
                            &date_format,
                            &cancel,
                        );
                        match &result {
                            Ok(snapshot) => crate::debug_options::log_property_timing(
                                started.elapsed(),
                                format_args!(
                                    "tree summary ready paths={} details={} title={:?}",
                                    path_count,
                                    detail_count(&snapshot.details),
                                    snapshot.title
                                ),
                            ),
                            Err(error) => crate::debug_options::log_property_timing(
                                started.elapsed(),
                                format_args!(
                                    "tree summary failed paths={} error={:?}",
                                    path_count, error
                                ),
                            ),
                        }
                        result
                    }
                })
                .await;

            let _ = this.update(cx, |dialog, cx| {
                if dialog.tree_summary_generation != generation
                    || dialog
                        .tree_summary_cancel
                        .as_ref()
                        .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
                {
                    return;
                }

                dialog.tree_summary_task = None;
                dialog.tree_summary_cancel = None;
                if let Ok(snapshot) = result {
                    dialog.apply_tree_summary_snapshot(snapshot);
                    cx.notify();
                }
            });
        });
        self.tree_summary_task = Some(task);
    }

    fn cancel_tree_summary_task(&mut self) {
        self.tree_summary_generation = self.tree_summary_generation.wrapping_add(1);
        if let Some(cancel) = self.tree_summary_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.tree_summary_task = None;
    }

    fn apply_tree_summary_snapshot(&mut self, tree_snapshot: PropertySnapshot) {
        let PropertySnapshotState::Ready(snapshot) = &mut self.snapshot_state else {
            return;
        };
        if snapshot.target != tree_snapshot.target {
            return;
        }

        snapshot.size = tree_snapshot.size;
        snapshot.size_on_disk = tree_snapshot.size_on_disk;
        snapshot.contains = tree_snapshot.contains;
        snapshot.selection_counts = tree_snapshot.selection_counts;
        snapshot.details = tree_snapshot.details;
        self.snapshot_generation = self.snapshot_generation.wrapping_add(1);
        self.clear_details_render_cache();
    }

    fn clear_details_render_cache(&mut self) {
        self.details_render_cache_key = None;
        self.details_render_cache.clear();
    }

    fn detail_groups_for_render_cached(
        &mut self,
        snapshot: &PropertySnapshot,
    ) -> &[PropertyDetailGroup] {
        detail_groups_for_render_cached(
            &mut self.details_render_cache_key,
            &mut self.details_render_cache,
            snapshot,
            &self.details_state,
            self.snapshot_generation,
        )
    }

    fn start_details_task(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.details_state, PropertyDetailsState::NotStarted) {
            return;
        }
        let PropertySnapshotState::Ready(snapshot) = &self.snapshot_state else {
            return;
        };
        if single_file_path(&snapshot.target, snapshot.item_kind).is_none() {
            self.details_state = PropertyDetailsState::Ready(Vec::new());
            self.clear_details_render_cache();
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
                    let groups = collect_single_file_detail_groups(&target, item_kind);
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
                    dialog.clear_details_render_cache();
                    cx.notify();
                }
            });
        });
        self.details_task = Some(task);
    }

    fn start_checksum_task(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.checksum_state,
            PropertyChecksumState::Loading | PropertyChecksumState::Ready(_)
        ) {
            return;
        }
        let PropertySnapshotState::Ready(snapshot) = &self.snapshot_state else {
            return;
        };
        let Some(path) =
            single_file_path(&snapshot.target, snapshot.item_kind).map(Path::to_path_buf)
        else {
            return;
        };
        let cache_key = match file_checksum_cache_key(&path) {
            Ok(cache_key) => cache_key,
            Err(error) => {
                self.checksum_state = PropertyChecksumState::Failed(error.to_string());
                cx.notify();
                return;
            }
        };
        if let Some(checksums) = cx
            .try_global::<FileChecksumCache>()
            .and_then(|cache| cache.get(&cache_key))
        {
            self.checksum_state = PropertyChecksumState::Ready(checksums);
            cx.notify();
            return;
        }

        self.cancel_checksum_task();
        self.checksum_generation = self.checksum_generation.wrapping_add(1);
        let generation = self.checksum_generation;
        let cancel = Arc::new(AtomicBool::new(false));
        self.checksum_cancel = Some(cancel.clone());
        self.checksum_state = PropertyChecksumState::Loading;
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let cancel = cancel.clone();
                    async move {
                        let started = Instant::now();
                        let result = file_checksums_with_cancel(&path, &cancel)
                            .map(|checksums| (cache_key, checksums));
                        crate::debug_options::log_property_timing(
                            started.elapsed(),
                            format_args!(
                                "checksums ready path={} ok={}",
                                path.display(),
                                result.is_ok()
                            ),
                        );
                        result
                    }
                })
                .await;

            let _ = this.update(cx, |dialog, cx| {
                if dialog.checksum_generation != generation
                    || dialog
                        .checksum_cancel
                        .as_ref()
                        .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
                {
                    return;
                }

                dialog.checksum_task = None;
                dialog.checksum_cancel = None;
                match result {
                    Ok((cache_key, checksums)) => {
                        if let Some(cache) = cx.try_global::<FileChecksumCache>() {
                            cache.insert(cache_key, checksums.clone());
                        }
                        dialog.checksum_state = PropertyChecksumState::Ready(checksums);
                    }
                    Err(error) => {
                        dialog.checksum_state = PropertyChecksumState::Failed(error.to_string());
                    }
                }
                cx.notify();
            });
        });
        self.checksum_task = Some(task);
        cx.notify();
    }

    fn cancel_checksum_task(&mut self) {
        self.checksum_generation = self.checksum_generation.wrapping_add(1);
        if let Some(cancel) = self.checksum_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.checksum_task = None;
    }

    fn start_code_task(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.code_state, PropertyCodeState::NotStarted) {
            return;
        }
        let PropertySnapshotState::Ready(snapshot) = &self.snapshot_state else {
            return;
        };
        let Some(repo_root) =
            single_folder_direct_git_repository_root(&snapshot.target, snapshot.item_kind)
                .map(Path::to_path_buf)
        else {
            self.code_state = PropertyCodeState::Failed(
                "Code information is not available for this item.".to_owned(),
            );
            return;
        };

        self.code_state = PropertyCodeState::Loading;
        let generation = self.code_generation;
        let task = cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let result = cx
                .background_executor()
                .spawn({
                    let repo_root = repo_root.clone();
                    async move { collect_property_code_summary(&repo_root) }
                })
                .await;

            match &result {
                Ok(summary) => crate::debug_options::log_property_timing(
                    started.elapsed(),
                    format_args!(
                        "code ready path={} branch={} commits={} languages={}",
                        repo_root.display(),
                        summary.git.branch,
                        summary.git.commit_count,
                        summary.codebase.languages.len()
                    ),
                ),
                Err(error) => crate::debug_options::log_property_timing(
                    started.elapsed(),
                    format_args!("code failed path={} error={}", repo_root.display(), error),
                ),
            }

            let _ = this.update(cx, |dialog, cx| {
                if dialog.code_generation == generation {
                    dialog.code_task = None;
                    dialog.code_state = match result {
                        Ok(summary) => PropertyCodeState::Ready(summary),
                        Err(error) => PropertyCodeState::Failed(error),
                    };
                    cx.notify();
                }
            });
        });
        self.code_task = Some(task);
    }

    fn ensure_image_viewer(
        &mut self,
        snapshot: &PropertySnapshot,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ImageViewerSurface>> {
        let path = single_file_image_path(&snapshot.target, snapshot.item_kind)?;
        if self.image_viewer_path.as_deref() != Some(path) {
            let path = path.to_path_buf();
            let viewer = new_embedded_image_viewer(path.clone(), self.focus_handle.clone(), cx);
            cx.subscribe_in(
                &viewer,
                window,
                |dialog, _, event, window, cx| match event {
                    ImageViewerEvent::OpenPath(path) => {
                        dialog.retarget_to_image_path(path.clone(), window, cx);
                    }
                },
            )
            .detach();
            self.image_viewer = Some(viewer);
            self.image_viewer_path = Some(path);
        }

        self.image_viewer.clone()
    }

    fn retarget_to_image_path(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.target = PropertyTarget {
            paths: vec![path.clone()],
        };
        self.active_tab = PropertyTab::Image;
        window.set_window_title(&properties_window_title(&self.target.paths));
        self.start_snapshot_task(cx);
        cx.notify();
    }

    fn retarget_to_path_preserving_tab(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.target = PropertyTarget {
            paths: vec![path.clone()],
        };
        window.set_window_title(&properties_window_title(&self.target.paths));
        self.start_snapshot_task(cx);
        cx.notify();
    }

    fn start_cover_task(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.cover_state, PropertyCoverState::NotStarted) {
            return;
        }
        let PropertySnapshotState::Ready(snapshot) = &self.snapshot_state else {
            return;
        };
        let Some(path) =
            single_file_audio_path(&snapshot.target, snapshot.item_kind).map(Path::to_path_buf)
        else {
            self.cover_state = PropertyCoverState::Failed(
                "Audio covers are not available for this item.".to_owned(),
            );
            return;
        };

        self.cover_state = PropertyCoverState::Loading;
        let generation = self.cover_generation;
        let task = cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let result = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        let path = property_media_local_path(path)?;
                        load_audio_cover_previews(&path)
                    }
                })
                .await;

            match &result {
                Ok(covers) => crate::debug_options::log_property_timing(
                    started.elapsed(),
                    format_args!(
                        "audio covers ready path={} covers={}",
                        path.display(),
                        covers.len()
                    ),
                ),
                Err(error) => crate::debug_options::log_property_timing(
                    started.elapsed(),
                    format_args!(
                        "audio covers failed path={} error={}",
                        path.display(),
                        error
                    ),
                ),
            }

            let _ = this.update(cx, |dialog, cx| {
                if dialog.cover_generation == generation {
                    dialog.cover_task = None;
                    dialog.cover_index = 0;
                    dialog.cover_state = match result {
                        Ok(covers) => PropertyCoverState::Ready(covers),
                        Err(error) => PropertyCoverState::Failed(error),
                    };
                    cx.notify();
                }
            });
        });
        self.cover_task = Some(task);
    }

    fn start_spectrum_task(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.spectrum_state, PropertySpectrumState::NotStarted) {
            crate::debug_options::log_property_marker(format_args!(
                "spectrum start skipped state={}",
                property_spectrum_state_label(&self.spectrum_state)
            ));
            return;
        }
        let PropertySnapshotState::Ready(snapshot) = &self.snapshot_state else {
            crate::debug_options::log_property_marker(format_args!(
                "spectrum start skipped state=snapshot-not-ready"
            ));
            return;
        };
        let Some(path) =
            single_file_audio_path(&snapshot.target, snapshot.item_kind).map(Path::to_path_buf)
        else {
            self.spectrum_state = PropertySpectrumState::Failed(
                "Audio spectrum is not available for this item.".to_owned(),
            );
            return;
        };

        self.spectrum_state = PropertySpectrumState::Loading;
        let generation = self.spectrum_generation;
        let range = self.spectrum_range;
        let target = PropertySpectrumTarget::initial();
        let cancel = Arc::new(AtomicBool::new(false));
        self.spectrum_cancel = Some(cancel.clone());
        crate::debug_options::log_property_marker(format_args!(
            "spectrum task started path={} generation={} columns={} rows={}",
            path.display(),
            generation,
            target.time_bins,
            target.frequency_bins
        ));
        let task = cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let media_path = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move { property_media_local_path(path) }
                })
                .await;
            let path = match media_path {
                Ok(path) => path,
                Err(error) => {
                    let _ = this.update(cx, |dialog, cx| {
                        if dialog.spectrum_generation == generation {
                            dialog.spectrum_task = None;
                            dialog.spectrum_cancel = None;
                            dialog.spectrum_state = PropertySpectrumState::Failed(error);
                            cx.notify();
                        }
                    });
                    return;
                }
            };

            let result = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    let cancel = cancel.clone();
                    async move { load_audio_spectrum_analysis(&path, range, target, &cancel) }
                })
                .await;

            match &result {
                Ok(analysis) => crate::debug_options::log_property_timing(
                    started.elapsed(),
                    format_args!(
                        "spectrum ready path={} duration={:.3}s sample_rate={} bit_rate={:?} bit_depth={:?} channels={} columns={} rows={}",
                        path.display(),
                        analysis.metadata.duration_seconds,
                        analysis.metadata.sample_rate,
                        analysis.metadata.bit_rate,
                        analysis.metadata.bit_depth,
                        analysis.metadata.channels,
                        analysis.width,
                        analysis.height
                    ),
                ),
                Err(error) => crate::debug_options::log_property_timing(
                    started.elapsed(),
                    format_args!("spectrum failed path={} error={}", path.display(), error),
                ),
            }

            let _ = this.update(cx, |dialog, cx| {
                if dialog.spectrum_generation == generation {
                    dialog.spectrum_task = None;
                    dialog.spectrum_cancel = None;
                    dialog.spectrum_state = match result {
                        Ok(analysis) => PropertySpectrumState::Ready(analysis),
                        Err(error) => PropertySpectrumState::Failed(error),
                    };
                    cx.notify();
                }
            });
        });
        self.spectrum_task = Some(task);
    }

    fn schedule_spectrum_resize_refinement(&mut self, cx: &mut Context<Self>) {
        let Some(render_size) = self.spectrum_render_size else {
            return;
        };
        let target = PropertySpectrumTarget::from_render_size(render_size);
        let PropertySpectrumState::Ready(analysis) = &self.spectrum_state else {
            return;
        };
        if analysis.target == target {
            if self.spectrum_refinement.pending_target.is_some() {
                self.cancel_spectrum_refinement_task();
            }
            return;
        }
        if self.spectrum_refinement.pending_target == Some(target) {
            return;
        }
        let Some(path) = self
            .ready_snapshot()
            .and_then(|snapshot| single_file_audio_path(&snapshot.target, snapshot.item_kind))
            .map(Path::to_path_buf)
        else {
            return;
        };

        if let Some(cancel) = self.spectrum_refinement.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.spectrum_refinement.task = None;
        self.spectrum_refinement.generation = self.spectrum_refinement.generation.wrapping_add(1);
        self.spectrum_refinement.pending_target = Some(target);
        let refinement_generation = self.spectrum_refinement.generation;
        let spectrum_generation = self.spectrum_generation;
        let range = self.spectrum_range;
        let cancel = Arc::new(AtomicBool::new(false));
        self.spectrum_refinement.cancel = Some(cancel.clone());
        crate::debug_options::log_property_marker(format_args!(
            "spectrum resize refinement scheduled path={} generation={} refinement={} columns={} rows={}",
            path.display(),
            spectrum_generation,
            refinement_generation,
            target.time_bins,
            target.frequency_bins
        ));

        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(PROPERTIES_SPECTRUM_RESIZE_DEBOUNCE_MS))
                .await;
            if cancel.load(Ordering::Relaxed) {
                return;
            }

            let started = Instant::now();
            let media_path = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move { property_media_local_path(path) }
                })
                .await;
            let path = match media_path {
                Ok(path) => path,
                Err(error) => {
                    let _ = this.update(cx, |dialog, cx| {
                        if dialog.apply_spectrum_refinement_result(
                            spectrum_generation,
                            refinement_generation,
                            target,
                            Err(error),
                        ) {
                            cx.notify();
                        }
                    });
                    return;
                }
            };

            let result = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    let cancel = cancel.clone();
                    async move { load_audio_spectrum_analysis(&path, range, target, &cancel) }
                })
                .await;

            match &result {
                Ok(analysis) => crate::debug_options::log_property_timing(
                    started.elapsed(),
                    format_args!(
                        "spectrum resize refinement ready path={} duration={:.3}s sample_rate={} bit_rate={:?} bit_depth={:?} channels={} columns={} rows={}",
                        path.display(),
                        analysis.metadata.duration_seconds,
                        analysis.metadata.sample_rate,
                        analysis.metadata.bit_rate,
                        analysis.metadata.bit_depth,
                        analysis.metadata.channels,
                        analysis.width,
                        analysis.height
                    ),
                ),
                Err(error) => crate::debug_options::log_property_timing(
                    started.elapsed(),
                    format_args!(
                        "spectrum resize refinement failed path={} columns={} rows={} error={}",
                        path.display(),
                        target.time_bins,
                        target.frequency_bins,
                        error
                    ),
                ),
            }

            let _ = this.update(cx, |dialog, cx| {
                if dialog.apply_spectrum_refinement_result(
                    spectrum_generation,
                    refinement_generation,
                    target,
                    result,
                ) {
                    cx.notify();
                }
            });
        });
        self.spectrum_refinement.task = Some(task);
    }

    fn apply_spectrum_refinement_result(
        &mut self,
        spectrum_generation: u64,
        refinement_generation: u64,
        target: PropertySpectrumTarget,
        result: Result<PropertySpectrumAnalysis, String>,
    ) -> bool {
        if self.spectrum_generation != spectrum_generation
            || self.spectrum_refinement.generation != refinement_generation
        {
            return false;
        }

        self.spectrum_refinement.task = None;
        self.spectrum_refinement.cancel = None;
        if self.spectrum_refinement.pending_target == Some(target) {
            self.spectrum_refinement.pending_target = None;
        }

        let Ok(mut analysis) = result else {
            return true;
        };
        if let Some(image) = spectrum_render_image(
            &analysis.db_values,
            analysis.width,
            analysis.height,
            self.spectrum_range,
        ) {
            analysis.image = image;
        }
        self.spectrum_state = PropertySpectrumState::Ready(analysis);
        self.clear_spectrum_render_cache();
        true
    }

    fn start_frames_task(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.frames_state, PropertyFramesState::NotStarted) {
            crate::debug_options::log_property_marker(format_args!(
                "video frames start skipped state={}",
                property_frames_state_label(&self.frames_state)
            ));
            return;
        }
        let PropertySnapshotState::Ready(snapshot) = &self.snapshot_state else {
            crate::debug_options::log_property_marker(format_args!(
                "video frames start skipped state=snapshot-not-ready"
            ));
            return;
        };
        let Some(path) =
            single_file_video_path(&snapshot.target, snapshot.item_kind).map(Path::to_path_buf)
        else {
            crate::debug_options::log_property_marker(format_args!(
                "video frames unavailable target_kind={:?}",
                snapshot.item_kind
            ));
            self.frames_state = PropertyFramesState::Failed(
                "Video frames are not available for this item.".to_owned(),
            );
            return;
        };

        self.frames_state = PropertyFramesState::Loading(Vec::new());
        let generation = self.frames_generation;
        let cancel = Arc::new(AtomicBool::new(false));
        self.frames_cancel = Some(cancel.clone());
        crate::debug_options::log_property_marker(format_args!(
            "video frames task started path={} generation={}",
            path.display(),
            generation
        ));
        let task = cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let media_path = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move { property_media_local_path(path) }
                })
                .await;
            let path = match media_path {
                Ok(path) => path,
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
                            dialog.frames_cancel = None;
                            dialog.frames_state = PropertyFramesState::Failed(error);
                            cx.notify();
                        }
                    });
                    return;
                }
            };
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
                            dialog.frames_cancel = None;
                            dialog.frames_state = PropertyFramesState::Failed(error);
                            cx.notify();
                        }
                    });
                    return;
                }
            };

            let request_count = requests.len();
            let shared = Arc::new(VideoFrameBatchShared::default());
            let worker = cx.background_executor().spawn({
                let path = path.clone();
                let requests = requests.clone();
                let cancel = cancel.clone();
                let shared = shared.clone();
                async move {
                    let seeks = requests
                        .iter()
                        .map(|request| request.seek_seconds)
                        .collect::<Vec<_>>();
                    let result = extract_video_frame_batch(
                        &path,
                        &seeks,
                        &cancel,
                        |index, frame| {
                            let Some(request) = requests.get(index) else {
                                return;
                            };
                            let label = video_frame_timestamp_label(request.label_seconds);
                            let thumbnail = prepare_video_frame_thumbnail_rgba(label, frame);
                            if let Ok(mut pending) = shared.pending.lock() {
                                pending.push(thumbnail);
                            }
                        },
                    );
                    if let Ok(metrics) = result.as_ref() {
                        crate::debug_options::log_property_timing(
                            metrics.total,
                            format_args!(
                                "video frame batch extracted path={} processes={} first_frame={:.3}ms stream_parse={:.3}ms render_prepare={:.3}ms",
                                path.display(),
                                metrics.processes,
                                metrics.first_frame.unwrap_or_default().as_secs_f64() * 1000.0,
                                metrics.stream_parse.as_secs_f64() * 1000.0,
                                metrics.render_prepare.as_secs_f64() * 1000.0,
                            ),
                        );
                    } else if let Err(error) = result
                        && !error.is_cancelled()
                        && let Ok(mut shared_error) = shared.error.lock()
                    {
                        *shared_error = Some(error.to_string());
                    }
                    shared.finished.store(true, Ordering::Release);
                }
            });

            let mut frame_count = 0usize;
            loop {
                let mut pending = shared
                    .pending
                    .lock()
                    .map(|mut pending| std::mem::take(&mut *pending))
                    .unwrap_or_default();
                let finished = shared.finished.load(Ordering::Acquire);
                let should_continue = this
                    .update(cx, |dialog, cx| {
                        if dialog.frames_generation != generation
                            || cancel.load(Ordering::Relaxed)
                        {
                            return false;
                        }
                        let PropertyFramesState::Loading(frames) = &mut dialog.frames_state else {
                            return false;
                        };
                        if !pending.is_empty() {
                            frames.append(&mut pending);
                            frame_count = frames.len();
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !should_continue {
                    cancel.store(true, Ordering::Relaxed);
                    worker.await;
                    return;
                }
                if finished {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(VIDEO_FRAME_PUBLISH_INTERVAL_MS))
                    .await;
            }
            worker.await;

            let extraction_error = shared.error.lock().ok().and_then(|error| error.clone());
            let error = (frame_count == 0).then(|| {
                format!(
                    "ffmpeg failed to extract video frames: {}",
                    extraction_error.unwrap_or_else(|| "no frame data was returned".to_owned())
                )
            });
            if let Some(error) = error.as_ref() {
                crate::debug_options::log_property_timing(
                    started.elapsed(),
                    format_args!(
                        "video frames failed path={} attempts={} successes={} error={}",
                        path.display(),
                        request_count,
                        frame_count,
                        error
                    ),
                );
            } else {
                crate::debug_options::log_property_timing(
                    started.elapsed(),
                    format_args!(
                        "video frames ready path={} attempts={} successes={}",
                        path.display(),
                        request_count,
                        frame_count,
                    ),
                );
            }

            let _ = this.update(cx, |dialog, cx| {
                if dialog.frames_generation == generation {
                    dialog.frames_task = None;
                    dialog.frames_cancel = None;
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

    fn handle_properties_open_previous(
        &mut self,
        _: &PropertiesOpenPrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_adjacent_properties_target(PropertyNavigationDirection::Previous, window, cx);
    }

    fn handle_properties_open_next(
        &mut self,
        _: &PropertiesOpenNext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_adjacent_properties_target(PropertyNavigationDirection::Next, window, cx);
    }

    fn handle_select_previous_tab(
        &mut self,
        _: &SelectPreviousTab,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_adjacent_property_tab(PropertyTabDirection::Previous, cx);
    }

    fn handle_select_next_tab(
        &mut self,
        _: &SelectNextTab,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_adjacent_property_tab(PropertyTabDirection::Next, cx);
    }

    fn open_adjacent_properties_target(
        &mut self,
        direction: PropertyNavigationDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.ready_snapshot().and_then(|snapshot| {
            adjacent_property_path(&snapshot.target, snapshot.item_kind, direction)
        }) else {
            return;
        };

        self.retarget_to_path_preserving_tab(path, window, cx);
    }

    fn select_adjacent_property_tab(
        &mut self,
        direction: PropertyTabDirection,
        cx: &mut Context<Self>,
    ) {
        let snapshot = self.ready_snapshot();
        if let Some(tab) = adjacent_property_tab(self.active_tab, snapshot, direction) {
            self.set_active_tab(tab, cx);
        }
    }

    fn close(&mut self, window: &mut Window, _: &mut Context<Self>) {
        self.completed = true;
        self.cancel_tree_summary_task();
        self.cancel_checksum_task();
        self.cancel_spectrum_task();
        self.cancel_spectrum_refinement_task();
        self.cancel_frames_task();
        window.remove_window();
    }

    fn release(&mut self, _cx: &mut App) {
        self.completed = true;
        self.cancel_tree_summary_task();
        self.cancel_checksum_task();
        self.cancel_spectrum_task();
        self.cancel_spectrum_refinement_task();
        self.cancel_frames_task();
        #[cfg(target_os = "linux")]
        if let Some(handle) = self.default_app_picker.take() {
            let _ = handle.update(_cx, |_, window, _| window.remove_window());
        }
    }

    fn open_image_copy_context_menu(
        &mut self,
        payload: PropertyImageCopyPayload,
        origin: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.image_copy_context_menu = Some(PropertyImageContextMenu { origin, payload });
        cx.notify();
    }

    fn close_image_copy_context_menu(&mut self) -> bool {
        self.image_copy_context_menu.take().is_some()
    }

    fn copy_property_image_payload_to_clipboard(
        &mut self,
        payload: PropertyImageCopyPayload,
        cx: &mut Context<Self>,
    ) {
        match clipboard_image_from_property_image_payload(&payload) {
            Ok(image) => cx.write_to_clipboard(ClipboardItem::new_image(&image)),
            Err(error) => crate::debug_options::log_property_marker(format_args!(
                "image copy failed width={} height={} error={}",
                payload.width, payload.height, error
            )),
        }
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
                        collect_property_snapshot_fast_with_date_format(target, &date_format).ok();
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

    #[cfg(target_os = "windows")]
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

        let before = snapshot.default_app.clone();
        let parent = crate::explorer::windows_shell::parent_hwnd(window);
        let task = cx.spawn(async move |this, cx| {
            let result =
                crate::explorer::open_with::windows_change_default_application_for_file_with_parent(
                    &path, parent,
                );

            let _ = this.update(cx, |dialog, cx| {
                dialog.refresh_after_default_app_change(path, before, result, cx);
            });
        });
        self.default_app_task = Some(task);
        cx.notify();
    }

    #[cfg(target_os = "macos")]
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

        let before = snapshot.default_app.clone();
        let result = crate::explorer::open_with::change_default_application_for_file(&path, window);
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
        if let Some(handle) = self.default_app_picker {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
            self.default_app_picker = None;
        }
        let Some(path) = single_file_default_app_path(snapshot).map(Path::to_path_buf) else {
            return;
        };

        let before = snapshot.default_app.clone();
        let _ = window;
        let task = cx.spawn(async move |this, cx| {
            let result =
                cx.background_executor()
                    .spawn({
                        let path = path.clone();
                        async move {
                            crate::explorer::open_with::linux_default_app_choices_for_file(&path)
                        }
                    })
                    .await;

            let _ = this.update(cx, |dialog, cx| {
                dialog.default_app_task = None;
                match result {
                    Ok(choices) => {
                        match dialog.open_linux_default_app_picker(path, before, choices, cx) {
                            Ok(()) => {}
                            Err(error) => dialog.default_app_error = Some(error),
                        }
                    }
                    Err(error) => {
                        dialog.default_app_error = Some(format!(
                            "Could not list default app choices for {}: {error}",
                            property_path_display_name(&path)
                        ));
                    }
                }
                cx.notify();
            });
        });
        self.default_app_task = Some(task);
        cx.notify();
    }

    fn refresh_after_default_app_change(
        &mut self,
        path: PathBuf,
        before: Option<PropertyDefaultApp>,
        result: std::io::Result<DefaultAppChangeOutcome>,
        cx: &mut Context<Self>,
    ) {
        if default_app_change_refreshes_file_type_icons(&result) {
            super::app_icons::invalidate_native_file_type_icons_for_path(&path, cx);
        }

        let target = self.target.clone();
        let date_format = self.date_format.clone();
        let task = cx.spawn(async move |this, cx| {
            let snapshot = cx
                .background_executor()
                .spawn(async move {
                    collect_property_snapshot_fast_with_date_format(target, &date_format)
                })
                .await
                .ok();

            let _ = this.update(cx, |dialog, cx| {
                dialog.default_app_task = None;
                if !matches!(result, Ok(DefaultAppChangeOutcome::Cancelled)) {
                    dialog.default_app_error =
                        default_app_change_error(&path, &before, &result, snapshot.as_ref());
                }
                if let Some(snapshot) = snapshot {
                    dialog.set_ready_snapshot(snapshot, cx);
                }
                cx.notify();
            });
        });
        self.default_app_task = Some(task);
        cx.notify();
    }

    #[cfg(target_os = "linux")]
    fn open_linux_default_app_picker(
        &mut self,
        path: PathBuf,
        before: Option<PropertyDefaultApp>,
        choices: crate::explorer::open_with::LinuxDefaultApplicationChoices,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if choices.choices.is_empty() {
            return Err(format!(
                "No applications were found for {}.",
                property_path_display_name(&path)
            ));
        }

        let handle = open_linux_default_app_picker_window(path, before, choices, cx.entity(), cx)?;
        self.default_app_picker = Some(handle);
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn apply_linux_default_app_choice(
        &mut self,
        path: PathBuf,
        before: Option<PropertyDefaultApp>,
        mime_type: String,
        desktop_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.default_app_task.is_some() {
            return;
        }

        self.default_app_error = None;
        let target = self.target.clone();
        let date_format = self.date_format.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    crate::explorer::open_with::linux_change_default_application(
                        &mime_type,
                        &desktop_id,
                    )
                })
                .await;
            let snapshot = cx
                .background_executor()
                .spawn(async move {
                    collect_property_snapshot_fast_with_date_format(target, &date_format)
                })
                .await
                .ok();

            let _ = this.update(cx, |dialog, cx| {
                dialog.default_app_task = None;
                dialog.default_app_error =
                    default_app_change_error(&path, &before, &result, snapshot.as_ref());
                if default_app_change_refreshes_file_type_icons(&result) {
                    super::app_icons::invalidate_native_file_type_icons_for_path(&path, cx);
                }
                if let Some(snapshot) = snapshot {
                    dialog.set_ready_snapshot(snapshot, cx);
                }
                cx.notify();
            });
        });
        self.default_app_task = Some(task);
        cx.notify();
    }

    #[cfg(target_os = "linux")]
    fn clear_linux_default_app_picker(&mut self, cx: &mut Context<Self>) {
        self.default_app_picker = None;
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
            self.image_copy_context_menu = None;
            self.active_tab = tab;
            if tab == PropertyTab::Details {
                self.start_details_task(cx);
            } else if tab == PropertyTab::Code {
                self.start_code_task(cx);
            } else if tab == PropertyTab::Cover {
                self.start_cover_task(cx);
            } else if tab == PropertyTab::Spectrum {
                if let Some(snapshot) = snapshot {
                    if let Some(path) = single_file_audio_path(&snapshot.target, snapshot.item_kind)
                    {
                        crate::debug_options::log_property_marker(format_args!(
                            "spectrum tab selected path={}",
                            path.display()
                        ));
                    }
                }
                self.start_spectrum_task(cx);
            } else if tab == PropertyTab::Frames {
                if let Some(snapshot) = snapshot {
                    if let Some(path) = single_file_video_path(&snapshot.target, snapshot.item_kind)
                    {
                        crate::debug_options::log_property_marker(format_args!(
                            "frames tab selected path={}",
                            path.display()
                        ));
                    }
                }
                self.start_frames_task(cx);
            }
            cx.notify();
        }
    }

    fn select_previous_cover(&mut self, cx: &mut Context<Self>) {
        if self.cover_index > 0 {
            self.cover_index -= 1;
            self.image_copy_context_menu = None;
            cx.notify();
        }
    }

    fn select_next_cover(&mut self, cx: &mut Context<Self>) {
        let cover_count = match &self.cover_state {
            PropertyCoverState::Ready(covers) => covers.len(),
            PropertyCoverState::NotStarted
            | PropertyCoverState::Loading
            | PropertyCoverState::Failed(_) => 0,
        };
        if self.cover_index + 1 < cover_count {
            self.cover_index += 1;
            self.image_copy_context_menu = None;
            cx.notify();
        }
    }

    fn adjust_spectrum_low_db(&mut self, delta: f32, cx: &mut Context<Self>) {
        let mut range = self.spectrum_range;
        range.low_db = (range.low_db + delta).clamp(
            PROPERTIES_SPECTRUM_MIN_DB,
            range.high_db - PROPERTIES_SPECTRUM_MIN_RANGE_DB,
        );
        self.set_spectrum_range(range, cx);
    }

    fn adjust_spectrum_high_db(&mut self, delta: f32, cx: &mut Context<Self>) {
        let mut range = self.spectrum_range;
        range.high_db = (range.high_db + delta).clamp(
            range.low_db + PROPERTIES_SPECTRUM_MIN_RANGE_DB,
            PROPERTIES_SPECTRUM_MAX_DB,
        );
        self.set_spectrum_range(range, cx);
    }

    fn set_spectrum_range(&mut self, range: PropertySpectrumRange, cx: &mut Context<Self>) {
        if self.spectrum_range == range {
            return;
        }
        self.spectrum_range = range;
        if let PropertySpectrumState::Ready(analysis) = &mut self.spectrum_state
            && let Some(image) =
                spectrum_render_image(&analysis.db_values, analysis.width, analysis.height, range)
        {
            analysis.image = image;
        }
        self.clear_spectrum_render_cache();
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

    fn toggle_run_as_admin(&mut self, cx: &mut Context<Self>) {
        let current = self.draft.run_as_admin.unwrap_or(false);
        self.draft.run_as_admin = Some(!current);
        cx.notify();
    }
}

impl Render for PropertiesDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .font(self.font.clone())
            .key_context("ExplorerDialog PropertiesDialog")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(rgb(0xf3f3f3))
            .cursor_default()
            .text_size(px(12.0))
            .text_color(rgb(0x000000))
            .on_action(cx.listener(Self::handle_cancel))
            .on_action(cx.listener(Self::handle_confirm))
            .on_action(cx.listener(Self::handle_properties_open_previous))
            .on_action(cx.listener(Self::handle_properties_open_next))
            .on_action(cx.listener(Self::handle_select_previous_tab))
            .on_action(cx.listener(Self::handle_select_next_tab))
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
            .when_some(
                self.render_image_copy_context_menu_overlay(window, cx),
                |this, menu| this.child(menu),
            )
    }
}

impl Focusable for PropertiesDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(target_os = "linux")]
const DEFAULT_APP_PICKER_WIDTH: f32 = 460.0;
#[cfg(target_os = "linux")]
const DEFAULT_APP_PICKER_HEIGHT: f32 = 420.0;
#[cfg(target_os = "linux")]
const DEFAULT_APP_PICKER_ROW_HEIGHT: f32 = 36.0;
#[cfg(target_os = "linux")]
const DEFAULT_APP_PICKER_SELECTED_BG: u32 = 0xcfe8ff;
#[cfg(target_os = "linux")]
const DEFAULT_APP_PICKER_HOVER_BG: u32 = 0xe5f3ff;

#[cfg(target_os = "linux")]
struct LinuxDefaultAppPickerDialog {
    path: PathBuf,
    before: Option<PropertyDefaultApp>,
    mime_type: String,
    choices: Vec<crate::explorer::open_with::LinuxDefaultApplicationChoice>,
    selected_index: Option<usize>,
    properties: WeakEntity<PropertiesDialog>,
    font: gpui::Font,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
}

#[cfg(target_os = "linux")]
impl LinuxDefaultAppPickerDialog {
    fn new(
        path: PathBuf,
        before: Option<PropertyDefaultApp>,
        choices: crate::explorer::open_with::LinuxDefaultApplicationChoices,
        properties: WeakEntity<PropertiesDialog>,
        focus_handle: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let font = crate::settings::current_app_font(cx);
        let selected_index =
            crate::explorer::open_with::linux_default_app_initial_selection(&choices.choices);
        let dialog = Self {
            path,
            before,
            mime_type: choices.mime_type,
            choices: choices.choices,
            selected_index,
            properties,
            font,
            focus_handle,
            scroll_handle: ScrollHandle::new(),
        };
        cx.observe_global::<SettingsState>(|this, cx| {
            this.font = crate::settings::current_app_font(cx);
            cx.notify();
        })
        .detach();
        dialog
    }

    fn handle_cancel(&mut self, _: &DialogCancel, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel(window, cx);
    }

    fn handle_confirm(&mut self, _: &DialogConfirm, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm(window, cx);
    }

    fn select_choice(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_index = Some(index);
        cx.notify();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.selected_index else {
            return;
        };
        let Some(choice) = self.choices.get(index).cloned() else {
            return;
        };

        let path = self.path.clone();
        let before = self.before.clone();
        let mime_type = self.mime_type.clone();
        let desktop_id = choice.desktop_id;
        let _ = self.properties.update(cx, |properties, cx| {
            properties.apply_linux_default_app_choice(path, before, mime_type, desktop_id, cx);
            properties.clear_linux_default_app_picker(cx);
        });
        window.remove_window();
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.properties.update(cx, |properties, cx| {
            properties.clear_linux_default_app_picker(cx);
        });
        window.remove_window();
    }

    fn release(&mut self, cx: &mut App) {
        let _ = self.properties.update(cx, |properties, cx| {
            properties.clear_linux_default_app_picker(cx);
        });
    }

    fn render_choice(
        &self,
        index: usize,
        choice: &crate::explorer::open_with::LinuxDefaultApplicationChoice,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected_index == Some(index);
        let mut sublabel = if choice.current_default {
            "Current default".to_owned()
        } else if choice.compatible {
            "Recommended for this file type".to_owned()
        } else {
            "Other application".to_owned()
        };
        if choice.current_default && choice.compatible {
            sublabel = "Current default, recommended for this file type".to_owned();
        }

        div()
            .id(("linux-default-app-choice", index))
            .h(px(DEFAULT_APP_PICKER_ROW_HEIGHT))
            .px(px(8.0))
            .flex()
            .flex_col()
            .justify_center()
            .border_b_1()
            .border_color(rgb(PROPERTIES_BORDER))
            .bg(rgb(if selected {
                DEFAULT_APP_PICKER_SELECTED_BG
            } else {
                0xffffff
            }))
            .when(!selected, |this| {
                this.hover(|style| style.bg(rgb(DEFAULT_APP_PICKER_HOVER_BG)))
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.select_choice(index, cx);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .min_w(px(0.0))
                    .truncate()
                    .child(SharedString::from(choice.name.clone())),
            )
            .child(
                div()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(11.0))
                    .text_color(rgb(PROPERTIES_MUTED_TEXT))
                    .child(SharedString::from(sublabel)),
            )
            .into_any_element()
    }
}

#[cfg(target_os = "linux")]
impl Render for LinuxDefaultAppPickerDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected_index.is_some();
        let file_name = property_path_display_name(&self.path);
        let mut list = div().flex().flex_col().bg(rgb(0xffffff));
        for (index, choice) in self.choices.iter().enumerate() {
            list = list.child(self.render_choice(index, choice, cx));
        }

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
                    .gap(px(10.0))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .child(format!("Choose a default app for {file_name}")),
                    )
                    .child(
                        div()
                            .text_color(rgb(PROPERTIES_MUTED_TEXT))
                            .child(format!("File type: {}", self.mime_type)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.0))
                            .id("linux-default-app-picker-list")
                            .border_1()
                            .border_color(rgb(PROPERTIES_BORDER))
                            .overflow_y_scroll()
                            .scrollbar_width(px(0.0))
                            .track_scroll(&self.scroll_handle)
                            .on_scroll_wheel(cx.listener(
                                |_: &mut Self, _: &ScrollWheelEvent, _, cx| {
                                    cx.notify();
                                },
                            ))
                            .child(list),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.0))
                            .child(
                                property_button(
                                    "linux-default-app-ok",
                                    "OK",
                                    selected,
                                    window.scale_factor(),
                                )
                                .when(selected, |this| {
                                    this.on_click(cx.listener(
                                        |this, _: &ClickEvent, window, cx| {
                                            this.confirm(window, cx);
                                            cx.stop_propagation();
                                        },
                                    ))
                                }),
                            )
                            .child(
                                property_button(
                                    "linux-default-app-cancel",
                                    "Cancel",
                                    true,
                                    window.scale_factor(),
                                )
                                .on_click(cx.listener(
                                    |this, _: &ClickEvent, window, cx| {
                                        this.cancel(window, cx);
                                        cx.stop_propagation();
                                    },
                                )),
                            ),
                    ),
            )
    }
}

#[cfg(target_os = "linux")]
impl Focusable for LinuxDefaultAppPickerDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl PropertiesDialog {
    fn render_image_copy_context_menu_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.image_copy_context_menu.as_ref()?;
        let window_size = (
            f32::from(window.bounds().size.width),
            f32::from(window.bounds().size.height),
        );
        let menu_width = PROPERTIES_IMAGE_CONTEXT_MENU_WIDTH;
        let menu_height = property_image_context_menu_height();
        let (left, top) = clamped_context_menu_origin(
            (f32::from(menu.origin.x), f32::from(menu.origin.y)),
            (menu_width, menu_height),
            window_size,
        );
        let payload = menu.payload.clone();

        Some(
            div()
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .size_full()
                .child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(0.0))
                        .size_full()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                if this.close_image_copy_context_menu() {
                                    cx.notify();
                                }
                                cx.stop_propagation();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                if this.close_image_copy_context_menu() {
                                    cx.notify();
                                }
                                cx.stop_propagation();
                            }),
                        ),
                )
                .child(
                    property_image_context_menu_dropdown()
                        .absolute()
                        .left(px(left))
                        .top(px(top))
                        .child(property_image_context_menu_copy_row(payload, cx)),
                )
                .into_any_element(),
        )
    }

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
                PropertyTab::Code => self.render_code(&snapshot, cx),
                PropertyTab::Image => self.render_image(&snapshot, window, cx),
                PropertyTab::Cover => self.render_cover(&snapshot, window, cx),
                PropertyTab::Spectrum => self.render_spectrum(&snapshot, cx),
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
                property_size_value_label(&snapshot.size),
                cx,
            ))
            .child(property_row(
                PROPERTIES_ROW_SIZE_ON_DISK_ID,
                "Size on disk:",
                property_size_value_label(&snapshot.size_on_disk),
                cx,
            ));
        if let Some(contains) = snapshot.contains.as_ref() {
            body = body.child(property_row(
                PROPERTIES_ROW_CONTAINS_ID,
                "Contains:",
                contains_value_label(contains),
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

        body = body.child(separator());
        body = body.child(self.render_attributes_row(snapshot, cx));
        if snapshot_has_run_as_admin_setting(snapshot) {
            body = body.child(self.render_run_as_admin_row(snapshot, cx));
        }
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
            selection_count_value_label(counts)
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
                return render_image_icon(
                    icon,
                    PROPERTIES_ITEM_ICON_SIZE,
                    PROPERTIES_ITEM_ICON_SIZE,
                );
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
                render_image_icon(
                    icon,
                    PROPERTIES_OPEN_WITH_ICON_SIZE,
                    PROPERTIES_OPEN_WITH_ICON_SIZE,
                )
            })
            .unwrap_or_else(|| {
                file_icon_for_path_sized(path, PROPERTIES_OPEN_WITH_ICON_SIZE).into_any_element()
            })
    }

    fn native_icon_for_path(
        &self,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        self.explorer
            .update(cx, |explorer, cx| {
                explorer.native_icon_for_path(path, NativeIconSize::Details, cx)
            })
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
        let loading_details = matches!(self.details_state, PropertyDetailsState::Loading);
        let (groups_empty, detail_children) = {
            let checksum_state = self.checksum_state.clone();
            let show_checksum_rows =
                single_file_path(&snapshot.target, snapshot.item_kind).is_some();
            let groups = self.detail_groups_for_render_cached(snapshot).to_vec();
            let groups_empty = groups.is_empty();
            let mut detail_children = Vec::new();
            let mut detail_row_index = 0;
            for group in &groups {
                detail_children.push(detail_group_header(&group.title));
                for detail in &group.details {
                    detail_children.push(detail_row(
                        detail_row_index,
                        &detail.name,
                        &detail.value,
                        cx,
                    ));
                    detail_row_index += 1;
                }
                if show_checksum_rows && group.kind == PropertyDetailGroupKind::File {
                    push_checksum_detail_rows(
                        &checksum_state,
                        &mut detail_children,
                        &mut detail_row_index,
                        cx,
                    );
                }
            }
            (groups_empty, detail_children)
        };
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

        for child in detail_children {
            body = body.child(child);
        }

        if groups_empty && !loading_details {
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
                this.child(self.render_details_scrollbar(cx))
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
            .when(loading_details, |this| {
                this.child(linear_indeterminate(
                    "properties-details-linear-progress",
                    LinearProgressStyle::explorer_copy_green(),
                ))
            })
            .into_any_element()
    }

    fn render_run_as_admin_row(
        &self,
        snapshot: &PropertySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let value = self
            .draft
            .run_as_admin
            .or(mixed_bool_value(&snapshot.run_as_admin));
        div()
            .id("properties-run-as-admin-row")
            .flex()
            .flex_row()
            .items_center()
            .min_h(px(PROPERTIES_ROW_HEIGHT))
            .cursor_default()
            .child(div().w(px(PROPERTIES_LABEL_WIDTH)).flex_shrink_0())
            .child(check_box(value))
            .child(
                div()
                    .ml(px(6.0))
                    .child("Run this program as an administrator"),
            )
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.toggle_run_as_admin(cx);
                cx.stop_propagation();
            }))
            .into_any_element()
    }

    fn render_code(&self, snapshot: &PropertySnapshot, cx: &mut Context<Self>) -> AnyElement {
        let has_code_tab =
            single_folder_direct_git_repository_root(&snapshot.target, snapshot.item_kind)
                .is_some();
        if !has_code_tab {
            return centered_message("Code information is not available for this item.");
        }

        let loading_code = matches!(
            self.code_state,
            PropertyCodeState::NotStarted | PropertyCodeState::Loading
        );
        let mut body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .w_full()
            .id("properties-code-body")
            .overflow_y_scroll()
            .p(px(PROPERTIES_PANEL_PADDING));

        match &self.code_state {
            PropertyCodeState::NotStarted | PropertyCodeState::Loading => {
                body = body.child(
                    div()
                        .min_w(px(0.0))
                        .w_full()
                        .text_color(rgb(PROPERTIES_MUTED_TEXT))
                        .child("Loading code information..."),
                );
            }
            PropertyCodeState::Failed(error) => {
                body = body.child(
                    div()
                        .min_w(px(0.0))
                        .w_full()
                        .text_color(rgb(PROPERTIES_MUTED_TEXT))
                        .child(SharedString::from(error.clone())),
                );
            }
            PropertyCodeState::Ready(summary) => {
                body = body
                    .child(detail_group_header("Git"))
                    .child(property_row(
                        "properties-code-row-repository",
                        "Repository:",
                        summary.git.repo_root.display().to_string(),
                        cx,
                    ))
                    .child(property_row(
                        "properties-code-row-branch",
                        "Branch:",
                        summary.git.branch.clone(),
                        cx,
                    ))
                    .child(property_row(
                        "properties-code-row-commit-count",
                        "Commit count:",
                        summary.git.commit_count.separate_with_commas(),
                        cx,
                    ))
                    .child(property_row(
                        "properties-code-row-outgoing",
                        "Outgoing commits:",
                        git_divergence_value(summary.git.divergence, GitDivergenceSide::Outgoing),
                        cx,
                    ))
                    .child(property_row(
                        "properties-code-row-incoming",
                        "Incoming commits:",
                        git_divergence_value(summary.git.divergence, GitDivergenceSide::Incoming),
                        cx,
                    ))
                    .child(detail_group_header("Language Makeup"))
                    .child(property_row(
                        "properties-code-row-total-loc",
                        "Total LoC:",
                        summary.codebase.total_code.separate_with_commas(),
                        cx,
                    ));

                if summary.codebase.languages.is_empty() {
                    body = body.child(
                        div()
                            .min_w(px(0.0))
                            .w_full()
                            .pt(px(8.0))
                            .text_color(rgb(PROPERTIES_MUTED_TEXT))
                            .child("No language makeup is available."),
                    );
                } else {
                    body = body
                        .child(properties_code_makeup_bar(&summary.codebase))
                        .child(code_language_header());
                    for (index, language) in summary.codebase.languages.iter().enumerate() {
                        body = body.child(code_language_row(index, language, cx));
                    }
                }
            }
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .w_full()
            .overflow_hidden()
            .child(body)
            .when(loading_code, |this| {
                this.child(linear_indeterminate(
                    "properties-code-linear-progress",
                    LinearProgressStyle::explorer_copy_green(),
                ))
            })
            .into_any_element()
    }

    fn render_image(
        &mut self,
        snapshot: &PropertySnapshot,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if single_file_image_path(&snapshot.target, snapshot.item_kind).is_none() {
            return centered_message("Image preview is not available for this item.");
        }

        let body = div()
            .id("properties-image-body")
            .flex()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .w_full()
            .overflow_hidden();

        if let Some(viewer) = self.ensure_image_viewer(snapshot, window, cx) {
            body.child(viewer).into_any_element()
        } else {
            body.items_center()
                .justify_center()
                .p(px(PROPERTIES_PANEL_PADDING))
                .text_color(rgb(PROPERTIES_MUTED_TEXT))
                .child("Image preview is not available for this item.")
                .into_any_element()
        }
    }

    fn render_cover(
        &mut self,
        snapshot: &PropertySnapshot,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if single_file_audio_path(&snapshot.target, snapshot.item_kind).is_none() {
            return centered_message("Audio covers are not available for this item.");
        }

        if let PropertyCoverState::Ready(covers) = &self.cover_state
            && !covers.is_empty()
            && self.cover_index >= covers.len()
        {
            self.cover_index = covers.len() - 1;
        }

        let body = div()
            .id("properties-cover-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .w_full()
            .overflow_hidden();

        match &self.cover_state {
            PropertyCoverState::NotStarted | PropertyCoverState::Loading => body
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_1()
                        .min_h(px(0.0))
                        .min_w(px(0.0))
                        .w_full()
                        .p(px(PROPERTIES_PANEL_PADDING))
                        .text_color(rgb(PROPERTIES_MUTED_TEXT))
                        .child("Loading cover..."),
                )
                .child(linear_indeterminate(
                    "properties-cover-linear-progress",
                    LinearProgressStyle::explorer_copy_green(),
                ))
                .into_any_element(),
            PropertyCoverState::Failed(error) => body
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_1()
                        .min_h(px(0.0))
                        .min_w(px(0.0))
                        .w_full()
                        .p(px(PROPERTIES_PANEL_PADDING))
                        .text_color(rgb(PROPERTIES_MUTED_TEXT))
                        .child(SharedString::from(error.clone())),
                )
                .into_any_element(),
            PropertyCoverState::Ready(covers) if covers.is_empty() => body
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_1()
                        .min_h(px(0.0))
                        .min_w(px(0.0))
                        .w_full()
                        .p(px(PROPERTIES_PANEL_PADDING))
                        .text_color(rgb(PROPERTIES_MUTED_TEXT))
                        .child("No embedded covers are available."),
                )
                .into_any_element(),
            PropertyCoverState::Ready(covers) => {
                let cover_count = covers.len();
                let index = self.cover_index.min(cover_count - 1);
                let cover = covers[index].clone();
                let previous_enabled = index > 0;
                let next_enabled = index + 1 < cover_count;
                let label = if cover_count > 1 {
                    format!("{} ({}/{})", cover.label, index + 1, cover_count)
                } else {
                    cover.label.clone()
                };

                let previous_button = cover_navigation_button(
                    "properties-cover-previous",
                    NavIcon::Back,
                    "Previous cover",
                    previous_enabled,
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.select_previous_cover(cx);
                        cx.stop_propagation();
                    }),
                );
                let next_button = cover_navigation_button(
                    "properties-cover-next",
                    NavIcon::Forward,
                    "Next cover",
                    next_enabled,
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.select_next_cover(cx);
                        cx.stop_propagation();
                    }),
                );
                let max_size = property_cover_preview_max_size(window);

                body.child(
                    div()
                        .id("properties-cover-preview")
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_1()
                        .min_h(px(0.0))
                        .min_w(px(0.0))
                        .w_full()
                        .p(px(PROPERTIES_PANEL_PADDING))
                        .overflow_hidden()
                        .child(property_image_preview(&cover.preview, max_size, cx)),
                )
                .child(
                    div()
                        .id("properties-cover-navigation")
                        .h(px(PROPERTIES_COVER_NAVIGATION_HEIGHT))
                        .w_full()
                        .flex_shrink_0()
                        .px(px(PROPERTIES_PANEL_PADDING))
                        .border_t_1()
                        .border_color(rgb(PROPERTIES_BORDER))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.0))
                        .child(previous_button)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .text_center()
                                .text_color(rgb(PROPERTIES_MUTED_TEXT))
                                .child(SharedString::from(label)),
                        )
                        .child(next_button),
                )
                .into_any_element()
            }
        }
    }

    fn render_spectrum(
        &mut self,
        snapshot: &PropertySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if single_file_audio_path(&snapshot.target, snapshot.item_kind).is_none() {
            return centered_message("Audio spectrum is not available for this item.");
        }

        let body = div()
            .id("properties-spectrum-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .w_full()
            .overflow_hidden()
            .bg(rgb(PROPERTIES_SPECTRUM_PANEL_BG));

        match &self.spectrum_state {
            PropertySpectrumState::NotStarted | PropertySpectrumState::Loading => body
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_1()
                        .min_h(px(0.0))
                        .min_w(px(0.0))
                        .w_full()
                        .p(px(PROPERTIES_PANEL_PADDING))
                        .text_color(rgb(PROPERTIES_SPECTRUM_AXIS_TEXT))
                        .child("Analysing spectrum..."),
                )
                .child(linear_indeterminate(
                    "properties-spectrum-linear-progress",
                    LinearProgressStyle::explorer_copy_green(),
                ))
                .into_any_element(),
            PropertySpectrumState::Failed(error) => body
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_1()
                        .min_h(px(0.0))
                        .min_w(px(0.0))
                        .w_full()
                        .p(px(PROPERTIES_PANEL_PADDING))
                        .text_color(rgb(PROPERTIES_SPECTRUM_AXIS_TEXT))
                        .child(SharedString::from(error.clone())),
                )
                .into_any_element(),
            PropertySpectrumState::Ready(analysis) => {
                let analysis = Self::render_spectrum_analysis(
                    analysis,
                    self.spectrum_range,
                    self.spectrum_generation,
                    self.spectrum_render_size,
                    &mut self.spectrum_render_cache,
                    cx,
                );
                body.child(analysis)
                    .child(self.render_spectrum_controls(cx))
                    .into_any_element()
            }
        }
    }

    fn render_spectrum_analysis(
        analysis: &PropertySpectrumAnalysis,
        range: PropertySpectrumRange,
        generation: u64,
        render_size: Option<PropertySpectrumRenderSize>,
        render_cache: &mut Option<PropertySpectrumRenderCache>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let frequency_labels = spectrum_frequency_ruler_labels(analysis.metadata.sample_rate);
        let time_labels = spectrum_time_ruler_labels(analysis.metadata.duration_seconds);
        let db_labels = spectrum_density_ruler_labels(range);
        let legend_image = spectrum_legend_render_image(range).unwrap_or_else(empty_render_image);
        let spectrum_image = spectrum_render_image_for_target(
            analysis,
            generation,
            range,
            render_size,
            render_cache,
        );

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .w_full()
            .p(px(2.0))
            .gap(px(3.0))
            .child(
                div()
                    .h(px(PROPERTIES_SPECTRUM_HEADER_HEIGHT))
                    .min_w(px(0.0))
                    .w_full()
                    .text_center()
                    .truncate()
                    .text_size(px(11.0))
                    .text_color(rgb(PROPERTIES_SPECTRUM_AXIS_TEXT))
                    .child(SharedString::from(analysis.metadata.header.clone())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .w_full()
                    .child(
                        div()
                            .debug_selector(|| "properties-spectrum-plot-row".to_owned())
                            .flex()
                            .flex_row()
                            .flex_1()
                            .min_h(px(0.0))
                            .min_w(px(0.0))
                            .w_full()
                            .gap(px(PROPERTIES_SPECTRUM_AXIS_GAP))
                            .child(spectrum_vertical_ruler(
                                "properties-spectrum-frequency-ruler",
                                frequency_labels,
                                PROPERTIES_SPECTRUM_FREQUENCY_RULER_WIDTH,
                                gpui::TextAlign::Right,
                            ))
                            .child(
                                div()
                                    .debug_selector(|| "properties-spectrum-image".to_owned())
                                    .flex_1()
                                    .min_h(px(0.0))
                                    .min_w(px(0.0))
                                    .w_full()
                                    .border_1()
                                    .border_color(rgb(PROPERTIES_SPECTRUM_BORDER))
                                    .overflow_hidden()
                                    .relative()
                                    .child(
                                        gpui::img(spectrum_image)
                                            .size_full()
                                            .object_fit(ObjectFit::Fill),
                                    )
                                    .child(Self::render_spectrum_image_bounds_observer(cx)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .h_full()
                                    .w(px(PROPERTIES_SPECTRUM_DB_RULER_WIDTH))
                                    .min_w(px(PROPERTIES_SPECTRUM_DB_RULER_WIDTH))
                                    .max_w(px(PROPERTIES_SPECTRUM_DB_RULER_WIDTH))
                                    .flex_shrink_0()
                                    .gap(px(PROPERTIES_SPECTRUM_AXIS_GAP))
                                    .child(
                                        div()
                                            .debug_selector(|| {
                                                "properties-spectrum-db-legend".to_owned()
                                            })
                                            .w(px(PROPERTIES_SPECTRUM_DB_LEGEND_WIDTH))
                                            .min_w(px(PROPERTIES_SPECTRUM_DB_LEGEND_WIDTH))
                                            .max_w(px(PROPERTIES_SPECTRUM_DB_LEGEND_WIDTH))
                                            .h_full()
                                            .flex_shrink_0()
                                            .overflow_hidden()
                                            .child(
                                                gpui::img(legend_image)
                                                    .size_full()
                                                    .object_fit(ObjectFit::Fill),
                                            ),
                                    )
                                    .child(spectrum_vertical_ruler(
                                        "properties-spectrum-db-ruler",
                                        db_labels,
                                        PROPERTIES_SPECTRUM_DB_LABEL_WIDTH,
                                        gpui::TextAlign::Left,
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .h(px(PROPERTIES_SPECTRUM_TIME_RULER_HEIGHT))
                            .min_w(px(0.0))
                            .w_full()
                            .gap(px(PROPERTIES_SPECTRUM_AXIS_GAP))
                            .child(
                                div()
                                    .w(px(PROPERTIES_SPECTRUM_FREQUENCY_RULER_WIDTH))
                                    .min_w(px(PROPERTIES_SPECTRUM_FREQUENCY_RULER_WIDTH))
                                    .max_w(px(PROPERTIES_SPECTRUM_FREQUENCY_RULER_WIDTH))
                                    .flex_shrink_0(),
                            )
                            .child(spectrum_horizontal_ruler(time_labels))
                            .child(
                                div()
                                    .w(px(PROPERTIES_SPECTRUM_DB_RULER_WIDTH))
                                    .min_w(px(PROPERTIES_SPECTRUM_DB_RULER_WIDTH))
                                    .max_w(px(PROPERTIES_SPECTRUM_DB_RULER_WIDTH))
                                    .flex_shrink_0(),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_spectrum_image_bounds_observer(cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        div()
            .debug_selector(|| "properties-spectrum-render-target".to_owned())
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .size_full()
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let size =
                            PropertySpectrumRenderSize::from_bounds(bounds, window.scale_factor());
                        let _ = entity.update(cx, |this, cx| {
                            if this.spectrum_render_size != size {
                                this.spectrum_render_size = size;
                                this.clear_spectrum_render_cache();
                                this.schedule_spectrum_resize_refinement(cx);
                                cx.notify();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .size_full(),
            )
            .into_any_element()
    }

    fn render_spectrum_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .h(px(PROPERTIES_SPECTRUM_CONTROL_HEIGHT))
            .w_full()
            .flex_shrink_0()
            .border_t_1()
            .border_color(rgb(PROPERTIES_SPECTRUM_CONTROL_BORDER))
            .px(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(spectrum_range_control_group(
                "Low",
                self.spectrum_range.low_db,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.adjust_spectrum_low_db(-PROPERTIES_SPECTRUM_RANGE_STEP_DB, cx);
                    cx.stop_propagation();
                }),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.adjust_spectrum_low_db(PROPERTIES_SPECTRUM_RANGE_STEP_DB, cx);
                    cx.stop_propagation();
                }),
            ))
            .child(spectrum_range_control_group(
                "High",
                self.spectrum_range.high_db,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.adjust_spectrum_high_db(-PROPERTIES_SPECTRUM_RANGE_STEP_DB, cx);
                    cx.stop_propagation();
                }),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.adjust_spectrum_high_db(PROPERTIES_SPECTRUM_RANGE_STEP_DB, cx);
                    cx.stop_propagation();
                }),
            ))
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
                body = body.child(frame_thumbnail_list(frames, cx));
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
                    body = body.child(frame_thumbnail_list(frames, cx));
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
        self.property_scrollbar_metrics(PropertyScrollbarTarget::Details)
    }

    fn set_details_scroll_top(&self, scroll_top: f32) {
        self.set_property_scroll_top(PropertyScrollbarTarget::Details, scroll_top);
    }

    fn frames_scrollbar_metrics(&self) -> Option<ScrollbarMetrics> {
        self.property_scrollbar_metrics(PropertyScrollbarTarget::Frames)
    }

    fn property_scrollbar_handle(&self, target: PropertyScrollbarTarget) -> &ScrollHandle {
        match target {
            PropertyScrollbarTarget::Details => &self.details_scroll_handle,
            PropertyScrollbarTarget::Frames => &self.frames_scroll_handle,
        }
    }

    fn property_scrollbar_metrics(
        &self,
        target: PropertyScrollbarTarget,
    ) -> Option<ScrollbarMetrics> {
        let handle = self.property_scrollbar_handle(target);
        let viewport_height = f32::from(handle.bounds().size.height);
        let scroll_max = f32::from(handle.max_offset().height);
        let scroll_top = -f32::from(handle.offset().y);
        property_scrollbar_metrics_for_dimensions(viewport_height, scroll_max, scroll_top)
    }

    fn set_property_scroll_top(&self, target: PropertyScrollbarTarget, scroll_top: f32) {
        let scroll_top = self
            .property_scrollbar_metrics(target)
            .map_or(0.0, |metrics| metrics.clamp_scroll_top(scroll_top));
        let handle = self.property_scrollbar_handle(target);
        let offset = handle.offset();
        handle.set_offset(point(offset.x, px(-scroll_top)));
    }

    fn property_scrollbar_hovered(&self, target: PropertyScrollbarTarget) -> bool {
        match target {
            PropertyScrollbarTarget::Details => self.details_scrollbar_hovered,
            PropertyScrollbarTarget::Frames => self.frames_scrollbar_hovered,
        }
    }

    fn set_property_scrollbar_hovered(&mut self, target: PropertyScrollbarTarget, hovered: bool) {
        match target {
            PropertyScrollbarTarget::Details => self.details_scrollbar_hovered = hovered,
            PropertyScrollbarTarget::Frames => self.frames_scrollbar_hovered = hovered,
        }
    }

    fn property_scrollbar_drag(&self, target: PropertyScrollbarTarget) -> Option<ScrollbarDrag> {
        match target {
            PropertyScrollbarTarget::Details => self.details_scrollbar_drag,
            PropertyScrollbarTarget::Frames => self.frames_scrollbar_drag,
        }
    }

    fn set_property_scrollbar_drag(
        &mut self,
        target: PropertyScrollbarTarget,
        drag: Option<ScrollbarDrag>,
    ) {
        match target {
            PropertyScrollbarTarget::Details => self.details_scrollbar_drag = drag,
            PropertyScrollbarTarget::Frames => self.frames_scrollbar_drag = drag,
        }
    }

    fn clear_property_scrollbar_drag(
        &mut self,
        target: PropertyScrollbarTarget,
    ) -> Option<ScrollbarDrag> {
        match target {
            PropertyScrollbarTarget::Details => self.details_scrollbar_drag.take(),
            PropertyScrollbarTarget::Frames => self.frames_scrollbar_drag.take(),
        }
    }

    fn handle_property_scrollbar_mouse_down(
        &mut self,
        target: PropertyScrollbarTarget,
        local_y: f32,
        metrics: ScrollbarMetrics,
    ) {
        if local_y < SCROLLBAR_ARROW_HEIGHT {
            self.set_property_scroll_top(target, metrics.scroll_by(-PROPERTIES_ROW_HEIGHT));
        } else if local_y > metrics.viewport_height - SCROLLBAR_ARROW_HEIGHT {
            self.set_property_scroll_top(target, metrics.scroll_by(PROPERTIES_ROW_HEIGHT));
        } else if local_y >= metrics.thumb_top && local_y <= metrics.thumb_bottom() {
            self.set_property_scrollbar_drag(
                target,
                Some(ScrollbarDrag {
                    pointer_offset_from_thumb_top: local_y - metrics.thumb_top,
                }),
            );
        } else if local_y < metrics.thumb_top {
            self.set_property_scroll_top(target, metrics.scroll_by(-metrics.viewport_height));
        } else {
            self.set_property_scroll_top(target, metrics.scroll_by(metrics.viewport_height));
        }
    }

    fn handle_property_scrollbar_drag(
        &mut self,
        target: PropertyScrollbarTarget,
        local_y: f32,
        metrics: ScrollbarMetrics,
    ) {
        let Some(drag) = self.property_scrollbar_drag(target) else {
            return;
        };

        let thumb_top = local_y - drag.pointer_offset_from_thumb_top;
        self.set_property_scroll_top(target, metrics.scroll_top_for_thumb_top(thumb_top));
    }

    fn render_details_scrollbar(&self, cx: &mut Context<Self>) -> AnyElement {
        self.render_property_scrollbar(PropertyScrollbarTarget::Details, cx)
    }

    fn render_frames_scrollbar(&self, cx: &mut Context<Self>) -> AnyElement {
        self.render_property_scrollbar(PropertyScrollbarTarget::Frames, cx)
    }

    fn render_property_scrollbar(
        &self,
        target: PropertyScrollbarTarget,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(metrics) = self.property_scrollbar_metrics(target) else {
            return div().into_any_element();
        };

        let hovered_or_dragged = self.property_scrollbar_hovered(target)
            || self.property_scrollbar_drag(target).is_some();
        let thumb_width = if hovered_or_dragged {
            SCROLLBAR_THUMB_HOVER_WIDTH
        } else {
            SCROLLBAR_THUMB_WIDTH
        };
        let thumb_right = (SCROLLBAR_GUTTER_WIDTH - thumb_width) / 2.0;
        let thumb_color = if self.property_scrollbar_drag(target).is_some() {
            SCROLLBAR_THUMB_ACTIVE_BG
        } else if hovered_or_dragged {
            SCROLLBAR_THUMB_HOVER_BG
        } else {
            SCROLLBAR_THUMB_BG
        };
        let bottom_arrow_top = (metrics.viewport_height - SCROLLBAR_ARROW_HEIGHT).max(0.0);

        div()
            .id(target.scrollbar_id())
            .relative()
            .w(px(SCROLLBAR_GUTTER_WIDTH))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(SCROLLBAR_TRACK_BG))
            .cursor_default()
            .block_mouse_except_scroll()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.set_property_scrollbar_hovered(target, *hovered);
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
            .child(self.render_property_scrollbar_hit_layer(target, cx))
            .into_any_element()
    }

    fn render_property_scrollbar_hit_layer(
        &self,
        target: PropertyScrollbarTarget,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
                            if let Some(metrics) = this.property_scrollbar_metrics(target) {
                                this.handle_property_scrollbar_mouse_down(target, local_y, metrics);
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
                            if this.property_scrollbar_drag(target).is_none() {
                                return;
                            }

                            if let Some(metrics) = this.property_scrollbar_metrics(target) {
                                this.handle_property_scrollbar_drag(target, local_y, metrics);
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
                        if this.clear_property_scrollbar_drag(target).is_some() {
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
            .pt(px(PROPERTIES_BUTTON_ROW_TOP_PADDING))
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
            run_as_admin: None,
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
            run_as_admin: mixed_bool_value(&snapshot.run_as_admin),
        }
    }
}

fn open_properties_window(
    target: PropertyTarget,
    explorer: gpui::Entity<ExplorerView>,
    date_format: String,
    parent_window: &Window,
    cx: &mut Context<ExplorerView>,
) -> Result<AnyWindowHandle, String> {
    let title = properties_window_title(&target.paths);
    let options = properties_window_options(title, parent_window, cx);
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

#[cfg(target_os = "linux")]
fn open_linux_default_app_picker_window(
    path: PathBuf,
    before: Option<PropertyDefaultApp>,
    choices: crate::explorer::open_with::LinuxDefaultApplicationChoices,
    properties: gpui::Entity<PropertiesDialog>,
    cx: &mut Context<PropertiesDialog>,
) -> Result<AnyWindowHandle, String> {
    let title = format!("Open With - {}", property_path_display_name(&path));
    let options = linux_default_app_picker_window_options(title, cx);
    let handle = cx
        .open_window(options, |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            cx.new(|cx| {
                cx.on_release(|dialog: &mut LinuxDefaultAppPickerDialog, cx| dialog.release(cx))
                    .detach();
                LinuxDefaultAppPickerDialog::new(
                    path,
                    before,
                    choices,
                    properties.downgrade(),
                    focus_handle,
                    cx,
                )
            })
        })
        .map_err(|error| error.to_string())?;

    Ok(handle.into())
}

#[cfg(target_os = "linux")]
fn linux_default_app_picker_window_options(title: String, cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::centered(
            size(px(DEFAULT_APP_PICKER_WIDTH), px(DEFAULT_APP_PICKER_HEIGHT)),
            cx,
        )),
        window_min_size: Some(size(
            px(DEFAULT_APP_PICKER_WIDTH),
            px(DEFAULT_APP_PICKER_HEIGHT),
        )),
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

fn properties_window_bounds(parent_bounds: Bounds<Pixels>) -> WindowBounds {
    WindowBounds::Windowed(Bounds::centered_at(
        parent_bounds.center(),
        size(px(PROPERTIES_WIDTH), px(PROPERTIES_HEIGHT)),
    ))
}

fn properties_window_options(title: String, parent_window: &Window, cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(properties_window_bounds(parent_window.bounds())),
        window_min_size: Some(size(px(PROPERTIES_WIDTH), px(PROPERTIES_HEIGHT))),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from(title)),
            ..Default::default()
        }),
        kind: WindowKind::Floating,
        is_movable: true,
        is_resizable: true,
        is_minimizable: false,
        display_id: parent_window.display(cx).map(|display| display.id()),
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
    let cancel = AtomicBool::new(false);
    collect_property_snapshot_full_with_date_format(
        target,
        crate::settings::DEFAULT_DATE_FORMAT,
        &cancel,
    )
}

fn collect_property_snapshot_fast_with_date_format(
    target: PropertyTarget,
    date_format: &str,
) -> Result<PropertySnapshot, String> {
    collect_property_snapshot_with_date_format(target, date_format, PropertyTreeMode::Fast)
}

fn collect_property_snapshot_full_with_date_format(
    target: PropertyTarget,
    date_format: &str,
    cancel: &AtomicBool,
) -> Result<PropertySnapshot, String> {
    collect_property_snapshot_with_date_format(target, date_format, PropertyTreeMode::Full(cancel))
}

#[derive(Clone, Copy)]
enum PropertyTreeMode<'a> {
    Fast,
    Full(&'a AtomicBool),
}

fn collect_property_snapshot_with_date_format(
    target: PropertyTarget,
    date_format: &str,
    tree_mode: PropertyTreeMode<'_>,
) -> Result<PropertySnapshot, String> {
    if target.paths.is_empty() {
        return Err("No items selected.".to_owned());
    }

    let mut items = Vec::new();
    for path in &target.paths {
        items.push(collect_property_item(path, date_format, tree_mode)?);
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
    let run_as_admin = mixed_from_iter(items.iter().map(|item| item.run_as_admin));
    let size = property_value_sum(items.iter().map(|item| item.size.as_ref()));
    let size_on_disk = property_value_sum(items.iter().map(|item| item.size_on_disk.as_ref()));
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
        run_as_admin,
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
    size: Option<PropertyValue<u64>>,
    size_on_disk: Option<PropertyValue<u64>>,
    contains: Option<PropertyValue<PropertyContains>>,
    selection_counts: Option<PropertyValue<PropertyContains>>,
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
    accessed: Option<SystemTime>,
    readonly: Option<bool>,
    hidden: Option<bool>,
    owner: Option<String>,
    group: Option<String>,
    unix_mode: Option<u32>,
    permission_summary: Option<String>,
    run_as_admin: Option<bool>,
    shortcut: Option<ShortcutDetails>,
    details: Vec<PropertyDetailGroup>,
}

fn collect_property_item(
    path: &Path,
    date_format: &str,
    tree_mode: PropertyTreeMode<'_>,
) -> Result<PropertyItem, String> {
    let link_metadata = fs::symlink_metadata(path).ok();
    let metadata = property_target_metadata(path, link_metadata.as_ref());
    let is_dir = metadata.as_ref().is_some_and(|metadata| metadata.is_dir());
    let exists = metadata.is_some();
    let is_directory_link = link_metadata
        .as_ref()
        .is_some_and(metadata_is_directory_link);
    let entry = link_metadata.as_ref().and_then(|metadata| {
        crate::explorer::FileEntry::from_path_with_link_metadata(
            path.to_path_buf(),
            metadata.clone(),
        )
    });

    let recursive_summary_is_pending =
        is_dir && !is_directory_link && matches!(tree_mode, PropertyTreeMode::Fast);
    let tree_summary = if recursive_summary_is_pending {
        None
    } else if metadata.is_some() {
        collect_property_tree_summary(path, tree_mode)?
    } else {
        None
    };
    let size = if recursive_summary_is_pending {
        Some(PropertyValue::Loading)
    } else {
        tree_summary
            .map(|summary| PropertyValue::ready(summary.size))
            .or_else(|| {
                metadata
                    .as_ref()
                    .map(|metadata| PropertyValue::ready(metadata.len()))
            })
    };
    let size_on_disk = if recursive_summary_is_pending {
        Some(PropertyValue::Loading)
    } else {
        tree_summary
            .map(|summary| PropertyValue::ready(summary.size_on_disk))
            .or_else(|| {
                metadata.as_ref().map(|metadata| {
                    PropertyValue::ready(
                        size_on_disk(path, metadata).unwrap_or_else(|| metadata.len()),
                    )
                })
            })
    };
    let contains = if recursive_summary_is_pending {
        Some(PropertyValue::Loading)
    } else if is_dir {
        tree_summary.map(|summary| {
            PropertyValue::ready(PropertyContains {
                files: summary.files,
                folders: summary.folders.saturating_sub(1),
            })
        })
    } else {
        None
    };
    let selection_counts = if recursive_summary_is_pending {
        Some(PropertyValue::Loading)
    } else {
        tree_summary.map(|summary| {
            PropertyValue::ready(PropertyContains {
                files: summary.files,
                folders: summary.folders,
            })
        })
    };
    let readonly = metadata
        .as_ref()
        .map(|metadata| metadata.permissions().readonly());
    let hidden = Some(path_is_hidden(path, metadata.as_ref()));
    let run_as_admin = property_run_as_admin_value(path, entry.as_ref());
    let shortcut = shortcut_details(path, entry.as_ref());
    let details = metadata_details(
        path,
        entry.as_ref(),
        metadata.as_ref(),
        size.as_ref().and_then(PropertyValue::as_ready).copied(),
        size_on_disk
            .as_ref()
            .and_then(PropertyValue::as_ready)
            .copied(),
        date_format,
    );

    Ok(PropertyItem {
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
        run_as_admin,
        shortcut,
        details,
    })
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

fn property_value_sum<'a>(
    values: impl IntoIterator<Item = Option<&'a PropertyValue<u64>>>,
) -> PropertyValue<u64> {
    let mut sum = 0u64;
    let mut loading = false;
    for value in values {
        match value {
            Some(PropertyValue::Ready(value)) => sum = sum.saturating_add(*value),
            Some(PropertyValue::Loading) => loading = true,
            None => {}
        }
    }

    if loading {
        PropertyValue::Loading
    } else {
        PropertyValue::Ready(sum)
    }
}

fn selection_counts_summary(items: &[PropertyItem]) -> Option<PropertyValue<PropertyContains>> {
    let mut files = 0;
    let mut folders = 0;
    let mut has_counts = false;
    for item in items {
        if let Some(counts) = &item.selection_counts {
            match counts {
                PropertyValue::Loading => return Some(PropertyValue::Loading),
                PropertyValue::Ready(counts) => {
                    has_counts = true;
                    files += counts.files;
                    folders += counts.folders;
                }
            }
        }
    }
    has_counts.then_some(PropertyValue::Ready(PropertyContains { files, folders }))
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

fn property_target_metadata(
    path: &Path,
    link_metadata: Option<&fs::Metadata>,
) -> Option<fs::Metadata> {
    let link_metadata = link_metadata?;
    if link_metadata.file_type().is_symlink() || metadata_is_directory_link(link_metadata) {
        fs::metadata(path)
            .ok()
            .or_else(|| Some(link_metadata.clone()))
    } else {
        Some(link_metadata.clone())
    }
}

fn collect_property_tree_summary(
    path: &Path,
    tree_mode: PropertyTreeMode<'_>,
) -> Result<Option<PropertyTreeSummary>, String> {
    let Some(link_metadata) = fs::symlink_metadata(path).ok() else {
        return Ok(None);
    };
    let metadata = property_target_metadata(path, Some(&link_metadata))
        .unwrap_or_else(|| link_metadata.clone());
    let mut summary = property_tree_entry_summary_from_metadata(path, &link_metadata, &metadata);
    if !metadata.is_dir() || metadata_is_directory_link(&link_metadata) {
        return Ok(Some(summary));
    }

    let PropertyTreeMode::Full(cancel) = tree_mode else {
        return Ok(Some(summary));
    };
    if cancel.load(Ordering::Relaxed) {
        return Err("Cancelled".to_owned());
    }

    for (index, entry_result) in WalkDirGeneric::<((), Option<PropertyTreeSummary>)>::new(path)
        .sort(false)
        .skip_hidden(false)
        .follow_links(false)
        .min_depth(1)
        .process_read_dir(|_, _, _, children| {
            for child in children.iter_mut() {
                let Ok(entry) = child else {
                    continue;
                };
                let child_path = entry.path();
                let Ok(link_metadata) = entry.metadata() else {
                    entry.read_children_path = None;
                    continue;
                };
                let metadata = property_target_metadata(&child_path, Some(&link_metadata))
                    .unwrap_or_else(|| link_metadata.clone());
                if metadata.is_dir() && metadata_is_directory_link(&link_metadata) {
                    entry.read_children_path = None;
                }
                entry.client_state = Some(property_tree_entry_summary_from_metadata(
                    &child_path,
                    &link_metadata,
                    &metadata,
                ));
            }
        })
        .into_iter()
        .enumerate()
    {
        if index % PROPERTY_TREE_CANCELLATION_CHECK_INTERVAL == 0 && cancel.load(Ordering::Relaxed)
        {
            return Err("Cancelled".to_owned());
        }
        let Ok(entry) = entry_result else {
            continue;
        };
        if let Some(entry_summary) = entry.client_state {
            summary.add(entry_summary);
        }
    }

    if cancel.load(Ordering::Relaxed) {
        return Err("Cancelled".to_owned());
    }
    Ok(Some(summary))
}

fn property_tree_entry_summary_from_metadata(
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
    path_may_have_exif(path) || ffprobe_metadata_kind_for_path(path).is_some()
}

fn property_media_local_path(path: PathBuf) -> Result<PathBuf, String> {
    Ok(path)
}

const VIDEO_FRAME_COUNT: usize = 20;
const FFMPEG_PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
#[cfg(test)]
const VIDEO_FRAME_FALLBACK_ASPECT_RATIO: f32 = 16.0 / 9.0;
const VIDEO_FRAME_PUBLISH_INTERVAL_MS: u64 = 16;

fn single_file_image_path(target: &PropertyTarget, item_kind: PropertyItemKind) -> Option<&Path> {
    let path = single_file_path(target, item_kind)?;
    path_may_have_image_preview(path).then_some(path)
}

fn single_file_audio_path(target: &PropertyTarget, item_kind: PropertyItemKind) -> Option<&Path> {
    let path = single_file_path(target, item_kind)?;
    path_may_have_audio_metadata(path).then_some(path)
}

fn single_file_video_path(target: &PropertyTarget, item_kind: PropertyItemKind) -> Option<&Path> {
    let path = single_file_path(target, item_kind)?;
    path_may_have_video_metadata(path).then_some(path)
}

fn single_folder_direct_git_repository_root(
    target: &PropertyTarget,
    item_kind: PropertyItemKind,
) -> Option<&Path> {
    if !matches!(item_kind, PropertyItemKind::SingleFolder) {
        return None;
    }
    let path = target.paths.first()?.as_path();
    direct_git_repository_root(path).map(|_| path)
}

fn single_file_path(target: &PropertyTarget, item_kind: PropertyItemKind) -> Option<&Path> {
    if !matches!(item_kind, PropertyItemKind::SingleFile) {
        return None;
    }
    target.paths.first().map(PathBuf::as_path)
}

#[derive(Clone, Debug)]
struct PropertyNavigationSibling {
    path: PathBuf,
    name: String,
    sorts_as_directory: bool,
}

fn single_property_navigation_path(
    target: &PropertyTarget,
    item_kind: PropertyItemKind,
) -> Option<&Path> {
    if !matches!(
        item_kind,
        PropertyItemKind::SingleFile
            | PropertyItemKind::SingleFolder
            | PropertyItemKind::SingleShortcut
    ) || target.paths.len() != 1
    {
        return None;
    }
    target.paths.first().map(PathBuf::as_path)
}

fn adjacent_property_path(
    target: &PropertyTarget,
    item_kind: PropertyItemKind,
    direction: PropertyNavigationDirection,
) -> Option<PathBuf> {
    let current_path = single_property_navigation_path(target, item_kind)?;
    adjacent_property_sibling_path(current_path, direction)
}

fn adjacent_property_sibling_path(
    current_path: &Path,
    direction: PropertyNavigationDirection,
) -> Option<PathBuf> {
    let current_name = current_path.file_name()?;
    let parent = current_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut siblings = fs::read_dir(parent)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| property_navigation_sibling(entry.path()))
        .collect::<Vec<_>>();

    if siblings.len() <= 1 {
        return None;
    }

    siblings.sort_by(compare_property_navigation_siblings);
    let current_index = siblings
        .iter()
        .position(|candidate| candidate.path.file_name() == Some(current_name))?;
    let target_index = match direction {
        PropertyNavigationDirection::Previous => {
            if current_index == 0 {
                siblings.len() - 1
            } else {
                current_index - 1
            }
        }
        PropertyNavigationDirection::Next => (current_index + 1) % siblings.len(),
    };

    Some(siblings[target_index].path.clone())
}

fn property_navigation_sibling(path: PathBuf) -> Option<PropertyNavigationSibling> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let link_metadata = fs::symlink_metadata(&path).ok()?;
    let entry = FileEntry::from_path_with_link_metadata(path.clone(), link_metadata.clone());
    let metadata = property_target_metadata(&path, Some(&link_metadata));
    let sorts_as_directory = entry.as_ref().is_some_and(FileEntry::sorts_as_directory)
        || metadata.as_ref().is_some_and(|metadata| {
            metadata.is_dir() && !metadata_is_directory_link(&link_metadata)
        });

    Some(PropertyNavigationSibling {
        path,
        name,
        sorts_as_directory,
    })
}

fn compare_property_navigation_siblings(
    left: &PropertyNavigationSibling,
    right: &PropertyNavigationSibling,
) -> CmpOrdering {
    match (left.sorts_as_directory, right.sorts_as_directory) {
        (true, false) => CmpOrdering::Less,
        (false, true) => CmpOrdering::Greater,
        _ => crate::explorer::compare_file_names(&left.name, &right.name)
            .then_with(|| left.path.cmp(&right.path)),
    }
}

fn collect_property_code_summary(repo_root: &Path) -> Result<PropertyCodeSummary, String> {
    let git = scan_git_repository_code_info(repo_root)
        .ok_or_else(|| "Git information is not available.".to_owned())?;
    let codebase = scan_direct_codebase_summary(repo_root)
        .ok_or_else(|| "Language makeup is not available.".to_owned())?;

    Ok(PropertyCodeSummary { git, codebase })
}

fn collect_single_file_detail_groups(
    target: &PropertyTarget,
    item_kind: PropertyItemKind,
) -> Vec<PropertyDetailGroup> {
    let Some(path) = single_file_path(target, item_kind) else {
        return Vec::new();
    };
    let mut groups = Vec::new();
    if path_may_have_media_details(path) {
        groups.extend(media_details(path));
    }
    groups
}

fn media_details(path: &Path) -> Vec<PropertyDetailGroup> {
    let mut groups = Vec::new();
    if path_may_have_exif(path) {
        groups.extend(exif_details(path));
    }
    if let Some(kind) = ffprobe_metadata_kind_for_path(path) {
        groups.extend(ffprobe_metadata_details(path, kind));
    }
    groups
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileChecksums {
    crc32: String,
    sha256: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct FileChecksumCacheKey {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
}

#[derive(Clone)]
struct CachedFileChecksums {
    checksums: FileChecksums,
    calculated_at: Instant,
}

struct FileChecksumCache {
    entries: RefCell<HashMap<FileChecksumCacheKey, CachedFileChecksums>>,
}

impl Global for FileChecksumCache {}

impl FileChecksumCache {
    fn new() -> Self {
        Self {
            entries: RefCell::new(HashMap::new()),
        }
    }

    fn get(&self, key: &FileChecksumCacheKey) -> Option<FileChecksums> {
        self.get_at(key, Instant::now())
    }

    fn get_at(&self, key: &FileChecksumCacheKey, now: Instant) -> Option<FileChecksums> {
        let cached = self.entries.borrow().get(key).cloned()?;
        if now.saturating_duration_since(cached.calculated_at) < FILE_CHECKSUM_CACHE_TTL {
            Some(cached.checksums)
        } else {
            self.entries.borrow_mut().remove(key);
            None
        }
    }

    fn insert(&self, key: FileChecksumCacheKey, checksums: FileChecksums) {
        self.insert_at(key, checksums, Instant::now());
    }

    fn insert_at(
        &self,
        key: FileChecksumCacheKey,
        checksums: FileChecksums,
        calculated_at: Instant,
    ) {
        self.entries.borrow_mut().insert(
            key,
            CachedFileChecksums {
                checksums,
                calculated_at,
            },
        );
    }
}

pub(crate) fn initialize_file_checksum_cache(cx: &mut App) {
    cx.set_global(FileChecksumCache::new());
}

const FILE_CHECKSUM_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const FILE_CHECKSUM_BUFFER_SIZE: usize = 1024 * 1024;

fn file_checksum_cache_key(path: &Path) -> io::Result<FileChecksumCacheKey> {
    let metadata = fs::metadata(path)?;
    Ok(FileChecksumCacheKey {
        path: path.to_path_buf(),
        size: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

#[cfg(test)]
fn file_checksums(path: &Path) -> std::io::Result<FileChecksums> {
    let cancel = AtomicBool::new(false);
    file_checksums_with_cancel(path, &cancel)
}

fn file_checksums_with_cancel(path: &Path, cancel: &AtomicBool) -> io::Result<FileChecksums> {
    let file = fs::File::open(path)?;
    let mut reader = file;
    let mut crc32 = crc32fast::Hasher::new();
    let mut sha256 = Sha256::new();
    let mut buffer = vec![0_u8; FILE_CHECKSUM_BUFFER_SIZE];

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "checksum calculation cancelled",
            ));
        }
        let byte_count = reader.read(&mut buffer)?;
        if byte_count == 0 {
            break;
        }
        let bytes = &buffer[..byte_count];
        crc32.update(bytes);
        sha256.update(bytes);
    }

    Ok(FileChecksums {
        crc32: format!("{:08x}", crc32.finalize()),
        sha256: format!("{:x}", sha256.finalize()),
    })
}

const EXIF_VALUE_TOO_LARGE_LABEL: &str = "<value too large to display>";
const EXIF_NON_STANDARD_VALUE_CHAR_LIMIT: usize = 1024;
const EXIF_STANDARD_VALUE_CHAR_LIMIT: usize = 5120;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FfprobeMetadataKind {
    Audio,
    Video,
}

impl FfprobeMetadataKind {
    fn log_label(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }

    fn unavailable_detail_name(self) -> &'static str {
        match self {
            Self::Audio => "Audio metadata",
            Self::Video => "Video metadata",
        }
    }

    fn no_metadata_message(self) -> &'static str {
        match self {
            Self::Audio => "No audio metadata was reported.",
            Self::Video => "No video metadata was reported.",
        }
    }
}

fn ffprobe_metadata_kind_for_path(path: &Path) -> Option<FfprobeMetadataKind> {
    let mime = mime_guess::from_path(path).first_raw();
    if mime.is_some_and(|mime| mime.starts_with("audio/")) {
        return Some(FfprobeMetadataKind::Audio);
    }
    if mime.is_some_and(|mime| mime.starts_with("video/")) {
        return Some(FfprobeMetadataKind::Video);
    }
    if path_may_have_audio_metadata(path) {
        return Some(FfprobeMetadataKind::Audio);
    }
    if path_may_have_video_metadata(path) {
        return Some(FfprobeMetadataKind::Video);
    }
    None
}

fn ffprobe_metadata_details(path: &Path, kind: FfprobeMetadataKind) -> Vec<PropertyDetailGroup> {
    let log_label = kind.log_label();
    let availability_started = Instant::now();
    let ffprobe_installed = ffprobe_is_installed();
    crate::debug_options::log_property_timing(
        availability_started.elapsed(),
        format_args!(
            "{log_label} ffprobe availability path={} installed={}",
            path.display(),
            ffprobe_installed
        ),
    );
    if !ffprobe_installed {
        return ffprobe_metadata_unavailable_groups(
            kind,
            "ffprobe is not available. Install FFmpeg/ffprobe or place ffprobe beside Explorer.",
        );
    }

    let output = match ffprobe_json_output(path) {
        Ok(output) => output,
        Err(error) => {
            return ffprobe_metadata_unavailable_groups(kind, format!("ffprobe failed: {error}"));
        }
    };
    let parse_started = Instant::now();
    let probe: serde_json::Value = match serde_json::from_slice(&output) {
        Ok(probe) => {
            crate::debug_options::log_property_timing(
                parse_started.elapsed(),
                format_args!(
                    "{log_label} ffprobe json parsed path={} stdout_bytes={}",
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
                    "{log_label} ffprobe json parse failed path={} stdout_bytes={} error={}",
                    path.display(),
                    output.len(),
                    error
                ),
            );
            return ffprobe_metadata_unavailable_groups(
                kind,
                format!("ffprobe returned unreadable metadata: {error}"),
            );
        }
    };
    let grouping_started = Instant::now();
    let groups = ffprobe_detail_groups_from_probe(&probe, kind);
    crate::debug_options::log_property_timing(
        grouping_started.elapsed(),
        format_args!(
            "{log_label} metadata grouped path={} groups={} details={}",
            path.display(),
            groups.len(),
            detail_count(&groups)
        ),
    );
    if groups.is_empty() {
        return ffprobe_metadata_unavailable_groups(kind, kind.no_metadata_message());
    }

    groups
}

fn ffprobe_metadata_unavailable_groups(
    kind: FfprobeMetadataKind,
    message: impl Into<String>,
) -> Vec<PropertyDetailGroup> {
    vec![property_detail_group(
        PropertyDetailGroupKind::Media,
        vec![PropertyDetail {
            name: kind.unavailable_detail_name().to_owned(),
            value: message.into(),
        }],
    )]
}

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn ffprobe_json_output(path: &Path) -> Result<Vec<u8>, String> {
    let mut command = Command::new(ffprobe_executable_path());
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
                    "ffprobe command failed path={} error={}",
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
                "ffprobe command succeeded path={} stdout_bytes={}",
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
                "ffprobe command failed path={} error={}",
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
                "ffprobe command failed path={} error={}",
                path.display(),
                error
            ),
        );
        Err(error)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AudioCoverRequest {
    stream_index: usize,
    label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AudioCoverPng {
    label: String,
    stream_index: usize,
    png: Vec<u8>,
}

fn load_audio_cover_previews(path: &Path) -> Result<Vec<PropertyCoverImage>, String> {
    let requests = prepare_audio_cover_requests(path)?;
    if requests.is_empty() {
        return Ok(Vec::new());
    }

    let mut covers = Vec::new();
    let mut errors = Vec::new();
    for request in requests {
        match extract_audio_cover_png(path, request).and_then(prepare_audio_cover_preview) {
            Ok(cover) => covers.push(cover),
            Err(error) => errors.push(error),
        }
    }

    if covers.is_empty() {
        Err(format!(
            "ffmpeg failed to extract audio covers: {}",
            cover_extraction_error_summary(&errors)
        ))
    } else {
        Ok(covers)
    }
}

fn prepare_audio_cover_requests(path: &Path) -> Result<Vec<AudioCoverRequest>, String> {
    let availability_started = Instant::now();
    let ffprobe_installed = ffprobe_is_installed();
    crate::debug_options::log_property_timing(
        availability_started.elapsed(),
        format_args!(
            "audio covers ffprobe availability path={} installed={}",
            path.display(),
            ffprobe_installed
        ),
    );
    if !ffprobe_installed {
        return Err(
            "ffprobe is not available. Install FFmpeg/ffprobe or place ffprobe beside Explorer."
                .to_owned(),
        );
    }
    let availability_started = Instant::now();
    let ffmpeg_installed = ffmpeg_is_installed();
    crate::debug_options::log_property_timing(
        availability_started.elapsed(),
        format_args!(
            "audio covers ffmpeg availability path={} installed={}",
            path.display(),
            ffmpeg_installed
        ),
    );
    if !ffmpeg_installed {
        return Err(
            "ffmpeg is not available. Install FFmpeg/ffprobe or place ffmpeg beside Explorer."
                .to_owned(),
        );
    }

    let output = ffprobe_json_output(path).map_err(|error| format!("ffprobe failed: {error}"))?;
    let parse_started = Instant::now();
    let probe: serde_json::Value = match serde_json::from_slice(&output) {
        Ok(probe) => {
            crate::debug_options::log_property_timing(
                parse_started.elapsed(),
                format_args!(
                    "audio covers ffprobe json parsed path={} stdout_bytes={}",
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
                    "audio covers ffprobe json parse failed path={} stdout_bytes={} error={}",
                    path.display(),
                    output.len(),
                    error
                ),
            );
            return Err(format!("ffprobe returned unreadable metadata: {error}"));
        }
    };

    Ok(audio_cover_requests_from_probe(&probe))
}

fn audio_cover_requests_from_probe(probe: &serde_json::Value) -> Vec<AudioCoverRequest> {
    let Some(streams) = probe.get("streams").and_then(|streams| streams.as_array()) else {
        return Vec::new();
    };

    let mut requests = Vec::new();
    for (array_index, stream_value) in streams.iter().enumerate() {
        let Some(stream) = stream_value.as_object() else {
            continue;
        };
        if !is_attached_picture_stream(stream) {
            continue;
        }
        let cover_number = requests.len() + 1;
        let stream_index = stream
            .get("index")
            .and_then(ffprobe_integer_value)
            .and_then(|index| usize::try_from(index).ok())
            .unwrap_or(array_index);
        requests.push(AudioCoverRequest {
            stream_index,
            label: audio_cover_label(stream, cover_number),
        });
    }
    requests
}

fn audio_cover_label(
    stream: &serde_json::Map<String, serde_json::Value>,
    cover_number: usize,
) -> String {
    stream
        .get("tags")
        .and_then(|tags| tags.as_object())
        .and_then(|tags| {
            tags.iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("title"))
                .and_then(|(_, value)| ffprobe_scalar_value_label(value))
        })
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| format!("Cover {cover_number}"))
}

fn extract_audio_cover_png(
    path: &Path,
    request: AudioCoverRequest,
) -> Result<AudioCoverPng, String> {
    let started = Instant::now();
    match ffmpeg_cover_png_output(path, request.stream_index) {
        Ok(png) if png.starts_with(FFMPEG_PNG_SIGNATURE) => {
            crate::debug_options::log_property_timing(
                started.elapsed(),
                format_args!(
                    "audio cover extracted path={} label={} stream={} stdout_bytes={}",
                    path.display(),
                    request.label,
                    request.stream_index,
                    png.len()
                ),
            );
            Ok(AudioCoverPng {
                label: request.label,
                stream_index: request.stream_index,
                png,
            })
        }
        Ok(png) => {
            crate::debug_options::log_property_timing(
                started.elapsed(),
                format_args!(
                    "audio cover rejected path={} label={} stream={} stdout_bytes={} error=not-png",
                    path.display(),
                    request.label,
                    request.stream_index,
                    png.len()
                ),
            );
            Err(format!(
                "{}: ffmpeg returned {} bytes, but not a PNG image",
                request.label,
                png.len()
            ))
        }
        Err(error) => {
            crate::debug_options::log_property_timing(
                started.elapsed(),
                format_args!(
                    "audio cover failed path={} label={} stream={} error={}",
                    path.display(),
                    request.label,
                    request.stream_index,
                    error
                ),
            );
            Err(format!("{}: {error}", request.label))
        }
    }
}

fn prepare_audio_cover_preview(cover: AudioCoverPng) -> Result<PropertyCoverImage, String> {
    let image = image::load_from_memory_with_format(&cover.png, image::ImageFormat::Png)
        .map_err(|error| {
            format!(
                "{}: ffmpeg returned unreadable PNG data for stream {}: {error}",
                cover.label, cover.stream_index
            )
        })?
        .into_rgba8();
    let preview = property_image_preview_from_rgba(&cover.label, image)?;
    Ok(PropertyCoverImage {
        label: cover.label,
        preview,
    })
}

fn property_image_preview_from_rgba(
    label: &str,
    mut image: image::RgbaImage,
) -> Result<PropertyImagePreview, String> {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return Err(format!(
            "{label}: ffmpeg returned a PNG image with no dimensions"
        ));
    }

    for pixel in image.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    Ok(PropertyImagePreview {
        image: Arc::new(RenderImage::new(vec![image::Frame::new(image)])),
        width,
        height,
        animated_source: None,
    })
}

fn ffmpeg_cover_png_output(path: &Path, stream_index: usize) -> Result<Vec<u8>, String> {
    let mut command = Command::new(ffmpeg_executable_path());
    command
        .arg("-v")
        .arg("error")
        .arg("-nostdin")
        .arg("-i")
        .arg(path)
        .arg("-map")
        .arg(format!("0:{stream_index}"))
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

fn cover_extraction_error_summary(errors: &[String]) -> String {
    match errors {
        [] => "no cover data was returned".to_owned(),
        [error] => error.clone(),
        [first, ..] => format!("{first} ({} cover attempts failed)", errors.len()),
    }
}

#[derive(Clone, Debug, PartialEq)]
struct VideoFrameRequest {
    label_seconds: f64,
    seek_seconds: f64,
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct VideoFramePng {
    label: String,
    png: Vec<u8>,
}

fn prepare_video_frame_requests(path: &Path) -> Result<Vec<VideoFrameRequest>, String> {
    let probe_started = Instant::now();
    let duration =
        probe_video_duration_seconds(path).map_err(|error| format!("ffprobe failed: {error}"))?;
    crate::debug_options::log_property_timing(
        probe_started.elapsed(),
        format_args!(
            "video frames duration probed path={} duration={duration:.3}",
            path.display()
        ),
    );
    let requests = video_frame_requests(duration);
    crate::debug_options::log_property_marker(format_args!(
        "video frames planned path={} {}",
        path.display(),
        video_frame_request_debug_summary(duration, &requests)
    ));
    if requests.is_empty() {
        return Err("Video duration is not long enough to extract frames.".to_owned());
    }

    Ok(requests)
}

#[cfg(test)]
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
        width,
        height,
        aspect_ratio: width as f32 / height as f32,
    })
}

fn prepare_video_frame_thumbnail_rgba(
    label: String,
    frame: VideoFrameRgba,
) -> PropertyFrameThumbnail {
    let mut image = frame.image;
    let width = image.width();
    let height = image.height();
    for pixel in image.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    PropertyFrameThumbnail {
        label,
        image: Arc::new(RenderImage::new(vec![image::Frame::new(image)])),
        width,
        height,
        aspect_ratio: width as f32 / height as f32,
    }
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

fn video_frame_request_debug_summary(
    duration_seconds: f64,
    requests: &[VideoFrameRequest],
) -> String {
    let duration = ffmpeg_seek_argument(duration_seconds);
    let Some(first) = requests.first() else {
        return format!("duration={duration} requests=0");
    };
    let last = requests.last().unwrap_or(first);
    format!(
        "duration={} requests={} first_label={} first_seek={} last_label={} last_seek={}",
        duration,
        requests.len(),
        video_frame_timestamp_label(first.label_seconds),
        ffmpeg_seek_argument(first.seek_seconds),
        video_frame_timestamp_label(last.label_seconds),
        ffmpeg_seek_argument(last.seek_seconds)
    )
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

fn load_audio_spectrum_analysis(
    path: &Path,
    range: PropertySpectrumRange,
    target: PropertySpectrumTarget,
    cancel: &AtomicBool,
) -> Result<PropertySpectrumAnalysis, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Spectrum analysis was cancelled.".to_owned());
    }

    let availability_started = Instant::now();
    let ffprobe_installed = ffprobe_is_installed();
    crate::debug_options::log_property_timing(
        availability_started.elapsed(),
        format_args!(
            "spectrum ffprobe availability path={} installed={}",
            path.display(),
            ffprobe_installed
        ),
    );
    if !ffprobe_installed {
        return Err(
            "ffprobe is not available. Install FFmpeg/ffprobe or place ffprobe beside Explorer."
                .to_owned(),
        );
    }

    let availability_started = Instant::now();
    let ffmpeg_installed = ffmpeg_is_installed();
    crate::debug_options::log_property_timing(
        availability_started.elapsed(),
        format_args!(
            "spectrum ffmpeg availability path={} installed={}",
            path.display(),
            ffmpeg_installed
        ),
    );
    if !ffmpeg_installed {
        return Err(
            "ffmpeg is not available. Install FFmpeg/ffprobe or place ffmpeg beside Explorer."
                .to_owned(),
        );
    }

    let output = ffprobe_json_output(path).map_err(|error| format!("ffprobe failed: {error}"))?;
    let parse_started = Instant::now();
    let probe: serde_json::Value = serde_json::from_slice(&output).map_err(|error| {
        crate::debug_options::log_property_timing(
            parse_started.elapsed(),
            format_args!(
                "spectrum ffprobe json parse failed path={} stdout_bytes={} error={}",
                path.display(),
                output.len(),
                error
            ),
        );
        format!("ffprobe returned unreadable metadata: {error}")
    })?;
    crate::debug_options::log_property_timing(
        parse_started.elapsed(),
        format_args!(
            "spectrum ffprobe json parsed path={} stdout_bytes={}",
            path.display(),
            output.len()
        ),
    );

    let metadata = audio_spectrum_metadata_from_probe(&probe)?;
    let db_values = decode_audio_spectrum_db_values(path, &metadata, target, cancel)?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Spectrum analysis was cancelled.".to_owned());
    }

    let image = spectrum_render_image(&db_values, target.width(), target.height(), range)
        .ok_or_else(|| "Spectrum image has invalid dimensions.".to_owned())?;

    Ok(PropertySpectrumAnalysis {
        metadata,
        db_values,
        target,
        image,
        width: target.width(),
        height: target.height(),
    })
}

fn audio_spectrum_metadata_from_probe(
    probe: &serde_json::Value,
) -> Result<PropertySpectrumMetadata, String> {
    let stream = first_audio_stream(probe)
        .ok_or_else(|| "No audio stream was reported by ffprobe.".to_owned())?;
    let sample_rate = stream
        .get("sample_rate")
        .and_then(ffprobe_scalar_value_label)
        .as_deref()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|sample_rate| *sample_rate > 0)
        .ok_or_else(|| "Audio sample rate is not available.".to_owned())?;
    let duration_seconds = ffprobe_audio_duration_seconds_from_probe(probe, stream)
        .ok_or_else(|| "Audio duration is not available.".to_owned())?;
    let codec = audio_spectrum_codec_label(stream);
    let bit_depth = audio_spectrum_bit_depth(stream);
    let bit_rate = audio_spectrum_bit_rate(probe, stream);
    let channels = stream
        .get("channels")
        .and_then(ffprobe_integer_value)
        .and_then(|channels| u32::try_from(channels).ok())
        .filter(|channels| *channels > 0)
        .ok_or_else(|| "Audio channel count is not available.".to_owned())?;
    let header_bit_rate = bit_depth.is_none().then_some(bit_rate).flatten();
    let header = audio_spectrum_header(
        &codec,
        header_bit_rate,
        sample_rate,
        bit_depth,
        Some(channels),
    );

    Ok(PropertySpectrumMetadata {
        header,
        sample_rate,
        duration_seconds,
        bit_rate,
        bit_depth,
        channels,
    })
}

fn first_audio_stream(
    probe: &serde_json::Value,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    probe
        .get("streams")
        .and_then(|streams| streams.as_array())
        .and_then(|streams| {
            streams.iter().find_map(|stream| {
                let stream = stream.as_object()?;
                let codec_type = stream
                    .get("codec_type")
                    .and_then(ffprobe_scalar_value_label);
                (codec_type.as_deref() == Some("audio")).then_some(stream)
            })
        })
}

fn ffprobe_audio_duration_seconds_from_probe(
    probe: &serde_json::Value,
    stream: &serde_json::Map<String, serde_json::Value>,
) -> Option<f64> {
    let stream_duration = stream.get("duration").and_then(|duration| {
        ffprobe_scalar_value_label(duration)
            .as_deref()
            .and_then(parse_positive_f64)
    });
    if stream_duration.is_some() {
        return stream_duration;
    }

    let format_duration = probe
        .get("format")
        .and_then(|format| format.as_object())
        .and_then(|format| format.get("duration"))
        .and_then(|duration| {
            ffprobe_scalar_value_label(duration)
                .as_deref()
                .and_then(parse_positive_f64)
        });
    if format_duration.is_some() {
        return format_duration;
    }

    None
}

fn audio_spectrum_codec_label(stream: &serde_json::Map<String, serde_json::Value>) -> String {
    if let Some(codec) = stream
        .get("codec_long_name")
        .and_then(ffprobe_scalar_value_label)
    {
        return codec;
    }

    stream
        .get("codec_name")
        .and_then(ffprobe_scalar_value_label)
        .map(|codec| codec.to_ascii_uppercase())
        .unwrap_or_else(|| "Audio".to_owned())
}

fn audio_spectrum_bit_depth(stream: &serde_json::Map<String, serde_json::Value>) -> Option<u32> {
    for key in [
        "bits_per_raw_sample",
        "bits_per_sample",
        "bits_per_coded_sample",
    ] {
        if let Some(bits) = stream
            .get(key)
            .and_then(ffprobe_integer_value)
            .and_then(|bits| u32::try_from(bits).ok())
            .filter(|bits| *bits > 0)
        {
            return Some(bits);
        }
    }

    None
}

fn audio_spectrum_bit_rate(
    probe: &serde_json::Value,
    stream: &serde_json::Map<String, serde_json::Value>,
) -> Option<u64> {
    let stream_bit_rate = stream
        .get("bit_rate")
        .and_then(ffprobe_scalar_value_label)
        .as_deref()
        .and_then(parse_positive_f64)
        .map(|bit_rate| bit_rate.round() as u64)
        .filter(|bit_rate| *bit_rate > 0);
    if stream_bit_rate.is_some() {
        return stream_bit_rate;
    }

    probe
        .get("format")
        .and_then(|format| format.as_object())
        .and_then(|format| format.get("bit_rate"))
        .and_then(ffprobe_scalar_value_label)
        .as_deref()
        .and_then(parse_positive_f64)
        .map(|bit_rate| bit_rate.round() as u64)
        .filter(|bit_rate| *bit_rate > 0)
}

fn audio_spectrum_header(
    codec: &str,
    bit_rate: Option<u64>,
    sample_rate: u32,
    bit_depth: Option<u32>,
    channels: Option<u32>,
) -> String {
    let mut parts = vec![codec.to_owned()];
    if let Some(bit_rate) = bit_rate {
        parts.push(format!("{} kbps", (bit_rate + 500) / 1000));
    }
    parts.push(format!("{sample_rate} Hz"));
    if let Some(bit_depth) = bit_depth
        && bit_rate.is_none()
    {
        parts.push(format!("{bit_depth} bits"));
    }
    if let Some(channels) = channels {
        let suffix = if channels == 1 { "channel" } else { "channels" };
        parts.push(format!("{channels} {suffix}"));
    }
    parts.join(", ")
}

fn decode_audio_spectrum_db_values(
    path: &Path,
    metadata: &PropertySpectrumMetadata,
    target: PropertySpectrumTarget,
    cancel: &AtomicBool,
) -> Result<Vec<f32>, String> {
    let mut child = spawn_audio_spectrum_ffmpeg(path, 0)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "ffmpeg did not provide decoded audio.".to_owned())?;
    let stderr_handle = child.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            bytes
        })
    });

    let mut reader = BufReader::new(stdout);
    let result = collect_spectrum_db_values_from_pcm_reader(&mut reader, metadata, target, cancel);
    if result.is_err() || cancel.load(Ordering::Relaxed) {
        let _ = child.kill();
    }
    let status = child
        .wait()
        .map_err(|error| format!("could not wait for ffmpeg: {error}"))?;
    let stderr = stderr_handle
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();

    if cancel.load(Ordering::Relaxed) {
        return Err("Spectrum analysis was cancelled.".to_owned());
    }
    let db_values = result?;
    if !status.success() {
        let stderr = command_error_output_label(&stderr);
        return if stderr.is_empty() {
            Err(format!("ffmpeg exited with {status}"))
        } else {
            Err(format!("ffmpeg exited with {status}: {stderr}"))
        };
    }

    Ok(db_values)
}

fn spawn_audio_spectrum_ffmpeg(
    path: &Path,
    audio_stream_index: usize,
) -> Result<std::process::Child, String> {
    let mut command = Command::new(ffmpeg_executable_path());
    for arg in ffmpeg_audio_spectrum_args(path, audio_stream_index) {
        command.arg(arg);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }

    command
        .spawn()
        .map_err(|error| format!("could not start ffmpeg: {error}"))
}

fn ffmpeg_audio_spectrum_args(path: &Path, audio_stream_index: usize) -> Vec<OsString> {
    vec![
        OsString::from("-v"),
        OsString::from("error"),
        OsString::from("-nostdin"),
        OsString::from("-i"),
        path.as_os_str().to_os_string(),
        OsString::from("-map"),
        OsString::from(format!("0:a:{audio_stream_index}")),
        OsString::from("-vn"),
        OsString::from("-f"),
        OsString::from("f32le"),
        OsString::from("-acodec"),
        OsString::from("pcm_f32le"),
        OsString::from("-"),
    ]
}

fn spectrum_column_end_samples(
    duration_seconds: f64,
    sample_rate: u32,
    columns: usize,
) -> Vec<u64> {
    if columns == 0 {
        return Vec::new();
    }
    let total_samples = (duration_seconds.max(0.0) * f64::from(sample_rate))
        .round()
        .max(0.0) as u64;

    (1..=columns)
        .map(|column| ((column as u128 * total_samples as u128) / columns as u128) as u64)
        .collect()
}

fn collect_spectrum_db_values_from_pcm_reader(
    reader: &mut impl Read,
    metadata: &PropertySpectrumMetadata,
    target: PropertySpectrumTarget,
    cancel: &AtomicBool,
) -> Result<Vec<f32>, String> {
    let channels = usize::try_from(metadata.channels)
        .ok()
        .filter(|channels| *channels > 0)
        .ok_or_else(|| "Audio channel count is not available.".to_owned())?;
    let frame_size = channels
        .checked_mul(4)
        .ok_or_else(|| "Audio channel count is too large.".to_owned())?;
    let column_ends = spectrum_column_end_samples(
        metadata.duration_seconds,
        metadata.sample_rate,
        target.time_bins,
    );
    let mut db_values = Vec::with_capacity(target.time_bins * target.frequency_bins);
    let mut accumulator = SpectrumColumnAccumulator::new(target);
    let mut read_buffer = [0u8; 64 * 1024];
    let mut pending = Vec::new();
    let mut sample_index = 0u64;
    let mut next_column = 0usize;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Spectrum analysis was cancelled.".to_owned());
        }
        let count = reader
            .read(&mut read_buffer)
            .map_err(|error| format!("could not read decoded audio: {error}"))?;
        if count == 0 {
            break;
        }

        pending.extend_from_slice(&read_buffer[..count]);
        let complete_len = (pending.len() / frame_size) * frame_size;
        for frame in pending[..complete_len].chunks_exact(frame_size) {
            while next_column < column_ends.len() && sample_index >= column_ends[next_column] {
                accumulator.finish_column(sample_index, &mut db_values);
                next_column += 1;
            }

            let sample = average_f32le_audio_frame(frame, channels);
            if next_column < column_ends.len() {
                accumulator.push_sample(sample, sample_index);
                sample_index = sample_index.saturating_add(1);

                if sample_index >= column_ends[next_column] {
                    accumulator.finish_column(sample_index, &mut db_values);
                    next_column += 1;
                }
            } else {
                sample_index = sample_index.saturating_add(1);
            }
        }
        pending.drain(..complete_len);
    }

    if sample_index == 0 {
        return Err("ffmpeg returned no decoded audio samples.".to_owned());
    }

    while next_column < column_ends.len() {
        accumulator.finish_column(sample_index, &mut db_values);
        next_column += 1;
    }

    Ok(db_values)
}

fn average_f32le_audio_frame(frame: &[u8], channels: usize) -> f32 {
    let mut sum = 0.0;
    for channel in 0..channels {
        let offset = channel * 4;
        sum += f32::from_le_bytes([
            frame[offset],
            frame[offset + 1],
            frame[offset + 2],
            frame[offset + 3],
        ]);
    }
    sum / channels as f32
}

fn copy_spectrum_window_ending_at(ring: &[f32], end_sample: u64, output: &mut [f32]) {
    output.fill(0.0);
    let available = end_sample.min(output.len() as u64) as usize;
    let output_start = output.len().saturating_sub(available);
    let sample_start = end_sample.saturating_sub(available as u64);
    for index in 0..available {
        output[output_start + index] = ring[((sample_start + index as u64) as usize) % ring.len()];
    }
}

struct SpectrumColumnAccumulator {
    target: PropertySpectrumTarget,
    ring: Vec<f32>,
    window: Vec<f32>,
    fft_buffer: Vec<rustfft::num_complex::Complex<f32>>,
    fft: Arc<dyn rustfft::Fft<f32>>,
    sum_db: Vec<f32>,
    column_db: Vec<f32>,
    interval_frames: usize,
    fft_count: usize,
}

impl SpectrumColumnAccumulator {
    fn new(target: PropertySpectrumTarget) -> Self {
        let mut planner = rustfft::FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(target.fft_size);
        Self {
            target,
            ring: vec![0.0; target.fft_size],
            window: vec![0.0; target.fft_size],
            fft_buffer: vec![rustfft::num_complex::Complex { re: 0.0, im: 0.0 }; target.fft_size],
            fft,
            sum_db: vec![0.0; target.frequency_bins],
            column_db: vec![0.0; target.frequency_bins],
            interval_frames: 0,
            fft_count: 0,
        }
    }

    fn push_sample(&mut self, sample: f32, sample_index: u64) {
        self.ring[(sample_index as usize) % self.target.fft_size] = sample;
        self.interval_frames += 1;
        if self.interval_frames % self.target.fft_size == 0 {
            self.accumulate_window_ending_at(sample_index.saturating_add(1));
        }
    }

    fn finish_column(&mut self, sample_index: u64, db_values: &mut Vec<f32>) {
        if self.fft_count == 0 {
            if self.interval_frames > 0 {
                self.accumulate_window_ending_at(sample_index);
            } else {
                self.accumulate_samples(&vec![0.0; self.target.fft_size]);
            }
        }

        let count = self.fft_count.max(1) as f32;
        for db in &self.sum_db {
            db_values.push(*db / count);
        }

        self.sum_db.fill(0.0);
        self.interval_frames = 0;
        self.fft_count = 0;
    }

    fn accumulate_window_ending_at(&mut self, sample_index: u64) {
        copy_spectrum_window_ending_at(&self.ring, sample_index, &mut self.window);
        self.accumulate_samples_from_window();
    }

    fn accumulate_samples_from_window(&mut self) {
        spectrum_db_column_with_plan(
            &self.window,
            self.target,
            self.fft.as_ref(),
            &mut self.fft_buffer,
            &mut self.column_db,
        );
        for (sum, db) in self.sum_db.iter_mut().zip(&self.column_db) {
            *sum += *db;
        }
        self.fft_count += 1;
    }

    fn accumulate_samples(&mut self, samples: &[f32]) {
        spectrum_db_column_with_plan(
            samples,
            self.target,
            self.fft.as_ref(),
            &mut self.fft_buffer,
            &mut self.column_db,
        );
        for (sum, db) in self.sum_db.iter_mut().zip(&self.column_db) {
            *sum += *db;
        }
        self.fft_count += 1;
    }
}

#[cfg(test)]
fn spectrum_db_values_from_windows(windows: &[f32], target: PropertySpectrumTarget) -> Vec<f32> {
    windows
        .chunks_exact(target.fft_size)
        .flat_map(|samples| spectrum_db_column(samples, target))
        .collect()
}

#[cfg(test)]
fn spectrum_db_column(samples: &[f32], target: PropertySpectrumTarget) -> Vec<f32> {
    let mut planner = rustfft::FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(target.fft_size);
    let mut buffer = vec![rustfft::num_complex::Complex { re: 0.0, im: 0.0 }; target.fft_size];
    let mut db_values = vec![0.0; target.frequency_bins];
    spectrum_db_column_with_plan(samples, target, fft.as_ref(), &mut buffer, &mut db_values);
    db_values
}

fn spectrum_db_column_with_plan(
    samples: &[f32],
    target: PropertySpectrumTarget,
    fft: &dyn rustfft::Fft<f32>,
    buffer: &mut [rustfft::num_complex::Complex<f32>],
    db_values: &mut [f32],
) {
    debug_assert_eq!(buffer.len(), target.fft_size);
    debug_assert_eq!(db_values.len(), target.frequency_bins);
    for (index, value) in buffer.iter_mut().enumerate() {
        let sample = samples.get(index).copied().unwrap_or(0.0);
        *value = rustfft::num_complex::Complex {
            re: sample * hann_window_value(index, target.fft_size),
            im: 0.0,
        };
    }
    fft.process(buffer);
    let nyquist_bin = target.fft_size / 2;
    for (frequency_bin, db) in db_values.iter_mut().enumerate() {
        let fft_bin = if target.frequency_bins <= 1 {
            0
        } else {
            frequency_bin * nyquist_bin / (target.frequency_bins - 1)
        };
        let value = buffer[fft_bin];
        let magnitude =
            (value.re.mul_add(value.re, value.im * value.im)).sqrt() / target.fft_size as f32;
        *db = 20.0 * magnitude.max(1.0e-12).log10();
    }
}

fn hann_window_value(index: usize, len: usize) -> f32 {
    if len <= 1 {
        return 1.0;
    }
    let phase = (std::f32::consts::TAU * index as f32) / (len - 1) as f32;
    0.5 - 0.5 * phase.cos()
}

fn spectrum_render_image_for_target(
    analysis: &PropertySpectrumAnalysis,
    generation: u64,
    range: PropertySpectrumRange,
    target: Option<PropertySpectrumRenderSize>,
    render_cache: &mut Option<PropertySpectrumRenderCache>,
) -> Arc<RenderImage> {
    let Some(target) = target else {
        return analysis.image.clone();
    };
    let key = PropertySpectrumRenderCacheKey::new(
        generation,
        analysis.width,
        analysis.height,
        target,
        range,
    );
    if let Some(cache) = render_cache.as_ref()
        && cache.key == key
    {
        return cache.image.clone();
    }

    let Some(image) = spectrum_render_image_resampled(
        &analysis.db_values,
        analysis.width,
        analysis.height,
        target.width,
        target.height,
        range,
    ) else {
        *render_cache = None;
        return analysis.image.clone();
    };

    *render_cache = Some(PropertySpectrumRenderCache {
        key,
        image: image.clone(),
    });
    image
}

fn spectrum_render_image(
    db_values: &[f32],
    width: u32,
    height: u32,
    range: PropertySpectrumRange,
) -> Option<Arc<RenderImage>> {
    let width_usize = usize::try_from(width).ok()?;
    let height_usize = usize::try_from(height).ok()?;
    if width_usize == 0
        || height_usize == 0
        || db_values.len() != width_usize.checked_mul(height_usize)?
    {
        return None;
    }

    let mut bgra = Vec::with_capacity(width_usize * height_usize * 4);
    for y in 0..height_usize {
        let frequency_bin = height_usize - y - 1;
        for x in 0..width_usize {
            let db = db_values[x * height_usize + frequency_bin];
            bgra.extend_from_slice(&spectrum_density_bgra(db, range));
        }
    }

    render_image_from_bgra_bytes(width, height, bgra)
}

fn spectrum_render_image_resampled(
    db_values: &[f32],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    range: PropertySpectrumRange,
) -> Option<Arc<RenderImage>> {
    let source_width = usize::try_from(source_width).ok()?;
    let source_height = usize::try_from(source_height).ok()?;
    let target_width = usize::try_from(target_width).ok()?;
    let target_height = usize::try_from(target_height).ok()?;
    if source_width == 0
        || source_height == 0
        || target_width == 0
        || target_height == 0
        || db_values.len() != source_width.checked_mul(source_height)?
    {
        return None;
    }

    let pixel_count = target_width.checked_mul(target_height)?;
    let mut bgra = Vec::with_capacity(pixel_count.checked_mul(4)?);
    for y in 0..target_height {
        let source_y_from_top = y * source_height / target_height;
        let frequency_bin = source_height - source_y_from_top - 1;
        for x in 0..target_width {
            let source_x = x * source_width / target_width;
            let db = db_values[source_x * source_height + frequency_bin];
            bgra.extend_from_slice(&spectrum_density_bgra(db, range));
        }
    }

    render_image_from_bgra_bytes(target_width as u32, target_height as u32, bgra)
}

fn spectrum_legend_render_image(range: PropertySpectrumRange) -> Option<Arc<RenderImage>> {
    let height = PROPERTIES_SPECTRUM_FREQUENCY_BINS as u32;
    let mut bgra = Vec::with_capacity(height as usize * 4);
    for y in 0..height {
        let level = if height <= 1 {
            1.0
        } else {
            (height - y - 1) as f32 / height as f32
        };
        let db = range.low_db + (range.high_db - range.low_db) * level;
        bgra.extend_from_slice(&spectrum_density_bgra(db, range));
    }
    render_image_from_bgra_bytes(1, height, bgra)
}

fn empty_render_image() -> Arc<RenderImage> {
    render_image_from_bgra_bytes(1, 1, vec![0, 0, 0, 255]).expect("static one-pixel image is valid")
}

fn render_image_from_bgra_bytes(
    width: u32,
    height: u32,
    bgra: Vec<u8>,
) -> Option<Arc<RenderImage>> {
    let image = image::RgbaImage::from_raw(width, height, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(image)])))
}

fn spectrum_density_bgra(db: f32, range: PropertySpectrumRange) -> [u8; 4] {
    let span = (range.high_db - range.low_db).max(PROPERTIES_SPECTRUM_MIN_RANGE_DB);
    let level = ((db - range.low_db) / span).clamp(0.0, 1.0 - f32::EPSILON);
    spectrum_palette_bgra(level)
}

fn spectrum_palette_bgra(level: f32) -> [u8; 4] {
    let level = f64::from(level.clamp(0.0, 1.0)) * 0.6625;
    let mut r = 0.0;
    let mut g = 0.0;
    let mut b = 0.0;

    if (0.0..0.15).contains(&level) {
        r = (0.15 - level) / (0.15 + 0.075);
        b = 1.0;
    } else if (0.15..0.275).contains(&level) {
        g = (level - 0.15) / (0.275 - 0.15);
        b = 1.0;
    } else if (0.275..0.325).contains(&level) {
        g = 1.0;
        b = (0.325 - level) / (0.325 - 0.275);
    } else if (0.325..0.5).contains(&level) {
        r = (level - 0.325) / (0.5 - 0.325);
        g = 1.0;
    } else if (0.5..0.6625).contains(&level) {
        r = 1.0;
        g = (0.6625 - level) / (0.6625 - 0.5);
    }

    let mut correction = 1.0;
    if (0.0..0.1).contains(&level) {
        correction = level / 0.1;
    }
    correction *= 255.0;

    let channel = |value: f64| (value * correction + 0.5).clamp(0.0, 255.0) as u8;
    [channel(b), channel(g), channel(r), 255]
}

fn spectrum_frequency_ruler_labels(sample_rate: u32) -> Vec<String> {
    let nyquist_hz = f64::from(sample_rate) / 2.0;
    if nyquist_hz <= 0.0 {
        return vec!["0 kHz".to_owned()];
    }

    let mut labels = vec![frequency_ruler_label(nyquist_hz)];
    let step_hz = if nyquist_hz <= 5_000.0 {
        1_000.0
    } else if nyquist_hz <= 12_000.0 {
        2_000.0
    } else {
        5_000.0
    };
    let mut tick_hz = (nyquist_hz / step_hz).floor() * step_hz;
    if (nyquist_hz - tick_hz).abs() < 1.0 {
        tick_hz -= step_hz;
    }
    while tick_hz > 0.0 {
        labels.push(frequency_ruler_label(tick_hz));
        tick_hz -= step_hz;
    }
    labels.push("0 kHz".to_owned());
    labels
}

fn frequency_ruler_label(hz: f64) -> String {
    if hz >= 1000.0 {
        format!("{} kHz", (hz / 1000.0).floor() as u64)
    } else {
        format!("{:.0} Hz", hz.round())
    }
}

fn spectrum_time_ruler_labels(duration_seconds: f64) -> Vec<String> {
    let duration_seconds = duration_seconds.max(0.0);
    if duration_seconds <= 0.0 {
        return vec!["0:00".to_owned()];
    }

    let step = nice_spectrum_time_step(duration_seconds / 8.0);
    let mut labels = vec!["0:00".to_owned()];
    let mut tick = step;
    while tick < duration_seconds - 0.5 {
        labels.push(spectrum_time_label(tick));
        tick += step;
    }
    let end = spectrum_time_label(duration_seconds);
    if labels.last().is_none_or(|label| *label != end) {
        labels.push(end);
    }
    labels
}

fn nice_spectrum_time_step(target_seconds: f64) -> f64 {
    for step in [
        1.0, 2.0, 5.0, 10.0, 20.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1200.0, 1800.0, 3600.0,
    ] {
        if target_seconds <= step {
            return step;
        }
    }
    let hours = (target_seconds / 3600.0).ceil().max(1.0);
    hours * 3600.0
}

fn spectrum_time_label(seconds: f64) -> String {
    let total_seconds = seconds.max(0.0).round() as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn spectrum_density_ruler_labels(range: PropertySpectrumRange) -> Vec<String> {
    let high = (range.high_db / 10.0).round() * 10.0;
    let low = (range.low_db / 10.0).round() * 10.0;
    let mut labels = Vec::new();
    let mut tick = high;
    while tick >= low {
        labels.push(format!("{tick:.0} dB"));
        tick -= 20.0;
    }
    let low_label = format!("{low:.0} dB");
    if labels.last().is_none_or(|label| *label != low_label) {
        labels.push(low_label);
    }
    labels
}

fn spectrum_vertical_ruler(
    debug_selector: &'static str,
    labels: Vec<String>,
    width: f32,
    text_align: gpui::TextAlign,
) -> AnyElement {
    let mut ruler = div()
        .debug_selector(move || debug_selector.to_owned())
        .w(px(width))
        .min_w(px(width))
        .max_w(px(width))
        .h_full()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .justify_between()
        .text_size(px(10.0))
        .line_height(px(12.0))
        .text_color(rgb(PROPERTIES_SPECTRUM_AXIS_TEXT));
    for (index, label) in labels.into_iter().enumerate() {
        ruler = ruler.child(
            div()
                .debug_selector(move || format!("{debug_selector}-label-{index}"))
                .w(px(width))
                .min_w(px(width))
                .max_w(px(width))
                .flex_shrink_0()
                .line_height(px(12.0))
                .text_align(text_align)
                .child(SharedString::from(label)),
        );
    }
    ruler.into_any_element()
}

fn spectrum_horizontal_ruler(labels: Vec<String>) -> AnyElement {
    let mut ruler = div()
        .h(px(PROPERTIES_SPECTRUM_TIME_RULER_HEIGHT))
        .w_full()
        .min_w(px(0.0))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .text_size(px(10.0))
        .text_color(rgb(PROPERTIES_SPECTRUM_AXIS_TEXT));
    for label in labels {
        ruler = ruler.child(
            div()
                .min_w(px(0.0))
                .truncate()
                .child(SharedString::from(label)),
        );
    }
    ruler.into_any_element()
}

fn spectrum_range_control_group(
    label: &'static str,
    value: f32,
    on_decrement: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_increment: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let (decrement_id, increment_id, decrement_tooltip, increment_tooltip) = match label {
        "Low" => (
            "properties-spectrum-low-db-decrement",
            "properties-spectrum-low-db-increment",
            "Lower minimum spectral density",
            "Raise minimum spectral density",
        ),
        "High" => (
            "properties-spectrum-high-db-decrement",
            "properties-spectrum-high-db-increment",
            "Lower maximum spectral density",
            "Raise maximum spectral density",
        ),
        _ => (
            "properties-spectrum-db-decrement",
            "properties-spectrum-db-increment",
            "Lower spectral density",
            "Raise spectral density",
        ),
    };

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .text_size(px(11.0))
        .text_color(rgb(PROPERTIES_SPECTRUM_AXIS_TEXT))
        .child(SharedString::from(format!("{label} {:.0} dB", value)))
        .child(spectrum_range_button(
            decrement_id,
            "-",
            decrement_tooltip,
            on_decrement,
        ))
        .child(spectrum_range_button(
            increment_id,
            "+",
            increment_tooltip,
            on_increment,
        ))
        .into_any_element()
}

fn spectrum_range_button(
    id: &'static str,
    label: &'static str,
    tooltip: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .w(px(PROPERTIES_SPECTRUM_CONTROL_BUTTON_SIZE))
        .h(px(PROPERTIES_SPECTRUM_CONTROL_BUTTON_SIZE))
        .border_1()
        .border_color(rgb(PROPERTIES_SPECTRUM_CONTROL_BORDER))
        .bg(rgb(0x161616))
        .hover(|style| style.bg(rgb(0x2b2b2b)))
        .active(|style| style.bg(rgb(0x3a3a3a)))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.0))
        .text_color(rgb(PROPERTIES_SPECTRUM_AXIS_TEXT))
        .cursor_default()
        .tooltip(explorer_tooltip(tooltip))
        .on_click(on_click)
        .child(label)
        .into_any_element()
}

#[derive(Default)]
struct FfprobeDetailBuilder {
    groups: BTreeMap<PropertyDetailGroupKind, Vec<PropertyDetail>>,
    used_paths: BTreeSet<String>,
    stream_labels: BTreeMap<usize, String>,
}

impl FfprobeDetailBuilder {
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

    fn scalar_field_from_keys(
        &mut self,
        object: &serde_json::Map<String, serde_json::Value>,
        base_path: &str,
        keys: &[&str],
    ) -> Option<String> {
        let mut selected = None;
        for key in keys {
            if let Some((actual_key, value)) = object
                .iter()
                .find(|(actual_key, _)| actual_key.eq_ignore_ascii_case(key))
            {
                self.used_paths.insert(format!("{base_path}.{actual_key}"));
                if selected.is_none() {
                    selected = ffprobe_scalar_value_label(value);
                }
            }
        }
        selected
    }

    fn tag_field(
        &mut self,
        object: &serde_json::Map<String, serde_json::Value>,
        base_path: &str,
        key: &str,
    ) -> Option<String> {
        self.tag_field_from_keys(object, base_path, &[key])
    }

    fn tag_field_from_keys(
        &mut self,
        object: &serde_json::Map<String, serde_json::Value>,
        base_path: &str,
        keys: &[&str],
    ) -> Option<String> {
        let tags = object.get("tags")?.as_object()?;
        let mut selected = None;
        for key in keys {
            if let Some((actual_key, value)) = tags
                .iter()
                .find(|(actual_key, _)| actual_key.eq_ignore_ascii_case(key))
            {
                self.used_paths
                    .insert(format!("{base_path}.tags.{actual_key}"));
                if selected.is_none() {
                    selected = ffprobe_scalar_value_label(value);
                }
            }
        }
        selected
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

fn ffprobe_detail_groups_from_probe(
    probe: &serde_json::Value,
    kind: FfprobeMetadataKind,
) -> Vec<PropertyDetailGroup> {
    let mut builder = FfprobeDetailBuilder::default();
    add_format_details(&mut builder, probe, kind);
    add_stream_details(&mut builder, probe, kind);
    add_chapter_details(&mut builder, probe);
    add_unknown_ffprobe_details(&mut builder, probe, kind);
    builder.into_groups()
}

fn add_format_details(
    builder: &mut FfprobeDetailBuilder,
    probe: &serde_json::Value,
    kind: FfprobeMetadataKind,
) {
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
    match kind {
        FfprobeMetadataKind::Audio => push_audio_format_tag_details(builder, format, probe),
        FfprobeMetadataKind::Video => {
            let embedded_title = builder.tag_field(format, "format", "title");
            builder.push(
                PropertyDetailGroupKind::Media,
                "Embedded title",
                embedded_title,
            );
            push_format_tag_details(builder, format);
        }
    }

    let counts = ffprobe_stream_counts(probe, kind);
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

fn push_format_tag_details(
    builder: &mut FfprobeDetailBuilder,
    format: &serde_json::Map<String, serde_json::Value>,
) {
    for (label, keys) in [
        ("Artist", &["artist"][..]),
        ("Album artist", &["album_artist", "albumartist"]),
        ("Album", &["album"][..]),
        ("Year", &["date", "year"]),
        ("Genre", &["genre"][..]),
        ("Track", &["track", "tracknumber"]),
        ("Disc", &["disc", "discnumber"]),
        ("Composer", &["composer"][..]),
    ] {
        let value = builder.tag_field_from_keys(format, "format", keys);
        builder.push(PropertyDetailGroupKind::Media, label, value);
    }
}

fn push_audio_format_tag_details(
    builder: &mut FfprobeDetailBuilder,
    format: &serde_json::Map<String, serde_json::Value>,
    probe: &serde_json::Value,
) {
    let mut consumed_keys = BTreeSet::new();
    let tags = format.get("tags").and_then(|tags| tags.as_object());

    if let Some(tags) = tags {
        for (label, keys) in [
            ("Title", &["title"][..]),
            ("Artist", &["artist"][..]),
            (
                "Album artist",
                &["album_artist", "albumartist", "album artist"],
            ),
            ("Album", &["album"][..]),
            ("Year", &["date", "year"]),
            ("Track", &["track", "tracknumber", "track_number"]),
            ("Discnumber", &["discnumber", "disc", "disc_number"]),
            ("Genre", &["genre"][..]),
            ("Comment", &["comment"][..]),
            ("Composer", &["composer"][..]),
        ] {
            let value = audio_tag_value(builder, tags, keys, &mut consumed_keys);
            builder.push(PropertyDetailGroupKind::Tags, label, value);
        }
    }

    builder.push(
        PropertyDetailGroupKind::Tags,
        "Cover",
        Some(cover_count_label(attached_picture_stream_count(probe))),
    );

    let Some(tags) = tags else {
        return;
    };
    for (key, value) in tags {
        if consumed_keys.contains(key) {
            continue;
        }
        builder.used_paths.insert(format!("format.tags.{key}"));
        if let Some(value) = ffprobe_scalar_value_label(value) {
            builder.push(
                PropertyDetailGroupKind::Tags,
                humanized_metadata_key(key),
                Some(value),
            );
        }
    }
}

fn audio_tag_value(
    builder: &mut FfprobeDetailBuilder,
    tags: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
    consumed_keys: &mut BTreeSet<String>,
) -> Option<String> {
    let mut selected = None;
    for key in keys {
        for (actual_key, value) in tags
            .iter()
            .filter(|(actual_key, _)| actual_key.eq_ignore_ascii_case(key))
        {
            consumed_keys.insert(actual_key.clone());
            builder
                .used_paths
                .insert(format!("format.tags.{actual_key}"));
            if selected.is_none() {
                selected = ffprobe_scalar_value_label(value);
            }
        }
    }
    selected
}

fn cover_count_label(count: usize) -> String {
    if count == 0 {
        "No (0)".to_owned()
    } else {
        format!("Yes ({count})")
    }
}

fn add_stream_details(
    builder: &mut FfprobeDetailBuilder,
    probe: &serde_json::Value,
    kind: FfprobeMetadataKind,
) {
    let Some(streams) = probe.get("streams").and_then(|streams| streams.as_array()) else {
        return;
    };

    let counts = ffprobe_stream_counts(probe, kind);
    let mut video_count = 0usize;
    let mut audio_count = 0usize;
    let mut subtitle_count = 0usize;
    let mut other_count = 0usize;
    for (index, stream_value) in streams.iter().enumerate() {
        let Some(stream) = stream_value.as_object() else {
            continue;
        };
        if kind == FfprobeMetadataKind::Audio && is_attached_picture_stream(stream) {
            continue;
        }
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
    builder: &mut FfprobeDetailBuilder,
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
    builder: &mut FfprobeDetailBuilder,
    stream: &serde_json::Map<String, serde_json::Value>,
    base_path: &str,
    label: Option<&str>,
) {
    let codec_name = builder.scalar_field(stream, base_path, "codec_name");
    let codec_long_name = builder.scalar_field(stream, base_path, "codec_long_name");
    let compression = codec_name.as_deref().and_then(audio_compression_label);
    builder.push(
        PropertyDetailGroupKind::Audio,
        stream_detail_name(label, "Codec"),
        metadata_name_label(codec_long_name, codec_name),
    );
    builder.push(
        PropertyDetailGroupKind::Audio,
        stream_detail_name(label, "Compression"),
        compression.map(str::to_owned),
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
    let bit_depth = builder
        .scalar_field_from_keys(
            stream,
            base_path,
            &[
                "bits_per_sample",
                "bits_per_raw_sample",
                "bits_per_coded_sample",
            ],
        )
        .as_deref()
        .and_then(format_bit_depth_label);
    builder.push(
        PropertyDetailGroupKind::Audio,
        stream_detail_name(label, "Bit depth"),
        bit_depth,
    );
    let sample_format = builder.scalar_field(stream, base_path, "sample_fmt");
    builder.push(
        PropertyDetailGroupKind::Audio,
        stream_detail_name(label, "Sample format"),
        sample_format,
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
    let disposition = disposition_label(builder, stream, base_path);
    builder.push(
        PropertyDetailGroupKind::Audio,
        stream_detail_name(label, "Disposition"),
        disposition,
    );
}

fn add_subtitle_stream_details(
    builder: &mut FfprobeDetailBuilder,
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
    builder: &mut FfprobeDetailBuilder,
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

fn add_chapter_details(builder: &mut FfprobeDetailBuilder, probe: &serde_json::Value) {
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

fn add_unknown_ffprobe_details(
    builder: &mut FfprobeDetailBuilder,
    probe: &serde_json::Value,
    kind: FfprobeMetadataKind,
) {
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
            if kind == FfprobeMetadataKind::Audio
                && stream.as_object().is_some_and(is_attached_picture_stream)
            {
                continue;
            }
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
    builder: &mut FfprobeDetailBuilder,
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

fn ffprobe_stream_counts(
    probe: &serde_json::Value,
    kind: FfprobeMetadataKind,
) -> FfprobeStreamCounts {
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
        if kind == FfprobeMetadataKind::Audio
            && stream.as_object().is_some_and(is_attached_picture_stream)
        {
            continue;
        }
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

fn attached_picture_stream_count(probe: &serde_json::Value) -> usize {
    probe
        .get("streams")
        .and_then(|streams| streams.as_array())
        .map_or(0, |streams| {
            streams
                .iter()
                .filter(|stream| stream.as_object().is_some_and(is_attached_picture_stream))
                .count()
        })
}

fn is_attached_picture_stream(stream: &serde_json::Map<String, serde_json::Value>) -> bool {
    stream
        .get("disposition")
        .and_then(|disposition| disposition.as_object())
        .and_then(|disposition| disposition.get("attached_pic"))
        .and_then(ffprobe_integer_value)
        == Some(1)
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

fn format_bit_depth_label(value: &str) -> Option<String> {
    let bits = value.trim().parse::<u64>().ok()?;
    Some(format!("{bits} bit"))
}

fn channels_label(channels: u64) -> String {
    format!(
        "{channels} {}",
        if channels == 1 { "channel" } else { "channels" }
    )
}

fn audio_compression_label(codec_name: &str) -> Option<&'static str> {
    let codec = codec_name.trim().to_ascii_lowercase();
    if codec.starts_with("pcm_")
        || matches!(
            codec.as_str(),
            "flac" | "alac" | "ape" | "wavpack" | "tta" | "tak" | "truehd" | "mlp"
        )
    {
        return Some("Lossless");
    }

    if codec.starts_with("wma")
        || codec.starts_with("amr")
        || codec.starts_with("ra_")
        || matches!(
            codec.as_str(),
            "mp3" | "aac" | "ac3" | "eac3" | "dts" | "opus" | "vorbis" | "mp2"
        )
    {
        return Some("Lossy");
    }

    None
}

fn disposition_label(
    builder: &mut FfprobeDetailBuilder,
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

fn ffprobe_integer_value(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(value) => value.as_u64(),
        serde_json::Value::String(value) => value.trim().parse::<u64>().ok(),
        _ => None,
    }
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
            return EXIF_VALUE_TOO_LARGE_LABEL.to_owned();
        }
        if label.write_str(value).is_err() {
            return EXIF_VALUE_TOO_LARGE_LABEL.to_owned();
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
            return EXIF_VALUE_TOO_LARGE_LABEL.to_owned();
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
        run_as_admin: (draft.run_as_admin != baseline.run_as_admin)
            .then_some(draft.run_as_admin)
            .flatten(),
    }
}

fn property_apply_plan_is_empty(plan: &EditablePropertyDraft) -> bool {
    plan.modified.is_none()
        && plan.accessed.is_none()
        && plan.readonly.is_none()
        && plan.hidden.is_none()
        && plan.unix_mode.is_none()
        && plan.run_as_admin.is_none()
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
    if let Some(run_as_admin) = draft.run_as_admin {
        apply_run_as_admin(path, run_as_admin)?;
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

#[cfg(target_os = "windows")]
fn apply_run_as_admin(path: &Path, enabled: bool) -> Result<(), String> {
    set_windows_run_as_admin_flag(path, enabled).map_err(|error| error.to_string())
}

#[cfg(not(target_os = "windows"))]
fn apply_run_as_admin(_: &Path, _: bool) -> Result<(), String> {
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

fn single_file_run_as_admin_path(
    target: &PropertyTarget,
    item_kind: PropertyItemKind,
) -> Option<&Path> {
    matches!(item_kind, PropertyItemKind::SingleFile)
        .then(|| target.paths.first().map(PathBuf::as_path))
        .flatten()
        .filter(|path| path_is_windows_executable(path))
        .filter(|_| cfg!(target_os = "windows"))
}

fn property_run_as_admin_value(path: &Path, entry: Option<&FileEntry>) -> Option<bool> {
    if !cfg!(target_os = "windows")
        || !entry.is_some_and(|entry| entry.is_open_with_target())
        || !path_is_windows_executable(path)
    {
        return None;
    }

    #[cfg(target_os = "windows")]
    {
        return Some(windows_run_as_admin_flag(path).unwrap_or(false));
    }

    #[allow(unreachable_code)]
    None
}

fn path_is_windows_executable(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

const WINDOWS_COMPATIBILITY_RUN_AS_ADMIN_FLAG: &str = "RUNASADMIN";
const WINDOWS_COMPATIBILITY_DEFAULT_PREFIX: &str = "~";

fn windows_compatibility_value_has_run_as_admin(value: &str) -> bool {
    value
        .split_whitespace()
        .any(|token| token.eq_ignore_ascii_case(WINDOWS_COMPATIBILITY_RUN_AS_ADMIN_FLAG))
}

fn windows_compatibility_value_with_run_as_admin(
    current: Option<&str>,
    enabled: bool,
) -> Option<String> {
    let mut tokens = current
        .map(|value| {
            value
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if enabled {
        if tokens.is_empty() {
            tokens.push(WINDOWS_COMPATIBILITY_DEFAULT_PREFIX.to_owned());
        }
        if !tokens
            .iter()
            .any(|token| token.eq_ignore_ascii_case(WINDOWS_COMPATIBILITY_RUN_AS_ADMIN_FLAG))
        {
            tokens.push(WINDOWS_COMPATIBILITY_RUN_AS_ADMIN_FLAG.to_owned());
        }
        return Some(tokens.join(" "));
    }

    tokens.retain(|token| !token.eq_ignore_ascii_case(WINDOWS_COMPATIBILITY_RUN_AS_ADMIN_FLAG));
    tokens
        .iter()
        .any(|token| token != WINDOWS_COMPATIBILITY_DEFAULT_PREFIX)
        .then(|| tokens.join(" "))
}

#[cfg(target_os = "windows")]
const WINDOWS_COMPATIBILITY_LAYERS_KEY: &str =
    r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers";

#[cfg(target_os = "windows")]
fn windows_run_as_admin_flag(path: &Path) -> std::io::Result<bool> {
    Ok(windows_compatibility_registry_value(path)?
        .as_deref()
        .is_some_and(windows_compatibility_value_has_run_as_admin))
}

#[cfg(target_os = "windows")]
fn set_windows_run_as_admin_flag(path: &Path, enabled: bool) -> std::io::Result<()> {
    let current = windows_compatibility_registry_value(path)?;
    match windows_compatibility_value_with_run_as_admin(current.as_deref(), enabled) {
        Some(value) => windows_write_compatibility_registry_value(path, &value),
        None => windows_delete_compatibility_registry_value(path),
    }
}

#[cfg(target_os = "windows")]
fn windows_compatibility_registry_value(path: &Path) -> std::io::Result<Option<String>> {
    use windows::Win32::{
        Foundation::ERROR_FILE_NOT_FOUND,
        System::Registry::{HKEY_CURRENT_USER, REG_VALUE_TYPE, RRF_RT_REG_SZ, RegGetValueW},
    };
    use windows::core::PCWSTR;

    let key = windows_wide_null_str(WINDOWS_COMPATIBILITY_LAYERS_KEY);
    let value_name = windows_wide_null_path(path);
    let mut value_type = REG_VALUE_TYPE(0);
    let mut byte_len = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            Some(&mut value_type),
            None,
            Some(&mut byte_len),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    windows_registry_status(status)?;
    if byte_len < 2 {
        return Ok(Some(String::new()));
    }

    let mut buffer = vec![0u16; byte_len.div_ceil(2) as usize];
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            Some(&mut value_type),
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut byte_len),
        )
    };
    windows_registry_status(status)?;

    let mut char_len = (byte_len / 2) as usize;
    if char_len > 0 && buffer.get(char_len - 1) == Some(&0) {
        char_len -= 1;
    }
    String::from_utf16(&buffer[..char_len])
        .map(Some)
        .map_err(std::io::Error::other)
}

#[cfg(target_os = "windows")]
fn windows_write_compatibility_registry_value(path: &Path, value: &str) -> std::io::Result<()> {
    use windows::Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
            RegCreateKeyExW, RegSetValueExW,
        },
    };
    use windows::core::PCWSTR;

    let key_path = windows_wide_null_str(WINDOWS_COMPATIBILITY_LAYERS_KEY);
    let value_name = windows_wide_null_path(path);
    let mut key = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    };
    windows_registry_status(status)?;

    let data = windows_utf16_bytes(value);
    let status =
        unsafe { RegSetValueExW(key, PCWSTR(value_name.as_ptr()), None, REG_SZ, Some(&data)) };
    let close_status = unsafe { RegCloseKey(key) };
    windows_registry_status(status)?;
    if close_status != ERROR_SUCCESS {
        windows_registry_status(close_status)?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_delete_compatibility_registry_value(path: &Path) -> std::io::Result<()> {
    use windows::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, RegCloseKey, RegDeleteValueW, RegOpenKeyExW,
        },
    };
    use windows::core::PCWSTR;

    let key_path = windows_wide_null_str(WINDOWS_COMPATIBILITY_LAYERS_KEY);
    let value_name = windows_wide_null_path(path);
    let mut key = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_path.as_ptr()),
            None,
            KEY_SET_VALUE,
            &mut key,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    windows_registry_status(status)?;

    let status = unsafe { RegDeleteValueW(key, PCWSTR(value_name.as_ptr())) };
    let close_status = unsafe { RegCloseKey(key) };
    if status != ERROR_FILE_NOT_FOUND {
        windows_registry_status(status)?;
    }
    if close_status != ERROR_SUCCESS {
        windows_registry_status(close_status)?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_registry_status(status: windows::Win32::Foundation::WIN32_ERROR) -> std::io::Result<()> {
    use windows::Win32::Foundation::ERROR_SUCCESS;

    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(status.0 as i32))
    }
}

#[cfg(target_os = "windows")]
fn windows_wide_null_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(target_os = "windows")]
fn windows_wide_null_str(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
fn windows_utf16_bytes(value: &str) -> Vec<u8> {
    value
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn default_app_change_error(
    path: &Path,
    before: &Option<PropertyDefaultApp>,
    result: &std::io::Result<DefaultAppChangeOutcome>,
    snapshot: Option<&PropertySnapshot>,
) -> Option<String> {
    match result {
        Err(error) => Some(format!(
            "Could not change the default app for {}: {error}",
            property_path_display_name(path)
        )),
        Ok(DefaultAppChangeOutcome::Cancelled) => None,
        Ok(DefaultAppChangeOutcome::Changed) => {
            let Some(snapshot) = snapshot else {
                return Some(format!(
                    "The default app for {} could not be verified.",
                    property_path_display_name(path)
                ));
            };
            if snapshot.default_app.is_none() {
                Some(format!(
                    "No default app for {} could be verified.",
                    property_path_display_name(path)
                ))
            } else if &snapshot.default_app != before {
                None
            } else {
                Some(format!(
                    "The default app for {} did not appear to change.",
                    property_path_display_name(path)
                ))
            }
        }
    }
}

fn default_app_change_refreshes_file_type_icons(
    result: &std::io::Result<DefaultAppChangeOutcome>,
) -> bool {
    matches!(result, Ok(DefaultAppChangeOutcome::Changed))
}

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

fn selection_count_value_label(counts: &PropertyValue<PropertyContains>) -> String {
    match counts {
        PropertyValue::Loading => PROPERTIES_CALCULATING_LABEL.to_owned(),
        PropertyValue::Ready(counts) => selection_count_label(counts),
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

fn contains_value_label(contains: &PropertyValue<PropertyContains>) -> String {
    match contains {
        PropertyValue::Loading => PROPERTIES_CALCULATING_LABEL.to_owned(),
        PropertyValue::Ready(contains) => contains_label(contains),
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

fn property_size_value_label(size: &PropertyValue<u64>) -> String {
    match size {
        PropertyValue::Loading => PROPERTIES_CALCULATING_LABEL.to_owned(),
        PropertyValue::Ready(size) => property_size_label(*size),
    }
}

fn property_size_label(size: u64) -> String {
    let label = format_size(Some(size));
    if label.ends_with(" bytes") {
        label
    } else {
        format!("{label} ({} bytes)", size.separate_with_commas())
    }
}

fn snapshot_needs_tree_summary(snapshot: &PropertySnapshot) -> bool {
    snapshot.size.is_loading()
        || snapshot.size_on_disk.is_loading()
        || snapshot
            .contains
            .as_ref()
            .is_some_and(PropertyValue::is_loading)
        || snapshot
            .selection_counts
            .as_ref()
            .is_some_and(PropertyValue::is_loading)
}

fn detail_groups_for_render(
    snapshot: &PropertySnapshot,
    details_state: &PropertyDetailsState,
) -> Vec<PropertyDetailGroup> {
    let mut groups = match details_state {
        PropertyDetailsState::Ready(extra_groups) => {
            merge_detail_groups([snapshot.details.as_slice(), extra_groups.as_slice()])
        }
        PropertyDetailsState::NotStarted | PropertyDetailsState::Loading => {
            snapshot.details.clone()
        }
    };
    groups.sort_by_key(|group| group.kind);
    groups
}

impl PropertyDetailsRenderCacheKey {
    fn new(snapshot_generation: u64, details_state: &PropertyDetailsState) -> Self {
        Self {
            snapshot_generation,
            extra_details_ready: matches!(details_state, PropertyDetailsState::Ready(_)),
        }
    }
}

fn detail_groups_for_render_cached<'a>(
    cache_key: &mut Option<PropertyDetailsRenderCacheKey>,
    cache: &'a mut Vec<PropertyDetailGroup>,
    snapshot: &PropertySnapshot,
    details_state: &PropertyDetailsState,
    snapshot_generation: u64,
) -> &'a [PropertyDetailGroup] {
    let key = PropertyDetailsRenderCacheKey::new(snapshot_generation, details_state);
    if cache_key.as_ref() != Some(&key) {
        *cache = detail_groups_for_render(snapshot, details_state);
        *cache_key = Some(key);
    }

    cache
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

fn adjacent_property_tab(
    active_tab: PropertyTab,
    snapshot: Option<&PropertySnapshot>,
    direction: PropertyTabDirection,
) -> Option<PropertyTab> {
    let visible_tabs = property_tabs_for_snapshot(snapshot);
    if visible_tabs.len() <= 1 {
        return None;
    }

    let active_index = visible_tabs
        .iter()
        .position(|(tab, _)| *tab == active_tab)
        .unwrap_or(0);
    let target_index = match direction {
        PropertyTabDirection::Previous => active_index
            .checked_sub(1)
            .unwrap_or(visible_tabs.len() - 1),
        PropertyTabDirection::Next => (active_index + 1) % visible_tabs.len(),
    };

    Some(visible_tabs[target_index].0)
}

fn property_tab_is_visible(tab: PropertyTab, snapshot: Option<&PropertySnapshot>) -> bool {
    match tab {
        PropertyTab::General | PropertyTab::Details => true,
        PropertyTab::Cover => snapshot.is_some_and(snapshot_has_cover_tab),
        PropertyTab::Spectrum => snapshot.is_some_and(snapshot_has_spectrum_tab),
        PropertyTab::Code => snapshot.is_some_and(snapshot_has_code_tab),
        PropertyTab::Image => snapshot.is_some_and(snapshot_has_image_tab),
        PropertyTab::Frames => snapshot.is_some_and(snapshot_has_frames_tab),
    }
}

fn snapshot_has_code_tab(snapshot: &PropertySnapshot) -> bool {
    single_folder_direct_git_repository_root(&snapshot.target, snapshot.item_kind).is_some()
}

fn snapshot_has_image_tab(snapshot: &PropertySnapshot) -> bool {
    single_file_image_path(&snapshot.target, snapshot.item_kind).is_some()
}

fn snapshot_has_cover_tab(snapshot: &PropertySnapshot) -> bool {
    single_file_audio_path(&snapshot.target, snapshot.item_kind).is_some()
}

fn snapshot_has_spectrum_tab(snapshot: &PropertySnapshot) -> bool {
    single_file_audio_path(&snapshot.target, snapshot.item_kind).is_some()
}

fn snapshot_has_frames_tab(snapshot: &PropertySnapshot) -> bool {
    single_file_video_path(&snapshot.target, snapshot.item_kind).is_some()
}

fn snapshot_has_run_as_admin_setting(snapshot: &PropertySnapshot) -> bool {
    single_file_run_as_admin_path(&snapshot.target, snapshot.item_kind).is_some()
}

fn property_scrollbar_metrics_for_dimensions(
    viewport_height: f32,
    scroll_max: f32,
    scroll_top: f32,
) -> Option<ScrollbarMetrics> {
    if scroll_max <= 0.0 {
        return None;
    }

    ScrollbarMetrics::new(
        0.0,
        viewport_height,
        viewport_height + scroll_max,
        scroll_top,
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PropertyPreviewSize {
    width: f32,
    height: f32,
}

fn property_cover_preview_max_size(window: &Window) -> PropertyPreviewSize {
    let bounds = window.bounds().size;
    let width = f32::from(bounds.width)
        - (PROPERTIES_PADDING * 2.0)
        - (PROPERTIES_BORDER_WIDTH * 2.0)
        - (PROPERTIES_PANEL_PADDING * 2.0);
    let height = f32::from(bounds.height)
        - (PROPERTIES_PADDING * 2.0)
        - PROPERTIES_TAB_HEIGHT
        - PROPERTIES_BORDER_WIDTH
        - PROPERTIES_BORDER_WIDTH
        - property_button_row_height()
        - (PROPERTIES_PANEL_PADDING * 2.0)
        - PROPERTIES_COVER_NAVIGATION_HEIGHT;

    PropertyPreviewSize {
        width: width.max(0.0),
        height: height.max(0.0),
    }
}

fn property_button_row_height() -> f32 {
    PROPERTIES_BUTTON_ROW_TOP_PADDING + PROPERTIES_BUTTON_HEIGHT
}

fn property_preview_fit_rect(
    source_width: u32,
    source_height: u32,
    max_width: f32,
    max_height: f32,
) -> Option<PropertyPreviewSize> {
    if source_width == 0
        || source_height == 0
        || !max_width.is_finite()
        || !max_height.is_finite()
        || max_width <= 0.0
        || max_height <= 0.0
    {
        return None;
    }

    let width = source_width as f32;
    let height = source_height as f32;
    let scale = (max_width / width).min(max_height / height).min(1.0);
    (scale.is_finite() && scale > 0.0).then_some(PropertyPreviewSize {
        width: width * scale,
        height: height * scale,
    })
}

fn property_image_preview(
    preview: &PropertyImagePreview,
    max_size: PropertyPreviewSize,
    cx: &mut Context<PropertiesDialog>,
) -> AnyElement {
    let payload = property_image_preview_copy_payload(preview);
    let fit = property_preview_fit_rect(
        preview.width,
        preview.height,
        max_size.width,
        max_size.height,
    );
    let preview = div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.open_image_copy_context_menu(payload.clone(), event.position, cx);
                cx.stop_propagation();
            }),
        )
        .when_some(fit, |this, fit| {
            this.child(
                div()
                    .w(px(fit.width))
                    .h(px(fit.height))
                    .overflow_hidden()
                    .child(property_image_preview_content(preview)),
            )
        });
    preview.into_any_element()
}

fn property_image_preview_content(preview: &PropertyImagePreview) -> AnyElement {
    let Some(source) = &preview.animated_source else {
        return gpui::img(preview.image.clone())
            .size_full()
            .object_fit(ObjectFit::Contain)
            .into_any_element();
    };

    let loading_image = preview.image.clone();
    let fallback_image = preview.image.clone();
    div()
        .debug_selector(|| "properties-image-animated-gif".to_owned())
        .size_full()
        .child(
            gpui::img(source.path.clone())
                .id("properties-image-animated-gif-image")
                .size_full()
                .object_fit(ObjectFit::Contain)
                .with_loading(move || {
                    gpui::img(loading_image.clone())
                        .size_full()
                        .object_fit(ObjectFit::Contain)
                        .into_any_element()
                })
                .with_fallback(move || {
                    gpui::img(fallback_image.clone())
                        .size_full()
                        .object_fit(ObjectFit::Contain)
                        .into_any_element()
                }),
        )
        .into_any_element()
}

fn frame_thumbnail_list(
    frames: &[PropertyFrameThumbnail],
    cx: &mut Context<PropertiesDialog>,
) -> AnyElement {
    let mut list = div()
        .flex()
        .flex_col()
        .gap(px(PROPERTIES_FRAME_LIST_GAP))
        .w_full()
        .min_w(px(0.0));
    for (index, frame) in frames.iter().enumerate() {
        list = list.child(frame_thumbnail_tile(index, frame, cx));
    }
    list.into_any_element()
}

fn frame_thumbnail_tile(
    index: usize,
    frame: &PropertyFrameThumbnail,
    cx: &mut Context<PropertiesDialog>,
) -> AnyElement {
    let payload = frame.copy_payload();
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
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.open_image_copy_context_menu(payload.clone(), event.position, cx);
                cx.stop_propagation();
            }),
        )
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

fn property_image_context_menu_height() -> f32 {
    PROPERTIES_IMAGE_CONTEXT_MENU_ROW_HEIGHT + PROPERTIES_IMAGE_CONTEXT_MENU_ROW_GAP + 8.0
}

fn property_image_context_menu_dropdown() -> gpui::Stateful<Div> {
    div()
        .id("properties-image-copy-context-menu")
        .debug_selector(|| "properties-image-copy-context-menu".to_owned())
        .w(px(PROPERTIES_IMAGE_CONTEXT_MENU_WIDTH))
        .py(px(4.0))
        .border_1()
        .border_color(rgb(0xd0d0d0))
        .rounded(px(6.0))
        .bg(rgb(0xffffff))
        .shadow_md()
        .occlude()
        .on_any_mouse_down(|_, _, cx| {
            cx.stop_propagation();
        })
}

fn property_image_context_menu_copy_row(
    payload: PropertyImageCopyPayload,
    cx: &mut Context<PropertiesDialog>,
) -> AnyElement {
    div()
        .id("properties-image-copy-context-menu-copy")
        .debug_selector(|| "properties-image-copy-context-menu-copy".to_owned())
        .flex()
        .flex_row()
        .items_center()
        .h(px(PROPERTIES_IMAGE_CONTEXT_MENU_ROW_HEIGHT))
        .px(px(PROPERTIES_IMAGE_CONTEXT_MENU_HORIZONTAL_PADDING / 2.0))
        .mx(px(PROPERTIES_IMAGE_CONTEXT_MENU_HORIZONTAL_PADDING / 2.0))
        .gap(px(PROPERTIES_IMAGE_CONTEXT_MENU_CHILD_GAP))
        .cursor_default()
        .hover(|style| style.bg(rgb(0xe5f3ff)))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.copy_property_image_payload_to_clipboard(payload.clone(), cx);
            this.close_image_copy_context_menu();
            cx.stop_propagation();
            cx.notify();
        }))
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(PROPERTIES_IMAGE_CONTEXT_MENU_ICON_SLOT_SIZE))
                .h(px(PROPERTIES_IMAGE_CONTEXT_MENU_ICON_SLOT_SIZE))
                .flex_shrink_0()
                .child(
                    gpui::img(COPY_ICON.clone())
                        .w(px(PROPERTIES_IMAGE_CONTEXT_MENU_ICON_SIZE))
                        .h(px(PROPERTIES_IMAGE_CONTEXT_MENU_ICON_SIZE)),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(PROPERTIES_IMAGE_CONTEXT_MENU_TEXT_SIZE))
                .text_color(rgb(0x1f1f1f))
                .child("Copy"),
        )
        .into_any_element()
}

fn clipboard_image_from_property_image_payload(
    payload: &PropertyImageCopyPayload,
) -> Result<Image, String> {
    if payload.width == 0 || payload.height == 0 {
        return Err("Image has no dimensions.".to_owned());
    }
    let bytes = payload
        .image
        .as_bytes(0)
        .ok_or_else(|| "Image render data is not available.".to_owned())?;
    let expected_len = (payload.width as usize)
        .checked_mul(payload.height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "Image dimensions are too large.".to_owned())?;
    if bytes.len() != expected_len {
        return Err(format!(
            "Image render data length {} does not match {}x{} RGBA data.",
            bytes.len(),
            payload.width,
            payload.height
        ));
    }

    let mut rgba = bytes.to_vec();
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let png = encode_property_rgba_png_bytes(&rgba, payload.width, payload.height)
        .ok_or_else(|| "Failed to encode copied image as PNG.".to_owned())?;
    Ok(Image::from_bytes(ImageFormat::Png, png))
}

fn encode_property_rgba_png_bytes(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut bytes,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::NoFilter,
    )
    .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
    .ok()?;
    bytes
        .starts_with(PROPERTIES_CLIPBOARD_IMAGE_PNG_SIGNATURE)
        .then_some(bytes)
}

#[derive(Clone, Copy)]
enum GitDivergenceSide {
    Outgoing,
    Incoming,
}

fn git_divergence_value(divergence: Option<GitDivergence>, side: GitDivergenceSide) -> String {
    let Some(divergence) = divergence else {
        return "Not configured".to_owned();
    };
    match side {
        GitDivergenceSide::Outgoing => divergence.outgoing,
        GitDivergenceSide::Incoming => divergence.incoming,
    }
    .separate_with_commas()
}

fn properties_code_makeup_bar(summary: &CodebaseSummary) -> AnyElement {
    let separator_count = summary.languages.len().saturating_sub(1);
    let separator_width = PROPERTIES_CODE_MAKEUP_SEPARATOR_WIDTH * separator_count as f32;
    let language_width = (PROPERTIES_CODE_MAKEUP_BAR_WIDTH - separator_width).max(0.0);
    let widths = language_segment_widths(&summary.languages, summary.total_code, language_width);
    let segments = summary
        .languages
        .iter()
        .zip(widths)
        .filter_map(|(language, width)| {
            (width > 0.0).then(|| {
                div()
                    .h_full()
                    .w(px(width))
                    .flex_shrink_0()
                    .bg(rgb(language.color))
                    .into_any_element()
            })
        })
        .collect::<Vec<_>>();

    div()
        .id("properties-code-language-makeup-bar")
        .mt(px(8.0))
        .mb(px(8.0))
        .h(px(PROPERTIES_CODE_MAKEUP_BAR_HEIGHT))
        .w(px(PROPERTIES_CODE_MAKEUP_BAR_WIDTH))
        .max_w_full()
        .flex()
        .flex_row()
        .gap(px(PROPERTIES_CODE_MAKEUP_SEPARATOR_WIDTH))
        .flex_shrink_0()
        .overflow_hidden()
        .rounded(px(PROPERTIES_CODE_MAKEUP_BAR_RADIUS))
        .bg(rgb(PROPERTIES_CODE_MAKEUP_SEPARATOR_COLOR))
        .children(segments)
        .into_any_element()
}

fn code_language_header() -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_w(px(0.0))
        .min_h(px(26.0))
        .border_b_1()
        .border_color(rgb(PROPERTIES_BORDER))
        .text_color(rgb(PROPERTIES_MUTED_TEXT))
        .child(
            div()
                .w(px(PROPERTIES_CODE_LANGUAGE_LABEL_WIDTH))
                .flex_shrink_0()
                .child("Language"),
        )
        .child(
            div()
                .w(px(PROPERTIES_CODE_LANGUAGE_LOC_WIDTH))
                .flex_shrink_0()
                .flex()
                .justify_end()
                .child("LoC"),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .justify_end()
                .child("Percentage"),
        )
        .into_any_element()
}

fn code_language_row(
    index: usize,
    language: &CodebaseLanguageSummary,
    cx: &mut Context<PropertiesDialog>,
) -> AnyElement {
    let copied_label = language.name.clone();
    let copied_value = format!(
        "{} LoC, {}%",
        language.code.separate_with_commas(),
        language.percentage
    );

    div()
        .id(("properties-code-language-row", index))
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
                .w(px(PROPERTIES_CODE_LANGUAGE_LABEL_WIDTH))
                .flex_shrink_0()
                .flex()
                .flex_row()
                .items_center()
                .min_w(px(0.0))
                .child(
                    div()
                        .w(px(PROPERTIES_CODE_LANGUAGE_SWATCH_SIZE))
                        .h(px(PROPERTIES_CODE_LANGUAGE_SWATCH_SIZE))
                        .mr(px(6.0))
                        .rounded(px(PROPERTIES_CODE_LANGUAGE_SWATCH_SIZE / 2.0))
                        .bg(rgb(language.color)),
                )
                .child(
                    div()
                        .min_w(px(0.0))
                        .truncate()
                        .child(SharedString::from(language.name.clone())),
                ),
        )
        .child(
            div()
                .w(px(PROPERTIES_CODE_LANGUAGE_LOC_WIDTH))
                .flex_shrink_0()
                .flex()
                .justify_end()
                .child(SharedString::from(language.code.separate_with_commas())),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .justify_end()
                .child(SharedString::from(format!("{}%", language.percentage))),
        )
        .into_any_element()
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
        PropertyTab::Cover => "properties-tab-cover",
        PropertyTab::Spectrum => "properties-tab-spectrum",
        PropertyTab::Code => "properties-tab-code",
        PropertyTab::Image => "properties-tab-image",
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

#[derive(Clone, Copy)]
struct PropertyRowStyle {
    label_width: f32,
    truncate_label: bool,
}

fn copyable_property_row_base(
    row: gpui::Stateful<Div>,
    label: String,
    value: String,
    style: PropertyRowStyle,
    cx: &mut Context<PropertiesDialog>,
) -> AnyElement {
    let copied_label = label.clone();
    let copied_value = value.clone();
    let label_cell = div()
        .w(px(style.label_width))
        .flex_shrink_0()
        .text_color(rgb(PROPERTIES_MUTED_TEXT))
        .when(style.truncate_label, |this| this.truncate())
        .child(SharedString::from(label));

    row.flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_w(px(0.0))
        .min_h(px(PROPERTIES_ROW_HEIGHT))
        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
            copy_property_to_clipboard(&copied_label, &copied_value, cx);
            cx.stop_propagation();
        }))
        .child(label_cell)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .child(SharedString::from(value)),
        )
        .into_any_element()
}

fn property_row(
    id: &'static str,
    label: impl Into<String>,
    value: impl Into<String>,
    cx: &mut Context<PropertiesDialog>,
) -> AnyElement {
    copyable_property_row_base(
        div().id(id),
        label.into(),
        value.into(),
        PropertyRowStyle {
            label_width: PROPERTIES_LABEL_WIDTH,
            truncate_label: false,
        },
        cx,
    )
}

fn detail_row(
    index: usize,
    label: &str,
    value: &str,
    cx: &mut Context<PropertiesDialog>,
) -> AnyElement {
    copyable_property_row_base(
        div().id(detail_row_id(index)),
        label.to_owned(),
        value.to_owned(),
        PropertyRowStyle {
            label_width: 154.0,
            truncate_label: true,
        },
        cx,
    )
}

fn push_checksum_detail_rows(
    state: &PropertyChecksumState,
    children: &mut Vec<AnyElement>,
    detail_row_index: &mut usize,
    cx: &mut Context<PropertiesDialog>,
) {
    match state {
        PropertyChecksumState::NotRequested => {
            children.push(checksum_action_row("Not calculated", Some("Calculate"), cx));
        }
        PropertyChecksumState::Loading => {
            children.push(checksum_action_row("Calculating...", None, cx));
        }
        PropertyChecksumState::Ready(checksums) => {
            children.push(detail_row(*detail_row_index, "CRC32", &checksums.crc32, cx));
            *detail_row_index += 1;
            children.push(detail_row(
                *detail_row_index,
                "SHA256",
                &checksums.sha256,
                cx,
            ));
            *detail_row_index += 1;
        }
        PropertyChecksumState::Failed(error) => {
            children.push(checksum_action_row(
                &format!("Could not calculate checksums: {error}"),
                Some("Retry"),
                cx,
            ));
        }
    }
}

fn checksum_action_row(
    value: &str,
    button_label: Option<&'static str>,
    cx: &mut Context<PropertiesDialog>,
) -> AnyElement {
    let mut row = div()
        .id("properties-detail-checksums-row")
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_w(px(0.0))
        .min_h(px(PROPERTIES_ROW_HEIGHT))
        .child(
            div()
                .w(px(154.0))
                .flex_shrink_0()
                .text_color(rgb(PROPERTIES_MUTED_TEXT))
                .truncate()
                .child("Checksums"),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .child(SharedString::from(value.to_owned())),
        );

    if let Some(button_label) = button_label {
        row = row.child(
            property_button("properties-calculate-checksums", button_label, true, 1.0).on_click(
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.start_checksum_task(cx);
                    cx.stop_propagation();
                }),
            ),
        );
    }

    row.into_any_element()
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

fn cover_navigation_button(
    id: &'static str,
    icon: NavIcon,
    tooltip: &'static str,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .w(px(UTILITY_ICON_BUTTON_SIZE))
        .h(px(UTILITY_ICON_BUTTON_SIZE))
        .rounded(px(4.0))
        .when(enabled, |this| {
            this.hover(|style| style.bg(rgb(NAV_BUTTON_HOVER_BG)))
                .active(|style| style.opacity(NAV_BUTTON_ACTIVE_OPACITY))
                .on_click(on_click)
        })
        .flex()
        .items_center()
        .justify_center()
        .cursor_default()
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

#[cfg(feature = "benchmarks")]
pub mod benchmark_support {
    use std::path::Path;
    use std::sync::atomic::AtomicBool;

    use super::{
        PropertyTarget, PropertyValue, collect_property_snapshot_fast_with_date_format,
        collect_property_snapshot_full_with_date_format, prepare_video_frame_requests,
    };

    pub fn collect_properties_fast(path: &Path) -> u64 {
        collect_property_snapshot_fast_with_date_format(
            PropertyTarget {
                paths: vec![path.to_path_buf()],
            },
            crate::settings::DEFAULT_DATE_FORMAT,
        )
        .map(snapshot_fingerprint)
        .unwrap_or_default()
    }

    pub fn collect_properties_full(path: &Path) -> u64 {
        let cancel = AtomicBool::new(false);
        collect_property_snapshot_full_with_date_format(
            PropertyTarget {
                paths: vec![path.to_path_buf()],
            },
            crate::settings::DEFAULT_DATE_FORMAT,
            &cancel,
        )
        .map(snapshot_fingerprint)
        .unwrap_or_default()
    }

    pub fn load_video_properties_frames_for_benchmark(path: &Path) -> usize {
        let Ok(requests) = prepare_video_frame_requests(path) else {
            return 0;
        };
        let seeks = requests
            .iter()
            .map(|request| request.seek_seconds)
            .collect::<Vec<_>>();
        let mut frames = 0usize;
        crate::explorer::video_thumbnails::extract_video_frame_batch(
            path,
            &seeks,
            &AtomicBool::new(false),
            |_, _| frames += 1,
        )
        .map(|_| frames)
        .unwrap_or_default()
    }

    fn snapshot_fingerprint(snapshot: super::PropertySnapshot) -> u64 {
        value_fingerprint(&snapshot.size)
            .saturating_add(value_fingerprint(&snapshot.size_on_disk))
            .saturating_add(
                snapshot
                    .contains
                    .as_ref()
                    .map(counts_fingerprint)
                    .unwrap_or_default(),
            )
            .saturating_add(
                snapshot
                    .selection_counts
                    .as_ref()
                    .map(counts_fingerprint)
                    .unwrap_or_default(),
            )
    }

    fn value_fingerprint(value: &PropertyValue<u64>) -> u64 {
        match value {
            PropertyValue::Loading => 1,
            PropertyValue::Ready(value) => value.saturating_add(2),
        }
    }

    fn counts_fingerprint(value: &PropertyValue<super::PropertyContains>) -> u64 {
        match value {
            PropertyValue::Loading => 3,
            PropertyValue::Ready(counts) => (counts.files as u64)
                .saturating_mul(31)
                .saturating_add((counts.folders as u64).saturating_mul(17)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;
    use crate::settings::{ExplorerSettings, SettingsState};
    use git2::{Commit, Oid, Repository, Signature};
    use std::{collections::HashSet, io::Cursor, time::Duration};

    fn write_test_png(path: &Path, width: u32, height: u32) {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(width, height));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn open_properties_target_uses_selected_entries() {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        fs::write(&file, b"abc").unwrap();
        fs::write(temp.path().join("b.txt"), b"def").unwrap();

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        let index = view.entry_index_by_path(&file).unwrap();
        view.select_single_index(index);

        assert_eq!(view.selected_or_current_property_paths(), vec![file]);
    }

    #[test]
    fn open_properties_target_falls_back_to_current_directory() {
        let temp = TempDir::new();
        fs::write(temp.path().join("a.txt"), b"abc").unwrap();
        let path = temp.path().to_path_buf();
        let view = ExplorerView::new(path.clone());

        assert_eq!(view.selected_or_current_property_paths(), vec![path]);
    }

    #[test]
    fn properties_window_bounds_centers_on_parent_window() {
        let parent_bounds = Bounds::new(point(px(100.0), px(80.0)), size(px(1000.0), px(700.0)));
        let expected = Bounds::new(
            point(px(396.0), px(170.0)),
            size(px(PROPERTIES_WIDTH), px(PROPERTIES_HEIGHT)),
        );

        assert_eq!(
            properties_window_bounds(parent_bounds),
            WindowBounds::Windowed(expected)
        );
    }

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
        assert_eq!(snapshot.size, PropertyValue::Ready(3));
        assert!(snapshot.size_on_disk.as_ready().unwrap() >= snapshot.size.as_ready().unwrap());
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
            Some(PropertyValue::Ready(PropertyContains {
                files: 2,
                folders: 2
            }))
        );
        assert_eq!(snapshot.size, PropertyValue::Ready(3));
        assert!(snapshot.size_on_disk.as_ready().unwrap() >= snapshot.size.as_ready().unwrap());
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
            Some(PropertyValue::Ready(PropertyContains {
                files: 2,
                folders: 2
            }))
        );
        assert!(snapshot.contains.is_none());
        assert_eq!(snapshot.size, PropertyValue::Ready(3));
        assert!(snapshot.size_on_disk.as_ready().unwrap() >= snapshot.size.as_ready().unwrap());
        assert_eq!(
            selection_count_value_label(snapshot.selection_counts.as_ref().unwrap()),
            "2 Files, 2 Folders"
        );
        assert_eq!(type_of_file_label(&snapshot), "Multiple Types");
        assert_eq!(
            location_label(&snapshot),
            Some(format!("All in {}", temp.path().display()))
        );
    }

    #[test]
    fn fast_folder_snapshot_marks_recursive_totals_loading() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        let child = folder.join("child");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("a.txt"), b"a").unwrap();
        fs::create_dir(&child).unwrap();
        fs::write(child.join("b.txt"), b"bb").unwrap();

        let snapshot = collect_property_snapshot_fast_with_date_format(
            PropertyTarget {
                paths: vec![folder],
            },
            crate::settings::DEFAULT_DATE_FORMAT,
        )
        .unwrap();

        assert_eq!(snapshot.item_kind, PropertyItemKind::SingleFolder);
        assert_eq!(snapshot.size, PropertyValue::Loading);
        assert_eq!(snapshot.size_on_disk, PropertyValue::Loading);
        assert_eq!(snapshot.contains, Some(PropertyValue::Loading));
        assert!(snapshot.selection_counts.is_none());
        assert_eq!(property_size_value_label(&snapshot.size), "Calculating...");
        assert_eq!(
            contains_value_label(snapshot.contains.as_ref().unwrap()),
            "Calculating..."
        );
        assert_eq!(
            detail_value(&snapshot.details, PropertyDetailGroupKind::File, "Size"),
            None
        );
    }

    #[test]
    fn fast_multiselect_marks_counts_loading_when_folder_selected() {
        let temp = TempDir::new();
        let file = temp.path().join("root.txt");
        let folder = temp.path().join("folder");
        fs::write(&file, b"a").unwrap();
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("inside.txt"), b"bb").unwrap();

        let snapshot = collect_property_snapshot_fast_with_date_format(
            PropertyTarget {
                paths: vec![file, folder],
            },
            crate::settings::DEFAULT_DATE_FORMAT,
        )
        .unwrap();

        assert_eq!(snapshot.item_kind, PropertyItemKind::MultipleItems);
        assert_eq!(snapshot.size, PropertyValue::Loading);
        assert_eq!(snapshot.selection_counts, Some(PropertyValue::Loading));
        assert!(snapshot.contains.is_none());
        assert_eq!(
            selection_count_value_label(snapshot.selection_counts.as_ref().unwrap()),
            "Calculating..."
        );
    }

    #[test]
    fn full_tree_summary_respects_pre_cancelled_flag() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("a.txt"), b"a").unwrap();
        let cancel = AtomicBool::new(true);

        let error = collect_property_snapshot_full_with_date_format(
            PropertyTarget {
                paths: vec![folder],
            },
            crate::settings::DEFAULT_DATE_FORMAT,
            &cancel,
        )
        .unwrap_err();

        assert_eq!(error, "Cancelled");
    }

    #[cfg(any(unix, target_os = "windows"))]
    #[test]
    fn full_tree_summary_counts_directory_links_without_recursing() {
        let temp = TempDir::new();
        let root = temp.path().join("root");
        let target = temp.path().join("target");
        let linked = root.join("linked");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(target.join("target-file.txt"), b"abc").unwrap();
        if create_directory_symlink(&target, &linked).is_err() {
            return;
        }

        let snapshot = collect_property_snapshot(PropertyTarget { paths: vec![root] }).unwrap();

        assert_eq!(
            snapshot.contains,
            Some(PropertyValue::Ready(PropertyContains {
                files: 0,
                folders: 1
            }))
        );
    }

    #[gpui::test]
    fn applying_tree_summary_preserves_dirty_draft(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("a.txt"), b"a").unwrap();

        let target = PropertyTarget {
            paths: vec![folder],
        };
        let fast_snapshot = collect_property_snapshot_fast_with_date_format(
            target.clone(),
            crate::settings::DEFAULT_DATE_FORMAT,
        )
        .unwrap();
        let full_snapshot = collect_property_snapshot(target.clone()).unwrap();
        let dialog = test_properties_dialog(cx, target);

        cx.update(|cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.set_ready_snapshot(fast_snapshot, cx);
                dialog.cancel_tree_summary_task();
                dialog.draft.hidden = Some(true);

                dialog.apply_tree_summary_snapshot(full_snapshot);

                assert_eq!(dialog.draft.hidden, Some(true));
                let PropertySnapshotState::Ready(snapshot) = &dialog.snapshot_state else {
                    panic!("snapshot should be ready");
                };
                assert_eq!(snapshot.size, PropertyValue::Ready(1));
            });
        });
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
    fn properties_dialog_defines_general_details_cover_spectrum_code_image_and_frames_tabs() {
        assert_eq!(
            PROPERTY_TABS,
            &[
                (PropertyTab::General, "General"),
                (PropertyTab::Details, "Details"),
                (PropertyTab::Cover, "Cover"),
                (PropertyTab::Spectrum, "Spectrum"),
                (PropertyTab::Code, "Code"),
                (PropertyTab::Image, "Image"),
                (PropertyTab::Frames, "Frames")
            ]
        );
    }

    #[test]
    fn property_navigation_siblings_include_files_and_folders_in_explorer_order() {
        let temp = TempDir::new();
        let folder_2 = temp.path().join("folder 2");
        let folder_10 = temp.path().join("folder 10");
        let file_2 = temp.path().join("file 2.txt");
        let file_10 = temp.path().join("file 10.txt");
        fs::create_dir(&folder_10).unwrap();
        fs::write(&file_10, b"10").unwrap();
        fs::create_dir(&folder_2).unwrap();
        fs::write(&file_2, b"2").unwrap();

        assert_eq!(
            adjacent_property_path(
                &PropertyTarget {
                    paths: vec![folder_2.clone()]
                },
                PropertyItemKind::SingleFolder,
                PropertyNavigationDirection::Next,
            ),
            Some(folder_10.clone())
        );
        assert_eq!(
            adjacent_property_path(
                &PropertyTarget {
                    paths: vec![folder_10.clone()]
                },
                PropertyItemKind::SingleFolder,
                PropertyNavigationDirection::Next,
            ),
            Some(file_2.clone())
        );
        assert_eq!(
            adjacent_property_path(
                &PropertyTarget {
                    paths: vec![file_2.clone()]
                },
                PropertyItemKind::SingleFile,
                PropertyNavigationDirection::Next,
            ),
            Some(file_10)
        );
        assert_eq!(
            adjacent_property_path(
                &PropertyTarget {
                    paths: vec![file_2]
                },
                PropertyItemKind::SingleFile,
                PropertyNavigationDirection::Previous,
            ),
            Some(folder_10)
        );
    }

    #[test]
    fn property_navigation_wraps_directory_edges() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        let a = temp.path().join("a.txt");
        let b = temp.path().join("b.txt");
        fs::create_dir(&folder).unwrap();
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();

        assert_eq!(
            adjacent_property_path(
                &PropertyTarget {
                    paths: vec![folder.clone()]
                },
                PropertyItemKind::SingleFolder,
                PropertyNavigationDirection::Previous,
            ),
            Some(b.clone())
        );
        assert_eq!(
            adjacent_property_path(
                &PropertyTarget { paths: vec![b] },
                PropertyItemKind::SingleFile,
                PropertyNavigationDirection::Next,
            ),
            Some(folder)
        );
    }

    #[cfg(any(unix, target_os = "windows"))]
    #[test]
    fn property_navigation_siblings_include_directory_links() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        let link = temp.path().join("linked");
        let file = temp.path().join("note.txt");
        fs::create_dir(&folder).unwrap();
        if create_directory_symlink(&folder, &link).is_err() {
            return;
        }
        fs::write(&file, b"note").unwrap();

        assert_eq!(
            adjacent_property_path(
                &PropertyTarget {
                    paths: vec![folder]
                },
                PropertyItemKind::SingleFolder,
                PropertyNavigationDirection::Next,
            ),
            Some(link.clone())
        );
        assert_eq!(
            adjacent_property_path(
                &PropertyTarget { paths: vec![link] },
                PropertyItemKind::SingleShortcut,
                PropertyNavigationDirection::Next,
            ),
            Some(file)
        );
    }

    #[test]
    fn property_navigation_returns_none_for_missing_current_multiselect_and_no_sibling() {
        let temp = TempDir::new();
        let a = temp.path().join("a.txt");
        let b = temp.path().join("b.txt");
        let missing = temp.path().join("missing.txt");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();

        assert_eq!(
            adjacent_property_path(
                &PropertyTarget {
                    paths: vec![missing]
                },
                PropertyItemKind::SingleFile,
                PropertyNavigationDirection::Next,
            ),
            None
        );
        assert_eq!(
            adjacent_property_path(
                &PropertyTarget {
                    paths: vec![a.clone(), b]
                },
                PropertyItemKind::MultipleFiles,
                PropertyNavigationDirection::Next,
            ),
            None
        );
        assert_eq!(
            adjacent_property_path(
                &PropertyTarget {
                    paths: vec![a.clone()]
                },
                PropertyItemKind::Missing,
                PropertyNavigationDirection::Next,
            ),
            None
        );

        let lone_temp = TempDir::new();
        let only = lone_temp.path().join("only.txt");
        fs::write(&only, b"only").unwrap();
        assert_eq!(
            adjacent_property_path(
                &PropertyTarget { paths: vec![only] },
                PropertyItemKind::SingleFile,
                PropertyNavigationDirection::Next,
            ),
            None
        );
    }

    #[gpui::test]
    fn properties_dialog_snapshot_task_loads_target(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        fs::write(&file, b"abc").unwrap();

        let dialog = test_properties_dialog(cx, PropertyTarget { paths: vec![file] });
        cx.run_until_parked();

        let (title, item_kind, draft_readonly, draft_hidden) = cx.update(|cx| {
            let dialog = dialog.read(cx);
            let PropertySnapshotState::Ready(snapshot) = &dialog.snapshot_state else {
                panic!("snapshot should be ready");
            };

            (
                snapshot.title.clone(),
                snapshot.item_kind,
                dialog.draft.readonly,
                dialog.draft.hidden,
            )
        });

        assert_eq!(title, "a.txt");
        assert_eq!(item_kind, PropertyItemKind::SingleFile);
        assert_eq!(draft_readonly, Some(false));
        assert_eq!(draft_hidden, Some(false));
    }

    #[gpui::test]
    fn properties_dialog_details_tab_does_not_auto_collect_file_checksums(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        fs::write(&file, b"abc").unwrap();

        let dialog = test_properties_dialog(cx, PropertyTarget { paths: vec![file] });
        cx.run_until_parked();

        cx.update(|cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.active_tab = PropertyTab::Details;
                dialog.start_details_task(cx);
                assert!(matches!(
                    dialog.details_state,
                    PropertyDetailsState::Loading
                ));
            });
        });
        cx.run_until_parked();

        cx.update(|cx| {
            let dialog = dialog.read(cx);
            let PropertyDetailsState::Ready(groups) = &dialog.details_state else {
                panic!("details should be ready");
            };

            assert_eq!(
                detail_value(groups, PropertyDetailGroupKind::File, "CRC32"),
                None
            );
            assert_eq!(
                detail_value(groups, PropertyDetailGroupKind::File, "SHA256"),
                None
            );
            assert!(matches!(
                dialog.checksum_state,
                PropertyChecksumState::NotRequested
            ));
        });
    }

    #[gpui::test]
    fn properties_dialog_calculates_file_checksums_on_request(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        fs::write(&file, b"abc").unwrap();

        let dialog = test_properties_dialog(cx, PropertyTarget { paths: vec![file] });
        cx.run_until_parked();

        cx.update(|cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.start_checksum_task(cx);
                assert!(matches!(
                    dialog.checksum_state,
                    PropertyChecksumState::Loading
                ));
            });
        });
        cx.run_until_parked();

        cx.update(|cx| {
            let dialog = dialog.read(cx);
            let PropertyChecksumState::Ready(checksums) = &dialog.checksum_state else {
                panic!("checksums should be ready");
            };

            assert_eq!(checksums.crc32, "352441c2");
            assert_eq!(
                checksums.sha256,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            );
        });
    }

    #[gpui::test]
    fn stale_checksum_task_result_is_ignored(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        fs::write(&file, b"abc").unwrap();

        let dialog = test_properties_dialog(cx, PropertyTarget { paths: vec![file] });
        cx.run_until_parked();

        cx.update(|cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.start_checksum_task(cx);
                assert!(matches!(
                    dialog.checksum_state,
                    PropertyChecksumState::Loading
                ));
                dialog.reset_checksum_state();
                assert!(matches!(
                    dialog.checksum_state,
                    PropertyChecksumState::NotRequested
                ));
            });
        });
        cx.run_until_parked();

        cx.update(|cx| {
            let dialog = dialog.read(cx);
            assert!(matches!(
                dialog.checksum_state,
                PropertyChecksumState::NotRequested
            ));
        });
    }

    #[gpui::test]
    fn properties_dialog_image_tab_embeds_viewer_and_decodes_image(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let path = temp.path().join("image.png");
        write_test_png(&path, 4, 2);

        let (dialog, cx) = test_properties_dialog_window(
            cx,
            PropertyTarget {
                paths: vec![path.clone()],
            },
        );
        cx.run_until_parked();

        cx.update(|_, cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.active_tab = PropertyTab::Image;
                cx.notify();
            });
        });
        cx.run_until_parked();

        cx.update(|_, cx| {
            let viewer = {
                let dialog = dialog.read(cx);
                assert_eq!(dialog.image_viewer_path.as_deref(), Some(path.as_path()));
                dialog.image_viewer.clone().expect("image viewer")
            };
            assert_eq!(viewer.read(cx).ready_dimensions_for_test(), Some((4, 2)));
        });
    }

    #[gpui::test]
    fn properties_dialog_image_viewer_open_path_retargets_dialog(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let first = temp.path().join("a.png");
        let second = temp.path().join("b.png");
        for (path, width, height) in [(&first, 4, 2), (&second, 6, 3)] {
            write_test_png(path, width, height);
        }

        let (dialog, cx) = test_properties_dialog_window(
            cx,
            PropertyTarget {
                paths: vec![first.clone()],
            },
        );
        cx.run_until_parked();

        cx.update(|_, cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.active_tab = PropertyTab::Image;
                cx.notify();
            });
        });
        cx.run_until_parked();

        let viewer = cx.update(|_, cx| dialog.read(cx).image_viewer.clone().expect("image viewer"));
        cx.update(|_, cx| {
            viewer.update(cx, |_, cx| {
                cx.emit(ImageViewerEvent::OpenPath(second.clone()));
            });
        });
        cx.run_until_parked();

        cx.update(|_, cx| {
            let replacement_viewer = {
                let dialog = dialog.read(cx);
                assert_eq!(dialog.target.paths, vec![second.clone()]);
                assert_eq!(dialog.active_tab, PropertyTab::Image);
                assert_eq!(dialog.image_viewer_path.as_deref(), Some(second.as_path()));
                let PropertySnapshotState::Ready(snapshot) = &dialog.snapshot_state else {
                    panic!("snapshot should be ready");
                };
                assert_eq!(snapshot.target.paths, vec![second.clone()]);
                dialog
                    .image_viewer
                    .clone()
                    .expect("replacement image viewer")
            };
            assert_eq!(
                replacement_viewer.read(cx).ready_dimensions_for_test(),
                Some((6, 3))
            );
        });
    }

    #[gpui::test]
    fn properties_dialog_arrow_navigation_retargets_file_or_folder_from_details_tab(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        let file = temp.path().join("note.txt");
        fs::create_dir(&folder).unwrap();
        fs::write(&file, b"note").unwrap();

        let (dialog, cx) = test_properties_dialog_window(
            cx,
            PropertyTarget {
                paths: vec![folder.clone()],
            },
        );
        cx.run_until_parked();

        cx.update(|_, cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.active_tab = PropertyTab::Details;
                dialog.start_details_task(cx);
                cx.notify();
            });
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            let focus_handle = dialog.read(cx).focus_handle(cx);
            focus_handle.focus(window);
        });
        cx.dispatch_action(PropertiesOpenNext);
        cx.run_until_parked();

        cx.update(|_, cx| {
            let dialog = dialog.read(cx);
            assert_eq!(dialog.target.paths, vec![file.clone()]);
            assert_eq!(dialog.active_tab, PropertyTab::Details);
            let PropertySnapshotState::Ready(snapshot) = &dialog.snapshot_state else {
                panic!("snapshot should be ready");
            };
            assert_eq!(snapshot.target.paths, vec![file]);
        });
    }

    #[test]
    fn property_tab_navigation_wraps_and_skips_hidden_tabs() {
        let temp = TempDir::new();
        let image_path = temp.path().join("photo.png");
        let text_path = temp.path().join("note.txt");
        write_test_png(&image_path, 4, 2);
        fs::write(&text_path, b"note").unwrap();

        let image = collect_property_snapshot(PropertyTarget {
            paths: vec![image_path],
        })
        .unwrap();
        let text = collect_property_snapshot(PropertyTarget {
            paths: vec![text_path],
        })
        .unwrap();

        assert_eq!(
            adjacent_property_tab(
                PropertyTab::Details,
                Some(&image),
                PropertyTabDirection::Next
            ),
            Some(PropertyTab::Image)
        );
        assert_eq!(
            adjacent_property_tab(PropertyTab::Image, Some(&image), PropertyTabDirection::Next),
            Some(PropertyTab::General)
        );
        assert_eq!(
            adjacent_property_tab(
                PropertyTab::General,
                Some(&image),
                PropertyTabDirection::Previous
            ),
            Some(PropertyTab::Image)
        );
        assert_eq!(
            adjacent_property_tab(
                PropertyTab::Details,
                Some(&text),
                PropertyTabDirection::Next
            ),
            Some(PropertyTab::General)
        );
        assert_eq!(
            adjacent_property_tab(
                PropertyTab::General,
                Some(&text),
                PropertyTabDirection::Previous
            ),
            Some(PropertyTab::Details)
        );
    }

    #[gpui::test]
    fn properties_dialog_tab_actions_switch_visible_tabs_without_retargeting(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp = TempDir::new();
        let image = temp.path().join("photo.png");
        write_test_png(&image, 4, 2);

        let target = PropertyTarget {
            paths: vec![image.clone()],
        };
        let (dialog, cx) = test_properties_dialog_window(cx, target.clone());
        cx.run_until_parked();

        cx.update(|window, cx| {
            let focus_handle = dialog.read(cx).focus_handle(cx);
            focus_handle.focus(window);
        });

        cx.dispatch_action(SelectNextTab);
        cx.run_until_parked();
        cx.update(|_, cx| {
            let dialog = dialog.read(cx);
            assert_eq!(dialog.target.paths, target.paths);
            assert_eq!(dialog.active_tab, PropertyTab::Details);
        });

        cx.dispatch_action(SelectNextTab);
        cx.run_until_parked();
        cx.update(|_, cx| {
            let dialog = dialog.read(cx);
            assert_eq!(dialog.target.paths, target.paths);
            assert_eq!(dialog.active_tab, PropertyTab::Image);
        });

        cx.dispatch_action(SelectPreviousTab);
        cx.run_until_parked();
        cx.update(|_, cx| {
            let dialog = dialog.read(cx);
            assert_eq!(dialog.target.paths, vec![image]);
            assert_eq!(dialog.active_tab, PropertyTab::Details);
        });
    }

    #[gpui::test]
    fn properties_dialog_arrow_navigation_resets_invalid_image_tab_to_general(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp = TempDir::new();
        let image = temp.path().join("a.png");
        let text = temp.path().join("b.txt");
        write_test_png(&image, 4, 2);
        fs::write(&text, b"text").unwrap();

        let (dialog, cx) = test_properties_dialog_window(
            cx,
            PropertyTarget {
                paths: vec![image.clone()],
            },
        );
        cx.run_until_parked();

        cx.update(|_, cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.active_tab = PropertyTab::Image;
                cx.notify();
            });
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            let focus_handle = dialog.read(cx).focus_handle(cx);
            focus_handle.focus(window);
        });
        cx.dispatch_action(PropertiesOpenNext);
        cx.run_until_parked();

        cx.update(|_, cx| {
            let dialog = dialog.read(cx);
            assert_eq!(dialog.target.paths, vec![text.clone()]);
            assert_eq!(dialog.active_tab, PropertyTab::General);
            let PropertySnapshotState::Ready(snapshot) = &dialog.snapshot_state else {
                panic!("snapshot should be ready");
            };
            assert_eq!(snapshot.target.paths, vec![text]);
        });
    }

    #[gpui::test]
    fn properties_dialog_arrow_navigation_keeps_image_tab_for_next_image(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp = TempDir::new();
        let first = temp.path().join("a.png");
        let second = temp.path().join("b.png");
        write_test_png(&first, 4, 2);
        write_test_png(&second, 6, 3);

        let (dialog, cx) = test_properties_dialog_window(
            cx,
            PropertyTarget {
                paths: vec![first.clone()],
            },
        );
        cx.run_until_parked();

        cx.update(|_, cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.active_tab = PropertyTab::Image;
                cx.notify();
            });
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            let focus_handle = dialog.read(cx).focus_handle(cx);
            focus_handle.focus(window);
        });
        cx.dispatch_action(PropertiesOpenNext);
        cx.run_until_parked();

        cx.update(|_, cx| {
            let dialog = dialog.read(cx);
            assert_eq!(dialog.target.paths, vec![second.clone()]);
            assert_eq!(dialog.active_tab, PropertyTab::Image);
            assert_eq!(dialog.image_viewer_path.as_deref(), Some(second.as_path()));
            let PropertySnapshotState::Ready(snapshot) = &dialog.snapshot_state else {
                panic!("snapshot should be ready");
            };
            assert_eq!(snapshot.target.paths, vec![second]);
        });
    }

    #[gpui::test]
    fn properties_dialog_arrow_navigation_no_ops_for_multiselect(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let first = temp.path().join("a.txt");
        let second = temp.path().join("b.txt");
        let third = temp.path().join("c.txt");
        fs::write(&first, b"a").unwrap();
        fs::write(&second, b"b").unwrap();
        fs::write(&third, b"c").unwrap();

        let target = PropertyTarget {
            paths: vec![first.clone(), second.clone()],
        };
        let (dialog, cx) = test_properties_dialog_window(cx, target.clone());
        cx.run_until_parked();

        cx.update(|_, cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.active_tab = PropertyTab::Details;
                dialog.start_details_task(cx);
                cx.notify();
            });
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            let focus_handle = dialog.read(cx).focus_handle(cx);
            focus_handle.focus(window);
        });
        cx.dispatch_action(PropertiesOpenNext);
        cx.run_until_parked();

        cx.update(|_, cx| {
            let dialog = dialog.read(cx);
            assert_eq!(dialog.target.paths, target.paths);
            assert_eq!(dialog.active_tab, PropertyTab::Details);
            let PropertySnapshotState::Ready(snapshot) = &dialog.snapshot_state else {
                panic!("snapshot should be ready");
            };
            assert_eq!(snapshot.target.paths, vec![first, second]);
        });
    }

    #[test]
    fn property_preview_fit_rect_bounds_landscape_by_width() {
        assert_eq!(
            property_preview_fit_rect(1000, 500, 400.0, 300.0),
            Some(PropertyPreviewSize {
                width: 400.0,
                height: 200.0
            })
        );
    }

    #[test]
    fn property_preview_fit_rect_bounds_portrait_by_height() {
        assert_eq!(
            property_preview_fit_rect(500, 1000, 400.0, 300.0),
            Some(PropertyPreviewSize {
                width: 150.0,
                height: 300.0
            })
        );
    }

    #[test]
    fn property_preview_fit_rect_keeps_small_image_natural_size() {
        assert_eq!(
            property_preview_fit_rect(100, 50, 400.0, 300.0),
            Some(PropertyPreviewSize {
                width: 100.0,
                height: 50.0
            })
        );
    }

    #[test]
    fn property_preview_fit_rect_omits_invalid_or_empty_space() {
        assert_eq!(property_preview_fit_rect(100, 50, 0.0, 300.0), None);
        assert_eq!(property_preview_fit_rect(100, 50, 400.0, 0.0), None);
        assert_eq!(property_preview_fit_rect(0, 50, 400.0, 300.0), None);
        assert_eq!(property_preview_fit_rect(100, 0, 400.0, 300.0), None);
    }

    #[test]
    fn property_image_copy_payload_encodes_render_image_as_png() {
        let expected = vec![10, 20, 30, 255, 50, 60, 70, 128];
        let render_image = render_image_from_bgra(2, 1, vec![30, 20, 10, 255, 70, 60, 50, 128]);
        let payload = PropertyImageCopyPayload {
            image: render_image,
            width: 2,
            height: 1,
        };

        let clipboard_image = clipboard_image_from_property_image_payload(&payload).unwrap();

        assert_eq!(clipboard_image.format(), ImageFormat::Png);
        assert_png_image_pixels(&clipboard_image, 2, 1, &expected);
    }

    #[gpui::test]
    fn properties_dialog_frames_tab_rejects_non_video_target(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        fs::write(&file, b"abc").unwrap();

        let dialog = test_properties_dialog(cx, PropertyTarget { paths: vec![file] });
        cx.run_until_parked();

        cx.update(|cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.active_tab = PropertyTab::Frames;
                dialog.start_frames_task(cx);
            });
        });

        cx.update(|cx| {
            let dialog = dialog.read(cx);
            let PropertyFramesState::Failed(error) = &dialog.frames_state else {
                panic!("frames should be unavailable");
            };
            assert_eq!(error, "Video frames are not available for this item.");
        });
    }

    #[gpui::test]
    fn properties_dialog_cover_tab_rejects_non_audio_target(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        fs::write(&file, b"abc").unwrap();

        let dialog = test_properties_dialog(cx, PropertyTarget { paths: vec![file] });
        cx.run_until_parked();

        cx.update(|cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.active_tab = PropertyTab::Cover;
                dialog.start_cover_task(cx);
            });
        });

        cx.update(|cx| {
            let dialog = dialog.read(cx);
            let PropertyCoverState::Failed(error) = &dialog.cover_state else {
                panic!("cover should be unavailable");
            };
            assert_eq!(error, "Audio covers are not available for this item.");
        });
    }

    #[gpui::test]
    fn properties_dialog_cover_buttons_render_and_change_cover_index(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp = TempDir::new();
        let path = temp.path().join("song.mp3");
        fs::write(&path, b"not real audio").unwrap();

        let (dialog, cx) = test_properties_dialog_window(cx, PropertyTarget { paths: vec![path] });
        cx.run_until_parked();

        cx.update(|_, cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.active_tab = PropertyTab::Cover;
                dialog.cover_state = PropertyCoverState::Ready(vec![
                    test_property_cover_image("Front"),
                    test_property_cover_image("Back"),
                ]);
                dialog.cover_index = 0;
                cx.notify();
            });
        });
        cx.run_until_parked();

        assert!(cx.debug_bounds("properties-cover-previous").is_some());
        assert!(cx.debug_bounds("properties-cover-next").is_some());

        click_visual_selector(cx, "properties-cover-previous");
        cx.run_until_parked();
        cx.read_entity(&dialog, |dialog, _| {
            assert_eq!(dialog.cover_index, 0);
        });

        click_visual_selector(cx, "properties-cover-next");
        cx.run_until_parked();
        cx.read_entity(&dialog, |dialog, _| {
            assert_eq!(dialog.cover_index, 1);
        });

        click_visual_selector(cx, "properties-cover-next");
        cx.run_until_parked();
        cx.read_entity(&dialog, |dialog, _| {
            assert_eq!(dialog.cover_index, 1);
        });

        click_visual_selector(cx, "properties-cover-previous");
        cx.run_until_parked();
        cx.read_entity(&dialog, |dialog, _| {
            assert_eq!(dialog.cover_index, 0);
        });
    }

    #[gpui::test]
    fn properties_dialog_spectrum_y_axis_labels_render_and_align(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let path = temp.path().join("song.flac");
        fs::write(&path, b"not real audio").unwrap();

        let (dialog, cx) = test_properties_dialog_window(cx, PropertyTarget { paths: vec![path] });
        cx.run_until_parked();

        cx.update(|_, cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.active_tab = PropertyTab::Spectrum;
                dialog.spectrum_range = PropertySpectrumRange::default();
                dialog.spectrum_state = PropertySpectrumState::Ready(PropertySpectrumAnalysis {
                    metadata: PropertySpectrumMetadata {
                        header: "FLAC (Free Lossless Audio Codec), 192000 Hz, 24 bits, 2 channels"
                            .to_owned(),
                        sample_rate: 192_000,
                        duration_seconds: 267.0,
                        bit_rate: None,
                        bit_depth: Some(24),
                        channels: 2,
                    },
                    db_values: vec![-120.0, -80.0, -40.0, -20.0],
                    image: render_image_from_bgra(2, 2, vec![0, 0, 0, 255].repeat(4)),
                    target: PropertySpectrumTarget {
                        time_bins: 2,
                        frequency_bins: 2,
                        fft_size: PROPERTIES_SPECTRUM_FFT_SIZE,
                    },
                    width: 2,
                    height: 2,
                });
                cx.notify();
            });
        });
        cx.run_until_parked();

        assert_eq!(spectrum_frequency_ruler_labels(192_000).len(), 21);

        let image = visible_debug_bounds(cx, "properties-spectrum-image");
        let frequency_ruler = visible_debug_bounds(cx, "properties-spectrum-frequency-ruler");
        let first_frequency_label =
            visible_debug_bounds(cx, "properties-spectrum-frequency-ruler-label-0");
        let last_frequency_label =
            visible_debug_bounds(cx, "properties-spectrum-frequency-ruler-label-20");
        let db_ruler = visible_debug_bounds(cx, "properties-spectrum-db-ruler");
        let first_db_label = visible_debug_bounds(cx, "properties-spectrum-db-ruler-label-0");
        let last_db_label = visible_debug_bounds(cx, "properties-spectrum-db-ruler-label-5");
        let db_legend = visible_debug_bounds(cx, "properties-spectrum-db-legend");

        assert_min_debug_width(
            &frequency_ruler,
            PROPERTIES_SPECTRUM_FREQUENCY_RULER_WIDTH,
            "properties-spectrum-frequency-ruler",
        );
        assert_min_debug_width(
            &first_frequency_label,
            PROPERTIES_SPECTRUM_FREQUENCY_RULER_WIDTH,
            "properties-spectrum-frequency-ruler-label-0",
        );
        assert_min_debug_width(
            &last_frequency_label,
            PROPERTIES_SPECTRUM_FREQUENCY_RULER_WIDTH,
            "properties-spectrum-frequency-ruler-label-20",
        );
        assert_min_debug_width(
            &db_ruler,
            PROPERTIES_SPECTRUM_DB_LABEL_WIDTH,
            "properties-spectrum-db-ruler",
        );
        assert_min_debug_width(
            &first_db_label,
            PROPERTIES_SPECTRUM_DB_LABEL_WIDTH,
            "properties-spectrum-db-ruler-label-0",
        );
        assert_min_debug_width(
            &last_db_label,
            PROPERTIES_SPECTRUM_DB_LABEL_WIDTH,
            "properties-spectrum-db-ruler-label-5",
        );
        assert_debug_width_near(
            &db_legend,
            PROPERTIES_SPECTRUM_DB_LEGEND_WIDTH,
            "properties-spectrum-db-legend",
        );

        assert_eq!(frequency_ruler.top(), image.top());
        assert_eq!(frequency_ruler.bottom(), image.bottom());
        assert_eq!(db_ruler.top(), image.top());
        assert_eq!(db_ruler.bottom(), image.bottom());
    }

    #[gpui::test]
    fn properties_dialog_spectrum_redraws_cached_image_on_resize(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let path = temp.path().join("song.flac");
        fs::write(&path, b"not real audio").unwrap();

        let (dialog, cx) = test_properties_dialog_window(cx, PropertyTarget { paths: vec![path] });
        cx.run_until_parked();

        cx.update(|_, cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.active_tab = PropertyTab::Spectrum;
                dialog.spectrum_range = PropertySpectrumRange::default();
                dialog.spectrum_state = PropertySpectrumState::Ready(test_spectrum_analysis(8, 4));
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.run_until_parked();
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.refresh().unwrap();
        cx.run_until_parked();

        let initial_bounds = visible_debug_bounds(cx, "properties-spectrum-render-target");
        let initial_target = spectrum_render_size_for_visual_bounds(cx, initial_bounds);
        let (initial_cache_target, initial_image_width, initial_image_height) =
            cx.read_entity(&dialog, |dialog, _| {
                let cache = dialog
                    .spectrum_render_cache
                    .as_ref()
                    .expect("initial spectrum render cache");
                (
                    cache.key.target,
                    cache.image.size(0).width.0,
                    cache.image.size(0).height.0,
                )
            });
        assert_eq!(initial_cache_target, initial_target);
        assert_eq!(initial_image_width, initial_target.width as i32);
        assert_eq!(initial_image_height, initial_target.height as i32);

        cx.simulate_resize(size(px(900.0), px(700.0)));
        cx.run_until_parked();
        cx.run_until_parked();
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.refresh().unwrap();
        cx.run_until_parked();

        let resized_bounds = visible_debug_bounds(cx, "properties-spectrum-render-target");
        let resized_target = spectrum_render_size_for_visual_bounds(cx, resized_bounds);
        let (resized_cache_target, resized_image_width, resized_image_height) =
            cx.read_entity(&dialog, |dialog, _| {
                let cache = dialog
                    .spectrum_render_cache
                    .as_ref()
                    .expect("resized spectrum render cache");
                (
                    cache.key.target,
                    cache.image.size(0).width.0,
                    cache.image.size(0).height.0,
                )
            });

        assert_ne!(resized_target, initial_target);
        assert_eq!(resized_cache_target, resized_target);
        assert_eq!(resized_image_width, resized_target.width as i32);
        assert_eq!(resized_image_height, resized_target.height as i32);
    }

    #[gpui::test]
    fn spectrum_refinement_result_updates_only_latest_generation(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let path = temp.path().join("song.flac");
        fs::write(&path, b"not real audio").unwrap();

        let dialog = test_properties_dialog(cx, PropertyTarget { paths: vec![path] });
        cx.run_until_parked();

        cx.update(|cx| {
            dialog.update(cx, |dialog, _| {
                let initial = test_spectrum_analysis(2, 2);
                let stale = test_spectrum_analysis(4, 3);
                let refined_target =
                    PropertySpectrumTarget::from_render_size(PropertySpectrumRenderSize {
                        width: 1024,
                        height: 512,
                    });
                let refined = test_spectrum_analysis_for_target(refined_target);

                dialog.spectrum_generation = 10;
                dialog.spectrum_refinement.generation = 3;
                dialog.spectrum_refinement.pending_target = Some(refined_target);
                dialog.spectrum_state = PropertySpectrumState::Ready(initial);

                assert!(!dialog.apply_spectrum_refinement_result(10, 2, stale.target, Ok(stale)));
                let PropertySpectrumState::Ready(analysis) = &dialog.spectrum_state else {
                    panic!("spectrum should stay ready");
                };
                assert_eq!(analysis.width, 2);
                assert_eq!(analysis.height, 2);

                assert!(dialog.apply_spectrum_refinement_result(
                    10,
                    3,
                    refined_target,
                    Ok(refined)
                ));
                let PropertySpectrumState::Ready(analysis) = &dialog.spectrum_state else {
                    panic!("spectrum should stay ready");
                };
                assert_eq!(analysis.target, refined_target);
                assert_eq!(analysis.width, refined_target.width());
                assert_eq!(analysis.height, refined_target.height());
                assert_ne!(analysis.width, PropertySpectrumTarget::initial().width());
                assert_eq!(dialog.spectrum_refinement.pending_target, None);
            });
        });
    }

    #[gpui::test]
    fn spectrum_refinement_debounce_keeps_latest_pending_target(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let path = temp.path().join("song.flac");
        fs::write(&path, b"not real audio").unwrap();

        let dialog = test_properties_dialog(cx, PropertyTarget { paths: vec![path] });
        cx.run_until_parked();

        cx.update(|cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.spectrum_state = PropertySpectrumState::Ready(test_spectrum_analysis(2, 2));
                dialog.spectrum_generation = 1;

                dialog.spectrum_render_size = Some(PropertySpectrumRenderSize {
                    width: 900,
                    height: 500,
                });
                dialog.schedule_spectrum_resize_refinement(cx);
                let first_generation = dialog.spectrum_refinement.generation;
                let first_cancel = dialog
                    .spectrum_refinement
                    .cancel
                    .as_ref()
                    .expect("first refinement cancel")
                    .clone();

                dialog.spectrum_render_size = Some(PropertySpectrumRenderSize {
                    width: 1200,
                    height: 700,
                });
                dialog.schedule_spectrum_resize_refinement(cx);

                let expected =
                    PropertySpectrumTarget::from_render_size(PropertySpectrumRenderSize {
                        width: 1200,
                        height: 700,
                    });
                assert!(first_cancel.load(Ordering::Relaxed));
                assert!(dialog.spectrum_refinement.generation > first_generation);
                assert_eq!(dialog.spectrum_refinement.pending_target, Some(expected));
                dialog.cancel_spectrum_refinement_task();
            });
        });
    }

    #[gpui::test]
    fn properties_dialog_code_tab_loads_git_and_language_summary(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let repo = init_property_test_repo(temp.path());
        let source = "fn main() {}\n";
        fs::write(temp.path().join("main.rs"), source).unwrap();
        commit_on_ref(&repo, Some("HEAD"), "main.rs", source, "initial", &[]);

        let dialog = test_properties_dialog(
            cx,
            PropertyTarget {
                paths: vec![temp.path().to_path_buf()],
            },
        );
        cx.run_until_parked();

        cx.update(|cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.active_tab = PropertyTab::Code;
                dialog.start_code_task(cx);
                assert!(matches!(dialog.code_state, PropertyCodeState::Loading));
            });
        });
        cx.run_until_parked();

        cx.update(|cx| {
            let dialog = dialog.read(cx);
            let PropertyCodeState::Ready(summary) = &dialog.code_state else {
                panic!("code summary should be ready");
            };

            assert_eq!(summary.git.branch, "main");
            assert_eq!(summary.git.commit_count, 1);
            assert_eq!(summary.git.divergence, None);
            assert_eq!(summary.codebase.total_code, 1);
            assert_eq!(
                summary
                    .codebase
                    .languages
                    .iter()
                    .map(|language| (language.name.as_str(), language.code, language.percentage))
                    .collect::<Vec<_>>(),
                vec![("Rust", 1, 100)]
            );
        });
    }

    #[test]
    fn code_tab_is_visible_only_for_direct_git_root_folder() {
        let temp = TempDir::new();
        let nested = temp.path().join("src");
        let file_path = temp.path().join("main.rs");
        fs::create_dir(temp.path().join(".git")).unwrap();
        fs::create_dir(&nested).unwrap();
        fs::write(&file_path, "fn main() {}\n").unwrap();

        let root = collect_property_snapshot(PropertyTarget {
            paths: vec![temp.path().to_path_buf()],
        })
        .unwrap();
        let nested = collect_property_snapshot(PropertyTarget {
            paths: vec![nested],
        })
        .unwrap();
        let file = collect_property_snapshot(PropertyTarget {
            paths: vec![file_path.clone()],
        })
        .unwrap();
        let mixed = collect_property_snapshot(PropertyTarget {
            paths: vec![temp.path().to_path_buf(), file_path],
        })
        .unwrap();

        assert_eq!(
            property_tabs_for_snapshot(Some(&root)),
            vec![
                (PropertyTab::General, "General"),
                (PropertyTab::Details, "Details"),
                (PropertyTab::Code, "Code")
            ]
        );
        assert_eq!(
            property_tabs_for_snapshot(Some(&nested)),
            vec![
                (PropertyTab::General, "General"),
                (PropertyTab::Details, "Details")
            ]
        );
        assert_eq!(
            property_tabs_for_snapshot(Some(&file)),
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
                (PropertyTab::Details, "Details"),
                (PropertyTab::Image, "Image")
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
    fn cover_tab_is_visible_only_for_single_audio_files() {
        let temp = TempDir::new();
        let audio = temp.path().join("song.mp3");
        let video = temp.path().join("movie.mp4");
        let image = temp.path().join("photo.jpg");
        let folder = temp.path().join("folder");
        let other = temp.path().join("other.txt");
        let missing_path = temp.path().join("missing.mp3");
        fs::write(&audio, b"not real audio").unwrap();
        fs::write(&video, b"not real video").unwrap();
        fs::write(&image, b"not real image").unwrap();
        fs::write(&other, b"other").unwrap();
        fs::create_dir(&folder).unwrap();

        let audio = collect_property_snapshot(PropertyTarget { paths: vec![audio] }).unwrap();
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
            paths: vec![audio.target.paths[0].clone(), other],
        })
        .unwrap();

        assert_eq!(
            property_tabs_for_snapshot(Some(&audio)),
            vec![
                (PropertyTab::General, "General"),
                (PropertyTab::Details, "Details"),
                (PropertyTab::Cover, "Cover"),
                (PropertyTab::Spectrum, "Spectrum")
            ]
        );
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
                (PropertyTab::Details, "Details"),
                (PropertyTab::Image, "Image")
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
    fn spectrum_tab_is_visible_only_for_single_audio_files() {
        let temp = TempDir::new();
        let audio = temp.path().join("song.flac");
        let video = temp.path().join("movie.mp4");
        let image = temp.path().join("photo.jpg");
        let folder = temp.path().join("folder");
        let other = temp.path().join("other.txt");
        let missing_path = temp.path().join("missing.flac");
        fs::write(&audio, b"not real audio").unwrap();
        fs::write(&video, b"not real video").unwrap();
        fs::write(&image, b"not real image").unwrap();
        fs::write(&other, b"other").unwrap();
        fs::create_dir(&folder).unwrap();

        let audio = collect_property_snapshot(PropertyTarget { paths: vec![audio] }).unwrap();
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
            paths: vec![audio.target.paths[0].clone(), other],
        })
        .unwrap();

        assert!(
            property_tabs_for_snapshot(Some(&audio))
                .iter()
                .any(|(tab, label)| *tab == PropertyTab::Spectrum && *label == "Spectrum")
        );
        for snapshot in [&video, &image, &folder, &missing, &mixed] {
            assert!(
                !property_tabs_for_snapshot(Some(snapshot))
                    .iter()
                    .any(|(tab, _)| *tab == PropertyTab::Spectrum)
            );
        }
    }

    #[test]
    fn image_tab_is_visible_only_for_single_image_files() {
        let temp = TempDir::new();
        let png = temp.path().join("photo.png");
        let jpg = temp.path().join("photo.jpg");
        let ico = temp.path().join("icon.ico");
        let svg = temp.path().join("vector.svg");
        let folder = temp.path().join("folder");
        let other = temp.path().join("other.txt");
        let missing_path = temp.path().join("missing.png");
        fs::write(&png, b"not real png").unwrap();
        fs::write(&jpg, b"not real jpg").unwrap();
        fs::write(&ico, b"not real ico").unwrap();
        fs::write(&svg, b"not real svg").unwrap();
        fs::write(&other, b"other").unwrap();
        fs::create_dir(&folder).unwrap();

        for image_path in [&png, &jpg, &ico, &svg] {
            let snapshot = collect_property_snapshot(PropertyTarget {
                paths: vec![image_path.clone()],
            })
            .unwrap();
            assert_eq!(
                property_tabs_for_snapshot(Some(&snapshot)),
                vec![
                    (PropertyTab::General, "General"),
                    (PropertyTab::Details, "Details"),
                    (PropertyTab::Image, "Image")
                ]
            );
        }

        let folder = collect_property_snapshot(PropertyTarget {
            paths: vec![folder],
        })
        .unwrap();
        let missing = collect_property_snapshot(PropertyTarget {
            paths: vec![missing_path],
        })
        .unwrap();
        let mixed = collect_property_snapshot(PropertyTarget {
            paths: vec![jpg, other],
        })
        .unwrap();

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
    fn properties_tabs_do_not_include_compatibility_for_windows_exe() {
        let temp = TempDir::new();
        let exe = temp.path().join("setup.EXE");
        let bat = temp.path().join("script.bat");
        let text = temp.path().join("note.txt");
        fs::write(&exe, b"not really executable").unwrap();
        fs::write(&bat, b"echo ok").unwrap();
        fs::write(&text, b"text").unwrap();

        let exe_snapshot = collect_property_snapshot(PropertyTarget {
            paths: vec![exe.clone()],
        })
        .unwrap();
        let bat_snapshot = collect_property_snapshot(PropertyTarget { paths: vec![bat] }).unwrap();
        let mixed_snapshot = collect_property_snapshot(PropertyTarget {
            paths: vec![exe, text],
        })
        .unwrap();

        assert_eq!(
            property_tabs_for_snapshot(Some(&exe_snapshot)),
            vec![
                (PropertyTab::General, "General"),
                (PropertyTab::Details, "Details")
            ]
        );
        assert_eq!(
            property_tabs_for_snapshot(Some(&bat_snapshot)),
            vec![
                (PropertyTab::General, "General"),
                (PropertyTab::Details, "Details")
            ]
        );
        assert_eq!(
            property_tabs_for_snapshot(Some(&mixed_snapshot)),
            vec![
                (PropertyTab::General, "General"),
                (PropertyTab::Details, "Details")
            ]
        );
    }

    #[test]
    fn run_as_admin_setting_is_available_only_for_single_windows_exe() {
        let temp = TempDir::new();
        let exe = temp.path().join("setup.EXE");
        let bat = temp.path().join("script.bat");
        let text = temp.path().join("note.txt");
        let folder = temp.path().join("folder");
        fs::write(&exe, b"not really executable").unwrap();
        fs::write(&bat, b"echo ok").unwrap();
        fs::write(&text, b"text").unwrap();
        fs::create_dir(&folder).unwrap();

        let exe_snapshot = collect_property_snapshot(PropertyTarget {
            paths: vec![exe.clone()],
        })
        .unwrap();
        let bat_snapshot = collect_property_snapshot(PropertyTarget { paths: vec![bat] }).unwrap();
        let folder_snapshot = collect_property_snapshot(PropertyTarget {
            paths: vec![folder],
        })
        .unwrap();
        let mixed_snapshot = collect_property_snapshot(PropertyTarget {
            paths: vec![exe, text],
        })
        .unwrap();

        assert_eq!(
            snapshot_has_run_as_admin_setting(&exe_snapshot),
            cfg!(target_os = "windows")
        );
        assert!(!snapshot_has_run_as_admin_setting(&bat_snapshot));
        assert!(!snapshot_has_run_as_admin_setting(&folder_snapshot));
        assert!(!snapshot_has_run_as_admin_setting(&mixed_snapshot));
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
    fn rendered_details_cache_invalidates_when_snapshot_generation_changes() {
        let mut cache_key = None;
        let mut cache = Vec::new();
        let mut snapshot = test_property_snapshot_with_file_detail("Name", "first.txt");
        let details_state = PropertyDetailsState::NotStarted;

        let first = detail_groups_for_render_cached(
            &mut cache_key,
            &mut cache,
            &snapshot,
            &details_state,
            1,
        );
        assert_eq!(
            detail_value(first, PropertyDetailGroupKind::File, "Name"),
            Some("first.txt")
        );

        snapshot.details = vec![test_property_detail_group(
            PropertyDetailGroupKind::File,
            "Name",
            "second.txt",
        )];
        let still_cached = detail_groups_for_render_cached(
            &mut cache_key,
            &mut cache,
            &snapshot,
            &details_state,
            1,
        );
        assert_eq!(
            detail_value(still_cached, PropertyDetailGroupKind::File, "Name"),
            Some("first.txt")
        );

        let refreshed = detail_groups_for_render_cached(
            &mut cache_key,
            &mut cache,
            &snapshot,
            &details_state,
            2,
        );
        assert_eq!(
            detail_value(refreshed, PropertyDetailGroupKind::File, "Name"),
            Some("second.txt")
        );
    }

    #[test]
    fn rendered_details_cache_invalidates_when_async_details_are_ready() {
        let mut cache_key = None;
        let mut cache = Vec::new();
        let snapshot = test_property_snapshot_with_file_detail("Name", "file.txt");

        let initial = detail_groups_for_render_cached(
            &mut cache_key,
            &mut cache,
            &snapshot,
            &PropertyDetailsState::Loading,
            1,
        );
        assert_eq!(
            detail_value(initial, PropertyDetailGroupKind::File, "SHA256"),
            None
        );

        let ready_state = PropertyDetailsState::Ready(vec![test_property_detail_group(
            PropertyDetailGroupKind::File,
            "SHA256",
            "abc123",
        )]);
        let refreshed =
            detail_groups_for_render_cached(&mut cache_key, &mut cache, &snapshot, &ready_state, 1);
        assert_eq!(
            detail_value(refreshed, PropertyDetailGroupKind::File, "SHA256"),
            Some("abc123")
        );
    }

    #[test]
    fn property_scrollbar_metrics_only_exist_for_overflow() {
        assert!(property_scrollbar_metrics_for_dimensions(100.0, 0.0, 0.0).is_none());

        let metrics = property_scrollbar_metrics_for_dimensions(100.0, 50.0, 500.0)
            .expect("overflow metrics");
        assert_eq!(metrics.viewport_height, 100.0);
        assert_eq!(metrics.content_height, 150.0);
        assert_eq!(metrics.scroll_max, 50.0);
        assert_eq!(metrics.scroll_top, 50.0);
    }

    #[test]
    fn svg_raster_dimensions_scale_longest_side_to_limit() {
        assert_eq!(svg_raster_dimensions(1000.0, 250.0, 500), Some((500, 125)));
        assert_eq!(svg_raster_dimensions(250.0, 1000.0, 500), Some((125, 500)));
        assert_eq!(svg_raster_dimensions(400.0, 400.0, 500), Some((500, 500)));
        assert_eq!(svg_raster_dimensions(3.0, 1.0, 500), Some((500, 167)));
    }

    #[test]
    fn svg_raster_dimensions_reject_invalid_inputs() {
        assert_eq!(svg_raster_dimensions(0.0, 100.0, 500), None);
        assert_eq!(svg_raster_dimensions(100.0, 0.0, 500), None);
        assert_eq!(svg_raster_dimensions(f32::NAN, 100.0, 500), None);
        assert_eq!(svg_raster_dimensions(100.0, f32::INFINITY, 500), None);
        assert_eq!(svg_raster_dimensions(100.0, 100.0, 0), None);
    }

    #[test]
    fn image_preview_decodes_png_dimensions() {
        let temp = TempDir::new();
        let path = temp.path().join("image.png");
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(4, 2));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        fs::write(&path, bytes).unwrap();

        let preview = load_property_image_preview(&path).unwrap();

        assert_eq!(preview.width, 4);
        assert_eq!(preview.height, 2);
        assert_render_image_size(&preview, 4, 2);
        assert!(!preview.image.as_bytes(0).unwrap().is_empty());
    }

    #[test]
    fn image_preview_decodes_ico_dimensions() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/explorer.ico");

        let preview = load_property_image_preview(&path).unwrap();

        assert!(preview.width > 0);
        assert!(preview.height > 0);
        assert_render_image_size(&preview, preview.width, preview.height);
        assert!(!preview.image.as_bytes(0).unwrap().is_empty());
    }

    #[test]
    fn image_preview_rasterizes_svg_to_500px_longest_side() {
        let temp = TempDir::new();
        let path = temp.path().join("vector.svg");
        fs::write(
            &path,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="250"><rect width="1000" height="250" fill="red"/></svg>"#,
        )
        .unwrap();

        let preview = load_property_image_preview(&path).unwrap();

        assert_eq!(preview.width, 500);
        assert_eq!(preview.height, 125);
        assert_render_image_size(&preview, 500, 125);
        assert!(!preview.image.as_bytes(0).unwrap().is_empty());
    }

    #[test]
    fn video_probe_details_are_grouped_and_formatted() {
        let groups = ffprobe_detail_groups_from_probe(
            &sample_ffprobe_json("5025.678", "4898900", "Studio Cut", 1920, 1080),
            FfprobeMetadataKind::Video,
        );

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
            detail_value(&groups, PropertyDetailGroupKind::Audio, "Compression"),
            Some("Lossy")
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
    fn audio_probe_details_are_grouped_and_formatted() {
        let groups = ffprobe_detail_groups_from_probe(
            &sample_audio_ffprobe_json_with_cover_count(2),
            FfprobeMetadataKind::Audio,
        );

        assert_detail_contains(&groups, PropertyDetailGroupKind::Media, "Format", "FLAC");
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Media, "Duration"),
            Some("0h 03m (0:03:12.500)")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Media, "Bit rate"),
            Some("0.92 Mb/s (921.60 kb/s)")
        );
        let tags_index = group_index(&groups, PropertyDetailGroupKind::Tags).unwrap();
        let media_index = group_index(&groups, PropertyDetailGroupKind::Media).unwrap();
        assert!(tags_index < media_index);
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Tags, "Title"),
            Some("Song Title")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Tags, "Artist"),
            Some("Example Artist")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Tags, "Album artist"),
            Some("Various Artists")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Tags, "Album"),
            Some("Example Album")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Tags, "Year"),
            Some("2024")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Tags, "Track"),
            Some("3/10")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Tags, "Discnumber"),
            Some("1/2")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Tags, "Genre"),
            Some("Jazz")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Tags, "Comment"),
            Some("Loose note")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Tags, "Composer"),
            Some("Example Composer")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Tags, "Cover"),
            Some("Yes (2)")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Tags, "Encoder"),
            Some("reference encoder")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Tags, "Mood"),
            Some("Reflective")
        );
        assert_eq!(
            detail_value(
                &groups,
                PropertyDetailGroupKind::Tags,
                "Replaygain Track Gain"
            ),
            Some("-7.50 dB")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Media, "Embedded title"),
            None
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Media, "Artist"),
            None
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Media, "Streams"),
            Some("1 Audio")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Media, "Audio tracks"),
            Some("1")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Media, "Chapters"),
            Some("1")
        );
        assert!(detail_group(&groups, PropertyDetailGroupKind::Video).is_none());

        assert_detail_contains(&groups, PropertyDetailGroupKind::Audio, "Codec", "FLAC");
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Audio, "Compression"),
            Some("Lossless")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Audio, "Channels"),
            Some("2 channels")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Audio, "Channel layout"),
            Some("stereo")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Audio, "Sample rate"),
            Some("44,100 Hz")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Audio, "Bit rate"),
            Some("0.92 Mb/s (921.60 kb/s)")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Audio, "Bit depth"),
            Some("24 bit")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Audio, "Sample format"),
            Some("s32")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Audio, "Duration"),
            Some("0h 03m (0:03:12.500)")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Audio, "Language"),
            Some("eng")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Audio, "Title"),
            Some("Main audio")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Audio, "Disposition"),
            Some("Default")
        );
        assert_detail_contains(
            &groups,
            PropertyDetailGroupKind::Chapters,
            "Chapter 1",
            "Intro",
        );

        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Misc, "Format Tag Artist"),
            None
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Misc, "Format Tag Encoder"),
            None
        );
        assert_eq!(
            detail_value(
                &groups,
                PropertyDetailGroupKind::Misc,
                "Format Tag Nested Ignored"
            ),
            None
        );
        assert_eq!(
            detail_value(
                &groups,
                PropertyDetailGroupKind::Misc,
                "Audio 1 Bits Per Raw Sample"
            ),
            None
        );
    }

    #[test]
    fn audio_tags_cover_reports_zero_when_no_attached_picture_streams() {
        let groups = ffprobe_detail_groups_from_probe(
            &sample_audio_ffprobe_json_with_cover_count(0),
            FfprobeMetadataKind::Audio,
        );

        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Tags, "Cover"),
            Some("No (0)")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Media, "Streams"),
            Some("1 Audio")
        );
        assert!(detail_group(&groups, PropertyDetailGroupKind::Video).is_none());
    }

    #[test]
    fn audio_cover_requests_use_attached_picture_stream_indexes_and_titles() {
        let requests =
            audio_cover_requests_from_probe(&sample_audio_ffprobe_json_with_cover_count(2));

        assert_eq!(
            requests,
            vec![
                AudioCoverRequest {
                    stream_index: 1,
                    label: "Cover 1".to_owned()
                },
                AudioCoverRequest {
                    stream_index: 2,
                    label: "Cover 2".to_owned()
                }
            ]
        );
    }

    #[test]
    fn audio_cover_requests_are_empty_without_attached_picture_streams() {
        assert!(
            audio_cover_requests_from_probe(&sample_audio_ffprobe_json_with_cover_count(0))
                .is_empty()
        );
    }

    #[test]
    fn audio_spectrum_headers_are_formatted_from_ffprobe_json() {
        let flac =
            audio_spectrum_metadata_from_probe(&sample_audio_ffprobe_json_with_cover_count(2))
                .unwrap();
        assert_eq!(
            flac.header,
            "FLAC (Free Lossless Audio Codec), 44100 Hz, 24 bits, 2 channels"
        );
        assert_eq!(flac.sample_rate, 44_100);
        assert_seconds(flac.duration_seconds, 192.5);
        assert_eq!(flac.bit_rate, Some(921_600));
        assert_eq!(flac.bit_depth, Some(24));
        assert_eq!(flac.channels, 2);

        let mp3_probe = serde_json::json!({
            "format": {
                "duration": "1261.000",
                "bit_rate": "63900"
            },
            "streams": [
                {
                    "codec_name": "mp3",
                    "codec_long_name": "MP3 (MPEG audio layer 3)",
                    "codec_type": "audio",
                    "sample_rate": "44100",
                    "channels": 1,
                    "sample_fmt": "fltp",
                    "duration": "1273.000",
                    "bit_rate": "64000"
                }
            ]
        });
        let mp3 = audio_spectrum_metadata_from_probe(&mp3_probe).unwrap();
        assert_eq!(
            mp3.header,
            "MP3 (MPEG audio layer 3), 64 kbps, 44100 Hz, 1 channel"
        );
        assert!(!mp3.header.contains("32 bits"));
        assert_eq!(mp3.bit_rate, Some(64_000));
        assert_eq!(mp3.bit_depth, None);
        assert_eq!(mp3.channels, 1);
        assert_seconds(mp3.duration_seconds, 1273.0);

        let pcm_probe = serde_json::json!({
            "format": {
                "duration": "9.250"
            },
            "streams": [
                {
                    "codec_name": "pcm_s16le",
                    "codec_long_name": "PCM signed 16-bit little-endian",
                    "codec_type": "audio",
                    "sample_rate": "48000",
                    "channels": 2,
                    "bits_per_raw_sample": "16"
                }
            ]
        });
        let pcm = audio_spectrum_metadata_from_probe(&pcm_probe).unwrap();
        assert_eq!(
            pcm.header,
            "PCM signed 16-bit little-endian, 48000 Hz, 16 bits, 2 channels"
        );
        assert_eq!(pcm.bit_rate, None);
        assert_eq!(pcm.bit_depth, Some(16));
    }

    #[test]
    fn ffmpeg_audio_spectrum_args_decode_first_audio_stream_to_f32le() {
        let args: Vec<_> = ffmpeg_audio_spectrum_args(Path::new("song.flac"), 0)
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert!(
            args.windows(2)
                .any(|window| window[0] == "-map" && window[1] == "0:a:0")
        );
        assert!(
            !args.iter().any(|arg| arg == "-ac"),
            "channels should be preserved for Rust-side averaging"
        );
        assert!(
            args.windows(2)
                .any(|window| window[0] == "-f" && window[1] == "f32le")
        );
        assert!(
            args.windows(2)
                .any(|window| window[0] == "-acodec" && window[1] == "pcm_f32le")
        );
        assert_eq!(args.first().map(String::as_str), Some("-v"));
        assert_eq!(args.last().map(String::as_str), Some("-"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn media_process_hidden_window_flag_stays_at_windows_create_no_window() {
        assert_eq!(CREATE_NO_WINDOW, 0x08000000);
    }

    #[test]
    fn spectrum_ruler_helpers_auto_fit_time_frequency_and_density_labels() {
        assert_eq!(
            spectrum_frequency_ruler_labels(48_000),
            vec!["24 kHz", "20 kHz", "15 kHz", "10 kHz", "5 kHz", "0 kHz"]
        );
        assert_eq!(
            spectrum_frequency_ruler_labels(44_100),
            vec!["22 kHz", "20 kHz", "15 kHz", "10 kHz", "5 kHz", "0 kHz"]
        );
        assert_eq!(
            spectrum_time_ruler_labels(145.0),
            vec![
                "0:00", "0:20", "0:40", "1:00", "1:20", "1:40", "2:00", "2:20", "2:25"
            ]
        );
        assert_eq!(
            spectrum_density_ruler_labels(PropertySpectrumRange::default()),
            vec!["-20 dB", "-40 dB", "-60 dB", "-80 dB", "-100 dB", "-120 dB"]
        );
    }

    #[test]
    fn spectrum_palette_clamps_and_range_remaps_existing_db_values() {
        let default_range = PropertySpectrumRange::default();
        assert_eq!(
            spectrum_density_bgra(-200.0, default_range),
            spectrum_density_bgra(default_range.low_db, default_range)
        );
        assert_eq!(
            spectrum_density_bgra(20.0, default_range),
            spectrum_density_bgra(default_range.high_db, default_range)
        );
        let high_color = spectrum_density_bgra(default_range.high_db, default_range);
        assert!(
            high_color[2] > 240 && high_color[1] < 8 && high_color[0] < 8,
            "high spectrum density should render as Spek red in GPUI BGRA order"
        );

        let db = -100.0;
        let remapped_range = PropertySpectrumRange {
            low_db: -80.0,
            high_db: -20.0,
        };
        assert_ne!(
            spectrum_density_bgra(db, default_range),
            spectrum_density_bgra(db, remapped_range)
        );

        let db_values = vec![-120.0, -80.0, -40.0, -20.0];
        assert!(spectrum_render_image(&db_values, 2, 2, default_range).is_some());
        assert!(spectrum_render_image(&db_values, 2, 2, remapped_range).is_some());
        assert!(spectrum_render_image(&db_values[..3], 2, 2, default_range).is_none());
    }

    #[test]
    fn spectrum_resampled_render_image_matches_target_size_and_validates_input() {
        let range = PropertySpectrumRange::default();
        let db_values = vec![-120.0, -80.0, -40.0, -20.0];

        let base = spectrum_render_image(&db_values, 2, 2, range).unwrap();
        assert_render_image_pixel_size(&base, 2, 2);

        let resized = spectrum_render_image_resampled(&db_values, 2, 2, 5, 3, range).unwrap();
        assert_render_image_pixel_size(&resized, 5, 3);

        assert!(spectrum_render_image_resampled(&db_values, 2, 2, 0, 3, range).is_none());
        assert!(spectrum_render_image_resampled(&db_values, 2, 2, 5, 0, range).is_none());
        assert!(spectrum_render_image_resampled(&db_values[..3], 2, 2, 5, 3, range).is_none());
    }

    #[test]
    fn spectrum_target_clamps_render_size_to_safe_analysis_bounds() {
        let initial = PropertySpectrumTarget::initial();
        assert_eq!(initial.time_bins, PROPERTIES_SPECTRUM_INITIAL_TIME_BINS);
        assert_eq!(initial.frequency_bins, PROPERTIES_SPECTRUM_FREQUENCY_BINS);
        assert_eq!(initial.fft_size, PROPERTIES_SPECTRUM_FFT_SIZE);
        assert_eq!(initial.fft_size, 2048);
        assert_eq!(initial.frequency_bins, 1025);

        let small = PropertySpectrumTarget::from_render_size(PropertySpectrumRenderSize {
            width: 0,
            height: 0,
        });
        assert_eq!(small.time_bins, 1);
        assert_eq!(small.frequency_bins, PROPERTIES_SPECTRUM_FREQUENCY_BINS);
        assert_eq!(small.fft_size, PROPERTIES_SPECTRUM_FFT_SIZE);

        let large = PropertySpectrumTarget::from_render_size(PropertySpectrumRenderSize {
            width: u32::MAX,
            height: u32::MAX,
        });
        assert_eq!(large.time_bins, PROPERTIES_SPECTRUM_MAX_TIME_BINS);
        assert_eq!(large.frequency_bins, PROPERTIES_SPECTRUM_FREQUENCY_BINS);
        assert_eq!(large.fft_size, PROPERTIES_SPECTRUM_FFT_SIZE);
    }

    #[test]
    fn spectrum_pcm_collector_averages_channels_and_columns() {
        let target = PropertySpectrumTarget {
            time_bins: 2,
            frequency_bins: 5,
            fft_size: 8,
        };
        let metadata = PropertySpectrumMetadata {
            header: "PCM signed 32-bit float little-endian, 8 Hz, 2 channels".to_owned(),
            sample_rate: 8,
            duration_seconds: 2.0,
            bit_rate: None,
            bit_depth: Some(32),
            channels: 2,
        };
        let frame = [1.0f32.to_le_bytes(), 0.0f32.to_le_bytes()].concat();
        assert_eq!(average_f32le_audio_frame(&frame, 2), 0.5);

        let mut pcm = Vec::new();
        for _ in 0..16 {
            pcm.extend_from_slice(&1.0f32.to_le_bytes());
            pcm.extend_from_slice(&0.0f32.to_le_bytes());
        }
        let cancel = AtomicBool::new(false);
        let db_values = collect_spectrum_db_values_from_pcm_reader(
            &mut Cursor::new(pcm),
            &metadata,
            target,
            &cancel,
        )
        .unwrap();

        assert_eq!(db_values.len(), target.time_bins * target.frequency_bins);
        assert!(db_values.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn spectrum_fft_helpers_use_target_dimensions() {
        let target = PropertySpectrumTarget {
            time_bins: 3,
            frequency_bins: 7,
            fft_size: 64,
        };
        let windows = vec![0.0; target.time_bins * target.fft_size];
        let db_values = spectrum_db_values_from_windows(&windows, target);
        assert_eq!(db_values.len(), target.time_bins * target.frequency_bins);

        let column = spectrum_db_column(&windows[..target.fft_size], target);
        assert_eq!(column.len(), target.frequency_bins);
        assert!(column.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn spectrum_fft_helpers_handle_silence_and_synthetic_sine_peak() {
        let target = PropertySpectrumTarget::initial();
        let silence = vec![0.0; target.fft_size];
        let silence_db = spectrum_db_column(&silence, target);
        assert_eq!(silence_db.len(), target.frequency_bins);
        assert!(silence_db.iter().all(|value| value.is_finite()));
        let silence_max = silence_db.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(silence_max < -200.0);

        let sine_bin = 512.0;
        let sine: Vec<_> = (0..target.fft_size)
            .map(|index| {
                (std::f32::consts::TAU * sine_bin * index as f32 / target.fft_size as f32).sin()
            })
            .collect();
        let sine_db = spectrum_db_column(&sine, target);
        let (peak_index, peak_db) = sine_db
            .iter()
            .copied()
            .enumerate()
            .max_by(|(_, left), (_, right)| left.total_cmp(right))
            .unwrap();

        assert!(peak_db > silence_max + 80.0);
        assert!(
            (500..=524).contains(&peak_index),
            "expected sine peak near frequency bin 512, got {peak_index}"
        );
    }

    #[test]
    fn audio_cover_preview_decodes_png_dimensions() {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(3, 2));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();

        let cover = prepare_audio_cover_preview(AudioCoverPng {
            label: "Front".to_owned(),
            stream_index: 1,
            png: bytes,
        })
        .unwrap();

        assert_eq!(cover.label, "Front");
        assert_eq!(cover.preview.width, 3);
        assert_eq!(cover.preview.height, 2);
        assert_render_image_size(&cover.preview, 3, 2);
    }

    #[test]
    fn audio_compression_labels_known_codecs() {
        assert_eq!(audio_compression_label("flac"), Some("Lossless"));
        assert_eq!(audio_compression_label("pcm_s16le"), Some("Lossless"));
        assert_eq!(audio_compression_label("aac"), Some("Lossy"));
        assert_eq!(audio_compression_label("wmav2"), Some("Lossy"));
        assert_eq!(audio_compression_label("ra_144"), Some("Lossy"));
        assert_eq!(audio_compression_label("mystery"), None);
    }

    #[test]
    fn video_probe_unknown_scalars_are_preserved() {
        let groups = ffprobe_detail_groups_from_probe(
            &sample_ffprobe_json("60.0", "1000000", "Clip", 1280, 720),
            FfprobeMetadataKind::Video,
        );

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
        let groups = ffprobe_detail_groups_from_probe(
            &sample_multi_stream_ffprobe_json(),
            FfprobeMetadataKind::Video,
        );

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
                ("Compression", "Lossy"),
                ("Channels", "2 channels"),
                ("#2", ""),
                ("Codec", "Opus"),
                ("Compression", "Lossy"),
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
        let groups =
            ffprobe_metadata_unavailable_groups(FfprobeMetadataKind::Video, "ffprobe missing");

        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Media, "Video metadata"),
            Some("ffprobe missing")
        );
    }

    #[test]
    fn unavailable_audio_metadata_returns_nonfatal_group() {
        let groups =
            ffprobe_metadata_unavailable_groups(FfprobeMetadataKind::Audio, "ffprobe missing");

        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::Media, "Audio metadata"),
            Some("ffprobe missing")
        );
    }

    #[test]
    fn ffprobe_metadata_detection_prefers_audio_mime_before_video_extension_fallback() {
        assert_eq!(
            ffprobe_metadata_kind_for_path(Path::new("movie.mp4")),
            Some(FfprobeMetadataKind::Video)
        );
        assert_eq!(
            ffprobe_metadata_kind_for_path(Path::new("clip.mkv")),
            Some(FfprobeMetadataKind::Video)
        );
        assert_eq!(
            ffprobe_metadata_kind_for_path(Path::new("song.mp3")),
            Some(FfprobeMetadataKind::Audio)
        );
        assert_eq!(
            ffprobe_metadata_kind_for_path(Path::new("track.flac")),
            Some(FfprobeMetadataKind::Audio)
        );
        assert_eq!(
            ffprobe_metadata_kind_for_path(Path::new("clip.m4a")),
            Some(FfprobeMetadataKind::Audio)
        );
        assert_eq!(
            ffprobe_metadata_kind_for_path(Path::new("sound.ogg")),
            Some(FfprobeMetadataKind::Audio)
        );
        assert_eq!(ffprobe_metadata_kind_for_path(Path::new("note.txt")), None);
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
    fn video_frame_request_debug_summary_includes_request_boundaries() {
        let requests = video_frame_requests(60.0);

        assert_eq!(
            video_frame_request_debug_summary(60.0, &requests),
            "duration=60.000 requests=22 first_label=0:00.000 first_seek=0.000 last_label=1:00.000 last_seek=59.950"
        );
    }

    #[test]
    fn video_frame_request_debug_summary_handles_empty_requests() {
        assert_eq!(
            video_frame_request_debug_summary(0.0, &[]),
            "duration=0.000 requests=0"
        );
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
        assert_eq!(thumbnail.width, 4);
        assert_eq!(thumbnail.height, 2);
        assert_eq!(thumbnail.aspect_ratio, 2.0);
        assert_eq!(size.width.0, 4);
        assert_eq!(size.height.0, 2);
    }

    #[gpui::test]
    fn properties_dialog_copies_frame_thumbnail_to_clipboard(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        let file = temp.path().join("movie.mp4");
        fs::write(&file, b"not real video").unwrap();
        let expected = vec![10, 20, 30, 255, 50, 60, 70, 128];
        let image = image::DynamicImage::ImageRgba8(
            image::RgbaImage::from_raw(2, 1, expected.clone()).unwrap(),
        );
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        let thumbnail = prepare_video_frame_thumbnail(VideoFramePng {
            label: "0:00.000".to_owned(),
            png: bytes,
        })
        .unwrap();

        let dialog = test_properties_dialog(cx, PropertyTarget { paths: vec![file] });
        cx.run_until_parked();

        cx.update(|cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.copy_property_image_payload_to_clipboard(thumbnail.copy_payload(), cx);
            });

            let item = cx.read_from_clipboard().expect("clipboard item");
            let clipboard_image = clipboard_item_image(&item).expect("clipboard image");
            assert_png_image_pixels(&clipboard_image, 2, 1, &expected);
        });
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
            Some(EXIF_VALUE_TOO_LARGE_LABEL)
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
            Some(EXIF_VALUE_TOO_LARGE_LABEL)
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
    fn file_checksums_match_known_values() {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        fs::write(&file, b"abc").unwrap();

        let checksums = file_checksums(&file).unwrap();

        assert_eq!(checksums.crc32, "352441c2");
        assert_eq!(
            checksums.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn file_checksum_cache_returns_fresh_values_and_expires_stale_values() {
        let cache = FileChecksumCache::new();
        let key = FileChecksumCacheKey {
            path: PathBuf::from("a.txt"),
            size: 3,
            modified: None,
        };
        let checksums = FileChecksums {
            crc32: "crc".to_owned(),
            sha256: "sha".to_owned(),
        };
        let now = Instant::now();

        cache.insert_at(key.clone(), checksums.clone(), now);

        assert_eq!(
            cache.get_at(&key, now + FILE_CHECKSUM_CACHE_TTL - Duration::from_secs(1)),
            Some(checksums)
        );
        assert_eq!(cache.get_at(&key, now + FILE_CHECKSUM_CACHE_TTL), None);
    }

    #[test]
    fn file_checksum_cache_key_changes_when_file_metadata_changes() {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        fs::write(&file, b"abc").unwrap();
        let first_time = FileTime::from_unix_time(1, 0);
        set_file_times(&file, first_time, first_time).unwrap();
        let first_key = file_checksum_cache_key(&file).unwrap();
        let cache = FileChecksumCache::new();
        cache.insert(
            first_key.clone(),
            FileChecksums {
                crc32: "crc".to_owned(),
                sha256: "sha".to_owned(),
            },
        );

        fs::write(&file, b"abcd").unwrap();
        let size_key = file_checksum_cache_key(&file).unwrap();
        assert_ne!(first_key, size_key);
        assert_eq!(cache.get(&size_key), None);

        fs::write(&file, b"abc").unwrap();
        let second_time = FileTime::from_unix_time(2, 0);
        set_file_times(&file, second_time, second_time).unwrap();
        let modified_key = file_checksum_cache_key(&file).unwrap();
        assert_ne!(first_key, modified_key);
        assert_eq!(cache.get(&modified_key), None);
    }

    #[test]
    fn single_file_details_do_not_collect_file_checksums() {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        fs::write(&file, b"abc").unwrap();

        let groups = collect_single_file_detail_groups(
            &PropertyTarget { paths: vec![file] },
            PropertyItemKind::SingleFile,
        );

        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::File, "CRC32"),
            None
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::File, "SHA256"),
            None
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::File, "SHA512"),
            None
        );
    }

    #[test]
    fn non_single_file_properties_do_not_collect_file_checksums() {
        let temp = TempDir::new();
        let first = temp.path().join("a.txt");
        let second = temp.path().join("b.txt");
        let folder = temp.path().join("folder");
        let missing = temp.path().join("missing.txt");
        fs::write(&first, b"a").unwrap();
        fs::write(&second, b"b").unwrap();
        fs::create_dir(&folder).unwrap();

        let folder_groups = collect_single_file_detail_groups(
            &PropertyTarget {
                paths: vec![folder],
            },
            PropertyItemKind::SingleFolder,
        );
        let missing_groups = collect_single_file_detail_groups(
            &PropertyTarget {
                paths: vec![missing],
            },
            PropertyItemKind::Missing,
        );
        let multiple_groups = collect_single_file_detail_groups(
            &PropertyTarget {
                paths: vec![first, second],
            },
            PropertyItemKind::MultipleFiles,
        );

        assert!(folder_groups.is_empty());
        assert!(missing_groups.is_empty());
        assert!(multiple_groups.is_empty());
    }

    #[test]
    fn rendered_details_merge_async_file_group_into_snapshot_file_group() {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        fs::write(&file, b"abc").unwrap();
        let snapshot = collect_property_snapshot(PropertyTarget {
            paths: vec![file.clone()],
        })
        .unwrap();
        let extra_groups = vec![test_property_detail_group(
            PropertyDetailGroupKind::File,
            "Custom",
            "metadata",
        )];

        let groups =
            detail_groups_for_render(&snapshot, &PropertyDetailsState::Ready(extra_groups));

        assert_eq!(
            groups
                .iter()
                .filter(|group| group.kind == PropertyDetailGroupKind::File)
                .count(),
            1
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::File, "Name"),
            Some("a.txt")
        );
        assert_eq!(
            detail_value(&groups, PropertyDetailGroupKind::File, "Custom"),
            Some("metadata")
        );
    }

    #[test]
    fn multiple_file_properties_do_not_collect_extra_details() {
        let temp = TempDir::new();
        let first = temp.path().join("a.jpg");
        let second = temp.path().join("b.jpg");
        fs::write(&first, jpeg_with_exif(&exif_tiff("Canon", "A", None))).unwrap();
        fs::write(&second, jpeg_with_exif(&exif_tiff("Nikon", "B", None))).unwrap();

        let groups = collect_single_file_detail_groups(
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
                &Ok(DefaultAppChangeOutcome::Cancelled),
                Some(&snapshot)
            ),
            None
        );
        assert!(
            default_app_change_error(
                &file,
                &before,
                &Ok(DefaultAppChangeOutcome::Changed),
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
                &Ok(DefaultAppChangeOutcome::Changed),
                Some(&snapshot)
            ),
            None
        );

        snapshot.default_app = None;
        assert!(
            default_app_change_error(
                &file,
                &before,
                &Ok(DefaultAppChangeOutcome::Changed),
                Some(&snapshot)
            )
            .unwrap()
            .contains("No default app")
        );

        assert!(
            default_app_change_error(
                &file,
                &before,
                &Err(std::io::Error::other("denied")),
                Some(&snapshot)
            )
            .unwrap()
            .contains("Could not change the default app")
        );
    }

    #[test]
    fn default_app_change_refreshes_file_type_icons_only_after_changed_result() {
        assert!(default_app_change_refreshes_file_type_icons(&Ok(
            DefaultAppChangeOutcome::Changed
        )));
        assert!(!default_app_change_refreshes_file_type_icons(&Ok(
            DefaultAppChangeOutcome::Cancelled
        )));
        assert!(!default_app_change_refreshes_file_type_icons(&Err(
            std::io::Error::other("denied")
        )));
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
    fn apply_plan_includes_changed_run_as_admin_flag() {
        let temp = TempDir::new();
        let file = temp.path().join("setup.exe");
        fs::write(&file, b"a").unwrap();
        let mut snapshot = collect_property_snapshot(PropertyTarget { paths: vec![file] }).unwrap();
        snapshot.run_as_admin = MixedValue::Single(false);
        let mut draft = EditablePropertyDraft::from_snapshot(&snapshot);
        draft.run_as_admin = Some(true);

        let plan = property_apply_plan(&snapshot, &draft);

        assert_eq!(plan.run_as_admin, Some(true));
        assert!(!property_apply_plan_is_empty(&plan));
    }

    #[test]
    fn run_as_admin_compatibility_value_preserves_other_flags() {
        assert_eq!(
            windows_compatibility_value_with_run_as_admin(None, true),
            Some("~ RUNASADMIN".to_owned())
        );
        assert_eq!(
            windows_compatibility_value_with_run_as_admin(Some("~ WINXPSP3"), true),
            Some("~ WINXPSP3 RUNASADMIN".to_owned())
        );
        assert_eq!(
            windows_compatibility_value_with_run_as_admin(Some("~ WINXPSP3 RUNASADMIN"), false),
            Some("~ WINXPSP3".to_owned())
        );
        assert_eq!(
            windows_compatibility_value_with_run_as_admin(Some("~ RUNASADMIN"), false),
            None
        );
        assert!(windows_compatibility_value_has_run_as_admin(
            "~ winxpsp3 runasadmin"
        ));
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

    fn render_image_from_bgra(width: u32, height: u32, bgra: Vec<u8>) -> Arc<RenderImage> {
        let image = image::RgbaImage::from_raw(width, height, bgra).expect("render image buffer");
        Arc::new(RenderImage::new(vec![image::Frame::new(image)]))
    }

    fn test_property_cover_image(label: &str) -> PropertyCoverImage {
        PropertyCoverImage {
            label: label.to_owned(),
            preview: PropertyImagePreview {
                image: render_image_from_bgra(2, 2, vec![0, 0, 0, 255].repeat(4)),
                width: 2,
                height: 2,
                animated_source: None,
            },
        }
    }

    fn clipboard_item_image(item: &ClipboardItem) -> Option<Image> {
        item.entries().iter().find_map(|entry| match entry {
            gpui::ClipboardEntry::Image(image) => Some(image.clone()),
            gpui::ClipboardEntry::String(_) => None,
            gpui::ClipboardEntry::Files(_) => None,
        })
    }

    fn assert_png_image_pixels(image: &Image, width: u32, height: u32, expected_rgba: &[u8]) {
        assert_eq!(image.format(), ImageFormat::Png);
        let decoded = image::load_from_memory_with_format(image.bytes(), image::ImageFormat::Png)
            .expect("clipboard PNG")
            .into_rgba8();
        assert_eq!(decoded.width(), width);
        assert_eq!(decoded.height(), height);
        assert_eq!(decoded.as_raw(), expected_rgba);
    }

    fn assert_render_image_pixel_size(image: &Arc<RenderImage>, width: u32, height: u32) {
        let size = image.size(0);
        assert_eq!(size.width.0, width as i32);
        assert_eq!(size.height.0, height as i32);
    }

    fn test_properties_dialog(
        cx: &mut gpui::TestAppContext,
        target: PropertyTarget,
    ) -> gpui::Entity<PropertiesDialog> {
        let explorer_root = target
            .paths
            .first()
            .and_then(|path| path.parent())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        cx.set_global(FileChecksumCache::new());

        cx.update(move |cx| {
            let explorer = cx.new(|_| ExplorerView::new(explorer_root));
            cx.new(|cx| {
                PropertiesDialog::new(
                    target,
                    explorer.downgrade(),
                    crate::settings::DEFAULT_DATE_FORMAT.to_owned(),
                    cx.focus_handle(),
                    cx,
                )
            })
        })
    }

    fn test_properties_dialog_window<'a>(
        cx: &'a mut gpui::TestAppContext,
        target: PropertyTarget,
    ) -> (
        gpui::Entity<PropertiesDialog>,
        &'a mut gpui::VisualTestContext,
    ) {
        let explorer_root = target
            .paths
            .first()
            .and_then(|path| path.parent())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        cx.set_global(FileChecksumCache::new());
        let explorer = cx.update(|cx| cx.new(|_| ExplorerView::new(explorer_root)));
        let explorer = explorer.downgrade();
        cx.add_window_view(move |_, cx| {
            PropertiesDialog::new(
                target,
                explorer,
                crate::settings::DEFAULT_DATE_FORMAT.to_owned(),
                cx.focus_handle(),
                cx,
            )
        })
    }

    fn click_visual_selector(cx: &mut gpui::VisualTestContext, selector: &'static str) {
        let position = cx.debug_bounds(selector).expect("element bounds").center();
        cx.simulate_mouse_down(position, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(position, MouseButton::Left, gpui::Modifiers::default());
    }

    fn test_spectrum_analysis(width: u32, height: u32) -> PropertySpectrumAnalysis {
        test_spectrum_analysis_for_target(PropertySpectrumTarget {
            time_bins: width as usize,
            frequency_bins: height as usize,
            fft_size: PROPERTIES_SPECTRUM_FFT_SIZE,
        })
    }

    fn test_spectrum_analysis_for_target(
        target: PropertySpectrumTarget,
    ) -> PropertySpectrumAnalysis {
        let width = target.width();
        let height = target.height();
        let value_count = target.time_bins.checked_mul(target.frequency_bins).unwrap();
        let db_values: Vec<_> = (0..value_count)
            .map(|index| {
                let level = index as f32 / value_count.max(1) as f32;
                PROPERTIES_SPECTRUM_DEFAULT_LOW_DB
                    + (PROPERTIES_SPECTRUM_DEFAULT_HIGH_DB - PROPERTIES_SPECTRUM_DEFAULT_LOW_DB)
                        * level
            })
            .collect();
        let image =
            spectrum_render_image(&db_values, width, height, PropertySpectrumRange::default())
                .expect("test spectrum image");
        PropertySpectrumAnalysis {
            metadata: PropertySpectrumMetadata {
                header: "FLAC (Free Lossless Audio Codec), 48000 Hz, 16 bits, 2 channels"
                    .to_owned(),
                sample_rate: 48_000,
                duration_seconds: 207.0,
                bit_rate: None,
                bit_depth: Some(16),
                channels: 2,
            },
            db_values,
            image,
            target,
            width,
            height,
        }
    }

    fn visible_debug_bounds(
        cx: &mut gpui::VisualTestContext,
        selector: &'static str,
    ) -> Bounds<Pixels> {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} should be visible"));
        assert!(
            f32::from(bounds.size.width) > 0.0,
            "{selector} should have nonzero width"
        );
        assert!(
            f32::from(bounds.size.height) > 0.0,
            "{selector} should have nonzero height"
        );
        bounds
    }

    fn spectrum_render_size_for_visual_bounds(
        cx: &mut gpui::VisualTestContext,
        bounds: Bounds<Pixels>,
    ) -> PropertySpectrumRenderSize {
        let scale_factor = cx.update(|window, _| window.scale_factor());
        PropertySpectrumRenderSize::from_bounds(bounds, scale_factor)
            .expect("visible spectrum bounds should produce a render size")
    }

    fn assert_min_debug_width(bounds: &Bounds<Pixels>, min_width: f32, selector: &str) {
        let width = f32::from(bounds.size.width);
        assert!(
            width >= min_width,
            "{selector} should be at least {min_width}px wide, got {width}px"
        );
    }

    fn assert_debug_width_near(bounds: &Bounds<Pixels>, expected_width: f32, selector: &str) {
        let width = f32::from(bounds.size.width);
        assert!(
            (width - expected_width).abs() <= 0.5,
            "{selector} should be {expected_width}px wide, got {width}px"
        );
    }

    #[cfg(unix)]
    fn create_directory_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(target_os = "windows")]
    fn create_directory_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    fn init_property_test_repo(path: &Path) -> Repository {
        let repo = Repository::init(path).expect("init repo");
        repo.set_head("refs/heads/main").expect("set HEAD branch");
        repo
    }

    fn commit_on_ref(
        repo: &Repository,
        update_ref: Option<&str>,
        file_name: &str,
        content: &str,
        message: &str,
        parents: &[&Commit<'_>],
    ) -> Oid {
        let signature =
            Signature::now("Explorer Tests", "explorer@example.com").expect("create signature");
        let parent_tree = parents.first().and_then(|parent| parent.tree().ok());
        let mut builder = repo
            .treebuilder(parent_tree.as_ref())
            .expect("create tree builder");
        let blob = repo.blob(content.as_bytes()).expect("write blob");
        builder
            .insert(file_name, blob, 0o100644)
            .expect("insert tree entry");
        let tree_oid = builder.write().expect("write tree");
        let tree = repo.find_tree(tree_oid).expect("find tree");

        repo.commit(update_ref, &signature, &signature, message, &tree, parents)
            .expect("commit")
    }

    fn test_property_snapshot_with_file_detail(name: &str, value: &str) -> PropertySnapshot {
        PropertySnapshot {
            target: PropertyTarget {
                paths: vec![PathBuf::from("file.txt")],
            },
            title: "file.txt".to_owned(),
            item_count: 1,
            item_kind: PropertyItemKind::SingleFile,
            type_label: MixedValue::None,
            location: MixedValue::None,
            size: PropertyValue::Ready(0),
            size_on_disk: PropertyValue::Ready(0),
            contains: None,
            selection_counts: None,
            created: MixedValue::None,
            modified: MixedValue::None,
            accessed: MixedValue::None,
            attributes: PropertyAttributes {
                readonly: MixedValue::None,
                hidden: MixedValue::None,
            },
            owner: MixedValue::None,
            group: MixedValue::None,
            unix_mode: MixedValue::None,
            permission_summary: MixedValue::None,
            default_app: None,
            run_as_admin: MixedValue::None,
            shortcut: None,
            details: vec![test_property_detail_group(
                PropertyDetailGroupKind::File,
                name,
                value,
            )],
        }
    }

    fn test_property_detail_group(
        kind: PropertyDetailGroupKind,
        name: &str,
        value: &str,
    ) -> PropertyDetailGroup {
        property_detail_group(
            kind,
            vec![PropertyDetail {
                name: name.to_owned(),
                value: value.to_owned(),
            }],
        )
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

    fn sample_audio_ffprobe_json_with_cover_count(cover_count: usize) -> serde_json::Value {
        let mut streams = vec![serde_json::json!({
            "index": 0,
            "codec_name": "flac",
            "codec_long_name": "FLAC (Free Lossless Audio Codec)",
            "codec_type": "audio",
            "sample_rate": "44100",
            "channels": 2,
            "channel_layout": "stereo",
            "sample_fmt": "s32",
            "bits_per_raw_sample": "24",
            "duration": "192.500",
            "bit_rate": "921600",
            "disposition": {
                "default": 1,
                "attached_pic": 0
            },
            "tags": {
                "language": "eng",
                "title": "Main audio"
            }
        })];
        for cover_index in 0..cover_count {
            streams.push(serde_json::json!({
                "index": cover_index + 1,
                "codec_name": "mjpeg",
                "codec_long_name": "Motion JPEG",
                "codec_type": "video",
                "width": 1200,
                "height": 1200,
                "disposition": {
                    "attached_pic": 1
                },
                "tags": {
                    "title": format!("Cover {}", cover_index + 1)
                }
            }));
        }

        serde_json::json!({
            "format": {
                "filename": "song.flac",
                "nb_streams": streams.len(),
                "format_name": "flac",
                "format_long_name": "raw FLAC",
                "duration": "192.500",
                "size": "22176000",
                "bit_rate": "921600",
                "tags": {
                    "TITLE": "Song Title",
                    "ARTIST": "Example Artist",
                    "album_artist": "Various Artists",
                    "album": "Example Album",
                    "DATE": "2024",
                    "track": "3/10",
                    "discnumber": "1/2",
                    "genre": "Jazz",
                    "COMMENT": "Loose note",
                    "composer": "Example Composer",
                    "encoder": "reference encoder",
                    "mood": "Reflective",
                    "replaygain_track_gain": "-7.50 dB",
                    "nested": {
                        "ignored": true
                    }
                }
            },
            "streams": streams,
            "programs": [],
            "chapters": [
                {
                    "id": 1,
                    "start_time": "0.000000",
                    "end_time": "15.000000",
                    "tags": {
                        "title": "Intro"
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

    fn assert_render_image_size(preview: &PropertyImagePreview, width: u32, height: u32) {
        let size = preview.image.size(0);
        assert_eq!(size.width.0, width as i32);
        assert_eq!(size.height.0, height as i32);
    }

    fn push_ifd_entry(tiff: &mut Vec<u8>, tag: u16, field_type: u16, count: u32, value: u32) {
        tiff.extend_from_slice(&tag.to_le_bytes());
        tiff.extend_from_slice(&field_type.to_le_bytes());
        tiff.extend_from_slice(&count.to_le_bytes());
        tiff.extend_from_slice(&value.to_le_bytes());
    }
}
