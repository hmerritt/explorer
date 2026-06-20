use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures::{StreamExt, stream::FuturesUnordered};
use gpui::{App, Context, Global, Image};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use crate::{
    explorer::{
        entry::FileEntry,
        image_preview::{
            ImageThumbnailExtractionTimings, load_image_thumbnail_png_with_cancel_timed,
            path_may_have_image_preview,
        },
        video::{
            ffmpeg_seek_argument, ffprobe_duration_seconds_from_probe,
            path_may_have_video_metadata, video_thumbnail_frame_seek_seconds,
        },
        view::ExplorerView,
    },
    settings::{APP_ID, ConfigPlatform, config_dir_for},
};

const IMAGE_THUMBNAIL_CACHE_VERSION: &str = "image-thumbnails-v4";
const IMAGE_THUMBNAIL_SIZE: u32 = 128;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub(super) struct ImageThumbnailCache {
    inner: RefCell<ImageThumbnailCacheInner>,
}

impl Global for ImageThumbnailCache {}

impl ImageThumbnailCache {
    fn new() -> Self {
        Self {
            inner: RefCell::new(ImageThumbnailCacheInner::new(image_thumbnail_cache_dir())),
        }
    }
}

pub(crate) fn initialize(cx: &mut App) {
    cx.set_global(ImageThumbnailCache::new());
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ImageThumbnailRequest {
    kind: ImageThumbnailKind,
    key: String,
    path: PathBuf,
    directory: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ImageThumbnailKind {
    Image,
    Video,
}

impl ImageThumbnailKind {
    fn cache_namespace(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
        }
    }
}

struct ImageThumbnailCacheInner {
    cache_dir: Option<PathBuf>,
    states: HashMap<String, ImageThumbnailState>,
    pending: VecDeque<String>,
    loader_running: bool,
    loader_generation: u64,
}

enum ImageThumbnailState {
    Pending {
        request: ImageThumbnailRequest,
        queued_at: Instant,
    },
    Loading {
        request: ImageThumbnailRequest,
        generation: u64,
        cancel: Arc<AtomicBool>,
    },
    Ready(Arc<Image>),
    Failed,
}

struct ImageThumbnailLoadJob {
    request: ImageThumbnailRequest,
    queued_at: Instant,
    cache_dir: Option<PathBuf>,
    generation: u64,
    cancel: Arc<AtomicBool>,
}

struct ImageThumbnailCacheWriteJob {
    cache_dir: PathBuf,
    key: String,
    bytes: Vec<u8>,
}

impl ImageThumbnailCacheInner {
    fn new(cache_dir: Option<PathBuf>) -> Self {
        Self {
            cache_dir,
            states: HashMap::new(),
            pending: VecDeque::new(),
            loader_running: false,
            loader_generation: 0,
        }
    }

    fn thumbnail_for_request(
        &mut self,
        request: ImageThumbnailRequest,
    ) -> (Option<Arc<Image>>, Option<u64>) {
        if let Some(state) = self.states.get(&request.key) {
            return (state.thumbnail(), None);
        }

        self.pending.push_back(request.key.clone());
        self.states.insert(
            request.key.clone(),
            ImageThumbnailState::Pending {
                request,
                queued_at: Instant::now(),
            },
        );

        (None, self.start_loader())
    }

    fn start_loader(&mut self) -> Option<u64> {
        if self.loader_running || self.pending.is_empty() {
            return None;
        }

        self.loader_generation = self.loader_generation.wrapping_add(1);
        self.loader_running = true;
        Some(self.loader_generation)
    }

    fn next_load_job(&mut self, generation: u64) -> Option<ImageThumbnailLoadJob> {
        if !self.loader_running || self.loader_generation != generation {
            return None;
        }

        while let Some(key) = self.pending.pop_front() {
            let Some(ImageThumbnailState::Pending { request, queued_at }) =
                self.states.remove(&key)
            else {
                continue;
            };

            let cancel = Arc::new(AtomicBool::new(false));
            self.states.insert(
                key,
                ImageThumbnailState::Loading {
                    request: request.clone(),
                    generation,
                    cancel: cancel.clone(),
                },
            );

            return Some(ImageThumbnailLoadJob {
                request,
                queued_at,
                cache_dir: self.cache_dir.clone(),
                generation,
                cancel,
            });
        }

        self.loader_running = false;
        None
    }

    fn finish_request(
        &mut self,
        request: ImageThumbnailRequest,
        generation: u64,
        bytes: Option<Vec<u8>>,
    ) -> bool {
        let should_finish = self.states.get(&request.key).is_some_and(|state| {
            matches!(
                state,
                ImageThumbnailState::Loading {
                    request: loading_request,
                    generation: loading_generation,
                    ..
                } if loading_request == &request && *loading_generation == generation
            )
        });

        if !should_finish {
            return false;
        }

        let state = match bytes.and_then(valid_png_bytes) {
            Some(bytes) => ImageThumbnailState::Ready(image_from_png_bytes(bytes)),
            None => ImageThumbnailState::Failed,
        };

        self.states.insert(request.key, state);
        true
    }

    fn cancel_directory(&mut self, directory: &Path) -> Option<u64> {
        self.pending.retain(|key| {
            !matches!(
                self.states.get(key),
                Some(ImageThumbnailState::Pending { request, .. })
                    if request.directory == directory
            )
        });

        let mut cancelled_loading = false;
        self.states.retain(|_, state| match state {
            ImageThumbnailState::Pending { request, .. } if request.directory == directory => false,
            ImageThumbnailState::Loading {
                request, cancel, ..
            } if request.directory == directory => {
                cancel.store(true, Ordering::Relaxed);
                cancelled_loading = true;
                false
            }
            _ => true,
        });

        if cancelled_loading {
            self.loader_running = false;
            self.loader_generation = self.loader_generation.wrapping_add(1);
        }

        self.start_loader()
    }
}

impl ImageThumbnailState {
    fn thumbnail(&self) -> Option<Arc<Image>> {
        match self {
            Self::Ready(image) => Some(image.clone()),
            Self::Pending { .. } | Self::Loading { .. } | Self::Failed => None,
        }
    }
}

impl ExplorerView {
    pub(super) fn observe_image_thumbnail_cache(&mut self, cx: &mut Context<Self>) {
        cx.observe_global::<ImageThumbnailCache>(|_, cx| cx.notify())
            .detach();
    }

    pub(super) fn image_thumbnail_for_entry(
        &mut self,
        entry: &FileEntry,
        cx: &mut Context<Self>,
    ) -> Option<Arc<Image>> {
        let request = image_thumbnail_request_for_entry(entry, &self.path)?;
        let (thumbnail, loader_generation) = cx
            .try_global::<ImageThumbnailCache>()
            .map(|cache| cache.inner.borrow_mut().thumbnail_for_request(request))
            .unwrap_or((None, None));

        if let Some(generation) = loader_generation {
            start_image_thumbnail_loader(cx, generation);
        }

        thumbnail
    }

    pub(super) fn cancel_image_thumbnail_extraction(&mut self, cx: &mut Context<Self>) {
        let directory = self.path.clone();
        let loader_generation = cx
            .try_global::<ImageThumbnailCache>()
            .and_then(|cache| cache.inner.borrow_mut().cancel_directory(&directory));

        if let Some(generation) = loader_generation {
            start_image_thumbnail_loader(cx, generation);
        }
    }
}

fn start_image_thumbnail_loader(cx: &mut Context<ExplorerView>, generation: u64) {
    cx.spawn(async move |_, cx| {
        let mut timings = ImageThumbnailTimingBatch::start();
        let concurrency = image_thumbnail_loader_concurrency();
        let mut in_flight = FuturesUnordered::new();

        loop {
            while in_flight.len() < concurrency {
                let job = cx
                    .update(|cx| {
                        cx.try_global::<ImageThumbnailCache>()
                            .and_then(|cache| cache.inner.borrow_mut().next_load_job(generation))
                    })
                    .ok()
                    .flatten();
                let Some(job) = job else {
                    break;
                };

                let request_started = timings.now();
                timings.record_request();
                timings.record_queue_wait(job.queued_at.elapsed());

                let timings_enabled = timings.enabled();
                let load_task = cx.background_executor().spawn(async move {
                    let thumbnail = load_or_create_thumbnail_png_with_timings(
                        &job.request,
                        job.cache_dir.as_deref(),
                        &job.cancel,
                        timings_enabled,
                    );
                    (job, request_started, thumbnail)
                });
                in_flight.push(load_task);
            }

            let Some((job, request_started, thumbnail)) = in_flight.next().await else {
                break;
            };
            timings.record_load_result(&thumbnail);
            let cache_write = thumbnail.cache_write_job(&job);

            let commit_started = timings.now();
            let finished = cx
                .update_global::<ImageThumbnailCache, _>(|cache, _| {
                    cache.inner.borrow_mut().finish_request(
                        job.request,
                        job.generation,
                        thumbnail.bytes,
                    )
                })
                .unwrap_or(false);
            timings.record_commit(commit_started);
            if !finished {
                timings.record_discarded();
            }
            timings.record_request_total(request_started);

            if finished && let Some(cache_write) = cache_write {
                let write_task = cx.background_executor().spawn(async move {
                    write_cached_thumbnail(
                        Some(&cache_write.cache_dir),
                        &cache_write.key,
                        &cache_write.bytes,
                    )
                });
                write_task.detach();
                timings.record_cache_write_scheduled();
            }
        }

        timings.finish();
    })
    .detach();
}

fn image_thumbnail_loader_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4)
        .clamp(2, 8)
}

#[cfg(test)]
fn load_or_create_thumbnail_png(
    request: &ImageThumbnailRequest,
    cache_dir: Option<&Path>,
    cancel: &AtomicBool,
) -> Option<Vec<u8>> {
    load_or_create_thumbnail_png_with_timings(request, cache_dir, cancel, false).bytes
}

fn load_or_create_thumbnail_png_with_timings(
    request: &ImageThumbnailRequest,
    cache_dir: Option<&Path>,
    cancel: &AtomicBool,
    timings_enabled: bool,
) -> ImageThumbnailLoadResult {
    if cancel.load(Ordering::Relaxed) {
        return ImageThumbnailLoadResult::cancelled();
    }

    let cache_read_started = timings_enabled.then(Instant::now);
    let cached = read_cached_thumbnail(cache_dir, &request.key);
    let cache_read_elapsed = cache_read_started.map(|started| started.elapsed());
    let cache_hit = cached.is_some();
    if let Some(bytes) = cached {
        if cancel.load(Ordering::Relaxed) {
            return ImageThumbnailLoadResult::cancelled_after_cache_read(
                cache_hit,
                cache_read_elapsed,
            );
        }

        return ImageThumbnailLoadResult::cache_hit(bytes, cache_read_elapsed);
    }

    if cancel.load(Ordering::Relaxed) {
        return ImageThumbnailLoadResult::cancelled_after_cache_read(cache_hit, cache_read_elapsed);
    }

    let extract_started = timings_enabled.then(Instant::now);
    let (result, extraction_timings) = match request.kind {
        ImageThumbnailKind::Image => {
            let extracted = load_image_thumbnail_png_with_cancel_timed(
                &request.path,
                IMAGE_THUMBNAIL_SIZE,
                cancel,
                timings_enabled,
            );
            (extracted.result, extracted.timings)
        }
        ImageThumbnailKind::Video => (
            load_video_thumbnail_png_with_cancel(&request.path, IMAGE_THUMBNAIL_SIZE, cancel),
            ImageThumbnailExtractionTimings::default(),
        ),
    };
    let bytes = match result {
        Ok(bytes) => bytes,
        Err(_) if cancel.load(Ordering::Relaxed) => {
            return ImageThumbnailLoadResult::cancelled_after_extract(
                cache_read_elapsed,
                extract_started.map(|started| started.elapsed()),
                extraction_timings,
            );
        }
        Err(_) => {
            return ImageThumbnailLoadResult::failed(
                cache_read_elapsed,
                extract_started.map(|started| started.elapsed()),
                extraction_timings,
            );
        }
    };
    let extract_elapsed = extract_started.map(|started| started.elapsed());
    if cancel.load(Ordering::Relaxed) {
        return ImageThumbnailLoadResult::cancelled_after_extract(
            cache_read_elapsed,
            extract_elapsed,
            extraction_timings,
        );
    }

    ImageThumbnailLoadResult::generated(
        bytes,
        cache_read_elapsed,
        extract_elapsed,
        None,
        extraction_timings,
    )
}

fn load_video_thumbnail_png_with_cancel(
    path: &Path,
    size: u32,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    if size == 0 {
        return Err("Thumbnail target has no dimensions.".to_owned());
    }

    check_thumbnail_cancelled(cancel)?;
    if !ffmpeg_sidecar::ffprobe::ffprobe_is_installed() {
        return Err(
            "ffprobe is not available. Install FFmpeg/ffprobe or place ffprobe beside Explorer."
                .to_owned(),
        );
    }
    if !ffmpeg_sidecar::command::ffmpeg_is_installed() {
        return Err(
            "ffmpeg is not available. Install FFmpeg/ffprobe or place ffmpeg beside Explorer."
                .to_owned(),
        );
    }

    check_thumbnail_cancelled(cancel)?;
    let duration = video_thumbnail_duration_seconds(path)?;
    let seek = video_thumbnail_frame_seek_seconds(duration)
        .ok_or_else(|| "Video duration is not long enough to extract a thumbnail.".to_owned())?;
    check_thumbnail_cancelled(cancel)?;

    let fast_result = ffmpeg_video_thumbnail_png_output(path, seek, size, true);
    match fast_result {
        Ok(png) if png.starts_with(PNG_SIGNATURE) => return Ok(png),
        Ok(png) => {
            check_thumbnail_cancelled(cancel)?;
            let fast_error = format!("ffmpeg returned {} bytes, but not a PNG image", png.len());
            retry_video_thumbnail_png(path, seek, size, fast_error)
        }
        Err(fast_error) => {
            check_thumbnail_cancelled(cancel)?;
            retry_video_thumbnail_png(path, seek, size, fast_error)
        }
    }
}

fn retry_video_thumbnail_png(
    path: &Path,
    seek_seconds: f64,
    size: u32,
    fast_error: String,
) -> Result<Vec<u8>, String> {
    match ffmpeg_video_thumbnail_png_output(path, seek_seconds, size, false) {
        Ok(png) if png.starts_with(PNG_SIGNATURE) => Ok(png),
        Ok(png) => Err(format!(
            "ffmpeg returned {} bytes, but not a PNG image; fast attempt also failed: {fast_error}",
            png.len()
        )),
        Err(error) => Err(format!("{error}; fast attempt also failed: {fast_error}")),
    }
}

fn video_thumbnail_duration_seconds(path: &Path) -> Result<f64, String> {
    let output = ffprobe_video_duration_json_output(path)
        .map_err(|error| format!("ffprobe failed: {error}"))?;
    let probe: serde_json::Value = serde_json::from_slice(&output)
        .map_err(|error| format!("ffprobe returned unreadable duration data: {error}"))?;
    ffprobe_duration_seconds_from_probe(&probe)
        .ok_or_else(|| "Video duration is not available.".to_owned())
}

fn ffprobe_video_duration_json_output(path: &Path) -> Result<Vec<u8>, String> {
    let mut command = Command::new(ffmpeg_sidecar::ffprobe::ffprobe_path());
    command
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("v:0")
        .arg("-show_entries")
        .arg("format=duration:stream=codec_type,duration")
        .arg("-of")
        .arg("json")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let output = command
        .output()
        .map_err(|error| format!("could not start ffprobe: {error}"))?;
    if output.status.success() {
        return Ok(output.stdout);
    }

    let stderr = command_error_output_label(&output.stderr);
    if stderr.is_empty() {
        Err(format!("ffprobe exited with {}", output.status))
    } else {
        Err(format!("ffprobe exited with {}: {stderr}", output.status))
    }
}

fn ffmpeg_video_thumbnail_png_output(
    path: &Path,
    seek_seconds: f64,
    size: u32,
    keyframe_only: bool,
) -> Result<Vec<u8>, String> {
    let mut command = Command::new(ffmpeg_sidecar::paths::ffmpeg_path());
    for arg in ffmpeg_video_thumbnail_args(path, seek_seconds, size, keyframe_only) {
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

fn ffmpeg_video_thumbnail_args(
    path: &Path,
    seek_seconds: f64,
    size: u32,
    keyframe_only: bool,
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("-v"),
        OsString::from("error"),
        OsString::from("-nostdin"),
        OsString::from("-noaccurate_seek"),
    ];
    if keyframe_only {
        args.push(OsString::from("-skip_frame"));
        args.push(OsString::from("nokey"));
    }
    args.extend([
        OsString::from("-ss"),
        OsString::from(ffmpeg_seek_argument(seek_seconds)),
        OsString::from("-i"),
        path.as_os_str().to_owned(),
        OsString::from("-map"),
        OsString::from("0:v:0"),
        OsString::from("-frames:v"),
        OsString::from("1"),
        OsString::from("-vf"),
        OsString::from(video_thumbnail_scale_filter(size)),
        OsString::from("-f"),
        OsString::from("image2pipe"),
        OsString::from("-vcodec"),
        OsString::from("png"),
        OsString::from("-"),
    ]);
    args
}

fn video_thumbnail_scale_filter(size: u32) -> String {
    format!("scale={size}:{size}:force_original_aspect_ratio=decrease:flags=fast_bilinear")
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

fn check_thumbnail_cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Thumbnail loading was cancelled.".to_owned())
    } else {
        Ok(())
    }
}

struct ImageThumbnailLoadResult {
    bytes: Option<Vec<u8>>,
    cache_hit: Option<bool>,
    cache_read_elapsed: Option<Duration>,
    extract_elapsed: Option<Duration>,
    cache_write_elapsed: Option<Duration>,
    extraction_timings: ImageThumbnailExtractionTimings,
    outcome: ImageThumbnailLoadOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ImageThumbnailLoadOutcome {
    CacheHit,
    Generated,
    Failed,
    Cancelled,
}

impl ImageThumbnailLoadResult {
    fn cache_hit(bytes: Vec<u8>, cache_read_elapsed: Option<Duration>) -> Self {
        Self {
            bytes: Some(bytes),
            cache_hit: Some(true),
            cache_read_elapsed,
            extract_elapsed: None,
            cache_write_elapsed: None,
            extraction_timings: ImageThumbnailExtractionTimings::default(),
            outcome: ImageThumbnailLoadOutcome::CacheHit,
        }
    }

    fn generated(
        bytes: Vec<u8>,
        cache_read_elapsed: Option<Duration>,
        extract_elapsed: Option<Duration>,
        cache_write_elapsed: Option<Duration>,
        extraction_timings: ImageThumbnailExtractionTimings,
    ) -> Self {
        Self {
            bytes: Some(bytes),
            cache_hit: Some(false),
            cache_read_elapsed,
            extract_elapsed,
            cache_write_elapsed,
            extraction_timings,
            outcome: ImageThumbnailLoadOutcome::Generated,
        }
    }

    fn failed(
        cache_read_elapsed: Option<Duration>,
        extract_elapsed: Option<Duration>,
        extraction_timings: ImageThumbnailExtractionTimings,
    ) -> Self {
        Self {
            bytes: None,
            cache_hit: Some(false),
            cache_read_elapsed,
            extract_elapsed,
            cache_write_elapsed: None,
            extraction_timings,
            outcome: ImageThumbnailLoadOutcome::Failed,
        }
    }

    fn cancelled() -> Self {
        Self {
            bytes: None,
            cache_hit: None,
            cache_read_elapsed: None,
            extract_elapsed: None,
            cache_write_elapsed: None,
            extraction_timings: ImageThumbnailExtractionTimings::default(),
            outcome: ImageThumbnailLoadOutcome::Cancelled,
        }
    }

    fn cancelled_after_cache_read(cache_hit: bool, cache_read_elapsed: Option<Duration>) -> Self {
        Self {
            bytes: None,
            cache_hit: Some(cache_hit),
            cache_read_elapsed,
            extract_elapsed: None,
            cache_write_elapsed: None,
            extraction_timings: ImageThumbnailExtractionTimings::default(),
            outcome: ImageThumbnailLoadOutcome::Cancelled,
        }
    }

    fn cancelled_after_extract(
        cache_read_elapsed: Option<Duration>,
        extract_elapsed: Option<Duration>,
        extraction_timings: ImageThumbnailExtractionTimings,
    ) -> Self {
        Self {
            bytes: None,
            cache_hit: Some(false),
            cache_read_elapsed,
            extract_elapsed,
            cache_write_elapsed: None,
            extraction_timings,
            outcome: ImageThumbnailLoadOutcome::Cancelled,
        }
    }

    fn cache_write_job(&self, job: &ImageThumbnailLoadJob) -> Option<ImageThumbnailCacheWriteJob> {
        if self.outcome != ImageThumbnailLoadOutcome::Generated {
            return None;
        }

        Some(ImageThumbnailCacheWriteJob {
            cache_dir: job.cache_dir.clone()?,
            key: job.request.key.clone(),
            bytes: self.bytes.clone()?,
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ImageThumbnailTimingBatch {
    enabled: bool,
    batch_started: Option<Instant>,
    requests: usize,
    cache_hits: usize,
    cache_misses: usize,
    generated: usize,
    failed: usize,
    cancelled: usize,
    discarded: usize,
    cache_writes_scheduled: usize,
    queue_wait: ImageThumbnailStageTimingStats,
    cache_read: ImageThumbnailStageTimingStats,
    extract: ImageThumbnailStageTimingStats,
    embedded_thumbnail_scan: ImageThumbnailStageTimingStats,
    embedded_thumbnail_decode: ImageThumbnailStageTimingStats,
    source_read: ImageThumbnailStageTimingStats,
    format_detect: ImageThumbnailStageTimingStats,
    raster_decode: ImageThumbnailStageTimingStats,
    rgba_convert: ImageThumbnailStageTimingStats,
    tiff_ifd_scan: ImageThumbnailStageTimingStats,
    tiff_raw_sample: ImageThumbnailStageTimingStats,
    tiff_chunk_decode: ImageThumbnailStageTimingStats,
    tiff_chunk_sample: ImageThumbnailStageTimingStats,
    svg_parse: ImageThumbnailStageTimingStats,
    svg_render: ImageThumbnailStageTimingStats,
    svg_unpremultiply: ImageThumbnailStageTimingStats,
    resize_canvas: ImageThumbnailStageTimingStats,
    png_encode: ImageThumbnailStageTimingStats,
    cache_write: ImageThumbnailStageTimingStats,
    commit: ImageThumbnailStageTimingStats,
    request_total: ImageThumbnailStageTimingStats,
}

impl ImageThumbnailTimingBatch {
    fn start() -> Self {
        let enabled = crate::debug_options::icon_timings_enabled();
        Self {
            enabled,
            batch_started: enabled.then(Instant::now),
            ..Self::default()
        }
    }

    #[cfg(test)]
    fn enabled_for_test() -> Self {
        Self {
            enabled: true,
            batch_started: Some(Instant::now()),
            ..Self::default()
        }
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    fn now(&self) -> Option<Instant> {
        self.enabled.then(Instant::now)
    }

    fn record_request(&mut self) {
        if self.enabled {
            self.requests += 1;
        }
    }

    fn record_queue_wait(&mut self, elapsed: Duration) {
        if self.enabled {
            self.queue_wait.record(elapsed);
        }
    }

    fn record_load_result(&mut self, result: &ImageThumbnailLoadResult) {
        if !self.enabled {
            return;
        }

        if let Some(elapsed) = result.cache_read_elapsed {
            self.cache_read.record(elapsed);
        }
        if let Some(cache_hit) = result.cache_hit {
            if cache_hit {
                self.cache_hits += 1;
            } else {
                self.cache_misses += 1;
            }
        }
        if let Some(elapsed) = result.extract_elapsed {
            self.extract.record(elapsed);
        }
        self.record_extraction_timings(&result.extraction_timings);
        if let Some(elapsed) = result.cache_write_elapsed {
            self.cache_write.record(elapsed);
        }

        match result.outcome {
            ImageThumbnailLoadOutcome::CacheHit => {}
            ImageThumbnailLoadOutcome::Generated => self.generated += 1,
            ImageThumbnailLoadOutcome::Failed => self.failed += 1,
            ImageThumbnailLoadOutcome::Cancelled => self.cancelled += 1,
        }
    }

    fn record_commit(&mut self, started: Option<Instant>) {
        if !self.enabled {
            return;
        }

        if let Some(started) = started {
            self.commit.record(started.elapsed());
        }
    }

    fn record_extraction_timings(&mut self, timings: &ImageThumbnailExtractionTimings) {
        record_image_thumbnail_stage_if_some(
            &mut self.embedded_thumbnail_scan,
            timings.embedded_thumbnail_scan,
        );
        record_image_thumbnail_stage_if_some(
            &mut self.embedded_thumbnail_decode,
            timings.embedded_thumbnail_decode,
        );
        record_image_thumbnail_stage_if_some(&mut self.source_read, timings.source_read);
        record_image_thumbnail_stage_if_some(&mut self.format_detect, timings.format_detect);
        record_image_thumbnail_stage_if_some(&mut self.raster_decode, timings.raster_decode);
        record_image_thumbnail_stage_if_some(&mut self.rgba_convert, timings.rgba_convert);
        record_image_thumbnail_stage_if_some(&mut self.tiff_ifd_scan, timings.tiff_ifd_scan);
        record_image_thumbnail_stage_if_some(&mut self.tiff_raw_sample, timings.tiff_raw_sample);
        record_image_thumbnail_stage_if_some(
            &mut self.tiff_chunk_decode,
            timings.tiff_chunk_decode,
        );
        record_image_thumbnail_stage_if_some(
            &mut self.tiff_chunk_sample,
            timings.tiff_chunk_sample,
        );
        record_image_thumbnail_stage_if_some(&mut self.svg_parse, timings.svg_parse);
        record_image_thumbnail_stage_if_some(&mut self.svg_render, timings.svg_render);
        record_image_thumbnail_stage_if_some(
            &mut self.svg_unpremultiply,
            timings.svg_unpremultiply,
        );
        record_image_thumbnail_stage_if_some(&mut self.resize_canvas, timings.resize_canvas);
        record_image_thumbnail_stage_if_some(&mut self.png_encode, timings.png_encode);
    }

    fn record_discarded(&mut self) {
        if self.enabled {
            self.discarded += 1;
        }
    }

    fn record_cache_write_scheduled(&mut self) {
        if self.enabled {
            self.cache_writes_scheduled += 1;
        }
    }

    fn record_request_total(&mut self, started: Option<Instant>) {
        if !self.enabled {
            return;
        }

        if let Some(started) = started {
            self.request_total.record(started.elapsed());
        }
    }

    fn finish(self) {
        if !self.enabled {
            return;
        }

        let batch_total = self
            .batch_started
            .map(|started| started.elapsed())
            .unwrap_or_default();
        for line in self.format_lines(batch_total) {
            crate::debug_options::log_icon_timing(format_args!("{line}"));
        }
    }

    fn format_lines(&self, batch_total: Duration) -> Vec<String> {
        if self.requests == 0 {
            return Vec::new();
        }

        let mut lines = vec![format!(
            "image_thumbnails total={} requests={} cache_hits={} cache_misses={} generated={} failed={} cancelled={} discarded={} cache_writes_scheduled={}",
            format_image_thumbnail_timing_duration(batch_total),
            self.requests,
            self.cache_hits,
            self.cache_misses,
            self.generated,
            self.failed,
            self.cancelled,
            self.discarded,
            self.cache_writes_scheduled
        )];
        push_image_thumbnail_stage_line(&mut lines, "queue_wait", &self.queue_wait);
        push_image_thumbnail_stage_line(&mut lines, "cache_read", &self.cache_read);
        push_image_thumbnail_stage_line(&mut lines, "extract", &self.extract);
        push_image_thumbnail_stage_line(
            &mut lines,
            "embedded_thumbnail_scan",
            &self.embedded_thumbnail_scan,
        );
        push_image_thumbnail_stage_line(
            &mut lines,
            "embedded_thumbnail_decode",
            &self.embedded_thumbnail_decode,
        );
        push_image_thumbnail_stage_line(&mut lines, "source_read", &self.source_read);
        push_image_thumbnail_stage_line(&mut lines, "format_detect", &self.format_detect);
        push_image_thumbnail_stage_line(&mut lines, "raster_decode", &self.raster_decode);
        push_image_thumbnail_stage_line(&mut lines, "rgba_convert", &self.rgba_convert);
        push_image_thumbnail_stage_line(&mut lines, "tiff_ifd_scan", &self.tiff_ifd_scan);
        push_image_thumbnail_stage_line(&mut lines, "tiff_raw_sample", &self.tiff_raw_sample);
        push_image_thumbnail_stage_line(&mut lines, "tiff_chunk_decode", &self.tiff_chunk_decode);
        push_image_thumbnail_stage_line(&mut lines, "tiff_chunk_sample", &self.tiff_chunk_sample);
        push_image_thumbnail_stage_line(&mut lines, "svg_parse", &self.svg_parse);
        push_image_thumbnail_stage_line(&mut lines, "svg_render", &self.svg_render);
        push_image_thumbnail_stage_line(&mut lines, "svg_unpremultiply", &self.svg_unpremultiply);
        push_image_thumbnail_stage_line(&mut lines, "resize_canvas", &self.resize_canvas);
        push_image_thumbnail_stage_line(&mut lines, "png_encode", &self.png_encode);
        push_image_thumbnail_stage_line(&mut lines, "cache_write", &self.cache_write);
        push_image_thumbnail_stage_line(&mut lines, "commit", &self.commit);
        push_image_thumbnail_stage_line(&mut lines, "request_total", &self.request_total);
        lines
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ImageThumbnailStageTimingStats {
    count: usize,
    total: Duration,
    fastest: Option<Duration>,
    slowest: Option<Duration>,
}

impl ImageThumbnailStageTimingStats {
    fn record(&mut self, elapsed: Duration) {
        self.count += 1;
        self.total += elapsed;
        self.fastest = Some(self.fastest.map_or(elapsed, |fastest| fastest.min(elapsed)));
        self.slowest = Some(self.slowest.map_or(elapsed, |slowest| slowest.max(elapsed)));
    }

    fn format_line(&self, stage: &str) -> Option<String> {
        if self.count == 0 {
            return None;
        }

        Some(format!(
            "image_thumbnails {stage} count={} total={} fastest={} slowest={}",
            self.count,
            format_image_thumbnail_timing_duration(self.total),
            format_image_thumbnail_timing_duration(self.fastest.unwrap_or_default()),
            format_image_thumbnail_timing_duration(self.slowest.unwrap_or_default())
        ))
    }
}

fn push_image_thumbnail_stage_line(
    lines: &mut Vec<String>,
    stage: &str,
    stats: &ImageThumbnailStageTimingStats,
) {
    if let Some(line) = stats.format_line(stage) {
        lines.push(line);
    }
}

fn record_image_thumbnail_stage_if_some(
    stats: &mut ImageThumbnailStageTimingStats,
    elapsed: Option<Duration>,
) {
    if let Some(elapsed) = elapsed {
        stats.record(elapsed);
    }
}

fn format_image_thumbnail_timing_duration(elapsed: Duration) -> String {
    format!("{:.3}ms", elapsed.as_secs_f64() * 1000.0)
}

fn image_thumbnail_request_for_entry(
    entry: &FileEntry,
    directory: &Path,
) -> Option<ImageThumbnailRequest> {
    if entry.is_directory_like() {
        return None;
    }

    let kind = image_thumbnail_kind_for_path(&entry.path)?;

    Some(ImageThumbnailRequest {
        kind,
        key: image_thumbnail_key(entry, kind),
        path: entry.path.clone(),
        directory: directory.to_path_buf(),
    })
}

fn image_thumbnail_kind_for_path(path: &Path) -> Option<ImageThumbnailKind> {
    if path_may_have_image_preview(path) {
        Some(ImageThumbnailKind::Image)
    } else if path_may_have_video_metadata(path) {
        Some(ImageThumbnailKind::Video)
    } else {
        None
    }
}

fn image_thumbnail_key(entry: &FileEntry, kind: ImageThumbnailKind) -> String {
    let mut hash = StableHash::new();
    hash.write_str(IMAGE_THUMBNAIL_CACHE_VERSION);
    hash.write_str(kind.cache_namespace());
    hash.write_str(&normalized_path_key(&entry.path));
    hash.write_u64(entry.size.unwrap_or(0));
    hash.write_u64(system_time_key(entry.modified));
    format!("{:016x}", hash.finish())
}

fn system_time_key(time: Option<SystemTime>) -> u64 {
    time.and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| {
            duration
                .as_secs()
                .saturating_mul(1_000_000_000)
                .saturating_add(u64::from(duration.subsec_nanos()))
        })
        .unwrap_or(0)
}

fn read_cached_thumbnail(cache_dir: Option<&Path>, key: &str) -> Option<Vec<u8>> {
    fs::read(thumbnail_file_path(cache_dir, key)?)
        .ok()
        .and_then(valid_png_bytes)
}

fn write_cached_thumbnail(cache_dir: Option<&Path>, key: &str, bytes: &[u8]) -> bool {
    let Some(path) = thumbnail_file_path(cache_dir, key) else {
        return false;
    };
    let _ = write_atomic(&path, bytes);
    true
}

fn thumbnail_file_path(cache_dir: Option<&Path>, key: &str) -> Option<PathBuf> {
    if key.is_empty()
        || key
            .chars()
            .any(|ch| !ch.is_ascii_hexdigit() || ch.is_ascii_uppercase())
    {
        return None;
    }

    Some(cache_dir?.join(format!("{key}.png")))
}

fn image_from_png_bytes(bytes: Vec<u8>) -> Arc<Image> {
    Arc::new(Image::from_bytes(gpui::ImageFormat::Png, bytes))
}

fn valid_png_bytes(bytes: Vec<u8>) -> Option<Vec<u8>> {
    bytes.starts_with(PNG_SIGNATURE).then_some(bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, bytes)?;
    match fs::rename(&tmp_path, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(path);
            fs::rename(&tmp_path, path).map_err(|_| error)
        }
    }
}

fn image_thumbnail_cache_dir() -> Option<PathBuf> {
    platform_cache_dir(current_config_platform(), env_path)
        .map(|dir| dir.join(IMAGE_THUMBNAIL_CACHE_VERSION))
}

fn current_config_platform() -> ConfigPlatform {
    if cfg!(target_os = "macos") {
        ConfigPlatform::MacOS
    } else if cfg!(target_os = "windows") {
        ConfigPlatform::Windows
    } else {
        ConfigPlatform::Linux
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn platform_cache_dir(
    platform: ConfigPlatform,
    mut env_path: impl FnMut(&str) -> Option<PathBuf>,
) -> Option<PathBuf> {
    match platform {
        ConfigPlatform::MacOS => {
            env_path("HOME").map(|home| home.join(".config").join("explorer").join("cache"))
        }
        ConfigPlatform::Linux => env_path("XDG_CACHE_HOME")
            .map(|cache_home| cache_home.join("explorer"))
            .or_else(|| env_path("HOME").map(|home| home.join(".cache").join("explorer"))),
        ConfigPlatform::Windows => env_path("LOCALAPPDATA")
            .map(|local_appdata| local_appdata.join(APP_ID).join("cache"))
            .or_else(|| {
                config_dir_for(ConfigPlatform::Windows, env_path).map(|dir| dir.join("cache"))
            }),
    }
}

fn normalized_path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

struct StableHash(u64);

impl StableHash {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn finish(self) -> u64 {
        self.0
    }

    fn write_u8(&mut self, value: u8) {
        self.write_bytes(&[value]);
    }

    fn write_u64(&mut self, value: u64) {
        self.write_bytes(&value.to_le_bytes());
    }

    fn write_str(&mut self, value: &str) {
        self.write_bytes(value.as_bytes());
        self.write_u8(0xff);
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::{entry::FileEntry, test_support::TempDir};
    use std::io::Cursor;

    #[test]
    fn thumbnail_requests_include_supported_image_extensions() {
        for name in ["image.png", "photo.jpg", "poster.webp", "vector.svg"] {
            let entry = FileEntry::test(name, false, Some(1), Some(UNIX_EPOCH));
            let request = image_thumbnail_request_for_entry(&entry, Path::new("folder"))
                .unwrap_or_else(|| panic!("expected request for {name}"));
            assert_eq!(request.kind, ImageThumbnailKind::Image);
        }
    }

    #[test]
    fn thumbnail_requests_include_supported_video_extensions() {
        for name in ["movie.mp4", "clip.mkv", "camera.mov"] {
            let entry = FileEntry::test(name, false, Some(1), Some(UNIX_EPOCH));
            let request = image_thumbnail_request_for_entry(&entry, Path::new("folder"))
                .unwrap_or_else(|| panic!("expected request for {name}"));
            assert_eq!(request.kind, ImageThumbnailKind::Video);
        }
    }

    #[test]
    fn thumbnail_requests_skip_directories_and_non_images() {
        assert!(
            image_thumbnail_request_for_entry(
                &FileEntry::test("folder", true, None, Some(UNIX_EPOCH)),
                Path::new("folder")
            )
            .is_none()
        );
        assert!(
            image_thumbnail_request_for_entry(
                &FileEntry::test("notes.txt", false, Some(1), Some(UNIX_EPOCH)),
                Path::new("folder")
            )
            .is_none()
        );
    }

    #[test]
    fn thumbnail_key_changes_when_file_metadata_changes() {
        let first = FileEntry::test("image.png", false, Some(1), Some(UNIX_EPOCH));
        let second = FileEntry::test(
            "image.png",
            false,
            Some(2),
            Some(UNIX_EPOCH + Duration::from_secs(1)),
        );

        assert_ne!(
            image_thumbnail_key(&first, ImageThumbnailKind::Image),
            image_thumbnail_key(&second, ImageThumbnailKind::Image)
        );
    }

    #[test]
    fn thumbnail_keys_are_namespaced_by_media_kind() {
        let entry = FileEntry::test("clip.mp4", false, Some(1), Some(UNIX_EPOCH));

        assert_ne!(
            image_thumbnail_key(&entry, ImageThumbnailKind::Image),
            image_thumbnail_key(&entry, ImageThumbnailKind::Video)
        );
    }

    #[test]
    fn cache_schedules_request_once() {
        let mut cache = ImageThumbnailCacheInner::new(None);
        let request = request("key", "folder");

        assert!(cache.thumbnail_for_request(request.clone()).0.is_none());
        assert_eq!(cache.pending.len(), 1);
        assert!(cache.thumbnail_for_request(request).0.is_none());
        assert_eq!(cache.pending.len(), 1);
    }

    #[test]
    fn image_thumbnail_timing_batch_omits_empty_batches() {
        let batch = ImageThumbnailTimingBatch::enabled_for_test();

        assert!(batch.format_lines(Duration::from_millis(1)).is_empty());
    }

    #[test]
    fn image_thumbnail_timing_batch_formats_stage_totals_fastest_and_slowest() {
        let mut batch = ImageThumbnailTimingBatch::enabled_for_test();
        batch.requests = 2;
        batch.queue_wait.record(Duration::from_millis(2));
        batch.queue_wait.record(Duration::from_micros(500));

        let lines = batch.format_lines(Duration::from_millis(3));
        let queue_wait = lines
            .iter()
            .find(|line| line.starts_with("image_thumbnails queue_wait "))
            .expect("queue_wait timing line");

        assert!(queue_wait.contains("count=2"));
        assert!(queue_wait.contains("total=2.500ms"));
        assert!(queue_wait.contains("fastest=0.500ms"));
        assert!(queue_wait.contains("slowest=2.000ms"));
    }

    #[test]
    fn image_thumbnail_timing_batch_formats_outcome_counters() {
        let mut batch = ImageThumbnailTimingBatch::enabled_for_test();
        batch.requests = 4;
        batch.cache_hits = 1;
        batch.cache_misses = 3;
        batch.generated = 2;
        batch.failed = 1;
        batch.cancelled = 1;
        batch.discarded = 1;
        batch.cache_writes_scheduled = 2;

        let lines = batch.format_lines(Duration::from_millis(15));
        let summary = lines.first().expect("summary line");

        assert!(summary.starts_with("image_thumbnails total=15.000ms"));
        assert!(summary.contains("requests=4"));
        assert!(summary.contains("cache_hits=1"));
        assert!(summary.contains("cache_misses=3"));
        assert!(summary.contains("generated=2"));
        assert!(summary.contains("failed=1"));
        assert!(summary.contains("cancelled=1"));
        assert!(summary.contains("discarded=1"));
        assert!(summary.contains("cache_writes_scheduled=2"));
    }

    #[test]
    fn image_thumbnail_timing_batch_formats_extraction_stage_lines() {
        let mut batch = ImageThumbnailTimingBatch::enabled_for_test();
        batch.requests = 1;
        let result = ImageThumbnailLoadResult::generated(
            vec![1],
            None,
            Some(Duration::from_millis(10)),
            None,
            ImageThumbnailExtractionTimings {
                embedded_thumbnail_scan: Some(Duration::from_millis(11)),
                embedded_thumbnail_decode: Some(Duration::from_millis(12)),
                source_read: Some(Duration::from_millis(1)),
                format_detect: Some(Duration::from_millis(2)),
                raster_decode: Some(Duration::from_millis(3)),
                rgba_convert: Some(Duration::from_millis(4)),
                tiff_ifd_scan: Some(Duration::from_millis(13)),
                tiff_raw_sample: Some(Duration::from_millis(14)),
                tiff_chunk_decode: Some(Duration::from_millis(15)),
                tiff_chunk_sample: Some(Duration::from_millis(16)),
                svg_parse: Some(Duration::from_millis(5)),
                svg_render: Some(Duration::from_millis(6)),
                svg_unpremultiply: Some(Duration::from_millis(7)),
                resize_canvas: Some(Duration::from_millis(8)),
                png_encode: Some(Duration::from_millis(9)),
            },
        );

        batch.record_load_result(&result);
        let lines = batch.format_lines(Duration::from_millis(20));

        for stage in [
            "extract",
            "embedded_thumbnail_scan",
            "embedded_thumbnail_decode",
            "source_read",
            "format_detect",
            "raster_decode",
            "rgba_convert",
            "tiff_ifd_scan",
            "tiff_raw_sample",
            "tiff_chunk_decode",
            "tiff_chunk_sample",
            "svg_parse",
            "svg_render",
            "svg_unpremultiply",
            "resize_canvas",
            "png_encode",
        ] {
            assert!(
                lines
                    .iter()
                    .any(|line| line.starts_with(&format!("image_thumbnails {stage} "))),
                "missing timing line for {stage}"
            );
        }
    }

    #[test]
    fn thumbnail_load_result_variants_account_for_outcomes_and_cache_writes() {
        let cache_dir = PathBuf::from("cache");
        let job = ImageThumbnailLoadJob {
            request: request("generated", "folder"),
            generation: 7,
            cache_dir: Some(cache_dir.clone()),
            cancel: Arc::new(AtomicBool::new(false)),
            queued_at: Instant::now(),
        };
        let generated = ImageThumbnailLoadResult::generated(
            vec![1, 2, 3],
            Some(Duration::from_millis(1)),
            Some(Duration::from_millis(2)),
            Some(Duration::from_millis(3)),
            ImageThumbnailExtractionTimings::default(),
        );

        let write = generated.cache_write_job(&job).expect("cache write job");
        assert_eq!(write.cache_dir, cache_dir);
        assert_eq!(write.key, "generated");
        assert_eq!(write.bytes, vec![1, 2, 3]);

        let uncached_job = ImageThumbnailLoadJob {
            request: job.request.clone(),
            generation: job.generation,
            cache_dir: None,
            cancel: job.cancel.clone(),
            queued_at: job.queued_at,
        };
        assert!(generated.cache_write_job(&uncached_job).is_none());

        let failed = ImageThumbnailLoadResult::failed(
            Some(Duration::from_millis(4)),
            Some(Duration::from_millis(5)),
            ImageThumbnailExtractionTimings::default(),
        );
        let cancelled = ImageThumbnailLoadResult::cancelled();
        let cancelled_after_cache = ImageThumbnailLoadResult::cancelled_after_cache_read(
            true,
            Some(Duration::from_millis(6)),
        );
        let cancelled_after_extract = ImageThumbnailLoadResult::cancelled_after_extract(
            Some(Duration::from_millis(7)),
            Some(Duration::from_millis(8)),
            ImageThumbnailExtractionTimings::default(),
        );

        for result in [
            &failed,
            &cancelled,
            &cancelled_after_cache,
            &cancelled_after_extract,
        ] {
            assert!(result.cache_write_job(&job).is_none());
        }

        let mut batch = ImageThumbnailTimingBatch::enabled_for_test();
        batch.record_load_result(&generated);
        batch.record_load_result(&failed);
        batch.record_load_result(&cancelled);
        batch.record_load_result(&cancelled_after_cache);
        batch.record_load_result(&cancelled_after_extract);

        assert_eq!(batch.generated, 1);
        assert_eq!(batch.failed, 1);
        assert_eq!(batch.cancelled, 3);
        assert_eq!(batch.cache_hits, 1);
        assert_eq!(batch.cache_misses, 3);
        assert_eq!(batch.cache_read.count, 4);
        assert_eq!(batch.extract.count, 3);
        assert_eq!(batch.cache_write.count, 1);
    }

    #[test]
    fn thumbnail_timing_batch_records_optional_events_when_enabled() {
        let disabled = ImageThumbnailTimingBatch::start();
        assert_eq!(disabled.now().is_some(), disabled.enabled());

        let mut batch = ImageThumbnailTimingBatch::enabled_for_test();
        assert!(batch.enabled());
        let started = batch.now();

        batch.record_request();
        batch.record_queue_wait(Duration::from_millis(2));
        batch.record_commit(started);
        batch.record_discarded();
        batch.record_cache_write_scheduled();
        batch.record_request_total(batch.now());

        assert_eq!(batch.requests, 1);
        assert_eq!(batch.queue_wait.count, 1);
        assert_eq!(batch.discarded, 1);
        assert_eq!(batch.cache_writes_scheduled, 1);
        assert!(batch.request_total.count <= 1);

        batch.finish();
    }

    #[test]
    fn thumbnail_errors_are_truncated_and_cancel_flag_is_reported() {
        let long = "x".repeat(350);
        let label = command_error_output_label(long.as_bytes());
        assert_eq!(label.chars().count(), 303);
        assert!(label.ends_with("..."));

        let cancel = AtomicBool::new(false);
        assert!(check_thumbnail_cancelled(&cancel).is_ok());
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            check_thumbnail_cancelled(&cancel),
            Err("Thumbnail loading was cancelled.".to_owned())
        );
    }

    #[test]
    fn cached_thumbnail_round_trips_from_disk() {
        let temp = TempDir::new();
        let source = temp.path().join("image.png");
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(4, 2));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        fs::write(&source, bytes).unwrap();
        let request = ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            key: "0123456789abcdef".to_owned(),
            path: source,
            directory: temp.path().to_path_buf(),
        };
        let cancel = AtomicBool::new(false);

        let generated = load_or_create_thumbnail_png(&request, Some(temp.path()), &cancel).unwrap();
        assert!(write_cached_thumbnail(
            Some(temp.path()),
            &request.key,
            &generated
        ));
        let cached = load_or_create_thumbnail_png(&request, Some(temp.path()), &cancel).unwrap();

        assert_eq!(generated, cached);
        assert!(
            thumbnail_file_path(Some(temp.path()), &request.key)
                .unwrap()
                .is_file()
        );
    }

    #[test]
    fn thumbnail_file_path_rejects_invalid_keys() {
        let dir = Path::new("cache");

        assert!(thumbnail_file_path(Some(dir), "../escape").is_none());
        assert!(thumbnail_file_path(Some(dir), "ABC").is_none());
        assert!(thumbnail_file_path(Some(dir), "").is_none());
        assert!(thumbnail_file_path(Some(dir), "0123456789abcdef").is_some());
    }

    #[test]
    fn cache_can_start_multiple_loading_jobs_for_generation() {
        let mut cache = ImageThumbnailCacheInner::new(None);
        let first = request("first", "folder");
        let second = request("second", "folder");
        push_pending(&mut cache, first.clone());
        push_pending(&mut cache, second.clone());
        let generation = cache.start_loader().unwrap();

        let first_job = cache.next_load_job(generation).unwrap();
        let second_job = cache.next_load_job(generation).unwrap();

        assert_eq!(first_job.request, first);
        assert_eq!(second_job.request, second);
        assert!(matches!(
            cache.states.get(&first_job.request.key),
            Some(ImageThumbnailState::Loading { .. })
        ));
        assert!(matches!(
            cache.states.get(&second_job.request.key),
            Some(ImageThumbnailState::Loading { .. })
        ));
    }

    #[test]
    fn cancel_directory_removes_pending_requests_and_preserves_completed_states() {
        let mut cache = ImageThumbnailCacheInner::new(None);
        let old_one = request("old-one", "old");
        let old_two = request("old-two", "old");
        let current = request("current", "current");
        push_pending(&mut cache, old_one.clone());
        push_pending(&mut cache, current.clone());
        push_pending(&mut cache, old_two.clone());
        cache.states.insert(
            "ready".to_owned(),
            ImageThumbnailState::Ready(image_from_png_bytes(one_pixel_png_bytes())),
        );

        let generation = cache.cancel_directory(Path::new("old"));

        assert!(generation.is_some());
        assert_eq!(cache.pending.iter().collect::<Vec<_>>(), vec![&current.key]);
        assert!(!cache.states.contains_key(&old_one.key));
        assert!(!cache.states.contains_key(&old_two.key));
        assert!(matches!(
            cache.states.get(&current.key),
            Some(ImageThumbnailState::Pending { .. })
        ));
        assert!(matches!(
            cache.states.get("ready"),
            Some(ImageThumbnailState::Ready(_))
        ));
    }

    #[test]
    fn cancel_directory_signals_loading_request_and_starts_next_generation() {
        let mut cache = ImageThumbnailCacheInner::new(None);
        let old = request("old", "old");
        let current = request("current", "current");
        push_pending(&mut cache, old.clone());
        push_pending(&mut cache, current.clone());
        let generation = cache.start_loader().unwrap();
        let job = cache.next_load_job(generation).unwrap();

        let next_generation = cache.cancel_directory(Path::new("old")).unwrap();

        assert!(job.cancel.load(Ordering::Relaxed));
        assert_ne!(next_generation, generation);
        assert!(!cache.states.contains_key(&old.key));
        let next_job = cache.next_load_job(next_generation).unwrap();
        assert_eq!(next_job.request, current);
    }

    #[test]
    fn cancel_directory_signals_multiple_loading_requests() {
        let mut cache = ImageThumbnailCacheInner::new(None);
        let old_one = request("old-one", "old");
        let old_two = request("old-two", "old");
        let current = request("current", "current");
        push_pending(&mut cache, old_one.clone());
        push_pending(&mut cache, old_two.clone());
        push_pending(&mut cache, current.clone());
        let generation = cache.start_loader().unwrap();
        let first_job = cache.next_load_job(generation).unwrap();
        let second_job = cache.next_load_job(generation).unwrap();

        let next_generation = cache.cancel_directory(Path::new("old")).unwrap();

        assert!(first_job.cancel.load(Ordering::Relaxed));
        assert!(second_job.cancel.load(Ordering::Relaxed));
        assert_ne!(next_generation, generation);
        assert!(!cache.states.contains_key(&old_one.key));
        assert!(!cache.states.contains_key(&old_two.key));
        let next_job = cache.next_load_job(next_generation).unwrap();
        assert_eq!(next_job.request, current);
    }

    #[test]
    fn stale_completion_after_cancellation_does_not_overwrite_new_request() {
        let mut cache = ImageThumbnailCacheInner::new(None);
        let old = request("shared", "old");
        let current = request("shared", "current");
        push_pending(&mut cache, old.clone());
        let old_generation = cache.start_loader().unwrap();
        let _old_job = cache.next_load_job(old_generation).unwrap();

        assert!(cache.cancel_directory(Path::new("old")).is_none());
        let (_, new_generation) = cache.thumbnail_for_request(current.clone());
        let new_generation = new_generation.unwrap();

        assert!(!cache.finish_request(old, old_generation, Some(one_pixel_png_bytes())));
        assert!(matches!(
            cache.states.get(&current.key),
            Some(ImageThumbnailState::Pending { request, .. }) if request == &current
        ));

        let new_job = cache.next_load_job(new_generation).unwrap();
        assert_eq!(new_job.request, current.clone());
        assert!(cache.finish_request(current.clone(), new_generation, Some(one_pixel_png_bytes())));
        assert!(matches!(
            cache.states.get(&current.key),
            Some(ImageThumbnailState::Ready(_))
        ));
    }

    #[test]
    fn fast_video_thumbnail_args_use_input_seek_and_keyframes() {
        let args = ffmpeg_video_thumbnail_args(Path::new("clip.mp4"), 5.0, 128, true);
        let args = args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        let ss = arg_index(&args, "-ss");
        let input = arg_index(&args, "-i");
        assert!(ss < input);
        assert_eq!(args[ss + 1], "5.000");
        assert!(args.iter().any(|arg| arg == "-noaccurate_seek"));
        assert_eq!(args[arg_index(&args, "-skip_frame") + 1], "nokey");
        assert_eq!(
            args[arg_index(&args, "-vf") + 1],
            "scale=128:128:force_original_aspect_ratio=decrease:flags=fast_bilinear"
        );
    }

    #[test]
    fn fallback_video_thumbnail_args_keep_fast_seek_without_keyframe_filter() {
        let args = ffmpeg_video_thumbnail_args(Path::new("clip.mp4"), 1.0, 128, false);
        let args = args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert!(arg_index(&args, "-ss") < arg_index(&args, "-i"));
        assert!(args.iter().any(|arg| arg == "-noaccurate_seek"));
        assert!(!args.iter().any(|arg| arg == "-skip_frame"));
    }

    fn request(key: &str, directory: &str) -> ImageThumbnailRequest {
        ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            key: key.to_owned(),
            path: PathBuf::from(directory).join(format!("{key}.png")),
            directory: PathBuf::from(directory),
        }
    }

    fn push_pending(cache: &mut ImageThumbnailCacheInner, request: ImageThumbnailRequest) {
        cache.pending.push_back(request.key.clone());
        cache.states.insert(
            request.key.clone(),
            ImageThumbnailState::Pending {
                request,
                queued_at: Instant::now(),
            },
        );
    }

    fn arg_index(args: &[String], expected: &str) -> usize {
        args.iter()
            .position(|arg| arg == expected)
            .unwrap_or_else(|| panic!("missing arg {expected} in {args:?}"))
    }

    fn one_pixel_png_bytes() -> Vec<u8> {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(1, 1));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }
}
