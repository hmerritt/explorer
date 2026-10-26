use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, UNIX_EPOCH},
};

use gpui::{App, Global, Task};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::book::{Block, Chapter, ContentPoint};

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub(super) struct Location {
    pub href: String,
    pub point: ContentPoint,
    #[serde(default)]
    pub quote: String,
    #[serde(default)]
    pub prefix: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct Bookmark {
    pub name: String,
    pub location: Location,
}

impl Location {
    pub fn at(href: String, point: ContentPoint, chapter: Option<&Chapter>) -> Self {
        let mut location = Self {
            href,
            point,
            ..Self::default()
        };
        if let Some(Block::Text { text, .. }) =
            chapter.and_then(|chapter| chapter.blocks.get(point.block))
        {
            let mut offset = point.offset.min(text.len());
            while !text.is_char_boundary(offset) {
                offset -= 1;
            }
            location.quote = text[offset..].chars().take(64).collect();
            location.prefix = text[..offset]
                .chars()
                .rev()
                .take(24)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
        }
        location
    }

    pub fn recover(&self, chapter: &Chapter) -> ContentPoint {
        if self.quote.is_empty() {
            return super::pagination::normalize_point(chapter, self.point);
        }
        let mut matches = Vec::new();
        for (block, content) in chapter.blocks.iter().enumerate() {
            if let Block::Text { text, .. } = content {
                for (offset, _) in text.match_indices(&self.quote) {
                    let prefix_match = text[..offset].ends_with(&self.prefix);
                    matches.push((
                        !prefix_match,
                        block.abs_diff(self.point.block),
                        offset.abs_diff(self.point.offset),
                        ContentPoint { block, offset },
                    ));
                }
            }
        }
        matches.sort_by_key(|entry| (entry.0, entry.1, entry.2));
        matches
            .first()
            .map(|entry| entry.3)
            .unwrap_or_else(|| super::pagination::normalize_point(chapter, self.point))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct Preferences {
    pub font_size: u8,
    pub light: bool,
    pub sans: bool,
    pub line_spacing: u8,
    pub reading_width: u16,
    pub justify: bool,
    pub publisher_styles: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            font_size: 20,
            light: false,
            sans: true,
            line_spacing: 16,
            reading_width: 900,
            justify: true,
            publisher_styles: false,
        }
    }
}

impl Preferences {
    fn validated(mut self) -> Self {
        self.font_size = self.font_size.clamp(12, 36) / 2 * 2;
        self.line_spacing = self.line_spacing.clamp(12, 22);
        self.reading_width = self.reading_width.clamp(400, 1000);
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
    #[serde(default)]
    bookmarks: Vec<Bookmark>,
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
            .filter(|book| key.starts_with("sha256:") || book.fingerprint == *fingerprint)
            .map(|book| book.location.clone())
    }
    pub fn remember(&mut self, key: String, fingerprint: Fingerprint, location: Location) {
        let bookmarks = self
            .books
            .get(&key)
            .map(|book| book.bookmarks.clone())
            .unwrap_or_default();
        self.books.insert(
            key,
            SavedBook {
                fingerprint,
                location,
                bookmarks,
            },
        );
    }

    pub fn migrate(&mut self, key: &str, legacy: &str, fingerprint: &Fingerprint) {
        if !self.books.contains_key(key) {
            if let Some(book) = self
                .books
                .get(legacy)
                .filter(|book| book.fingerprint == *fingerprint)
                .cloned()
            {
                self.books.insert(key.to_owned(), book);
            }
        }
    }
}

/// Stream the archive on the worker rather than allocating the whole book.
pub(super) fn content_key(
    path: &Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        super::book::check_cancel(cancel)?;
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
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

pub(super) fn migrate(cx: &mut App, key: &str, legacy: &str, fingerprint: &Fingerprint) {
    cx.global_mut::<ReaderState>()
        .store
        .migrate(key, legacy, fingerprint);
    schedule_save(cx);
}

pub(super) fn bookmarks(cx: &App, key: &str) -> Vec<Bookmark> {
    cx.global::<ReaderState>()
        .store
        .books
        .get(key)
        .map(|book| book.bookmarks.clone())
        .unwrap_or_default()
}

pub(super) fn set_bookmarks(cx: &mut App, key: &str, bookmarks: Vec<Bookmark>) {
    if let Some(book) = cx.global_mut::<ReaderState>().store.books.get_mut(key) {
        book.bookmarks = bookmarks;
        schedule_save(cx);
    }
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
