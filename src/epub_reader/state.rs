use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, UNIX_EPOCH},
};

use gpui::{App, Global, Task};
use serde::{Deserialize, Serialize};

use super::book::ContentPoint;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub(super) struct Location {
    pub href: String,
    pub point: ContentPoint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct Preferences {
    pub font_size: u8,
    pub light: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            font_size: 20,
            light: false,
        }
    }
}

impl Preferences {
    fn validated(mut self) -> Self {
        self.font_size = self.font_size.clamp(12, 36) / 2 * 2;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(super) struct Fingerprint {
    pub size: u64,
    pub modified: u64,
    pub nanos: u32,
}

impl Fingerprint {
    pub fn for_path(path: &Path) -> io::Result<Self> {
        let metadata = fs::metadata(path)?;
        let modified = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Ok(Self {
            size: metadata.len(),
            modified: modified.as_secs(),
            nanos: modified.subsec_nanos(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SavedBook {
    fingerprint: Fingerprint,
    location: Location,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct Store {
    preferences: Preferences,
    books: BTreeMap<String, SavedBook>,
}

impl Store {
    pub fn load(path: &Path) -> Self {
        let mut store: Self = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        store.preferences = store.preferences.validated();
        store
    }
    pub fn resume(&self, key: &str, fingerprint: &Fingerprint) -> Option<Location> {
        self.books
            .get(key)
            .filter(|book| book.fingerprint == *fingerprint)
            .map(|book| book.location.clone())
    }
    pub fn remember(&mut self, key: String, fingerprint: Fingerprint, location: Location) {
        self.books.insert(
            key,
            SavedBook {
                fingerprint,
                location,
            },
        );
    }
}

pub(super) fn book_key(path: &Path) -> String {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    format!(
        "{:016x}",
        xxhash_rust::xxh3::xxh3_64(canonical.as_os_str().as_encoded_bytes())
    )
}

struct Writer {
    revision: AtomicU64,
    lock: Mutex<()>,
}

struct ReaderState {
    store: Store,
    path: Option<PathBuf>,
    writer: Arc<Writer>,
    task: Option<Task<()>>,
}

impl Global for ReaderState {}

pub(super) fn initialize(cx: &mut App) {
    if cx.has_global::<ReaderState>() {
        return;
    }
    let path = if cfg!(test) {
        None
    } else {
        crate::settings::config_dir().map(|path| path.join("epub-reader-state.json"))
    };
    let store = path.as_deref().map(Store::load).unwrap_or_default();
    cx.set_global(ReaderState {
        store,
        path,
        writer: Arc::new(Writer {
            revision: AtomicU64::new(0),
            lock: Mutex::new(()),
        }),
        task: None,
    });
    cx.on_app_quit(|cx| {
        let state = cx.global::<ReaderState>();
        let writer = state.writer.clone();
        let revision = writer.revision.fetch_add(1, Ordering::SeqCst) + 1;
        let store = state.store.clone();
        let path = state.path.clone();
        let task = cx.background_executor().spawn(async move {
            write_snapshot(path, &store, &writer, revision);
        });
        async move {
            task.await;
        }
    })
    .detach();
}

pub(super) fn preferences(cx: &App) -> Preferences {
    cx.global::<ReaderState>().store.preferences
}

pub(super) fn resume(cx: &App, key: &str, fingerprint: &Fingerprint) -> Option<Location> {
    cx.global::<ReaderState>().store.resume(key, fingerprint)
}

pub(super) fn remember(cx: &mut App, key: String, fingerprint: Fingerprint, location: Location) {
    cx.global_mut::<ReaderState>()
        .store
        .remember(key, fingerprint, location);
    schedule_save(cx);
}

pub(super) fn set_preferences(cx: &mut App, preferences: Preferences) {
    cx.global_mut::<ReaderState>().store.preferences = preferences.validated();
    schedule_save(cx);
}

fn schedule_save(cx: &mut App) {
    let state = cx.global::<ReaderState>();
    let writer = state.writer.clone();
    let revision = writer.revision.fetch_add(1, Ordering::SeqCst) + 1;
    let store = state.store.clone();
    let path = state.path.clone();
    let task = cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(250))
            .await;
        cx.background_executor()
            .spawn(async move {
                write_snapshot(path, &store, &writer, revision);
            })
            .await;
    });
    cx.global_mut::<ReaderState>().task = Some(task);
}

fn write_snapshot(path: Option<PathBuf>, store: &Store, writer: &Writer, revision: u64) {
    let Ok(_guard) = writer.lock.lock() else {
        return;
    };
    if writer.revision.load(Ordering::SeqCst) != revision {
        return;
    }
    if let Some(path) = path {
        if let Err(error) = save_store(&path, store) {
            eprintln!("Could not save EPUB reading state: {error}");
        }
    }
}

pub(super) fn save_store(path: &Path, store: &Store) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("State path has no parent."))?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temporary, store).map_err(io::Error::other)?;
    temporary.flush()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}
