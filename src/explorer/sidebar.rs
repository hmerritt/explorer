use std::path::{Path, PathBuf};

use crate::explorer::filesystem::{
    SshfsMount, SshfsMountState, sshfs_mounts, windows_local_os_drive_root,
};
use crate::explorer::{
    DirectoryKind, drive_display_label, local_drive_roots, resolve_directory_kind, wsl_drive_roots,
};
use crate::settings::{DriveHideKind, SidebarSettings, expand_configured_path};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SidebarItem {
    pub(super) label: String,
    pub(super) path: PathBuf,
    pub(super) kind: SidebarItemKind,
    pub(super) configured_index: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SidebarItemKind {
    Directory(DirectoryKind),
    CustomDirectory,
    Drive,
    DriveWindows,
    DriveSshfs(SshfsMountState),
    DriveWsl,
}

pub(super) fn sidebar_sections(
    settings: &SidebarSettings,
    filesystem_name: &str,
) -> SidebarSections {
    sidebar_sections_from_roots_internal(
        settings,
        filesystem_name,
        local_drive_roots(),
        sshfs_mounts(),
        wsl_drive_roots(),
    )
}

fn sidebar_sections_from_roots_internal(
    settings: &SidebarSettings,
    filesystem_name: &str,
    drive_roots: Vec<PathBuf>,
    sshfs_mounts: Vec<SshfsMount>,
    wsl_roots: Vec<PathBuf>,
) -> SidebarSections {
    let hide_wsl_drives = settings.hide.contains(&DriveHideKind::Wsl);
    let mut drives = drive_items_from_roots(drive_roots, filesystem_name);
    drives.extend(sshfs_drive_items(sshfs_mounts));
    SidebarSections {
        user_directories: configured_sidebar_items(&settings.items, filesystem_name),
        drives,
        wsl_drives: if hide_wsl_drives {
            Vec::new()
        } else {
            wsl_drive_items_from_roots(wsl_roots)
        },
    }
}

#[cfg(test)]
fn sidebar_sections_from_roots(
    settings: &SidebarSettings,
    filesystem_name: &str,
    drive_roots: Vec<PathBuf>,
    wsl_roots: Vec<PathBuf>,
) -> SidebarSections {
    sidebar_sections_from_roots_internal(
        settings,
        filesystem_name,
        drive_roots,
        Vec::new(),
        wsl_roots,
    )
}

#[cfg(test)]
fn sidebar_sections_from_sources(
    settings: &SidebarSettings,
    filesystem_name: &str,
    drive_roots: Vec<PathBuf>,
    sshfs_mounts: Vec<SshfsMount>,
    wsl_roots: Vec<PathBuf>,
) -> SidebarSections {
    sidebar_sections_from_roots_internal(
        settings,
        filesystem_name,
        drive_roots,
        sshfs_mounts,
        wsl_roots,
    )
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct SidebarSections {
    pub(super) user_directories: Vec<SidebarItem>,
    pub(super) drives: Vec<SidebarItem>,
    pub(super) wsl_drives: Vec<SidebarItem>,
}

#[cfg(test)]
fn user_directory_items_from_paths(
    home: Option<PathBuf>,
    desktop: Option<PathBuf>,
    documents: Option<PathBuf>,
    downloads: Option<PathBuf>,
) -> Vec<SidebarItem> {
    [
        (
            home.as_deref()
                .map(home_sidebar_label)
                .unwrap_or_else(|| "Home".to_owned()),
            home,
            DirectoryKind::Home,
        ),
        ("Desktop".to_owned(), desktop, DirectoryKind::Desktop),
        ("Documents".to_owned(), documents, DirectoryKind::Documents),
        ("Downloads".to_owned(), downloads, DirectoryKind::Downloads),
    ]
    .into_iter()
    .filter_map(|(label, path, kind)| {
        path.filter(|path| path.is_dir()).map(|path| SidebarItem {
            label,
            path,
            kind: SidebarItemKind::Directory(kind),
            configured_index: None,
        })
    })
    .collect()
}

fn configured_sidebar_items(
    configured_items: &[PathBuf],
    filesystem_name: &str,
) -> Vec<SidebarItem> {
    configured_items
        .iter()
        .enumerate()
        .filter_map(|(configured_index, configured_path)| {
            let path = expand_configured_path(configured_path)?;
            if !path.is_dir() {
                return None;
            }
            let kind = sidebar_item_kind_for_path(&path);
            let label = sidebar_item_label_for_path(&path, kind, filesystem_name);
            Some(SidebarItem {
                label,
                path,
                kind,
                configured_index: Some(configured_index),
            })
        })
        .collect()
}

fn sidebar_item_kind_for_path(path: &Path) -> SidebarItemKind {
    match resolve_directory_kind(path) {
        Some(DirectoryKind::Drive) => SidebarItemKind::Drive,
        Some(DirectoryKind::DriveWindows) => SidebarItemKind::DriveWindows,
        Some(DirectoryKind::DriveWsl) => SidebarItemKind::DriveWsl,
        Some(kind) => SidebarItemKind::Directory(kind),
        None => SidebarItemKind::CustomDirectory,
    }
}

fn sidebar_item_label_for_path(
    path: &Path,
    kind: SidebarItemKind,
    filesystem_name: &str,
) -> String {
    match kind {
        SidebarItemKind::Directory(DirectoryKind::Home) => home_sidebar_label(path),
        SidebarItemKind::Directory(DirectoryKind::Desktop) => "Desktop".to_owned(),
        SidebarItemKind::Directory(DirectoryKind::Documents) => "Documents".to_owned(),
        SidebarItemKind::Directory(DirectoryKind::Downloads) => "Downloads".to_owned(),
        SidebarItemKind::Directory(DirectoryKind::Music) => "Music".to_owned(),
        SidebarItemKind::Directory(DirectoryKind::Pictures) => "Pictures".to_owned(),
        SidebarItemKind::Directory(DirectoryKind::Videos) => "Videos".to_owned(),
        SidebarItemKind::Directory(DirectoryKind::Applications) => "Applications".to_owned(),
        SidebarItemKind::Directory(DirectoryKind::Bin) => "Bin".to_owned(),
        SidebarItemKind::Directory(DirectoryKind::Drive | DirectoryKind::DriveWindows) => {
            sidebar_drive_label(path, filesystem_name)
        }
        SidebarItemKind::Directory(DirectoryKind::DriveWsl) => sidebar_wsl_drive_label(path),
        SidebarItemKind::Drive | SidebarItemKind::DriveWindows => {
            sidebar_drive_label(path, filesystem_name)
        }
        SidebarItemKind::DriveSshfs(_) => home_sidebar_label(path),
        SidebarItemKind::DriveWsl => sidebar_wsl_drive_label(path),
        SidebarItemKind::CustomDirectory => home_sidebar_label(path),
    }
}

fn home_sidebar_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Home")
        .to_owned()
}

fn drive_items_from_roots(roots: Vec<PathBuf>, filesystem_name: &str) -> Vec<SidebarItem> {
    roots
        .into_iter()
        .map(|path| {
            let kind = if windows_local_os_drive_root().as_ref() == Some(&path) {
                SidebarItemKind::DriveWindows
            } else {
                SidebarItemKind::Drive
            };

            SidebarItem {
                label: sidebar_drive_label(&path, filesystem_name),
                path,
                kind,
                configured_index: None,
            }
        })
        .collect()
}

fn sshfs_drive_items(mounts: Vec<SshfsMount>) -> Vec<SidebarItem> {
    mounts
        .into_iter()
        .map(|mount| SidebarItem {
            label: mount.label,
            path: mount.path,
            kind: SidebarItemKind::DriveSshfs(mount.state),
            configured_index: None,
        })
        .collect()
}

fn wsl_drive_items_from_roots(roots: Vec<PathBuf>) -> Vec<SidebarItem> {
    roots
        .into_iter()
        .map(|path| SidebarItem {
            label: sidebar_wsl_drive_label(&path),
            path,
            kind: SidebarItemKind::DriveWsl,
            configured_index: None,
        })
        .collect()
}

fn sidebar_drive_label(path: &Path, filesystem_name: &str) -> String {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        unix_sidebar_drive_label(path, filesystem_name)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = filesystem_name;
        drive_display_label(path)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn unix_sidebar_drive_label(path: &Path, filesystem_name: &str) -> String {
    if path == Path::new("/") {
        return filesystem_name.to_owned();
    }

    path.file_name()
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| drive_display_label(path))
}

fn sidebar_wsl_drive_label(path: &Path) -> String {
    path.display()
        .to_string()
        .trim_end_matches(['\\', '/'])
        .rsplit(['\\', '/'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("Linux")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::test_support::TempDir;
    use crate::settings::{DriveHideKind, SidebarSettings};
    use std::fs;

    #[test]
    fn user_directory_items_preserve_windows_explorer_order() {
        let temp = TempDir::new();
        let home = temp.path().join("home");
        let desktop = home.join("Desktop");
        let documents = home.join("Documents");
        let downloads = home.join("Downloads");
        fs::create_dir_all(&desktop).expect("create desktop");
        fs::create_dir_all(&documents).expect("create documents");
        fs::create_dir_all(&downloads).expect("create downloads");

        let items = user_directory_items_from_paths(
            Some(home.clone()),
            Some(desktop.clone()),
            Some(documents.clone()),
            Some(downloads.clone()),
        );

        assert_eq!(
            items,
            vec![
                SidebarItem {
                    label: "home".to_owned(),
                    path: home,
                    kind: SidebarItemKind::Directory(DirectoryKind::Home),
                    configured_index: None,
                },
                SidebarItem {
                    label: "Desktop".to_owned(),
                    path: desktop,
                    kind: SidebarItemKind::Directory(DirectoryKind::Desktop),
                    configured_index: None,
                },
                SidebarItem {
                    label: "Documents".to_owned(),
                    path: documents,
                    kind: SidebarItemKind::Directory(DirectoryKind::Documents),
                    configured_index: None,
                },
                SidebarItem {
                    label: "Downloads".to_owned(),
                    path: downloads,
                    kind: SidebarItemKind::Directory(DirectoryKind::Downloads),
                    configured_index: None,
                },
            ]
        );
    }

    #[test]
    fn user_directory_items_omit_missing_paths() {
        let temp = TempDir::new();
        let home = temp.path().join("home");
        let missing_desktop = home.join("Desktop");
        let missing_documents = home.join("Documents");
        let downloads = temp.path().join("Downloads");
        fs::create_dir_all(&downloads).expect("create downloads");

        let items = user_directory_items_from_paths(
            None,
            Some(missing_desktop),
            Some(missing_documents),
            Some(downloads),
        );

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "Downloads");
    }

    #[test]
    fn configured_custom_items_preserve_order_infer_labels_and_omit_missing_paths() {
        let temp = TempDir::new();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir_all(&first).expect("create first");
        fs::create_dir_all(&second).expect("create second");

        let items = configured_sidebar_items(
            &[second.clone(), temp.path().join("missing"), first.clone()],
            "Filesystem",
        );

        assert_eq!(
            items,
            vec![
                SidebarItem {
                    label: "second".to_owned(),
                    path: second,
                    kind: SidebarItemKind::CustomDirectory,
                    configured_index: Some(0),
                },
                SidebarItem {
                    label: "first".to_owned(),
                    path: first,
                    kind: SidebarItemKind::CustomDirectory,
                    configured_index: Some(2),
                },
            ]
        );
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
    fn unix_sidebar_drive_label_uses_filesystem_for_root_and_mount_tail() {
        assert_eq!(
            unix_sidebar_drive_label(Path::new("/"), "Filesystem"),
            "Filesystem"
        );
        assert_eq!(
            unix_sidebar_drive_label(Path::new("/"), "System Root"),
            "System Root"
        );
        assert_eq!(
            unix_sidebar_drive_label(Path::new("/run/media/hrmer/CDROM"), "Filesystem"),
            "CDROM"
        );
        assert_eq!(
            unix_sidebar_drive_label(Path::new("/run/media/hrmer/Ubuntu 26"), "Filesystem"),
            "Ubuntu 26"
        );
        assert_eq!(
            unix_sidebar_drive_label(Path::new("/media/hrmer/disk"), "Filesystem"),
            "disk"
        );
        assert_eq!(
            unix_sidebar_drive_label(Path::new("/Volumes/Backup Disk"), "Filesystem"),
            "Backup Disk"
        );
        assert_eq!(
            unix_sidebar_drive_label(Path::new("/mnt/share"), "Filesystem"),
            "share"
        );
    }

    #[test]
    fn drive_items_use_final_path_component_for_unix_mounts() {
        if cfg!(target_os = "windows") {
            return;
        }

        let items = drive_items_from_roots(
            vec![
                PathBuf::from("/"),
                PathBuf::from("/run/media/hrmer/CDROM"),
                PathBuf::from("/run/media/hrmer/Ubuntu 26"),
                PathBuf::from("/Volumes/Backup Disk"),
            ],
            "Filesystem",
        );
        let labels = items
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec!["Filesystem", "CDROM", "Ubuntu 26", "Backup Disk"]
        );

        let custom_root = drive_items_from_roots(vec![PathBuf::from("/")], "System Root");
        assert_eq!(custom_root[0].label, "System Root");
    }

    #[test]
    fn drive_items_use_local_disk_labels_on_windows_and_filesystem_for_unix_root_elsewhere() {
        let items = drive_items_from_roots(
            vec![PathBuf::from(if cfg!(target_os = "windows") {
                r"C:\"
            } else {
                "/"
            })],
            "Filesystem",
        );

        assert_eq!(items.len(), 1);
        if cfg!(target_os = "windows") {
            assert_eq!(items[0].kind, SidebarItemKind::DriveWindows);
        } else {
            assert_eq!(items[0].kind, SidebarItemKind::Drive);
        }

        if cfg!(target_os = "windows") {
            let fallback_items = drive_items_from_roots(vec![PathBuf::from(r"?:\")], "Filesystem");
            assert_eq!(fallback_items[0].label, "Local Disk (?:)");
        } else {
            assert_eq!(items[0].label, "Filesystem");
        }
    }

    #[test]
    fn wsl_drive_items_use_distribution_labels_and_wsl_kind() {
        let roots = vec![
            PathBuf::from("\\\\wsl.localhost\\Ubuntu-24.04\\"),
            PathBuf::from("\\\\wsl.localhost\\docker-desktop\\"),
        ];

        let items = wsl_drive_items_from_roots(roots.clone());

        assert_eq!(
            items,
            vec![
                SidebarItem {
                    label: "Ubuntu-24.04".to_owned(),
                    path: roots[0].clone(),
                    kind: SidebarItemKind::DriveWsl,
                    configured_index: None,
                },
                SidebarItem {
                    label: "docker-desktop".to_owned(),
                    path: roots[1].clone(),
                    kind: SidebarItemKind::DriveWsl,
                    configured_index: None,
                },
            ]
        );
    }

    #[test]
    fn sidebar_sections_keep_wsl_drives_separate_from_local_drives() {
        let sections = sidebar_sections_from_roots(
            &SidebarSettings {
                items: Vec::new(),
                ..SidebarSettings::default()
            },
            "Filesystem",
            vec![PathBuf::from("X:\\")],
            vec![PathBuf::from("\\\\wsl.localhost\\Ubuntu-24.04\\")],
        );

        assert_eq!(sections.drives.len(), 1);
        assert_eq!(sections.wsl_drives.len(), 1);
        assert_eq!(sections.wsl_drives[0].label, "Ubuntu-24.04");
        assert_eq!(sections.wsl_drives[0].kind, SidebarItemKind::DriveWsl);
    }

    #[test]
    fn sidebar_sections_append_sshfs_mounts_after_local_drives_before_wsl() {
        let sshfs_path = PathBuf::from(r"\\sshfs\ada@example.com");
        let sections = sidebar_sections_from_sources(
            &SidebarSettings {
                items: Vec::new(),
                ..SidebarSettings::default()
            },
            "Filesystem",
            vec![PathBuf::from("X:\\")],
            vec![SshfsMount {
                label: "hbox".to_owned(),
                path: sshfs_path.clone(),
                state: SshfsMountState::Connected,
                local_name: Some("S:".to_owned()),
            }],
            vec![PathBuf::from("\\\\wsl.localhost\\Ubuntu-24.04\\")],
        );

        assert_eq!(sections.drives.len(), 2);
        assert_eq!(sections.drives[0].path, PathBuf::from("X:\\"));
        assert_eq!(sections.drives[0].kind, SidebarItemKind::Drive);
        assert_eq!(
            sections.drives[1],
            SidebarItem {
                label: "hbox".to_owned(),
                path: sshfs_path,
                kind: SidebarItemKind::DriveSshfs(SshfsMountState::Connected),
                configured_index: None,
            }
        );
        assert_eq!(sections.wsl_drives.len(), 1);
        assert_eq!(sections.wsl_drives[0].kind, SidebarItemKind::DriveWsl);
    }

    #[test]
    fn sidebar_sections_hide_wsl_drives_when_configured() {
        let sections = sidebar_sections_from_roots(
            &SidebarSettings {
                hide: vec![DriveHideKind::Wsl],
                items: Vec::new(),
                ..SidebarSettings::default()
            },
            "Filesystem",
            vec![PathBuf::from("X:\\")],
            vec![PathBuf::from("\\\\wsl.localhost\\Ubuntu-24.04\\")],
        );

        assert_eq!(sections.drives.len(), 1);
        assert!(sections.wsl_drives.is_empty());
    }
}
