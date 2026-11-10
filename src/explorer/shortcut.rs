use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use super::{
    address_bar::resolve_shortcut_path,
    explorer_fs::{ExplorerFs, ExplorerLocation},
    filesystem::create_link,
    rename::validate_rename_text,
    url_shortcut::{
        URL_SHORTCUT_EXTENSION, UrlShortcut, is_reserved_shortcut_name, sanitize_shortcut_name,
        supported_url,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ShortcutTarget {
    Filesystem(PathBuf),
    Url(UrlShortcut),
}

pub(super) fn is_shortcut_directory(path: &Path) -> bool {
    !super::trash::is_root(path)
        && !super::trash::is_item(path)
        && matches!(ExplorerFs::new().classify(path), ExplorerLocation::Local(_))
}

impl ShortcutTarget {
    pub(super) fn parse(input: &str, directory: &Path) -> Result<Self, String> {
        let input = input.trim();
        if let Some((scheme, address)) = input.split_once("://")
            && scheme.starts_with(|ch: char| ch.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
        {
            return supported_url(input)
                .filter(|url| {
                    matches!(url.scheme(), "http" | "https" | "ftp")
                        && !address.starts_with(['/', '\\'])
                })
                .map(|url| Self::Url(UrlShortcut { url, title: None }))
                .ok_or_else(|| "Enter a valid HTTP, HTTPS, or FTP address.".to_owned());
        }

        Self::from_path(resolve_shortcut_path(input, directory, |name| {
            env::var_os(name)
        })?)
    }

    pub(super) fn from_path(path: PathBuf) -> Result<Self, String> {
        target_metadata(&path)?;
        Ok(Self::Filesystem(path))
    }

    pub(super) fn suggested_name(&self) -> String {
        match self {
            Self::Filesystem(path) => path
                .file_name()
                .map(|name| sanitize_shortcut_name(&name.to_string_lossy()))
                .unwrap_or_else(|| "New Shortcut".into()),
            Self::Url(_) => "New Internet Shortcut".into(),
        }
    }

    pub(super) fn create_in(&self, directory: &Path, name: &str) -> Result<PathBuf, String> {
        validate_shortcut_name(name)?;
        if !is_shortcut_directory(directory) {
            return Err("Shortcuts can only be saved in filesystem folders.".into());
        }

        match self {
            Self::Filesystem(target) => {
                let metadata = target_metadata(target)?;
                create_filesystem_shortcut(directory, name, target, metadata.is_dir())
                    .map_err(|error| format!("Could not create shortcut: {error}"))
            }
            Self::Url(shortcut) => {
                let name = without_shortcut_extension(name, URL_SHORTCUT_EXTENSION);
                validate_shortcut_name(name)?;
                shortcut.create_named_in(directory, name)
            }
        }
    }
}

fn target_metadata(path: &Path) -> Result<fs::Metadata, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Could not find or access the shortcut destination: {error}"))?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err("The shortcut destination must be a file or folder.".into());
    }
    Ok(metadata)
}

pub(super) fn validate_shortcut_name(name: &str) -> Result<(), String> {
    validate_rename_text(name)?;
    if name.ends_with(['.', ' '])
        || name.chars().any(char::is_control)
        || is_reserved_shortcut_name(name)
    {
        return Err("The shortcut name is not valid.".into());
    }
    Ok(())
}

fn without_shortcut_extension<'a>(name: &'a str, extension: &str) -> &'a str {
    let path = Path::new(name);
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|actual| actual.eq_ignore_ascii_case(extension))
    {
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(name)
    } else {
        name
    }
}

fn create_filesystem_shortcut(
    directory: &Path,
    name: &str,
    target: &Path,
    target_is_directory: bool,
) -> io::Result<PathBuf> {
    #[cfg(target_os = "windows")]
    let (stem, extension) = {
        let _ = target_is_directory;
        let stem = without_shortcut_extension(name, "lnk");
        validate_shortcut_name(stem).map_err(io::Error::other)?;
        (stem, Some("lnk"))
    };
    #[cfg(unix)]
    let (stem, extension) = if target_is_directory {
        (name, None)
    } else {
        let path = Path::new(name);
        (
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or(name),
            path.extension().and_then(|extension| extension.to_str()),
        )
    };

    for index in 1..=usize::MAX {
        let mut filename = stem.to_owned();
        if index > 1 {
            filename.push_str(&format!(" ({index})"));
        }
        if let Some(extension) = extension {
            filename.push('.');
            filename.push_str(extension);
        }
        let path = directory.join(filename);
        match write_filesystem_shortcut(target, &path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other("No available shortcut filename."))
}

#[cfg(unix)]
fn write_filesystem_shortcut(target: &Path, path: &Path) -> io::Result<()> {
    create_link(target, path)
}

#[cfg(target_os = "windows")]
fn write_filesystem_shortcut(target: &Path, path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path).is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    // IPersistFile::Save can replace an existing file. Save beside the final path so
    // the link's relative destination is computed correctly, then publish exclusively.
    let temporary = tempfile::Builder::new()
        .prefix(".explorer-shortcut-")
        .suffix(".lnk")
        .tempfile_in(path.parent().expect("shortcut has a parent directory"))?
        .into_temp_path();
    create_link(target, &temporary)?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_filesystem_destinations_relative_to_the_origin() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report with spaces.txt");
        fs::write(&path, b"report").unwrap();
        assert_eq!(
            ShortcutTarget::parse(" \"report with spaces.txt\" ", directory.path()).unwrap(),
            ShortcutTarget::Filesystem(path.clone())
        );
        for input in [
            "~/report with spaces.txt",
            "%USERPROFILE%/report with spaces.txt",
        ] {
            assert_eq!(
                resolve_shortcut_path(input, directory.path(), |key| {
                    (key == "USERPROFILE").then(|| directory.path().as_os_str().to_owned())
                })
                .unwrap(),
                path
            );
        }
        assert!(ShortcutTarget::parse("missing.txt", directory.path()).is_err());
        assert!(ShortcutTarget::parse("  ", directory.path()).is_err());
    }

    #[test]
    fn accepts_only_supported_url_destinations() {
        for input in [
            "https://example.com/a?q=one&b=two#part",
            "http://example.com",
            "FTP://example.com/file",
        ] {
            let target = ShortcutTarget::parse(input, Path::new(".")).unwrap();
            assert!(matches!(target, ShortcutTarget::Url(_)));
            assert_eq!(target.suggested_name(), "New Internet Shortcut");
        }
        for input in [
            "sftp://example.com",
            "file:///tmp/file",
            "https://",
            "https:///example.com",
            "https://example.com/has a space",
        ] {
            assert!(
                ShortcutTarget::parse(input, Path::new(".")).is_err(),
                "{input}"
            );
        }
    }

    #[test]
    fn validates_the_entered_name() {
        for name in [
            "",
            " ",
            ".",
            "..",
            "dir/file",
            "file\\name",
            "file\nname",
            "name.",
            "name ",
            "NUL.txt",
            "LPT²",
        ] {
            assert!(validate_shortcut_name(name).is_err(), "{name:?}");
        }
        assert!(validate_shortcut_name("Résumé 2026.txt").is_ok());
    }

    #[test]
    fn creates_native_file_and_folder_shortcuts_without_replacing_entries() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("Report.txt");
        let folder = directory.path().join("Projects");
        fs::write(&file, b"original contents").unwrap();
        fs::create_dir(&folder).unwrap();

        for (target, name) in [(&file, "Report.txt"), (&folder, "Projects")] {
            let shortcut = ShortcutTarget::from_path(target.clone()).unwrap();
            assert_eq!(shortcut.suggested_name(), name);
            let first = shortcut.create_in(directory.path(), name).unwrap();
            let second = shortcut.create_in(directory.path(), name).unwrap();
            assert_ne!(first, second);
            for path in [first, second] {
                #[cfg(target_os = "windows")]
                let resolved = super::super::entry::shell_shortcut_target(&path).unwrap();
                #[cfg(unix)]
                let resolved = fs::read_link(path).unwrap();
                assert_eq!(
                    fs::canonicalize(resolved).unwrap(),
                    fs::canonicalize(target).unwrap()
                );
            }
        }
        assert_eq!(fs::read(file).unwrap(), b"original contents");
    }

    #[test]
    fn writes_a_named_url_shortcut_with_its_native_extension() {
        let directory = tempfile::tempdir().unwrap();
        let shortcut = ShortcutTarget::parse("https://example.com/docs", directory.path()).unwrap();
        let name = format!("Reference.{URL_SHORTCUT_EXTENSION}");
        let occupied = directory.path().join(&name);
        fs::write(&occupied, b"existing").unwrap();
        let created = shortcut.create_in(directory.path(), &name).unwrap();
        assert_eq!(
            created,
            directory
                .path()
                .join(format!("Reference (2).{URL_SHORTCUT_EXTENSION}"))
        );
        assert_eq!(fs::read(occupied).unwrap(), b"existing");
        assert!(
            fs::read_to_string(created)
                .unwrap()
                .contains("https://example.com/docs")
        );
    }

    #[test]
    fn failed_saves_leave_no_partial_shortcuts() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.txt");
        fs::write(&target, b"target").unwrap();
        let shortcut = ShortcutTarget::from_path(target.clone()).unwrap();
        assert!(
            shortcut
                .create_in(directory.path(), &"a".repeat(300))
                .is_err()
        );
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        fs::remove_file(target).unwrap();
        assert!(shortcut.create_in(directory.path(), "Link").is_err());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
