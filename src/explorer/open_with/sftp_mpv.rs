use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
};

use percent_encoding::percent_decode_str;

use crate::explorer::{
    remote_fs::{self, RemoteLocation},
    video::path_may_have_video_metadata,
};

#[derive(Debug, PartialEq, Eq)]
struct Mpv {
    executable: PathBuf,
    #[cfg(target_os = "linux")]
    flatpak_args: Vec<String>,
}

impl Mpv {
    fn native(executable: PathBuf) -> Self {
        Self {
            executable,
            #[cfg(target_os = "linux")]
            flatpak_args: Vec::new(),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(crate::os_paths::native_path(&self.executable));
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        #[cfg(target_os = "linux")]
        command.args(&self.flatpak_args);
        command.stdin(Stdio::null());
        command
    }
}

#[derive(Debug)]
struct Playlist {
    mpv: Mpv,
    urls: Vec<String>,
    label: String,
}

impl Playlist {
    fn command(&self) -> Command {
        let mut command = self.mpv.command();
        command
            .args(["--player-operation-mode=pseudo-gui", "--idle=no", "--"])
            .args(&self.urls)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }
}

pub(super) struct Playback {
    pub label: String,
    pub exit: tokio::task::JoinHandle<io::Result<ExitStatus>>,
}

pub(super) fn launch(paths: &[PathBuf]) -> io::Result<Option<Playback>> {
    let Some(playlist) = prepare(paths, default_mpv)? else {
        return Ok(None);
    };
    let protocols = playlist
        .mpv
        .command()
        .args(["--no-config", "--terminal=yes", "--list-protocols"])
        .output()
        .map_err(|error| io::Error::other(format!("Could not start mpv. {error}")))?;
    if !protocols.status.success() {
        return Err(io::Error::other("Could not check mpv for SFTP support."));
    }
    if !lists_sftp(&protocols.stdout) && !lists_sftp(&protocols.stderr) {
        return Err(io::Error::other(
            "The default mpv installation does not support SFTP.",
        ));
    }

    let mut child = playlist
        .command()
        .spawn()
        .map_err(|error| io::Error::other(format!("Could not start mpv. {error}")))?;
    Ok(Some(Playback {
        label: playlist.label,
        exit: remote_fs::runtime().spawn_blocking(move || child.wait()),
    }))
}

fn lists_sftp(output: &[u8]) -> bool {
    String::from_utf8_lossy(output)
        .lines()
        .any(|line| line.trim() == "sftp://")
}

fn prepare(
    paths: &[PathBuf],
    mut default_mpv: impl FnMut(&Path) -> Option<Mpv>,
) -> io::Result<Option<Playlist>> {
    let mut targets = Vec::with_capacity(paths.len());
    for path in paths {
        let Some(location) = RemoteLocation::from_provider(path) else {
            return Ok(None);
        };
        let name = Path::new(location.path.rsplit('/').next().unwrap_or_default());
        if !path_may_have_video_metadata(name) || remote_fs::cached_is_dir(path).unwrap_or(true) {
            return Ok(None);
        }
        let Some(mpv) = default_mpv(name) else {
            return Ok(None);
        };
        if targets.first().is_some_and(|(first, _)| *first != mpv) {
            return Ok(None);
        }
        targets.push((mpv, location));
    }

    let mut targets = targets.into_iter();
    let Some((mpv, first)) = targets.next() else {
        return Ok(None);
    };
    let label = if paths.len() == 1 {
        first.tab_label()
    } else {
        format!("the {} selected videos", paths.len())
    };
    let urls = std::iter::once(first)
        .chain(targets.map(|(_, location)| location))
        .map(|location| mpv_sftp_url(&location))
        .collect::<io::Result<Vec<_>>>()?;
    Ok(Some(Playlist { mpv, urls, label }))
}

fn mpv_sftp_url(location: &RemoteLocation) -> io::Result<String> {
    let authority = location.site.trim_end_matches('/');
    let authority = if let Some((user, host)) = authority.rsplit_once('@') {
        let username = percent_decode_str(user.strip_prefix("sftp://").unwrap_or_default())
            .decode_utf8()
            .ok()
            .filter(|user| {
                !user.is_empty()
                    && !user.contains([':', '/', '?', '#'])
                    && !user.chars().any(char::is_control)
            })
            .ok_or_else(|| io::Error::other("mpv cannot open this SFTP username in a URL."))?;
        format!("sftp://{username}@{host}")
    } else {
        authority.to_owned()
    };

    // FFmpeg's SFTP reader passes the path to libssh without percent-decoding it.
    Ok(format!("{authority}{}", location.path))
}

#[cfg(target_os = "windows")]
fn default_mpv(name: &Path) -> Option<Mpv> {
    let association = super::windows_file_association_query(name)?;
    let path = super::windows_assoc_query_string(&association, super::windows_assoc_executable())?;
    let path = PathBuf::from(path);
    if !path.is_absolute() || !windows_is_mpv(&path) {
        return None;
    }
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("com"))
    {
        let exe = path.with_extension("exe");
        if exe.is_file() {
            return Some(Mpv::native(exe));
        }
    }
    Some(Mpv::native(path))
}

#[cfg(any(target_os = "windows", test))]
fn windows_is_mpv(executable: &Path) -> bool {
    executable.file_name().is_some_and(|name| {
        name.eq_ignore_ascii_case("mpv.exe") || name.eq_ignore_ascii_case("mpv.com")
    })
}

#[cfg(target_os = "macos")]
fn default_mpv(name: &Path) -> Option<Mpv> {
    use cocoa::{
        base::{id, nil},
        foundation::NSString,
    };
    use objc::{class, msg_send, sel, sel_impl};

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        static kUTTagClassFilenameExtension: id;
        fn UTTypeCreatePreferredIdentifierForTag(tag_class: id, tag: id, conforms_to: id) -> id;
        fn LSCopyDefaultApplicationURLForContentType(
            content_type: id,
            roles: u32,
            error: *mut id,
        ) -> id;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: id);
    }

    let extension = name.extension()?.to_str()?.to_ascii_lowercase();
    unsafe {
        let pool: id = msg_send![class!(NSAutoreleasePool), new];
        let executable = (|| {
            let extension = NSString::alloc(nil).init_str(&extension);
            let _: id = msg_send![extension, autorelease];
            let content_type =
                UTTypeCreatePreferredIdentifierForTag(kUTTagClassFilenameExtension, extension, nil);
            if content_type == nil {
                return None;
            }
            let app_url = LSCopyDefaultApplicationURLForContentType(
                content_type,
                super::MAC_LS_ROLES_ALL,
                std::ptr::null_mut(),
            );
            CFRelease(content_type);
            if app_url == nil {
                return None;
            }
            let app = super::mac_path_from_url(app_url);
            let bundle: id = msg_send![class!(NSBundle), bundleWithURL: app_url];
            CFRelease(app_url);
            if bundle == nil || super::mac_application_bundle_id(&app?).as_deref() != Some("io.mpv")
            {
                return None;
            }
            let executable_url: id = msg_send![bundle, executableURL];
            super::mac_path_from_url(executable_url)
        })();
        let _: () = msg_send![pool, drain];
        executable.map(Mpv::native)
    }
}

#[cfg(target_os = "linux")]
fn default_mpv(name: &Path) -> Option<Mpv> {
    let mime = linux_video_mime(name)?;
    let desktop_id = super::linux_default_desktop_id_for_mime(mime)?;
    let entries = freedesktop_desktop_entry::desktop_entries(&[]);
    let entry = entries
        .iter()
        .find(|entry| super::linux_default_desktop_ids_match(entry.id(), &desktop_id))?;
    if entry.hidden() {
        return None;
    }
    linux_mpv_from_desktop_exec(entry.id(), entry.exec()?, linux_executable)
}

#[cfg(target_os = "linux")]
fn linux_mpv_from_desktop_exec(
    desktop_id: &str,
    exec: &str,
    resolve_executable: impl FnOnce(&Path) -> Option<PathBuf>,
) -> Option<Mpv> {
    let words = shlex::split(exec)?;
    let (program, args) = words.split_first()?;
    let program = Path::new(program);
    let program_name = program.file_name()?.to_str()?;
    if program_name == "mpv" {
        return Some(Mpv::native(resolve_executable(program)?));
    }
    if program_name != "flatpak"
        || !super::linux_default_desktop_ids_match(desktop_id, "io.mpv.Mpv")
    {
        return None;
    }

    let (command, args) = args.split_first()?;
    let app_index = args.iter().position(|arg| !arg.starts_with('-'))?;
    if command != "run" || args[app_index] != "io.mpv.Mpv" {
        return None;
    }

    // Keep the associated branch and architecture. Pass the URL directly to the app.
    let mut flatpak_args = vec!["run".to_owned()];
    flatpak_args.extend(
        args[..app_index]
            .iter()
            .filter(|arg| arg.starts_with("--branch=") || arg.starts_with("--arch="))
            .cloned(),
    );
    flatpak_args.push("io.mpv.Mpv".to_owned());
    Some(Mpv {
        executable: resolve_executable(program)?,
        flatpak_args,
    })
}

#[cfg(any(target_os = "linux", test))]
fn linux_video_mime(name: &Path) -> Option<&'static str> {
    // Use shared-mime-info's names where they differ from mime_guess.
    let extension = name.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "asf" => Some("application/vnd.ms-asf"),
        "avi" => Some("video/vnd.avi"),
        "m4v" => Some("video/mp4"),
        "m2ts" | "mts" | "ts" => Some("video/mp2t"),
        "vob" => Some("video/mpeg"),
        _ => mime_guess::from_ext(&extension).first_raw(),
    }
}

#[cfg(target_os = "linux")]
fn linux_executable(program: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let is_executable = |path: &Path| {
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    };
    if program.is_absolute() {
        return is_executable(program).then(|| program.to_path_buf());
    }
    if program.components().count() != 1 {
        return None;
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(program))
        .find(|path| path.is_absolute() && is_executable(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(address: &str) -> PathBuf {
        RemoteLocation::parse(address).unwrap().provider_path()
    }

    fn mpv(_: &Path) -> Option<Mpv> {
        Some(Mpv::native(PathBuf::from("mpv")))
    }

    #[test]
    fn playlist_passes_literal_remote_urls_as_separate_ordered_arguments() {
        let paths = [
            remote(
                "sftp://alice%40example.com@movie-host:2222/library/caf%C3%A9%20100%25%20%23cut.MKV",
            ),
            remote("sftp://[2001:db8::1]/library/--second.mp4"),
        ];
        let playlist = prepare(&paths, |name| {
            assert!(matches!(
                name.to_str(),
                Some("café 100% #cut.MKV" | "--second.mp4")
            ));
            mpv(name)
        })
        .unwrap()
        .unwrap();
        let command = playlist.command();
        assert_eq!(command.get_program(), "mpv");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "--player-operation-mode=pseudo-gui",
                "--idle=no",
                "--",
                "sftp://alice@example.com@movie-host:2222/library/café 100% #cut.MKV",
                "sftp://[2001:db8::1]/library/--second.mp4",
            ],
        );
    }

    #[test]
    fn playlist_requires_remote_video_files() {
        let video = remote("sftp://movie-host/first.mkv");
        for invalid in [
            PathBuf::from("local.mp4"),
            remote("sftp://movie-host/"),
            remote("sftp://movie-host/audio.flac"),
            remote("sftp://movie-host/readme.txt"),
        ] {
            assert!(prepare(&[video.clone(), invalid], mpv).unwrap().is_none());
        }
    }

    #[test]
    fn playlist_requires_the_same_default_mpv_for_each_video() {
        let first = remote("sftp://movie-host/first.mkv");
        let second = remote("sftp://movie-host/second.mp4");
        assert!(
            prepare(std::slice::from_ref(&first), |_| None)
                .unwrap()
                .is_none()
        );
        let paths = [first, second];
        assert!(
            prepare(&paths, |name| (name == Path::new("first.mkv"))
                .then(|| mpv(name).unwrap()))
            .unwrap()
            .is_none()
        );
        assert!(
            prepare(&paths, |name| Some(Mpv::native(name.join("mpv"))))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn playlist_rejects_usernames_that_could_change_the_sftp_authority() {
        for username in ["user%3Apassword", "user%3Fhost", "user%2Fpath"] {
            let path = remote(&format!("sftp://{username}@movie-host/video.mkv"));
            assert!(prepare(&[path], mpv).is_err());
        }
    }

    #[test]
    fn protocol_list_must_include_sftp_itself() {
        assert!(lists_sftp(
            b"Available protocols:\r\n  https://\r\n  sftp://\r\n"
        ));
        assert!(!lists_sftp(b"ftp://\nhttps://\n"));
        assert!(!lists_sftp(b"Could not open sftp://\n"));
    }

    #[test]
    fn windows_mpv_matches_the_registered_executable_name() {
        for name in ["mpv.exe", "MPV.COM"] {
            assert!(windows_is_mpv(Path::new(name)));
        }
        for name in ["vlc.exe", "mpv.net.exe", "mpv-helper.exe", "mpv.exe.bat"] {
            assert!(!windows_is_mpv(Path::new(name)));
        }
    }

    #[test]
    fn linux_association_uses_video_mime_from_filename() {
        for (name, mime) in [
            ("movie.MKV", "video/x-matroska"),
            ("movie.mp4", "video/mp4"),
            ("movie.avi", "video/vnd.avi"),
            ("movie.m2ts", "video/mp2t"),
        ] {
            assert_eq!(linux_video_mime(Path::new(name)), Some(mime));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_association_launches_the_selected_mpv_installation() {
        let resolve = |program: &Path| Some(program.to_path_buf());
        let native = linux_mpv_from_desktop_exec(
            "mpv.desktop",
            "\"/opt/media tools/mpv\" --player-operation-mode=pseudo-gui -- %U",
            resolve,
        )
        .unwrap();
        assert_eq!(native.command().get_program(), "/opt/media tools/mpv");

        let flatpak = linux_mpv_from_desktop_exec(
            "io.mpv.Mpv.desktop",
            "/usr/bin/flatpak run --branch=stable --arch=x86_64 --command=mpv --file-forwarding io.mpv.Mpv @@u %U @@",
            resolve,
        )
        .unwrap();
        assert_eq!(flatpak.command().get_program(), "/usr/bin/flatpak");
        assert_eq!(
            flatpak.command().get_args().collect::<Vec<_>>(),
            ["run", "--branch=stable", "--arch=x86_64", "io.mpv.Mpv"]
        );
        for exec in ["vlc %U", "flatpak run org.videolan.VLC", "sh -c 'mpv %U'"] {
            assert!(linux_mpv_from_desktop_exec("io.mpv.Mpv.desktop", exec, resolve).is_none());
        }
    }
}
