use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

#[cfg(debug_assertions)]
use std::time::Instant;
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

use jwalk::{WalkDirGeneric, rayon::prelude::*};

use crate::explorer::{
    entry::FileEntry,
    filesystem::{should_hide_entry, should_hide_entry_with_metadata},
};

const CANCELLATION_CHECK_INTERVAL: usize = 5120;
const PARALLEL_MATERIALIZATION_THRESHOLD: usize = 128;

#[derive(Default)]
pub(super) struct RecursiveSearchProgress {
    scanning: AtomicBool,
    scanned_paths: AtomicUsize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RecursiveSearchProgressSnapshot {
    Scanning(usize),
    Searching(Option<usize>),
}

impl RecursiveSearchProgress {
    pub(super) fn snapshot(&self) -> RecursiveSearchProgressSnapshot {
        let scanned_paths = self.scanned_paths.load(Ordering::Relaxed);
        if self.scanning.load(Ordering::Relaxed) {
            RecursiveSearchProgressSnapshot::Scanning(scanned_paths)
        } else {
            RecursiveSearchProgressSnapshot::Searching((scanned_paths > 0).then_some(scanned_paths))
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct RecursiveSearchPath {
    parent_path: Arc<Path>,
    file_name: OsString,
    normalized_name: String,
}

impl RecursiveSearchPath {
    fn path(&self) -> PathBuf {
        self.parent_path.join(&self.file_name)
    }
}

#[derive(Clone)]
pub(super) struct RecursiveSearchCache {
    pub(super) root: PathBuf,
    pub(super) show_hidden_files: bool,
    pub(super) paths: Arc<Vec<RecursiveSearchPath>>,
}

#[derive(Clone)]
pub(super) struct RecursiveSearchOutput {
    pub(super) generation: u64,
    pub(super) root: PathBuf,
    pub(super) query: String,
    pub(super) show_hidden_files: bool,
    pub(super) scanned_paths: Arc<Vec<RecursiveSearchPath>>,
    pub(super) entries: Vec<FileEntry>,
}

pub(super) fn recursive_search_entries(
    generation: u64,
    root: PathBuf,
    query: String,
    show_hidden_files: bool,
    cached_search: Option<RecursiveSearchCache>,
    cancel: Arc<AtomicBool>,
    progress: Arc<RecursiveSearchProgress>,
) -> RecursiveSearchOutput {
    #[cfg(debug_assertions)]
    let total_started = Instant::now();
    #[cfg(debug_assertions)]
    let cache_hit = cached_search.is_some();

    let scanned_paths = match cached_search {
        Some(cache) => {
            progress
                .scanned_paths
                .store(cache.paths.len(), Ordering::Relaxed);
            cache.paths
        }
        None => {
            #[cfg(debug_assertions)]
            let scan_started = Instant::now();
            progress.scanned_paths.store(0, Ordering::Relaxed);
            progress.scanning.store(true, Ordering::Relaxed);
            let paths = scan_recursive_paths_with_progress(
                &root,
                show_hidden_files,
                cancel.clone(),
                Some(&progress.scanned_paths),
            );
            progress.scanning.store(false, Ordering::Relaxed);
            #[cfg(debug_assertions)]
            recursive_search_timing!(
                generation,
                scan_started.elapsed(),
                "scan paths={} cancelled={}",
                paths.len().separate_with_commas(),
                cancel.load(Ordering::Relaxed)
            );
            paths
        }
    };

    #[cfg(debug_assertions)]
    let filter_started = Instant::now();
    let result_paths = if cancel.load(Ordering::Relaxed) {
        Vec::new()
    } else {
        filter_recursive_paths(&scanned_paths, &query, &cancel)
    };
    #[cfg(debug_assertions)]
    recursive_search_timing!(
        generation,
        filter_started.elapsed(),
        "filter matches={} cancelled={}",
        result_paths.len(),
        cancel.load(Ordering::Relaxed)
    );

    #[cfg(debug_assertions)]
    let result_path_count = result_paths.len();
    #[cfg(debug_assertions)]
    let materialize_started = Instant::now();
    let entries =
        materialize_recursive_entries(&scanned_paths, &result_paths, show_hidden_files, &cancel);
    #[cfg(debug_assertions)]
    recursive_search_timing!(
        generation,
        materialize_started.elapsed(),
        "materialize matches={result_path_count} entries={}",
        entries.len()
    );
    #[cfg(debug_assertions)]
    recursive_search_timing!(
        generation,
        total_started.elapsed(),
        "total query={query:?} cache_hit={cache_hit} paths={} matches={result_path_count} entries={} cancelled={}",
        scanned_paths.len(),
        entries.len(),
        cancel.load(Ordering::Relaxed)
    );

    RecursiveSearchOutput {
        generation,
        root,
        query,
        show_hidden_files,
        scanned_paths,
        entries,
    }
}

#[cfg(any(test, feature = "benchmarks"))]
pub(super) fn scan_recursive_paths(
    root: &Path,
    show_hidden_files: bool,
    cancel: Arc<AtomicBool>,
) -> Arc<Vec<RecursiveSearchPath>> {
    scan_recursive_paths_with_progress(root, show_hidden_files, cancel, None)
}

fn scan_recursive_paths_with_progress(
    root: &Path,
    show_hidden_files: bool,
    cancel: Arc<AtomicBool>,
    progress: Option<&AtomicUsize>,
) -> Arc<Vec<RecursiveSearchPath>> {
    if cancel.load(Ordering::Relaxed) {
        return Arc::new(Vec::new());
    }

    let process_cancel = cancel.clone();
    let walker = WalkDirGeneric::<((), String)>::new(root)
        .sort(false)
        .skip_hidden(!show_hidden_files)
        .follow_links(false)
        .min_depth(1)
        .process_read_dir(move |_, _, _, children| {
            if process_cancel.load(Ordering::Relaxed) {
                children.clear();
                return;
            }

            for child in children.iter_mut() {
                let Ok(entry) = child else {
                    continue;
                };
                entry.client_state = entry.file_name().to_string_lossy().to_lowercase();

                if entry.file_type().is_dir() {
                    let path = entry.path();
                    if should_hide_entry(entry.file_name(), &path, show_hidden_files) {
                        entry.read_children_path = None;
                    }
                }
            }
        });

    let mut paths = Vec::new();
    for (index, entry_result) in walker.into_iter().enumerate() {
        if index % CANCELLATION_CHECK_INTERVAL == 0 && cancel.load(Ordering::Relaxed) {
            break;
        }

        if let Ok(entry) = entry_result {
            paths.push(RecursiveSearchPath {
                parent_path: entry.parent_path,
                file_name: entry.file_name,
                normalized_name: entry.client_state,
            });
            if let Some(progress) = progress {
                progress.store(paths.len(), Ordering::Relaxed);
            }
        }
    }

    if cancel.load(Ordering::Relaxed) {
        return Arc::new(Vec::new());
    }

    Arc::new(paths)
}

fn filter_recursive_paths(
    paths: &[RecursiveSearchPath],
    query: &str,
    cancel: &AtomicBool,
) -> Vec<usize> {
    if paths.is_empty() || query.trim().is_empty() {
        return Vec::new();
    }

    let query = query.to_lowercase();
    let mut matches = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        if index % CANCELLATION_CHECK_INTERVAL == 0 && cancel.load(Ordering::Relaxed) {
            return Vec::new();
        }
        if path.normalized_name.contains(&query) {
            matches.push(index);
        }
    }

    if cancel.load(Ordering::Relaxed) {
        Vec::new()
    } else {
        matches
    }
}

fn materialize_recursive_entries(
    paths: &[RecursiveSearchPath],
    result_indices: &[usize],
    show_hidden_files: bool,
    cancel: &AtomicBool,
) -> Vec<FileEntry> {
    if result_indices.is_empty() || cancel.load(Ordering::Relaxed) {
        return Vec::new();
    }

    let materialized = if result_indices.len() < PARALLEL_MATERIALIZATION_THRESHOLD {
        result_indices
            .iter()
            .filter_map(|&index| {
                materialize_recursive_entry(&paths[index], show_hidden_files, cancel)
            })
            .collect()
    } else {
        result_indices
            .par_iter()
            .map(|&index| materialize_recursive_entry(&paths[index], show_hidden_files, cancel))
            .collect::<Vec<_>>()
            .into_iter()
            .flatten()
            .collect()
    };

    if cancel.load(Ordering::Relaxed) {
        Vec::new()
    } else {
        materialized
    }
}

fn materialize_recursive_entry(
    recursive_path: &RecursiveSearchPath,
    show_hidden_files: bool,
    cancel: &AtomicBool,
) -> Option<FileEntry> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    let path = recursive_path.path();
    let link_metadata = fs::symlink_metadata(&path).ok()?;
    if should_hide_entry_with_metadata(
        &recursive_path.file_name,
        &path,
        show_hidden_files,
        &link_metadata,
    ) || cancel.load(Ordering::Relaxed)
    {
        return None;
    }

    FileEntry::from_path_with_link_metadata(path, link_metadata)
}

#[cfg(feature = "benchmarks")]
#[doc(hidden)]
pub mod benchmark_support {
    use super::*;

    pub struct ScannedPaths {
        root: PathBuf,
        show_hidden_files: bool,
        paths: Arc<Vec<RecursiveSearchPath>>,
    }

    pub struct FilteredPaths(Vec<usize>);

    pub struct MaterializedEntries(Vec<FileEntry>);

    pub struct SearchResult(RecursiveSearchOutput);

    impl ScannedPaths {
        pub fn len(&self) -> usize {
            self.paths.len()
        }

        pub fn is_empty(&self) -> bool {
            self.paths.is_empty()
        }
    }

    impl FilteredPaths {
        pub fn len(&self) -> usize {
            self.0.len()
        }

        pub fn is_empty(&self) -> bool {
            self.0.is_empty()
        }
    }

    impl MaterializedEntries {
        pub fn len(&self) -> usize {
            self.0.len()
        }

        pub fn is_empty(&self) -> bool {
            self.0.is_empty()
        }
    }

    impl SearchResult {
        pub fn entry_count(&self) -> usize {
            self.0.entries.len()
        }

        pub fn scanned_path_count(&self) -> usize {
            self.0.scanned_paths.len()
        }
    }

    pub fn scan(root: &Path, show_hidden_files: bool) -> ScannedPaths {
        ScannedPaths {
            root: root.to_path_buf(),
            show_hidden_files,
            paths: scan_recursive_paths(root, show_hidden_files, Arc::new(AtomicBool::new(false))),
        }
    }

    pub fn filter(paths: &ScannedPaths, query: &str) -> FilteredPaths {
        FilteredPaths(filter_recursive_paths(
            &paths.paths,
            query,
            &AtomicBool::new(false),
        ))
    }

    pub fn materialize(paths: &ScannedPaths, filtered: &FilteredPaths) -> MaterializedEntries {
        MaterializedEntries(materialize_recursive_entries(
            &paths.paths,
            &filtered.0,
            paths.show_hidden_files,
            &AtomicBool::new(false),
        ))
    }

    pub fn cancelled_materialize(paths: &ScannedPaths, filtered: &FilteredPaths) -> usize {
        materialize_recursive_entries(
            &paths.paths,
            &filtered.0,
            paths.show_hidden_files,
            &AtomicBool::new(true),
        )
        .len()
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
                paths: paths.paths.clone(),
            }),
            Arc::new(AtomicBool::new(false)),
            Arc::new(RecursiveSearchProgress::default()),
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
            Arc::new(RecursiveSearchProgress::default()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;
    use std::fs;

    fn recursive_paths(paths: Vec<PathBuf>) -> Arc<Vec<RecursiveSearchPath>> {
        Arc::new(
            paths
                .into_iter()
                .map(|path| {
                    let file_name = path.file_name().unwrap().to_owned();
                    RecursiveSearchPath {
                        parent_path: Arc::from(path.parent().unwrap_or(Path::new(""))),
                        normalized_name: file_name.to_string_lossy().to_lowercase(),
                        file_name,
                    }
                })
                .collect::<Vec<_>>(),
        )
    }

    fn path_names(paths: &[RecursiveSearchPath]) -> Vec<String> {
        let mut names = paths
            .iter()
            .map(|path| {
                path.path()
                    .file_name()
                    .expect("file name")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn filtered_paths(
        paths: &[RecursiveSearchPath],
        query: &str,
        cancel: &AtomicBool,
    ) -> Vec<PathBuf> {
        filter_recursive_paths(paths, query, cancel)
            .into_iter()
            .map(|index| paths[index].path())
            .collect()
    }

    #[test]
    fn recursive_scan_includes_nested_items() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir(&child).expect("create child");
        fs::write(temp.path().join("root.txt"), b"root").expect("create root file");
        fs::write(child.join("nested.txt"), b"nested").expect("create nested file");

        let cancel = Arc::new(AtomicBool::new(false));
        let paths = scan_recursive_paths(temp.path(), true, cancel);

        assert_eq!(path_names(&paths), vec!["child", "nested.txt", "root.txt"]);
    }

    #[test]
    fn recursive_scan_publishes_discovered_item_count() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir(&child).expect("create child");
        fs::write(temp.path().join("root.txt"), b"root").expect("create root file");
        fs::write(child.join("nested.txt"), b"nested").expect("create nested file");
        let progress = AtomicUsize::new(0);

        let paths = scan_recursive_paths_with_progress(
            temp.path(),
            true,
            Arc::new(AtomicBool::new(false)),
            Some(&progress),
        );

        assert_eq!(progress.load(Ordering::Relaxed), paths.len());
        assert_eq!(progress.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn recursive_search_progress_exposes_phase_and_retains_count() {
        let progress = RecursiveSearchProgress::default();
        assert_eq!(
            progress.snapshot(),
            RecursiveSearchProgressSnapshot::Searching(None)
        );

        progress.scanning.store(true, Ordering::Relaxed);
        assert_eq!(
            progress.snapshot(),
            RecursiveSearchProgressSnapshot::Scanning(0)
        );

        progress.scanned_paths.store(42, Ordering::Relaxed);
        assert_eq!(
            progress.snapshot(),
            RecursiveSearchProgressSnapshot::Scanning(42)
        );

        progress.scanning.store(false, Ordering::Relaxed);
        assert_eq!(
            progress.snapshot(),
            RecursiveSearchProgressSnapshot::Searching(Some(42))
        );
    }

    #[test]
    fn recursive_scan_reconstructs_exact_paths() {
        let temp = TempDir::new();
        let nested = temp.path().join("child").join("Mixed Case.txt");
        fs::create_dir(nested.parent().unwrap()).expect("create child");
        fs::write(&nested, b"nested").expect("create nested file");

        let paths = scan_recursive_paths(temp.path(), true, Arc::new(AtomicBool::new(false)));

        assert!(paths.iter().any(|path| path.path() == nested));
    }

    #[test]
    fn recursive_scan_honors_hidden_and_metadata_filtering() {
        let temp = TempDir::new();
        fs::write(temp.path().join(".hidden"), b"hidden").expect("create hidden");
        fs::write(temp.path().join(".DS_Store"), b"metadata").expect("create metadata");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible");

        let cancel = Arc::new(AtomicBool::new(false));
        let paths = scan_recursive_paths(temp.path(), false, cancel);

        assert_eq!(path_names(&paths), vec!["visible.txt"]);
    }

    #[test]
    fn recursive_scan_does_not_recurse_into_hidden_directories_when_hidden_files_are_off() {
        let temp = TempDir::new();
        let hidden = temp.path().join(".hidden-dir");
        fs::create_dir(&hidden).expect("create hidden directory");
        fs::write(hidden.join("nested.txt"), b"nested").expect("create nested file");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible");

        let cancel = Arc::new(AtomicBool::new(false));
        let paths = scan_recursive_paths(temp.path(), false, cancel);

        assert_eq!(path_names(&paths), vec!["visible.txt"]);
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
        let paths = scan_recursive_paths(temp.path(), true, cancel);

        assert_eq!(
            path_names(&paths),
            vec![".DS_Store", ".hidden-dir", "nested.txt", "visible.txt"]
        );
    }

    #[test]
    fn recursive_scan_honors_existing_cancellation() {
        let temp = TempDir::new();
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible");

        let cancel = Arc::new(AtomicBool::new(true));
        let paths = scan_recursive_paths(temp.path(), true, cancel);

        assert!(paths.is_empty());
    }

    #[test]
    fn filter_matches_basename_only() {
        let paths = recursive_paths(vec![
            PathBuf::from("reports").join("image.png"),
            PathBuf::from("other").join("report.txt"),
        ]);
        let cancel = AtomicBool::new(false);

        assert_eq!(
            filtered_paths(&paths, "report", &cancel),
            vec![PathBuf::from("other").join("report.txt")]
        );
    }

    #[test]
    fn filter_matches_case_insensitively() {
        let paths = recursive_paths(vec![
            PathBuf::from("reports").join("image.png"),
            PathBuf::from("other").join("Annual Report.txt"),
        ]);
        let cancel = AtomicBool::new(false);

        assert_eq!(
            filtered_paths(&paths, "REPORT", &cancel),
            vec![PathBuf::from("other").join("Annual Report.txt")]
        );
    }

    #[test]
    fn filter_treats_regex_characters_as_literal_substring_text() {
        let paths = recursive_paths(vec![
            PathBuf::from("file[1].txt"),
            PathBuf::from("file1.txt"),
            PathBuf::from("notes.txt"),
        ]);
        let cancel = AtomicBool::new(false);

        assert_eq!(
            filtered_paths(&paths, "[1]", &cancel),
            vec![PathBuf::from("file[1].txt")]
        );
    }

    #[test]
    fn filter_preserves_scan_order_for_duplicate_basenames() {
        let expected = vec![
            PathBuf::from("z").join("report.txt"),
            PathBuf::from("a").join("report.txt"),
        ];
        let paths = recursive_paths(vec![
            expected[0].clone(),
            expected[1].clone(),
            PathBuf::from("middle").join("other.txt"),
        ]);
        let cancel = AtomicBool::new(false);

        assert_eq!(filtered_paths(&paths, "report", &cancel), expected);
    }

    #[test]
    fn filter_honors_existing_cancellation() {
        let paths = recursive_paths(vec![PathBuf::from("report.txt")]);
        let cancel = AtomicBool::new(true);

        assert!(filter_recursive_paths(&paths, "report", &cancel).is_empty());
    }

    #[test]
    fn recursive_search_reuses_shared_cached_paths() {
        let temp = TempDir::new();
        let report = temp.path().join("report.txt");
        let notes = temp.path().join("notes.txt");
        fs::write(&report, b"report").expect("create report");
        fs::write(&notes, b"notes").expect("create notes");
        let cancel = Arc::new(AtomicBool::new(false));
        let paths = recursive_paths(vec![report.clone(), notes]);
        let progress = Arc::new(RecursiveSearchProgress::default());
        let cache = RecursiveSearchCache {
            root: temp.path().to_path_buf(),
            show_hidden_files: true,
            paths: paths.clone(),
        };

        let output = recursive_search_entries(
            1,
            temp.path().to_path_buf(),
            "report".to_owned(),
            true,
            Some(cache),
            cancel,
            progress.clone(),
        );

        assert!(Arc::ptr_eq(&output.scanned_paths, &paths));
        assert_eq!(output.entries.len(), 1);
        assert_eq!(output.entries[0].path, report);
        assert_eq!(
            progress.snapshot(),
            RecursiveSearchProgressSnapshot::Searching(Some(2))
        );
    }

    #[test]
    fn recursive_search_skips_missing_files_during_materialization() {
        let temp = TempDir::new();
        let existing = temp.path().join("report.txt");
        let missing = temp.path().join("report-missing.txt");
        fs::write(&existing, b"existing").expect("create existing match");
        let cancel = Arc::new(AtomicBool::new(false));
        let cached_search = RecursiveSearchCache {
            root: temp.path().to_path_buf(),
            show_hidden_files: true,
            paths: recursive_paths(vec![existing.clone(), missing]),
        };

        let output = recursive_search_entries(
            1,
            temp.path().to_path_buf(),
            "report".to_owned(),
            true,
            Some(cached_search),
            cancel,
            Arc::new(RecursiveSearchProgress::default()),
        );

        assert_eq!(output.entries.len(), 1);
        assert_eq!(output.entries[0].path, existing);
    }

    #[test]
    fn materialization_hides_hidden_matches_when_hidden_files_are_off() {
        let temp = TempDir::new();
        let hidden = temp.path().join(".hidden-report.txt");
        let visible = temp.path().join("visible-report.txt");
        fs::write(&hidden, b"hidden").expect("create hidden report");
        fs::write(&visible, b"visible").expect("create visible report");
        let paths = recursive_paths(vec![hidden, visible.clone()]);
        let indices = filter_recursive_paths(&paths, "report", &AtomicBool::new(false));

        let entries =
            materialize_recursive_entries(&paths, &indices, false, &AtomicBool::new(false));

        assert_eq!(
            entries
                .into_iter()
                .map(|entry| entry.path)
                .collect::<Vec<_>>(),
            vec![visible]
        );
    }

    #[test]
    fn parallel_materialization_preserves_match_order() {
        let temp = TempDir::new();
        let expected = (0..PARALLEL_MATERIALIZATION_THRESHOLD + 8)
            .rev()
            .map(|index| temp.path().join(format!("report-{index:03}.txt")))
            .collect::<Vec<_>>();
        for path in &expected {
            fs::write(path, b"report").expect("create report");
        }
        let paths = recursive_paths(expected.clone());
        let indices = filter_recursive_paths(&paths, "report", &AtomicBool::new(false));

        let entries =
            materialize_recursive_entries(&paths, &indices, true, &AtomicBool::new(false));

        assert_eq!(
            entries
                .into_iter()
                .map(|entry| entry.path)
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn materialization_discards_results_after_cancellation() {
        let temp = TempDir::new();
        let report = temp.path().join("report.txt");
        fs::write(&report, b"report").expect("create report");
        let paths = recursive_paths(vec![report]);
        let cancel = AtomicBool::new(true);

        assert!(materialize_recursive_entries(&paths, &[0], true, &cancel).is_empty());
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
        let paths = scan_recursive_paths(temp.path(), true, cancel);

        assert_eq!(path_names(&paths), vec!["link", "nested.txt", "real"]);
        let link_indices = filter_recursive_paths(&paths, "link", &AtomicBool::new(false));
        let entries =
            materialize_recursive_entries(&paths, &link_indices, true, &AtomicBool::new(false));
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_directory_like());
        assert_eq!(entries[0].path, link);
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
        let paths = scan_recursive_paths(temp.path(), true, cancel);

        assert_eq!(path_names(&paths), vec!["link", "nested.txt", "real"]);
        let link_indices = filter_recursive_paths(&paths, "link", &AtomicBool::new(false));
        let entries =
            materialize_recursive_entries(&paths, &link_indices, true, &AtomicBool::new(false));
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_directory_like());
        assert_eq!(entries[0].path, link);
    }
}
