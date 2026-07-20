use std::{fs, io, path::Path};

use crate::settings::config_dir;

const CACHE_DIRECTORY_NAME: &str = "cache";

pub(crate) fn initialize() {
    let Some(config_dir) = config_dir() else {
        return;
    };
    let cache_dir = config_dir.join(CACHE_DIRECTORY_NAME);
    if let Err(error) = create_cache_root(&cache_dir) {
        eprintln!(
            "Unable to initialize Explorer cache directory {}: {error}",
            cache_dir.display()
        );
    }
}

fn create_cache_root(path: &Path) -> io::Result<bool> {
    create_cache_root_with(path, mark_hidden)
}

fn create_cache_root_with(
    path: &Path,
    mark_hidden: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    match fs::create_dir(path) {
        Ok(()) => {
            mark_hidden(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "windows")]
fn mark_hidden(path: &Path) -> io::Result<()> {
    use std::os::windows::{ffi::OsStrExt, fs::MetadataExt};
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_HIDDEN, FILE_FLAGS_AND_ATTRIBUTES, SetFileAttributesW,
    };
    use windows::core::PCWSTR;

    let attributes = fs::metadata(path)?.file_attributes() | FILE_ATTRIBUTE_HIDDEN.0;
    let wide_path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    unsafe {
        SetFileAttributesW(
            PCWSTR(wide_path.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(attributes),
        )
        .map_err(io::Error::other)
    }
}

#[cfg(target_os = "macos")]
fn mark_hidden(path: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::macos::fs::MetadataExt, os::unix::ffi::OsStrExt};

    const UF_HIDDEN: u32 = 0x0000_8000;

    let flags = fs::symlink_metadata(path)?.st_flags() | UF_HIDDEN;
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a null byte"))?;
    if unsafe { libc::chflags(path.as_ptr(), flags) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn mark_hidden(_: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::RefCell,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn newly_created_cache_root_is_marked_once() {
        let temp = TempDir::new("new-cache-root");
        let cache_dir = temp.path().join(CACHE_DIRECTORY_NAME);
        let marked = RefCell::new(Vec::new());

        let created = create_cache_root_with(&cache_dir, |path| {
            marked.borrow_mut().push(path.to_path_buf());
            Ok(())
        })
        .unwrap();

        assert!(created);
        assert!(cache_dir.is_dir());
        assert_eq!(*marked.borrow(), vec![cache_dir]);
    }

    #[test]
    fn existing_cache_root_is_not_marked() {
        let temp = TempDir::new("existing-cache-root");
        let cache_dir = temp.path().join(CACHE_DIRECTORY_NAME);
        fs::create_dir(&cache_dir).unwrap();
        let marker_called = RefCell::new(false);

        let created = create_cache_root_with(&cache_dir, |_| {
            *marker_called.borrow_mut() = true;
            Ok(())
        })
        .unwrap();

        assert!(!created);
        assert!(!*marker_called.borrow());
    }

    #[test]
    fn marker_failure_preserves_the_new_cache_root_without_retrying() {
        let temp = TempDir::new("marker-failure");
        let cache_dir = temp.path().join(CACHE_DIRECTORY_NAME);

        let error = create_cache_root_with(&cache_dir, |_| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "cannot mark hidden",
            ))
        })
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(cache_dir.is_dir());
        assert!(
            !create_cache_root_with(&cache_dir, |_| panic!("existing root must not be marked"))
                .unwrap()
        );
        fs::create_dir(cache_dir.join("native-icons-v1")).unwrap();
    }

    #[test]
    fn only_the_cache_root_is_passed_to_the_marker() {
        let temp = TempDir::new("top-level-only");
        let cache_dir = temp.path().join(CACHE_DIRECTORY_NAME);
        let child_dir = cache_dir.join("image-thumbnails-v2");
        let marked = RefCell::new(Vec::new());

        create_cache_root_with(&cache_dir, |path| {
            marked.borrow_mut().push(path.to_path_buf());
            Ok(())
        })
        .unwrap();
        fs::create_dir(&child_dir).unwrap();

        assert_eq!(*marked.borrow(), vec![cache_dir]);
        assert!(child_dir.is_dir());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_marks_only_new_cache_root_hidden() {
        use std::os::windows::fs::MetadataExt;
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN;

        let temp = TempDir::new("windows-hidden-cache-root");
        let cache_dir = temp.path().join(CACHE_DIRECTORY_NAME);
        let child_dir = cache_dir.join("native-icons-v1");

        assert!(create_cache_root(&cache_dir).unwrap());
        fs::create_dir(&child_dir).unwrap();

        assert_ne!(
            fs::metadata(&cache_dir).unwrap().file_attributes() & FILE_ATTRIBUTE_HIDDEN.0,
            0
        );
        assert_eq!(
            fs::metadata(&child_dir).unwrap().file_attributes() & FILE_ATTRIBUTE_HIDDEN.0,
            0
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_marks_only_new_cache_root_hidden() {
        use std::os::macos::fs::MetadataExt;

        const UF_HIDDEN: u32 = 0x0000_8000;

        let temp = TempDir::new("macos-hidden-cache-root");
        let cache_dir = temp.path().join(CACHE_DIRECTORY_NAME);
        let child_dir = cache_dir.join("native-icons-v1");

        assert!(create_cache_root(&cache_dir).unwrap());
        fs::create_dir(&child_dir).unwrap();

        assert_ne!(
            fs::symlink_metadata(&cache_dir).unwrap().st_flags() & UF_HIDDEN,
            0
        );
        assert_eq!(
            fs::symlink_metadata(&child_dir).unwrap().st_flags() & UF_HIDDEN,
            0
        );
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "explorer-cache-directory-{name}-{}-{nanos}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
