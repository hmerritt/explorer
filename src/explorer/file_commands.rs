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
        ClipboardMaterialization, ClipboardTextPayload, FileClipboard, FileClipboardOperation,
        clipboard_item_for_files, clipboard_text_payload_from_item, file_clipboard_from_item,
        image_clipboard_from_item,
    },
    explorer_fs::ExplorerFs,
    filesystem::{
        ConflictChoice, FileOperationCopyUndo, FileOperationError, FileOperationJob,
        FileOperationKind, FileOperationMove, FileOperationReplacedFile, FileOperationSummary,
        PreparedFileOperation, RemoteDeleteError, RemoteDeletePhase, RemoteDeleteProgress,
        RemoteDeleteSummary, archive_path_is_supported, cleanup_copy_undo_backups,
        execute_file_operation, execute_file_operation_with_progress,
        mountable_image_path_is_supported, prepare_move_paths_to_directory,
        remove_existing_paths_permanently, remove_remote_paths_permanently_with_progress,
        restore_replaced_file_from_copy_undo,
    },
    view::{
        ExplorerView, FileOperationState, PendingPermanentDelete, PendingTrash, RemoteDeleteState,
    },
};

#[cfg(test)]
use crate::explorer::filesystem::{
    FileConflictBatch, FileOperationOutcome, copy_paths_to_directory, create_links_to_directory,
    move_paths_to_directory, prepare_compress_paths, resolve_file_conflicts,
};

const FILE_OPERATION_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const FILE_OPERATION_UNDO_LIMIT: usize = 32;

#[derive(Clone, Debug)]
pub(super) enum FileOperationUndo {
    Copy { undo: FileOperationCopyUndo },
    Move { paths: Vec<FileOperationMove> },
    Trash(TrashUndo),
    Recovery(super::trash::RecoveryUndo),
}

#[derive(Clone, Debug)]
pub(super) enum TrashUndo {
    Native {
        ids: Vec<super::trash::TrashItemId>,
        original_paths: Vec<PathBuf>,
        failures: Vec<String>,
    },
    #[cfg(test)]
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
        if self.is_sidebar_group_view()
            || !crate::explorer::explorer_fs::ExplorerFs::new().can_mutate(&self.path)
        {
            return;
        }
        if super::remote_fs::is_remote(&self.path) {
            let path = self.path.clone();
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { create_new_item_in_directory(&path, kind) })
                    .await;
                let _ = this.update(cx, |view, cx| {
                    match result {
                        Ok(path) => {
                            view.reload_async_with_options(
                                super::view::ReloadMode {
                                    cache_policy:
                                        super::remote_directory_cache::DirectoryLoadPolicy::Fresh,
                                    preserve_selection: true,
                                    rebuild_sidebar: false,
                                    preserve_context_menu: false,
                                },
                                vec![path],
                                true,
                                false,
                                false,
                                cx,
                            );
                        }
                        Err(error) => view.set_error_notice(error),
                    }
                    cx.notify();
                });
            })
            .detach();
            return;
        }
        match create_new_item_in_directory(&self.path, kind) {
            Ok(path) => {
                self.clear_operation_notice();
                self.reload_async_with_options_and_focused_rename(
                    crate::explorer::view::ReloadMode {
                        cache_policy:
                            crate::explorer::remote_directory_cache::DirectoryLoadPolicy::Fresh,
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
        if self.is_trash_view() {
            return;
        }
        let selected_paths = self.selected_paths();
        if selected_paths
            .iter()
            .any(|path| crate::explorer::archive_fs::is_archive_path(path))
        {
            self.copy_selected_archive_entries_to_clipboard(selected_paths, cx);
            return;
        }
        let Some(clipboard) = self.selected_file_clipboard(FileClipboardOperation::Copy) else {
            return;
        };

        match clipboard_item_for_files(&clipboard) {
            Ok(item) => {
                crate::explorer::clipboard::write_to_clipboard_and_refresh(item, cx);
                self.cut_paths.clear();
                self.clear_operation_notice();
            }
            Err(error) => self.set_error_notice(error),
        }
    }

    fn copy_selected_archive_entries_to_clipboard(
        &mut self,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() || self.archive_copy_task.is_some() {
            return;
        }
        self.set_info_notice(format!(
            "Preparing {} from the archive...",
            if paths.len() == 1 {
                "1 item".to_owned()
            } else {
                format!("{} items", paths.len())
            }
        ));
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::explorer::archive_fs::materialize_paths(&paths) })
                .await;
            let _ = this.update(cx, |explorer, cx| {
                explorer.archive_copy_task = None;
                match result {
                    Ok(paths) => {
                        let clipboard = FileClipboard::new(FileClipboardOperation::Copy, paths);
                        match clipboard_item_for_files(&clipboard) {
                            Ok(item) => {
                                crate::explorer::clipboard::write_to_clipboard_and_refresh(
                                    item, cx,
                                );
                                explorer.cut_paths.clear();
                                explorer.clear_operation_notice();
                            }
                            Err(error) => explorer.set_error_notice(error),
                        }
                    }
                    Err(error) => explorer.set_error_notice(error),
                }
                cx.notify();
            });
        });
        self.archive_copy_task = Some(task);
    }

    pub(super) fn cut_selected_to_clipboard(&mut self, cx: &mut Context<Self>) {
        if self.is_sidebar_group_view() {
            return;
        }
        if self
            .selected_paths()
            .iter()
            .any(|path| crate::explorer::archive_fs::is_archive_path(path))
        {
            self.set_error_notice("Items inside archives cannot be cut.".to_owned());
            return;
        }
        let Some(clipboard) = self.selected_file_clipboard(FileClipboardOperation::Cut) else {
            return;
        };

        match clipboard_item_for_files(&clipboard) {
            Ok(item) => {
                crate::explorer::clipboard::write_to_clipboard_and_refresh(item, cx);
                self.mark_cut_paths(&clipboard.paths);
                self.clear_operation_notice();
            }
            Err(error) => self.set_error_notice(error),
        }
    }

    pub(super) fn paste_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_sidebar_group_view()
            || !crate::explorer::explorer_fs::ExplorerFs::new().can_mutate(&self.path)
        {
            return;
        }
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };

        if let Some(clipboard) = file_clipboard_from_item(&item) {
            self.paste_file_clipboard(clipboard, cx);
            return;
        }

        if let Some(image) = image_clipboard_from_item(&item) {
            self.paste_clipboard_image(image, window, cx);
            return;
        }

        if let Some(payload) = clipboard_text_payload_from_item(&item) {
            match payload {
                ClipboardTextPayload::Downloads(downloads) => {
                    for download in downloads {
                        self.start_clipboard_download(download, cx);
                    }
                }
                ClipboardTextPayload::VideoDownloads(downloads) => {
                    self.start_video_downloads(downloads, cx);
                }
                ClipboardTextPayload::Materialization(materialization) => {
                    self.paste_clipboard_materialization(materialization, window, cx);
                }
            }
        }
    }

    fn paste_file_clipboard(&mut self, clipboard: FileClipboard, cx: &mut Context<Self>) {
        if clipboard.paths.iter().any(|p| super::trash::is_item(p)) {
            if clipboard.operation == FileClipboardOperation::Cut
                && clipboard.paths.iter().all(|p| super::trash::is_item(p))
            {
                self.prepare_bin_recovery(
                    super::trash::RecoveryRequest {
                        ids: super::trash::ids(&clipboard.paths),
                        directory: Some(self.path.clone()),
                    },
                    cx,
                );
            } else {
                self.set_error_notice(
                    "Bin items can only be recovered with Cut and Paste.".to_owned(),
                );
            }
            return;
        }
        if super::remote_fs::is_remote(&self.path)
            || clipboard
                .paths
                .iter()
                .any(|p| super::remote_fs::is_remote(p))
        {
            self.start_native_transfer(
                clipboard.paths,
                self.path.clone(),
                clipboard.operation == FileClipboardOperation::Cut,
                cx,
            );
            return;
        }
        if crate::explorer::portable_devices::is_portable_path(&self.path)
            || clipboard.paths.iter().any(|path| {
                super::remote_fs::is_remote(path)
                    || crate::explorer::portable_devices::is_portable_path(path)
            })
        {
            self.start_portable_transfer(
                clipboard.paths,
                self.path.clone(),
                clipboard.operation == FileClipboardOperation::Cut,
                cx,
            );
            return;
        }
        self.enqueue_operation(
            super::operations::Request::Files {
                sources: clipboard.paths,
                destination: self.path.clone(),
                kind: if clipboard.operation == FileClipboardOperation::Cut {
                    FileOperationKind::Move
                } else {
                    FileOperationKind::Copy
                },
                paste: true,
            },
            cx,
        );
    }

    pub(super) fn start_portable_transfer(
        &mut self,
        sources: Vec<PathBuf>,
        destination: PathBuf,
        move_sources: bool,
        cx: &mut Context<Self>,
    ) {
        self.enqueue_operation(
            super::operations::Request::Portable {
                sources,
                destination,
                moving: move_sources,
            },
            cx,
        );
    }

    fn paste_clipboard_image(
        &mut self,
        image: &Image,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if super::remote_fs::is_remote(&self.path) {
            self.set_error_notice(
                "Paste the image into a local folder, then copy that file to the server."
                    .to_owned(),
            );
            cx.notify();
            return;
        }
        match create_clipboard_image_file_in_directory(&self.path, image) {
            Ok(path) => {
                self.clear_operation_notice();
                self.reload_async_with_options_and_focused_rename(
                    crate::explorer::view::ReloadMode {
                        cache_policy:
                            crate::explorer::remote_directory_cache::DirectoryLoadPolicy::Fresh,
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

    fn paste_clipboard_materialization(
        &mut self,
        materialization: ClipboardMaterialization,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if super::remote_fs::is_remote(&self.path) {
            self.set_error_notice(
                "Paste this content into a local file, then copy that file to the server."
                    .to_owned(),
            );
            cx.notify();
            return;
        }
        match create_clipboard_materialization_in_directory(&self.path, &materialization) {
            Ok(path) => {
                self.clear_operation_notice();
                self.reload_async_with_options_and_focused_rename(
                    crate::explorer::view::ReloadMode {
                        cache_policy:
                            crate::explorer::remote_directory_cache::DirectoryLoadPolicy::Fresh,
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

        self.enqueue_operation(
            super::operations::Request::Extract {
                sources: paths,
                destination: self.path.clone(),
            },
            cx,
        );
    }

    pub(super) fn compress_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        self.enqueue_operation(super::operations::Request::Compress(paths), cx);
    }

    pub(super) fn trash_selected_paths(&mut self, cx: &mut Context<Self>) {
        if self.is_trash_view() {
            self.request_bin_delete(cx);
            return;
        }
        if self.is_sidebar_group_view()
            || !crate::explorer::explorer_fs::ExplorerFs::new().can_mutate(&self.path)
        {
            return;
        }
        if self.has_active_mutating_operation() {
            self.set_error_notice("Another file operation is already running.".to_owned());
            return;
        }
        if self.pending_trash_task.is_some() {
            return;
        }

        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        if paths.iter().any(|path| {
            super::remote_fs::is_remote(path)
                || crate::explorer::portable_devices::is_portable_path(path)
        }) {
            self.pending_permanent_delete = Some(PendingPermanentDelete { paths });
            self.clear_operation_notice();
            self.open_pending_dialog_window(cx);
            return;
        }

        self.start_trash_operation(paths, cx);
    }

    pub(super) fn request_trash_paths_with_confirmation(
        &mut self,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() || self.pending_trash_task.is_some() {
            return;
        }
        if self.has_active_mutating_operation() {
            self.set_error_notice("Another file operation is already running.".to_owned());
            return;
        }

        if paths.iter().any(|path| {
            super::remote_fs::is_remote(path)
                || crate::explorer::portable_devices::is_portable_path(path)
        }) {
            self.pending_permanent_delete = Some(PendingPermanentDelete { paths });
            self.clear_operation_notice();
            self.open_pending_dialog_window(cx);
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
        if self.pending_trash_task.is_some() {
            return;
        }

        self.start_trash_operation(pending.paths, cx);
    }

    pub(super) fn cancel_pending_trash(&mut self) {
        self.pending_trash = None;
    }

    fn start_trash_operation(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if !paths.is_empty() {
            self.enqueue_operation(super::operations::Request::Trash(paths), cx);
        }
    }

    fn complete_trash_operation(
        &mut self,
        operation_path: PathBuf,
        paths: Vec<PathBuf>,
        selection_after_delete: Option<PathBuf>,
        result: Result<Option<FileOperationUndo>, String>,
        cx: &mut Context<Self>,
    ) {
        self.pending_trash_task = None;
        self.pending_deleted_paths.clear();

        match result {
            Ok(trash_undo) => {
                let (completed_paths, failures) = match &trash_undo {
                    Some(FileOperationUndo::Trash(TrashUndo::Native {
                        original_paths,
                        failures,
                        ..
                    })) => (original_paths.clone(), failures.clone()),
                    _ => (paths.clone(), Vec::new()),
                };
                self.push_file_operation_undo(trash_undo);
                self.remove_cut_paths(&completed_paths);
                if self.path == operation_path {
                    self.reload_after_successful_delete(selection_after_delete, cx);
                }
                if failures.is_empty() {
                    self.clear_operation_notice();
                } else {
                    self.set_error_notice(failures.join("\n"));
                }
                self.emit_filesystem_changed(cx);
            }
            Err(error) => {
                self.emit_filesystem_changed(cx);
                if self.path == operation_path {
                    self.reload_after_failed_delete(paths, cx);
                }
                self.set_error_notice(error);
            }
        }
        cx.notify();
    }

    pub(super) fn request_permanent_delete_selected(&mut self, cx: &mut Context<Self>) {
        if self.is_trash_view() {
            self.request_bin_delete(cx);
            return;
        }
        if self.is_sidebar_group_view()
            || !crate::explorer::explorer_fs::ExplorerFs::new().can_mutate(&self.path)
        {
            return;
        }
        if self.has_active_mutating_operation() {
            self.set_error_notice("Another file operation is already running.".to_owned());
            return;
        }
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

        self.enqueue_operation(super::operations::Request::Delete(pending.paths), cx);
    }

    pub(super) fn cancel_pending_permanent_delete(&mut self) {
        self.pending_permanent_delete = None;
    }

    pub(super) fn cancel_active_remote_delete(&mut self) {
        if let Some(operation) = self.active_remote_delete.as_ref() {
            operation.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn start_remote_delete_operation(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        if self.has_active_mutating_operation() || self.pending_drop_task.is_some() {
            self.set_error_notice("Another file operation is already running.".to_owned());
            return;
        }

        let selection_after_delete = self.selection_after_removing_paths(&paths);
        let operation_path = self.path.clone();
        self.pending_deleted_paths = paths.clone();
        self.filter_pending_deleted_entries();
        if let Some(path) = selection_after_delete.as_ref() {
            self.restore_selection_from_paths(std::slice::from_ref(path));
            self.reveal_selection_after_delete();
        } else {
            self.clear_selection();
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let progress = RemoteDeleteProgress {
            phase: RemoteDeletePhase::Preparing,
            total_items: paths.len(),
            completed_items: 0,
            current_item: paths.first().cloned(),
        };
        self.active_remote_delete = Some(RemoteDeleteState {
            progress,
            cancel: cancel.clone(),
            task: None,
        });
        self.clear_operation_notice();
        self.open_remote_delete_operation_window(cx);
        cx.notify();

        let (progress_tx, progress_rx) = mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let paths_for_operation = paths.clone();
        let task = cx.spawn({
            let cancel = cancel.clone();
            let finished = finished.clone();
            async move |this, cx| {
                let operation_task = cx.background_executor().spawn({
                    let progress_tx = progress_tx.clone();
                    let finished = finished.clone();
                    async move {
                        let result = remove_remote_paths_permanently_with_progress(
                            &paths_for_operation,
                            &cancel,
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
                    Self::drain_remote_delete_progress(&this, cx, &progress_rx);
                }

                let result = operation_task.await;
                Self::drain_remote_delete_progress(&this, cx, &progress_rx);
                let _ = this.update(cx, |explorer, cx| {
                    explorer.complete_remote_delete_operation(
                        operation_path,
                        paths,
                        selection_after_delete,
                        result,
                        cx,
                    );
                    cx.notify();
                });
            }
        });

        if let Some(operation) = self.active_remote_delete.as_mut() {
            operation.task = Some(task);
        }
    }

    fn drain_remote_delete_progress(
        this: &gpui::WeakEntity<Self>,
        cx: &mut gpui::AsyncApp,
        progress_rx: &mpsc::Receiver<RemoteDeleteProgress>,
    ) {
        let mut latest = None;
        while let Ok(progress) = progress_rx.try_recv() {
            latest = Some(progress);
        }

        if let Some(progress) = latest {
            let _ = this.update(cx, |explorer, cx| {
                if let Some(operation) = explorer.active_remote_delete.as_mut() {
                    operation.progress = progress;
                    cx.notify();
                }
            });
        }
    }

    fn complete_remote_delete_operation(
        &mut self,
        operation_path: PathBuf,
        paths: Vec<PathBuf>,
        selection_after_delete: Option<PathBuf>,
        result: Result<RemoteDeleteSummary, RemoteDeleteError>,
        cx: &mut Context<Self>,
    ) {
        if let Some(handle) = self.active_dialog_window.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
        self.active_remote_delete = None;
        self.pending_deleted_paths.clear();

        let deleted_any = result
            .as_ref()
            .map(|summary| summary.deleted_any)
            .unwrap_or_else(|error| error.deleted_any());
        match result {
            Ok(_) => {
                self.remove_cut_paths(&paths);
                if self.path == operation_path {
                    self.reload_after_successful_delete(selection_after_delete, cx);
                }
                self.clear_operation_notice();
            }
            Err(RemoteDeleteError::Cancelled { .. }) => {
                self.clear_operation_notice();
                if self.path == operation_path {
                    self.reload_after_failed_delete(paths, cx);
                }
            }
            Err(RemoteDeleteError::Failed { message, .. }) => {
                self.set_error_notice(message);
                if self.path == operation_path {
                    self.reload_after_failed_delete(paths, cx);
                } else {
                    self.reload_with_entry_metadata_resolution(cx);
                }
            }
        }
        if deleted_any {
            self.emit_filesystem_changed(cx);
        }
    }

    pub(super) fn complete_external_paths_drag(
        &mut self,
        source_paths: &[PathBuf],
        result: ExternalPathsDragResult,
        cx: &mut Context<Self>,
    ) {
        // Native handoff only transports the private token. Recovery owns source
        // cleanup and clipboard reconciliation, even when the OS reports a move.
        if source_paths.iter().any(|path| super::trash::is_item(path)) {
            return;
        }
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
        mut job: FileOperationJob,
        conflict_choice: ConflictChoice,
        cx: &mut Context<Self>,
    ) {
        if self.has_active_mutating_operation() {
            self.set_error_notice("Another file operation is already running.".to_owned());
            return;
        }

        job.set_copy_verify(self.copy_verify);

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
        self.reload_async_with_options_preserving_live_selection(
            crate::explorer::view::ReloadMode {
                cache_policy: crate::explorer::remote_directory_cache::DirectoryLoadPolicy::Fresh,
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
            FileOperationKind::Copy | FileOperationKind::Link | FileOperationKind::Compress => {
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

    pub(super) fn push_file_operation_undo(&mut self, undo: Option<FileOperationUndo>) {
        let Some(undo) = undo else {
            return;
        };

        if self.file_operation_undo_stack.len() == FILE_OPERATION_UNDO_LIMIT {
            let expired = self.file_operation_undo_stack.remove(0);
            cleanup_file_operation_undo(expired);
        }
        self.file_operation_undo_stack.push(undo);
    }

    pub(super) fn enqueue_operation(
        &mut self,
        request: super::operations::Request,
        cx: &mut Context<Self>,
    ) {
        let sources = request.locations().0;
        let after = if matches!(
            request,
            super::operations::Request::Delete(_) | super::operations::Request::Trash(_)
        ) {
            self.selection_after_removing_paths(&sources)
        } else {
            None
        };
        super::operations::submit(
            request,
            cx.entity(),
            self.path.clone(),
            self.copy_verify,
            &self.entries,
            self.selected_paths(),
            after,
            cx,
        );
    }

    pub(super) fn complete_queued_operation(
        &mut self,
        outcome: super::operations::Outcome,
        operation_path: &Path,
        sources: &[PathBuf],
        selection_before: &[PathBuf],
        after_delete: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        use super::operations::Outcome;
        let mut selection = Vec::new();
        let mut completed_deletions = Vec::new();
        let deleting = matches!(outcome, Outcome::Trash(_) | Outcome::Deleted { .. });
        let preserve_delete_selection = self.path == operation_path
            && (self.selected_paths() == selection_before
                || self
                    .selected_paths()
                    .iter()
                    .all(|path| sources.contains(path)));
        match outcome {
            Outcome::Files(summary) => {
                self.record_file_operation_undo(&summary);
                self.remove_cut_paths(&summary.moved_source_paths);
                selection = if summary.kind == FileOperationKind::Extract {
                    self.selected_paths()
                } else {
                    summary.destination_paths
                };
            }
            Outcome::Trash(undo) => {
                if let Some(FileOperationUndo::Trash(TrashUndo::Native {
                    original_paths, ..
                })) = &undo
                {
                    self.remove_cut_paths(original_paths);
                    completed_deletions = original_paths.clone();
                }
                self.push_file_operation_undo(undo);
            }
            Outcome::Deleted { paths, .. } => {
                self.remove_cut_paths(&paths);
                completed_deletions = paths;
            }
            Outcome::Portable {
                destinations,
                moved,
                ..
            } => {
                self.remove_cut_paths(&moved);
                selection = destinations;
            }
            Outcome::Bin(result) => {
                if !result.undo.paths.is_empty() {
                    self.push_file_operation_undo(Some(FileOperationUndo::Recovery(result.undo)));
                }
                self.reconcile_bin_clipboard(cx);
                self.reconcile_bin_undo();
            }
            Outcome::Cancelled | Outcome::Cleaned => {}
        }
        if deleting && preserve_delete_selection {
            let remaining_selection: Vec<_> = selection_before
                .iter()
                .filter(|path| !completed_deletions.contains(path))
                .cloned()
                .collect();
            if remaining_selection.is_empty() {
                self.reload_after_successful_delete(after_delete, cx);
            } else {
                self.reload_after_failed_delete(remaining_selection, cx);
            }
            self.emit_filesystem_changed(cx);
            cx.notify();
            return;
        }
        selection.retain(|path| path.parent() == Some(self.path.as_path()));
        if self.path == operation_path && !selection.is_empty() {
            self.reload_async_with_options_preserving_live_selection(
                super::view::ReloadMode {
                    cache_policy: super::remote_directory_cache::DirectoryLoadPolicy::Fresh,
                    preserve_selection: true,
                    rebuild_sidebar: true,
                    preserve_context_menu: false,
                },
                selection,
                true,
                false,
                false,
                cx,
            );
        } else {
            self.refresh_with_entry_metadata_resolution(cx);
        }
        self.emit_filesystem_changed(cx);
        cx.notify();
    }

    pub(super) fn undo_file_operation(&mut self, cx: &mut Context<Self>) {
        if self.has_background_operation() || super::operations::outstanding(cx) > 0 {
            return;
        }
        if let Some(FileOperationUndo::Trash(TrashUndo::Native {
            ids,
            original_paths,
            ..
        })) = self.file_operation_undo_stack.last().cloned()
        {
            self.undo_bin_delete(ids, original_paths, cx);
            return;
        }
        if let Some(FileOperationUndo::Recovery(undo)) =
            self.file_operation_undo_stack.last().cloned()
        {
            self.undo_bin_recovery(undo, cx);
            return;
        }
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
                let restored_paths = undo_moved_paths(&paths, self.copy_verify)?;
                self.remove_cut_paths(&restored_paths);
                Ok(UndoSelection::Paths(restored_paths))
            }
            FileOperationUndo::Trash(trash) => undo_trash_paths(trash).map(UndoSelection::Paths),
            FileOperationUndo::Recovery(_) => unreachable!("recovery undo runs in the background"),
        }
    }
}

impl Drop for ExplorerView {
    fn drop(&mut self) {
        if let Some(operation) = &self.trash_operation {
            operation.cancel.store(true, Ordering::Relaxed);
        }
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
    let _cache_invalidation = crate::explorer::remote_directory_cache::DirectoryMutation::new(
        undo.created_files
            .iter()
            .chain(&undo.created_directories)
            .cloned()
            .chain(
                undo.replaced_files
                    .iter()
                    .map(|file| file.destination.clone()),
            ),
    );
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

fn undo_moved_paths(
    paths: &[FileOperationMove],
    copy_verify: bool,
) -> Result<Vec<PathBuf>, String> {
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
            PreparedFileOperation::Ready(mut job) => {
                job.set_copy_verify(copy_verify);
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

pub(super) fn run_trash_operation(
    paths: Vec<PathBuf>,
) -> Result<Option<FileOperationUndo>, String> {
    let outcome = super::trash::trash_paths_batch(&paths)?;
    if outcome.paths.is_empty() && !outcome.failures.is_empty() {
        return Err(outcome.failures.join("\n"));
    }
    Ok(Some(FileOperationUndo::Trash(TrashUndo::Native {
        ids: outcome.ids,
        original_paths: outcome.paths,
        failures: outcome.failures,
    })))
}

fn undo_trash_paths(trash: TrashUndo) -> Result<Vec<PathBuf>, String> {
    match trash {
        TrashUndo::Native {
            ids,
            original_paths,
            ..
        } => {
            let result = super::trash::recover(
                super::trash::RecoveryRequest {
                    ids,
                    directory: None,
                },
                super::trash::RecoveryChoice::Skip,
                &AtomicBool::new(false),
                |_, _, _| {},
            );
            super::trash::cleanup_undo(result.undo);
            if !result.failures.is_empty() || !result.skipped.is_empty() {
                return Err(format!(
                    "Some items could not be restored. {}",
                    result.failures.join("\n")
                ));
            }
            Ok(original_paths)
        }
        #[cfg(test)]
        TrashUndo::Unsupported {
            original_paths,
            reason,
        } => {
            let _ = original_paths;
            Err(reason)
        }
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
        let path = if let Some(location) = super::remote_fs::RemoteLocation::from_provider(parent) {
            location.child(&name)?.provider_path()
        } else {
            parent.join(&name)
        };

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

fn create_clipboard_materialization_in_directory(
    parent: &Path,
    materialization: &ClipboardMaterialization,
) -> Result<PathBuf, String> {
    let explorer_fs = ExplorerFs::new();
    let cancel = AtomicBool::new(false);
    let mut index = 1usize;
    loop {
        let name = clipboard_materialization_file_name(materialization.file_name, index);
        let path = parent.join(&name);
        if explorer_fs.exists(&path)? {
            index = next_new_item_index(index, &name)?;
            continue;
        }
        match write_file_path_with_cancel(&path, &materialization.contents, &cancel, &explorer_fs) {
            Ok(()) => return Ok(path),
            Err(error) if error.to_ascii_lowercase().contains("already exist") => {
                index = next_new_item_index(index, &name)?;
            }
            Err(error) => return Err(format!("Could not create \"{name}\": {error}")),
        }
    }
}

fn clipboard_materialization_file_name(file_name: &str, index: usize) -> String {
    if index == 1 {
        return file_name.to_owned();
    }
    let (stem, extension) = file_name
        .rsplit_once('.')
        .expect("clipboard materialization names always have an extension");
    format!("{stem} ({index}).{extension}")
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
        view::ExplorerContentBranch,
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
    fn clipboard_materialization_uses_windows_style_suffixes_without_overwrite() {
        let temp = TempDir::new();
        fs::write(temp.path().join("document.md"), b"existing").unwrap();
        let materialization = ClipboardMaterialization {
            file_name: "document.md",
            contents: b"# New".to_vec(),
        };

        let path = create_clipboard_materialization_in_directory(temp.path(), &materialization)
            .expect("materialized file");

        assert_eq!(path.file_name().unwrap(), "document (2).md");
        assert_eq!(
            fs::read(temp.path().join("document.md")).unwrap(),
            b"existing"
        );
        assert_eq!(fs::read(path).unwrap(), b"# New");
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
    fn file_clipboard_paste_conflicts_hold_the_shared_queue_for_copy_and_cut(
        cx: &mut TestAppContext,
    ) {
        let temp = TempDir::new();
        let source_dir = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir(&source_dir).unwrap();
        fs::create_dir(&destination).unwrap();
        let source = source_dir.join("file.txt");
        fs::write(&source, b"source").unwrap();
        fs::write(destination.join("file.txt"), b"destination").unwrap();
        let (view, cx) = test_view_entity_at_path(cx, destination.clone());
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                for operation in [FileClipboardOperation::Copy, FileClipboardOperation::Cut] {
                    view.paste_file_clipboard(
                        FileClipboard::new(operation, vec![source.clone()]),
                        cx,
                    );
                }
                assert!(view.pending_file_conflict.is_none());
            })
        });
        super::super::operations::settle_for_test(cx);
        cx.update(|_, app| {
            assert_eq!(
                super::super::operations::states_for_test(app),
                vec![
                    super::super::operations::State::Attention,
                    super::super::operations::State::Queued
                ]
            );
            super::super::operations::control_for_test(1, "cancel", app);
        });
        super::super::operations::settle_for_test(cx);
        cx.update(|_, app| {
            assert_eq!(
                super::super::operations::states_for_test(app)[1],
                super::super::operations::State::Attention
            );
            super::super::operations::control_for_test(2, "cancel", app);
        });
        super::super::operations::settle_for_test(cx);
        assert!(source.exists());
        assert_eq!(
            fs::read(destination.join("file.txt")).unwrap(),
            b"destination"
        );
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
                assert!(view.entry_is_cut(&file));
            });
        });

        super::super::operations::settle_for_test(cx);
        assert!(!file.exists());
        cx.read_entity(&view, |view, _| assert!(!view.entry_is_cut(&file)));
    }

    #[gpui::test]
    fn multiple_trash_requests_are_queued(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let first = temp.path().join("first.txt");
        let second = temp.path().join("second.txt");
        fs::write(&first, b"first").expect("create first file");
        fs::write(&second, b"second").expect("create second file");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());
        super::super::operations::settle_for_test(cx);

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.restore_selection_from_paths(std::slice::from_ref(&first));
                view.trash_selected_paths(cx);
                assert!(super::super::operations::outstanding(cx) > 0);

                view.restore_selection_from_paths(std::slice::from_ref(&second));
                view.trash_selected_paths(cx);
                assert!(super::super::operations::outstanding(cx) > 0);
            });
        });

        super::super::operations::settle_for_test(cx);

        assert!(!first.exists());
        assert!(!second.exists());
        cx.read_entity(&view, |view, _| {
            assert!(view.pending_trash_task.is_none());
            assert_eq!(view.file_operation_undo_stack.len(), 2);
        });
    }

    #[gpui::test]
    fn failed_trash_operation_survives_tab_close_and_holds_the_queue(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.start_trash_operation(vec![temp.path().join("missing.txt")], cx);
                view.prepare_for_tab_close(cx);
            })
        });
        super::super::operations::settle_for_test(cx);
        cx.update(|_, app| {
            assert_eq!(
                super::super::operations::states_for_test(app),
                vec![super::super::operations::State::Attention]
            );
            assert_eq!(super::super::operations::outstanding(app), 1);
            super::super::operations::control_for_test(1, "cancel", app);
        });
        cx.read_entity(&view, |view, _| {
            assert!(view.file_operation_undo_stack.is_empty())
        });
    }

    #[gpui::test]
    fn failed_trash_completion_restores_hidden_paths_and_selection(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let first = temp.path().join("first.txt");
        let second = temp.path().join("second.txt");
        fs::write(&first, b"first").expect("create first file");
        fs::write(&second, b"second").expect("create second file");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());
        cx.run_until_parked();

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.pending_deleted_paths = vec![first.clone()];
                view.filter_pending_deleted_entries();
                view.select_single_path(&second);
                assert_eq!(selected_names(view), vec!["second.txt"]);

                view.complete_trash_operation(
                    temp.path().to_path_buf(),
                    vec![first.clone()],
                    Some(second.clone()),
                    Err("Could not move the selected item to the Recycle Bin.".to_owned()),
                    cx,
                );

                assert!(view.pending_deleted_paths.is_empty());
                assert!(view.entries.iter().all(|entry| entry.path != first));
                assert!(
                    view.operation_notice
                        .as_ref()
                        .is_some_and(|notice| notice.text.contains("Recycle Bin"))
                );
            });
        });
        cx.run_until_parked();

        assert!(first.exists());
        assert!(second.exists());
        cx.read_entity(&view, |view, _| {
            assert!(view.pending_deleted_paths.is_empty());
            assert_eq!(selected_names(view), vec!["first.txt"]);
            assert!(view.entries.iter().any(|entry| entry.path == first));
            assert!(
                view.operation_notice
                    .as_ref()
                    .is_some_and(|notice| notice.text.contains("Recycle Bin"))
            );
        });
    }

    #[gpui::test]
    fn failed_trash_completion_does_not_change_selection_after_navigation(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let origin = temp.path().join("origin");
        let destination = temp.path().join("destination");
        fs::create_dir(&origin).expect("create origin directory");
        fs::create_dir(&destination).expect("create destination directory");
        let attempted = origin.join("attempted.txt");
        let current = destination.join("current.txt");
        fs::write(&attempted, b"attempted").expect("create attempted file");
        fs::write(&current, b"current").expect("create current file");
        let (view, cx) = test_view_entity_at_path(cx, destination);
        cx.run_until_parked();

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.select_single_path(&current);
                view.pending_deleted_paths = vec![attempted.clone()];

                view.complete_trash_operation(
                    origin,
                    vec![attempted],
                    None,
                    Err("Could not move the selected item to the Recycle Bin.".to_owned()),
                    cx,
                );

                assert!(view.pending_deleted_paths.is_empty());
                assert_eq!(selected_names(view), vec!["current.txt"]);
                assert!(view.directory_load_task.is_none());
            });
        });
    }

    #[gpui::test]
    fn successful_trash_completion_does_not_change_selection_or_scroll_after_navigation(
        cx: &mut TestAppContext,
    ) {
        let temp = TempDir::new();
        let origin = temp.path().join("origin");
        let destination = temp.path().join("destination");
        fs::create_dir(&origin).unwrap();
        fs::create_dir(&destination).unwrap();
        for ix in 0..80 {
            fs::write(destination.join(format!("item-{ix:03}.txt")), b"file").unwrap();
        }
        let current = destination.join("item-040.txt");
        let (view, cx) = test_view_entity_at_path(cx, destination);
        cx.run_until_parked();
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.restore_selection_from_paths(std::slice::from_ref(&current));
                view.set_scroll_offset(400.0);
                view.complete_trash_operation(
                    origin.clone(),
                    vec![origin.join("deleted.txt")],
                    Some(origin.join("previous.txt")),
                    Ok(None),
                    cx,
                );
                assert!(view.directory_load_task.is_none());
                assert_eq!(view.selected_paths(), vec![current.clone()]);
            });
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.selected_paths(), vec![current]);
            super::super::test_support::assert_approx_eq(
                view.scrollbar_metrics().unwrap().scroll_top,
                400.0,
            );
        });
    }

    #[gpui::test]
    fn failed_trash_restores_large_icon_selection_without_resetting_scroll(
        cx: &mut TestAppContext,
    ) {
        let temp = TempDir::new();
        for ix in 0..180 {
            fs::write(temp.path().join(format!("item-{ix:03}.txt")), b"file").unwrap();
        }
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.select_view_mode(crate::settings::FileViewMode::LargeIcons, cx);
                cx.notify();
            });
        });
        cx.run_until_parked();
        let (attempted, before) = cx.update(|_, app| {
            view.update(app, |view, cx| {
                let layout = view.large_icon_layout.as_ref().unwrap();
                let ix = 7 * layout.columns + 1;
                let before = (layout.row_bounds(5).unwrap().top + 19.0)
                    .min(view.scrollbar_metrics().unwrap().scroll_max);
                let attempted = view.entries[ix].path.clone();
                let previous = view.entries[ix - 1].path.clone();
                view.set_scroll_offset(before);
                view.pending_deleted_paths = vec![attempted.clone()];
                view.filter_pending_deleted_entries();
                view.restore_selection_from_paths(&[previous]);
                view.reveal_selection_after_delete();
                cx.notify();
                (attempted, before)
            })
        });
        cx.run_until_parked();
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.complete_trash_operation(
                    temp.path().to_path_buf(),
                    vec![attempted.clone()],
                    None,
                    Err("Failed to trash the selected file.".to_owned()),
                    cx,
                );
            });
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.selected_paths(), vec![attempted]);
            super::super::test_support::assert_approx_eq(
                view.scrollbar_metrics().unwrap().scroll_top,
                before,
            );
        });
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
                assert_eq!(view.content_branch(), ExplorerContentBranch::List);
                assert_eq!(view.entries.len(), 1);
                assert_eq!(view.entries[0].name, "created.txt");
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

    #[test]
    fn compression_summary_selects_archive_and_records_created_file_undo() {
        let temp = TempDir::new();
        let source = temp.path().join("notes.txt");
        fs::write(&source, b"notes").expect("create source");
        let prepared = prepare_compress_paths(std::slice::from_ref(&source)).unwrap();
        let PreparedFileOperation::Ready(job) = prepared else {
            panic!("compression should not conflict");
        };
        let summary = execute_file_operation(job, ConflictChoice::Replace).unwrap();
        let archive = summary.destination_paths[0].clone();
        let mut view = ExplorerView::new(temp.path().to_path_buf());

        view.finish_file_operation_for_test(summary);

        assert_eq!(selected_names(&view), vec!["notes.txt.zip"]);
        assert_eq!(view.file_operation_undo_stack.len(), 1);
        let undo = view.file_operation_undo_stack.last().cloned().unwrap();
        view.apply_file_operation_undo(undo)
            .expect("undo compression");
        assert!(!archive.exists());
        assert!(source.exists());
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

    #[gpui::test]
    fn successful_delete_selects_and_reveals_the_previous_row_in_a_large_folder(
        cx: &mut TestAppContext,
    ) {
        let temp = TempDir::new();
        for ix in 0..80 {
            fs::write(temp.path().join(format!("item-{ix:03}.txt")), b"file")
                .expect("create test file");
        }
        let deleted = temp.path().join("item-040.txt");
        let previous = temp.path().join("item-039.txt");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());
        super::super::operations::settle_for_test(cx);

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.restore_selection_from_paths(std::slice::from_ref(&deleted));
                view.pending_permanent_delete = Some(PendingPermanentDelete {
                    paths: vec![deleted.clone()],
                });
                view.confirm_pending_permanent_delete(cx);
            });
        });
        super::super::operations::settle_for_test(cx);

        assert!(!deleted.exists());
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.selected_paths(), vec![previous]);
            assert_eq!(view.selection.focused_index, Some(39));
            assert!(
                view.scrollbar_metrics()
                    .is_some_and(|metrics| metrics.scroll_top > 0.0),
                "the replacement selection should be revealed below the top of the list"
            );
        });
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
