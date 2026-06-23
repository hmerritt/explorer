use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::{
    explorer::{
        entry::FileEntry,
        filesystem::{EntryVisibility, should_hide_entry},
    },
    settings::RcloneSettings,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ExplorerLocation {
    Local(PathBuf),
    #[cfg(feature = "rclone")]
    RcloneMounted {
        local_path: PathBuf,
        remote_path: crate::explorer::rclone::RclonePath,
    },
    #[cfg(feature = "rclone")]
    RcloneTransfer(crate::explorer::rclone::RclonePath),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(super) enum ExplorerRefreshDriver {
    Notify,
    Poll,
}

#[allow(dead_code)]
pub(super) struct ExplorerFs<'a> {
    rclone_settings: &'a RcloneSettings,
}

impl<'a> ExplorerFs<'a> {
    pub(super) fn new(rclone_settings: &'a RcloneSettings) -> Self {
        Self { rclone_settings }
    }

    pub(super) fn classify(&self, path: &Path) -> ExplorerLocation {
        #[cfg(feature = "rclone")]
        {
            if let Some(remote_path) = crate::explorer::rclone::managed_mounted_path(path) {
                return ExplorerLocation::RcloneMounted {
                    local_path: path.to_path_buf(),
                    remote_path,
                };
            }
            if crate::explorer::rclone::is_transfer_path(path) {
                if let Some(remote_path) = crate::explorer::rclone::parse_virtual_path(path) {
                    return ExplorerLocation::RcloneTransfer(remote_path);
                }
            }
        }

        ExplorerLocation::Local(path.to_path_buf())
    }

    pub(super) fn can_mutate(&self, path: &Path) -> bool {
        match self.classify(path) {
            ExplorerLocation::Local(_) => true,
            #[cfg(feature = "rclone")]
            ExplorerLocation::RcloneMounted { .. } | ExplorerLocation::RcloneTransfer(_) => {
                self.rclone_settings.enabled && !self.rclone_settings.mount.read_only
            }
        }
    }

    pub(super) fn read_only_error(&self) -> String {
        #[cfg(feature = "rclone")]
        {
            if !self.rclone_settings.enabled {
                crate::explorer::rclone::disabled_error()
            } else {
                "This rclone remote is read-only.".to_owned()
            }
        }
        #[cfg(not(feature = "rclone"))]
        {
            "This location is read-only.".to_owned()
        }
    }

    pub(super) fn exists(&self, path: &Path) -> Result<bool, String> {
        match self.classify(path) {
            ExplorerLocation::Local(_) => Ok(path.exists()),
            #[cfg(feature = "rclone")]
            ExplorerLocation::RcloneMounted { .. } => Ok(path.exists()),
            #[cfg(feature = "rclone")]
            ExplorerLocation::RcloneTransfer(_) => {
                crate::explorer::rclone::transfer_path_exists(path, self.rclone_settings)
            }
        }
    }

    pub(super) fn is_dir(&self, path: &Path) -> Result<bool, String> {
        match self.classify(path) {
            ExplorerLocation::Local(_) => Ok(path.is_dir()),
            #[cfg(feature = "rclone")]
            ExplorerLocation::RcloneMounted { .. } => Ok(path.is_dir()),
            #[cfg(feature = "rclone")]
            ExplorerLocation::RcloneTransfer(_) => {
                crate::explorer::rclone::transfer_path_is_dir(path, self.rclone_settings)
            }
        }
    }

    #[allow(dead_code)]
    pub(super) fn list_dir(
        &self,
        path: &Path,
        visibility: EntryVisibility,
    ) -> io::Result<Vec<FileEntry>> {
        match self.classify(path) {
            #[cfg(feature = "rclone")]
            ExplorerLocation::RcloneTransfer(_) => crate::explorer::rclone::load_transfer_entries(
                path,
                visibility,
                self.rclone_settings,
            ),
            ExplorerLocation::Local(_) => list_local_dir(path, visibility),
            #[cfg(feature = "rclone")]
            ExplorerLocation::RcloneMounted { .. } => list_local_dir(path, visibility),
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
            #[cfg(feature = "rclone")]
            ExplorerLocation::RcloneMounted { .. } => {
                fs::create_dir(path).map_err(|error| format!("Could not create folder: {error}"))
            }
            #[cfg(feature = "rclone")]
            ExplorerLocation::RcloneTransfer(_) => {
                crate::explorer::rclone::create_transfer_folder(path, self.rclone_settings)
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
            #[cfg(feature = "rclone")]
            ExplorerLocation::RcloneMounted { .. } => write_new_file(path, bytes)
                .map_err(|error| format!("Could not create {}: {error}", display_name(path))),
            #[cfg(feature = "rclone")]
            ExplorerLocation::RcloneTransfer(_) => {
                crate::explorer::rclone::create_transfer_file(path, bytes, self.rclone_settings)
            }
        }
    }

    pub(super) fn refresh_driver(&self, path: &Path) -> ExplorerRefreshDriver {
        match self.classify(path) {
            #[cfg(feature = "rclone")]
            ExplorerLocation::RcloneTransfer(_) => ExplorerRefreshDriver::Poll,
            ExplorerLocation::Local(_) => ExplorerRefreshDriver::Notify,
            #[cfg(feature = "rclone")]
            ExplorerLocation::RcloneMounted { .. } => ExplorerRefreshDriver::Notify,
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
        let settings = RcloneSettings::default();
        let fs = ExplorerFs::new(&settings);

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

    #[cfg(feature = "rclone")]
    #[test]
    fn transfer_paths_use_rclone_mutability_and_polling() {
        let mut settings = RcloneSettings::default();
        let path = crate::explorer::rclone::virtual_root_for_remote("gdrive");
        let fs = ExplorerFs::new(&settings);

        assert!(matches!(
            fs.classify(&path),
            ExplorerLocation::RcloneTransfer(_)
        ));
        assert!(fs.can_mutate(&path));
        assert_eq!(fs.refresh_driver(&path), ExplorerRefreshDriver::Poll);

        settings.mount.read_only = true;
        let fs = ExplorerFs::new(&settings);
        assert!(!fs.can_mutate(&path));
    }
}
