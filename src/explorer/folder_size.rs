use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    fs::{self, Metadata},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use gpui::{App, Global};
use jwalk::{Parallelism, WalkDirGeneric};

const FOLDER_SIZE_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const FOLDER_SIZE_CACHE_ENTRY_LIMIT: usize = 4096;

pub(super) struct FolderSizeCache {
    entries: RefCell<HashMap<PathBuf, CachedFolderSize>>,
    access_generation: Cell<u64>,
}

impl Global for FolderSizeCache {}

#[derive(Clone, Copy)]
struct CachedFolderSize {
    size: u64,
    calculated_at: Instant,
    last_access: u64,
}

impl FolderSizeCache {
    pub(super) fn new() -> Self {
        Self {
            entries: RefCell::new(HashMap::new()),
            access_generation: Cell::new(0),
        }
    }

    pub(super) fn get(&self, path: &Path) -> Option<u64> {
        self.get_at(path, Instant::now())
    }

    fn get_at(&self, path: &Path, now: Instant) -> Option<u64> {
        let mut entries = self.entries.borrow_mut();
        entries.retain(|_, cached| {
            now.saturating_duration_since(cached.calculated_at) < FOLDER_SIZE_CACHE_TTL
        });
        let access = self.next_access();
        let cached = entries.get_mut(path)?;
        cached.last_access = access;
        Some(cached.size)
    }

    pub(super) fn insert(&self, path: PathBuf, size: u64) {
        self.insert_at(path, size, Instant::now());
    }

    fn insert_at(&self, path: PathBuf, size: u64, calculated_at: Instant) {
        self.insert_at_with_limit(path, size, calculated_at, FOLDER_SIZE_CACHE_ENTRY_LIMIT);
    }

    fn insert_at_with_limit(
        &self,
        path: PathBuf,
        size: u64,
        calculated_at: Instant,
        entry_limit: usize,
    ) {
        let mut entries = self.entries.borrow_mut();
        entries.retain(|_, cached| {
            calculated_at.saturating_duration_since(cached.calculated_at) < FOLDER_SIZE_CACHE_TTL
        });
        while !entries.contains_key(&path) && entries.len() >= entry_limit {
            let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, cached)| cached.last_access)
                .map(|(path, _)| path.clone())
            else {
                break;
            };
            entries.remove(&oldest);
        }
        let last_access = self.next_access();
        entries.insert(
            path,
            CachedFolderSize {
                size,
                calculated_at,
                last_access,
            },
        );
    }

    fn next_access(&self) -> u64 {
        let access = self.access_generation.get().wrapping_add(1);
        self.access_generation.set(access);
        access
    }

    pub(super) fn invalidate<'a>(&self, paths: impl IntoIterator<Item = &'a PathBuf>) {
        let mut entries = self.entries.borrow_mut();
        for path in paths {
            entries.remove(path);
        }
    }
}

pub(crate) fn initialize(cx: &mut App) {
    cx.set_global(FolderSizeCache::new());
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum FolderSizeError {
    Cancelled,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FolderSizeCalculation {
    pub(super) path: PathBuf,
    pub(super) size: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum FolderSizeEntryState {
    #[default]
    Directory,
    Size(u64),
    Unavailable,
}

pub(super) fn calculate_folder_size(
    path: &Path,
    cancel: Arc<AtomicBool>,
) -> Result<u64, FolderSizeError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(FolderSizeError::Cancelled);
    }

    let metadata = fs::symlink_metadata(path).map_err(|_| FolderSizeError::Unavailable)?;
    if metadata_is_directory_link(&metadata) {
        return Ok(metadata.len());
    }

    if !metadata.is_dir() {
        return Ok(metadata.len());
    }

    let mut size = 0u64;
    let walker = WalkDirGeneric::<((), FolderSizeEntryState)>::new(path)
        .skip_hidden(false)
        .follow_links(false)
        .sort(false)
        .process_read_dir({
            let cancel = cancel.clone();
            move |depth, _, _, children| {
                if cancel.load(Ordering::Relaxed) {
                    children.clear();
                    return;
                }

                if depth.is_some() {
                    prepare_folder_size_entries(children);
                }
            }
        });

    for entry in walker {
        if cancel.load(Ordering::Relaxed) {
            return Err(FolderSizeError::Cancelled);
        }

        let entry = entry.map_err(|_| FolderSizeError::Unavailable)?;
        if entry.read_children_error.is_some() {
            return Err(FolderSizeError::Unavailable);
        }
        if entry.depth() == 0 {
            continue;
        }

        match entry.client_state {
            FolderSizeEntryState::Size(contribution) => {
                size = size.saturating_add(contribution);
            }
            FolderSizeEntryState::Directory => {}
            FolderSizeEntryState::Unavailable => return Err(FolderSizeError::Unavailable),
        }
    }

    if cancel.load(Ordering::Relaxed) {
        Err(FolderSizeError::Cancelled)
    } else {
        Ok(size)
    }
}

pub(super) fn calculate_folder_sizes(
    root: &Path,
    targets: Vec<PathBuf>,
    cancel: Arc<AtomicBool>,
    on_calculation: impl FnMut(FolderSizeCalculation),
) -> Result<(), FolderSizeError> {
    calculate_folder_sizes_with_parallelism(root, targets, cancel, None, on_calculation)
}

fn calculate_folder_sizes_with_parallelism(
    root: &Path,
    targets: Vec<PathBuf>,
    cancel: Arc<AtomicBool>,
    parallelism: Option<Parallelism>,
    mut on_calculation: impl FnMut(FolderSizeCalculation),
) -> Result<(), FolderSizeError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(FolderSizeError::Cancelled);
    }
    if targets.is_empty() {
        return Ok(());
    }

    let target_paths = targets.into_iter().collect::<HashSet<_>>();
    let mut current_target = None;
    let mut current_size = 0u64;
    let mut current_unavailable = false;
    let mut walker = WalkDirGeneric::<((), FolderSizeEntryState)>::new(root)
        .skip_hidden(false)
        .follow_links(false)
        .sort(false)
        .process_read_dir({
            let root = root.to_path_buf();
            let target_paths = target_paths.clone();
            let cancel = cancel.clone();
            move |depth, path, _, children| {
                if cancel.load(Ordering::Relaxed) {
                    children.clear();
                    return;
                }

                let reading_root = depth == Some(0) && path == root.as_path();
                if depth.is_some() {
                    prepare_folder_size_entries(children);
                }
                if reading_root {
                    for child in children.iter_mut() {
                        let Ok(entry) = child else {
                            continue;
                        };
                        if entry.file_type().is_dir() && !target_paths.contains(&entry.path()) {
                            entry.read_children_path = None;
                        }
                    }
                }
            }
        });
    if let Some(parallelism) = parallelism {
        walker = walker.parallelism(parallelism);
    }

    for entry in walker {
        if cancel.load(Ordering::Relaxed) {
            return Err(FolderSizeError::Cancelled);
        }

        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                if let Some(path) = error.path() {
                    if let Some(target) = target_for_path(root, path, &target_paths) {
                        begin_target(
                            target,
                            &mut current_target,
                            &mut current_size,
                            &mut current_unavailable,
                            &mut on_calculation,
                        );
                        current_unavailable = true;
                        continue;
                    }
                    if direct_child_path(root, path).is_some() {
                        flush_current_target(
                            &mut current_target,
                            &mut current_size,
                            &mut current_unavailable,
                            &mut on_calculation,
                        );
                        continue;
                    }
                }
                return Err(FolderSizeError::Unavailable);
            }
        };
        let path = entry.path();
        if entry.depth() == 0 {
            if entry.read_children_error.is_some() {
                return Err(FolderSizeError::Unavailable);
            }
            continue;
        }
        let direct_child = direct_child_path(root, &path);
        let target = direct_child
            .as_ref()
            .filter(|path| target_paths.contains(*path))
            .cloned();
        let Some(target) = target else {
            if direct_child.is_some() {
                flush_current_target(
                    &mut current_target,
                    &mut current_size,
                    &mut current_unavailable,
                    &mut on_calculation,
                );
            }
            continue;
        };

        begin_target(
            target.clone(),
            &mut current_target,
            &mut current_size,
            &mut current_unavailable,
            &mut on_calculation,
        );

        if entry.read_children_error.is_some() {
            current_unavailable = true;
        }

        if path == target {
            continue;
        }

        match entry.client_state {
            FolderSizeEntryState::Size(size) => {
                current_size = current_size.saturating_add(size);
            }
            FolderSizeEntryState::Directory => {}
            FolderSizeEntryState::Unavailable => {
                current_unavailable = true;
            }
        }
    }

    if cancel.load(Ordering::Relaxed) {
        return Err(FolderSizeError::Cancelled);
    }

    flush_current_target(
        &mut current_target,
        &mut current_size,
        &mut current_unavailable,
        &mut on_calculation,
    );
    Ok(())
}

fn begin_target(
    target: PathBuf,
    current_target: &mut Option<PathBuf>,
    current_size: &mut u64,
    current_unavailable: &mut bool,
    on_calculation: &mut impl FnMut(FolderSizeCalculation),
) {
    if current_target.as_ref() == Some(&target) {
        return;
    }

    flush_current_target(
        current_target,
        current_size,
        current_unavailable,
        on_calculation,
    );
    *current_target = Some(target);
}

fn flush_current_target(
    current_target: &mut Option<PathBuf>,
    current_size: &mut u64,
    current_unavailable: &mut bool,
    on_calculation: &mut impl FnMut(FolderSizeCalculation),
) {
    if let Some(path) = current_target.take()
        && !*current_unavailable
    {
        let calculation = FolderSizeCalculation {
            path,
            size: *current_size,
        };
        on_calculation(calculation);
    }
    *current_size = 0;
    *current_unavailable = false;
}

fn prepare_folder_size_entries(
    children: &mut [jwalk::Result<jwalk::DirEntry<((), FolderSizeEntryState)>>],
) {
    for child in children.iter_mut() {
        let Ok(entry) = child else {
            continue;
        };
        let Ok(metadata) = entry.metadata() else {
            entry.client_state = FolderSizeEntryState::Unavailable;
            entry.read_children_path = None;
            continue;
        };
        if metadata_is_directory_link(&metadata) || !metadata.is_dir() {
            entry.client_state = FolderSizeEntryState::Size(metadata.len());
            entry.read_children_path = None;
        } else {
            entry.client_state = FolderSizeEntryState::Directory;
        };
    }
}

fn target_for_path(root: &Path, path: &Path, target_paths: &HashSet<PathBuf>) -> Option<PathBuf> {
    let target = direct_child_path(root, path)?;
    target_paths.contains(&target).then_some(target)
}

fn direct_child_path(root: &Path, path: &Path) -> Option<PathBuf> {
    let mut components = path.strip_prefix(root).ok()?.components();
    let Component::Normal(name) = components.next()? else {
        return None;
    };
    Some(root.join(name))
}

#[cfg(not(target_os = "windows"))]
fn metadata_is_directory_link(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(target_os = "windows")]
fn metadata_is_directory_link(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn folder_size_cache_returns_fresh_values_and_expires_stale_values() {
        let cache = FolderSizeCache::new();
        let path = PathBuf::from("folder");
        let now = Instant::now();

        cache.insert_at(path.clone(), 42, now);
        assert_eq!(
            cache.get_at(&path, now + FOLDER_SIZE_CACHE_TTL - Duration::from_secs(1)),
            Some(42)
        );
        assert_eq!(cache.get_at(&path, now + FOLDER_SIZE_CACHE_TTL), None);
    }

    #[test]
    fn folder_size_cache_invalidates_requested_paths_only() {
        let cache = FolderSizeCache::new();
        let first = PathBuf::from("first");
        let second = PathBuf::from("second");
        cache.insert(first.clone(), 1);
        cache.insert(second.clone(), 2);

        cache.invalidate(std::iter::once(&first));

        assert_eq!(cache.get(&first), None);
        assert_eq!(cache.get(&second), Some(2));
    }

    #[test]
    fn folder_size_cache_prunes_expired_entries_eagerly() {
        let cache = FolderSizeCache::new();
        let now = Instant::now();
        cache.insert_at(PathBuf::from("stale"), 1, now);

        cache.insert_at(PathBuf::from("fresh"), 2, now + FOLDER_SIZE_CACHE_TTL);

        assert_eq!(cache.entries.borrow().len(), 1);
        assert_eq!(
            cache.get_at(&PathBuf::from("fresh"), now + FOLDER_SIZE_CACHE_TTL),
            Some(2)
        );
    }

    #[test]
    fn folder_size_cache_evicts_least_recently_used_entry_at_limit() {
        let cache = FolderSizeCache::new();
        let now = Instant::now();
        let first = PathBuf::from("first");
        let second = PathBuf::from("second");
        let third = PathBuf::from("third");
        cache.insert_at_with_limit(first.clone(), 1, now, 2);
        cache.insert_at_with_limit(second.clone(), 2, now, 2);
        assert_eq!(cache.get_at(&first, now), Some(1));

        cache.insert_at_with_limit(third.clone(), 3, now, 2);

        assert_eq!(cache.get_at(&first, now), Some(1));
        assert_eq!(cache.get_at(&second, now), None);
        assert_eq!(cache.get_at(&third, now), Some(3));
    }

    #[test]
    fn folder_size_sums_nested_files() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        let nested = folder.join("nested");
        fs::create_dir(&folder).expect("create folder");
        fs::create_dir(&nested).expect("create nested folder");
        fs::write(folder.join("a.txt"), b"abc").expect("create first file");
        fs::write(nested.join("b.txt"), b"defg").expect("create second file");
        let cancel = Arc::new(AtomicBool::new(false));

        assert_eq!(calculate_folder_size(&folder, cancel), Ok(7));
    }

    #[test]
    fn batched_folder_sizes_sum_multiple_visible_siblings() {
        let temp = TempDir::new();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let nested = first.join("nested");
        fs::create_dir(&first).expect("create first folder");
        fs::create_dir(&second).expect("create second folder");
        fs::create_dir(&nested).expect("create nested folder");
        fs::write(first.join("a.txt"), b"abc").expect("create first file");
        fs::write(nested.join("b.txt"), b"defg").expect("create nested file");
        fs::write(second.join("c.txt"), b"hello").expect("create second file");
        let cancel = Arc::new(AtomicBool::new(false));

        let calculations =
            collect_folder_sizes(temp.path(), vec![first.clone(), second.clone()], cancel)
                .expect("calculate folder sizes");
        let sizes = calculation_map(calculations);

        assert_eq!(sizes.get(&first), Some(&7));
        assert_eq!(sizes.get(&second), Some(&5));
    }

    #[test]
    fn batched_folder_sizes_emit_completed_target_before_returning() {
        let temp = TempDir::new();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir(&first).expect("create first folder");
        fs::create_dir(&second).expect("create second folder");
        fs::write(first.join("a.txt"), b"a").expect("create first file");
        fs::write(second.join("b.txt"), b"b").expect("create second file");
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_from_callback = cancel.clone();
        let mut calculations = Vec::new();

        let result = calculate_folder_sizes_with_parallelism(
            temp.path(),
            vec![first.clone(), second.clone()],
            cancel,
            Some(Parallelism::Serial),
            |calculation| {
                calculations.push(calculation);
                cancel_from_callback.store(true, Ordering::Relaxed);
            },
        );

        assert_eq!(result, Err(FolderSizeError::Cancelled));
        assert_eq!(calculations.len(), 1);
        assert_eq!(calculations[0].size, 1);
    }

    #[test]
    fn batched_folder_sizes_ignore_root_level_non_targets() {
        let temp = TempDir::new();
        let target = temp.path().join("target");
        let other = temp.path().join("other");
        fs::create_dir(&target).expect("create target folder");
        fs::create_dir(&other).expect("create other folder");
        fs::write(target.join("target.txt"), b"abc").expect("create target file");
        fs::write(other.join("other.txt"), b"ignored").expect("create other file");
        fs::write(temp.path().join("root.txt"), b"ignored").expect("create root file");
        let cancel = Arc::new(AtomicBool::new(false));

        let calculations = collect_folder_sizes(temp.path(), vec![target.clone()], cancel)
            .expect("calculate folder sizes");
        let sizes = calculation_map(calculations);

        assert_eq!(sizes.len(), 1);
        assert_eq!(sizes.get(&target), Some(&3));
    }

    #[test]
    fn folder_size_counts_dotfiles() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        fs::create_dir(&folder).expect("create folder");
        fs::write(folder.join(".hidden"), b"abc").expect("create hidden file");
        let cancel = Arc::new(AtomicBool::new(false));

        assert_eq!(calculate_folder_size(&folder, cancel.clone()), Ok(3));
        let calculations = collect_folder_sizes(temp.path(), vec![folder.clone()], cancel)
            .expect("calculate folder sizes");
        let sizes = calculation_map(calculations);
        assert_eq!(sizes.get(&folder), Some(&3));
    }

    #[test]
    fn folder_size_stops_when_cancelled() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        fs::create_dir(&folder).expect("create folder");
        fs::write(folder.join("a.txt"), b"abc").expect("create file");
        let cancel = Arc::new(AtomicBool::new(true));

        assert_eq!(
            calculate_folder_size(&folder, cancel.clone()),
            Err(FolderSizeError::Cancelled)
        );
        assert!(matches!(
            calculate_folder_sizes(temp.path(), vec![folder], cancel, |_| {}),
            Err(FolderSizeError::Cancelled)
        ));
    }

    #[test]
    fn folder_size_reports_unavailable_for_missing_path() {
        let cancel = Arc::new(AtomicBool::new(false));

        assert_eq!(
            calculate_folder_size(&PathBuf::from("missing-folder"), cancel),
            Err(FolderSizeError::Unavailable)
        );
    }

    #[test]
    fn folder_size_does_not_descend_into_directory_symlink() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        let target = temp.path().join("target");
        let link = folder.join("link");
        fs::create_dir(&folder).expect("create folder");
        fs::create_dir(&target).expect("create target");
        fs::write(target.join("inside-target.txt"), b"not counted").expect("create target file");
        if create_directory_symlink(&target, &link).is_err() {
            return;
        }
        let link_size = fs::symlink_metadata(&link)
            .expect("read link metadata")
            .len();
        let cancel = Arc::new(AtomicBool::new(false));

        assert_eq!(
            calculate_folder_size(&folder, cancel.clone()),
            Ok(link_size)
        );
        let calculations = collect_folder_sizes(temp.path(), vec![folder.clone()], cancel)
            .expect("calculate folder sizes");
        let sizes = calculation_map(calculations);
        assert_eq!(sizes.get(&folder), Some(&link_size));
    }

    fn collect_folder_sizes(
        root: &Path,
        targets: Vec<PathBuf>,
        cancel: Arc<AtomicBool>,
    ) -> Result<Vec<FolderSizeCalculation>, FolderSizeError> {
        let mut calculations = Vec::new();
        calculate_folder_sizes(root, targets, cancel, |calculation| {
            calculations.push(calculation);
        })?;
        Ok(calculations)
    }

    fn calculation_map(calculations: Vec<FolderSizeCalculation>) -> HashMap<PathBuf, u64> {
        calculations
            .into_iter()
            .map(|calculation| (calculation.path, calculation.size))
            .collect()
    }

    #[cfg(not(target_os = "windows"))]
    fn create_directory_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(target_os = "windows")]
    fn create_directory_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }
}
