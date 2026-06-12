use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use gpui::Context;

use crate::explorer::{
    entry::FileEntry,
    filesystem::{format_open_error, open_path_with_default_app},
    selection::SelectionModifiers,
    view::ExplorerView,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HistoryMode {
    Record,
    Preserve,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum EntryAction {
    OpenFile(PathBuf),
}

impl ExplorerView {
    #[cfg(test)]
    pub(super) fn navigate_to_directory(&mut self, path: PathBuf, history_mode: HistoryMode) {
        self.navigate_to_directory_inner(path, history_mode, None);
    }

    pub(super) fn navigate_to_directory_with_watcher(
        &mut self,
        path: PathBuf,
        history_mode: HistoryMode,
        cx: &mut Context<Self>,
    ) {
        self.navigate_to_directory_inner(path, history_mode, Some(cx));
    }

    fn navigate_to_directory_inner(
        &mut self,
        path: PathBuf,
        history_mode: HistoryMode,
        mut cx: Option<&mut Context<Self>>,
    ) {
        let total_started = Instant::now();
        let original_path = self.path.clone();
        crate::debug_options::log_nav_timing(
            total_started.elapsed(),
            format_args!(
                "navigate.start from={original_path:?} to={path:?} history={history_mode:?} same_path={}",
                path == original_path
            ),
        );

        if path == self.path {
            let reload_started = Instant::now();
            self.reload();
            if let Some(cx) = cx.as_deref_mut() {
                self.schedule_pending_shell_shortcut_resolution(cx);
            }
            crate::debug_options::log_nav_timing(
                reload_started.elapsed(),
                format_args!("navigate.reload same_path=true path={:?}", self.path),
            );
            if let Some(cx) = cx.as_deref_mut() {
                let refresh_started = Instant::now();
                self.refresh_search_after_external_change(cx);
                crate::debug_options::log_nav_timing(
                    refresh_started.elapsed(),
                    format_args!(
                        "navigate.refresh_search same_path=true path={:?}",
                        self.path
                    ),
                );
                self.restart_directory_watcher(cx);
            }
            crate::debug_options::log_nav_timing(
                total_started.elapsed(),
                format_args!(
                    "navigate.total from={original_path:?} to={:?} same_path=true",
                    self.path
                ),
            );
            return;
        }

        let select_entry_after_reload =
            (self.path.parent() == Some(path.as_path())).then(|| self.path.clone());

        let pre_reload_started = Instant::now();
        if matches!(history_mode, HistoryMode::Record) {
            self.back_stack.push(self.path.clone());
            self.forward_stack.clear();
        }

        self.path = path;
        self.clear_search();
        self.clear_selection();
        self.read_error = None;
        self.open_error = None;
        self.scroll_to_top();
        crate::debug_options::log_nav_timing(
            pre_reload_started.elapsed(),
            format_args!(
                "navigate.pre_reload from={original_path:?} to={:?} history={history_mode:?}",
                self.path
            ),
        );

        let reload_started = Instant::now();
        self.reload();
        if let Some(cx) = cx.as_deref_mut() {
            self.schedule_pending_shell_shortcut_resolution(cx);
        }
        crate::debug_options::log_nav_timing(
            reload_started.elapsed(),
            format_args!("navigate.reload same_path=false path={:?}", self.path),
        );
        if let Some(cx) = cx.as_deref_mut() {
            self.restart_directory_watcher(cx);
        }

        if let Some(path) = select_entry_after_reload {
            let selection_started = Instant::now();
            self.select_single_path(&path);
            crate::debug_options::log_nav_timing(
                selection_started.elapsed(),
                format_args!(
                    "navigate.select_origin path={:?} selected={}",
                    path,
                    self.selected_paths().len()
                ),
            );
        }
        crate::debug_options::log_nav_timing(
            total_started.elapsed(),
            format_args!(
                "navigate.total from={original_path:?} to={:?} same_path=false entries={} read_error={}",
                self.path,
                self.entries.len(),
                self.read_error.is_some()
            ),
        );
    }

    #[cfg(test)]
    pub(super) fn navigate_to_sidebar_path(&mut self, path: PathBuf) {
        self.navigate_to_directory(path, HistoryMode::Record);
    }

    pub(super) fn navigate_to_sidebar_path_with_watcher(
        &mut self,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.navigate_to_directory_with_watcher(path, HistoryMode::Record, cx);
    }

    #[cfg(test)]
    pub(super) fn navigate_back(&mut self) {
        if let Some(path) = self.back_stack.pop() {
            self.forward_stack.push(self.path.clone());
            self.navigate_to_directory(path, HistoryMode::Preserve);
        }
    }

    pub(super) fn navigate_back_with_watcher(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.back_stack.pop() {
            self.forward_stack.push(self.path.clone());
            self.navigate_to_directory_with_watcher(path, HistoryMode::Preserve, cx);
        }
    }

    #[cfg(test)]
    pub(super) fn navigate_forward(&mut self) {
        if let Some(path) = self.forward_stack.pop() {
            self.back_stack.push(self.path.clone());
            self.navigate_to_directory(path, HistoryMode::Preserve);
        }
    }

    pub(super) fn navigate_forward_with_watcher(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.forward_stack.pop() {
            self.back_stack.push(self.path.clone());
            self.navigate_to_directory_with_watcher(path, HistoryMode::Preserve, cx);
        }
    }

    #[cfg(test)]
    pub(super) fn navigate_up(&mut self) {
        if let Some(parent) = self.path.parent().map(Path::to_path_buf) {
            self.navigate_to_directory(parent, HistoryMode::Record);
        }
    }

    pub(super) fn navigate_up_with_watcher(&mut self, cx: &mut Context<Self>) {
        if let Some(parent) = self.path.parent().map(Path::to_path_buf) {
            self.navigate_to_directory_with_watcher(parent, HistoryMode::Record, cx);
        }
    }

    pub(super) fn can_go_back(&self) -> bool {
        !self.back_stack.is_empty()
    }

    pub(super) fn can_go_forward(&self) -> bool {
        !self.forward_stack.is_empty()
    }

    pub(super) fn can_go_up(&self) -> bool {
        self.path.parent().is_some()
    }

    #[cfg(test)]
    pub(super) fn handle_entry_click(
        &mut self,
        entry: &FileEntry,
        click_count: usize,
        modifiers: SelectionModifiers,
    ) -> Option<EntryAction> {
        self.handle_entry_click_inner(entry, click_count, modifiers, None)
    }

    pub(super) fn handle_entry_click_with_watcher(
        &mut self,
        entry: &FileEntry,
        click_count: usize,
        modifiers: SelectionModifiers,
        cx: &mut Context<Self>,
    ) -> Option<EntryAction> {
        self.handle_entry_click_inner(entry, click_count, modifiers, Some(cx))
    }

    fn handle_entry_click_inner(
        &mut self,
        entry: &FileEntry,
        click_count: usize,
        modifiers: SelectionModifiers,
        cx: Option<&mut Context<Self>>,
    ) -> Option<EntryAction> {
        self.cancel_pending_click_rename();

        if let Some(ix) = self.entry_index_by_path(&entry.path) {
            self.apply_click_selection(ix, modifiers);
        } else {
            self.clear_selection();
        }
        self.open_error = None;

        if click_count < 2 {
            return None;
        }

        if entry.is_app_bundle() {
            Some(EntryAction::OpenFile(entry.path.clone()))
        } else if entry.is_directory_like() {
            self.navigate_to_directory_inner(
                entry.navigation_path().to_path_buf(),
                HistoryMode::Record,
                cx,
            );
            None
        } else {
            Some(EntryAction::OpenFile(entry.path.clone()))
        }
    }

    pub(super) fn handle_entry_middle_click(
        &mut self,
        entry: &FileEntry,
        modifiers: SelectionModifiers,
    ) -> Option<PathBuf> {
        self.cancel_pending_click_rename();

        let target = directory_new_tab_target(entry)?;

        if let Some(ix) = self.entry_index_by_path(&entry.path) {
            self.apply_click_selection(ix, modifiers);
        } else {
            self.clear_selection();
        }
        self.open_error = None;

        Some(target)
    }

    #[cfg(test)]
    pub(super) fn activate_focused_entry(&mut self, open_files: bool) -> Option<EntryAction> {
        self.activate_focused_entry_inner(open_files, None)
    }

    pub(super) fn activate_focused_entry_with_watcher(
        &mut self,
        open_files: bool,
        cx: &mut Context<Self>,
    ) -> Option<EntryAction> {
        self.activate_focused_entry_inner(open_files, Some(cx))
    }

    fn activate_focused_entry_inner(
        &mut self,
        open_files: bool,
        cx: Option<&mut Context<Self>>,
    ) -> Option<EntryAction> {
        let entry = self.focused_entry()?.clone();
        self.open_error = None;

        if entry.is_app_bundle() {
            if open_files {
                Some(EntryAction::OpenFile(entry.path))
            } else {
                None
            }
        } else if entry.is_directory_like() {
            self.navigate_to_directory_inner(
                entry.navigation_path().to_path_buf(),
                HistoryMode::Record,
                cx,
            );
            None
        } else if open_files {
            Some(EntryAction::OpenFile(entry.path))
        } else {
            None
        }
    }

    pub(super) fn open_file_with_default_app(&mut self, path: &Path) {
        let result = open_path_with_default_app(path);
        self.handle_open_file_result(path, result);
    }

    pub(super) fn open_settings_file(&mut self, path: Option<&Path>) {
        self.open_settings_file_with(path, open_path_with_default_app);
    }

    fn open_settings_file_with(
        &mut self,
        path: Option<&Path>,
        open: impl FnOnce(&Path) -> std::io::Result<()>,
    ) {
        let Some(path) = path else {
            self.open_error =
                Some("Could not open settings.json: settings file path is unavailable".to_owned());
            return;
        };

        self.handle_open_file_result(path, open(path));
    }

    pub(super) fn handle_open_file_result(&mut self, path: &Path, result: std::io::Result<()>) {
        match result {
            Ok(()) => self.open_error = None,
            Err(error) => {
                self.open_error = Some(format_open_error(path, &error));
            }
        }
    }
}

pub(super) fn directory_new_tab_target(entry: &FileEntry) -> Option<PathBuf> {
    (entry.is_directory_like() && !entry.is_app_bundle())
        .then(|| entry.navigation_path().to_path_buf())
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::explorer::{
        entry::{DirectoryLinkKind, EntryKind, FileEntry, ShellShortcutTargetKind},
        test_support::TempDir,
        view::ExplorerView,
    };
    use std::{fs, path::PathBuf};

    #[test]
    fn navigating_to_valid_directory_updates_path_and_clears_selection() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir_all(&child).expect("create child directory");
        fs::write(child.join("inside.txt"), b"data").expect("create child file");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&child);
        view.open_error = Some("stale error".to_owned());

        view.navigate_to_directory(child.clone(), HistoryMode::Record);

        assert_eq!(view.path, child);
        assert!(view.selected_paths().is_empty());
        assert_eq!(view.read_error, None);
        assert_eq!(view.open_error, None);
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
        assert!(view.forward_stack.is_empty());
        assert_eq!(view.entries.len(), 1);
        assert_eq!(view.entries[0].name, "inside.txt");
    }

    #[test]
    fn navigating_to_missing_directory_sets_read_error_and_empty_entries() {
        let temp = TempDir::new();
        let missing = temp.path().join("missing");
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_index(0);

        view.navigate_to_directory(missing.clone(), HistoryMode::Record);

        assert_eq!(view.path, missing);
        assert!(view.selected_paths().is_empty());
        assert!(view.read_error.is_some());
        assert!(view.entries.is_empty());
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn single_click_selects_without_navigating() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir_all(&child).expect("create child directory");
        let entry = FileEntry {
            path: child.clone(),
            name: "child".to_owned(),
            kind: EntryKind::Directory,
            modified: None,
            size: None,
        };
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.open_error = Some("stale error".to_owned());

        let action = view.handle_entry_click(&entry, 1, SelectionModifiers::default());

        assert_eq!(action, None);
        assert_eq!(view.path, temp.path());
        assert_eq!(view.selected_paths(), vec![child]);
        assert_eq!(view.open_error, None);
        assert!(view.back_stack.is_empty());
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn double_click_opens_files_and_navigates_directories() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        let file = temp.path().join("file.txt");
        fs::create_dir_all(&child).expect("create child directory");
        fs::write(&file, b"data").expect("create file");

        let file_entry = FileEntry {
            path: file.clone(),
            name: "file.txt".to_owned(),
            kind: EntryKind::File,
            modified: None,
            size: Some(4),
        };
        let dir_entry = FileEntry {
            path: child.clone(),
            name: "child".to_owned(),
            kind: EntryKind::Directory,
            modified: None,
            size: None,
        };
        let mut view = ExplorerView::new(temp.path().to_path_buf());

        let action = view.handle_entry_click(&file_entry, 2, SelectionModifiers::default());
        assert_eq!(action, Some(EntryAction::OpenFile(file.clone())));
        assert_eq!(view.path, temp.path());
        assert_eq!(view.selected_paths(), vec![file.clone()]);
        assert!(view.back_stack.is_empty());
        assert!(view.forward_stack.is_empty());

        let action = view.handle_entry_click(&dir_entry, 2, SelectionModifiers::default());
        assert_eq!(action, None);
        assert_eq!(view.path, child);
        assert!(view.selected_paths().is_empty());
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn double_click_opens_app_bundles_instead_of_navigating_into_them() {
        let temp = TempDir::new();
        let app = temp.path().join("Preview.app");
        fs::create_dir_all(&app).expect("create app bundle");
        let entry = FileEntry {
            path: app.clone(),
            name: "Preview.app".to_owned(),
            kind: EntryKind::Directory,
            modified: None,
            size: None,
        };
        let mut view = ExplorerView::new(temp.path().to_path_buf());

        let action = view.handle_entry_click(&entry, 2, SelectionModifiers::default());

        assert_eq!(action, Some(EntryAction::OpenFile(app.clone())));
        assert_eq!(view.path, temp.path());
        assert_eq!(view.selected_paths(), vec![app]);
        assert!(view.back_stack.is_empty());
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn middle_click_target_uses_real_directory_path() {
        let entry = FileEntry::test("folder", true, None, None);

        assert_eq!(
            directory_new_tab_target(&entry),
            Some(PathBuf::from("folder"))
        );
    }

    #[test]
    fn middle_click_target_uses_filesystem_directory_link_path() {
        let entry = FileEntry::test_directory_link("linked", DirectoryLinkKind::FilesystemLink);

        assert_eq!(
            directory_new_tab_target(&entry),
            Some(PathBuf::from("linked"))
        );
    }

    #[test]
    fn middle_click_target_uses_shell_directory_shortcut_target() {
        let entry = FileEntry::test_directory_link(
            "shortcut.lnk",
            DirectoryLinkKind::ShellShortcut {
                target: PathBuf::from("target"),
                target_kind: ShellShortcutTargetKind::Directory,
            },
        );

        assert_eq!(
            directory_new_tab_target(&entry),
            Some(PathBuf::from("target"))
        );
    }

    #[test]
    fn middle_click_target_ignores_unresolved_shell_shortcuts() {
        let pending = FileEntry::test_directory_link(
            "pending.lnk",
            DirectoryLinkKind::ShellShortcut {
                target: PathBuf::from("target"),
                target_kind: ShellShortcutTargetKind::Pending,
            },
        );
        let non_directory = FileEntry::test_directory_link(
            "file.lnk",
            DirectoryLinkKind::ShellShortcut {
                target: PathBuf::from("target.txt"),
                target_kind: ShellShortcutTargetKind::NonDirectory,
            },
        );

        assert_eq!(directory_new_tab_target(&pending), None);
        assert_eq!(directory_new_tab_target(&non_directory), None);
    }

    #[test]
    fn middle_click_target_ignores_files_and_app_bundles() {
        let file = FileEntry::test("file.txt", false, Some(4), None);
        assert_eq!(directory_new_tab_target(&file), None);

        #[cfg(target_os = "macos")]
        {
            let app = FileEntry::test("Preview.app", true, None, None);
            assert_eq!(directory_new_tab_target(&app), None);
        }
    }

    #[test]
    fn middle_click_selects_directory_and_returns_background_tab_target() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.entries = vec![
            FileEntry::test("first", true, None, None),
            FileEntry::test("second", true, None, None),
        ];
        view.select_single_index(0);

        let entry = view.entries[1].clone();
        let target = view.handle_entry_middle_click(&entry, SelectionModifiers::default());

        assert_eq!(target, Some(PathBuf::from("second")));
        assert_eq!(view.path, PathBuf::from("root"));
        assert_eq!(view.selected_paths(), vec![PathBuf::from("second")]);
        assert!(view.back_stack.is_empty());
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn middle_click_ignores_file_selection() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.entries = vec![
            FileEntry::test("folder", true, None, None),
            FileEntry::test("file.txt", false, Some(4), None),
        ];
        view.select_single_index(0);

        let entry = view.entries[1].clone();
        let target = view.handle_entry_middle_click(&entry, SelectionModifiers::default());

        assert_eq!(target, None);
        assert_eq!(view.selected_paths(), vec![PathBuf::from("folder")]);
    }

    #[test]
    fn open_file_result_sets_and_clears_open_error() {
        let temp = TempDir::new();
        let file = temp.path().join("file.txt");
        let mut view = ExplorerView::new(temp.path().to_path_buf());

        view.handle_open_file_result(
            &file,
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing")),
        );

        assert_eq!(
            view.open_error,
            Some("Could not open file.txt: missing".to_owned())
        );

        view.handle_open_file_result(&file, Ok(()));

        assert_eq!(view.open_error, None);
    }

    #[test]
    fn settings_file_open_result_sets_and_clears_open_error() {
        let temp = TempDir::new();
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        let settings = temp.path().join("settings.json");

        view.open_settings_file_with(Some(&settings), |_| {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"))
        });
        assert_eq!(
            view.open_error.as_deref(),
            Some("Could not open settings.json: missing")
        );

        view.open_settings_file_with(Some(&settings), |_| Ok(()));
        assert_eq!(view.open_error, None);
    }

    #[test]
    fn unavailable_settings_path_sets_open_error_without_opening() {
        let temp = TempDir::new();
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        let mut opened = false;

        view.open_settings_file_with(None, |_| {
            opened = true;
            Ok(())
        });

        assert!(!opened);
        assert_eq!(
            view.open_error.as_deref(),
            Some("Could not open settings.json: settings file path is unavailable")
        );
    }

    #[test]
    fn refresh_clears_open_error() {
        let temp = TempDir::new();
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.open_error = Some("stale error".to_owned());

        view.reload();

        assert_eq!(view.open_error, None);
    }

    #[test]
    fn focused_activation_enters_directories_and_opens_files_on_enter() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.entries = vec![
            FileEntry::test("folder", true, None, None),
            FileEntry::test("file.txt", false, Some(4), None),
        ];

        view.select_single_index(0);
        assert_eq!(view.activate_focused_entry(true), None);
        assert_eq!(view.path, PathBuf::from("folder"));

        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.entries = vec![FileEntry::test("file.txt", false, Some(4), None)];
        view.select_single_index(0);

        assert_eq!(
            view.activate_focused_entry(true),
            Some(EntryAction::OpenFile(PathBuf::from("file.txt")))
        );
        assert_eq!(view.path, PathBuf::from("root"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn focused_activation_opens_app_bundles_on_enter() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.entries = vec![FileEntry::test("Preview.app", true, None, None)];
        view.select_single_index(0);

        assert_eq!(
            view.activate_focused_entry(true),
            Some(EntryAction::OpenFile(PathBuf::from("Preview.app")))
        );
        assert_eq!(view.path, PathBuf::from("root"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn right_arrow_activation_ignores_app_bundles() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.entries = vec![FileEntry::test("Preview.app", true, None, None)];
        view.select_single_index(0);

        assert_eq!(view.activate_focused_entry(false), None);
        assert_eq!(view.path, PathBuf::from("root"));
    }

    #[test]
    fn directory_shortcut_activation_navigates_to_target() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.entries = vec![FileEntry::test_directory_link(
            "shortcut.lnk",
            DirectoryLinkKind::ShellShortcut {
                target: PathBuf::from("target"),
                target_kind: ShellShortcutTargetKind::Directory,
            },
        )];

        view.select_single_index(0);

        assert_eq!(view.activate_focused_entry(true), None);
        assert_eq!(view.path, PathBuf::from("target"));
    }

    #[test]
    fn pending_shell_shortcut_activation_opens_shortcut_file() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.entries = vec![FileEntry::test_directory_link(
            "shortcut.lnk",
            DirectoryLinkKind::ShellShortcut {
                target: PathBuf::from("target"),
                target_kind: ShellShortcutTargetKind::Pending,
            },
        )];

        view.select_single_index(0);

        assert_eq!(
            view.activate_focused_entry(true),
            Some(EntryAction::OpenFile(PathBuf::from("shortcut.lnk")))
        );
        assert_eq!(view.path, PathBuf::from("root"));
    }

    #[test]
    fn non_directory_shell_shortcut_activation_opens_shortcut_file() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.entries = vec![FileEntry::test_directory_link(
            "shortcut.lnk",
            DirectoryLinkKind::ShellShortcut {
                target: PathBuf::from("target.txt"),
                target_kind: ShellShortcutTargetKind::NonDirectory,
            },
        )];

        view.select_single_index(0);

        assert_eq!(
            view.activate_focused_entry(true),
            Some(EntryAction::OpenFile(PathBuf::from("shortcut.lnk")))
        );
        assert_eq!(view.path, PathBuf::from("root"));
    }

    #[test]
    fn right_arrow_activation_ignores_files() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.entries = vec![FileEntry::test("file.txt", false, Some(4), None)];
        view.select_single_index(0);

        assert_eq!(view.activate_focused_entry(false), None);
        assert_eq!(view.path, PathBuf::from("root"));
    }

    #[test]
    fn folder_navigation_records_back_and_clears_forward() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir_all(&child).expect("create child directory");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.forward_stack.push(temp.path().join("forward"));

        view.navigate_to_directory(child.clone(), HistoryMode::Record);

        assert_eq!(view.path, child);
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn sidebar_navigation_records_history_and_clears_selection() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir_all(&child).expect("create child directory");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.entries = vec![FileEntry::test("selected.txt", false, Some(1), None)];
        view.select_single_index(0);

        view.navigate_to_sidebar_path(child.clone());

        assert_eq!(view.path, child);
        assert!(view.selected_paths().is_empty());
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn back_and_forward_move_between_paths() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir_all(&child).expect("create child directory");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.navigate_to_directory(child.clone(), HistoryMode::Record);

        view.navigate_back();
        assert_eq!(view.path, temp.path());
        assert_eq!(view.selected_paths(), vec![child.clone()]);
        assert!(view.back_stack.is_empty());
        assert_eq!(view.forward_stack, vec![child.clone()]);

        view.navigate_forward();
        assert_eq!(view.path, child);
        assert!(view.selected_paths().is_empty());
        assert_eq!(view.back_stack, vec![temp.path().to_path_buf()]);
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn up_navigates_to_parent_selects_origin_and_records_history() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        let grandchild = child.join("grandchild");
        fs::create_dir_all(&grandchild).expect("create nested directories");

        let mut view = ExplorerView::new(grandchild.clone());

        view.navigate_up();

        assert_eq!(view.path, child);
        assert_eq!(view.selected_paths(), vec![grandchild.clone()]);
        assert_eq!(view.back_stack, vec![grandchild]);
        assert!(view.forward_stack.is_empty());
    }

    #[test]
    fn refresh_preserves_path_and_history() {
        let temp = TempDir::new();
        let child = temp.path().join("child");
        fs::create_dir_all(&child).expect("create child directory");
        let back = temp.path().join("back");
        let forward = temp.path().join("forward");

        let mut view = ExplorerView::new(child.clone());
        view.back_stack.push(back.clone());
        view.forward_stack.push(forward.clone());

        view.reload();

        assert_eq!(view.path, child);
        assert_eq!(view.back_stack, vec![back]);
        assert_eq!(view.forward_stack, vec![forward]);
    }
}
