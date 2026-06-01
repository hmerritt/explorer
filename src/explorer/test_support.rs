use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering as AtomicOrdering},
};

use crate::explorer::{entry::FileEntry, view::ExplorerView};

static TEST_DIR_ID: AtomicU64 = AtomicU64::new(0);

pub(super) fn assert_approx_eq(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.000_1,
        "expected {actual} to approximately equal {expected}",
    );
}

pub(super) fn selected_names(view: &ExplorerView) -> Vec<String> {
    view.selected_paths()
        .iter()
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect()
}

pub(super) fn test_view_with_entries(names: &[&str]) -> ExplorerView {
    let mut view = ExplorerView::new(PathBuf::from("selection"));
    view.entries = names
        .iter()
        .map(|name| FileEntry::test(name, false, Some(1), None))
        .collect();
    view.read_error = None;
    view.clear_selection();
    view
}

pub(super) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub(super) fn new() -> Self {
        let id = TEST_DIR_ID.fetch_add(1, AtomicOrdering::Relaxed);
        let path = std::env::temp_dir().join(format!("explorer-test-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp test directory");
        Self { path }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
