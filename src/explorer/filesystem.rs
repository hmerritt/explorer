use std::{
    collections::{HashMap, HashSet},
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use filetime::FileTime;
use thousands::Separable;

use crate::explorer::{entry::FileEntry, sorting::sort_entries};

const COPY_BUFFER_SIZE: usize = 1024 * 1024;
const COMPOUND_ARCHIVE_EXTENSIONS: &[&str] = &["tar.gz", "tar.bz2", "tar.xz", "tar.zst"];
const SIMPLE_ARCHIVE_EXTENSIONS: &[&str] = &[
    "zip", "tar", "tgz", "tbz", "txz", "tzst", "ar", "gz", "bz", "bz2", "xz", "zst", "rar",
];
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn default_start_path() -> PathBuf {
    let home_dir = user_home_dir();
    let downloads_dir = user_downloads_dir(home_dir.as_deref());

    preferred_start_path(downloads_dir, home_dir, std::env::current_dir().ok())
}

fn preferred_start_path(
    downloads_dir: Option<PathBuf>,
    home_dir: Option<PathBuf>,
    current_dir: Option<PathBuf>,
) -> PathBuf {
    downloads_dir
        .filter(|path| path.is_dir())
        .or(home_dir)
        .or(current_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

#[cfg(target_os = "windows")]
fn known_folder_path(folder_id: &windows::core::GUID) -> Option<PathBuf> {
    use windows::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{KNOWN_FOLDER_FLAG, SHGetKnownFolderPath},
    };

    unsafe {
        let known_folder = SHGetKnownFolderPath(folder_id, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let path = known_folder.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(known_folder.as_ptr().cast()));
        path
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn user_desktop_dir(_home_dir: Option<&Path>) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::FOLDERID_Desktop;

    known_folder_path(&FOLDERID_Desktop)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn user_desktop_dir(home_dir: Option<&Path>) -> Option<PathBuf> {
    home_dir.map(|home_dir| home_dir.join("Desktop"))
}

#[cfg(target_os = "windows")]
pub(crate) fn user_documents_dir(_home_dir: Option<&Path>) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::FOLDERID_Documents;

    known_folder_path(&FOLDERID_Documents)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn user_documents_dir(home_dir: Option<&Path>) -> Option<PathBuf> {
    home_dir.map(|home_dir| home_dir.join("Documents"))
}

#[cfg(target_os = "windows")]
pub(crate) fn user_downloads_dir(_home_dir: Option<&Path>) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::FOLDERID_Downloads;

    known_folder_path(&FOLDERID_Downloads)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn user_downloads_dir(home_dir: Option<&Path>) -> Option<PathBuf> {
    home_dir.map(|home_dir| home_dir.join("Downloads"))
}

#[cfg(target_os = "windows")]
pub(super) fn local_drive_roots() -> Vec<PathBuf> {
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    use windows::core::PCWSTR;

    let mask = unsafe { GetLogicalDrives() };
    let mut roots = Vec::new();

    for drive_index in 0..26 {
        if mask & (1 << drive_index) == 0 {
            continue;
        }

        let letter = char::from(b'A' + drive_index as u8);
        let root = format!("{letter}:\\");
        let mut encoded = root.encode_utf16().collect::<Vec<_>>();
        encoded.push(0);

        let drive_type = unsafe { GetDriveTypeW(PCWSTR(encoded.as_ptr())) };
        if windows_drive_type_is_explorer_local(drive_type) {
            roots.push(PathBuf::from(root));
        }
    }

    roots
}

pub(super) fn drive_display_label(path: &Path) -> String {
    let display = path.display().to_string();

    #[cfg(target_os = "windows")]
    {
        return windows_drive_display_label(&display, windows_volume_label(path).as_deref());
    }

    #[cfg(not(target_os = "windows"))]
    {
        display
    }
}

#[cfg(target_os = "windows")]
fn windows_volume_label(path: &Path) -> Option<String> {
    use windows::Win32::Storage::FileSystem::GetVolumeInformationW;
    use windows::core::PCWSTR;

    let root = path.display().to_string();
    let mut encoded = root.encode_utf16().collect::<Vec<_>>();
    encoded.push(0);

    let mut volume_name = [0u16; 261];
    unsafe {
        GetVolumeInformationW(
            PCWSTR(encoded.as_ptr()),
            Some(&mut volume_name),
            None,
            None,
            None,
            None,
        )
        .ok()?;
    }

    let length = volume_name
        .iter()
        .position(|code_unit| *code_unit == 0)
        .unwrap_or(volume_name.len());

    String::from_utf16(&volume_name[..length]).ok()
}

#[cfg(target_os = "windows")]
fn windows_drive_display_label(path_display: &str, volume_label: Option<&str>) -> String {
    let drive = path_display.trim_end_matches(['\\', '/']);
    let label = volume_label
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .unwrap_or("Local Disk");

    format!("{label} ({drive})")
}

#[cfg(target_os = "windows")]
fn windows_drive_type_is_explorer_local(drive_type: u32) -> bool {
    matches!(drive_type, 2 | 3 | 5 | 6)
}

#[cfg(not(target_os = "windows"))]
pub(super) fn local_drive_roots() -> Vec<PathBuf> {
    vec![PathBuf::from("/")]
}

pub(super) fn load_entries(
    path: &Path,
    show_hidden_files: bool,
) -> std::io::Result<Vec<FileEntry>> {
    load_entries_with_options(path, EntryLoadOptions::for_path(path, show_hidden_files))
}

#[derive(Clone, Copy)]
struct EntryLoadOptions {
    hide_hidden_entries: bool,
    applications_view: bool,
}

impl EntryLoadOptions {
    fn for_path(path: &Path, show_hidden_files: bool) -> Self {
        Self {
            hide_hidden_entries: !show_hidden_files,
            applications_view: should_use_applications_view(path),
        }
    }
}

fn load_entries_with_options(
    path: &Path,
    options: EntryLoadOptions,
) -> std::io::Result<Vec<FileEntry>> {
    if options.applications_view {
        return load_applications_entries(path, options);
    }

    let mut entries = fs::read_dir(path)?
        .filter_map(Result::ok)
        .filter(|entry| !should_skip_directory_entry(entry, options))
        .filter_map(|entry| FileEntry::from_path(entry.path()))
        .collect::<Vec<_>>();

    sort_entries(&mut entries);
    Ok(entries)
}

fn load_applications_entries(
    path: &Path,
    options: EntryLoadOptions,
) -> std::io::Result<Vec<FileEntry>> {
    let mut entries = Vec::new();

    for directory_entry in fs::read_dir(path)?
        .filter_map(Result::ok)
        .filter(|entry| !should_skip_directory_entry(entry, options))
    {
        let Some(entry) = FileEntry::from_path(directory_entry.path()) else {
            continue;
        };

        if entry.is_app_bundle() {
            entries.push(entry);
        } else if entry.is_directory_like() {
            collect_nested_applications(entry.navigation_path(), options, &mut entries);
        }
    }

    sort_entries(&mut entries);
    Ok(entries)
}

fn collect_nested_applications(
    path: &Path,
    options: EntryLoadOptions,
    entries: &mut Vec<FileEntry>,
) {
    let Ok(nested_entries) = fs::read_dir(path) else {
        return;
    };

    entries.extend(
        nested_entries
            .filter_map(Result::ok)
            .filter(|entry| !should_skip_directory_entry(entry, options))
            .filter_map(|entry| FileEntry::from_path(entry.path()))
            .filter(FileEntry::is_app_bundle),
    );
}

#[cfg(target_os = "macos")]
fn should_use_applications_view(path: &Path) -> bool {
    path == Path::new("/Applications")
}

#[cfg(not(target_os = "macos"))]
fn should_use_applications_view(_: &Path) -> bool {
    false
}

fn should_skip_directory_entry(entry: &fs::DirEntry, options: EntryLoadOptions) -> bool {
    should_hide_directory_entry(entry, !options.hide_hidden_entries)
}

pub(super) fn should_hide_directory_entry(entry: &fs::DirEntry, show_hidden_files: bool) -> bool {
    should_hide_entry(&entry.file_name(), &entry.path(), show_hidden_files)
}

pub(super) fn should_hide_entry(name: &OsStr, path: &Path, show_hidden_files: bool) -> bool {
    is_always_hidden_metadata_entry_name(name) || !show_hidden_files && is_hidden_entry(name, path)
}

pub(super) fn should_hide_entry_with_metadata(
    name: &OsStr,
    path: &Path,
    show_hidden_files: bool,
    metadata: &fs::Metadata,
) -> bool {
    is_always_hidden_metadata_entry_name(name)
        || !show_hidden_files && is_hidden_entry_with_metadata(name, path, metadata)
}

fn is_always_hidden_metadata_entry_name(name: &OsStr) -> bool {
    name == OsStr::new(".localized") || name == OsStr::new(".DS_Store")
}

fn is_hidden_entry(name: &OsStr, path: &Path) -> bool {
    name.to_string_lossy().starts_with('.')
        || has_macos_hidden_flag(path)
        || has_windows_hidden_attribute(path)
}

fn is_hidden_entry_with_metadata(name: &OsStr, path: &Path, metadata: &fs::Metadata) -> bool {
    name.to_string_lossy().starts_with('.')
        || has_macos_hidden_flag_with_metadata(path, metadata)
        || has_windows_hidden_attribute_with_metadata(path, metadata)
}

#[cfg(target_os = "macos")]
fn has_macos_hidden_flag(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| has_macos_hidden_flag_with_metadata(path, &metadata))
}

#[cfg(not(target_os = "macos"))]
fn has_macos_hidden_flag(_: &Path) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn has_macos_hidden_flag_with_metadata(_: &Path, metadata: &fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;

    const UF_HIDDEN: u32 = 0x0000_8000;

    metadata.st_flags() & UF_HIDDEN != 0
}

#[cfg(not(target_os = "macos"))]
fn has_macos_hidden_flag_with_metadata(_: &Path, _: &fs::Metadata) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn has_windows_hidden_attribute(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| has_windows_hidden_attribute_with_metadata(path, &metadata))
}

#[cfg(not(target_os = "windows"))]
fn has_windows_hidden_attribute(_: &Path) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn has_windows_hidden_attribute_with_metadata(_: &Path, metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN;

    metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN.0 != 0
}

#[cfg(not(target_os = "windows"))]
fn has_windows_hidden_attribute_with_metadata(_: &Path, _: &fs::Metadata) -> bool {
    false
}

pub(super) fn open_path_with_default_app(path: &Path) -> std::io::Result<()> {
    open::that_detached(path)
}

pub(super) fn format_open_error(path: &Path, error: &std::io::Error) -> String {
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .map(str::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned());

    format!("Could not open {name}: {error}")
}

#[cfg(test)]
pub(super) fn move_paths_to_directory(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<FileOperationOutcome, String> {
    prepare_move_paths_to_directory(paths, destination).and_then(run_prepared_file_operation)
}

pub(super) fn prepare_move_paths_to_directory(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<PreparedFileOperation, String> {
    prepare_file_operation(
        paths,
        destination,
        FileOperationKind::Move,
        CopyNamePolicy::Original,
    )
    .map(prepared_or_conflicts)
}

#[cfg(test)]
pub(super) fn copy_paths_to_directory(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<FileOperationOutcome, String> {
    prepare_copy_paths_to_directory(paths, destination).and_then(run_prepared_file_operation)
}

pub(super) fn prepare_copy_paths_to_directory(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<PreparedFileOperation, String> {
    prepare_file_operation(
        paths,
        destination,
        FileOperationKind::Copy,
        CopyNamePolicy::Original,
    )
    .map(prepared_or_conflicts)
}

#[cfg(test)]
pub(super) fn copy_paths_to_directory_for_paste(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<FileOperationOutcome, String> {
    prepare_copy_paths_to_directory_for_paste(paths, destination)
        .and_then(run_prepared_file_operation)
}

pub(super) fn prepare_copy_paths_to_directory_for_paste(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<PreparedFileOperation, String> {
    prepare_file_operation(
        paths,
        destination,
        FileOperationKind::Copy,
        CopyNamePolicy::UseCopyNamesInSameDirectory,
    )
    .map(prepared_or_conflicts)
}

pub(super) fn archive_path_is_supported(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(archive_name_has_supported_extension)
}

pub(super) fn prepare_extract_archives_to_directory(
    archives: &[PathBuf],
    destination: &Path,
) -> Result<PreparedFileOperation, String> {
    prepare_extract_archive_operation(archives, destination).map(prepared_or_conflicts)
}

#[cfg(test)]
pub(super) fn resolve_file_conflicts(
    conflicts: FileConflictBatch,
    choice: ConflictChoice,
) -> Result<FileOperationSummary, String> {
    execute_file_operation(conflicts.job, choice)
}

#[cfg(test)]
fn run_prepared_file_operation(
    prepared: PreparedFileOperation,
) -> Result<FileOperationOutcome, String> {
    match prepared {
        PreparedFileOperation::Ready(job) => {
            execute_file_operation(job, ConflictChoice::Replace).map(FileOperationOutcome::Finished)
        }
        PreparedFileOperation::Conflicts(conflicts) => {
            Ok(FileOperationOutcome::Conflicts(conflicts))
        }
    }
}

fn prepared_or_conflicts(job: FileOperationJob) -> PreparedFileOperation {
    let conflicts = file_conflicts_for_job(&job);
    if conflicts.is_empty() {
        PreparedFileOperation::Ready(job)
    } else {
        PreparedFileOperation::Conflicts(FileConflictBatch { conflicts, job })
    }
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum FileOperationOutcome {
    Finished(FileOperationSummary),
    Conflicts(FileConflictBatch),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum PreparedFileOperation {
    Ready(FileOperationJob),
    Conflicts(FileConflictBatch),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct FileOperationSummary {
    pub(super) destination_paths: Vec<PathBuf>,
    pub(super) moved_source_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileConflictBatch {
    pub(super) conflicts: Vec<FileConflict>,
    job: FileOperationJob,
}

impl FileConflictBatch {
    pub(super) fn len(&self) -> usize {
        self.conflicts.len()
    }

    pub(super) fn operation_label(&self) -> &'static str {
        self.job.kind.progress_title()
    }

    pub(super) fn into_job(self) -> FileOperationJob {
        self.job
    }

    pub(super) fn item_count_label(&self) -> String {
        let count = self.job.roots.len();
        let count_friendly = count.separate_with_commas();
        if count == 1 {
            "1 item".to_owned()
        } else {
            format!("{count_friendly} items")
        }
    }

    pub(super) fn source_location_name(&self) -> String {
        self.unique_root_parent_name(|root| root.source.parent())
    }

    pub(super) fn destination_location_name(&self) -> String {
        self.unique_root_parent_name(|root| root.destination.parent())
    }

    pub(super) fn first_destination_name(&self) -> String {
        self.conflicts
            .first()
            .map(|conflict| path_display_name(&conflict.destination))
            .unwrap_or_else(|| "this file".to_owned())
    }

    fn unique_root_parent_name<'a>(
        &'a self,
        parent_for_root: impl Fn(&'a FileOperationRoot) -> Option<&'a Path>,
    ) -> String {
        let parents = self
            .job
            .roots
            .iter()
            .filter_map(parent_for_root)
            .collect::<HashSet<_>>();

        if parents.len() == 1 {
            path_display_name(parents.iter().next().expect("one parent"))
        } else {
            "multiple locations".to_owned()
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileConflict {
    pub(super) source: PathBuf,
    pub(super) destination: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ConflictChoice {
    Replace,
    Skip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FileOperationKind {
    Move,
    Copy,
    Extract,
}

impl FileOperationKind {
    pub(super) fn progress_title(self) -> &'static str {
        match self {
            FileOperationKind::Move => "Moving",
            FileOperationKind::Copy => "Copying",
            FileOperationKind::Extract => "Extracting",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FileOperationPhase {
    Preparing,
    Copying,
    Extracting,
    Moving,
    Removing,
    Finished,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileOperationProgress {
    pub(super) kind: FileOperationKind,
    pub(super) phase: FileOperationPhase,
    pub(super) total_bytes: u64,
    pub(super) copied_bytes: u64,
    pub(super) total_files: usize,
    pub(super) completed_files: usize,
    pub(super) current_item: Option<PathBuf>,
    pub(super) cancellable: bool,
}

impl FileOperationProgress {
    pub(super) fn percent(&self) -> Option<f32> {
        (self.total_bytes > 0)
            .then(|| (self.copied_bytes as f32 / self.total_bytes as f32).clamp(0.0, 1.0))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileOperationStats {
    pub(super) total_bytes: u64,
    pub(super) total_files: usize,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum FileOperationError {
    Cancelled,
    Failed(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CopyNamePolicy {
    Original,
    UseCopyNamesInSameDirectory,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct FileOperationJob {
    pub(super) kind: FileOperationKind,
    pub(super) stats: FileOperationStats,
    steps: Vec<FileOperationStep>,
    roots: Vec<FileOperationRoot>,
}

impl FileOperationJob {
    pub(super) fn initial_progress(&self) -> FileOperationProgress {
        FileOperationProgress {
            kind: self.kind,
            phase: FileOperationPhase::Preparing,
            total_bytes: self.stats.total_bytes,
            copied_bytes: 0,
            total_files: self.stats.total_files,
            completed_files: 0,
            current_item: None,
            cancellable: self.kind != FileOperationKind::Extract,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileOperationRoot {
    source: PathBuf,
    destination: PathBuf,
    source_is_dir: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ArchiveExtractEntry {
    display_path: PathBuf,
    destination: PathBuf,
    conflict: bool,
    byte_weight: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum FileOperationStep {
    CreateDirectory(PathBuf),
    CopyFile {
        source: PathBuf,
        destination: PathBuf,
        conflict: bool,
    },
    MoveFile {
        source: PathBuf,
        destination: PathBuf,
        conflict: bool,
    },
    ExtractArchive {
        archive: PathBuf,
        destination: PathBuf,
        entries: Vec<ArchiveExtractEntry>,
    },
    RemoveEmptyDirectory(PathBuf),
}

fn prepare_file_operation(
    paths: &[PathBuf],
    destination: &Path,
    kind: FileOperationKind,
    copy_name_policy: CopyNamePolicy,
) -> Result<FileOperationJob, String> {
    if paths.is_empty() {
        return Err("No items were selected for drag-and-drop.".to_owned());
    }

    if !destination.is_dir() {
        return Err(format!(
            "{} is not a folder.",
            path_display_name(destination)
        ));
    }

    let destination_canonical = canonicalize_for_operation(destination)?;
    let mut reserved_destinations = HashSet::new();
    let mut steps = Vec::new();
    let mut roots = Vec::new();
    let mut stats = FileOperationStats {
        total_bytes: 0,
        total_files: 0,
    };

    for source in paths {
        if !source.exists() {
            return Err(format!("Could not find {}.", path_display_name(source)));
        }

        let file_name = source
            .file_name()
            .ok_or_else(|| format!("{} cannot be copied.", path_display_name(source)))?;
        let source_parent = source
            .parent()
            .ok_or_else(|| format!("{} cannot be moved or copied.", path_display_name(source)))?;
        let source_parent_canonical = canonicalize_for_operation(source_parent)?;
        let same_directory = source_parent_canonical == destination_canonical;
        if kind == FileOperationKind::Move && same_directory {
            continue;
        }

        let planned_destination = if kind == FileOperationKind::Copy
            && same_directory
            && copy_name_policy == CopyNamePolicy::UseCopyNamesInSameDirectory
        {
            paste_copy_destination(destination, file_name, &mut reserved_destinations)
        } else {
            let planned_destination = destination.join(file_name);
            if !reserved_destinations.insert(planned_destination.clone()) {
                return Err(format!(
                    "Multiple selected items are named {}.",
                    file_name.to_string_lossy()
                ));
            }
            planned_destination
        };

        if source.is_dir() {
            let source_canonical = canonicalize_for_operation(source)?;
            let canonical_planned_destination = destination_canonical.join(
                planned_destination
                    .file_name()
                    .unwrap_or_else(|| OsStr::new("")),
            );
            if canonical_planned_destination.starts_with(&source_canonical) {
                let operation = match kind {
                    FileOperationKind::Move => "move",
                    FileOperationKind::Copy => "copy",
                    FileOperationKind::Extract => "extract",
                };
                return Err(format!(
                    "Cannot {operation} {} into itself.",
                    path_display_name(source)
                ));
            }
        }

        plan_path_operation(source, &planned_destination, kind, &mut steps, &mut stats)?;
        roots.push(FileOperationRoot {
            source: source.clone(),
            destination: planned_destination,
            source_is_dir: source.is_dir(),
        });
    }

    Ok(FileOperationJob {
        kind,
        stats,
        steps,
        roots,
    })
}

pub(super) fn trash_paths(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("No items were selected to delete.".to_owned());
    }

    trash::delete_all(paths)
        .map_err(|error| format!("Could not move selected items to the Recycle Bin: {error}"))
}

pub(super) fn remove_paths_permanently(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("No items were selected to delete.".to_owned());
    }

    for path in paths {
        if !path.exists() {
            return Err(format!("Could not find {}.", path_display_name(path)));
        }
    }

    for path in paths {
        remove_source(path).map_err(|error| format_path_error("delete", path, error))?;
    }

    Ok(())
}

fn plan_path_operation(
    source: &Path,
    destination: &Path,
    kind: FileOperationKind,
    steps: &mut Vec<FileOperationStep>,
    stats: &mut FileOperationStats,
) -> Result<(), String> {
    let metadata =
        fs::metadata(source).map_err(|error| format_path_error("read", source, error))?;

    if metadata.is_dir() {
        if destination.exists() {
            if !destination.is_dir() {
                return Err(format!(
                    "{} already exists and is not a folder.",
                    path_display_name(destination)
                ));
            }
        } else {
            steps.push(FileOperationStep::CreateDirectory(
                destination.to_path_buf(),
            ));
        }

        for entry in
            fs::read_dir(source).map_err(|error| format_path_error("read", source, error))?
        {
            let entry = entry.map_err(|error| format_path_error("read", source, error))?;
            plan_path_operation(
                &entry.path(),
                &destination.join(entry.file_name()),
                kind,
                steps,
                stats,
            )?;
        }

        if kind == FileOperationKind::Move {
            steps.push(FileOperationStep::RemoveEmptyDirectory(
                source.to_path_buf(),
            ));
        }
    } else if destination.is_dir() {
        return Err(format!(
            "{} already exists and is a folder.",
            path_display_name(destination)
        ));
    } else {
        let conflict = destination.exists();
        stats.total_files += 1;
        stats.total_bytes = stats.total_bytes.saturating_add(metadata.len());
        match kind {
            FileOperationKind::Copy => steps.push(FileOperationStep::CopyFile {
                source: source.to_path_buf(),
                destination: destination.to_path_buf(),
                conflict,
            }),
            FileOperationKind::Move => steps.push(FileOperationStep::MoveFile {
                source: source.to_path_buf(),
                destination: destination.to_path_buf(),
                conflict,
            }),
            FileOperationKind::Extract => {}
        }
    }

    Ok(())
}

fn prepare_extract_archive_operation(
    archives: &[PathBuf],
    destination: &Path,
) -> Result<FileOperationJob, String> {
    if archives.is_empty() {
        return Err("No archive files were selected.".to_owned());
    }

    if !destination.is_dir() {
        return Err(format!(
            "{} is not a folder.",
            path_display_name(destination)
        ));
    }

    let mut steps = Vec::new();
    let mut roots = Vec::new();
    let mut reserved_destinations = HashSet::new();
    let mut stats = FileOperationStats {
        total_bytes: 0,
        total_files: 0,
    };

    for archive in archives {
        if !archive.exists() {
            return Err(format!("Could not find {}.", path_display_name(archive)));
        }

        if !archive.is_file() || !archive_path_is_supported(archive) {
            return Err(format!(
                "{} is not a supported archive.",
                path_display_name(archive)
            ));
        }

        let listing = archive_listing(archive)?;
        let top_level_entries = top_level_entries_from_listing(&listing.entries);
        if top_level_entries.is_empty() {
            return Err(format!(
                "{} does not contain any files.",
                path_display_name(archive)
            ));
        }

        let extract_to = archive_extract_destination(archive, destination, &top_level_entries)?;
        let output_roots = archive_output_roots(&extract_to, &top_level_entries);
        let archive_size = fs::metadata(archive)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let mut entries = planned_extract_entries_from_listing(&listing.entries, &extract_to);
        assign_archive_entry_byte_weights(&mut entries, archive_size);

        for entry in &mut entries {
            if !reserved_destinations.insert(entry.destination.clone()) {
                return Err(format!(
                    "Multiple selected archives contain {}.",
                    path_display_name(&entry.destination)
                ));
            }
            entry.conflict = entry.destination.exists();
        }

        steps.push(FileOperationStep::ExtractArchive {
            archive: archive.clone(),
            destination: extract_to,
            entries: entries.clone(),
        });

        stats.total_files = stats.total_files.saturating_add(entries.len().max(1));
        stats.total_bytes = stats.total_bytes.saturating_add(archive_size);

        for output in output_roots {
            roots.push(FileOperationRoot {
                source: archive.clone(),
                destination: output,
                source_is_dir: false,
            });
        }
    }

    Ok(FileOperationJob {
        kind: FileOperationKind::Extract,
        stats,
        steps,
        roots,
    })
}

fn file_conflicts_for_job(job: &FileOperationJob) -> Vec<FileConflict> {
    let mut file_conflicts = Vec::new();
    for step in &job.steps {
        match step {
            FileOperationStep::CopyFile {
                source,
                destination,
                conflict: true,
            }
            | FileOperationStep::MoveFile {
                source,
                destination,
                conflict: true,
            } => file_conflicts.push(FileConflict {
                source: source.clone(),
                destination: destination.clone(),
            }),
            FileOperationStep::ExtractArchive {
                archive, entries, ..
            } => {
                file_conflicts.extend(entries.iter().filter_map(|entry| {
                    entry.conflict.then(|| FileConflict {
                        source: archive.clone(),
                        destination: entry.destination.clone(),
                    })
                }));
            }
            _ => {}
        }
    }
    file_conflicts
}

#[cfg(test)]
fn execute_file_operation(
    job: FileOperationJob,
    conflict_choice: ConflictChoice,
) -> Result<FileOperationSummary, String> {
    execute_file_operation_with_progress(
        job,
        conflict_choice,
        Arc::new(AtomicBool::new(false)),
        |_| {},
    )
    .map_err(|error| match error {
        FileOperationError::Cancelled => "The file operation was cancelled.".to_owned(),
        FileOperationError::Failed(error) => error,
    })
}

pub(super) fn execute_file_operation_with_progress(
    job: FileOperationJob,
    conflict_choice: ConflictChoice,
    cancel: Arc<AtomicBool>,
    mut on_progress: impl FnMut(FileOperationProgress),
) -> Result<FileOperationSummary, FileOperationError> {
    let mut operated_destinations = HashSet::new();
    let mut progress = job.initial_progress();
    on_progress(progress.clone());

    for step in &job.steps {
        if cancel.load(Ordering::Relaxed) {
            progress.phase = FileOperationPhase::Cancelled;
            progress.cancellable = false;
            on_progress(progress);
            return Err(FileOperationError::Cancelled);
        }

        match step {
            FileOperationStep::CreateDirectory(path) => {
                progress.phase = FileOperationPhase::Preparing;
                progress.current_item = Some(path.clone());
                on_progress(progress.clone());
                fs::create_dir(path).map_err(|error| operation_error("create", path, error))?;
            }
            FileOperationStep::CopyFile {
                source,
                destination,
                conflict,
            } => {
                if *conflict && conflict_choice == ConflictChoice::Skip {
                    continue;
                }
                progress.phase = FileOperationPhase::Copying;
                progress.current_item = Some(source.clone());
                on_progress(progress.clone());
                copy_source_file_with_progress(
                    source,
                    destination,
                    &cancel,
                    &mut progress,
                    &mut on_progress,
                )
                .map_err(|error| operation_error("copy", source, error))?;
                operated_destinations.insert(destination.clone());
            }
            FileOperationStep::MoveFile {
                source,
                destination,
                conflict,
            } => {
                if *conflict && conflict_choice == ConflictChoice::Skip {
                    continue;
                }
                if *conflict {
                    progress.phase = FileOperationPhase::Copying;
                    progress.current_item = Some(source.clone());
                    on_progress(progress.clone());
                    copy_source_file_with_progress(
                        source,
                        destination,
                        &cancel,
                        &mut progress,
                        &mut on_progress,
                    )
                    .map_err(|error| operation_error("move", source, error))?;
                    remove_source(source)
                        .map_err(|error| operation_error("remove", source, error))?;
                } else {
                    progress.phase = FileOperationPhase::Moving;
                    progress.current_item = Some(source.clone());
                    on_progress(progress.clone());
                    move_source_file_with_progress(
                        source,
                        destination,
                        &cancel,
                        &mut progress,
                        &mut on_progress,
                    )
                    .map_err(|error| operation_error("move", source, error))?;
                }
                operated_destinations.insert(destination.clone());
            }
            FileOperationStep::ExtractArchive {
                archive,
                destination,
                entries,
            } => {
                progress.phase = FileOperationPhase::Extracting;
                on_progress(progress.clone());
                extract_archive_with_entry_progress(
                    archive,
                    destination,
                    entries,
                    conflict_choice,
                    &cancel,
                    &mut progress,
                    &mut on_progress,
                )?;
                operated_destinations.insert(destination.clone());
            }
            FileOperationStep::RemoveEmptyDirectory(path) => {
                progress.phase = FileOperationPhase::Removing;
                progress.current_item = Some(path.clone());
                on_progress(progress.clone());
                remove_empty_directory(path)
                    .map_err(|error| operation_error("remove", path, error))?;
            }
        }
    }

    let mut summary = FileOperationSummary::default();
    for root in &job.roots {
        if job.kind == FileOperationKind::Extract {
            if root.destination.exists() {
                summary.destination_paths.push(root.destination.clone());
            }
        } else if root.source_is_dir {
            if root.destination.exists() {
                summary.destination_paths.push(root.destination.clone());
            }
        } else if operated_destinations.contains(&root.destination) {
            summary.destination_paths.push(root.destination.clone());
        }

        if job.kind == FileOperationKind::Move && !root.source.exists() {
            summary.moved_source_paths.push(root.source.clone());
        }
    }

    progress.phase = FileOperationPhase::Finished;
    progress.current_item = None;
    progress.copied_bytes = progress.copied_bytes.max(progress.total_bytes);
    progress.completed_files = progress.completed_files.max(progress.total_files);
    progress.cancellable = false;
    on_progress(progress);

    Ok(summary)
}

fn extract_archive_with_entry_progress(
    archive: &Path,
    destination: &Path,
    entries: &[ArchiveExtractEntry],
    conflict_choice: ConflictChoice,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) -> Result<(), FileOperationError> {
    if archive_is_rar(archive) {
        extract_rar_archive_with_entry_progress(
            archive,
            destination,
            entries,
            conflict_choice,
            cancel,
            progress,
            on_progress,
        )?;
    } else if archive_is_ar(archive) {
        extract_ar_archive_with_entry_progress(
            archive,
            destination,
            entries,
            conflict_choice,
            cancel,
            progress,
            on_progress,
        )?;
    } else if archive_supports_filtered_extract(archive)
        || archive_is_single_file_compression(archive)
    {
        let entry_weights: HashMap<PathBuf, u64> = entries
            .iter()
            .map(|entry| (entry.destination.clone(), entry.byte_weight))
            .collect();
        let conflict_destinations: HashSet<PathBuf> = entries
            .iter()
            .filter(|entry| entry.conflict)
            .map(|entry| entry.destination.clone())
            .collect();

        let cancel_filter = cancel.clone();
        let (tx, rx) = std::sync::mpsc::channel();

        if let Some(entry) = entries.first() {
            progress.current_item = Some(entry.display_path.clone());
            on_progress(progress.clone());
        }

        let archive_buf = archive.to_path_buf();
        let destination_buf = destination.to_path_buf();

        let handle = std::thread::spawn(move || {
            let opts = decompress::ExtractOptsBuilder::default()
                .filter(move |path| {
                    if cancel_filter.load(Ordering::Relaxed) {
                        return false;
                    }

                    let is_conflict = conflict_destinations.contains(path);
                    let is_allowed = !is_conflict || conflict_choice == ConflictChoice::Replace;
                    if is_allowed {
                        if let Some(weight) = entry_weights.get(path) {
                            let _ = tx.send((path.to_path_buf(), *weight));
                        }
                    }
                    is_allowed
                })
                .build()
                .map_err(|error| {
                    FileOperationError::Failed(format!("Could not prepare extraction: {error}"))
                })?;

            decompress::decompress(&archive_buf, &destination_buf, &opts).map_err(|error| {
                FileOperationError::Failed(format_path_error(
                    "extract",
                    &archive_buf,
                    io::Error::other(error.to_string()),
                ))
            })?;
            
            Ok::<(), FileOperationError>(())
        });

        while let Ok((path, weight)) = rx.recv() {
            progress.current_item = Some(path);
            progress.copied_bytes = progress.copied_bytes.saturating_add(weight);
            progress.completed_files = progress.completed_files.saturating_add(1);
            on_progress(progress.clone());
        }

        handle
            .join()
            .map_err(|_| FileOperationError::Failed("Extraction thread panicked".to_owned()))??;
    } else {
        if let Some(entry) = entries.first() {
            progress.current_item = Some(entry.display_path.clone());
            on_progress(progress.clone());
        }

        let conflict_paths = entries
            .iter()
            .filter(|entry| entry.conflict)
            .map(|entry| entry.destination.clone())
            .collect::<HashSet<_>>();
        let cancel_filter = cancel.clone();
        let opts = decompress::ExtractOptsBuilder::default()
            .filter(move |path| {
                if cancel_filter.load(Ordering::Relaxed) {
                    return false;
                }
                conflict_choice == ConflictChoice::Replace || !conflict_paths.contains(path)
            })
            .build()
            .map_err(|error| {
                FileOperationError::Failed(format!("Could not prepare extraction: {error}"))
            })?;

        decompress::decompress(archive, destination, &opts).map_err(|error| {
            FileOperationError::Failed(format_path_error(
                "extract",
                archive,
                io::Error::other(error.to_string()),
            ))
        })?;

        progress.completed_files = progress
            .completed_files
            .saturating_add(entries.len().max(1));
        progress.copied_bytes = progress
            .copied_bytes
            .saturating_add(archive_entry_byte_total(entries));
        on_progress(progress.clone());
    }

    Ok(())
}

fn extract_ar_archive_with_entry_progress(
    archive: &Path,
    _destination: &Path,
    entries: &[ArchiveExtractEntry],
    conflict_choice: ConflictChoice,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) -> Result<(), FileOperationError> {
    let file = File::open(archive).map_err(|error| operation_error("read", archive, error))?;
    let mut reader = ar::Archive::new(file);

    while let Some(entry) = reader.next_entry() {
        if cancel.load(Ordering::Relaxed) {
            return Err(FileOperationError::Cancelled);
        }
        let mut archive_entry =
            entry.map_err(|error| operation_error("extract", archive, error))?;
        let entry_name = String::from_utf8_lossy(archive_entry.header().identifier());
        let display_path = sanitized_archive_entry_path(Path::new(entry_name.as_ref()));
        let Some(planned_entry) = entries
            .iter()
            .find(|entry| entry.display_path == display_path)
        else {
            continue;
        };

        progress.current_item = Some(planned_entry.display_path.clone());
        on_progress(progress.clone());

        if planned_entry.conflict && conflict_choice == ConflictChoice::Skip {
            progress.completed_files = progress.completed_files.saturating_add(1);
            on_progress(progress.clone());
            continue;
        }

        if let Some(parent) = planned_entry.destination.parent() {
            fs::create_dir_all(parent).map_err(|error| operation_error("create", parent, error))?;
        }
        let mut output = File::create(&planned_entry.destination)
            .map_err(|error| operation_error("extract", &planned_entry.destination, error))?;
        io::copy(&mut archive_entry, &mut output)
            .map_err(|error| operation_error("extract", &planned_entry.destination, error))?;
        progress.copied_bytes = progress
            .copied_bytes
            .saturating_add(planned_entry.byte_weight);
        progress.completed_files = progress.completed_files.saturating_add(1);
        on_progress(progress.clone());
    }

    Ok(())
}

fn extract_rar_archive_with_entry_progress(
    archive: &Path,
    destination: &Path,
    entries: &[ArchiveExtractEntry],
    conflict_choice: ConflictChoice,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) -> Result<(), FileOperationError> {
    let temp_directory = temp_extract_directory_for(destination)
        .map_err(|error| operation_error("create", destination, error))?;
    fs::create_dir_all(&temp_directory)
        .map_err(|error| operation_error("create", &temp_directory, error))?;

    let result = extract_rar_archive_to_temp(
        archive,
        &temp_directory,
        entries,
        conflict_choice,
        cancel,
        progress,
        on_progress,
    );

    let cleanup = fs::remove_dir_all(&temp_directory);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(error)) => Err(operation_error("remove", &temp_directory, error)),
        (Err(error), _) => Err(error),
    }
}

fn extract_rar_archive_to_temp(
    archive: &Path,
    temp_directory: &Path,
    entries: &[ArchiveExtractEntry],
    conflict_choice: ConflictChoice,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) -> Result<(), FileOperationError> {
    let archive_file = archive.to_string_lossy().to_string();
    let temp_destination = temp_directory.to_string_lossy().to_string();
    let mut archive_reader = unrar::Archive::new(archive_file)
        .extract_to(temp_destination)
        .map_err(|error| {
            FileOperationError::Failed(format!(
                "Could not extract {}: {error}",
                path_display_name(archive)
            ))
        })?;
    let mut index = 0;

    while let Some(result) = archive_reader.next() {
        if cancel.load(Ordering::Relaxed) {
            return Err(FileOperationError::Cancelled);
        }
        if let Some(entry) = entries.get(index) {
            progress.current_item = Some(entry.display_path.clone());
            on_progress(progress.clone());
        }

        let rar_entry = result.map_err(|error| {
            FileOperationError::Failed(format!(
                "Could not extract {}: {error}",
                path_display_name(archive)
            ))
        })?;
        let display_path = sanitized_archive_entry_path(Path::new(&rar_entry.filename));
        let planned_entry = entries
            .iter()
            .find(|entry| entry.display_path == display_path)
            .or_else(|| entries.get(index));
        index += 1;

        let Some(planned_entry) = planned_entry else {
            continue;
        };

        progress.current_item = Some(planned_entry.display_path.clone());
        if planned_entry.conflict && conflict_choice == ConflictChoice::Skip {
            remove_temp_extract_output(&temp_directory.join(&planned_entry.display_path))
                .map_err(|error| operation_error("remove", &planned_entry.destination, error))?;
            progress.completed_files = progress.completed_files.saturating_add(1);
            on_progress(progress.clone());
            continue;
        }

        merge_temp_extract_output(
            &temp_directory.join(&planned_entry.display_path),
            &planned_entry.destination,
            rar_entry.is_directory(),
        )?;
        progress.copied_bytes = progress
            .copied_bytes
            .saturating_add(planned_entry.byte_weight);
        progress.completed_files = progress.completed_files.saturating_add(1);
        on_progress(progress.clone());
    }

    Ok(())
}

fn archive_listing(archive: &Path) -> Result<decompress::Listing, String> {
    let opts = default_extract_opts()?;
    decompress::list(archive, &opts)
        .map_err(|error| format!("Could not list {}: {error}", path_display_name(archive)))
}

fn default_extract_opts() -> Result<decompress::ExtractOpts, String> {
    decompress::ExtractOptsBuilder::default()
        .build()
        .map_err(|error| format!("Could not prepare extraction: {error}"))
}

fn top_level_entries_from_listing(entries: &[String]) -> Vec<PathBuf> {
    let mut top_level_entries = Vec::new();
    let mut seen = HashSet::new();
    for entry in entries {
        let Some(top_level) = top_level_archive_component(Path::new(entry)) else {
            continue;
        };
        if seen.insert(top_level.clone()) {
            top_level_entries.push(top_level);
        }
    }

    top_level_entries
}

#[cfg(test)]
fn planned_output_paths_from_listing(entries: &[String], destination: &Path) -> Vec<PathBuf> {
    planned_extract_entries_from_listing(entries, destination)
        .into_iter()
        .map(|entry| entry.destination)
        .collect()
}

fn planned_extract_entries_from_listing(
    entries: &[String],
    destination: &Path,
) -> Vec<ArchiveExtractEntry> {
    let mut planned_entries = Vec::new();
    let mut seen = HashSet::new();
    for entry in entries {
        let relative = sanitized_archive_entry_path(Path::new(entry));
        if relative.as_os_str().is_empty() {
            continue;
        }
        let output = destination.join(relative);
        if seen.insert(output.clone()) {
            planned_entries.push(ArchiveExtractEntry {
                display_path: sanitized_archive_entry_path(Path::new(entry)),
                destination: output,
                conflict: false,
                byte_weight: 0,
            });
        }
    }
    planned_entries
}

fn assign_archive_entry_byte_weights(entries: &mut [ArchiveExtractEntry], archive_size: u64) {
    let entry_count = entries.len() as u64;
    if entry_count == 0 {
        return;
    }

    let base_weight = archive_size / entry_count;
    let mut remainder = archive_size % entry_count;
    for entry in entries {
        entry.byte_weight = base_weight + u64::from(remainder > 0);
        remainder = remainder.saturating_sub(1);
    }
}

fn archive_entry_byte_total(entries: &[ArchiveExtractEntry]) -> u64 {
    entries
        .iter()
        .map(|entry| entry.byte_weight)
        .fold(0_u64, u64::saturating_add)
}

fn archive_extract_destination(
    archive: &Path,
    destination: &Path,
    top_level_entries: &[PathBuf],
) -> Result<PathBuf, String> {
    if top_level_entries.len() == 1 {
        return Ok(destination.to_path_buf());
    }

    let name = archive_extract_root_name(archive)?;
    Ok(destination.join(name))
}

fn archive_output_roots(destination: &Path, top_level_entries: &[PathBuf]) -> Vec<PathBuf> {
    if top_level_entries.len() > 1 {
        return vec![destination.to_path_buf()];
    }

    top_level_entries
        .iter()
        .map(|entry| destination.join(entry))
        .collect()
}

fn archive_extract_root_name(archive: &Path) -> Result<OsString, String> {
    let file_name = archive
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| format!("{} cannot be extracted.", path_display_name(archive)))?;
    let lower = file_name.to_ascii_lowercase();

    for extension in COMPOUND_ARCHIVE_EXTENSIONS {
        let suffix = format!(".{extension}");
        if lower.ends_with(&suffix) && file_name.len() > suffix.len() {
            return Ok(OsString::from(&file_name[..file_name.len() - suffix.len()]));
        }
    }

    for extension in SIMPLE_ARCHIVE_EXTENSIONS {
        let suffix = format!(".{extension}");
        if lower.ends_with(&suffix) && file_name.len() > suffix.len() {
            return Ok(OsString::from(&file_name[..file_name.len() - suffix.len()]));
        }
    }

    archive
        .file_stem()
        .map(OsStr::to_os_string)
        .ok_or_else(|| format!("{} cannot be extracted.", path_display_name(archive)))
}

fn archive_name_has_supported_extension(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    COMPOUND_ARCHIVE_EXTENSIONS
        .iter()
        .chain(SIMPLE_ARCHIVE_EXTENSIONS.iter())
        .any(|extension| {
            let suffix = format!(".{extension}");
            lower.ends_with(&suffix) && file_name.len() > suffix.len()
        })
}

fn archive_is_single_file_compression(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            ["gz", "bz", "bz2", "xz", "zst"].iter().any(|extension| {
                lower.ends_with(&format!(".{extension}"))
                    && !COMPOUND_ARCHIVE_EXTENSIONS
                        .iter()
                        .any(|compound| lower.ends_with(&format!(".{compound}")))
            })
        })
        .unwrap_or(false)
}

fn archive_supports_filtered_extract(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            [
                "zip", "tar", "tar.gz", "tgz", "tar.bz2", "tbz", "tar.xz", "txz", "tar.zst", "tzst",
            ]
            .iter()
            .any(|extension| lower.ends_with(&format!(".{extension}")))
        })
        .unwrap_or(false)
}

fn archive_is_ar(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".ar") && name.len() > ".ar".len()
        })
        .unwrap_or(false)
}

fn archive_is_rar(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".rar") && name.len() > ".rar".len()
        })
        .unwrap_or(false)
}

fn top_level_archive_component(path: &Path) -> Option<PathBuf> {
    sanitized_archive_entry_path(path)
        .components()
        .next()
        .and_then(|component| match component {
            Component::Normal(name) => Some(PathBuf::from(name)),
            _ => None,
        })
}

fn sanitized_archive_entry_path(path: &Path) -> PathBuf {
    let mut sanitized = PathBuf::new();
    for component in path.components() {
        if let Component::Normal(name) = component {
            sanitized.push(name);
        }
    }
    sanitized
}

fn move_source_file_with_progress(
    source: &Path,
    destination: &Path,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) -> std::io::Result<()> {
    match fs::rename(source, destination) {
        Ok(()) => {
            let file_size = fs::metadata(destination)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            progress.copied_bytes = progress.copied_bytes.saturating_add(file_size);
            progress.completed_files += 1;
            on_progress(progress.clone());
            Ok(())
        }
        Err(error) if is_cross_device_error(&error) => {
            progress.phase = FileOperationPhase::Copying;
            on_progress(progress.clone());
            copy_source_file_with_progress(source, destination, cancel, progress, on_progress)?;
            remove_source(source)
        }
        Err(error) => Err(error),
    }
}

fn copy_source_file_with_progress(
    source: &Path,
    destination: &Path,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) -> std::io::Result<()> {
    let metadata = fs::metadata(source)?;
    let temp_destination = temp_destination_for(destination)?;
    let copy_result =
        copy_source_file_to_temp(source, &temp_destination, cancel, progress, on_progress);

    match copy_result {
        Ok(()) => {
            preserve_file_metadata(&metadata, &temp_destination)?;
            replace_destination_with_temp(&temp_destination, destination)?;
            progress.completed_files += 1;
            on_progress(progress.clone());
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temp_destination);
            Err(error)
        }
    }
}

fn copy_source_file_to_temp(
    source: &Path,
    temp_destination: &Path,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) -> std::io::Result<()> {
    let mut source_file = File::open(source)?;
    let mut destination_file = File::create(temp_destination)?;
    let mut buffer = vec![0; COPY_BUFFER_SIZE];

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "file operation cancelled",
            ));
        }

        let read = source_file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        destination_file.write_all(&buffer[..read])?;
        progress.copied_bytes = progress.copied_bytes.saturating_add(read as u64);
        on_progress(progress.clone());
    }

    destination_file.sync_all()?;
    Ok(())
}

fn preserve_file_metadata(metadata: &fs::Metadata, destination: &Path) -> std::io::Result<()> {
    fs::set_permissions(destination, metadata.permissions())?;
    let accessed = FileTime::from_last_access_time(metadata);
    let modified = FileTime::from_last_modification_time(metadata);
    filetime::set_file_times(destination, accessed, modified)
}

fn temp_destination_for(destination: &Path) -> std::io::Result<PathBuf> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .unwrap_or_else(|| OsStr::new("file"))
        .to_string_lossy();
    let process_id = std::process::id();

    loop {
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".explorer-copy-{process_id}-{counter}-{file_name}.tmp"
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
}

fn temp_extract_directory_for(destination: &Path) -> std::io::Result<PathBuf> {
    let parent = if destination.is_dir() {
        destination
    } else {
        destination.parent().unwrap_or_else(|| Path::new("."))
    };
    let process_id = std::process::id();

    loop {
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".explorer-extract-{process_id}-{counter}.tmp"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
}

fn merge_temp_extract_output(
    source: &Path,
    destination: &Path,
    is_directory: bool,
) -> Result<(), FileOperationError> {
    if is_directory || source.is_dir() {
        fs::create_dir_all(destination)
            .map_err(|error| operation_error("create", destination, error))?;
        remove_temp_extract_output(source)
            .map_err(|error| operation_error("remove", source, error))?;
        return Ok(());
    }

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| operation_error("create", parent, error))?;
    }

    if destination.is_dir() {
        return Err(FileOperationError::Failed(format!(
            "{} already exists and is a folder.",
            path_display_name(destination)
        )));
    }

    replace_destination_with_temp(source, destination)
        .map_err(|error| operation_error("extract", destination, error))
}

fn remove_temp_extract_output(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else if path.exists() {
        fs::remove_file(path)
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
fn replace_destination_with_temp(temp: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(temp, destination)
}

#[cfg(target_os = "windows")]
fn replace_destination_with_temp(temp: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::core::PCWSTR;

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let temp = wide(temp);
    let destination = wide(destination);
    unsafe {
        MoveFileExW(
            PCWSTR(temp.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|error| std::io::Error::other(error.to_string()))
    }
}

fn remove_empty_directory(path: &Path) -> std::io::Result<()> {
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn remove_source(source: &Path) -> std::io::Result<()> {
    if source.is_dir() {
        fs::remove_dir_all(source)
    } else {
        fs::remove_file(source)
    }
}

fn paste_copy_destination(
    destination: &Path,
    file_name: &OsStr,
    reserved_destinations: &mut HashSet<PathBuf>,
) -> PathBuf {
    let mut copy_number = 1;

    loop {
        let candidate = destination.join(copy_file_name(file_name, copy_number));
        if !candidate.exists() && reserved_destinations.insert(candidate.clone()) {
            return candidate;
        }
        copy_number += 1;
    }
}

fn copy_file_name(file_name: &OsStr, copy_number: usize) -> OsString {
    let file_name = file_name.to_string_lossy();
    let path = Path::new(file_name.as_ref());
    let stem = path
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or(file_name.as_ref());
    let extension = path.extension().and_then(OsStr::to_str);
    let suffix = if copy_number == 1 {
        " - Copy".to_owned()
    } else {
        format!(" - Copy ({copy_number})")
    };

    match extension {
        Some(extension) => OsString::from(format!("{stem}{suffix}.{extension}")),
        None => OsString::from(format!("{stem}{suffix}")),
    }
}

fn canonicalize_for_operation(path: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|error| format_path_error("read", path, error))
}

fn is_cross_device_error(error: &std::io::Error) -> bool {
    matches!(error.kind(), std::io::ErrorKind::CrossesDevices)
}

fn operation_error(operation: &str, path: &Path, error: std::io::Error) -> FileOperationError {
    if error.kind() == std::io::ErrorKind::Interrupted && error.to_string().contains("cancelled") {
        FileOperationError::Cancelled
    } else {
        FileOperationError::Failed(format_path_error(operation, path, error))
    }
}

fn format_path_error(operation: &str, path: &Path, error: std::io::Error) -> String {
    format!("Could not {operation} {}: {error}", path_display_name(path))
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
    use crate::explorer::test_support::TempDir;
    #[cfg(target_os = "windows")]
    use windows::{
        Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES,
            SetFileAttributesW,
        },
        core::PCWSTR,
    };

    #[cfg(target_os = "windows")]
    fn set_windows_file_attributes(path: &Path, attributes: FILE_FLAGS_AND_ATTRIBUTES) {
        use std::os::windows::ffi::OsStrExt;

        let mut wide_path = path.as_os_str().encode_wide().collect::<Vec<_>>();
        wide_path.push(0);
        unsafe {
            SetFileAttributesW(PCWSTR(wide_path.as_ptr()), attributes)
                .expect("set windows file attributes");
        }
    }

    fn create_ar_archive(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).expect("create ar archive");
        let mut builder = ar::Builder::new(file);
        for (name, data) in entries {
            let header = ar::Header::new(name.as_bytes().to_vec(), data.len() as u64);
            let mut reader = *data;
            builder
                .append(&header, &mut reader)
                .expect("append ar entry");
        }
        builder.into_inner().expect("finish ar archive");
    }

    #[test]
    fn archive_extract_root_name_strips_simple_and_compound_extensions() {
        assert_eq!(
            archive_extract_root_name(Path::new("package.zip")).unwrap(),
            OsString::from("package")
        );
        assert_eq!(
            archive_extract_root_name(Path::new("package.tar.gz")).unwrap(),
            OsString::from("package")
        );
        assert_eq!(
            archive_extract_root_name(Path::new("package.tar.zst")).unwrap(),
            OsString::from("package")
        );
        assert_eq!(
            archive_extract_root_name(Path::new("package.rar")).unwrap(),
            OsString::from("package")
        );
    }

    #[test]
    fn top_level_entries_from_listing_counts_unique_roots() {
        let entries = vec![
            "file.txt".to_owned(),
            "folder/a.txt".to_owned(),
            "folder/nested/b.txt".to_owned(),
            "../ignored.txt".to_owned(),
            "/rooted.txt".to_owned(),
        ];

        assert_eq!(
            top_level_entries_from_listing(&entries),
            vec![
                PathBuf::from("file.txt"),
                PathBuf::from("folder"),
                PathBuf::from("ignored.txt"),
                PathBuf::from("rooted.txt"),
            ]
        );
    }

    #[test]
    fn archive_destination_uses_current_directory_for_single_root() {
        let destination = Path::new("downloads");
        let top_level_entries = vec![PathBuf::from("folder")];

        assert_eq!(
            archive_extract_destination(
                Path::new("downloads/archive.zip"),
                destination,
                &top_level_entries,
            )
            .unwrap(),
            PathBuf::from("downloads")
        );
        assert_eq!(
            archive_output_roots(destination, &top_level_entries),
            vec![PathBuf::from("downloads/folder")]
        );
    }

    #[test]
    fn archive_destination_uses_archive_named_folder_for_multiple_roots() {
        let destination = Path::new("downloads");
        let top_level_entries = vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")];

        let extract_to = archive_extract_destination(
            Path::new("downloads/archive.tar.gz"),
            destination,
            &top_level_entries,
        )
        .unwrap();

        assert_eq!(extract_to, PathBuf::from("downloads/archive"));
        assert_eq!(
            archive_output_roots(&extract_to, &top_level_entries),
            vec![PathBuf::from("downloads/archive")]
        );
    }

    #[test]
    fn planned_output_paths_sanitize_and_dedupe_listing_entries() {
        let entries = vec![
            "folder/a.txt".to_owned(),
            "folder/a.txt".to_owned(),
            "./folder/b.txt".to_owned(),
            "../outside.txt".to_owned(),
        ];

        assert_eq!(
            planned_output_paths_from_listing(&entries, Path::new("dest")),
            vec![
                PathBuf::from("dest/folder/a.txt"),
                PathBuf::from("dest/folder/b.txt"),
                PathBuf::from("dest/outside.txt"),
            ]
        );
    }

    #[test]
    fn extract_progress_reports_archive_bytes_per_entry() {
        let temp = TempDir::new();
        let archive = temp.path().join("archive.ar");
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).expect("create destination");
        create_ar_archive(&archive, &[("a.txt", b"one"), ("b.txt", b"two")]);
        let archive_size = fs::metadata(&archive).expect("archive metadata").len();
        let job = ready_job(prepare_extract_archives_to_directory(
            std::slice::from_ref(&archive),
            &destination,
        ));
        let mut progress_events = Vec::new();

        let summary = execute_file_operation_with_progress(
            job,
            ConflictChoice::Replace,
            Arc::new(AtomicBool::new(false)),
            |progress| progress_events.push(progress),
        )
        .expect("extract with progress");

        let extracted_root = destination.join("archive");
        assert_eq!(fs::read(extracted_root.join("a.txt")).unwrap(), b"one");
        assert_eq!(fs::read(extracted_root.join("b.txt")).unwrap(), b"two");
        assert_eq!(summary.destination_paths, vec![extracted_root]);
        assert!(progress_events.iter().any(|progress| {
            progress.phase == FileOperationPhase::Extracting && progress.copied_bytes > 0
        }));
        assert_eq!(
            progress_events.last().map(|progress| progress.copied_bytes),
            Some(archive_size)
        );
    }

    #[test]
    fn extract_progress_skip_conflict_advances_items_without_skipped_bytes() {
        let temp = TempDir::new();
        let archive = temp.path().join("archive.ar");
        let destination = temp.path().join("destination");
        let extracted_root = destination.join("archive");
        fs::create_dir(&destination).expect("create destination");
        fs::create_dir(&extracted_root).expect("create extracted root");
        fs::write(extracted_root.join("a.txt"), b"existing").expect("create conflict");
        create_ar_archive(&archive, &[("a.txt", b"one"), ("b.txt", b"two")]);
        let conflicts = prepared_conflict_batch(prepare_extract_archives_to_directory(
            std::slice::from_ref(&archive),
            &destination,
        ));
        let mut progress_events = Vec::new();

        execute_file_operation_with_progress(
            conflicts.into_job(),
            ConflictChoice::Skip,
            Arc::new(AtomicBool::new(false)),
            |progress| progress_events.push(progress),
        )
        .expect("extract with skipped conflict");

        assert_eq!(fs::read(extracted_root.join("a.txt")).unwrap(), b"existing");
        assert_eq!(fs::read(extracted_root.join("b.txt")).unwrap(), b"two");
        assert!(progress_events.iter().any(|progress| {
            progress.phase == FileOperationPhase::Extracting
                && progress.completed_files == 1
                && progress.copied_bytes == 0
        }));
        assert!(progress_events.iter().any(|progress| {
            progress.phase == FileOperationPhase::Extracting && progress.copied_bytes > 0
        }));
    }

    #[test]
    fn default_start_path_prefers_existing_downloads_directory() {
        let temp = TempDir::new();
        let home = temp.path().join("home");
        let downloads = home.join("Downloads");
        let current = temp.path().join("current");
        fs::create_dir_all(&downloads).expect("create downloads");
        fs::create_dir(&current).expect("create current directory");

        let start_path = preferred_start_path(
            Some(downloads.clone()),
            Some(home.clone()),
            Some(current.clone()),
        );

        assert_eq!(start_path, downloads);
    }

    #[test]
    fn default_start_path_falls_back_to_home_when_downloads_is_missing() {
        let temp = TempDir::new();
        let home = temp.path().join("home");
        let downloads = home.join("Downloads");
        let current = temp.path().join("current");
        fs::create_dir(&home).expect("create home");
        fs::create_dir(&current).expect("create current directory");

        let start_path =
            preferred_start_path(Some(downloads), Some(home.clone()), Some(current.clone()));

        assert_eq!(start_path, home);
    }

    #[test]
    fn default_start_path_falls_back_to_current_directory_or_dot_without_home() {
        let temp = TempDir::new();
        let current = temp.path().join("current");
        fs::create_dir(&current).expect("create current directory");

        assert_eq!(
            preferred_start_path(None, None, Some(current.clone())),
            current
        );
        assert_eq!(preferred_start_path(None, None, None), PathBuf::from("."));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn local_drive_roots_falls_back_to_unix_root() {
        assert_eq!(local_drive_roots(), vec![PathBuf::from("/")]);
    }

    #[test]
    fn hidden_entry_filter_omits_dot_prefixed_entries_when_enabled() {
        let temp = TempDir::new();
        fs::write(temp.path().join(".hidden"), b"hidden").expect("create hidden file");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible file");

        let entries = load_entries_with_options(
            temp.path(),
            EntryLoadOptions {
                hide_hidden_entries: true,
                applications_view: false,
            },
        )
        .expect("load entries");

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["visible.txt"]
        );
    }

    #[test]
    fn hidden_entry_filter_keeps_dot_prefixed_entries_when_disabled() {
        let temp = TempDir::new();
        fs::write(temp.path().join(".hidden"), b"hidden").expect("create hidden file");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible file");

        let entries = load_entries_with_options(
            temp.path(),
            EntryLoadOptions {
                hide_hidden_entries: false,
                applications_view: false,
            },
        )
        .expect("load entries");

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec![".hidden", "visible.txt"]
        );
    }

    #[test]
    fn load_entries_omits_hidden_entries_when_show_hidden_files_is_false() {
        let temp = TempDir::new();
        fs::write(temp.path().join(".hidden"), b"hidden").expect("create hidden file");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible file");

        let entries = load_entries(temp.path(), false).expect("load entries");

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["visible.txt"]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn load_entries_omits_windows_hidden_attribute_entries_when_show_hidden_files_is_false() {
        let temp = TempDir::new();
        let hidden = temp.path().join("hidden.txt");
        fs::write(&hidden, b"hidden").expect("create hidden file");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible file");
        set_windows_file_attributes(&hidden, FILE_ATTRIBUTE_HIDDEN);

        let hidden_off = load_entries(temp.path(), false).expect("load entries");
        let hidden_on = load_entries(temp.path(), true).expect("load entries");

        assert_eq!(
            hidden_off
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["visible.txt"]
        );
        assert_eq!(
            hidden_on
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["hidden.txt", "visible.txt"]
        );

        set_windows_file_attributes(&hidden, FILE_ATTRIBUTE_NORMAL);
    }

    #[test]
    fn load_entries_keeps_hidden_entries_when_show_hidden_files_is_true() {
        let temp = TempDir::new();
        fs::write(temp.path().join(".hidden"), b"hidden").expect("create hidden file");
        fs::write(temp.path().join(".DS_Store"), b"metadata").expect("create metadata file");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible file");

        let entries = load_entries(temp.path(), true).expect("load entries");

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec![".hidden", "visible.txt"]
        );
    }

    #[test]
    fn metadata_entry_filter_omits_macos_metadata_names_even_when_hidden_filter_is_disabled() {
        let temp = TempDir::new();
        fs::write(temp.path().join(".DS_Store"), b"metadata").expect("create ds store file");
        fs::write(temp.path().join(".hidden"), b"hidden").expect("create hidden file");
        fs::write(temp.path().join(".localized"), b"metadata").expect("create localized file");
        fs::write(temp.path().join("visible.txt"), b"visible").expect("create visible file");

        let entries = load_entries_with_options(
            temp.path(),
            EntryLoadOptions {
                hide_hidden_entries: false,
                applications_view: false,
            },
        )
        .expect("load entries");

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec![".hidden", "visible.txt"]
        );
    }

    #[test]
    fn applications_view_includes_direct_and_one_level_nested_app_bundles() {
        let temp = TempDir::new();
        let preview = temp.path().join("Preview.app");
        let utilities = temp.path().join("Utilities");
        let terminal = utilities.join("Terminal.app");
        let macports = temp.path().join("MacPorts");
        fs::create_dir(&preview).expect("create direct app");
        fs::create_dir(&utilities).expect("create utilities");
        fs::create_dir(&terminal).expect("create nested app");
        fs::create_dir(&macports).expect("create non-app folder");
        fs::write(temp.path().join("readme.txt"), b"not an app").expect("create file");

        let entries = load_entries_with_options(
            temp.path(),
            EntryLoadOptions {
                hide_hidden_entries: true,
                applications_view: true,
            },
        )
        .expect("load applications view");

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
            vec![preview, terminal]
        );
    }

    #[test]
    fn applications_view_omits_hidden_direct_and_nested_app_bundles() {
        let temp = TempDir::new();
        let visible = temp.path().join("Visible.app");
        let hidden_direct = temp.path().join(".Hidden.app");
        let utilities = temp.path().join("Utilities");
        let hidden_nested = utilities.join(".Nested.app");
        fs::create_dir(&visible).expect("create visible app");
        fs::create_dir(&hidden_direct).expect("create hidden direct app");
        fs::create_dir(&utilities).expect("create utilities");
        fs::create_dir(&hidden_nested).expect("create hidden nested app");

        let entries = load_entries_with_options(
            temp.path(),
            EntryLoadOptions {
                hide_hidden_entries: true,
                applications_view: true,
            },
        )
        .expect("load applications view");

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
            vec![visible]
        );
    }

    #[test]
    fn normal_entries_view_keeps_non_app_folders() {
        let temp = TempDir::new();
        let utilities = temp.path().join("Utilities");
        let terminal = utilities.join("Terminal.app");
        fs::create_dir(&utilities).expect("create utilities");
        fs::create_dir(&terminal).expect("create nested app");

        let entries = load_entries_with_options(
            temp.path(),
            EntryLoadOptions {
                hide_hidden_entries: false,
                applications_view: false,
            },
        )
        .expect("load normal view");

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Utilities"]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_drive_filter_includes_this_pc_local_drive_types() {
        assert!(windows_drive_type_is_explorer_local(3));
        assert!(windows_drive_type_is_explorer_local(2));
        assert!(windows_drive_type_is_explorer_local(5));
        assert!(windows_drive_type_is_explorer_local(6));
        assert!(!windows_drive_type_is_explorer_local(4));
        assert!(!windows_drive_type_is_explorer_local(1));
        assert!(!windows_drive_type_is_explorer_local(0));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_drive_label_uses_custom_volume_name() {
        assert_eq!(
            windows_drive_display_label(r"C:\", Some("Work")),
            "Work (C:)"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_drive_label_falls_back_for_empty_volume_name() {
        assert_eq!(
            windows_drive_display_label(r"C:\", Some("")),
            "Local Disk (C:)"
        );
        assert_eq!(windows_drive_display_label(r"C:\", None), "Local Disk (C:)");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn drive_display_label_uses_path_label_on_non_windows() {
        assert_eq!(drive_display_label(Path::new("/")), "/");
    }

    fn finished_summary(result: Result<FileOperationOutcome, String>) -> FileOperationSummary {
        match result.expect("file operation") {
            FileOperationOutcome::Finished(summary) => summary,
            FileOperationOutcome::Conflicts(conflicts) => {
                panic!(
                    "expected operation to finish, found {} conflicts",
                    conflicts.len()
                )
            }
        }
    }

    fn conflict_batch(result: Result<FileOperationOutcome, String>) -> FileConflictBatch {
        match result.expect("file operation") {
            FileOperationOutcome::Conflicts(conflicts) => conflicts,
            FileOperationOutcome::Finished(_) => panic!("expected file conflicts"),
        }
    }

    fn ready_job(result: Result<PreparedFileOperation, String>) -> FileOperationJob {
        match result.expect("prepared operation") {
            PreparedFileOperation::Ready(job) => job,
            PreparedFileOperation::Conflicts(conflicts) => {
                panic!(
                    "expected ready operation, found {} conflicts",
                    conflicts.len()
                )
            }
        }
    }

    fn prepared_conflict_batch(result: Result<PreparedFileOperation, String>) -> FileConflictBatch {
        match result.expect("prepared operation") {
            PreparedFileOperation::Conflicts(conflicts) => conflicts,
            PreparedFileOperation::Ready(_) => panic!("expected file conflicts"),
        }
    }

    fn temp_copy_files(path: &Path) -> Vec<PathBuf> {
        fs::read_dir(path)
            .expect("read temp files")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(OsStr::to_str)
                    .is_some_and(|name| name.starts_with(".explorer-copy-"))
            })
            .collect()
    }

    #[test]
    fn prepared_operation_counts_nested_file_totals() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        let destination = temp.path().join("destination");
        fs::create_dir_all(source.join("nested")).expect("create nested source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(source.join("a.txt"), b"abc").expect("create first file");
        fs::write(source.join("nested").join("b.txt"), b"defg").expect("create second file");

        let job = ready_job(prepare_copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));

        assert_eq!(job.stats.total_files, 2);
        assert_eq!(job.stats.total_bytes, 7);
    }

    #[test]
    fn progress_copy_uses_temp_file_and_reports_bytes() {
        let temp = TempDir::new();
        let source = temp.path().join("large.bin");
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).expect("create destination");
        let data = vec![7; COPY_BUFFER_SIZE + 128];
        fs::write(&source, &data).expect("create source");
        let job = ready_job(prepare_copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let mut progress_events = Vec::new();

        let summary = execute_file_operation_with_progress(
            job,
            ConflictChoice::Replace,
            Arc::new(AtomicBool::new(false)),
            |progress| progress_events.push(progress),
        )
        .expect("copy with progress");

        let copied = destination.join("large.bin");
        assert_eq!(fs::read(&copied).unwrap(), data);
        assert_eq!(summary.destination_paths, vec![copied]);
        assert!(temp_copy_files(&destination).is_empty());
        assert!(
            progress_events
                .iter()
                .any(|progress| progress.copied_bytes > 0)
        );
        assert_eq!(
            progress_events.last().map(|progress| progress.phase),
            Some(FileOperationPhase::Finished)
        );
    }

    #[test]
    fn cancelling_chunked_copy_removes_temp_file_and_keeps_source() {
        let temp = TempDir::new();
        let source = temp.path().join("large.bin");
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).expect("create destination");
        fs::write(&source, vec![9; COPY_BUFFER_SIZE + 128]).expect("create source");
        let job = ready_job(prepare_copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut requested_cancel = false;

        let result = execute_file_operation_with_progress(
            job,
            ConflictChoice::Replace,
            cancel.clone(),
            |progress| {
                if progress.copied_bytes > 0 && !requested_cancel {
                    requested_cancel = true;
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );

        assert_eq!(result, Err(FileOperationError::Cancelled));
        assert!(source.exists());
        assert!(!destination.join("large.bin").exists());
        assert!(temp_copy_files(&destination).is_empty());
    }

    #[test]
    fn chunked_copy_preserves_modified_time() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).expect("create destination");
        fs::write(&source, b"data").expect("create source");
        let modified = FileTime::from_unix_time(1_700_000_000, 0);
        filetime::set_file_mtime(&source, modified).expect("set modified time");
        let job = ready_job(prepare_copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));

        execute_file_operation_with_progress(
            job,
            ConflictChoice::Replace,
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .expect("copy with metadata");

        let copied_metadata = fs::metadata(destination.join("file.txt")).unwrap();
        assert_eq!(
            FileTime::from_last_modification_time(&copied_metadata),
            modified
        );
    }

    #[test]
    fn move_file_to_directory() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::write(&source, b"data").expect("create file");
        fs::create_dir(&destination).expect("create destination");

        let moved = move_paths_to_directory(std::slice::from_ref(&source), &destination)
            .expect("move file");
        let moved = finished_summary(Ok(moved));

        assert!(!source.exists());
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"data");
        assert_eq!(moved.destination_paths, vec![destination.join("file.txt")]);
        assert_eq!(moved.moved_source_paths, vec![source]);
    }

    #[test]
    fn move_directory_recursively_to_directory() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        let nested = source.join("nested");
        let destination = temp.path().join("destination");
        fs::create_dir_all(&nested).expect("create nested source");
        fs::write(nested.join("file.txt"), b"data").expect("create nested file");
        fs::create_dir(&destination).expect("create destination");

        let moved = move_paths_to_directory(std::slice::from_ref(&source), &destination)
            .expect("move directory");
        let moved = finished_summary(Ok(moved));

        assert!(!source.exists());
        assert_eq!(
            fs::read(destination.join("folder").join("nested").join("file.txt")).unwrap(),
            b"data"
        );
        assert_eq!(moved.destination_paths, vec![destination.join("folder")]);
        assert_eq!(moved.moved_source_paths, vec![source]);
    }

    #[test]
    fn copy_file_to_directory() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::write(&source, b"data").expect("create file");
        fs::create_dir(&destination).expect("create destination");

        let copied = copy_paths_to_directory(std::slice::from_ref(&source), &destination)
            .expect("copy file");
        let copied = finished_summary(Ok(copied));

        assert_eq!(fs::read(&source).unwrap(), b"data");
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"data");
        assert_eq!(copied.destination_paths, vec![destination.join("file.txt")]);
        assert!(copied.moved_source_paths.is_empty());
    }

    #[test]
    fn copy_directory_recursively_to_directory() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        let nested = source.join("nested");
        let destination = temp.path().join("destination");
        fs::create_dir_all(&nested).expect("create nested source");
        fs::write(nested.join("file.txt"), b"data").expect("create nested file");
        fs::create_dir(&destination).expect("create destination");

        let copied = copy_paths_to_directory(std::slice::from_ref(&source), &destination)
            .expect("copy directory");
        let copied = finished_summary(Ok(copied));

        assert!(source.exists());
        assert_eq!(
            fs::read(destination.join("folder").join("nested").join("file.txt")).unwrap(),
            b"data"
        );
        assert_eq!(copied.destination_paths, vec![destination.join("folder")]);
    }

    #[test]
    fn copy_conflict_replace_overwrites_destination() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::write(&source, b"source").expect("create source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(destination.join("file.txt"), b"existing").expect("create existing");

        let conflicts = conflict_batch(copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let summary =
            resolve_file_conflicts(conflicts, ConflictChoice::Replace).expect("replace conflict");

        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"source");
        assert_eq!(
            summary.destination_paths,
            vec![destination.join("file.txt")]
        );
    }

    #[test]
    fn copy_conflict_skip_leaves_files_unchanged() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::write(&source, b"source").expect("create source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(destination.join("file.txt"), b"existing").expect("create existing");

        let conflicts = conflict_batch(copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let summary =
            resolve_file_conflicts(conflicts, ConflictChoice::Skip).expect("skip conflict");

        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"existing");
        assert!(summary.destination_paths.is_empty());
    }

    #[test]
    fn move_conflict_replace_overwrites_destination_and_removes_source() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::write(&source, b"source").expect("create source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(destination.join("file.txt"), b"existing").expect("create existing");

        let conflicts = conflict_batch(move_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let summary =
            resolve_file_conflicts(conflicts, ConflictChoice::Replace).expect("replace conflict");

        assert!(!source.exists());
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"source");
        assert_eq!(
            summary.destination_paths,
            vec![destination.join("file.txt")]
        );
        assert_eq!(summary.moved_source_paths, vec![source]);
    }

    #[test]
    fn move_conflict_skip_leaves_files_unchanged() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::write(&source, b"source").expect("create source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(destination.join("file.txt"), b"existing").expect("create existing");

        let conflicts = conflict_batch(move_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        let summary =
            resolve_file_conflicts(conflicts, ConflictChoice::Skip).expect("skip conflict");

        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"existing");
        assert!(summary.destination_paths.is_empty());
        assert!(summary.moved_source_paths.is_empty());
    }

    #[test]
    fn multiple_conflicts_replace_applies_to_all_conflicts() {
        let temp = TempDir::new();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir(&source).expect("create source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(source.join("a.txt"), b"new a").expect("create source a");
        fs::write(source.join("b.txt"), b"new b").expect("create source b");
        fs::write(destination.join("a.txt"), b"old a").expect("create destination a");
        fs::write(destination.join("b.txt"), b"old b").expect("create destination b");

        let conflicts = conflict_batch(copy_paths_to_directory(
            &[source.join("a.txt"), source.join("b.txt")],
            &destination,
        ));
        assert_eq!(conflicts.len(), 2);

        resolve_file_conflicts(conflicts, ConflictChoice::Replace).expect("replace conflicts");

        assert_eq!(fs::read(destination.join("a.txt")).unwrap(), b"new a");
        assert_eq!(fs::read(destination.join("b.txt")).unwrap(), b"new b");
    }

    #[test]
    fn multiple_conflicts_skip_applies_to_all_conflicts() {
        let temp = TempDir::new();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir(&source).expect("create source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(source.join("a.txt"), b"new a").expect("create source a");
        fs::write(source.join("b.txt"), b"new b").expect("create source b");
        fs::write(destination.join("a.txt"), b"old a").expect("create destination a");
        fs::write(destination.join("b.txt"), b"old b").expect("create destination b");

        let conflicts = conflict_batch(copy_paths_to_directory(
            &[source.join("a.txt"), source.join("b.txt")],
            &destination,
        ));
        assert_eq!(conflicts.len(), 2);

        let summary =
            resolve_file_conflicts(conflicts, ConflictChoice::Skip).expect("skip conflicts");

        assert_eq!(fs::read(destination.join("a.txt")).unwrap(), b"old a");
        assert_eq!(fs::read(destination.join("b.txt")).unwrap(), b"old b");
        assert!(summary.destination_paths.is_empty());
    }

    #[test]
    fn mixed_conflicting_and_non_conflicting_files_continue_after_skip() {
        let temp = TempDir::new();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir(&source).expect("create source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(source.join("conflict.txt"), b"new").expect("create conflict source");
        fs::write(source.join("new.txt"), b"new file").expect("create non-conflict source");
        fs::write(destination.join("conflict.txt"), b"old").expect("create conflict destination");

        let conflicts = conflict_batch(copy_paths_to_directory(
            &[source.join("conflict.txt"), source.join("new.txt")],
            &destination,
        ));
        let summary =
            resolve_file_conflicts(conflicts, ConflictChoice::Skip).expect("skip conflicts");

        assert_eq!(fs::read(destination.join("conflict.txt")).unwrap(), b"old");
        assert_eq!(fs::read(destination.join("new.txt")).unwrap(), b"new file");
        assert_eq!(summary.destination_paths, vec![destination.join("new.txt")]);
    }

    #[test]
    fn duplicate_destination_names_fail_before_copying() {
        let temp = TempDir::new();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let destination = temp.path().join("destination");
        fs::create_dir_all(&first).expect("create first");
        fs::create_dir_all(&second).expect("create second");
        fs::create_dir(&destination).expect("create destination");
        let first_file = first.join("file.txt");
        let second_file = second.join("file.txt");
        fs::write(&first_file, b"first").expect("create first file");
        fs::write(&second_file, b"second").expect("create second file");

        let error = copy_paths_to_directory(&[first_file, second_file], &destination)
            .expect_err("duplicate names should fail");

        assert!(error.contains("Multiple selected items are named file.txt"));
        assert!(!destination.join("file.txt").exists());
    }

    #[test]
    fn same_directory_move_is_no_op() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        fs::write(&source, b"data").expect("create source");

        let moved =
            move_paths_to_directory(std::slice::from_ref(&source), temp.path()).expect("move noop");
        let moved = finished_summary(Ok(moved));

        assert!(moved.destination_paths.is_empty());
        assert_eq!(fs::read(&source).unwrap(), b"data");
    }

    #[test]
    fn moving_directory_into_descendant_fails() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        let descendant = source.join("child");
        fs::create_dir_all(&descendant).expect("create descendant");

        let error = move_paths_to_directory(std::slice::from_ref(&source), &descendant)
            .expect_err("descendant move should fail");

        assert!(error.contains("Cannot move folder into itself"));
        assert!(source.exists());
        assert!(descendant.exists());
    }

    #[test]
    fn paste_copy_in_same_directory_uses_windows_copy_names() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        fs::write(&source, b"data").expect("create source");
        fs::write(temp.path().join("file - Copy.txt"), b"existing").expect("create first copy");

        let copied = copy_paths_to_directory_for_paste(std::slice::from_ref(&source), temp.path())
            .expect("paste copy");
        let copied = finished_summary(Ok(copied));

        assert_eq!(
            copied.destination_paths,
            vec![temp.path().join("file - Copy (2).txt")]
        );
        assert_eq!(fs::read(&source).unwrap(), b"data");
        assert_eq!(
            fs::read(temp.path().join("file - Copy (2).txt")).unwrap(),
            b"data"
        );
    }

    #[test]
    fn paste_copy_directory_in_same_directory_uses_copy_name() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        fs::create_dir(&source).expect("create source directory");
        fs::write(source.join("nested.txt"), b"data").expect("create nested file");

        let copied = copy_paths_to_directory_for_paste(std::slice::from_ref(&source), temp.path())
            .expect("paste copy");
        let copied = finished_summary(Ok(copied));

        let copied_folder = temp.path().join("folder - Copy");
        assert_eq!(copied.destination_paths, vec![copied_folder.clone()]);
        assert_eq!(fs::read(copied_folder.join("nested.txt")).unwrap(), b"data");
    }

    #[test]
    fn folder_merge_copies_non_conflicting_nested_files() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        let destination = temp.path().join("destination");
        fs::create_dir_all(source.join("nested")).expect("create source nested");
        fs::create_dir_all(destination.join("folder")).expect("create destination folder");
        fs::write(source.join("nested").join("file.txt"), b"data").expect("create source file");

        let summary = finished_summary(copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));

        assert_eq!(
            fs::read(destination.join("folder").join("nested").join("file.txt")).unwrap(),
            b"data"
        );
        assert_eq!(summary.destination_paths, vec![destination.join("folder")]);
    }

    #[test]
    fn folder_merge_includes_nested_file_conflicts_in_global_choice() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        let destination = temp.path().join("destination");
        let destination_folder = destination.join("folder");
        fs::create_dir_all(source.join("nested")).expect("create source nested");
        fs::create_dir_all(destination_folder.join("nested")).expect("create destination nested");
        fs::write(source.join("nested").join("file.txt"), b"new").expect("create source file");
        fs::write(destination_folder.join("nested").join("file.txt"), b"old")
            .expect("create destination file");

        let conflicts = conflict_batch(copy_paths_to_directory(
            std::slice::from_ref(&source),
            &destination,
        ));
        assert_eq!(conflicts.len(), 1);

        resolve_file_conflicts(conflicts, ConflictChoice::Replace).expect("replace nested");

        assert_eq!(
            fs::read(destination_folder.join("nested").join("file.txt")).unwrap(),
            b"new"
        );
    }

    #[test]
    fn paste_copy_directory_into_descendant_fails() {
        let temp = TempDir::new();
        let source = temp.path().join("folder");
        let descendant = source.join("child");
        fs::create_dir_all(&descendant).expect("create descendant directory");

        let error = copy_paths_to_directory_for_paste(std::slice::from_ref(&source), &descendant)
            .expect_err("descendant copy should fail");

        assert!(error.contains("Cannot copy folder into itself"));
    }

    #[test]
    fn permanent_delete_removes_files_and_directories() {
        let temp = TempDir::new();
        let file = temp.path().join("file.txt");
        let folder = temp.path().join("folder");
        fs::write(&file, b"data").expect("create file");
        fs::create_dir(&folder).expect("create folder");
        fs::write(folder.join("nested.txt"), b"data").expect("create nested file");

        remove_paths_permanently(&[file.clone(), folder.clone()]).expect("delete paths");

        assert!(!file.exists());
        assert!(!folder.exists());
    }

    #[test]
    fn trash_delete_missing_selection_errors() {
        assert!(trash_paths(&[]).is_err());
    }
}
