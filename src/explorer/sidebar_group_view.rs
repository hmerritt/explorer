use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use gpui::Task;

use crate::{
    explorer::{
        constants::{GB_BYTES, KB_BYTES, MB_BYTES, TB_BYTES},
        entry::FileEntry,
        filesystem::NetworkDriveState,
        sidebar::{SidebarItem, SidebarItemKind, SidebarSections},
    },
    settings::SidebarGroupKind,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DriveCapacity {
    pub(super) total_bytes: u64,
    pub(super) available_bytes: u64,
}

pub(super) struct SidebarGroupViewState {
    pub(super) kind: SidebarGroupKind,
    pub(super) items: Vec<SidebarItem>,
    pub(super) capacities: HashMap<PathBuf, DriveCapacity>,
    pub(super) capacity_generation: u64,
    pub(super) capacity_task: Option<Task<()>>,
}

impl SidebarGroupViewState {
    pub(super) fn new(kind: SidebarGroupKind, sections: &SidebarSections) -> Self {
        Self {
            kind,
            items: sidebar_group_items(kind, sections),
            capacities: HashMap::new(),
            capacity_generation: 0,
            capacity_task: None,
        }
    }

    pub(super) fn item_for_path(&self, path: &Path) -> Option<&SidebarItem> {
        self.items.iter().find(|item| item.path == path)
    }

    pub(super) fn capacity_paths(&self) -> Vec<PathBuf> {
        if self.kind == SidebarGroupKind::Pinned {
            return Vec::new();
        }

        self.items
            .iter()
            .filter(|item| {
                matches!(
                    item.kind,
                    SidebarItemKind::Drive
                        | SidebarItemKind::DriveWindows
                        | SidebarItemKind::DriveNetwork(NetworkDriveState::Connected)
                        | SidebarItemKind::DriveWsl
                )
            })
            .map(|item| item.path.clone())
            .collect()
    }
}

pub(super) fn sidebar_group_label(kind: SidebarGroupKind) -> &'static str {
    match kind {
        SidebarGroupKind::Pinned => "Pinned",
        SidebarGroupKind::Drives => "Drives",
        SidebarGroupKind::Network => "Network",
        SidebarGroupKind::Wsl => "WSL",
    }
}

pub(super) fn sidebar_group_items(
    kind: SidebarGroupKind,
    sections: &SidebarSections,
) -> Vec<SidebarItem> {
    match kind {
        SidebarGroupKind::Pinned => sections.user_directories.clone(),
        SidebarGroupKind::Drives => sections.drives.clone(),
        SidebarGroupKind::Network => sections.network_drives.clone(),
        SidebarGroupKind::Wsl => sections.wsl_drives.clone(),
    }
}

pub(super) fn sidebar_group_entries(items: &[SidebarItem], query: &str) -> Vec<FileEntry> {
    let query = query.trim().to_lowercase();
    items
        .iter()
        .filter(|item| {
            query.is_empty()
                || item.label.to_lowercase().contains(&query)
                || item.path.to_string_lossy().to_lowercase().contains(&query)
        })
        .map(|item| {
            FileEntry::from_provider(item.path.clone(), item.label.clone(), true, None, None)
        })
        .collect()
}

pub(super) fn used_capacity_fraction(capacity: DriveCapacity) -> f32 {
    if capacity.total_bytes == 0 {
        return 0.0;
    }

    let available = capacity.available_bytes.min(capacity.total_bytes);
    ((capacity.total_bytes - available) as f64 / capacity.total_bytes as f64).clamp(0.0, 1.0) as f32
}

pub(super) fn drive_capacity_text(capacity: DriveCapacity) -> String {
    format!(
        "{} free of {}",
        format_drive_capacity_size(capacity.available_bytes),
        format_drive_capacity_size(capacity.total_bytes)
    )
}

fn format_drive_capacity_size(bytes: u64) -> String {
    let (unit_bytes, unit) = if bytes >= TB_BYTES {
        (TB_BYTES, "TB")
    } else if bytes >= GB_BYTES {
        (GB_BYTES, "GB")
    } else if bytes >= MB_BYTES {
        (MB_BYTES, "MB")
    } else if bytes >= KB_BYTES {
        (KB_BYTES, "KB")
    } else {
        return format!("{bytes} bytes");
    };
    let value = bytes as f64 / unit_bytes as f64;
    if value >= 10.0 {
        format!("{value:.0} {unit}")
    } else {
        let value = format!("{value:.1}");
        format!("{} {unit}", value.trim_end_matches(".0"))
    }
}

pub(super) fn query_drive_capacities(paths: Vec<PathBuf>) -> HashMap<PathBuf, DriveCapacity> {
    paths
        .into_iter()
        .filter_map(|path| query_drive_capacity(&path).map(|capacity| (path, capacity)))
        .collect()
}

#[cfg(target_os = "windows")]
fn query_drive_capacity(path: &Path) -> Option<DriveCapacity> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{Win32::Storage::FileSystem::GetDiskFreeSpaceExW, core::PCWSTR};

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut available = 0u64;
    let mut total = 0u64;
    let mut free = 0u64;
    // SAFETY: `wide` is nul-terminated and the output pointers are valid for writes.
    unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(wide.as_ptr()),
            Some(&mut available),
            Some(&mut total),
            Some(&mut free),
        )
        .ok()?;
    }
    (total > 0).then_some(DriveCapacity {
        total_bytes: total,
        available_bytes: available,
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn query_drive_capacity(path: &Path) -> Option<DriveCapacity> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is nul-terminated and `stats` points to writable storage.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: a successful `statvfs` call initialized the structure.
    let stats = unsafe { stats.assume_init() };
    let fragment_size = if stats.f_frsize > 0 {
        stats.f_frsize as u64
    } else {
        stats.f_bsize as u64
    };
    let total = (stats.f_blocks as u64).saturating_mul(fragment_size);
    let available = (stats.f_bavail as u64).saturating_mul(fragment_size);
    (total > 0).then_some(DriveCapacity {
        total_bytes: total,
        available_bytes: available,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_capacity_text_matches_explorer_style() {
        let capacity = DriveCapacity {
            total_bytes: 21 * TB_BYTES / 10,
            available_bytes: 800 * GB_BYTES,
        };
        assert_eq!(drive_capacity_text(capacity), "800 GB free of 2.1 TB");
    }

    #[test]
    fn used_fraction_clamps_invalid_available_space() {
        assert_eq!(
            used_capacity_fraction(DriveCapacity {
                total_bytes: 100,
                available_bytes: 25,
            }),
            0.75
        );
        assert_eq!(
            used_capacity_fraction(DriveCapacity {
                total_bytes: 100,
                available_bytes: 200,
            }),
            0.0
        );
    }

    #[test]
    fn group_filter_matches_labels_and_display_paths_without_reordering() {
        let items = vec![
            SidebarItem {
                label: "Documents".to_owned(),
                path: PathBuf::from("/home/ada/Documents"),
                kind: SidebarItemKind::CustomDirectory,
                configured_index: Some(0),
            },
            SidebarItem {
                label: "Work".to_owned(),
                path: PathBuf::from("/srv/projects"),
                kind: SidebarItemKind::CustomDirectory,
                configured_index: Some(1),
            },
        ];

        assert_eq!(sidebar_group_entries(&items, "doc")[0].name, "Documents");
        assert_eq!(sidebar_group_entries(&items, "projects")[0].name, "Work");
        assert_eq!(
            sidebar_group_entries(&items, "")
                .into_iter()
                .map(|entry| entry.name)
                .collect::<Vec<_>>(),
            vec!["Documents", "Work"]
        );
    }
}
