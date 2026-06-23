use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use ffmpeg_sidecar::{
    child::FfmpegChild,
    command::FfmpegCommand,
    event::{FfmpegEvent, OutputVideoFrame},
};
use gpui::{Context, RenderImage, Task};

use crate::explorer::{
    entry::FileEntry,
    image_thumbnails::{
        CachedThumbnailImage, dimensions_for_preview, entry_may_have_hover_image_preview,
        entry_may_have_hover_video_preview,
    },
    video::path_may_have_video_metadata,
    view::ExplorerView,
};

const VIDEO_HOVER_PREVIEW_SIZE: u32 = 400;
const VIDEO_HOVER_PREVIEW_FPS: u32 = 15;
const VIDEO_HOVER_PREVIEW_POLL_INTERVAL: Duration = Duration::from_millis(16);

type SharedFfmpegChild = Arc<Mutex<Option<FfmpegChild>>>;

pub(super) struct VideoHoverPreviewSession {
    path: PathBuf,
    generation: u64,
    cancel: Arc<AtomicBool>,
    child: SharedFfmpegChild,
    task: Option<Task<()>>,
    frame: Option<VideoHoverPreviewFrame>,
    loading_thumbnail: Option<CachedThumbnailImage>,
    failed: bool,
}

#[derive(Clone, Debug)]
pub(super) struct VideoHoverPreviewFrame {
    pub(super) image: Arc<RenderImage>,
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Clone, Debug)]
pub(super) enum VideoHoverPreviewLookup {
    Loading {
        width: u32,
        height: u32,
        thumbnail: Option<CachedThumbnailImage>,
    },
    Playing(VideoHoverPreviewFrame),
    Failed,
}

#[derive(Default)]
struct VideoHoverPreviewShared {
    latest_frame: Mutex<Option<RawVideoHoverPreviewFrame>>,
    failed: AtomicBool,
    finished: AtomicBool,
}

struct RawVideoHoverPreviewFrame {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl ExplorerView {
    pub(super) fn hover_video_preview_for_entry(
        &mut self,
        entry: &FileEntry,
        cx: &mut Context<Self>,
    ) -> Option<VideoHoverPreviewLookup> {
        if !entry_may_have_hover_video_preview(entry) {
            return None;
        }

        let loading_thumbnail = self.ready_standard_video_thumbnail_for_entry(entry, cx);
        if self
            .video_hover_preview
            .as_ref()
            .is_none_or(|session| session.path != entry.path)
        {
            self.start_video_hover_preview(entry, loading_thumbnail.clone(), cx);
        } else if let Some(session) = self.video_hover_preview.as_mut()
            && session.loading_thumbnail.is_none()
            && loading_thumbnail.is_some()
        {
            session.loading_thumbnail = loading_thumbnail;
        }

        let session = self.video_hover_preview.as_ref()?;
        if session.failed {
            return Some(VideoHoverPreviewLookup::Failed);
        }
        if let Some(frame) = session.frame.clone() {
            return Some(VideoHoverPreviewLookup::Playing(frame));
        }

        let thumbnail = session.loading_thumbnail.clone();
        let (width, height) = thumbnail
            .as_ref()
            .and_then(|thumbnail| {
                dimensions_for_preview(thumbnail.width, thumbnail.height, VIDEO_HOVER_PREVIEW_SIZE)
            })
            .unwrap_or((VIDEO_HOVER_PREVIEW_SIZE, VIDEO_HOVER_PREVIEW_SIZE));

        Some(VideoHoverPreviewLookup::Loading {
            width,
            height,
            thumbnail,
        })
    }

    fn start_video_hover_preview(
        &mut self,
        entry: &FileEntry,
        loading_thumbnail: Option<CachedThumbnailImage>,
        cx: &mut Context<Self>,
    ) {
        self.cancel_video_hover_preview(cx);

        self.video_hover_preview_generation = self.video_hover_preview_generation.wrapping_add(1);
        let generation = self.video_hover_preview_generation;
        let path = entry.path.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let child = Arc::new(Mutex::new(None));
        let task = start_video_hover_preview_task(
            path.clone(),
            generation,
            cancel.clone(),
            child.clone(),
            cx,
        );

        self.video_hover_preview = Some(VideoHoverPreviewSession {
            path,
            generation,
            cancel,
            child,
            task: Some(task),
            frame: None,
            loading_thumbnail,
            failed: false,
        });
    }

    pub(super) fn cancel_video_hover_preview(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(mut session) = self.video_hover_preview.take() else {
            return false;
        };

        session.cancel.store(true, Ordering::Relaxed);
        if let Ok(mut child) = session.child.lock()
            && let Some(child) = child.as_mut()
        {
            let _ = child.kill();
        }
        if let Some(frame) = session.frame.take() {
            cx.drop_image(frame.image, None);
        }
        drop(session.task.take());
        true
    }

    fn video_hover_preview_matches(&self, path: &Path, generation: u64) -> bool {
        self.video_hover_preview.as_ref().is_some_and(|session| {
            session.path == path
                && session.generation == generation
                && !session.cancel.load(Ordering::Relaxed)
        })
    }
}

fn start_video_hover_preview_task(
    path: PathBuf,
    generation: u64,
    cancel: Arc<AtomicBool>,
    child: SharedFfmpegChild,
    cx: &mut Context<ExplorerView>,
) -> Task<()> {
    let shared = Arc::new(VideoHoverPreviewShared::default());
    cx.spawn(async move |view, cx| {
        let worker = cx.background_executor().spawn({
            let path = path.clone();
            let cancel = cancel.clone();
            let child = child.clone();
            let shared = shared.clone();
            async move {
                run_video_hover_preview_worker(&path, &cancel, &child, &shared);
            }
        });

        loop {
            if cancel.load(Ordering::Relaxed) {
                break;
            }

            let latest_frame = shared
                .latest_frame
                .lock()
                .ok()
                .and_then(|mut frame| frame.take());
            let failed = shared.failed.swap(false, Ordering::Relaxed);
            let finished = shared.finished.load(Ordering::Relaxed);

            if latest_frame.is_some() || failed || finished {
                let _ = view.update(cx, |view, cx| {
                    if !view.video_hover_preview_matches(&path, generation) {
                        return;
                    }

                    if let Some(frame) = latest_frame.and_then(video_hover_preview_frame_from_raw)
                        && let Some(session) = view.video_hover_preview.as_mut()
                    {
                        if let Some(old_frame) = session.frame.replace(frame) {
                            cx.drop_image(old_frame.image, None);
                        }
                    }

                    if failed && let Some(session) = view.video_hover_preview.as_mut() {
                        session.failed = true;
                        if let Some(old_frame) = session.frame.take() {
                            cx.drop_image(old_frame.image, None);
                        }
                    }

                    if finished && let Some(session) = view.video_hover_preview.as_mut() {
                        session.task = None;
                    }

                    cx.notify();
                });
            }

            if finished {
                break;
            }

            cx.background_executor()
                .timer(VIDEO_HOVER_PREVIEW_POLL_INTERVAL)
                .await;
        }

        cancel.store(true, Ordering::Relaxed);
        if let Ok(mut child) = child.lock()
            && let Some(child) = child.as_mut()
        {
            let _ = child.kill();
        }
        worker.await;
    })
}

fn run_video_hover_preview_worker(
    path: &Path,
    cancel: &AtomicBool,
    child: &SharedFfmpegChild,
    shared: &VideoHoverPreviewShared,
) {
    let mut ffmpeg = match spawn_video_hover_preview_ffmpeg(path) {
        Ok(ffmpeg) => ffmpeg,
        Err(_) => {
            shared.failed.store(true, Ordering::Relaxed);
            shared.finished.store(true, Ordering::Relaxed);
            return;
        }
    };
    let mut iterator = match ffmpeg.iter() {
        Ok(iterator) => iterator,
        Err(_) => {
            let _ = ffmpeg.kill();
            let _ = ffmpeg.wait();
            shared.failed.store(true, Ordering::Relaxed);
            shared.finished.store(true, Ordering::Relaxed);
            return;
        }
    };
    if let Ok(mut child_slot) = child.lock() {
        *child_slot = Some(ffmpeg);
    }

    let mut failed = false;
    for event in &mut iterator {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        match event {
            FfmpegEvent::OutputFrame(frame) => {
                if let Some(frame) = raw_video_hover_preview_frame_from_output(frame) {
                    if let Ok(mut latest_frame) = shared.latest_frame.lock() {
                        *latest_frame = Some(frame);
                    }
                } else {
                    failed = true;
                    break;
                }
            }
            FfmpegEvent::Error(_) => {
                failed = true;
                break;
            }
            FfmpegEvent::Done => break,
            _ => {}
        }
    }

    if let Ok(mut child_slot) = child.lock()
        && let Some(mut ffmpeg) = child_slot.take()
    {
        if cancel.load(Ordering::Relaxed) || failed {
            let _ = ffmpeg.kill();
        }
        let _ = ffmpeg.wait();
    }

    if failed && !cancel.load(Ordering::Relaxed) {
        shared.failed.store(true, Ordering::Relaxed);
    }
    shared.finished.store(true, Ordering::Relaxed);
}

fn spawn_video_hover_preview_ffmpeg(path: &Path) -> Result<FfmpegChild, String> {
    if !path_may_have_video_metadata(path) {
        return Err("Path is not a recognized video.".to_owned());
    }
    if !ffmpeg_sidecar::command::ffmpeg_is_installed() {
        return Err("ffmpeg is not available.".to_owned());
    }

    let mut command = FfmpegCommand::new();
    for arg in video_hover_preview_ffmpeg_args(path) {
        command.arg(arg);
    }
    command
        .spawn()
        .map_err(|error| format!("could not start ffmpeg: {error}"))
}

pub(super) fn video_hover_preview_ffmpeg_args(path: &Path) -> Vec<OsString> {
    vec![
        OsString::from("-stream_loop"),
        OsString::from("-1"),
        OsString::from("-re"),
        OsString::from("-i"),
        path.as_os_str().to_owned(),
        OsString::from("-map"),
        OsString::from("0:v:0"),
        OsString::from("-an"),
        OsString::from("-sn"),
        OsString::from("-dn"),
        OsString::from("-vf"),
        OsString::from(video_hover_preview_filter(
            VIDEO_HOVER_PREVIEW_SIZE,
            VIDEO_HOVER_PREVIEW_FPS,
        )),
        OsString::from("-f"),
        OsString::from("rawvideo"),
        OsString::from("-pix_fmt"),
        OsString::from("bgra"),
        OsString::from("-"),
    ]
}

pub(super) fn video_hover_preview_filter(size: u32, fps: u32) -> String {
    format!(
        "fps={fps},scale=trunc(iw*sar):ih,scale={size}:{size}:force_original_aspect_ratio=decrease:flags=fast_bilinear,setsar=1"
    )
}

fn raw_video_hover_preview_frame_from_output(
    frame: OutputVideoFrame,
) -> Option<RawVideoHoverPreviewFrame> {
    if frame.width == 0 || frame.height == 0 || frame.pix_fmt != "bgra" {
        return None;
    }
    let expected_len: usize = frame
        .width
        .checked_mul(frame.height)?
        .checked_mul(4)?
        .try_into()
        .ok()?;
    if frame.data.len() != expected_len {
        return None;
    }

    Some(RawVideoHoverPreviewFrame {
        width: frame.width,
        height: frame.height,
        data: frame.data,
    })
}

fn video_hover_preview_frame_from_raw(
    frame: RawVideoHoverPreviewFrame,
) -> Option<VideoHoverPreviewFrame> {
    let image = image::RgbaImage::from_raw(frame.width, frame.height, frame.data)?;
    Some(VideoHoverPreviewFrame {
        image: Arc::new(RenderImage::new(vec![image::Frame::new(image)])),
        width: frame.width,
        height: frame.height,
    })
}

pub(super) fn entry_may_have_hover_media_preview(entry: &FileEntry) -> bool {
    entry_may_have_hover_image_preview(entry) || entry_may_have_hover_video_preview(entry)
}

pub(super) fn hover_preview_is_video(entry: &FileEntry) -> bool {
    entry_may_have_hover_video_preview(entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::image_thumbnails::ThumbnailSourcePolicy;

    #[gpui::test]
    fn cache_only_standard_policy_allows_video_hover_preview_source_playback(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|window, cx| {
            let focus_handle = cx.focus_handle();
            focus_handle.focus(window);
            let mut view = ExplorerView::new_unloaded_with_settings_for_test(
                PathBuf::from("remote"),
                Some(focus_handle),
                &crate::settings::ExplorerSettings::default(),
            );
            view.thumbnail_source_policy = ThumbnailSourcePolicy::CacheOnly;
            view
        });

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                let entry = FileEntry::test("movie.mp4", false, Some(1), None);

                assert!(matches!(
                    view.hover_video_preview_for_entry(&entry, cx),
                    Some(VideoHoverPreviewLookup::Loading {
                        width: VIDEO_HOVER_PREVIEW_SIZE,
                        height: VIDEO_HOVER_PREVIEW_SIZE,
                        thumbnail: None,
                    })
                ));
                assert!(view.video_hover_preview.is_some());
            });
        });
    }

    #[test]
    fn video_hover_preview_filter_caps_size_and_corrects_sar() {
        assert_eq!(
            video_hover_preview_filter(400, 15),
            "fps=15,scale=trunc(iw*sar):ih,scale=400:400:force_original_aspect_ratio=decrease:flags=fast_bilinear,setsar=1"
        );
    }

    #[test]
    fn video_hover_preview_args_loop_silent_and_emit_bgra_rawvideo() {
        let args = video_hover_preview_ffmpeg_args(Path::new("clip.mp4"));
        let args: Vec<_> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert!(args.windows(2).any(|args| args == ["-stream_loop", "-1"]));
        assert!(args.windows(2).any(|args| args == ["-map", "0:v:0"]));
        assert!(args.contains(&"-re".to_owned()));
        assert!(args.contains(&"-an".to_owned()));
        assert!(args.contains(&"-sn".to_owned()));
        assert!(args.contains(&"-dn".to_owned()));
        assert!(args.windows(2).any(|args| args == ["-f", "rawvideo"]));
        assert!(args.windows(2).any(|args| args == ["-pix_fmt", "bgra"]));
        assert_eq!(args.last().map(String::as_str), Some("-"));
        assert!(!args.contains(&"-v".to_owned()));
    }

    #[test]
    fn raw_video_hover_preview_frame_rejects_unexpected_buffers() {
        assert!(
            raw_video_hover_preview_frame_from_output(OutputVideoFrame {
                width: 1,
                height: 1,
                pix_fmt: "rgba".to_owned(),
                output_index: 0,
                data: vec![0, 0, 0, 255],
                frame_num: 0,
                timestamp: 0.0,
            })
            .is_none()
        );
        assert!(
            raw_video_hover_preview_frame_from_output(OutputVideoFrame {
                width: 1,
                height: 1,
                pix_fmt: "bgra".to_owned(),
                output_index: 0,
                data: vec![0, 0, 0],
                frame_num: 0,
                timestamp: 0.0,
            })
            .is_none()
        );
        assert!(
            raw_video_hover_preview_frame_from_output(OutputVideoFrame {
                width: 1,
                height: 1,
                pix_fmt: "bgra".to_owned(),
                output_index: 0,
                data: vec![0, 0, 0, 255],
                frame_num: 0,
                timestamp: 0.0,
            })
            .is_some()
        );
    }
}
