use std::path::{Path, PathBuf};

use crate::explorer::filesystem::windows_local_os_drive_root;
#[cfg(feature = "rclone")]
use crate::explorer::rclone::{
    RcloneSidebarState, apply_connecting_remote_states, apply_known_mount_states, discover_remotes,
    sidebar_path_for_remote,
};
use crate::explorer::{
    DirectoryKind, drive_display_label, local_drive_roots, macos_applications_dir, macos_bin_dir,
    resolve_directory_kind, user_home_dir, wsl_drive_roots,
};
use crate::settings::{DriveHideKind, RcloneSettings, SidebarSettings, expand_configured_path};

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
    DriveWsl,
    #[cfg(feature = "rclone")]
    RcloneRemote(RcloneSidebarState),
}

pub(super) fn sidebar_sections(
    settings: &SidebarSettings,
    rclone_settings: &RcloneSettings,
) -> SidebarSections {
    #[cfg(feature = "rclone")]
    {
        let _ = rclone_settings;
        sidebar_sections_without_rclone(settings)
    }
    #[cfg(not(feature = "rclone"))]
    {
        let _ = rclone_settings;
        sidebar_sections_without_rclone(settings)
    }
}

#[cfg(feature = "rclone")]
pub(super) fn sidebar_sections_with_rclone_remotes(
    settings: &SidebarSettings,
    rclone_settings: &RcloneSettings,
) -> SidebarSections {
    let mut sections = sidebar_sections_without_rclone(settings);
    sections.rclone_remotes = rclone_remote_items(rclone_settings, RcloneRemoteLoad::Blocking);
    sections
}

pub(super) fn sidebar_sections_without_rclone(settings: &SidebarSettings) -> SidebarSections {
    sidebar_sections_without_rclone_from_roots(settings, local_drive_roots(), wsl_drive_roots())
}

fn sidebar_sections_without_rclone_from_roots(
    settings: &SidebarSettings,
    drive_roots: Vec<PathBuf>,
    wsl_roots: Vec<PathBuf>,
) -> SidebarSections {
    let home_dir = user_home_dir();
    let hide_wsl_drives = settings.hide.contains(&DriveHideKind::Wsl);
    SidebarSections {
        user_directories: configured_sidebar_items(&settings.items),
        macos_system_locations: macos_system_location_items(home_dir.as_deref()),
        drives: drive_items_from_roots(drive_roots),
        wsl_drives: if hide_wsl_drives {
            Vec::new()
        } else {
            wsl_drive_items_from_roots(wsl_roots)
        },
        #[cfg(feature = "rclone")]
        rclone_remotes: Vec::new(),
    }
}

#[cfg(test)]
fn sidebar_sections_from_roots(
    settings: &SidebarSettings,
    rclone_settings: &RcloneSettings,
    drive_roots: Vec<PathBuf>,
    wsl_roots: Vec<PathBuf>,
) -> SidebarSections {
    #[cfg(feature = "rclone")]
    {
        let mut sections =
            sidebar_sections_without_rclone_from_roots(settings, drive_roots, wsl_roots);
        sections.rclone_remotes = rclone_remote_items(rclone_settings, RcloneRemoteLoad::Cached);
        sections
    }
    #[cfg(not(feature = "rclone"))]
    {
        let _ = rclone_settings;
        sidebar_sections_without_rclone_from_roots(settings, drive_roots, wsl_roots)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct SidebarSections {
    pub(super) user_directories: Vec<SidebarItem>,
    pub(super) macos_system_locations: Vec<SidebarItem>,
    pub(super) drives: Vec<SidebarItem>,
    pub(super) wsl_drives: Vec<SidebarItem>,
    #[cfg(feature = "rclone")]
    pub(super) rclone_remotes: Vec<SidebarItem>,
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

fn configured_sidebar_items(configured_items: &[PathBuf]) -> Vec<SidebarItem> {
    configured_items
        .iter()
        .enumerate()
        .filter_map(|(configured_index, configured_path)| {
            let path = expand_configured_path(configured_path)?;
            if !path.is_dir() {
                return None;
            }
            let kind = sidebar_item_kind_for_path(&path);
            let label = sidebar_item_label_for_path(&path, kind);
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

fn sidebar_item_label_for_path(path: &Path, kind: SidebarItemKind) -> String {
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
            sidebar_drive_label(path)
        }
        SidebarItemKind::Directory(DirectoryKind::DriveWsl) => sidebar_wsl_drive_label(path),
        SidebarItemKind::Drive | SidebarItemKind::DriveWindows => sidebar_drive_label(path),
        SidebarItemKind::DriveWsl => sidebar_wsl_drive_label(path),
        SidebarItemKind::CustomDirectory => home_sidebar_label(path),
        #[cfg(feature = "rclone")]
        SidebarItemKind::RcloneRemote(_) => home_sidebar_label(path),
    }
}

fn home_sidebar_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Home")
        .to_owned()
}

fn macos_system_location_items(home: Option<&Path>) -> Vec<SidebarItem> {
    macos_system_location_items_from_paths(macos_applications_dir(), macos_bin_dir(home))
}

fn macos_system_location_items_from_paths(
    applications: Option<PathBuf>,
    bin: Option<PathBuf>,
) -> Vec<SidebarItem> {
    [
        (
            "Applications".to_owned(),
            applications,
            DirectoryKind::Applications,
        ),
        ("Bin".to_owned(), bin, DirectoryKind::Bin),
    ]
    .into_iter()
    .filter_map(|(label, path, kind)| {
        path.filter(|path| {
            if !path.is_dir() {
                return false;
            }
            if kind == DirectoryKind::Bin {
                std::fs::read_dir(path).is_ok()
            } else {
                true
            }
        })
        .map(|path| SidebarItem {
            label,
            path,
            kind: SidebarItemKind::Directory(kind),
            configured_index: None,
        })
    })
    .collect()
}

fn drive_items_from_roots(roots: Vec<PathBuf>) -> Vec<SidebarItem> {
    roots
        .into_iter()
        .map(|path| {
            let kind = if windows_local_os_drive_root().as_ref() == Some(&path) {
                SidebarItemKind::DriveWindows
            } else {
                SidebarItemKind::Drive
            };

            SidebarItem {
                label: sidebar_drive_label(&path),
                path,
                kind,
                configured_index: None,
            }
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

#[cfg(feature = "rclone")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RcloneRemoteLoad {
    Cached,
    Blocking,
}

#[cfg(feature = "rclone")]
fn rclone_remote_items(settings: &RcloneSettings, load: RcloneRemoteLoad) -> Vec<SidebarItem> {
    if load == RcloneRemoteLoad::Cached {
        let _ = settings;
        return Vec::new();
    }

    let mut remotes = discover_remotes(settings);
    apply_known_mount_states(&mut remotes);
    apply_connecting_remote_states(&mut remotes);
    remotes
        .into_iter()
        .map(|remote| SidebarItem {
            label: rclone_remote_sidebar_label(&remote.display_name, remote.sidebar_state()),
            path: sidebar_path_for_remote(&remote),
            kind: SidebarItemKind::RcloneRemote(remote.sidebar_state()),
            configured_index: None,
        })
        .collect()
}

#[cfg(feature = "rclone")]
fn rclone_remote_sidebar_label(display_name: &str, state: RcloneSidebarState) -> String {
    let _ = state;
    display_name.to_owned()
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
    use crate::settings::{DriveHideKind, RcloneSettings, SidebarSettings};
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

        let items =
            configured_sidebar_items(&[second.clone(), temp.path().join("missing"), first.clone()]);

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
    fn macos_system_location_items_preserve_requested_order() {
        let temp = TempDir::new();
        let applications = temp.path().join("Applications");
        let bin = temp.path().join(".Trash");
        fs::create_dir_all(&applications).expect("create applications");
        fs::create_dir_all(&bin).expect("create bin");

        let items =
            macos_system_location_items_from_paths(Some(applications.clone()), Some(bin.clone()));

        assert_eq!(
            items,
            vec![
                SidebarItem {
                    label: "Applications".to_owned(),
                    path: applications,
                    kind: SidebarItemKind::Directory(DirectoryKind::Applications),
                    configured_index: None,
                },
                SidebarItem {
                    label: "Bin".to_owned(),
                    path: bin,
                    kind: SidebarItemKind::Directory(DirectoryKind::Bin),
                    configured_index: None,
                },
            ]
        );
    }

    #[test]
    fn macos_system_location_items_omit_missing_paths() {
        let temp = TempDir::new();
        let bin = temp.path().join(".Trash");
        fs::create_dir_all(&bin).expect("create bin");

        let items = macos_system_location_items_from_paths(
            Some(temp.path().join("missing Applications")),
            Some(bin),
        );

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "Bin");
        assert_eq!(
            items[0].kind,
            SidebarItemKind::Directory(DirectoryKind::Bin)
        );
    }

    #[test]
    fn macos_system_locations_are_empty_off_macos() {
        if cfg!(target_os = "macos") {
            return;
        }

        assert!(macos_system_location_items(None).is_empty());
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
        if cfg!(target_os = "windows") {
            assert_eq!(items[0].kind, SidebarItemKind::DriveWindows);
        } else {
            assert_eq!(items[0].kind, SidebarItemKind::Drive);
        }

        if cfg!(target_os = "windows") {
            let fallback_items = drive_items_from_roots(vec![PathBuf::from(r"?:\")]);
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
            &RcloneSettings::default(),
            vec![PathBuf::from("X:\\")],
            vec![PathBuf::from("\\\\wsl.localhost\\Ubuntu-24.04\\")],
        );

        assert_eq!(sections.drives.len(), 1);
        assert_eq!(sections.wsl_drives.len(), 1);
        assert_eq!(sections.wsl_drives[0].label, "Ubuntu-24.04");
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
            &RcloneSettings::default(),
            vec![PathBuf::from("X:\\")],
            vec![PathBuf::from("\\\\wsl.localhost\\Ubuntu-24.04\\")],
        );

        assert_eq!(sections.drives.len(), 1);
        assert!(sections.wsl_drives.is_empty());
    }

    #[cfg(feature = "rclone")]
    #[test]
    fn sidebar_sections_hide_rclone_remotes_when_disabled() {
        let sections = sidebar_sections_from_roots(
            &SidebarSettings {
                items: Vec::new(),
                ..SidebarSettings::default()
            },
            &RcloneSettings {
                enabled: false,
                ..RcloneSettings::default()
            },
            Vec::new(),
            Vec::new(),
        );

        assert!(sections.rclone_remotes.is_empty());
    }

    #[cfg(feature = "rclone")]
    #[test]
    fn rclone_remote_sidebar_label_omits_state_suffixes() {
        for state in [
            RcloneSidebarState::Disconnected,
            RcloneSidebarState::Connecting,
            RcloneSidebarState::Mounted,
            RcloneSidebarState::TransferMode,
            RcloneSidebarState::Error,
        ] {
            assert_eq!(rclone_remote_sidebar_label("gdrive", state), "gdrive");
        }
    }
}
