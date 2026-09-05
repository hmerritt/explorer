//! Session-only snapshots of remote directory listings. Filesystem reads and writes
//! happen outside the lock; tickets prevent superseded reads from publishing.

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use super::{entry::FileEntry, filesystem::{EntryVisibility, load_entries, path_is_same_or_descendant}};

const TTL: Duration = Duration::from_secs(5 * 60);
const MAX_LISTINGS: usize = 128;
const MAX_ENTRIES: usize = 100_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DirectoryLoadPolicy {
    Cached,
    Fresh,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Key {
    path: PathBuf,
    visibility: EntryVisibility,
}

struct Listing {
    entries: Vec<FileEntry>,
    loaded_at: Instant,
    last_used: u64,
}

#[derive(Default)]
struct Cache {
    listings: HashMap<Key, Listing>,
    in_flight: HashMap<Key, u64>,
    sequence: u64,
}

impl Cache {
    fn next_sequence(&mut self) -> u64 {
        self.sequence = self.sequence.checked_add(1).expect("directory cache sequence exhausted");
        self.sequence
    }

    fn invalidate(&mut self, matches: impl Fn(&Path) -> bool) {
        self.listings.retain(|key, _| !matches(&key.path));
        self.in_flight.retain(|key, _| !matches(&key.path));
    }

    fn prune(&mut self, now: Instant) {
        self.listings.retain(|_, listing| now.duration_since(listing.loaded_at) < TTL);
    }

    fn insert(&mut self, key: Key, entries: Vec<FileEntry>, now: Instant) {
        self.prune(now);
        if entries.len() > MAX_ENTRIES {
            return;
        }
        let last_used = self.next_sequence();
        self.listings.insert(key, Listing { entries, loaded_at: now, last_used });
        let mut count: usize = self.listings.values().map(|listing| listing.entries.len()).sum();
        while self.listings.len() > MAX_LISTINGS || count > MAX_ENTRIES {
            let oldest = self.listings.iter().min_by_key(|(_, listing)| listing.last_used)
                .map(|(key, _)| key.clone()).expect("nonempty directory cache");
            count -= self.listings.remove(&oldest).unwrap().entries.len();
        }
    }
}

fn shared_cache() -> Arc<Mutex<Cache>> {
    static CACHE: OnceLock<Arc<Mutex<Cache>>> = OnceLock::new();
    CACHE.get_or_init(|| Arc::new(Mutex::new(Cache::default()))).clone()
}

pub(super) struct DirectoryLoadRequest {
    cache: Arc<Mutex<Cache>>,
    key: Key,
    ticket: Option<u64>,
    cached: Option<Vec<FileEntry>>,
}

impl DirectoryLoadRequest {
    /// Prepare before spawning background work so Refresh invalidates immediately.
    pub(super) fn new(path: &Path, visibility: EntryVisibility, remote: bool, policy: DirectoryLoadPolicy) -> Self {
        let eligible = remote
            && !super::archive_fs::is_archive_path(path)
            && !super::portable_devices::is_portable_path(path);
        Self::prepare(shared_cache(), path, visibility, eligible, policy, Instant::now())
    }

    fn prepare(cache: Arc<Mutex<Cache>>, path: &Path, visibility: EntryVisibility, eligible: bool,
        policy: DirectoryLoadPolicy, now: Instant) -> Self {
        let key = Key { path: path.to_path_buf(), visibility };
        let mut request = Self { cache, key, ticket: None, cached: None };
        {
            let mut cache = request.cache.lock().unwrap();
            cache.prune(now);
            if policy == DirectoryLoadPolicy::Fresh {
                // A disconnected mount can stop classifying as remote. Refresh
                // must still remove its previous snapshots and outstanding reads.
                cache.invalidate(|candidate| same_directory(candidate, path));
            } else if eligible {
                let last_used = cache.next_sequence();
                if let Some(listing) = cache.listings.get_mut(&request.key) {
                    listing.last_used = last_used;
                    request.cached = Some(listing.entries.clone());
                }
            }
            if eligible && request.cached.is_none() {
                let ticket = cache.next_sequence();
                cache.in_flight.insert(request.key.clone(), ticket);
                request.ticket = Some(ticket);
            }
        }
        request
    }

    pub(super) fn load(self) -> io::Result<Vec<FileEntry>> {
        self.load_with(load_entries, Instant::now)
    }

    fn load_with(mut self, loader: impl FnOnce(&Path, EntryVisibility) -> io::Result<Vec<FileEntry>>,
        now: impl FnOnce() -> Instant) -> io::Result<Vec<FileEntry>> {
        if let Some(entries) = self.cached.take() {
            return Ok(entries);
        }
        let result = loader(&self.key.path, self.key.visibility);
        if let Some(ticket) = self.ticket {
            let completed_at = now();
            let mut cache = self.cache.lock().unwrap();
            if cache.in_flight.get(&self.key) == Some(&ticket) {
                cache.in_flight.remove(&self.key);
                if let Ok(entries) = &result && entries.len() <= MAX_ENTRIES {
                    cache.insert(self.key.clone(), entries.clone(), completed_at);
                }
            }
        }
        result
    }
}

impl Drop for DirectoryLoadRequest {
fn drop(&mut self) {
        if let Some(ticket) = self.ticket {
            let mut cache = self.cache.lock().unwrap();
            if cache.in_flight.get(&self.key) == Some(&ticket) {
                cache.in_flight.remove(&self.key);
            }
        }
    }
}

fn same_directory(left: &Path, right: &Path) -> bool {
    path_is_same_or_descendant(left, right) && path_is_same_or_descendant(right, left)
}

/// Invalidate both before and after an operation, including early returns after
/// partial success. Include parents for membership changes and subtrees for moves.
pub(super) struct DirectoryMutation {
    paths: Vec<PathBuf>,
    cache: Arc<Mutex<Cache>>,
}

impl DirectoryMutation {
    pub(super) fn new(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self::with_cache(shared_cache(), paths)
    }

    fn with_cache(cache: Arc<Mutex<Cache>>, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        let guard = Self { paths: paths.into_iter().collect(), cache };
        guard.invalidate();
        guard
    }

    fn invalidate(&self) {
        let mut cache = self.cache.lock().unwrap();
        cache.invalidate(|candidate| self.paths.iter().any(|path| {
            path.parent().is_some_and(|parent| same_directory(candidate, parent))
                || path_is_same_or_descendant(candidate, path)
        }));
    }
}

impl Drop for DirectoryMutation {
    fn drop(&mut self) {
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> Arc<Mutex<Cache>> {
        Arc::new(Mutex::new(Cache::default()))
    }

    fn request(cache: &Arc<Mutex<Cache>>, path: &str, policy: DirectoryLoadPolicy, now: Instant) -> DirectoryLoadRequest {
        DirectoryLoadRequest::prepare(cache.clone(), Path::new(path), false.into(), true, policy, now)
    }

    fn entries(name: &str) -> Vec<FileEntry> {
        vec![FileEntry::test(name, false, Some(10), None)]
    }

    fn seed(cache: &Arc<Mutex<Cache>>, path: &str, now: Instant) {
        request(cache, path, DirectoryLoadPolicy::Cached, now)
            .load_with(|_, _| Ok(entries("original")), || now).unwrap();
    }

    fn hit(cache: &Arc<Mutex<Cache>>, path: &str, now: Instant) -> bool {
        request(cache, path, DirectoryLoadPolicy::Cached, now).cached.is_some()
    }

    #[test]
    fn navigation_reuses_listing_without_reading_and_hits_do_not_extend_expiry() {
        let cache = cache();
        let now = Instant::now();
        seed(&cache, "share/folder", now);
        let cached = request(&cache, "share/folder", DirectoryLoadPolicy::Cached, now + TTL - Duration::from_secs(1))
            .load_with(|_, _| panic!("cache hit must not read directory metadata"), || panic!("no completion time on hit"))
            .unwrap();
        assert_eq!(cached, entries("original"));
        assert!(!hit(&cache, "share/folder", now + TTL));
    }

    #[test]
    fn lifetime_starts_at_successful_completion_and_empty_directories_are_cached() {
        let cache = cache();
        let now = Instant::now();
        let finished = now + Duration::from_secs(30);
        request(&cache, "share/empty", DirectoryLoadPolicy::Cached, now)
            .load_with(|_, _| Ok(Vec::new()), || finished).unwrap();
        assert!(hit(&cache, "share/empty", now + TTL));
        assert!(!hit(&cache, "share/empty", finished + TTL));
    }

    #[test]
    fn refresh_clears_all_visibility_variants_only_for_current_folder_and_replaces_listing() {
        let cache = cache();
        let now = Instant::now();
        seed(&cache, "share/other", now);
        for visibility in [EntryVisibility::new(false, false), EntryVisibility::new(true, false),
            EntryVisibility::new(false, true), EntryVisibility::new(true, true)] {
            let variant = DirectoryLoadRequest::prepare(cache.clone(), Path::new("share/current"), visibility, true, DirectoryLoadPolicy::Cached, now);
            assert!(variant.cached.is_none());
            variant.load_with(|_, actual| {
                assert_eq!(actual, visibility);
                Ok(entries("variant"))
            }, || now).unwrap();
        }
        let refresh = request(&cache, "share/current", DirectoryLoadPolicy::Fresh, now);
        assert!(refresh.cached.is_none());
        assert!(cache.lock().unwrap().listings.keys().all(|key| key.path != Path::new("share/current")));
        assert!(hit(&cache, "share/other", now));
        refresh.load_with(|_, _| {
            assert!(cache.try_lock().is_ok(), "filesystem work must not hold the cache lock");
            Ok(entries("updated"))
        }, || now).unwrap();
        let cached = request(&cache, "share/current", DirectoryLoadPolicy::Cached, now);
        assert_eq!(cached.cached.as_ref().unwrap(), &entries("updated"));
    }

    #[test]
    fn failures_are_not_cached_and_failed_refresh_does_not_restore_cleared_listing() {
        let cache = cache();
        let now = Instant::now();
        seed(&cache, "share/folder", now);
        let result = request(&cache, "share/folder", DirectoryLoadPolicy::Fresh, now)
            .load_with(|_, _| Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied")), || now);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert!(!hit(&cache, "share/folder", now));
        assert!(cache.lock().unwrap().in_flight.is_empty());
    }

    #[test]
    fn refresh_still_invalidates_after_remote_mount_becomes_unavailable() {
        let cache = cache();
        let now = Instant::now();
        seed(&cache, "share/folder", now);
        let refresh = DirectoryLoadRequest::prepare(cache.clone(), Path::new("share/folder"), false.into(), false, DirectoryLoadPolicy::Fresh, now);
        assert!(cache.lock().unwrap().listings.is_empty());
        assert!(refresh.ticket.is_none());
        assert!(refresh.load_with(|_, _| Err(io::Error::other("disconnected")), || now).is_err());
        assert!(!hit(&cache, "share/folder", now));
    }

    #[test]
    fn newer_reads_win_and_refresh_rejects_older_visibility_reads() {
        let cache = cache();
        let now = Instant::now();
        let old = request(&cache, "share/folder", DirectoryLoadPolicy::Cached, now);
        let old_hidden = DirectoryLoadRequest::prepare(cache.clone(), Path::new("share/folder"), true.into(), true, DirectoryLoadPolicy::Cached, now);
        let refresh = request(&cache, "share/folder", DirectoryLoadPolicy::Fresh, now);
        refresh.load_with(|_, _| Ok(entries("new")), || now).unwrap();
        old.load_with(|_, _| Ok(entries("old")), || now).unwrap();
        old_hidden.load_with(|_, _| Ok(entries("hidden old")), || now).unwrap();
        let locked = cache.lock().unwrap();
        assert_eq!(locked.listings.len(), 1);
        assert_eq!(locked.listings.values().next().unwrap().entries, entries("new"));
        assert!(locked.in_flight.is_empty());
    }

    #[test]
    fn overlapping_cache_misses_and_cancelled_requests_do_not_remove_newer_tickets() {
        let cache = cache();
        let now = Instant::now();
        let old = request(&cache, "share/folder", DirectoryLoadPolicy::Cached, now);
        let newer = request(&cache, "share/folder", DirectoryLoadPolicy::Cached, now);
        drop(old);
        assert_eq!(cache.lock().unwrap().in_flight.len(), 1);
        newer.load_with(|_, _| Ok(entries("new")), || now).unwrap();
        assert!(hit(&cache, "share/folder", now));
        let cancelled = request(&cache, "share/cancelled", DirectoryLoadPolicy::Cached, now);
        drop(cancelled);
        assert!(cache.lock().unwrap().in_flight.is_empty());
    }

    #[test]
    fn ineligible_locations_do_not_populate_cache() {
        let cache = cache();
        let now = Instant::now();
        for policy in [DirectoryLoadPolicy::Cached, DirectoryLoadPolicy::Fresh] {
            DirectoryLoadRequest::prepare(cache.clone(), Path::new("local/folder"), false.into(), false, policy, now)
                .load_with(|_, _| Ok(entries("local")), || now).unwrap();
        }
        assert!(cache.lock().unwrap().listings.is_empty());
        assert!(cache.lock().unwrap().in_flight.is_empty());
        let portable = super::super::portable_devices::virtual_root().join("cache-test-device");
        #[cfg(target_os = "windows")]
        let archive = Path::new(r"\\explorer.archive\archives\cache-test");
        #[cfg(not(target_os = "windows"))]
        let archive = Path::new("/__explorer_archive__/archives/cache-test");
        for path in [portable.as_path(), archive] {
            assert!(DirectoryLoadRequest::new(path, false.into(), true, DirectoryLoadPolicy::Cached).ticket.is_none());
        }
    }

    #[test]
    fn least_recently_used_listing_is_evicted_and_entry_budget_is_enforced() {
        let cache = cache();
        let now = Instant::now();
        for i in 0..MAX_LISTINGS {
            seed(&cache, &format!("share/{i}"), now);
        }
        assert!(hit(&cache, "share/0", now));
        seed(&cache, "share/new", now);
        assert!(hit(&cache, "share/0", now));
        assert!(!hit(&cache, "share/1", now));
        assert_eq!(cache.lock().unwrap().listings.len(), MAX_LISTINGS);

        let cache = self::cache();
        for path in ["share/large-a", "share/large-b"] {
            request(&cache, path, DirectoryLoadPolicy::Cached, now)
                .load_with(|_, _| Ok(vec![FileEntry::test("file", false, None, None); MAX_ENTRIES / 2 + 1]), || now).unwrap();
        }
        assert!(!hit(&cache, "share/large-a", now));
        assert!(hit(&cache, "share/large-b", now));
        request(&cache, "share/oversized", DirectoryLoadPolicy::Cached, now)
            .load_with(|_, _| Ok(vec![FileEntry::test("file", false, None, None); MAX_ENTRIES + 1]), || now).unwrap();
        assert!(!hit(&cache, "share/oversized", now));
        assert!(hit(&cache, "share/large-b", now));
    }

    #[test]
    fn mutation_invalidates_source_destination_and_descendants_even_after_partial_failure() {
        let cache = cache();
        let now = Instant::now();
        let affected = ["share/source", "share/source/folder", "share/source/folder/child", "share/dest", "share/dest/folder/child"];
        for path in affected.into_iter().chain(["share/unrelated", "share/source/folder-other"]) {
            seed(&cache, path, now);
        }
        let result: io::Result<()> = (|| {
            let _guard = DirectoryMutation::with_cache(cache.clone(), [PathBuf::from("share/source/folder"), PathBuf::from("share/dest/folder")]);
            for path in affected {
                assert!(!hit(&cache, path, now));
            }
            // A concurrent tab reads halfway through a partially successful move.
            seed(&cache, "share/dest/folder/child", now);
            Err(io::Error::other("partial operation failed"))
        })();
        assert!(result.is_err());
        for path in affected {
            assert!(!hit(&cache, path, now));
        }
        assert!(hit(&cache, "share/unrelated", now));
        assert!(hit(&cache, "share/source/folder-other", now));
    }

    #[test]
    fn reads_started_before_or_during_mutation_cannot_repopulate_cache() {
        let cache = cache();
        let now = Instant::now();
        let old = request(&cache, "share/folder", DirectoryLoadPolicy::Cached, now);
        let mutation = DirectoryMutation::with_cache(cache.clone(), [PathBuf::from("share/folder/file")]);
        let during = request(&cache, "share/folder", DirectoryLoadPolicy::Cached, now);
        drop(mutation);
        old.load_with(|_, _| Ok(entries("old")), || now).unwrap();
        during.load_with(|_, _| Ok(entries("partial")), || now).unwrap();
        assert!(!hit(&cache, "share/folder", now));
    }

    #[test]
    fn real_create_copy_move_and_delete_invalidate_shared_snapshots() {
        use super::super::{explorer_fs::ExplorerFs, filesystem, test_support::TempDir};
        let temp = TempDir::new();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&destination).unwrap();
        let read = |path: &Path| DirectoryLoadRequest::new(path, false.into(), true, DirectoryLoadPolicy::Cached).load().unwrap();
        assert!(read(&source).is_empty());
        ExplorerFs::new().create_empty_file(&source.join("file")).unwrap();
        assert_eq!(read(&source).len(), 1);
        assert!(read(&destination).is_empty());
        filesystem::copy_paths_to_directory(&[source.join("file")], &destination).unwrap();
        assert_eq!(read(&destination).len(), 1);
        let moved = temp.path().join("moved");
        std::fs::create_dir(&moved).unwrap();
        assert!(read(&moved).is_empty());
        filesystem::move_paths_to_directory(&[destination.join("file")], &moved).unwrap();
        assert!(read(&destination).is_empty());
        assert_eq!(read(&moved).len(), 1);
        filesystem::remove_paths_permanently(&[moved.join("file")]).unwrap();
        assert!(read(&moved).is_empty());
    }

    #[test]
    fn remote_back_forward_navigation_uses_cache_and_preserves_history_and_sorting() {
        use super::super::{
            navigation::HistoryMode,
            test_support::{RemoteDriveForTest, TempDir},
            view::ExplorerView,
        };
        let temp = TempDir::new();
        let _remote = RemoteDriveForTest::new(temp.path());
        let child = temp.path().join("child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(temp.path().join("z.txt"), b"z").unwrap();
        std::fs::write(temp.path().join("a.txt"), b"a").unwrap();
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.navigate_to_directory(child.clone(), HistoryMode::Record);
        std::fs::write(temp.path().join("external.txt"), b"new").unwrap();
        view.navigate_back();
        assert_eq!(view.path(), temp.path());
        assert_eq!(view.entries.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(), ["child", "a.txt", "z.txt"]);
        assert_eq!(view.selected_paths(), [child.clone()]);
        assert_eq!(view.forward_stack, vec![child.clone()]);
        view.navigate_forward();
        assert_eq!(view.path(), child.as_path());
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
    }
}
