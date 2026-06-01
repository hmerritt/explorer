use std::{
    collections::HashSet,
    ffi::OsStr,
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

fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

#[cfg(target_os = "windows")]
fn user_downloads_dir(_home_dir: Option<&Path>) -> Option<PathBuf> {
    use windows::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_Downloads, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath},
    };

    unsafe {
        let downloads =
            SHGetKnownFolderPath(&FOLDERID_Downloads, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let path = downloads.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(downloads.as_ptr().cast()));
        path
    }
}

#[cfg(not(target_os = "windows"))]
fn user_downloads_dir(home_dir: Option<&Path>) -> Option<PathBuf> {
    home_dir.map(|home_dir| home_dir.join("Downloads"))
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
) -> Result<Vec<PathBuf>, String> {
    let plans = file_operation_plans(paths, destination, PlannedOperation::Move)?;
    let mut moved_paths = Vec::new();

    for plan in plans {
        if plan.same_directory_move {
            continue;
        }

        match fs::rename(&plan.source, &plan.destination) {
            Ok(()) => {}
            Err(error) if is_cross_device_error(&error) => {
                copy_path_recursively(&plan.source, &plan.destination)
                    .map_err(|error| format_path_error("move", &plan.source, error))?;
                remove_source(&plan.source)
                    .map_err(|error| format_path_error("remove", &plan.source, error))?;
            }
            Err(error) => return Err(format_path_error("move", &plan.source, error)),
        }

        moved_paths.push(plan.destination);
    }

    Ok(moved_paths)
}

pub(super) fn copy_paths_to_directory(
    paths: &[PathBuf],
    destination: &Path,
) -> Result<Vec<PathBuf>, String> {
    let plans = file_operation_plans(paths, destination, PlannedOperation::Copy)?;
    let mut copied_paths = Vec::new();

    for plan in plans {
        copy_path_recursively(&plan.source, &plan.destination)
            .map_err(|error| format_path_error("copy", &plan.source, error))?;
        copied_paths.push(plan.destination);
    }

    Ok(copied_paths)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlannedOperation {
    Move,
    Copy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileOperationPlan {
    source: PathBuf,
    destination: PathBuf,
    same_directory_move: bool,
}

fn file_operation_plans(
    paths: &[PathBuf],
    destination: &Path,
    operation: PlannedOperation,
) -> Result<Vec<FileOperationPlan>, String> {
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
    let mut destination_names = HashSet::new();
    let mut plans = Vec::with_capacity(paths.len());

    for source in paths {
        if !source.exists() {
            return Err(format!("Could not find {}.", path_display_name(source)));
        }

        let file_name = source
            .file_name()
            .ok_or_else(|| format!("{} cannot be moved or copied.", path_display_name(source)))?;

        if !destination_names.insert(file_name.to_os_string()) {
            return Err(format!(
                "Multiple selected items are named {}.",
                file_name.to_string_lossy()
            ));
        }

        let source_parent = source
            .parent()
            .ok_or_else(|| format!("{} cannot be moved or copied.", path_display_name(source)))?;
        let source_parent_canonical = canonicalize_for_operation(source_parent)?;
        let same_directory_move =
            operation == PlannedOperation::Move && source_parent_canonical == destination_canonical;
        let planned_destination = destination.join(file_name);

        if !same_directory_move && planned_destination.exists() {
            return Err(format!(
                "{} already contains {}.",
                path_display_name(destination),
                file_name.to_string_lossy()
            ));
        }

        if operation == PlannedOperation::Move && source.is_dir() {
            let source_canonical = canonicalize_for_operation(source)?;
            if destination_canonical.starts_with(source_canonical) {
                return Err(format!(
                    "Cannot move {} into itself.",
                    path_display_name(source)
                ));
            }
        }

        plans.push(FileOperationPlan {
            source: source.clone(),
            destination: planned_destination,
            same_directory_move,
        });
    }

    Ok(plans)
}

fn copy_path_recursively(source: &Path, destination: &Path) -> std::io::Result<()> {
    let metadata = fs::metadata(source)?;

    if metadata.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_path_recursively(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        fs::copy(source, destination)?;
    }

    Ok(())
}

fn remove_source(source: &Path) -> std::io::Result<()> {
    if source.is_dir() {
        fs::remove_dir_all(source)
    } else {
        fs::remove_file(source)
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

    #[test]
    fn move_file_to_directory() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::write(&source, b"data").expect("create file");
        fs::create_dir(&destination).expect("create destination");

        let moved = move_paths_to_directory(std::slice::from_ref(&source), &destination)
            .expect("move file");

        assert!(!source.exists());
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"data");
        assert_eq!(moved, vec![destination.join("file.txt")]);
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

        assert!(!source.exists());
        assert_eq!(
            fs::read(destination.join("folder").join("nested").join("file.txt")).unwrap(),
            b"data"
        );
        assert_eq!(moved, vec![destination.join("folder")]);
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

        assert_eq!(fs::read(&source).unwrap(), b"data");
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"data");
        assert_eq!(copied, vec![destination.join("file.txt")]);
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

        assert!(source.exists());
        assert_eq!(
            fs::read(destination.join("folder").join("nested").join("file.txt")).unwrap(),
            b"data"
        );
        assert_eq!(copied, vec![destination.join("folder")]);
    }

    #[test]
    fn destination_conflict_fails_without_overwrite() {
        let temp = TempDir::new();
        let source = temp.path().join("file.txt");
        let destination = temp.path().join("destination");
        fs::write(&source, b"source").expect("create source");
        fs::create_dir(&destination).expect("create destination");
        fs::write(destination.join("file.txt"), b"existing").expect("create existing");

        let error = move_paths_to_directory(std::slice::from_ref(&source), &destination)
            .expect_err("conflict should fail");

        assert!(error.contains("already contains file.txt"));
        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"existing");
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

        assert!(moved.is_empty());
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
}
