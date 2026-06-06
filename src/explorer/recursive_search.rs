use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use jwalk::WalkDir;
use tantivy::{
    Index, IndexReader, Order,
    collector::TopDocs,
    doc,
    query::RegexQuery,
    schema::{FAST, Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions},
};

use crate::explorer::{entry::FileEntry, filesystem::should_hide_entry};

const NAME_FIELD: &str = "name";
const PATH_ID_FIELD: &str = "path_id";
const INDEX_WRITER_MEMORY_BUDGET: usize = 15_000_000;

#[derive(Clone)]
pub(super) struct RecursiveSearchIndex {
    reader: IndexReader,
    name_field: Field,
}

#[derive(Clone)]
pub(super) struct RecursiveSearchCache {
    pub(super) root: PathBuf,
    pub(super) show_hidden_files: bool,
    pub(super) paths: Vec<PathBuf>,
    pub(super) index: Option<RecursiveSearchIndex>,
}

#[derive(Clone)]
pub(super) struct RecursiveSearchOutput {
    pub(super) generation: u64,
    pub(super) root: PathBuf,
    pub(super) query: String,
    pub(super) show_hidden_files: bool,
    pub(super) scanned_paths: Vec<PathBuf>,
    pub(super) index: Option<RecursiveSearchIndex>,
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
    let (scanned_paths, index) = match cached_search {
        Some(cache) => (cache.paths, cache.index),
        None => {
            let paths = scan_recursive_paths(&root, show_hidden_files, cancel.clone());
            let index = if cancel.load(Ordering::Relaxed) {
                None
            } else {
                build_search_index(&paths, &cancel)
            };
            (paths, index)
        }
    };

    let result_paths = if cancel.load(Ordering::Relaxed) {
        Vec::new()
    } else {
        index
            .as_ref()
            .and_then(|index| tantivy_ranked_paths(index, &scanned_paths, &query, &cancel))
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
        index,
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
        .skip_hidden(false)
        .follow_links(false)
        .min_depth(1)
        .process_read_dir(move |_, _, _, children| {
            if process_cancel.load(Ordering::Relaxed) {
                children.clear();
                return;
            }

            children.retain(|child| match child {
                Ok(entry) => {
                    !should_hide_entry(entry.file_name(), &entry.path(), show_hidden_files)
                }
                Err(_) => true,
            });
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

fn search_schema() -> (Schema, Field, Field) {
    let mut schema = Schema::builder();
    let name_options = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer("raw")
            .set_index_option(IndexRecordOption::Basic)
            .set_fieldnorms(false),
    );
    let name_field = schema.add_text_field(NAME_FIELD, name_options);
    let path_id_field = schema.add_u64_field(PATH_ID_FIELD, FAST);
    (schema.build(), name_field, path_id_field)
}

fn build_search_index(paths: &[PathBuf], cancel: &AtomicBool) -> Option<RecursiveSearchIndex> {
    if paths.is_empty() || cancel.load(Ordering::Relaxed) {
        return None;
    }

    let (schema, name_field, path_id_field) = search_schema();
    let index = Index::create_in_ram(schema);
    let mut writer = index
        .writer_with_num_threads(1, INDEX_WRITER_MEMORY_BUDGET)
        .ok()?;

    for (path_id, path) in paths.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }

        let name = path.file_name()?.to_string_lossy().to_lowercase();
        writer
            .add_document(doc!(
                name_field => name,
                path_id_field => path_id as u64,
            ))
            .ok()?;
    }

    writer.commit().ok()?;
    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    Some(RecursiveSearchIndex {
        reader: index.reader().ok()?,
        name_field,
    })
}

fn tantivy_ranked_paths(
    index: &RecursiveSearchIndex,
    paths: &[PathBuf],
    query: &str,
    cancel: &AtomicBool,
) -> Option<Vec<PathBuf>> {
    if paths.is_empty() || query.trim().is_empty() {
        return Some(Vec::new());
    }

    let pattern = format!(".*{}.*", regex::escape(&query.to_lowercase()));
    let regex_query = RegexQuery::from_pattern(&pattern, index.name_field).ok()?;
    let searcher = index.reader.searcher();
    let results = searcher
        .search(
            &regex_query,
            &TopDocs::with_limit(paths.len()).order_by_fast_field::<u64>(PATH_ID_FIELD, Order::Asc),
        )
        .ok()?;

    if cancel.load(Ordering::Relaxed) {
        return Some(Vec::new());
    }

    Some(
        results
            .into_iter()
            .filter_map(|(path_id, _)| paths.get(path_id? as usize).cloned())
            .collect(),
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;
    use std::fs;

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
    fn tantivy_schema_keeps_basename_minimal_and_path_id_fast_only() {
        let (schema, name_field, path_id_field) = search_schema();
        let name_entry = schema.get_field_entry(name_field);
        let path_id_entry = schema.get_field_entry(path_id_field);

        assert!(name_entry.is_indexed());
        assert!(!name_entry.is_stored());
        assert!(!name_entry.is_fast());
        assert!(!name_entry.has_fieldnorms());
        assert_eq!(
            name_entry.field_type().index_record_option(),
            Some(IndexRecordOption::Basic)
        );
        let tantivy::schema::FieldType::Str(name_options) = name_entry.field_type() else {
            panic!("name field should be text");
        };
        assert_eq!(
            name_options
                .get_indexing_options()
                .expect("name indexing options")
                .tokenizer(),
            "raw"
        );

        assert!(!path_id_entry.is_indexed());
        assert!(!path_id_entry.is_stored());
        assert!(path_id_entry.is_fast());
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
    fn tantivy_matches_basename_only_case_insensitively() {
        let paths = vec![
            PathBuf::from("reports").join("image.png"),
            PathBuf::from("other").join("Annual Report.txt"),
        ];
        let cancel = AtomicBool::new(false);
        let index = build_search_index(&paths, &cancel).expect("tantivy index");

        let ranked_paths =
            tantivy_ranked_paths(&index, &paths, "REPORT", &cancel).expect("tantivy search");

        assert_eq!(
            ranked_paths,
            vec![PathBuf::from("other").join("Annual Report.txt")]
        );
    }

    #[test]
    fn tantivy_treats_regex_characters_as_literal_substring_text() {
        let paths = vec![
            PathBuf::from("file[1].txt"),
            PathBuf::from("file1.txt"),
            PathBuf::from("notes.txt"),
        ];
        let cancel = AtomicBool::new(false);
        let index = build_search_index(&paths, &cancel).expect("tantivy index");

        assert_eq!(
            tantivy_ranked_paths(&index, &paths, "[1]", &cancel).expect("tantivy search"),
            vec![PathBuf::from("file[1].txt")]
        );
    }

    #[test]
    fn tantivy_preserves_scan_order_for_duplicate_basenames() {
        let paths = vec![
            PathBuf::from("z").join("report.txt"),
            PathBuf::from("a").join("report.txt"),
            PathBuf::from("middle").join("other.txt"),
        ];
        let cancel = AtomicBool::new(false);
        let index = build_search_index(&paths, &cancel).expect("tantivy index");

        assert_eq!(
            tantivy_ranked_paths(&index, &paths, "report", &cancel).expect("tantivy search"),
            paths[..2]
        );
    }

    #[test]
    fn recursive_search_reuses_cached_tantivy_index() {
        let temp = TempDir::new();
        let report = temp.path().join("report.txt");
        let notes = temp.path().join("notes.txt");
        fs::write(&report, b"report").expect("create report");
        fs::write(&notes, b"notes").expect("create notes");
        let cancel = Arc::new(AtomicBool::new(false));
        let index =
            build_search_index(std::slice::from_ref(&report), &cancel).expect("tantivy index");
        let cache = RecursiveSearchCache {
            root: temp.path().to_path_buf(),
            show_hidden_files: true,
            // Deliberately differ from the indexed path to prove the cached index is queried.
            paths: vec![notes.clone()],
            index: Some(index),
        };

        let output = recursive_search_entries(
            1,
            temp.path().to_path_buf(),
            "report".to_owned(),
            true,
            Some(cache),
            cancel,
        );

        assert!(output.index.is_some());
        assert_eq!(output.entries.len(), 1);
        assert_eq!(output.entries[0].path, notes);
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
            paths: vec![existing.clone(), missing],
            index: None,
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
