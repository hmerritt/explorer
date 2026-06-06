use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use jwalk::WalkDir;
use seekstorm::{
    INDEX_RUNTIME,
    commit::Commit,
    index::{
        AccessType, Clustering, Document, DocumentCompression, FieldType, FrequentwordType,
        IndexDocuments, IndexMetaObject, LexicalSimilarity, NgramSet, SchemaField, StemmerType,
        StopwordType, TokenizerType, create_index,
    },
    search::{QueryRewriting, QueryType, ResultType, Search, SearchMode},
};
use serde_json::Value;

use crate::explorer::entry::FileEntry;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RecursiveSearchCache {
    pub(super) root: PathBuf,
    pub(super) show_hidden_files: bool,
    pub(super) paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RecursiveSearchOutput {
    pub(super) generation: u64,
    pub(super) root: PathBuf,
    pub(super) query: String,
    pub(super) show_hidden_files: bool,
    pub(super) scanned_paths: Vec<PathBuf>,
    pub(super) entries: Vec<FileEntry>,
}

pub(super) fn recursive_search_entries(
    generation: u64,
    root: PathBuf,
    query: String,
    show_hidden_files: bool,
    cached_paths: Option<Vec<PathBuf>>,
    cancel: Arc<AtomicBool>,
) -> RecursiveSearchOutput {
    let scanned_paths = match cached_paths {
        Some(paths) => paths,
        None => scan_recursive_paths(&root, show_hidden_files, cancel.clone()),
    };

    let result_paths = if cancel.load(Ordering::Relaxed) {
        Vec::new()
    } else {
        seekstorm_ranked_paths(&scanned_paths, &query, &cancel)
            .unwrap_or_else(|| fallback_ranked_paths(&scanned_paths, &query))
    };

    let entries = result_paths
        .into_iter()
        .filter_map(FileEntry::from_path)
        .collect();

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
) -> Vec<PathBuf> {
    if cancel.load(Ordering::Relaxed) {
        return Vec::new();
    }

    let process_cancel = cancel.clone();
    let walker = WalkDir::new(root)
        .sort(false)
        .skip_hidden(!show_hidden_files)
        .follow_links(false)
        .min_depth(1)
        .process_read_dir(move |_, _, _, children| {
            if process_cancel.load(Ordering::Relaxed) {
                children.clear();
                return;
            }
        });

    let mut paths = Vec::new();
    for entry_result in walker {
        if let Ok(entry) = entry_result {
            paths.push(entry.path());
        }
    }

    if cancel.load(Ordering::Relaxed) {
        return Vec::new();
    }

    paths
}

#[cfg(target_os = "windows")]
fn is_filesystem_directory_link(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0)
}

#[cfg(not(target_os = "windows"))]
fn is_filesystem_directory_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

fn seekstorm_ranked_paths(
    paths: &[PathBuf],
    query: &str,
    cancel: &AtomicBool,
) -> Option<Vec<PathBuf>> {
    if paths.is_empty() || query.trim().is_empty() {
        return Some(Vec::new());
    }

    let index_path = temp_index_path();
    let schema = vec![SchemaField::new(
        "name".to_owned(),
        false,
        true,
        false,
        FieldType::Text,
        false,
        true,
        1.0,
        false,
        false,
    )];
    let meta = IndexMetaObject {
        id: 0,
        name: "explorer-recursive-search".to_owned(),
        lexical_similarity: LexicalSimilarity::Bm25fProximity,
        tokenizer: TokenizerType::UnicodeAlphanumericFolded,
        stemmer: StemmerType::None,
        stop_words: StopwordType::None,
        frequent_words: FrequentwordType::None,
        ngram_indexing: NgramSet::SingleTerm as u8,
        document_compression: DocumentCompression::None,
        access_type: AccessType::Ram,
        spelling_correction: None,
        query_completion: None,
        clustering: Clustering::None,
        inference: Default::default(),
    };

    let ranked_paths = INDEX_RUNTIME.block_on(async {
        let index = create_index(&index_path, meta, &schema, &Vec::new(), 11, true, Some(1))
            .await
            .ok()?;

        let documents = paths
            .iter()
            .filter_map(|path| {
                let name = path.file_name()?.to_string_lossy().into_owned();
                let mut document = Document::new();
                document.insert("name".to_owned(), Value::String(name));
                Some(document)
            })
            .collect::<Vec<_>>();

        if cancel.load(Ordering::Relaxed) {
            return Some(Vec::new());
        }

        index.index_documents(documents).await;
        index.commit().await;

        if cancel.load(Ordering::Relaxed) {
            return Some(Vec::new());
        }

        let result = index
            .search(
                query.to_owned(),
                None,
                QueryType::Union,
                SearchMode::Lexical,
                false,
                0,
                paths.len(),
                ResultType::Topk,
                true,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                QueryRewriting::SearchOnly,
            )
            .await;

        Some(
            result
                .results
                .into_iter()
                .filter_map(|result| paths.get(result.doc_id).cloned())
                .collect(),
        )
    });

    let _ = fs::remove_dir_all(index_path);
    ranked_paths
}

fn fallback_ranked_paths(paths: &[PathBuf], query: &str) -> Vec<PathBuf> {
    let query = query.to_lowercase();
    paths
        .iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.to_lowercase().contains(&query))
        })
        .cloned()
        .collect()
}

fn temp_index_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!("explorer-seekstorm-{}-{nanos}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;

    fn path_names(paths: &[PathBuf]) -> Vec<String> {
        let mut names = paths
            .iter()
            .map(|path| {
                path.file_name()
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
            vec![".hidden-dir", "nested.txt", "visible.txt"]
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
    fn fallback_matches_basename_only() {
        let paths = vec![
            PathBuf::from("reports").join("image.png"),
            PathBuf::from("other").join("report.txt"),
        ];

        assert_eq!(
            fallback_ranked_paths(&paths, "report"),
            vec![PathBuf::from("other").join("report.txt")]
        );
    }

    #[test]
    fn seekstorm_matches_basename_only() {
        let paths = vec![
            PathBuf::from("reports").join("image.png"),
            PathBuf::from("other").join("report.txt"),
        ];
        let cancel = AtomicBool::new(false);

        let ranked_paths =
            seekstorm_ranked_paths(&paths, "report", &cancel).expect("seekstorm search");

        assert_eq!(
            ranked_paths,
            vec![PathBuf::from("other").join("report.txt")]
        );
    }

    #[test]
    fn recursive_search_skips_missing_files_during_materialization() {
        let temp = TempDir::new();
        let existing = temp.path().join("report.txt");
        let missing = temp.path().join("report-missing.txt");
        fs::write(&existing, b"existing").expect("create existing match");
        let cancel = Arc::new(AtomicBool::new(false));

        let output = recursive_search_entries(
            1,
            temp.path().to_path_buf(),
            "report".to_owned(),
            true,
            Some(vec![existing.clone(), missing]),
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
