use std::{
    cell::RefCell,
    collections::HashMap,
    fs::{self, Metadata},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use gpui::{App, Global};

const FOLDER_SIZE_CACHE_TTL: Duration = Duration::from_secs(10 * 60);

pub(super) struct FolderSizeCache {
    entries: RefCell<HashMap<PathBuf, CachedFolderSize>>,
}

impl Global for FolderSizeCache {}

#[derive(Clone, Copy)]
struct CachedFolderSize {
    size: u64,
    calculated_at: Instant,
}

impl FolderSizeCache {
    pub(super) fn new() -> Self {
        Self {
            entries: RefCell::new(HashMap::new()),
        }
    }

    pub(super) fn get(&self, path: &Path) -> Option<u64> {
        self.get_at(path, Instant::now())
    }

    fn get_at(&self, path: &Path, now: Instant) -> Option<u64> {
        let cached = self.entries.borrow().get(path).copied()?;
        if now.saturating_duration_since(cached.calculated_at) < FOLDER_SIZE_CACHE_TTL {
            Some(cached.size)
        } else {
            self.entries.borrow_mut().remove(path);
            None
        }
    }

    pub(super) fn insert(&self, path: PathBuf, size: u64) {
        self.insert_at(path, size, Instant::now());
    }

    fn insert_at(&self, path: PathBuf, size: u64, calculated_at: Instant) {
        self.entries.borrow_mut().insert(
            path,
            CachedFolderSize {
                size,
                calculated_at,
            },
        );
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

pub(super) fn calculate_folder_size(
    path: &Path,
    cancel: &AtomicBool,
) -> Result<u64, FolderSizeError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(FolderSizeError::Cancelled);
    }

    let metadata = fs::symlink_metadata(path).map_err(|_| FolderSizeError::Unavailable)?;
    if metadata_is_directory_link(&metadata) {
        return Ok(metadata.len());
    }

    if metadata.is_dir() {
        let mut size = 0;
        let entries = fs::read_dir(path).map_err(|_| FolderSizeError::Unavailable)?;
        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                return Err(FolderSizeError::Cancelled);
            }

            let entry = entry.map_err(|_| FolderSizeError::Unavailable)?;
            size += calculate_folder_size(&entry.path(), cancel)?;
        }
        Ok(size)
    } else {
        Ok(metadata.len())
    }
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
    fn folder_size_sums_nested_files() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        let nested = folder.join("nested");
        fs::create_dir(&folder).expect("create folder");
        fs::create_dir(&nested).expect("create nested folder");
        fs::write(folder.join("a.txt"), b"abc").expect("create first file");
        fs::write(nested.join("b.txt"), b"defg").expect("create second file");
        let cancel = AtomicBool::new(false);

        assert_eq!(calculate_folder_size(&folder, &cancel), Ok(7));
    }

    #[test]
    fn folder_size_stops_when_cancelled() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        fs::create_dir(&folder).expect("create folder");
        fs::write(folder.join("a.txt"), b"abc").expect("create file");
        let cancel = AtomicBool::new(true);

        assert_eq!(
            calculate_folder_size(&folder, &cancel),
            Err(FolderSizeError::Cancelled)
        );
    }

    #[test]
    fn folder_size_reports_unavailable_for_missing_path() {
        let cancel = AtomicBool::new(false);

        assert_eq!(
            calculate_folder_size(&PathBuf::from("missing-folder"), &cancel),
            Err(FolderSizeError::Unavailable)
        );
    }
}
