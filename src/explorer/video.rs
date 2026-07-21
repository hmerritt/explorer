use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[cfg(any(target_os = "macos", test))]
use std::env;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
#[cfg(target_os = "macos")]
use std::{fs, os::unix::fs::PermissionsExt};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub(super) fn ffmpeg_executable_path() -> PathBuf {
    resolve_media_tool_path(ffmpeg_sidecar::paths::ffmpeg_path(), OsStr::new("ffmpeg"))
}

pub(super) fn ffprobe_executable_path() -> PathBuf {
    resolve_media_tool_path(
        ffmpeg_sidecar::ffprobe::ffprobe_path(),
        OsStr::new("ffprobe"),
    )
}

pub(super) fn ffmpeg_is_installed() -> bool {
    media_tool_is_installed(&ffmpeg_executable_path())
}

pub(super) fn ffprobe_is_installed() -> bool {
    media_tool_is_installed(&ffprobe_executable_path())
}

fn resolve_media_tool_path(default_path: PathBuf, command_name: &OsStr) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let homebrew_dirs = macos_homebrew_bin_dirs();
        return resolve_media_tool_path_with(
            default_path,
            command_name,
            env::var_os("PATH").as_deref(),
            &homebrew_dirs,
            is_executable_file,
        );
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = command_name;
        default_path
    }
}

#[cfg(any(target_os = "macos", test))]
fn resolve_media_tool_path_with(
    default_path: PathBuf,
    command_name: &OsStr,
    path_var: Option<&OsStr>,
    fallback_dirs: &[PathBuf],
    mut is_executable_file: impl FnMut(&Path) -> bool,
) -> PathBuf {
    if default_path != Path::new(command_name) {
        return default_path;
    }

    if let Some(path_var) = path_var {
        for directory in env::split_paths(path_var) {
            let candidate = directory.join(command_name);
            if is_executable_file(&candidate) {
                return candidate;
            }
        }
    }

    for directory in fallback_dirs {
        let candidate = directory.join(command_name);
        if is_executable_file(&candidate) {
            return candidate;
        }
    }

    default_path
}

#[cfg(target_os = "macos")]
fn macos_homebrew_bin_dirs() -> [PathBuf; 2] {
    macos_homebrew_bin_dirs_for_arch(cfg!(target_arch = "aarch64"))
}

#[cfg(any(target_os = "macos", test))]
fn macos_homebrew_bin_dirs_for_arch(is_apple_silicon: bool) -> [PathBuf; 2] {
    if is_apple_silicon {
        [
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]
    } else {
        [
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/opt/homebrew/bin"),
        ]
    }
}

#[cfg(target_os = "macos")]
fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn media_tool_is_installed(path: &Path) -> bool {
    let mut command = Command::new(path);
    command
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(target_os = "windows")]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }

    command
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

const VIDEO_FRAME_SHORT_THRESHOLD_SECONDS: f64 = 60.0;
const VIDEO_FRAME_LONG_THRESHOLD_SECONDS: f64 = 600.0;
pub(super) const VIDEO_FRAME_MEDIUM_INSET_SECONDS: f64 = 1.0;
pub(super) const VIDEO_FRAME_LONG_INSET_SECONDS: f64 = 5.0;
const VIDEO_FRAME_EOF_SEEK_INSET_SECONDS: f64 = 0.05;

pub(super) fn path_may_have_video_metadata(path: &Path) -> bool {
    if mime_guess::from_path(path)
        .first_raw()
        .is_some_and(|mime| mime.starts_with("video/"))
    {
        return true;
    }

    let Some(extension) = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
    else {
        return false;
    };

    VIDEO_METADATA_EXTENSIONS.contains(&extension.as_str())
}

pub(super) fn path_may_have_audio_metadata(path: &Path) -> bool {
    if mime_guess::from_path(path)
        .first_raw()
        .is_some_and(|mime| mime.starts_with("audio/"))
    {
        return true;
    }

    let Some(extension) = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
    else {
        return false;
    };

    AUDIO_METADATA_EXTENSIONS.contains(&extension.as_str())
}

const VIDEO_METADATA_EXTENSIONS: &[&str] = &[
    "webm", "mkv", "flv", "vob", "ogv", "ogg", "rrc", "gifv", "mng", "mov", "avi", "qt", "wmv",
    "yuv", "rm", "asf", "amv", "m2ts", "mts", "ts", "mp4", "m4p", "m4v", "mpg", "mp2", "mpeg",
    "mpe", "mpv", "m2v", "svi", "3gp", "3g2", "mxf", "roq", "nsv", "f4v", "f4p", "f4a", "f4b",
];

const AUDIO_METADATA_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "wave", "flac", "aac", "m4a", "m4b", "wma", "opus", "ogg", "oga", "mid", "midi",
    "aif", "aiff", "aifc", "ape", "amr", "au", "snd", "ac3", "dts", "ra", "mka", "mp2",
];

pub(super) fn ffprobe_duration_seconds_from_probe(probe: &serde_json::Value) -> Option<f64> {
    let format_duration = probe
        .get("format")
        .and_then(|format| format.as_object())
        .and_then(|format| format.get("duration"))
        .and_then(ffprobe_seconds_value);
    if format_duration.is_some() {
        return format_duration;
    }

    probe
        .get("streams")
        .and_then(|streams| streams.as_array())
        .and_then(|streams| {
            streams.iter().find_map(|stream| {
                let stream = stream.as_object()?;
                let codec_type = stream
                    .get("codec_type")
                    .and_then(ffprobe_scalar_value_label);
                if codec_type.as_deref() != Some("video") {
                    return None;
                }
                stream.get("duration").and_then(ffprobe_seconds_value)
            })
        })
}

fn ffprobe_seconds_value(value: &serde_json::Value) -> Option<f64> {
    ffprobe_scalar_value_label(value)
        .as_deref()
        .and_then(parse_positive_f64)
}

pub(super) fn ffprobe_scalar_value_label(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Bool(value) => Some(value.to_string()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        serde_json::Value::String(value) => {
            let value = value.trim();
            (!value.is_empty() && value != "N/A").then(|| value.to_owned())
        }
        serde_json::Value::Null | serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            None
        }
    }
}

pub(super) fn probe_video_duration_seconds(path: &Path) -> Result<f64, String> {
    let mut command = Command::new(ffprobe_executable_path());
    command
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("v:0")
        .arg("-show_entries")
        .arg("format=duration:stream=codec_type,duration")
        .arg("-of")
        .arg("json")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);

    let output = command
        .output()
        .map_err(|error| format!("could not start ffprobe: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return if stderr.is_empty() {
            Err(format!("ffprobe exited with {}", output.status))
        } else {
            Err(format!("ffprobe exited with {}: {stderr}", output.status))
        };
    }
    let probe: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("ffprobe returned unreadable duration data: {error}"))?;
    ffprobe_duration_seconds_from_probe(&probe)
        .ok_or_else(|| "Video duration is not available.".to_owned())
}

pub(super) fn parse_positive_f64(value: &str) -> Option<f64> {
    let value = value.trim().parse::<f64>().ok()?;
    (value.is_finite() && value > 0.0).then_some(value)
}

#[cfg(test)]
pub(super) fn video_thumbnail_frame_seek_seconds(duration_seconds: f64) -> Option<f64> {
    (duration_seconds.is_finite() && duration_seconds > 0.0).then(|| {
        safe_video_frame_seek_seconds(
            video_frame_inset_seconds(duration_seconds),
            duration_seconds,
        )
    })
}

pub(super) fn video_frame_inset_seconds(duration_seconds: f64) -> f64 {
    if duration_seconds < VIDEO_FRAME_SHORT_THRESHOLD_SECONDS {
        0.0
    } else if duration_seconds < VIDEO_FRAME_LONG_THRESHOLD_SECONDS {
        VIDEO_FRAME_MEDIUM_INSET_SECONDS
    } else {
        VIDEO_FRAME_LONG_INSET_SECONDS
    }
}

pub(super) fn safe_video_frame_seek_seconds(label_seconds: f64, duration_seconds: f64) -> f64 {
    if !duration_seconds.is_finite() || duration_seconds <= 0.0 {
        return 0.0;
    }
    let max_seek = (duration_seconds - VIDEO_FRAME_EOF_SEEK_INSET_SECONDS).max(0.0);
    label_seconds.clamp(0.0, max_seek)
}

pub(super) fn ffmpeg_seek_argument(seconds: f64) -> String {
    format!("{:.3}", seconds.max(0.0))
}

pub(super) fn video_frame_timestamp_label(seconds: f64) -> String {
    let total_millis = (seconds.max(0.0) * 1000.0).round() as u64;
    let hours = total_millis / 3_600_000;
    let minutes = (total_millis % 3_600_000) / 60_000;
    let seconds = (total_millis % 60_000) / 1000;
    let millis = total_millis % 1000;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}.{millis:03}")
    } else {
        format!("{minutes}:{seconds:02}.{millis:03}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined_path(paths: &[&str]) -> std::ffi::OsString {
        env::join_paths(paths.iter().map(PathBuf::from)).unwrap()
    }

    #[test]
    fn media_tool_resolution_prefers_adjacent_sidecar() {
        let sidecar = PathBuf::from("bundle").join("ffmpeg");
        let path_dir = PathBuf::from("path-bin");
        let homebrew_dir = PathBuf::from("homebrew-bin");
        let path_var = joined_path(&["path-bin"]);

        let resolved = resolve_media_tool_path_with(
            sidecar.clone(),
            OsStr::new("ffmpeg"),
            Some(&path_var),
            &[homebrew_dir.clone()],
            |candidate| {
                candidate == path_dir.join("ffmpeg") || candidate == homebrew_dir.join("ffmpeg")
            },
        );

        assert_eq!(resolved, sidecar);
    }

    #[test]
    fn media_tool_resolution_prefers_inherited_path_to_homebrew() {
        let path_dir = PathBuf::from("path-bin");
        let homebrew_dir = PathBuf::from("homebrew-bin");
        let path_var = joined_path(&["path-bin"]);

        let resolved = resolve_media_tool_path_with(
            PathBuf::from("ffmpeg"),
            OsStr::new("ffmpeg"),
            Some(&path_var),
            &[homebrew_dir.clone()],
            |candidate| {
                candidate == path_dir.join("ffmpeg") || candidate == homebrew_dir.join("ffmpeg")
            },
        );

        assert_eq!(resolved, path_dir.join("ffmpeg"));
    }

    #[test]
    fn media_tool_resolution_uses_homebrew_fallback_order() {
        let native_homebrew = PathBuf::from("native-homebrew-bin");
        let other_homebrew = PathBuf::from("other-homebrew-bin");

        let resolved = resolve_media_tool_path_with(
            PathBuf::from("ffmpeg"),
            OsStr::new("ffmpeg"),
            None,
            &[native_homebrew.clone(), other_homebrew.clone()],
            |candidate| {
                candidate == native_homebrew.join("ffmpeg")
                    || candidate == other_homebrew.join("ffmpeg")
            },
        );

        assert_eq!(resolved, native_homebrew.join("ffmpeg"));
    }

    #[test]
    fn macos_homebrew_fallbacks_prefer_the_native_architecture_prefix() {
        assert_eq!(
            macos_homebrew_bin_dirs_for_arch(true),
            [
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin")
            ]
        );
        assert_eq!(
            macos_homebrew_bin_dirs_for_arch(false),
            [
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/opt/homebrew/bin")
            ]
        );
    }

    #[test]
    fn media_tool_resolution_falls_back_to_bare_command() {
        let resolved = resolve_media_tool_path_with(
            PathBuf::from("ffmpeg"),
            OsStr::new("ffmpeg"),
            None,
            &[PathBuf::from("homebrew-bin")],
            |_| false,
        );

        assert_eq!(resolved, PathBuf::from("ffmpeg"));
    }

    #[test]
    fn media_tool_resolution_handles_ffmpeg_and_ffprobe_independently() {
        let homebrew_dir = PathBuf::from("homebrew-bin");
        let fallbacks = [homebrew_dir.clone()];
        let is_executable = |candidate: &Path| candidate == homebrew_dir.join("ffprobe");

        let ffmpeg = resolve_media_tool_path_with(
            PathBuf::from("ffmpeg"),
            OsStr::new("ffmpeg"),
            None,
            &fallbacks,
            is_executable,
        );
        let ffprobe = resolve_media_tool_path_with(
            PathBuf::from("ffprobe"),
            OsStr::new("ffprobe"),
            None,
            &fallbacks,
            is_executable,
        );

        assert_eq!(ffmpeg, PathBuf::from("ffmpeg"));
        assert_eq!(ffprobe, homebrew_dir.join("ffprobe"));
    }

    #[test]
    fn video_thumbnail_seek_uses_frames_tab_inset_policy() {
        assert_eq!(video_thumbnail_frame_seek_seconds(59.0), Some(0.0));
        assert_eq!(video_thumbnail_frame_seek_seconds(60.0), Some(1.0));
        assert_eq!(video_thumbnail_frame_seek_seconds(600.0), Some(5.0));
    }

    #[test]
    fn video_metadata_detection_uses_video_mime_or_extension() {
        assert!(path_may_have_video_metadata(Path::new("movie.mp4")));
        assert!(path_may_have_video_metadata(Path::new("clip.mkv")));
        assert!(!path_may_have_video_metadata(Path::new("note.txt")));
    }

    #[test]
    fn audio_metadata_detection_uses_audio_mime_or_extension() {
        assert!(path_may_have_audio_metadata(Path::new("song.mp3")));
        assert!(path_may_have_audio_metadata(Path::new("track.flac")));
        assert!(path_may_have_audio_metadata(Path::new("clip.m4a")));
        assert!(path_may_have_audio_metadata(Path::new("sound.ogg")));
        assert!(path_may_have_audio_metadata(Path::new("mix.mka")));
        assert!(path_may_have_audio_metadata(Path::new("sample.mp2")));
        assert!(!path_may_have_audio_metadata(Path::new("note.txt")));
    }
}
