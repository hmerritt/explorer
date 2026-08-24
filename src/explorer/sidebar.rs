use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
use crate::explorer::entry::shell_shortcut_target;
use crate::explorer::filesystem::{
    NetworkDrive, NetworkDriveState, network_drives, path_is_remote_drive,
    windows_local_os_drive_root,
};
use crate::explorer::portable_devices::{PortableDeviceRoot, portable_device_roots};
use crate::explorer::{
    DirectoryKind, drive_display_label, local_drive_roots, resolve_directory_kind, wsl_drive_roots,
};
use crate::settings::{SidebarSettings, expand_configured_path};

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
    DriveNetwork(NetworkDriveState),
    GoogleDrive,
    PortableDevice,
    DriveWsl,
}

pub(super) fn sidebar_sections(
    settings: &SidebarSettings,
    filesystem_name: &str,
    google_drive: bool,
) -> SidebarSections {
    let (network_roots, drive_roots) = local_drive_roots()
        .into_iter()
        .partition(|path| path_is_remote_drive(path));
    sidebar_sections_from_roots_internal(
        settings,
        filesystem_name,
        drive_roots,
        network_roots,
        network_drives(),
        google_drive_item(google_drive),
        wsl_drive_roots(),
        portable_device_roots(),
    )
}

fn sidebar_sections_from_roots_internal(
    settings: &SidebarSettings,
    filesystem_name: &str,
    drive_roots: Vec<PathBuf>,
    network_roots: Vec<PathBuf>,
    discovered_network_drives: Vec<NetworkDrive>,
    google_drive: Option<SidebarItem>,
    wsl_roots: Vec<PathBuf>,
    portable_roots: Vec<PortableDeviceRoot>,
) -> SidebarSections {
    let mut network_drives = network_drive_items_from_roots(network_roots, filesystem_name);
    network_drives.extend(network_drive_items(discovered_network_drives));
    network_drives.extend(google_drive);
    SidebarSections {
        user_directories: configured_sidebar_items(&settings.items, filesystem_name),
        drives: {
            let mut drives = drive_items_from_roots(drive_roots, filesystem_name);
            drives.extend(portable_device_items(portable_roots));
            drives
        },
        network_drives,
        wsl_drives: wsl_drive_items_from_roots(wsl_roots),
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
        Vec::new(),
        None,
        wsl_roots,
        Vec::new(),
    )
}

#[cfg(test)]
fn sidebar_sections_from_sources(
    settings: &SidebarSettings,
    filesystem_name: &str,
    drive_roots: Vec<PathBuf>,
    network_roots: Vec<PathBuf>,
    network_drives: Vec<NetworkDrive>,
    google_drive: Option<SidebarItem>,
    wsl_roots: Vec<PathBuf>,
) -> SidebarSections {
    sidebar_sections_from_roots_internal(
        settings,
        filesystem_name,
        drive_roots,
        network_roots,
        network_drives,
        google_drive,
        wsl_roots,
        Vec::new(),
    )
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct SidebarSections {
    pub(super) user_directories: Vec<SidebarItem>,
    pub(super) drives: Vec<SidebarItem>,
    pub(super) network_drives: Vec<SidebarItem>,
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
        SidebarItemKind::DriveNetwork(_) => home_sidebar_label(path),
        SidebarItemKind::GoogleDrive => "Google Drive".to_owned(),
        SidebarItemKind::PortableDevice => home_sidebar_label(path),
        SidebarItemKind::DriveWsl => sidebar_wsl_drive_label(path),
        SidebarItemKind::CustomDirectory => home_sidebar_label(path),
    }
}

fn portable_device_items(devices: Vec<PortableDeviceRoot>) -> Vec<SidebarItem> {
    let mut items = devices
        .into_iter()
        .map(|device| SidebarItem {
            label: device.label,
            path: device.path,
            kind: SidebarItemKind::PortableDevice,
            configured_index: None,
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        left.label
            .to_ascii_lowercase()
            .cmp(&right.label.to_ascii_lowercase())
            .then_with(|| left.label.cmp(&right.label))
    });
    items
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

fn network_drive_items(drives: Vec<NetworkDrive>) -> Vec<SidebarItem> {
    drives
        .into_iter()
        .map(|drive| SidebarItem {
            label: drive.label,
            path: drive.path,
            kind: SidebarItemKind::DriveNetwork(drive.state),
            configured_index: None,
        })
        .collect()
}

fn network_drive_items_from_roots(roots: Vec<PathBuf>, filesystem_name: &str) -> Vec<SidebarItem> {
    roots
        .into_iter()
        .map(|path| SidebarItem {
            label: sidebar_drive_label(&path, filesystem_name),
            path,
            kind: SidebarItemKind::DriveNetwork(NetworkDriveState::Connected),
            configured_index: None,
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn google_drive_item(enabled: bool) -> Option<SidebarItem> {
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    google_drive_item_from_local_app_data(enabled, &local_app_data)
}

#[cfg(not(target_os = "windows"))]
fn google_drive_item(_: bool) -> Option<SidebarItem> {
    None
}

#[cfg(target_os = "windows")]
fn google_drive_item_from_local_app_data(
    enabled: bool,
    local_app_data: &Path,
) -> Option<SidebarItem> {
    if !enabled {
        return None;
    }

    let shortcut = local_app_data
        .join("Google")
        .join("Google Drive Streaming")
        .join("My Drive.lnk");
    let target = shell_shortcut_target(&shortcut)?;
    if !target.is_dir() {
        return None;
    }

    Some(SidebarItem {
        label: "Google Drive".to_owned(),
        path: target,
        kind: SidebarItemKind::GoogleDrive,
        configured_index: None,
    })
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
    use crate::settings::{SidebarGroupKind, SidebarSettings};
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
    fn portable_devices_follow_native_volumes_and_are_sorted_case_insensitively() {
        let native = PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" });
        let sections = sidebar_sections_from_roots_internal(
            &SidebarSettings::default(),
            "Filesystem",
            vec![native.clone()],
            Vec::new(),
            Vec::new(),
            None,
            Vec::new(),
            vec![
                PortableDeviceRoot {
                    label: "zeta phone".to_owned(),
                    path: PathBuf::from("portable-zeta"),
                    unavailable_reason: None,
                },
                PortableDeviceRoot {
                    label: "Alpha camera".to_owned(),
                    path: PathBuf::from("portable-alpha"),
                    unavailable_reason: Some("locked".to_owned()),
                },
            ],
        );

        assert_eq!(sections.drives[0].path, native);
        assert_eq!(sections.drives[1].label, "Alpha camera");
        assert_eq!(sections.drives[2].label, "zeta phone");
        assert!(
            sections.drives[1..]
                .iter()
                .all(|item| item.kind == SidebarItemKind::PortableDevice)
        );
    }

    #[test]
    fn sidebar_sections_keep_network_drives_separate_from_local_drives_and_wsl() {
        let mapped_path = PathBuf::from(r"S:\");
        let mounted_path = PathBuf::from("/mnt/team");
        let sections = sidebar_sections_from_sources(
            &SidebarSettings {
                items: Vec::new(),
                ..SidebarSettings::default()
            },
            "Filesystem",
            vec![PathBuf::from("X:\\")],
            vec![mounted_path.clone()],
            vec![NetworkDrive {
                label: "Team Share (S:)".to_owned(),
                path: mapped_path.clone(),
                state: NetworkDriveState::Connected,
                local_name: Some("S:".to_owned()),
                remote_name: r"\\server\team".to_owned(),
            }],
            None,
            vec![PathBuf::from("\\\\wsl.localhost\\Ubuntu-24.04\\")],
        );

        assert_eq!(sections.drives.len(), 1);
        assert_eq!(sections.drives[0].path, PathBuf::from("X:\\"));
        assert_eq!(sections.drives[0].kind, SidebarItemKind::Drive);
        assert_eq!(sections.network_drives.len(), 2);
        assert_eq!(sections.network_drives[0].path, mounted_path);
        assert_eq!(
            sections.network_drives[0].kind,
            SidebarItemKind::DriveNetwork(NetworkDriveState::Connected)
        );
        assert_eq!(
            sections.network_drives[1],
            SidebarItem {
                label: "Team Share (S:)".to_owned(),
                path: mapped_path,
                kind: SidebarItemKind::DriveNetwork(NetworkDriveState::Connected),
                configured_index: None,
            }
        );
        assert_eq!(sections.wsl_drives.len(), 1);
        assert_eq!(sections.wsl_drives[0].kind, SidebarItemKind::DriveWsl);
    }

    #[test]
    fn google_drive_is_appended_to_the_network_group() {
        let mapped_path = PathBuf::from(r"S:\");
        let google_path = PathBuf::from("google-drive");
        let google_drive = SidebarItem {
            label: "Google Drive".to_owned(),
            path: google_path.clone(),
            kind: SidebarItemKind::GoogleDrive,
            configured_index: None,
        };
        let sections = sidebar_sections_from_sources(
            &SidebarSettings {
                items: Vec::new(),
                ..SidebarSettings::default()
            },
            "Filesystem",
            Vec::new(),
            Vec::new(),
            vec![NetworkDrive {
                label: "Team Share (S:)".to_owned(),
                path: mapped_path.clone(),
                state: NetworkDriveState::Connected,
                local_name: Some("S:".to_owned()),
                remote_name: r"\\server\team".to_owned(),
            }],
            Some(google_drive.clone()),
            Vec::new(),
        );

        assert!(sections.drives.is_empty());
        assert!(sections.wsl_drives.is_empty());
        assert_eq!(sections.network_drives.len(), 2);
        assert_eq!(sections.network_drives[0].path, mapped_path);
        assert_eq!(sections.network_drives[1], google_drive);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn google_drive_discovery_requires_an_enabled_valid_directory_shortcut() {
        let local_app_data = TempDir::new();
        let shortcut = local_app_data
            .path()
            .join("Google")
            .join("Google Drive Streaming")
            .join("My Drive.lnk");
        fs::create_dir_all(shortcut.parent().unwrap()).expect("create Google Drive metadata path");

        let target = local_app_data.path().join("synced-drive");
        fs::create_dir(&target).expect("create Google Drive target");
        crate::explorer::windows_shell::create_shell_shortcut(&shortcut, &target)
            .expect("create Google Drive shortcut");

        assert_eq!(
            google_drive_item_from_local_app_data(true, local_app_data.path()),
            Some(SidebarItem {
                label: "Google Drive".to_owned(),
                path: target.clone(),
                kind: SidebarItemKind::GoogleDrive,
                configured_index: None,
            })
        );
        assert_eq!(
            google_drive_item_from_local_app_data(false, local_app_data.path()),
            None
        );

        let file_target = local_app_data.path().join("drive.txt");
        fs::write(&file_target, b"not a directory").expect("create file shortcut target");
        crate::explorer::windows_shell::create_shell_shortcut(&shortcut, &file_target)
            .expect("replace shortcut with file target");
        assert_eq!(
            google_drive_item_from_local_app_data(true, local_app_data.path()),
            None
        );

        crate::explorer::windows_shell::create_shell_shortcut(
            &shortcut,
            &local_app_data.path().join("missing"),
        )
        .expect("replace shortcut with broken target");
        assert_eq!(
            google_drive_item_from_local_app_data(true, local_app_data.path()),
            None
        );

        fs::write(&shortcut, b"not a shell shortcut").expect("create malformed shortcut");
        assert_eq!(
            google_drive_item_from_local_app_data(true, local_app_data.path()),
            None
        );

        fs::remove_file(&shortcut).expect("remove shortcut");
        assert_eq!(
            google_drive_item_from_local_app_data(true, local_app_data.path()),
            None
        );
    }

    #[test]
    fn hidden_wsl_group_keeps_underlying_sidebar_items() {
        let sections = sidebar_sections_from_roots(
            &SidebarSettings {
                hide_groups: vec![SidebarGroupKind::Wsl],
                items: Vec::new(),
                ..SidebarSettings::default()
            },
            "Filesystem",
            vec![PathBuf::from("X:\\")],
            vec![PathBuf::from("\\\\wsl.localhost\\Ubuntu-24.04\\")],
        );

        assert_eq!(sections.drives.len(), 1);
        assert_eq!(sections.wsl_drives.len(), 1);
    }
}
