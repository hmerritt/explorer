use std::{ffi::OsStr, fs, path::PathBuf, time::SystemTime};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEntry {
    pub(super) path: PathBuf,
    pub(super) name: String,
    pub(super) is_dir: bool,
    pub(super) modified: Option<SystemTime>,
    pub(super) size: Option<u64>,
}

impl FileEntry {
    pub(super) fn from_path(path: PathBuf) -> Option<Self> {
        let metadata = fs::metadata(&path).ok()?;
        let name = path.file_name()?.to_string_lossy().into_owned();
        let is_dir = metadata.is_dir();

        Some(Self {
            path,
            name,
            is_dir,
            modified: metadata.modified().ok(),
            size: (!is_dir).then_some(metadata.len()),
        })
    }

    #[cfg(test)]
    pub(super) fn test(
        name: &str,
        is_dir: bool,
        size: Option<u64>,
        modified: Option<SystemTime>,
    ) -> Self {
        Self {
            path: PathBuf::from(name),
            name: name.to_owned(),
            is_dir,
            modified,
            size,
        }
    }

    pub(super) fn type_label(&self) -> String {
        if self.is_dir {
            return "File folder".to_owned();
        }

        let Some(extension) = self.path.extension().and_then(OsStr::to_str) else {
            return "File".to_owned();
        };

        format!("{} File", extension.to_uppercase())
    }
}
