use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use gpui::{App, Context, Global, Image};

use crate::{
    explorer::{
        entry::FileEntry,
        image_preview::{load_image_thumbnail_png_with_cancel, path_may_have_image_preview},
        view::ExplorerView,
    },
    settings::{APP_ID, ConfigPlatform, config_dir_for},
};

const IMAGE_THUMBNAIL_LOAD_INTERVAL: Duration = Duration::from_millis(16);
const IMAGE_THUMBNAIL_CACHE_VERSION: &str = "image-thumbnails-v1";
const IMAGE_THUMBNAIL_SIZE: u32 = 128;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

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
    key: String,
    path: PathBuf,
    directory: PathBuf,
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
        loop {
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

            let request = job.request.clone();
            let cache_dir = job.cache_dir.clone();
            let cancel = job.cancel.clone();
            let load_task = cx.background_executor().spawn(async move {
                load_or_create_thumbnail_png(&request, cache_dir.as_deref(), &cancel)
            });
            let thumbnail = load_task.await;

            let finished = cx
                .update_global::<ImageThumbnailCache, _>(|cache, _| {
                    cache
                        .inner
                        .borrow_mut()
                        .finish_request(job.request, job.generation, thumbnail)
                })
                .unwrap_or(false);

            if !finished {
                continue;
            }

            let elapsed = job.queued_at.elapsed();
            crate::debug_options::log_icon_timing(format_args!(
                "image_thumbnail loaded in {:.3}ms",
                elapsed.as_secs_f64() * 1000.0
            ));

            cx.background_executor()
                .timer(IMAGE_THUMBNAIL_LOAD_INTERVAL)
                .await;
        }
    })
    .detach();
}

fn load_or_create_thumbnail_png(
    request: &ImageThumbnailRequest,
    cache_dir: Option<&Path>,
    cancel: &AtomicBool,
) -> Option<Vec<u8>> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    if let Some(bytes) = read_cached_thumbnail(cache_dir, &request.key) {
        return (!cancel.load(Ordering::Relaxed)).then_some(bytes);
    }

    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    let bytes =
        load_image_thumbnail_png_with_cancel(&request.path, IMAGE_THUMBNAIL_SIZE, cancel).ok()?;
    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    write_cached_thumbnail(cache_dir, &request.key, &bytes);
    (!cancel.load(Ordering::Relaxed)).then_some(bytes)
}

fn image_thumbnail_request_for_entry(
    entry: &FileEntry,
    directory: &Path,
) -> Option<ImageThumbnailRequest> {
    if entry.is_directory_like() || !path_may_have_image_preview(&entry.path) {
        return None;
    }

    Some(ImageThumbnailRequest {
        key: image_thumbnail_key(entry),
        path: entry.path.clone(),
        directory: directory.to_path_buf(),
    })
}

fn image_thumbnail_key(entry: &FileEntry) -> String {
    let mut hash = StableHash::new();
    hash.write_str(IMAGE_THUMBNAIL_CACHE_VERSION);
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

fn write_cached_thumbnail(cache_dir: Option<&Path>, key: &str, bytes: &[u8]) {
    let Some(path) = thumbnail_file_path(cache_dir, key) else {
        return;
    };
    let _ = write_atomic(&path, bytes);
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
            assert!(
                image_thumbnail_request_for_entry(&entry, Path::new("folder")).is_some(),
                "expected request for {name}"
            );
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

        assert_ne!(image_thumbnail_key(&first), image_thumbnail_key(&second));
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
            key: "0123456789abcdef".to_owned(),
            path: source,
            directory: temp.path().to_path_buf(),
        };
        let cancel = AtomicBool::new(false);

        let generated = load_or_create_thumbnail_png(&request, Some(temp.path()), &cancel).unwrap();
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

    fn one_pixel_png_bytes() -> Vec<u8> {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(1, 1));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }
}
