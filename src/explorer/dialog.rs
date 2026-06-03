use gpui::{
    AnyElement, AnyWindowHandle, App, ClickEvent, Context, Entity, FocusHandle, Focusable,
    IntoElement, Render, SharedString, TitlebarOptions, WeakEntity, Window, WindowBounds,
    WindowDecorations, WindowKind, WindowOptions, actions, div, prelude::*, px, rgb, size,
};

use crate::explorer::{
    filesystem::FileConflictBatch,
    view::{ExplorerView, PendingPermanentDelete},
};

actions!(dialog, [DialogCancel]);

const DELETE_DIALOG_WIDTH: f32 = 380.0;
const DELETE_DIALOG_HEIGHT: f32 = 132.0;
const CONFLICT_DIALOG_WIDTH: f32 = 430.0;
const CONFLICT_DIALOG_HEIGHT: f32 = 190.0;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ExplorerDialogKind {
    PermanentDelete(PendingPermanentDelete),
    FileConflict(FileConflictBatch),
}

pub(super) struct ExplorerDialog {
    kind: ExplorerDialogKind,
    explorer: WeakEntity<ExplorerView>,
    focus_handle: FocusHandle,
    completed: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct PermanentDeleteDialogText {
    pub(super) message: String,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct FileConflictDialogText {
    pub(super) title: String,
    pub(super) detail: String,
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
        } else if let Some(conflicts) = self.pending_file_conflict.clone() {
            ExplorerDialogKind::FileConflict(conflicts)
        } else {
            return;
        };

        match open_dialog_window(kind, cx.entity(), cx) {
            Ok(handle) => self.active_dialog_window = Some(handle),
            Err(error) => self.open_error = Some(format!("Failed to open dialog: {error}")),
        }
    }

    pub(super) fn clear_active_dialog_window(&mut self) {
        self.active_dialog_window = None;
    }

    fn dialog_window_released(&mut self, kind: ExplorerDialogKind, completed: bool) {
        if !completed {
            match kind {
                ExplorerDialogKind::PermanentDelete(_) => self.cancel_pending_permanent_delete(),
                ExplorerDialogKind::FileConflict(_) => self.skip_pending_file_conflicts(),
            }
        }
        self.clear_active_dialog_window();
    }
}

impl ExplorerDialog {
    fn new(
        kind: ExplorerDialogKind,
        explorer: WeakEntity<ExplorerView>,
        focus_handle: FocusHandle,
    ) -> Self {
        Self {
            kind,
            explorer,
            focus_handle,
            completed: false,
        }
    }

    fn handle_cancel(&mut self, _: &DialogCancel, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel(window, cx);
    }

    fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.completed = true;
        let _ = self.explorer.update(cx, |explorer, cx| {
            explorer.confirm_pending_permanent_delete();
            explorer.clear_active_dialog_window();
            cx.notify();
        });
        window.remove_window();
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.completed = true;
        let kind = self.kind.clone();
        let _ = self.explorer.update(cx, |explorer, cx| {
            match kind {
                ExplorerDialogKind::PermanentDelete(_) => {
                    explorer.cancel_pending_permanent_delete()
                }
                ExplorerDialogKind::FileConflict(_) => explorer.skip_pending_file_conflicts(),
            }
            explorer.clear_active_dialog_window();
            cx.notify();
        });
        window.remove_window();
    }

    fn replace_conflicts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.completed = true;
        let _ = self.explorer.update(cx, |explorer, cx| {
            explorer.replace_pending_file_conflicts();
            explorer.clear_active_dialog_window();
            cx.notify();
        });
        window.remove_window();
    }

    fn release(&mut self, cx: &mut App) {
        let kind = self.kind.clone();
        let completed = self.completed;
        let _ = self.explorer.update(cx, move |explorer, cx| {
            explorer.dialog_window_released(kind, completed);
            cx.notify();
        });
    }
}

impl Render for ExplorerDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("ExplorerDialog")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(rgb(0xffffff))
            .cursor_default()
            .p(px(16.0))
            .text_size(px(12.0))
            .text_color(rgb(0x1f1f1f))
            .on_action(cx.listener(Self::handle_cancel))
            .child(match self.kind.clone() {
                ExplorerDialogKind::PermanentDelete(pending) => {
                    self.render_permanent_delete(pending, cx)
                }
                ExplorerDialogKind::FileConflict(conflicts) => {
                    self.render_file_conflict(conflicts, cx)
                }
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
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text = permanent_delete_dialog_text(&pending);

        div()
            .id("permanent-delete-confirmation")
            .flex()
            .flex_col()
            .size_full()
            .child(div().child(SharedString::from(text.message)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(8.0))
                    .mt(px(18.0))
                    .child(
                        dialog_button("permanent-delete-yes", "Yes").on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                this.confirm_delete(window, cx);
                                cx.stop_propagation();
                            },
                        )),
                    )
                    .child(
                        dialog_button("permanent-delete-no", "No").on_click(cx.listener(
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
            .size_full()
            .child(div().child(SharedString::from(text.title)))
            .child(
                div()
                    .mt(px(8.0))
                    .text_color(rgb(0x5f5f5f))
                    .child(SharedString::from(text.detail)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .mt(px(18.0))
                    .child(
                        dialog_wide_button("file-conflict-replace", text.replace_label).on_click(
                            cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.replace_conflicts(window, cx);
                                cx.stop_propagation();
                            }),
                        ),
                    )
                    .child(
                        dialog_wide_button("file-conflict-skip", text.skip_label).on_click(
                            cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.cancel(window, cx);
                                cx.stop_propagation();
                            }),
                        ),
                    ),
            )
            .into_any_element()
    }
}

fn open_dialog_window(
    kind: ExplorerDialogKind,
    explorer: Entity<ExplorerView>,
    cx: &mut Context<ExplorerView>,
) -> Result<AnyWindowHandle, String> {
    let options = dialog_window_options(&kind, cx);
    let handle = cx
        .open_window(options, |window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);

            cx.new(|cx| {
                cx.on_release(|dialog: &mut ExplorerDialog, cx| dialog.release(cx))
                    .detach();
                ExplorerDialog::new(kind, explorer.downgrade(), focus_handle)
            })
        })
        .map_err(|error| error.to_string())?;

    Ok(handle.into())
}

fn dialog_window_options(kind: &ExplorerDialogKind, cx: &App) -> WindowOptions {
    let (width, height) = match kind {
        ExplorerDialogKind::PermanentDelete(_) => (DELETE_DIALOG_WIDTH, DELETE_DIALOG_HEIGHT),
        ExplorerDialogKind::FileConflict(_) => (CONFLICT_DIALOG_WIDTH, CONFLICT_DIALOG_HEIGHT),
    };

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
        is_minimizable: false,
        window_decorations: Some(WindowDecorations::Server),
        ..Default::default()
    }
}

impl ExplorerDialogKind {
    fn window_title(&self) -> &'static str {
        match self {
            ExplorerDialogKind::PermanentDelete(_) => "Delete File",
            ExplorerDialogKind::FileConflict(_) => "Replace or Skip Files",
        }
    }
}

fn dialog_button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .min_w(px(76.0))
        .h(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        .border_1()
        .border_color(rgb(0xadadad))
        .bg(rgb(0xf5f5f5))
        .hover(|style| style.bg(rgb(0xe5f3ff)).border_color(rgb(0x0078d7)))
        .cursor_default()
        .child(label)
}

fn dialog_wide_button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .min_h(px(32.0))
        .flex()
        .items_center()
        .px(px(10.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(rgb(0xadadad))
        .bg(rgb(0xf5f5f5))
        .hover(|style| style.bg(rgb(0xe5f3ff)).border_color(rgb(0x0078d7)))
        .cursor_default()
        .child(label)
}

pub(super) fn permanent_delete_dialog_text(
    pending: &PendingPermanentDelete,
) -> PermanentDeleteDialogText {
    let message = if pending.paths.len() == 1 {
        "Are you sure you want to permanently delete this item?".to_owned()
    } else {
        format!(
            "Are you sure you want to permanently delete these {} items?",
            pending.paths.len()
        )
    };

    PermanentDeleteDialogText { message }
}

pub(super) fn file_conflict_dialog_text(conflicts: &FileConflictBatch) -> FileConflictDialogText {
    let count = conflicts.len();
    let title = if count == 1 {
        "There is already a file with the same name in this location.".to_owned()
    } else {
        format!("There are {count} files with the same names in this location.")
    };
    let detail = if count == 1 {
        format!(
            "Choose what to do with {}.",
            conflicts.first_destination_name()
        )
    } else {
        "The choice you make will apply to all conflicts in this operation.".to_owned()
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
        title,
        detail,
        replace_label,
        skip_label,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::{
        filesystem::{FileOperationOutcome, move_paths_to_directory},
        test_support::TempDir,
    };
    use std::{fs, path::PathBuf};

    #[test]
    fn permanent_delete_text_uses_singular_item_message() {
        let text = permanent_delete_dialog_text(&PendingPermanentDelete {
            paths: vec![PathBuf::from("a.txt")],
            fallback_index: None,
        });

        assert_eq!(
            text.message,
            "Are you sure you want to permanently delete this item?"
        );
    }

    #[test]
    fn permanent_delete_text_uses_plural_item_count() {
        let text = permanent_delete_dialog_text(&PendingPermanentDelete {
            paths: vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")],
            fallback_index: None,
        });

        assert_eq!(
            text.message,
            "Are you sure you want to permanently delete these 2 items?"
        );
    }

    #[test]
    fn file_conflict_text_uses_single_file_labels() {
        let conflicts = single_conflict_batch();
        let text = file_conflict_dialog_text(&conflicts);

        assert_eq!(
            text.title,
            "There is already a file with the same name in this location."
        );
        assert_eq!(text.detail, "Choose what to do with file.txt.");
        assert_eq!(text.replace_label, "Replace the file in the destination");
        assert_eq!(text.skip_label, "Skip this file");
    }

    #[test]
    fn file_conflict_text_uses_plural_file_labels() {
        let conflicts = multi_conflict_batch();
        let text = file_conflict_dialog_text(&conflicts);

        assert_eq!(
            text.title,
            "There are 2 files with the same names in this location."
        );
        assert_eq!(
            text.detail,
            "The choice you make will apply to all conflicts in this operation."
        );
        assert_eq!(text.replace_label, "Replace the files in the destination");
        assert_eq!(text.skip_label, "Skip these files");
    }

    #[test]
    fn dialog_window_release_cancels_pending_delete() {
        let mut view = ExplorerView::new(PathBuf::from("delete"));
        view.pending_permanent_delete = Some(PendingPermanentDelete {
            paths: vec![PathBuf::from("a.txt")],
            fallback_index: None,
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
            fallback_index: None,
        });

        view.dialog_window_released(
            ExplorerDialogKind::PermanentDelete(view.pending_permanent_delete.clone().unwrap()),
            true,
        );

        assert!(view.pending_permanent_delete.is_some());
        assert!(view.active_dialog_window.is_none());
    }

    fn single_conflict_batch() -> FileConflictBatch {
        conflict_batch(&["file.txt"])
    }

    fn multi_conflict_batch() -> FileConflictBatch {
        conflict_batch(&["a.txt", "b.txt"])
    }

    fn conflict_batch(names: &[&str]) -> FileConflictBatch {
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

        match move_paths_to_directory(&sources, &destination_dir)
            .expect("move operation should succeed")
        {
            FileOperationOutcome::Conflicts(conflicts) => conflicts,
            FileOperationOutcome::Finished(_) => panic!("expected conflict batch"),
        }
    }
}
