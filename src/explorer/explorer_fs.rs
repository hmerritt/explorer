use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::explorer::{
    archive_fs,
    entry::FileEntry,
    filesystem::{EntryVisibility, should_hide_entry},
    portable_devices,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ExplorerLocation {
    Remote(super::remote_fs::RemoteLocation),
    Local(PathBuf),
    Portable(PathBuf),
    Archive(PathBuf),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(super) enum ExplorerRefreshDriver {
    Notify,
    Poll,
    Events,
}

#[allow(dead_code)]
pub(super) struct ExplorerFs;

impl ExplorerFs {
    pub(super) fn new() -> Self {
        Self
    }

    pub(super) fn classify(&self, path: &Path) -> ExplorerLocation {
        if let Some(location) = super::remote_fs::RemoteLocation::from_provider(path) { return ExplorerLocation::Remote(location); }
        if archive_fs::is_archive_path(path) {
            ExplorerLocation::Archive(path.to_path_buf())
        } else if portable_devices::is_portable_path(path) {
            ExplorerLocation::Portable(path.to_path_buf())
        } else {
            ExplorerLocation::Local(path.to_path_buf())
        }
    }

    pub(super) fn can_mutate(&self, path: &Path) -> bool {
        match self.classify(path) {
            ExplorerLocation::Remote(_) => true,
            ExplorerLocation::Local(_) => true,
            ExplorerLocation::Portable(_) => {
                portable_devices::capabilities(path).can_mutate()
                    || path
                        .parent()
                        .is_some_and(|parent| portable_devices::capabilities(parent).can_mutate())
            }
            ExplorerLocation::Archive(_) => false,
        }
    }

    pub(super) fn read_only_error(&self) -> String {
        "This location is read-only.".to_owned()
    }

    pub(super) fn exists(&self, path: &Path) -> Result<bool, String> {
        match self.classify(path) {
            ExplorerLocation::Remote(_) => super::remote_fs::exists(path),
            ExplorerLocation::Local(_) => Ok(path.exists()),
            ExplorerLocation::Portable(_) => Ok(portable_devices::exists(path)),
            ExplorerLocation::Archive(_) => {
                Ok(archive_fs::is_dir(path) || archive_fs::is_file(path))
            }
        }
    }

    pub(super) fn is_dir(&self, path: &Path) -> Result<bool, String> {
        match self.classify(path) {
            ExplorerLocation::Remote(_) => super::remote_fs::cached_is_dir(path),
            ExplorerLocation::Local(_) => Ok(path.is_dir()),
            ExplorerLocation::Portable(_) => Ok(portable_devices::is_dir(path)),
            ExplorerLocation::Archive(_) => Ok(archive_fs::is_dir(path)),
        }
    }

    #[allow(dead_code)]
    pub(super) fn list_dir(
        &self,
        path: &Path,
        visibility: EntryVisibility,
    ) -> io::Result<Vec<FileEntry>> {
        match self.classify(path) {
            ExplorerLocation::Remote(_) => super::remote_fs::list_dir(path, visibility),
            ExplorerLocation::Local(_) => list_local_dir(path, visibility),
            ExplorerLocation::Portable(_) => portable_devices::list_dir(path),
            ExplorerLocation::Archive(_) => archive_fs::list_dir(path),
        }
    }

    pub(super) fn create_dir(&self, path: &Path) -> Result<(), String> {
        let _cache_invalidation = crate::explorer::remote_directory_cache::DirectoryMutation::new([path.to_path_buf()]);
        if !self.can_mutate(path) {
            return Err(self.read_only_error());
        }
        match self.classify(path) {
            ExplorerLocation::Remote(_) => super::remote_fs::create_dir(path),
            ExplorerLocation::Local(_) => {
                fs::create_dir(path).map_err(|error| format!("Could not create folder: {error}"))
            }
            ExplorerLocation::Portable(_) => {
                let parent = path
                    .parent()
                    .ok_or_else(|| "Portable-device parent was not found.".to_owned())?;
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| "Portable-device folder name is invalid.".to_owned())?;
                portable_devices::create_folder(parent, name).map(|_| ())
            }
            ExplorerLocation::Archive(_) => Err(self.read_only_error()),
        }
    }

    pub(super) fn create_empty_file(&self, path: &Path) -> Result<(), String> {
        self.write_file(path, &[])
    }

    pub(super) fn write_file(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        let _cache_invalidation = crate::explorer::remote_directory_cache::DirectoryMutation::new([path.to_path_buf()]);
        if !self.can_mutate(path) {
            return Err(self.read_only_error());
        }
        match self.classify(path) {
            ExplorerLocation::Remote(_) => super::remote_fs::write_new_file(path, bytes),
            ExplorerLocation::Local(_) => write_new_file(path, bytes)
                .map_err(|error| format!("Could not create {}: {error}", display_name(path))),
            ExplorerLocation::Portable(_) => {
                let parent = path
                    .parent()
                    .ok_or_else(|| "Portable-device parent was not found.".to_owned())?;
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| "Portable-device file name is invalid.".to_owned())?;
                portable_devices::write_file(parent, name, bytes).map(|_| ())
            }
            ExplorerLocation::Archive(_) => Err(self.read_only_error()),
        }
    }

    pub(super) fn refresh_driver(&self, path: &Path) -> ExplorerRefreshDriver {
        match self.classify(path) {
            ExplorerLocation::Remote(_) => ExplorerRefreshDriver::Poll,
            ExplorerLocation::Local(_) => ExplorerRefreshDriver::Notify,
            ExplorerLocation::Portable(_) => {
                if portable_devices::capabilities(path).supports_events {
                    ExplorerRefreshDriver::Events
                } else {
                    ExplorerRefreshDriver::Poll
                }
            }
            ExplorerLocation::Archive(_) => ExplorerRefreshDriver::Poll,
        }
    }
}

#[allow(dead_code)]
fn list_local_dir(path: &Path, visibility: EntryVisibility) -> io::Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        if should_hide_entry(&file_name, &path, visibility) {
            continue;
        }
        if let Some(entry) = FileEntry::from_path(path) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

fn write_new_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    if let Err(error) = file.write_all(bytes) {
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("item")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_paths_are_mutable_by_default() {
        let fs = ExplorerFs::new();

        assert_eq!(
            fs.classify(Path::new("/tmp/local")),
            ExplorerLocation::Local(PathBuf::from("/tmp/local"))
        );
        assert!(fs.can_mutate(Path::new("/tmp/local")));
        assert_eq!(
            fs.refresh_driver(Path::new("/tmp/local")),
            ExplorerRefreshDriver::Notify
        );
    }

    #[test]
    fn portable_paths_use_the_poll_refresh_driver() {
        let fs = ExplorerFs::new();
        let path = super::portable_devices::virtual_root().join("device-test");
        assert!(matches!(fs.classify(&path), ExplorerLocation::Portable(_)));
        assert_eq!(fs.refresh_driver(&path), ExplorerRefreshDriver::Poll);
    }
}
