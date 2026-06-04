#[cfg(target_os = "macos")]
use std::fs::{self, File};
use std::io;
#[cfg(target_os = "macos")]
use std::io::Read;
#[cfg(any(target_os = "macos", test))]
use std::path::{Path, PathBuf};

const FULL_DISK_ACCESS_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";
const PRIVACY_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacosFullDiskAccessStatus {
    Granted,
    Missing,
    Unknown,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProbeKind {
    Directory,
    File,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Debug, Eq, PartialEq)]
struct ProtectedProbePath {
    path: PathBuf,
    kind: ProbeKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProbeAccess {
    Readable,
    PermissionDenied,
    Unavailable,
}

#[cfg(target_os = "macos")]
pub fn macos_full_disk_access_status() -> MacosFullDiskAccessStatus {
    let home_dir = std::env::var_os("HOME").map(PathBuf::from);
    macos_full_disk_access_status_from_probes(protected_probe_paths(home_dir.as_deref()))
}

#[cfg(target_os = "macos")]
pub fn open_macos_full_disk_access_settings() -> io::Result<()> {
    open_macos_full_disk_access_settings_with(|url| open::that_detached(url))
}

#[cfg(any(target_os = "macos", test))]
fn protected_probe_paths(home_dir: Option<&Path>) -> Vec<ProtectedProbePath> {
    let mut probes = Vec::new();

    if let Some(home_dir) = home_dir {
        probes.extend([
            ProtectedProbePath {
                path: home_dir
                    .join("Library")
                    .join("Application Support")
                    .join("com.apple.TCC")
                    .join("TCC.db"),
                kind: ProbeKind::File,
            },
            ProtectedProbePath {
                path: home_dir.join("Library").join("Mail"),
                kind: ProbeKind::Directory,
            },
            ProtectedProbePath {
                path: home_dir.join("Library").join("Messages"),
                kind: ProbeKind::Directory,
            },
            ProtectedProbePath {
                path: home_dir.join("Library").join("Safari"),
                kind: ProbeKind::Directory,
            },
            ProtectedProbePath {
                path: home_dir.join("Library").join("Safari").join("CloudTabs.db"),
                kind: ProbeKind::File,
            },
            ProtectedProbePath {
                path: home_dir
                    .join("Library")
                    .join("Safari")
                    .join("Bookmarks.plist"),
                kind: ProbeKind::File,
            },
        ]);
    }

    probes.extend([
        ProtectedProbePath {
            path: PathBuf::from("/Library")
                .join("Application Support")
                .join("com.apple.TCC")
                .join("TCC.db"),
            kind: ProbeKind::File,
        },
        ProtectedProbePath {
            path: PathBuf::from("/Library")
                .join("Preferences")
                .join("com.apple.TimeMachine.plist"),
            kind: ProbeKind::File,
        },
    ]);

    probes
}

#[cfg(target_os = "macos")]
fn macos_full_disk_access_status_from_probes(
    probes: Vec<ProtectedProbePath>,
) -> MacosFullDiskAccessStatus {
    classify_probe_access(probes.iter().map(probe_access))
}

fn classify_probe_access(
    accesses: impl IntoIterator<Item = ProbeAccess>,
) -> MacosFullDiskAccessStatus {
    let mut saw_permission_denied = false;

    for access in accesses {
        match access {
            ProbeAccess::Readable => return MacosFullDiskAccessStatus::Granted,
            ProbeAccess::PermissionDenied => saw_permission_denied = true,
            ProbeAccess::Unavailable => {}
        }
    }

    if saw_permission_denied {
        MacosFullDiskAccessStatus::Missing
    } else {
        MacosFullDiskAccessStatus::Unknown
    }
}

#[cfg(target_os = "macos")]
fn probe_access(probe: &ProtectedProbePath) -> ProbeAccess {
    let result = match probe.kind {
        ProbeKind::Directory => read_directory(&probe.path),
        ProbeKind::File => read_file(&probe.path),
    };

    match result {
        Ok(()) => ProbeAccess::Readable,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            ProbeAccess::PermissionDenied
        }
        Err(_) => ProbeAccess::Unavailable,
    }
}

#[cfg(target_os = "macos")]
fn read_directory(path: &Path) -> io::Result<()> {
    let mut entries = fs::read_dir(path)?;
    let _ = entries.next();
    Ok(())
}

#[cfg(target_os = "macos")]
fn read_file(path: &Path) -> io::Result<()> {
    let mut file = File::open(path)?;
    let mut buffer = [0; 1];
    let _ = file.read(&mut buffer)?;
    Ok(())
}

fn open_macos_full_disk_access_settings_with(
    mut open_url: impl FnMut(&str) -> io::Result<()>,
) -> io::Result<()> {
    match open_url(FULL_DISK_ACCESS_SETTINGS_URL) {
        Ok(()) => Ok(()),
        Err(primary_error) => match open_url(PRIVACY_SETTINGS_URL) {
            Ok(()) => Ok(()),
            Err(fallback_error) => Err(io::Error::new(
                fallback_error.kind(),
                format!(
                    "could not open Full Disk Access settings ({primary_error}); fallback failed ({fallback_error})"
                ),
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn readable_probe_grants_full_disk_access() {
        assert_eq!(
            classify_probe_access([
                ProbeAccess::Unavailable,
                ProbeAccess::Readable,
                ProbeAccess::PermissionDenied,
            ]),
            MacosFullDiskAccessStatus::Granted
        );
    }

    #[test]
    fn permission_denied_probe_means_full_disk_access_is_missing() {
        assert_eq!(
            classify_probe_access([ProbeAccess::Unavailable, ProbeAccess::PermissionDenied]),
            MacosFullDiskAccessStatus::Missing
        );
    }

    #[test]
    fn only_unavailable_probes_make_full_disk_access_unknown() {
        assert_eq!(
            classify_probe_access([ProbeAccess::Unavailable, ProbeAccess::Unavailable]),
            MacosFullDiskAccessStatus::Unknown
        );
    }

    #[test]
    fn no_probes_make_full_disk_access_unknown() {
        assert_eq!(
            classify_probe_access([]),
            MacosFullDiskAccessStatus::Unknown
        );
    }

    #[test]
    fn protected_probe_paths_include_home_and_system_locations() {
        let home = Path::new("/Users/example");
        let probes = protected_probe_paths(Some(home));
        let paths = probe_paths(&probes);

        assert!(paths.contains(&home_tcc_db(home)));
        assert!(paths.contains(&home.join("Library").join("Mail")));
        assert!(paths.contains(&home.join("Library").join("Messages")));
        assert!(paths.contains(&home.join("Library").join("Safari")));
        assert!(paths.contains(&home.join("Library").join("Safari").join("CloudTabs.db")));
        assert!(paths.contains(&home.join("Library").join("Safari").join("Bookmarks.plist")));
        assert!(paths.contains(&system_tcc_db()));
        assert!(paths.contains(&system_time_machine_preferences()));
    }

    #[test]
    fn protected_probe_paths_keep_system_locations_without_home() {
        let probes = protected_probe_paths(None);
        let paths = probe_paths(&probes);

        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&system_tcc_db()));
        assert!(paths.contains(&system_time_machine_preferences()));
    }

    #[test]
    fn settings_opener_uses_fallback_when_full_disk_access_url_fails() {
        let opened_urls = RefCell::new(Vec::new());

        open_macos_full_disk_access_settings_with(|url| {
            opened_urls.borrow_mut().push(url.to_owned());
            if url == FULL_DISK_ACCESS_SETTINGS_URL {
                Err(io::Error::other("primary failed"))
            } else {
                Ok(())
            }
        })
        .expect("fallback opens");

        assert_eq!(
            opened_urls.into_inner(),
            vec![FULL_DISK_ACCESS_SETTINGS_URL, PRIVACY_SETTINGS_URL]
        );
    }

    #[test]
    fn settings_opener_reports_error_when_both_urls_fail() {
        let opened_urls = RefCell::new(Vec::new());

        let error = open_macos_full_disk_access_settings_with(|url| {
            opened_urls.borrow_mut().push(url.to_owned());
            Err(io::Error::new(io::ErrorKind::NotFound, "missing opener"))
        })
        .expect_err("both open attempts fail");

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(error.to_string().contains("fallback failed"));
        assert_eq!(
            opened_urls.into_inner(),
            vec![FULL_DISK_ACCESS_SETTINGS_URL, PRIVACY_SETTINGS_URL]
        );
    }

    fn probe_paths(probes: &[ProtectedProbePath]) -> Vec<PathBuf> {
        probes.iter().map(|probe| probe.path.clone()).collect()
    }

    fn home_tcc_db(home: &Path) -> PathBuf {
        home.join("Library")
            .join("Application Support")
            .join("com.apple.TCC")
            .join("TCC.db")
    }

    fn system_tcc_db() -> PathBuf {
        PathBuf::from("/Library")
            .join("Application Support")
            .join("com.apple.TCC")
            .join("TCC.db")
    }

    fn system_time_machine_preferences() -> PathBuf {
        PathBuf::from("/Library")
            .join("Preferences")
            .join("com.apple.TimeMachine.plist")
    }
}
