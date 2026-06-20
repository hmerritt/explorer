use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering as AtomicOrdering},
};

use crate::{
    explorer::{entry::FileEntry, view::ExplorerView},
    settings::{ExplorerSettings, SettingsState},
};

use gpui::{Entity, TestAppContext, VisualTestContext};

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
    view.all_entries = view.entries.clone();
    view.read_error = None;
    view.clear_selection();
    view
}

pub(super) fn test_view_entity<'a>(
    cx: &'a mut TestAppContext,
    file_names: &[&str],
) -> (TempDir, Entity<ExplorerView>, &'a mut VisualTestContext) {
    let temp = TempDir::new();
    for file_name in file_names {
        fs::write(temp.path().join(file_name), b"file").expect("create test file");
    }
    let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());
    (temp, view, cx)
}

pub(super) fn test_view_entity_at_path<'a>(
    cx: &'a mut TestAppContext,
    path: PathBuf,
) -> (Entity<ExplorerView>, &'a mut VisualTestContext) {
    cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
    cx.add_window_view(move |window, cx| {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        ExplorerView::new_with_focus_handle_for_test(path, focus_handle)
    })
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
