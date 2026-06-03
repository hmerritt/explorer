use std::{
    fs::{self, Metadata},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Eq, PartialEq)]
pub(super) enum FolderSizeError {
    Cancelled,
    Unavailable,
}

pub(super) fn calculate_folder_size(
    path: &Path,
    cancel: &AtomicBool,
) -> Result<u64, FolderSizeError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(FolderSizeError::Cancelled);
    }

    let metadata = fs::symlink_metadata(path).map_err(|_| FolderSizeError::Unavailable)?;
    if metadata_is_directory_link(&metadata) {
        return Ok(metadata.len());
    }

    if metadata.is_dir() {
        let mut size = 0;
        let entries = fs::read_dir(path).map_err(|_| FolderSizeError::Unavailable)?;
        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                return Err(FolderSizeError::Cancelled);
            }

            let entry = entry.map_err(|_| FolderSizeError::Unavailable)?;
            size += calculate_folder_size(&entry.path(), cancel)?;
        }
        Ok(size)
    } else {
        Ok(metadata.len())
    }
}

#[cfg(not(target_os = "windows"))]
fn metadata_is_directory_link(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(target_os = "windows")]
fn metadata_is_directory_link(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;
    use std::path::PathBuf;

    #[test]
    fn folder_size_sums_nested_files() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        let nested = folder.join("nested");
        fs::create_dir(&folder).expect("create folder");
        fs::create_dir(&nested).expect("create nested folder");
        fs::write(folder.join("a.txt"), b"abc").expect("create first file");
        fs::write(nested.join("b.txt"), b"defg").expect("create second file");
        let cancel = AtomicBool::new(false);

        assert_eq!(calculate_folder_size(&folder, &cancel), Ok(7));
    }

    #[test]
    fn folder_size_stops_when_cancelled() {
        let temp = TempDir::new();
        let folder = temp.path().join("folder");
        fs::create_dir(&folder).expect("create folder");
        fs::write(folder.join("a.txt"), b"abc").expect("create file");
        let cancel = AtomicBool::new(true);

        assert_eq!(
            calculate_folder_size(&folder, &cancel),
            Err(FolderSizeError::Cancelled)
        );
    }

    #[test]
    fn folder_size_reports_unavailable_for_missing_path() {
        let cancel = AtomicBool::new(false);

        assert_eq!(
            calculate_folder_size(&PathBuf::from("missing-folder"), &cancel),
            Err(FolderSizeError::Unavailable)
        );
    }
}
