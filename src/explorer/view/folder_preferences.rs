use std::{path::Path, time::Instant};

use gpui::{App, Context};

use crate::{
    explorer::folder_views::{self, FolderView, FolderViews, Ready},
    settings::{self, FileColumnKind, FileColumnSettings, FileSortSettings, FileViewMode},
};

use super::{ExplorerView, FileColumnResizeResult, FileColumnResizeTarget, ViewModeSelection};

impl ExplorerView {
    fn folder_view_path(&self) -> Option<&Path> {
        (!self.is_sidebar_group_view() && folder_views::eligible(&self.path))
            .then_some(self.path.as_path())
    }

    pub(in crate::explorer) fn folder_view_enabled(&self, cx: &App) -> bool {
        cx.has_global::<FolderViews>() && self.folder_view_path().is_some()
    }

    pub(in crate::explorer) fn folder_view_is_loading(&self, cx: &App) -> bool {
        self.folder_view_path().is_some()
            && cx
                .try_global::<FolderViews>()
                .is_some_and(FolderViews::is_loading)
    }

    pub(super) fn folder_views_pending(&self, cx: &App) -> Option<Ready> {
        self.folder_view_path()?;
        cx.try_global::<FolderViews>()?.pending()
    }

    fn folder_view<'a>(&self, cx: &'a App) -> Option<(u64, &'a FolderView)> {
        if self.is_sidebar_group_view() {
            return None;
        }
        cx.try_global::<FolderViews>()?.get(&self.path)
    }

    pub(super) fn resolved_folder_view(
        &self,
        cx: &App,
    ) -> (Option<FileViewMode>, FileSortSettings, FileColumnSettings) {
        let view = self.folder_view(cx).map(|(_, view)| view);
        self.resolve_preferences(view)
    }

    fn resolve_preferences(
        &self,
        view: Option<&FolderView>,
    ) -> (Option<FileViewMode>, FileSortSettings, FileColumnSettings) {
        view.unwrap_or(&FolderView::default())
            .resolve(self.default_file_sort, &self.default_file_columns)
    }

    pub(in crate::explorer) fn reset_folder_view_for_navigation(&mut self, cx: Option<&App>) {
        self.file_column_resize_drag = None;
        self.folder_view_revision = None;
        let mut mode = None;
        if let Some(cx) = cx.filter(|cx| cx.has_global::<FolderViews>()) {
            let saved = self.folder_view(cx);
            self.folder_view_revision = saved.map(|(revision, _)| revision);
            let (saved_mode, sort, columns) = self.resolve_preferences(saved.map(|(_, view)| view));
            mode = saved_mode;
            self.file_sort = sort;
            self.file_columns = columns;
        }
        self.view_mode_selection = if mode.is_some() {
            ViewModeSelection::Manual
        } else {
            ViewModeSelection::Pending
        };
        self.set_active_view_mode(mode.unwrap_or(self.base_view_mode));
    }

    pub(super) fn observe_folder_views(&self, cx: &mut Context<Self>) {
        if cx.has_global::<FolderViews>() {
            cx.observe_global::<FolderViews>(|view, cx| {
                view.refresh_folder_view(cx);
            })
            .detach();
        }
    }

    pub(super) fn refresh_folder_view(&mut self, cx: &mut Context<Self>) {
        if !self.folder_view_enabled(cx) {
            return;
        }
        let saved = self.folder_view(cx);
        let revision = saved.map(|(revision, _)| revision);
        if self.folder_view_revision == revision {
            return;
        }
        self.folder_view_revision = revision;
        let (mode, sort, columns) = self.resolve_preferences(saved.map(|(_, view)| view));
        self.apply_folder_view_mode(mode, false);
        self.apply_view_columns(columns);
        if self.file_sort != sort {
            self.file_sort = sort;
            self.apply_file_sort_preserving_selection();
        }
        cx.notify();
    }

    pub(super) fn apply_folder_view_mode(
        &mut self,
        mode: Option<FileViewMode>,
        defaults_changed: bool,
    ) {
        if let Some(mode) = mode {
            self.view_mode_selection = ViewModeSelection::Manual;
            self.set_active_view_mode(mode);
            return;
        }
        match self.view_mode_selection {
            ViewModeSelection::Manual
                if !self.is_directory_loading() || !self.all_entries.is_empty() =>
            {
                self.view_mode_selection = ViewModeSelection::Automatic;
                self.apply_automatic_view_mode();
            }
            ViewModeSelection::Pending | ViewModeSelection::Manual => {
                self.view_mode_selection = ViewModeSelection::Pending;
                self.set_active_view_mode(self.base_view_mode);
            }
            ViewModeSelection::Automatic if defaults_changed => {
                self.apply_automatic_view_mode();
            }
            ViewModeSelection::Automatic => {}
        }
    }

    pub(super) fn apply_view_columns(&mut self, mut columns: FileColumnSettings) {
        // A settings update from another pane must preserve the local resize preview.
        if let Some(drag) = self.file_column_resize_drag {
            match drag.target {
                FileColumnResizeTarget::Name => columns.name_width = self.file_columns.name_width,
                FileColumnResizeTarget::Column(kind) => {
                    if let Some(width) = self.file_columns.widths.get(&kind) {
                        columns.widths.insert(kind, *width);
                    }
                }
            }
        }
        self.file_columns = columns;
    }

    pub(in crate::explorer) fn has_saved_folder_view(&self, cx: &App) -> bool {
        self.folder_view(cx).is_some()
    }

    pub(in crate::explorer) fn use_default_folder_view(&mut self, cx: &mut Context<Self>) {
        self.file_column_resize_drag = None;
        self.edit_folder_view(cx, |view| *view = FolderView::default());
    }

    pub(super) fn edit_folder_view(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut FolderView),
    ) {
        if let Some(path) = self.folder_view_path() {
            folder_views::edit(cx, path, edit);
            self.refresh_folder_view(cx);
        }
    }

    pub(in crate::explorer) fn persist_view_sort(
        &mut self,
        sort: FileSortSettings,
        cx: &mut Context<Self>,
    ) {
        if self.is_trash_view() || self.search.recursive_results_active {
            return;
        }
        if self.folder_view_enabled(cx) {
            self.edit_folder_view(cx, |view| view.sort = Some(sort));
        } else {
            settings::set_file_sort(sort, cx);
        }
    }

    fn edit_view_columns(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut FileColumnSettings),
    ) {
        let defaults = self.default_file_columns.clone();
        self.edit_folder_view(cx, |view| edit(view.columns.get_or_insert(defaults)));
    }

    pub(in crate::explorer) fn toggle_folder_column(
        &mut self,
        kind: FileColumnKind,
        cx: &mut Context<Self>,
    ) {
        if self.folder_view_enabled(cx) && !self.folder_view_is_loading(cx) {
            self.edit_view_columns(cx, |columns| {
                if !columns.hidden.remove(&kind) {
                    columns.hidden.insert(kind);
                }
            });
        }
    }

    pub(in crate::explorer) fn persist_column_resize(
        &mut self,
        result: FileColumnResizeResult,
        cx: &mut Context<Self>,
    ) {
        if self.is_trash_view() {
            return;
        }
        if self.folder_view_enabled(cx) {
            self.edit_view_columns(cx, |columns| match result {
                FileColumnResizeResult::Name(width) => columns.name_width = Some(width),
                FileColumnResizeResult::Column(kind, width) => {
                    columns.widths.insert(kind, width);
                }
            });
        } else {
            match result {
                FileColumnResizeResult::Name(width) => settings::set_name_column_width(width, cx),
                FileColumnResizeResult::Column(kind, width) => {
                    settings::set_file_column_width(kind, width, cx)
                }
            }
        }
    }

    pub(in crate::explorer) fn persist_name_width_reset(&mut self, cx: &mut Context<Self>) {
        if self.is_trash_view() {
            return;
        }
        if self.folder_view_enabled(cx) {
            self.edit_view_columns(cx, |columns| columns.name_width = None);
        } else {
            settings::clear_name_column_width(cx);
        }
    }

    pub(in crate::explorer) fn persist_column_reorder(
        &mut self,
        dragged: FileColumnKind,
        target: FileColumnKind,
        before: bool,
        cx: &mut Context<Self>,
    ) {
        if self.is_trash_view() {
            return;
        }
        if self.folder_view_enabled(cx) {
            self.edit_view_columns(cx, |columns| {
                settings::reorder_file_column_in_settings(columns, dragged, target, before);
            });
        } else {
            settings::reorder_file_column(dragged, target, before, cx);
        }
    }
}

pub(super) async fn wait_for_folder_views(ready: Option<Ready>) {
    if let Some(ready) = ready {
        let _span = crate::startup_trace::Span::new("folder_views_wait");
        let started = Instant::now();
        let _ = ready.await;
        crate::debug_options::log_nav_timing(
            started.elapsed(),
            format_args!("reload.folder_views_wait"),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use gpui::{AppContext, Modifiers, MouseButton, TestAppContext};

    use crate::{
        explorer::{navigation::HistoryMode, view::ExplorerContentBranch},
        settings::{ExplorerSettings, FileSortColumn, SortDirection},
    };

    use super::*;

    fn open_folder(path: PathBuf, cx: &mut Context<ExplorerView>) -> ExplorerView {
        let mut view = ExplorerView::new_unloaded_with_settings_for_test(
            path,
            Some(cx.focus_handle()),
            &ExplorerSettings::default(),
        );
        view.reset_folder_view_for_navigation(Some(cx));
        view.observe_folder_views(cx);
        view.start_initial_directory_load(cx);
        view
    }

    #[gpui::test]
    fn folder_view_changes_sync_between_windows_and_inherit_untouched_defaults(
        cx: &mut TestAppContext,
    ) {
        cx.set_global(FolderViews::default());
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        let item = path.join("photo.jpg");
        fs::write(&item, "photo").unwrap();
        let first = cx
            .add_window_view({
                let path = path.clone();
                move |_, cx| open_folder(path, cx)
            })
            .0;
        let (second, cx) = cx.add_window_view({
            let path = path.clone();
            move |_, cx| open_folder(path, cx)
        });
        cx.run_until_parked();
        let sort = FileSortSettings {
            column: FileSortColumn::Size,
            direction: SortDirection::Descending,
        };
        cx.update(|_, app| {
            second.update(app, |view, _| {
                view.restore_selection_from_paths(std::slice::from_ref(&item))
            });
            first.update(app, |view, cx| view.persist_view_sort(sort, cx));
        });
        cx.read_entity(&second, |view, _| {
            assert_eq!(view.file_sort, sort);
            assert_eq!(view.view_mode, FileViewMode::LargeIcons);
            assert_eq!(view.selected_paths(), [item.clone()]);
        });

        let mut settings = ExplorerSettings::default();
        settings.view.mode_media = FileViewMode::Details;
        settings.view.file_columns.name_width = Some(400);
        cx.update(|_, app| {
            for pane in [&first, &second] {
                pane.update(app, |view, cx| view.apply_settings(&settings, cx));
            }
        });
        cx.read_entity(&second, |view, _| {
            assert_eq!(view.file_sort, sort);
            assert_eq!(view.view_mode, FileViewMode::Details);
            assert_eq!(view.file_columns.name_width, Some(400));
        });
        cx.update(|_, app| {
            first.update(app, |view, cx| {
                view.select_view_mode(FileViewMode::LargeIcons, cx)
            });
            second.update(app, |view, cx| {
                view.persist_column_resize(FileColumnResizeResult::Name(480), cx)
            });
            first.update(app, |view, cx| {
                view.toggle_folder_column(FileColumnKind::Size, cx)
            });
        });
        cx.read_entity(&second, |view, _| {
            assert_eq!(view.view_mode, FileViewMode::LargeIcons);
            assert_eq!(view.file_columns.name_width, Some(480));
            assert!(view.file_columns.hidden.contains(&FileColumnKind::Size));
        });
        cx.update(|_, app| {
            first.update(app, |view, cx| view.use_default_folder_view(cx));
        });
        cx.read_entity(&second, |view, app| {
            assert_eq!(view.file_sort, settings.view.sort);
            assert_eq!(view.file_columns, settings.view.file_columns);
            assert_eq!(view.view_mode, FileViewMode::Details);
            assert!(!view.has_saved_folder_view(app));
            assert_eq!(view.selected_paths(), [item]);
        });
    }

    #[gpui::test]
    fn folder_view_startup_gates_entries_and_discards_a_stale_navigation(cx: &mut TestAppContext) {
        let pending = cx.update(FolderViews::pause_startup);
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("first");
        let target = temp.path().join("second");
        fs::create_dir(&original).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(original.join("original.txt"), "1").unwrap();
        fs::write(target.join("destination.txt"), "2").unwrap();
        let (view, cx) = cx.add_window_view({
            let original = original.clone();
            move |_, cx| open_folder(original, cx)
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(view.entries.is_empty());
            assert_eq!(view.content_branch(), ExplorerContentBranch::Loading);
        });
        assert!(cx.debug_bounds("explorer-header-name").is_none());

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.navigate_to_directory_with_watcher(target.clone(), HistoryMode::Record, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|_, app| {
            app.set_global(FolderViews::default());
            folder_views::edit(app, &original, |view| {
                view.mode = Some(FileViewMode::Details)
            });
            folder_views::edit(app, &target, |view| {
                view.mode = Some(FileViewMode::LargeIcons)
            });
            pending.send(()).unwrap();
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.path, target);
            assert_eq!(view.view_mode, FileViewMode::LargeIcons);
            assert_eq!(view.entries[0].name, "destination.txt");
        });
        cx.update(|_, app| view.update(app, |view, cx| view.navigate_back_with_watcher(cx)));
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.path, original);
            assert_eq!(view.view_mode, FileViewMode::Details);
            assert_eq!(view.entries[0].name, "original.txt");
        });
    }

    #[gpui::test]
    fn folder_view_details_menu_hides_and_restores_a_column_and_can_reset(cx: &mut TestAppContext) {
        cx.set_global(FolderViews::default());
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        fs::write(path.join("file.txt"), "contents").unwrap();
        let (view, cx) = cx.add_window_view(move |_, cx| open_folder(path, cx));
        cx.run_until_parked();
        let name_bounds = cx
            .debug_bounds("explorer-header-name")
            .expect("Name header");
        let name = name_bounds.center();
        let size_width = cx
            .debug_bounds("explorer-header-size")
            .expect("Size header")
            .size
            .width;

        for hidden in [true, false] {
            cx.simulate_mouse_move(name, None, Modifiers::default());
            cx.simulate_mouse_down(name, MouseButton::Right, Modifiers::default());
            cx.simulate_mouse_up(name, MouseButton::Right, Modifiers::default());
            cx.run_until_parked();
            let menu_item = cx
                .debug_bounds("file-column-Size")
                .expect("Size menu item")
                .center();
            cx.simulate_click(menu_item, Modifiers::default());
            cx.run_until_parked();
            cx.read_entity(&view, |view, _| {
                assert_eq!(
                    view.file_columns.hidden.contains(&FileColumnKind::Size),
                    hidden
                );
            });
            let rendered_name = cx.debug_bounds("explorer-header-name").unwrap();
            let expected_width =
                name_bounds.size.width + if hidden { size_width } else { gpui::px(0.0) };
            assert_eq!(rendered_name.size.width, expected_width);
        }
        let view_button = cx
            .debug_bounds("utility-view")
            .expect("View button")
            .center();
        cx.simulate_click(view_button, Modifiers::default());
        cx.run_until_parked();
        let reset = cx
            .debug_bounds("utility-use-default-view")
            .expect("Use default view")
            .center();
        cx.simulate_click(reset, Modifiers::default());
        cx.run_until_parked();
        cx.read_entity(&view, |view, app| assert!(!view.has_saved_folder_view(app)));
    }

    #[gpui::test]
    fn folder_view_recursive_sort_does_not_replace_the_folder_sort(cx: &mut TestAppContext) {
        cx.set_global(FolderViews::default());
        let temp = tempfile::tempdir().unwrap();
        let (view, cx) = cx.add_window_view({
            let path = temp.path().to_path_buf();
            move |_, cx| open_folder(path, cx)
        });
        cx.run_until_parked();
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                let sort = view.file_sort;
                view.search.recursive_results_active = true;
                let recursive = view.sort_entries_from_header(FileSortColumn::Size);
                view.persist_view_sort(recursive, cx);
                assert_eq!(view.header_file_sort(), Some(recursive));
                assert_eq!(view.file_sort, sort);
                assert!(!view.has_saved_folder_view(cx));
            });
        });
    }
}
