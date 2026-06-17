use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use gpui::{
    AnyWindowHandle, Context, EventEmitter, FocusHandle, Font, Pixels, Point, Subscription, Task,
    UniformListScrollHandle, point, px,
};

use crate::explorer::sidebar::{SidebarSections, sidebar_sections};
use crate::explorer::{
    address_bar::AddressBarState,
    archive_diagnostics::ArchiveDiagnostics,
    codebase_summary::{CodebaseSummary, find_git_repository_root, scan_codebase_summary},
    context_menu::ContextMenuState,
    drag_drop::DropIndicator,
    entry::{FileEntry, ShellShortcutTargetKind, resolve_shell_shortcut_target_kind},
    filesystem::{FileConflictBatch, FileOperationProgress, load_entries},
    folder_size::{FolderSizeCache, FolderSizeError, calculate_folder_size},
    large_icons::{LargeIconLayout, LargeIconLayoutKey},
    mouse_selection::MouseSelectionDrag,
    rename::{PendingClickRename, RenameState},
    scrollbar::{HorizontalScrollbarDrag, ScrollbarDrag},
    search::SearchState,
    selection::SelectionState,
    watcher::DirectoryWatcher,
};
use crate::settings::{
    ExplorerSettings, FileColumnKind, FileColumnSettings, FileViewMode, SidebarLocation,
};

pub struct ExplorerView {
    pub(super) path: PathBuf,
    pub(super) entries: Vec<FileEntry>,
    pub(super) all_entries: Vec<FileEntry>,
    pub(super) selection: SelectionState,
    pub(super) read_error: Option<String>,
    pub(super) open_error: Option<String>,
    pub(super) open_with_task: Option<Task<()>>,
    pub(super) back_stack: Vec<PathBuf>,
    pub(super) forward_stack: Vec<PathBuf>,
    pub(super) scroll_handle: UniformListScrollHandle,
    pub(super) large_icon_list_state: gpui::ListState,
    pub(super) large_icon_layout: Option<LargeIconLayout>,
    pub(super) large_icon_layout_key: Option<LargeIconLayoutKey>,
    pub(super) focus_handle: Option<FocusHandle>,
    pub(super) scrollbar_hovered: bool,
    pub(super) scrollbar_drag: Option<ScrollbarDrag>,
    pub(super) horizontal_scrollbar_hovered: bool,
    pub(super) horizontal_scrollbar_drag: Option<HorizontalScrollbarDrag>,
    pub(super) mouse_selection_drag: Option<MouseSelectionDrag>,
    pub(super) suppress_next_click: bool,
    pub(super) entry_click_sequence: Option<EntryClickSequence>,
    pub(super) cut_paths: BTreeSet<PathBuf>,
    pub(super) active_drop_indicator: Option<DropIndicator>,
    pub(super) dragging_sidebar_item: Option<usize>,
    pub(super) sidebar_width: f32,
    pub(super) sidebar_resize_drag: Option<SidebarResizeDrag>,
    pub(super) file_columns: FileColumnSettings,
    pub(super) file_column_resize_drag: Option<FileColumnResizeDrag>,
    pub(super) pending_permanent_delete: Option<PendingPermanentDelete>,
    pub(super) pending_trash: Option<PendingTrash>,
    pub(super) pending_file_conflict: Option<FileConflictBatch>,
    pub(super) active_file_operation: Option<FileOperationState>,
    pub(super) active_dialog_window: Option<AnyWindowHandle>,
    pub(super) active_rename: Option<RenameState>,
    pub(super) rename_focus_out: Option<Subscription>,
    pub(super) active_address_bar: Option<AddressBarState>,
    pub(super) search: SearchState,
    pub(super) pending_click_rename: Option<PendingClickRename>,
    pub(super) next_pending_click_rename_id: u64,
    pub(super) date_format: String,
    pub(super) font: Font,
    pub(super) show_hidden_files: bool,
    pub(super) show_file_name_extensions: bool,
    pub(super) show_folder_size: bool,
    pub(super) resolve_icons: bool,
    pub(super) base_view_mode: FileViewMode,
    pub(super) media_view_mode: FileViewMode,
    pub(super) view_mode: FileViewMode,
    pub(super) open_utility_menu: Option<UtilityMenu>,
    pub(super) context_menu: Option<ContextMenuState>,
    pub(super) view_origin: Point<Pixels>,
    pub(super) directory_watcher: Option<DirectoryWatcher>,
    pub(super) sidebar_items: Vec<SidebarLocation>,
    pub(super) sidebar_sections: SidebarSections,
    pub(super) shell_shortcut_resolution_generation: u64,
    pub(super) shell_shortcut_resolution_task: Option<Task<()>>,
    pub(super) folder_size_generation: u64,
    pub(super) folder_size_task: Option<Task<()>>,
    pub(super) folder_size_cancel: Option<Arc<AtomicBool>>,
    pub(super) codebase_summary: Option<CodebaseSummary>,
    pub(super) codebase_summary_generation: u64,
    pub(super) codebase_summary_task: Option<Task<()>>,
}

pub(super) struct FileOperationState {
    pub(super) progress: FileOperationProgress,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) task: Option<Task<()>>,
    pub(super) archive_diagnostics: Option<ArchiveDiagnostics>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct EntryClickSequence {
    pub(super) path: PathBuf,
    pub(super) last_raw_click_count: usize,
    pub(super) effective_click_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SidebarResizeDrag {
    pub(super) start_pointer_x: f32,
    pub(super) start_width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FileColumnResizeDrag {
    pub(super) kind: FileColumnKind,
    pub(super) start_pointer_x: f32,
    pub(super) start_width: f32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ShellShortcutResolution {
    pub(super) path: PathBuf,
    pub(super) target_kind: ShellShortcutTargetKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ExplorerViewEvent {
    FilesystemChanged,
    OpenDirectoryInNewTab(PathBuf),
}

impl EventEmitter<ExplorerViewEvent> for ExplorerView {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PendingPermanentDelete {
    pub(super) paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PendingTrash {
    pub(super) paths: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExplorerContentBranch {
    Error,
    Empty,
    SearchWorking,
    NoSearchMatches,
    List,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum UtilityMenu {
    New,
    View,
}

impl ExplorerView {
    #[cfg(test)]
    pub fn new(initial_path: PathBuf) -> Self {
        Self::new_inner(initial_path, None)
    }

    pub fn new_watched_with_focus_handle(
        initial_path: PathBuf,
        focus_handle: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = cx.global::<crate::settings::SettingsState>().value.clone();
        let mut view = Self::new_inner_with_settings(initial_path, Some(focus_handle), &settings);
        view.restart_directory_watcher(cx);
        view.schedule_entry_metadata_resolution(cx);
        view.observe_icon_caches(cx);
        view.observe_image_thumbnail_cache(cx);
        view
    }

    #[cfg(test)]
    pub(super) fn new_with_focus_handle_for_test(
        initial_path: PathBuf,
        focus_handle: FocusHandle,
    ) -> Self {
        Self::new_inner(initial_path, Some(focus_handle))
    }

    #[cfg(test)]
    fn new_inner(initial_path: PathBuf, focus_handle: Option<FocusHandle>) -> Self {
        Self::new_inner_with_settings(initial_path, focus_handle, &ExplorerSettings::default())
    }

    fn new_inner_with_settings(
        initial_path: PathBuf,
        focus_handle: Option<FocusHandle>,
        settings: &ExplorerSettings,
    ) -> Self {
        let mut view = Self {
            path: initial_path,
            entries: Vec::new(),
            all_entries: Vec::new(),
            selection: SelectionState::default(),
            read_error: None,
            open_error: None,
            open_with_task: None,
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            scroll_handle: UniformListScrollHandle::new(),
            large_icon_list_state: gpui::ListState::new(0, gpui::ListAlignment::Top, px(400.0))
                .measure_all(),
            large_icon_layout: None,
            large_icon_layout_key: None,
            focus_handle,
            scrollbar_hovered: false,
            scrollbar_drag: None,
            horizontal_scrollbar_hovered: false,
            horizontal_scrollbar_drag: None,
            mouse_selection_drag: None,
            suppress_next_click: false,
            entry_click_sequence: None,
            cut_paths: BTreeSet::new(),
            active_drop_indicator: None,
            dragging_sidebar_item: None,
            sidebar_width: settings.sidebar.width as f32,
            sidebar_resize_drag: None,
            file_columns: settings.view.file_columns.clone(),
            file_column_resize_drag: None,
            pending_permanent_delete: None,
            pending_trash: None,
            pending_file_conflict: None,
            active_file_operation: None,
            active_dialog_window: None,
            active_rename: None,
            rename_focus_out: None,
            active_address_bar: None,
            search: SearchState::default(),
            pending_click_rename: None,
            next_pending_click_rename_id: 0,
            date_format: settings.view.date_format.clone(),
            font: crate::settings::app_font(settings),
            show_hidden_files: settings.view.show_hidden,
            show_file_name_extensions: settings.view.show_extensions,
            show_folder_size: settings.view.show_folder_sizes,
            resolve_icons: settings.view.native_icons,
            base_view_mode: settings.view.mode,
            media_view_mode: settings.view.mode_media,
            view_mode: settings.view.mode,
            open_utility_menu: None,
            context_menu: None,
            view_origin: point(px(0.0), px(0.0)),
            directory_watcher: None,
            sidebar_items: settings.sidebar.items.clone(),
            sidebar_sections: SidebarSections::default(),
            shell_shortcut_resolution_generation: 0,
            shell_shortcut_resolution_task: None,
            folder_size_generation: 0,
            folder_size_task: None,
            folder_size_cancel: None,
            codebase_summary: None,
            codebase_summary_generation: 0,
            codebase_summary_task: None,
        };
        view.reload();
        view
    }

    pub(super) fn apply_settings(&mut self, settings: &ExplorerSettings, cx: &mut Context<Self>) {
        let hidden_changed = self.show_hidden_files != settings.view.show_hidden;
        let folder_size_changed = self.show_folder_size != settings.view.show_folder_sizes;
        self.date_format.clone_from(&settings.view.date_format);
        self.font = crate::settings::app_font(settings);
        self.show_hidden_files = settings.view.show_hidden;
        self.show_file_name_extensions = settings.view.show_extensions;
        self.show_folder_size = settings.view.show_folder_sizes;
        self.resolve_icons = settings.view.native_icons;
        self.base_view_mode = settings.view.mode;
        self.media_view_mode = settings.view.mode_media;

        self.sidebar_items = settings.sidebar.items.clone();
        if self.sidebar_resize_drag.is_none() {
            self.sidebar_width = settings.sidebar.width as f32;
        }
        if let Some(drag) = self.file_column_resize_drag {
            let width = self.file_columns.widths.get(&drag.kind).copied();
            self.file_columns = settings.view.file_columns.clone();
            if let Some(width) = width {
                self.file_columns.widths.insert(drag.kind, width);
            }
        } else {
            self.file_columns = settings.view.file_columns.clone();
        }

        if hidden_changed {
            self.invalidate_recursive_search_cache();
            self.reload();
            self.schedule_entry_metadata_resolution(cx);
            self.refresh_search_after_external_change(cx);
        } else if folder_size_changed {
            if self.show_folder_size {
                self.schedule_folder_sizes(cx);
            } else {
                self.cancel_folder_size_task();
                self.clear_folder_sizes();
            }
        } else {
            self.sidebar_sections = sidebar_sections(&self.sidebar_items);
        }
        self.apply_effective_view_mode();
        cx.notify();
    }

    pub fn reload(&mut self) {
        let _timing_batch = crate::debug_options::NavTimingBatch::start();
        self.reload_inner(ReloadMode {
            preserve_selection: true,
            rebuild_sidebar: true,
        });
    }

    pub(super) fn reload_for_navigation(&mut self) {
        self.reload_inner(ReloadMode {
            preserve_selection: false,
            rebuild_sidebar: false,
        });
    }

    fn reload_inner(&mut self, mode: ReloadMode) {
        let total_started = Instant::now();
        self.cancel_folder_size_task();
        self.context_menu = None;
        self.open_error = None;
        let selected_paths_started = Instant::now();
        let selected_paths = if mode.preserve_selection {
            self.selected_paths()
        } else {
            Vec::new()
        };
        crate::debug_options::log_nav_timing(
            selected_paths_started.elapsed(),
            format_args!(
                "reload.selected_paths path={:?} selected={}",
                self.path,
                selected_paths.len()
            ),
        );

        if mode.rebuild_sidebar {
            let sidebar_started = Instant::now();
            self.sidebar_sections = sidebar_sections(&self.sidebar_items);
            crate::debug_options::log_nav_timing(
                sidebar_started.elapsed(),
                format_args!("reload.sidebar_sections path={:?}", self.path),
            );
        }

        let load_started = Instant::now();
        match load_entries(&self.path, self.show_hidden_files) {
            Ok(entries) => {
                crate::debug_options::log_nav_timing(
                    load_started.elapsed(),
                    format_args!(
                        "reload.load_entries path={:?} ok=true entries={}",
                        self.path,
                        entries.len()
                    ),
                );
                self.read_error = None;
                let filter_started = Instant::now();
                if self.search_is_active() {
                    self.all_entries = entries;
                    self.apply_search_filter_preserving_selection(&selected_paths);
                } else {
                    self.entries = entries.clone();
                    self.all_entries = entries;
                    if mode.preserve_selection {
                        self.restore_selection_from_paths(&selected_paths);
                    } else {
                        self.selection = SelectionState::default();
                    }
                }
                crate::debug_options::log_nav_timing(
                    filter_started.elapsed(),
                    format_args!(
                        "reload.search_filter path={:?} query={:?} visible={} selected={}",
                        self.path,
                        self.search_query(),
                        self.entries.len(),
                        self.selection.selected_indices.len()
                    ),
                );
            }
            Err(error) => {
                crate::debug_options::log_nav_timing(
                    load_started.elapsed(),
                    format_args!(
                        "reload.load_entries path={:?} ok=false error={error}",
                        self.path
                    ),
                );
                self.all_entries.clear();
                self.entries.clear();
                self.clear_selection();
                self.read_error = Some(error.to_string());
            }
        }
        if self.entries.is_empty() {
            self.set_horizontal_scroll_offset(0.0);
            self.horizontal_scrollbar_drag = None;
        }
        self.apply_effective_view_mode();
        crate::debug_options::log_nav_timing(
            total_started.elapsed(),
            format_args!(
                "reload.total path={:?} entries={} all_entries={} read_error={}",
                self.path,
                self.entries.len(),
                self.all_entries.len(),
                self.read_error.is_some()
            ),
        );
    }

    pub(super) fn reload_with_entry_metadata_resolution(&mut self, cx: &mut Context<Self>) {
        self.reload();
        self.schedule_entry_metadata_resolution(cx);
    }

    pub(super) fn refresh_with_entry_metadata_resolution(&mut self, cx: &mut Context<Self>) {
        self.reload();
        self.invalidate_current_folder_size_cache(cx);
        self.schedule_entry_metadata_resolution(cx);
    }

    pub(super) fn schedule_entry_metadata_resolution(&mut self, cx: &mut Context<Self>) {
        self.schedule_pending_shell_shortcut_resolution(cx);
        self.schedule_folder_sizes(cx);
        self.schedule_codebase_summary(cx);
    }

    pub(super) fn schedule_codebase_summary(&mut self, cx: &mut Context<Self>) {
        self.codebase_summary_generation = self.codebase_summary_generation.wrapping_add(1);
        let generation = self.codebase_summary_generation;
        let path = self.path.clone();
        let Some(repo_root) = find_git_repository_root(&path) else {
            self.codebase_summary = None;
            self.codebase_summary_task = None;
            return;
        };

        if self
            .codebase_summary
            .as_ref()
            .is_none_or(|summary| summary.repo_root != repo_root)
        {
            self.codebase_summary = None;
        }

        let task = cx.spawn(async move |this, cx| {
            let output_task = cx
                .background_executor()
                .spawn(async move { scan_codebase_summary(&path) });
            let summary = output_task.await;

            let _ = this.update(cx, |explorer, cx| {
                if explorer.codebase_summary_generation != generation {
                    return;
                }

                explorer.codebase_summary = summary;
                explorer.codebase_summary_task = None;
                cx.notify();
            });
        });
        self.codebase_summary_task = Some(task);
    }

    fn schedule_pending_shell_shortcut_resolution(&mut self, cx: &mut Context<Self>) {
        self.shell_shortcut_resolution_generation =
            self.shell_shortcut_resolution_generation.wrapping_add(1);
        let generation = self.shell_shortcut_resolution_generation;
        let path = self.path.clone();
        let pending_targets = self.pending_shell_shortcut_targets();

        if pending_targets.is_empty() {
            self.shell_shortcut_resolution_task = None;
            return;
        }

        let task = cx.spawn(async move |this, cx| {
            let output_task = cx.background_executor().spawn(async move {
                pending_targets
                    .into_iter()
                    .map(|(path, target)| ShellShortcutResolution {
                        path,
                        target_kind: resolve_shell_shortcut_target_kind(&target),
                    })
                    .collect::<Vec<_>>()
            });
            let resolutions = output_task.await;

            let _ = this.update(cx, |explorer, cx| {
                if explorer.apply_shell_shortcut_resolutions(&path, generation, resolutions) {
                    cx.notify();
                }
            });
        });
        self.shell_shortcut_resolution_task = Some(task);
    }

    pub(super) fn schedule_folder_sizes(&mut self, cx: &mut Context<Self>) {
        self.cancel_folder_size_task();
        if !self.show_folder_size {
            return;
        }

        let root = self.path.clone();
        let generation = self.folder_size_generation;
        let targets = self
            .all_entries
            .iter()
            .filter(|entry| entry.is_real_directory())
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let mut missing = Vec::new();
        let mut cached = Vec::new();
        if let Some(cache) = cx.try_global::<FolderSizeCache>() {
            for path in targets {
                if let Some(size) = cache.get(&path) {
                    cached.push((path, size));
                } else {
                    missing.push(path);
                }
            }
        } else {
            missing = targets;
        }

        for (path, size) in cached {
            self.apply_folder_size(&path, size);
        }
        if missing.is_empty() {
            return;
        }

        let cancel = Arc::new(AtomicBool::new(false));
        self.folder_size_cancel = Some(cancel.clone());
        let task = cx.spawn(async move |this, cx| {
            for path in missing {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }

                let calculation_path = path.clone();
                let calculation_cancel = cancel.clone();
                let result =
                    cx.background_executor()
                        .spawn(async move {
                            calculate_folder_size(&calculation_path, &calculation_cancel)
                        })
                        .await;

                let should_continue = this
                    .update(cx, |explorer, cx| {
                        if explorer.path != root
                            || explorer.folder_size_generation != generation
                            || !explorer.show_folder_size
                        {
                            return false;
                        }

                        match result {
                            Ok(size) => {
                                if let Some(cache) = cx.try_global::<FolderSizeCache>() {
                                    cache.insert(path.clone(), size);
                                }
                                if explorer.apply_folder_size(&path, size) {
                                    cx.notify();
                                }
                                true
                            }
                            Err(FolderSizeError::Cancelled) => false,
                            Err(FolderSizeError::Unavailable) => true,
                        }
                    })
                    .unwrap_or(false);
                if !should_continue {
                    break;
                }
            }

            let _ = this.update(cx, |explorer, _| {
                if explorer.folder_size_generation == generation {
                    explorer.folder_size_cancel = None;
                    explorer.folder_size_task = None;
                }
            });
        });
        self.folder_size_task = Some(task);
    }

    fn cancel_folder_size_task(&mut self) {
        self.folder_size_generation = self.folder_size_generation.wrapping_add(1);
        if let Some(cancel) = self.folder_size_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.folder_size_task = None;
    }

    fn invalidate_current_folder_size_cache(&self, cx: &mut Context<Self>) {
        let paths = self
            .all_entries
            .iter()
            .filter(|entry| entry.is_real_directory())
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        if let Some(cache) = cx.try_global::<FolderSizeCache>() {
            cache.invalidate(paths.iter());
        }
    }

    fn apply_folder_size(&mut self, path: &Path, size: u64) -> bool {
        let mut changed = apply_folder_size_to_entries(&mut self.all_entries, path, Some(size));
        if !self.search.recursive_results_active {
            changed |= apply_folder_size_to_entries(&mut self.entries, path, Some(size));
        }
        changed
    }

    fn clear_folder_sizes(&mut self) -> bool {
        let mut changed = clear_folder_sizes_in_entries(&mut self.all_entries);
        if !self.search.recursive_results_active {
            changed |= clear_folder_sizes_in_entries(&mut self.entries);
        }
        changed
    }

    fn pending_shell_shortcut_targets(&self) -> Vec<(PathBuf, PathBuf)> {
        self.all_entries
            .iter()
            .filter_map(FileEntry::pending_shell_shortcut_target)
            .collect()
    }

    pub(super) fn apply_shell_shortcut_resolutions(
        &mut self,
        path: &Path,
        generation: u64,
        resolutions: Vec<ShellShortcutResolution>,
    ) -> bool {
        if self.path != path || self.shell_shortcut_resolution_generation != generation {
            return false;
        }

        let selected_paths = self.selected_paths();
        let mut changed = false;
        for resolution in resolutions {
            changed |=
                apply_shell_shortcut_resolution_to_entries(&mut self.all_entries, &resolution);
            changed |= apply_shell_shortcut_resolution_to_entries(&mut self.entries, &resolution);
        }

        if changed {
            self.restore_selection_from_paths(&selected_paths);
        }

        changed
    }

    pub(super) fn emit_filesystem_changed(&self, cx: &mut Context<Self>) {
        cx.emit(ExplorerViewEvent::FilesystemChanged);
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn restart_directory_watcher(&mut self, cx: &mut Context<Self>) -> bool {
        let started = Instant::now();
        self.directory_watcher = DirectoryWatcher::start(self.path.clone(), cx);
        let ok = self.directory_watcher.is_some();
        crate::debug_options::log_nav_timing(
            started.elapsed(),
            format_args!("watcher.restart path={:?} ok={ok}", self.path),
        );
        ok
    }

    pub(super) fn tab_label(&self) -> String {
        tab_label_for_path(&self.path)
    }

    pub(super) fn has_active_file_operation(&self) -> bool {
        self.active_file_operation.is_some()
    }

    pub(super) fn active_drop_indicator(&self) -> Option<DropIndicator> {
        self.active_drop_indicator.clone()
    }

    pub(super) fn begin_sidebar_resize(&mut self, pointer_x: f32) {
        self.sidebar_resize_drag = Some(SidebarResizeDrag {
            start_pointer_x: pointer_x,
            start_width: self.sidebar_width,
        });
    }

    pub(super) fn update_sidebar_resize(&mut self, pointer_x: f32) -> bool {
        let Some(drag) = self.sidebar_resize_drag else {
            return false;
        };

        let width = sidebar_width_for_drag(drag.start_width, pointer_x - drag.start_pointer_x);
        if (self.sidebar_width - width).abs() <= f32::EPSILON {
            return false;
        }

        self.sidebar_width = width;
        true
    }

    pub(super) fn finish_sidebar_resize(&mut self) -> Option<u32> {
        self.sidebar_resize_drag.take()?;
        Some(normalized_sidebar_width_f32(self.sidebar_width).round() as u32)
    }

    pub(super) fn reset_sidebar_width(&mut self) -> u32 {
        self.sidebar_resize_drag = None;
        self.sidebar_width = crate::settings::SIDEBAR_DEFAULT_WIDTH as f32;
        crate::settings::SIDEBAR_DEFAULT_WIDTH
    }

    pub(super) fn file_column_width(&self, kind: FileColumnKind) -> f32 {
        crate::explorer::columns::file_column_width(&self.file_columns, kind)
    }

    pub(super) fn minimum_file_columns_width(&self) -> f32 {
        crate::explorer::columns::minimum_file_columns_width(&self.file_columns)
    }

    pub(super) fn effective_name_column_width(&self, viewport_width: f32) -> f32 {
        crate::explorer::columns::effective_name_column_width(viewport_width, &self.file_columns)
    }

    pub(super) fn begin_file_column_resize(&mut self, kind: FileColumnKind, pointer_x: f32) {
        self.file_column_resize_drag = Some(FileColumnResizeDrag {
            kind,
            start_pointer_x: pointer_x,
            start_width: self.file_column_width(kind),
        });
    }

    pub(super) fn update_file_column_resize(&mut self, pointer_x: f32) -> bool {
        let Some(drag) = self.file_column_resize_drag else {
            return false;
        };

        let width = crate::settings::normalized_file_column_width(
            (drag.start_width + pointer_x - drag.start_pointer_x)
                .round()
                .max(0.0) as u32,
        );
        let current_width = self
            .file_columns
            .widths
            .get(&drag.kind)
            .copied()
            .unwrap_or_else(|| crate::settings::default_file_column_width(drag.kind));
        if current_width == width {
            return false;
        }

        self.file_columns.widths.insert(drag.kind, width);
        true
    }

    pub(super) fn finish_file_column_resize(&mut self) -> Option<(FileColumnKind, u32)> {
        let drag = self.file_column_resize_drag.take()?;
        let width = self
            .file_columns
            .widths
            .get(&drag.kind)
            .copied()
            .unwrap_or_else(|| crate::settings::default_file_column_width(drag.kind));
        Some((
            drag.kind,
            crate::settings::normalized_file_column_width(width),
        ))
    }

    pub(super) fn reset_file_column_width(
        &mut self,
        kind: FileColumnKind,
    ) -> (FileColumnKind, u32) {
        self.file_column_resize_drag = None;
        let width = crate::settings::default_file_column_width(kind);
        self.file_columns.widths.insert(kind, width);
        (kind, width)
    }

    pub(super) fn reorder_file_column(
        &mut self,
        dragged: FileColumnKind,
        target: FileColumnKind,
        before: bool,
    ) -> bool {
        if dragged == target {
            return false;
        }
        let Some(dragged_index) = self
            .file_columns
            .order
            .iter()
            .position(|kind| *kind == dragged)
        else {
            return false;
        };
        let Some(mut target_index) = self
            .file_columns
            .order
            .iter()
            .position(|kind| *kind == target)
        else {
            return false;
        };
        if dragged_index < target_index {
            target_index -= 1;
        }

        let insert_index = if before {
            target_index
        } else {
            target_index + 1
        };
        let dragged = self.file_columns.order.remove(dragged_index);
        self.file_columns.order.insert(insert_index, dragged);
        true
    }

    pub(super) fn prepare_for_tab_close(&mut self, cx: &mut Context<Self>) {
        self.cancel_image_thumbnail_extraction(cx);
        self.cancel_active_rename();
        self.cancel_address_bar_edit();
        self.finish_search_edit();
        self.close_context_menu();
        self.cancel_mouse_selection_drag();
        self.sidebar_resize_drag = None;
        self.file_column_resize_drag = None;
        self.clear_drop_indicator();
        self.pending_permanent_delete = None;
        self.pending_trash = None;
        self.pending_file_conflict = None;

        if self.active_file_operation.is_none()
            && let Some(handle) = self.active_dialog_window.take()
        {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }

        self.directory_watcher = None;
    }
}

pub(super) fn normalized_sidebar_width_f32(width: f32) -> f32 {
    if width.is_finite() {
        width.max(crate::settings::SIDEBAR_MIN_WIDTH as f32)
    } else {
        crate::settings::SIDEBAR_DEFAULT_WIDTH as f32
    }
}

pub(super) fn sidebar_width_for_drag(start_width: f32, pointer_delta_x: f32) -> f32 {
    normalized_sidebar_width_f32(start_width + pointer_delta_x)
}

#[derive(Clone, Copy)]
struct ReloadMode {
    preserve_selection: bool,
    rebuild_sidebar: bool,
}

pub(super) fn tab_label_for_path(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let display = path.display().to_string();
            if display.is_empty() {
                ".".to_owned()
            } else {
                display
            }
        })
}

impl ExplorerView {
    pub(super) fn should_show_empty_folder_message(&self) -> bool {
        self.all_entries.is_empty() && self.read_error.is_none()
    }

    pub(super) fn content_branch(&self) -> ExplorerContentBranch {
        if self.read_error.is_some() {
            ExplorerContentBranch::Error
        } else if self.recursive_search_is_working() {
            ExplorerContentBranch::SearchWorking
        } else if self.should_show_empty_folder_message() {
            ExplorerContentBranch::Empty
        } else if self.entries.is_empty() && self.search_is_active() {
            ExplorerContentBranch::NoSearchMatches
        } else {
            ExplorerContentBranch::List
        }
    }
}

impl ExplorerView {
    fn apply_effective_view_mode(&mut self) -> bool {
        let view_mode = effective_view_mode_for_entries(
            &self.all_entries,
            self.base_view_mode,
            self.media_view_mode,
        );
        if self.view_mode == view_mode {
            return false;
        }

        self.scroll_to_top();
        self.horizontal_scrollbar_drag = None;
        self.view_mode = view_mode;
        true
    }
}

fn effective_view_mode_for_entries(
    entries: &[FileEntry],
    base_view_mode: FileViewMode,
    media_view_mode: FileViewMode,
) -> FileViewMode {
    if directory_is_media_majority(entries) {
        media_view_mode
    } else {
        base_view_mode
    }
}

fn directory_is_media_majority(entries: &[FileEntry]) -> bool {
    let media_entries = entries
        .iter()
        .filter(|entry| !entry.is_directory_like())
        .filter(|entry| super::icons::file_path_is_image_or_video(&entry.path))
        .count();

    (media_entries as u128) * 4 > (entries.len() as u128) * 3
}

fn apply_shell_shortcut_resolution_to_entries(
    entries: &mut [FileEntry],
    resolution: &ShellShortcutResolution,
) -> bool {
    let mut changed = false;
    for entry in entries
        .iter_mut()
        .filter(|entry| entry.path == resolution.path)
    {
        entry.resolve_shell_shortcut_target_kind(resolution.target_kind);
        changed = true;
    }
    changed
}

fn apply_folder_size_to_entries(entries: &mut [FileEntry], path: &Path, size: Option<u64>) -> bool {
    entries
        .iter_mut()
        .filter(|entry| entry.path == path)
        .fold(false, |changed, entry| {
            entry.set_folder_size(size) || changed
        })
}

fn clear_folder_sizes_in_entries(entries: &mut [FileEntry]) -> bool {
    entries.iter_mut().fold(false, |changed, entry| {
        entry.set_folder_size(None) || changed
    })
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::explorer::entry::{DirectoryLinkKind, EntryKind, FileEntry};
    use gpui::AppContext;
    use std::path::PathBuf;

    fn test_pending_shell_shortcut(path: &str, target: &str) -> FileEntry {
        FileEntry {
            path: PathBuf::from(path),
            name: path.to_owned(),
            kind: EntryKind::DirectoryLink(DirectoryLinkKind::ShellShortcut {
                target: PathBuf::from(target),
                target_kind: ShellShortcutTargetKind::Pending,
            }),
            modified: None,
            size: Some(1),
        }
    }

    fn test_file(name: &str) -> FileEntry {
        FileEntry::test(name, false, Some(1), None)
    }

    fn test_folder(name: &str) -> FileEntry {
        FileEntry::test(name, true, None, None)
    }

    #[test]
    fn empty_directory_without_error_shows_empty_folder_message() {
        let mut view = ExplorerView::new(PathBuf::from("empty"));
        view.entries.clear();
        view.read_error = None;

        assert!(view.should_show_empty_folder_message());
    }

    #[test]
    fn view_options_use_settings_defaults() {
        let view = ExplorerView::new(PathBuf::from("defaults"));

        assert!(!view.show_hidden_files);
        assert_eq!(view.date_format, crate::settings::DEFAULT_DATE_FORMAT);
        assert_eq!(view.font.family, ".SystemUIFont");
        assert!(view.show_file_name_extensions);
        assert!(!view.show_folder_size);
        assert!(view.resolve_icons);
        assert_eq!(view.base_view_mode, crate::settings::FileViewMode::Details);
        assert_eq!(
            view.media_view_mode,
            crate::settings::FileViewMode::LargeIcons
        );
        assert_eq!(view.view_mode, crate::settings::FileViewMode::Details);
        assert_eq!(
            view.sidebar_width,
            crate::settings::SIDEBAR_DEFAULT_WIDTH as f32
        );
        assert_eq!(view.open_utility_menu, None);
        assert!(view.directory_watcher.is_none());
    }

    #[test]
    fn view_uses_configured_sidebar_width() {
        let view = ExplorerView::new_inner_with_settings(
            PathBuf::from("configured"),
            None,
            &ExplorerSettings {
                sidebar: crate::settings::SidebarSettings {
                    width: 320,
                    ..crate::settings::SidebarSettings::default()
                },
                ..ExplorerSettings::default()
            },
        );

        assert_eq!(view.sidebar_width, 320.0);
    }

    #[test]
    fn view_uses_configured_resolve_icons() {
        let view = ExplorerView::new_inner_with_settings(
            PathBuf::from("configured"),
            None,
            &ExplorerSettings {
                view: crate::settings::ViewSettings {
                    native_icons: false,
                    ..crate::settings::ViewSettings::default()
                },
                ..ExplorerSettings::default()
            },
        );

        assert!(!view.resolve_icons);
    }

    #[test]
    fn view_uses_configured_view_mode() {
        let view = ExplorerView::new_inner_with_settings(
            PathBuf::from("configured"),
            None,
            &ExplorerSettings {
                view: crate::settings::ViewSettings {
                    mode: crate::settings::FileViewMode::LargeIcons,
                    ..crate::settings::ViewSettings::default()
                },
                ..ExplorerSettings::default()
            },
        );

        assert_eq!(view.view_mode, crate::settings::FileViewMode::LargeIcons);
    }

    #[test]
    fn directory_media_majority_is_strict_and_counts_folders() {
        assert!(!directory_is_media_majority(&[]));
        assert!(!directory_is_media_majority(&[
            test_file("photo.jpg"),
            test_file("notes.txt"),
        ]));
        assert!(!directory_is_media_majority(&[
            test_file("photo.jpg"),
            test_file("clip.mp4"),
            test_file("scan.png"),
            test_file("notes.txt"),
        ]));
        assert!(directory_is_media_majority(&[
            test_file("photo.jpg"),
            test_file("clip.mp4"),
            test_file("scan.png"),
            test_file("poster.webp"),
            test_file("notes.txt"),
        ]));
        assert!(!directory_is_media_majority(&[
            test_folder("folder-1"),
            test_file("photo.jpg"),
            test_file("clip.mp4"),
            test_file("scan.png"),
        ]));
        assert!(directory_is_media_majority(&[
            test_folder("folder"),
            test_file("photo.jpg"),
            test_file("clip.mp4"),
            test_file("scan.PNG"),
            test_file("poster.webp"),
        ]));
    }

    #[test]
    fn effective_view_mode_uses_base_for_non_media_majority_directories() {
        let entries = vec![
            test_folder("folder"),
            test_file("photo.jpg"),
            test_file("clip.mp4"),
            test_file("scan.png"),
        ];

        assert_eq!(
            effective_view_mode_for_entries(
                &entries,
                crate::settings::FileViewMode::LargeIcons,
                crate::settings::FileViewMode::Details,
            ),
            crate::settings::FileViewMode::LargeIcons
        );
    }

    #[test]
    fn effective_view_mode_uses_media_mode_for_media_majority_directories() {
        let entries = vec![
            test_folder("folder"),
            test_file("photo.jpg"),
            test_file("clip.mov"),
            test_file("scan.png"),
            test_file("poster.webp"),
        ];

        assert_eq!(
            effective_view_mode_for_entries(
                &entries,
                crate::settings::FileViewMode::Details,
                crate::settings::FileViewMode::LargeIcons,
            ),
            crate::settings::FileViewMode::LargeIcons
        );
    }

    #[test]
    fn effective_view_mode_uses_all_entries_not_search_filtered_entries() {
        let mut view = ExplorerView::new(PathBuf::from("media-search"));
        view.read_error = None;
        view.all_entries = vec![
            test_file("photo.jpg"),
            test_file("clip.mp4"),
            test_file("scan.png"),
            test_file("poster.webp"),
            test_file("notes.txt"),
        ];
        view.entries = vec![test_file("notes.txt")];

        assert!(view.apply_effective_view_mode());
        assert_eq!(view.view_mode, crate::settings::FileViewMode::LargeIcons);
    }

    #[test]
    fn view_uses_configured_date_format() {
        let view = ExplorerView::new_inner_with_settings(
            PathBuf::from("configured"),
            None,
            &ExplorerSettings {
                view: crate::settings::ViewSettings {
                    date_format: "%d %B %Y".to_owned(),
                    ..crate::settings::ViewSettings::default()
                },
                ..ExplorerSettings::default()
            },
        );

        assert_eq!(view.date_format, "%d %B %Y");
    }

    #[test]
    fn view_uses_configured_font() {
        let view = ExplorerView::new_inner_with_settings(
            PathBuf::from("configured"),
            None,
            &ExplorerSettings {
                view: crate::settings::ViewSettings {
                    font: "Inter".to_owned(),
                    ..crate::settings::ViewSettings::default()
                },
                ..ExplorerSettings::default()
            },
        );

        assert_eq!(view.font.family, "Inter");
    }

    #[gpui::test]
    fn apply_settings_updates_font(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_focus_handle_for_test(PathBuf::from("settings"), focus_handle)
        });

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.apply_settings(
                    &ExplorerSettings {
                        view: crate::settings::ViewSettings {
                            font: "Inter".to_owned(),
                            ..crate::settings::ViewSettings::default()
                        },
                        ..ExplorerSettings::default()
                    },
                    cx,
                );
            });
        });

        cx.read_entity(&view, |view, _| {
            assert_eq!(view.font.family, "Inter");
        });
    }

    #[gpui::test]
    fn apply_settings_updates_sidebar_width(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_focus_handle_for_test(PathBuf::from("settings"), focus_handle)
        });

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.apply_settings(
                    &ExplorerSettings {
                        sidebar: crate::settings::SidebarSettings {
                            width: 333,
                            ..crate::settings::SidebarSettings::default()
                        },
                        ..ExplorerSettings::default()
                    },
                    cx,
                );
            });
        });

        cx.read_entity(&view, |view, _| {
            assert_eq!(view.sidebar_width, 333.0);
        });
    }

    #[gpui::test]
    fn apply_settings_updates_resolve_icons(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_focus_handle_for_test(PathBuf::from("settings"), focus_handle)
        });

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.apply_settings(
                    &ExplorerSettings {
                        view: crate::settings::ViewSettings {
                            native_icons: false,
                            ..crate::settings::ViewSettings::default()
                        },
                        ..ExplorerSettings::default()
                    },
                    cx,
                );
            });
        });

        cx.read_entity(&view, |view, _| {
            assert!(!view.resolve_icons);
        });
    }

    #[gpui::test]
    fn apply_settings_updates_view_mode(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_focus_handle_for_test(PathBuf::from("settings"), focus_handle)
        });

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.apply_settings(
                    &ExplorerSettings {
                        view: crate::settings::ViewSettings {
                            mode: crate::settings::FileViewMode::LargeIcons,
                            ..crate::settings::ViewSettings::default()
                        },
                        ..ExplorerSettings::default()
                    },
                    cx,
                );
            });
        });

        cx.read_entity(&view, |view, _| {
            assert_eq!(view.view_mode, crate::settings::FileViewMode::LargeIcons);
        });
    }

    #[gpui::test]
    fn apply_settings_recomputes_media_view_mode(cx: &mut gpui::TestAppContext) {
        let temp = crate::explorer::test_support::TempDir::new();
        std::fs::write(temp.path().join("photo.jpg"), b"jpg").unwrap();
        std::fs::write(temp.path().join("clip.mp4"), b"mp4").unwrap();
        std::fs::write(temp.path().join("scan.png"), b"png").unwrap();
        std::fs::write(temp.path().join("poster.webp"), b"webp").unwrap();
        std::fs::write(temp.path().join("notes.txt"), b"txt").unwrap();
        let path = temp.path().to_path_buf();
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_focus_handle_for_test(path, focus_handle)
        });

        cx.read_entity(&view, |view, _| {
            assert_eq!(view.view_mode, crate::settings::FileViewMode::LargeIcons);
        });

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.apply_settings(
                    &ExplorerSettings {
                        view: crate::settings::ViewSettings {
                            mode_media: crate::settings::FileViewMode::Details,
                            ..crate::settings::ViewSettings::default()
                        },
                        ..ExplorerSettings::default()
                    },
                    cx,
                );
            });
        });

        cx.read_entity(&view, |view, _| {
            assert_eq!(view.view_mode, crate::settings::FileViewMode::Details);
        });
    }

    #[gpui::test]
    fn enabling_folder_sizes_calculates_and_disabling_clears_real_directories(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.set_global(FolderSizeCache::new());
        let temp = crate::explorer::test_support::TempDir::new();
        let folder = temp.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("file.txt"), b"abc").unwrap();
        let path = temp.path().to_path_buf();
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_focus_handle_for_test(path, focus_handle)
        });

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.apply_settings(
                    &ExplorerSettings {
                        view: crate::settings::ViewSettings {
                            show_folder_sizes: true,
                            ..crate::settings::ViewSettings::default()
                        },
                        ..ExplorerSettings::default()
                    },
                    cx,
                );
            });
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.all_entries[0].size, Some(3));
            assert_eq!(view.entries[0].size, Some(3));
        });

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.apply_settings(&ExplorerSettings::default(), cx);
            });
        });
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.all_entries[0].size, None);
            assert_eq!(view.entries[0].size, None);
        });
        assert_eq!(
            cx.read_global::<FolderSizeCache, _>(|cache, _| cache.get(&folder)),
            Some(3)
        );
    }

    #[gpui::test]
    fn folder_sizes_reuse_cache_until_explicit_refresh(cx: &mut gpui::TestAppContext) {
        cx.set_global(FolderSizeCache::new());
        let temp = crate::explorer::test_support::TempDir::new();
        let folder = temp.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("file.txt"), b"abc").unwrap();
        cx.read_global::<FolderSizeCache, _>(|cache, _| cache.insert(folder.clone(), 99));
        let path = temp.path().to_path_buf();
        let (view, cx) = cx.add_window_view(move |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            ExplorerView::new_with_focus_handle_for_test(path, focus_handle)
        });

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.apply_settings(
                    &ExplorerSettings {
                        view: crate::settings::ViewSettings {
                            show_folder_sizes: true,
                            ..crate::settings::ViewSettings::default()
                        },
                        ..ExplorerSettings::default()
                    },
                    cx,
                );
            });
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| assert_eq!(view.entries[0].size, Some(99)));

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.refresh_with_entry_metadata_resolution(cx);
            });
        });
        cx.run_until_parked();

        cx.read_entity(&view, |view, _| assert_eq!(view.entries[0].size, Some(3)));
        assert_eq!(
            cx.read_global::<FolderSizeCache, _>(|cache, _| cache.get(&folder)),
            Some(3)
        );
    }

    #[test]
    fn folder_size_updates_only_real_directories_and_skip_recursive_results() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        let folder = FileEntry::test("folder", true, None, None);
        let link = FileEntry::test_directory_link("link", DirectoryLinkKind::FilesystemLink);
        view.all_entries = vec![folder.clone(), link.clone()];
        view.entries = vec![folder, link];

        assert!(view.apply_folder_size(Path::new("folder"), 12));
        assert!(!view.apply_folder_size(Path::new("link"), 34));
        assert_eq!(view.all_entries[0].size, Some(12));
        assert_eq!(view.all_entries[1].size, None);

        view.search.recursive_results_active = true;
        view.entries = vec![FileEntry::test("folder", true, None, None)];
        assert!(view.apply_folder_size(Path::new("folder"), 56));
        assert_eq!(view.all_entries[0].size, Some(56));
        assert_eq!(view.entries[0].size, None);
    }

    #[test]
    fn cancelling_folder_sizes_invalidates_generation_and_signals_work() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        let cancel = Arc::new(AtomicBool::new(false));
        view.folder_size_generation = 7;
        view.folder_size_cancel = Some(cancel.clone());

        view.cancel_folder_size_task();

        assert_eq!(view.folder_size_generation, 8);
        assert!(cancel.load(Ordering::Relaxed));
        assert!(view.folder_size_cancel.is_none());
        assert!(view.folder_size_task.is_none());
    }

    #[test]
    fn sidebar_resize_drag_clamps_to_minimum() {
        assert_eq!(
            normalized_sidebar_width_f32((crate::settings::SIDEBAR_MIN_WIDTH - 1) as f32),
            crate::settings::SIDEBAR_MIN_WIDTH as f32
        );
        assert_eq!(sidebar_width_for_drag(225.0, 25.0), 250.0);
        assert_eq!(
            sidebar_width_for_drag(225.0, -200.0),
            crate::settings::SIDEBAR_MIN_WIDTH as f32
        );
    }

    #[test]
    fn reset_sidebar_width_restores_default_and_clears_drag() {
        let mut view = ExplorerView::new(PathBuf::from("reset-sidebar"));
        view.sidebar_width = 320.0;
        view.begin_sidebar_resize(320.0);

        assert_eq!(
            view.reset_sidebar_width(),
            crate::settings::SIDEBAR_DEFAULT_WIDTH
        );
        assert_eq!(
            view.sidebar_width,
            crate::settings::SIDEBAR_DEFAULT_WIDTH as f32
        );
        assert_eq!(view.sidebar_resize_drag, None);
    }

    #[test]
    fn reset_file_column_width_restores_default_and_clears_drag() {
        let mut view = ExplorerView::new(PathBuf::from("reset-file-column"));
        view.file_columns.widths.insert(FileColumnKind::Type, 320);
        view.begin_file_column_resize(FileColumnKind::Type, 320.0);

        assert_eq!(
            view.reset_file_column_width(FileColumnKind::Type),
            (
                FileColumnKind::Type,
                crate::settings::default_file_column_width(FileColumnKind::Type)
            )
        );
        assert_eq!(
            view.file_columns.widths[&FileColumnKind::Type],
            crate::settings::default_file_column_width(FileColumnKind::Type)
        );
        assert_eq!(view.file_column_resize_drag, None);
    }

    #[test]
    fn read_error_suppresses_empty_folder_message() {
        let mut view = ExplorerView::new(PathBuf::from("missing"));
        view.entries.clear();
        view.read_error = Some("missing".to_owned());

        assert!(!view.should_show_empty_folder_message());
    }

    #[test]
    fn reload_without_visible_rows_resets_horizontal_scroll() {
        let mut view = ExplorerView::new(PathBuf::from("missing"));
        view.set_horizontal_scroll_offset(80.0);

        view.reload();

        assert_eq!(view.visible_horizontal_scroll_offset(), 0.0);
    }

    #[test]
    fn non_empty_directory_suppresses_empty_folder_message() {
        let mut view = ExplorerView::new(PathBuf::from("non-empty"));
        view.all_entries = vec![FileEntry::test("file.txt", false, Some(1), None)];
        view.entries = view.all_entries.clone();
        view.read_error = None;

        assert!(!view.should_show_empty_folder_message());
    }

    #[test]
    fn content_branch_prioritizes_error_empty_then_list() {
        let mut view = ExplorerView::new(PathBuf::from("branch"));

        view.entries.clear();
        view.read_error = Some("error".to_owned());
        assert_eq!(view.content_branch(), ExplorerContentBranch::Error);

        view.read_error = None;
        assert_eq!(view.content_branch(), ExplorerContentBranch::Empty);

        view.all_entries = vec![FileEntry::test("file.txt", false, Some(1), None)];
        view.entries = view.all_entries.clone();
        assert_eq!(view.content_branch(), ExplorerContentBranch::List);

        view.set_search_query("missing".to_owned());
        assert_eq!(
            view.content_branch(),
            ExplorerContentBranch::NoSearchMatches
        );
    }

    #[test]
    fn shell_shortcut_resolution_updates_entries_and_preserves_selection() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.path = PathBuf::from("root");
        view.shell_shortcut_resolution_generation = 7;
        view.all_entries = vec![test_pending_shell_shortcut("shortcut.lnk", "target")];
        view.entries = view.all_entries.clone();
        view.select_single_path(Path::new("shortcut.lnk"));

        assert!(view.apply_shell_shortcut_resolutions(
            Path::new("root"),
            7,
            vec![ShellShortcutResolution {
                path: PathBuf::from("shortcut.lnk"),
                target_kind: ShellShortcutTargetKind::Directory,
            }],
        ));

        assert!(view.all_entries[0].is_directory_like());
        assert!(view.entries[0].is_directory_like());
        assert_eq!(view.entries[0].navigation_path(), Path::new("target"));
        assert_eq!(view.selected_paths(), vec![PathBuf::from("shortcut.lnk")]);
    }

    #[test]
    fn stale_shell_shortcut_resolution_is_ignored() {
        let mut view = ExplorerView::new(PathBuf::from("root"));
        view.path = PathBuf::from("root");
        view.shell_shortcut_resolution_generation = 2;
        view.all_entries = vec![test_pending_shell_shortcut("shortcut.lnk", "target")];
        view.entries = view.all_entries.clone();

        assert!(!view.apply_shell_shortcut_resolutions(
            Path::new("root"),
            1,
            vec![ShellShortcutResolution {
                path: PathBuf::from("shortcut.lnk"),
                target_kind: ShellShortcutTargetKind::Directory,
            }],
        ));
        assert!(!view.entries[0].is_directory_like());

        assert!(!view.apply_shell_shortcut_resolutions(
            Path::new("other"),
            2,
            vec![ShellShortcutResolution {
                path: PathBuf::from("shortcut.lnk"),
                target_kind: ShellShortcutTargetKind::Directory,
            }],
        ));
        assert!(!view.entries[0].is_directory_like());
    }
}
