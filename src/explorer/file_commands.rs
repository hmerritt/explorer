use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use gpui::{Context, Image, ImageFormat, Window};

use crate::explorer::{
    clipboard::{
        FileClipboard, FileClipboardOperation, clipboard_item_for_files, file_clipboard_from_item,
        image_clipboard_from_item,
    },
    filesystem::{
        ConflictChoice, FileOperationError, FileOperationJob, FileOperationSummary,
        PreparedFileOperation, archive_path_is_supported, execute_file_operation_with_progress,
        prepare_copy_paths_to_directory_for_paste, prepare_extract_archives_to_directory,
        prepare_move_paths_to_directory, remove_paths_permanently, trash_paths,
    },
    view::{ExplorerView, FileOperationState, PendingPermanentDelete, PendingTrash},
};

#[cfg(test)]
use crate::explorer::filesystem::{FileOperationOutcome, move_paths_to_directory};

const FILE_OPERATION_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

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

    fn create(self, path: &Path) -> io::Result<()> {
        match self {
            Self::Folder => fs::create_dir(path),
            Self::File => OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map(drop),
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
                self.open_error = None;
                self.reload_with_entry_metadata_resolution(cx);
                self.select_single_path(&path);
                self.start_rename_for_path(&path, window, cx);
                self.emit_filesystem_changed(cx);
            }
            Err(error) => {
                self.open_error = Some(error);
                self.reload_with_entry_metadata_resolution(cx);
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
                self.open_error = None;
            }
            Err(error) => self.open_error = Some(error),
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
                self.open_error = None;
            }
            Err(error) => self.open_error = Some(error),
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
                self.open_error = None;
                self.reload_with_entry_metadata_resolution(cx);
                self.select_single_path(&path);
                self.start_rename_for_path(&path, window, cx);
                self.emit_filesystem_changed(cx);
            }
            Err(error) => {
                self.open_error = Some(error);
                self.reload_with_entry_metadata_resolution(cx);
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

        match trash_paths(&paths) {
            Ok(()) => {
                self.remove_cut_paths(&paths);
                self.reload_with_entry_metadata_resolution(cx);
                self.clear_selection();
                self.open_error = None;
                self.emit_filesystem_changed(cx);
            }
            Err(error) => {
                self.open_error = Some(error);
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
        self.open_error = None;
        self.open_pending_dialog_window(cx);
    }

    pub(super) fn confirm_pending_trash(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_trash.take() else {
            return;
        };

        match trash_paths(&pending.paths) {
            Ok(()) => {
                self.remove_cut_paths(&pending.paths);
                self.reload_with_entry_metadata_resolution(cx);
                self.clear_selection();
                self.open_error = None;
                self.emit_filesystem_changed(cx);
            }
            Err(error) => {
                self.open_error = Some(error);
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
        self.open_error = None;
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
                self.open_error = None;
                self.emit_filesystem_changed(cx);
            }
            Err(error) => {
                self.open_error = Some(error);
                self.reload_with_entry_metadata_resolution(cx);
            }
        }
    }

    pub(super) fn cancel_pending_permanent_delete(&mut self) {
        self.pending_permanent_delete = None;
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
                self.finish_file_operation(summary);
            }
            Ok(FileOperationOutcome::Conflicts(conflicts)) => {
                self.pending_file_conflict = Some(conflicts);
                self.open_error = None;
            }
            Err(error) => {
                self.open_error = Some(error);
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
                self.open_error = None;
                self.open_pending_dialog_window(cx);
            }
            Err(error) => {
                self.open_error = Some(error);
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

    fn start_file_operation(
        &mut self,
        job: FileOperationJob,
        conflict_choice: ConflictChoice,
        cx: &mut Context<Self>,
    ) {
        if self.active_file_operation.is_some() {
            self.open_error = Some("Another file operation is already running.".to_owned());
            return;
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let progress = job.initial_progress();
        let archive_diagnostics = job.archive_diagnostics();
        self.active_file_operation = Some(FileOperationState {
            progress: progress.clone(),
            cancel: cancel.clone(),
            task: None,
            archive_diagnostics: archive_diagnostics.clone(),
        });
        self.open_error = None;
        self.open_file_operation_window(cx);
        if let Some(diagnostics) = &archive_diagnostics {
            diagnostics.mark_progress_dialog_visible();
        }

        let (progress_tx, progress_rx) = mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let task = cx.spawn({
            let cancel = cancel.clone();
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
                self.finish_file_operation(summary);
                let metadata_started = Instant::now();
                self.schedule_entry_metadata_resolution(cx);
                if let Some(diagnostics) = diagnostics {
                    diagnostics.add_metadata_resolution(metadata_started.elapsed());
                    diagnostics.finish("ok");
                }
                self.emit_filesystem_changed(cx);
            }
            Err(FileOperationError::Cancelled) => {
                self.open_error = None;
                self.reload_with_entry_metadata_resolution(cx);
            }
            Err(FileOperationError::Failed(error)) => {
                self.open_error = Some(error);
                self.reload_with_entry_metadata_resolution(cx);
            }
        }
    }

    fn finish_file_operation(&mut self, summary: FileOperationSummary) {
        let reload_started = Instant::now();
        self.open_error = None;
        self.remove_cut_paths(&summary.moved_source_paths);
        self.reload();
        self.restore_selection_from_paths(&summary.destination_paths);
        if let Some(diagnostics) = summary.archive_diagnostics {
            diagnostics.add_reload(reload_started.elapsed());
        }
    }
}

fn create_new_item_in_directory(parent: &Path, kind: NewItemKind) -> Result<PathBuf, String> {
    let mut index = 1usize;

    loop {
        let name = new_item_name(kind.base_name(), index);
        let path = parent.join(&name);

        if path.exists() {
            index = next_new_item_index(index, &name)?;
            continue;
        }

        match kind.create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
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
    let extension = image_format_extension(image.format());
    let mut index = 1usize;

    loop {
        let name = clipboard_image_file_name(extension, index);
        let path = parent.join(&name);

        if path.exists() {
            index = next_new_item_index(index, &name)?;
            continue;
        }

        match write_new_file(&path, image.bytes()) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                index = next_new_item_index(index, &name)?;
            }
            Err(error) => {
                return Err(format!("Could not create image \"{name}\": {error}"));
            }
        }
    }
}

fn write_new_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    if let Err(error) = file.write_all(bytes) {
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(())
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
        test_support::{TempDir, selected_names, test_view_with_entries},
    };
    use gpui::{Image, ImageFormat};
    use std::fs;

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
    fn only_cut_paths_are_dimmed() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);

        view.mark_cut_paths(&[PathBuf::from("b.txt")]);

        assert!(!view.entry_is_cut(Path::new("a.txt")));
        assert!(view.entry_is_cut(Path::new("b.txt")));
        view.clear_cut_paths();
        assert!(!view.entry_is_cut(Path::new("b.txt")));
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
}
