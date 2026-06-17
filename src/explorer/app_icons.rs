use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use futures::AsyncReadExt;
use gpui::{App, Context, Global, Image};
use serde::{Deserialize, Serialize};

use crate::{
    explorer::{entry::FileEntry, view::ExplorerView},
    settings::{APP_ID, ConfigPlatform, config_dir_for},
};

const NATIVE_ICON_LOAD_INTERVAL: Duration = Duration::from_millis(16);
const URL_ICON_RETRY_INTERVAL: Duration = Duration::from_secs(30);
const NATIVE_ICON_CACHE_VERSION: &str = "native-icons-v3";
const URL_ICON_CACHE_VERSION: &str = "url-icons-v1";
const DISK_MANIFEST_FILE_NAME: &str = "mappings.json";
const DISK_ICON_DIR_NAME: &str = "icons";
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const NATIVE_ICON_NORMALIZED_PNG_SIZE: u32 = 128;
const NATIVE_ICON_NORMALIZED_CONTENT_SIZE: u32 = 112;
const NATIVE_ICON_UNDERSIZED_RATIO: f32 = 0.75;
const NATIVE_ICON_CENTER_TOLERANCE_RATIO: f32 = 0.08;
#[cfg(target_os = "macos")]
const APP_ICON_PNG_SIZE: f64 = 128.0;
#[cfg(target_os = "windows")]
const WINDOWS_ICON_PNG_SIZE: i32 = 128;
#[cfg(target_os = "linux")]
const LINUX_ICON_PNG_SIZE: u32 = 128;

pub(super) struct NativeIconCache {
    inner: RefCell<NativeIconCacheInner>,
}

impl Global for NativeIconCache {}

pub(super) struct UrlIconCache {
    inner: RefCell<UrlIconCacheInner>,
}

impl Global for UrlIconCache {}

impl NativeIconCache {
    fn new() -> Self {
        Self {
            inner: RefCell::new(NativeIconCacheInner::new(DiskIconStore::load(
                native_icon_cache_dir(),
            ))),
        }
    }
}

impl UrlIconCache {
    fn new() -> Self {
        Self {
            inner: RefCell::new(UrlIconCacheInner::new(url_icon_cache_dir())),
        }
    }
}

pub(crate) fn initialize(cx: &mut App) {
    cx.set_global(NativeIconCache::new());
    cx.set_global(UrlIconCache::new());
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeIconRequest {
    key: String,
    source: PlatformIconRequest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PlatformIconRequest {
    #[cfg(any(target_os = "linux", test))]
    Linux(LinuxIconRequest),
    #[cfg(any(target_os = "windows", test))]
    Windows(WindowsIconRequest),
    #[cfg(any(target_os = "macos", test))]
    Mac(MacIconRequest),
    #[cfg(test)]
    Test,
}

#[cfg(any(target_os = "linux", test))]
#[cfg_attr(test, allow(dead_code))]
#[derive(Clone, Debug, Eq, PartialEq)]
enum LinuxIconRequest {
    Path { path: PathBuf },
}

#[cfg(any(target_os = "macos", test))]
#[cfg_attr(test, allow(dead_code))]
#[derive(Clone, Debug, Eq, PartialEq)]
enum MacIconRequest {
    AppBundle { path: PathBuf },
    FileType { extension: String },
    Path { path: PathBuf },
}

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Debug, Eq, PartialEq)]
enum WindowsIconRequest {
    Extension { extension: String },
    Path { path: PathBuf },
}

struct NativeIconCacheInner {
    states: HashMap<String, NativeIconState>,
    pending: VecDeque<String>,
    loader_running: bool,
    store: DiskIconStore,
}

enum NativeIconState {
    Pending {
        request: NativeIconRequest,
        queued_at: Instant,
    },
    Loading {
        icon: Option<Arc<Image>>,
    },
    Ready(Arc<Image>),
    Failed(Option<Arc<Image>>),
}

struct NativeIconLoadJob {
    request: NativeIconRequest,
    queued_at: Instant,
    cache_dir: Option<PathBuf>,
    stale_hash: Option<String>,
}

struct UrlIconCacheInner {
    cache_dir: Option<PathBuf>,
    states: HashMap<String, UrlIconState>,
    pending: VecDeque<String>,
    loader_running: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum UrlIconState {
    Pending,
    Ready(PathBuf),
    Failed { retry_after: Instant },
}

struct UrlIconLoadJob {
    url: String,
    path: PathBuf,
}

impl NativeIconCacheInner {
    fn new(store: DiskIconStore) -> Self {
        Self {
            states: HashMap::new(),
            pending: VecDeque::new(),
            loader_running: false,
            store,
        }
    }

    fn icon_for_request(&mut self, request: NativeIconRequest) -> (Option<Arc<Image>>, bool) {
        if let Some(state) = self.states.get(&request.key) {
            return (state.icon(), false);
        }

        self.pending.push_back(request.key.clone());
        self.states.insert(
            request.key.clone(),
            NativeIconState::Pending {
                request: request.clone(),
                queued_at: Instant::now(),
            },
        );

        (None, self.start_loader())
    }

    fn start_loader(&mut self) -> bool {
        if self.loader_running || self.pending.is_empty() {
            return false;
        }

        self.loader_running = true;
        true
    }

    fn next_load_job(&mut self) -> Option<NativeIconLoadJob> {
        while let Some(key) = self.pending.pop_front() {
            let Some(NativeIconState::Pending { request, queued_at }) = self.states.remove(&key)
            else {
                continue;
            };

            let stale_hash = self.store.icon_hash(&request.key).map(ToOwned::to_owned);
            let cache_dir = self.store.cache_dir().map(Path::to_path_buf);
            self.states
                .insert(key, NativeIconState::Loading { icon: None });

            return Some(NativeIconLoadJob {
                request,
                queued_at,
                cache_dir,
                stale_hash,
            });
        }

        self.loader_running = false;
        None
    }

    fn publish_stale_icon(&mut self, key: &str, bytes: Vec<u8>) -> bool {
        let Some(icon) = valid_png_bytes(bytes).map(image_from_png_bytes) else {
            return false;
        };
        let Some(NativeIconState::Loading {
            icon: current_icon, ..
        }) = self.states.get_mut(key)
        else {
            return false;
        };

        if current_icon.is_some() {
            return false;
        }

        *current_icon = Some(icon);
        true
    }

    fn finish_request(&mut self, request: NativeIconRequest, bytes: Option<Vec<u8>>) -> bool {
        let stale_icon = match self.states.remove(&request.key) {
            Some(NativeIconState::Loading { icon, .. }) => icon,
            Some(NativeIconState::Ready(icon)) => Some(icon),
            Some(NativeIconState::Failed(icon)) => icon,
            Some(NativeIconState::Pending { .. }) | None => None,
        };

        let state = match bytes.and_then(normalize_native_icon_png_bytes) {
            Some(bytes) => {
                self.store.write_mapping(&request.key, &bytes);
                NativeIconState::Ready(image_from_png_bytes(bytes))
            }
            None => NativeIconState::Failed(stale_icon),
        };

        self.states.insert(request.key, state);
        true
    }
}

impl UrlIconCacheInner {
    fn new(cache_dir: Option<PathBuf>) -> Self {
        Self {
            cache_dir,
            states: HashMap::new(),
            pending: VecDeque::new(),
            loader_running: false,
        }
    }

    fn icon_path_for_url(&mut self, url: &str) -> (Option<PathBuf>, bool) {
        match self.states.get(url) {
            Some(UrlIconState::Ready(path)) if path.is_file() => {
                return (Some(path.clone()), false);
            }
            Some(UrlIconState::Pending) => return (None, false),
            Some(UrlIconState::Failed { retry_after }) if Instant::now() < *retry_after => {
                return (None, false);
            }
            Some(UrlIconState::Ready(_)) | None => {}
            Some(UrlIconState::Failed { .. }) => {}
        }

        if let Some(path) = existing_url_icon_file_path(self.cache_dir.as_deref(), url) {
            self.states
                .insert(url.to_owned(), UrlIconState::Ready(path.clone()));
            return (Some(path), false);
        }

        if preferred_url_icon_file_path(self.cache_dir.as_deref(), url).is_none() {
            return (None, false);
        }

        self.states.insert(url.to_owned(), UrlIconState::Pending);
        self.pending.push_back(url.to_owned());
        let should_start_loader = !self.loader_running;
        self.loader_running = true;
        (None, should_start_loader)
    }

    fn next_load_job(&mut self) -> Option<UrlIconLoadJob> {
        loop {
            let url = self.pending.pop_front()?;
            let Some(path) = preferred_url_icon_file_path(self.cache_dir.as_deref(), &url) else {
                self.states.insert(
                    url,
                    UrlIconState::Failed {
                        retry_after: Instant::now() + URL_ICON_RETRY_INTERVAL,
                    },
                );
                continue;
            };
            return Some(UrlIconLoadJob { url, path });
        }
    }

    fn finish_request(&mut self, url: String, path: Option<PathBuf>) {
        let state = path
            .map(UrlIconState::Ready)
            .unwrap_or_else(|| UrlIconState::Failed {
                retry_after: Instant::now() + URL_ICON_RETRY_INTERVAL,
            });
        self.states.insert(url, state);

        if self.pending.is_empty() {
            self.loader_running = false;
        }
    }
}

impl NativeIconState {
    fn icon(&self) -> Option<Arc<Image>> {
        match self {
            Self::Ready(icon) => Some(icon.clone()),
            Self::Loading {
                icon: Some(icon), ..
            } => Some(icon.clone()),
            Self::Failed(Some(icon)) => Some(icon.clone()),
            Self::Pending { .. } | Self::Loading { icon: None, .. } | Self::Failed(None) => None,
        }
    }
}

impl ExplorerView {
    pub(super) fn observe_icon_caches(&mut self, cx: &mut Context<Self>) {
        cx.observe_global::<NativeIconCache>(|_, cx| cx.notify())
            .detach();
        cx.observe_global::<UrlIconCache>(|_, cx| cx.notify())
            .detach();
    }

    pub(super) fn native_icon_for_entry(
        &mut self,
        entry: &FileEntry,
        cx: &mut Context<Self>,
    ) -> Option<Arc<Image>> {
        self.native_icon_for_request(native_icon_request_for_entry(entry), cx)
    }

    pub(super) fn native_icon_for_path(
        &mut self,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> Option<Arc<Image>> {
        self.native_icon_for_request(native_icon_request_for_path(path), cx)
    }

    pub(super) fn cached_url_icon_path(
        &mut self,
        url: &str,
        cx: &mut Context<Self>,
    ) -> Option<PathBuf> {
        let (path, should_start_loader) = cx
            .try_global::<UrlIconCache>()
            .map(|cache| cache.inner.borrow_mut().icon_path_for_url(url))
            .unwrap_or((None, false));

        if should_start_loader {
            start_url_icon_loader(cx);
        }

        path
    }

    fn native_icon_for_request(
        &mut self,
        request: Option<NativeIconRequest>,
        cx: &mut Context<Self>,
    ) -> Option<Arc<Image>> {
        if !self.resolve_icons {
            return None;
        }

        let Some(request) = request else {
            return None;
        };

        let (icon, should_start_loader) = cx
            .try_global::<NativeIconCache>()
            .map(|cache| cache.inner.borrow_mut().icon_for_request(request))
            .unwrap_or((None, false));

        if should_start_loader {
            start_native_icon_loader(cx);
        }

        icon
    }
}

fn start_native_icon_loader(cx: &mut Context<ExplorerView>) {
    cx.spawn(async move |_, cx| {
        let mut timings = IconTimingBatch::start();

        loop {
            let job = cx
                .update(|cx| {
                    cx.try_global::<NativeIconCache>()
                        .and_then(|cache| cache.inner.borrow_mut().next_load_job())
                })
                .ok()
                .flatten();
            let Some(job) = job else {
                break;
            };

            let request_started = timings.now();
            timings.record_request();
            timings.record_queue_wait(job.queued_at.elapsed());

            if let Some(stale_hash) = job.stale_hash.clone() {
                let cache_dir = job.cache_dir.clone();
                let key = job.request.key.clone();
                let stale_read_started = timings.now();
                let stale_task = cx.background_executor().spawn(async move {
                    read_cached_icon_by_hash(cache_dir.as_deref(), &stale_hash)
                });

                let stale_bytes = stale_task.await;
                let stale_hit = stale_bytes.is_some();
                timings.record_stale_disk_read(stale_read_started, stale_hit);

                if let Some(bytes) = stale_bytes {
                    let stale_publish_started = timings.now();
                    let published = cx
                        .update_global::<NativeIconCache, _>(|cache, _| {
                            cache.inner.borrow_mut().publish_stale_icon(&key, bytes)
                        })
                        .ok()
                        .unwrap_or(false);
                    timings.record_stale_publish(stale_publish_started, published);
                }
            }

            let request = job.request.clone();
            let platform_extract_started = timings.now();
            let load_task = cx
                .background_executor()
                .spawn(async move { load_platform_icon_png_bytes(&request) });
            let icon = load_task.await;
            let fresh_ok = icon.is_some();
            timings.record_platform_extract(platform_extract_started, fresh_ok);

            let fresh_commit_started = timings.now();
            let _committed = cx.update_global::<NativeIconCache, _>(|cache, _| {
                cache.inner.borrow_mut().finish_request(job.request, icon);
            });
            timings.record_fresh_commit(fresh_commit_started);
            timings.record_request_total(request_started);

            cx.background_executor()
                .timer(NATIVE_ICON_LOAD_INTERVAL)
                .await;
        }

        timings.finish();
    })
    .detach();
}

fn start_url_icon_loader(cx: &mut Context<ExplorerView>) {
    let client = cx.http_client();
    cx.spawn(async move |view, cx| {
        loop {
            let job = cx
                .update(|cx| {
                    cx.try_global::<UrlIconCache>()
                        .and_then(|cache| cache.inner.borrow_mut().next_load_job())
                })
                .ok()
                .flatten();
            let Some(job) = job else {
                break;
            };

            let url = job.url.clone();
            let path = job.path.clone();
            let client = client.clone();
            let load_task = cx
                .background_executor()
                .spawn(async move { download_url_icon_to_path(client, &url, &path).await });
            let loaded_path = load_task.await;

            let _committed = cx.update_global::<UrlIconCache, _>(|cache, _| {
                cache
                    .inner
                    .borrow_mut()
                    .finish_request(job.url, loaded_path);
            });
            let _ = view.update(cx, |_, cx| cx.notify());

            cx.background_executor()
                .timer(NATIVE_ICON_LOAD_INTERVAL)
                .await;
        }
    })
    .detach();
}

async fn download_url_icon_to_path(
    client: Arc<dyn gpui::http_client::HttpClient>,
    url: &str,
    path: &Path,
) -> Option<PathBuf> {
    let mut response = match client.get(url, ().into(), true).await {
        Ok(response) => response,
        Err(error) => {
            eprintln!("failed to download context menu URL icon {url}: {error}");
            return None;
        }
    };
    let content_type_extension = response
        .headers()
        .get(gpui::http_client::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(url_icon_extension_for_content_type);
    let mut body = Vec::new();
    if let Err(error) = response.body_mut().read_to_end(&mut body).await {
        eprintln!("failed to read context menu URL icon {url}: {error}");
        return None;
    }
    if !response.status().is_success() {
        eprintln!(
            "failed to download context menu URL icon {url}: HTTP {}",
            response.status()
        );
        return None;
    }
    if body.is_empty() {
        eprintln!("failed to download context menu URL icon {url}: empty response body");
        return None;
    }

    let path = if path.extension().and_then(OsStr::to_str) == Some("img") {
        content_type_extension
            .map(|extension| path.with_extension(extension))
            .unwrap_or_else(|| path.to_path_buf())
    } else {
        path.to_path_buf()
    };

    if let Err(error) = write_atomic(&path, &body) {
        eprintln!(
            "failed to cache context menu URL icon {url} at {}: {error}",
            path.display()
        );
        return None;
    }

    Some(path)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct IconTimingBatch {
    enabled: bool,
    batch_started: Option<Instant>,
    requests: usize,
    stale_hits: usize,
    stale_misses: usize,
    stale_published: usize,
    fresh_ok: usize,
    failed: usize,
    queue_wait: IconStageTimingStats,
    stale_disk_read: IconStageTimingStats,
    stale_publish: IconStageTimingStats,
    platform_extract: IconStageTimingStats,
    fresh_commit: IconStageTimingStats,
    request_total: IconStageTimingStats,
}

impl IconTimingBatch {
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

    fn record_stale_disk_read(&mut self, started: Option<Instant>, hit: bool) {
        if !self.enabled {
            return;
        }

        if let Some(started) = started {
            self.stale_disk_read.record(started.elapsed());
        }
        if hit {
            self.stale_hits += 1;
        } else {
            self.stale_misses += 1;
        }
    }

    fn record_stale_publish(&mut self, started: Option<Instant>, published: bool) {
        if !self.enabled {
            return;
        }

        if let Some(started) = started {
            self.stale_publish.record(started.elapsed());
        }
        if published {
            self.stale_published += 1;
        }
    }

    fn record_platform_extract(&mut self, started: Option<Instant>, ok: bool) {
        if !self.enabled {
            return;
        }

        if let Some(started) = started {
            self.platform_extract.record(started.elapsed());
        }
        if ok {
            self.fresh_ok += 1;
        } else {
            self.failed += 1;
        }
    }

    fn record_fresh_commit(&mut self, started: Option<Instant>) {
        if !self.enabled {
            return;
        }

        if let Some(started) = started {
            self.fresh_commit.record(started.elapsed());
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
            "total={} requests={} stale_hits={} stale_misses={} stale_published={} fresh_ok={} failed={}",
            format_icon_timing_duration(batch_total),
            self.requests,
            self.stale_hits,
            self.stale_misses,
            self.stale_published,
            self.fresh_ok,
            self.failed
        )];
        push_icon_stage_line(&mut lines, "queue_wait", &self.queue_wait, "");
        push_icon_stage_line(
            &mut lines,
            "stale_disk_read",
            &self.stale_disk_read,
            &format!("hits={} misses={}", self.stale_hits, self.stale_misses),
        );
        push_icon_stage_line(
            &mut lines,
            "stale_publish",
            &self.stale_publish,
            &format!("published={}", self.stale_published),
        );
        push_icon_stage_line(
            &mut lines,
            "platform_extract",
            &self.platform_extract,
            &format!("ok={} failed={}", self.fresh_ok, self.failed),
        );
        push_icon_stage_line(&mut lines, "fresh_commit", &self.fresh_commit, "");
        push_icon_stage_line(&mut lines, "request_total", &self.request_total, "");
        lines
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct IconStageTimingStats {
    count: usize,
    total: Duration,
    fastest: Option<Duration>,
    slowest: Option<Duration>,
}

impl IconStageTimingStats {
    fn record(&mut self, elapsed: Duration) {
        self.count += 1;
        self.total += elapsed;
        self.fastest = Some(self.fastest.map_or(elapsed, |fastest| fastest.min(elapsed)));
        self.slowest = Some(self.slowest.map_or(elapsed, |slowest| slowest.max(elapsed)));
    }

    fn format_line(&self, stage: &str, extra: &str) -> Option<String> {
        if self.count == 0 {
            return None;
        }

        let mut line = format!(
            "{stage} count={} total={} fastest={} slowest={}",
            self.count,
            format_icon_timing_duration(self.total),
            format_icon_timing_duration(self.fastest.unwrap_or_default()),
            format_icon_timing_duration(self.slowest.unwrap_or_default())
        );
        if !extra.is_empty() {
            line.push(' ');
            line.push_str(extra);
        }
        Some(line)
    }
}

fn push_icon_stage_line(
    lines: &mut Vec<String>,
    stage: &str,
    stats: &IconStageTimingStats,
    extra: &str,
) {
    if let Some(line) = stats.format_line(stage, extra) {
        lines.push(line);
    }
}

fn format_icon_timing_duration(elapsed: Duration) -> String {
    format!("{:.3}ms", elapsed.as_secs_f64() * 1000.0)
}

fn native_icon_request_for_entry(entry: &FileEntry) -> Option<NativeIconRequest> {
    #[cfg(target_os = "macos")]
    {
        return mac_icon_request_for_entry(entry);
    }

    #[cfg(target_os = "windows")]
    {
        return windows_icon_request_for_entry(entry);
    }

    #[allow(unreachable_code)]
    None
}

fn native_icon_request_for_path(path: &Path) -> Option<NativeIconRequest> {
    #[cfg(target_os = "macos")]
    {
        return Some(mac_native_icon_request(MacIconRequest::Path {
            path: path.to_path_buf(),
        }));
    }

    #[cfg(target_os = "windows")]
    {
        return Some(windows_native_icon_request(WindowsIconRequest::Path {
            path: path.to_path_buf(),
        }));
    }

    #[cfg(target_os = "linux")]
    {
        return Some(linux_native_icon_request(LinuxIconRequest::Path {
            path: path.to_path_buf(),
        }));
    }

    #[allow(unreachable_code)]
    None
}

#[cfg(any(target_os = "linux", test))]
#[cfg_attr(test, allow(dead_code))]
fn linux_native_icon_request(request: LinuxIconRequest) -> NativeIconRequest {
    let key = match &request {
        LinuxIconRequest::Path { path } => {
            format!(
                "{NATIVE_ICON_CACHE_VERSION}:linux:path:{}",
                normalized_path_key(path)
            )
        }
    };

    NativeIconRequest {
        key,
        source: PlatformIconRequest::Linux(request),
    }
}

#[cfg(any(target_os = "macos", test))]
fn mac_icon_request_for_entry(entry: &FileEntry) -> Option<NativeIconRequest> {
    if entry.uses_app_bundle_icon() {
        return Some(mac_native_icon_request(MacIconRequest::AppBundle {
            path: entry.path.clone(),
        }));
    }

    if entry.is_directory_like() {
        return None;
    }

    Some(mac_native_icon_request(MacIconRequest::FileType {
        extension: lowercase_extension(&entry.path).unwrap_or_default(),
    }))
}

#[cfg(any(target_os = "macos", test))]
fn mac_native_icon_request(request: MacIconRequest) -> NativeIconRequest {
    let key = match &request {
        MacIconRequest::AppBundle { path } => {
            format!(
                "{NATIVE_ICON_CACHE_VERSION}:macos:app:{}",
                normalized_path_key(path)
            )
        }
        MacIconRequest::FileType { extension, .. } => {
            format!("{NATIVE_ICON_CACHE_VERSION}:macos:file-type:{extension}")
        }
        MacIconRequest::Path { path } => {
            format!(
                "{NATIVE_ICON_CACHE_VERSION}:macos:path:{}",
                normalized_path_key(path)
            )
        }
    };

    NativeIconRequest {
        key,
        source: PlatformIconRequest::Mac(request),
    }
}

#[cfg(any(target_os = "windows", test))]
fn windows_icon_request_for_entry(entry: &FileEntry) -> Option<NativeIconRequest> {
    if entry.is_directory_like() && !entry.uses_directory_shortcut_icon() {
        return None;
    }

    let request = if windows_entry_uses_path_icon(entry) {
        WindowsIconRequest::Path {
            path: entry.path.clone(),
        }
    } else {
        WindowsIconRequest::Extension {
            extension: lowercase_extension(&entry.path).unwrap_or_default(),
        }
    };

    Some(windows_native_icon_request(request))
}

#[cfg(any(target_os = "windows", test))]
fn windows_native_icon_request(request: WindowsIconRequest) -> NativeIconRequest {
    let key = match &request {
        WindowsIconRequest::Extension { extension } => {
            format!("{NATIVE_ICON_CACHE_VERSION}:windows:extension:{extension}")
        }
        WindowsIconRequest::Path { path } => {
            format!(
                "{NATIVE_ICON_CACHE_VERSION}:windows:path:{}",
                normalized_path_key(path)
            )
        }
    };

    NativeIconRequest {
        key,
        source: PlatformIconRequest::Windows(request),
    }
}

#[cfg(any(target_os = "windows", test))]
fn windows_entry_uses_path_icon(entry: &FileEntry) -> bool {
    use crate::explorer::entry::{DirectoryLinkKind, EntryKind};

    if matches!(
        entry.kind,
        EntryKind::DirectoryLink(DirectoryLinkKind::FilesystemLink)
    ) {
        return true;
    }

    let Some(extension) = lowercase_extension(&entry.path) else {
        return false;
    };

    matches!(
        extension.as_str(),
        "exe"
            | "com"
            | "scr"
            | "cpl"
            | "dll"
            | "ico"
            | "lnk"
            | "url"
            | "msi"
            | "msix"
            | "msixbundle"
            | "appx"
            | "appxbundle"
    )
}

fn lowercase_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(OsStr::to_str)
        .map(str::trim)
        .filter(|extension| !extension.is_empty())
        .map(str::to_ascii_lowercase)
}

fn normalized_path_key(path: &Path) -> String {
    let key = path.to_string_lossy().replace('\\', "/");
    if cfg!(target_os = "windows") {
        key.to_ascii_lowercase()
    } else {
        key
    }
}

fn load_platform_icon_png_bytes(request: &NativeIconRequest) -> Option<Vec<u8>> {
    match &request.source {
        #[cfg(target_os = "linux")]
        PlatformIconRequest::Linux(request) => load_linux_icon_png_bytes(request),
        #[cfg(all(test, not(target_os = "linux")))]
        PlatformIconRequest::Linux(_) => None,
        #[cfg(target_os = "macos")]
        PlatformIconRequest::Mac(request) => load_macos_icon_png_bytes(request),
        #[cfg(all(test, not(target_os = "macos")))]
        PlatformIconRequest::Mac(_) => None,
        #[cfg(target_os = "windows")]
        PlatformIconRequest::Windows(request) => load_windows_shell_icon_png_bytes(request),
        #[cfg(all(test, not(target_os = "windows")))]
        PlatformIconRequest::Windows(_) => None,
        #[cfg(test)]
        PlatformIconRequest::Test => None,
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn load_linux_icon_png_bytes(request: &LinuxIconRequest) -> Option<Vec<u8>> {
    use freedesktop_desktop_entry::IconSource;

    let LinuxIconRequest::Path { path } = request;
    let entries =
        std::panic::catch_unwind(|| freedesktop_desktop_entry::desktop_entries(&[])).ok()?;
    let icon = linux_icon_source_for_executable(path, &entries)?;
    let icon_path = match icon {
        IconSource::Path(path) => path,
        IconSource::Name(name) => {
            let theme = freedesktop_icons::default_theme_gtk();
            let lookup = freedesktop_icons::lookup(&name)
                .with_size(LINUX_ICON_PNG_SIZE as u16)
                .with_cache();
            match theme {
                Some(theme) => lookup.with_theme(&theme).find(),
                None => lookup.find(),
            }?
        }
    };

    load_linux_icon_path_as_png(&icon_path)
}

#[cfg(target_os = "linux")]
fn linux_icon_source_for_executable(
    executable: &Path,
    entries: &[freedesktop_desktop_entry::DesktopEntry],
) -> Option<freedesktop_desktop_entry::IconSource> {
    use freedesktop_desktop_entry::IconSource;

    let executable_name = executable.file_name();
    let entry = entries
        .iter()
        .filter_map(|entry| {
            let command = entry
                .parse_exec()
                .ok()
                .and_then(|arguments| arguments.into_iter().next())
                .or_else(|| entry.try_exec().map(str::to_owned))?;
            Some((entry, PathBuf::from(command)))
        })
        .find(|(_, command)| command == executable)
        .or_else(|| {
            entries
                .iter()
                .filter_map(|entry| {
                    let command = entry
                        .parse_exec()
                        .ok()
                        .and_then(|arguments| arguments.into_iter().next())
                        .or_else(|| entry.try_exec().map(str::to_owned))?;
                    let command = PathBuf::from(command);
                    (command.file_name() == executable_name).then_some((entry, command))
                })
                .next()
        })
        .map(|(entry, _)| entry)?;

    Some(IconSource::from_unknown(entry.icon()?))
}

#[cfg(target_os = "linux")]
fn load_linux_icon_path_as_png(path: &Path) -> Option<Vec<u8>> {
    let bytes = fs::read(path).ok()?;
    if path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
    {
        return linux_svg_to_png(&bytes);
    }
    valid_png_bytes(bytes)
}

#[cfg(target_os = "linux")]
fn linux_svg_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    use image::ImageEncoder;

    let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).ok()?;
    let size = tree.size();
    let target_size = LINUX_ICON_PNG_SIZE as f32;
    let scale = (target_size / size.width()).min(target_size / size.height());
    let mut pixmap = resvg::tiny_skia::Pixmap::new(LINUX_ICON_PNG_SIZE, LINUX_ICON_PNG_SIZE)?;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            pixmap.data(),
            pixmap.width(),
            pixmap.height(),
            image::ExtendedColorType::Rgba8,
        )
        .ok()?;
    valid_png_bytes(png)
}

#[cfg(target_os = "windows")]
fn load_windows_shell_icon_png_bytes(request: &WindowsIconRequest) -> Option<Vec<u8>> {
    load_windows_image_list_icon_png_bytes(request)
        .or_else(|| load_windows_file_info_icon_png_bytes(request))
}

#[cfg(target_os = "windows")]
fn load_windows_image_list_icon_png_bytes(request: &WindowsIconRequest) -> Option<Vec<u8>> {
    use std::{mem, os::windows::ffi::OsStrExt};
    use windows::{
        Win32::{
            Storage::FileSystem::{FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES},
            UI::{
                Controls::{IImageList, ILD_TRANSPARENT},
                Shell::{
                    SHFILEINFOW, SHGFI_ADDOVERLAYS, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES,
                    SHGetFileInfoW, SHGetImageList, SHIL_EXTRALARGE, SHIL_JUMBO,
                },
                WindowsAndMessaging::DestroyIcon,
            },
        },
        core::PCWSTR,
    };

    let (path, use_file_attributes) = match request {
        WindowsIconRequest::Extension { extension } => {
            (PathBuf::from(windows_extension_probe_name(extension)), true)
        }
        WindowsIconRequest::Path { path } => (path.clone(), false),
    };
    let wide_path = path
        .as_os_str()
        .encode_wide()
        .map(|unit| {
            if !use_file_attributes && unit == b'/' as u16 {
                b'\\' as u16
            } else {
                unit
            }
        })
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let attributes = if use_file_attributes {
        FILE_ATTRIBUTE_NORMAL
    } else {
        FILE_FLAGS_AND_ATTRIBUTES(0)
    };
    let mut flags = SHGFI_SYSICONINDEX | SHGFI_ADDOVERLAYS;
    if use_file_attributes {
        flags |= SHGFI_USEFILEATTRIBUTES;
    }

    let mut info = SHFILEINFOW::default();
    let result = unsafe {
        SHGetFileInfoW(
            PCWSTR::from_raw(wide_path.as_ptr()),
            attributes,
            Some(&mut info),
            mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };

    if result == 0 {
        return None;
    }

    for image_list_size in [SHIL_JUMBO, SHIL_EXTRALARGE] {
        let Some(image_list) = unsafe { SHGetImageList::<IImageList>(image_list_size as i32) }.ok()
        else {
            continue;
        };
        let Some(icon) = unsafe { image_list.GetIcon(info.iIcon, ILD_TRANSPARENT.0) }.ok() else {
            continue;
        };
        if icon.is_invalid() {
            continue;
        }

        let bytes = unsafe { hicon_to_png_bytes(icon, WINDOWS_ICON_PNG_SIZE) };
        let _ = unsafe { DestroyIcon(icon) };
        if bytes.is_some() {
            return bytes;
        }
    }

    None
}

#[cfg(target_os = "windows")]
fn load_windows_file_info_icon_png_bytes(request: &WindowsIconRequest) -> Option<Vec<u8>> {
    use std::{mem, os::windows::ffi::OsStrExt};
    use windows::{
        Win32::{
            Storage::FileSystem::{FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES},
            UI::{
                Shell::{
                    SHFILEINFOW, SHGFI_ADDOVERLAYS, SHGFI_ICON, SHGFI_LARGEICON,
                    SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW,
                },
                WindowsAndMessaging::DestroyIcon,
            },
        },
        core::PCWSTR,
    };

    let (path, use_file_attributes) = match request {
        WindowsIconRequest::Extension { extension } => {
            (PathBuf::from(windows_extension_probe_name(extension)), true)
        }
        WindowsIconRequest::Path { path } => (path.clone(), false),
    };
    let wide_path = path
        .as_os_str()
        .encode_wide()
        .map(|unit| {
            if !use_file_attributes && unit == b'/' as u16 {
                b'\\' as u16
            } else {
                unit
            }
        })
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let attributes = if use_file_attributes {
        FILE_ATTRIBUTE_NORMAL
    } else {
        FILE_FLAGS_AND_ATTRIBUTES(0)
    };
    let mut flags = SHGFI_ICON | SHGFI_LARGEICON | SHGFI_ADDOVERLAYS;
    if use_file_attributes {
        flags |= SHGFI_USEFILEATTRIBUTES;
    }

    let mut info = SHFILEINFOW::default();
    let result = unsafe {
        SHGetFileInfoW(
            PCWSTR::from_raw(wide_path.as_ptr()),
            attributes,
            Some(&mut info),
            mem::size_of::<SHFILEINFOW>() as u32,
            flags,
        )
    };

    if result == 0 || info.hIcon.is_invalid() {
        return None;
    }

    let bytes = unsafe { hicon_to_png_bytes(info.hIcon, WINDOWS_ICON_PNG_SIZE) };
    let _ = unsafe { DestroyIcon(info.hIcon) };
    bytes
}

#[cfg(target_os = "windows")]
fn windows_extension_probe_name(extension: &str) -> String {
    if extension.is_empty() {
        "file".to_owned()
    } else {
        format!("file.{extension}")
    }
}

#[cfg(target_os = "windows")]
unsafe fn hicon_to_png_bytes(
    hicon: windows::Win32::UI::WindowsAndMessaging::HICON,
    size: i32,
) -> Option<Vec<u8>> {
    let transparent = unsafe { draw_hicon_to_bgra(hicon, size, [0, 0, 0, 0])? };

    if transparent.chunks_exact(4).any(|pixel| pixel[3] != 0) {
        return bgra_to_png_bytes(transparent, size as u32, size as u32);
    }

    let black = unsafe { draw_hicon_to_bgra(hicon, size, [0, 0, 0, 255])? };
    let white = unsafe { draw_hicon_to_bgra(hicon, size, [255, 255, 255, 255])? };
    inferred_alpha_png_bytes(&black, &white, size as u32, size as u32)
}

#[cfg(target_os = "windows")]
unsafe fn draw_hicon_to_bgra(
    hicon: windows::Win32::UI::WindowsAndMessaging::HICON,
    size: i32,
    background: [u8; 4],
) -> Option<Vec<u8>> {
    use std::{ffi::c_void, mem, ptr, slice};
    use windows::Win32::{
        Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection,
            DIB_RGB_COLORS, DeleteDC, DeleteObject, HGDIOBJ, SelectObject,
        },
        UI::WindowsAndMessaging::{DI_NORMAL, DrawIconEx},
    };

    let hdc = unsafe { CreateCompatibleDC(None) };
    if hdc.is_invalid() {
        return None;
    }

    let mut bits = ptr::null_mut::<c_void>();
    let mut info = BITMAPINFO::default();
    info.bmiHeader.biSize = mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = size;
    info.bmiHeader.biHeight = -size;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB.0;

    let bitmap =
        match unsafe { CreateDIBSection(Some(hdc), &info, DIB_RGB_COLORS, &mut bits, None, 0) } {
            Ok(bitmap) if !bits.is_null() => bitmap,
            _ => {
                let _ = unsafe { DeleteDC(hdc) };
                return None;
            }
        };

    let len = (size as usize) * (size as usize) * 4;
    let pixels = unsafe { slice::from_raw_parts_mut(bits.cast::<u8>(), len) };
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&background);
    }

    let old_object = unsafe { SelectObject(hdc, HGDIOBJ::from(bitmap)) };
    let draw_result = unsafe { DrawIconEx(hdc, 0, 0, hicon, size, size, 0, None, DI_NORMAL) };
    let bytes = pixels.to_vec();

    if !old_object.is_invalid() {
        let _ = unsafe { SelectObject(hdc, old_object) };
    }
    let _ = unsafe { DeleteObject(HGDIOBJ::from(bitmap)) };
    let _ = unsafe { DeleteDC(hdc) };

    draw_result.ok()?;
    Some(bytes)
}

#[cfg(target_os = "windows")]
fn inferred_alpha_png_bytes(
    black: &[u8],
    white: &[u8],
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    let mut rgba = Vec::with_capacity(black.len());

    for (black, white) in black.chunks_exact(4).zip(white.chunks_exact(4)) {
        let b_delta = white[0].saturating_sub(black[0]);
        let g_delta = white[1].saturating_sub(black[1]);
        let r_delta = white[2].saturating_sub(black[2]);
        let alpha = 255u8.saturating_sub(r_delta.max(g_delta).max(b_delta));

        if alpha == 0 {
            rgba.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            rgba.push(((u16::from(black[2]) * 255) / u16::from(alpha)).min(255) as u8);
            rgba.push(((u16::from(black[1]) * 255) / u16::from(alpha)).min(255) as u8);
            rgba.push(((u16::from(black[0]) * 255) / u16::from(alpha)).min(255) as u8);
            rgba.push(alpha);
        }
    }

    rgba_to_png_bytes(rgba, width, height)
}

#[cfg(target_os = "windows")]
fn bgra_to_png_bytes(mut bgra: Vec<u8>, width: u32, height: u32) -> Option<Vec<u8>> {
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    rgba_to_png_bytes(bgra, width, height)
}

#[cfg(target_os = "windows")]
fn rgba_to_png_bytes(rgba: Vec<u8>, width: u32, height: u32) -> Option<Vec<u8>> {
    use image::ImageEncoder;

    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&rgba, width, height, image::ExtendedColorType::Rgba8)
        .ok()?;
    valid_png_bytes(bytes)
}

#[cfg(target_os = "macos")]
fn load_macos_icon_png_bytes(request: &MacIconRequest) -> Option<Vec<u8>> {
    match request {
        MacIconRequest::AppBundle { path } => load_app_bundle_icon_png_bytes(path),
        MacIconRequest::FileType { extension } => load_file_type_icon_png_bytes(extension),
        MacIconRequest::Path { path } => load_icon_from_workspace(path),
    }
}

#[cfg(target_os = "macos")]
fn load_app_bundle_icon_png_bytes(path: &Path) -> Option<Vec<u8>> {
    load_icon_from_workspace(path).or_else(|| {
        resolve_bundle_icon_path(path)
            .as_deref()
            .and_then(load_icon_from_icns)
    })
}

#[cfg(target_os = "macos")]
fn load_file_type_icon_png_bytes(extension: &str) -> Option<Vec<u8>> {
    load_icon_from_file_type(extension)
}

#[cfg(target_os = "macos")]
fn resolve_bundle_icon_path(app_path: &Path) -> Option<PathBuf> {
    let info_path = app_path.join("Contents").join("Info.plist");
    let icon_name = bundle_icon_name_from_plist(&info_path)?;
    bundle_icon_resource_path(app_path, &icon_name)
}

#[cfg(target_os = "macos")]
fn bundle_icon_name_from_plist(info_path: &Path) -> Option<String> {
    let plist = plist::Value::from_file(info_path).ok()?;
    plist
        .as_dictionary()?
        .get("CFBundleIconFile")?
        .as_string()
        .and_then(non_empty_icon_name)
}

#[cfg(any(target_os = "macos", test))]
fn non_empty_icon_name(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(any(target_os = "macos", test))]
fn bundle_icon_resource_path(app_path: &Path, icon_name: &str) -> Option<PathBuf> {
    let icon_name = non_empty_icon_name(icon_name)?;
    let mut icon_path = PathBuf::from(icon_name);
    if icon_path.extension().is_none() {
        icon_path.set_extension("icns");
    }

    Some(app_path.join("Contents").join("Resources").join(icon_path))
}

#[cfg(target_os = "macos")]
fn load_icon_from_icns(icon_path: &Path) -> Option<Vec<u8>> {
    use icns::{IconFamily, IconType};

    let icon_family = IconFamily::read(fs::File::open(icon_path).ok()?).ok()?;
    let icon_type = preferred_icon_type(&icon_family.available_icons())?;
    let image = icon_family.get_icon_with_type(icon_type).ok()?;
    let mut bytes = Vec::new();
    image.write_png(&mut bytes).ok()?;
    valid_png_bytes(bytes)
}

#[cfg(target_os = "macos")]
fn preferred_icon_type(available_icons: &[icns::IconType]) -> Option<icns::IconType> {
    use icns::IconType;

    let preferred = [
        IconType::RGBA32_128x128,
        IconType::RGBA32_128x128_2x,
        IconType::RGBA32_256x256,
        IconType::RGBA32_256x256_2x,
        IconType::RGBA32_512x512,
        IconType::RGBA32_512x512_2x,
        IconType::RGB24_128x128,
        IconType::RGBA32_64x64,
        IconType::RGBA32_32x32_2x,
        IconType::RGBA32_32x32,
        IconType::RGB24_48x48,
        IconType::RGBA32_16x16_2x,
        IconType::RGBA32_16x16,
        IconType::RGB24_32x32,
        IconType::RGB24_16x16,
    ];

    preferred
        .into_iter()
        .find(|icon_type| available_icons.contains(icon_type))
        .or_else(|| {
            available_icons.iter().copied().min_by_key(|icon_type| {
                let width = icon_type.screen_width() as i64;
                ((width - APP_ICON_PNG_SIZE as i64).abs(), width)
            })
        })
}

#[cfg(target_os = "macos")]
fn load_icon_from_workspace(path: &Path) -> Option<Vec<u8>> {
    use cocoa::{
        appkit::NSImage,
        base::{id, nil},
        foundation::NSString,
    };
    use objc::{class, msg_send, sel, sel_impl};

    let path = path.to_str()?;

    unsafe {
        let pool: id = msg_send![class!(NSAutoreleasePool), new];
        let result = (|| {
            let ns_path = NSString::alloc(nil).init_str(path);
            let _: id = msg_send![ns_path, autorelease];

            let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
            if workspace == nil {
                return None;
            }

            let icon: id = msg_send![workspace, iconForFile: ns_path];
            if icon == nil {
                return None;
            }

            small_png_from_ns_image(icon)
        })();
        let _: () = msg_send![pool, drain];
        result
    }
}

#[cfg(target_os = "macos")]
fn load_icon_from_file_type(file_type: &str) -> Option<Vec<u8>> {
    use cocoa::{
        appkit::NSImage,
        base::{id, nil},
        foundation::NSString,
    };
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let pool: id = msg_send![class!(NSAutoreleasePool), new];
        let result = (|| {
            let ns_file_type = NSString::alloc(nil).init_str(file_type);
            let _: id = msg_send![ns_file_type, autorelease];

            let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
            if workspace == nil {
                return None;
            }

            let icon: id = msg_send![workspace, iconForFileType: ns_file_type];
            if icon == nil {
                return None;
            }

            small_png_from_ns_image(icon)
        })();
        let _: () = msg_send![pool, drain];
        result
    }
}

#[cfg(target_os = "macos")]
unsafe fn small_png_from_ns_image(icon: cocoa::base::id) -> Option<Vec<u8>> {
    use cocoa::{
        appkit::{NSCompositingOperation, NSImage},
        base::{NO, YES, id, nil},
        foundation::{NSData, NSPoint, NSRect, NSSize, NSString},
    };
    use objc::{class, msg_send, sel, sel_impl};

    let size = APP_ICON_PNG_SIZE as isize;
    let color_space = NSString::alloc(nil).init_str("NSDeviceRGBColorSpace");
    let _: id = msg_send![color_space, autorelease];

    let bitmap_rep: id = msg_send![class!(NSBitmapImageRep), alloc];
    let bitmap_rep: id = msg_send![
        bitmap_rep,
        initWithBitmapDataPlanes: nil
        pixelsWide: size
        pixelsHigh: size
        bitsPerSample: 8isize
        samplesPerPixel: 4isize
        hasAlpha: YES
        isPlanar: NO
        colorSpaceName: color_space
        bitmapFormat: 0usize
        bytesPerRow: 0isize
        bitsPerPixel: 0isize
    ];
    if bitmap_rep == nil {
        return None;
    }
    let _: id = msg_send![bitmap_rep, autorelease];

    let graphics_context: id =
        msg_send![class!(NSGraphicsContext), graphicsContextWithBitmapImageRep: bitmap_rep];
    if graphics_context == nil {
        return None;
    }

    let _: () = msg_send![class!(NSGraphicsContext), saveGraphicsState];
    let _: () = msg_send![class!(NSGraphicsContext), setCurrentContext: graphics_context];

    let destination = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(APP_ICON_PNG_SIZE, APP_ICON_PNG_SIZE),
    );
    let source = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
    icon.drawInRect_fromRect_operation_fraction_(
        destination,
        source,
        NSCompositingOperation::NSCompositeSourceOver,
        1.0,
    );

    let _: () = msg_send![class!(NSGraphicsContext), restoreGraphicsState];

    let png_file_type = 4usize;
    let data: id = msg_send![
        bitmap_rep,
        representationUsingType: png_file_type
        properties: nil
    ];
    if data == nil {
        return None;
    }

    let length = data.length();
    if length == 0 {
        return None;
    }

    let bytes = data.bytes().cast::<u8>();
    let bytes = std::slice::from_raw_parts(bytes, length as usize).to_vec();
    valid_png_bytes(bytes)
}

fn image_from_png_bytes(bytes: Vec<u8>) -> Arc<Image> {
    Arc::new(Image::from_bytes(gpui::ImageFormat::Png, bytes))
}

fn valid_png_bytes(bytes: Vec<u8>) -> Option<Vec<u8>> {
    bytes.starts_with(PNG_SIGNATURE).then_some(bytes)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AlphaBounds {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl AlphaBounds {
    fn right(self) -> u32 {
        self.x + self.width
    }

    fn bottom(self) -> u32 {
        self.y + self.height
    }
}

fn normalize_native_icon_png_bytes(bytes: Vec<u8>) -> Option<Vec<u8>> {
    let bytes = valid_png_bytes(bytes)?;
    let image = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let Some(bounds) = alpha_bounds(&image) else {
        return None;
    };
    let (width, height) = image.dimensions();
    if !native_icon_needs_normalization(width, height, bounds) {
        return Some(bytes);
    }

    normalized_rgba_icon_png(&image, bounds)
}

fn alpha_bounds(image: &image::RgbaImage) -> Option<AlphaBounds> {
    let (width, height) = image.dimensions();
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0;
    let mut max_y = 0;
    let mut found = false;

    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] == 0 {
            continue;
        }

        found = true;
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }

    if !found {
        return None;
    }

    Some(AlphaBounds {
        x: min_x,
        y: min_y,
        width: max_x - min_x + 1,
        height: max_y - min_y + 1,
    })
}

fn native_icon_needs_normalization(width: u32, height: u32, bounds: AlphaBounds) -> bool {
    if width != NATIVE_ICON_NORMALIZED_PNG_SIZE || height != NATIVE_ICON_NORMALIZED_PNG_SIZE {
        return true;
    }

    let min_dimension = width.min(height) as f32;
    let content_ratio = bounds.width.max(bounds.height) as f32 / min_dimension;
    if content_ratio < NATIVE_ICON_UNDERSIZED_RATIO {
        return true;
    }

    let image_center_x = width as f32 / 2.0;
    let image_center_y = height as f32 / 2.0;
    let content_center_x = (bounds.x + bounds.right()) as f32 / 2.0;
    let content_center_y = (bounds.y + bounds.bottom()) as f32 / 2.0;
    let tolerance = min_dimension * NATIVE_ICON_CENTER_TOLERANCE_RATIO;

    (content_center_x - image_center_x).abs() > tolerance
        || (content_center_y - image_center_y).abs() > tolerance
}

fn normalized_rgba_icon_png(image: &image::RgbaImage, bounds: AlphaBounds) -> Option<Vec<u8>> {
    let cropped = image::imageops::crop_imm(image, bounds.x, bounds.y, bounds.width, bounds.height)
        .to_image();
    let scale = (NATIVE_ICON_NORMALIZED_CONTENT_SIZE as f32 / bounds.width as f32)
        .min(NATIVE_ICON_NORMALIZED_CONTENT_SIZE as f32 / bounds.height as f32);
    let resized_width = ((bounds.width as f32 * scale).round() as u32).max(1);
    let resized_height = ((bounds.height as f32 * scale).round() as u32).max(1);
    let resized = image::imageops::resize(
        &cropped,
        resized_width,
        resized_height,
        image::imageops::FilterType::Lanczos3,
    );

    let mut canvas = image::RgbaImage::from_pixel(
        NATIVE_ICON_NORMALIZED_PNG_SIZE,
        NATIVE_ICON_NORMALIZED_PNG_SIZE,
        image::Rgba([0, 0, 0, 0]),
    );
    let x = ((NATIVE_ICON_NORMALIZED_PNG_SIZE - resized_width) / 2) as i64;
    let y = ((NATIVE_ICON_NORMALIZED_PNG_SIZE - resized_height) / 2) as i64;
    image::imageops::overlay(&mut canvas, &resized, x, y);
    encode_rgba_png_bytes(
        canvas.as_raw(),
        NATIVE_ICON_NORMALIZED_PNG_SIZE,
        NATIVE_ICON_NORMALIZED_PNG_SIZE,
    )
}

fn encode_rgba_png_bytes(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    use image::ImageEncoder;

    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .ok()?;
    valid_png_bytes(bytes)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct DiskIconManifest {
    version: String,
    #[serde(default)]
    mappings: HashMap<String, String>,
}

impl Default for DiskIconManifest {
    fn default() -> Self {
        Self {
            version: NATIVE_ICON_CACHE_VERSION.to_owned(),
            mappings: HashMap::new(),
        }
    }
}

struct DiskIconStore {
    cache_dir: Option<PathBuf>,
    manifest: DiskIconManifest,
}

impl DiskIconStore {
    fn load(cache_dir: Option<PathBuf>) -> Self {
        let manifest = load_disk_manifest(cache_dir.as_deref()).unwrap_or_default();
        Self {
            cache_dir,
            manifest,
        }
    }

    fn cache_dir(&self) -> Option<&Path> {
        self.cache_dir.as_deref()
    }

    fn icon_hash(&self, key: &str) -> Option<&str> {
        self.manifest.mappings.get(key).map(String::as_str)
    }

    fn write_mapping(&mut self, key: &str, bytes: &[u8]) {
        let Some(cache_dir) = self.cache_dir.as_deref() else {
            return;
        };

        let hash = icon_content_hash(bytes);
        let Some(icon_path) = icon_file_path_from_dir(Some(cache_dir), &hash) else {
            return;
        };

        if !icon_path.exists() {
            let _ = write_atomic(&icon_path, bytes);
        }

        if self.manifest.mappings.get(key) == Some(&hash) {
            return;
        }

        self.manifest.mappings.insert(key.to_owned(), hash);
        let _ = save_disk_manifest(cache_dir, &self.manifest);
    }
}

fn load_disk_manifest(cache_dir: Option<&Path>) -> Option<DiskIconManifest> {
    let path = cache_dir?.join(DISK_MANIFEST_FILE_NAME);
    let manifest =
        serde_json::from_str::<DiskIconManifest>(&fs::read_to_string(path).ok()?).ok()?;
    (manifest.version == NATIVE_ICON_CACHE_VERSION).then_some(manifest)
}

fn save_disk_manifest(cache_dir: &Path, manifest: &DiskIconManifest) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(manifest).map_err(io::Error::other)?;
    write_atomic(&cache_dir.join(DISK_MANIFEST_FILE_NAME), &json)
}

fn read_cached_icon_by_hash(cache_dir: Option<&Path>, hash: &str) -> Option<Vec<u8>> {
    let path = icon_file_path_from_dir(cache_dir, hash)?;
    fs::read(path).ok().and_then(valid_png_bytes)
}

fn icon_file_path_from_dir(cache_dir: Option<&Path>, hash: &str) -> Option<PathBuf> {
    if hash.is_empty()
        || hash
            .chars()
            .any(|ch| !ch.is_ascii_hexdigit() || ch.is_ascii_uppercase())
    {
        return None;
    }

    Some(
        cache_dir?
            .join(DISK_ICON_DIR_NAME)
            .join(format!("{hash}.png")),
    )
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

fn native_icon_cache_dir() -> Option<PathBuf> {
    native_icon_cache_dir_for(current_config_platform(), env_path)
}

fn url_icon_cache_dir() -> Option<PathBuf> {
    platform_cache_dir(current_config_platform(), env_path)
        .map(|dir| dir.join(URL_ICON_CACHE_VERSION))
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

fn native_icon_cache_dir_for(
    platform: ConfigPlatform,
    env_path: impl FnMut(&str) -> Option<PathBuf>,
) -> Option<PathBuf> {
    platform_cache_dir(platform, env_path).map(|dir| dir.join(NATIVE_ICON_CACHE_VERSION))
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

fn existing_url_icon_file_path(cache_dir: Option<&Path>, url: &str) -> Option<PathBuf> {
    let cache_dir = cache_dir?;
    for extension in url_icon_file_extensions(url) {
        let path = url_icon_file_path_with_extension(Some(cache_dir), url, extension)?;
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn preferred_url_icon_file_path(cache_dir: Option<&Path>, url: &str) -> Option<PathBuf> {
    url_icon_file_path_with_extension(cache_dir, url, preferred_url_icon_extension(url))
}

fn url_icon_file_path_with_extension(
    cache_dir: Option<&Path>,
    url: &str,
    extension: &str,
) -> Option<PathBuf> {
    let hash = url_icon_key_hash(url);
    Some(cache_dir?.join(format!("{hash}.{extension}")))
}

fn url_icon_file_extensions(url: &str) -> Vec<&'static str> {
    let preferred = preferred_url_icon_extension(url);
    let mut extensions = vec![preferred];
    for extension in ["png", "svg", "ico", "img"] {
        if extension != preferred {
            extensions.push(extension);
        }
    }
    extensions
}

fn preferred_url_icon_extension(url: &str) -> &'static str {
    url_icon_extension_for_path(url).unwrap_or("img")
}

fn url_icon_extension_for_path(url: &str) -> Option<&'static str> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let file_name = path.rsplit('/').next().unwrap_or(path);
    let extension = file_name.rsplit_once('.')?.1;
    supported_url_icon_extension(extension)
}

fn url_icon_extension_for_content_type(content_type: &str) -> Option<&'static str> {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    match media_type.as_str() {
        "image/png" => Some("png"),
        "image/svg+xml" | "text/svg" => Some("svg"),
        "image/x-icon" | "image/vnd.microsoft.icon" | "image/ico" => Some("ico"),
        _ => None,
    }
}

fn supported_url_icon_extension(extension: &str) -> Option<&'static str> {
    match extension.to_ascii_lowercase().as_str() {
        "png" => Some("png"),
        "svg" => Some("svg"),
        "ico" => Some("ico"),
        _ => None,
    }
}

fn url_icon_key_hash(url: &str) -> String {
    let mut hash = StableHash::new();
    hash.write_str(URL_ICON_CACHE_VERSION);
    hash.write_u64(url.len() as u64);
    hash.write_str(url);
    format!("{:016x}", hash.finish())
}

fn icon_content_hash(bytes: &[u8]) -> String {
    let mut hash = StableHash::new();
    hash.write_str(NATIVE_ICON_CACHE_VERSION);
    hash.write_u64(bytes.len() as u64);
    hash.write_bytes(bytes);
    format!("{:016x}", hash.finish())
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
    use crate::explorer::{
        entry::{DirectoryLinkKind, EntryKind, ShellShortcutTargetKind},
        test_support::TempDir,
    };

    fn one_pixel_png_bytes() -> Vec<u8> {
        encode_rgba_png_bytes(&[0, 0, 0, 255], 1, 1).expect("encode test png")
    }

    fn test_png_with_rect(width: u32, height: u32, rect: Option<AlphaBounds>) -> Vec<u8> {
        let mut image = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 0]));
        if let Some(rect) = rect {
            for y in rect.y..rect.bottom() {
                for x in rect.x..rect.right() {
                    image.put_pixel(x, y, image::Rgba([200, 40, 10, 255]));
                }
            }
        }

        encode_rgba_png_bytes(image.as_raw(), width, height).expect("encode test png")
    }

    fn decoded_alpha_bounds(bytes: Vec<u8>) -> AlphaBounds {
        let image = image::load_from_memory(&bytes)
            .expect("decode png")
            .to_rgba8();
        alpha_bounds(&image).expect("alpha bounds")
    }

    fn test_request(key: &str) -> NativeIconRequest {
        NativeIconRequest {
            key: key.to_owned(),
            source: PlatformIconRequest::Test,
        }
    }

    fn cache_with_dir(cache_dir: Option<PathBuf>) -> NativeIconCacheInner {
        NativeIconCacheInner::new(DiskIconStore::load(cache_dir))
    }

    #[test]
    fn native_icon_cache_version_invalidates_old_low_resolution_icons() {
        assert_eq!(NATIVE_ICON_CACHE_VERSION, "native-icons-v3");
    }

    #[test]
    fn native_icon_normalization_centers_and_scales_small_top_left_content() {
        let bytes = test_png_with_rect(
            128,
            128,
            Some(AlphaBounds {
                x: 0,
                y: 0,
                width: 36,
                height: 36,
            }),
        );

        let normalized = normalize_native_icon_png_bytes(bytes).expect("normalized png");
        let bounds = decoded_alpha_bounds(normalized);

        assert_eq!(
            bounds,
            AlphaBounds {
                x: 8,
                y: 8,
                width: 112,
                height: 112
            }
        );
    }

    #[test]
    fn native_icon_normalization_preserves_non_square_aspect_ratio() {
        let bytes = test_png_with_rect(
            128,
            128,
            Some(AlphaBounds {
                x: 0,
                y: 0,
                width: 32,
                height: 64,
            }),
        );

        let normalized = normalize_native_icon_png_bytes(bytes).expect("normalized png");
        let bounds = decoded_alpha_bounds(normalized);

        assert_eq!(
            bounds,
            AlphaBounds {
                x: 36,
                y: 8,
                width: 56,
                height: 112
            }
        );
    }

    #[test]
    fn native_icon_normalization_leaves_centered_full_size_content_unchanged() {
        let bytes = test_png_with_rect(
            128,
            128,
            Some(AlphaBounds {
                x: 8,
                y: 8,
                width: 112,
                height: 112,
            }),
        );

        assert_eq!(normalize_native_icon_png_bytes(bytes.clone()), Some(bytes));
    }

    #[test]
    fn native_icon_normalization_rejects_transparent_and_invalid_pngs() {
        let transparent = test_png_with_rect(128, 128, None);

        assert!(normalize_native_icon_png_bytes(transparent).is_none());
        assert!(normalize_native_icon_png_bytes(b"not png".to_vec()).is_none());
        assert!(normalize_native_icon_png_bytes(PNG_SIGNATURE.to_vec()).is_none());
    }

    #[test]
    fn url_icon_cache_schedules_missing_urls_once() {
        let temp = TempDir::new();
        let mut cache = UrlIconCacheInner::new(Some(temp.path().join("url-icons")));
        let url = "https://example.com/icon.svg";

        assert_eq!(cache.icon_path_for_url(url), (None, true));
        assert_eq!(cache.pending.len(), 1);
        assert_eq!(cache.icon_path_for_url(url), (None, false));
        assert_eq!(cache.pending.len(), 1);

        let job = cache.next_load_job().expect("url icon job");
        assert_eq!(job.url, url);
        assert_eq!(
            job.path,
            preferred_url_icon_file_path(Some(&temp.path().join("url-icons")), url).unwrap()
        );
        assert_eq!(job.path.extension().and_then(OsStr::to_str), Some("svg"));
    }

    #[test]
    fn url_icon_cache_reuses_existing_disk_file() {
        let temp = TempDir::new();
        let cache_dir = temp.path().join("url-icons");
        let url = "https://example.com/icon.ico";
        let path = preferred_url_icon_file_path(Some(&cache_dir), url).unwrap();
        write_atomic(&path, b"icon-bytes").expect("write cached url icon");
        let mut cache = UrlIconCacheInner::new(Some(cache_dir));

        assert_eq!(cache.icon_path_for_url(url), (Some(path), false));
        assert!(cache.pending.is_empty());
    }

    #[test]
    fn url_icon_cache_paths_preserve_supported_url_extensions() {
        let temp = TempDir::new();
        let cache_dir = temp.path().join("url-icons");

        for (url, extension) in [
            ("https://example.com/icon.png", "png"),
            ("https://example.com/icon.svg?raw=1", "svg"),
            ("https://example.com/icon.ico#icon", "ico"),
            ("https://example.com/icon", "img"),
        ] {
            let path = preferred_url_icon_file_path(Some(&cache_dir), url).unwrap();
            assert_eq!(path.extension().and_then(OsStr::to_str), Some(extension));
        }
    }

    #[test]
    fn url_icon_content_type_fallback_selects_supported_extensions() {
        assert_eq!(
            url_icon_extension_for_content_type("image/png; charset=utf-8"),
            Some("png")
        );
        assert_eq!(
            url_icon_extension_for_content_type("image/svg+xml"),
            Some("svg")
        );
        assert_eq!(
            url_icon_extension_for_content_type("image/vnd.microsoft.icon"),
            Some("ico")
        );
        assert_eq!(url_icon_extension_for_content_type("text/plain"), None);
    }

    #[test]
    fn url_icon_cache_retries_failed_urls_after_cooldown() {
        let temp = TempDir::new();
        let cache_dir = temp.path().join("url-icons");
        let url = "https://example.com/icon.png";
        let mut cache = UrlIconCacheInner::new(Some(cache_dir));

        cache.finish_request(url.to_owned(), None);
        assert_eq!(cache.icon_path_for_url(url), (None, false));

        cache.states.insert(
            url.to_owned(),
            UrlIconState::Failed {
                retry_after: Instant::now() - Duration::from_secs(1),
            },
        );
        assert_eq!(cache.icon_path_for_url(url), (None, true));
        assert_eq!(cache.pending.len(), 1);
    }

    #[gpui::test]
    fn disabled_resolve_icons_skips_native_icon_cache(cx: &mut gpui::TestAppContext) {
        cx.set_global(NativeIconCache::new());
        let (view, cx) = cx.add_window_view(|window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_focus_handle_for_test(PathBuf::from("icons"), focus_handle)
        });

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.apply_settings(
                    &crate::settings::ExplorerSettings {
                        view: crate::settings::ViewSettings {
                            native_icons: false,
                            ..crate::settings::ViewSettings::default()
                        },
                        ..crate::settings::ExplorerSettings::default()
                    },
                    cx,
                );

                assert!(
                    view.native_icon_for_request(Some(test_request("key")), cx)
                        .is_none()
                );
            });

            let cache = app.global::<NativeIconCache>();
            let inner = cache.inner.borrow();
            assert!(inner.pending.is_empty());
            assert!(inner.states.is_empty());
        });
    }

    #[test]
    fn icon_timing_batch_omits_empty_batches() {
        let batch = IconTimingBatch::enabled_for_test();

        assert!(batch.format_lines(Duration::from_millis(1)).is_empty());
    }

    #[test]
    fn icon_timing_batch_formats_stage_totals_fastest_and_slowest() {
        let mut batch = IconTimingBatch::enabled_for_test();
        batch.requests = 2;
        batch.queue_wait.record(Duration::from_millis(2));
        batch.queue_wait.record(Duration::from_micros(500));

        let lines = batch.format_lines(Duration::from_millis(3));
        let queue_wait = lines
            .iter()
            .find(|line| line.starts_with("queue_wait "))
            .expect("queue_wait timing line");

        assert!(queue_wait.contains("count=2"));
        assert!(queue_wait.contains("total=2.500ms"));
        assert!(queue_wait.contains("fastest=0.500ms"));
        assert!(queue_wait.contains("slowest=2.000ms"));
    }

    #[test]
    fn icon_timing_batch_formats_hit_and_success_counters() {
        let mut batch = IconTimingBatch::enabled_for_test();
        batch.requests = 2;
        batch.stale_hits = 1;
        batch.stale_misses = 1;
        batch.stale_published = 1;
        batch.fresh_ok = 1;
        batch.failed = 1;
        batch.stale_disk_read.record(Duration::from_millis(1));
        batch.stale_disk_read.record(Duration::from_millis(3));
        batch.stale_publish.record(Duration::from_millis(1));
        batch.platform_extract.record(Duration::from_millis(4));
        batch.platform_extract.record(Duration::from_millis(6));

        let lines = batch.format_lines(Duration::from_millis(15));
        let summary = lines.first().expect("summary line");
        let stale_read = lines
            .iter()
            .find(|line| line.starts_with("stale_disk_read "))
            .expect("stale read timing line");
        let platform_extract = lines
            .iter()
            .find(|line| line.starts_with("platform_extract "))
            .expect("platform extract timing line");

        assert!(summary.contains("total=15.000ms"));
        assert!(summary.contains("requests=2"));
        assert!(summary.contains("stale_hits=1"));
        assert!(summary.contains("stale_misses=1"));
        assert!(summary.contains("stale_published=1"));
        assert!(summary.contains("fresh_ok=1"));
        assert!(summary.contains("failed=1"));
        assert!(stale_read.contains("hits=1 misses=1"));
        assert!(platform_extract.contains("ok=1 failed=1"));
    }

    #[test]
    fn cache_schedules_request_once() {
        let mut cache = cache_with_dir(None);
        let request = test_request("key");

        assert_eq!(cache.icon_for_request(request.clone()).0.is_some(), false);
        assert_eq!(cache.pending.len(), 1);

        assert_eq!(cache.icon_for_request(request.clone()).0.is_some(), false);
        assert_eq!(cache.pending.len(), 1);

        let job = cache.next_load_job().expect("load job");
        assert_eq!(job.request, request);
        assert!(cache.pending.is_empty());
        assert!(cache.icon_for_request(test_request("key")).0.is_none());
    }

    #[test]
    fn failed_refresh_retains_stale_icon_without_rescheduling() {
        let mut cache = cache_with_dir(None);
        let request = test_request("key");

        cache.icon_for_request(request.clone());
        cache.next_load_job().expect("load job");
        assert!(cache.publish_stale_icon("key", one_pixel_png_bytes()));
        assert!(cache.finish_request(request, None));

        assert!(cache.icon_for_request(test_request("key")).0.is_some());
        assert!(cache.pending.is_empty());
    }

    #[test]
    fn ready_icon_loads_are_reused() {
        let mut cache = cache_with_dir(None);
        let request = test_request("key");

        cache.icon_for_request(request.clone());
        cache.next_load_job().expect("load job");
        assert!(cache.finish_request(request, Some(one_pixel_png_bytes())));

        assert!(cache.icon_for_request(test_request("key")).0.is_some());
        assert!(cache.pending.is_empty());
    }

    #[test]
    fn stale_icon_is_loaded_from_manifest_hash() {
        let temp = TempDir::new();
        let cache_dir = temp.path().join("cache");
        let bytes = one_pixel_png_bytes();
        let hash = icon_content_hash(&bytes);
        write_atomic(
            &cache_dir
                .join(DISK_ICON_DIR_NAME)
                .join(format!("{hash}.png")),
            &bytes,
        )
        .expect("write icon");
        let manifest = DiskIconManifest {
            version: NATIVE_ICON_CACHE_VERSION.to_owned(),
            mappings: HashMap::from([("key".to_owned(), hash.clone())]),
        };
        save_disk_manifest(&cache_dir, &manifest).expect("write manifest");

        let mut cache = cache_with_dir(Some(cache_dir));
        cache.icon_for_request(test_request("key"));
        let job = cache.next_load_job().expect("load job");

        assert_eq!(job.stale_hash, Some(hash));
        assert!(
            read_cached_icon_by_hash(job.cache_dir.as_deref(), job.stale_hash.as_ref().unwrap())
                .is_some()
        );
    }

    #[test]
    fn corrupt_manifest_is_ignored() {
        let temp = TempDir::new();
        let cache_dir = temp.path().join("cache");
        fs::create_dir_all(&cache_dir).expect("create cache");
        fs::write(cache_dir.join(DISK_MANIFEST_FILE_NAME), "{").expect("write corrupt manifest");

        let cache = DiskIconStore::load(Some(cache_dir));

        assert!(cache.manifest.mappings.is_empty());
    }

    #[test]
    fn manifest_rejects_wrong_version() {
        let temp = TempDir::new();
        let cache_dir = temp.path().join("cache");
        let manifest = DiskIconManifest {
            version: "old".to_owned(),
            mappings: HashMap::from([("key".to_owned(), "abc".to_owned())]),
        };
        save_disk_manifest(&cache_dir, &manifest).expect("write manifest");

        let cache = DiskIconStore::load(Some(cache_dir));

        assert!(cache.manifest.mappings.is_empty());
    }

    #[test]
    fn disk_mapping_round_trips_pretty_manifest_and_deduped_icon() {
        let temp = TempDir::new();
        let cache_dir = temp.path().join("cache");
        let mut store = DiskIconStore::load(Some(cache_dir.clone()));
        let bytes = one_pixel_png_bytes();
        let hash = icon_content_hash(&bytes);

        store.write_mapping("first", &bytes);
        store.write_mapping("second", &bytes);

        assert_eq!(store.icon_hash("first"), Some(hash.as_str()));
        assert_eq!(store.icon_hash("second"), Some(hash.as_str()));
        assert_eq!(
            fs::read(
                cache_dir
                    .join(DISK_ICON_DIR_NAME)
                    .join(format!("{hash}.png"))
            )
            .ok(),
            Some(bytes)
        );
        assert!(
            fs::read_to_string(cache_dir.join(DISK_MANIFEST_FILE_NAME))
                .expect("manifest")
                .contains("\n  \"mappings\"")
        );
    }

    #[test]
    fn cached_icon_reader_rejects_invalid_hashes_and_pngs() {
        let temp = TempDir::new();
        let cache_dir = temp.path().join("cache");
        let hash = "abc123";
        write_atomic(
            &cache_dir
                .join(DISK_ICON_DIR_NAME)
                .join(format!("{hash}.png")),
            b"not png",
        )
        .expect("write invalid png");

        assert!(read_cached_icon_by_hash(Some(&cache_dir), hash).is_none());
        assert!(read_cached_icon_by_hash(Some(&cache_dir), "../escape").is_none());
        assert!(read_cached_icon_by_hash(Some(&cache_dir), "ABC").is_none());
    }

    #[test]
    fn fresh_refresh_updates_existing_disk_mapping() {
        let temp = TempDir::new();
        let cache_dir = temp.path().join("cache");
        let first_bytes = one_pixel_png_bytes();
        let mut second_bytes = one_pixel_png_bytes();
        second_bytes.extend_from_slice(b"changed");
        let mut store = DiskIconStore::load(Some(cache_dir));

        store.write_mapping("key", &first_bytes);
        let first_hash = store.icon_hash("key").map(ToOwned::to_owned);

        store.write_mapping("key", &second_bytes);
        let second_hash = store.icon_hash("key").map(ToOwned::to_owned);

        assert_ne!(first_hash, second_hash);
        assert_eq!(
            second_hash.as_deref(),
            Some(icon_content_hash(&second_bytes).as_str())
        );
    }

    #[test]
    fn icon_cache_dirs_follow_platform_conventions() {
        assert_eq!(
            native_icon_cache_dir_for(ConfigPlatform::MacOS, |name| {
                (name == "HOME").then(|| PathBuf::from("home"))
            }),
            Some(
                PathBuf::from("home")
                    .join(".config")
                    .join("explorer")
                    .join("cache")
                    .join(NATIVE_ICON_CACHE_VERSION)
            )
        );
        assert_eq!(
            native_icon_cache_dir_for(ConfigPlatform::Windows, |name| {
                (name == "LOCALAPPDATA").then(|| PathBuf::from("local"))
            }),
            Some(
                PathBuf::from("local")
                    .join(APP_ID)
                    .join("cache")
                    .join(NATIVE_ICON_CACHE_VERSION)
            )
        );
        assert_eq!(
            native_icon_cache_dir_for(ConfigPlatform::Linux, |name| {
                (name == "XDG_CACHE_HOME").then(|| PathBuf::from("xdg"))
            }),
            Some(
                PathBuf::from("xdg")
                    .join("explorer")
                    .join(NATIVE_ICON_CACHE_VERSION)
            )
        );
    }

    #[test]
    fn icon_resource_path_adds_icns_extension_when_missing() {
        assert_eq!(
            bundle_icon_resource_path(Path::new("/Applications/Preview.app"), "Preview"),
            Some(PathBuf::from(
                "/Applications/Preview.app/Contents/Resources/Preview.icns"
            ))
        );
    }

    #[test]
    fn icon_resource_path_keeps_existing_icns_extension() {
        assert_eq!(
            bundle_icon_resource_path(Path::new("/Applications/Preview.app"), "Preview.icns"),
            Some(PathBuf::from(
                "/Applications/Preview.app/Contents/Resources/Preview.icns"
            ))
        );
    }

    #[test]
    fn icon_resource_path_ignores_empty_icon_names() {
        assert_eq!(
            bundle_icon_resource_path(Path::new("Preview.app"), ""),
            None
        );
        assert_eq!(
            bundle_icon_resource_path(Path::new("Preview.app"), "  "),
            None
        );
    }

    #[test]
    fn valid_png_bytes_rejects_non_png_data() {
        assert!(valid_png_bytes(b"not png".to_vec()).is_none());
        assert!(valid_png_bytes(PNG_SIGNATURE.to_vec()).is_some());
    }

    #[test]
    fn windows_extension_requests_are_shared_and_case_insensitive() {
        let first = FileEntry::test("Report.TXT", false, Some(1), None);
        let second = FileEntry::test("notes.txt", false, Some(1), None);

        let first = windows_icon_request_for_entry(&first).expect("first icon request");
        let second = windows_icon_request_for_entry(&second).expect("second icon request");

        assert_eq!(first.key, second.key);
        assert!(matches!(
            first.source,
            PlatformIconRequest::Windows(WindowsIconRequest::Extension { ref extension })
                if extension == "txt"
        ));
    }

    #[test]
    fn windows_path_icons_are_used_for_executables_shortcuts_and_directory_links() {
        let exe = FileEntry::test("app.exe", false, Some(1), None);
        let shortcut = FileEntry::test("target.lnk", false, Some(1), None);
        let directory_link = FileEntry {
            path: PathBuf::from("linked"),
            name: "linked".to_owned(),
            kind: EntryKind::DirectoryLink(DirectoryLinkKind::FilesystemLink),
            modified: None,
            size: None,
        };

        for entry in [exe, shortcut, directory_link] {
            let request = windows_icon_request_for_entry(&entry).expect("icon request");
            assert!(matches!(
                request.source,
                PlatformIconRequest::Windows(WindowsIconRequest::Path { .. })
            ));
        }
    }

    #[test]
    fn windows_plain_directories_do_not_request_native_icons() {
        let folder = FileEntry::test("folder", true, None, None);

        assert!(windows_icon_request_for_entry(&folder).is_none());
    }

    #[test]
    fn windows_directory_shortcuts_request_path_icons() {
        let entry = FileEntry::test_directory_link(
            "target.lnk",
            DirectoryLinkKind::ShellShortcut {
                target: PathBuf::from("target"),
                target_kind: ShellShortcutTargetKind::Directory,
            },
        );

        let request = windows_icon_request_for_entry(&entry).expect("icon request");

        assert!(matches!(
            request.source,
            PlatformIconRequest::Windows(WindowsIconRequest::Path { .. })
        ));
    }

    #[test]
    fn mac_file_type_requests_are_shared_case_insensitively() {
        let first = FileEntry::test("Report.TXT", false, Some(1), None);
        let second = FileEntry::test("notes.txt", false, Some(1), None);

        let first = mac_icon_request_for_entry(&first).expect("first icon request");
        let second = mac_icon_request_for_entry(&second).expect("second icon request");

        assert_eq!(first.key, second.key);
        assert!(matches!(
            first.source,
            PlatformIconRequest::Mac(MacIconRequest::FileType { ref extension })
                if extension == "txt"
        ));
    }

    #[test]
    fn mac_unknown_file_type_requests_use_extension_key() {
        let entry = FileEntry::test("document.custom", false, Some(1), None);
        let request = mac_icon_request_for_entry(&entry).expect("icon request");

        assert!(matches!(
            request.source,
            PlatformIconRequest::Mac(MacIconRequest::FileType { ref extension })
                if extension == "custom"
        ));
    }

    #[test]
    fn mac_plain_directories_do_not_request_native_icons() {
        let folder = FileEntry::test("folder", true, None, None);

        assert!(mac_icon_request_for_entry(&folder).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_app_bundle_requests_use_path_specific_cache_key() {
        let entry = FileEntry::test("Preview.app", true, None, None);
        let request = mac_icon_request_for_entry(&entry).expect("icon request");

        assert!(
            request
                .key
                .starts_with(&format!("{NATIVE_ICON_CACHE_VERSION}:macos:app:"))
        );
        assert!(matches!(
            request.source,
            PlatformIconRequest::Mac(MacIconRequest::AppBundle { .. })
        ));
    }

    #[cfg(target_os = "windows")]
    fn assert_windows_shell_icon_request_extracts_valid_png(request: WindowsIconRequest) {
        static SHELL_ICON_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = SHELL_ICON_TEST_LOCK.lock().expect("shell icon test lock");
        let bytes = load_windows_shell_icon_png_bytes(&request).expect("icon png");

        assert!(valid_png_bytes(bytes).is_some());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_shell_icon_loader_extracts_valid_png_for_current_exe() {
        let request = WindowsIconRequest::Path {
            path: std::env::current_exe().expect("current exe"),
        };
        assert_windows_shell_icon_request_extracts_valid_png(request);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_shell_icon_loader_accepts_forward_slash_paths() {
        let current_exe = std::env::current_exe().expect("current exe");
        let forward_slash_path = PathBuf::from(current_exe.to_string_lossy().replace('\\', "/"));
        assert!(forward_slash_path.to_string_lossy().contains('/'));
        let request = WindowsIconRequest::Path {
            path: forward_slash_path,
        };
        assert_windows_shell_icon_request_extracts_valid_png(request);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_file_type_icon_loader_extracts_valid_png_for_text_files() {
        let bytes = load_file_type_icon_png_bytes("txt").expect("icon png");

        assert!(valid_png_bytes(bytes).is_some());
    }
}
