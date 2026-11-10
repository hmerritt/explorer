use super::{
    dialog::{
        DELETE_DIALOG_BUTTON_HEIGHT, DeleteDialogIconKind, PermanentDeleteDialogText,
        PermanentDeleteItemKind, delete_confirmation, delete_confirmation_size, dialog_button,
        shell_dialog_body,
    },
    trash::{self, BatchResult, RecoveryChoice, RecoveryRequest, RecoveryUndo, TrashItemId},
    view::ExplorerView,
};
use crate::dialog_height::DialogHeight;
use crate::settings::{FileColumnSettings, FileSortSettings, FileViewMode};
use gpui::{
    AppContext, Bounds, Context, FocusHandle, Focusable, IntoElement, MouseButton, Render,
    SharedString, Task, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, rgb, size,
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const DIALOG_WIDTH: f32 = 520.0;
const DIALOG_PADDING: f32 = 24.0;
const DIALOG_GAP: f32 = 14.0;
const DIALOG_TEXT_SIZE: f32 = 14.0;
const DIALOG_LINE_HEIGHT: f32 = 22.0;
const DIALOG_BUTTON_HEIGHT: f32 = 36.0;
const PROPERTIES_GAP: f32 = 10.0;
const PROPERTIES_SCROLL_SPACE: f32 = 16.0;
const PROPERTIES_MAX_HEIGHT: f32 = 360.0;
const PROGRESS_BAR_HEIGHT: f32 = 16.0;

pub(super) struct TrashRevision(pub(super) u64);
impl gpui::Global for TrashRevision {}

#[derive(Clone)]
pub(super) struct TrashPreferences {
    columns: FileColumnSettings,
    sort: FileSortSettings,
    view_mode: FileViewMode,
    location_sort: Option<crate::settings::SortDirection>,
}
pub(super) struct TrashViewState {
    selection: super::view::ViewModeSelection,
    pub(super) columns: FileColumnSettings,
    pub(super) sort: FileSortSettings,
    pub(super) view_mode: FileViewMode,
    pub(super) location_sort: Option<crate::settings::SortDirection>,
}

pub(super) struct TrashOperation {
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) task: Option<Task<()>>,
}
#[derive(Default)]
struct Progress {
    done: usize,
    total: usize,
    name: String,
    finished: bool,
}

#[derive(Clone)]
enum DialogKind {
    Purge(PurgeConfirmation),
    Conflict(RecoveryRequest),
    Properties(Vec<trash::TrashEntry>),
    Progress(Arc<Mutex<Progress>>, Arc<AtomicBool>),
}

#[derive(Clone)]
struct PurgeConfirmation {
    ids: Vec<TrashItemId>,
    text: PermanentDeleteDialogText,
}

impl PurgeConfirmation {
    fn new(ids: Vec<TrashItemId>, date_format: &str) -> Self {
        // Capture display metadata with the IDs; original paths may no longer exist.
        let item = if let [id] = ids.as_slice() {
            trash::cached_by_id(id)
        } else {
            None
        };
        Self::from_item(ids, item.as_ref(), date_format)
    }

    fn from_item(
        ids: Vec<TrashItemId>,
        item: Option<&trash::TrashEntry>,
        date_format: &str,
    ) -> Self {
        let mut text = PermanentDeleteDialogText {
            message: format!(
                "Are you sure you want to permanently delete these {} items?",
                ids.len()
            ),
            item_name: None,
            item_kind: None,
            size_label: None,
            date_label: None,
            folder_size_path: None,
        };
        if ids.len() == 1 {
            let noun = match item {
                Some(item) if item.directory => "folder",
                Some(_) => "file",
                None => "item",
            };
            text.message = format!("Are you sure you want to permanently delete this {noun}?");
            text.item_name = Some(item.map_or_else(
                || "Unknown".into(),
                |item| item.name.to_string_lossy().into_owned(),
            ));
            text.item_kind = item.map(|item| {
                if item.directory {
                    PermanentDeleteItemKind::Folder
                } else {
                    PermanentDeleteItemKind::File
                }
            });
            let size = item.and_then(|item| item.size).map_or_else(
                || "Unknown".into(),
                |size| super::formatting::format_size(Some(size)),
            );
            text.size_label = Some(format!("Size: {size}"));
            let deleted = item.and_then(|item| item.deleted).map_or_else(
                || "Unknown".into(),
                |deleted| super::formatting::format_timestamp(Some(deleted), date_format),
            );
            text.date_label = Some(format!("Date deleted: {deleted}"));
        }
        Self { ids, text }
    }

    fn icon_kind(&self) -> DeleteDialogIconKind {
        if self.ids.len() > 1 {
            DeleteDialogIconKind::MultipleFiles
        } else {
            DeleteDialogIconKind::File
        }
    }
}

struct TrashDialog {
    kind: DialogKind,
    explorer: WeakEntity<ExplorerView>,
    focus: FocusHandle,
    completed: bool,
    serial: u64,
    focused_choice: usize,
    poll: Option<Task<()>>,
}

impl ExplorerView {
    pub(super) fn is_trash_view(&self) -> bool {
        self.sidebar_group_view.is_none() && trash::is_root(&self.path)
    }

    pub(super) fn sync_trash_view_settings(&mut self) {
        if self.is_trash_view() && self.trash_view.is_none() {
            self.trash_view = Some(TrashViewState {
                selection: self.view_mode_selection,
                columns: self.file_columns.clone(),
                sort: self.file_sort,
                view_mode: self.view_mode,
                location_sort: None,
            });
            self.file_columns = FileColumnSettings::default();
            self.file_sort = FileSortSettings::default();
            self.view_mode = FileViewMode::Details;
            self.view_mode_selection = super::view::ViewModeSelection::Manual;
            if let Some(preferences) = &self.trash_preferences {
                self.file_columns = preferences.columns.clone();
                self.file_sort = preferences.sort;
                self.view_mode = preferences.view_mode;
                self.trash_view.as_mut().unwrap().location_sort = preferences.location_sort;
            }
        } else if !self.is_trash_view()
            && let Some(state) = self.trash_view.take()
        {
            self.trash_preferences = Some(TrashPreferences {
                columns: self.file_columns.clone(),
                sort: self.file_sort,
                view_mode: self.view_mode,
                location_sort: state.location_sort,
            });
            self.file_columns = state.columns;
            self.file_sort = state.sort;
            self.view_mode = state.view_mode;
            self.view_mode_selection = state.selection;
        }
    }

    pub(super) fn sort_trash_locations(&mut self, cx: &mut Context<Self>) {
        use crate::settings::SortDirection;
        let selected = self.selected_paths();
        if let Some(state) = self.trash_view.as_mut() {
            state.location_sort = Some(match state.location_sort {
                Some(SortDirection::Ascending) => SortDirection::Descending,
                _ => SortDirection::Ascending,
            });
        }
        self.apply_trash_location_sort();
        self.restore_selection_from_paths(&selected);
        cx.notify();
    }
    pub(super) fn apply_trash_location_sort(&mut self) {
        let selected = self.selected_paths();
        if let Some(direction) = self.trash_view.as_ref().and_then(|s| s.location_sort) {
            let compare = |a: &super::entry::FileEntry, b: &super::entry::FileEntry| {
                let left = trash::cached(&a.path)
                    .map(|e| e.original_location())
                    .unwrap_or_default();
                let right = trash::cached(&b.path)
                    .map(|e| e.original_location())
                    .unwrap_or_default();
                let ordering = super::sorting::compare_file_names(&left, &right)
                    .then_with(|| super::sorting::compare_file_names(&a.name, &b.name));
                if direction == crate::settings::SortDirection::Descending {
                    ordering.reverse()
                } else {
                    ordering
                }
            };
            self.entries.sort_by(compare);
            self.invalidate_visible_entries();
            self.all_entries.sort_by(compare);
            self.restore_selection_from_paths(&selected);
        }
    }

    pub(super) fn restore_bin_selected(&mut self, cx: &mut Context<Self>) {
        self.prepare_bin_recovery(
            RecoveryRequest {
                ids: trash::ids(&self.selected_paths()),
                directory: None,
            },
            cx,
        );
    }
    pub(super) fn empty_bin(&mut self, cx: &mut Context<Self>) {
        if self.has_background_operation() {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { trash::list() })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.pending_trash_task = None;
                match result {
                    Ok(items) => {
                        let ids: Vec<_> = items.into_iter().map(|e| e.id).collect();
                        if !ids.is_empty() {
                            view.show_bin_dialog(
                                DialogKind::Purge(PurgeConfirmation::new(ids, &view.date_format)),
                                cx,
                            );
                        }
                    }
                    Err(error) => view.set_error_notice(error),
                }
                cx.notify();
            });
        });
        self.pending_trash_task = Some(task);
    }

    pub(super) fn restore_bin_to_folder(&mut self, cx: &mut Context<Self>) {
        let ids = trash::ids(&self.selected_paths());
        if ids.is_empty() || self.has_background_operation() {
            return;
        }
        let prompt = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose restore folder".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = prompt.await
                && let Some(directory) = paths.into_iter().next()
            {
                let _ = this.update(cx, |view, cx| {
                    view.prepare_bin_recovery(
                        RecoveryRequest {
                            ids,
                            directory: Some(directory),
                        },
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    pub(super) fn prepare_bin_recovery(
        &mut self,
        request: RecoveryRequest,
        cx: &mut Context<Self>,
    ) {
        if request.ids.is_empty() || self.has_background_operation() {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            let request_for_check = request.clone();
            let result = cx
                .background_executor()
                .spawn(async move { trash::has_conflicts(&request_for_check) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.pending_trash_task = None;
                match result {
                    Ok(true) => view.show_bin_dialog(DialogKind::Conflict(request), cx),
                    Ok(false) => view.start_bin_recovery(request, RecoveryChoice::Skip, cx),
                    Err(error) => view.set_error_notice(error),
                }
                cx.notify();
            });
        });
        self.pending_trash_task = Some(task);
    }

    pub(super) fn request_bin_delete(&mut self, cx: &mut Context<Self>) {
        let ids = trash::ids(&self.selected_paths());
        if !ids.is_empty() && !self.has_background_operation() {
            self.show_bin_dialog(
                DialogKind::Purge(PurgeConfirmation::new(ids, &self.date_format)),
                cx,
            );
        }
    }

    pub(super) fn open_bin_properties(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let items: Vec<_> = paths.iter().filter_map(|p| trash::cached(p)).collect();
        if !items.is_empty() {
            self.show_bin_dialog(DialogKind::Properties(items), cx);
        }
    }

    fn show_bin_dialog(&mut self, kind: DialogKind, cx: &mut Context<Self>) {
        self.close_context_menu();
        if self.active_dialog_window.is_some() {
            return;
        }
        self.trash_dialog_serial = self.trash_dialog_serial.wrapping_add(1);
        let serial = self.trash_dialog_serial;
        let focused_choice = match &kind {
            DialogKind::Conflict(_) => 3,
            _ => 0,
        };
        let title = match &kind {
            DialogKind::Purge(_) => "Delete permanently",
            DialogKind::Conflict(_) => "Restore files",
            DialogKind::Properties(_) => "Properties",
            DialogKind::Progress(_, _) => "File operation",
        };
        let (width, height) = dialog_size(&kind, cx);
        let mut options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(format!("{} — {title}", trash::label()).into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(width), px(height)),
                cx,
            ))),
            is_resizable: false,
            is_minimizable: false,
            ..Default::default()
        };
        self.dialog_placement.apply(&mut options);
        let explorer = cx.entity().downgrade();
        match cx.open_window(options, |window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window);
            cx.new(|cx| {
                cx.on_release(|dialog: &mut TrashDialog, cx| {
                    if !dialog.completed
                        && let DialogKind::Progress(_, cancel) = &dialog.kind
                    {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    if !dialog.completed {
                        let _ = dialog.explorer.update(cx, |view, _| {
                            if view.trash_dialog_serial == dialog.serial {
                                view.active_dialog_window = None;
                            }
                        });
                    }
                })
                .detach();
                let poll = if matches!(kind, DialogKind::Progress(_, _)) {
                    Some(cx.spawn(async move |this, cx| {
                        loop {
                            cx.background_executor()
                                .timer(Duration::from_millis(100))
                                .await;
                            if this
                                .update(cx, |dialog, cx| {
                                    cx.notify();
                                    match &dialog.kind {
                                        DialogKind::Progress(progress, _) => {
                                            !progress.lock().unwrap().finished
                                        }
                                        _ => false,
                                    }
                                })
                                .unwrap_or(false)
                                == false
                            {
                                break;
                            }
                        }
                    }))
                } else {
                    None
                };
                TrashDialog {
                    kind,
                    explorer,
                    focus,
                    completed: false,
                    serial,
                    focused_choice,
                    poll,
                }
            })
        }) {
            Ok(handle) => self.active_dialog_window = Some(handle.into()),
            Err(error) => self.set_error_notice(format!("Could not open dialog: {error}")),
        }
    }

    fn start_bin_recovery(
        &mut self,
        request: RecoveryRequest,
        choice: RecoveryChoice,
        cx: &mut Context<Self>,
    ) {
        self.start_bin_job(Some((request, choice)), Vec::new(), cx);
    }
    fn start_bin_job(
        &mut self,
        recovery: Option<(RecoveryRequest, RecoveryChoice)>,
        purge: Vec<TrashItemId>,
        cx: &mut Context<Self>,
    ) {
        if self.has_background_operation() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(Progress::default()));
        self.trash_operation = Some(TrashOperation {
            cancel: cancel.clone(),
            task: None,
        });
        self.show_bin_dialog(DialogKind::Progress(progress.clone(), cancel.clone()), cx);
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let reported_progress = progress.clone();
                    let report = move |done, total, name: &str| {
                        let mut state = reported_progress.lock().unwrap();
                        state.done = done;
                        state.total = total;
                        state.name = name.into();
                    };
                    let result = if let Some((request, choice)) = recovery {
                        trash::recover(request, choice, &cancel, report)
                    } else {
                        trash::purge(purge, cancel, report)
                    };
                    progress.lock().unwrap().finished = true;
                    result
                })
                .await;
            let _ = this.update(cx, |view, cx| view.finish_bin_job(result, cx));
        });
        self.trash_operation.as_mut().unwrap().task = Some(task);
    }

    fn finish_bin_job(&mut self, result: BatchResult, cx: &mut Context<Self>) {
        self.trash_operation = None;
        if let Some(handle) = self.active_dialog_window.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
        if !result.undo.paths.is_empty() {
            self.push_file_operation_undo(Some(super::file_commands::FileOperationUndo::Recovery(
                result.undo,
            )));
        }
        self.reconcile_bin_clipboard(cx);
        self.reconcile_bin_undo();
        self.reload_async_with_entry_metadata_resolution(cx);
        self.emit_filesystem_changed(cx);
        let message = format!(
            "{} completed, {} skipped{}",
            result.completed.len(),
            result.skipped.len(),
            if result.cancelled { "; cancelled" } else { "" }
        );
        if result.failures.is_empty() {
            self.set_info_notice(message);
        } else {
            self.set_error_notice(format!("{message}. {}", result.failures.join("\n")));
        }
        cx.notify();
    }

    pub(super) fn reconcile_bin_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let Some(clipboard) = super::clipboard::file_clipboard_from_item(&item) else {
            return;
        };
        if !clipboard.paths.iter().all(|p| trash::is_item(p)) {
            return;
        }
        let remaining: Vec<_> = clipboard
            .paths
            .into_iter()
            .filter(|p| trash::cached(p).is_some())
            .collect();
        if remaining.is_empty() {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(String::new()));
        } else if let Ok(item) =
            super::clipboard::clipboard_item_for_files(&super::clipboard::FileClipboard::new(
                super::clipboard::FileClipboardOperation::Cut,
                remaining,
            ))
        {
            cx.write_to_clipboard(item);
        }
    }

    pub(super) fn reconcile_bin_undo(&mut self) {
        self.file_operation_undo_stack.retain_mut(|undo| {
            if let super::file_commands::FileOperationUndo::Trash(
                super::file_commands::TrashUndo::Native { ids, .. },
            ) = undo
            {
                ids.retain(trash::available);
                !ids.is_empty()
            } else {
                true
            }
        });
    }

    pub(super) fn undo_bin_delete(
        &mut self,
        ids: Vec<TrashItemId>,
        original_paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    trash::recover(
                        RecoveryRequest {
                            ids,
                            directory: None,
                        },
                        RecoveryChoice::Skip,
                        &AtomicBool::new(false),
                        |_, _, _| {},
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.pending_trash_task = None;
                trash::cleanup_undo(result.undo);
                view.reconcile_bin_undo();
                view.reload_async_with_options(
                    super::view::ReloadMode {
                        cache_policy: super::remote_directory_cache::DirectoryLoadPolicy::Fresh,
                        preserve_selection: true,
                        rebuild_sidebar: true,
                        preserve_context_menu: false,
                    },
                    original_paths,
                    true,
                    false,
                    false,
                    cx,
                );
                if !result.failures.is_empty() || !result.skipped.is_empty() {
                    view.set_error_notice(format!(
                        "Some deleted items could not be restored. {}",
                        result.failures.join("\n")
                    ));
                }
                view.emit_filesystem_changed(cx);
                cx.notify();
            });
        });
        self.pending_trash_task = Some(task);
    }

    pub(super) fn undo_bin_recovery(&mut self, mut undo: RecoveryUndo, cx: &mut Context<Self>) {
        if self.has_background_operation() {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            let (undo, result) = cx
                .background_executor()
                .spawn(async move {
                    let result = trash::undo_recovery(&mut undo);
                    (undo, result)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.pending_trash_task = None;
                match result {
                    Ok(()) => {
                        view.file_operation_undo_stack.pop();
                        view.clear_operation_notice();
                    }
                    Err(error) => {
                        if let Some(last) = view.file_operation_undo_stack.last_mut() {
                            *last = super::file_commands::FileOperationUndo::Recovery(undo);
                        }
                        view.set_error_notice(error);
                    }
                }
                view.reload_async_with_entry_metadata_resolution(cx);
                view.emit_filesystem_changed(cx);
                cx.notify();
            });
        });
        self.pending_trash_task = Some(task);
    }
}

impl TrashDialog {
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.completed = true;
        let _ = self
            .explorer
            .update(cx, |view, _| view.active_dialog_window = None);
        window.remove_window();
    }
    fn choose(&mut self, choice: RecoveryChoice, window: &mut Window, cx: &mut Context<Self>) {
        let kind = self.kind.clone();
        self.close(window, cx);
        let _ = self.explorer.update(cx, |view, cx| match kind {
            DialogKind::Purge(pending) => view.start_bin_job(None, pending.ids, cx),
            DialogKind::Conflict(request) => view.start_bin_recovery(request, choice, cx),
            _ => {}
        });
    }
}
impl Focusable for TrashDialog {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for TrashDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = &self.poll;
        let font = crate::settings::current_app_font(cx);
        let root = if matches!(self.kind, DialogKind::Purge(_)) {
            shell_dialog_body(font.clone())
        } else {
            div()
                .size_full()
                .p(px(DIALOG_PADDING))
                .font(font.clone())
                .text_size(px(DIALOG_TEXT_SIZE))
                .line_height(px(DIALOG_LINE_HEIGHT))
                .bg(rgb(0xffffff))
        }
        .id("trash-dialog")
        .debug_selector(|| "trash-dialog".into())
        .key_context("ExplorerDialog")
        .track_focus(&self.focus)
        .on_action(
            cx.listener(|this, _: &super::dialog::DialogCancel, window, cx| {
                if let DialogKind::Progress(_, cancel) = &this.kind {
                    cancel.store(true, Ordering::Relaxed);
                }
                this.close(window, cx);
            }),
        )
        .on_action(
            cx.listener(|this, _: &super::dialog::DialogConfirm, window, cx| {
                match &this.kind {
                    DialogKind::Purge(_) if this.focused_choice == 0 => {
                        this.choose(RecoveryChoice::Replace, window, cx)
                    }
                    DialogKind::Conflict(_) if this.focused_choice < 3 => this.choose(
                        [
                            RecoveryChoice::Replace,
                            RecoveryChoice::Skip,
                            RecoveryChoice::KeepBoth,
                        ][this.focused_choice],
                        window,
                        cx,
                    ),
                    DialogKind::Progress(_, cancel) => {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    _ => this.close(window, cx),
                }
                cx.stop_propagation();
            }),
        )
        .on_action(
            cx.listener(|this, _: &super::dialog::DialogFocusPrimary, _, cx| {
                this.focused_choice = this.focused_choice.saturating_sub(1);
                cx.notify();
                cx.stop_propagation();
            }),
        )
        .on_action(
            cx.listener(|this, _: &super::dialog::DialogFocusSecondary, _, cx| {
                let max = match this.kind {
                    DialogKind::Purge(_) => 1,
                    DialogKind::Conflict(_) => 3,
                    _ => 0,
                };
                this.focused_choice = (this.focused_choice + 1).min(max);
                cx.notify();
                cx.stop_propagation();
            }),
        );
        let column = || div().flex().flex_col().gap(px(DIALOG_GAP));
        let (body, actions) = match &self.kind {
            DialogKind::Purge(pending) => delete_confirmation(
                pending.text.clone(),
                pending.icon_kind(),
                &font,
                window,
                dialog_button(
                    "trash-confirm-delete",
                    "Yes",
                    self.focused_choice == 0,
                    window.scale_factor(),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.focused_choice = 0;
                        cx.notify();
                    }),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.choose(RecoveryChoice::Replace, window, cx);
                    cx.stop_propagation();
                })),
                dialog_button(
                    "trash-cancel",
                    "No",
                    self.focused_choice == 1,
                    window.scale_factor(),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.focused_choice = 1;
                        cx.notify();
                    }),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close(window, cx);
                    cx.stop_propagation();
                })),
            ),
            DialogKind::Conflict(_) => {
                let body = column()
                    .child(dialog_text(
                        "The destination already contains items with the same names.",
                    ))
                    .child(dialog_text("Choose how to restore conflicting items:"));
                let mut actions = column().mt(px(DIALOG_GAP));
                for (index, (id, label, choice)) in [
                    (
                        "trash-conflict-replace",
                        "Replace files in the destination",
                        RecoveryChoice::Replace,
                    ),
                    (
                        "trash-conflict-skip",
                        "Skip these files",
                        RecoveryChoice::Skip,
                    ),
                    (
                        "trash-conflict-keep-both",
                        "Keep both",
                        RecoveryChoice::KeepBoth,
                    ),
                ]
                .into_iter()
                .enumerate()
                {
                    actions = actions.child(button(
                        id,
                        label,
                        self.focused_choice == index,
                        cx.listener(move |this, _, window, cx| this.choose(choice, window, cx)),
                    ));
                }
                actions = actions.child(button(
                    "trash-cancel",
                    "Cancel",
                    self.focused_choice == 3,
                    cx.listener(|this, _, window, cx| this.close(window, cx)),
                ));
                (body.into_any_element(), Some(actions.into_any_element()))
            }
            DialogKind::Properties(items) => {
                let mut details = div()
                    .id("trash-properties-details")
                    .debug_selector(|| "trash-properties-details".into())
                    .pr(px(PROPERTIES_SCROLL_SPACE))
                    .flex()
                    .flex_col()
                    .gap(px(PROPERTIES_GAP));
                for item in items {
                    for line in property_lines(item) {
                        details = details.child(dialog_text(line));
                    }
                }
                let actions = div().mt(px(DIALOG_GAP)).child(button(
                    "trash-properties-close",
                    "Close",
                    true,
                    cx.listener(|this, _, window, cx| this.close(window, cx)),
                ));
                (details.into_any_element(), Some(actions.into_any_element()))
            }
            DialogKind::Progress(progress, _) => {
                let progress = progress.lock().unwrap();
                let fraction = if progress.total == 0 {
                    0.0
                } else {
                    progress.done as f32 / progress.total as f32
                };
                let body = column()
                    .child(dialog_text("Working…"))
                    .child(
                        div()
                            .debug_selector(|| "trash-progress-track".into())
                            .w_full()
                            .h(px(PROGRESS_BAR_HEIGHT))
                            .flex_shrink_0()
                            .border_1()
                            .border_color(rgb(0xcccccc))
                            .child(
                                div()
                                    .debug_selector(|| "trash-progress-fill".into())
                                    .w(gpui::relative(fraction.clamp(0.0, 1.0)))
                                    .h_full()
                                    .bg(rgb(super::constants::EXPLORER_COPY_GREEN)),
                            ),
                    )
                    .child(dialog_text(format!(
                        "{} of {} items",
                        progress.done, progress.total
                    )))
                    .child(
                        div()
                            .debug_selector(|| "trash-progress-name".into())
                            .w_full()
                            .h(px(DIALOG_LINE_HEIGHT))
                            .flex_shrink_0()
                            .truncate()
                            .child(progress.name.clone()),
                    );
                let actions = div()
                    .mt(px(DIALOG_GAP))
                    .w_full()
                    .flex()
                    .justify_end()
                    .child(
                        dialog_button(
                            "trash-progress-cancel",
                            "Cancel",
                            true,
                            window.scale_factor(),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let DialogKind::Progress(_, cancel) = &this.kind {
                                cancel.store(true, Ordering::Relaxed);
                            }
                            cx.stop_propagation();
                        })),
                    );
                (body.into_any_element(), Some(actions.into_any_element()))
            }
        };
        let cap =
            matches!(self.kind, DialogKind::Properties(_)).then_some(px(PROPERTIES_MAX_HEIGHT));
        root.child(DialogHeight::new(window, cx, false, cap).layout(body, actions))
    }
}
fn dialog_text(text: impl Into<SharedString>) -> gpui::Div {
    div().w_full().flex_shrink_0().child(text.into())
}

fn property_lines(item: &trash::TrashEntry) -> [String; 4] {
    [
        item.name.to_string_lossy().into_owned(),
        format!("Original location: {}", item.original_location()),
        format!(
            "Date deleted: {}",
            item.deleted
                .map(|date| super::formatting::format_timestamp(
                    Some(date),
                    crate::settings::DEFAULT_DATE_FORMAT
                ))
                .unwrap_or_else(|| "Unknown".into())
        ),
        format!(
            "Type: {}    Size: {}",
            item.file_entry().type_label(),
            super::formatting::format_size(item.size)
        ),
    ]
}

fn dialog_text_height(text: &str, width: f32, cx: &gpui::App) -> f32 {
    let font = crate::settings::current_app_font(cx);
    let mut wrapper = cx.text_system().line_wrapper(font, px(DIALOG_TEXT_SIZE));
    text.split('\n')
        .map(|line| {
            let fragments = [gpui::LineFragment::text(line)];
            (wrapper.wrap_line(&fragments, px(width)).count() + 1) as f32 * DIALOG_LINE_HEIGHT
        })
        .sum()
}

fn dialog_size(kind: &DialogKind, cx: &gpui::App) -> (f32, f32) {
    let width = DIALOG_WIDTH - DIALOG_PADDING * 2.0;
    let text_height = |text: &str| dialog_text_height(text, width, cx);
    let content = match kind {
        DialogKind::Purge(pending) => {
            return delete_confirmation_size(
                &pending.text,
                &crate::settings::current_app_font(cx),
                cx,
            );
        }
        DialogKind::Conflict(_) => {
            text_height("The destination already contains items with the same names.")
                + text_height("Choose how to restore conflicting items:")
                + DIALOG_BUTTON_HEIGHT * 4.0
                + DIALOG_GAP * 5.0
        }
        DialogKind::Progress(_, _) => {
            DIALOG_LINE_HEIGHT * 3.0
                + PROGRESS_BAR_HEIGHT
                + DELETE_DIALOG_BUTTON_HEIGHT
                + DIALOG_GAP * 4.0
        }
        DialogKind::Properties(items) => {
            let lines: Vec<_> = items.iter().flat_map(property_lines).collect();
            let details: f32 = lines
                .iter()
                .map(|line| dialog_text_height(line, width - PROPERTIES_SCROLL_SPACE, cx))
                .sum();
            details
                + PROPERTIES_GAP * lines.len().saturating_sub(1) as f32
                + DIALOG_GAP
                + DIALOG_BUTTON_HEIGHT
        }
    };
    let height = DIALOG_PADDING * 2.0 + content;
    let height = if matches!(kind, DialogKind::Properties(_)) {
        height.min(PROPERTIES_MAX_HEIGHT)
    } else {
        height
    };
    (DIALOG_WIDTH, height)
}

fn button(
    id: &'static str,
    label: &'static str,
    focused: bool,
    click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .px(px(12.0))
        .h(px(DIALOG_BUTTON_HEIGHT))
        .flex_shrink_0()
        .flex()
        .items_center()
        .border_1()
        .border_color(rgb(if focused { 0x0078d4 } else { 0xcccccc }))
        .hover(|style| style.bg(rgb(0xe5f3ff)))
        .cursor_pointer()
        .child(label)
        .on_click(click)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        explorer::{
            entry::FileEntry, navigation::HistoryMode, test_support::TempDir,
            view::NavigationLocation,
        },
        settings::{ExplorerSettings, FileSortColumn, SettingsState, SortDirection},
    };
    fn bin_view(focus: Option<FocusHandle>) -> ExplorerView {
        let mut view = ExplorerView::new_unloaded_with_settings_for_test(
            trash::root(),
            focus,
            &ExplorerSettings::default(),
        );
        view.sync_trash_view_settings();
        let path = PathBuf::from(format!("{}items/{}", trash::ADDRESS, "a".repeat(64)));
        view.entries = vec![FileEntry::from_provider(
            path,
            "deleted folder".into(),
            true,
            None,
            None,
        )];
        view.all_entries = view.entries.clone();
        view.restore_selection_from_paths(&[view.entries[0].path.clone()]);
        view
    }
    #[test]
    fn purge_confirmation_uses_bin_metadata_and_configured_date_format() {
        let temp = TempDir::new();
        let payload = temp.path().join("payload");
        std::fs::write(&payload, b"deleted").unwrap();
        // The original file can be absent or replaced without affecting the details.
        let original = temp.path().join("original.txt");
        std::fs::write(&original, b"a different file with a different size").unwrap();
        let mut item = trash::fixture_entry("deleted.txt", &payload, Some(original));
        item.deleted = Some(std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000));
        let pending = PurgeConfirmation::from_item(vec![item.id.clone()], Some(&item), "%Y");
        assert_eq!(
            pending.text.message,
            "Are you sure you want to permanently delete this file?"
        );
        assert_eq!(pending.text.item_name.as_deref(), Some("deleted.txt"));
        assert_eq!(pending.text.size_label.as_deref(), Some("Size: 7 bytes"));
        assert_eq!(
            pending.text.date_label.as_deref(),
            Some("Date deleted: 2023")
        );
        assert!(pending.text.folder_size_path.is_none());
        assert_eq!(pending.icon_kind(), DeleteDialogIconKind::File);
    }

    #[test]
    fn purge_confirmation_uses_file_icon_for_folder_and_unknown_metadata() {
        let temp = TempDir::new();
        let mut item = trash::fixture_entry("deleted folder", temp.path(), None);
        item.size = None;
        let pending = PurgeConfirmation::from_item(
            vec![item.id.clone()],
            Some(&item),
            crate::settings::DEFAULT_DATE_FORMAT,
        );
        assert_eq!(
            pending.text.message,
            "Are you sure you want to permanently delete this folder?"
        );
        assert_eq!(
            pending.text.item_kind,
            Some(PermanentDeleteItemKind::Folder)
        );
        assert_eq!(pending.text.size_label.as_deref(), Some("Size: Unknown"));
        assert_eq!(
            pending.text.date_label.as_deref(),
            Some("Date deleted: Unknown")
        );
        assert_eq!(pending.icon_kind(), DeleteDialogIconKind::File);
        let missing = PurgeConfirmation::from_item(vec![item.id], None, "%Y");
        assert_eq!(
            missing.text.message,
            "Are you sure you want to permanently delete this item?"
        );
        assert_eq!(missing.text.item_name.as_deref(), Some("Unknown"));
        assert_eq!(missing.text.size_label.as_deref(), Some("Size: Unknown"));
        assert_eq!(
            missing.text.date_label.as_deref(),
            Some("Date deleted: Unknown")
        );
    }

    #[test]
    fn purge_confirmation_keeps_metadata_after_bin_snapshot_changes() {
        let temp = TempDir::new();
        let payload = temp.path().join("payload");
        std::fs::write(&payload, b"deleted").unwrap();
        let item = trash::fixture_entry("deleted.txt", &payload, None);
        let pending = {
            let _snapshot = trash::SnapshotFixture::new(vec![item.clone()]);
            PurgeConfirmation::new(vec![item.id.clone()], "%Y")
        };
        assert_eq!(pending.ids, vec![item.id]);
        assert_eq!(pending.text.item_name.as_deref(), Some("deleted.txt"));
        assert_eq!(pending.text.size_label.as_deref(), Some("Size: 7 bytes"));
    }

    #[test]
    fn purge_confirmation_multiple_items_uses_count_and_stacked_files_icon() {
        for count in [2, 36] {
            let ids = (0..count)
                .map(|index| TrashItemId(format!("{index:064x}")))
                .collect();
            let pending = PurgeConfirmation::from_item(ids, None, "%Y");
            assert_eq!(
                pending.text.message,
                format!("Are you sure you want to permanently delete these {count} items?")
            );
            assert!(pending.text.item_name.is_none());
            assert!(pending.text.size_label.is_none());
            assert!(pending.text.date_label.is_none());
            assert_eq!(pending.icon_kind(), DeleteDialogIconKind::MultipleFiles);
        }
    }

    #[gpui::test]
    fn purge_confirmation_fits_long_names_and_wrapped_prompts(cx: &mut gpui::TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let explorer = cx.new(|cx| bin_view(Some(cx.focus_handle())));
        let temp = TempDir::new();
        let payload = temp.path().join("payload");
        std::fs::write(&payload, b"deleted").unwrap();
        let item = trash::fixture_entry(&"long filename ".repeat(100), &payload, None);
        let single = PurgeConfirmation::from_item(vec![item.id.clone()], Some(&item), "%Y");
        let multiple = PurgeConfirmation::from_item(vec![item.id.clone(); 36], None, "%Y");
        let mut wrapped = multiple.clone();
        wrapped.text.message =
            "Are you sure you want to permanently delete these items? ".repeat(8);
        let mut heights = Vec::new();
        for (index, pending) in [single, multiple, wrapped].into_iter().enumerate() {
            let has_name = pending.text.item_name.is_some();
            let kind = DialogKind::Purge(pending);
            let (width, height) = cx.update(|cx| dialog_size(&kind, cx));
            heights.push(height);
            let (dialog, window) = cx.add_window_view(|window, cx| {
                let focus = cx.focus_handle();
                focus.focus(window);
                TrashDialog {
                    kind,
                    explorer: explorer.downgrade(),
                    focus,
                    completed: false,
                    serial: 0,
                    focused_choice: 0,
                    poll: None,
                }
            });
            window.simulate_resize(size(px(width), px(height)));
            window.run_until_parked();
            let icon = window.debug_bounds("delete-confirmation-icon").unwrap();
            let message = window.debug_bounds("delete-confirmation-message").unwrap();
            let yes = window.debug_bounds("trash-confirm-delete").unwrap();
            let no = window.debug_bounds("trash-cancel").unwrap();
            assert_eq!(icon.left(), px(20.0));
            assert_eq!(icon.size.width, px(40.0));
            assert_eq!(message.left() - icon.right(), px(16.0));
            assert!(message.bottom() < yes.top());
            assert_eq!(yes.size, size(px(84.0), px(28.0)));
            assert_eq!(yes.top(), no.top());
            assert_eq!(no.right(), px(width - 20.0));
            assert!(no.bottom() <= px(height - 14.0));
            let name = window.debug_bounds("delete-confirmation-name");
            assert_eq!(name.is_some(), has_name);
            if let Some(name) = name {
                assert_eq!(name.left(), message.left());
                assert!(name.right() <= px(width - 20.0));
                assert!(
                    name.size.height <= px(20.0),
                    "filename should fit on one line"
                );
            }
            window.dispatch_action(super::super::dialog::DialogFocusSecondary);
            window.read_entity(&dialog, |dialog, _| assert_eq!(dialog.focused_choice, 1));
            window.dispatch_action(super::super::dialog::DialogFocusPrimary);
            window.read_entity(&dialog, |dialog, _| assert_eq!(dialog.focused_choice, 0));
            match index {
                0 => {
                    let no_center = no.center();
                    window.simulate_mouse_down(
                        no_center,
                        MouseButton::Left,
                        gpui::Modifiers::default(),
                    );
                    window.read_entity(&dialog, |dialog, _| assert_eq!(dialog.focused_choice, 1));
                    window.simulate_mouse_up(
                        no_center,
                        MouseButton::Left,
                        gpui::Modifiers::default(),
                    );
                }
                1 => {
                    window.dispatch_action(super::super::dialog::DialogFocusSecondary);
                    window.dispatch_action(super::super::dialog::DialogConfirm);
                }
                _ => window.dispatch_action(super::super::dialog::DialogCancel),
            }
            window.read_entity(&dialog, |dialog, _| assert!(dialog.completed));
        }
        assert!(
            heights[0] > heights[1],
            "single-item details require more height"
        );
        assert!(heights[2] > heights[1], "wrapped prompt expands the dialog");
        assert_eq!(std::fs::read(payload).unwrap(), b"deleted");
        cx.read_entity(&explorer, |view, _| assert!(view.trash_operation.is_none()));
    }

    #[test]
    fn bin_defaults_and_preferences_preserve_ordinary_folder_settings() {
        let temp = TempDir::new();
        let mut view = ExplorerView::new_unloaded_with_settings_for_test(
            temp.path().to_owned(),
            None,
            &ExplorerSettings::default(),
        );
        view.file_sort = FileSortSettings {
            column: FileSortColumn::Size,
            direction: SortDirection::Descending,
        };
        view.file_columns.name_width = Some(333);
        view.view_mode = FileViewMode::LargeIcons;
        view.path = trash::root();
        view.sync_trash_view_settings();
        assert_eq!(view.file_sort, FileSortSettings::default());
        assert_eq!(view.view_mode, FileViewMode::Details);
        assert_eq!(view.file_columns.name_width, None);
        view.file_columns.name_width = Some(444);
        view.file_sort.direction = SortDirection::Descending;
        view.path = temp.path().to_owned();
        view.sync_trash_view_settings();
        assert_eq!(view.file_sort.column, FileSortColumn::Size);
        assert_eq!(view.file_columns.name_width, Some(333));
        assert_eq!(view.view_mode, FileViewMode::LargeIcons);
        view.path = trash::root();
        view.sync_trash_view_settings();
        assert_eq!(view.file_columns.name_width, Some(444));
        assert_eq!(view.file_sort.direction, SortDirection::Descending);
    }
    #[test]
    fn bin_history_and_deleted_folder_activation_use_virtual_location() {
        let temp = TempDir::new();
        let mut view = bin_view(None);
        assert_eq!(
            view.current_navigation_location(),
            NavigationLocation::Trash
        );
        assert_eq!(view.tab_label(), trash::label());
        assert!(!view.can_go_up());
        assert!(view.activate_focused_entry(true).is_none());
        assert!(
            view.handle_entry_middle_click(
                &view.entries[0].clone(),
                super::super::selection::SelectionModifiers::default()
            )
            .is_none()
        );
        view.navigate_to_directory(temp.path().to_owned(), HistoryMode::Record);
        assert_eq!(view.back_stack.last(), Some(&NavigationLocation::Trash));
        assert_eq!(view.address_text_for_path(&trash::root()), trash::ADDRESS);
    }
    #[gpui::test]
    fn bin_toolbar_order_view_menu_and_disabled_actions(cx: &mut gpui::TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (view, cx) = cx.add_window_view(|window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window);
            let mut view = bin_view(Some(focus));
            view.sidebar_width = 320.0;
            view
        });
        for width in [1000.0, 799.0] {
            cx.simulate_resize(size(px(width), px(600.0)));
            cx.run_until_parked();
            let controls = [
                "utility-bin-cut",
                "utility-bin-delete",
                "utility-view",
                "utility-bin-restore",
                "utility-bin-restore-to",
                "utility-bin-empty",
            ];
            let bounds: Vec<_> = controls
                .iter()
                .map(|id| cx.debug_bounds(id).unwrap())
                .collect();
            for pair in bounds.windows(2) {
                assert!(pair[0].right() < pair[1].left());
            }
            for removed in [
                "utility-bin-restore-all",
                "utility-bin-properties",
                "utility-bin-details",
                "utility-bin-icons",
            ] {
                assert!(cx.debug_bounds(removed).is_none());
            }
            let view_button = cx.debug_bounds("utility-view").unwrap();
            cx.simulate_click(view_button.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            let row = cx.debug_bounds("utility-large-icons").unwrap();
            assert!((row.left() - view_button.left()).abs() <= px(16.0));
            cx.simulate_click(row.center(), gpui::Modifiers::default());
            cx.read_entity(&view, |view, _| {
                assert_eq!(view.view_mode, FileViewMode::LargeIcons);
                assert!(view.open_utility_menu.is_none());
            });
        }
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.selection.selected_indices.clear();
                cx.write_to_clipboard(gpui::ClipboardItem::new_string("unchanged".into()));
                cx.notify();
            })
        });
        cx.run_until_parked();
        for id in [
            "utility-bin-cut",
            "utility-bin-delete",
            "utility-bin-restore",
            "utility-bin-restore-to",
        ] {
            let bounds = cx.debug_bounds(id).unwrap();
            cx.simulate_click(bounds.center(), gpui::Modifiers::default());
            cx.run_until_parked();
        }
        cx.read_entity(&view, |view, app| {
            assert!(view.active_dialog_window.is_none());
            assert!(view.pending_trash_task.is_none());
            assert_eq!(
                app.read_from_clipboard().unwrap().text().unwrap(),
                "unchanged"
            );
        });
    }

    #[gpui::test]
    fn bin_dialogs_open_centered_on_explorer(cx: &mut gpui::TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (view, cx) = cx.add_window_view(|_, cx| bin_view(Some(cx.focus_handle())));
        cx.simulate_resize(gpui::size(px(1100.), px(750.)));
        cx.update(|window, app| {
            let parent = window.bounds();
            let placement =
                super::super::dialog_placement::DialogPlacement::from_window(window, app);
            let kinds = [
                DialogKind::Purge(PurgeConfirmation::new(
                    vec![],
                    crate::settings::DEFAULT_DATE_FORMAT,
                )),
                DialogKind::Conflict(RecoveryRequest {
                    ids: vec![],
                    directory: None,
                }),
                DialogKind::Properties(vec![]),
                DialogKind::Progress(
                    Arc::new(Mutex::new(Progress {
                        done: 0,
                        total: 1,
                        name: "file.txt".into(),
                        finished: false,
                    })),
                    Arc::new(AtomicBool::new(false)),
                ),
            ];
            for kind in kinds {
                let handle = view.update(app, |view, cx| {
                    view.dialog_placement = placement;
                    view.show_bin_dialog(kind, cx);
                    view.active_dialog_window.unwrap()
                });
                handle
                    .update(app, |_, window, _| {
                        assert_eq!(window.bounds().center(), parent.center());
                        window.remove_window();
                    })
                    .unwrap();
                view.update(app, |view, _| view.active_dialog_window = None);
            }
        });
    }

    #[gpui::test]
    fn bin_dialogs_fit_content_and_keep_actions_visible(cx: &mut gpui::TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let explorer = cx.new(|cx| bin_view(Some(cx.focus_handle())));
        let temp = TempDir::new();
        let payload = temp.path().join("payload");
        std::fs::write(&payload, b"properties fixture").unwrap();
        let item = trash::fixture_entry("short.txt", &payload, Some(temp.path().join("short.txt")));
        let mut long = item.clone();
        long.name = "long filename ".repeat(40).into();
        long.original_path = Some(temp.path().join("long directory ".repeat(40)));
        let progress = Arc::new(Mutex::new(Progress {
            done: 1,
            total: 2,
            name: "long filename ".repeat(100),
            finished: false,
        }));
        let cancel = Arc::new(AtomicBool::new(false));
        let kinds = [
            (
                DialogKind::Purge(PurgeConfirmation::from_item(
                    vec![item.id.clone()],
                    Some(&item),
                    crate::settings::DEFAULT_DATE_FORMAT,
                )),
                "trash-cancel",
            ),
            (
                DialogKind::Conflict(RecoveryRequest {
                    ids: vec![item.id.clone()],
                    directory: None,
                }),
                "trash-cancel",
            ),
            (DialogKind::Properties(vec![item]), "trash-properties-close"),
            (
                DialogKind::Properties(vec![long; 3]),
                "trash-properties-close",
            ),
            (
                DialogKind::Progress(progress, cancel.clone()),
                "trash-progress-cancel",
            ),
        ];
        for (kind, action) in kinds {
            let (width, height) = cx.update(|cx| dialog_size(&kind, cx));
            let is_purge = matches!(kind, DialogKind::Purge(_));
            let is_properties = matches!(kind, DialogKind::Properties(_));
            let is_progress = matches!(kind, DialogKind::Progress(_, _));
            if is_properties {
                assert!(height <= PROPERTIES_MAX_HEIGHT);
            }
            let explorer = explorer.downgrade();
            let (dialog, window) = cx.add_window_view(|window, cx| {
                let focus = cx.focus_handle();
                focus.focus(window);
                TrashDialog {
                    kind,
                    explorer,
                    focus,
                    completed: false,
                    serial: 0,
                    focused_choice: 0,
                    poll: None,
                }
            });
            window.simulate_resize(size(px(width), px(height)));
            let height = f32::from(crate::dialog_height::settle_dialog_height(window).height);
            let bounds = window.debug_bounds(action).unwrap();
            if is_properties {
                assert!(height <= PROPERTIES_MAX_HEIGHT);
                let content = window.debug_bounds("trash-properties-details").unwrap();
                let scroll = window.debug_bounds("dialog-scroll-body").unwrap();
                if height == PROPERTIES_MAX_HEIGHT {
                    assert!(content.size.height > scroll.size.height);
                }
            }
            if is_purge {
                let yes = window.debug_bounds("trash-confirm-delete").unwrap();
                assert_eq!(bounds.size, size(px(84.0), px(28.0)));
                assert_eq!(yes.top(), bounds.top());
                assert_eq!(bounds.left() - yes.right(), px(12.0));
                assert_eq!(bounds.right(), px(width - 20.0));
                assert!(bounds.bottom() <= px(height - 14.0));
            } else {
                if is_progress {
                    assert_eq!(bounds.size, size(px(84.0), px(28.0)));
                    assert_eq!(bounds.right(), px(width - DIALOG_PADDING));
                } else {
                    assert_eq!(bounds.size.height, px(DIALOG_BUTTON_HEIGHT));
                }
                assert!(
                    (bounds.bottom() - px(height - DIALOG_PADDING)).abs() <= px(1.0),
                    "{action}: button bottom {:?}, expected {}",
                    bounds.bottom(),
                    height - DIALOG_PADDING
                );
            }
            if is_progress {
                let track = window.debug_bounds("trash-progress-track").unwrap();
                let fill = window.debug_bounds("trash-progress-fill").unwrap();
                assert!((fill.size.width * 2.0 - (track.size.width - px(2.0))).abs() <= px(1.0));
                assert_eq!(
                    window
                        .debug_bounds("trash-progress-name")
                        .unwrap()
                        .size
                        .height,
                    px(DIALOG_LINE_HEIGHT)
                );
                window.read_entity(&dialog, |dialog, _| assert_eq!(dialog.focused_choice, 0));
                assert!(!cancel.load(Ordering::Relaxed));
                window.simulate_click(bounds.center(), gpui::Modifiers::default());
                assert!(cancel.load(Ordering::Relaxed));
                cancel.store(false, Ordering::Relaxed);
                window.dispatch_action(super::super::dialog::DialogConfirm);
                assert!(cancel.load(Ordering::Relaxed));
                cancel.store(false, Ordering::Relaxed);
                window.dispatch_action(super::super::dialog::DialogCancel);
                assert!(cancel.load(Ordering::Relaxed));
            } else {
                // Close without starting a filesystem operation.
                window.update(|window, app| {
                    dialog.update(app, |dialog, cx| dialog.close(window, cx))
                });
            }
        }
    }

    #[gpui::test]
    fn bin_renders_columns_toolbar_and_blocks_rename_copy_and_creation(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (view, cx) = cx.add_window_view(|window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window);
            bin_view(Some(focus))
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("trash-header-original-location").is_some());
        assert!(cx.debug_bounds("utility-bin-restore").is_some());
        assert!(cx.debug_bounds("utility-bin-empty").is_some());
        assert!(cx.debug_bounds("explorer-sidebar-bin").is_some());
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                assert!(!view.can_start_selected_rename());
                assert!(!super::super::explorer_fs::ExplorerFs::new().can_mutate(&view.path));
                cx.write_to_clipboard(gpui::ClipboardItem::new_string("unchanged".into()));
                view.copy_selected_to_clipboard(cx);
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().unwrap(),
                    "unchanged"
                );
                view.cut_selected_to_clipboard(cx);
                let item = cx.read_from_clipboard().unwrap();
                assert!(item.files().is_none());
                assert_eq!(
                    super::super::clipboard::file_clipboard_from_item(&item)
                        .unwrap()
                        .paths,
                    view.selected_paths()
                );
            })
        });
    }
    #[gpui::test]
    fn bin_delete_confirmation_captures_ids_and_defaults_to_yes(cx: &mut gpui::TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (view, cx) = cx.add_window_view(|_, cx| bin_view(Some(cx.focus_handle())));
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.request_bin_delete(cx);
                assert!(view.pending_permanent_delete.is_none());
                assert!(view.active_dialog_window.is_some());
            })
        });
        let handle = cx.read_entity(&view, |view, _| view.active_dialog_window.unwrap());
        handle
            .update(cx, |entity, window, cx| {
                let entity = entity.downcast::<TrashDialog>().unwrap();
                entity.update(cx, |dialog, cx| {
                    assert_eq!(dialog.focused_choice, 0);
                    let DialogKind::Purge(pending) = &dialog.kind else {
                        panic!("expected confirmation")
                    };
                    assert_eq!(pending.ids.len(), 1);
                    dialog.close(window, cx);
                });
            })
            .unwrap();
        cx.read_entity(&view, |view, _| {
            assert!(view.active_dialog_window.is_none());
            assert!(view.trash_operation.is_none());
        });
    }
    #[gpui::test]
    fn bin_open_keyboard_action_shows_properties_and_shift_delete_confirms_ids(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp = TempDir::new();
        let payload = temp.path().join("payload");
        std::fs::write(&payload, b"deleted").unwrap();
        let item = trash::fixture_entry("unknown.txt", &payload, None);
        let _snapshot = trash::SnapshotFixture::new(vec![item.clone()]);
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (view, cx) = cx.add_window_view(|window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window);
            let mut view = bin_view(Some(focus));
            view.entries = vec![item.file_entry()];
            view.all_entries = view.entries.clone();
            view.restore_selection_from_paths(&[item.path()]);
            view
        });
        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.handle_open_selected(&super::super::actions::OpenSelected, window, cx)
            })
        });
        let handle = cx.read_entity(&view, |view, _| view.active_dialog_window.unwrap());
        handle
            .update(cx, |entity, window, cx| {
                entity
                    .downcast::<TrashDialog>()
                    .unwrap()
                    .update(cx, |dialog, cx| {
                        let DialogKind::Properties(items) = &dialog.kind else {
                            panic!("expected properties")
                        };
                        assert_eq!(items[0].id, item.id);
                        assert_eq!(items[0].original_location(), "Unknown");
                        assert!(items[0].deleted.is_none());
                        dialog.close(window, cx);
                    });
            })
            .unwrap();
        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.handle_permanently_delete_selected(
                    &super::super::actions::PermanentlyDeleteSelected,
                    window,
                    cx,
                )
            })
        });
        // Changing selection after opening must not retarget the confirmation.
        cx.update(|_, app| {
            view.update(app, |view, _| {
                view.selection.selected_indices.clear();
            });
        });
        let handle = cx.read_entity(&view, |view, _| view.active_dialog_window.unwrap());
        handle
            .update(cx, |entity, window, cx| {
                entity
                    .downcast::<TrashDialog>()
                    .unwrap()
                    .update(cx, |dialog, cx| {
                        let DialogKind::Purge(pending) = &dialog.kind else {
                            panic!("expected permanent deletion confirmation")
                        };
                        assert_eq!(pending.ids, vec![item.id.clone()]);
                        assert_eq!(dialog.focused_choice, 0);
                        dialog.close(window, cx);
                    });
            })
            .unwrap();
        assert_eq!(std::fs::read(&payload).unwrap(), b"deleted");
    }
    #[gpui::test]
    fn consumed_bin_ids_reconcile_undo_and_clipboard_across_windows(cx: &mut gpui::TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let temp = TempDir::new();
        let path = temp.path().to_owned();
        let first = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    ExplorerView::new_unloaded_with_settings_for_test(
                        path.clone(),
                        Some(cx.focus_handle()),
                        &ExplorerSettings::default(),
                    )
                })
            })
            .unwrap()
        });
        let second = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    ExplorerView::new_unloaded_with_settings_for_test(
                        path.clone(),
                        Some(cx.focus_handle()),
                        &ExplorerSettings::default(),
                    )
                })
            })
            .unwrap()
        });
        let id = TrashItemId("a".repeat(64));
        for window in [first, second] {
            window
                .update(cx, |view, _, cx| {
                    view.observe_clipboard_summary(cx);
                    view.file_operation_undo_stack.push(
                        super::super::file_commands::FileOperationUndo::Trash(
                            super::super::file_commands::TrashUndo::Native {
                                ids: vec![id.clone()],
                                original_paths: Vec::new(),
                                failures: Vec::new(),
                            },
                        ),
                    );
                })
                .unwrap();
        }
        let clipboard = super::super::clipboard::FileClipboard::new(
            super::super::clipboard::FileClipboardOperation::Cut,
            vec![PathBuf::from(format!("{}items/{}", trash::ADDRESS, id.0))],
        );
        cx.write_to_clipboard(
            super::super::clipboard::clipboard_item_for_files(&clipboard).unwrap(),
        );
        cx.set_global(TrashRevision(trash::revision()));
        cx.run_until_parked();
        for window in [first, second] {
            window
                .update(cx, |view, _, _| {
                    assert!(view.file_operation_undo_stack.is_empty())
                })
                .unwrap();
        }
        assert!(
            super::super::clipboard::file_clipboard_from_item(&cx.read_from_clipboard().unwrap())
                .is_none()
        );
    }
    #[gpui::test]
    fn native_bin_drag_completion_never_cleans_synthetic_sources(cx: &mut gpui::TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (view, cx) = cx.add_window_view(|_, cx| bin_view(Some(cx.focus_handle())));
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.cut_selected_to_clipboard(cx);
                let paths = view.selected_paths();
                view.complete_external_paths_drag(
                    &paths,
                    gpui::ExternalPathsDragResult::Completed {
                        operation: gpui::ExternalPathDragOperation::Move,
                        cleanup_source: true,
                    },
                    cx,
                );
                assert_eq!(
                    super::super::clipboard::file_clipboard_from_item(
                        &cx.read_from_clipboard().unwrap()
                    )
                    .unwrap()
                    .paths,
                    paths
                );
                assert!(view.operation_notice.is_none());
                assert!(view.trash_operation.is_none());
            })
        });
    }
    #[test]
    fn bin_original_location_search_and_listing_identity_validation_use_native_metadata() {
        let temp = TempDir::new();
        let payload = temp.path().join("payload");
        std::fs::write(&payload, b"deleted").unwrap();
        let item = trash::fixture_entry(
            "report.txt",
            &payload,
            Some(temp.path().join("historical-origin/report.txt")),
        );
        let _snapshot = trash::SnapshotFixture::new(vec![item.clone()]);
        let mut view = bin_view(None);
        view.entries = vec![item.file_entry()];
        view.all_entries = view.entries.clone();
        assert!(trash::listing_is_current(&view.entries));
        assert!(!trash::listing_is_current(&[]));
        view.search.content = "historical-origin".into();
        view.apply_search_filter_preserving_selection(&[]);
        assert_eq!(view.entries.len(), 1);
        view.search.content = "other-origin".into();
        view.apply_search_filter_preserving_selection(&[]);
        assert!(view.entries.is_empty());
        assert_eq!(view.all_entries.len(), 1);
    }
    #[test]
    fn bin_name_filter_supports_wildcards_and_deleted_directories_never_recurse() {
        let mut view = bin_view(None);
        view.search.content = "deleted*".into();
        view.apply_search_filter_preserving_selection(&[]);
        assert_eq!(view.entries.len(), 1);
        view.search.content = "absent*".into();
        view.apply_search_filter_preserving_selection(&[]);
        assert!(view.entries.is_empty());
        assert!(!view.recursive_search_is_enabled());
    }
}
