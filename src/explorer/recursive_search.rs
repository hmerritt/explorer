use std::ffi::OsStr;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use thousands::Separable;

#[cfg(debug_assertions)]
use std::time::Instant;

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

use jwalk::WalkDir;

use crate::explorer::{entry::FileEntry, filesystem::should_hide_entry};

const CANCELLATION_CHECK_INTERVAL: usize = 5120;

#[derive(Debug, Eq, PartialEq)]
pub(super) struct RecursiveSearchPath {
    path: PathBuf,
    file_name: String,
}

#[derive(Clone)]
pub(super) struct RecursiveSearchCache {
    pub(super) root: PathBuf,
    pub(super) show_hidden_files: bool,
    pub(super) paths: Arc<[RecursiveSearchPath]>,
}

#[derive(Clone)]
pub(super) struct RecursiveSearchOutput {
    pub(super) generation: u64,
    pub(super) root: PathBuf,
    pub(super) query: String,
    pub(super) show_hidden_files: bool,
    pub(super) scanned_paths: Arc<[RecursiveSearchPath]>,
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

    let scanned_paths = match cached_search {
        Some(cache) => cache.paths,
        None => {
            #[cfg(debug_assertions)]
            let scan_started = Instant::now();
            let paths = scan_recursive_paths(&root, show_hidden_files, cancel.clone());
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
        filter_recursive_paths(&scanned_paths, &query, &cancel, show_hidden_files)
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
    let entries = result_paths
        .into_iter()
        .filter_map(FileEntry::from_path)
        .collect::<Vec<_>>();
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

pub(super) fn scan_recursive_paths(
    root: &Path,
    show_hidden_files: bool,
    cancel: Arc<AtomicBool>,
) -> Arc<[RecursiveSearchPath]> {
    if cancel.load(Ordering::Relaxed) {
        return Arc::from([]);
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

    let mut paths = Vec::new();
    for entry_result in walker {
        if let Ok(entry) = entry_result {
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().into_owned();
            paths.push(RecursiveSearchPath { path, file_name });
        }
    }

    if cancel.load(Ordering::Relaxed) {
        return Arc::from([]);
    }

    paths.into()
}

fn filter_recursive_paths(
    paths: &[RecursiveSearchPath],
    query: &str,
    cancel: &AtomicBool,
    show_hidden_files: bool,
) -> Vec<PathBuf> {
    if paths.is_empty() || query.trim().is_empty() {
        return Vec::new();
    }

    let query = query.to_lowercase();
    let mut matches = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        if index % CANCELLATION_CHECK_INTERVAL == 0 && cancel.load(Ordering::Relaxed) {
            return Vec::new();
        }
        let normalized_name = path.file_name.to_lowercase();
        if normalized_name.contains(&query)
            && !should_hide_entry(OsStr::new(&path.file_name), &path.path, show_hidden_files)
        {
            matches.push(path.path.clone());
        }
    }

    if cancel.load(Ordering::Relaxed) {
        Vec::new()
    } else {
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;
    use std::fs;

    fn recursive_paths(paths: Vec<PathBuf>) -> Arc<[RecursiveSearchPath]> {
        paths
            .into_iter()
            .map(|path| RecursiveSearchPath {
                file_name: path.file_name().unwrap().to_string_lossy().into_owned(),
                path,
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn path_names(paths: &[RecursiveSearchPath]) -> Vec<String> {
        let mut names = paths
            .iter()
            .map(|path| {
                path.path
                    .file_name()
                    .expect("file name")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        names.sort();
        names
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
            filter_recursive_paths(&paths, "report", &cancel, true),
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
            filter_recursive_paths(&paths, "REPORT", &cancel, true),
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
            filter_recursive_paths(&paths, "[1]", &cancel, true),
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

        assert_eq!(
            filter_recursive_paths(&paths, "report", &cancel, true),
            expected
        );
    }

    #[test]
    fn filter_honors_existing_cancellation() {
        let paths = recursive_paths(vec![PathBuf::from("report.txt")]);
        let cancel = AtomicBool::new(true);

        assert!(filter_recursive_paths(&paths, "report", &cancel, true).is_empty());
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
        );

        assert!(Arc::ptr_eq(&output.scanned_paths, &paths));
        assert_eq!(output.entries.len(), 1);
        assert_eq!(output.entries[0].path, report);
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
        );

        assert_eq!(output.entries.len(), 1);
        assert_eq!(output.entries[0].path, existing);
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
    }
}
