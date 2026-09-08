use std::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

#[cfg(any(target_os = "windows", test))]
use std::fs;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const PACKAGE_ID: &str = "explorer";
pub(crate) const PACKAGED_EXECUTABLE: &str = "file-explorer.exe";
const UPDATE_EXECUTABLE: &str = "Update.exe";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SquirrelEvent {
    Install,
    Updated,
    Uninstall,
    Obsolete,
    FirstRun,
}

impl SquirrelEvent {
    fn from_arg(arg: &OsStr) -> Option<Self> {
        match arg.to_str()? {
            "--squirrel-install" => Some(Self::Install),
            "--squirrel-updated" => Some(Self::Updated),
            "--squirrel-uninstall" => Some(Self::Uninstall),
            "--squirrel-obsolete" => Some(Self::Obsolete),
            "--squirrel-firstrun" => Some(Self::FirstRun),
            _ => None,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum SquirrelStartup {
    Continue {
        args: Vec<OsString>,
        first_run: bool,
    },
    Exit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SquirrelInstallation {
    pub(crate) root: PathBuf,
    pub(crate) update_exe: PathBuf,
    pub(crate) launcher: PathBuf,
}

pub(crate) fn handle_startup(args: Vec<OsString>) -> io::Result<SquirrelStartup> {
    let (event, filtered_args) = classify_startup_args(args)?;
    let Some(event) = event else {
        return Ok(SquirrelStartup::Continue {
            args: filtered_args,
            first_run: false,
        });
    };

    match event {
        SquirrelEvent::FirstRun => Ok(SquirrelStartup::Continue {
            args: filtered_args,
            first_run: true,
        }),
        SquirrelEvent::Install | SquirrelEvent::Updated => {
            install_integrations()?;
            Ok(SquirrelStartup::Exit)
        }
        SquirrelEvent::Uninstall => {
            uninstall_integrations()?;
            Ok(SquirrelStartup::Exit)
        }
        SquirrelEvent::Obsolete => Ok(SquirrelStartup::Exit),
    }
}

fn classify_startup_args(
    args: Vec<OsString>,
) -> io::Result<(Option<SquirrelEvent>, Vec<OsString>)> {
    let mut event = None;
    let mut filtered = Vec::with_capacity(args.len());
    for arg in args {
        if let Some(candidate) = SquirrelEvent::from_arg(&arg) {
            if event.replace(candidate).is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "only one Squirrel lifecycle argument is supported",
                ));
            }
        } else {
            filtered.push(arg);
        }
    }
    Ok((event, filtered))
}

#[cfg(target_os = "windows")]
fn install_integrations() -> io::Result<()> {
    let installation = installation_for_lifecycle(&std::env::current_exe()?)?;
    sync_root_icon(&std::env::current_exe()?, &installation.root)?;
    create_shortcuts(&installation.update_exe)?;
    crate::windows_file_associations::register_file_associations(&installation.launcher)
}

#[cfg(target_os = "windows")]
fn uninstall_integrations() -> io::Result<()> {
    let installation = installation_for_lifecycle(&std::env::current_exe()?)?;
    let shortcut_result = remove_shortcuts(&installation.update_exe);
    let association_result = crate::windows_file_associations::unregister_file_associations();
    shortcut_result.and(association_result)
}

#[cfg(not(target_os = "windows"))]
fn install_integrations() -> io::Result<()> {
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn uninstall_integrations() -> io::Result<()> {
    Ok(())
}

fn installation_for_lifecycle(current_exe: &Path) -> io::Result<SquirrelInstallation> {
    let app_dir = current_exe.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "executable has no parent directory",
        )
    })?;
    let root = app_dir.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Squirrel app directory has no root",
        )
    })?;
    Ok(SquirrelInstallation {
        root: root.to_path_buf(),
        update_exe: root.join(UPDATE_EXECUTABLE),
        launcher: root.join(PACKAGED_EXECUTABLE),
    })
}

pub(crate) fn validated_installation() -> io::Result<SquirrelInstallation> {
    let current_exe = std::env::current_exe()?;
    let local_app_data = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is not available"))?;
    validated_installation_from(&current_exe, Path::new(&local_app_data))
}

pub(crate) fn stable_launcher_for_current_exe(current_exe: &Path) -> Option<PathBuf> {
    let local_app_data = std::env::var_os("LOCALAPPDATA")?;
    validated_installation_from(current_exe, Path::new(&local_app_data))
        .ok()
        .map(|installation| installation.launcher)
}

fn validated_installation_from(
    current_exe: &Path,
    local_app_data: &Path,
) -> io::Result<SquirrelInstallation> {
    if !current_exe
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(PACKAGED_EXECUTABLE))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "current executable is not the packaged Explorer executable",
        ));
    }

    let installation = installation_for_lifecycle(current_exe)?;
    let app_dir = current_exe.parent().expect("checked above");
    let expected_app_dir = format!("app-{}", env!("CARGO_PKG_VERSION"));
    if !app_dir
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(&expected_app_dir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "current executable is not in Explorer's versioned Squirrel directory",
        ));
    }

    let expected_root = local_app_data.join(PACKAGE_ID);
    if !paths_equal_windows_style(&installation.root, &expected_root) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "current executable is outside Explorer's per-user Squirrel root",
        ));
    }
    if !installation.update_exe.is_file() || !installation.launcher.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Explorer's Squirrel Update.exe or stable launcher is missing",
        ));
    }
    Ok(installation)
}

fn paths_equal_windows_style(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .eq_ignore_ascii_case(right.to_string_lossy().trim_end_matches(['\\', '/']))
}

fn sync_root_icon(current_exe: &Path, root: &Path) -> io::Result<()> {
    let version_icon = current_exe
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "executable has no parent"))?
        .join("app.ico");
    fs::copy(version_icon, root.join("app.ico"))?;
    Ok(())
}

fn create_shortcuts(update_exe: &Path) -> io::Result<()> {
    run_update_variants(
        update_exe,
        &[
            "--createShortcut=file-explorer.exe",
            "--shortcut-locations=Desktop,StartMenu",
        ],
        LIFECYCLE_TIMEOUT,
    )
    .map(|_| ())
}

fn remove_shortcuts(update_exe: &Path) -> io::Result<()> {
    run_update_variants(
        update_exe,
        &[
            "--removeShortcut=file-explorer.exe",
            "--shortcut-locations=Desktop,StartMenu",
        ],
        LIFECYCLE_TIMEOUT,
    )
    .map(|_| ())
}

pub(crate) fn run_update_variants(
    update_exe: &Path,
    args: &[&str],
    timeout: Duration,
) -> io::Result<Output> {
    let mut command = Command::new(update_exe);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);

    let mut child = command.spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            if output.status.success() {
                return Ok(output);
            }
            return Err(io::Error::other(format!(
                "{} {} failed with {}: {}",
                update_exe.display(),
                args.join(" "),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{} {} timed out", update_exe.display(), args.join(" ")),
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_arguments_are_removed_before_normal_startup() {
        let args = vec![
            OsString::from("file-explorer.exe"),
            OsString::from("--squirrel-firstrun"),
            OsString::from(r"C:\Pictures\photo.png"),
        ];
        let (event, filtered) = classify_startup_args(args).unwrap();
        assert_eq!(event, Some(SquirrelEvent::FirstRun));
        assert_eq!(
            filtered,
            [
                OsString::from("file-explorer.exe"),
                OsString::from(r"C:\Pictures\photo.png")
            ]
        );
    }

    #[test]
    fn lifecycle_dispatch_rejects_ambiguous_events() {
        let error = classify_startup_args(vec![
            OsString::from("file-explorer.exe"),
            OsString::from("--squirrel-install"),
            OsString::from("--squirrel-updated"),
        ])
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn validates_installation_and_stable_launcher_paths() {
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("Local App Data");
        let root = local.join(PACKAGE_ID);
        let app_dir = root.join(format!("app-{}", env!("CARGO_PKG_VERSION")));
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(root.join(UPDATE_EXECUTABLE), b"update").unwrap();
        fs::write(root.join(PACKAGED_EXECUTABLE), b"stub").unwrap();
        let current = app_dir.join(PACKAGED_EXECUTABLE);
        fs::write(&current, b"app").unwrap();

        let installation = validated_installation_from(&current, &local).unwrap();
        assert_eq!(installation.root, root);
        assert_eq!(
            installation.launcher,
            local.join("explorer/file-explorer.exe")
        );
    }

    #[test]
    fn portable_layout_is_not_a_valid_installation() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join(PACKAGED_EXECUTABLE);
        fs::write(&current, b"portable").unwrap();
        assert!(validated_installation_from(&current, temp.path()).is_err());
    }

    #[test]
    fn lifecycle_paths_use_stable_root_launcher() {
        let root = PathBuf::from("Local App Data").join(PACKAGE_ID);
        let current = root.join("app-0.22.0").join(PACKAGED_EXECUTABLE);
        let installation = installation_for_lifecycle(&current).unwrap();
        assert_eq!(installation.launcher, root.join(PACKAGED_EXECUTABLE));
    }

    #[test]
    fn install_and_update_sync_the_packaged_icon_to_the_stable_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("explorer");
        let app_dir = root.join("app-0.22.0");
        fs::create_dir_all(&app_dir).unwrap();
        let current = app_dir.join(PACKAGED_EXECUTABLE);
        fs::write(&current, b"app").unwrap();
        fs::write(app_dir.join("app.ico"), b"canonical-icon").unwrap();

        sync_root_icon(&current, &root).unwrap();
        assert_eq!(fs::read(root.join("app.ico")).unwrap(), b"canonical-icon");
    }

    #[test]
    fn uninstall_shortcut_command_targets_both_installer_locations() {
        let args = [
            "--removeShortcut=file-explorer.exe",
            "--shortcut-locations=Desktop,StartMenu",
        ];
        assert_eq!(args[0], "--removeShortcut=file-explorer.exe");
        assert!(args[1].contains("Desktop"));
        assert!(args[1].contains("StartMenu"));
    }
}
