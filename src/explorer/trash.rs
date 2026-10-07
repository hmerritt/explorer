//! System trash identities and operations. UI paths are never filesystem paths.
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::SystemTime,
};

use super::entry::FileEntry;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::time::UNIX_EPOCH;

pub(super) const ADDRESS: &str = "trash:///";
static OPERATIONS: Mutex<()> = Mutex::new(());
static REVISION: AtomicU64 = AtomicU64::new(0);
static SNAPSHOT: OnceLock<Mutex<BTreeMap<TrashItemId, TrashEntry>>> = OnceLock::new();

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub(super) struct TrashItemId(pub(super) String);

#[derive(Clone, Debug)]
pub(super) struct TrashEntry {
    pub(super) id: TrashItemId,
    pub(super) name: OsString,
    pub(super) original_path: Option<PathBuf>,
    pub(super) deleted: Option<SystemTime>,
    pub(super) size: Option<u64>,
    pub(super) directory: bool,
    payload: PathBuf,
    fingerprint: String,
    #[cfg(target_os = "macos")]
    original_volume: Option<(PathBuf, String)>,
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    native: trash::TrashItem,
}

pub(super) fn item_path(id: &TrashItemId) -> PathBuf {
    PathBuf::from(format!("{ADDRESS}items/{}", id.0))
}

impl TrashEntry {
    pub(super) fn path(&self) -> PathBuf {
        PathBuf::from(format!("{ADDRESS}items/{}", self.id.0))
    }
    pub(super) fn file_entry(&self) -> FileEntry {
        let mut entry = FileEntry::from_provider(
            self.path(),
            self.name.to_string_lossy().into_owned(),
            self.directory,
            self.size,
            self.deleted,
        );
        if self.directory {
            entry.set_folder_size(self.size);
        }
        entry
    }
    pub(super) fn original_location(&self) -> String {
        self.original_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "Unknown".into())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RecoveryChoice {
    Replace,
    Skip,
    KeepBoth,
}

#[derive(Clone, Debug)]
pub(super) struct RecoveryRequest {
    pub(super) ids: Vec<TrashItemId>,
    pub(super) directory: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub(super) struct RecoveredPath {
    pub(super) path: PathBuf,
    fingerprint: String,
    original_path: Option<PathBuf>,
    replacement: Option<ReplacementBackup>,
    returned_to_bin: bool,
    #[cfg(target_os = "macos")]
    original_volume: Option<(PathBuf, String)>,
}

#[derive(Clone, Debug)]
pub(super) struct ReplacementBackup {
    pub(super) path: PathBuf,
    backup: Arc<BackupContents>,
    previous_identity: String,
}

#[derive(Debug)]
struct BackupContents {
    path: PathBuf,
    fingerprint: String,
}
impl Drop for BackupContents {
    fn drop(&mut self) {
        let _ = remove_payload(&self.path);
        if let Some(parent) = self.path.parent() {
            let _ = fs::remove_dir(parent);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct RecoveryUndo {
    pub(super) paths: Vec<RecoveredPath>,
    created_directories: Vec<CreatedDirectory>,
}

#[derive(Clone, Debug)]
struct CreatedDirectory {
    path: PathBuf,
    fingerprint: String,
}

#[derive(Default, Debug)]
pub(super) struct BatchResult {
    pub(super) completed: Vec<TrashItemId>,
    pub(super) skipped: Vec<TrashItemId>,
    pub(super) failures: Vec<String>,
    pub(super) failed: Vec<TrashItemId>,
    pub(super) cancelled_items: Vec<TrashItemId>,
    pub(super) cancelled: bool,
    pub(super) undo: RecoveryUndo,
}

pub(super) fn label() -> &'static str {
    if cfg!(target_os = "windows") {
        "Recycle Bin"
    } else {
        "Trash"
    }
}
pub(super) fn root() -> PathBuf {
    PathBuf::from(ADDRESS)
}
pub(super) fn is_root(path: &Path) -> bool {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        == "trash:"
}
pub(super) fn is_item(path: &Path) -> bool {
    item_id(path).is_some()
}
pub(super) fn item_id(path: &Path) -> Option<TrashItemId> {
    let value = path.to_string_lossy().replace('\\', "/");
    let id = value.strip_prefix("trash:///items/")?;
    (id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit())).then(|| TrashItemId(id.into()))
}
pub(super) fn revision() -> u64 {
    REVISION.load(Ordering::Acquire)
}
fn changed() {
    REVISION.fetch_add(1, Ordering::Release);
}
fn snapshot() -> &'static Mutex<BTreeMap<TrashItemId, TrashEntry>> {
    SNAPSHOT.get_or_init(Default::default)
}
pub(super) fn cached(path: &Path) -> Option<TrashEntry> {
    snapshot().lock().unwrap().get(&item_id(path)?).cloned()
}
pub(super) fn available(id: &TrashItemId) -> bool {
    snapshot().lock().unwrap().contains_key(id)
}
pub(super) fn ids(paths: &[PathBuf]) -> Vec<TrashItemId> {
    paths.iter().filter_map(|p| item_id(p)).collect()
}

pub(super) fn list() -> Result<Vec<TrashEntry>, String> {
    let _guard = OPERATIONS.lock().unwrap_or_else(|e| e.into_inner());
    native_worker(list_inner)
}
fn list_inner() -> Result<Vec<TrashEntry>, String> {
    let entries = platform_list()?;
    let refreshed: BTreeMap<_, _> = entries.iter().map(|e| (e.id.clone(), e.clone())).collect();
    let mut current = snapshot().lock().unwrap();
    if current.keys().ne(refreshed.keys()) {
        changed();
    }
    *current = refreshed;
    Ok(entries)
}
pub(super) fn listing_is_current(entries: &[FileEntry]) -> bool {
    let identities: BTreeSet<_> = entries
        .iter()
        .filter_map(|entry| item_id(&entry.path))
        .collect();
    let current = snapshot().lock().unwrap();
    identities.len() == entries.len() && identities.iter().eq(current.keys())
}
pub(super) fn list_entries() -> io::Result<Vec<FileEntry>> {
    list()
        .map(|items| items.iter().map(TrashEntry::file_entry).collect())
        .map_err(io::Error::other)
}

#[derive(Default)]
pub(super) struct TrashOutcome {
    pub(super) ids: Vec<TrashItemId>,
    pub(super) paths: Vec<PathBuf>,
    pub(super) failures: Vec<String>,
}

#[cfg(test)]
pub(super) fn trash_paths(paths: &[PathBuf]) -> Result<Vec<TrashItemId>, String> {
    let outcome = trash_paths_batch(paths)?;
    if outcome.failures.is_empty() {
        Ok(outcome.ids)
    } else {
        Err(outcome.failures.join("\n"))
    }
}

pub(super) fn trash_paths_batch(paths: &[PathBuf]) -> Result<TrashOutcome, String> {
    let _guard = OPERATIONS.lock().unwrap_or_else(|e| e.into_inner());
    native_worker(|| trash_paths_inner(paths))
        .map_err(|error| format!("Could not move items to {}: {error}", label()))
}

fn trash_paths_inner(paths: &[PathBuf]) -> Result<TrashOutcome, String> {
    let before: BTreeSet<_> = list_inner()?.into_iter().map(|e| e.id).collect();
    let mut outcome = TrashOutcome::default();
    for (index, path) in paths.iter().enumerate() {
        if crate::explorer::operation_control::current_checkpoint() {
            break;
        }
        crate::explorer::operation_control::report_items(
            index,
            paths.len(),
            &path.display().to_string(),
        );
        if is_root(path) || is_item(path) {
            outcome
                .failures
                .push("An item already in the bin cannot be trashed again.".into());
            continue;
        }
        match platform_trash(path) {
            Ok(()) => outcome.paths.push(path.clone()),
            Err(error) => outcome.failures.push(format!(
                "Could not move {} to {}: {error}",
                path.display(),
                label()
            )),
        }
    }
    changed();
    let entries = match list_inner() {
        Ok(entries) => entries,
        Err(error) => {
            outcome.failures.push(format!(
                "Items were moved, but their recovery identities could not be refreshed: {error}"
            ));
            return Ok(outcome);
        }
    };
    outcome.ids = entries
        .into_iter()
        .filter(|e| {
            !before.contains(&e.id)
                && e.original_path
                    .as_ref()
                    .is_some_and(|p| outcome.paths.iter().any(|original| same_path(original, p)))
        })
        .map(|e| e.id)
        .collect();
    Ok(outcome)
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = std::path::absolute(left).unwrap_or_else(|_| left.to_owned());
    let right = std::path::absolute(right).unwrap_or_else(|_| right.to_owned());
    if cfg!(target_os = "windows") {
        left.to_string_lossy()
            .trim_start_matches(r"\\?\")
            .eq_ignore_ascii_case(right.to_string_lossy().trim_start_matches(r"\\?\"))
    } else {
        left == right
    }
}

fn remote_destination(path: &Path) -> bool {
    #[cfg(windows)]
    if let Some(std::path::Component::Prefix(prefix)) = path.components().next() {
        if let std::path::Prefix::VerbatimDisk(drive) = prefix.kind() {
            let mut ordinary = PathBuf::from(format!("{}:\\", char::from(drive)));
            ordinary.extend(path.components().skip(2));
            return super::filesystem::path_is_remote_drive(&ordinary);
        }
    }
    super::filesystem::path_is_remote_drive(path)
}
pub(super) fn local_destination(path: &Path) -> bool {
    if remote_destination(path)
        || fs::canonicalize(path).is_ok_and(|resolved| remote_destination(&resolved))
    {
        return false;
    }
    !is_root(path)
        && !is_item(path)
        && !super::remote_fs::is_remote(path)
        && !super::portable_devices::is_portable_path(path)
        && !super::archive_fs::is_archive_path(path)
        && path.is_dir()
}

pub(super) fn has_conflicts(request: &RecoveryRequest) -> Result<bool, String> {
    let entries = list()?;
    let items: BTreeMap<_, _> = entries.into_iter().map(|e| (e.id.clone(), e)).collect();
    let mut destinations = BTreeSet::new();
    for id in &request.ids {
        let Some(item) = items.get(id) else {
            continue;
        };
        let Ok(target) = destination(item, request) else {
            continue;
        };
        if fs::symlink_metadata(&target).is_ok() || !destinations.insert(target) {
            return Ok(true);
        }
    }
    Ok(false)
}
fn destination(item: &TrashEntry, request: &RecoveryRequest) -> Result<PathBuf, String> {
    if let Some(directory) = &request.directory {
        if !local_destination(directory) {
            return Err("Choose a writable local folder to restore into.".into());
        }
        return Ok(directory.join(&item.name));
    }
    #[cfg(target_os = "macos")]
    if let Some(original) = &item.original_path {
        if let Ok(relative) = original.strip_prefix("/Volumes") {
            if let Some(volume) = relative.components().next() {
                if !Path::new("/Volumes").join(volume).is_dir() {
                    return Err("The original volume is unavailable. Use Restore to… to choose another folder.".into());
                }
            }
        }
    }
    let original = item.original_path.clone().ok_or_else(|| {
        format!(
            "The original location of {} is unknown. Use Restore to… to choose a folder.",
            item.name.to_string_lossy()
        )
    })?;
    let parent = original.ancestors().skip(1).find(|parent| parent.is_dir());
    if remote_destination(&original)
        || parent.is_some_and(|parent| !local_destination(parent))
        || super::remote_fs::is_remote(&original)
        || super::portable_devices::is_portable_path(&original)
    {
        return Err("Recovery to remote servers or portable devices is unavailable. Use Restore to… to choose a local folder.".into());
    }
    Ok(original)
}

type PurgeProgress = Arc<dyn Fn(usize, usize, &str) + Send + Sync>;

#[derive(Clone)]
struct PurgeContext {
    cancel: Arc<AtomicBool>,
    progress: PurgeProgress,
    done: Arc<AtomicUsize>,
    total: usize,
}

impl PurgeContext {
    fn report(&self, name: &str) {
        (self.progress)(self.done.load(Ordering::Relaxed), self.total, name);
    }

    fn processed(&self, name: &str) {
        self.done.fetch_add(1, Ordering::Relaxed);
        self.report(name);
    }
}

impl BatchResult {
    fn purge_failed(&mut self, item: &TrashEntry, error: String) {
        self.failed.push(item.id.clone());
        self.failures
            .push(format!("{}: {error}", item.name.to_string_lossy()));
    }
}

trait TrashBackend {
    fn list(&self) -> Result<Vec<TrashEntry>, String>;
    fn purge(&self, item: &TrashEntry) -> Result<(), String>;
    fn purge_batch(&self, items: Vec<TrashEntry>, context: PurgeContext) -> BatchResult {
        let mut result = BatchResult::default();
        for (index, item) in items.iter().enumerate() {
            if crate::explorer::operation_control::cancelled(&context.cancel) {
                result.cancelled = true;
                result
                    .cancelled_items
                    .extend(items[index..].iter().map(|item| item.id.clone()));
                break;
            }
            context.report(&item.name.to_string_lossy());
            match self.purge(item) {
                Ok(()) => result.completed.push(item.id.clone()),
                Err(error) => result.purge_failed(item, error),
            }
            context.processed(&item.name.to_string_lossy());
        }
        result
    }
    fn trash(&self, path: &Path) -> Result<(), String>;
    fn update_origin(&self, _: &RecoveredPath) -> Result<(), String> {
        Ok(())
    }
    fn recover(
        &self,
        item: &TrashEntry,
        target: &Path,
        choice: RecoveryChoice,
        cancel: &AtomicBool,
        undo: &mut RecoveryUndo,
    ) -> Result<bool, String> {
        let complete = recover_node(
            &item.payload,
            target,
            choice,
            cancel,
            undo,
            item.original_path.as_deref(),
        )?;
        if complete {
            self.purge(item)?;
        }
        Ok(complete)
    }
}
struct NativeBackend;
impl TrashBackend for NativeBackend {
    fn list(&self) -> Result<Vec<TrashEntry>, String> {
        list_inner()
    }
    fn purge(&self, item: &TrashEntry) -> Result<(), String> {
        platform_purge(item)
    }
    #[cfg(target_os = "windows")]
    fn purge_batch(&self, items: Vec<TrashEntry>, context: PurgeContext) -> BatchResult {
        windows_backend::shell_delete_batch(items, context)
    }
    fn trash(&self, path: &Path) -> Result<(), String> {
        let outcome = trash_paths_inner(&[path.to_owned()])?;
        if outcome.failures.is_empty() {
            Ok(())
        } else {
            Err(outcome.failures.join("\n"))
        }
    }
    fn update_origin(&self, recovered: &RecoveredPath) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            if let Some(original) = &recovered.original_path {
                update_macos_origin(&recovered.path, original, recovered.original_volume.clone())?;
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = &recovered.original_path;
        }
        Ok(())
    }
    fn recover(
        &self,
        item: &TrashEntry,
        target: &Path,
        choice: RecoveryChoice,
        cancel: &AtomicBool,
        undo: &mut RecoveryUndo,
    ) -> Result<bool, String> {
        #[cfg(target_os = "linux")]
        if item.original_path.as_deref() == Some(target)
            && fs::symlink_metadata(target).is_err()
            && !crate::explorer::operation_control::cancelled(&cancel)
            && fs::symlink_metadata(&item.payload)
                .is_ok_and(|metadata| !metadata.file_type().is_symlink())
            && target.parent().is_some_and(|parent| {
                use std::os::unix::fs::MetadataExt;
                fs::metadata(parent)
                    .ok()
                    .zip(fs::symlink_metadata(&item.payload).ok())
                    .is_some_and(|(destination, source)| destination.dev() == source.dev())
            })
        {
            let info_identity = fingerprint(Path::new(&item.native.id)).ok();
            let pinned_source = pin_inode(&item.payload).map_err(|e| e.to_string())?;
            let restored = trash::os_limited::restore_all([item.native.clone()]);
            if let Err(error) = restored {
                // A rename can complete even if removing .trashinfo fails.
                // Report and journal the actual recovery, never lose its undo.
                if fs::symlink_metadata(&item.payload).is_ok()
                    || !is_pinned_inode(&pinned_source, target)
                {
                    return Err(error.to_string());
                }
                if fingerprint(Path::new(&item.native.id)).ok() == info_identity {
                    let _ = fs::remove_file(&item.native.id);
                }
            }
            if fs::symlink_metadata(&item.payload).is_ok() {
                return Err("Restore did not remove the selected bin item.".into());
            }
            undo.paths.push(RecoveredPath {
                path: target.to_owned(),
                fingerprint: fingerprint(target).map_err(|e| e.to_string())?,
                original_path: item.original_path.clone(),
                replacement: None,
                returned_to_bin: false,
            });
            return Ok(true);
        }
        #[cfg(target_os = "windows")]
        let complete = recover_node_using(
            &item.payload,
            target,
            choice,
            cancel,
            undo,
            item.original_path.as_deref(),
            &|source, staged, cancel| {
                if crate::explorer::operation_control::cancelled(&cancel) {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "Recovery cancelled.",
                    ));
                }
                let parsing_name = if source == item.payload {
                    item.native.id.as_os_str()
                } else {
                    source.as_os_str()
                };
                windows_backend::shell_transfer(parsing_name, staged, false)
                    .map_err(io::Error::other)?;
                verify_tree(source, staged, cancel)
            },
            &|source, target| {
                windows_backend::shell_transfer(source.as_os_str(), target, true)
                    .map_err(io::Error::other)
            },
        )?;
        #[cfg(target_os = "macos")]
        let undo_start = undo.paths.len();
        #[cfg(not(target_os = "windows"))]
        let complete = recover_node(
            &item.payload,
            target,
            choice,
            cancel,
            undo,
            item.original_path.as_deref(),
        )?;
        #[cfg(target_os = "macos")]
        for recovered in &mut undo.paths[undo_start..] {
            recovered.original_volume = item.original_volume.clone();
        }
        if complete {
            self.purge(item)?;
        }
        Ok(complete)
    }
}

pub(super) fn recover(
    request: RecoveryRequest,
    choice: RecoveryChoice,
    cancel: &AtomicBool,
    progress: impl Fn(usize, usize, &str) + Sync + Send,
) -> BatchResult {
    let _guard = OPERATIONS.lock().unwrap_or_else(|e| e.into_inner());
    let failed = request.ids.clone();
    let result = native_worker(|| {
        Ok(recover_with(
            &NativeBackend,
            request,
            choice,
            cancel,
            progress,
        ))
    });
    changed();
    result.unwrap_or_else(|error| BatchResult {
        failures: vec![error],
        failed,
        ..Default::default()
    })
}

fn recover_with(
    backend: &impl TrashBackend,
    request: RecoveryRequest,
    choice: RecoveryChoice,
    cancel: &AtomicBool,
    progress: impl Fn(usize, usize, &str),
) -> BatchResult {
    let mut result = BatchResult::default();
    for (index, id) in request.ids.iter().enumerate() {
        if crate::explorer::operation_control::cancelled(&cancel) {
            result.cancelled = true;
            result
                .cancelled_items
                .extend(request.ids[index..].iter().cloned());
            break;
        }
        let items = match backend.list() {
            Ok(items) => items,
            Err(error) => {
                result.failures.push(error);
                result.failed.extend(request.ids[index..].iter().cloned());
                break;
            }
        };
        let Some(item) = items.into_iter().find(|e| &e.id == id) else {
            result.skipped.push(id.clone());
            continue;
        };
        progress(index, request.ids.len(), &item.name.to_string_lossy());
        let attempt = (|| {
            let target = destination(&item, &request)?;
            #[cfg(target_os = "macos")]
            if request.directory.is_none() {
                macos::validate_original_volume(&item)?;
            }
            let parent = target.parent().ok_or("Invalid restore destination.")?;
            create_parents(parent, &mut result.undo)?;
            let target =
                if choice == RecoveryChoice::KeepBoth && fs::symlink_metadata(&target).is_ok() {
                    keep_both_path_for(&target, item.directory)
                } else {
                    target
                };
            if fingerprint(&item.payload).map_err(|e| e.to_string())? != item.fingerprint {
                return Err("The selected bin item changed. Refresh and try again.".into());
            }
            let complete = backend.recover(&item, &target, choice, cancel, &mut result.undo)?;
            if complete {
                result.completed.push(id.clone());
            } else if crate::explorer::operation_control::cancelled(&cancel) {
                result.cancelled_items.push(id.clone());
            } else {
                result.skipped.push(id.clone());
            }
            Ok::<_, String>(())
        })();
        if let Err(error) = attempt {
            if crate::explorer::operation_control::cancelled(&cancel) {
                result.cancelled_items.push(id.clone());
            } else {
                result.failed.push(id.clone());
                result
                    .failures
                    .push(format!("{}: {error}", item.name.to_string_lossy()));
            }
        }
        progress(index + 1, request.ids.len(), &item.name.to_string_lossy());
    }
    result.cancelled |= crate::explorer::operation_control::cancelled(&cancel);
    let _ = backend.list();
    result
}

pub(super) fn purge(
    ids: Vec<TrashItemId>,
    cancel: Arc<AtomicBool>,
    progress: impl Fn(usize, usize, &str) + Sync + Send + 'static,
) -> BatchResult {
    let _guard = OPERATIONS.lock().unwrap_or_else(|e| e.into_inner());
    let failed = ids.clone();
    let result = native_worker(|| Ok(purge_with(&NativeBackend, ids, cancel, progress)));
    changed();
    result.unwrap_or_else(|error| BatchResult {
        failures: vec![error],
        failed,
        ..Default::default()
    })
}
fn purge_with(
    backend: &impl TrashBackend,
    ids: Vec<TrashItemId>,
    cancel: Arc<AtomicBool>,
    progress: impl Fn(usize, usize, &str) + Sync + Send + 'static,
) -> BatchResult {
    let mut result = BatchResult::default();
    let context = PurgeContext {
        cancel,
        progress: Arc::new(progress),
        done: Arc::new(AtomicUsize::new(0)),
        total: ids.len(),
    };
    context.report("");
    let mut entries: BTreeMap<_, _> = match backend.list() {
        Ok(items) => items
            .into_iter()
            .map(|item| (item.id.clone(), item))
            .collect(),
        Err(error) => {
            result.failures.push(error);
            result.failed = ids;
            let _ = backend.list();
            return result;
        }
    };
    let mut resolved = Vec::with_capacity(ids.len());
    for (index, id) in ids.iter().enumerate() {
        if crate::explorer::operation_control::cancelled(&context.cancel) {
            result.cancelled = true;
            result
                .cancelled_items
                .extend(resolved.iter().map(|item: &TrashEntry| item.id.clone()));
            result.cancelled_items.extend(ids[index..].iter().cloned());
            resolved.clear();
            break;
        }
        let Some(item) = entries.remove(id) else {
            result.skipped.push(id.clone());
            context.processed("");
            continue;
        };
        resolved.push(item);
    }
    let batch = backend.purge_batch(resolved, context.clone());
    result.completed = batch.completed;
    result.failed = batch.failed;
    result.failures.extend(batch.failures);
    result.cancelled_items.extend(batch.cancelled_items);
    result.cancelled |=
        batch.cancelled || crate::explorer::operation_control::cancelled(&context.cancel);
    let _ = backend.list();
    result
}

fn create_parents(path: &Path, undo: &mut RecoveryUndo) -> Result<(), String> {
    if path.is_dir() {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or("The destination volume is unavailable.")?;
    create_parents(parent, undo)?;
    fs::create_dir(path).map_err(|e| e.to_string())?;
    undo.created_directories.push(CreatedDirectory {
        path: path.to_owned(),
        fingerprint: fingerprint(path).map_err(|e| e.to_string())?,
    });
    Ok(())
}

/// Recover leaves individually when merging folders. Skipped leaves and their
/// metadata remain in the bin; source symlinks are never traversed.
fn recover_node(
    source: &Path,
    target: &Path,
    choice: RecoveryChoice,
    cancel: &AtomicBool,
    undo: &mut RecoveryUndo,
    original: Option<&Path>,
) -> Result<bool, String> {
    recover_node_using(
        source,
        target,
        choice,
        cancel,
        undo,
        original,
        &copy_verified,
        &rename_exclusive,
    )
}

fn recover_node_using(
    source: &Path,
    target: &Path,
    choice: RecoveryChoice,
    cancel: &AtomicBool,
    undo: &mut RecoveryUndo,
    original: Option<&Path>,
    copy: &impl Fn(&Path, &Path, &AtomicBool) -> io::Result<()>,
    commit: &impl Fn(&Path, &Path) -> io::Result<()>,
) -> Result<bool, String> {
    if crate::explorer::operation_control::cancelled(&cancel) {
        return Ok(false);
    }
    let metadata = fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    let existing = fs::symlink_metadata(target).ok();
    let existing_identity = existing
        .as_ref()
        .map(|_| fingerprint(target))
        .transpose()
        .map_err(|e| e.to_string())?;
    if metadata.is_dir()
        && existing
            .as_ref()
            .is_some_and(|m| m.is_dir() && !m.file_type().is_symlink())
    {
        let mut complete = true;
        for child in fs::read_dir(source).map_err(|e| e.to_string())? {
            let child = child.map_err(|e| e.to_string())?;
            let name = child.file_name();
            let child_original = original.map(|p| p.join(&name));
            if recover_node_using(
                &child.path(),
                &target.join(name),
                choice,
                cancel,
                undo,
                child_original.as_deref(),
                copy,
                commit,
            )? {
                remove_payload(&child.path()).map_err(|e| e.to_string())?;
            } else {
                complete = false;
            }
        }
        return Ok(complete);
    }
    if existing.is_some() && choice == RecoveryChoice::Skip {
        return Ok(false);
    }
    let target = if existing.is_some() && choice == RecoveryChoice::KeepBoth {
        keep_both_path_for(target, metadata.is_dir())
    } else {
        target.to_owned()
    };
    // Stage and verify before replacing any destination or deleting any source.
    let parent = target.parent().ok_or("Invalid destination.")?;
    let stage = tempfile::Builder::new()
        .prefix(".explorer-restore-")
        .tempdir_in(parent)
        .map_err(|e| e.to_string())?;
    let staged = stage.path().join("payload");
    copy(source, &staged, cancel).map_err(|e| e.to_string())?;
    verify_tree(source, &staged, cancel).map_err(|e| e.to_string())?;
    #[cfg(not(target_os = "linux"))]
    let staged_identity = fingerprint(&staged).map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    let pinned_stage = pin_inode(&staged).map_err(|e| e.to_string())?;
    if fingerprint(&target).ok() != existing_identity {
        return Err("The destination changed during recovery; source retained.".into());
    }
    let backup = if fs::symlink_metadata(&target).is_ok() {
        if choice != RecoveryChoice::Replace {
            return Err("The destination changed. Try restoring again.".into());
        }
        let holder = tempfile::Builder::new()
            .prefix(".explorer-restore-undo-")
            .tempdir_in(parent)
            .map_err(|e| e.to_string())?;
        let backup = holder.path().join("payload");
        rename_exclusive(&target, &backup).map_err(|e| e.to_string())?;
        let _ = holder.keep();
        Some(backup)
    } else {
        None
    };
    let committed = commit(&staged, &target).and_then(|()| {
        let actual = fingerprint(&target)?;
        #[cfg(target_os = "linux")]
        let matches = is_pinned_inode(&pinned_stage, &target);
        #[cfg(not(target_os = "linux"))]
        let matches = actual == staged_identity;
        if matches {
            Ok(actual)
        } else {
            Err(io::Error::other(
                "The destination was replaced during recovery; source retained.",
            ))
        }
    });
    let committed_identity = match committed {
        Ok(identity) => identity,
        Err(error) => {
            if let Some(backup) = &backup {
                if let Err(rollback) = rename_exclusive(backup, &target) {
                    return Err(format!(
                        "{error}. The previous destination remains safely stored at {} because it could not be restored: {rollback}",
                        backup.display()
                    ));
                }
                if let Some(parent) = backup.parent() {
                    let _ = fs::remove_dir(parent);
                }
            }
            return Err(error.to_string());
        }
    };
    let replacement = backup.map(|backup| {
        let identity = fingerprint(&backup).unwrap_or_default();
        ReplacementBackup {
            path: target.clone(),
            previous_identity: existing_identity.clone().unwrap_or_default(),
            backup: Arc::new(BackupContents {
                path: backup,
                fingerprint: identity,
            }),
        }
    });
    undo.paths.push(RecoveredPath {
        fingerprint: committed_identity,
        path: target,
        original_path: original.map(Path::to_owned),
        replacement,
        returned_to_bin: false,
        #[cfg(target_os = "macos")]
        original_volume: None,
    });
    Ok(true)
}

fn copy_verified(source: &Path, target: &Path, cancel: &AtomicBool) -> io::Result<()> {
    let _boundaries = crate::explorer::operation_control::boundaries_available();
    use std::io::{Read, Write};
    if crate::explorer::operation_control::cancelled(&cancel) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Recovery cancelled.",
        ));
    }
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        let link = fs::read_link(source)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(link, target)?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            use std::os::windows::fs::{symlink_dir, symlink_file};
            if metadata.file_attributes() & 0x10 != 0 {
                symlink_dir(link, target)?;
            } else {
                symlink_file(link, target)?;
            }
        }
    } else if metadata.is_dir() {
        fs::create_dir(target)?;
        for child in fs::read_dir(source)? {
            let child = child?;
            copy_verified(&child.path(), &target.join(child.file_name()), cancel)?;
        }
        fs::set_permissions(target, metadata.permissions())?;
    } else if metadata.is_file() {
        let initial = fingerprint(source)?;
        let mut input = fs::File::open(source)?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            if crate::explorer::operation_control::cancelled(&cancel) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Recovery cancelled.",
                ));
            }
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count])?;
            hash.update(&buffer[..count]);
        }
        output.sync_all()?;
        drop(output);
        let mut actual = Sha256::new();
        let mut output = fs::File::open(target)?;
        loop {
            if crate::explorer::operation_control::cancelled(cancel) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "Recovery cancelled.",
                ));
            }
            let count = output.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            actual.update(&buffer[..count]);
        }
        if hash.finalize() != actual.finalize() || fingerprint(source)? != initial {
            return Err(io::Error::other(
                "Recovery verification failed; source retained.",
            ));
        }
        fs::set_permissions(target, metadata.permissions())?;
        filetime::set_file_times(
            target,
            filetime::FileTime::from_last_access_time(&metadata),
            filetime::FileTime::from_last_modification_time(&metadata),
        )?;
    } else {
        return Err(io::Error::other(
            "This filesystem object cannot be recovered.",
        ));
    }
    Ok(())
}

fn remove_payload(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn verify_tree(source: &Path, target: &Path, cancel: &AtomicBool) -> io::Result<()> {
    let _boundaries = crate::explorer::operation_control::boundaries_available();
    use std::io::Read;
    if crate::explorer::operation_control::cancelled(&cancel) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Recovery cancelled.",
        ));
    }
    let left = fs::symlink_metadata(source)?;
    let right = fs::symlink_metadata(target)?;
    if left.file_type() != right.file_type() {
        return Err(io::Error::other("The source changed during recovery."));
    }
    if left.file_type().is_symlink() {
        if fs::read_link(source)? != fs::read_link(target)? {
            return Err(io::Error::other("Link verification failed."));
        }
    } else if left.is_dir() {
        let names = |path: &Path| -> io::Result<BTreeSet<OsString>> {
            fs::read_dir(path)?
                .map(|entry| entry.map(|e| e.file_name()))
                .collect()
        };
        let source_names = names(source)?;
        if source_names != names(target)? {
            return Err(io::Error::other("The folder changed during recovery."));
        }
        for name in source_names {
            verify_tree(&source.join(&name), &target.join(name), cancel)?;
        }
    } else {
        let hash = |path: &Path| -> io::Result<_> {
            let mut input = fs::File::open(path)?;
            let mut hash = Sha256::new();
            let mut buffer = vec![0u8; 1024 * 1024];
            loop {
                if crate::explorer::operation_control::cancelled(&cancel) {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "Recovery cancelled.",
                    ));
                }
                let count = input.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hash.update(&buffer[..count]);
            }
            Ok(hash.finalize())
        };
        if hash(source)? != hash(target)? {
            return Err(io::Error::other("The file changed during recovery."));
        }
    }
    Ok(())
}

#[cfg(test)]
fn keep_both_path(path: &Path) -> PathBuf {
    keep_both_path_for(path, path.is_dir())
}
fn keep_both_path_for(path: &Path, directory: bool) -> PathBuf {
    let stem = if directory {
        path.file_name()
    } else {
        path.file_stem()
    }
    .unwrap_or_default();
    let extension = if directory { None } else { path.extension() };
    for number in 2u64.. {
        let mut name = stem.to_os_string();
        name.push(format!(" ({number})"));
        if let Some(extension) = extension {
            name.push(".");
            name.push(extension);
        }
        let candidate = path.with_file_name(name);
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
    unreachable!()
}

#[cfg(target_os = "linux")]
fn rename_exclusive(source: &Path, destination: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let source = CString::new(source.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let destination = CString::new(destination.as_os_str().as_bytes()).map_err(io::Error::other)?;
    // SAFETY: both paths are valid nul-terminated strings; flags forbid replacement.
    if unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
#[cfg(target_os = "macos")]
fn rename_exclusive(source: &Path, destination: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let source = CString::new(source.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let destination = CString::new(destination.as_os_str().as_bytes()).map_err(io::Error::other)?;
    // SAFETY: both paths are valid nul-terminated strings.
    if unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
#[cfg(target_os = "windows")]
fn rename_exclusive(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(target_os = "linux")]
fn pin_inode(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    // Keep the inode alive across rename, including links and directories. This
    // also works on filesystems without birth time, where rename changes ctime.
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_NOFOLLOW)
        .open(path)
}
#[cfg(target_os = "linux")]
fn is_pinned_inode(pinned: &fs::File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    pinned
        .metadata()
        .ok()
        .zip(fs::symlink_metadata(path).ok())
        .is_some_and(|(left, right)| left.dev() == right.dev() && left.ino() == right.ino())
}

pub(super) fn fingerprint(path: &Path) -> io::Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        #[cfg(target_os = "macos")]
        let birth = {
            use std::os::macos::fs::MetadataExt;
            format!(
                "{}:{}",
                metadata.st_birthtime(),
                metadata.st_birthtime_nsec()
            )
        };
        #[cfg(target_os = "linux")]
        let birth = {
            use std::{ffi::CString, os::unix::ffi::OsStrExt};
            let name = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)?;
            let mut value = std::mem::MaybeUninit::<libc::statx>::uninit();
            // SAFETY: statx writes the output on success and does not follow source symlinks.
            if unsafe {
                libc::statx(
                    libc::AT_FDCWD,
                    name.as_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                    libc::STATX_BTIME,
                    value.as_mut_ptr(),
                )
            } == 0
            {
                let value = unsafe { value.assume_init() };
                if value.stx_mask & libc::STATX_BTIME != 0 {
                    format!("{}:{}", value.stx_btime.tv_sec, value.stx_btime.tv_nsec)
                } else {
                    format!("{}:{}", metadata.ctime(), metadata.ctime_nsec())
                }
            } else {
                format!("{}:{}", metadata.ctime(), metadata.ctime_nsec())
            }
        };
        Ok(format!("{}:{}:{birth}", metadata.dev(), metadata.ino()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows::Win32::{
            Foundation::HANDLE,
            Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle},
        };
        let file = fs::OpenOptions::new()
            .access_mode(0)
            .custom_flags(0x02000000 | 0x00200000)
            .open(path)?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the handle remains open and the output lives through the call.
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
            .map_err(io::Error::other)?;
        let _ = metadata;
        Ok(format!(
            "{}:{}:{}:{}:{}",
            info.dwVolumeSerialNumber,
            info.nFileIndexHigh,
            info.nFileIndexLow,
            info.ftCreationTime.dwHighDateTime,
            info.ftCreationTime.dwLowDateTime
        ))
    }
}

pub(super) fn undo_recovery(undo: &mut RecoveryUndo) -> Result<(), String> {
    let _guard = OPERATIONS.lock().unwrap_or_else(|e| e.into_inner());
    let result = native_worker(|| undo_recovery_with(&NativeBackend, undo));
    changed();
    result
}
fn undo_recovery_with(backend: &impl TrashBackend, undo: &mut RecoveryUndo) -> Result<(), String> {
    // Only the last recovered version of each destination is currently visible.
    // Earlier versions live in replacement backups and are unwound in reverse.
    let mut seen = Vec::<PathBuf>::new();
    for recovered in undo.paths.iter().rev() {
        let first = !seen.iter().any(|path| same_path(path, &recovered.path));
        seen.push(recovered.path.clone());
        if first
            && !recovered.returned_to_bin
            && fingerprint(&recovered.path).ok().as_ref() != Some(&recovered.fingerprint)
        {
            return Err(format!(
                "{} changed or was replaced; recovery cannot be undone.",
                recovered.path.display()
            ));
        }
    }
    while let Some(recovered) = undo.paths.last_mut() {
        if !recovered.returned_to_bin {
            if fingerprint(&recovered.path).ok().as_ref() != Some(&recovered.fingerprint) {
                return Err(format!(
                    "{} changed or was replaced; recovery cannot be undone.",
                    recovered.path.display()
                ));
            }
            backend.trash(&recovered.path)?;
            recovered.returned_to_bin = true;
        }
        backend.update_origin(recovered)?;
        if let Some(backup) = &recovered.replacement {
            if fingerprint(&backup.backup.path).ok().as_ref() != Some(&backup.backup.fingerprint) {
                return Err(
                    "The replacement backup changed; it cannot be restored automatically.".into(),
                );
            }
            rename_exclusive(&backup.backup.path, &backup.path).map_err(|e| e.to_string())?;
        }
        let finished = undo.paths.pop().unwrap();
        if let Some(backup) = finished.replacement {
            if let Some(previous) = undo.paths.iter_mut().rev().find(|previous| {
                same_path(&previous.path, &backup.path)
                    && previous.fingerprint == backup.previous_identity
            }) {
                // Our own backup rename can change ctime on older Linux filesystems.
                previous.fingerprint = fingerprint(&backup.path).map_err(|e| e.to_string())?;
            }
        }
    }
    for directory in undo.created_directories.iter().rev() {
        if fingerprint(&directory.path).ok().as_ref() == Some(&directory.fingerprint) {
            let _ = fs::remove_dir(&directory.path);
        }
    }
    undo.created_directories.clear();
    Ok(())
}
pub(super) fn cleanup_undo(undo: RecoveryUndo) {
    drop(undo);
}

#[cfg(target_os = "windows")]
fn native_worker<T: Send>(work: impl FnOnce() -> Result<T, String> + Send) -> Result<T, String> {
    let control = crate::explorer::operation_control::current();
    let _suspended = crate::explorer::operation_control::suspend_current();
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let _worker = control.as_ref().map(|control| control.enter());
                use windows::Win32::System::Com::{
                    COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize,
                };
                // SAFETY: a fresh thread owns this apartment and releases it after work.
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
                    .ok()
                    .map_err(|e| e.to_string())?;
                let result = work();
                unsafe {
                    CoUninitialize();
                }
                result
            })
            .join()
            .map_err(|_| "The Recycle Bin worker stopped unexpectedly.".to_string())?
    })
}
#[cfg(not(target_os = "windows"))]
fn native_worker<T>(work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    work()
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn platform_list() -> Result<Vec<TrashEntry>, String> {
    let mut entries = Vec::new();
    for native in trash::os_limited::list().map_err(|e| e.to_string())? {
        let Ok(payload) = native_payload(&native) else {
            continue;
        };
        let Ok(identity) = fingerprint(&payload) else {
            continue;
        };
        let Ok(metadata) = fs::symlink_metadata(&payload) else {
            continue;
        };
        let size = trash::os_limited::metadata(&native)
            .ok()
            .and_then(|metadata| metadata.size.size())
            .or_else(|| metadata.is_file().then_some(metadata.len()));
        #[cfg(target_os = "linux")]
        let identity_key = {
            use std::os::unix::fs::MetadataExt;
            let info = fingerprint(Path::new(&native.id)).map_err(|e| e.to_string())?;
            format!(
                "{:?}:{}:{info}:{}:{}",
                native.id,
                native.time_deleted,
                metadata.dev(),
                metadata.ino()
            )
        };
        #[cfg(target_os = "windows")]
        let identity_key = format!("{:?}:{}:{identity}", native.id, native.time_deleted);
        let id = TrashItemId(format!("{:x}", Sha256::digest(identity_key.as_bytes())));
        let deleted = u64::try_from(native.time_deleted)
            .ok()
            .and_then(|s| UNIX_EPOCH.checked_add(std::time::Duration::from_secs(s)));
        entries.push(TrashEntry {
            id,
            name: native.name.clone(),
            original_path: Some(native.original_path()),
            deleted,
            directory: metadata.is_dir(),
            size,
            payload,
            fingerprint: identity,
            native,
        });
    }
    Ok(entries)
}
#[cfg(target_os = "linux")]
fn native_payload(item: &trash::TrashItem) -> Result<PathBuf, String> {
    let info = Path::new(&item.id);
    let root = info
        .parent()
        .and_then(Path::parent)
        .ok_or("Invalid trash information path.")?;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let name = info
        .file_name()
        .ok_or("Invalid trash item name.")?
        .as_bytes()
        .strip_suffix(b".trashinfo")
        .ok_or("Invalid trash item name.")?;
    Ok(root.join("files").join(OsString::from_vec(name.to_vec())))
}
#[cfg(target_os = "windows")]
fn native_payload(item: &trash::TrashItem) -> Result<PathBuf, String> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows::{
        Win32::{
            System::Com::CoTaskMemFree,
            UI::Shell::{IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH},
        },
        core::PCWSTR,
    };
    let wide: Vec<_> = item.id.encode_wide().chain(Some(0)).collect();
    // SAFETY: the parsing name is terminated and COM is initialized on this worker.
    unsafe {
        let shell: IShellItem =
            SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).map_err(|e| e.to_string())?;
        let name = shell
            .GetDisplayName(SIGDN_FILESYSPATH)
            .map_err(|e| e.to_string())?;
        let mut len = 0;
        while *name.0.add(len) != 0 {
            len += 1;
        }
        let path = PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(name.0, len)));
        CoTaskMemFree(Some(name.0.cast()));
        Ok(path)
    }
}
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn platform_trash(path: &Path) -> Result<(), String> {
    trash::delete(Path::new(&crate::os_paths::native_path(path))).map_err(|e| e.to_string())
}
#[cfg(target_os = "linux")]
fn platform_purge(item: &TrashEntry) -> Result<(), String> {
    if fingerprint(&item.payload).ok().as_ref() != Some(&item.fingerprint) {
        return Err("The bin item changed; refresh and try again.".into());
    }
    trash::os_limited::purge_all([&item.native]).map_err(|e| e.to_string())
}
#[cfg(target_os = "windows")]
#[path = "trash_windows.rs"]
mod windows_backend;
#[cfg(target_os = "windows")]
fn platform_purge(item: &TrashEntry) -> Result<(), String> {
    if fingerprint(&item.payload).ok().as_ref() != Some(&item.fingerprint) {
        return Err("The bin item changed; refresh and try again.".into());
    }
    windows_backend::shell_delete(&item.native.id)?;
    if fs::symlink_metadata(&item.payload).is_ok() {
        return Err("The item could not be removed from the Recycle Bin.".into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[path = "trash_macos.rs"]
mod macos;
#[cfg(target_os = "macos")]
use macos::{platform_list, platform_purge, platform_trash, update_macos_origin};

#[cfg(test)]
pub(super) fn fixture_entry(name: &str, payload: &Path, original: Option<PathBuf>) -> TrashEntry {
    let metadata = fs::symlink_metadata(payload).unwrap();
    let identity = fingerprint(payload).unwrap();
    TrashEntry {
        id: TrashItemId(format!(
            "{:x}",
            Sha256::digest(format!("{payload:?}:{identity}").as_bytes())
        )),
        name: name.into(),
        original_path: original.clone(),
        deleted: None,
        size: Some(metadata.len()),
        directory: metadata.is_dir(),
        payload: payload.to_owned(),
        fingerprint: identity,
        #[cfg(target_os = "macos")]
        original_volume: None,
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        native: trash::TrashItem {
            id: "fake".into(),
            name: name.into(),
            original_parent: original
                .as_ref()
                .and_then(|p| p.parent())
                .unwrap_or(Path::new("/"))
                .to_owned(),
            time_deleted: 0,
        },
    }
}
#[cfg(test)]
pub(super) struct SnapshotFixture {
    previous: BTreeMap<TrashItemId, TrashEntry>,
    _operations: std::sync::MutexGuard<'static, ()>,
}
#[cfg(test)]
impl SnapshotFixture {
    pub(super) fn new(items: Vec<TrashEntry>) -> Self {
        let operations = OPERATIONS.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::mem::replace(
            &mut *snapshot().lock().unwrap(),
            items
                .into_iter()
                .map(|item| (item.id.clone(), item))
                .collect(),
        );
        Self {
            previous,
            _operations: operations,
        }
    }
}
#[cfg(test)]
impl Drop for SnapshotFixture {
    fn drop(&mut self) {
        *snapshot().lock().unwrap() = std::mem::take(&mut self.previous);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    struct FakeBackend {
        root: tempfile::TempDir,
        items: RefCell<Vec<TrashEntry>>,
        failed_purges: RefCell<BTreeSet<TrashItemId>>,
        list_calls: Cell<usize>,
        fail_list: Cell<bool>,
    }
    impl FakeBackend {
        fn new() -> Self {
            Self {
                root: tempfile::tempdir().unwrap(),
                items: RefCell::new(Vec::new()),
                failed_purges: RefCell::new(BTreeSet::new()),
                list_calls: Cell::new(0),
                fail_list: Cell::new(false),
            }
        }
        fn add(&self, name: &str, original: Option<PathBuf>, contents: &[u8]) -> TrashEntry {
            let sequence = self.items.borrow().len();
            let payload = self.root.path().join(format!("item-{sequence}"));
            fs::write(&payload, contents).unwrap();
            let identity = fingerprint(&payload).unwrap();
            let id = TrashItemId(format!(
                "{:x}",
                Sha256::digest(format!("{payload:?}:{identity}").as_bytes())
            ));
            let item = TrashEntry {
                id,
                name: name.into(),
                original_path: original.clone(),
                deleted: None,
                size: Some(contents.len() as u64),
                directory: false,
                payload,
                fingerprint: identity,
                #[cfg(target_os = "macos")]
                original_volume: None,
                #[cfg(any(target_os = "linux", target_os = "windows"))]
                native: trash::TrashItem {
                    id: "fake".into(),
                    name: name.into(),
                    original_parent: original
                        .as_ref()
                        .and_then(|p| p.parent())
                        .unwrap_or(Path::new("/"))
                        .to_owned(),
                    time_deleted: 0,
                },
            };
            self.items.borrow_mut().push(item.clone());
            item
        }
    }
    impl TrashBackend for FakeBackend {
        fn list(&self) -> Result<Vec<TrashEntry>, String> {
            self.list_calls.set(self.list_calls.get() + 1);
            if self.fail_list.get() {
                return Err("Injected enumeration failure".into());
            }
            Ok(self.items.borrow().clone())
        }
        fn purge(&self, item: &TrashEntry) -> Result<(), String> {
            if self.failed_purges.borrow().contains(&item.id) {
                return Err("Injected purge failure".into());
            }
            if fingerprint(&item.payload).ok().as_ref() != Some(&item.fingerprint) {
                return Err("Stale bin identity".into());
            }
            remove_payload(&item.payload).map_err(|e| e.to_string())?;
            self.items.borrow_mut().retain(|e| e.id != item.id);
            Ok(())
        }
        fn trash(&self, path: &Path) -> Result<(), String> {
            let sequence = fs::read_dir(self.root.path())
                .map_err(|e| e.to_string())?
                .count();
            let payload = self.root.path().join(format!("undo-{sequence}"));
            rename_exclusive(path, &payload).map_err(|e| e.to_string())?;
            // Undo tests assert returned data without calling the system bin.
            Ok(())
        }
    }
    fn request(ids: &[TrashItemId], directory: Option<&Path>) -> RecoveryRequest {
        RecoveryRequest {
            ids: ids.to_vec(),
            directory: directory.map(Path::to_owned),
        }
    }
    #[test]
    fn purge_and_restore_pause_between_items_without_replaying_completed_work() {
        use crate::explorer::operation_control::OperationControl;
        for restoring in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let control = OperationControl::new();
            let worker_control = control.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::scope(|scope| {
                let worker = scope.spawn(|| {
                    let _participant = worker_control.enter();
                    let backend = FakeBackend::new();
                    let items: Vec<_> = (0..3)
                        .map(|index| backend.add(&format!("item-{index}.txt"), None, b"data"))
                        .collect();
                    let ids: Vec<_> = items.iter().map(|item| item.id.clone()).collect();
                    let pause_control = worker_control.clone();
                    let requested = AtomicBool::new(false);
                    let report = move |done, _: usize, _: &str| {
                        if done == 1 && !requested.swap(true, Ordering::Relaxed) {
                            pause_control.pause();
                            tx.send(()).unwrap();
                        }
                    };
                    let result = if restoring {
                        recover_with(
                            &backend,
                            request(&ids, Some(directory.path())),
                            RecoveryChoice::Skip,
                            &worker_control.cancel,
                            report,
                        )
                    } else {
                        purge_with(&backend, ids.clone(), worker_control.cancel.clone(), report)
                    };
                    assert_eq!(result.completed, ids);
                    assert!(result.failures.is_empty());
                    assert!(backend.items.borrow().is_empty());
                });
                rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while !control.paused() {
                    if std::time::Instant::now() > deadline {
                        control.cancel();
                        panic!("bin worker did not pause");
                    }
                    std::thread::yield_now();
                }
                if restoring {
                    assert!(directory.path().join("item-0.txt").exists());
                    assert!(!directory.path().join("item-1.txt").exists());
                }
                control.resume();
                worker.join().unwrap();
            });
        }
    }

    #[test]
    fn snapshot_empty_ignores_new_arrivals_and_reports_partial_failure() {
        let backend = FakeBackend::new();
        let one = backend.add("same.txt", None, b"one");
        let two = backend.add("same.txt", None, b"two");
        let confirmed = vec![one.id.clone(), two.id.clone()];
        let later = backend.add("later.txt", None, b"later");
        backend.failed_purges.borrow_mut().insert(two.id.clone());
        let result = purge_with(
            &backend,
            confirmed,
            Arc::new(AtomicBool::new(false)),
            |_, _, _| {},
        );
        assert_eq!(result.completed, vec![one.id]);
        assert_eq!(result.failed, vec![two.id]);
        assert_eq!(fs::read(later.payload).unwrap(), b"later");
        assert_eq!(fs::read(two.payload).unwrap(), b"two");
        assert!(result.undo.paths.is_empty());
    }
    #[test]
    fn purge_hundreds_of_items_enumerates_twice_and_finishes_progress() {
        let backend = FakeBackend::new();
        let ids: Vec<_> = (0..300)
            .map(|index| {
                backend
                    .add(&format!("file-{index}.txt"), None, b"fixture")
                    .id
            })
            .collect();
        let progress = Arc::new(Mutex::new(Vec::new()));
        let reports = progress.clone();
        let result = purge_with(
            &backend,
            ids.clone(),
            Arc::new(AtomicBool::new(false)),
            move |done, total, _| {
                reports.lock().unwrap().push((done, total));
            },
        );
        assert_eq!(result.completed, ids);
        assert!(result.failures.is_empty());
        assert!(result.undo.paths.is_empty());
        assert_eq!(backend.list_calls.get(), 2);
        assert!(backend.items.borrow().is_empty());
        let progress = progress.lock().unwrap();
        assert_eq!(progress.last(), Some(&(300, 300)));
        assert!(progress.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    }

    #[test]
    fn purge_missing_and_failed_items_advance_progress_and_preserve_originals() {
        let backend = FakeBackend::new();
        let original = backend.root.path().join("original.txt");
        fs::write(&original, b"unrelated replacement").unwrap();
        let missing = backend.add("missing.txt", Some(original.clone()), b"missing");
        let failed = backend.add("failed.txt", None, b"failed");
        let complete = backend.add("complete.txt", None, b"complete");
        backend
            .items
            .borrow_mut()
            .retain(|item| item.id != missing.id);
        backend.failed_purges.borrow_mut().insert(failed.id.clone());
        let done = Arc::new(AtomicUsize::new(0));
        let reported = done.clone();
        let result = purge_with(
            &backend,
            vec![missing.id.clone(), failed.id.clone(), complete.id.clone()],
            Arc::new(AtomicBool::new(false)),
            move |count, _, _| {
                reported.store(count, Ordering::Relaxed);
            },
        );
        assert_eq!(result.skipped, vec![missing.id]);
        assert_eq!(result.failed, vec![failed.id]);
        assert_eq!(result.completed, vec![complete.id]);
        assert_eq!(done.load(Ordering::Relaxed), 3);
        assert_eq!(fs::read(original).unwrap(), b"unrelated replacement");
        assert_eq!(fs::read(failed.payload).unwrap(), b"failed");
        assert_eq!(backend.list_calls.get(), 2);
    }

    #[test]
    fn purge_cancel_before_start_retains_every_item_and_refreshes() {
        let backend = FakeBackend::new();
        let item = backend.add("file.txt", None, b"fixture");
        let result = purge_with(
            &backend,
            vec![item.id.clone()],
            Arc::new(AtomicBool::new(true)),
            |_, _, _| {},
        );
        assert!(result.cancelled);
        assert_eq!(result.cancelled_items, vec![item.id]);
        assert!(result.completed.is_empty());
        assert!(result.failed.is_empty());
        assert!(item.payload.exists());
        assert_eq!(backend.list_calls.get(), 2);
    }

    #[test]
    fn purge_cancel_during_execution_preserves_completed_and_remaining_items() {
        let backend = FakeBackend::new();
        let items: Vec<_> = (0..3)
            .map(|index| backend.add(&format!("file-{index}"), None, b"fixture"))
            .collect();
        let cancel = Arc::new(AtomicBool::new(false));
        let request_cancel = cancel.clone();
        let result = purge_with(
            &backend,
            items.iter().map(|item| item.id.clone()).collect(),
            cancel,
            move |done, _, _| {
                if done == 1 {
                    request_cancel.store(true, Ordering::Relaxed);
                }
            },
        );
        assert!(result.cancelled);
        assert_eq!(result.completed, vec![items[0].id.clone()]);
        assert_eq!(
            result.cancelled_items,
            vec![items[1].id.clone(), items[2].id.clone()]
        );
        assert!(result.failed.is_empty());
        assert!(!items[0].payload.exists());
        assert!(items[1..].iter().all(|item| item.payload.exists()));
        assert_eq!(backend.list_calls.get(), 2);
    }

    #[test]
    fn purge_revalidates_payload_identity_after_initial_listing() {
        let backend = FakeBackend::new();
        let item = backend.add("changed.txt", None, b"original");
        let payload = item.payload.clone();
        let original_identity = item.fingerprint.clone();
        // Replace just before deletion, after the single enumeration. Keep the
        // old inode alive so filesystems cannot reuse it for the replacement.
        let old_payload = backend.root.path().join("old-payload");
        let result = purge_with(
            &backend,
            vec![item.id.clone()],
            Arc::new(AtomicBool::new(false)),
            move |_, _, name| {
                if name == "changed.txt" && fingerprint(&payload).unwrap() == original_identity {
                    fs::rename(&payload, &old_payload).unwrap();
                    fs::write(&payload, b"replacement").unwrap();
                }
            },
        );
        assert_eq!(result.failed, vec![item.id]);
        assert!(result.completed.is_empty());
        assert_eq!(fs::read(item.payload).unwrap(), b"replacement");
    }

    #[test]
    fn purge_enumeration_failure_does_not_delete_anything() {
        let backend = FakeBackend::new();
        let item = backend.add("file.txt", None, b"fixture");
        backend.fail_list.set(true);
        let result = purge_with(
            &backend,
            vec![item.id.clone()],
            Arc::new(AtomicBool::new(false)),
            |_, _, _| {},
        );
        assert_eq!(result.failed, vec![item.id]);
        assert_eq!(result.failures.len(), 1);
        assert!(result.completed.is_empty());
        assert!(item.payload.exists());
        assert_eq!(backend.list_calls.get(), 2);
    }
    #[test]
    fn unavailable_identity_does_not_touch_original_replacement() {
        let backend = FakeBackend::new();
        let live = tempfile::tempdir().unwrap().keep();
        let original = live.join("same.txt");
        fs::write(&original, b"unrelated").unwrap();
        let item = backend.add("same.txt", Some(original.clone()), b"deleted");
        backend.items.borrow_mut().clear();
        let result = recover_with(
            &backend,
            request(&[item.id.clone()], None),
            RecoveryChoice::Replace,
            &AtomicBool::new(false),
            |_, _, _| {},
        );
        assert_eq!(result.skipped, vec![item.id]);
        assert_eq!(fs::read(original).unwrap(), b"unrelated");
        fs::remove_dir_all(live).unwrap();
    }
    #[test]
    fn unknown_origin_can_be_recovered_to_folder_and_known_origin_recreates_parents() {
        let backend = FakeBackend::new();
        let destination = tempfile::tempdir().unwrap();
        let unknown = backend.add("unknown.txt", None, b"unknown");
        let failure = recover_with(
            &backend,
            request(&[unknown.id.clone()], None),
            RecoveryChoice::Skip,
            &AtomicBool::new(false),
            |_, _, _| {},
        );
        assert_eq!(failure.failed, vec![unknown.id.clone()]);
        assert!(unknown.payload.exists());
        let restored = recover_with(
            &backend,
            request(&[unknown.id.clone()], Some(destination.path())),
            RecoveryChoice::Skip,
            &AtomicBool::new(false),
            |_, _, _| {},
        );
        assert_eq!(restored.completed, vec![unknown.id]);
        assert_eq!(
            fs::read(destination.path().join("unknown.txt")).unwrap(),
            b"unknown"
        );
        let original = destination.path().join("missing/parents/known.txt");
        let known = backend.add("known.txt", Some(original.clone()), b"known");
        let restored = recover_with(
            &backend,
            request(&[known.id.clone()], None),
            RecoveryChoice::Skip,
            &AtomicBool::new(false),
            |_, _, _| {},
        );
        assert_eq!(restored.completed, vec![known.id]);
        assert_eq!(fs::read(original).unwrap(), b"known");
        assert_eq!(restored.undo.created_directories.len(), 2);
    }
    #[test]
    fn duplicate_versions_keep_both_and_cancel_remaining_items() {
        let backend = FakeBackend::new();
        let destination = tempfile::tempdir().unwrap();
        let one = backend.add("version.txt", None, b"one");
        let two = backend.add("version.txt", None, b"two");
        let three = backend.add("version.txt", None, b"three");
        let cancel = AtomicBool::new(false);
        let result = recover_with(
            &backend,
            request(
                &[one.id.clone(), two.id.clone(), three.id.clone()],
                Some(destination.path()),
            ),
            RecoveryChoice::KeepBoth,
            &cancel,
            |done, _, _| {
                if done == 2 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );
        assert_eq!(result.completed, vec![one.id, two.id]);
        assert_eq!(result.cancelled_items, vec![three.id]);
        assert_eq!(
            fs::read(destination.path().join("version.txt")).unwrap(),
            b"one"
        );
        assert_eq!(
            fs::read(destination.path().join("version (2).txt")).unwrap(),
            b"two"
        );
        assert!(three.payload.exists());
    }
    #[test]
    fn failed_verification_keeps_bin_source_and_live_destination() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::write(&source, b"deleted").unwrap();
        fs::write(&target, b"live").unwrap();
        let mut undo = RecoveryUndo::default();
        let result = recover_node_using(
            &source,
            &target,
            RecoveryChoice::Replace,
            &AtomicBool::new(false),
            &mut undo,
            None,
            &|_, staged, _| fs::write(staged, b"corrupt"),
            &rename_exclusive,
        );
        assert!(result.is_err());
        assert_eq!(fs::read(source).unwrap(), b"deleted");
        assert_eq!(fs::read(target).unwrap(), b"live");
        assert!(undo.paths.is_empty());
    }
    #[test]
    fn changed_destination_during_staging_is_never_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::write(&source, b"deleted").unwrap();
        fs::write(&target, b"original").unwrap();
        let mut undo = RecoveryUndo::default();
        let result = recover_node_using(
            &source,
            &target,
            RecoveryChoice::Replace,
            &AtomicBool::new(false),
            &mut undo,
            None,
            &|source, staged, cancel| {
                copy_verified(source, staged, cancel)?;
                let replacement = temp.path().join("unrelated");
                fs::write(&replacement, b"unrelated")?;
                fs::remove_file(&target)?;
                rename_exclusive(&replacement, &target)
            },
            &rename_exclusive,
        );
        assert!(result.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"unrelated");
        assert_eq!(fs::read(&source).unwrap(), b"deleted");
        assert!(undo.paths.is_empty());
    }
    #[test]
    fn undo_recovery_returns_data_to_fake_bin_and_restores_backup() {
        let backend = FakeBackend::new();
        let destination = tempfile::tempdir().unwrap();
        let target = destination.path().join("file.txt");
        fs::write(&target, b"live").unwrap();
        let item = backend.add("file.txt", None, b"deleted");
        let mut result = recover_with(
            &backend,
            request(&[item.id], Some(destination.path())),
            RecoveryChoice::Replace,
            &AtomicBool::new(false),
            |_, _, _| {},
        );
        assert_eq!(fs::read(&target).unwrap(), b"deleted");
        undo_recovery_with(&backend, &mut result.undo).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"live");
        assert_eq!(
            fs::read(backend.root.path().join("undo-0")).unwrap(),
            b"deleted"
        );
        assert!(result.undo.paths.is_empty());
    }
    #[test]
    fn undo_leaves_unrelated_parent_replacement_in_place() {
        let backend = FakeBackend::new();
        let destination = tempfile::tempdir().unwrap();
        let parent = destination.path().join("parent");
        let target = parent.join("file.txt");
        let item = backend.add("file.txt", Some(target.clone()), b"deleted");
        let mut result = recover_with(
            &backend,
            request(&[item.id], None),
            RecoveryChoice::Skip,
            &AtomicBool::new(false),
            |_, _, _| {},
        );
        assert_eq!(result.completed.len(), 1);
        let moved_parent = destination.path().join("moved-parent");
        rename_exclusive(&parent, &moved_parent).unwrap();
        fs::create_dir(&parent).unwrap();
        rename_exclusive(&moved_parent.join("file.txt"), &target).unwrap();
        undo_recovery_with(&backend, &mut result.undo).unwrap();
        assert!(parent.is_dir());
        assert_eq!(
            fs::read(backend.root.path().join("undo-0")).unwrap(),
            b"deleted"
        );
    }
    #[test]
    fn duplicate_replace_versions_undo_each_version_and_restore_original() {
        let backend = FakeBackend::new();
        let destination = tempfile::tempdir().unwrap();
        let target = destination.path().join("version.txt");
        fs::write(&target, b"original").unwrap();
        let one = backend.add("version.txt", None, b"one");
        let two = backend.add("version.txt", None, b"two");
        let mut result = recover_with(
            &backend,
            request(&[one.id, two.id], Some(destination.path())),
            RecoveryChoice::Replace,
            &AtomicBool::new(false),
            |_, _, _| {},
        );
        assert_eq!(result.completed.len(), 2);
        assert_eq!(fs::read(&target).unwrap(), b"two");
        undo_recovery_with(&backend, &mut result.undo).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert_eq!(
            fs::read(backend.root.path().join("undo-0")).unwrap(),
            b"two"
        );
        assert_eq!(
            fs::read(backend.root.path().join("undo-1")).unwrap(),
            b"one"
        );
    }
    #[test]
    fn backup_survives_cloned_worker_undo_and_is_removed_with_last_owner() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::write(&source, b"deleted").unwrap();
        fs::write(&target, b"original").unwrap();
        let mut undo = RecoveryUndo::default();
        recover_node(
            &source,
            &target,
            RecoveryChoice::Replace,
            &AtomicBool::new(false),
            &mut undo,
            None,
        )
        .unwrap();
        let backup = undo.paths[0]
            .replacement
            .as_ref()
            .unwrap()
            .backup
            .path
            .clone();
        let worker = undo.clone();
        cleanup_undo(undo);
        assert_eq!(fs::read(&backup).unwrap(), b"original");
        cleanup_undo(worker);
        assert!(!backup.exists());
        assert!(!backup.parent().unwrap().exists());
    }
    #[test]
    fn purge_failure_after_recovery_preserves_source_and_records_destination_undo() {
        let backend = FakeBackend::new();
        let destination = tempfile::tempdir().unwrap();
        let item = backend.add("file.txt", None, b"deleted");
        backend.failed_purges.borrow_mut().insert(item.id.clone());
        let result = recover_with(
            &backend,
            request(&[item.id.clone()], Some(destination.path())),
            RecoveryChoice::Skip,
            &AtomicBool::new(false),
            |_, _, _| {},
        );
        assert_eq!(result.failed, vec![item.id]);
        assert!(item.payload.exists());
        assert_eq!(
            fs::read(destination.path().join("file.txt")).unwrap(),
            b"deleted"
        );
        assert_eq!(result.undo.paths.len(), 1);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn cross_filesystem_recovery_verifies_data_before_consuming_bin_payload() {
        use std::os::unix::fs::MetadataExt;
        let backend = FakeBackend::new();
        let Ok(destination) = tempfile::tempdir_in("/dev/shm") else {
            return;
        };
        if fs::metadata(destination.path()).unwrap().dev()
            == fs::metadata(backend.root.path()).unwrap().dev()
        {
            return;
        }
        let contents = vec![42u8; 1024 * 1024];
        let item = backend.add("file.bin", None, &contents);
        let result = recover_with(
            &backend,
            request(&[item.id.clone()], Some(destination.path())),
            RecoveryChoice::Skip,
            &AtomicBool::new(false),
            |_, _, _| {},
        );
        assert_eq!(result.completed, vec![item.id]);
        assert!(!item.payload.exists());
        assert_eq!(
            fs::read(destination.path().join("file.bin")).unwrap(),
            contents
        );
    }
    #[test]
    #[ignore = "manual disposable-file native smoke test; never empties the system bin"]
    fn native_bin_disposable_file_roundtrip() {
        let temp = tempfile::tempdir().unwrap().keep();
        let source = temp.join("explorer-native-bin-smoke.txt");
        eprintln!("Disposable fixture: {}", source.display());
        fs::write(&source, b"disposable native bin smoke fixture").unwrap();
        let outcome = trash_paths_batch(&[source.clone()]).unwrap();
        assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
        assert_eq!(outcome.ids.len(), 1);
        assert!(!source.exists());
        let result = recover(
            request(&outcome.ids, None),
            RecoveryChoice::Skip,
            &AtomicBool::new(false),
            |_, _, _| {},
        );
        assert!(result.failures.is_empty(), "{:?}", result.failures);
        assert_eq!(result.completed, outcome.ids);
        assert_eq!(
            fs::read(&source).unwrap(),
            b"disposable native bin smoke fixture"
        );
        cleanup_undo(result.undo);
        fs::remove_file(source).unwrap();
        let folder = temp.join("explorer-native-bin-folder");
        fs::create_dir_all(folder.join("nested/empty")).unwrap();
        fs::write(folder.join("nested/file.txt"), b"disposable folder fixture").unwrap();
        let outcome = trash_paths_batch(&[folder.clone()]).unwrap();
        assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
        assert_eq!(outcome.ids.len(), 1);
        let alternate = temp.join("alternate");
        fs::create_dir(&alternate).unwrap();
        let result = recover(
            request(&outcome.ids, Some(&alternate)),
            RecoveryChoice::Skip,
            &AtomicBool::new(false),
            |_, _, _| {},
        );
        assert!(result.failures.is_empty(), "{:?}", result.failures);
        assert_eq!(result.completed, outcome.ids);
        let recovered = alternate.join("explorer-native-bin-folder");
        assert_eq!(
            fs::read(recovered.join("nested/file.txt")).unwrap(),
            b"disposable folder fixture"
        );
        assert!(recovered.join("nested/empty").is_dir());
        cleanup_undo(result.undo);
        fs::remove_dir_all(alternate).unwrap();
        let disposable = temp.join("explorer-native-bin-purge.txt");
        fs::write(&disposable, b"owned purge fixture").unwrap();
        let outcome = trash_paths_batch(&[disposable]).unwrap();
        assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
        assert_eq!(outcome.ids.len(), 1);
        let result = purge(
            outcome.ids.clone(),
            Arc::new(AtomicBool::new(false)),
            |_, _, _| {},
        );
        assert!(result.failures.is_empty(), "{:?}", result.failures);
        assert_eq!(result.completed, outcome.ids);
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            let external = temp.join("externally-deleted.txt");
            fs::write(&external, b"external trash fixture").unwrap();
            // Delete without Explorer's service/cache to exercise other-app discovery.
            trash::delete(&external).unwrap();
            let ids: Vec<_> = list()
                .unwrap()
                .into_iter()
                .filter(|item| {
                    item.original_path
                        .as_ref()
                        .is_some_and(|path| same_path(path, &external))
                })
                .map(|item| item.id)
                .collect();
            assert_eq!(ids.len(), 1);
            let result = recover(
                request(&ids, None),
                RecoveryChoice::Skip,
                &AtomicBool::new(false),
                |_, _, _| {},
            );
            assert!(result.failures.is_empty(), "{:?}", result.failures);
            assert_eq!(result.completed, ids);
            assert_eq!(fs::read(&external).unwrap(), b"external trash fixture");
            fs::remove_file(external).unwrap();
        }
        fs::remove_dir(temp).unwrap();
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "disposable native recovery test; only operates on its own fixtures"]
    fn native_bin_paste_destination_formats() {
        let temp = tempfile::tempdir().unwrap();
        for format in ["ordinary", "forward-slash", "canonical"] {
            for folder in [false, true] {
                let name = format!(
                    "explorer-paste-{format}-{}",
                    if folder { "folder" } else { "file" }
                );
                let source = temp.path().join(&name);
                if folder {
                    fs::create_dir_all(source.join("nested/empty")).unwrap();
                    fs::write(source.join("nested/file.txt"), b"paste fixture").unwrap();
                } else {
                    fs::write(&source, b"paste fixture").unwrap();
                }
                let outcome = trash_paths_batch(&[source.clone()]).unwrap();
                assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
                let directory = temp.path().join(format!("destination-{format}-{folder}"));
                fs::create_dir(&directory).unwrap();
                let directory = match format {
                    "forward-slash" => {
                        PathBuf::from(directory.to_str().unwrap().replace('\\', "/"))
                    }
                    "canonical" => fs::canonicalize(directory).unwrap(),
                    _ => directory,
                };
                let target = directory.join(&name);
                fs::write(&target, b"existing destination").unwrap();
                let skipped = recover(
                    request(&outcome.ids, Some(&directory)),
                    RecoveryChoice::Skip,
                    &AtomicBool::new(false),
                    |_, _, _| {},
                );
                assert!(
                    skipped.failures.is_empty(),
                    "{format}: {:?}",
                    skipped.failures
                );
                assert!(skipped.completed.is_empty());
                assert!(
                    list()
                        .unwrap()
                        .iter()
                        .any(|item| outcome.ids.contains(&item.id))
                );
                fs::remove_file(&target).unwrap();
                let mut result = recover(
                    request(&outcome.ids, Some(&directory)),
                    RecoveryChoice::Skip,
                    &AtomicBool::new(false),
                    |_, _, _| {},
                );
                assert!(
                    result.failures.is_empty(),
                    "{format}, folder={folder}: {:?}",
                    result.failures
                );
                assert_eq!(result.completed, outcome.ids);
                let payload = if folder {
                    target.join("nested/file.txt")
                } else {
                    target.clone()
                };
                assert_eq!(fs::read(payload).unwrap(), b"paste fixture");
                if folder {
                    assert!(target.join("nested/empty").is_dir());
                }
                assert!(
                    list()
                        .unwrap()
                        .iter()
                        .all(|item| !outcome.ids.contains(&item.id))
                );
                undo_recovery(&mut result.undo).unwrap();
                assert!(!target.exists());
                let ids: Vec<_> = list()
                    .unwrap()
                    .into_iter()
                    .filter(|item| {
                        item.original_path
                            .as_deref()
                            .is_some_and(|p| same_path(p, &target))
                    })
                    .map(|item| item.id)
                    .collect();
                assert_eq!(ids.len(), 1);
                let recovered = recover(
                    request(&ids, Some(temp.path())),
                    RecoveryChoice::Skip,
                    &AtomicBool::new(false),
                    |_, _, _| {},
                );
                assert!(recovered.failures.is_empty(), "{:?}", recovered.failures);
                cleanup_undo(recovered.undo);
            }
        }
    }

    #[test]
    fn virtual_identity_never_aliases_an_original_path() {
        let id = TrashItemId("a".repeat(64));
        assert_eq!(
            item_id(&PathBuf::from(format!("{ADDRESS}items/{}", id.0))),
            Some(id)
        );
        assert!(is_root(&root()));
        assert!(!is_root(Path::new("trash:///items/a")));
        assert!(item_id(Path::new("/tmp/deleted.txt")).is_none());
        assert!(item_id(Path::new("trash:///items/../../live")).is_none());
    }
    #[test]
    fn duplicate_names_get_numbered_without_overwriting() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("report.txt");
        fs::write(&path, b"live").unwrap();
        fs::write(temp.path().join("report (2).txt"), b"other").unwrap();
        assert_eq!(keep_both_path(&path), temp.path().join("report (3).txt"));
    }
    #[test]
    fn keep_both_uses_recovered_type_when_file_and_folder_names_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let existing = temp.path().join("report.txt");
        fs::create_dir(&existing).unwrap();
        assert_eq!(
            keep_both_path_for(&existing, false),
            temp.path().join("report (2).txt")
        );
        let existing = temp.path().join("folder.v1");
        fs::write(&existing, b"file").unwrap();
        assert_eq!(
            keep_both_path_for(&existing, true),
            temp.path().join("folder.v1 (2)")
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn pinned_inode_survives_rename_and_rejects_unrelated_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::write(&source, b"deleted").unwrap();
        let pinned = pin_inode(&source).unwrap();
        rename_exclusive(&source, &target).unwrap();
        assert!(is_pinned_inode(&pinned, &target));
        fs::remove_file(&target).unwrap();
        fs::write(&target, b"unrelated").unwrap();
        assert!(!is_pinned_inode(&pinned, &target));
    }
    #[test]
    fn recovery_replace_keeps_backup_and_checks_undo_identity() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("deleted.txt");
        let target = temp.path().join("live.txt");
        fs::write(&source, b"deleted").unwrap();
        fs::write(&target, b"live").unwrap();
        let mut undo = RecoveryUndo::default();
        assert!(
            recover_node(
                &source,
                &target,
                RecoveryChoice::Replace,
                &AtomicBool::new(false),
                &mut undo,
                None
            )
            .unwrap()
        );
        assert_eq!(fs::read(&source).unwrap(), b"deleted");
        assert_eq!(fs::read(&target).unwrap(), b"deleted");
        assert_eq!(
            fs::read(&undo.paths[0].replacement.as_ref().unwrap().backup.path).unwrap(),
            b"live"
        );
        fs::remove_file(&target).unwrap();
        fs::write(&target, b"unrelated").unwrap();
        assert!(undo_recovery(&mut undo).is_err());
        cleanup_undo(undo);
    }
    #[test]
    fn merge_skip_preserves_remaining_source_and_recovers_other_children() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(source.join("conflict"), b"old").unwrap();
        fs::write(source.join("new"), b"new").unwrap();
        fs::write(target.join("conflict"), b"live").unwrap();
        let mut undo = RecoveryUndo::default();
        assert!(
            !recover_node(
                &source,
                &target,
                RecoveryChoice::Skip,
                &AtomicBool::new(false),
                &mut undo,
                None
            )
            .unwrap()
        );
        assert!(source.join("conflict").exists());
        assert!(!source.join("new").exists());
        assert_eq!(fs::read(target.join("conflict")).unwrap(), b"live");
        assert_eq!(fs::read(target.join("new")).unwrap(), b"new");
    }
    #[test]
    fn cancellation_retains_source_and_destination() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::write(&source, b"data").unwrap();
        assert!(
            !recover_node(
                &source,
                &target,
                RecoveryChoice::Replace,
                &AtomicBool::new(true),
                &mut RecoveryUndo::default(),
                None
            )
            .unwrap()
        );
        assert!(source.exists());
        assert!(!target.exists());
    }
    #[cfg(unix)]
    #[test]
    fn recovery_preserves_symlink_without_touching_target() {
        let temp = tempfile::tempdir().unwrap();
        let live = temp.path().join("live");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::write(&live, b"data").unwrap();
        std::os::unix::fs::symlink(&live, &source).unwrap();
        assert!(
            recover_node(
                &source,
                &target,
                RecoveryChoice::Replace,
                &AtomicBool::new(false),
                &mut RecoveryUndo::default(),
                None
            )
            .unwrap()
        );
        assert_eq!(fs::read_link(target).unwrap(), live);
        assert_eq!(fs::read(live).unwrap(), b"data");
    }
}
