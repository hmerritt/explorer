use std::{
    ffi::OsString,
    io::{BufRead, BufReader, Read},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use crate::explorer::video::{ffmpeg_executable_path, ffmpeg_seek_argument};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;
const STATIC_THUMBNAIL_SEEK_SECONDS: f64 = 1.0;
const STATIC_THUMBNAIL_DECODER_THREADS: u32 = 2;
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(4);
const STDERR_LIMIT: u64 = 64 * 1024;
const MAX_PPM_DIMENSION: u32 = 16_384;
const MAX_PPM_FRAME_BYTES: usize = 512 * 1024 * 1024;
const WINDOWS_COMMAND_LINE_SAFETY_LIMIT: usize = 24 * 1024;

#[derive(Debug)]
pub(super) struct VideoFrameRgba {
    pub(super) image: image::RgbaImage,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct VideoExtractionMetrics {
    pub(super) processes: usize,
    pub(super) frames: usize,
    pub(super) spawn: Duration,
    pub(super) first_frame: Option<Duration>,
    pub(super) stream_parse: Duration,
    pub(super) render_prepare: Duration,
    pub(super) total: Duration,
    pub(super) used_fallback: bool,
}

#[derive(Debug)]
pub(super) struct VideoExtraction<T> {
    pub(super) value: T,
    pub(super) metrics: VideoExtractionMetrics,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum VideoExtractionErrorKind {
    Cancelled,
    Spawn,
    Process,
    InvalidFrame,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct VideoExtractionError {
    pub(super) kind: VideoExtractionErrorKind,
    pub(super) message: String,
}

impl VideoExtractionError {
    fn new(kind: VideoExtractionErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn cancelled() -> Self {
        Self::new(
            VideoExtractionErrorKind::Cancelled,
            "Video frame extraction was cancelled.",
        )
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.kind == VideoExtractionErrorKind::Cancelled
    }

    fn permits_fallback(&self) -> bool {
        matches!(
            self.kind,
            VideoExtractionErrorKind::Process | VideoExtractionErrorKind::InvalidFrame
        )
    }
}

impl std::fmt::Display for VideoExtractionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

pub(super) fn load_video_thumbnail_rgba(
    path: &Path,
    size: u32,
    cancel: &AtomicBool,
) -> Result<VideoExtraction<image::RgbaImage>, VideoExtractionError> {
    if size == 0 {
        return Err(VideoExtractionError::new(
            VideoExtractionErrorKind::InvalidFrame,
            "Thumbnail target has no dimensions.",
        ));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(VideoExtractionError::cancelled());
    }

    let total_started = Instant::now();
    let fast_args = static_video_thumbnail_args(path, STATIC_THUMBNAIL_SEEK_SECONDS, size, true);
    let fast = run_ffmpeg_ppm_stream(&fast_args, cancel, |_, frame| Some(frame));
    match fast {
        Ok(mut extraction) => {
            let image = extraction.value.pop().ok_or_else(|| {
                VideoExtractionError::new(
                    VideoExtractionErrorKind::InvalidFrame,
                    "ffmpeg did not return a video thumbnail frame.",
                )
            })?;
            extraction.metrics.total = total_started.elapsed();
            log_video_thumbnail_metrics(path, &extraction.metrics, "ready");
            Ok(VideoExtraction {
                value: image.image,
                metrics: extraction.metrics,
            })
        }
        Err(fast_error) if fast_error.permits_fallback() => {
            if cancel.load(Ordering::Relaxed) {
                return Err(VideoExtractionError::cancelled());
            }
            let fallback_args = static_video_thumbnail_args(path, 0.0, size, false);
            let mut extraction =
                run_ffmpeg_ppm_stream(&fallback_args, cancel, |_, frame| Some(frame))?;
            let image = extraction.value.pop().ok_or_else(|| {
                VideoExtractionError::new(
                    VideoExtractionErrorKind::InvalidFrame,
                    format!(
                        "ffmpeg did not return a fallback video thumbnail frame; fast attempt failed: {fast_error}"
                    ),
                )
            })?;
            extraction.metrics.processes += 1;
            extraction.metrics.used_fallback = true;
            extraction.metrics.total = total_started.elapsed();
            log_video_thumbnail_metrics(path, &extraction.metrics, "ready");
            Ok(VideoExtraction {
                value: image.image,
                metrics: extraction.metrics,
            })
        }
        Err(error) => {
            if crate::debug_options::icon_timings_enabled() {
                crate::debug_options::log_icon_timing(format_args!(
                    "video_thumbnails path={} outcome=failed total={:.3}ms error={}",
                    path.display(),
                    total_started.elapsed().as_secs_f64() * 1000.0,
                    error
                ));
            }
            Err(error)
        }
    }
}

pub(super) fn extract_video_frame_batch(
    path: &Path,
    seek_seconds: &[f64],
    cancel: &AtomicBool,
    mut on_frame: impl FnMut(usize, VideoFrameRgba),
) -> Result<VideoExtractionMetrics, VideoExtractionError> {
    if seek_seconds.is_empty() {
        return Ok(VideoExtractionMetrics::default());
    }

    let total_started = Instant::now();
    let batches = video_frame_batch_ranges(path, seek_seconds);
    let mut metrics = VideoExtractionMetrics::default();
    let mut emitted = 0usize;
    for range in batches {
        if cancel.load(Ordering::Relaxed) {
            return Err(VideoExtractionError::cancelled());
        }
        let args = video_frame_batch_args(path, &seek_seconds[range.clone()]);
        let extraction = run_ffmpeg_ppm_stream(&args, cancel, |index, frame| {
            on_frame(range.start + index, frame);
            None
        })?;
        emitted += extraction.metrics.frames;
        merge_metrics(&mut metrics, extraction.metrics);
    }
    if emitted == 0 {
        return Err(VideoExtractionError::new(
            VideoExtractionErrorKind::InvalidFrame,
            "ffmpeg did not return any video frames.",
        ));
    }
    metrics.total = total_started.elapsed();
    Ok(metrics)
}

fn merge_metrics(total: &mut VideoExtractionMetrics, next: VideoExtractionMetrics) {
    total.processes += next.processes;
    total.frames += next.frames;
    total.spawn += next.spawn;
    total.stream_parse += next.stream_parse;
    total.render_prepare += next.render_prepare;
    if total.first_frame.is_none() {
        total.first_frame = next.first_frame;
    }
}

pub(super) fn static_video_thumbnail_args(
    path: &Path,
    seek_seconds: f64,
    size: u32,
    keyframe_only: bool,
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("-v"),
        OsString::from("error"),
        OsString::from("-nostdin"),
        OsString::from("-noaccurate_seek"),
    ];
    if keyframe_only {
        args.extend([OsString::from("-skip_frame"), OsString::from("nokey")]);
    }
    args.extend([
        OsString::from("-threads"),
        OsString::from(STATIC_THUMBNAIL_DECODER_THREADS.to_string()),
        OsString::from("-ss"),
        OsString::from(ffmpeg_seek_argument(seek_seconds)),
        OsString::from("-i"),
        path.as_os_str().to_owned(),
        OsString::from("-map"),
        OsString::from("0:v:0"),
        OsString::from("-an"),
        OsString::from("-sn"),
        OsString::from("-dn"),
        OsString::from("-frames:v"),
        OsString::from("1"),
        OsString::from("-vf"),
        OsString::from(format!(
            "scale={size}:{size}:force_original_aspect_ratio=decrease:flags=fast_bilinear,setsar=1"
        )),
        OsString::from("-f"),
        OsString::from("image2pipe"),
        OsString::from("-vcodec"),
        OsString::from("ppm"),
        OsString::from("-"),
    ]);
    args
}

pub(super) fn video_frame_batch_args(path: &Path, seek_seconds: &[f64]) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("-v"),
        OsString::from("error"),
        OsString::from("-nostdin"),
    ];
    let mut filters = Vec::with_capacity(seek_seconds.len());
    let mut concat_inputs = String::new();
    for (index, seek) in seek_seconds.iter().copied().enumerate() {
        args.extend([
            OsString::from("-ss"),
            OsString::from(ffmpeg_seek_argument(seek)),
            OsString::from("-i"),
            path.as_os_str().to_owned(),
        ]);
        filters.push(format!(
            "[{index}:v:0]trim=end_frame=1,setpts=PTS-STARTPTS,settb=AVTB[v{index}]"
        ));
        concat_inputs.push_str(&format!("[v{index}]"));
    }
    filters.push(format!(
        "{concat_inputs}concat=n={}:v=1:a=0[out]",
        seek_seconds.len()
    ));
    args.extend([
        OsString::from("-filter_complex"),
        OsString::from(filters.join(";")),
        OsString::from("-map"),
        OsString::from("[out]"),
        OsString::from("-an"),
        OsString::from("-sn"),
        OsString::from("-dn"),
        OsString::from("-fps_mode"),
        OsString::from("passthrough"),
        OsString::from("-frames:v"),
        OsString::from(seek_seconds.len().to_string()),
        OsString::from("-f"),
        OsString::from("image2pipe"),
        OsString::from("-vcodec"),
        OsString::from("ppm"),
        OsString::from("-"),
    ]);
    args
}

fn video_frame_batch_ranges(path: &Path, seek_seconds: &[f64]) -> Vec<std::ops::Range<usize>> {
    let all = 0..seek_seconds.len();
    if !cfg!(target_os = "windows")
        || estimated_command_line_len(&video_frame_batch_args(path, seek_seconds))
            <= WINDOWS_COMMAND_LINE_SAFETY_LIMIT
    {
        return vec![all];
    }

    let mut ranges = Vec::new();
    let mut start = 0usize;
    while start < seek_seconds.len() {
        let mut end = start + 1;
        while end < seek_seconds.len()
            && estimated_command_line_len(&video_frame_batch_args(path, &seek_seconds[start..=end]))
                <= WINDOWS_COMMAND_LINE_SAFETY_LIMIT
        {
            end += 1;
        }
        ranges.push(start..end);
        start = end;
    }
    ranges
}

fn estimated_command_line_len(args: &[OsString]) -> usize {
    args.iter()
        .map(|argument| argument.to_string_lossy().len().saturating_add(3))
        .sum()
}

enum ReaderEvent {
    Frame {
        frame: VideoFrameRgba,
        parse: Duration,
    },
    Done,
    Failed(String),
}

fn run_ffmpeg_ppm_stream(
    args: &[OsString],
    cancel: &AtomicBool,
    mut on_frame: impl FnMut(usize, VideoFrameRgba) -> Option<VideoFrameRgba>,
) -> Result<VideoExtraction<Vec<VideoFrameRgba>>, VideoExtractionError> {
    let program = ffmpeg_executable_path();
    run_program_ppm_stream(&program, args, cancel, &mut on_frame)
}

fn run_program_ppm_stream(
    program: &Path,
    args: &[OsString],
    cancel: &AtomicBool,
    mut on_frame: impl FnMut(usize, VideoFrameRgba) -> Option<VideoFrameRgba>,
) -> Result<VideoExtraction<Vec<VideoFrameRgba>>, VideoExtractionError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(VideoExtractionError::cancelled());
    }
    let total_started = Instant::now();
    let spawn_started = Instant::now();
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);

    let mut child = command.spawn().map_err(|error| {
        VideoExtractionError::new(
            VideoExtractionErrorKind::Spawn,
            format!("could not start ffmpeg: {error}"),
        )
    })?;
    let spawn_elapsed = spawn_started.elapsed();
    let stdout = child.stdout.take().ok_or_else(|| {
        VideoExtractionError::new(
            VideoExtractionErrorKind::Process,
            "ffmpeg did not provide frame output.",
        )
    })?;
    let stderr = child.stderr.take();

    let stderr_task = stderr.map(|stderr| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.take(STDERR_LIMIT).read_to_end(&mut bytes);
            bytes
        })
    });
    let (sender, receiver) = mpsc::channel();
    let reader_task = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let parse_started = Instant::now();
            match read_ppm_frame(&mut reader) {
                Ok(Some(frame)) => {
                    if sender
                        .send(ReaderEvent::Frame {
                            frame,
                            parse: parse_started.elapsed(),
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                Ok(None) => {
                    let _ = sender.send(ReaderEvent::Done);
                    return;
                }
                Err(error) => {
                    let _ = sender.send(ReaderEvent::Failed(error));
                    return;
                }
            }
        }
    });

    let mut metrics = VideoExtractionMetrics {
        processes: 1,
        spawn: spawn_elapsed,
        ..VideoExtractionMetrics::default()
    };
    let mut frames = Vec::new();
    let mut reader_done = false;
    let mut reader_error = None;
    let mut status = None;
    let mut owned_child = Some(child);

    loop {
        if cancel.load(Ordering::Relaxed) {
            if let Some(child) = owned_child.as_mut() {
                let _ = child.kill();
            }
            status = wait_for_child(owned_child.as_mut());
            break;
        }

        match receiver.recv_timeout(CHILD_POLL_INTERVAL) {
            Ok(ReaderEvent::Frame { frame, parse }) => {
                let parsed_at = total_started.elapsed();
                metrics.first_frame.get_or_insert(parsed_at);
                metrics.stream_parse += parse;
                let index = metrics.frames;
                metrics.frames += 1;
                let render_started = Instant::now();
                if let Some(frame) = on_frame(index, frame) {
                    frames.push(frame);
                }
                metrics.render_prepare += render_started.elapsed();
            }
            Ok(ReaderEvent::Done) => reader_done = true,
            Ok(ReaderEvent::Failed(error)) => {
                reader_error = Some(error);
                reader_done = true;
            }
            Err(RecvTimeoutError::Disconnected) => reader_done = true,
            Err(RecvTimeoutError::Timeout) => {}
        }

        if status.is_none()
            && let Some(child) = owned_child.as_mut()
        {
            status = child.try_wait().ok().flatten();
        }
        if reader_done && status.is_some() {
            break;
        }
    }

    if status.is_none() {
        status = wait_for_child(owned_child.as_mut());
    }
    drop(owned_child.take());
    let _ = reader_task.join();
    let stderr = stderr_task
        .and_then(|task| task.join().ok())
        .unwrap_or_default();
    metrics.total = total_started.elapsed();

    if cancel.load(Ordering::Relaxed) {
        return Err(VideoExtractionError::cancelled());
    }
    if let Some(error) = reader_error {
        return Err(VideoExtractionError::new(
            VideoExtractionErrorKind::InvalidFrame,
            format!("ffmpeg returned an invalid PPM frame: {error}"),
        ));
    }
    let Some(status) = status else {
        return Err(VideoExtractionError::new(
            VideoExtractionErrorKind::Process,
            "could not wait for ffmpeg.",
        ));
    };
    if !status.success() {
        return Err(process_error(status, &stderr));
    }
    if frames.is_empty() {
        return Err(VideoExtractionError::new(
            VideoExtractionErrorKind::InvalidFrame,
            "ffmpeg completed without returning a PPM frame.",
        ));
    }

    Ok(VideoExtraction {
        value: frames,
        metrics,
    })
}

fn wait_for_child(child: Option<&mut Child>) -> Option<ExitStatus> {
    child.and_then(|child| child.wait().ok())
}

fn process_error(status: ExitStatus, stderr: &[u8]) -> VideoExtractionError {
    let stderr = String::from_utf8_lossy(stderr).trim().to_owned();
    let message = if stderr.is_empty() {
        format!("ffmpeg exited with {status}")
    } else {
        format!("ffmpeg exited with {status}: {stderr}")
    };
    VideoExtractionError::new(VideoExtractionErrorKind::Process, message)
}

fn read_ppm_frame(reader: &mut impl BufRead) -> Result<Option<VideoFrameRgba>, String> {
    let Some(magic) = read_ppm_token(reader)? else {
        return Ok(None);
    };
    if magic != b"P6" {
        return Err(format!(
            "expected P6 magic, received {:?}",
            String::from_utf8_lossy(&magic)
        ));
    }
    let width = parse_ppm_u32(read_ppm_token(reader)?, "width")?;
    let height = parse_ppm_u32(read_ppm_token(reader)?, "height")?;
    let max_value = parse_ppm_u32(read_ppm_token(reader)?, "maximum channel value")?;
    if width == 0 || height == 0 || width > MAX_PPM_DIMENSION || height > MAX_PPM_DIMENSION {
        return Err(format!("invalid PPM dimensions {width}x{height}"));
    }
    if max_value != 255 {
        return Err(format!("unsupported PPM maximum channel value {max_value}"));
    }

    let rgb_len = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(3))
        .filter(|length| *length <= MAX_PPM_FRAME_BYTES)
        .ok_or_else(|| format!("PPM frame {width}x{height} is too large"))?;
    let mut rgb = vec![0u8; rgb_len];
    reader
        .read_exact(&mut rgb)
        .map_err(|error| format!("truncated PPM pixel data: {error}"))?;
    let rgba_len = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| format!("PPM frame {width}x{height} is too large"))?;
    let mut rgba = Vec::with_capacity(rgba_len);
    for pixel in rgb.chunks_exact(3) {
        rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
    }
    let image = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| "could not construct PPM image".to_owned())?;
    Ok(Some(VideoFrameRgba { image }))
}

fn read_ppm_token(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, String> {
    let mut token = Vec::new();
    let mut in_comment = false;
    loop {
        let available = reader
            .fill_buf()
            .map_err(|error| format!("could not read PPM header: {error}"))?;
        if available.is_empty() {
            return Ok((!token.is_empty()).then_some(token));
        }
        let mut consumed = 0usize;
        while consumed < available.len() {
            let byte = available[consumed];
            if in_comment {
                consumed += 1;
                if byte == b'\n' {
                    in_comment = false;
                }
                continue;
            }
            if byte == b'#' && token.is_empty() {
                in_comment = true;
                consumed += 1;
                continue;
            }
            if byte.is_ascii_whitespace() {
                consumed += 1;
                if !token.is_empty() {
                    reader.consume(consumed);
                    return Ok(Some(token));
                }
                continue;
            }
            token.push(byte);
            consumed += 1;
        }
        reader.consume(consumed);
    }
}

fn parse_ppm_u32(token: Option<Vec<u8>>, label: &str) -> Result<u32, String> {
    let token = token.ok_or_else(|| format!("missing PPM {label}"))?;
    std::str::from_utf8(&token)
        .ok()
        .and_then(|token| token.parse::<u32>().ok())
        .ok_or_else(|| format!("invalid PPM {label}"))
}

fn log_video_thumbnail_metrics(path: &Path, metrics: &VideoExtractionMetrics, outcome: &str) {
    if crate::debug_options::icon_timings_enabled() {
        crate::debug_options::log_icon_timing(format_args!(
            "video_thumbnails path={} outcome={} processes={} fallback={} spawn={:.3}ms first_frame={:.3}ms stream_parse={:.3}ms render_prepare={:.3}ms total={:.3}ms",
            path.display(),
            outcome,
            metrics.processes,
            metrics.used_fallback,
            metrics.spawn.as_secs_f64() * 1000.0,
            metrics.first_frame.unwrap_or_default().as_secs_f64() * 1000.0,
            metrics.stream_parse.as_secs_f64() * 1000.0,
            metrics.render_prepare.as_secs_f64() * 1000.0,
            metrics.total.as_secs_f64() * 1000.0,
        ));
    }
}

#[cfg(feature = "benchmarks")]
pub mod benchmark_support {
    use std::{
        path::Path,
        sync::{Arc, atomic::AtomicBool},
        thread,
    };

    pub fn load_video_thumbnail_for_benchmark(path: &Path, size: u32) -> usize {
        super::load_video_thumbnail_rgba(path, size, &AtomicBool::new(false))
            .map(|result| result.value.len())
            .unwrap_or_default()
    }

    pub fn load_video_thumbnail_batch_for_benchmark(paths: &[&Path], size: u32) -> usize {
        let paths = Arc::new(
            paths
                .iter()
                .map(|path| path.to_path_buf())
                .collect::<Vec<_>>(),
        );
        let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ready = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let workers = super::super::image_thumbnails::image_thumbnail_loader_concurrency();
        thread::scope(|scope| {
            for _ in 0..workers {
                let paths = paths.clone();
                let next = next.clone();
                let ready = ready.clone();
                scope.spawn(move || {
                    loop {
                        let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(path) = paths.get(index) else {
                            break;
                        };
                        if super::load_video_thumbnail_rgba(path, size, &AtomicBool::new(false))
                            .is_ok()
                        {
                            ready.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                });
            }
        });
        ready.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn load_video_frame_batch_for_benchmark(path: &Path, seeks: &[f64]) -> usize {
        let mut frames = 0usize;
        let result =
            super::extract_video_frame_batch(path, seeks, &AtomicBool::new(false), |_, _| {
                frames += 1
            });
        result.map(|_| frames).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, path::PathBuf};

    fn ppm(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
        let mut bytes = format!("P6\n{width} {height}\n255\n").into_bytes();
        bytes.extend_from_slice(rgb);
        bytes
    }

    #[test]
    fn parses_multiple_ppm_frames_and_comments() {
        let mut bytes = b"P6\n# frame one\n1 1\n255\n".to_vec();
        bytes.extend_from_slice(&[10, 20, 30]);
        bytes.extend(ppm(2, 1, &[40, 50, 60, 70, 80, 90]));
        let mut reader = BufReader::new(Cursor::new(bytes));

        let first = read_ppm_frame(&mut reader).unwrap().unwrap();
        let second = read_ppm_frame(&mut reader).unwrap().unwrap();

        assert_eq!(first.image.dimensions(), (1, 1));
        assert_eq!(first.image.as_raw(), &[10, 20, 30, 255]);
        assert_eq!(second.image.dimensions(), (2, 1));
        assert_eq!(second.image.as_raw(), &[40, 50, 60, 255, 70, 80, 90, 255]);
        assert!(read_ppm_frame(&mut reader).unwrap().is_none());
    }

    #[test]
    fn rejects_truncated_and_oversized_ppm_frames() {
        let mut truncated = BufReader::new(Cursor::new(ppm(2, 1, &[1, 2, 3])));
        assert!(
            read_ppm_frame(&mut truncated)
                .unwrap_err()
                .contains("truncated")
        );

        let bytes = format!("P6\n{} 1\n255\n", MAX_PPM_DIMENSION + 1).into_bytes();
        let mut oversized = BufReader::new(Cursor::new(bytes));
        assert!(
            read_ppm_frame(&mut oversized)
                .unwrap_err()
                .contains("dimensions")
        );
    }

    #[test]
    fn missing_ffmpeg_and_pre_cancelled_requests_are_distinct() {
        let missing = Path::new("definitely-missing-explorer-ffmpeg-test-binary");
        let active = AtomicBool::new(false);
        let missing_error =
            run_program_ppm_stream(missing, &[], &active, |_, frame| Some(frame)).unwrap_err();
        assert_eq!(missing_error.kind, VideoExtractionErrorKind::Spawn);

        let cancelled = AtomicBool::new(true);
        let cancelled_error =
            run_program_ppm_stream(missing, &[], &cancelled, |_, frame| Some(frame)).unwrap_err();
        assert_eq!(cancelled_error.kind, VideoExtractionErrorKind::Cancelled);
    }

    #[test]
    fn ppm_comments_may_cross_reader_buffer_boundaries() {
        let mut bytes = b"P6\n# comment longer than the tiny buffer\n1 1\n255\n".to_vec();
        bytes.extend_from_slice(&[1, 2, 3]);
        let mut reader = BufReader::with_capacity(4, Cursor::new(bytes));

        let frame = read_ppm_frame(&mut reader).unwrap().unwrap();
        assert_eq!(frame.image.as_raw(), &[1, 2, 3, 255]);
    }

    #[test]
    fn static_thumbnail_args_seek_before_input_and_emit_ppm() {
        let args = static_video_thumbnail_args(Path::new("clip.mp4"), 1.0, 128, true);
        let args = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let index = |needle: &str| args.iter().position(|arg| arg == needle).unwrap();

        assert!(index("-ss") < index("-i"));
        assert_eq!(args[index("-ss") + 1], "1.000");
        assert_eq!(args[index("-threads") + 1], "2");
        assert_eq!(args[index("-skip_frame") + 1], "nokey");
        assert_eq!(args[index("-vcodec") + 1], "ppm");
        assert!(args[index("-vf") + 1].contains("fast_bilinear"));
    }

    #[test]
    fn frame_batch_args_preserve_seek_and_output_order() {
        let args = video_frame_batch_args(Path::new("clip.mp4"), &[0.0, 5.0, 10.0]);
        let args = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args.iter().filter(|arg| *arg == "-i").count(), 3);
        let filter = &args[args
            .iter()
            .position(|arg| arg == "-filter_complex")
            .unwrap()
            + 1];
        assert!(filter.contains("[v0][v1][v2]concat=n=3:v=1:a=0[out]"));
        assert_eq!(
            args[args.iter().position(|arg| arg == "-frames:v").unwrap() + 1],
            "3"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn frame_batches_split_before_the_windows_command_line_safety_limit() {
        let path = PathBuf::from(format!("C:/{}.mp4", "x".repeat(13_000)));
        let seeks = (0..20).map(f64::from).collect::<Vec<_>>();
        let ranges = video_frame_batch_ranges(&path, &seeks);

        assert!(ranges.len() > 1);
        assert_eq!(ranges.first().unwrap().start, 0);
        assert_eq!(ranges.last().unwrap().end, seeks.len());
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
        for range in ranges {
            let args = video_frame_batch_args(&path, &seeks[range]);
            assert!(estimated_command_line_len(&args) <= WINDOWS_COMMAND_LINE_SAFETY_LIMIT);
        }
    }
}
