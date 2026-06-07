#[cfg(debug_assertions)]
use std::time::Instant;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
#[cfg(debug_assertions)]
use thousands::Separable;

#[cfg(debug_assertions)]
macro_rules! recursive_search_timing {
    ($generation:expr, $elapsed:expr, $($message:tt)*) => {
        eprintln!(
            "[recursive-search:{}] {:<10.3?} {}",
            $generation,
            $elapsed,
            format_args!($($message)*)
        );
    };
}

use jwalk::{WalkDir, rayon::prelude::*};

use crate::{
    explorer::{
        entry::FileEntry,
        filesystem::{should_hide_entry, should_hide_entry_with_metadata},
    },
    ngram::{NgramIndex, NgramIndexBuilder, NgramSearchSession},
};

const CANCELLATION_CHECK_INTERVAL: usize = 5120;

pub(super) struct RecursiveSearchIndex {
    index: NgramIndex<FileEntry>,
    session: Mutex<NgramSearchSession>,
}

impl RecursiveSearchIndex {
    pub(super) fn new(index: NgramIndex<FileEntry>) -> Self {
        Self {
            index,
            session: Mutex::new(NgramSearchSession::new()),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.index.len()
    }

    fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

#[derive(Clone)]
pub(super) struct RecursiveSearchCache {
    pub(super) root: PathBuf,
    pub(super) show_hidden_files: bool,
    pub(super) index: Arc<RecursiveSearchIndex>,
}

#[derive(Clone)]
pub(super) struct RecursiveSearchOutput {
    pub(super) generation: u64,
    pub(super) root: PathBuf,
    pub(super) query: String,
    pub(super) show_hidden_files: bool,
    pub(super) scanned_index: Arc<RecursiveSearchIndex>,
    pub(super) entries: Vec<FileEntry>,
}

pub(super) fn recursive_search_entries(
    generation: u64,
    root: PathBuf,
    query: String,
    show_hidden_files: bool,
    cached_search: Option<RecursiveSearchCache>,
    cancel: Arc<AtomicBool>,
) -> RecursiveSearchOutput {
    #[cfg(debug_assertions)]
    let total_started = Instant::now();
    #[cfg(debug_assertions)]
    let cache_hit = cached_search.is_some();

    let scanned_index = match cached_search {
        Some(cache) => cache.index,
        None => {
            #[cfg(debug_assertions)]
            let scan_started = Instant::now();
            let index = Arc::new(RecursiveSearchIndex::new(scan_recursive_paths(
                &root,
                show_hidden_files,
                cancel.clone(),
            )));
            #[cfg(debug_assertions)]
            recursive_search_timing!(
                generation,
                scan_started.elapsed(),
                "scan paths={} cancelled={}",
                index.len().separate_with_commas(),
                cancel.load(Ordering::Relaxed)
            );
            index
        }
    };

    #[cfg(debug_assertions)]
    let filter_started = Instant::now();
    let entries = if cancel.load(Ordering::Relaxed) {
        Vec::new()
    } else {
        filter_recursive_paths(&scanned_index, &query, &cancel, show_hidden_files)
    };
    #[cfg(debug_assertions)]
    recursive_search_timing!(
        generation,
        filter_started.elapsed(),
        "filter matches={} cancelled={}",
        entries.len(),
        cancel.load(Ordering::Relaxed)
    );

    #[cfg(debug_assertions)]
    recursive_search_timing!(
        generation,
        total_started.elapsed(),
        "total query={query:?} cache_hit={cache_hit} paths={} entries={} cancelled={}",
        scanned_index.len(),
        entries.len(),
        cancel.load(Ordering::Relaxed)
    );

    RecursiveSearchOutput {
        generation,
        root,
        query,
        show_hidden_files,
        scanned_index,
        entries,
    }
}

pub(super) fn scan_recursive_paths(
    root: &Path,
    show_hidden_files: bool,
    cancel: Arc<AtomicBool>,
) -> NgramIndex<FileEntry> {
    if cancel.load(Ordering::Relaxed) {
        return NgramIndexBuilder::new().finish();
    }

    let process_cancel = cancel.clone();
    let walker = WalkDir::new(root)
        .sort(false)
        .skip_hidden(!show_hidden_files)
        .follow_links(false)
        .min_depth(1)
        .process_read_dir(move |_, path, _, children| {
            let should_hide_this_dir = !show_hidden_files
                && should_hide_entry(path.file_name().unwrap(), path, show_hidden_files);

            if process_cancel.load(Ordering::Relaxed) || should_hide_this_dir {
                children.clear();
                return;
            }
        });

    let entries = walker
        .into_iter()
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    let materialized = entries
        .into_par_iter()
        .filter_map(|entry| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let path = entry.path();
            let link_metadata = entry.metadata().ok()?;
            if should_hide_entry_with_metadata(
                entry.file_name(),
                &path,
                show_hidden_files,
                &link_metadata,
            ) {
                return None;
            }
            FileEntry::from_path_with_link_metadata(path, link_metadata)
        })
        .collect::<Vec<_>>();

    if cancel.load(Ordering::Relaxed) {
        return NgramIndexBuilder::new().finish();
    }

    let mut builder = NgramIndexBuilder::new();
    for entry in materialized {
        let normalized_name = entry.name.to_lowercase();
        builder.add(&normalized_name, entry);
    }
    builder.finish()
}

fn filter_recursive_paths(
    index: &RecursiveSearchIndex,
    query: &str,
    cancel: &AtomicBool,
    _: bool,
) -> Vec<FileEntry> {
    if index.is_empty() || query.trim().is_empty() {
        return Vec::new();
    }

    let query = query.to_lowercase();
    let mut session = index
        .session
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let result_ids = session.search(&index.index, &query);
    let mut entries = Vec::with_capacity(result_ids.len());
    for (result_index, &id) in result_ids.iter().enumerate() {
        if result_index % CANCELLATION_CHECK_INTERVAL == 0 && cancel.load(Ordering::Relaxed) {
            return Vec::new();
        }
        if let Some(entry) = index.index.get(id) {
            entries.push(entry.clone());
        }
    }

    if cancel.load(Ordering::Relaxed) {
        Vec::new()
    } else {
        entries
    }
}

#[cfg(feature = "benchmarks")]
#[doc(hidden)]
pub mod benchmark_support {
    use super::*;

    pub struct ScannedPaths {
        root: PathBuf,
        show_hidden_files: bool,
        index: Arc<RecursiveSearchIndex>,
    }

    pub struct FilteredPaths(Vec<FileEntry>);

    pub struct SearchResult(RecursiveSearchOutput);

    impl ScannedPaths {
        pub fn len(&self) -> usize {
            self.index.len()
        }

        pub fn ngram_count(&self) -> usize {
            self.index.index.ngram_count()
        }

        pub fn posting_count(&self) -> usize {
            self.index.index.posting_count()
        }

        pub fn posting_bytes(&self) -> usize {
            self.index.index.posting_bytes()
        }
    }

    impl FilteredPaths {
        pub fn len(&self) -> usize {
            self.0.len()
        }
    }

    impl SearchResult {
        pub fn entry_count(&self) -> usize {
            self.0.entries.len()
        }

        pub fn scanned_path_count(&self) -> usize {
            self.0.scanned_index.len()
        }
    }

    pub fn scan(root: &Path, show_hidden_files: bool) -> ScannedPaths {
        ScannedPaths {
            root: root.to_path_buf(),
            show_hidden_files,
            index: Arc::new(RecursiveSearchIndex::new(scan_recursive_paths(
                root,
                show_hidden_files,
                Arc::new(AtomicBool::new(false)),
            ))),
        }
    }

    pub fn filter(paths: &ScannedPaths, query: &str) -> FilteredPaths {
        FilteredPaths(filter_recursive_paths(
            &paths.index,
            query,
            &AtomicBool::new(false),
            paths.show_hidden_files,
        ))
    }

    pub fn cached_search(paths: &ScannedPaths, query: &str) -> SearchResult {
        SearchResult(recursive_search_entries(
            0,
            paths.root.clone(),
            query.to_owned(),
            paths.show_hidden_files,
            Some(RecursiveSearchCache {
                root: paths.root.clone(),
                show_hidden_files: paths.show_hidden_files,
                index: paths.index.clone(),
            }),
            Arc::new(AtomicBool::new(false)),
        ))
    }

    pub fn uncached_search(root: &Path, query: &str, show_hidden_files: bool) -> SearchResult {
        SearchResult(recursive_search_entries(
            0,
            root.to_path_buf(),
            query.to_owned(),
            show_hidden_files,
            None,
            Arc::new(AtomicBool::new(false)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;
    use std::fs;

    fn recursive_index(paths: Vec<PathBuf>) -> Arc<RecursiveSearchIndex> {
        let mut builder = NgramIndexBuilder::new();
        for path in paths {
            let entry = FileEntry::from_path(path).expect("materialize recursive entry");
            let file_name = entry.name.to_lowercase();
            builder.add(&file_name, entry);
        }
        Arc::new(RecursiveSearchIndex::new(builder.finish()))
    }

    fn scan(root: &Path, show_hidden_files: bool, cancel: Arc<AtomicBool>) -> RecursiveSearchIndex {
        RecursiveSearchIndex::new(scan_recursive_paths(root, show_hidden_files, cancel))
    }

    fn assert_index_contains(index: &RecursiveSearchIndex, path: &Path) {
        let query = path.file_name().unwrap().to_string_lossy().to_lowercase();
        let mut session = index.session.lock().expect("lock search session");
        let ids = session.search(&index.index, &query).to_vec();
        assert!(
            ids.iter()
                .any(|&id| index.index.get(id).is_some_and(|entry| entry.path == path)),
            "index did not contain {}",
            path.display()
        );
    }

    fn entry_paths(entries: Vec<FileEntry>) -> Vec<PathBuf> {
        entries.iter().map(|entry| entry.path.clone()).collect()
    }

    #[test]
    fn recursive_scan_includes_nested_items() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir(&child).expect("create child");
        fs::write(temp.path().join("root.txt"), b"root").expect("create root file");
        fs::write(child.join("nested.txt"), b"nested").expect("create nested file");

        let cancel = Arc::new(AtomicBool::new(false));
        let index = scan(temp.path(), true, cancel);

        assert_eq!(index.len(), 3);
        assert_index_contains(&index, &child);
        assert_index_contains(&index, &child.join("nested.txt"));
        assert_index_contains(&index, &temp.path().join("root.txt"));
    }

    #[test]
    fn recursive_scan_honors_hidden_and_metadata_filtering() {
        let temp = TempDir::new();
        fs::write(temp.path().join(".hidden"), b"hidden").expect("create hidden");
        fs::write(temp.path().join(".DS_Store"), b"metadata").expect("create metadata");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible");

        let cancel = Arc::new(AtomicBool::new(false));
        let index = scan(temp.path(), false, cancel);

        assert_eq!(index.len(), 1);
        assert_index_contains(&index, &temp.path().join("visible.txt"));
    }

    #[test]
    fn recursive_scan_does_not_recurse_into_hidden_directories_when_hidden_files_are_off() {
        let temp = TempDir::new();
        let hidden = temp.path().join(".hidden-dir");
        fs::create_dir(&hidden).expect("create hidden directory");
        fs::write(hidden.join("nested.txt"), b"nested").expect("create nested file");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible");

        let cancel = Arc::new(AtomicBool::new(false));
        let index = scan(temp.path(), false, cancel);

        assert_eq!(index.len(), 1);
        assert_index_contains(&index, &temp.path().join("visible.txt"));
    }

    #[test]
    fn recursive_scan_includes_hidden_directories_when_hidden_files_are_on() {
        let temp = TempDir::new();
        let hidden = temp.path().join(".hidden-dir");
        fs::create_dir(&hidden).expect("create hidden directory");
        fs::write(hidden.join("nested.txt"), b"nested").expect("create nested file");
        fs::write(hidden.join(".DS_Store"), b"metadata").expect("create nested metadata");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible");

        let cancel = Arc::new(AtomicBool::new(false));
        let index = scan(temp.path(), true, cancel);

        assert_eq!(index.len(), 3);
        assert_index_contains(&index, &hidden);
        assert_index_contains(&index, &hidden.join("nested.txt"));
        assert_index_contains(&index, &temp.path().join("visible.txt"));
    }

    #[test]
    fn recursive_scan_honors_existing_cancellation() {
        let temp = TempDir::new();
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible");

        let cancel = Arc::new(AtomicBool::new(true));
        let index = scan(temp.path(), true, cancel);

        assert!(index.is_empty());
    }

    #[test]
    fn filter_matches_basename_only() {
        let temp = TempDir::new();
        let reports = temp.path().join("reports");
        let other = temp.path().join("other");
        fs::create_dir(&reports).expect("create reports");
        fs::create_dir(&other).expect("create other");
        let image = reports.join("image.png");
        let report = other.join("report.txt");
        fs::write(&image, b"image").expect("create image");
        fs::write(&report, b"report").expect("create report");
        let index = recursive_index(vec![image, report.clone()]);
        let cancel = AtomicBool::new(false);

        assert_eq!(
            entry_paths(filter_recursive_paths(&index, "report", &cancel, true)),
            vec![report]
        );
    }

    #[test]
    fn filter_matches_case_insensitively() {
        let temp = TempDir::new();
        let report = temp.path().join("Annual Report.txt");
        fs::write(&report, b"report").expect("create report");
        let index = recursive_index(vec![report.clone()]);
        let cancel = AtomicBool::new(false);

        assert_eq!(
            entry_paths(filter_recursive_paths(&index, "REPORT", &cancel, true)),
            vec![report]
        );
    }

    #[test]
    fn filter_returns_ranked_partial_ngram_matches() {
        let temp = TempDir::new();
        let strong = temp.path().join("abcdef.txt");
        let weak = temp.path().join("abcxxx.txt");
        fs::write(&strong, b"strong").expect("create strong match");
        fs::write(&weak, b"weak").expect("create weak match");
        let index = recursive_index(vec![weak.clone(), strong.clone()]);
        let cancel = AtomicBool::new(false);

        assert_eq!(
            entry_paths(filter_recursive_paths(&index, "abcdef", &cancel, true)),
            vec![strong, weak]
        );
    }

    #[test]
    fn filter_preserves_scan_order_for_duplicate_basenames() {
        let temp = TempDir::new();
        let z = temp.path().join("z");
        let a = temp.path().join("a");
        fs::create_dir(&z).expect("create z");
        fs::create_dir(&a).expect("create a");
        let expected = vec![z.join("report.txt"), a.join("report.txt")];
        fs::write(&expected[0], b"z").expect("create z report");
        fs::write(&expected[1], b"a").expect("create a report");
        let index = recursive_index(expected.clone());
        let cancel = AtomicBool::new(false);

        assert_eq!(
            entry_paths(filter_recursive_paths(&index, "report", &cancel, true)),
            expected
        );
    }

    #[test]
    fn filter_honors_existing_cancellation() {
        let temp = TempDir::new();
        let report = temp.path().join("report.txt");
        fs::write(&report, b"report").expect("create report");
        let index = recursive_index(vec![report]);
        let cancel = AtomicBool::new(true);

        assert!(filter_recursive_paths(&index, "report", &cancel, true).is_empty());
    }

    #[test]
    fn filter_returns_no_matches_for_queries_shorter_than_ngram_length() {
        let temp = TempDir::new();
        let report = temp.path().join("report.txt");
        fs::write(&report, b"report").expect("create report");
        let index = recursive_index(vec![report]);
        let cancel = AtomicBool::new(false);

        assert!(filter_recursive_paths(&index, "re", &cancel, true).is_empty());
    }

    #[test]
    fn filter_skips_hidden_index_results_when_hidden_files_are_off() {
        let temp = TempDir::new();
        let hidden = temp.path().join(".hidden-report.txt");
        let visible = temp.path().join("visible-report.txt");
        fs::write(&hidden, b"hidden").expect("create hidden report");
        fs::write(&visible, b"visible").expect("create visible report");
        let index = scan(temp.path(), false, Arc::new(AtomicBool::new(false)));
        let cancel = AtomicBool::new(false);

        assert_eq!(
            entry_paths(filter_recursive_paths(&index, "report", &cancel, false)),
            vec![visible]
        );
    }

    #[test]
    fn recursive_search_reuses_shared_cached_paths() {
        let temp = TempDir::new();
        let report = temp.path().join("report.txt");
        let notes = temp.path().join("notes.txt");
        fs::write(&report, b"report").expect("create report");
        fs::write(&notes, b"notes").expect("create notes");
        let cancel = Arc::new(AtomicBool::new(false));
        let index = recursive_index(vec![report.clone(), notes]);
        let cache = RecursiveSearchCache {
            root: temp.path().to_path_buf(),
            show_hidden_files: true,
            index: index.clone(),
        };

        let output = recursive_search_entries(
            1,
            temp.path().to_path_buf(),
            "report".to_owned(),
            true,
            Some(cache),
            cancel,
        );

        assert!(Arc::ptr_eq(&output.scanned_index, &index));
        assert_eq!(output.entries.len(), 1);
        assert_eq!(output.entries[0].path, report);
    }

    #[test]
    fn recursive_search_session_updates_exactly_across_cached_queries() {
        let temp = TempDir::new();
        let report = temp.path().join("report.txt");
        let notes = temp.path().join("notes.txt");
        fs::write(&report, b"report").expect("create report");
        fs::write(&notes, b"notes").expect("create notes");
        let index = recursive_index(vec![report.clone(), notes.clone()]);
        let cancel = AtomicBool::new(false);

        assert_eq!(
            entry_paths(filter_recursive_paths(&index, "report", &cancel, true)),
            vec![report.clone()]
        );
        assert_eq!(
            entry_paths(filter_recursive_paths(&index, "notes", &cancel, true)),
            vec![notes]
        );
        assert_eq!(
            entry_paths(filter_recursive_paths(&index, "report", &cancel, true)),
            vec![report]
        );
    }

    #[test]
    fn recursive_search_uses_cached_entry_after_file_is_deleted() {
        let temp = TempDir::new();
        let report = temp.path().join("report.txt");
        fs::write(&report, b"report").expect("create report");
        let index = recursive_index(vec![report.clone()]);
        fs::remove_file(&report).expect("remove report");
        let cancel = Arc::new(AtomicBool::new(false));
        let cached_search = RecursiveSearchCache {
            root: temp.path().to_path_buf(),
            show_hidden_files: true,
            index,
        };

        let output = recursive_search_entries(
            1,
            temp.path().to_path_buf(),
            "report".to_owned(),
            true,
            Some(cached_search),
            cancel,
        );

        assert_eq!(output.entries.len(), 1);
        assert_eq!(output.entries[0].path, report);
    }

    #[cfg(unix)]
    #[test]
    fn recursive_scan_does_not_recurse_into_symlinked_directories() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new();
        let real = temp.path().join("real");
        let link = temp.path().join("link");
        fs::create_dir(&real).expect("create real directory");
        fs::write(real.join("nested.txt"), b"nested").expect("create nested file");
        symlink(&real, &link).expect("create symlink");

        let cancel = Arc::new(AtomicBool::new(false));
        let index = scan(temp.path(), true, cancel);

        assert_eq!(index.len(), 3);
        assert_index_contains(&index, &link);
        assert_index_contains(&index, &real);
        assert_index_contains(&index, &real.join("nested.txt"));
    }

    #[cfg(windows)]
    #[test]
    fn recursive_scan_does_not_recurse_into_symlinked_directories() {
        use std::io;
        use std::os::windows::fs::symlink_dir;

        let temp = TempDir::new();
        let real = temp.path().join("real");
        let link = temp.path().join("link");
        fs::create_dir(&real).expect("create real directory");
        fs::write(real.join("nested.txt"), b"nested").expect("create nested file");
        match symlink_dir(&real, &link) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => return,
            Err(error) => panic!("create symlink: {error}"),
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let index = scan(temp.path(), true, cancel);

        assert_eq!(index.len(), 3);
        assert_index_contains(&index, &link);
        assert_index_contains(&index, &real);
        assert_index_contains(&index, &real.join("nested.txt"));
    }
}
