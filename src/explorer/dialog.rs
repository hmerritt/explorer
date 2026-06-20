use gpui::{
    AnyElement, AnyWindowHandle, App, ClickEvent, Context, Entity, FocusHandle, Focusable,
    IntoElement, LineFragment, MouseButton, Render, SharedString, Task, TextRun, TitlebarOptions,
    WeakEntity, Window, WindowBounds, WindowDecorations, WindowKind, WindowOptions, actions, div,
    prelude::*, px, rgb, size,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crate::explorer::{
    constants::EXPLORER_COPY_GREEN,
    entry::FileEntry,
    filesystem::{FileConflictBatch, FileOperationPhase, FileOperationProgress},
    folder_size::{FolderSizeError, calculate_folder_size},
    formatting::{format_size, format_timestamp},
    icons::{
        DELETE_FILE_DIALOG_ICON, DELETE_FOLDER_DIALOG_ICON, DELETE_MIXED_DIALOG_ICON, image_icon,
    },
    view::{ExplorerView, PendingPermanentDelete, PendingTrash},
};
use crate::settings::SettingsState;

actions!(
    dialog,
    [
        DialogCancel,
        DialogConfirm,
        DialogFocusPrimary,
        DialogFocusSecondary
    ]
);

const SHELL_DIALOG_HORIZONTAL_PADDING: f32 = 20.0;
const SHELL_DIALOG_TOP_PADDING: f32 = 14.0;
const SHELL_DIALOG_BOTTOM_PADDING: f32 = 14.0;
const SHELL_DIALOG_VERTICAL_SLACK: f32 = 6.0;
const SHELL_DIALOG_TEXT_COLOR: u32 = 0x000000;
const SHELL_DIALOG_LINK_BLUE: u32 = 0x0067c0;
const SHELL_PROGRESS_GREEN: u32 = EXPLORER_COPY_GREEN;
const SHELL_DIALOG_COMMAND_BLUE: u32 = 0x001f60;
const SHELL_DIALOG_COMMAND_SELECTED_BG: u32 = 0xcfe8ff;
const SHELL_DIALOG_COMMAND_HOVER_BG: u32 = 0xe5f3ff;
const SHELL_DIALOG_BUTTON_BG: u32 = 0xfdfdfd;
const SHELL_DIALOG_BUTTON_BORDER: u32 = 0xd0d0d0;
const SHELL_DIALOG_BUTTON_HOVER_BG: u32 = 0xe0eef9;
const SHELL_DIALOG_BUTTON_PRESSED_BG: u32 = 0xcce4f7;
const SHELL_DIALOG_BUTTON_ACTIVE_BORDER: u32 = 0x0078d4;
const SHELL_DIALOG_LINE_HEIGHT_SCALE: f32 = 1.618;
const CONFLICT_DIALOG_WIDTH: f32 = 450.0;
const DELETE_DIALOG_WIDTH: f32 = 460.0;
const DELETE_DIALOG_PROMPT_TEXT_SIZE: f32 = 12.0;
const DELETE_DIALOG_BUTTONS_TOP_MARGIN: f32 = 24.0;
const DELETE_DIALOG_BUTTON_HEIGHT: f32 = 28.0;
const DELETE_DIALOG_BUTTON_MIN_WIDTH: f32 = 84.0;
const DELETE_DIALOG_BUTTON_GAP: f32 = 12.0;
const DELETE_DIALOG_BUTTON_BORDER_RADIUS: f32 = 0.0;
const DELETE_DIALOG_ICON_SLOT_WIDTH: f32 = 40.0;
const DELETE_DIALOG_ICON_SLOT_HEIGHT: f32 = 64.0;
const DELETE_DIALOG_COMPACT_ICON_SLOT_HEIGHT: f32 = 46.0;
const DELETE_DIALOG_ICON_GAP: f32 = 16.0;
const DELETE_DIALOG_DETAILS_TOP_MARGIN: f32 = 8.0;
const DELETE_DIALOG_TRUNCATION_SUFFIX: &str = "...";
const CONFLICT_HEADER_TEXT_SIZE: f32 = 12.0;
const CONFLICT_TITLE_TEXT_SIZE: f32 = 16.0;
const CONFLICT_TITLE_TOP_MARGIN: f32 = 5.0;
const CONFLICT_COMMANDS_TOP_MARGIN: f32 = 14.0;
const CONFLICT_COMMAND_GAP: f32 = 5.0;
const CONFLICT_COMMAND_ROW_HEIGHT: f32 = 40.0;
const CONFLICT_COMMAND_ROW_HORIZONTAL_PADDING: f32 = 12.0;
const CONFLICT_COMMAND_ICON_SLOT_WIDTH: f32 = 18.0;
const CONFLICT_COMMAND_ICON_TEXT_SIZE: f32 = 20.0;
const CONFLICT_COMMAND_LABEL_TEXT_SIZE: f32 = 16.0;
const PROGRESS_DIALOG_WIDTH: f32 = 430.0;
const PROGRESS_DIALOG_TITLE_TEXT_SIZE: f32 = 16.0;
const PROGRESS_DIALOG_TEXT_SIZE: f32 = 12.0;
const PROGRESS_DIALOG_CURRENT_ITEM_TOP_MARGIN: f32 = 10.0;
const PROGRESS_DIALOG_BAR_TOP_MARGIN: f32 = 12.0;
const PROGRESS_DIALOG_BAR_HEIGHT: f32 = 16.0;
const PROGRESS_DIALOG_BAR_WIDTH: f32 = 390.0;
const PROGRESS_DIALOG_STATUS_TOP_MARGIN: f32 = 8.0;
const PROGRESS_DIALOG_BUTTONS_TOP_MARGIN: f32 = 16.0;
const PROGRESS_DIALOG_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ExplorerDialogKind {
    PermanentDelete(PendingPermanentDelete),
    Trash(PendingTrash),
    FileConflict(FileConflictBatch),
    FileOperation(FileOperationProgress),
}

pub(super) struct ExplorerDialog {
    kind: ExplorerDialogKind,
    explorer: WeakEntity<ExplorerView>,
    date_format: String,
    font: gpui::Font,
    focus_handle: FocusHandle,
    focused_choice: Option<DialogChoice>,
    completed: bool,
    folder_size_state: FolderSizeState,
    folder_size_task: Option<Task<()>>,
    folder_size_cancel: Option<Arc<AtomicBool>>,
    file_operation_progress: Option<FileOperationProgress>,
    file_operation_task: Option<Task<()>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DialogChoice {
    Primary,
    Secondary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DialogActivation {
    ConfirmDelete,
    ConfirmTrash,
    ReplaceConflicts,
    Cancel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeleteDialogIconKind {
    File,
    Folder,
    Mixed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PermanentDeleteItemKind {
    File,
    Folder,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct PermanentDeleteDialogText {
    pub(super) message: String,
    pub(super) item_name: Option<String>,
    pub(super) item_kind: Option<PermanentDeleteItemKind>,
    pub(super) size_label: Option<String>,
    pub(super) date_modified_label: Option<String>,
    pub(super) folder_size_path: Option<PathBuf>,
}

impl PermanentDeleteDialogText {
    fn has_file_details(&self) -> bool {
        self.item_name.is_some() || self.size_label.is_some() || self.date_modified_label.is_some()
    }
}

#[derive(Debug, Eq, PartialEq)]
enum FolderSizeState {
    NotNeeded,
    Calculating { path: PathBuf },
    Ready { path: PathBuf, size: u64 },
    Failed { path: PathBuf },
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct FileConflictDialogText {
    pub(super) operation: &'static str,
    pub(super) item_count: String,
    pub(super) source_name: String,
    pub(super) destination_name: String,
    pub(super) title: String,
    pub(super) replace_label: &'static str,
    pub(super) skip_label: &'static str,
}

impl ExplorerView {
    pub(super) fn open_pending_dialog_window(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.active_dialog_window {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
            self.active_dialog_window = None;
        }

        let kind = if let Some(pending) = self.pending_permanent_delete.clone() {
            ExplorerDialogKind::PermanentDelete(pending)
        } else if let Some(pending) = self.pending_trash.clone() {
            ExplorerDialogKind::Trash(pending)
        } else if let Some(conflicts) = self.pending_file_conflict.clone() {
            ExplorerDialogKind::FileConflict(conflicts)
        } else {
            return;
        };

        match open_dialog_window(kind, cx.entity(), self.date_format.clone(), cx) {
            Ok(handle) => self.active_dialog_window = Some(handle),
            Err(error) => self.open_error = Some(format!("Failed to open dialog: {error}")),
        }
    }

    pub(super) fn open_file_operation_window(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.active_dialog_window {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
            self.active_dialog_window = None;
        }

        let Some(progress) = self
            .active_file_operation
            .as_ref()
            .map(|operation| operation.progress.clone())
        else {
            return;
        };

        match open_dialog_window(
            ExplorerDialogKind::FileOperation(progress),
            cx.entity(),
            self.date_format.clone(),
            cx,
        ) {
            Ok(handle) => self.active_dialog_window = Some(handle),
            Err(error) => {
                self.open_error = Some(format!("Failed to open progress dialog: {error}"))
            }
        }
    }

    pub(super) fn clear_active_dialog_window(&mut self) {
        self.active_dialog_window = None;
    }

    fn dialog_window_released(&mut self, kind: ExplorerDialogKind, completed: bool) {
        if !completed {
            match kind {
                ExplorerDialogKind::PermanentDelete(_) => self.cancel_pending_permanent_delete(),
                ExplorerDialogKind::Trash(_) => self.cancel_pending_trash(),
                ExplorerDialogKind::FileConflict(_) => {}
                ExplorerDialogKind::FileOperation(_) => self.cancel_active_file_operation(),
            }
        }
        self.clear_active_dialog_window();
    }
}

impl ExplorerDialog {
    fn new(
        kind: ExplorerDialogKind,
        explorer: WeakEntity<ExplorerView>,
        date_format: String,
        focus_handle: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let file_operation_progress = match &kind {
            ExplorerDialogKind::FileOperation(progress) => Some(progress.clone()),
            _ => None,
        };
        let focused_choice = default_dialog_choice(&kind);
        let font = crate::settings::current_app_font(cx);

        let mut dialog = Self {
            kind,
            explorer,
            date_format,
            font,
            focus_handle,
            focused_choice,
            completed: false,
            folder_size_state: FolderSizeState::NotNeeded,
            folder_size_task: None,
            folder_size_cancel: None,
            file_operation_progress,
            file_operation_task: None,
        };
        dialog.start_folder_size_task(cx);
        dialog.start_file_operation_progress_task(cx);
        cx.observe_global::<SettingsState>(|this, cx| {
            this.font = crate::settings::current_app_font(cx);
            cx.notify();
        })
        .detach();
        dialog
    }

    fn handle_cancel(&mut self, _: &DialogCancel, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel(window, cx);
    }

    fn handle_confirm(&mut self, _: &DialogConfirm, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(activation) = dialog_activation(&self.kind, self.focused_choice) {
            self.activate(activation, window, cx);
        }
        cx.stop_propagation();
    }

    fn handle_focus_primary(
        &mut self,
        _: &DialogFocusPrimary,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_choice(DialogChoice::Primary, cx);
        cx.stop_propagation();
    }

    fn handle_focus_secondary(
        &mut self,
        _: &DialogFocusSecondary,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_choice(DialogChoice::Secondary, cx);
        cx.stop_propagation();
    }

    fn focus_choice(&mut self, choice: DialogChoice, cx: &mut Context<Self>) {
        let focused_choice = focused_dialog_choice(self.focused_choice, choice);
        if self.focused_choice != focused_choice {
            self.focused_choice = focused_choice;
            cx.notify();
        }
    }

    fn focus_choice_from_pointer(&mut self, choice: DialogChoice, cx: &mut Context<Self>) {
        let focused_choice = pointer_focused_dialog_choice(&self.kind, self.focused_choice, choice);
        if self.focused_choice != focused_choice {
            self.focused_choice = focused_choice;
            cx.notify();
        }
    }

    fn activate(
        &mut self,
        activation: DialogActivation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match activation {
            DialogActivation::ConfirmDelete => self.confirm_delete(window, cx),
            DialogActivation::ConfirmTrash => self.confirm_trash(window, cx),
            DialogActivation::ReplaceConflicts => self.replace_conflicts(window, cx),
            DialogActivation::Cancel => self.cancel(window, cx),
        }
    }

    fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.completed = true;
        self.cancel_folder_size_task();
        let _ = self.explorer.update(cx, |explorer, cx| {
            explorer.confirm_pending_permanent_delete(cx);
            explorer.clear_active_dialog_window();
            cx.notify();
        });
        window.remove_window();
    }

    fn confirm_trash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.completed = true;
        let _ = self.explorer.update(cx, |explorer, cx| {
            explorer.confirm_pending_trash(cx);
            explorer.clear_active_dialog_window();
            cx.notify();
        });
        window.remove_window();
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.completed = true;
        self.cancel_folder_size_task();
        let kind = self.kind.clone();
        let _ = self.explorer.update(cx, |explorer, cx| {
            match kind {
                ExplorerDialogKind::PermanentDelete(_) => {
                    explorer.cancel_pending_permanent_delete();
                    explorer.clear_active_dialog_window();
                }
                ExplorerDialogKind::Trash(_) => {
                    explorer.cancel_pending_trash();
                    explorer.clear_active_dialog_window();
                }
                ExplorerDialogKind::FileConflict(_) => {
                    explorer.clear_active_dialog_window();
                    explorer.resolve_pending_file_conflicts_and_open_progress(
                        crate::explorer::filesystem::ConflictChoice::Skip,
                        cx,
                    )
                }
                ExplorerDialogKind::FileOperation(_) => {
                    explorer.cancel_active_file_operation();
                    explorer.clear_active_dialog_window();
                }
            }
            cx.notify();
        });
        window.remove_window();
    }

    fn replace_conflicts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.completed = true;
        let _ = self.explorer.update(cx, |explorer, cx| {
            explorer.clear_active_dialog_window();
            explorer.resolve_pending_file_conflicts_and_open_progress(
                crate::explorer::filesystem::ConflictChoice::Replace,
                cx,
            );
            cx.notify();
        });
        window.remove_window();
    }

    fn release(&mut self, cx: &mut App) {
        self.cancel_folder_size_task();
        let kind = self.kind.clone();
        let completed = self.completed;
        let _ = self.explorer.update(cx, move |explorer, cx| {
            match kind {
                ExplorerDialogKind::FileConflict(_) if !completed => {
                    explorer.clear_active_dialog_window();
                    explorer.resolve_pending_file_conflicts_and_open_progress(
                        crate::explorer::filesystem::ConflictChoice::Skip,
                        cx,
                    );
                }
                ExplorerDialogKind::FileConflict(_) => {}
                kind => explorer.dialog_window_released(kind, completed),
            }
            cx.notify();
        });
    }

    fn start_folder_size_task(&mut self, cx: &mut Context<Self>) {
        let Some(path) = permanent_delete_folder_size_path(&self.kind) else {
            return;
        };

        let cancel = Arc::new(AtomicBool::new(false));
        self.folder_size_state = FolderSizeState::Calculating { path: path.clone() };
        self.folder_size_cancel = Some(cancel.clone());

        let task = cx.spawn({
            let path = path.clone();
            async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn({
                        let path = path.clone();
                        let cancel = cancel.clone();
                        async move { calculate_folder_size(&path, cancel) }
                    })
                    .await;

                let _ = this.update(cx, |dialog, cx| {
                    if dialog.completed || dialog.folder_size_cancelled() {
                        return;
                    }
                    if !dialog.folder_size_state_matches_path(&path) {
                        return;
                    }

                    dialog.folder_size_state = match result {
                        Ok(size) => FolderSizeState::Ready {
                            path: path.clone(),
                            size,
                        },
                        Err(FolderSizeError::Cancelled) => return,
                        Err(FolderSizeError::Unavailable) => {
                            FolderSizeState::Failed { path: path.clone() }
                        }
                    };
                    cx.notify();
                });
            }
        });
        self.folder_size_task = Some(task);
    }

    fn cancel_folder_size_task(&mut self) {
        if let Some(cancel) = self.folder_size_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.folder_size_task = None;
    }

    fn folder_size_cancelled(&self) -> bool {
        self.folder_size_cancel
            .as_ref()
            .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
    }

    fn folder_size_state_matches_path(&self, path: &Path) -> bool {
        match &self.folder_size_state {
            FolderSizeState::Calculating { path: state_path }
            | FolderSizeState::Ready {
                path: state_path, ..
            }
            | FolderSizeState::Failed { path: state_path } => state_path == path,
            FolderSizeState::NotNeeded => false,
        }
    }

    fn folder_size_label(&self, path: &Path) -> String {
        match &self.folder_size_state {
            FolderSizeState::Ready {
                path: state_path,
                size,
            } if state_path == path => format!("Size: {}", format_size(Some(*size))),
            FolderSizeState::Failed { path: state_path } if state_path == path => {
                "Size: unavailable".to_owned()
            }
            FolderSizeState::Calculating { path: state_path } if state_path == path => {
                "Size: calculating...".to_owned()
            }
            _ => "Size: calculating...".to_owned(),
        }
    }

    fn start_file_operation_progress_task(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.kind, ExplorerDialogKind::FileOperation(_)) {
            return;
        }

        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(PROGRESS_DIALOG_POLL_INTERVAL)
                    .await;

                let should_continue = this
                    .update(cx, |dialog, cx| {
                        if dialog.completed
                            || !matches!(dialog.kind, ExplorerDialogKind::FileOperation(_))
                        {
                            return false;
                        }
                        dialog.refresh_file_operation_progress(cx);
                        cx.notify();
                        dialog.file_operation_progress.is_some()
                    })
                    .unwrap_or(false);

                if !should_continue {
                    break;
                }
            }
        });
        self.file_operation_task = Some(task);
    }

    fn refresh_file_operation_progress(&mut self, cx: &mut Context<Self>) {
        self.file_operation_progress = self
            .explorer
            .read_with(cx, |explorer, _| {
                explorer
                    .active_file_operation
                    .as_ref()
                    .map(|operation| operation.progress.clone())
            })
            .ok()
            .flatten();
    }
}

impl Render for ExplorerDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .font(self.font.clone())
            .key_context("ExplorerDialog")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(rgb(0xffffff))
            .cursor_default()
            .pt(px(SHELL_DIALOG_TOP_PADDING))
            .px(px(SHELL_DIALOG_HORIZONTAL_PADDING))
            .pb(px(SHELL_DIALOG_BOTTOM_PADDING))
            .text_size(px(12.0))
            .text_color(rgb(SHELL_DIALOG_TEXT_COLOR))
            .on_action(cx.listener(Self::handle_cancel))
            .on_action(cx.listener(Self::handle_confirm))
            .on_action(cx.listener(Self::handle_focus_primary))
            .on_action(cx.listener(Self::handle_focus_secondary))
            .child(match self.kind.clone() {
                ExplorerDialogKind::PermanentDelete(pending) => {
                    self.render_permanent_delete(pending, window, cx)
                }
                ExplorerDialogKind::Trash(pending) => self.render_trash(pending, window, cx),
                ExplorerDialogKind::FileConflict(conflicts) => {
                    self.render_file_conflict(conflicts, cx)
                }
                ExplorerDialogKind::FileOperation(_) => self.render_file_operation(window, cx),
            })
    }
}

impl Focusable for ExplorerDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ExplorerDialog {
    fn render_permanent_delete(
        &self,
        pending: PendingPermanentDelete,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let icon_kind = delete_dialog_icon_kind(&pending.paths);
        let mut text = permanent_delete_dialog_text_with_format(&pending, &self.date_format);
        if let Some(path) = text.folder_size_path.as_deref() {
            text.size_label = Some(self.folder_size_label(path));
        }
        let item_name = text
            .item_name
            .as_deref()
            .map(|name| truncated_permanent_delete_file_name(name, &self.font, window));
        let body = if text.has_file_details() {
            render_permanent_delete_file_body(text, item_name, icon_kind)
        } else {
            render_permanent_delete_compact_body(text.message, icon_kind)
        };

        div()
            .id("permanent-delete-confirmation")
            .flex()
            .flex_col()
            .w_full()
            .child(body)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(DELETE_DIALOG_BUTTON_GAP))
                    .mt(px(DELETE_DIALOG_BUTTONS_TOP_MARGIN))
                    .child(
                        dialog_button(
                            "permanent-delete-yes",
                            "Yes",
                            self.focused_choice == Some(DialogChoice::Primary),
                            window.scale_factor(),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.focus_choice_from_pointer(DialogChoice::Primary, cx);
                            }),
                        )
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                this.confirm_delete(window, cx);
                                cx.stop_propagation();
                            },
                        )),
                    )
                    .child(
                        dialog_button(
                            "permanent-delete-no",
                            "No",
                            self.focused_choice == Some(DialogChoice::Secondary),
                            window.scale_factor(),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.focus_choice_from_pointer(DialogChoice::Secondary, cx);
                            }),
                        )
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                this.cancel(window, cx);
                                cx.stop_propagation();
                            },
                        )),
                    ),
            )
            .into_any_element()
    }

    fn render_trash(
        &self,
        pending: PendingTrash,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let icon_kind = delete_dialog_icon_kind(&pending.paths);
        let text = trash_dialog_text(&pending);
        let item_name = text
            .item_name
            .as_deref()
            .map(|name| truncated_permanent_delete_file_name(name, &self.font, window));
        let body = if text.has_file_details() {
            render_permanent_delete_file_body(text, item_name, icon_kind)
        } else {
            render_permanent_delete_compact_body(text.message, icon_kind)
        };

        div()
            .id("trash-confirmation")
            .flex()
            .flex_col()
            .w_full()
            .child(body)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(DELETE_DIALOG_BUTTON_GAP))
                    .mt(px(DELETE_DIALOG_BUTTONS_TOP_MARGIN))
                    .child(
                        dialog_button(
                            "trash-yes",
                            "Yes",
                            self.focused_choice == Some(DialogChoice::Primary),
                            window.scale_factor(),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.focus_choice_from_pointer(DialogChoice::Primary, cx);
                            }),
                        )
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                this.confirm_trash(window, cx);
                                cx.stop_propagation();
                            },
                        )),
                    )
                    .child(
                        dialog_button(
                            "trash-no",
                            "No",
                            self.focused_choice == Some(DialogChoice::Secondary),
                            window.scale_factor(),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.focus_choice_from_pointer(DialogChoice::Secondary, cx);
                            }),
                        )
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                this.cancel(window, cx);
                                cx.stop_propagation();
                            },
                        )),
                    ),
            )
            .into_any_element()
    }

    fn render_file_conflict(
        &self,
        conflicts: FileConflictBatch,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text = file_conflict_dialog_text(&conflicts);

        div()
            .id("file-conflict-dialog")
            .flex()
            .flex_col()
            .w_full()
            .child(render_operation_header(&text))
            .child(
                div()
                    .mt(px(CONFLICT_TITLE_TOP_MARGIN))
                    .text_size(px(CONFLICT_TITLE_TEXT_SIZE))
                    .child(SharedString::from(text.title)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(CONFLICT_COMMAND_GAP))
                    .mt(px(CONFLICT_COMMANDS_TOP_MARGIN))
                    .child(
                        dialog_command_row(
                            "file-conflict-replace",
                            "✓",
                            EXPLORER_COPY_GREEN,
                            text.replace_label,
                            self.focused_choice == Some(DialogChoice::Primary),
                        )
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                this.replace_conflicts(window, cx);
                                cx.stop_propagation();
                            },
                        )),
                    )
                    .child(
                        dialog_command_row(
                            "file-conflict-skip",
                            "↶",
                            SHELL_DIALOG_LINK_BLUE,
                            text.skip_label,
                            self.focused_choice == Some(DialogChoice::Secondary),
                        )
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                this.cancel(window, cx);
                                cx.stop_propagation();
                            },
                        )),
                    ),
            )
            .into_any_element()
    }

    fn render_file_operation(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let progress = self.file_operation_progress.clone();
        let title = progress
            .as_ref()
            .map(|progress| progress.kind.progress_title())
            .unwrap_or("Working");
        let current_item = file_operation_current_item_label(progress.as_ref());
        let item_label = progress
            .as_ref()
            .map(file_operation_item_label)
            .unwrap_or_else(|| "Preparing".to_owned());
        let cancellable = progress
            .as_ref()
            .is_none_or(|progress| progress.cancellable);

        div()
            .id("file-operation-progress")
            .flex()
            .flex_col()
            .w_full()
            .child(
                div()
                    .text_size(px(PROGRESS_DIALOG_TITLE_TEXT_SIZE))
                    .child(SharedString::from(title.to_owned())),
            )
            .child(
                div()
                    .mt(px(PROGRESS_DIALOG_CURRENT_ITEM_TOP_MARGIN))
                    .text_size(px(PROGRESS_DIALOG_TEXT_SIZE))
                    .child(SharedString::from(current_item)),
            )
            .child(render_file_operation_progress_bar(progress.as_ref()))
            .child(
                div()
                    .mt(px(PROGRESS_DIALOG_STATUS_TOP_MARGIN))
                    .text_size(px(PROGRESS_DIALOG_TEXT_SIZE))
                    .text_color(rgb(0x595959))
                    .child(SharedString::from(item_label)),
            )
            .when(cancellable, |this| {
                this.child(
                    div()
                        .flex()
                        .justify_end()
                        .mt(px(PROGRESS_DIALOG_BUTTONS_TOP_MARGIN))
                        .child(
                            dialog_button(
                                "file-operation-cancel",
                                "Cancel",
                                false,
                                window.scale_factor(),
                            )
                            .on_click(cx.listener(
                                |this, _: &ClickEvent, window, cx| {
                                    this.cancel(window, cx);
                                    cx.stop_propagation();
                                },
                            )),
                        ),
                )
            })
            .into_any_element()
    }
}

fn open_dialog_window(
    kind: ExplorerDialogKind,
    explorer: Entity<ExplorerView>,
    date_format: String,
    cx: &mut Context<ExplorerView>,
) -> Result<AnyWindowHandle, String> {
    let font = crate::settings::current_app_font(cx);
    let options = dialog_window_options(&kind, &date_format, &font, cx);
    let handle = cx
        .open_window(options, |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);

            cx.new(|cx| {
                cx.on_release(|dialog: &mut ExplorerDialog, cx| dialog.release(cx))
                    .detach();
                ExplorerDialog::new(kind, explorer.downgrade(), date_format, focus_handle, cx)
            })
        })
        .map_err(|error| error.to_string())?;

    Ok(handle.into())
}

fn dialog_window_options(
    kind: &ExplorerDialogKind,
    date_format: &str,
    font: &gpui::Font,
    cx: &App,
) -> WindowOptions {
    let (width, height) = dialog_window_size(kind, date_format, font, cx);

    WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(width), px(height)), cx)),
        window_min_size: Some(size(px(width), px(height))),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from(kind.window_title())),
            ..Default::default()
        }),
        kind: WindowKind::Floating,
        is_movable: true,
        is_resizable: false,
        is_minimizable: true,
        window_decorations: Some(WindowDecorations::Server),
        ..Default::default()
    }
}

fn dialog_window_size(
    kind: &ExplorerDialogKind,
    date_format: &str,
    font: &gpui::Font,
    cx: &App,
) -> (f32, f32) {
    match kind {
        ExplorerDialogKind::PermanentDelete(pending) => {
            let text = permanent_delete_dialog_text_with_format(pending, date_format);
            let height = permanent_delete_dialog_height(&text, font, cx);
            (DELETE_DIALOG_WIDTH, height)
        }
        ExplorerDialogKind::Trash(pending) => {
            let text = trash_dialog_text(pending);
            let height = permanent_delete_dialog_height(&text, font, cx);
            (DELETE_DIALOG_WIDTH, height)
        }
        ExplorerDialogKind::FileConflict(conflicts) => {
            let text = file_conflict_dialog_text(conflicts);
            let height = file_conflict_dialog_height(&text.title, font, cx);
            (CONFLICT_DIALOG_WIDTH, height)
        }
        ExplorerDialogKind::FileOperation(_) => (PROGRESS_DIALOG_WIDTH, progress_dialog_height()),
    }
}

fn permanent_delete_dialog_height(
    text: &PermanentDeleteDialogText,
    font: &gpui::Font,
    cx: &App,
) -> f32 {
    let prompt_height = wrapped_dialog_text_height(
        &text.message,
        DELETE_DIALOG_PROMPT_TEXT_SIZE,
        permanent_delete_detail_text_width(),
        font,
        cx,
    );

    let content_height = if text.has_file_details() {
        permanent_delete_text_stack_height(prompt_height).max(DELETE_DIALOG_ICON_SLOT_HEIGHT)
    } else {
        prompt_height.max(DELETE_DIALOG_COMPACT_ICON_SLOT_HEIGHT)
    };

    permanent_delete_dialog_height_for_content_height(content_height)
}

fn permanent_delete_text_stack_height(prompt_height: f32) -> f32 {
    prompt_height
        + DELETE_DIALOG_DETAILS_TOP_MARGIN
        + (dialog_line_height(DELETE_DIALOG_PROMPT_TEXT_SIZE) * 3.0)
}

fn permanent_delete_dialog_height_for_content_height(content_height: f32) -> f32 {
    SHELL_DIALOG_TOP_PADDING
        + content_height
        + DELETE_DIALOG_BUTTONS_TOP_MARGIN
        + DELETE_DIALOG_BUTTON_HEIGHT
        + SHELL_DIALOG_BOTTOM_PADDING
        + SHELL_DIALOG_VERTICAL_SLACK
}

fn file_conflict_dialog_height(title: &str, font: &gpui::Font, cx: &App) -> f32 {
    let title_height = wrapped_dialog_text_height(
        title,
        CONFLICT_TITLE_TEXT_SIZE,
        dialog_content_width(CONFLICT_DIALOG_WIDTH),
        font,
        cx,
    );

    file_conflict_dialog_height_for_title_height(title_height)
}

fn file_conflict_dialog_height_for_title_height(title_height: f32) -> f32 {
    SHELL_DIALOG_TOP_PADDING
        + dialog_line_height(CONFLICT_HEADER_TEXT_SIZE)
        + CONFLICT_TITLE_TOP_MARGIN
        + title_height
        + CONFLICT_COMMANDS_TOP_MARGIN
        + (CONFLICT_COMMAND_ROW_HEIGHT * 2.0)
        + CONFLICT_COMMAND_GAP
        + SHELL_DIALOG_BOTTOM_PADDING
        + SHELL_DIALOG_VERTICAL_SLACK
}

fn progress_dialog_height() -> f32 {
    SHELL_DIALOG_TOP_PADDING
        + dialog_line_height(PROGRESS_DIALOG_TITLE_TEXT_SIZE)
        + PROGRESS_DIALOG_CURRENT_ITEM_TOP_MARGIN
        + dialog_line_height(PROGRESS_DIALOG_TEXT_SIZE)
        + PROGRESS_DIALOG_BAR_TOP_MARGIN
        + PROGRESS_DIALOG_BAR_HEIGHT
        + PROGRESS_DIALOG_STATUS_TOP_MARGIN
        + dialog_line_height(PROGRESS_DIALOG_TEXT_SIZE)
        + PROGRESS_DIALOG_BUTTONS_TOP_MARGIN
        + DELETE_DIALOG_BUTTON_HEIGHT
        + SHELL_DIALOG_BOTTOM_PADDING
        + SHELL_DIALOG_VERTICAL_SLACK
}

fn dialog_content_width(window_width: f32) -> f32 {
    (window_width - (SHELL_DIALOG_HORIZONTAL_PADDING * 2.0)).max(0.0)
}

fn permanent_delete_detail_text_width() -> f32 {
    (dialog_content_width(DELETE_DIALOG_WIDTH)
        - DELETE_DIALOG_ICON_SLOT_WIDTH
        - DELETE_DIALOG_ICON_GAP)
        .max(0.0)
}

fn wrapped_dialog_text_height(
    text: &str,
    text_size: f32,
    width: f32,
    font: &gpui::Font,
    cx: &App,
) -> f32 {
    let line_height = dialog_line_height(text_size);
    if text.is_empty() {
        return line_height;
    }

    let mut line_wrapper = cx.text_system().line_wrapper(font.clone(), px(text_size));
    let fragments = [LineFragment::text(text)];
    let wrapped_lines = line_wrapper.wrap_line(&fragments, px(width)).count() + 1;

    line_height * wrapped_lines as f32
}

fn dialog_line_height(text_size: f32) -> f32 {
    (text_size * SHELL_DIALOG_LINE_HEIGHT_SCALE).round()
}

impl ExplorerDialogKind {
    fn window_title(&self) -> &'static str {
        match self {
            ExplorerDialogKind::PermanentDelete(_) => "Delete File",
            ExplorerDialogKind::Trash(_) => "Delete File",
            ExplorerDialogKind::FileConflict(_) => "Replace or Skip Files",
            ExplorerDialogKind::FileOperation(_) => "File Operation",
        }
    }
}

fn dialog_button(
    id: &'static str,
    label: &'static str,
    selected: bool,
    scale_factor: f32,
) -> gpui::Stateful<gpui::Div> {
    let focus_inset = dialog_focus_inset(scale_factor);

    div()
        .id(id)
        .min_w(px(DELETE_DIALOG_BUTTON_MIN_WIDTH))
        .h(px(28.0))
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(DELETE_DIALOG_BUTTON_BORDER_RADIUS))
        .border_1()
        .border_color(rgb(SHELL_DIALOG_BUTTON_BORDER))
        .bg(rgb(SHELL_DIALOG_BUTTON_BG))
        .when(selected, |this| {
            this.border_color(rgb(SHELL_DIALOG_BUTTON_ACTIVE_BORDER))
        })
        .hover(|style| {
            style
                .bg(rgb(SHELL_DIALOG_BUTTON_HOVER_BG))
                .border_color(rgb(SHELL_DIALOG_BUTTON_ACTIVE_BORDER))
        })
        .active(|style| {
            style
                .bg(rgb(SHELL_DIALOG_BUTTON_PRESSED_BG))
                .border_color(rgb(SHELL_DIALOG_BUTTON_ACTIVE_BORDER))
        })
        .cursor_default()
        .when(selected, |this| {
            this.child(
                div()
                    .absolute()
                    .top(focus_inset)
                    .right(focus_inset)
                    .bottom(focus_inset)
                    .left(focus_inset)
                    .rounded(px(DELETE_DIALOG_BUTTON_BORDER_RADIUS))
                    .border_1()
                    .border_dashed()
                    .border_color(rgb(SHELL_DIALOG_TEXT_COLOR)),
            )
        })
        .child(label)
}

fn dialog_focus_inset(scale_factor: f32) -> gpui::Pixels {
    px(1.0 / scale_factor)
}

fn default_dialog_choice(kind: &ExplorerDialogKind) -> Option<DialogChoice> {
    match kind {
        ExplorerDialogKind::PermanentDelete(_)
        | ExplorerDialogKind::Trash(_)
        | ExplorerDialogKind::FileConflict(_) => Some(DialogChoice::Primary),
        ExplorerDialogKind::FileOperation(_) => None,
    }
}

fn focused_dialog_choice(
    current_choice: Option<DialogChoice>,
    requested_choice: DialogChoice,
) -> Option<DialogChoice> {
    current_choice.map(|_| requested_choice)
}

fn pointer_focused_dialog_choice(
    kind: &ExplorerDialogKind,
    current_choice: Option<DialogChoice>,
    requested_choice: DialogChoice,
) -> Option<DialogChoice> {
    match kind {
        ExplorerDialogKind::PermanentDelete(_) | ExplorerDialogKind::Trash(_) => {
            Some(requested_choice)
        }
        ExplorerDialogKind::FileConflict(_) | ExplorerDialogKind::FileOperation(_) => {
            current_choice
        }
    }
}

fn dialog_activation(
    kind: &ExplorerDialogKind,
    focused_choice: Option<DialogChoice>,
) -> Option<DialogActivation> {
    match (kind, focused_choice) {
        (ExplorerDialogKind::PermanentDelete(_), Some(DialogChoice::Primary)) => {
            Some(DialogActivation::ConfirmDelete)
        }
        (ExplorerDialogKind::Trash(_), Some(DialogChoice::Primary)) => {
            Some(DialogActivation::ConfirmTrash)
        }
        (ExplorerDialogKind::FileConflict(_), Some(DialogChoice::Primary)) => {
            Some(DialogActivation::ReplaceConflicts)
        }
        (
            ExplorerDialogKind::PermanentDelete(_)
            | ExplorerDialogKind::Trash(_)
            | ExplorerDialogKind::FileConflict(_),
            Some(DialogChoice::Secondary),
        ) => Some(DialogActivation::Cancel),
        _ => None,
    }
}

fn render_permanent_delete_file_body(
    text: PermanentDeleteDialogText,
    file_name: Option<SharedString>,
    icon_kind: DeleteDialogIconKind,
) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(DELETE_DIALOG_ICON_GAP))
        .child(render_delete_file_icon(
            icon_kind,
            DELETE_DIALOG_ICON_SLOT_HEIGHT,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(px(0.0))
                .w(px(permanent_delete_detail_text_width()))
                .text_size(px(DELETE_DIALOG_PROMPT_TEXT_SIZE))
                .child(SharedString::from(text.message))
                .when_some(file_name, |this, file_name| {
                    this.child(
                        div()
                            .mt(px(DELETE_DIALOG_DETAILS_TOP_MARGIN))
                            .min_w(px(0.0))
                            .w_full()
                            .child(file_name),
                    )
                })
                .when_some(text.size_label, |this, size_label| {
                    this.child(SharedString::from(size_label))
                })
                .when_some(text.date_modified_label, |this, date_modified_label| {
                    this.child(SharedString::from(date_modified_label))
                }),
        )
        .into_any_element()
}

fn render_permanent_delete_compact_body(
    message: String,
    icon_kind: DeleteDialogIconKind,
) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(DELETE_DIALOG_ICON_GAP))
        .child(render_delete_file_icon(
            icon_kind,
            DELETE_DIALOG_COMPACT_ICON_SLOT_HEIGHT,
        ))
        .child(
            div()
                .min_w(px(0.0))
                .w(px(permanent_delete_detail_text_width()))
                .text_size(px(DELETE_DIALOG_PROMPT_TEXT_SIZE))
                .child(SharedString::from(message)),
        )
        .into_any_element()
}

fn render_delete_file_icon(icon_kind: DeleteDialogIconKind, slot_height: f32) -> gpui::Div {
    let image = match icon_kind {
        DeleteDialogIconKind::File => DELETE_FILE_DIALOG_ICON.clone(),
        DeleteDialogIconKind::Folder => DELETE_FOLDER_DIALOG_ICON.clone(),
        DeleteDialogIconKind::Mixed => DELETE_MIXED_DIALOG_ICON.clone(),
    };

    div()
        .w(px(DELETE_DIALOG_ICON_SLOT_WIDTH))
        .h(px(slot_height))
        .flex_shrink_0()
        .child(image_icon(
            image,
            DELETE_DIALOG_ICON_SLOT_WIDTH,
            DELETE_DIALOG_ICON_SLOT_WIDTH,
        ))
}

fn truncated_permanent_delete_file_name(
    name: &str,
    name_font: &gpui::Font,
    window: &Window,
) -> SharedString {
    let mut runs = vec![TextRun {
        len: name.len(),
        font: name_font.clone(),
        color: rgb(SHELL_DIALOG_TEXT_COLOR).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }];

    window
        .text_system()
        .line_wrapper(name_font.clone(), px(DELETE_DIALOG_PROMPT_TEXT_SIZE))
        .truncate_line(
            SharedString::from(name.to_owned()),
            px(permanent_delete_detail_text_width()),
            DELETE_DIALOG_TRUNCATION_SUFFIX,
            &mut runs,
        )
}

fn render_operation_header(text: &FileConflictDialogText) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .text_size(px(CONFLICT_HEADER_TEXT_SIZE))
        .child(text.operation)
        .child(" ")
        .child(SharedString::from(text.item_count.clone()))
        .child(" from ")
        .child(
            div()
                .text_color(rgb(SHELL_DIALOG_LINK_BLUE))
                .child(SharedString::from(text.source_name.clone())),
        )
        .child(" to ")
        .child(
            div()
                .text_color(rgb(SHELL_DIALOG_LINK_BLUE))
                .child(SharedString::from(text.destination_name.clone())),
        )
        .into_any_element()
}

fn render_file_operation_progress_bar(progress: Option<&FileOperationProgress>) -> AnyElement {
    let percent = progress
        .and_then(FileOperationProgress::percent)
        .unwrap_or(0.0);
    let fill_width = percent * PROGRESS_DIALOG_BAR_WIDTH;

    div()
        .mt(px(PROGRESS_DIALOG_BAR_TOP_MARGIN))
        .w(px(PROGRESS_DIALOG_BAR_WIDTH))
        .h(px(PROGRESS_DIALOG_BAR_HEIGHT))
        .border_1()
        .border_color(rgb(0x8a8a8a))
        .bg(rgb(0xffffff))
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(px(fill_width))
                .bg(rgb(SHELL_PROGRESS_GREEN)),
        )
        .into_any_element()
}

fn file_operation_current_item_label(progress: Option<&FileOperationProgress>) -> String {
    let Some(progress) = progress else {
        return "Preparing...".to_owned();
    };
    let Some(current_item) = progress.current_item.as_deref() else {
        return "Preparing...".to_owned();
    };

    if progress.phase == FileOperationPhase::Extracting {
        current_item.display().to_string()
    } else {
        path_display_name(current_item)
    }
}

fn file_operation_item_label(progress: &FileOperationProgress) -> String {
    let action = match progress.phase {
        FileOperationPhase::Preparing => "Preparing",
        FileOperationPhase::Copying => "Copying",
        FileOperationPhase::Extracting => "Extracting",
        FileOperationPhase::Moving => "Moving",
        FileOperationPhase::Removing => "Removing",
        FileOperationPhase::Finished => "Finished",
        FileOperationPhase::Cancelled => "Cancelled",
    };

    if progress.total_files == 0 {
        return action.to_owned();
    }

    let copied = format_size(Some(progress.copied_bytes));
    let total = format_size(Some(progress.total_bytes));
    format!(
        "{action} {} of {} items ({copied} of {total})",
        progress.completed_files.min(progress.total_files),
        progress.total_files
    )
}

fn dialog_command_row(
    id: &'static str,
    icon: &'static str,
    icon_color: u32,
    label: &'static str,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(CONFLICT_COMMAND_ROW_HEIGHT))
        .w_full()
        .flex()
        .items_center()
        .gap(px(10.0))
        .px(px(CONFLICT_COMMAND_ROW_HORIZONTAL_PADDING))
        .rounded(px(0.0))
        .when(selected, |this| {
            this.border_1()
                .border_color(rgb(0x000000))
                .bg(rgb(SHELL_DIALOG_COMMAND_SELECTED_BG))
        })
        .when(!selected, |this| {
            this.border_1()
                .border_color(rgb(0xffffff))
                .hover(|style| style.bg(rgb(SHELL_DIALOG_COMMAND_HOVER_BG)))
        })
        .cursor_default()
        .child(
            div()
                .w(px(CONFLICT_COMMAND_ICON_SLOT_WIDTH))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(CONFLICT_COMMAND_ICON_TEXT_SIZE))
                .text_color(rgb(icon_color))
                .child(icon),
        )
        .child(
            div()
                .text_size(px(CONFLICT_COMMAND_LABEL_TEXT_SIZE))
                .text_color(rgb(SHELL_DIALOG_COMMAND_BLUE))
                .child(label),
        )
}

#[cfg(test)]
pub(super) fn permanent_delete_dialog_text(
    pending: &PendingPermanentDelete,
) -> PermanentDeleteDialogText {
    permanent_delete_dialog_text_with_format(pending, crate::settings::DEFAULT_DATE_FORMAT)
}

fn permanent_delete_dialog_text_with_format(
    pending: &PendingPermanentDelete,
    date_format: &str,
) -> PermanentDeleteDialogText {
    let (item_name, item_kind, size_label, date_modified_label, folder_size_path) =
        if let [path] = pending.paths.as_slice() {
            let detail = permanent_delete_file_detail(path, date_format);
            (
                Some(detail.item_name),
                Some(detail.item_kind),
                Some(detail.size_label),
                Some(detail.date_modified_label),
                detail.folder_size_path,
            )
        } else {
            (None, None, None, None, None)
        };

    let message = match item_kind {
        Some(PermanentDeleteItemKind::File) => {
            "Are you sure you want to permanently delete this file?".to_owned()
        }
        Some(PermanentDeleteItemKind::Folder) => {
            "Are you sure you want to permanently delete this folder?".to_owned()
        }
        None => format!(
            "Are you sure you want to permanently delete these {} items?",
            pending.paths.len()
        ),
    };

    PermanentDeleteDialogText {
        message,
        item_name,
        item_kind,
        size_label,
        date_modified_label,
        folder_size_path,
    }
}

pub(super) fn trash_dialog_text(pending: &PendingTrash) -> PermanentDeleteDialogText {
    let (item_name, item_kind) = if let [path] = pending.paths.as_slice() {
        let detail = permanent_delete_file_detail(path, crate::settings::DEFAULT_DATE_FORMAT);
        (Some(detail.item_name), Some(detail.item_kind))
    } else {
        (None, None)
    };

    let message = match item_kind {
        Some(PermanentDeleteItemKind::File) => {
            "Are you sure you want to move this file to the Recycle Bin?".to_owned()
        }
        Some(PermanentDeleteItemKind::Folder) => {
            "Are you sure you want to move this folder to the Recycle Bin?".to_owned()
        }
        None => format!(
            "Are you sure you want to move these {} items to the Recycle Bin?",
            pending.paths.len()
        ),
    };

    PermanentDeleteDialogText {
        message,
        item_name,
        item_kind,
        size_label: None,
        date_modified_label: None,
        folder_size_path: None,
    }
}

fn permanent_delete_folder_size_path(kind: &ExplorerDialogKind) -> Option<PathBuf> {
    let ExplorerDialogKind::PermanentDelete(pending) = kind else {
        return None;
    };
    let [path] = pending.paths.as_slice() else {
        return None;
    };
    let entry = FileEntry::from_path(path.clone())?;
    entry.is_directory_like().then(|| path.clone())
}

struct PermanentDeleteFileDetail {
    item_name: String,
    item_kind: PermanentDeleteItemKind,
    size_label: String,
    date_modified_label: String,
    folder_size_path: Option<PathBuf>,
}

fn permanent_delete_file_detail(path: &Path, date_format: &str) -> PermanentDeleteFileDetail {
    if let Some(entry) = FileEntry::from_path(path.to_path_buf()) {
        let is_folder = entry.is_directory_like();
        return PermanentDeleteFileDetail {
            item_name: entry.display_name().to_owned(),
            item_kind: if is_folder {
                PermanentDeleteItemKind::Folder
            } else {
                PermanentDeleteItemKind::File
            },
            size_label: if is_folder {
                "Size: calculating...".to_owned()
            } else {
                format!("Size: {}", format_size(entry.size))
            },
            date_modified_label: format!(
                "Date Modified: {}",
                format_timestamp(entry.modified, date_format)
            ),
            folder_size_path: is_folder.then(|| path.to_path_buf()),
        };
    }

    PermanentDeleteFileDetail {
        item_name: path_display_name(path),
        item_kind: PermanentDeleteItemKind::File,
        size_label: "Size: ".to_owned(),
        date_modified_label: "Date Modified: ".to_owned(),
        folder_size_path: None,
    }
}

fn delete_dialog_icon_kind(paths: &[PathBuf]) -> DeleteDialogIconKind {
    let Some((first, rest)) = paths.split_first() else {
        return DeleteDialogIconKind::Mixed;
    };

    let Some(first_kind) = delete_dialog_item_kind(first) else {
        return if rest.is_empty() {
            DeleteDialogIconKind::File
        } else {
            DeleteDialogIconKind::Mixed
        };
    };

    let mut has_file = first_kind == PermanentDeleteItemKind::File;
    let mut has_folder = first_kind == PermanentDeleteItemKind::Folder;

    for path in rest {
        let Some(item_kind) = delete_dialog_item_kind(path) else {
            return DeleteDialogIconKind::Mixed;
        };

        match item_kind {
            PermanentDeleteItemKind::File => has_file = true,
            PermanentDeleteItemKind::Folder => has_folder = true,
        }

        if has_file && has_folder {
            return DeleteDialogIconKind::Mixed;
        }
    }

    if has_folder {
        DeleteDialogIconKind::Folder
    } else {
        DeleteDialogIconKind::File
    }
}

fn delete_dialog_item_kind(path: &Path) -> Option<PermanentDeleteItemKind> {
    let entry = FileEntry::from_path(path.to_path_buf())?;
    Some(if entry.is_directory_like() {
        PermanentDeleteItemKind::Folder
    } else {
        PermanentDeleteItemKind::File
    })
}

fn path_display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.display().to_string())
}

pub(super) fn file_conflict_dialog_text(conflicts: &FileConflictBatch) -> FileConflictDialogText {
    let count = conflicts.len();
    let title = if count == 1 {
        format!(
            "The destination already has a file named \"{}\"",
            conflicts.first_destination_name()
        )
    } else {
        "The destination already has files with the same names".to_owned()
    };
    let replace_label = if count == 1 {
        "Replace the file in the destination"
    } else {
        "Replace the files in the destination"
    };
    let skip_label = if count == 1 {
        "Skip this file"
    } else {
        "Skip these files"
    };

    FileConflictDialogText {
        operation: conflicts.operation_label(),
        item_count: conflicts.item_count_label(),
        source_name: conflicts.source_location_name(),
        destination_name: conflicts.destination_location_name(),
        title,
        replace_label,
        skip_label,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::{
        filesystem::{
            FileOperationKind, FileOperationOutcome, copy_paths_to_directory,
            move_paths_to_directory,
        },
        test_support::TempDir,
        view::FileOperationState,
    };
    use crate::settings::{ExplorerSettings, SettingsState};
    use gpui::{Entity, TestAppContext, VisualTestContext};
    use std::{fs, path::PathBuf};

    #[test]
    fn permanent_delete_text_uses_singular_file_message_and_details() {
        let temp = TempDir::new();
        let path = temp.path().join("a.tif");
        fs::write(&path, []).expect("create selected file");
        let modified = fs::metadata(&path)
            .expect("read selected file metadata")
            .modified()
            .ok();

        let text = permanent_delete_dialog_text(&PendingPermanentDelete { paths: vec![path] });

        assert_eq!(
            text.message,
            "Are you sure you want to permanently delete this file?"
        );
        assert_eq!(text.item_name, Some("a.tif".to_owned()));
        assert_eq!(text.item_kind, Some(PermanentDeleteItemKind::File));
        assert_eq!(text.size_label, Some("Size: 0 bytes".to_owned()));
        assert_eq!(
            text.date_modified_label,
            Some(format!(
                "Date Modified: {}",
                format_timestamp(modified, crate::settings::DEFAULT_DATE_FORMAT)
            ))
        );
        assert_eq!(text.folder_size_path, None);
    }

    #[test]
    fn permanent_delete_text_uses_configured_date_format() {
        let temp = TempDir::new();
        let path = temp.path().join("a.txt");
        fs::write(&path, []).expect("create selected file");
        let modified = fs::metadata(&path)
            .expect("read selected file metadata")
            .modified()
            .ok();

        let text = permanent_delete_dialog_text_with_format(
            &PendingPermanentDelete { paths: vec![path] },
            "%d %B %Y",
        );

        assert_eq!(
            text.date_modified_label,
            Some(format!(
                "Date Modified: {}",
                format_timestamp(modified, "%d %B %Y")
            ))
        );
    }

    #[test]
    fn permanent_delete_text_uses_singular_folder_message_and_calculating_size() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        fs::create_dir(&folder).expect("create selected folder");
        let modified = fs::metadata(&folder)
            .expect("read selected folder metadata")
            .modified()
            .ok();

        let text = permanent_delete_dialog_text(&PendingPermanentDelete {
            paths: vec![folder.clone()],
        });

        assert_eq!(
            text.message,
            "Are you sure you want to permanently delete this folder?"
        );
        assert_eq!(text.item_name, Some("folder".to_owned()));
        assert_eq!(text.item_kind, Some(PermanentDeleteItemKind::Folder));
        assert_eq!(text.size_label, Some("Size: calculating...".to_owned()));
        assert_eq!(
            text.date_modified_label,
            Some(format!(
                "Date Modified: {}",
                format_timestamp(modified, crate::settings::DEFAULT_DATE_FORMAT)
            ))
        );
        assert_eq!(text.folder_size_path, Some(folder));
    }

    #[test]
    fn permanent_delete_text_falls_back_to_path_name_when_metadata_is_missing() {
        let text = permanent_delete_dialog_text(&PendingPermanentDelete {
            paths: vec![PathBuf::from("missing.txt")],
        });

        assert_eq!(text.item_name, Some("missing.txt".to_owned()));
        assert_eq!(text.item_kind, Some(PermanentDeleteItemKind::File));
        assert_eq!(text.size_label, Some("Size: ".to_owned()));
        assert_eq!(text.date_modified_label, Some("Date Modified: ".to_owned()));
        assert_eq!(text.folder_size_path, None);
    }

    #[test]
    fn permanent_delete_text_keeps_long_file_name_separate_from_prompt() {
        let long_name = format!("{}{}", "a".repeat(160), ".txt");
        let text = permanent_delete_dialog_text(&PendingPermanentDelete {
            paths: vec![PathBuf::from(&long_name)],
        });

        assert_eq!(
            text.message,
            "Are you sure you want to permanently delete this file?"
        );
        assert_eq!(text.item_name, Some(long_name));
    }

    #[test]
    fn permanent_delete_filename_truncation_has_visible_text_width() {
        assert!(
            permanent_delete_detail_text_width() > DELETE_DIALOG_TRUNCATION_SUFFIX.len() as f32
        );
    }

    #[test]
    fn permanent_delete_text_uses_plural_item_count() {
        let text = permanent_delete_dialog_text(&PendingPermanentDelete {
            paths: vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")],
        });

        assert_eq!(
            text.message,
            "Are you sure you want to permanently delete these 2 items?"
        );
        assert_eq!(text.item_name, None);
        assert_eq!(text.item_kind, None);
        assert_eq!(text.size_label, None);
        assert_eq!(text.date_modified_label, None);
        assert_eq!(text.folder_size_path, None);
    }

    #[test]
    fn delete_dialog_icon_uses_file_for_single_file() {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        fs::write(&file, []).expect("create selected file");

        assert_eq!(delete_dialog_icon_kind(&[file]), DeleteDialogIconKind::File);
    }

    #[test]
    fn delete_dialog_icon_uses_folder_for_single_folder() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        fs::create_dir(&folder).expect("create selected folder");

        assert_eq!(
            delete_dialog_icon_kind(&[folder]),
            DeleteDialogIconKind::Folder
        );
    }

    #[test]
    fn delete_dialog_icon_uses_file_for_multiple_files() {
        let temp = TempDir::new();
        let first = temp.path().join("a.txt");
        let second = temp.path().join("b.txt");
        fs::write(&first, []).expect("create first selected file");
        fs::write(&second, []).expect("create second selected file");

        assert_eq!(
            delete_dialog_icon_kind(&[first, second]),
            DeleteDialogIconKind::File
        );
    }

    #[test]
    fn delete_dialog_icon_uses_folder_for_multiple_folders() {
        let temp = TempDir::new();
        let first = temp.path().join("a");
        let second = temp.path().join("b");
        fs::create_dir(&first).expect("create first selected folder");
        fs::create_dir(&second).expect("create second selected folder");

        assert_eq!(
            delete_dialog_icon_kind(&[first, second]),
            DeleteDialogIconKind::Folder
        );
    }

    #[test]
    fn delete_dialog_icon_uses_alert_for_mixed_file_and_folder() {
        let temp = TempDir::new();
        let file = temp.path().join("a.txt");
        let folder = temp.path().join("folder");
        fs::write(&file, []).expect("create selected file");
        fs::create_dir(&folder).expect("create selected folder");

        assert_eq!(
            delete_dialog_icon_kind(&[file, folder]),
            DeleteDialogIconKind::Mixed
        );
    }

    #[test]
    fn delete_dialog_icon_falls_back_for_missing_paths() {
        let single_missing = PathBuf::from("missing.txt");
        let multi_missing = vec![
            PathBuf::from("missing-a.txt"),
            PathBuf::from("missing-b.txt"),
        ];

        assert_eq!(
            delete_dialog_icon_kind(&[single_missing]),
            DeleteDialogIconKind::File
        );
        assert_eq!(
            delete_dialog_icon_kind(&multi_missing),
            DeleteDialogIconKind::Mixed
        );
    }

    #[test]
    fn trash_text_uses_recycle_bin_message_without_size_details() {
        let temp = TempDir::new();
        let path = temp.path().join("a.txt");
        fs::write(&path, []).expect("create selected file");

        let text = trash_dialog_text(&PendingTrash { paths: vec![path] });

        assert_eq!(
            text.message,
            "Are you sure you want to move this file to the Recycle Bin?"
        );
        assert_eq!(text.item_name, Some("a.txt".to_owned()));
        assert_eq!(text.item_kind, Some(PermanentDeleteItemKind::File));
        assert_eq!(text.size_label, None);
        assert_eq!(text.date_modified_label, None);
        assert_eq!(text.folder_size_path, None);
    }

    #[test]
    fn trash_text_uses_plural_recycle_bin_message() {
        let text = trash_dialog_text(&PendingTrash {
            paths: vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")],
        });

        assert_eq!(
            text.message,
            "Are you sure you want to move these 2 items to the Recycle Bin?"
        );
        assert_eq!(text.item_name, None);
        assert_eq!(text.item_kind, None);
        assert_eq!(text.size_label, None);
        assert_eq!(text.date_modified_label, None);
        assert_eq!(text.folder_size_path, None);
    }

    #[test]
    fn confirmation_dialogs_default_to_primary_choice() {
        let delete = ExplorerDialogKind::PermanentDelete(PendingPermanentDelete {
            paths: vec![PathBuf::from("a.txt")],
        });
        let trash = ExplorerDialogKind::Trash(PendingTrash {
            paths: vec![PathBuf::from("a.txt")],
        });
        let conflict = ExplorerDialogKind::FileConflict(single_conflict_batch());

        assert_eq!(default_dialog_choice(&delete), Some(DialogChoice::Primary));
        assert_eq!(default_dialog_choice(&trash), Some(DialogChoice::Primary));
        assert_eq!(
            default_dialog_choice(&conflict),
            Some(DialogChoice::Primary)
        );
    }

    #[test]
    fn progress_dialog_has_no_default_choice() {
        assert_eq!(
            default_dialog_choice(&ExplorerDialogKind::FileOperation(test_progress())),
            None
        );
    }

    #[test]
    fn arrow_navigation_selects_requested_choice_and_stays_unfocused_for_progress() {
        assert_eq!(
            focused_dialog_choice(Some(DialogChoice::Primary), DialogChoice::Secondary),
            Some(DialogChoice::Secondary)
        );
        assert_eq!(
            focused_dialog_choice(Some(DialogChoice::Secondary), DialogChoice::Primary),
            Some(DialogChoice::Primary)
        );
        assert_eq!(
            focused_dialog_choice(None, DialogChoice::Primary),
            None,
            "progress dialogs must not gain an Enter-activated choice"
        );
    }

    #[test]
    fn pointer_down_focuses_confirmation_choice_but_not_other_dialog_controls() {
        let delete = ExplorerDialogKind::PermanentDelete(PendingPermanentDelete {
            paths: vec![PathBuf::from("a.txt")],
        });
        let trash = ExplorerDialogKind::Trash(PendingTrash {
            paths: vec![PathBuf::from("a.txt")],
        });
        let conflict = ExplorerDialogKind::FileConflict(single_conflict_batch());
        let progress = ExplorerDialogKind::FileOperation(test_progress());

        for kind in [&delete, &trash] {
            assert_eq!(
                pointer_focused_dialog_choice(
                    kind,
                    Some(DialogChoice::Primary),
                    DialogChoice::Secondary,
                ),
                Some(DialogChoice::Secondary)
            );
        }
        assert_eq!(
            pointer_focused_dialog_choice(
                &conflict,
                Some(DialogChoice::Primary),
                DialogChoice::Secondary,
            ),
            Some(DialogChoice::Primary)
        );
        assert_eq!(
            pointer_focused_dialog_choice(&progress, None, DialogChoice::Secondary),
            None
        );
    }

    #[test]
    fn dialog_focus_inset_is_one_device_pixel_at_common_scale_factors() {
        for scale_factor in [1.0, 1.25, 1.5, 2.0] {
            let logical_inset = f32::from(dialog_focus_inset(scale_factor));
            assert_approx_eq(logical_inset * scale_factor, 1.0);
        }
    }

    #[test]
    fn focused_choice_maps_to_each_dialog_action() {
        let delete = ExplorerDialogKind::PermanentDelete(PendingPermanentDelete {
            paths: vec![PathBuf::from("a.txt")],
        });
        let trash = ExplorerDialogKind::Trash(PendingTrash {
            paths: vec![PathBuf::from("a.txt")],
        });
        let conflict = ExplorerDialogKind::FileConflict(single_conflict_batch());

        assert_eq!(
            dialog_activation(&delete, Some(DialogChoice::Primary)),
            Some(DialogActivation::ConfirmDelete)
        );
        assert_eq!(
            dialog_activation(&trash, Some(DialogChoice::Primary)),
            Some(DialogActivation::ConfirmTrash)
        );
        assert_eq!(
            dialog_activation(&conflict, Some(DialogChoice::Primary)),
            Some(DialogActivation::ReplaceConflicts)
        );
        for kind in [&delete, &trash, &conflict] {
            assert_eq!(
                dialog_activation(kind, Some(DialogChoice::Secondary)),
                Some(DialogActivation::Cancel)
            );
        }
    }

    #[test]
    fn progress_dialog_enter_does_not_activate_cancel() {
        assert_eq!(
            dialog_activation(
                &ExplorerDialogKind::FileOperation(test_progress()),
                default_dialog_choice(&ExplorerDialogKind::FileOperation(test_progress()))
            ),
            None
        );
    }

    #[test]
    fn file_conflict_text_uses_single_file_labels() {
        let conflicts = single_conflict_batch();
        let text = file_conflict_dialog_text(&conflicts);

        assert_eq!(text.operation, "Moving");
        assert_eq!(text.item_count, "1 item");
        assert_eq!(text.source_name, "source");
        assert_eq!(text.destination_name, "destination");
        assert_eq!(
            text.title,
            "The destination already has a file named \"file.txt\""
        );
        assert_eq!(text.replace_label, "Replace the file in the destination");
        assert_eq!(text.skip_label, "Skip this file");
    }

    #[test]
    fn file_conflict_text_uses_plural_file_labels() {
        let conflicts = multi_conflict_batch();
        let text = file_conflict_dialog_text(&conflicts);

        assert_eq!(text.operation, "Moving");
        assert_eq!(text.item_count, "2 items");
        assert_eq!(text.source_name, "source");
        assert_eq!(text.destination_name, "destination");
        assert_eq!(
            text.title,
            "The destination already has files with the same names"
        );
        assert_eq!(text.replace_label, "Replace the files in the destination");
        assert_eq!(text.skip_label, "Skip these files");
    }

    #[test]
    fn file_conflict_text_uses_copying_operation() {
        let conflicts = conflict_batch(&["file.txt"], copy_paths_to_directory);
        let text = file_conflict_dialog_text(&conflicts);

        assert_eq!(text.operation, "Copying");
        assert_eq!(text.item_count, "1 item");
    }

    #[test]
    fn file_conflict_text_reports_multiple_source_locations() {
        let conflicts = multi_source_conflict_batch();
        let text = file_conflict_dialog_text(&conflicts);

        assert_eq!(text.operation, "Moving");
        assert_eq!(text.item_count, "2 items");
        assert_eq!(text.source_name, "multiple locations");
        assert_eq!(text.destination_name, "destination");
    }

    #[test]
    fn permanent_delete_dialog_height_matches_content_baseline() {
        let prompt_height = dialog_line_height(DELETE_DIALOG_PROMPT_TEXT_SIZE);
        let content_height = permanent_delete_text_stack_height(prompt_height);
        let height = permanent_delete_dialog_height_for_content_height(content_height);

        assert_approx_eq(height, 170.0);
    }

    #[test]
    fn permanent_delete_multi_item_height_uses_compact_prompt_only_body() {
        let prompt_height = dialog_line_height(DELETE_DIALOG_PROMPT_TEXT_SIZE);
        let content_height = prompt_height.max(DELETE_DIALOG_COMPACT_ICON_SLOT_HEIGHT);
        let height = permanent_delete_dialog_height_for_content_height(content_height);

        assert_approx_eq(height, 132.0);
    }

    #[test]
    fn file_conflict_dialog_height_matches_single_line_baseline() {
        let height = file_conflict_dialog_height_for_title_height(dialog_line_height(
            CONFLICT_TITLE_TEXT_SIZE,
        ));

        assert_approx_eq(height, 183.0);
    }

    #[test]
    fn file_conflict_dialog_height_grows_for_wrapped_title() {
        let single_line_height = file_conflict_dialog_height_for_title_height(dialog_line_height(
            CONFLICT_TITLE_TEXT_SIZE,
        ));
        let wrapped_height = file_conflict_dialog_height_for_title_height(
            dialog_line_height(CONFLICT_TITLE_TEXT_SIZE) * 2.0,
        );

        assert!(wrapped_height > single_line_height);
    }

    #[test]
    fn progress_dialog_height_fits_cancel_button_row() {
        let minimum_height = SHELL_DIALOG_TOP_PADDING
            + dialog_line_height(PROGRESS_DIALOG_TITLE_TEXT_SIZE)
            + PROGRESS_DIALOG_CURRENT_ITEM_TOP_MARGIN
            + dialog_line_height(PROGRESS_DIALOG_TEXT_SIZE)
            + PROGRESS_DIALOG_BAR_TOP_MARGIN
            + PROGRESS_DIALOG_BAR_HEIGHT
            + PROGRESS_DIALOG_STATUS_TOP_MARGIN
            + dialog_line_height(PROGRESS_DIALOG_TEXT_SIZE)
            + PROGRESS_DIALOG_BUTTONS_TOP_MARGIN
            + DELETE_DIALOG_BUTTON_HEIGHT
            + SHELL_DIALOG_BOTTOM_PADDING;

        assert!(progress_dialog_height() >= minimum_height);
        assert_approx_eq(progress_dialog_height(), 188.0);
    }

    #[test]
    fn confirmation_button_geometry_matches_windows_spacing() {
        assert_approx_eq(DELETE_DIALOG_BUTTON_MIN_WIDTH, 84.0);
        assert_approx_eq(DELETE_DIALOG_BUTTON_GAP, 12.0);
        assert_approx_eq(DELETE_DIALOG_BUTTON_BORDER_RADIUS, 0.0);
    }

    #[test]
    fn shell_progress_green_uses_copy_progress_green() {
        assert_eq!(SHELL_PROGRESS_GREEN, EXPLORER_COPY_GREEN);
    }

    #[gpui::test]
    fn dialog_focus_actions_move_between_confirmation_buttons(cx: &mut TestAppContext) {
        let pending = PendingPermanentDelete {
            paths: vec![PathBuf::from("a.txt")],
        };
        let (_explorer, dialog, cx) =
            test_dialog_entity(cx, ExplorerDialogKind::PermanentDelete(pending));

        cx.update(|window, app| {
            dialog.update(app, |dialog, cx| {
                assert_eq!(dialog.focused_choice, Some(DialogChoice::Primary));

                dialog.handle_focus_secondary(&DialogFocusSecondary, window, cx);
                assert_eq!(dialog.focused_choice, Some(DialogChoice::Secondary));

                dialog.handle_focus_primary(&DialogFocusPrimary, window, cx);
                assert_eq!(dialog.focused_choice, Some(DialogChoice::Primary));
            });
        });
    }

    #[gpui::test]
    fn delete_dialog_confirm_removes_pending_file(cx: &mut TestAppContext) {
        let temp = TempDir::new();
        let file = temp.path().join("delete-me.txt");
        fs::write(&file, b"delete").expect("create file");
        let pending = PendingPermanentDelete {
            paths: vec![file.clone()],
        };
        let (explorer, dialog, cx) =
            test_dialog_entity(cx, ExplorerDialogKind::PermanentDelete(pending.clone()));

        cx.update(|window, app| {
            explorer.update(app, |view, _| {
                view.pending_permanent_delete = Some(pending);
            });
            dialog.update(app, |dialog, cx| {
                dialog.handle_confirm(&DialogConfirm, window, cx);
                assert!(dialog.completed);
            });
            explorer.update(app, |view, _| {
                assert!(view.pending_permanent_delete.is_none());
                assert!(view.open_error.is_none());
            });
        });
        assert!(!file.exists());
    }

    #[gpui::test]
    fn file_operation_dialog_cancel_signals_active_operation(cx: &mut TestAppContext) {
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = test_progress();
        let (explorer, dialog, cx) =
            test_dialog_entity(cx, ExplorerDialogKind::FileOperation(progress.clone()));

        cx.update(|window, app| {
            explorer.update(app, |view, _| {
                view.active_file_operation = Some(FileOperationState {
                    progress,
                    cancel: cancel.clone(),
                    task: None,
                    archive_diagnostics: None,
                });
            });
            dialog.update(app, |dialog, cx| {
                dialog.handle_cancel(&DialogCancel, window, cx);
                assert!(dialog.completed);
            });
            explorer.update(app, |view, _| {
                assert!(view.active_dialog_window.is_none());
            });
        });

        assert!(cancel.load(Ordering::Relaxed));
    }

    #[gpui::test]
    fn file_operation_dialog_opens_during_explorer_view_update(cx: &mut TestAppContext) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let (view, cx) = cx.add_window_view(|_, cx| {
            ExplorerView::new_with_focus_handle_for_test(
                PathBuf::from("dialog-test"),
                cx.focus_handle(),
            )
        });

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.active_file_operation = Some(FileOperationState {
                    progress: test_progress(),
                    cancel: Arc::new(AtomicBool::new(false)),
                    task: None,
                    archive_diagnostics: None,
                });
                view.open_file_operation_window(cx);
                assert!(view.active_dialog_window.is_some());
            });
        });
    }

    #[test]
    fn dialog_window_release_cancels_pending_delete() {
        let mut view = ExplorerView::new(PathBuf::from("delete"));
        view.pending_permanent_delete = Some(PendingPermanentDelete {
            paths: vec![PathBuf::from("a.txt")],
        });

        view.dialog_window_released(
            ExplorerDialogKind::PermanentDelete(view.pending_permanent_delete.clone().unwrap()),
            false,
        );

        assert_eq!(view.pending_permanent_delete, None);
        assert!(view.active_dialog_window.is_none());
    }

    #[test]
    fn dialog_window_release_does_not_cancel_completed_delete() {
        let mut view = ExplorerView::new(PathBuf::from("delete"));
        view.pending_permanent_delete = Some(PendingPermanentDelete {
            paths: vec![PathBuf::from("a.txt")],
        });

        view.dialog_window_released(
            ExplorerDialogKind::PermanentDelete(view.pending_permanent_delete.clone().unwrap()),
            true,
        );

        assert!(view.pending_permanent_delete.is_some());
        assert!(view.active_dialog_window.is_none());
    }

    fn test_dialog_entity<'a>(
        cx: &'a mut TestAppContext,
        kind: ExplorerDialogKind,
    ) -> (
        Entity<ExplorerView>,
        Entity<ExplorerDialog>,
        &'a mut VisualTestContext,
    ) {
        cx.set_global(SettingsState::for_test(ExplorerSettings::default()));
        let explorer = cx.update(|cx| cx.new(|_| ExplorerView::new(PathBuf::from("dialog-test"))));
        let weak_explorer = explorer.downgrade();
        let (dialog, cx) = cx.add_window_view(move |_, cx| {
            ExplorerDialog::new(
                kind,
                weak_explorer,
                crate::settings::DEFAULT_DATE_FORMAT.to_owned(),
                cx.focus_handle(),
                cx,
            )
        });
        (explorer, dialog, cx)
    }

    fn single_conflict_batch() -> FileConflictBatch {
        conflict_batch(&["file.txt"], move_paths_to_directory)
    }

    fn test_progress() -> FileOperationProgress {
        FileOperationProgress {
            kind: FileOperationKind::Copy,
            phase: FileOperationPhase::Copying,
            total_bytes: 1,
            copied_bytes: 0,
            total_files: 1,
            completed_files: 0,
            current_item: None,
            cancellable: true,
        }
    }

    fn multi_conflict_batch() -> FileConflictBatch {
        conflict_batch(&["a.txt", "b.txt"], move_paths_to_directory)
    }

    fn conflict_batch(
        names: &[&str],
        operation: fn(&[PathBuf], &std::path::Path) -> Result<FileOperationOutcome, String>,
    ) -> FileConflictBatch {
        let temp = TempDir::new();
        let source_dir = temp.path().join("source");
        let destination_dir = temp.path().join("destination");
        fs::create_dir(&source_dir).expect("create source folder");
        fs::create_dir(&destination_dir).expect("create destination folder");

        let mut sources = Vec::new();
        for name in names {
            let source = source_dir.join(name);
            let destination = destination_dir.join(name);
            fs::write(&source, b"source").expect("create source file");
            fs::write(destination, b"destination").expect("create destination file");
            sources.push(source);
        }

        conflict_batch_from_result(operation(&sources, &destination_dir))
    }

    fn multi_source_conflict_batch() -> FileConflictBatch {
        let temp = TempDir::new();
        let first_source_dir = temp.path().join("source-a");
        let second_source_dir = temp.path().join("source-b");
        let destination_dir = temp.path().join("destination");
        fs::create_dir(&first_source_dir).expect("create first source folder");
        fs::create_dir(&second_source_dir).expect("create second source folder");
        fs::create_dir(&destination_dir).expect("create destination folder");

        let first_source = first_source_dir.join("a.txt");
        let second_source = second_source_dir.join("b.txt");
        fs::write(&first_source, b"source").expect("create first source file");
        fs::write(&second_source, b"source").expect("create second source file");
        fs::write(destination_dir.join("a.txt"), b"destination")
            .expect("create first destination file");
        fs::write(destination_dir.join("b.txt"), b"destination")
            .expect("create second destination file");

        conflict_batch_from_result(move_paths_to_directory(
            &[first_source, second_source],
            &destination_dir,
        ))
    }

    fn conflict_batch_from_result(
        result: Result<FileOperationOutcome, String>,
    ) -> FileConflictBatch {
        match result.expect("operation should succeed") {
            FileOperationOutcome::Conflicts(conflicts) => conflicts,
            FileOperationOutcome::Finished(_) => panic!("expected conflict batch"),
        }
    }

    fn assert_approx_eq(left: f32, right: f32) {
        assert!(
            (left - right).abs() <= f32::EPSILON,
            "expected {left} to equal {right}"
        );
    }
}
