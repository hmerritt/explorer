use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::explorer::{
    entry::FileEntry,
    filesystem::{EntryVisibility, should_hide_entry},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ExplorerLocation {
    Local(PathBuf),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(super) enum ExplorerRefreshDriver {
    Notify,
    Poll,
}

#[allow(dead_code)]
pub(super) struct ExplorerFs;

impl ExplorerFs {
    pub(super) fn new() -> Self {
        Self
    }

    pub(super) fn classify(&self, path: &Path) -> ExplorerLocation {
        ExplorerLocation::Local(path.to_path_buf())
    }

    pub(super) fn can_mutate(&self, path: &Path) -> bool {
        match self.classify(path) {
            ExplorerLocation::Local(_) => true,
        }
    }

    pub(super) fn read_only_error(&self) -> String {
        "This location is read-only.".to_owned()
    }

    pub(super) fn exists(&self, path: &Path) -> Result<bool, String> {
        match self.classify(path) {
            ExplorerLocation::Local(_) => Ok(path.exists()),
        }
    }

    pub(super) fn is_dir(&self, path: &Path) -> Result<bool, String> {
        match self.classify(path) {
            ExplorerLocation::Local(_) => Ok(path.is_dir()),
        }
    }

    #[allow(dead_code)]
    pub(super) fn list_dir(
        &self,
        path: &Path,
        visibility: EntryVisibility,
    ) -> io::Result<Vec<FileEntry>> {
        match self.classify(path) {
            ExplorerLocation::Local(_) => list_local_dir(path, visibility),
        }
    }

    pub(super) fn create_dir(&self, path: &Path) -> Result<(), String> {
        if !self.can_mutate(path) {
            return Err(self.read_only_error());
        }
        match self.classify(path) {
            ExplorerLocation::Local(_) => {
                fs::create_dir(path).map_err(|error| format!("Could not create folder: {error}"))
            }
        }
    }

    pub(super) fn create_empty_file(&self, path: &Path) -> Result<(), String> {
        self.write_file(path, &[])
    }

    pub(super) fn write_file(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        if !self.can_mutate(path) {
            return Err(self.read_only_error());
        }
        match self.classify(path) {
            ExplorerLocation::Local(_) => write_new_file(path, bytes)
                .map_err(|error| format!("Could not create {}: {error}", display_name(path))),
        }
    }

    pub(super) fn refresh_driver(&self, path: &Path) -> ExplorerRefreshDriver {
        match self.classify(path) {
            ExplorerLocation::Local(_) => ExplorerRefreshDriver::Notify,
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
}
