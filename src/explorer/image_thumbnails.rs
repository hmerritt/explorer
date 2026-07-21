use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures::{
    StreamExt,
    channel::mpsc::{self, Sender},
    stream::FuturesUnordered,
};
use gpui::{App, BackgroundExecutor, Context, Global, RenderImage};
use serde::{Deserialize, Serialize};

use crate::{
    explorer::{
        entry::FileEntry,
        filesystem::path_is_remote_drive,
        image_preview::{
            AnimatedImageSource, ImageThumbnailExtractionTimings, ThumbnailSpec, ThumbnailStage,
            animated_gif_source_for_path, load_thumbnail_rgba_with_cancel_timed,
            path_may_have_image_preview,
        },
        image_resize::dimensions_for_longest_side,
        video::path_may_have_video_metadata,
        video_thumbnails::load_video_thumbnail_rgba,
        view::ExplorerView,
    },
    settings::{ConfigPlatform, config_dir_for},
};

#[cfg(test)]
use crate::explorer::image_preview::{
    hover_image_preview_dimensions, load_image_thumbnail_png_with_cancel_timed,
};

const IMAGE_THUMBNAIL_CACHE_VERSION: &str = "image-thumbnails-v2";
const LEGACY_IMAGE_THUMBNAIL_CACHE_VERSIONS: &[&str] = &["image-thumbnails-v1"];
const DISK_MANIFEST_FILE_NAME: &str = "manifest.json";
const IMAGE_THUMBNAIL_SIZE: u32 = 128;
const HOVER_IMAGE_PREVIEW_SIZE: u32 = 400;
const IMAGE_THUMBNAIL_CACHE_WRITER_CAPACITY: usize = 64;
const IMAGE_THUMBNAIL_CACHE_BATCH_DELAY: Duration = Duration::from_millis(8);
const QOI_SIGNATURE: &[u8] = b"qoif";

pub(super) struct ImageThumbnailCache {
    inner: RefCell<ImageThumbnailCacheInner>,
}

impl Global for ImageThumbnailCache {}

impl ImageThumbnailCache {
    fn new(
        cache_dir: Option<PathBuf>,
        cache_writer: Option<Sender<ImageThumbnailCacheWriteJob>>,
    ) -> Self {
        Self {
            inner: RefCell::new(ImageThumbnailCacheInner::with_writer(
                cache_dir,
                cache_writer,
            )),
        }
    }
}

pub(crate) fn initialize(cx: &mut App) {
    let cache_dir = image_thumbnail_cache_dir();
    let cache_writer = start_image_thumbnail_cache_writer(cache_dir.clone(), cx);
    cx.set_global(ImageThumbnailCache::new(cache_dir, cache_writer));
}

#[cfg(test)]
pub(super) fn initialize_for_test(cx: &mut App) {
    cx.set_global(ImageThumbnailCache {
        inner: RefCell::new(ImageThumbnailCacheInner::new(None)),
    });
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ImageThumbnailRequest {
    kind: ImageThumbnailKind,
    usage: ImageThumbnailUsage,
    source_policy: ThumbnailSourcePolicy,
    key: String,
    path: PathBuf,
    directory: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ThumbnailSourcePolicy {
    ReadSource,
    CacheOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ImageThumbnailKind {
    Image,
    Video,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ImageThumbnailUsage {
    Standard,
    HoverPreview,
}

impl ImageThumbnailUsage {
    fn cache_namespace(self, kind: ImageThumbnailKind) -> &'static str {
        match (self, kind) {
            (Self::Standard, ImageThumbnailKind::Image) => "image",
            (Self::Standard, ImageThumbnailKind::Video) => "video",
            (Self::HoverPreview, ImageThumbnailKind::Image) => "image-hover-preview-v1",
            (Self::HoverPreview, ImageThumbnailKind::Video) => "video-hover-preview",
        }
    }

    fn size(self) -> u32 {
        match self {
            Self::Standard => IMAGE_THUMBNAIL_SIZE,
            Self::HoverPreview => HOVER_IMAGE_PREVIEW_SIZE,
        }
    }
}

struct ImageThumbnailCacheInner {
    cache_dir: Option<PathBuf>,
    cache_writer: Option<Sender<ImageThumbnailCacheWriteJob>>,
    states: HashMap<String, ImageThumbnailState>,
    pending: VecDeque<String>,
    loader_running: bool,
    loader_generation: u64,
}

enum ImageThumbnailState {
    Pending {
        request: ImageThumbnailRequest,
        queued_at: Instant,
        preview_dimensions: Option<(u32, u32)>,
        loading_thumbnail: Option<CachedThumbnailImage>,
    },
    Loading {
        request: ImageThumbnailRequest,
        generation: u64,
        cancel: Arc<AtomicBool>,
        preview_dimensions: Option<(u32, u32)>,
        loading_thumbnail: Option<CachedThumbnailImage>,
    },
    Ready(CachedThumbnailImage),
    Failed {
        request: ImageThumbnailRequest,
    },
}

#[derive(Clone, Debug)]
pub(super) struct CachedThumbnailImage {
    pub(super) image: Arc<RenderImage>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) animated_source: Option<AnimatedImageSource>,
}

#[derive(Clone, Debug)]
pub(super) enum HoverImagePreviewLookup {
    Loading {
        width: u32,
        height: u32,
        thumbnail: Option<CachedThumbnailImage>,
    },
    Ready(CachedThumbnailImage),
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
    source_path: PathBuf,
    image: image::RgbaImage,
    queued_at: Instant,
}

impl ImageThumbnailCacheInner {
    #[cfg(test)]
    fn new(cache_dir: Option<PathBuf>) -> Self {
        Self::with_writer(cache_dir, None)
    }

    fn with_writer(
        cache_dir: Option<PathBuf>,
        cache_writer: Option<Sender<ImageThumbnailCacheWriteJob>>,
    ) -> Self {
        Self {
            cache_dir,
            cache_writer,
            states: HashMap::new(),
            pending: VecDeque::new(),
            loader_running: false,
            loader_generation: 0,
        }
    }

    fn queue_cache_write(&mut self, job: ImageThumbnailCacheWriteJob) -> bool {
        self.cache_writer
            .as_mut()
            .is_some_and(|writer| writer.try_send(job).is_ok())
    }

    fn thumbnail_for_request(
        &mut self,
        request: ImageThumbnailRequest,
    ) -> (Option<CachedThumbnailImage>, Option<u64>) {
        if let Some(state) = self.states.get(&request.key) {
            if state.should_retry_for_request(&request) {
                self.states.remove(&request.key);
            } else {
                return (state.thumbnail(), None);
            }
        }

        if let Some(state) = self.states.get(&request.key) {
            return (state.thumbnail(), None);
        }

        self.pending.push_back(request.key.clone());
        self.states.insert(
            request.key.clone(),
            ImageThumbnailState::Pending {
                request,
                queued_at: Instant::now(),
                preview_dimensions: None,
                loading_thumbnail: None,
            },
        );

        (None, self.start_loader())
    }

    fn hover_preview_for_request(
        &mut self,
        request: ImageThumbnailRequest,
        standard_request: ImageThumbnailRequest,
    ) -> (HoverImagePreviewLookup, Option<u64>) {
        if let Some(state) = self.states.get(&request.key) {
            if state.should_retry_for_request(&request) {
                self.states.remove(&request.key);
            } else {
                return (state.hover_preview(), None);
            }
        }

        let loading_thumbnail = self
            .states
            .get(&standard_request.key)
            .and_then(ImageThumbnailState::thumbnail);
        let (width, height) = loading_thumbnail
            .as_ref()
            .and_then(|thumbnail| {
                dimensions_for_preview(thumbnail.width, thumbnail.height, HOVER_IMAGE_PREVIEW_SIZE)
            })
            .unwrap_or((HOVER_IMAGE_PREVIEW_SIZE, HOVER_IMAGE_PREVIEW_SIZE));
        self.pending.push_front(request.key.clone());
        self.states.insert(
            request.key.clone(),
            ImageThumbnailState::Pending {
                request,
                queued_at: Instant::now(),
                preview_dimensions: Some((width, height)),
                loading_thumbnail: loading_thumbnail.clone(),
            },
        );

        (
            HoverImagePreviewLookup::Loading {
                width,
                height,
                thumbnail: loading_thumbnail,
            },
            self.start_loader(),
        )
    }

    fn start_loader(&mut self) -> Option<u64> {
        if self.loader_running || self.pending.is_empty() {
            return None;
        }

        self.loader_generation = self.loader_generation.wrapping_add(1);
        self.loader_running = true;
        Some(self.loader_generation)
    }

    #[cfg(test)]
    fn next_load_job(&mut self, generation: u64) -> Option<ImageThumbnailLoadJob> {
        self.next_load_job_matching(generation, |_| true)
    }

    fn next_load_job_matching(
        &mut self,
        generation: u64,
        mut should_start: impl FnMut(&ImageThumbnailRequest) -> bool,
    ) -> Option<ImageThumbnailLoadJob> {
        if !self.loader_running || self.loader_generation != generation {
            return None;
        }

        let pending_count = self.pending.len();
        if pending_count == 0 {
            self.loader_running = false;
            return None;
        }

        for _ in 0..pending_count {
            let Some(key) = self.pending.pop_front() else {
                break;
            };
            let Some(ImageThumbnailState::Pending {
                request,
                queued_at,
                preview_dimensions,
                loading_thumbnail,
            }) = self.states.remove(&key)
            else {
                continue;
            };

            if !should_start(&request) {
                self.states.insert(
                    key.clone(),
                    ImageThumbnailState::Pending {
                        request,
                        queued_at,
                        preview_dimensions,
                        loading_thumbnail,
                    },
                );
                self.pending.push_back(key);
                continue;
            }

            let cancel = Arc::new(AtomicBool::new(false));
            self.states.insert(
                key,
                ImageThumbnailState::Loading {
                    request: request.clone(),
                    generation,
                    cancel: cancel.clone(),
                    preview_dimensions,
                    loading_thumbnail,
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

        if self.pending.is_empty() {
            self.loader_running = false;
        }
        None
    }

    fn finish_prepared_request(
        &mut self,
        request: ImageThumbnailRequest,
        generation: u64,
        image: Option<CachedThumbnailImage>,
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

        let state = match image {
            Some(image) => ImageThumbnailState::Ready(image),
            None => ImageThumbnailState::Failed {
                request: request.clone(),
            },
        };

        self.states.insert(request.key, state);
        true
    }

    #[cfg(test)]
    fn finish_request(
        &mut self,
        request: ImageThumbnailRequest,
        generation: u64,
        bytes: Option<Vec<u8>>,
    ) -> bool {
        self.finish_prepared_request(
            request,
            generation,
            bytes.and_then(cached_thumbnail_image_from_png_bytes),
        )
    }

    fn cancel_directory(&mut self, directory: &Path) -> Option<u64> {
        self.cancel_directory_matching(directory, |_| true)
    }

    fn cancel_standard_thumbnail_requests(&mut self, directory: &Path) -> Option<u64> {
        self.cancel_directory_matching(directory, |request| {
            request.usage == ImageThumbnailUsage::Standard
        })
    }

    fn cancel_directory_matching(
        &mut self,
        directory: &Path,
        mut should_cancel: impl FnMut(&ImageThumbnailRequest) -> bool,
    ) -> Option<u64> {
        self.pending.retain(|key| {
            !matches!(
                self.states.get(key),
                Some(ImageThumbnailState::Pending { request, .. })
                    if request.directory == directory && should_cancel(request)
            )
        });

        let mut cancelled_loading = false;
        self.states.retain(|_, state| match state {
            ImageThumbnailState::Pending { request, .. }
                if request.directory == directory && should_cancel(request) =>
            {
                false
            }
            ImageThumbnailState::Loading {
                request, cancel, ..
            } if request.directory == directory && should_cancel(request) => {
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
    fn should_retry_for_request(&self, request: &ImageThumbnailRequest) -> bool {
        matches!(
            self,
            Self::Failed {
                request: failed_request
            } if failed_request.source_policy == ThumbnailSourcePolicy::CacheOnly
                && request.source_policy == ThumbnailSourcePolicy::ReadSource
        )
    }

    fn thumbnail(&self) -> Option<CachedThumbnailImage> {
        match self {
            Self::Ready(image) => Some(image.clone()),
            Self::Pending { .. } | Self::Loading { .. } | Self::Failed { .. } => None,
        }
    }

    fn hover_preview(&self) -> HoverImagePreviewLookup {
        match self {
            Self::Pending {
                preview_dimensions: Some((width, height)),
                loading_thumbnail,
                ..
            }
            | Self::Loading {
                preview_dimensions: Some((width, height)),
                loading_thumbnail,
                ..
            } => HoverImagePreviewLookup::Loading {
                width: *width,
                height: *height,
                thumbnail: loading_thumbnail.clone(),
            },
            Self::Ready(image) => HoverImagePreviewLookup::Ready(image.clone()),
            Self::Pending { .. } | Self::Loading { .. } | Self::Failed { .. } => {
                HoverImagePreviewLookup::Failed
            }
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
    ) -> Option<Arc<RenderImage>> {
        let request =
            image_thumbnail_request_for_entry(entry, &self.path, self.thumbnail_source_policy)?;
        let (thumbnail, loader_generation) = cx
            .try_global::<ImageThumbnailCache>()
            .map(|cache| cache.inner.borrow_mut().thumbnail_for_request(request))
            .unwrap_or((None, None));

        if let Some(generation) = loader_generation {
            start_image_thumbnail_loader(cx, generation);
        }

        thumbnail.map(|thumbnail| thumbnail.image)
    }

    pub(super) fn hover_image_preview_for_entry(
        &mut self,
        entry: &FileEntry,
        cx: &mut Context<Self>,
    ) -> Option<HoverImagePreviewLookup> {
        let request =
            hover_image_preview_request_for_entry(entry, &self.path, self.thumbnail_source_policy)?;
        let standard_request =
            image_thumbnail_request_for_entry(entry, &self.path, self.thumbnail_source_policy)?;
        let (preview, loader_generation) = cx
            .try_global::<ImageThumbnailCache>()
            .map(|cache| {
                cache
                    .inner
                    .borrow_mut()
                    .hover_preview_for_request(request, standard_request)
            })
            .unwrap_or((HoverImagePreviewLookup::Failed, None));

        if let Some(generation) = loader_generation {
            start_image_thumbnail_loader(cx, generation);
        }

        Some(preview)
    }

    pub(super) fn ready_standard_video_thumbnail_for_entry(
        &self,
        entry: &FileEntry,
        cx: &mut Context<Self>,
    ) -> Option<CachedThumbnailImage> {
        let request =
            image_thumbnail_request_for_entry(entry, &self.path, self.thumbnail_source_policy)?;
        if request.kind != ImageThumbnailKind::Video
            || request.usage != ImageThumbnailUsage::Standard
        {
            return None;
        }

        cx.try_global::<ImageThumbnailCache>().and_then(|cache| {
            cache
                .inner
                .borrow()
                .states
                .get(&request.key)
                .and_then(ImageThumbnailState::thumbnail)
        })
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

    pub(super) fn cancel_standard_image_thumbnail_extraction(&mut self, cx: &mut Context<Self>) {
        let directory = self.path.clone();
        let loader_generation = cx.try_global::<ImageThumbnailCache>().and_then(|cache| {
            cache
                .inner
                .borrow_mut()
                .cancel_standard_thumbnail_requests(&directory)
        });

        if let Some(generation) = loader_generation {
            start_image_thumbnail_loader(cx, generation);
        }
    }

    #[cfg(test)]
    pub(super) fn hold_hover_image_preview_loading_for_test(
        &mut self,
        entry: &FileEntry,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = hover_image_preview_request_for_entry(
            entry,
            &self.path,
            ThumbnailSourcePolicy::ReadSource,
        ) else {
            return;
        };
        let Ok((width, height)) =
            hover_image_preview_dimensions(&request.path, HOVER_IMAGE_PREVIEW_SIZE)
        else {
            return;
        };
        if let Some(cache) = cx.try_global::<ImageThumbnailCache>() {
            cache.inner.borrow_mut().states.insert(
                request.key.clone(),
                ImageThumbnailState::Pending {
                    request,
                    queued_at: Instant::now(),
                    preview_dimensions: Some((width, height)),
                    loading_thumbnail: None,
                },
            );
        }
    }

    #[cfg(test)]
    pub(super) fn hold_hover_image_preview_loading_with_thumbnail_for_test(
        &mut self,
        entry: &FileEntry,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = hover_image_preview_request_for_entry(
            entry,
            &self.path,
            ThumbnailSourcePolicy::ReadSource,
        ) else {
            return;
        };
        let Some(standard_request) =
            image_thumbnail_request_for_entry(entry, &self.path, self.thumbnail_source_policy)
        else {
            return;
        };
        let Ok((width, height)) =
            hover_image_preview_dimensions(&request.path, HOVER_IMAGE_PREVIEW_SIZE)
        else {
            return;
        };
        let thumbnail = load_image_thumbnail_png_with_cancel_timed(
            &standard_request.path,
            IMAGE_THUMBNAIL_SIZE,
            &AtomicBool::new(false),
            false,
        )
        .result
        .ok()
        .and_then(cached_thumbnail_image_from_png_bytes);
        if let (Some(cache), Some(thumbnail)) = (cx.try_global::<ImageThumbnailCache>(), thumbnail)
        {
            let mut cache = cache.inner.borrow_mut();
            cache.states.insert(
                standard_request.key,
                ImageThumbnailState::Ready(thumbnail.clone()),
            );
            cache.states.insert(
                request.key.clone(),
                ImageThumbnailState::Pending {
                    request,
                    queued_at: Instant::now(),
                    preview_dimensions: Some((width, height)),
                    loading_thumbnail: Some(thumbnail),
                },
            );
        }
    }
}

fn start_image_thumbnail_loader(cx: &mut Context<ExplorerView>, generation: u64) {
    cx.spawn(async move |_, cx| {
        let mut timings = ImageThumbnailTimingBatch::start();
        let concurrency = image_thumbnail_loader_concurrency();
        let mut remote_directories_in_flight = HashSet::new();
        let mut in_flight = FuturesUnordered::new();

        loop {
            while in_flight.len() < concurrency {
                let job = cx
                    .update(|cx| {
                        cx.try_global::<ImageThumbnailCache>().and_then(|cache| {
                            cache
                                .inner
                                .borrow_mut()
                                .next_load_job_matching(generation, |request| {
                                    !thumbnail_request_reads_remote_source(request)
                                        || !remote_directories_in_flight
                                            .contains(&request.directory)
                                })
                        })
                    })
                    .ok()
                    .flatten();
                let Some(job) = job else {
                    break;
                };
                if thumbnail_request_reads_remote_source(&job.request) {
                    remote_directories_in_flight.insert(job.request.directory.clone());
                }

                let request_started = timings.now();
                timings.record_request();
                timings.record_queue_wait(job.queued_at.elapsed());

                let timings_enabled = timings.enabled();
                let load_task = cx.background_executor().spawn(async move {
                    let thumbnail = load_or_create_thumbnail_with_timings(
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
            if thumbnail_request_reads_remote_source(&job.request) {
                remote_directories_in_flight.remove(&job.request.directory);
            }
            timings.record_load_result(&thumbnail);
            let cache_write = thumbnail.cache_write_job(&job);

            let commit_started = timings.now();
            let finished = cx
                .update_global::<ImageThumbnailCache, _>(|cache, _| {
                    cache.inner.borrow_mut().finish_prepared_request(
                        job.request,
                        job.generation,
                        thumbnail.image,
                    )
                })
                .unwrap_or(false);
            timings.record_commit(commit_started);
            if !finished {
                timings.record_discarded();
            }
            timings.record_request_total(request_started);

            if finished && let Some(cache_write) = cache_write {
                let queued = cx
                    .update_global::<ImageThumbnailCache, _>(|cache, _| {
                        cache.inner.borrow_mut().queue_cache_write(cache_write)
                    })
                    .unwrap_or(false);
                if queued {
                    timings.record_cache_write_scheduled();
                }
            }
        }

        timings.finish();
    })
    .detach();
}

fn start_image_thumbnail_cache_writer(
    cache_dir: Option<PathBuf>,
    cx: &App,
) -> Option<Sender<ImageThumbnailCacheWriteJob>> {
    let cache_dir = cache_dir?;
    let (sender, receiver) = mpsc::channel(IMAGE_THUMBNAIL_CACHE_WRITER_CAPACITY);
    let executor = cx.background_executor().clone();
    let writer_executor = executor.clone();
    executor
        .spawn(async move {
            run_image_thumbnail_cache_writer(cache_dir, receiver, writer_executor).await;
        })
        .detach();
    Some(sender)
}

async fn run_image_thumbnail_cache_writer(
    cache_dir: PathBuf,
    mut receiver: mpsc::Receiver<ImageThumbnailCacheWriteJob>,
    executor: BackgroundExecutor,
) {
    let mut manifest = load_disk_manifest(Some(&cache_dir)).unwrap_or_default();
    while let Some(first) = receiver.next().await {
        executor.timer(IMAGE_THUMBNAIL_CACHE_BATCH_DELAY).await;
        let mut batch = Vec::with_capacity(IMAGE_THUMBNAIL_CACHE_WRITER_CAPACITY);
        batch.push(first);
        while batch.len() < IMAGE_THUMBNAIL_CACHE_WRITER_CAPACITY {
            match receiver.try_recv() {
                Ok(job) => batch.push(job),
                Err(_) => break,
            }
        }
        let metrics = write_image_thumbnail_cache_batch(&cache_dir, &mut manifest, batch);
        if crate::debug_options::icon_timings_enabled() {
            crate::debug_options::log_icon_timing(format_args!(
                "image_thumbnails cache_writer count={} writer_queue={} cache_encode={} cache_write={} manifest_flush={}",
                metrics.written,
                format_image_thumbnail_timing_duration(metrics.queue_wait),
                format_image_thumbnail_timing_duration(metrics.encode),
                format_image_thumbnail_timing_duration(metrics.write),
                format_image_thumbnail_timing_duration(metrics.manifest_flush),
            ));
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ImageThumbnailCacheWriteMetrics {
    written: usize,
    queue_wait: Duration,
    encode: Duration,
    write: Duration,
    manifest_flush: Duration,
}

fn write_image_thumbnail_cache_batch(
    cache_dir: &Path,
    manifest: &mut DiskThumbnailManifest,
    batch: Vec<ImageThumbnailCacheWriteJob>,
) -> ImageThumbnailCacheWriteMetrics {
    let mut metrics = ImageThumbnailCacheWriteMetrics::default();
    for job in batch {
        metrics.queue_wait += job.queued_at.elapsed();
        let encode_started = Instant::now();
        let encoded =
            encode_rgba_qoi_bytes(job.image.as_raw(), job.image.width(), job.image.height());
        metrics.encode += encode_started.elapsed();
        let Some(bytes) = encoded else {
            continue;
        };

        let write_started = Instant::now();
        let did_write = write_cached_thumbnail(Some(&job.cache_dir), &job.key, &bytes);
        metrics.write += write_started.elapsed();
        if did_write {
            manifest.mappings.insert(job.key, job.source_path);
            metrics.written += 1;
        }
    }

    let manifest_started = Instant::now();
    if metrics.written > 0 {
        let _ = save_disk_manifest(cache_dir, manifest);
    }
    metrics.manifest_flush = manifest_started.elapsed();
    metrics
}

pub(super) fn image_thumbnail_loader_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4)
        .clamp(2, 4)
}

fn thumbnail_request_reads_remote_source(request: &ImageThumbnailRequest) -> bool {
    request.source_policy == ThumbnailSourcePolicy::ReadSource
        && path_is_remote_drive(&request.directory)
}

#[cfg(test)]
fn load_or_create_thumbnail_cache_bytes(
    request: &ImageThumbnailRequest,
    cache_dir: Option<&Path>,
    cancel: &AtomicBool,
) -> Option<Vec<u8>> {
    if let Some(bytes) = read_cached_thumbnail(cache_dir, &request.key) {
        return Some(bytes);
    }
    if request.source_policy == ThumbnailSourcePolicy::CacheOnly {
        return None;
    }
    let spec = match request.usage {
        ImageThumbnailUsage::Standard => ThumbnailSpec::standard(request.usage.size()),
        ImageThumbnailUsage::HoverPreview => ThumbnailSpec::hover(request.usage.size()),
    };
    let image = load_thumbnail_rgba_with_cancel_timed(&request.path, spec, cancel, false)
        .result
        .ok()?;
    encode_rgba_qoi_bytes(image.as_raw(), image.width(), image.height())
}

fn load_or_create_thumbnail_with_timings(
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

        let decode_started = timings_enabled.then(Instant::now);
        let image = decode_cached_thumbnail_rgba(&bytes);
        let cache_decode_elapsed = decode_started.map(|started| started.elapsed());
        if let Some(image) = image {
            return ImageThumbnailLoadResult::cache_hit(
                image,
                animated_source_for_request(request),
                cache_read_elapsed,
                cache_decode_elapsed,
            );
        }

        if let Some(path) = thumbnail_file_path(cache_dir, &request.key) {
            let _ = fs::remove_file(path);
        }
    }

    if cancel.load(Ordering::Relaxed) {
        return ImageThumbnailLoadResult::cancelled_after_cache_read(cache_hit, cache_read_elapsed);
    }

    if request.source_policy == ThumbnailSourcePolicy::CacheOnly {
        return ImageThumbnailLoadResult::failed(
            cache_read_elapsed,
            None,
            ImageThumbnailExtractionTimings::default(),
        );
    }

    let extract_started = timings_enabled.then(Instant::now);
    let (result, extraction_timings) = match request.kind {
        ImageThumbnailKind::Image => {
            let spec = match request.usage {
                ImageThumbnailUsage::Standard => ThumbnailSpec::standard(request.usage.size()),
                ImageThumbnailUsage::HoverPreview => ThumbnailSpec::hover(request.usage.size()),
            };
            let extracted =
                load_thumbnail_rgba_with_cancel_timed(&request.path, spec, cancel, timings_enabled);
            (extracted.result, extracted.timings)
        }
        ImageThumbnailKind::Video => {
            let result = load_video_thumbnail_rgba(&request.path, IMAGE_THUMBNAIL_SIZE, cancel)
                .map(|extraction| extraction.value)
                .map_err(|error| error.to_string());
            (result, ImageThumbnailExtractionTimings::default())
        }
    };
    let image = match result {
        Ok(image) => image,
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
        image,
        animated_source_for_request(request),
        cache_read_elapsed,
        extract_elapsed,
        extraction_timings,
    )
}

fn animated_source_for_request(request: &ImageThumbnailRequest) -> Option<AnimatedImageSource> {
    if request.kind != ImageThumbnailKind::Image
        || request.usage != ImageThumbnailUsage::HoverPreview
    {
        return None;
    }

    animated_gif_source_for_path(&request.path, request.key.clone())
}

struct ImageThumbnailLoadResult {
    image: Option<CachedThumbnailImage>,
    cache_image: Option<image::RgbaImage>,
    cache_hit: Option<bool>,
    timings: ImageThumbnailExtractionTimings,
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
    fn empty(outcome: ImageThumbnailLoadOutcome) -> Self {
        Self {
            image: None,
            cache_image: None,
            cache_hit: None,
            timings: ImageThumbnailExtractionTimings::default(),
            outcome,
        }
    }

    fn cache_hit(
        image: image::RgbaImage,
        animated_source: Option<AnimatedImageSource>,
        cache_read_elapsed: Option<Duration>,
        cache_decode_elapsed: Option<Duration>,
    ) -> Self {
        let render_started = Instant::now();
        let image = cached_thumbnail_image_from_rgba_with_animated_source(image, animated_source);
        let mut timings = ImageThumbnailExtractionTimings::default();
        timings.set(ThumbnailStage::CacheRead, cache_read_elapsed);
        timings.set(ThumbnailStage::CacheDecode, cache_decode_elapsed);
        timings.record(ThumbnailStage::RenderPrepare, render_started.elapsed());
        Self {
            image: Some(image),
            cache_hit: Some(true),
            timings,
            ..Self::empty(ImageThumbnailLoadOutcome::CacheHit)
        }
    }

    fn generated(
        image: image::RgbaImage,
        animated_source: Option<AnimatedImageSource>,
        cache_read_elapsed: Option<Duration>,
        extract_elapsed: Option<Duration>,
        mut timings: ImageThumbnailExtractionTimings,
    ) -> Self {
        let cache_image = image.clone();
        let render_started = Instant::now();
        let image = cached_thumbnail_image_from_rgba_with_animated_source(image, animated_source);
        timings.set(ThumbnailStage::CacheRead, cache_read_elapsed);
        timings.set(ThumbnailStage::Extract, extract_elapsed);
        timings.record(ThumbnailStage::RenderPrepare, render_started.elapsed());
        Self {
            image: Some(image),
            cache_image: Some(cache_image),
            cache_hit: Some(false),
            timings,
            ..Self::empty(ImageThumbnailLoadOutcome::Generated)
        }
    }

    fn failed(
        cache_read_elapsed: Option<Duration>,
        extract_elapsed: Option<Duration>,
        mut timings: ImageThumbnailExtractionTimings,
    ) -> Self {
        timings.set(ThumbnailStage::CacheRead, cache_read_elapsed);
        timings.set(ThumbnailStage::Extract, extract_elapsed);
        Self {
            cache_hit: Some(false),
            timings,
            ..Self::empty(ImageThumbnailLoadOutcome::Failed)
        }
    }

    fn cancelled() -> Self {
        Self::empty(ImageThumbnailLoadOutcome::Cancelled)
    }

    fn cancelled_after_cache_read(cache_hit: bool, cache_read_elapsed: Option<Duration>) -> Self {
        let mut timings = ImageThumbnailExtractionTimings::default();
        timings.set(ThumbnailStage::CacheRead, cache_read_elapsed);
        Self {
            cache_hit: Some(cache_hit),
            timings,
            ..Self::empty(ImageThumbnailLoadOutcome::Cancelled)
        }
    }

    fn cancelled_after_extract(
        cache_read_elapsed: Option<Duration>,
        extract_elapsed: Option<Duration>,
        mut timings: ImageThumbnailExtractionTimings,
    ) -> Self {
        timings.set(ThumbnailStage::CacheRead, cache_read_elapsed);
        timings.set(ThumbnailStage::Extract, extract_elapsed);
        Self {
            cache_hit: Some(false),
            timings,
            ..Self::empty(ImageThumbnailLoadOutcome::Cancelled)
        }
    }

    fn cache_write_job(&self, job: &ImageThumbnailLoadJob) -> Option<ImageThumbnailCacheWriteJob> {
        if self.outcome != ImageThumbnailLoadOutcome::Generated {
            return None;
        }

        Some(ImageThumbnailCacheWriteJob {
            cache_dir: job.cache_dir.clone()?,
            key: job.request.key.clone(),
            source_path: job.request.path.clone(),
            image: self.cache_image.clone()?,
            queued_at: Instant::now(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
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
    stages: [ImageThumbnailStageTimingStats; ThumbnailStage::COUNT],
}

impl Default for ImageThumbnailTimingBatch {
    fn default() -> Self {
        Self {
            enabled: false,
            batch_started: None,
            requests: 0,
            cache_hits: 0,
            cache_misses: 0,
            generated: 0,
            failed: 0,
            cancelled: 0,
            discarded: 0,
            cache_writes_scheduled: 0,
            stages: std::array::from_fn(|_| ImageThumbnailStageTimingStats::default()),
        }
    }
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
            self.record_stage(ThumbnailStage::QueueWait, elapsed);
        }
    }

    fn record_load_result(&mut self, result: &ImageThumbnailLoadResult) {
        if !self.enabled {
            return;
        }

        if let Some(cache_hit) = result.cache_hit {
            if cache_hit {
                self.cache_hits += 1;
            } else {
                self.cache_misses += 1;
            }
        }
        self.record_timings(&result.timings);

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
            self.record_stage(ThumbnailStage::Commit, started.elapsed());
        }
    }

    fn record_timings(&mut self, timings: &ImageThumbnailExtractionTimings) {
        for (stage, elapsed) in timings.stages() {
            if let Some(elapsed) = elapsed {
                self.record_stage(stage, elapsed);
            }
        }
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
            self.record_stage(ThumbnailStage::RequestTotal, started.elapsed());
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
        for stage in ThumbnailStage::ALL {
            push_image_thumbnail_stage_line(&mut lines, stage.name(), self.stage(stage));
        }
        lines
    }

    fn record_stage(&mut self, stage: ThumbnailStage, elapsed: Duration) {
        self.stages[stage as usize].record(elapsed);
    }

    fn stage(&self, stage: ThumbnailStage) -> &ImageThumbnailStageTimingStats {
        &self.stages[stage as usize]
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

fn format_image_thumbnail_timing_duration(elapsed: Duration) -> String {
    format!("{:.3}ms", elapsed.as_secs_f64() * 1000.0)
}

fn image_thumbnail_request_for_entry(
    entry: &FileEntry,
    directory: &Path,
    source_policy: ThumbnailSourcePolicy,
) -> Option<ImageThumbnailRequest> {
    if entry.is_directory_like() {
        return None;
    }

    let kind = image_thumbnail_kind_for_path(&entry.path)?;

    Some(ImageThumbnailRequest {
        kind,
        usage: ImageThumbnailUsage::Standard,
        source_policy,
        key: image_thumbnail_key(entry, kind),
        path: entry.path.clone(),
        directory: directory.to_path_buf(),
    })
}

pub(super) fn entry_may_have_hover_image_preview(entry: &FileEntry) -> bool {
    !entry.is_directory_like() && path_may_have_image_preview(&entry.path)
}

pub(super) fn entry_may_have_hover_video_preview(entry: &FileEntry) -> bool {
    !entry.is_directory_like() && path_may_have_video_metadata(&entry.path)
}

fn hover_image_preview_request_for_entry(
    entry: &FileEntry,
    directory: &Path,
    source_policy: ThumbnailSourcePolicy,
) -> Option<ImageThumbnailRequest> {
    if !entry_may_have_hover_image_preview(entry) {
        return None;
    }

    Some(ImageThumbnailRequest {
        kind: ImageThumbnailKind::Image,
        usage: ImageThumbnailUsage::HoverPreview,
        source_policy,
        key: hover_image_preview_key(entry),
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
    image_thumbnail_key_for_usage(entry, kind, ImageThumbnailUsage::Standard)
}

fn hover_image_preview_key(entry: &FileEntry) -> String {
    image_thumbnail_key_for_usage(
        entry,
        ImageThumbnailKind::Image,
        ImageThumbnailUsage::HoverPreview,
    )
}

fn image_thumbnail_key_for_usage(
    entry: &FileEntry,
    kind: ImageThumbnailKind,
    usage: ImageThumbnailUsage,
) -> String {
    let mut hash = StableHash::new();
    hash.write_str(IMAGE_THUMBNAIL_CACHE_VERSION);
    hash.write_str(usage.cache_namespace(kind));
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
        .and_then(valid_qoi_bytes)
}

fn write_cached_thumbnail(cache_dir: Option<&Path>, key: &str, bytes: &[u8]) -> bool {
    let Some(path) = thumbnail_file_path(cache_dir, key) else {
        return false;
    };
    write_atomic(&path, bytes).is_ok()
}

#[cfg(test)]
fn write_cached_thumbnail_with_source(
    cache_dir: Option<&Path>,
    key: &str,
    source_path: &Path,
    bytes: &[u8],
) -> bool {
    if !write_cached_thumbnail(cache_dir, key, bytes) {
        return false;
    }
    if let Some(cache_dir) = cache_dir {
        let mut manifest = load_disk_manifest(Some(cache_dir)).unwrap_or_default();
        manifest
            .mappings
            .insert(key.to_owned(), source_path.to_path_buf());
        let _ = save_disk_manifest(cache_dir, &manifest);
    }
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

    Some(cache_dir?.join(format!("{key}.qoi")))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct DiskThumbnailManifest {
    version: String,
    #[serde(default)]
    mappings: HashMap<String, PathBuf>,
}

impl Default for DiskThumbnailManifest {
    fn default() -> Self {
        Self {
            version: IMAGE_THUMBNAIL_CACHE_VERSION.to_owned(),
            mappings: HashMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ImageThumbnailCacheCleanupSummary {
    removed_mappings: usize,
    removed_files: usize,
}

pub(super) fn cleanup_stale_path_cache_entries() -> ImageThumbnailCacheCleanupSummary {
    let Some(cache_dir) = image_thumbnail_cache_dir() else {
        return ImageThumbnailCacheCleanupSummary::default();
    };
    cleanup_legacy_image_thumbnail_cache_dirs(&cache_dir);
    cleanup_stale_path_cache_entries_in_dir(&cache_dir)
}

fn cleanup_legacy_image_thumbnail_cache_dirs(current_cache_dir: &Path) {
    let Some(parent) = current_cache_dir.parent() else {
        return;
    };
    for version in LEGACY_IMAGE_THUMBNAIL_CACHE_VERSIONS {
        let legacy = parent.join(version);
        if legacy != current_cache_dir {
            let _ = fs::remove_dir_all(legacy);
        }
    }
}

fn cleanup_stale_path_cache_entries_in_dir(cache_dir: &Path) -> ImageThumbnailCacheCleanupSummary {
    let Some(mut manifest) = load_disk_manifest(Some(cache_dir)) else {
        return ImageThumbnailCacheCleanupSummary::default();
    };

    let mut removed_keys = Vec::new();
    manifest.mappings.retain(|key, source_path| {
        if matches!(source_path.try_exists(), Ok(false)) {
            removed_keys.push(key.clone());
            false
        } else {
            true
        }
    });

    if removed_keys.is_empty() {
        return ImageThumbnailCacheCleanupSummary::default();
    }

    let mut summary = ImageThumbnailCacheCleanupSummary {
        removed_mappings: removed_keys.len(),
        removed_files: 0,
    };
    for key in &removed_keys {
        if let Some(path) = thumbnail_file_path(Some(cache_dir), key)
            && path.is_file()
            && fs::remove_file(path).is_ok()
        {
            summary.removed_files += 1;
        }
    }
    let _ = save_disk_manifest(cache_dir, &manifest);
    summary
}

fn load_disk_manifest(cache_dir: Option<&Path>) -> Option<DiskThumbnailManifest> {
    let path = cache_dir?.join(DISK_MANIFEST_FILE_NAME);
    let manifest =
        serde_json::from_str::<DiskThumbnailManifest>(&fs::read_to_string(path).ok()?).ok()?;
    (manifest.version == IMAGE_THUMBNAIL_CACHE_VERSION).then_some(manifest)
}

fn save_disk_manifest(cache_dir: &Path, manifest: &DiskThumbnailManifest) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(manifest).map_err(io::Error::other)?;
    write_atomic(&cache_dir.join(DISK_MANIFEST_FILE_NAME), &json)
}

pub(super) fn dimensions_for_preview(width: u32, height: u32, size: u32) -> Option<(u32, u32)> {
    dimensions_for_longest_side(width, height, size)
}

#[cfg(test)]
fn cached_thumbnail_image_from_png_bytes(bytes: Vec<u8>) -> Option<CachedThumbnailImage> {
    cached_thumbnail_image_from_rgba(decode_png_rgba(&bytes)?).into()
}

fn encode_rgba_qoi_bytes(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    qoi::encode_to_vec(rgba, width, height).ok()
}

fn decode_cached_thumbnail_rgba(bytes: &[u8]) -> Option<image::RgbaImage> {
    let (header, pixels) = qoi::decode_to_vec(bytes).ok()?;
    header
        .channels
        .is_rgba()
        .then(|| image::RgbaImage::from_raw(header.width, header.height, pixels))?
}

#[cfg(test)]
fn decode_png_rgba(bytes: &[u8]) -> Option<image::RgbaImage> {
    image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .ok()
        .map(image::DynamicImage::into_rgba8)
}

#[cfg(any(test, feature = "benchmarks"))]
fn cached_thumbnail_image_from_rgba(image: image::RgbaImage) -> CachedThumbnailImage {
    cached_thumbnail_image_from_rgba_with_animated_source(image, None)
}

fn cached_thumbnail_image_from_rgba_with_animated_source(
    mut image: image::RgbaImage,
    animated_source: Option<AnimatedImageSource>,
) -> CachedThumbnailImage {
    let width = image.width();
    let height = image.height();
    for pixel in image.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    CachedThumbnailImage {
        image: Arc::new(RenderImage::new(vec![image::Frame::new(image)])),
        width,
        height,
        animated_source,
    }
}

fn valid_qoi_bytes(bytes: Vec<u8>) -> Option<Vec<u8>> {
    bytes.starts_with(QOI_SIGNATURE).then_some(bytes)
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
    image_thumbnail_cache_dir_for(current_config_platform(), env_path)
}

fn image_thumbnail_cache_dir_for(
    platform: ConfigPlatform,
    env_path: impl FnMut(&str) -> Option<PathBuf>,
) -> Option<PathBuf> {
    platform_cache_dir(platform, env_path).map(|dir| dir.join(IMAGE_THUMBNAIL_CACHE_VERSION))
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
    env_path: impl FnMut(&str) -> Option<PathBuf>,
) -> Option<PathBuf> {
    config_dir_for(platform, env_path).map(|dir| dir.join("cache"))
}

fn normalized_path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(feature = "benchmarks")]
pub mod benchmark_support {
    use std::{
        path::{Path, PathBuf},
        time::Instant,
    };

    pub fn encode_cached_thumbnail_for_benchmark(image: image::RgbaImage) -> Option<Vec<u8>> {
        super::encode_rgba_qoi_bytes(image.as_raw(), image.width(), image.height())
    }

    pub fn prepare_cached_thumbnail_for_benchmark(bytes: Vec<u8>) -> Option<(u32, u32)> {
        let image =
            super::cached_thumbnail_image_from_rgba(super::decode_cached_thumbnail_rgba(&bytes)?);
        Some((image.width, image.height))
    }

    pub fn write_cached_thumbnail_batch_for_benchmark(
        cache_dir: &Path,
        images: &[image::RgbaImage],
    ) -> usize {
        let jobs = images
            .iter()
            .enumerate()
            .map(|(index, image)| super::ImageThumbnailCacheWriteJob {
                cache_dir: cache_dir.to_path_buf(),
                key: format!("{index:016x}"),
                source_path: cache_dir.join(format!("source-{index}.png")),
                image: image.clone(),
                queued_at: Instant::now(),
            })
            .collect();
        let mut manifest = super::load_disk_manifest(Some(cache_dir)).unwrap_or_default();
        super::write_image_thumbnail_cache_batch(cache_dir, &mut manifest, jobs).written
    }

    pub fn queue_and_cancel_thumbnails_for_benchmark(count: usize) -> usize {
        let directory = PathBuf::from("benchmark-folder");
        let mut cache = super::ImageThumbnailCacheInner::with_writer(None, None);
        for index in 0..count {
            let request = super::ImageThumbnailRequest {
                kind: super::ImageThumbnailKind::Image,
                usage: super::ImageThumbnailUsage::Standard,
                source_policy: super::ThumbnailSourcePolicy::ReadSource,
                key: format!("benchmark-{index}"),
                path: directory.join(format!("image-{index}.png")),
                directory: directory.clone(),
            };
            let _ = cache.thumbnail_for_request(request);
        }
        let queued = cache.pending.len();
        let _ = cache.cancel_directory(&directory);
        queued + cache.pending.len()
    }
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
    fn thumbnail_cache_version_invalidates_prepared_render_images() {
        assert_eq!(IMAGE_THUMBNAIL_CACHE_VERSION, "image-thumbnails-v2");
    }

    #[test]
    fn thumbnail_cache_dirs_follow_platform_conventions() {
        assert_eq!(
            image_thumbnail_cache_dir_for(ConfigPlatform::MacOS, |name| {
                (name == "HOME").then(|| PathBuf::from("home"))
            }),
            Some(
                PathBuf::from("home")
                    .join(".config")
                    .join("explorer")
                    .join("cache")
                    .join(IMAGE_THUMBNAIL_CACHE_VERSION)
            )
        );
        assert_eq!(
            image_thumbnail_cache_dir_for(ConfigPlatform::Windows, |name| {
                (name == "USERPROFILE").then(|| PathBuf::from("profile"))
            }),
            Some(
                PathBuf::from("profile")
                    .join(".config")
                    .join("explorer")
                    .join("cache")
                    .join(IMAGE_THUMBNAIL_CACHE_VERSION)
            )
        );
        assert_eq!(
            image_thumbnail_cache_dir_for(ConfigPlatform::Windows, |name| {
                (name == "LOCALAPPDATA").then(|| PathBuf::from("local"))
            }),
            None
        );
        assert_eq!(
            image_thumbnail_cache_dir_for(ConfigPlatform::Linux, |name| {
                (name == "XDG_CONFIG_HOME").then(|| PathBuf::from("xdg"))
            }),
            Some(
                PathBuf::from("xdg")
                    .join("explorer")
                    .join("cache")
                    .join(IMAGE_THUMBNAIL_CACHE_VERSION)
            )
        );
        assert_eq!(
            image_thumbnail_cache_dir_for(ConfigPlatform::Linux, |name| {
                (name == "XDG_CACHE_HOME").then(|| PathBuf::from("xdg-cache"))
            }),
            None
        );
    }

    #[test]
    fn thumbnail_requests_include_supported_image_extensions() {
        for name in ["image.png", "photo.jpg", "poster.webp", "vector.svg"] {
            let entry = FileEntry::test(name, false, Some(1), Some(UNIX_EPOCH));
            let request = image_thumbnail_request_for_entry(
                &entry,
                Path::new("folder"),
                ThumbnailSourcePolicy::ReadSource,
            )
            .unwrap_or_else(|| panic!("expected request for {name}"));
            assert_eq!(request.kind, ImageThumbnailKind::Image);
            assert_eq!(request.usage, ImageThumbnailUsage::Standard);
        }
    }

    #[test]
    fn thumbnail_requests_include_supported_video_extensions() {
        for name in ["movie.mp4", "clip.mkv", "camera.mov"] {
            let entry = FileEntry::test(name, false, Some(1), Some(UNIX_EPOCH));
            let request = image_thumbnail_request_for_entry(
                &entry,
                Path::new("folder"),
                ThumbnailSourcePolicy::ReadSource,
            )
            .unwrap_or_else(|| panic!("expected request for {name}"));
            assert_eq!(request.kind, ImageThumbnailKind::Video);
            assert_eq!(request.usage, ImageThumbnailUsage::Standard);
        }
    }

    #[test]
    fn thumbnail_requests_skip_directories_and_non_images() {
        assert!(
            image_thumbnail_request_for_entry(
                &FileEntry::test("folder", true, None, Some(UNIX_EPOCH)),
                Path::new("folder"),
                ThumbnailSourcePolicy::ReadSource,
            )
            .is_none()
        );
        assert!(
            image_thumbnail_request_for_entry(
                &FileEntry::test("notes.txt", false, Some(1), Some(UNIX_EPOCH)),
                Path::new("folder"),
                ThumbnailSourcePolicy::ReadSource,
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
    fn standard_image_thumbnail_key_uses_current_cache_namespace() {
        let entry = FileEntry::test("image.png", false, Some(1), Some(UNIX_EPOCH));

        assert_eq!(
            image_thumbnail_key(&entry, ImageThumbnailKind::Image),
            "37df2f5441ec5ea6"
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
    fn hover_preview_key_is_distinct_from_standard_image_thumbnail_key() {
        let entry = FileEntry::test("image.png", false, Some(1), Some(UNIX_EPOCH));
        let preview = hover_image_preview_request_for_entry(
            &entry,
            Path::new("folder"),
            ThumbnailSourcePolicy::ReadSource,
        )
        .expect("expected hover preview request");

        assert_eq!(preview.kind, ImageThumbnailKind::Image);
        assert_eq!(preview.usage, ImageThumbnailUsage::HoverPreview);
        assert_eq!(
            preview.usage.cache_namespace(preview.kind),
            "image-hover-preview-v1"
        );
        assert_eq!(preview.key, hover_image_preview_key(&entry));
        assert_ne!(
            preview.key,
            image_thumbnail_key(&entry, ImageThumbnailKind::Image)
        );
    }

    #[test]
    fn hover_preview_requests_are_image_only() {
        assert!(
            hover_image_preview_request_for_entry(
                &FileEntry::test("folder", true, None, Some(UNIX_EPOCH)),
                Path::new("folder"),
                ThumbnailSourcePolicy::ReadSource,
            )
            .is_none()
        );
        assert!(
            hover_image_preview_request_for_entry(
                &FileEntry::test("notes.txt", false, Some(1), Some(UNIX_EPOCH)),
                Path::new("folder"),
                ThumbnailSourcePolicy::ReadSource,
            )
            .is_none()
        );
        assert!(
            hover_image_preview_request_for_entry(
                &FileEntry::test("clip.mp4", false, Some(1), Some(UNIX_EPOCH)),
                Path::new("folder"),
                ThumbnailSourcePolicy::ReadSource,
            )
            .is_none()
        );
        assert!(
            hover_image_preview_request_for_entry(
                &FileEntry::test("image.png", false, Some(1), Some(UNIX_EPOCH)),
                Path::new("folder"),
                ThumbnailSourcePolicy::ReadSource,
            )
            .is_some()
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
        batch.record_stage(ThumbnailStage::QueueWait, Duration::from_millis(2));
        batch.record_stage(ThumbnailStage::QueueWait, Duration::from_micros(500));

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
        let mut extraction_timings = ImageThumbnailExtractionTimings::default();
        for (stage, millis) in [
            (ThumbnailStage::SourceRead, 1),
            (ThumbnailStage::FormatDetect, 2),
            (ThumbnailStage::RasterDecode, 3),
            (ThumbnailStage::RgbaConvert, 4),
            (ThumbnailStage::SvgParse, 5),
            (ThumbnailStage::SvgRender, 6),
            (ThumbnailStage::SvgUnpremultiply, 7),
            (ThumbnailStage::ResizeCanvas, 8),
            (ThumbnailStage::PngEncode, 9),
            (ThumbnailStage::EmbeddedThumbnailScan, 11),
            (ThumbnailStage::EmbeddedThumbnailDecode, 12),
            (ThumbnailStage::TiffIfdScan, 13),
            (ThumbnailStage::TiffRawSample, 14),
            (ThumbnailStage::TiffChunkDecode, 15),
            (ThumbnailStage::TiffChunkSample, 16),
        ] {
            extraction_timings.record(stage, Duration::from_millis(millis));
        }
        let result = ImageThumbnailLoadResult::generated(
            image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255])),
            None,
            None,
            Some(Duration::from_millis(10)),
            extraction_timings,
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
            image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255])),
            None,
            Some(Duration::from_millis(1)),
            Some(Duration::from_millis(2)),
            ImageThumbnailExtractionTimings::default(),
        );

        let write = generated.cache_write_job(&job).expect("cache write job");
        assert_eq!(write.cache_dir, cache_dir);
        assert_eq!(write.key, "generated");
        assert_eq!(
            write.source_path,
            PathBuf::from("folder").join("generated.png")
        );
        assert_eq!(write.image.as_raw(), &[1, 2, 3, 255]);

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
        assert_eq!(batch.stage(ThumbnailStage::CacheRead).count, 4);
        assert_eq!(batch.stage(ThumbnailStage::Extract).count, 3);
        assert_eq!(batch.stage(ThumbnailStage::CacheWrite).count, 0);
        assert_eq!(batch.stage(ThumbnailStage::RenderPrepare).count, 1);
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
        assert_eq!(batch.stage(ThumbnailStage::QueueWait).count, 1);
        assert_eq!(batch.discarded, 1);
        assert_eq!(batch.cache_writes_scheduled, 1);
        assert!(batch.stage(ThumbnailStage::RequestTotal).count <= 1);

        batch.finish();
    }

    #[test]
    fn cached_thumbnail_round_trips_from_disk() {
        let temp = TempDir::new();
        let source = temp.path().join("image.png");
        fs::write(&source, png_bytes(4, 2)).unwrap();
        let request = ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            usage: ImageThumbnailUsage::Standard,
            source_policy: ThumbnailSourcePolicy::ReadSource,
            key: "0123456789abcdef".to_owned(),
            path: source,
            directory: temp.path().to_path_buf(),
        };
        let cancel = AtomicBool::new(false);

        let generated =
            load_or_create_thumbnail_cache_bytes(&request, Some(temp.path()), &cancel).unwrap();
        assert_eq!(
            decode_cached_thumbnail_rgba(&generated).map(|image| image.dimensions()),
            Some((128, 64))
        );
        assert!(write_cached_thumbnail(
            Some(temp.path()),
            &request.key,
            &generated
        ));
        let cached =
            load_or_create_thumbnail_cache_bytes(&request, Some(temp.path()), &cancel).unwrap();

        assert_eq!(generated, cached);
        assert!(
            thumbnail_file_path(Some(temp.path()), &request.key)
                .unwrap()
                .is_file()
        );
    }

    #[test]
    fn corrupt_qoi_cache_entry_is_removed_and_regenerated() {
        let temp = TempDir::new();
        let source = temp.path().join("image.png");
        fs::write(&source, png_bytes(4, 2)).unwrap();
        let request = ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            usage: ImageThumbnailUsage::Standard,
            source_policy: ThumbnailSourcePolicy::ReadSource,
            key: "0123456789abcdef".to_owned(),
            path: source,
            directory: temp.path().to_path_buf(),
        };
        let cache_path = thumbnail_file_path(Some(temp.path()), &request.key).unwrap();
        fs::write(&cache_path, b"qoif-corrupt").unwrap();

        let result = load_or_create_thumbnail_with_timings(
            &request,
            Some(temp.path()),
            &AtomicBool::new(false),
            true,
        );

        assert_eq!(result.outcome, ImageThumbnailLoadOutcome::Generated);
        assert_eq!(
            result.image.map(|image| (image.width, image.height)),
            Some((128, 64))
        );
        assert!(!cache_path.exists());
    }

    #[test]
    fn cache_writer_batches_qoi_files_and_one_manifest() {
        let temp = TempDir::new();
        let mut manifest = DiskThumbnailManifest::default();
        let jobs = (0..3)
            .map(|index| ImageThumbnailCacheWriteJob {
                cache_dir: temp.path().to_path_buf(),
                key: format!("{index:016x}"),
                source_path: temp.path().join(format!("source-{index}.png")),
                image: image::RgbaImage::from_pixel(4, 2, image::Rgba([index as u8, 40, 80, 255])),
                queued_at: Instant::now(),
            })
            .collect();

        let metrics = write_image_thumbnail_cache_batch(temp.path(), &mut manifest, jobs);

        assert_eq!(metrics.written, 3);
        assert_eq!(manifest.mappings.len(), 3);
        assert_eq!(load_disk_manifest(Some(temp.path())), Some(manifest));
        for index in 0..3 {
            let bytes = read_cached_thumbnail(Some(temp.path()), &format!("{index:016x}"))
                .expect("cached QOI");
            let image = decode_cached_thumbnail_rgba(&bytes).expect("decode cached QOI");
            assert_eq!(image.dimensions(), (4, 2));
            assert_eq!(
                image.get_pixel(0, 0),
                &image::Rgba([index as u8, 40, 80, 255])
            );
        }
    }

    #[test]
    fn saturated_cache_writer_drops_work_without_blocking() {
        let temp = TempDir::new();
        let (sender, _receiver) = mpsc::channel(1);
        let mut cache =
            ImageThumbnailCacheInner::with_writer(Some(temp.path().to_path_buf()), Some(sender));
        let job = |key: &str| ImageThumbnailCacheWriteJob {
            cache_dir: temp.path().to_path_buf(),
            key: key.to_owned(),
            source_path: temp.path().join("source.png"),
            image: image::RgbaImage::new(1, 1),
            queued_at: Instant::now(),
        };

        assert!(cache.queue_cache_write(job("0000000000000001")));
        assert!(cache.queue_cache_write(job("0000000000000002")));
        assert!(!cache.queue_cache_write(job("0000000000000003")));
    }

    #[test]
    fn cache_only_thumbnail_returns_cached_qoi() {
        let temp = TempDir::new();
        let request = ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            usage: ImageThumbnailUsage::Standard,
            source_policy: ThumbnailSourcePolicy::CacheOnly,
            key: "0123456789abcdef".to_owned(),
            path: temp.path().join("missing.png"),
            directory: temp.path().to_path_buf(),
        };
        write_cached_thumbnail(Some(temp.path()), &request.key, &qoi_bytes(4, 2));

        let result = load_or_create_thumbnail_with_timings(
            &request,
            Some(temp.path()),
            &AtomicBool::new(false),
            true,
        );

        assert_eq!(result.outcome, ImageThumbnailLoadOutcome::CacheHit);
        assert_eq!(
            result
                .image
                .as_ref()
                .map(|image| (image.width, image.height)),
            Some((4, 2))
        );
        assert!(result.timings.get(ThumbnailStage::Extract).is_none());
    }

    #[test]
    fn cache_only_thumbnail_miss_does_not_read_image_source() {
        let temp = TempDir::new();
        let source = temp.path().join("image.png");
        fs::write(&source, png_bytes(4, 2)).unwrap();
        let request = ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            usage: ImageThumbnailUsage::Standard,
            source_policy: ThumbnailSourcePolicy::CacheOnly,
            key: "0123456789abcdef".to_owned(),
            path: source,
            directory: temp.path().to_path_buf(),
        };

        let result = load_or_create_thumbnail_with_timings(
            &request,
            Some(temp.path()),
            &AtomicBool::new(false),
            true,
        );

        assert_eq!(result.outcome, ImageThumbnailLoadOutcome::Failed);
        assert!(result.image.is_none());
        assert!(result.timings.get(ThumbnailStage::Extract).is_none());
        assert_eq!(
            load_or_create_thumbnail_cache_bytes(
                &request,
                Some(temp.path()),
                &AtomicBool::new(false)
            ),
            None
        );
    }

    #[test]
    fn cache_only_video_thumbnail_miss_does_not_start_probe_or_ffmpeg() {
        let temp = TempDir::new();
        let request = ImageThumbnailRequest {
            kind: ImageThumbnailKind::Video,
            usage: ImageThumbnailUsage::Standard,
            source_policy: ThumbnailSourcePolicy::CacheOnly,
            key: "0123456789abcdef".to_owned(),
            path: temp.path().join("movie.mp4"),
            directory: temp.path().to_path_buf(),
        };

        let result = load_or_create_thumbnail_with_timings(
            &request,
            Some(temp.path()),
            &AtomicBool::new(false),
            true,
        );

        assert_eq!(result.outcome, ImageThumbnailLoadOutcome::Failed);
        assert!(result.image.is_none());
        assert!(result.timings.get(ThumbnailStage::Extract).is_none());
    }

    #[test]
    fn cache_only_failed_thumbnail_can_retry_as_read_source() {
        let mut cache = ImageThumbnailCacheInner::new(None);
        let cache_only = ImageThumbnailRequest {
            source_policy: ThumbnailSourcePolicy::CacheOnly,
            ..request("shared", "folder")
        };
        let read_source = ImageThumbnailRequest {
            source_policy: ThumbnailSourcePolicy::ReadSource,
            ..cache_only.clone()
        };
        let (_, cache_only_generation) = cache.thumbnail_for_request(cache_only.clone());
        let cache_only_generation = cache_only_generation.expect("cache-only generation");
        let _job = cache
            .next_load_job(cache_only_generation)
            .expect("cache-only job");
        assert!(cache.finish_request(cache_only, cache_only_generation, None));
        assert!(cache.next_load_job(cache_only_generation).is_none());

        let (_, read_source_generation) = cache.thumbnail_for_request(read_source.clone());

        assert!(read_source_generation.is_some());
        assert!(matches!(
            cache.states.get(&read_source.key),
            Some(ImageThumbnailState::Pending { request, .. })
                if request.source_policy == ThumbnailSourcePolicy::ReadSource
        ));
    }

    #[test]
    fn cache_only_failed_hover_preview_can_retry_as_read_source() {
        let mut cache = ImageThumbnailCacheInner::new(None);
        let cache_only = ImageThumbnailRequest {
            usage: ImageThumbnailUsage::HoverPreview,
            source_policy: ThumbnailSourcePolicy::CacheOnly,
            ..request("hover", "folder")
        };
        let read_source = ImageThumbnailRequest {
            source_policy: ThumbnailSourcePolicy::ReadSource,
            ..cache_only.clone()
        };
        let standard_request = ImageThumbnailRequest {
            key: "standard".to_owned(),
            usage: ImageThumbnailUsage::Standard,
            ..cache_only.clone()
        };
        let (_, cache_only_generation) =
            cache.hover_preview_for_request(cache_only.clone(), standard_request.clone());
        let cache_only_generation = cache_only_generation.expect("cache-only generation");
        let _job = cache
            .next_load_job(cache_only_generation)
            .expect("cache-only hover job");
        assert!(cache.finish_request(cache_only, cache_only_generation, None));
        assert!(cache.next_load_job(cache_only_generation).is_none());

        let (_, read_source_generation) =
            cache.hover_preview_for_request(read_source.clone(), standard_request);

        assert!(read_source_generation.is_some());
        assert!(matches!(
            cache.states.get(&read_source.key),
            Some(ImageThumbnailState::Pending { request, .. })
                if request.source_policy == ThumbnailSourcePolicy::ReadSource
                    && request.usage == ImageThumbnailUsage::HoverPreview
        ));
    }

    #[test]
    fn hover_preview_thumbnail_preserves_aspect_ratio() {
        let temp = TempDir::new();
        let source = temp.path().join("image.png");
        fs::write(&source, png_bytes(8, 4)).unwrap();
        let request = ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            usage: ImageThumbnailUsage::HoverPreview,
            source_policy: ThumbnailSourcePolicy::ReadSource,
            key: "0123456789abcdef".to_owned(),
            path: source,
            directory: temp.path().to_path_buf(),
        };
        let cancel = AtomicBool::new(false);

        let generated =
            load_or_create_thumbnail_cache_bytes(&request, Some(temp.path()), &cancel).unwrap();
        let image =
            cached_thumbnail_image_from_rgba(decode_cached_thumbnail_rgba(&generated).unwrap());

        assert_eq!((image.width, image.height), (400, 200));
    }

    #[test]
    fn hover_preview_lookup_uses_placeholder_dimensions_without_file_io() {
        let temp = TempDir::new();
        let source = temp.path().join("image.png");
        fs::write(&source, png_bytes(8, 4)).unwrap();
        let request = ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            usage: ImageThumbnailUsage::HoverPreview,
            source_policy: ThumbnailSourcePolicy::ReadSource,
            key: "0123456789abcdef".to_owned(),
            path: source,
            directory: temp.path().to_path_buf(),
        };
        let mut cache = ImageThumbnailCacheInner::new(None);

        let standard_request = ImageThumbnailRequest {
            usage: ImageThumbnailUsage::Standard,
            key: "fedcba9876543210".to_owned(),
            ..request.clone()
        };
        let (lookup, generation) =
            cache.hover_preview_for_request(request.clone(), standard_request.clone());

        assert!(generation.is_some());
        assert!(matches!(
            lookup,
            HoverImagePreviewLookup::Loading {
                width: 400,
                height: 400,
                thumbnail: None
            }
        ));
        assert!(matches!(
            cache.hover_preview_for_request(request, standard_request).0,
            HoverImagePreviewLookup::Loading {
                width: 400,
                height: 400,
                thumbnail: None
            }
        ));
    }

    #[gpui::test]
    fn hover_image_preview_request_uses_cache_only_when_standard_policy_is_cache_only(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|app| initialize_for_test(app));
        let (view, cx) = cx.add_window_view(|window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            let mut view = ExplorerView::new_unloaded_with_settings_for_test(
                PathBuf::from("remote"),
                Some(focus_handle),
                &crate::settings::ExplorerSettings::default(),
            );
            view.thumbnail_source_policy = ThumbnailSourcePolicy::CacheOnly;
            view
        });

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                let entry = FileEntry::test("image.png", false, Some(1), Some(UNIX_EPOCH));

                assert!(matches!(
                    view.hover_image_preview_for_entry(&entry, cx),
                    Some(HoverImagePreviewLookup::Loading {
                        width: HOVER_IMAGE_PREVIEW_SIZE,
                        height: HOVER_IMAGE_PREVIEW_SIZE,
                        thumbnail: None,
                    })
                ));

                let hover_key = hover_image_preview_key(&entry);
                let standard_key = image_thumbnail_key(&entry, ImageThumbnailKind::Image);
                let cache = cx.global::<ImageThumbnailCache>();
                let cache = cache.inner.borrow();
                assert!(matches!(
                    cache.states.get(&hover_key),
                    Some(ImageThumbnailState::Pending { request, .. })
                        if request.source_policy == ThumbnailSourcePolicy::CacheOnly
                            && request.usage == ImageThumbnailUsage::HoverPreview
                ));
                assert!(!cache.states.contains_key(&standard_key));
            });
        });
    }

    #[test]
    fn hover_preview_reuses_ready_standard_thumbnail_without_loading_it() {
        let temp = TempDir::new();
        let source = temp.path().join("image.png");
        fs::write(&source, png_bytes(8, 4)).unwrap();
        let hover_request = ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            usage: ImageThumbnailUsage::HoverPreview,
            source_policy: ThumbnailSourcePolicy::ReadSource,
            key: "0123456789abcdef".to_owned(),
            path: source.clone(),
            directory: temp.path().to_path_buf(),
        };
        let standard_request = ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            usage: ImageThumbnailUsage::Standard,
            source_policy: ThumbnailSourcePolicy::ReadSource,
            key: "fedcba9876543210".to_owned(),
            path: source,
            directory: temp.path().to_path_buf(),
        };
        let thumbnail =
            cached_thumbnail_image_from_png_bytes(png_bytes(128, 128)).expect("thumbnail");
        let mut cache = ImageThumbnailCacheInner::new(None);
        cache.states.insert(
            standard_request.key.clone(),
            ImageThumbnailState::Ready(thumbnail),
        );

        let (lookup, generation) =
            cache.hover_preview_for_request(hover_request, standard_request.clone());

        assert!(generation.is_some());
        assert!(matches!(
            lookup,
            HoverImagePreviewLookup::Loading {
                width: 400,
                height: 400,
                thumbnail: Some(CachedThumbnailImage {
                    width: 128,
                    height: 128,
                    ..
                })
            }
        ));
        assert_eq!(cache.pending.len(), 1);
        assert!(matches!(
            cache.states.get(&standard_request.key),
            Some(ImageThumbnailState::Ready(_))
        ));
    }

    #[test]
    fn hover_preview_replaces_loading_thumbnail_with_ready_preview() {
        let temp = TempDir::new();
        let source = temp.path().join("image.png");
        fs::write(&source, png_bytes(8, 4)).unwrap();
        let hover_request = ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            usage: ImageThumbnailUsage::HoverPreview,
            source_policy: ThumbnailSourcePolicy::ReadSource,
            key: "0123456789abcdef".to_owned(),
            path: source.clone(),
            directory: temp.path().to_path_buf(),
        };
        let standard_request = ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            usage: ImageThumbnailUsage::Standard,
            source_policy: ThumbnailSourcePolicy::ReadSource,
            key: "fedcba9876543210".to_owned(),
            path: source,
            directory: temp.path().to_path_buf(),
        };
        let thumbnail =
            cached_thumbnail_image_from_png_bytes(png_bytes(128, 128)).expect("thumbnail");
        let mut cache = ImageThumbnailCacheInner::new(None);
        cache.states.insert(
            standard_request.key.clone(),
            ImageThumbnailState::Ready(thumbnail),
        );
        let (_, generation) =
            cache.hover_preview_for_request(hover_request.clone(), standard_request.clone());
        let generation = generation.expect("loader generation");
        let _job = cache.next_load_job(generation).expect("hover preview job");

        assert!(cache.finish_request(hover_request.clone(), generation, Some(png_bytes(400, 200))));

        assert!(matches!(
            cache
                .hover_preview_for_request(hover_request, standard_request)
                .0,
            HoverImagePreviewLookup::Ready(CachedThumbnailImage {
                width: 400,
                height: 200,
                ..
            })
        ));
    }

    #[test]
    fn generated_gif_hover_preview_keeps_animated_source_path() {
        let temp = TempDir::new();
        let source = temp.path().join("loop.gif");
        fs::write(&source, animated_gif_bytes(8, 4)).unwrap();
        let request = ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            usage: ImageThumbnailUsage::HoverPreview,
            source_policy: ThumbnailSourcePolicy::ReadSource,
            key: "0123456789abcdef".to_owned(),
            path: source.clone(),
            directory: temp.path().to_path_buf(),
        };
        let cancel = AtomicBool::new(false);

        let result = load_or_create_thumbnail_with_timings(&request, None, &cancel, false);

        let image = result.image.expect("generated hover preview");
        assert_eq!(image.width, 400);
        assert_eq!(image.height, 200);
        assert_eq!(
            image.animated_source.as_ref().map(|source| &source.path),
            Some(&source)
        );
        assert_eq!(
            image
                .animated_source
                .as_ref()
                .map(|source| source.cache_key.as_str()),
            Some(request.key.as_str())
        );
    }

    #[test]
    fn cached_gif_hover_preview_keeps_animated_source_path() {
        let temp = TempDir::new();
        let source = temp.path().join("loop.gif");
        fs::write(&source, animated_gif_bytes(8, 4)).unwrap();
        let request = ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            usage: ImageThumbnailUsage::HoverPreview,
            source_policy: ThumbnailSourcePolicy::ReadSource,
            key: "0123456789abcdef".to_owned(),
            path: source.clone(),
            directory: temp.path().to_path_buf(),
        };
        assert!(write_cached_thumbnail(
            Some(temp.path()),
            &request.key,
            &qoi_bytes(400, 200),
        ));
        let cancel = AtomicBool::new(false);

        let result =
            load_or_create_thumbnail_with_timings(&request, Some(temp.path()), &cancel, false);

        let image = result.image.expect("cached hover preview");
        assert_eq!(
            image.animated_source.as_ref().map(|source| &source.path),
            Some(&source)
        );
    }

    #[test]
    fn hover_preview_does_not_read_standard_thumbnail_disk_cache_on_render_path() {
        let temp = TempDir::new();
        let source = temp.path().join("image.png");
        fs::write(&source, png_bytes(8, 4)).unwrap();
        let hover_request = ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            usage: ImageThumbnailUsage::HoverPreview,
            source_policy: ThumbnailSourcePolicy::ReadSource,
            key: "0123456789abcdef".to_owned(),
            path: source.clone(),
            directory: temp.path().to_path_buf(),
        };
        let standard_request = ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            usage: ImageThumbnailUsage::Standard,
            source_policy: ThumbnailSourcePolicy::ReadSource,
            key: "fedcba9876543210".to_owned(),
            path: source,
            directory: temp.path().to_path_buf(),
        };
        assert!(write_cached_thumbnail(
            Some(temp.path()),
            &standard_request.key,
            &qoi_bytes(128, 128)
        ));
        let mut cache = ImageThumbnailCacheInner::new(Some(temp.path().to_path_buf()));

        let (lookup, generation) =
            cache.hover_preview_for_request(hover_request, standard_request.clone());

        assert!(generation.is_some());
        assert!(matches!(
            lookup,
            HoverImagePreviewLookup::Loading {
                thumbnail: None,
                ..
            }
        ));
        assert_eq!(cache.pending.len(), 1);
        assert!(!cache.states.contains_key(&standard_request.key));
    }

    #[test]
    fn hover_preview_does_not_queue_missing_or_invalid_standard_thumbnail() {
        for invalid_cache in [false, true] {
            let temp = TempDir::new();
            let source = temp.path().join("image.png");
            fs::write(&source, png_bytes(8, 4)).unwrap();
            let hover_request = ImageThumbnailRequest {
                kind: ImageThumbnailKind::Image,
                usage: ImageThumbnailUsage::HoverPreview,
                source_policy: ThumbnailSourcePolicy::ReadSource,
                key: "0123456789abcdef".to_owned(),
                path: source.clone(),
                directory: temp.path().to_path_buf(),
            };
            let standard_request = ImageThumbnailRequest {
                kind: ImageThumbnailKind::Image,
                usage: ImageThumbnailUsage::Standard,
                source_policy: ThumbnailSourcePolicy::ReadSource,
                key: "fedcba9876543210".to_owned(),
                path: source,
                directory: temp.path().to_path_buf(),
            };
            if invalid_cache {
                fs::write(
                    thumbnail_file_path(Some(temp.path()), &standard_request.key).unwrap(),
                    b"not qoi",
                )
                .unwrap();
            }
            let mut cache = ImageThumbnailCacheInner::new(Some(temp.path().to_path_buf()));

            let (lookup, generation) =
                cache.hover_preview_for_request(hover_request, standard_request.clone());

            assert!(generation.is_some());
            assert!(matches!(
                lookup,
                HoverImagePreviewLookup::Loading {
                    thumbnail: None,
                    ..
                }
            ));
            assert_eq!(cache.pending.len(), 1);
            assert!(!cache.states.contains_key(&standard_request.key));
        }
    }

    #[test]
    fn hover_preview_invalid_image_is_failed_by_background_loader() {
        let temp = TempDir::new();
        let source = temp.path().join("broken.png");
        fs::write(&source, b"not an image").unwrap();
        let request = ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            usage: ImageThumbnailUsage::HoverPreview,
            source_policy: ThumbnailSourcePolicy::ReadSource,
            key: "0123456789abcdef".to_owned(),
            path: source,
            directory: temp.path().to_path_buf(),
        };
        let mut cache = ImageThumbnailCacheInner::new(None);

        let standard_request = ImageThumbnailRequest {
            usage: ImageThumbnailUsage::Standard,
            key: "fedcba9876543210".to_owned(),
            ..request.clone()
        };
        let (lookup, generation) = cache.hover_preview_for_request(request, standard_request);

        assert!(matches!(
            lookup,
            HoverImagePreviewLookup::Loading {
                width: 400,
                height: 400,
                thumbnail: None
            }
        ));
        assert!(generation.is_some());
        assert_eq!(cache.pending.len(), 1);
    }

    #[test]
    fn finish_request_stores_ready_thumbnail_dimensions() {
        let mut cache = ImageThumbnailCacheInner::new(None);
        let request = request("preview", "folder");
        push_pending(&mut cache, request.clone());
        let generation = cache.start_loader().unwrap();
        let _job = cache.next_load_job(generation).unwrap();

        assert!(cache.finish_request(request.clone(), generation, Some(png_bytes(400, 200))));
        let thumbnail = cache.thumbnail_for_request(request).0.unwrap();

        assert_eq!((thumbnail.width, thumbnail.height), (400, 200));
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
    fn thumbnail_cleanup_deletes_manifest_entries_for_missing_sources() {
        let temp = TempDir::new();
        let source = temp.path().join("missing.png");
        let key = "0123456789abcdef";
        assert!(write_cached_thumbnail_with_source(
            Some(temp.path()),
            key,
            &source,
            &qoi_bytes(4, 2),
        ));

        let summary = cleanup_stale_path_cache_entries_in_dir(temp.path());

        assert_eq!(
            summary,
            ImageThumbnailCacheCleanupSummary {
                removed_mappings: 1,
                removed_files: 1,
            }
        );
        assert!(thumbnail_file_path(Some(temp.path()), key).is_some_and(|path| !path.exists()));
        assert!(
            load_disk_manifest(Some(temp.path()))
                .unwrap()
                .mappings
                .is_empty()
        );
    }

    #[test]
    fn thumbnail_cleanup_keeps_manifest_entries_for_existing_sources() {
        let temp = TempDir::new();
        let source = temp.path().join("image.png");
        fs::write(&source, png_bytes(4, 2)).unwrap();
        let key = "0123456789abcdef";
        assert!(write_cached_thumbnail_with_source(
            Some(temp.path()),
            key,
            &source,
            &png_bytes(4, 2),
        ));

        let summary = cleanup_stale_path_cache_entries_in_dir(temp.path());

        assert_eq!(summary, ImageThumbnailCacheCleanupSummary::default());
        assert!(thumbnail_file_path(Some(temp.path()), key).is_some_and(|path| path.exists()));
        assert_eq!(
            load_disk_manifest(Some(temp.path()))
                .unwrap()
                .mappings
                .get(key),
            Some(&source)
        );
    }

    #[test]
    fn thumbnail_cleanup_leaves_unmapped_historical_files() {
        let temp = TempDir::new();
        let key = "0123456789abcdef";
        assert!(write_cached_thumbnail(
            Some(temp.path()),
            key,
            &qoi_bytes(4, 2),
        ));

        let summary = cleanup_stale_path_cache_entries_in_dir(temp.path());

        assert_eq!(summary, ImageThumbnailCacheCleanupSummary::default());
        assert!(thumbnail_file_path(Some(temp.path()), key).is_some_and(|path| path.exists()));
    }

    #[test]
    fn scheduled_cleanup_removes_legacy_thumbnail_cache_versions() {
        let temp = TempDir::new();
        let current = temp.path().join(IMAGE_THUMBNAIL_CACHE_VERSION);
        let legacy = temp.path().join("image-thumbnails-v1");
        fs::create_dir_all(&current).unwrap();
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("old.png"), b"old cache").unwrap();

        cleanup_legacy_image_thumbnail_cache_dirs(&current);

        assert!(current.is_dir());
        assert!(!legacy.exists());
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
    fn hover_preview_is_dequeued_before_older_standard_thumbnails() {
        let mut cache = ImageThumbnailCacheInner::new(None);
        let first = request("first", "folder");
        let second = request("second", "folder");
        push_pending(&mut cache, first);
        push_pending(&mut cache, second);

        let hover = ImageThumbnailRequest {
            usage: ImageThumbnailUsage::HoverPreview,
            source_policy: ThumbnailSourcePolicy::ReadSource,
            key: "hover".to_owned(),
            ..request("hover-source", "folder")
        };
        let standard = ImageThumbnailRequest {
            usage: ImageThumbnailUsage::Standard,
            source_policy: ThumbnailSourcePolicy::ReadSource,
            key: "hover-standard".to_owned(),
            ..hover.clone()
        };
        let (_, generation) = cache.hover_preview_for_request(hover.clone(), standard);
        let generation = generation.expect("loader generation");

        assert_eq!(
            cache.next_load_job(generation).unwrap().request.key,
            hover.key
        );
    }

    #[test]
    fn thumbnail_loader_concurrency_is_capped_at_four() {
        assert!((2..=4).contains(&image_thumbnail_loader_concurrency()));
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
            ImageThumbnailState::Ready(
                cached_thumbnail_image_from_png_bytes(one_pixel_png_bytes()).unwrap(),
            ),
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
    fn cancel_standard_thumbnail_requests_preserves_hover_preview_requests() {
        let mut cache = ImageThumbnailCacheInner::new(None);
        let standard = ImageThumbnailRequest {
            usage: ImageThumbnailUsage::Standard,
            ..request("standard", "old")
        };
        let hover = ImageThumbnailRequest {
            usage: ImageThumbnailUsage::HoverPreview,
            ..request("hover", "old")
        };
        push_pending(&mut cache, standard.clone());
        push_pending(&mut cache, hover.clone());

        let generation = cache.cancel_standard_thumbnail_requests(Path::new("old"));

        assert!(generation.is_some());
        assert_eq!(cache.pending.iter().collect::<Vec<_>>(), vec![&hover.key]);
        assert!(!cache.states.contains_key(&standard.key));
        assert!(matches!(
            cache.states.get(&hover.key),
            Some(ImageThumbnailState::Pending { request, .. })
                if request.usage == ImageThumbnailUsage::HoverPreview
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

    fn request(key: &str, directory: &str) -> ImageThumbnailRequest {
        ImageThumbnailRequest {
            kind: ImageThumbnailKind::Image,
            usage: ImageThumbnailUsage::Standard,
            source_policy: ThumbnailSourcePolicy::ReadSource,
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
                preview_dimensions: None,
                loading_thumbnail: None,
            },
        );
    }

    fn one_pixel_png_bytes() -> Vec<u8> {
        png_bytes(1, 1)
    }

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(width, height));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn qoi_bytes(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::new(width, height);
        encode_rgba_qoi_bytes(image.as_raw(), width, height).unwrap()
    }

    fn animated_gif_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            encoder
                .set_repeat(image::codecs::gif::Repeat::Infinite)
                .unwrap();
            for rgba in [[220, 40, 80, 255], [40, 140, 220, 255]] {
                encoder
                    .encode_frame(image::Frame::from_parts(
                        image::RgbaImage::from_pixel(width, height, image::Rgba(rgba)),
                        0,
                        0,
                        image::Delay::from_numer_denom_ms(80, 1),
                    ))
                    .unwrap();
            }
        }
        bytes
    }
}
