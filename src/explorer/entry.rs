use std::{
    ffi::OsStr,
    fs::{self, Metadata},
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEntry {
    pub(super) path: PathBuf,
    pub(super) name: String,
    pub(super) kind: EntryKind,
    pub(super) modified: Option<SystemTime>,
    pub(super) size: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum EntryKind {
    Directory,
    File,
    DirectoryLink(DirectoryLinkKind),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum DirectoryLinkKind {
    FilesystemLink,
    ShellShortcut { target: PathBuf },
}

impl FileEntry {
    pub(super) fn from_path(path: PathBuf) -> Option<Self> {
        let link_metadata = fs::symlink_metadata(&path).ok()?;
        Self::from_path_with_link_metadata(path, link_metadata)
    }

    pub(super) fn from_path_with_link_metadata(
        path: PathBuf,
        link_metadata: Metadata,
    ) -> Option<Self> {
        let target_metadata = if is_filesystem_directory_link(&link_metadata) {
            Some(fs::metadata(&path).ok()?)
        } else {
            None
        };
        let metadata = target_metadata.as_ref().unwrap_or(&link_metadata);
        let name = path.file_name()?.to_string_lossy().into_owned();
        let kind = entry_kind(&path, &link_metadata, metadata);

        Some(Self {
            path,
            name,
            size: display_size(&kind, &link_metadata),
            modified: link_metadata.modified().ok(),
            kind,
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
            kind: if is_dir {
                EntryKind::Directory
            } else {
                EntryKind::File
            },
            modified,
            size,
        }
    }

    #[cfg(test)]
    pub(super) fn test_directory_link(name: &str, link_kind: DirectoryLinkKind) -> Self {
        Self {
            path: PathBuf::from(name),
            name: name.to_owned(),
            kind: EntryKind::DirectoryLink(link_kind),
            modified: None,
            size: None,
        }
    }

    pub(super) fn is_directory_like(&self) -> bool {
        matches!(
            self.kind,
            EntryKind::Directory | EntryKind::DirectoryLink(_)
        )
    }

    pub(super) fn sorts_as_directory(&self) -> bool {
        matches!(
            self.kind,
            EntryKind::Directory | EntryKind::DirectoryLink(DirectoryLinkKind::FilesystemLink)
        )
    }

    pub(super) fn uses_directory_shortcut_icon(&self) -> bool {
        matches!(self.kind, EntryKind::DirectoryLink(_))
    }

    pub(super) fn is_app_bundle(&self) -> bool {
        self.is_directory_like()
            && self
                .path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
    }

    pub(super) fn uses_app_bundle_icon(&self) -> bool {
        self.is_app_bundle()
    }

    pub(super) fn display_name(&self) -> &str {
        let suffix_start = self.name.len().saturating_sub(4);

        if self.is_app_bundle()
            && self
                .name
                .get(suffix_start..)
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".app"))
        {
            &self.name[..suffix_start]
        } else if self
            .name
            .get(suffix_start..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".lnk"))
        {
            &self.name[..suffix_start]
        } else {
            &self.name
        }
    }

    pub(super) fn display_name_with_extensions(&self, show_file_name_extensions: bool) -> &str {
        let display_name = self.display_name();

        if show_file_name_extensions || self.is_directory_like() {
            return display_name;
        }

        match display_name.rfind('.') {
            Some(0) | None => display_name,
            Some(dot) => &display_name[..dot],
        }
    }

    pub(super) fn navigation_path(&self) -> &Path {
        match &self.kind {
            EntryKind::DirectoryLink(DirectoryLinkKind::ShellShortcut { target }) => target,
            EntryKind::Directory | EntryKind::DirectoryLink(_) | EntryKind::File => &self.path,
        }
    }

    pub(super) fn drop_target_path(&self) -> &Path {
        self.navigation_path()
    }

    pub(super) fn type_label(&self) -> String {
        if self.is_app_bundle() {
            return "Application".to_owned();
        }

        match self.kind {
            EntryKind::Directory | EntryKind::DirectoryLink(DirectoryLinkKind::FilesystemLink) => {
                return "File folder".to_owned();
            }
            EntryKind::DirectoryLink(DirectoryLinkKind::ShellShortcut { .. }) => {
                return "Shortcut".to_owned();
            }
            EntryKind::File => {}
        }

        let Some(extension) = self.path.extension().and_then(OsStr::to_str) else {
            return "File".to_owned();
        };

        format!("{} File", extension.to_uppercase())
    }
}

fn entry_kind(path: &Path, link_metadata: &Metadata, metadata: &Metadata) -> EntryKind {
    if metadata.is_dir() {
        if is_filesystem_directory_link(link_metadata) {
            EntryKind::DirectoryLink(DirectoryLinkKind::FilesystemLink)
        } else {
            EntryKind::Directory
        }
    } else if let Some(target) = shell_shortcut_directory_target(path) {
        EntryKind::DirectoryLink(DirectoryLinkKind::ShellShortcut { target })
    } else {
        EntryKind::File
    }
}

fn display_size(kind: &EntryKind, link_metadata: &Metadata) -> Option<u64> {
    match kind {
        EntryKind::Directory | EntryKind::DirectoryLink(DirectoryLinkKind::FilesystemLink) => None,
        EntryKind::File | EntryKind::DirectoryLink(DirectoryLinkKind::ShellShortcut { .. }) => {
            Some(link_metadata.len())
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn is_filesystem_directory_link(link_metadata: &Metadata) -> bool {
    link_metadata.file_type().is_symlink()
}

#[cfg(target_os = "windows")]
fn is_filesystem_directory_link(link_metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    link_metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
}

#[cfg(not(target_os = "windows"))]
fn shell_shortcut_directory_target(_: &Path) -> Option<PathBuf> {
    None
}

#[cfg(target_os = "windows")]
fn shell_shortcut_directory_target(path: &Path) -> Option<PathBuf> {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};

    if !path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
    {
        return None;
    }

    unsafe {
        let initialized_com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let target = resolve_shell_shortcut_target(path);
        if initialized_com {
            CoUninitialize();
        }
        target.filter(|target| target.is_dir())
    }
}

#[cfg(target_os = "windows")]
unsafe fn resolve_shell_shortcut_target(path: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::{
            Storage::FileSystem::WIN32_FIND_DATAW,
            System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ},
            UI::Shell::{IShellLinkW, SLGP_RAWPATH, ShellLink},
        },
        core::{Interface, PCWSTR},
    };

    let shell_link: IShellLinkW =
        unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }.ok()?;
    let persist_file: IPersistFile = shell_link.cast().ok()?;
    let wide_path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();

    unsafe {
        persist_file
            .Load(PCWSTR::from_raw(wide_path.as_ptr()), STGM_READ)
            .ok()?;
    }

    let mut target_buffer = vec![0u16; 32_768];
    let mut find_data = WIN32_FIND_DATAW::default();
    unsafe {
        shell_link
            .GetPath(&mut target_buffer, &mut find_data, SLGP_RAWPATH.0 as u32)
            .ok()?;
    }

    let end = target_buffer.iter().position(|ch| *ch == 0)?;
    (end > 0).then(|| PathBuf::from(String::from_utf16_lossy(&target_buffer[..end])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;
    use std::fs;

    #[test]
    fn directory_links_get_folder_type_labels() {
        let entry = FileEntry::test_directory_link("linked", DirectoryLinkKind::FilesystemLink);

        assert_eq!(entry.type_label(), "File folder");
        assert!(entry.is_directory_like());
        assert!(entry.sorts_as_directory());
        assert!(entry.uses_directory_shortcut_icon());
    }

    #[test]
    fn shell_directory_shortcuts_get_shortcut_type_labels() {
        let target = PathBuf::from("target");
        let entry = FileEntry::test_directory_link(
            "target.lnk",
            DirectoryLinkKind::ShellShortcut {
                target: target.clone(),
            },
        );

        assert_eq!(entry.type_label(), "Shortcut");
        assert!(entry.is_directory_like());
        assert!(!entry.sorts_as_directory());
        assert_eq!(entry.navigation_path(), target.as_path());
        assert!(entry.uses_directory_shortcut_icon());
    }

    #[test]
    fn file_entries_keep_extension_type_labels() {
        let entry = FileEntry::test("readme.md", false, Some(10), None);

        assert_eq!(entry.type_label(), "MD File");
        assert!(!entry.is_directory_like());
        assert!(!entry.sorts_as_directory());
        assert!(!entry.uses_directory_shortcut_icon());
    }

    #[test]
    fn app_bundle_icon_detection_matches_app_directories_only() {
        let entry = FileEntry::test("Preview.app", true, None, None);
        assert!(entry.is_app_bundle());
        assert!(entry.uses_app_bundle_icon());
        assert_eq!(entry.type_label(), "Application");

        assert!(FileEntry::test("preview.APP", true, None, None).is_app_bundle());
        assert!(!FileEntry::test("Preview.app", false, Some(1), None).is_app_bundle());
        assert!(!FileEntry::test("folder", true, None, None).is_app_bundle());
    }

    #[test]
    fn display_name_hides_shortcut_extension_case_insensitively() {
        assert_eq!(
            FileEntry::test("target.lnk", false, Some(1), None).display_name(),
            "target"
        );
        assert_eq!(
            FileEntry::test("target.LNK", false, Some(1), None).display_name(),
            "target"
        );
    }

    #[test]
    fn display_name_hides_app_bundle_extension_case_insensitively() {
        assert_eq!(
            FileEntry::test("Preview.app", true, None, None).display_name(),
            "Preview"
        );
        assert_eq!(
            FileEntry::test("Terminal.APP", true, None, None).display_name(),
            "Terminal"
        );
    }

    #[test]
    fn display_name_keeps_app_extension_for_non_bundle_files() {
        assert_eq!(
            FileEntry::test("Something.app", false, Some(1), None).display_name(),
            "Something.app"
        );
    }

    #[test]
    fn display_name_keeps_non_shortcut_names() {
        assert_eq!(
            FileEntry::test("archive.lnk.backup", false, Some(1), None).display_name(),
            "archive.lnk.backup"
        );
        assert_eq!(
            FileEntry::test("readme.md", false, Some(1), None).display_name(),
            "readme.md"
        );
    }

    #[test]
    fn display_name_with_extensions_keeps_current_special_suffix_hiding_when_enabled() {
        assert_eq!(
            FileEntry::test("target.lnk", false, Some(1), None).display_name_with_extensions(true),
            "target"
        );
        assert_eq!(
            FileEntry::test("Preview.app", true, None, None).display_name_with_extensions(true),
            "Preview"
        );
        assert_eq!(
            FileEntry::test("readme.md", false, Some(1), None).display_name_with_extensions(true),
            "readme.md"
        );
    }

    #[test]
    fn display_name_with_extensions_hides_normal_file_extensions_when_disabled() {
        assert_eq!(
            FileEntry::test("readme.md", false, Some(1), None).display_name_with_extensions(false),
            "readme"
        );
        assert_eq!(
            FileEntry::test("archive.tar.gz", false, Some(1), None)
                .display_name_with_extensions(false),
            "archive.tar"
        );
        assert_eq!(
            FileEntry::test("README", false, Some(1), None).display_name_with_extensions(false),
            "README"
        );
        assert_eq!(
            FileEntry::test(".env", false, Some(1), None).display_name_with_extensions(false),
            ".env"
        );
    }

    #[test]
    fn display_name_with_extensions_keeps_directory_names_when_disabled() {
        assert_eq!(
            FileEntry::test("folder.with.dots", true, None, None)
                .display_name_with_extensions(false),
            "folder.with.dots"
        );
        assert_eq!(
            FileEntry::test("Terminal.app", true, None, None).display_name_with_extensions(false),
            "Terminal"
        );
    }

    #[test]
    fn directory_symlink_is_classified_as_filesystem_directory_link() {
        let temp = TempDir::new();
        let target = temp.path().join("target");
        let link = temp.path().join("linked");
        fs::create_dir(&target).expect("create target");

        if create_directory_symlink(&target, &link).is_err() {
            return;
        }

        let entry = FileEntry::from_path(link).expect("entry");

        assert!(matches!(
            entry.kind,
            EntryKind::DirectoryLink(DirectoryLinkKind::FilesystemLink)
        ));
        assert_eq!(entry.type_label(), "File folder");
        assert_eq!(entry.size, None);
        assert_eq!(entry.navigation_path(), entry.path.as_path());
    }

    #[cfg(unix)]
    fn create_directory_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn create_directory_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn directory_shortcut_is_classified_as_shell_directory_link() {
        let temp = TempDir::new();
        let target = temp.path().join("target");
        let shortcut = temp.path().join("target.lnk");
        fs::create_dir(&target).expect("create target");
        create_shell_shortcut(&shortcut, &target).expect("create shortcut");

        let entry = FileEntry::from_path(shortcut.clone()).expect("entry");

        assert!(matches!(
            entry.kind,
            EntryKind::DirectoryLink(DirectoryLinkKind::ShellShortcut { target: ref actual_target })
                if actual_target == &target
        ));
        assert_eq!(entry.type_label(), "Shortcut");
        assert_eq!(entry.navigation_path(), target.as_path());
        assert_eq!(
            entry.size,
            fs::metadata(shortcut).ok().map(|metadata| metadata.len())
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn non_directory_shortcuts_remain_files() {
        let temp = TempDir::new();
        let target_file = temp.path().join("target.txt");
        let file_shortcut = temp.path().join("target-file.lnk");
        let broken_shortcut = temp.path().join("broken.lnk");
        fs::write(&target_file, b"data").expect("create target");
        create_shell_shortcut(&file_shortcut, &target_file).expect("create file shortcut");
        create_shell_shortcut(&broken_shortcut, &temp.path().join("missing"))
            .expect("create broken shortcut");

        let file_entry = FileEntry::from_path(file_shortcut).expect("file shortcut");
        let broken_entry = FileEntry::from_path(broken_shortcut).expect("broken shortcut");

        assert!(matches!(file_entry.kind, EntryKind::File));
        assert!(matches!(broken_entry.kind, EntryKind::File));
        assert_eq!(file_entry.type_label(), "LNK File");
        assert_eq!(broken_entry.type_label(), "LNK File");
    }

    #[cfg(target_os = "windows")]
    fn create_shell_shortcut(shortcut: &Path, target: &Path) -> windows::core::Result<()> {
        use std::os::windows::ffi::OsStrExt;
        use windows::{
            Win32::{
                System::Com::{
                    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance,
                    CoInitializeEx, CoUninitialize, IPersistFile,
                },
                UI::Shell::{IShellLinkW, ShellLink},
            },
            core::{Interface, PCWSTR},
        };

        unsafe {
            let initialized_com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
            let result = (|| {
                let shell_link: IShellLinkW =
                    CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                let target_path = target
                    .as_os_str()
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect::<Vec<_>>();
                shell_link.SetPath(PCWSTR::from_raw(target_path.as_ptr()))?;

                let persist_file: IPersistFile = shell_link.cast()?;
                let shortcut_path = shortcut
                    .as_os_str()
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect::<Vec<_>>();
                persist_file.Save(PCWSTR::from_raw(shortcut_path.as_ptr()), true)
            })();
            if initialized_com {
                CoUninitialize();
            }
            result
        }
    }
}
