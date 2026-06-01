use std::{
    any::Any,
    ffi::OsStr,
    path::{Path, PathBuf},
};

use gpui::{
    Context, CursorStyle, Modifiers, Pixels, Point, Render, SharedString, Window, div, prelude::*,
    px, rgb,
};

use crate::explorer::{
    filesystem::{copy_paths_to_directory, move_paths_to_directory},
    view::ExplorerView,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DraggedEntries {
    pub(super) paths: Vec<PathBuf>,
    pub(super) source_dir: PathBuf,
    pub(super) display_name: String,
    pub(super) count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum DropDestination {
    CurrentDirectory,
    Directory(PathBuf),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FileOperationKind {
    Move,
    Copy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ResolvedDrop {
    Move,
    Copy,
    UnsupportedShortcut,
    Invalid,
}

#[derive(Clone, Debug)]
pub(super) struct DragPreview {
    label: SharedString,
    cursor_offset: Point<Pixels>,
}

const DRAG_PREVIEW_WIDTH: f32 = 160.0;
const DRAG_PREVIEW_HEIGHT: f32 = 24.0;
const DRAG_PREVIEW_VERTICAL_PADDING: f32 = 4.0;
const DRAG_PREVIEW_HORIZONTAL_PADDING: f32 = 8.0;

impl DragPreview {
    pub(super) fn new(dragged: &DraggedEntries, cursor_offset: Point<Pixels>) -> Self {
        let label = if dragged.count == 1 {
            dragged.display_name.clone()
        } else {
            format!("{} items", dragged.count)
        };

        Self {
            label: SharedString::from(label),
            cursor_offset,
        }
    }
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let origin = drag_preview_origin(self.cursor_offset);
        let root_width = f32::from(self.cursor_offset.x) + (DRAG_PREVIEW_WIDTH / 2.0);
        let root_height = f32::from(self.cursor_offset.y) + DRAG_PREVIEW_HEIGHT;

        div().relative().w(px(root_width)).h(px(root_height)).child(
            div()
                .absolute()
                .left(px(origin.0))
                .top(px(origin.1))
                .w(px(DRAG_PREVIEW_WIDTH))
                .h(px(DRAG_PREVIEW_HEIGHT))
                .flex()
                .items_center()
                .px(px(DRAG_PREVIEW_HORIZONTAL_PADDING))
                .py(px(DRAG_PREVIEW_VERTICAL_PADDING))
                .rounded(px(3.0))
                .bg(rgb(0xffffff))
                .border_1()
                .border_color(rgb(0x8a8a8a))
                .shadow_md()
                .text_size(px(12.0))
                .text_color(rgb(0x1f1f1f))
                .child(
                    div()
                        .min_w(px(0.0))
                        .w_full()
                        .truncate()
                        .child(self.label.clone()),
                ),
        )
    }
}

pub(super) fn drag_preview_origin(cursor_offset: Point<Pixels>) -> (f32, f32) {
    (
        f32::from(cursor_offset.x) - (DRAG_PREVIEW_WIDTH / 2.0),
        f32::from(cursor_offset.y),
    )
}

impl DropDestination {
    pub(super) fn resolve(&self, current_directory: &Path) -> PathBuf {
        match self {
            Self::CurrentDirectory => current_directory.to_path_buf(),
            Self::Directory(path) => path.clone(),
        }
    }
}

impl ResolvedDrop {
    pub(super) fn operation(self) -> Option<FileOperationKind> {
        match self {
            Self::Move => Some(FileOperationKind::Move),
            Self::Copy => Some(FileOperationKind::Copy),
            Self::UnsupportedShortcut | Self::Invalid => None,
        }
    }

    pub(super) fn cursor_style(self) -> CursorStyle {
        match self {
            Self::Copy => CursorStyle::DragCopy,
            Self::UnsupportedShortcut | Self::Invalid => CursorStyle::OperationNotAllowed,
            Self::Move => CursorStyle::Arrow,
        }
    }
}

impl DraggedEntries {
    fn new(paths: Vec<PathBuf>, source_dir: PathBuf) -> Option<Self> {
        let first = paths.first()?;
        let display_name = path_display_name(first);
        let count = paths.len();

        Some(Self {
            paths,
            source_dir,
            display_name,
            count,
        })
    }
}

impl ExplorerView {
    pub(super) fn dragged_entries_for_index(&self, ix: usize) -> Option<DraggedEntries> {
        self.entries.get(ix)?;
        if !self.entry_is_selected(ix) {
            return None;
        }

        DraggedEntries::new(self.selected_paths(), self.path.clone())
    }

    #[cfg(test)]
    pub(super) fn test_dragged_entries_for_index(&self, ix: usize) -> Option<DraggedEntries> {
        self.dragged_entries_for_index(ix)
    }

    pub(super) fn drag_cursor_for_value(
        &self,
        dragged_value: &dyn Any,
        destination: &DropDestination,
        modifiers: Modifiers,
    ) -> CursorStyle {
        let destination = destination.resolve(&self.path);
        resolve_dragged_value_drop(dragged_value, &destination, modifiers).cursor_style()
    }

    pub(super) fn can_drop_value(
        &self,
        dragged_value: &dyn Any,
        destination: &DropDestination,
        modifiers: Modifiers,
    ) -> bool {
        let destination = destination.resolve(&self.path);
        resolve_dragged_value_drop(dragged_value, &destination, modifiers)
            .operation()
            .is_some()
    }

    pub(super) fn drop_internal_entries(
        &mut self,
        dragged: &DraggedEntries,
        destination: DropDestination,
        modifiers: Modifiers,
    ) {
        if matches!(destination, DropDestination::CurrentDirectory) {
            self.open_error = None;
            return;
        }

        let destination = destination.resolve(&self.path);
        self.perform_file_drop(&dragged.paths, &destination, modifiers);
    }

    pub(super) fn drop_external_paths(
        &mut self,
        paths: &[PathBuf],
        destination: DropDestination,
        modifiers: Modifiers,
    ) {
        let destination = destination.resolve(&self.path);
        self.perform_file_drop(paths, &destination, modifiers);
    }

    fn perform_file_drop(&mut self, paths: &[PathBuf], destination: &Path, modifiers: Modifiers) {
        match resolve_drop_operation(modifiers, destination.is_dir()) {
            ResolvedDrop::Move => {
                self.handle_file_operation_result(move_paths_to_directory(paths, destination));
            }
            ResolvedDrop::Copy => {
                self.handle_file_operation_result(copy_paths_to_directory(paths, destination));
            }
            ResolvedDrop::UnsupportedShortcut => {
                self.open_error = Some("Shortcut drag-and-drop is not supported yet.".to_owned());
            }
            ResolvedDrop::Invalid => {
                self.open_error = Some("This drop target is not valid.".to_owned());
            }
        }
    }

    fn handle_file_operation_result(&mut self, result: Result<Vec<PathBuf>, String>) {
        match result {
            Ok(moved_or_copied_paths) => {
                self.open_error = None;
                self.reload();
                self.restore_selection_from_paths(&moved_or_copied_paths);
            }
            Err(error) => {
                self.open_error = Some(error);
                self.reload();
            }
        }
    }
}

pub(super) fn resolve_drop_operation(modifiers: Modifiers, valid_target: bool) -> ResolvedDrop {
    if !valid_target {
        return ResolvedDrop::Invalid;
    }

    if modifiers.alt || (modifiers.secondary() && modifiers.shift) {
        return ResolvedDrop::UnsupportedShortcut;
    }

    if modifiers.secondary() {
        ResolvedDrop::Copy
    } else {
        ResolvedDrop::Move
    }
}

fn resolve_dragged_value_drop(
    dragged_value: &dyn Any,
    destination: &Path,
    modifiers: Modifiers,
) -> ResolvedDrop {
    let valid_target = destination.is_dir()
        && (dragged_value.is::<DraggedEntries>()
            || dragged_value
                .downcast_ref::<gpui::ExternalPaths>()
                .is_some_and(|paths| !paths.paths().is_empty()));

    resolve_drop_operation(modifiers, valid_target)
}

fn path_display_name(path: &Path) -> String {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::{
        selection::SelectionModifiers,
        test_support::{selected_names, test_view_with_entries},
    };

    #[test]
    fn modifiers_resolve_drag_operation() {
        assert_eq!(
            resolve_drop_operation(Modifiers::default(), true),
            ResolvedDrop::Move
        );
        assert_eq!(
            resolve_drop_operation(
                Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
                true,
            ),
            ResolvedDrop::Move
        );
        assert_eq!(
            resolve_drop_operation(
                Modifiers {
                    control: true,
                    platform: cfg!(target_os = "macos"),
                    ..Modifiers::default()
                },
                true,
            ),
            ResolvedDrop::Copy
        );
        assert_eq!(
            resolve_drop_operation(
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
                true,
            ),
            ResolvedDrop::UnsupportedShortcut
        );
        assert_eq!(
            resolve_drop_operation(
                Modifiers {
                    control: true,
                    platform: cfg!(target_os = "macos"),
                    shift: true,
                    ..Modifiers::default()
                },
                true,
            ),
            ResolvedDrop::UnsupportedShortcut
        );
        assert_eq!(
            resolve_drop_operation(Modifiers::default(), false),
            ResolvedDrop::Invalid
        );
    }

    #[test]
    fn selected_row_drag_includes_current_selection() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);
        view.select_single_index(0);
        view.apply_click_selection(
            2,
            SelectionModifiers {
                toggle: true,
                extend: false,
            },
        );

        let dragged = view.test_dragged_entries_for_index(0).expect("dragged row");

        assert_eq!(
            dragged.paths,
            vec![PathBuf::from("a.txt"), PathBuf::from("c.txt")]
        );
        assert_eq!(dragged.source_dir, PathBuf::from("selection"));
        assert_eq!(dragged.display_name, "a.txt");
        assert_eq!(dragged.count, 2);
        assert_eq!(selected_names(&view), vec!["a.txt", "c.txt"]);
    }

    #[test]
    fn unselected_row_drag_has_no_dnd_payload() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);
        view.select_single_index(0);

        let dragged = view.test_dragged_entries_for_index(1);

        assert!(dragged.is_none());
        assert_eq!(selected_names(&view), vec!["a.txt"]);
    }

    #[test]
    fn drag_preview_origin_centers_preview_on_cursor_offset() {
        let cursor_offset = gpui::point(px(120.0), px(32.0));

        let origin = drag_preview_origin(cursor_offset);

        assert_eq!(origin.0, 120.0 - (DRAG_PREVIEW_WIDTH / 2.0));
    }

    #[test]
    fn drag_preview_origin_keeps_top_at_cursor_offset() {
        let cursor_offset = gpui::point(px(120.0), px(32.0));

        let origin = drag_preview_origin(cursor_offset);

        assert_eq!(origin.1, 32.0);
    }
}
