use std::path::{Path, PathBuf};

use crate::explorer::filesystem::{
    drive_display_label, local_drive_roots, user_desktop_dir, user_downloads_dir, user_home_dir,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SidebarItem {
    pub(super) label: String,
    pub(super) path: PathBuf,
    pub(super) kind: SidebarItemKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SidebarItemKind {
    UserDirectory(UserDirectoryKind),
    Drive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum UserDirectoryKind {
    Home,
    Desktop,
    Downloads,
}

pub(super) fn sidebar_sections() -> SidebarSections {
    let home_dir = user_home_dir();
    SidebarSections {
        user_directories: user_directory_items_from_paths(
            home_dir.clone(),
            user_desktop_dir(home_dir.as_deref()),
            user_downloads_dir(home_dir.as_deref()),
        ),
        drives: drive_items_from_roots(local_drive_roots()),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SidebarSections {
    pub(super) user_directories: Vec<SidebarItem>,
    pub(super) drives: Vec<SidebarItem>,
}

fn user_directory_items_from_paths(
    home: Option<PathBuf>,
    desktop: Option<PathBuf>,
    downloads: Option<PathBuf>,
) -> Vec<SidebarItem> {
    [
        (
            home.as_deref()
                .map(home_sidebar_label)
                .unwrap_or_else(|| "Home".to_owned()),
            home,
            UserDirectoryKind::Home,
        ),
        ("Desktop".to_owned(), desktop, UserDirectoryKind::Desktop),
        (
            "Downloads".to_owned(),
            downloads,
            UserDirectoryKind::Downloads,
        ),
    ]
    .into_iter()
    .filter_map(|(label, path, kind)| {
        path.filter(|path| path.is_dir()).map(|path| SidebarItem {
            label,
            path,
            kind: SidebarItemKind::UserDirectory(kind),
        })
    })
    .collect()
}

fn home_sidebar_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Home")
        .to_owned()
}

fn drive_items_from_roots(roots: Vec<PathBuf>) -> Vec<SidebarItem> {
    roots
        .into_iter()
        .map(|path| SidebarItem {
            label: sidebar_drive_label(&path),
            path,
            kind: SidebarItemKind::Drive,
        })
        .collect()
}

fn sidebar_drive_label(path: &Path) -> String {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        if path == Path::new("/") {
            return "Filesystem".to_owned();
        }
    }

    drive_display_label(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;
    use std::fs;

    #[test]
    fn user_directory_items_preserve_windows_explorer_order() {
        let temp = TempDir::new();
        let home = temp.path().join("home");
        let desktop = home.join("Desktop");
        let downloads = home.join("Downloads");
        fs::create_dir_all(&desktop).expect("create desktop");
        fs::create_dir_all(&downloads).expect("create downloads");

        let items = user_directory_items_from_paths(
            Some(home.clone()),
            Some(desktop.clone()),
            Some(downloads.clone()),
        );

        assert_eq!(
            items,
            vec![
                SidebarItem {
                    label: "home".to_owned(),
                    path: home,
                    kind: SidebarItemKind::UserDirectory(UserDirectoryKind::Home),
                },
                SidebarItem {
                    label: "Desktop".to_owned(),
                    path: desktop,
                    kind: SidebarItemKind::UserDirectory(UserDirectoryKind::Desktop),
                },
                SidebarItem {
                    label: "Downloads".to_owned(),
                    path: downloads,
                    kind: SidebarItemKind::UserDirectory(UserDirectoryKind::Downloads),
                },
            ]
        );
    }

    #[test]
    fn user_directory_items_omit_missing_paths() {
        let temp = TempDir::new();
        let home = temp.path().join("home");
        let missing_desktop = home.join("Desktop");
        let downloads = temp.path().join("Downloads");
        fs::create_dir_all(&downloads).expect("create downloads");

        let items = user_directory_items_from_paths(None, Some(missing_desktop), Some(downloads));

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "Downloads");
    }

    #[test]
    fn home_sidebar_label_falls_back_when_path_has_no_file_name() {
        let path = Path::new(if cfg!(target_os = "windows") {
            r"C:\"
        } else {
            "/"
        });

        assert_eq!(home_sidebar_label(path), "Home");
    }

    #[test]
    fn drive_items_use_local_disk_labels_on_windows_and_filesystem_for_unix_root_elsewhere() {
        let items = drive_items_from_roots(vec![PathBuf::from(if cfg!(target_os = "windows") {
            r"C:\"
        } else {
            "/"
        })]);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, SidebarItemKind::Drive);

        if cfg!(target_os = "windows") {
            let fallback_items = drive_items_from_roots(vec![PathBuf::from(r"?:\")]);
            assert_eq!(fallback_items[0].label, "Local Disk (?:)");
        } else {
            assert_eq!(items[0].label, "Filesystem");
        }
    }
}
