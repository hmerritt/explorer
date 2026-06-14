use std::{
    any::Any,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::path::{Component, Prefix};

use gpui::{
    Context, CursorStyle, Modifiers, Pixels, Point, Render, SharedString, TextRun, Window, div,
    prelude::*, px, rgb,
};

use crate::explorer::{
    entry::FileEntry,
    filesystem::{prepare_copy_paths_to_directory, prepare_move_paths_to_directory},
    view::ExplorerView,
};

#[cfg(test)]
use crate::explorer::filesystem::{copy_paths_to_directory, move_paths_to_directory};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DraggedEntries {
    pub(super) paths: Vec<PathBuf>,
    pub(super) source_dir: PathBuf,
    pub(super) display_name: String,
    pub(super) count: usize,
    pub(super) folder_count: usize,
    pub(super) file_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum DropDestination {
    CurrentDirectory,
    Directory {
        item_path: PathBuf,
        target_path: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FileOperationKind {
    Move,
    Copy,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct DropIndicator {
    pub(super) operation: FileOperationKind,
    pub(super) target_label: String,
    pub(super) mouse_position: Point<Pixels>,
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
    font: gpui::Font,
}

const DRAG_PREVIEW_WIDTH: f32 = 160.0;
const DRAG_PREVIEW_HEIGHT: f32 = 28.0;
const DRAG_PREVIEW_VERTICAL_PADDING: f32 = 4.0;
const DRAG_PREVIEW_HORIZONTAL_PADDING: f32 = 8.0;
const DRAG_PREVIEW_CURSOR_OVERLAP: f32 = 10.0;
const DRAG_PREVIEW_TEXT_SIZE: f32 = 12.0;
const DRAG_PREVIEW_TEXT_COLOR: u32 = 0x1f1f1f;
const DRAG_PREVIEW_TRUNCATION_SUFFIX: &str = "…";

impl DragPreview {
    pub(super) fn new(
        dragged: &DraggedEntries,
        cursor_offset: Point<Pixels>,
        font: gpui::Font,
    ) -> Self {
        Self {
            label: SharedString::from(drag_preview_label(dragged)),
            cursor_offset,
            font,
        }
    }
}

impl Render for DragPreview {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let origin = drag_preview_origin(self.cursor_offset);
        let root_width = f32::from(self.cursor_offset.x) + (DRAG_PREVIEW_WIDTH / 2.0);
        let root_height = f32::from(self.cursor_offset.y) + DRAG_PREVIEW_CURSOR_OVERLAP;
        let label = truncated_drag_preview_label(&self.label, &self.font, window);

        div()
            .font(self.font.clone())
            .relative()
            .w(px(root_width))
            .h(px(root_height))
            .child(
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
                    .text_size(px(DRAG_PREVIEW_TEXT_SIZE))
                    .text_color(rgb(DRAG_PREVIEW_TEXT_COLOR))
                    .child(div().min_w(px(0.0)).w_full().truncate().child(label)),
            )
    }
}

fn truncated_drag_preview_label(
    label: &str,
    label_font: &gpui::Font,
    window: &Window,
) -> SharedString {
    let mut runs = vec![TextRun {
        len: label.len(),
        font: label_font.clone(),
        color: rgb(DRAG_PREVIEW_TEXT_COLOR).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }];

    window
        .text_system()
        .line_wrapper(label_font.clone(), px(DRAG_PREVIEW_TEXT_SIZE))
        .truncate_line(
            SharedString::from(label.to_owned()),
            px(drag_preview_text_width()),
            DRAG_PREVIEW_TRUNCATION_SUFFIX,
            &mut runs,
        )
}

fn drag_preview_text_width() -> f32 {
    (DRAG_PREVIEW_WIDTH - (DRAG_PREVIEW_HORIZONTAL_PADDING * 2.0)).max(0.0)
}

pub(super) fn drag_preview_origin(cursor_offset: Point<Pixels>) -> (f32, f32) {
    (
        f32::from(cursor_offset.x) - (DRAG_PREVIEW_WIDTH / 2.0),
        f32::from(cursor_offset.y) - DRAG_PREVIEW_HEIGHT + DRAG_PREVIEW_CURSOR_OVERLAP,
    )
}

pub(super) fn drop_indicator_origin(mouse_position: Point<Pixels>) -> (f32, f32) {
    let drag_origin = drag_preview_origin(mouse_position);
    (
        f32::from(mouse_position.x),
        drag_origin.1 + DRAG_PREVIEW_HEIGHT - 1.0, // - 1.0 so there is not a double-border for each drop ui box
    )
}

impl DropDestination {
    pub(super) fn resolve(&self, current_directory: &Path) -> PathBuf {
        match self {
            Self::CurrentDirectory => current_directory.to_path_buf(),
            Self::Directory { target_path, .. } => target_path.clone(),
        }
    }

    pub(super) fn item_path(&self, current_directory: &Path) -> PathBuf {
        match self {
            Self::CurrentDirectory => current_directory.to_path_buf(),
            Self::Directory { item_path, .. } => item_path.clone(),
        }
    }
}

pub(super) fn row_drop_destination_for_entry(entry: &FileEntry) -> DropDestination {
    if entry.is_directory_like() {
        DropDestination::Directory {
            item_path: entry.path.clone(),
            target_path: entry.drop_target_path().to_path_buf(),
        }
    } else {
        DropDestination::CurrentDirectory
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
    fn new(entries: Vec<&FileEntry>, source_dir: PathBuf) -> Option<Self> {
        let first = entries.first()?;
        let display_name = path_display_name(&first.path);
        let count = entries.len();
        let folder_count = entries
            .iter()
            .filter(|entry| entry.is_directory_like())
            .count();
        let file_count = count - folder_count;
        let paths = entries
            .into_iter()
            .map(|entry| entry.path.clone())
            .collect();

        Some(Self {
            paths,
            source_dir,
            display_name,
            count,
            folder_count,
            file_count,
        })
    }
}

impl ExplorerView {
    pub(super) fn dragged_entries_for_index(&self, ix: usize) -> Option<DraggedEntries> {
        self.entries.get(ix)?;
        if !self.entry_is_selected(ix) {
            return None;
        }

        let entries = self
            .selection
            .selected_indices
            .iter()
            .filter_map(|ix| self.entries.get(*ix))
            .collect();
        DraggedEntries::new(entries, self.path.clone())
    }

    pub(super) fn dragged_entry_for_index(&self, ix: usize) -> Option<DraggedEntries> {
        DraggedEntries::new(vec![self.entries.get(ix)?], self.path.clone())
    }

    pub(super) fn can_start_item_drag_for_index(&self, ix: usize) -> bool {
        self.mouse_selection_drag.is_none() && self.dragged_entries_for_index(ix).is_some()
    }

    pub(super) fn can_start_individual_item_drag_for_index(&self, ix: usize) -> bool {
        self.mouse_selection_drag.is_none()
            && !self.entry_is_selected(ix)
            && self.dragged_entry_for_index(ix).is_some()
    }

    pub(super) fn begin_individual_item_drag(&mut self, dragged: &DraggedEntries) {
        self.cancel_mouse_selection_drag();
        self.restore_selection_from_paths(&dragged.paths);
    }

    #[cfg(test)]
    pub(super) fn test_dragged_entries_for_index(&self, ix: usize) -> Option<DraggedEntries> {
        self.dragged_entries_for_index(ix)
    }

    #[cfg(test)]
    pub(super) fn test_dragged_entry_for_index(&self, ix: usize) -> Option<DraggedEntries> {
        self.dragged_entry_for_index(ix)
    }

    pub(super) fn drag_cursor_for_value(
        &self,
        dragged_value: &dyn Any,
        destination: &DropDestination,
        modifiers: Modifiers,
    ) -> CursorStyle {
        let destination_item = destination.item_path(&self.path);
        let resolved_destination = destination.resolve(&self.path);
        resolve_dragged_value_drop(
            dragged_value,
            destination,
            &self.path,
            &destination_item,
            &resolved_destination,
            modifiers,
        )
        .cursor_style()
    }

    pub(super) fn can_drop_value(
        &self,
        dragged_value: &dyn Any,
        destination: &DropDestination,
        modifiers: Modifiers,
    ) -> bool {
        let destination_item = destination.item_path(&self.path);
        let resolved_destination = destination.resolve(&self.path);
        resolve_dragged_value_drop(
            dragged_value,
            destination,
            &self.path,
            &destination_item,
            &resolved_destination,
            modifiers,
        )
        .operation()
        .is_some()
    }

    pub(super) fn can_trash_drop_value(&self, dragged_value: &dyn Any) -> bool {
        if let Some(dragged) = dragged_value.downcast_ref::<DraggedEntries>() {
            return !dragged.paths.is_empty();
        }

        dragged_value
            .downcast_ref::<gpui::ExternalPaths>()
            .is_some_and(|paths| !paths.paths().is_empty())
    }

    pub(super) fn drop_indicator_for_value(
        &self,
        dragged_value: &dyn Any,
        destination: &DropDestination,
        modifiers: Modifiers,
        mouse_position: Point<Pixels>,
    ) -> Option<DropIndicator> {
        let destination_item = destination.item_path(&self.path);
        let resolved_destination = destination.resolve(&self.path);
        let operation = resolve_dragged_value_drop(
            dragged_value,
            destination,
            &self.path,
            &destination_item,
            &resolved_destination,
            modifiers,
        )
        .operation()?;

        Some(DropIndicator {
            operation,
            target_label: drop_target_display_name(destination, &self.path),
            mouse_position,
        })
    }

    pub(super) fn clear_drop_indicator(&mut self) -> bool {
        self.active_drop_indicator.take().is_some()
    }

    pub(super) fn clear_stale_drop_indicator(&mut self, mouse_position: Point<Pixels>) -> bool {
        let Some(indicator) = &self.active_drop_indicator else {
            return false;
        };

        if indicator.mouse_position == mouse_position {
            false
        } else {
            self.active_drop_indicator = None;
            true
        }
    }

    pub(super) fn update_drop_indicator_modifiers(&mut self, modifiers: Modifiers) -> bool {
        let Some(indicator) = self.active_drop_indicator.as_mut() else {
            return false;
        };

        let Some(operation) = resolve_drop_operation(modifiers, true).operation() else {
            self.active_drop_indicator = None;
            return true;
        };

        if indicator.operation == operation {
            false
        } else {
            indicator.operation = operation;
            true
        }
    }

    #[cfg(test)]
    pub(super) fn drop_internal_entries(
        &mut self,
        dragged: &DraggedEntries,
        destination: DropDestination,
        modifiers: Modifiers,
    ) {
        let destination_item = destination.item_path(&self.path);
        let resolved_destination = destination.resolve(&self.path);
        if dragged.paths.iter().any(|path| path == &destination_item)
            || destination_contains_internal_drag_source(
                &destination,
                &self.path,
                &resolved_destination,
                dragged,
            )
        {
            return;
        }

        self.perform_file_drop(&dragged.paths, &resolved_destination, modifiers);
    }

    pub(super) fn drop_internal_entries_and_open_dialog(
        &mut self,
        dragged: &DraggedEntries,
        destination: DropDestination,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let destination_item = destination.item_path(&self.path);
        let resolved_destination = destination.resolve(&self.path);
        if dragged.paths.iter().any(|path| path == &destination_item)
            || destination_contains_internal_drag_source(
                &destination,
                &self.path,
                &resolved_destination,
                dragged,
            )
        {
            return;
        }

        self.perform_file_drop_and_open_dialog(
            &dragged.paths,
            &resolved_destination,
            modifiers,
            cx,
        );
    }

    #[cfg(test)]
    pub(super) fn drop_external_paths(
        &mut self,
        paths: &[PathBuf],
        destination: DropDestination,
        modifiers: Modifiers,
    ) {
        let paths = normalize_external_drop_paths(paths);
        if paths.is_empty() {
            return;
        }

        let resolved_destination = destination.resolve(&self.path);
        if destination_contains_all_external_path_sources(
            &destination,
            &self.path,
            &resolved_destination,
            &paths,
        ) {
            return;
        }

        self.perform_file_drop(&paths, &resolved_destination, modifiers);
    }

    pub(super) fn drop_external_paths_and_open_dialog(
        &mut self,
        paths: &[PathBuf],
        destination: DropDestination,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let paths = normalize_external_drop_paths(paths);
        if paths.is_empty() {
            return;
        }

        let resolved_destination = destination.resolve(&self.path);
        if destination_contains_all_external_path_sources(
            &destination,
            &self.path,
            &resolved_destination,
            &paths,
        ) {
            return;
        }

        self.perform_file_drop_and_open_dialog(&paths, &resolved_destination, modifiers, cx);
    }

    #[cfg(test)]
    fn perform_file_drop(&mut self, paths: &[PathBuf], destination: &Path, modifiers: Modifiers) {
        match resolve_drop_operation_for_paths(modifiers, destination.is_dir(), paths, destination)
        {
            ResolvedDrop::Move => {
                self.handle_file_command_result(move_paths_to_directory(paths, destination));
            }
            ResolvedDrop::Copy => {
                self.handle_file_command_result(copy_paths_to_directory(paths, destination));
            }
            ResolvedDrop::UnsupportedShortcut => {
                self.open_error = Some("Shortcut drag-and-drop is not supported yet.".to_owned());
            }
            ResolvedDrop::Invalid => {
                self.open_error = Some("This drop target is not valid.".to_owned());
            }
        }
    }

    fn perform_file_drop_and_open_dialog(
        &mut self,
        paths: &[PathBuf],
        destination: &Path,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        match resolve_drop_operation_for_paths(modifiers, destination.is_dir(), paths, destination)
        {
            ResolvedDrop::Move => {
                self.handle_prepared_file_command_result_and_open_dialog(
                    prepare_move_paths_to_directory(paths, destination),
                    cx,
                );
            }
            ResolvedDrop::Copy => {
                self.handle_prepared_file_command_result_and_open_dialog(
                    prepare_copy_paths_to_directory(paths, destination),
                    cx,
                );
            }
            ResolvedDrop::UnsupportedShortcut => {
                self.open_error = Some("Shortcut drag-and-drop is not supported yet.".to_owned());
            }
            ResolvedDrop::Invalid => {
                self.open_error = Some("This drop target is not valid.".to_owned());
            }
        }
    }
}

pub(super) fn resolve_drop_operation(modifiers: Modifiers, valid_target: bool) -> ResolvedDrop {
    resolve_drop_operation_for_paths(modifiers, valid_target, &[], Path::new(""))
}

pub(super) fn resolve_drop_operation_for_paths(
    modifiers: Modifiers,
    valid_target: bool,
    source_paths: &[PathBuf],
    destination: &Path,
) -> ResolvedDrop {
    if !valid_target {
        return ResolvedDrop::Invalid;
    }

    if modifiers.alt || (modifiers.secondary() && modifiers.shift) {
        return ResolvedDrop::UnsupportedShortcut;
    }

    if modifiers.secondary() {
        ResolvedDrop::Copy
    } else if modifiers.shift {
        ResolvedDrop::Move
    } else if drop_should_copy_by_default(source_paths, destination) {
        ResolvedDrop::Copy
    } else {
        ResolvedDrop::Move
    }
}

fn resolve_dragged_value_drop(
    dragged_value: &dyn Any,
    destination_kind: &DropDestination,
    current_directory: &Path,
    destination_item: &Path,
    destination: &Path,
    modifiers: Modifiers,
) -> ResolvedDrop {
    let valid_target = if let Some(dragged) = dragged_value.downcast_ref::<DraggedEntries>() {
        destination.is_dir()
            && !dragged.paths.iter().any(|path| path == destination_item)
            && !destination_contains_internal_drag_source(
                destination_kind,
                current_directory,
                destination,
                dragged,
            )
    } else {
        destination.is_dir()
            && dragged_value
                .downcast_ref::<gpui::ExternalPaths>()
                .is_some_and(|paths| {
                    let paths = normalize_external_drop_paths(paths.paths());
                    !paths.is_empty()
                        && !destination_contains_all_external_path_sources(
                            destination_kind,
                            current_directory,
                            destination,
                            &paths,
                        )
                })
    };

    let source_paths = if let Some(dragged) = dragged_value.downcast_ref::<DraggedEntries>() {
        dragged.paths.as_slice()
    } else if let Some(paths) = dragged_value.downcast_ref::<gpui::ExternalPaths>() {
        paths.paths()
    } else {
        &[]
    };

    resolve_drop_operation_for_paths(modifiers, valid_target, source_paths, destination)
}

fn normalize_external_drop_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter_map(|path| {
            fs::canonicalize(path)
                .ok()
                .or_else(|| path.exists().then(|| path.clone()))
        })
        .collect()
}

fn drop_should_copy_by_default(source_paths: &[PathBuf], destination: &Path) -> bool {
    !source_paths.is_empty()
        && source_paths
            .iter()
            .any(|source| !paths_are_on_same_volume(source, destination))
}

fn paths_are_on_same_volume(source: &Path, destination: &Path) -> bool {
    match (path_volume_key(source), path_volume_key(destination)) {
        (Some(source), Some(destination)) => source == destination,
        _ => true,
    }
}

#[cfg(windows)]
fn path_volume_key(path: &Path) -> Option<String> {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let Component::Prefix(prefix) = path.components().next()? else {
        return None;
    };

    Some(match prefix.kind() {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
            char::from(letter).to_ascii_uppercase().to_string()
        }
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            format!(
                r"\\{}\{}",
                server.to_string_lossy().to_ascii_lowercase(),
                share.to_string_lossy().to_ascii_lowercase()
            )
        }
        _ => prefix.as_os_str().to_string_lossy().to_ascii_lowercase(),
    })
}

#[cfg(unix)]
fn path_volume_key(path: &Path) -> Option<u64> {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let metadata = fs::metadata(path).ok()?;
    Some(metadata.dev())
}

#[cfg(not(any(windows, unix)))]
fn path_volume_key(_: &Path) -> Option<()> {
    None
}

fn destination_contains_internal_drag_source(
    destination: &DropDestination,
    current_directory: &Path,
    resolved_destination: &Path,
    dragged: &DraggedEntries,
) -> bool {
    let target_directory = match destination {
        DropDestination::CurrentDirectory => current_directory,
        DropDestination::Directory { .. } => resolved_destination,
    };

    paths_match_for_drop(&dragged.source_dir, target_directory)
}

fn destination_contains_all_external_path_sources(
    destination: &DropDestination,
    current_directory: &Path,
    resolved_destination: &Path,
    paths: &[PathBuf],
) -> bool {
    let target_directory = match destination {
        DropDestination::CurrentDirectory => current_directory,
        DropDestination::Directory { .. } => resolved_destination,
    };

    !paths.is_empty()
        && paths.iter().all(|path| {
            path.parent()
                .is_some_and(|parent| paths_match_for_drop(parent, target_directory))
        })
}

fn paths_match_for_drop(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn path_display_name(path: &Path) -> String {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn drop_target_display_name(destination: &DropDestination, current_directory: &Path) -> String {
    path_display_name(&destination.item_path(current_directory))
}

fn drag_preview_label(dragged: &DraggedEntries) -> String {
    if dragged.count == 1 {
        return dragged.display_name.clone();
    }

    let mut parts = Vec::new();
    if dragged.folder_count > 0 {
        parts.push(count_label(dragged.folder_count, "folder", "folders"));
    }
    if dragged.file_count > 0 {
        parts.push(count_label(dragged.file_count, "file", "files"));
    }
    parts.join(", ")
}

fn count_label(count: usize, singular: &str, plural: &str) -> String {
    let name = if count == 1 { singular } else { plural };
    format!("{count} {name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::{
        constants::ROW_HEIGHT,
        entry::{DirectoryLinkKind, FileEntry, ShellShortcutTargetKind},
        selection::SelectionModifiers,
        test_support::{TempDir, selected_names, test_view_with_entries},
    };
    use std::fs;

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
    fn source_aware_drop_defaults_to_move_on_same_volume() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::write(&source, b"data").expect("create source");
        fs::create_dir(&destination).expect("create destination");

        assert_eq!(
            resolve_drop_operation_for_paths(
                Modifiers::default(),
                true,
                std::slice::from_ref(&source),
                &destination,
            ),
            ResolvedDrop::Move
        );
    }

    #[cfg(windows)]
    #[test]
    fn source_aware_drop_defaults_to_copy_across_windows_volumes() {
        assert_eq!(
            resolve_drop_operation_for_paths(
                Modifiers::default(),
                true,
                &[PathBuf::from(r"C:\source\file.txt")],
                Path::new(r"D:\destination"),
            ),
            ResolvedDrop::Copy
        );
    }

    #[test]
    fn shift_forces_move_even_when_default_would_copy() {
        assert_eq!(
            resolve_drop_operation_for_paths(
                Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
                true,
                &[PathBuf::from(r"C:\source\file.txt")],
                Path::new(r"D:\destination"),
            ),
            ResolvedDrop::Move
        );
    }

    #[test]
    fn external_paths_constructor_preserves_paths() {
        let paths = vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")];
        let external_paths = gpui::ExternalPaths::new(paths.clone());

        assert_eq!(external_paths.paths(), paths.as_slice());
    }

    #[test]
    fn file_row_drop_target_is_current_directory() {
        let entry = FileEntry::test("file.txt", false, Some(1), None);

        assert_eq!(
            row_drop_destination_for_entry(&entry),
            DropDestination::CurrentDirectory
        );
    }

    #[test]
    fn directory_row_drop_target_is_directory() {
        let entry = FileEntry::test("folder", true, None, None);

        assert_eq!(
            row_drop_destination_for_entry(&entry),
            DropDestination::Directory {
                item_path: PathBuf::from("folder"),
                target_path: PathBuf::from("folder"),
            }
        );
    }

    #[test]
    fn directory_link_row_drop_target_is_directory() {
        let target = PathBuf::from("target");
        let entry = FileEntry::test_directory_link(
            "shortcut",
            DirectoryLinkKind::ShellShortcut {
                target: target.clone(),
                target_kind: ShellShortcutTargetKind::Directory,
            },
        );

        assert_eq!(
            row_drop_destination_for_entry(&entry),
            DropDestination::Directory {
                item_path: PathBuf::from("shortcut"),
                target_path: target,
            }
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
        assert_eq!(dragged.folder_count, 0);
        assert_eq!(dragged.file_count, 2);
        assert_eq!(drag_preview_label(&dragged), "2 files");
        assert_eq!(selected_names(&view), vec!["a.txt", "c.txt"]);
    }

    #[test]
    fn unselected_row_drag_has_no_dnd_payload() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);
        view.select_single_index(0);

        let dragged = view.test_dragged_entries_for_index(1);

        assert!(dragged.is_none());
        assert!(!view.can_start_item_drag_for_index(1));
        assert_eq!(selected_names(&view), vec!["a.txt"]);
    }

    #[test]
    fn unselected_individual_row_drag_payload_uses_only_that_row() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);
        view.select_single_index(0);

        let dragged = view
            .test_dragged_entry_for_index(1)
            .expect("individual dragged row");

        assert_eq!(dragged.paths, vec![PathBuf::from("b.txt")]);
        assert_eq!(dragged.source_dir, PathBuf::from("selection"));
        assert_eq!(dragged.display_name, "b.txt");
        assert_eq!(dragged.count, 1);
        assert_eq!(dragged.folder_count, 0);
        assert_eq!(dragged.file_count, 1);
        assert_eq!(selected_names(&view), vec!["a.txt"]);
    }

    #[test]
    fn unselected_individual_row_drag_can_start_without_mutating_selection() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);
        view.select_single_index(0);

        assert!(view.can_start_individual_item_drag_for_index(1));
        assert_eq!(selected_names(&view), vec!["a.txt"]);
    }

    #[test]
    fn individual_row_drag_replaces_existing_selection_with_dragged_row() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);
        view.select_single_index(0);
        let dragged = view
            .test_dragged_entry_for_index(1)
            .expect("individual dragged row");

        view.begin_individual_item_drag(&dragged);

        assert_eq!(selected_names(&view), vec!["b.txt"]);
    }

    #[test]
    fn selected_row_does_not_use_individual_drag_payload() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        view.select_single_index(0);

        assert!(!view.can_start_individual_item_drag_for_index(0));
        assert!(view.can_start_item_drag_for_index(0));
    }

    #[test]
    fn selected_row_can_start_item_drag_without_rubber_band() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        view.select_single_index(0);

        assert!(view.can_start_item_drag_for_index(0));
    }

    #[test]
    fn selected_row_cannot_start_item_drag_while_rubber_band_exists() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        view.select_single_index(0);
        view.begin_mouse_selection_drag(
            gpui::MouseButton::Left,
            gpui::point(px(1.0), px(1.0)),
            SelectionModifiers::default(),
        );

        assert!(view.test_dragged_entries_for_index(0).is_some());
        assert!(!view.can_start_item_drag_for_index(0));
    }

    #[test]
    fn unselected_file_row_starts_rubber_band_instead_of_dnd_payload() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        view.select_single_index(0);

        assert!(view.test_dragged_entries_for_index(1).is_none());
        assert!(!view.can_start_item_drag_for_index(1));

        assert!(view.begin_mouse_selection_drag_for_intent(
            gpui::MouseButton::Left,
            gpui::point(px(1.0), px(ROW_HEIGHT + 1.0)),
            gpui::size(px(800.0), px(100.0)),
            SelectionModifiers::default(),
        ));

        assert!(view.mouse_selection_drag.is_some());
        assert!(!view.can_start_item_drag_for_index(0));
        assert!(selected_names(&view).is_empty());
    }

    #[test]
    fn unselected_folder_row_starts_rubber_band_instead_of_dnd_payload() {
        let mut view = test_view_with_entries(&[]);
        view.entries = vec![
            FileEntry::test("folder", true, None, None),
            FileEntry::test("file.txt", false, Some(1), None),
        ];
        view.select_single_index(1);

        assert!(view.test_dragged_entries_for_index(0).is_none());
        assert!(!view.can_start_item_drag_for_index(0));

        assert!(view.begin_mouse_selection_drag_for_intent(
            gpui::MouseButton::Left,
            gpui::point(px(1.0), px(1.0)),
            gpui::size(px(800.0), px(100.0)),
            SelectionModifiers::default(),
        ));

        assert!(view.mouse_selection_drag.is_some());
        assert!(!view.can_start_item_drag_for_index(1));
        assert!(selected_names(&view).is_empty());
    }

    #[test]
    fn selected_single_row_drag_produces_payload() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt"]);
        view.select_single_index(1);

        let dragged = view.test_dragged_entries_for_index(1).expect("dragged row");

        assert_eq!(dragged.paths, vec![PathBuf::from("b.txt")]);
        assert_eq!(dragged.display_name, "b.txt");
        assert_eq!(dragged.count, 1);
        assert_eq!(dragged.folder_count, 0);
        assert_eq!(dragged.file_count, 1);
        assert_eq!(drag_preview_label(&dragged), "b.txt");
    }

    #[test]
    fn selected_single_folder_drag_preview_uses_folder_name() {
        let mut view = test_view_with_entries(&[]);
        view.entries = vec![
            FileEntry::test("folder", true, None, None),
            FileEntry::test("file.txt", false, Some(1), None),
        ];
        view.select_single_index(0);

        let dragged = view.test_dragged_entries_for_index(0).expect("dragged row");

        assert_eq!(dragged.paths, vec![PathBuf::from("folder")]);
        assert_eq!(dragged.display_name, "folder");
        assert_eq!(dragged.count, 1);
        assert_eq!(dragged.folder_count, 1);
        assert_eq!(dragged.file_count, 0);
        assert_eq!(drag_preview_label(&dragged), "folder");
    }

    #[test]
    fn multi_file_drag_preview_counts_files() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);
        view.select_all_entries();

        let dragged = view.test_dragged_entries_for_index(0).expect("dragged row");

        assert_eq!(
            dragged.paths,
            vec![
                PathBuf::from("a.txt"),
                PathBuf::from("b.txt"),
                PathBuf::from("c.txt")
            ]
        );
        assert_eq!(dragged.count, 3);
        assert_eq!(dragged.folder_count, 0);
        assert_eq!(dragged.file_count, 3);
        assert_eq!(drag_preview_label(&dragged), "3 files");
    }

    #[test]
    fn multi_folder_drag_preview_counts_folders() {
        let mut view = test_view_with_entries(&[]);
        view.entries = vec![
            FileEntry::test("folder-a", true, None, None),
            FileEntry::test("folder-b", true, None, None),
            FileEntry::test("folder-c", true, None, None),
            FileEntry::test("folder-d", true, None, None),
        ];
        view.select_all_entries();

        let dragged = view.test_dragged_entries_for_index(0).expect("dragged row");

        assert_eq!(dragged.count, 4);
        assert_eq!(dragged.folder_count, 4);
        assert_eq!(dragged.file_count, 0);
        assert_eq!(drag_preview_label(&dragged), "4 folders");
    }

    #[test]
    fn mixed_multi_selection_drag_preview_counts_folders_and_files() {
        let mut view = test_view_with_entries(&[]);
        view.entries = vec![
            FileEntry::test("folder", true, None, None),
            FileEntry::test("file.txt", false, Some(1), None),
        ];
        view.select_all_entries();

        let dragged = view.test_dragged_entries_for_index(0).expect("dragged row");

        assert_eq!(
            dragged.paths,
            vec![PathBuf::from("folder"), PathBuf::from("file.txt")]
        );
        assert_eq!(dragged.count, 2);
        assert_eq!(dragged.folder_count, 1);
        assert_eq!(dragged.file_count, 1);
        assert_eq!(drag_preview_label(&dragged), "1 folder, 1 file");
    }

    #[test]
    fn every_selected_row_drag_uses_same_multi_selection_payload() {
        let mut view = test_view_with_entries(&["a.txt", "b.txt", "c.txt"]);
        view.select_single_index(0);
        view.apply_click_selection(
            2,
            SelectionModifiers {
                toggle: true,
                extend: false,
            },
        );

        let first_drag = view.test_dragged_entries_for_index(0).expect("first row");
        let second_drag = view
            .test_dragged_entries_for_index(2)
            .expect("second selected row");

        assert_eq!(
            first_drag.paths,
            vec![PathBuf::from("a.txt"), PathBuf::from("c.txt")]
        );
        assert_eq!(second_drag.paths, first_drag.paths);
        assert_eq!(second_drag.source_dir, first_drag.source_dir);
        assert_eq!(second_drag.count, first_drag.count);
        assert_eq!(second_drag.folder_count, first_drag.folder_count);
        assert_eq!(second_drag.file_count, first_drag.file_count);
    }

    #[test]
    fn drag_preview_origin_centers_preview_on_cursor_offset() {
        let cursor_offset = gpui::point(px(120.0), px(32.0));

        let origin = drag_preview_origin(cursor_offset);

        assert_eq!(origin.0, 120.0 - (DRAG_PREVIEW_WIDTH / 2.0));
    }

    #[test]
    fn drag_preview_origin_overlaps_cursor_offset() {
        let cursor_offset = gpui::point(px(120.0), px(32.0));

        let origin = drag_preview_origin(cursor_offset);

        assert_eq!(
            origin.1,
            32.0 - DRAG_PREVIEW_HEIGHT + DRAG_PREVIEW_CURSOR_OVERLAP
        );
    }

    #[test]
    fn drag_preview_text_width_subtracts_horizontal_padding() {
        assert_eq!(
            drag_preview_text_width(),
            DRAG_PREVIEW_WIDTH - (DRAG_PREVIEW_HORIZONTAL_PADDING * 2.0)
        );
    }

    #[test]
    fn default_drop_indicator_uses_move_operation() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let target = temp.path().join("target");
        fs::write(&source, b"data").expect("create source");
        fs::create_dir(&target).expect("create target folder");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&source);
        let ix = view
            .entries
            .iter()
            .position(|entry| entry.path == source)
            .expect("source entry");
        let dragged = view
            .test_dragged_entries_for_index(ix)
            .expect("dragged row");

        let indicator = view
            .drop_indicator_for_value(
                &dragged,
                &DropDestination::Directory {
                    item_path: target.clone(),
                    target_path: target,
                },
                Modifiers::default(),
                gpui::point(px(32.0), px(48.0)),
            )
            .expect("drop indicator");

        assert_eq!(indicator.operation, FileOperationKind::Move);
        assert_eq!(indicator.target_label, "target");
    }

    #[test]
    fn secondary_modifier_drop_indicator_uses_copy_operation() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let target = temp.path().join("target");
        fs::write(&source, b"data").expect("create source");
        fs::create_dir(&target).expect("create target folder");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&source);
        let ix = view
            .entries
            .iter()
            .position(|entry| entry.path == source)
            .expect("source entry");
        let dragged = view
            .test_dragged_entries_for_index(ix)
            .expect("dragged row");

        let indicator = view
            .drop_indicator_for_value(
                &dragged,
                &DropDestination::Directory {
                    item_path: target.clone(),
                    target_path: target,
                },
                Modifiers::secondary_key(),
                gpui::point(px(32.0), px(48.0)),
            )
            .expect("drop indicator");

        assert_eq!(indicator.operation, FileOperationKind::Copy);
        assert_eq!(indicator.target_label, "target");
    }

    #[test]
    fn active_drop_indicator_updates_operation_when_modifiers_change() {
        let mut view = test_view_with_entries(&["file.txt"]);
        view.active_drop_indicator = Some(DropIndicator {
            operation: FileOperationKind::Move,
            target_label: "target".to_owned(),
            mouse_position: gpui::point(px(32.0), px(48.0)),
        });

        assert!(view.update_drop_indicator_modifiers(Modifiers::secondary_key()));

        assert_eq!(
            view.active_drop_indicator.as_ref().unwrap().operation,
            FileOperationKind::Copy
        );
    }

    #[test]
    fn unsupported_modifier_combination_clears_active_drop_indicator() {
        let mut view = test_view_with_entries(&["file.txt"]);
        view.active_drop_indicator = Some(DropIndicator {
            operation: FileOperationKind::Move,
            target_label: "target".to_owned(),
            mouse_position: gpui::point(px(32.0), px(48.0)),
        });

        assert!(view.update_drop_indicator_modifiers(Modifiers {
            alt: true,
            ..Modifiers::default()
        }));

        assert_eq!(view.active_drop_indicator, None);
    }

    #[test]
    fn stale_drop_indicator_clears_when_drag_position_changes() {
        let mut view = test_view_with_entries(&["file.txt"]);
        view.active_drop_indicator = Some(DropIndicator {
            operation: FileOperationKind::Move,
            target_label: "target".to_owned(),
            mouse_position: gpui::point(px(32.0), px(48.0)),
        });

        assert!(view.clear_stale_drop_indicator(gpui::point(px(33.0), px(48.0))));

        assert_eq!(view.active_drop_indicator, None);
    }

    #[test]
    fn current_position_drop_indicator_is_not_stale() {
        let mut view = test_view_with_entries(&["file.txt"]);
        let mouse_position = gpui::point(px(32.0), px(48.0));
        view.active_drop_indicator = Some(DropIndicator {
            operation: FileOperationKind::Move,
            target_label: "target".to_owned(),
            mouse_position,
        });

        assert!(!view.clear_stale_drop_indicator(mouse_position));

        assert!(view.active_drop_indicator.is_some());
    }

    #[test]
    fn external_drag_exit_clears_active_drop_indicator() {
        let mut view = test_view_with_entries(&["file.txt"]);
        view.active_drop_indicator = Some(DropIndicator {
            operation: FileOperationKind::Copy,
            target_label: "target".to_owned(),
            mouse_position: gpui::point(px(32.0), px(48.0)),
        });

        assert!(view.clear_drop_indicator());
        assert_eq!(view.active_drop_indicator, None);
        assert!(!view.clear_drop_indicator());
    }

    #[test]
    fn invalid_drop_has_no_drop_indicator() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        fs::create_dir(&folder).expect("create selected folder");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&folder);
        let dragged = view.test_dragged_entries_for_index(0).expect("dragged row");

        assert_eq!(
            view.drop_indicator_for_value(
                &dragged,
                &DropDestination::Directory {
                    item_path: folder.clone(),
                    target_path: folder,
                },
                Modifiers::default(),
                gpui::point(px(32.0), px(48.0)),
            ),
            None
        );
    }

    #[test]
    fn same_folder_current_directory_drop_has_no_indicator() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        fs::write(&source, b"data").expect("create source");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&source);
        let ix = view
            .entries
            .iter()
            .position(|entry| entry.path == source)
            .expect("source entry");
        let dragged = view
            .test_dragged_entries_for_index(ix)
            .expect("dragged row");

        assert_eq!(
            view.drop_indicator_for_value(
                &dragged,
                &DropDestination::CurrentDirectory,
                Modifiers::default(),
                gpui::point(px(32.0), px(48.0)),
            ),
            None
        );
    }

    #[test]
    fn same_folder_internal_drop_cannot_target_resolved_directory() {
        let temp = TempDir::new();
        let target = temp.path().join("target");
        fs::create_dir(&target).expect("create target folder");
        let source = target.join("file.txt");
        fs::write(&source, b"data").expect("create source");

        let view = ExplorerView::new(temp.path().to_path_buf());
        let dragged = DraggedEntries {
            paths: vec![source],
            source_dir: target.clone(),
            display_name: "file.txt".to_owned(),
            count: 1,
            folder_count: 0,
            file_count: 1,
        };
        let destination = DropDestination::Directory {
            item_path: target.clone(),
            target_path: target,
        };

        assert!(!view.can_drop_value(&dragged, &destination, Modifiers::default()));
        assert_eq!(
            view.drop_indicator_for_value(
                &dragged,
                &destination,
                Modifiers::default(),
                gpui::point(px(32.0), px(48.0)),
            ),
            None
        );
    }

    #[test]
    fn cross_folder_internal_drop_can_target_resolved_directory() {
        let temp = TempDir::new();
        let source_dir = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir(&source_dir).expect("create source folder");
        fs::create_dir(&target).expect("create target folder");

        let view = ExplorerView::new(temp.path().to_path_buf());
        let dragged = DraggedEntries {
            paths: vec![source_dir.join("file.txt")],
            source_dir,
            display_name: "file.txt".to_owned(),
            count: 1,
            folder_count: 0,
            file_count: 1,
        };

        assert!(view.can_drop_value(
            &dragged,
            &DropDestination::Directory {
                item_path: target.clone(),
                target_path: target,
            },
            Modifiers::default(),
        ));
    }

    #[test]
    fn cross_folder_current_directory_drop_indicator_uses_current_folder_name() {
        let temp = TempDir::new();
        let source_dir = temp.path().join("source");
        fs::create_dir(&source_dir).expect("create source folder");

        let view = ExplorerView::new(temp.path().to_path_buf());
        let dragged = DraggedEntries {
            paths: vec![source_dir.join("file.txt")],
            source_dir,
            display_name: "file.txt".to_owned(),
            count: 1,
            folder_count: 0,
            file_count: 1,
        };

        let indicator = view
            .drop_indicator_for_value(
                &dragged,
                &DropDestination::CurrentDirectory,
                Modifiers::default(),
                gpui::point(px(32.0), px(48.0)),
            )
            .expect("drop indicator");

        assert_eq!(indicator.operation, FileOperationKind::Move);
        assert_eq!(
            indicator.target_label,
            temp.path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        );
    }

    #[test]
    fn drop_indicator_origin_uses_mouse_position() {
        let mouse_position = gpui::point(px(120.0), px(32.0));
        let indicator_origin = drop_indicator_origin(mouse_position);

        assert_eq!(indicator_origin.0, 120.0);
    }

    #[test]
    fn drop_indicator_origin_uses_root_space_mouse_position_without_view_offset() {
        let root_space_position = gpui::point(px(120.0), px(68.0));
        let indicator_origin = drop_indicator_origin(root_space_position);

        assert_eq!(indicator_origin.0, 120.0);
        assert_eq!(
            indicator_origin.1,
            drag_preview_origin(root_space_position).1 + DRAG_PREVIEW_HEIGHT - 1.0
        );
    }

    #[test]
    fn drop_indicator_top_overlaps_drag_preview_bottom_by_one_pixel() {
        let mouse_position = gpui::point(px(120.0), px(32.0));
        let drag_origin = drag_preview_origin(mouse_position);
        let indicator_origin = drop_indicator_origin(mouse_position);
        let drag_bottom = drag_origin.1 + DRAG_PREVIEW_HEIGHT;

        assert_eq!(indicator_origin.1, drag_bottom - 1.0);
    }

    #[test]
    fn selected_directory_cannot_be_internal_drop_target() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        fs::create_dir(&folder).expect("create selected folder");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&folder);
        let dragged = view.test_dragged_entries_for_index(0).expect("dragged row");

        assert!(!view.can_drop_value(
            &dragged,
            &DropDestination::Directory {
                item_path: folder.clone(),
                target_path: folder,
            },
            Modifiers::default(),
        ));
    }

    #[test]
    fn same_folder_internal_drop_cannot_target_current_directory() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        fs::write(&source, b"data").expect("create source");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&source);
        let dragged = view.test_dragged_entries_for_index(0).expect("dragged row");

        assert!(!view.can_drop_value(
            &dragged,
            &DropDestination::CurrentDirectory,
            Modifiers::default(),
        ));
    }

    #[test]
    fn cross_folder_internal_drop_can_target_current_directory() {
        let temp = TempDir::new();
        let source_dir = temp.path().join("source");
        fs::create_dir(&source_dir).expect("create source folder");

        let view = ExplorerView::new(temp.path().to_path_buf());
        let dragged = DraggedEntries {
            paths: vec![source_dir.join("file.txt")],
            source_dir,
            display_name: "file.txt".to_owned(),
            count: 1,
            folder_count: 0,
            file_count: 1,
        };

        assert!(view.can_drop_value(
            &dragged,
            &DropDestination::CurrentDirectory,
            Modifiers::default(),
        ));
    }

    #[test]
    fn same_folder_external_paths_are_current_directory_sources() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");

        assert!(destination_contains_all_external_path_sources(
            &DropDestination::CurrentDirectory,
            temp.path(),
            temp.path(),
            &[source],
        ));
    }

    #[test]
    fn external_paths_from_other_folder_are_not_current_directory_sources() {
        let temp = TempDir::new();
        let source_dir = temp.path().join("source");
        let source = source_dir.join("file.txt");

        assert!(!destination_contains_all_external_path_sources(
            &DropDestination::CurrentDirectory,
            temp.path(),
            temp.path(),
            &[source],
        ));
    }

    #[test]
    fn same_folder_external_paths_are_resolved_directory_sources() {
        let temp = TempDir::new();
        let target = temp.path().join("target");
        fs::create_dir(&target).expect("create target folder");
        let source = target.join("file.txt");

        assert!(destination_contains_all_external_path_sources(
            &DropDestination::Directory {
                item_path: target.clone(),
                target_path: target.clone(),
            },
            temp.path(),
            &target,
            &[source],
        ));
    }

    #[test]
    fn external_paths_from_other_folder_are_not_resolved_directory_sources() {
        let temp = TempDir::new();
        let source_dir = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir(&target).expect("create target folder");
        let source = source_dir.join("file.txt");

        assert!(!destination_contains_all_external_path_sources(
            &DropDestination::Directory {
                item_path: target.clone(),
                target_path: target.clone(),
            },
            temp.path(),
            &target,
            &[source],
        ));
    }

    #[test]
    fn mixed_external_paths_do_not_make_resolved_directory_source_no_op() {
        let temp = TempDir::new();
        let source_dir = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir(&target).expect("create target folder");
        let same_folder_source = target.join("same.txt");
        let other_folder_source = source_dir.join("other.txt");

        assert!(!destination_contains_all_external_path_sources(
            &DropDestination::Directory {
                item_path: target.clone(),
                target_path: target.clone(),
            },
            temp.path(),
            &target,
            &[same_folder_source, other_folder_source],
        ));
    }

    #[test]
    fn current_directory_external_drop_from_same_folder_is_no_op() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        fs::write(&source, b"data").expect("create source");
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.open_error = Some("stale error".to_owned());

        view.drop_external_paths(
            std::slice::from_ref(&source),
            DropDestination::CurrentDirectory,
            Modifiers::default(),
        );

        assert_eq!(fs::read(&source).unwrap(), b"data");
        assert_eq!(view.open_error, Some("stale error".to_owned()));
    }

    #[test]
    fn current_directory_external_drop_from_other_folder_moves_to_current_directory() {
        let temp = TempDir::new();
        let source_dir = temp.path().join("source");
        fs::create_dir(&source_dir).expect("create source folder");
        let source = source_dir.join("file.txt");
        fs::write(&source, b"data").expect("create source");
        let mut view = ExplorerView::new(temp.path().to_path_buf());

        view.drop_external_paths(
            std::slice::from_ref(&source),
            DropDestination::CurrentDirectory,
            Modifiers::default(),
        );

        assert!(!source.exists());
        assert_eq!(fs::read(temp.path().join("file.txt")).unwrap(), b"data");
    }

    #[test]
    fn resolved_directory_external_drop_from_same_folder_is_no_op() {
        let temp = TempDir::new();
        let target = temp.path().join("target");
        fs::create_dir(&target).expect("create target folder");
        let source = target.join("file.txt");
        fs::write(&source, b"data").expect("create source");
        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.open_error = Some("stale error".to_owned());

        view.drop_external_paths(
            std::slice::from_ref(&source),
            DropDestination::Directory {
                item_path: target.clone(),
                target_path: target.clone(),
            },
            Modifiers::default(),
        );

        assert_eq!(fs::read(&source).unwrap(), b"data");
        assert_eq!(view.open_error, Some("stale error".to_owned()));
        assert!(target.join("file.txt").exists());
    }

    #[test]
    fn resolved_directory_drop_from_other_folder_preserves_conflict_dialog() {
        let temp = TempDir::new();
        let source_dir = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir(&source_dir).expect("create source folder");
        fs::create_dir(&target).expect("create target folder");
        let source = source_dir.join("file.txt");
        let existing = target.join("file.txt");
        fs::write(&source, b"source").expect("create source");
        fs::write(&existing, b"existing").expect("create existing");
        let mut view = ExplorerView::new(temp.path().to_path_buf());

        view.drop_external_paths(
            std::slice::from_ref(&source),
            DropDestination::Directory {
                item_path: target.clone(),
                target_path: target,
            },
            Modifiers::default(),
        );

        assert!(view.pending_file_conflict.is_some());
        assert_eq!(view.open_error, None);
        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert_eq!(fs::read(&existing).unwrap(), b"existing");
    }

    #[test]
    fn all_selected_directories_are_rejected_as_internal_drop_targets() {
        let temp = TempDir::new();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let target = temp.path().join("target");
        fs::create_dir(&first).expect("create first folder");
        fs::create_dir(&second).expect("create second folder");
        fs::create_dir(&target).expect("create target folder");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.restore_selection_from_paths(&[first.clone(), second.clone()]);
        let dragged = view.test_dragged_entries_for_index(0).expect("dragged row");

        assert!(!view.can_drop_value(
            &dragged,
            &DropDestination::Directory {
                item_path: first.clone(),
                target_path: first,
            },
            Modifiers::default(),
        ));
        assert!(!view.can_drop_value(
            &dragged,
            &DropDestination::Directory {
                item_path: second.clone(),
                target_path: second,
            },
            Modifiers::default(),
        ));
        assert!(view.can_drop_value(
            &dragged,
            &DropDestination::Directory {
                item_path: target.clone(),
                target_path: target,
            },
            Modifiers::default(),
        ));
    }

    #[test]
    fn selected_directory_drop_is_no_op() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        fs::create_dir(&folder).expect("create selected folder");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.select_single_path(&folder);
        let dragged = view.test_dragged_entries_for_index(0).expect("dragged row");
        view.open_error = Some("stale error".to_owned());

        view.drop_internal_entries(
            &dragged,
            DropDestination::Directory {
                item_path: folder.clone(),
                target_path: folder.clone(),
            },
            Modifiers::default(),
        );

        assert_eq!(view.open_error, Some("stale error".to_owned()));
        assert!(folder.is_dir());
    }

    #[test]
    fn selected_directory_shortcut_cannot_be_its_own_drop_target() {
        let temp = TempDir::new();
        let shortcut = temp.path().join("target.lnk");
        let target = temp.path().join("target");
        fs::write(&shortcut, b"shortcut").expect("create shortcut placeholder");
        fs::create_dir(&target).expect("create target folder");

        let mut view = ExplorerView::new(temp.path().to_path_buf());
        view.restore_selection_from_paths(std::slice::from_ref(&shortcut));
        let ix = view
            .entries
            .iter()
            .position(|entry| entry.path == shortcut)
            .expect("shortcut entry");
        let dragged = view
            .test_dragged_entries_for_index(ix)
            .expect("dragged row");

        assert!(!view.can_drop_value(
            &dragged,
            &DropDestination::Directory {
                item_path: shortcut,
                target_path: target,
            },
            Modifiers::default(),
        ));
    }

    #[test]
    fn external_drop_on_directory_shortcut_uses_resolved_target() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let shortcut = temp.path().join("target.lnk");
        let target = temp.path().join("target");
        fs::write(&source, b"data").expect("create source");
        fs::write(&shortcut, b"shortcut").expect("create shortcut placeholder");
        fs::create_dir(&target).expect("create target folder");
        let mut view = ExplorerView::new(temp.path().to_path_buf());

        view.drop_external_paths(
            std::slice::from_ref(&source),
            DropDestination::Directory {
                item_path: shortcut.clone(),
                target_path: target.clone(),
            },
            Modifiers::default(),
        );

        assert!(!source.exists());
        assert_eq!(fs::read(target.join("file.txt")).unwrap(), b"data");
        assert!(shortcut.exists());
    }
}
