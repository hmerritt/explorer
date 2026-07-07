use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use gpui::{
    Context, ExternalPathDragOperation, ExternalPathsDragResult, Image, ImageFormat, Window,
};

use crate::explorer::{
    clipboard::{
        FileClipboard, FileClipboardOperation, clipboard_item_for_files, file_clipboard_from_item,
        image_clipboard_from_item,
    },
    explorer_fs::ExplorerFs,
    filesystem::{
        ConflictChoice, FileOperationCopyUndo, FileOperationError, FileOperationJob,
        FileOperationKind, FileOperationMove, FileOperationReplacedFile, FileOperationSummary,
        PreparedFileOperation, archive_path_is_supported, cleanup_copy_undo_backups,
        execute_file_operation, execute_file_operation_with_progress,
        mountable_image_path_is_supported, prepare_copy_paths_to_directory_for_paste,
        prepare_extract_archives_to_directory, prepare_move_paths_to_directory,
        remove_existing_paths_permanently, remove_paths_permanently,
        restore_replaced_file_from_copy_undo, trash_paths,
    },
    view::{ExplorerView, FileOperationState, PendingPermanentDelete, PendingTrash},
};

#[cfg(test)]
use crate::explorer::filesystem::{
    FileConflictBatch, FileOperationOutcome, copy_paths_to_directory, create_links_to_directory,
    move_paths_to_directory, resolve_file_conflicts,
};

const FILE_OPERATION_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const FILE_OPERATION_UNDO_LIMIT: usize = 32;

#[derive(Clone, Debug)]
pub(super) enum FileOperationUndo {
    Copy { undo: FileOperationCopyUndo },
    Move { paths: Vec<FileOperationMove> },
    Trash(TrashUndo),
}

#[derive(Clone, Debug)]
pub(super) enum TrashUndo {
    Restorable {
        items: Vec<trash::TrashItem>,
        original_paths: Vec<PathBuf>,
    },
    Unsupported {
        original_paths: Vec<PathBuf>,
        reason: String,
    },
}

#[derive(Debug)]
enum UndoSelection {
    Clear,
    Paths(Vec<PathBuf>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NewItemKind {
    Folder,
    File,
}

impl NewItemKind {
    fn base_name(self) -> &'static str {
        match self {
            Self::Folder => "New folder",
            Self::File => "New file",
        }
    }

    fn operation_label(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::File => "file",
        }
    }
}

impl ExplorerView {
    pub(super) fn create_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.create_new_item(NewItemKind::Folder, window, cx);
    }

    pub(super) fn create_new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.create_new_item(NewItemKind::File, window, cx);
    }

    fn create_new_item(&mut self, kind: NewItemKind, window: &mut Window, cx: &mut Context<Self>) {
        match create_new_item_in_directory(&self.path, kind) {
            Ok(path) => {
                self.clear_operation_notice();
                self.reload_async_with_options_and_focused_rename(
                    crate::explorer::view::ReloadMode {
                        preserve_selection: true,
                        rebuild_sidebar: true,
                        preserve_context_menu: false,
                    },
                    vec![path.clone()],
                    path,
                    true,
                    false,
                    false,
                    window,
                    cx,
                );
                self.emit_filesystem_changed(cx);
            }
            Err(error) => {
                self.reload_with_entry_metadata_resolution(cx);
                self.set_error_notice(error);
            }
        }
    }

    pub(super) fn copy_selected_to_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(clipboard) = self.selected_file_clipboard(FileClipboardOperation::Copy) else {
            return;
        };

        match clipboard_item_for_files(&clipboard) {
            Ok(item) => {
                cx.write_to_clipboard(item);
                self.cut_paths.clear();
                self.clear_operation_notice();
            }
            Err(error) => self.set_error_notice(error),
        }
    }

    pub(super) fn cut_selected_to_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(clipboard) = self.selected_file_clipboard(FileClipboardOperation::Cut) else {
            return;
        };

        match clipboard_item_for_files(&clipboard) {
            Ok(item) => {
                cx.write_to_clipboard(item);
                self.mark_cut_paths(&clipboard.paths);
                self.clear_operation_notice();
            }
            Err(error) => self.set_error_notice(error),
        }
    }

    pub(super) fn paste_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };

        if let Some(clipboard) = file_clipboard_from_item(&item) {
            self.paste_file_clipboard(clipboard, cx);
            return;
        }

        if let Some(image) = image_clipboard_from_item(&item) {
            self.paste_clipboard_image(image, window, cx);
        }
    }

    fn paste_file_clipboard(&mut self, clipboard: FileClipboard, cx: &mut Context<Self>) {
        match clipboard.operation {
            FileClipboardOperation::Copy => {
                self.handle_prepared_file_command_result_and_open_dialog(
                    prepare_copy_paths_to_directory_for_paste(&clipboard.paths, &self.path),
                    cx,
                );
            }
            FileClipboardOperation::Cut => {
                let result = prepare_move_paths_to_directory(&clipboard.paths, &self.path);
                self.handle_prepared_file_command_result_and_open_dialog(result, cx);
            }
        }
    }

    fn paste_clipboard_image(
        &mut self,
        image: &Image,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match create_clipboard_image_file_in_directory(&self.path, image) {
            Ok(path) => {
                self.clear_operation_notice();
                self.reload_async_with_options_and_focused_rename(
                    crate::explorer::view::ReloadMode {
                        preserve_selection: true,
                        rebuild_sidebar: true,
                        preserve_context_menu: false,
                    },
                    vec![path.clone()],
                    path,
                    true,
                    false,
                    false,
                    window,
                    cx,
                );
                self.emit_filesystem_changed(cx);
            }
            Err(error) => {
                self.reload_with_entry_metadata_resolution(cx);
                self.set_error_notice(error);
            }
        }
    }

    pub(super) fn extract_selected_archives(&mut self, cx: &mut Context<Self>) {
        let Some(paths) = self.selected_archive_paths() else {
            return;
        };

        self.handle_prepared_file_command_result_and_open_dialog(
            prepare_extract_archives_to_directory(&paths, &self.path),
            cx,
        );
    }

    pub(super) fn trash_selected_paths(&mut self, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }

        let trash_undo = TrashUndoCapture::before_delete(&paths);
        match trash_paths(&paths) {
            Ok(()) => {
                self.push_file_operation_undo(trash_undo.after_delete());
                self.remove_cut_paths(&paths);
                self.reload_with_entry_metadata_resolution(cx);
                self.clear_selection();
                self.clear_operation_notice();
                self.emit_filesystem_changed(cx);
            }
            Err(error) => {
                self.set_error_notice(error);
                self.reload_with_entry_metadata_resolution(cx);
            }
        }
    }

    pub(super) fn request_trash_paths_with_confirmation(
        &mut self,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }

        self.pending_trash = Some(PendingTrash { paths });
        self.clear_operation_notice();
        self.open_pending_dialog_window(cx);
    }

    pub(super) fn confirm_pending_trash(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_trash.take() else {
            return;
        };

        let trash_undo = TrashUndoCapture::before_delete(&pending.paths);
        match trash_paths(&pending.paths) {
            Ok(()) => {
                self.push_file_operation_undo(trash_undo.after_delete());
                self.remove_cut_paths(&pending.paths);
                self.reload_with_entry_metadata_resolution(cx);
                self.clear_selection();
                self.clear_operation_notice();
                self.emit_filesystem_changed(cx);
            }
            Err(error) => {
                self.set_error_notice(error);
                self.reload_with_entry_metadata_resolution(cx);
            }
        }
    }

    pub(super) fn cancel_pending_trash(&mut self) {
        self.pending_trash = None;
    }

    pub(super) fn request_permanent_delete_selected(&mut self, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }

        self.pending_permanent_delete = Some(PendingPermanentDelete { paths });
        self.clear_operation_notice();
        self.open_pending_dialog_window(cx);
    }

    pub(super) fn confirm_pending_permanent_delete(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_permanent_delete.take() else {
            return;
        };

        match remove_paths_permanently(&pending.paths) {
            Ok(()) => {
                self.remove_cut_paths(&pending.paths);
                self.reload_with_entry_metadata_resolution(cx);
                self.clear_selection();
                self.clear_operation_notice();
                self.emit_filesystem_changed(cx);
            }
            Err(error) => {
                self.set_error_notice(error);
                self.reload_with_entry_metadata_resolution(cx);
            }
        }
    }

    pub(super) fn cancel_pending_permanent_delete(&mut self) {
        self.pending_permanent_delete = None;
    }

    pub(super) fn complete_external_paths_drag(
        &mut self,
        source_paths: &[PathBuf],
        result: ExternalPathsDragResult,
        cx: &mut Context<Self>,
    ) {
        let ExternalPathsDragResult::Completed {
            operation,
            cleanup_source,
        } = result
        else {
            return;
        };

        if cleanup_source {
            match remove_existing_paths_permanently(source_paths) {
                Ok(removed_any) => {
                    self.remove_cut_paths(source_paths);
                    self.reload_with_entry_metadata_resolution(cx);
                    self.clear_selection();
                    self.clear_operation_notice();
                    if removed_any || operation == ExternalPathDragOperation::Move {
                        self.emit_filesystem_changed(cx);
                    }
                }
                Err(error) => {
                    self.set_error_notice(error);
                    self.reload_with_entry_metadata_resolution(cx);
                }
            }
        } else {
            self.refresh_with_entry_metadata_resolution(cx);
            self.clear_operation_notice();
            if operation == ExternalPathDragOperation::Move {
                self.emit_filesystem_changed(cx);
            }
        }
    }

    pub(super) fn selected_file_clipboard(
        &self,
        operation: FileClipboardOperation,
    ) -> Option<FileClipboard> {
        let paths = self.selected_paths();
        (!paths.is_empty()).then(|| FileClipboard::new(operation, paths))
    }

    pub(super) fn selected_archive_paths(&self) -> Option<Vec<PathBuf>> {
        let paths = self.selected_paths();
        if paths.is_empty()
            || paths
                .iter()
                .any(|path| !path.is_file() || !archive_path_is_supported(path))
        {
            return None;
        }

        Some(paths)
    }

    pub(super) fn selected_mountable_image_path(&self) -> Option<PathBuf> {
        let paths = self.selected_paths();
        let [path] = paths.as_slice() else {
            return None;
        };
        if !path.is_file() || !mountable_image_path_is_supported(path) {
            return None;
        }

        Some(path.clone())
    }

    pub(super) fn mark_cut_paths(&mut self, paths: &[PathBuf]) {
        self.cut_paths = paths.iter().cloned().collect();
    }

    #[cfg(test)]
    pub(super) fn clear_cut_paths(&mut self) {
        self.cut_paths.clear();
    }

    pub(super) fn remove_cut_paths(&mut self, paths: &[PathBuf]) {
        let paths = paths.iter().collect::<BTreeSet<_>>();
        self.cut_paths.retain(|path| !paths.contains(path));
    }

    pub(super) fn entry_is_cut(&self, path: &Path) -> bool {
        self.cut_paths.contains(path)
    }

    #[cfg(test)]
    pub(super) fn handle_file_command_result(
        &mut self,
        result: Result<FileOperationOutcome, String>,
    ) {
        match result {
            Ok(FileOperationOutcome::Finished(summary)) => {
                self.finish_file_operation_for_test(summary);
            }
            Ok(FileOperationOutcome::Conflicts(conflicts)) => {
                self.pending_file_conflict = Some(conflicts);
                self.clear_operation_notice();
            }
            Err(error) => {
                self.set_error_notice(error);
                self.reload();
            }
        }
    }

    pub(super) fn handle_prepared_file_command_result_and_open_dialog(
        &mut self,
        result: Result<PreparedFileOperation, String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(PreparedFileOperation::Ready(job)) => {
                self.start_file_operation(job, ConflictChoice::Replace, cx);
            }
            Ok(PreparedFileOperation::Conflicts(conflicts)) => {
                if let Some(diagnostics) = conflicts.archive_diagnostics() {
                    diagnostics.mark_conflict_wait_started();
                }
                self.pending_file_conflict = Some(conflicts);
                self.clear_operation_notice();
                self.open_pending_dialog_window(cx);
            }
            Err(error) => {
                self.set_error_notice(error);
                self.reload_with_entry_metadata_resolution(cx);
            }
        }
    }

    pub(super) fn resolve_pending_file_conflicts_and_open_progress(
        &mut self,
        choice: ConflictChoice,
        cx: &mut Context<Self>,
    ) {
        let Some(conflicts) = self.pending_file_conflict.take() else {
            return;
        };
        if let Some(diagnostics) = conflicts.archive_diagnostics() {
            diagnostics.mark_conflict_wait_finished();
        }
        self.start_file_operation(conflicts.into_job(), choice, cx);
    }

    pub(super) fn cancel_active_file_operation(&mut self) {
        if let Some(operation) = self.active_file_operation.as_ref() {
            if let Some(diagnostics) = &operation.archive_diagnostics {
                diagnostics.mark_cancel_requested();
            }
            operation.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub(super) fn terminate_active_file_operation(&mut self) {
        if let Some(operation) = self.active_file_operation.as_ref() {
            operation.terminate.store(true, Ordering::Relaxed);
        }
        self.cancel_active_file_operation();
    }

    fn start_file_operation(
        &mut self,
        job: FileOperationJob,
        conflict_choice: ConflictChoice,
        cx: &mut Context<Self>,
    ) {
        if self.active_file_operation.is_some() {
            self.set_error_notice("Another file operation is already running.".to_owned());
            return;
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let terminate = Arc::new(AtomicBool::new(false));
        let progress = job.initial_progress();
        let archive_diagnostics = job.archive_diagnostics();
        self.active_file_operation = Some(FileOperationState {
            progress: progress.clone(),
            cancel: cancel.clone(),
            terminate: terminate.clone(),
            task: None,
            archive_diagnostics: archive_diagnostics.clone(),
        });
        self.clear_operation_notice();
        self.open_file_operation_window(cx);
        if let Some(diagnostics) = &archive_diagnostics {
            diagnostics.mark_progress_dialog_visible();
        }

        let (progress_tx, progress_rx) = mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let task = cx.spawn({
            let cancel = cancel.clone();
            let terminate = terminate.clone();
            let finished = finished.clone();
            async move |this, cx| {
                let operation_task = cx.background_executor().spawn({
                    let progress_tx = progress_tx.clone();
                    let finished = finished.clone();
                    async move {
                        let result = execute_file_operation_with_progress(
                            job,
                            conflict_choice,
                            cancel,
                            terminate,
                            |progress| {
                                let _ = progress_tx.send(progress);
                            },
                        );
                        finished.store(true, Ordering::Relaxed);
                        result
                    }
                });

                while !finished.load(Ordering::Relaxed) {
                    cx.background_executor()
                        .timer(FILE_OPERATION_PROGRESS_INTERVAL)
                        .await;
                    Self::drain_file_operation_progress(&this, cx, &progress_rx);
                }

                let result = operation_task.await;
                Self::drain_file_operation_progress(&this, cx, &progress_rx);

                let _ = this.update(cx, |explorer, cx| {
                    explorer.complete_active_file_operation(result, cx);
                    cx.notify();
                });
            }
        });

        if let Some(operation) = self.active_file_operation.as_mut() {
            operation.task = Some(task);
        }
    }

    fn drain_file_operation_progress(
        this: &gpui::WeakEntity<Self>,
        cx: &mut gpui::AsyncApp,
        progress_rx: &mpsc::Receiver<crate::explorer::filesystem::FileOperationProgress>,
    ) {
        let mut latest = None;
        while let Ok(progress) = progress_rx.try_recv() {
            latest = Some(progress);
        }

        if let Some(progress) = latest {
            let _ = this.update(cx, |explorer, cx| {
                if let Some(operation) = explorer.active_file_operation.as_mut() {
                    operation.progress = progress;
                    cx.notify();
                }
            });
        }
    }

    fn complete_active_file_operation(
        &mut self,
        result: Result<FileOperationSummary, FileOperationError>,
        cx: &mut Context<Self>,
    ) {
        if let Some(handle) = self.active_dialog_window.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
        self.active_file_operation = None;

        match result {
            Ok(summary) => {
                let diagnostics = summary.archive_diagnostics.clone();
                self.finish_file_operation(summary, cx);
                if let Some(diagnostics) = diagnostics {
                    diagnostics.add_metadata_resolution(Duration::ZERO);
                    diagnostics.finish("ok");
                }
                self.emit_filesystem_changed(cx);
            }
            Err(FileOperationError::Cancelled) => {
                self.clear_operation_notice();
                self.reload_with_entry_metadata_resolution(cx);
            }
            Err(FileOperationError::Failed(error)) => {
                self.set_error_notice(error);
                self.reload_with_entry_metadata_resolution(cx);
            }
        }
    }

    fn finish_file_operation(&mut self, summary: FileOperationSummary, cx: &mut Context<Self>) {
        let reload_started = Instant::now();
        let destination_paths = summary.destination_paths.clone();
        self.clear_operation_notice();
        self.record_file_operation_undo(&summary);
        self.remove_cut_paths(&summary.moved_source_paths);
        self.reload_async_with_options(
            crate::explorer::view::ReloadMode {
                preserve_selection: true,
                rebuild_sidebar: true,
                preserve_context_menu: false,
            },
            destination_paths,
            true,
            false,
            false,
            cx,
        );
        if let Some(diagnostics) = summary.archive_diagnostics {
            diagnostics.add_reload(reload_started.elapsed());
        }
    }

    #[cfg(test)]
    fn finish_file_operation_for_test(&mut self, summary: FileOperationSummary) {
        self.clear_operation_notice();
        self.record_file_operation_undo(&summary);
        self.remove_cut_paths(&summary.moved_source_paths);
        self.reload();
        self.restore_selection_from_paths(&summary.destination_paths);
    }

    fn record_file_operation_undo(&mut self, summary: &FileOperationSummary) {
        match summary.kind {
            FileOperationKind::Copy | FileOperationKind::Link => {
                if !summary.copy_undo.is_empty() {
                    self.push_file_operation_undo(Some(FileOperationUndo::Copy {
                        undo: summary.copy_undo.clone(),
                    }));
                }
            }
            FileOperationKind::Move => {
                if !summary.moved_paths.is_empty() {
                    self.push_file_operation_undo(Some(FileOperationUndo::Move {
                        paths: summary.moved_paths.clone(),
                    }));
                }
            }
            FileOperationKind::Extract => {}
        }
    }

    fn push_file_operation_undo(&mut self, undo: Option<FileOperationUndo>) {
        let Some(undo) = undo else {
            return;
        };

        if self.file_operation_undo_stack.len() == FILE_OPERATION_UNDO_LIMIT {
            let expired = self.file_operation_undo_stack.remove(0);
            cleanup_file_operation_undo(expired);
        }
        self.file_operation_undo_stack.push(undo);
    }

    pub(super) fn undo_file_operation(&mut self, cx: &mut Context<Self>) {
        let Some(undo) = self.file_operation_undo_stack.last().cloned() else {
            return;
        };

        match self.apply_file_operation_undo(undo) {
            Ok(selection) => {
                if let Some(applied) = self.file_operation_undo_stack.pop() {
                    cleanup_file_operation_undo(applied);
                }
                self.clear_operation_notice();
                self.reload_with_entry_metadata_resolution(cx);
                match selection {
                    UndoSelection::Clear => self.clear_selection(),
                    UndoSelection::Paths(paths) => self.restore_selection_from_paths(&paths),
                }
                self.emit_filesystem_changed(cx);
            }
            Err(error) => {
                self.reload_with_entry_metadata_resolution(cx);
                self.set_error_notice(error);
            }
        }
    }

    fn apply_file_operation_undo(
        &mut self,
        undo: FileOperationUndo,
    ) -> Result<UndoSelection, String> {
        match undo {
            FileOperationUndo::Copy { undo } => {
                undo_copied_paths(&undo)?;
                Ok(UndoSelection::Clear)
            }
            FileOperationUndo::Move { paths } => {
                let restored_paths = undo_moved_paths(&paths)?;
                self.remove_cut_paths(&restored_paths);
                Ok(UndoSelection::Paths(restored_paths))
            }
            FileOperationUndo::Trash(trash) => undo_trash_paths(trash).map(UndoSelection::Paths),
        }
    }
}

impl Drop for ExplorerView {
    fn drop(&mut self) {
        for undo in self.file_operation_undo_stack.drain(..) {
            cleanup_file_operation_undo(undo);
        }
    }
}

fn cleanup_file_operation_undo(undo: FileOperationUndo) {
    if let FileOperationUndo::Copy { undo } = undo {
        cleanup_copy_undo_backups(&undo);
    }
}

fn undo_copied_paths(undo: &FileOperationCopyUndo) -> Result<(), String> {
    preflight_copy_undo(undo)?;

    for path in undo.created_files.iter().rev() {
        remove_created_file_for_undo(path)?;
    }
    for replaced in &undo.replaced_files {
        restore_replaced_file_from_copy_undo(replaced)?;
    }
    for path in undo.created_directories.iter().rev() {
        remove_created_directory_for_undo(path)?;
    }
    cleanup_copy_undo_backups(undo);
    Ok(())
}

fn preflight_copy_undo(undo: &FileOperationCopyUndo) -> Result<(), String> {
    for path in &undo.created_files {
        preflight_created_file_for_undo(path)?;
    }
    for replaced in &undo.replaced_files {
        preflight_replaced_file_for_undo(replaced)?;
    }
    Ok(())
}

fn preflight_created_file_for_undo(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Err(format!(
            "Could not undo copy of {} because it is now a folder.",
            path.display()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Could not undo copy of {}: {error}",
            path.display()
        )),
    }
}

fn preflight_replaced_file_for_undo(replaced: &FileOperationReplacedFile) -> Result<(), String> {
    match fs::metadata(&replaced.backup) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            return Err(format!(
                "Could not undo copy of {} because its undo backup is not a file.",
                replaced.destination.display()
            ));
        }
        Err(error) => {
            return Err(format!(
                "Could not undo copy of {} because its undo backup is unavailable: {error}",
                replaced.destination.display()
            ));
        }
    }

    match fs::symlink_metadata(&replaced.destination) {
        Ok(metadata) if metadata.is_dir() => Err(format!(
            "Could not undo copy of {} because it is now a folder.",
            replaced.destination.display()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Could not undo copy of {}: {error}",
            replaced.destination.display()
        )),
    }
}

fn remove_created_file_for_undo(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Could not undo copy of {}: {error}",
            path.display()
        )),
    }
}

fn remove_created_directory_for_undo(path: &Path) -> Result<(), String> {
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound
                    | io::ErrorKind::DirectoryNotEmpty
                    | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(format!(
            "Could not undo copy of {}: {error}",
            path.display()
        )),
    }
}

fn undo_moved_paths(paths: &[FileOperationMove]) -> Result<Vec<PathBuf>, String> {
    preflight_move_undo(paths)?;

    let mut by_parent = BTreeMap::<PathBuf, Vec<PathBuf>>::new();
    for path in paths {
        let parent = path
            .source
            .parent()
            .ok_or_else(|| format!("Could not undo move of {}.", path.source.display()))?;
        by_parent
            .entry(parent.to_path_buf())
            .or_default()
            .push(path.destination.clone());
    }

    for (parent, destinations) in by_parent {
        match prepare_move_paths_to_directory(&destinations, &parent)? {
            PreparedFileOperation::Ready(job) => {
                execute_file_operation(job, ConflictChoice::Replace)?;
            }
            PreparedFileOperation::Conflicts(_) => {
                return Err(
                    "Could not undo move because an original location is no longer available."
                        .to_owned(),
                );
            }
        }
    }

    Ok(paths.iter().map(|path| path.source.clone()).collect())
}

fn preflight_move_undo(paths: &[FileOperationMove]) -> Result<(), String> {
    for path in paths {
        if !path.destination.exists() {
            return Err(format!(
                "Could not undo move because {} no longer exists.",
                path.destination.display()
            ));
        }
        if path.source.exists() {
            return Err(format!(
                "Could not undo move because {} already exists.",
                path.source.display()
            ));
        }
        if let Some(parent) = path.source.parent()
            && !parent.is_dir()
        {
            return Err(format!(
                "Could not undo move because {} is no longer available.",
                parent.display()
            ));
        }
    }
    Ok(())
}

fn undo_trash_paths(trash: TrashUndo) -> Result<Vec<PathBuf>, String> {
    match trash {
        TrashUndo::Restorable {
            items,
            original_paths,
        } => {
            restore_trash_items(items)?;
            Ok(original_paths)
        }
        TrashUndo::Unsupported {
            original_paths,
            reason,
        } => {
            let _ = original_paths;
            Err(reason)
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn restore_trash_items(items: Vec<trash::TrashItem>) -> Result<(), String> {
    trash::os_limited::restore_all(items)
        .map_err(|error| format!("Could not restore deleted items from the Recycle Bin: {error}"))
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn restore_trash_items(items: Vec<trash::TrashItem>) -> Result<(), String> {
    let _ = items;
    Err("Undo for Trash delete is not supported on this platform yet.".to_owned())
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
struct TrashUndoCapture {
    original_paths: Vec<PathBuf>,
    original_keys: BTreeSet<String>,
    before_ids: Result<BTreeSet<std::ffi::OsString>, String>,
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
impl TrashUndoCapture {
    fn before_delete(paths: &[PathBuf]) -> Self {
        let original_paths = paths.to_vec();
        let original_keys = trash_undo_path_keys(paths);
        let before_ids = trash::os_limited::list()
            .map(|items| items.into_iter().map(|item| item.id).collect())
            .map_err(|error| format!("Could not inspect the Recycle Bin for undo: {error}"));
        Self {
            original_paths,
            original_keys,
            before_ids,
        }
    }

    fn after_delete(self) -> Option<FileOperationUndo> {
        let trash = match self.before_ids {
            Ok(before_ids) => match trash::os_limited::list() {
                Ok(items) => restorable_trash_undo_from_items(
                    self.original_paths,
                    self.original_keys,
                    before_ids,
                    items,
                ),
                Err(error) => TrashUndo::Unsupported {
                    original_paths: self.original_paths,
                    reason: format!("Could not inspect the Recycle Bin for undo: {error}"),
                },
            },
            Err(reason) => TrashUndo::Unsupported {
                original_paths: self.original_paths,
                reason,
            },
        };
        Some(FileOperationUndo::Trash(trash))
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn restorable_trash_undo_from_items(
    original_paths: Vec<PathBuf>,
    original_keys: BTreeSet<String>,
    before_ids: BTreeSet<std::ffi::OsString>,
    items: Vec<trash::TrashItem>,
) -> TrashUndo {
    let items = items
        .into_iter()
        .filter(|item| {
            original_keys.contains(&trash_undo_path_key(&item.original_path()))
                && !before_ids.contains(&item.id)
        })
        .collect::<Vec<_>>();

    if items.is_empty() {
        TrashUndo::Unsupported {
            original_paths,
            reason: "Could not find deleted items in the Recycle Bin for undo.".to_owned(),
        }
    } else {
        TrashUndo::Restorable {
            items,
            original_paths,
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
struct TrashUndoCapture {
    original_paths: Vec<PathBuf>,
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
impl TrashUndoCapture {
    fn before_delete(paths: &[PathBuf]) -> Self {
        Self {
            original_paths: paths.to_vec(),
        }
    }

    fn after_delete(self) -> Option<FileOperationUndo> {
        Some(FileOperationUndo::Trash(TrashUndo::Unsupported {
            original_paths: self.original_paths,
            reason: "Undo for Trash delete is not supported on this platform yet.".to_owned(),
        }))
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn trash_undo_path_keys(paths: &[PathBuf]) -> BTreeSet<String> {
    paths
        .iter()
        .map(|path| {
            fs::canonicalize(path)
                .unwrap_or_else(|_| path.clone())
                .as_path()
                .to_owned()
        })
        .map(|path| trash_undo_path_key(&path))
        .collect()
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn trash_undo_path_key(path: &Path) -> String {
    if cfg!(target_os = "windows") {
        let key = path.to_string_lossy().replace('/', "\\");
        let key = key.strip_prefix(r"\\?\").unwrap_or(&key);
        key.trim_end_matches('\\').to_ascii_lowercase()
    } else {
        path.to_string_lossy().into_owned()
    }
}

fn create_new_item_in_directory(parent: &Path, kind: NewItemKind) -> Result<PathBuf, String> {
    let cancel = AtomicBool::new(false);
    create_new_item_in_directory_with_cancel(parent, kind, &cancel)
}

fn create_new_item_in_directory_with_cancel(
    parent: &Path,
    kind: NewItemKind,
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    let explorer_fs = ExplorerFs::new();

    let mut index = 1usize;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_owned());
        }

        let name = new_item_name(kind.base_name(), index);
        let path = parent.join(&name);

        if explorer_fs.exists(&path)? {
            index = next_new_item_index(index, &name)?;
            continue;
        }

        let result = match kind {
            NewItemKind::Folder => create_folder_path_with_cancel(&path, cancel, &explorer_fs),
            NewItemKind::File => write_file_path_with_cancel(&path, &[], cancel, &explorer_fs),
        };
        match result {
            Ok(()) => return Ok(path),
            Err(error) if error.to_ascii_lowercase().contains("already exist") => {
                index = next_new_item_index(index, &name)?;
            }
            Err(error) => {
                return Err(format!(
                    "Could not create {} \"{}\": {error}",
                    kind.operation_label(),
                    name
                ));
            }
        }
    }
}

fn create_clipboard_image_file_in_directory(
    parent: &Path,
    image: &Image,
) -> Result<PathBuf, String> {
    let (extension, bytes) = clipboard_image_file_payload(image)?;
    create_clipboard_image_file_payload_in_directory(parent, extension, bytes.as_ref())
}

fn create_clipboard_image_file_payload_in_directory(
    parent: &Path,
    extension: &'static str,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    let cancel = AtomicBool::new(false);
    create_clipboard_image_file_payload_in_directory_with_cancel(parent, extension, bytes, &cancel)
}

fn create_clipboard_image_file_payload_in_directory_with_cancel(
    parent: &Path,
    extension: &'static str,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    let explorer_fs = ExplorerFs::new();
    let mut index = 1usize;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_owned());
        }

        let name = clipboard_image_file_name(extension, index);
        let path = parent.join(&name);

        if explorer_fs.exists(&path)? {
            index = next_new_item_index(index, &name)?;
            continue;
        }

        match write_file_path_with_cancel(&path, bytes, cancel, &explorer_fs) {
            Ok(()) => return Ok(path),
            Err(error) if error.to_ascii_lowercase().contains("already exist") => {
                index = next_new_item_index(index, &name)?;
            }
            Err(error) => {
                return Err(format!("Could not create image \"{name}\": {error}"));
            }
        }
    }
}

fn create_folder_path_with_cancel(
    path: &Path,
    cancel: &AtomicBool,
    explorer_fs: &ExplorerFs,
) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".to_owned());
    }
    explorer_fs.create_dir(path)
}

fn write_file_path_with_cancel(
    path: &Path,
    bytes: &[u8],
    cancel: &AtomicBool,
    explorer_fs: &ExplorerFs,
) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".to_owned());
    }
    if bytes.is_empty() {
        explorer_fs.create_empty_file(path)
    } else {
        explorer_fs.write_file(path, bytes)
    }
}

fn clipboard_image_file_payload(image: &Image) -> Result<(&'static str, Cow<'_, [u8]>), String> {
    if image.format() == ImageFormat::Tiff {
        return clipboard_tiff_image_png_bytes(image.bytes())
            .map(|bytes| ("png", Cow::Owned(bytes)));
    }

    Ok((
        image_format_extension(image.format()),
        Cow::Borrowed(image.bytes()),
    ))
}

fn clipboard_tiff_image_png_bytes(bytes: &[u8]) -> Result<Vec<u8>, String> {
    #[cfg(target_os = "macos")]
    if let Some(png) = macos_tiff_image_png_bytes(bytes) {
        return Ok(png);
    }

    rust_tiff_image_png_bytes(bytes)
}

#[cfg(target_os = "macos")]
fn macos_tiff_image_png_bytes(bytes: &[u8]) -> Option<Vec<u8>> {
    use cocoa::{
        base::{id, nil},
        foundation::NSData,
    };
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::c_void;

    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

    if bytes.is_empty() {
        return None;
    }

    unsafe {
        let pool: id = msg_send![class!(NSAutoreleasePool), new];
        let result = (|| {
            let data = NSData::dataWithBytes_length_(
                nil,
                bytes.as_ptr() as *const c_void,
                bytes.len() as u64,
            );
            if data == nil {
                return None;
            }

            let bitmap_rep: id = msg_send![class!(NSBitmapImageRep), alloc];
            let bitmap_rep: id = msg_send![bitmap_rep, initWithData: data];
            if bitmap_rep == nil {
                return None;
            }
            let _: id = msg_send![bitmap_rep, autorelease];

            let png_file_type = 4usize;
            let png_data: id = msg_send![
                bitmap_rep,
                representationUsingType: png_file_type
                properties: nil
            ];
            if png_data == nil {
                return None;
            }

            let length = png_data.length();
            if length == 0 || png_data.bytes().is_null() {
                return None;
            }

            let png =
                std::slice::from_raw_parts(png_data.bytes().cast::<u8>(), length as usize).to_vec();
            png.starts_with(PNG_SIGNATURE).then_some(png)
        })();
        let _: () = msg_send![pool, drain];
        result
    }
}

fn rust_tiff_image_png_bytes(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Tiff)
        .map_err(|error| format!("Could not convert clipboard image to PNG: {error}"))?;
    let mut png = Vec::new();
    decoded
        .write_with_encoder(image::codecs::png::PngEncoder::new_with_quality(
            &mut png,
            image::codecs::png::CompressionType::Fast,
            image::codecs::png::FilterType::NoFilter,
        ))
        .map_err(|error| format!("Could not convert clipboard image to PNG: {error}"))?;
    Ok(png)
}

fn image_format_extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
    }
}

fn clipboard_image_file_name(extension: &str, index: usize) -> String {
    if index == 1 {
        format!("image.{extension}")
    } else {
        format!("image ({index}).{extension}")
    }
}

fn next_new_item_index(index: usize, name: &str) -> Result<usize, String> {
    index
        .checked_add(1)
        .ok_or_else(|| format!("Could not create {name}: too many existing names"))
}

fn new_item_name(base_name: &str, index: usize) -> String {
    if index == 1 {
        base_name.to_owned()
    } else {
        format!("{base_name} ({index})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::{
        clipboard::FileClipboardOperation,
        selection::SelectionModifiers,
        test_support::{TempDir, selected_names, test_view_entity_at_path, test_view_with_entries},
    };
    use gpui::{AppContext, Image, ImageFormat, TestAppContext};
    use std::{fs, io::Cursor};

    fn file_conflicts(result: Result<FileOperationOutcome, String>) -> FileConflictBatch {
        match result.expect("file operation") {
            FileOperationOutcome::Conflicts(conflicts) => conflicts,
            FileOperationOutcome::Finished(_) => panic!("expected file conflicts"),
        }
    }

    #[test]
    fn new_folder_uses_base_name_in_empty_directory() {
        let temp = TempDir::new();

        let path = create_new_item_in_directory(temp.path(), NewItemKind::Folder).unwrap();

        assert_eq!(path.file_name().unwrap(), "New folder");
        assert!(path.is_dir());
    }

    #[test]
    fn new_file_uses_base_name_in_empty_directory() {
        let temp = TempDir::new();

        let path = create_new_item_in_directory(temp.path(), NewItemKind::File).unwrap();

        assert_eq!(path.file_name().unwrap(), "New file");
        assert!(path.is_file());
        assert_eq!(fs::read(&path).unwrap(), b"");
    }

    #[test]
    fn new_folder_first_duplicate_uses_two() {
        let temp = TempDir::new();
        fs::create_dir(temp.path().join("New folder")).expect("create base folder");

        let path = create_new_item_in_directory(temp.path(), NewItemKind::Folder).unwrap();

        assert_eq!(path.file_name().unwrap(), "New folder (2)");
        assert!(path.is_dir());
    }

    #[test]
    fn new_file_first_duplicate_uses_two() {
        let temp = TempDir::new();
        fs::write(temp.path().join("New file"), b"existing").expect("create base file");

        let path = create_new_item_in_directory(temp.path(), NewItemKind::File).unwrap();

        assert_eq!(path.file_name().unwrap(), "New file (2)");
        assert!(path.is_file());
    }

    #[test]
    fn new_folder_existing_base_and_two_uses_three() {
        let temp = TempDir::new();
        fs::create_dir(temp.path().join("New folder")).expect("create base folder");
        fs::create_dir(temp.path().join("New folder (2)")).expect("create second folder");

        let path = create_new_item_in_directory(temp.path(), NewItemKind::Folder).unwrap();

        assert_eq!(path.file_name().unwrap(), "New folder (3)");
        assert!(path.is_dir());
    }

    #[test]
    fn new_file_existing_base_and_two_uses_three() {
        let temp = TempDir::new();
        fs::write(temp.path().join("New file"), b"base").expect("create base file");
        fs::write(temp.path().join("New file (2)"), b"second").expect("create second file");

        let path = create_new_item_in_directory(temp.path(), NewItemKind::File).unwrap();

        assert_eq!(path.file_name().unwrap(), "New file (3)");
        assert!(path.is_file());
    }

    #[test]
    fn new_folder_uses_first_free_suffix() {
        let temp = TempDir::new();
        fs::create_dir(temp.path().join("New folder")).expect("create base folder");
        fs::create_dir(temp.path().join("New folder (3)")).expect("create third folder");

        let path = create_new_item_in_directory(temp.path(), NewItemKind::Folder).unwrap();

        assert_eq!(path.file_name().unwrap(), "New folder (2)");
        assert!(path.is_dir());
    }

    #[test]
    fn new_file_uses_first_free_suffix() {
        let temp = TempDir::new();
        fs::write(temp.path().join("New file"), b"base").expect("create base file");
        fs::write(temp.path().join("New file (3)"), b"third").expect("create third file");

        let path = create_new_item_in_directory(temp.path(), NewItemKind::File).unwrap();

        assert_eq!(path.file_name().unwrap(), "New file (2)");
        assert!(path.is_file());
    }

    #[test]
    fn image_format_extensions_match_clipboard_formats() {
        assert_eq!(image_format_extension(ImageFormat::Png), "png");
        assert_eq!(image_format_extension(ImageFormat::Jpeg), "jpg");
        assert_eq!(image_format_extension(ImageFormat::Webp), "webp");
        assert_eq!(image_format_extension(ImageFormat::Gif), "gif");
        assert_eq!(image_format_extension(ImageFormat::Svg), "svg");
        assert_eq!(image_format_extension(ImageFormat::Bmp), "bmp");
        assert_eq!(image_format_extension(ImageFormat::Tiff), "tiff");
    }

    #[test]
    fn clipboard_image_file_name_uses_windows_style_suffixes() {
        assert_eq!(clipboard_image_file_name("png", 1), "image.png");
        assert_eq!(clipboard_image_file_name("png", 2), "image (2).png");
    }

    #[test]
    fn clipboard_image_file_uses_base_name_in_empty_directory() {
        let temp = TempDir::new();
        let image = Image::from_bytes(ImageFormat::Png, vec![1, 2, 3]);

        let path = create_clipboard_image_file_in_directory(temp.path(), &image).unwrap();

        assert_eq!(path.file_name().unwrap(), "image.png");
        assert_eq!(fs::read(path).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn clipboard_tiff_image_file_saves_png_in_empty_directory() {
        let temp = TempDir::new();
        let image = Image::from_bytes(ImageFormat::Tiff, test_tiff_bytes());

        let path = create_clipboard_image_file_in_directory(temp.path(), &image).unwrap();

        assert_eq!(path.file_name().unwrap(), "image.png");
        assert_saved_png_image(&fs::read(path).unwrap());
    }

    #[test]
    fn clipboard_image_file_uses_first_free_suffix() {
        let temp = TempDir::new();
        fs::write(temp.path().join("image.png"), b"base").expect("create base image");
        fs::write(temp.path().join("image (3).png"), b"third").expect("create third image");
        let image = Image::from_bytes(ImageFormat::Png, vec![4, 5, 6]);

        let path = create_clipboard_image_file_in_directory(temp.path(), &image).unwrap();

        assert_eq!(path.file_name().unwrap(), "image (2).png");
        assert_eq!(fs::read(path).unwrap(), vec![4, 5, 6]);
        assert_eq!(fs::read(temp.path().join("image.png")).unwrap(), b"base");
    }

    #[test]
    fn clipboard_tiff_image_file_uses_first_free_png_suffix() {
        let temp = TempDir::new();
        fs::write(temp.path().join("image.png"), b"base").expect("create base image");
        fs::write(temp.path().join("image (3).png"), b"third").expect("create third image");
        let image = Image::from_bytes(ImageFormat::Tiff, test_tiff_bytes());

        let path = create_clipboard_image_file_in_directory(temp.path(), &image).unwrap();

        assert_eq!(path.file_name().unwrap(), "image (2).png");
        assert_saved_png_image(&fs::read(path).unwrap());
        assert_eq!(fs::read(temp.path().join("image.png")).unwrap(), b"base");
        assert!(!temp.path().join("image.tiff").exists());
    }

    #[test]
    fn clipboard_tiff_image_file_rejects_invalid_tiff_without_creating_file() {
        let temp = TempDir::new();
        let image = Image::from_bytes(ImageFormat::Tiff, b"not a tiff".to_vec());

        let error = create_clipboard_image_file_in_directory(temp.path(), &image).unwrap_err();

        assert!(error.contains("Could not convert clipboard image to PNG"));
        assert!(fs::read_dir(temp.path()).unwrap().next().is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_tiff_image_png_bytes_converts_valid_tiff() {
        let png = macos_tiff_image_png_bytes(&test_tiff_bytes()).expect("converted png");

        assert_saved_png_image(&png);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_tiff_image_png_bytes_rejects_invalid_tiff() {
        assert_eq!(macos_tiff_image_png_bytes(b"not a tiff"), None);
    }

    #[test]
    fn new_folder_conflicts_with_existing_file_name() {
        let temp = TempDir::new();
        fs::write(temp.path().join("New folder"), b"file").expect("create conflicting file");

        let path = create_new_item_in_directory(temp.path(), NewItemKind::Folder).unwrap();

        assert_eq!(path.file_name().unwrap(), "New folder (2)");
        assert!(path.is_dir());
    }

    #[test]
    fn new_file_conflicts_with_existing_folder_name() {
        let temp = TempDir::new();
        fs::create_dir(temp.path().join("New file")).expect("create conflicting folder");

        let path = create_new_item_in_directory(temp.path(), NewItemKind::File).unwrap();

        assert_eq!(path.file_name().unwrap(), "New file (2)");
        assert!(path.is_file());
    }

    #[test]
    fn created_new_item_can_be_reloaded_and_selected() {
        let temp = TempDir::new();
        let created = create_new_item_in_directory(temp.path(), NewItemKind::Folder).unwrap();
        let mut view = ExplorerView::new(temp.path().to_path_buf());

        view.select_single_path(&created);

        assert_eq!(selected_names(&view), vec!["New folder"]);
    }

    #[test]
    fn selected_file_clipboard_is_empty_without_selection() {
        let view = test_view_with_entries(&["a.txt"]);

        assert_eq!(
            view.selected_file_clipboard(FileClipboardOperation::Copy),
            None
        );
    }

    #[test]
    fn selected_file_clipboard_includes_single_selection() {
        let mut view = test_view_with_entries(&["a.txt"]);
        view.select_single_index(0);

        let clipboard = view
            .selected_file_clipboard(FileClipboardOperation::Copy)
            .expect("clipboard");

        assert_eq!(clipboard.operation, FileClipboardOperation::Copy);
        assert_eq!(clipboard.paths, vec![PathBuf::from("a.txt")]);
    }

    #[test]
    fn selected_file_clipboard_includes_multi_selection() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);
        view.select_single_index(0);
        view.apply_click_selection(
            2,
            SelectionModifiers {
                toggle: true,
                extend: false,
            },
        );

        let clipboard = view
            .selected_file_clipboard(FileClipboardOperation::Cut)
            .expect("clipboard");

        assert_eq!(clipboard.operation, FileClipboardOperation::Cut);
        assert_eq!(
            clipboard.paths,
            vec![PathBuf::from("a.txt"), PathBuf::from("c.txt")]
        );
    }

    #[test]
    fn selected_archive_paths_requires_all_selected_items_to_be_archive_files() {
        let temp = TempDir::new();
        let archive = temp.path().join("archive.zip");
        let other_archive = temp.path().join("other.tar.gz");
        let text = temp.path().join("file.txt");
        fs::write(&archive, b"not a real zip").expect("create archive path");
        fs::write(&other_archive, b"not a real tarball").expect("create archive path");
        fs::write(&text, b"text").expect("create text");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&archive);

        assert_eq!(view.selected_archive_paths(), Some(vec![archive.clone()]));

        view.apply_click_selection(
            view.entries
                .iter()
                .position(|entry| entry.path == other_archive)
                .expect("other archive index"),
            SelectionModifiers {
                toggle: true,
                extend: false,
            },
        );

        assert_eq!(
            view.selected_archive_paths(),
            Some(vec![archive.clone(), other_archive])
        );

        view.apply_click_selection(
            view.entries
                .iter()
                .position(|entry| entry.path == text)
                .expect("text index"),
            SelectionModifiers {
                toggle: true,
                extend: false,
            },
        );

        assert_eq!(view.selected_archive_paths(), None);
    }

    #[test]
    fn selected_mountable_image_path_requires_one_supported_file() {
        let temp = TempDir::new();
        let image = temp.path().join("installer.iso");
        let other_image = temp.path().join("rescue.IMG");
        let text = temp.path().join("notes.txt");
        let folder = temp.path().join("folder.iso");
        fs::write(&image, b"not a real image").expect("create image path");
        fs::write(&other_image, b"not a real image").expect("create image path");
        fs::write(&text, b"text").expect("create text");
        fs::create_dir(&folder).expect("create folder");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&image);

        assert_eq!(view.selected_mountable_image_path(), Some(image.clone()));

        view.apply_click_selection(
            view.entries
                .iter()
                .position(|entry| entry.path == other_image)
                .expect("other image index"),
            SelectionModifiers {
                toggle: true,
                extend: false,
            },
        );

        assert_eq!(view.selected_mountable_image_path(), None);

        view.select_single_path(&text);
        assert_eq!(view.selected_mountable_image_path(), None);

        view.select_single_path(&folder);
        assert_eq!(view.selected_mountable_image_path(), None);
    }

    #[test]
    fn only_cut_paths_are_dimmed() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);

        view.mark_cut_paths(&[PathBuf::from("b.txt")]);

        assert!(!view.entry_is_cut(Path::new("a.txt")));
        assert!(view.entry_is_cut(Path::new("b.txt")));
        view.clear_cut_paths();
        assert!(!view.entry_is_cut(Path::new("b.txt")));
    }

    #[gpui::test]
    fn file_clipboard_paste_conflicts_open_dialog_for_copy_and_cut(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let source_dir = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir(&source_dir).expect("create source");
        fs::create_dir(&destination).expect("create destination");
        let source = source_dir.join("file.txt");
        fs::write(&source, b"source").expect("create source file");
        fs::write(destination.join("file.txt"), b"destination").expect("create destination file");
        let (view, cx) = test_view_entity_at_path(cx, destination);

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.paste_file_clipboard(
                    FileClipboard::new(FileClipboardOperation::Copy, vec![source.clone()]),
                    cx,
                );
                assert!(view.pending_file_conflict.is_some());
                assert!(view.operation_notice.is_none());

                view.pending_file_conflict = None;
                view.clear_active_dialog_window();
                view.paste_file_clipboard(
                    FileClipboard::new(FileClipboardOperation::Cut, vec![source.clone()]),
                    cx,
                );
                assert!(view.pending_file_conflict.is_some());
                assert!(view.operation_notice.is_none());
            });
        });
    }

    #[gpui::test]
    fn delete_confirmation_paths_stage_cancel_and_confirm(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let file = temp.path().join("delete.txt");
        fs::write(&file, b"delete").expect("create file");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.request_trash_paths_with_confirmation(Vec::new(), cx);
                assert!(view.pending_trash.is_none());

                view.request_trash_paths_with_confirmation(vec![file.clone()], cx);
                assert_eq!(
                    view.pending_trash
                        .as_ref()
                        .map(|pending| pending.paths.as_slice()),
                    Some([file.clone()].as_slice())
                );
                view.cancel_pending_trash();
                assert!(view.pending_trash.is_none());

                view.mark_cut_paths(std::slice::from_ref(&file));
                view.pending_permanent_delete = Some(PendingPermanentDelete {
                    paths: vec![file.clone()],
                });
                view.confirm_pending_permanent_delete(cx);
                assert!(view.pending_permanent_delete.is_none());
                assert!(view.operation_notice.is_none());
                assert!(!view.entry_is_cut(&file));
            });
        });

        assert!(!file.exists());
    }

    #[gpui::test]
    fn external_drag_unoptimized_move_cleans_up_existing_sources(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let file = temp.path().join("dragged.txt");
        let missing = temp.path().join("already-moved.txt");
        fs::write(&file, b"dragged").expect("create file");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.mark_cut_paths(std::slice::from_ref(&file));
                view.complete_external_paths_drag(
                    &[file.clone(), missing.clone()],
                    ExternalPathsDragResult::move_(true),
                    cx,
                );

                assert!(view.operation_notice.is_none());
                assert!(!view.entry_is_cut(&file));
            });
        });

        assert!(!file.exists());
    }

    #[gpui::test]
    fn external_drag_optimized_move_does_not_delete_sources(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let file = temp.path().join("dragged.txt");
        fs::write(&file, b"dragged").expect("create file");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.complete_external_paths_drag(
                    std::slice::from_ref(&file),
                    ExternalPathsDragResult::move_(false),
                    cx,
                );

                assert!(view.operation_notice.is_none());
            });
        });

        assert!(file.exists());
    }

    #[gpui::test]
    fn file_operation_cancel_and_error_completion_update_state(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.handle_prepared_file_command_result_and_open_dialog(
                    Err("prepare failed".to_owned()),
                    cx,
                );
                assert!(view.active_file_operation.is_none());

                view.resolve_pending_file_conflicts_and_open_progress(ConflictChoice::Skip, cx);
                assert!(view.active_file_operation.is_none());

                let cancel = Arc::new(AtomicBool::new(false));
                let terminate = Arc::new(AtomicBool::new(false));
                view.active_file_operation = Some(FileOperationState {
                    progress: test_progress(),
                    cancel: cancel.clone(),
                    terminate: terminate.clone(),
                    task: None,
                    archive_diagnostics: None,
                });
                view.cancel_active_file_operation();
                assert!(cancel.load(Ordering::Relaxed));
                assert!(!terminate.load(Ordering::Relaxed));

                view.terminate_active_file_operation();
                assert!(cancel.load(Ordering::Relaxed));
                assert!(terminate.load(Ordering::Relaxed));

                view.complete_active_file_operation(Err(FileOperationError::Cancelled), cx);
                assert!(view.active_file_operation.is_none());
                assert!(view.operation_notice.is_none());

                view.active_file_operation = Some(FileOperationState {
                    progress: test_progress(),
                    cancel: Arc::new(AtomicBool::new(false)),
                    terminate: Arc::new(AtomicBool::new(false)),
                    task: None,
                    archive_diagnostics: None,
                });
                view.complete_active_file_operation(
                    Err(FileOperationError::Failed("copy failed".to_owned())),
                    cx,
                );
                assert!(view.active_file_operation.is_none());
            });
        });
    }

    #[gpui::test]
    fn file_operation_success_completion_reloads_directory_async(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let destination = temp.path().join("created.txt");
        fs::write(&destination, b"data").expect("create destination");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.finish_file_operation(
                    FileOperationSummary {
                        kind: FileOperationKind::Copy,
                        destination_paths: vec![destination.clone()],
                        copy_undo: FileOperationCopyUndo::default(),
                        moved_source_paths: Vec::new(),
                        moved_paths: Vec::new(),
                        archive_diagnostics: None,
                    },
                    cx,
                );

                assert_eq!(view.loading_path.as_deref(), Some(temp.path()));
                assert!(view.directory_load_task.is_some());
                assert!(view.entries.is_empty());
            });
        });

        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(selected_names(view), vec!["created.txt"]);
        });
    }

    #[test]
    fn successful_cut_paste_moves_files_and_clears_cut_state() {
        let temp = TempDir::new();
        let source_dir = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir_all(&source_dir).expect("create source");
        fs::create_dir(&destination).expect("create destination");
        let source = source_dir.join("file.txt");
        fs::write(&source, b"data").expect("create source file");

        let mut view = ExplorerView::new(destination.clone());
        view.mark_cut_paths(std::slice::from_ref(&source));
        let result = move_paths_to_directory(std::slice::from_ref(&source), &view.path);
        view.handle_file_command_result(result);

        assert!(!source.exists());
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"data");
        assert!(view.cut_paths.is_empty());
        assert_eq!(selected_names(&view), vec!["file.txt"]);
    }

    #[test]
    fn undo_copy_removes_copied_files_and_folders() {
        let temp = TempDir::new();
        let source_dir = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir_all(source_dir.join("folder")).expect("create source folder");
        fs::create_dir(&destination).expect("create destination");
        let source_file = source_dir.join("file.txt");
        let source_folder = source_dir.join("folder");
        fs::write(&source_file, b"file").expect("create source file");
        fs::write(source_folder.join("nested.txt"), b"nested").expect("create nested file");

        let mut view = ExplorerView::new(destination.clone());
        let result = copy_paths_to_directory(&[source_file, source_folder], &view.path);
        view.handle_file_command_result(result);

        assert_eq!(view.file_operation_undo_stack.len(), 1);
        assert!(destination.join("file.txt").exists());
        assert!(destination.join("folder").join("nested.txt").exists());

        let undo = view.file_operation_undo_stack.last().cloned().unwrap();
        let selection = view.apply_file_operation_undo(undo).expect("undo copy");

        assert!(matches!(selection, UndoSelection::Clear));
        assert!(!destination.join("file.txt").exists());
        assert!(!destination.join("folder").exists());
    }

    #[test]
    fn undo_copy_replace_restores_existing_file() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        let replaced = destination.join("file.txt");
        fs::write(&source, b"source").expect("create source file");
        fs::create_dir(&destination).expect("create destination");
        fs::write(&replaced, b"existing").expect("create existing file");

        let conflicts = file_conflicts(copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let summary =
            resolve_file_conflicts(conflicts, ConflictChoice::Replace).expect("replace conflict");
        let mut view = ExplorerView::new(destination.clone());
        view.handle_file_command_result(Ok(FileOperationOutcome::Finished(summary)));

        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert_eq!(fs::read(&replaced).unwrap(), b"source");
        assert_eq!(view.file_operation_undo_stack.len(), 1);

        let undo = view.file_operation_undo_stack.last().cloned().unwrap();
        let selection = view.apply_file_operation_undo(undo).expect("undo copy");

        assert!(matches!(selection, UndoSelection::Clear));
        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert_eq!(fs::read(&replaced).unwrap(), b"existing");
    }

    #[test]
    fn undo_copy_folder_merge_preserves_destination_only_files() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        let destination = temp.path().join("destination");
        let destination_folder = destination.join("folder");
        fs::create_dir_all(source.join("nested")).expect("create source nested");
        fs::write(source.join("nested").join("file.txt"), b"source").expect("create source file");
        fs::create_dir_all(&destination_folder).expect("create destination folder");
        fs::write(destination_folder.join("extra.txt"), b"extra").expect("create destination file");

        let mut view = ExplorerView::new(destination.clone());
        let result = copy_paths_to_directory(std::slice::from_ref(&source), &view.path);
        view.handle_file_command_result(result);

        assert_eq!(
            fs::read(destination_folder.join("nested").join("file.txt")).unwrap(),
            b"source"
        );
        assert_eq!(view.file_operation_undo_stack.len(), 1);

        let undo = view.file_operation_undo_stack.last().cloned().unwrap();
        view.apply_file_operation_undo(undo).expect("undo copy");

        assert!(destination_folder.exists());
        assert_eq!(
            fs::read(destination_folder.join("extra.txt")).unwrap(),
            b"extra"
        );
        assert!(!destination_folder.join("nested").exists());
    }

    #[test]
    fn undo_copy_folder_merge_restores_nested_replacement() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        let destination = temp.path().join("destination");
        let destination_folder = destination.join("folder");
        let replaced = destination_folder.join("nested").join("file.txt");
        fs::create_dir_all(source.join("nested")).expect("create source nested");
        fs::write(source.join("nested").join("file.txt"), b"new").expect("create source file");
        fs::create_dir_all(replaced.parent().unwrap()).expect("create destination nested");
        fs::write(&replaced, b"old").expect("create destination file");

        let conflicts = file_conflicts(copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let summary =
            resolve_file_conflicts(conflicts, ConflictChoice::Replace).expect("replace nested");
        let mut view = ExplorerView::new(destination.clone());
        view.handle_file_command_result(Ok(FileOperationOutcome::Finished(summary)));

        assert_eq!(fs::read(&replaced).unwrap(), b"new");

        let undo = view.file_operation_undo_stack.last().cloned().unwrap();
        view.apply_file_operation_undo(undo).expect("undo copy");

        assert!(destination_folder.exists());
        assert_eq!(fs::read(&replaced).unwrap(), b"old");
    }

    #[test]
    fn skipped_copy_conflict_records_no_undo() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        let existing = destination.join("file.txt");
        fs::write(&source, b"source").expect("create source file");
        fs::create_dir(&destination).expect("create destination");
        fs::write(&existing, b"existing").expect("create existing file");

        let conflicts = file_conflicts(copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let summary =
            resolve_file_conflicts(conflicts, ConflictChoice::Skip).expect("skip conflict");
        let mut view = ExplorerView::new(destination.clone());
        view.handle_file_command_result(Ok(FileOperationOutcome::Finished(summary)));

        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert_eq!(fs::read(&existing).unwrap(), b"existing");
        assert!(view.file_operation_undo_stack.is_empty());
    }

    #[test]
    fn undo_link_removes_created_link_and_preserves_source() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::write(&source, b"data").expect("create source file");
        fs::create_dir(&destination).expect("create destination");

        let mut view = ExplorerView::new(destination.clone());
        let result = create_links_to_directory(std::slice::from_ref(&source), &view.path);
        view.handle_file_command_result(result);

        let shortcut = destination.join(if cfg!(target_os = "windows") {
            "file.txt - Shortcut.lnk"
        } else {
            "file.txt - Shortcut"
        });
        assert!(source.exists());
        assert!(fs::symlink_metadata(&shortcut).is_ok());
        assert_eq!(view.file_operation_undo_stack.len(), 1);

        let undo = view.file_operation_undo_stack.last().cloned().unwrap();
        view.apply_file_operation_undo(undo).expect("undo link");

        assert_eq!(fs::read(&source).unwrap(), b"data");
        assert!(fs::symlink_metadata(&shortcut).is_err());
    }

    #[test]
    fn undo_move_restores_source_destination_pairs() {
        let temp = TempDir::new();
        let source_dir = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir_all(&source_dir).expect("create source");
        fs::create_dir(&destination).expect("create destination");
        let source = source_dir.join("file.txt");
        let moved = destination.join("file.txt");
        fs::write(&source, b"data").expect("create source file");

        let mut view = ExplorerView::new(destination.clone());
        view.mark_cut_paths(std::slice::from_ref(&source));
        let result = move_paths_to_directory(std::slice::from_ref(&source), &view.path);
        view.handle_file_command_result(result);

        assert!(!source.exists());
        assert!(moved.exists());
        assert!(view.cut_paths.is_empty());

        let undo = view.file_operation_undo_stack.last().cloned().unwrap();
        let selection = view.apply_file_operation_undo(undo).expect("undo move");

        assert!(matches!(selection, UndoSelection::Paths(paths) if paths == vec![source.clone()]));
        assert_eq!(fs::read(&source).unwrap(), b"data");
        assert!(!moved.exists());
        assert!(!view.entry_is_cut(&source));
    }

    #[test]
    fn undo_move_rejects_original_path_collision() {
        let temp = TempDir::new();
        let source_dir = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir_all(&source_dir).expect("create source");
        fs::create_dir(&destination).expect("create destination");
        let source = source_dir.join("file.txt");
        let moved = destination.join("file.txt");
        fs::write(&source, b"data").expect("create source file");

        let mut view = ExplorerView::new(destination.clone());
        let result = move_paths_to_directory(std::slice::from_ref(&source), &view.path);
        view.handle_file_command_result(result);
        fs::write(&source, b"collision").expect("create collision");

        let undo = view.file_operation_undo_stack.last().cloned().unwrap();
        let error = view
            .apply_file_operation_undo(undo)
            .expect_err("collision should block undo");

        assert!(error.contains("already exists"));
        assert_eq!(fs::read(&source).unwrap(), b"collision");
        assert_eq!(fs::read(&moved).unwrap(), b"data");
    }

    #[test]
    fn extraction_summary_does_not_record_copy_undo() {
        let temp = TempDir::new();
        let extracted = temp.path().join("archive");
        fs::create_dir(&extracted).expect("create extracted folder");
        let mut view = ExplorerView::new(temp.path().to_path_buf());

        view.finish_file_operation_for_test(FileOperationSummary {
            kind: FileOperationKind::Extract,
            destination_paths: vec![extracted],
            copy_undo: FileOperationCopyUndo::default(),
            moved_source_paths: Vec::new(),
            moved_paths: Vec::new(),
            archive_diagnostics: None,
        });

        assert!(view.file_operation_undo_stack.is_empty());
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn trash_undo_capture_selects_new_matching_trash_items() {
        let temp = TempDir::new();
        let deleted = temp.path().join("deleted.txt");
        let before_ids = std::collections::BTreeSet::from([std::ffi::OsString::from("before")]);
        let original_keys = trash_undo_path_keys(std::slice::from_ref(&deleted));
        let items = vec![
            trash::TrashItem {
                id: std::ffi::OsString::from("before"),
                name: std::ffi::OsString::from("deleted.txt"),
                original_parent: temp.path().to_path_buf(),
                time_deleted: 1,
            },
            trash::TrashItem {
                id: std::ffi::OsString::from("after"),
                name: std::ffi::OsString::from("deleted.txt"),
                original_parent: temp.path().to_path_buf(),
                time_deleted: 2,
            },
        ];

        let undo =
            restorable_trash_undo_from_items(vec![deleted], original_keys, before_ids, items);

        match undo {
            TrashUndo::Restorable { items, .. } => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].id, std::ffi::OsString::from("after"));
            }
            TrashUndo::Unsupported { reason, .. } => {
                panic!("expected restorable trash undo, got {reason}");
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_trash_undo_capture_records_unsupported_entry() {
        let path = PathBuf::from("/tmp/deleted.txt");
        let undo = TrashUndoCapture::before_delete(std::slice::from_ref(&path))
            .after_delete()
            .expect("undo record");

        match undo {
            FileOperationUndo::Trash(TrashUndo::Unsupported {
                original_paths,
                reason,
            }) => {
                assert_eq!(original_paths, vec![path]);
                assert!(reason.contains("not supported"));
            }
            _ => panic!("expected unsupported trash undo"),
        }
    }

    #[gpui::test]
    fn undo_action_noops_empty_stack_and_reports_unsupported(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.handle_undo_file_operation(&crate::explorer::UndoFileOperation, window, cx);
                assert!(view.operation_notice.is_none());

                view.file_operation_undo_stack
                    .push(FileOperationUndo::Trash(TrashUndo::Unsupported {
                        original_paths: vec![temp.path().join("deleted.txt")],
                        reason: "unsupported undo".to_owned(),
                    }));
                view.handle_undo_file_operation(&crate::explorer::UndoFileOperation, window, cx);

                assert_eq!(
                    view.operation_notice
                        .as_ref()
                        .map(|notice| notice.text.as_str()),
                    Some("unsupported undo")
                );
                assert_eq!(view.file_operation_undo_stack.len(), 1);
            });
        });
    }

    #[test]
    fn successful_delete_reload_can_leave_surviving_rows_deselected() {
        let temp = TempDir::new();
        let a = temp.path().join("a.txt");
        let b = temp.path().join("b.txt");
        let c = temp.path().join("c.txt");
        fs::write(&a, b"a").expect("create a");
        fs::write(&b, b"b").expect("create b");
        fs::write(&c, b"c").expect("create c");
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&b);

        remove_paths_permanently(std::slice::from_ref(&b)).expect("delete b");
        view.reload();
        view.clear_selection();

        assert_eq!(selected_names(&view), Vec::<String>::new());
        assert_eq!(
            view.entries
                .iter()
                .map(|entry| entry.name.clone())
                .collect::<Vec<_>>(),
            vec!["a.txt", "c.txt"]
        );
    }

    fn test_tiff_bytes() -> Vec<u8> {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            1,
            image::Rgba([10, 20, 30, 255]),
        ));
        let mut bytes = Cursor::new(Vec::new());
        image
            .write_to(&mut bytes, image::ImageFormat::Tiff)
            .expect("encode test tiff");
        bytes.into_inner()
    }

    fn test_progress() -> crate::explorer::filesystem::FileOperationProgress {
        crate::explorer::filesystem::FileOperationProgress {
            kind: crate::explorer::filesystem::FileOperationKind::Copy,
            phase: crate::explorer::filesystem::FileOperationPhase::Copying,
            total_bytes: 1,
            copied_bytes: 0,
            verified_bytes: 0,
            work_total_bytes: 1,
            work_completed_bytes: 0,
            total_files: 1,
            completed_files: 0,
            current_item: None,
            cancellable: true,
        }
    }

    fn assert_saved_png_image(bytes: &[u8]) {
        const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

        assert_eq!(&bytes[..PNG_SIGNATURE.len()], PNG_SIGNATURE);
        let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
            .expect("decode saved png")
            .to_rgba8();
        assert_eq!(image.dimensions(), (2, 1));
        assert_eq!(image.get_pixel(0, 0).0, [10, 20, 30, 255]);
    }
}
