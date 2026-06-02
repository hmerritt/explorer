use std::{
    collections::HashSet,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
};

use crate::explorer::{entry::FileEntry, sorting::sort_entries};

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

pub(super) fn user_home_dir() -> Option<PathBuf> {
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
pub(super) fn user_desktop_dir(_home_dir: Option<&Path>) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::FOLDERID_Desktop;

    known_folder_path(&FOLDERID_Desktop)
}

#[cfg(not(target_os = "windows"))]
pub(super) fn user_desktop_dir(home_dir: Option<&Path>) -> Option<PathBuf> {
    home_dir.map(|home_dir| home_dir.join("Desktop"))
}

#[cfg(target_os = "windows")]
pub(super) fn user_downloads_dir(_home_dir: Option<&Path>) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::FOLDERID_Downloads;

    known_folder_path(&FOLDERID_Downloads)
}

#[cfg(not(target_os = "windows"))]
pub(super) fn user_downloads_dir(home_dir: Option<&Path>) -> Option<PathBuf> {
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

#[cfg(target_os = "windows")]
fn windows_drive_type_is_explorer_local(drive_type: u32) -> bool {
    matches!(drive_type, 2 | 3 | 5 | 6)
}

#[cfg(not(target_os = "windows"))]
pub(super) fn local_drive_roots() -> Vec<PathBuf> {
    vec![PathBuf::from("/")]
}

pub(super) fn load_entries(path: &Path) -> std::io::Result<Vec<FileEntry>> {
    let mut entries = fs::read_dir(path)?
        .filter_map(Result::ok)
        .filter_map(|entry| FileEntry::from_path(entry.path()))
        .collect::<Vec<_>>();

    sort_entries(&mut entries);
    Ok(entries)
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

pub(super) fn move_paths_to_directory(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<FileOperationOutcome, String> {
    prepare_file_operation(
        paths,
        destination,
        FileOperationKind::Move,
        CopyNamePolicy::Original,
    )
    .and_then(run_or_return_conflicts)
}

pub(super) fn copy_paths_to_directory(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<FileOperationOutcome, String> {
    prepare_file_operation(
        paths,
        destination,
        FileOperationKind::Copy,
        CopyNamePolicy::Original,
    )
    .and_then(run_or_return_conflicts)
}

pub(super) fn copy_paths_to_directory_for_paste(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<FileOperationOutcome, String> {
    prepare_file_operation(
        paths,
        destination,
        FileOperationKind::Copy,
        CopyNamePolicy::UseCopyNamesInSameDirectory,
    )
    .and_then(run_or_return_conflicts)
}

pub(super) fn resolve_file_conflicts(
    conflicts: FileConflictBatch,
    choice: ConflictChoice,
) -> Result<FileOperationSummary, String> {
    execute_file_operation(conflicts.job, choice)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum FileOperationOutcome {
    Finished(FileOperationSummary),
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

    pub(super) fn first_destination_name(&self) -> String {
        self.conflicts
            .first()
            .map(|conflict| path_display_name(&conflict.destination))
            .unwrap_or_else(|| "this file".to_owned())
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CopyNamePolicy {
    Original,
    UseCopyNamesInSameDirectory,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileOperationJob {
    kind: FileOperationKind,
    steps: Vec<FileOperationStep>,
    roots: Vec<FileOperationRoot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileOperationRoot {
    source: PathBuf,
    destination: PathBuf,
    source_is_dir: bool,
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
    RemoveEmptyDirectory(PathBuf),
}

fn run_or_return_conflicts(job: FileOperationJob) -> Result<FileOperationOutcome, String> {
    let conflicts = file_conflicts_for_job(&job);
    if conflicts.is_empty() {
        execute_file_operation(job, ConflictChoice::Replace).map(FileOperationOutcome::Finished)
    } else {
        Ok(FileOperationOutcome::Conflicts(FileConflictBatch {
            conflicts,
            job,
        }))
    }
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
                };
                return Err(format!(
                    "Cannot {operation} {} into itself.",
                    path_display_name(source)
                ));
            }
        }

        plan_path_operation(source, &planned_destination, kind, &mut steps)?;
        roots.push(FileOperationRoot {
            source: source.clone(),
            destination: planned_destination,
            source_is_dir: source.is_dir(),
        });
    }

    Ok(FileOperationJob { kind, steps, roots })
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
        }
    }

    Ok(())
}

fn file_conflicts_for_job(job: &FileOperationJob) -> Vec<FileConflict> {
    job.steps
        .iter()
        .filter_map(|step| match step {
            FileOperationStep::CopyFile {
                source,
                destination,
                conflict: true,
            }
            | FileOperationStep::MoveFile {
                source,
                destination,
                conflict: true,
            } => Some(FileConflict {
                source: source.clone(),
                destination: destination.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn execute_file_operation(
    job: FileOperationJob,
    conflict_choice: ConflictChoice,
) -> Result<FileOperationSummary, String> {
    let mut operated_destinations = HashSet::new();

    for step in &job.steps {
        match step {
            FileOperationStep::CreateDirectory(path) => {
                fs::create_dir(path).map_err(|error| format_path_error("create", path, error))?;
            }
            FileOperationStep::CopyFile {
                source,
                destination,
                conflict,
            } => {
                if *conflict && conflict_choice == ConflictChoice::Skip {
                    continue;
                }
                copy_source_file(source, destination)
                    .map_err(|error| format_path_error("copy", source, error))?;
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
                    copy_source_file(source, destination)
                        .map_err(|error| format_path_error("move", source, error))?;
                    remove_source(source)
                        .map_err(|error| format_path_error("remove", source, error))?;
                } else {
                    move_source_file(source, destination)
                        .map_err(|error| format_path_error("move", source, error))?;
                }
                operated_destinations.insert(destination.clone());
            }
            FileOperationStep::RemoveEmptyDirectory(path) => remove_empty_directory(path)
                .map_err(|error| format_path_error("remove", path, error))?,
        }
    }

    let mut summary = FileOperationSummary::default();
    for root in &job.roots {
        if root.source_is_dir {
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

    Ok(summary)
}

fn copy_source_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::copy(source, destination).map(|_| ())
}

fn move_source_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(error) if is_cross_device_error(&error) => {
            copy_source_file(source, destination)?;
            remove_source(source)
        }
        Err(error) => Err(error),
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
