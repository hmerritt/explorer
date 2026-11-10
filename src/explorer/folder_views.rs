use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, RecvTimeoutError, Sender},
    time::{Duration, Instant},
};

use futures::{FutureExt, channel::oneshot, future::Shared};
use gpui::{App, AsyncApp, BorrowAppContext, Global};
use serde::{Deserialize, Serialize};

use crate::settings::{FileColumnSettings, FileSortSettings, FileViewMode};

mod volume;

const VERSION: u32 = 1;
const WRITE_DELAY: Duration = Duration::from_millis(200);
const RETRY_DELAY: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct FolderView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) mode: Option<FileViewMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) sort: Option<FileSortSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) columns: Option<FileColumnSettings>,
}

impl FolderView {
    pub(super) fn resolve(
        &self,
        default_sort: FileSortSettings,
        default_columns: &FileColumnSettings,
    ) -> (Option<FileViewMode>, FileSortSettings, FileColumnSettings) {
        (
            self.mode,
            self.sort.unwrap_or(default_sort),
            self.columns.as_ref().unwrap_or(default_columns).clone(),
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Record {
    #[serde(flatten)]
    view: FolderView,
    #[serde(skip_serializing_if = "Option::is_none")]
    volume: Option<volume::Volume>,
    #[serde(skip)]
    revision: u64,
}

type Index = HashMap<PathBuf, Record>;
pub(super) type Ready = Shared<oneshot::Receiver<()>>;

#[derive(Default)]
pub(super) struct FolderViews {
    index: Index,
    revision: u64,
    loading: Option<Ready>,
    writer: Option<Sender<WriteCommand>>,
}

impl Global for FolderViews {}

impl FolderViews {
    pub(super) fn get(&self, path: &Path) -> Option<(u64, &FolderView)> {
        if !eligible(path) {
            return None;
        }
        let record = self.index.get(path)?;
        Some((record.revision, &record.view))
    }

    pub(super) fn pending(&self) -> Option<Ready> {
        self.loading.clone()
    }

    pub(super) fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    #[cfg(test)]
    pub(super) fn pause_startup(cx: &mut App) -> oneshot::Sender<()> {
        let (ready, loading) = oneshot::channel();
        cx.set_global(Self {
            loading: Some(loading.shared()),
            ..Self::default()
        });
        ready
    }

    fn replace(&mut self, path: PathBuf, record: Option<Record>) {
        if let Some(record) = &record {
            self.index.insert(path.clone(), record.clone());
        } else {
            self.index.remove(&path);
        }
        if let Some(writer) = &self.writer {
            let _ = writer.send(WriteCommand::Replace(path, record.map(Box::new)));
        }
    }
}

pub(super) fn eligible(path: &Path) -> bool {
    path.is_absolute()
        && !super::archive_fs::is_archive_path(path)
        && !super::portable_devices::is_portable_path(path)
        && !super::remote_fs::is_remote(path)
}

pub(crate) fn initialize(cx: &mut App, writable: bool) {
    let (ready, loading) = oneshot::channel();
    cx.set_global(FolderViews {
        loading: Some(loading.shared()),
        ..FolderViews::default()
    });
    let path = crate::settings::config_dir().map(|dir| dir.join("folder-views.json"));
    let task = cx.background_executor().spawn(async move {
        let _span = crate::startup_trace::Span::new("folder_views_load");
        open(path, writable)
    });
    cx.spawn(async move |cx| {
        let store = task.await;
        let _ = cx.update(|cx| cx.set_global(store));
        let _ = ready.send(());
    })
    .detach();
}

fn open(path: Option<PathBuf>, writable: bool) -> FolderViews {
    let (index, path) = match path {
        Some(path) => match load(&path) {
            Ok(index) => (index, writable.then_some(path)),
            Err(error) => {
                eprintln!("Unable to load Explorer folder views: {error}");
                (Index::new(), None)
            }
        },
        None => (Index::new(), None),
    };
    let writer = path.and_then(|path| match start_writer(path, index.clone()) {
        Ok(writer) => Some(writer),
        Err(error) => {
            eprintln!("Unable to start Explorer folder view writer: {error}");
            None
        }
    });
    FolderViews {
        index,
        writer,
        ..FolderViews::default()
    }
}

pub(crate) fn flush_on_quit(cx: &App) {
    if let Some(writer) = cx
        .try_global::<FolderViews>()
        .and_then(|store| store.writer.as_ref())
    {
        let (done, wait) = oneshot::channel();
        let _ = writer.send(WriteCommand::Shutdown(done));
        // GPUI limits async quit hooks to 100 ms. A large snapshot can take
        // longer, so finish the final write before releasing instance ownership.
        let _ = futures::executor::block_on(wait);
    }
}

pub(super) fn edit(cx: &mut App, path: &Path, edit: impl FnOnce(&mut FolderView)) {
    let path = path.to_path_buf();
    let probe = cx.update_global::<FolderViews, _>(|store, _| {
        if store.loading.is_some() {
            return None;
        }
        let mut record = store.index.get(&path).cloned().unwrap_or_else(|| Record {
            view: FolderView::default(),
            volume: None,
            revision: 0,
        });
        let before = record.view.clone();
        edit(&mut record.view);
        if record.view == before {
            return None;
        }
        store.revision += 1;
        record.revision = store.revision;
        let probe = store.writer.is_some().then_some(record.revision);
        let record = (record.view != FolderView::default()).then_some(record);
        store.replace(path.clone(), record);
        probe.filter(|_| store.index.contains_key(&path))
    });
    if let Some(revision) = probe {
        probe_volume(cx, path, revision);
    }
}

pub(super) fn repair_missing_volume(cx: &mut App, path: &Path) {
    if cx.try_global::<FolderViews>().is_some_and(|store| {
        store.writer.is_some()
            && store
                .index
                .get(path)
                .is_some_and(|record| record.revision == 0 && record.volume.is_none())
    }) {
        probe_volume(cx, path.to_path_buf(), 0);
    }
}

fn probe_volume(cx: &mut App, path: PathBuf, revision: u64) {
    let target = path.clone();
    let task = cx.background_executor().spawn(async move {
        fs::metadata(&target).ok().filter(|meta| meta.is_dir())?;
        volume::for_path(&target)
    });
    cx.spawn(async move |cx| {
        if let Some(volume) = task.await {
            let _ = cx.update(|cx| {
                cx.update_global::<FolderViews, _>(|store, _| {
                    if let Some(mut record) = store.index.get(&path).cloned()
                        && record.revision == revision
                        && record.volume.as_ref() != Some(&volume)
                    {
                        store.revision += 1;
                        record.revision = store.revision;
                        record.volume = Some(volume);
                        store.replace(path, Some(record));
                    }
                });
            });
        }
    })
    .detach();
}

pub(super) async fn cleanup(cx: &AsyncApp) {
    let pending = cx
        .update(|cx| cx.try_global::<FolderViews>()?.pending())
        .ok()
        .flatten();
    if let Some(pending) = pending {
        let _ = pending.await;
    }
    let snapshot = cx.update(|cx| cleanup_snapshot(cx)).ok().flatten();
    if let Some(snapshot) = snapshot
        && let Ok(snapshot) = snapshot.await
    {
        let stale = cx
            .background_executor()
            .spawn(async move { stale_records(snapshot) })
            .await;
        let _ = cx.update(|cx| prune(cx, stale));
    }
}

fn cleanup_snapshot(cx: &App) -> Option<oneshot::Receiver<Index>> {
    let writer = cx.try_global::<FolderViews>()?.writer.as_ref()?;
    let (send, receive) = oneshot::channel();
    writer.send(WriteCommand::Snapshot(send)).ok()?;
    Some(receive)
}

fn stale_records(snapshot: Index) -> Vec<(PathBuf, u64)> {
    snapshot
        .into_iter()
        .filter_map(|(path, record)| {
            let volume = record.volume?;
            volume::confirmed_absent(&path, &volume).then_some((path, record.revision))
        })
        .collect()
}

fn prune(cx: &mut App, stale: Vec<(PathBuf, u64)>) {
    if stale.is_empty() {
        return;
    }
    cx.update_global::<FolderViews, _>(|store, _| {
        for (path, revision) in stale {
            if store
                .index
                .get(&path)
                .is_some_and(|record| record.revision == revision)
            {
                store.replace(path, None);
            }
        }
    });
}

enum WriteCommand {
    Replace(PathBuf, Option<Box<Record>>),
    Snapshot(oneshot::Sender<Index>),
    Shutdown(oneshot::Sender<()>),
}

fn start_writer(path: PathBuf, mut index: Index) -> io::Result<Sender<WriteCommand>> {
    let (send, receive) = mpsc::channel();
    std::thread::Builder::new()
        .name("explorer-folder-views".into())
        .spawn(move || {
            let mut deadline = None::<Instant>;
            loop {
                let command = match deadline {
                    Some(at) if at <= Instant::now() => Err(RecvTimeoutError::Timeout),
                    Some(at) => receive.recv_timeout(at.saturating_duration_since(Instant::now())),
                    None => receive.recv().map_err(|_| RecvTimeoutError::Disconnected),
                };
                match command {
                    Ok(WriteCommand::Replace(key, record)) => {
                        if let Some(record) = record {
                            index.insert(key, *record);
                        } else {
                            index.remove(&key);
                        }
                        deadline.get_or_insert_with(|| Instant::now() + WRITE_DELAY);
                    }
                    Ok(WriteCommand::Snapshot(reply)) => {
                        let _ = reply.send(index.clone());
                    }
                    Ok(WriteCommand::Shutdown(reply)) => {
                        if deadline.is_some() {
                            write_pending(&path, &index);
                        }
                        let _ = reply.send(());
                        break;
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        deadline =
                            (!write_pending(&path, &index)).then(|| Instant::now() + RETRY_DELAY);
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        })?;
    Ok(send)
}

fn write_pending(path: &Path, index: &Index) -> bool {
    if let Err(error) = save(path, index) {
        eprintln!("Unable to save Explorer folder views: {error}");
        return false;
    }
    true
}

#[derive(Deserialize)]
struct DiskStore {
    version: u32,
    entries: Vec<(NativePath, Record)>,
}

fn load(path: &Path) -> io::Result<Index> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Index::new()),
        Err(error) => return Err(error),
    };
    let disk: DiskStore = serde_json::from_slice(&bytes)?;
    if disk.version != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unsupported folder view version",
        ));
    }
    Ok(disk
        .entries
        .into_iter()
        .filter_map(|(path, record)| {
            (eligible(&path.0) && record.view != FolderView::default()).then_some((path.0, record))
        })
        .collect())
}

fn save(path: &Path, index: &Index) -> io::Result<()> {
    #[derive(Serialize)]
    struct Snapshot<'a> {
        version: u32,
        entries: Vec<(NativePath, &'a Record)>,
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing config directory"))?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut writer = BufWriter::new(temp.as_file_mut());
        serde_json::to_writer(
            &mut writer,
            &Snapshot {
                version: VERSION,
                entries: index
                    .iter()
                    .map(|(key, record)| (NativePath(key.clone()), record))
                    .collect(),
            },
        )?;
        writer.flush()?;
    }
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

// UTF-8 paths stay readable. The platform-specific representation also preserves
// Unix non-UTF-8 filenames and Windows unpaired UTF-16 surrogates.
#[derive(Clone, Debug, Eq, PartialEq)]
struct NativePath(PathBuf);

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum EncodedPath {
    Text(String),
    #[cfg(unix)]
    Unix {
        unix: Vec<u8>,
    },
    #[cfg(windows)]
    Windows {
        windows: Vec<u16>,
    },
}

impl Serialize for NativePath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let encoded = if let Some(path) = self.0.to_str() {
            EncodedPath::Text(path.to_owned())
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt;
                EncodedPath::Unix {
                    unix: self.0.as_os_str().as_bytes().to_vec(),
                }
            }
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStrExt;
                EncodedPath::Windows {
                    windows: self.0.as_os_str().encode_wide().collect(),
                }
            }
        };
        encoded.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for NativePath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let path = match EncodedPath::deserialize(deserializer)? {
            EncodedPath::Text(path) => OsString::from(path),
            #[cfg(unix)]
            EncodedPath::Unix { unix } => {
                use std::os::unix::ffi::OsStringExt;
                OsString::from_vec(unix)
            }
            #[cfg(windows)]
            EncodedPath::Windows { windows } => {
                use std::os::windows::ffi::OsStringExt;
                OsString::from_wide(&windows)
            }
        };
        Ok(Self(path.into()))
    }
}

#[cfg(feature = "benchmarks")]
pub mod benchmark_support {
    use std::{
        hint::black_box,
        sync::{
            Barrier,
            atomic::{AtomicBool, Ordering},
        },
    };

    use super::*;

    pub struct FolderViewLookup {
        store: FolderViews,
        paths: Vec<(PathBuf, PathBuf)>,
        defaults: FileColumnSettings,
        fixture: tempfile::TempDir,
    }

    impl FolderViewLookup {
        pub fn new(count: usize) -> Self {
            let fixture = tempfile::tempdir().expect("folder view benchmark directory");
            let paths: Vec<_> = (0..count)
                .map(|i| {
                    let parent = fixture.path().join(format!("workspace-{}", i / 100));
                    (
                        parent.join(format!("photos-{i}")),
                        parent.join(format!("other-{i}")),
                    )
                })
                .collect();
            let defaults = FileColumnSettings::default();
            let record = Record {
                view: FolderView {
                    mode: Some(FileViewMode::LargeIcons),
                    sort: Some(FileSortSettings::default()),
                    columns: Some(defaults.clone()),
                },
                volume: None,
                revision: 0,
            };
            let index = paths
                .iter()
                .map(|(key, _)| (key.clone(), record.clone()))
                .collect();
            Self {
                store: FolderViews {
                    index,
                    ..FolderViews::default()
                },
                paths,
                defaults,
                fixture,
            }
        }

        pub fn resolve(&self, index: usize, hit: bool) {
            let (saved, missing) = &self.paths[index % self.paths.len()];
            let path = black_box(if hit { saved } else { missing });
            let view = self.store.get(path).map(|(_, view)| view);
            black_box(
                view.unwrap_or(&FolderView::default())
                    .resolve(FileSortSettings::default(), &self.defaults),
            );
        }

        pub fn measure_under_io(&self) -> (Duration, Duration, Duration) {
            let file = self.fixture.path().join("folder-views.json");
            let volume = volume::for_path(self.fixture.path()).expect("benchmark volume");
            let missing = self.fixture.path().join("missing");
            let stop = AtomicBool::new(false);
            let start = Barrier::new(3);
            let (hit, miss) = std::thread::scope(|scope| {
                scope.spawn(|| {
                    start.wait();
                    loop {
                        save(&file, &self.store.index).expect("write benchmark folder views");
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                });
                scope.spawn(|| {
                    start.wait();
                    while !stop.load(Ordering::Relaxed) {
                        black_box(volume::confirmed_absent(&missing, &volume));
                    }
                });
                start.wait();
                let mut hit = Vec::with_capacity(20_000);
                let mut miss = Vec::with_capacity(20_000);
                for i in 0..20_000 {
                    let index = i * 97;
                    let started = Instant::now();
                    self.resolve(index, true);
                    hit.push(started.elapsed());
                    let started = Instant::now();
                    self.resolve(index, false);
                    miss.push(started.elapsed());
                }
                stop.store(true, Ordering::Relaxed);
                hit.sort_unstable();
                miss.sort_unstable();
                (hit[19_799], miss[19_799])
            });
            let started = Instant::now();
            let loaded = load(&file).expect("read benchmark folder views");
            let bootstrap = started.elapsed();
            assert_eq!(loaded.len(), self.paths.len());
            (hit, miss, bootstrap)
        }
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;

    use super::*;

    fn record(volume: Option<volume::Volume>) -> Record {
        Record {
            view: FolderView {
                mode: Some(FileViewMode::LargeIcons),
                columns: Some(FileColumnSettings::default()),
                ..FolderView::default()
            },
            volume,
            revision: 0,
        }
    }

    #[test]
    fn folder_view_round_trips_native_paths_and_persists_resets() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("folder-views.json");
        #[cfg(unix)]
        let name = {
            use std::os::unix::ffi::OsStringExt;
            OsString::from_vec(vec![b'a', 0xff])
        };
        #[cfg(windows)]
        let name = {
            use std::os::windows::ffi::OsStringExt;
            OsString::from_wide(&[u16::from(b'a'), 0xd800])
        };
        let folder = temp.path().join(name);
        let writer = start_writer(file.clone(), load(&file).unwrap()).unwrap();
        let original = record(None);
        writer
            .send(WriteCommand::Replace(
                folder.clone(),
                Some(Box::new(original.clone())),
            ))
            .unwrap();
        shutdown(writer);
        let loaded = load(&file).unwrap();
        assert_eq!(loaded[&folder].view, original.view);

        let writer = start_writer(file.clone(), loaded).unwrap();
        writer.send(WriteCommand::Replace(folder, None)).unwrap();
        shutdown(writer);
        assert!(load(&file).unwrap().is_empty());
    }

    #[test]
    fn folder_view_writer_recovers_from_a_failed_save() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("config");
        fs::write(&parent, "temporarily prevents directory creation").unwrap();
        let file = parent.join("folder-views.json");
        let folder = temp.path().join("folder");
        let index = Index::from([(folder.clone(), record(None))]);
        assert!(!write_pending(&file, &index));
        let writer = start_writer(file.clone(), Index::new()).unwrap();
        writer
            .send(WriteCommand::Replace(
                folder.clone(),
                Some(Box::new(record(None))),
            ))
            .unwrap();
        fs::remove_file(&parent).unwrap();
        shutdown(writer);
        assert!(load(&file).unwrap().contains_key(&folder));
    }

    #[test]
    fn folder_view_preserves_corrupt_and_unsupported_files() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("folder-views.json");
        for contents in [
            r#"{"version":1,"entries":["#,
            r#"{"version":2,"entries":[]}"#,
        ] {
            fs::write(&file, contents).unwrap();
            let store = open(Some(file.clone()), true);
            assert!(store.index.is_empty());
            assert!(store.writer.is_none());
            assert_eq!(fs::read_to_string(&file).unwrap(), contents);
        }
    }

    #[test]
    fn folder_view_cleanup_prunes_deleted_moved_and_replaced_directories() {
        let temp = tempfile::tempdir().unwrap();
        let deleted = temp.path().join("parent").join("deleted");
        let moved = temp.path().join("before");
        let replaced = temp.path().join("replaced");
        let kept = temp.path().join("kept");
        let mut snapshot = Index::new();
        for folder in [&deleted, &moved, &replaced, &kept] {
            fs::create_dir_all(folder).unwrap();
            snapshot.insert(
                folder.clone(),
                record(Some(volume::for_path(folder).unwrap())),
            );
        }
        fs::remove_dir(&deleted).unwrap();
        fs::remove_dir(deleted.parent().unwrap()).unwrap();
        fs::rename(&moved, temp.path().join("after")).unwrap();
        fs::remove_dir(&replaced).unwrap();
        fs::write(&replaced, "file").unwrap();
        let mut stale: Vec<_> = stale_records(snapshot)
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        stale.sort();
        let mut expected = [deleted, moved, replaced];
        expected.sort();
        assert_eq!(stale, expected);
    }

    #[gpui::test]
    fn folder_view_cleanup_keeps_a_record_edited_after_the_snapshot(cx: &mut gpui::TestAppContext) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path();
        cx.update(|cx| {
            cx.set_global(FolderViews::default());
            edit(cx, path, |view| view.mode = Some(FileViewMode::LargeIcons));
            let revision = cx.global::<FolderViews>().get(path).unwrap().0;
            edit(cx, path, |view| view.mode = Some(FileViewMode::Details));
            prune(cx, vec![(path.to_path_buf(), revision)]);
            assert_eq!(
                cx.global::<FolderViews>().get(path).unwrap().1.mode,
                Some(FileViewMode::Details)
            );
            let revision = cx.global::<FolderViews>().get(path).unwrap().0;
            prune(cx, vec![(path.to_path_buf(), revision)]);
            assert!(cx.global::<FolderViews>().get(path).is_none());
        });
    }

    #[gpui::test]
    fn folder_view_edit_refreshes_the_saved_backing_volume(cx: &mut gpui::TestAppContext) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("folder");
        fs::create_dir(&path).unwrap();
        let current = volume::for_path(&path).unwrap();
        let mut previous = serde_json::to_value(&current).unwrap();
        previous["source"] = "previous-backing".into();
        let previous = serde_json::from_value(previous).unwrap();
        let index = Index::from([(path.clone(), record(Some(previous)))]);
        let writer = start_writer(temp.path().join("folder-views.json"), index.clone()).unwrap();
        cx.update(|cx| {
            cx.set_global(FolderViews {
                index,
                writer: Some(writer),
                ..FolderViews::default()
            });
            edit(cx, &path, |view| view.mode = Some(FileViewMode::Details));
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                cx.global::<FolderViews>().index[&path].volume,
                Some(current)
            );
            flush_on_quit(cx);
        });
    }

    fn shutdown(writer: Sender<WriteCommand>) {
        let (send, receive) = oneshot::channel();
        writer.send(WriteCommand::Shutdown(send)).unwrap();
        block_on(receive).unwrap();
    }
}
