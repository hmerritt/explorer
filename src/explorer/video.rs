use std::path::Path;

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

const VIDEO_METADATA_EXTENSIONS: &[&str] = &[
    "webm", "mkv", "flv", "vob", "ogv", "ogg", "rrc", "gifv", "mng", "mov", "avi", "qt", "wmv",
    "yuv", "rm", "asf", "amv", "m2ts", "mts", "ts", "mp4", "m4p", "m4v", "mpg", "mp2", "mpeg",
    "mpe", "mpv", "m2v", "svi", "3gp", "3g2", "mxf", "roq", "nsv", "f4v", "f4p", "f4a", "f4b",
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

pub(super) fn parse_positive_f64(value: &str) -> Option<f64> {
    let value = value.trim().parse::<f64>().ok()?;
    (value.is_finite() && value > 0.0).then_some(value)
}

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
}
