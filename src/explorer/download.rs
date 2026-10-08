use std::{
    collections::VecDeque,
    env,
    ffi::{OsStr, OsString},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::{fs::PermissionsExt, process::CommandExt};
#[cfg(target_os = "windows")]
use std::os::windows::{
    io::{AsRawHandle, FromRawHandle, OwnedHandle},
    process::CommandExt,
};

#[cfg(target_os = "windows")]
use windows::{
    Win32::{
        Foundation::HANDLE,
        System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject,
        },
    },
    core::PCWSTR,
};

use futures::AsyncReadExt;
use gpui::{Context, http_client::HttpClient};
use serde::Deserialize;
use tempfile::NamedTempFile;

use crate::explorer::{
    clipboard::{ClipboardDownload, ClipboardVideoDownload},
    portable_devices,
    remote_dialog::{open_remote_credentials_dialog, open_remote_host_key_dialog},
    remote_download::{
        RemoteCredentials, RemoteDownloadError, RemoteHostKey, download_remote_to_temporary_file,
        embedded_credentials, endpoint_key, is_remote_download, remember_host_key,
    },
    view::{ExplorerView, ExplorerViewEvent, OperationNotice},
};

const DOWNLOAD_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const DOWNLOAD_SPEED_WINDOW: Duration = Duration::from_secs(5);
const DOWNLOAD_SPEED_MIN_INTERVAL: Duration = Duration::from_millis(500);
pub(super) const DOWNLOAD_BUFFER_SIZE: usize = 64 * 1024;
const YTDLP_ERROR_MESSAGE_LIMIT: usize = 4 * 1024;
const YTDLP_PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);
const YTDLP_DOWNLOAD_PROGRESS_PREFIX: &str = "__EXPLORER_YTDLP_DOWNLOAD_PROGRESS__";
const YTDLP_POSTPROCESS_PROGRESS_PREFIX: &str = "__EXPLORER_YTDLP_POSTPROCESS_PROGRESS__";
const YTDLP_OUTPUT_PREFIX: &str = "__EXPLORER_YTDLP_OUTPUT__";
const YTDLP_OUTPUT_TEMPLATE: &str = "after_move:__EXPLORER_YTDLP_OUTPUT__%(filepath)j";
const YTDLP_DOWNLOAD_PROGRESS_TEMPLATE: &str = "download:__EXPLORER_YTDLP_DOWNLOAD_PROGRESS__%(progress.{status,downloaded_bytes,total_bytes})j";
const YTDLP_POSTPROCESS_PROGRESS_TEMPLATE: &str =
    "postprocess:__EXPLORER_YTDLP_POSTPROCESS_PROGRESS__%(progress.{status})j";
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum DownloadNoticeKind {
    File,
    Video { site_domain: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DownloadNoticeRow {
    pub(super) id: u64,
    pub(super) kind: DownloadNoticeKind,
    pub(super) file_name: String,
    pub(super) destination: PathBuf,
    pub(super) status: DownloadNoticeStatus,
    pub(super) speed_tracker: DownloadSpeedTracker,
    pub(super) output_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct DownloadSpeedTracker {
    samples: VecDeque<(Instant, u64)>,
    has_progress: bool,
}

impl DownloadSpeedTracker {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn record(&mut self, captured_at: Instant, bytes: u64) {
        if let Some(&(last_time, last_bytes)) = self.samples.back() {
            if captured_at < last_time {
                return;
            }
            if bytes < last_bytes {
                self.reset();
            } else {
                self.has_progress |= bytes > last_bytes;
                // Bound storage for chunk-level events, preserving the initial baseline.
                if captured_at == last_time
                    || (self.samples.len() > 1
                        && captured_at.duration_since(self.samples[self.samples.len() - 2].0)
                            < DOWNLOAD_PROGRESS_INTERVAL)
                {
                    self.samples.pop_back();
                    if self.samples.is_empty() {
                        self.has_progress = false;
                    }
                }
            }
        }
        self.samples.push_back((captured_at, bytes));
        if let Some(cutoff) = captured_at.checked_sub(DOWNLOAD_SPEED_WINDOW) {
            // Retain one sample before the window for interpolation at its boundary.
            while self.samples.len() > 1 && self.samples[1].0 <= cutoff {
                self.samples.pop_front();
            }
        }
    }

    fn speed(&self, now: Instant) -> Option<f64> {
        let &(first_time, _) = self.samples.front()?;
        let &(last_time, last_bytes) = self.samples.back()?;
        if !self.has_progress
            || now.checked_duration_since(first_time)? < DOWNLOAD_SPEED_MIN_INTERVAL
            || now < last_time
        {
            return None;
        }
        let start = now
            .checked_sub(DOWNLOAD_SPEED_WINDOW)
            .unwrap_or(first_time)
            .max(first_time);
        if start >= last_time {
            return Some(0.0);
        }
        let mut baseline = *self.samples.front()?;
        let mut transferred = (last_bytes - baseline.1) as f64;
        for &(time, bytes) in self.samples.iter().skip(1) {
            if time <= start {
                baseline = (time, bytes);
                transferred = (last_bytes - bytes) as f64;
                continue;
            }
            let fraction = start.duration_since(baseline.0).as_secs_f64()
                / time.duration_since(baseline.0).as_secs_f64();
            transferred -= (bytes - baseline.1) as f64 * fraction;
            break;
        }
        // Include idle time after the latest event so stalled rates decay to zero.
        Some(transferred.max(0.0) / now.duration_since(start).as_secs_f64())
    }
}

impl DownloadNoticeRow {
    pub(super) fn record_progress(&mut self, progress: DownloadProgress, captured_at: Instant) {
        self.speed_tracker
            .record(captured_at, progress.downloaded_bytes);
        self.status = DownloadNoticeStatus::Downloading {
            downloaded_bytes: progress.downloaded_bytes,
            total_bytes: progress.total_bytes,
        };
    }

    fn set_status(&mut self, status: DownloadNoticeStatus) {
        self.speed_tracker.reset();
        self.status = status;
    }

    pub(super) fn transfer_metrics(&self, now: Instant) -> (Option<f64>, Option<Duration>) {
        let DownloadNoticeStatus::Downloading {
            downloaded_bytes,
            total_bytes,
        } = self.status
        else {
            return (None, None);
        };
        let speed = self.speed_tracker.speed(now);
        let remaining = total_bytes.zip(speed).and_then(|(total, speed)| {
            (speed > 0.0)
                .then(|| {
                    Duration::try_from_secs_f64(
                        total.saturating_sub(downloaded_bytes) as f64 / speed,
                    )
                    .ok()
                })
                .flatten()
        });
        (speed, remaining)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum DownloadNoticeStatus {
    Connecting,
    WaitingForCredentials,
    WaitingForHostConfirmation,
    Downloading {
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Completed,
    Failed(String),
}

impl DownloadNoticeStatus {
    pub(super) fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Connecting
                | Self::WaitingForCredentials
                | Self::WaitingForHostConfirmation
                | Self::Downloading { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DownloadProgress {
    pub(super) downloaded_bytes: u64,
    pub(super) total_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum YtDlpProgressEvent {
    Downloading(DownloadProgress),
    Finished(DownloadProgress),
    PostProcessing,
}

#[derive(Debug, Deserialize)]
struct YtDlpProgressRecord {
    status: String,
    downloaded_bytes: Option<u64>,
    total_bytes: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct YtDlpPostProcessRecord {
    status: String,
}

pub(super) struct PendingDownload {
    pub(super) temporary: NamedTempFile,
    pub(super) destination: PathBuf,
    pub(super) file_name: String,
}

pub(super) struct ActiveRemoteDownload {
    pub(super) id: u64,
    pub(super) download: ClipboardDownload,
    pub(super) credentials: Option<RemoteCredentials>,
    pub(super) destination: PathBuf,
    pub(super) cancel: Arc<AtomicBool>,
}

impl Drop for ActiveRemoteDownload {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

#[derive(Debug)]
pub(super) enum DownloadResult {
    File(PathBuf),
    Video(Vec<PathBuf>),
}

pub(super) struct YtDlpProcessControl {
    shared: Arc<YtDlpProcessState>,
}

struct YtDlpProcessState {
    cancelled: AtomicBool,
    child: Mutex<Option<YtDlpChild>>,
}

struct YtDlpChild {
    child: Child,
    #[cfg(target_os = "windows")]
    job: WindowsJob,
}

#[cfg(target_os = "windows")]
struct WindowsJob(OwnedHandle);

impl YtDlpProcessControl {
    fn new() -> Self {
        Self {
            shared: Arc::new(YtDlpProcessState {
                cancelled: AtomicBool::new(false),
                child: Mutex::new(None),
            }),
        }
    }

    fn shared(&self) -> Arc<YtDlpProcessState> {
        self.shared.clone()
    }

    pub(super) fn cancel(&self) {
        self.shared.cancel();
    }
}

impl Drop for YtDlpProcessControl {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl YtDlpProcessState {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Ok(mut child) = self.child.lock()
            && let Some(child) = child.as_mut()
        {
            child.terminate();
        }
    }

    fn register_child(&self, child: YtDlpChild) -> Result<(), String> {
        let mut slot = self
            .child
            .lock()
            .map_err(|_| "Could not track the yt-dlp process.".to_owned())?;
        *slot = Some(child);
        if self.cancelled.load(Ordering::Relaxed)
            && let Some(child) = slot.as_mut()
        {
            child.terminate();
        }
        Ok(())
    }

    fn poll_exit(&self) -> Result<Option<ExitStatus>, String> {
        let mut slot = self
            .child
            .lock()
            .map_err(|_| "Could not access the yt-dlp process.".to_owned())?;
        let Some(child) = slot.as_mut() else {
            return Err("The yt-dlp process was not available.".to_owned());
        };
        match child.child.try_wait() {
            Ok(Some(status)) => {
                slot.take();
                Ok(Some(status))
            }
            Ok(None) => Ok(None),
            Err(error) => Err(format!("Could not wait for yt-dlp: {error}")),
        }
    }
}

impl YtDlpChild {
    fn spawn(command: &mut Command) -> Result<Self, String> {
        #[cfg(unix)]
        command.process_group(0);

        #[cfg(target_os = "windows")]
        let job = WindowsJob::new()?;

        let mut child = command
            .spawn()
            .map_err(|error| format!("Could not start yt-dlp: {error}"))?;

        #[cfg(target_os = "windows")]
        if let Err(error) = job.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }

        Ok(Self {
            child,
            #[cfg(target_os = "windows")]
            job,
        })
    }

    fn terminate(&mut self) {
        #[cfg(unix)]
        {
            let process_group = i32::try_from(self.child.id()).ok().map(|pid| -pid);
            let killed_group =
                process_group.is_some_and(|group| unsafe { libc::kill(group, libc::SIGKILL) == 0 });
            if !killed_group {
                let _ = self.child.kill();
            }
        }

        #[cfg(target_os = "windows")]
        if self.job.terminate().is_err() {
            let _ = self.child.kill();
        }

        #[cfg(not(any(unix, target_os = "windows")))]
        let _ = self.child.kill();
    }
}

#[cfg(target_os = "windows")]
impl WindowsJob {
    fn new() -> Result<Self, String> {
        let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
            .map_err(|error| format!("Could not create a yt-dlp process job: {error}"))?;
        let job = Self(unsafe { OwnedHandle::from_raw_handle(handle.0) });
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        unsafe {
            SetInformationJobObject(
                job.handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        }
        .map_err(|error| format!("Could not configure the yt-dlp process job: {error}"))?;
        Ok(job)
    }

    fn assign(&self, child: &Child) -> Result<(), String> {
        let process = HANDLE(child.as_raw_handle());
        unsafe { AssignProcessToJobObject(self.handle(), process) }
            .map_err(|error| format!("Could not track the yt-dlp process tree: {error}"))
    }

    fn terminate(&self) -> windows::core::Result<()> {
        unsafe { TerminateJobObject(self.handle(), 1) }
    }

    fn handle(&self) -> HANDLE {
        HANDLE(self.0.as_raw_handle())
    }
}

impl PendingDownload {
    fn persist(mut self) -> Result<DownloadResult, String> {
        let _cache_invalidation =
            crate::explorer::remote_directory_cache::DirectoryMutation::new([self
                .destination
                .join(&self.file_name)]);
        let mut index = 1usize;
        loop {
            let file_name = download_file_name(&self.file_name, index);
            let path = self.destination.join(&file_name);
            match self.temporary.persist_noclobber(&path) {
                Ok(_) => return Ok(DownloadResult::File(path)),
                Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                    self.temporary = error.file;
                    index = index.checked_add(1).ok_or_else(|| {
                        format!(
                            "Could not save \"{}\": too many existing names",
                            self.file_name
                        )
                    })?;
                }
                Err(error) => {
                    return Err(format!("Could not save \"{file_name}\": {}", error.error));
                }
            }
        }
    }
}

impl ExplorerView {
    pub(super) fn start_clipboard_download(
        &mut self,
        download: ClipboardDownload,
        cx: &mut Context<Self>,
    ) {
        if portable_devices::is_portable_path(&self.path) || !self.path.is_dir() {
            self.set_error_notice("Could not download to this location.".to_owned());
            return;
        }

        if is_remote_download(&download) {
            if download.url.scheme() == "sftp" {
                match super::remote_fs::RemoteLocation::parse(download.url.as_str()) {
                    Ok(location) => self.start_native_transfer(
                        vec![location.provider_path()],
                        self.path.clone(),
                        false,
                        cx,
                    ),
                    Err(error) => {
                        self.set_error_notice(error);
                        cx.notify();
                    }
                }
                return;
            }
            self.enqueue_remote_download(download, cx);
            return;
        }

        self.begin_download_batch_if_needed();

        let id = self.next_download_id;
        self.next_download_id = self.next_download_id.wrapping_add(1);
        let destination = self.path.clone();
        self.download_notice_rows.push(DownloadNoticeRow {
            speed_tracker: Default::default(),
            output_paths: Vec::new(),
            id,
            kind: DownloadNoticeKind::File,
            file_name: download.file_name.clone(),
            destination: destination.clone(),
            status: DownloadNoticeStatus::Connecting,
        });
        self.request_transfer_panel_expansion(cx);

        let client = cx.http_client();
        let (progress_tx, progress_rx) = mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let task = cx.spawn({
            let finished = finished.clone();
            async move |this, cx| {
                let operation_task = cx.background_executor().spawn({
                    let finished = finished.clone();
                    async move {
                        let result = download_url_to_temporary_file(
                            client,
                            download,
                            &destination,
                            |progress| {
                                let _ = progress_tx.send((progress, Instant::now()));
                            },
                        )
                        .await;
                        finished.store(true, Ordering::Relaxed);
                        result
                    }
                });

                while !finished.load(Ordering::Relaxed) {
                    cx.background_executor()
                        .timer(DOWNLOAD_PROGRESS_INTERVAL)
                        .await;
                    Self::drain_download_progress(&this, cx, id, &progress_rx);
                }

                let result = operation_task.await.and_then(PendingDownload::persist);
                Self::drain_download_progress(&this, cx, id, &progress_rx);
                let _ = this.update(cx, |explorer, cx| {
                    explorer.complete_download(id, result, cx);
                    cx.notify();
                });
            }
        });
        self.download_tasks.push((id, task));
        cx.notify();
    }

    fn enqueue_remote_download(&mut self, download: ClipboardDownload, cx: &mut Context<Self>) {
        self.begin_download_batch_if_needed();
        self.pending_remote_downloads
            .push_back((download, self.path.clone()));
        if self.active_remote_download.is_none() {
            self.start_next_remote_download(cx);
        }
    }

    fn start_next_remote_download(&mut self, cx: &mut Context<Self>) {
        let Some((download, destination)) = self.pending_remote_downloads.pop_front() else {
            self.remote_credentials.clear();
            self.finish_download_batch_if_idle(cx);
            cx.notify();
            return;
        };

        let id = self.next_download_id;
        self.next_download_id = self.next_download_id.wrapping_add(1);
        self.download_notice_rows.push(DownloadNoticeRow {
            speed_tracker: Default::default(),
            output_paths: Vec::new(),
            id,
            kind: DownloadNoticeKind::File,
            file_name: download.file_name.clone(),
            destination: destination.clone(),
            status: DownloadNoticeStatus::Connecting,
        });
        self.request_transfer_panel_expansion(cx);
        let credentials = embedded_credentials(&download).or_else(|| {
            endpoint_key(&download).and_then(|key| self.remote_credentials.get(&key).cloned())
        });
        self.active_remote_download = Some(ActiveRemoteDownload {
            id,
            download,
            credentials,
            destination,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        self.start_active_remote_attempt(cx);
    }

    fn start_active_remote_attempt(&mut self, cx: &mut Context<Self>) {
        let Some(active) = self.active_remote_download.as_ref() else {
            return;
        };
        let id = active.id;
        let download = active.download.clone();
        let credentials = active.credentials.clone();
        let destination = active.destination.clone();
        let cancel = active.cancel.clone();
        if let Some(row) = self
            .download_notice_rows
            .iter_mut()
            .find(|row| row.id == id)
        {
            row.set_status(DownloadNoticeStatus::Connecting);
        }
        self.remove_download_task(id);

        let (progress_tx, progress_rx) = mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let task = cx.spawn({
            let finished = finished.clone();
            async move |this, cx| {
                let operation_task = cx.background_executor().spawn({
                    let finished = finished.clone();
                    async move {
                        let result = download_remote_to_temporary_file(
                            download,
                            credentials,
                            &destination,
                            cancel,
                            |progress| {
                                let _ = progress_tx.send((progress, Instant::now()));
                            },
                        );
                        finished.store(true, Ordering::Relaxed);
                        result
                    }
                });

                while !finished.load(Ordering::Relaxed) {
                    cx.background_executor()
                        .timer(DOWNLOAD_PROGRESS_INTERVAL)
                        .await;
                    Self::drain_download_progress(&this, cx, id, &progress_rx);
                }

                let result = operation_task.await;
                Self::drain_download_progress(&this, cx, id, &progress_rx);
                let _ = this.update(cx, |explorer, cx| {
                    explorer.finish_remote_attempt(id, result, cx);
                    cx.notify();
                });
            }
        });
        self.download_tasks.push((id, task));
        cx.notify();
    }

    fn finish_remote_attempt(
        &mut self,
        id: u64,
        result: Result<PendingDownload, RemoteDownloadError>,
        cx: &mut Context<Self>,
    ) {
        if self.active_remote_download.as_ref().map(|active| active.id) != Some(id) {
            return;
        }
        self.remove_download_task(id);
        match result {
            Ok(download) => {
                self.complete_download(id, download.persist(), cx);
                self.finish_active_remote_download(cx);
            }
            Err(RemoteDownloadError::Fatal(error)) => {
                self.complete_download(id, Err(error), cx);
                self.finish_active_remote_download(cx);
            }
            Err(RemoteDownloadError::CredentialsRequired {
                host,
                username,
                message,
            }) => {
                if let Some(row) = self
                    .download_notice_rows
                    .iter_mut()
                    .find(|row| row.id == id)
                {
                    row.set_status(DownloadNoticeStatus::WaitingForCredentials);
                }
                self.request_transfer_panel_expansion(cx);
                match open_remote_credentials_dialog(
                    cx.entity(),
                    id,
                    host,
                    username,
                    message,
                    false,
                    cx,
                ) {
                    Ok(handle) => self.active_dialog_window = Some(handle),
                    Err(error) => {
                        self.complete_download(
                            id,
                            Err(format!("Could not open the sign-in dialog: {error}")),
                            cx,
                        );
                        self.finish_active_remote_download(cx);
                    }
                }
            }
            Err(RemoteDownloadError::PassphraseRequired {
                host,
                username,
                key_path,
            }) => {
                if let Some(row) = self
                    .download_notice_rows
                    .iter_mut()
                    .find(|row| row.id == id)
                {
                    row.set_status(DownloadNoticeStatus::WaitingForCredentials);
                }
                self.request_transfer_panel_expansion(cx);
                match open_remote_credentials_dialog(
                    cx.entity(),
                    id,
                    host,
                    username,
                    Some(format!("Unlock private key {}", key_path.display())),
                    true,
                    cx,
                ) {
                    Ok(handle) => self.active_dialog_window = Some(handle),
                    Err(error) => {
                        self.complete_download(id, Err(error), cx);
                        self.finish_active_remote_download(cx);
                    }
                }
            }
            Err(RemoteDownloadError::UnknownHost(key)) => {
                if let Some(row) = self
                    .download_notice_rows
                    .iter_mut()
                    .find(|row| row.id == id)
                {
                    row.set_status(DownloadNoticeStatus::WaitingForHostConfirmation);
                }
                self.request_transfer_panel_expansion(cx);
                match open_remote_host_key_dialog(cx.entity(), id, *key, cx) {
                    Ok(handle) => self.active_dialog_window = Some(handle),
                    Err(error) => {
                        self.complete_download(
                            id,
                            Err(format!("Could not open the host confirmation: {error}")),
                            cx,
                        );
                        self.finish_active_remote_download(cx);
                    }
                }
            }
        }
    }

    pub(super) fn submit_remote_credentials(
        &mut self,
        id: u64,
        credentials: RemoteCredentials,
        cx: &mut Context<Self>,
    ) {
        if super::remote_fs::reply(
            id,
            super::remote_fs::PromptReply::Credentials(credentials.clone()),
        ) {
            self.clear_active_dialog_window();
            return;
        }
        let Some(active) = self
            .active_remote_download
            .as_mut()
            .filter(|active| active.id == id)
        else {
            return;
        };
        if let Some(key) = endpoint_key(&active.download) {
            self.remote_credentials.insert(key, credentials.clone());
        }
        active.credentials = Some(credentials);
        self.clear_active_dialog_window();
        self.start_active_remote_attempt(cx);
    }

    pub(super) fn confirm_remote_host_key(
        &mut self,
        id: u64,
        key: RemoteHostKey,
        cx: &mut Context<Self>,
    ) {
        if id >= (1 << 63) {
            match remember_host_key(&key) {
                Ok(()) => {
                    super::remote_fs::reply(id, super::remote_fs::PromptReply::Accept);
                }
                Err(error) => {
                    super::remote_fs::reply(id, super::remote_fs::PromptReply::Cancel);
                    self.set_error_notice(error);
                }
            }
            self.clear_active_dialog_window();
            return;
        }
        if self.active_remote_download.as_ref().map(|active| active.id) != Some(id) {
            return;
        }
        self.clear_active_dialog_window();
        match remember_host_key(&key) {
            Ok(()) => self.start_active_remote_attempt(cx),
            Err(error) => {
                self.complete_download(id, Err(error), cx);
                self.finish_active_remote_download(cx);
            }
        }
    }

    pub(super) fn cancel_remote_prompt(&mut self, id: u64, cx: &mut Context<Self>) {
        if super::remote_fs::reply(id, super::remote_fs::PromptReply::Cancel) {
            self.clear_active_dialog_window();
            return;
        }
        if self.active_remote_download.as_ref().map(|active| active.id) != Some(id) {
            return;
        }
        self.clear_active_dialog_window();
        self.complete_download(id, Err("Download cancelled.".to_owned()), cx);
        self.finish_active_remote_download(cx);
    }

    fn finish_active_remote_download(&mut self, cx: &mut Context<Self>) {
        self.active_remote_download = None;
        self.start_next_remote_download(cx);
    }

    fn remove_download_task(&mut self, id: u64) {
        if let Some(index) = self
            .download_tasks
            .iter()
            .position(|(task_id, _)| *task_id == id)
        {
            let (_, task) = self.download_tasks.swap_remove(index);
            drop(task);
        }
    }

    pub(super) fn start_video_downloads(
        &mut self,
        downloads: Vec<ClipboardVideoDownload>,
        cx: &mut Context<Self>,
    ) {
        if portable_devices::is_portable_path(&self.path) || !self.path.is_dir() {
            self.set_error_notice("Could not download to this location.");
            return;
        }

        let Some(executable) = ytdlp_executable_from_path() else {
            self.set_error_notice("Could not download video: yt-dlp was not found in PATH.");
            return;
        };
        let options = cx
            .try_global::<crate::settings::SettingsState>()
            .map(|settings| settings.value.app.ytdlp_options.clone())
            .unwrap_or_default();

        for download in downloads {
            self.start_video_download(download, executable.clone(), options.clone(), cx);
        }
    }

    fn start_video_download(
        &mut self,
        download: ClipboardVideoDownload,
        executable: PathBuf,
        options: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.begin_download_batch_if_needed();

        let ClipboardVideoDownload { url, site_domain } = download;
        let id = self.next_download_id;
        self.next_download_id = self.next_download_id.wrapping_add(1);
        let destination = self.path.clone();
        self.download_notice_rows.push(DownloadNoticeRow {
            speed_tracker: Default::default(),
            output_paths: Vec::new(),
            id,
            kind: DownloadNoticeKind::Video {
                site_domain: site_domain.clone(),
            },
            file_name: format!("Video from {site_domain}"),
            destination: destination.clone(),
            status: DownloadNoticeStatus::Connecting,
        });
        self.request_transfer_panel_expansion(cx);

        let command = ytdlp_command_spec(executable, options, url.as_str(), destination);
        let process_control = YtDlpProcessControl::new();
        let process_state = process_control.shared();
        self.ytdlp_process_controls.push((id, process_control));
        let (progress_tx, progress_rx) = mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let task = cx.spawn({
            let finished = finished.clone();
            async move |this, cx| {
                let operation_task = cx.background_executor().spawn({
                    let finished = finished.clone();
                    async move {
                        let result = run_ytdlp_download(command, process_state, move |event| {
                            let _ = progress_tx.send((event, Instant::now()));
                        });
                        finished.store(true, Ordering::Relaxed);
                        result
                    }
                });

                while !finished.load(Ordering::Relaxed) {
                    cx.background_executor()
                        .timer(DOWNLOAD_PROGRESS_INTERVAL)
                        .await;
                    Self::drain_ytdlp_progress(&this, cx, id, &progress_rx);
                }

                let result = operation_task.await;
                Self::drain_ytdlp_progress(&this, cx, id, &progress_rx);
                let _ = this.update(cx, |explorer, cx| {
                    explorer.remove_ytdlp_process_control(id);
                    explorer.complete_download(id, result, cx);
                    cx.notify();
                });
            }
        });
        self.download_tasks.push((id, task));
        cx.notify();
    }

    fn begin_download_batch_if_needed(&mut self) {
        self.cancel_transfer_completion_cleanup();
        if self.download_batch_active {
            return;
        }
        self.download_batch_active = true;
        self.download_tasks.clear();
        self.ytdlp_process_controls.clear();
        self.download_batch_succeeded = 0;
        self.download_batch_failed = 0;
        self.download_batch_last_error = None;
        self.clear_operation_notice();
    }

    pub(super) fn cancel_download(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(row_index) = self
            .download_notice_rows
            .iter()
            .position(|row| row.id == id && row.status.is_active())
        else {
            return;
        };

        if let Some(control) = self.take_ytdlp_process_control(id) {
            control.cancel();
        }
        self.download_notice_rows.remove(row_index);
        self.finish_download_reveal(id, Vec::new(), cx);
        self.remove_download_task(id);
        if self.active_remote_download.as_ref().map(|active| active.id) == Some(id) {
            if let Some(handle) = self.active_dialog_window.take() {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
            self.active_remote_download = None;
            self.start_next_remote_download(cx);
            return;
        }

        self.finish_download_batch_if_idle(cx);
        cx.notify();
    }

    fn take_ytdlp_process_control(&mut self, id: u64) -> Option<YtDlpProcessControl> {
        let index = self
            .ytdlp_process_controls
            .iter()
            .position(|(download_id, _)| *download_id == id)?;
        Some(self.ytdlp_process_controls.swap_remove(index).1)
    }

    fn remove_ytdlp_process_control(&mut self, id: u64) {
        drop(self.take_ytdlp_process_control(id));
    }

    fn drain_download_progress(
        this: &gpui::WeakEntity<Self>,
        cx: &mut gpui::AsyncApp,
        id: u64,
        progress_rx: &mpsc::Receiver<(DownloadProgress, Instant)>,
    ) {
        let _ = this.update(cx, |explorer, cx| {
            if let Some(row) = explorer
                .download_notice_rows
                .iter_mut()
                .find(|row| row.id == id)
            {
                for (progress, captured_at) in progress_rx.try_iter() {
                    row.record_progress(progress, captured_at);
                }
                if matches!(row.status, DownloadNoticeStatus::Downloading { .. }) {
                    cx.notify();
                }
            }
        });
    }

    fn drain_ytdlp_progress(
        this: &gpui::WeakEntity<Self>,
        cx: &mut gpui::AsyncApp,
        id: u64,
        progress_rx: &mpsc::Receiver<(YtDlpProgressEvent, Instant)>,
    ) {
        let _ = this.update(cx, |explorer, cx| {
            let mut changed = false;
            for (event, captured_at) in progress_rx.try_iter() {
                changed |= explorer.apply_ytdlp_progress_event(id, event, captured_at);
            }
            if changed
                || explorer.download_notice_rows.iter().any(|row| {
                    row.id == id && matches!(row.status, DownloadNoticeStatus::Downloading { .. })
                })
            {
                cx.notify();
            }
        });
    }

    fn apply_ytdlp_progress_event(
        &mut self,
        id: u64,
        event: YtDlpProgressEvent,
        captured_at: Instant,
    ) -> bool {
        let Some(row) = self
            .download_notice_rows
            .iter_mut()
            .find(|row| row.id == id)
        else {
            return false;
        };
        match event {
            YtDlpProgressEvent::Downloading(progress) => row.record_progress(progress, captured_at),
            YtDlpProgressEvent::Finished(progress) => {
                row.record_progress(progress, captured_at);
                row.speed_tracker.reset();
            }
            YtDlpProgressEvent::PostProcessing => row.set_status(DownloadNoticeStatus::Connecting),
        }
        true
    }

    pub(super) fn complete_download(
        &mut self,
        id: u64,
        result: Result<DownloadResult, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(row_index) = self
            .download_notice_rows
            .iter()
            .position(|row| row.id == id)
        else {
            return;
        };

        let mut output_paths = Vec::new();
        match result {
            Ok(DownloadResult::File(path)) => {
                self.download_batch_succeeded += 1;
                let final_name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| self.download_notice_rows[row_index].file_name.clone());
                self.download_notice_rows[row_index].file_name = final_name;
                output_paths.push(path.clone());
                self.download_notice_rows[row_index].set_status(DownloadNoticeStatus::Completed);
                if path.parent() == Some(self.path.as_path()) {
                    self.reload_with_entry_metadata_resolution(cx);
                }
                self.emit_filesystem_changed(cx);
            }
            Ok(DownloadResult::Video(paths)) => {
                output_paths = paths;
                self.download_batch_succeeded += 1;
                self.download_notice_rows[row_index].set_status(DownloadNoticeStatus::Completed);
                self.reload_with_entry_metadata_resolution(cx);
                self.emit_filesystem_changed(cx);
            }
            Err(error) => {
                self.download_batch_failed += 1;
                self.download_batch_last_error = Some(error.clone());
                self.download_notice_rows[row_index]
                    .set_status(DownloadNoticeStatus::Failed(error));
                self.request_transfer_panel_expansion(cx);
            }
        }

        self.download_notice_rows[row_index].output_paths = output_paths.clone();
        self.finish_download_reveal(id, output_paths, cx);
        self.finish_download_batch_if_idle(cx);
    }

    fn finish_download_reveal(&mut self, id: u64, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.complete_pending_download_reveal(cx.entity_id(), id, &paths, cx);
        // FilesystemChanged is emitted first, so other panes finish their normal
        // refresh before the selection reload requested by this event.
        cx.emit(ExplorerViewEvent::DownloadFinished { id, paths });
    }

    fn finish_download_batch_if_idle(&mut self, cx: &mut Context<Self>) {
        if self.active_remote_download.is_some() || !self.pending_remote_downloads.is_empty() {
            return;
        }
        if self
            .download_notice_rows
            .iter()
            .any(|row| row.status.is_active())
        {
            return;
        }

        let succeeded = self.download_batch_succeeded;
        let failed = self.download_batch_failed;
        let last_error = self.download_batch_last_error.clone().unwrap_or_default();
        self.download_batch_active = false;
        self.download_notice_rows
            .retain(|row| !matches!(row.status, DownloadNoticeStatus::Failed(_)));
        if succeeded == 0 && failed == 0 {
            self.operation_notice = None;
            self.update_transfer_completion_retention(cx);
            return;
        }
        self.operation_notice = if failed == 0 {
            None
        } else {
            let text = match (succeeded, failed) {
                (0, 1) => format!("Download failed: {last_error}"),
                (0, failed) => format!("{failed} downloads failed: {last_error}"),
                (succeeded, failed) => {
                    format!("Downloaded {succeeded} files; {failed} failed: {last_error}")
                }
            };
            Some(OperationNotice::error(text))
        };
        self.update_transfer_completion_retention(cx);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct YtDlpCommandSpec {
    executable: PathBuf,
    args: Vec<OsString>,
    current_dir: PathBuf,
}

fn ytdlp_command_spec(
    executable: PathBuf,
    options: Vec<String>,
    url: &str,
    current_dir: PathBuf,
) -> YtDlpCommandSpec {
    let mut args = options.into_iter().map(OsString::from).collect::<Vec<_>>();
    args.push(OsString::from("--no-playlist"));
    args.push(OsString::from("--print"));
    args.push(OsString::from(YTDLP_OUTPUT_TEMPLATE));
    args.push(OsString::from("--no-quiet"));
    args.push(OsString::from("--newline"));
    args.push(OsString::from("--progress"));
    args.push(OsString::from("--progress-delta"));
    args.push(OsString::from("0.1"));
    args.push(OsString::from("--progress-template"));
    args.push(OsString::from(YTDLP_DOWNLOAD_PROGRESS_TEMPLATE));
    args.push(OsString::from("--progress-template"));
    args.push(OsString::from(YTDLP_POSTPROCESS_PROGRESS_TEMPLATE));
    args.push(OsString::from("--"));
    args.push(OsString::from(url));
    YtDlpCommandSpec {
        executable,
        args,
        current_dir,
    }
}

fn run_ytdlp_download(
    command_spec: YtDlpCommandSpec,
    process: Arc<YtDlpProcessState>,
    on_progress: impl Fn(YtDlpProgressEvent) + Send + 'static,
) -> Result<DownloadResult, String> {
    let mut command = Command::new(crate::os_paths::native_path(&command_spec.executable));
    let _cache_invalidation =
        crate::explorer::remote_directory_cache::DirectoryMutation::new([command_spec
            .current_dir
            .clone()]);
    command
        .args(&command_spec.args)
        .current_dir(crate::os_paths::native_path(&command_spec.current_dir))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);

    run_ytdlp_command(&mut command, process, on_progress).map(|result| match result {
        DownloadResult::Video(paths) => {
            DownloadResult::Video(resolve_ytdlp_output_paths(paths, &command_spec.current_dir))
        }
        other => other,
    })
}

fn run_ytdlp_command(
    command: &mut Command,
    process: Arc<YtDlpProcessState>,
    on_progress: impl Fn(YtDlpProgressEvent) + Send + 'static,
) -> Result<DownloadResult, String> {
    let mut child = YtDlpChild::spawn(command)?;
    let stdout = child.child.stdout.take();
    let stderr = child.child.stderr.take();
    process.register_child(child)?;

    let stdout_reader =
        stdout.map(|stdout| thread::spawn(move || read_ytdlp_progress(stdout, on_progress)));
    let stderr_reader = stderr
        .map(|stderr| thread::spawn(move || read_bounded_tail(stderr, YTDLP_ERROR_MESSAGE_LIMIT)));
    let status = loop {
        if let Some(status) = process.poll_exit()? {
            break status;
        }
        thread::sleep(YTDLP_PROCESS_POLL_INTERVAL);
    };
    let output_paths = stdout_reader
        .map(|reader| {
            reader
                .join()
                .map_err(|_| "Could not read yt-dlp progress output.".to_owned())?
                .map_err(|error| format!("Could not read yt-dlp progress output: {error}"))
        })
        .transpose()?
        .unwrap_or_default();
    let stderr = stderr_reader
        .map(|reader| {
            reader
                .join()
                .map_err(|_| "Could not read yt-dlp error output.".to_owned())?
                .map_err(|error| format!("Could not read yt-dlp error output: {error}"))
        })
        .transpose()?
        .unwrap_or_default();
    ytdlp_result_from_process(status.success(), &status.to_string(), &stderr)
        .map(|_| DownloadResult::Video(output_paths))
}

fn read_ytdlp_progress(
    reader: impl Read,
    on_progress: impl Fn(YtDlpProgressEvent),
) -> io::Result<Vec<PathBuf>> {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    let mut output_paths = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(output_paths);
        }
        let line = String::from_utf8_lossy(&line);
        if let Some(path) = ytdlp_output_path_from_line(&line) {
            if !output_paths.contains(&path) {
                output_paths.push(path);
            }
        } else if let Some(event) = ytdlp_progress_event_from_line(&line) {
            on_progress(event);
        }
    }
}

fn ytdlp_output_path_from_line(line: &str) -> Option<PathBuf> {
    let value = line.trim().strip_prefix(YTDLP_OUTPUT_PREFIX)?;
    let path = serde_json::from_str::<String>(value).ok()?;
    (!path.is_empty() && !path.contains('\0') && path != "-" && path != "NA")
        .then(|| PathBuf::from(path))
}

fn resolve_ytdlp_output_paths(paths: Vec<PathBuf>, directory: &Path) -> Vec<PathBuf> {
    let mut resolved = Vec::new();
    for path in paths {
        let path = if path.is_absolute() {
            path
        } else {
            directory.join(path)
        };
        if !resolved.contains(&path) {
            resolved.push(path);
        }
    }
    resolved
}

fn ytdlp_progress_event_from_line(line: &str) -> Option<YtDlpProgressEvent> {
    if let Some(prefix) = line.find(YTDLP_DOWNLOAD_PROGRESS_PREFIX) {
        let record = serde_json::from_str::<YtDlpProgressRecord>(
            line[prefix + YTDLP_DOWNLOAD_PROGRESS_PREFIX.len()..].trim(),
        )
        .ok()?;
        if !matches!(record.status.as_str(), "downloading" | "finished") {
            return None;
        }
        let downloaded_bytes = record.downloaded_bytes.or_else(|| {
            (record.status == "finished")
                .then_some(record.total_bytes)
                .flatten()
        })?;
        let progress = DownloadProgress {
            downloaded_bytes,
            total_bytes: record.total_bytes,
        };
        return Some(if record.status == "finished" {
            YtDlpProgressEvent::Finished(progress)
        } else {
            YtDlpProgressEvent::Downloading(progress)
        });
    }

    let prefix = line.find(YTDLP_POSTPROCESS_PROGRESS_PREFIX)?;
    let record = serde_json::from_str::<YtDlpPostProcessRecord>(
        line[prefix + YTDLP_POSTPROCESS_PROGRESS_PREFIX.len()..].trim(),
    )
    .ok()?;
    matches!(
        record.status.as_str(),
        "started" | "processing" | "finished"
    )
    .then_some(YtDlpProgressEvent::PostProcessing)
}

fn ytdlp_result_from_process(success: bool, status: &str, stderr: &[u8]) -> Result<(), String> {
    if success {
        return Ok(());
    }

    let stderr = bounded_ytdlp_message(stderr);
    if stderr.is_empty() {
        Err(format!("yt-dlp exited with {status}."))
    } else {
        Err(format!("yt-dlp exited with {status}: {stderr}"))
    }
}

fn read_bounded_tail(mut reader: impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut tail = Vec::with_capacity(limit);
    let mut buffer = [0u8; 4096];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(tail);
        }
        tail.extend_from_slice(&buffer[..read]);
        if tail.len() > limit {
            tail.drain(..tail.len() - limit);
        }
    }
}

fn bounded_ytdlp_message(bytes: &[u8]) -> String {
    let output = String::from_utf8_lossy(bytes);
    let message = output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_owned();
    if message.len() <= YTDLP_ERROR_MESSAGE_LIMIT {
        return message;
    }

    let mut start = message.len() - YTDLP_ERROR_MESSAGE_LIMIT;
    while !message.is_char_boundary(start) {
        start += 1;
    }
    format!("â€¦{}", &message[start..])
}

fn ytdlp_executable_from_path() -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    let extensions = ytdlp_path_extensions();
    resolve_ytdlp_executable_with(&path_var, &extensions, executable_file_is_usable)
}

fn resolve_ytdlp_executable_with(
    path_var: &OsStr,
    extensions: &[OsString],
    mut is_usable: impl FnMut(&Path) -> bool,
) -> Option<PathBuf> {
    for directory in env::split_paths(path_var) {
        let direct = directory.join("yt-dlp");
        if is_usable(&direct) {
            return Some(direct);
        }
        for extension in extensions {
            let candidate = directory.join(format!("yt-dlp{}", extension.to_string_lossy()));
            if is_usable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn ytdlp_path_extensions() -> Vec<OsString> {
    env::var_os("PATHEXT")
        .unwrap_or_else(|| OsString::from(".COM;.EXE;.BAT;.CMD"))
        .to_string_lossy()
        .split(';')
        .map(str::trim)
        .filter(|extension| !extension.is_empty())
        .map(OsString::from)
        .collect()
}

#[cfg(not(target_os = "windows"))]
fn ytdlp_path_extensions() -> Vec<OsString> {
    Vec::new()
}

fn executable_file_is_usable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

async fn download_url_to_temporary_file(
    client: Arc<dyn HttpClient>,
    download: ClipboardDownload,
    destination: &Path,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<PendingDownload, String> {
    let url = download.url.as_str();
    let mut response = client
        .get(url, ().into(), true)
        .await
        .map_err(|error| format!("Could not download \"{}\": {error}", download.file_name))?;
    if !response.status().is_success() {
        return Err(format!(
            "Could not download \"{}\": HTTP {}",
            download.file_name,
            response.status()
        ));
    }

    let total_bytes = response
        .headers()
        .get(gpui::http_client::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    on_progress(DownloadProgress {
        downloaded_bytes: 0,
        total_bytes,
    });

    let mut temporary = NamedTempFile::new_in(destination).map_err(|error| {
        format!(
            "Could not create a temporary file for \"{}\": {error}",
            download.file_name
        )
    })?;
    let mut buffer = vec![0; DOWNLOAD_BUFFER_SIZE];
    let mut downloaded_bytes = 0u64;
    loop {
        let read = response
            .body_mut()
            .read(&mut buffer)
            .await
            .map_err(|error| format!("Could not download \"{}\": {error}", download.file_name))?;
        if read == 0 {
            break;
        }
        temporary
            .as_file_mut()
            .write_all(&buffer[..read])
            .map_err(|error| format!("Could not save \"{}\": {error}", download.file_name))?;
        downloaded_bytes = downloaded_bytes.saturating_add(read as u64);
        on_progress(DownloadProgress {
            downloaded_bytes,
            total_bytes,
        });
    }

    if total_bytes.is_some_and(|total| total != downloaded_bytes) {
        return Err(format!(
            "Could not download \"{}\": expected {} bytes but received {}",
            download.file_name,
            total_bytes.unwrap_or_default(),
            downloaded_bytes
        ));
    }
    temporary
        .as_file_mut()
        .flush()
        .map_err(|error| format!("Could not save \"{}\": {error}", download.file_name))?;

    Ok(PendingDownload {
        temporary,
        destination: destination.to_path_buf(),
        file_name: download.file_name,
    })
}

fn download_file_name(file_name: &str, index: usize) -> String {
    if index == 1 {
        return file_name.to_owned();
    }
    let extension_dot = file_name
        .rfind('.')
        .expect("validated download names always have an extension");
    format!(
        "{} ({index}){}",
        &file_name[..extension_dot],
        &file_name[extension_dot..]
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use gpui::{
        AppContext, ClipboardItem, TestAppContext,
        http_client::{AsyncBody, FakeHttpClient, Response, Url, http},
    };

    use super::*;
    use crate::explorer::test_support::test_view_entity_at_path;

    fn progress_row(id: u64) -> DownloadNoticeRow {
        DownloadNoticeRow {
            id,
            kind: DownloadNoticeKind::File,
            file_name: "file.zip".to_owned(),
            destination: PathBuf::from("downloads"),
            status: DownloadNoticeStatus::Connecting,
            speed_tracker: Default::default(),
            output_paths: Vec::new(),
        }
    }

    #[test]
    fn download_speed_needs_timed_progress_and_excludes_existing_bytes() {
        let now = Instant::now();
        let mut tracker = DownloadSpeedTracker::default();
        tracker.record(now, 1000);
        assert_eq!(tracker.speed(now + Duration::from_secs(1)), None);
        tracker.record(now + Duration::from_millis(400), 1200);
        assert_eq!(tracker.speed(now + Duration::from_millis(400)), None);
        tracker.record(now + Duration::from_millis(500), 1250);
        assert_eq!(tracker.speed(now + Duration::from_millis(500)), Some(500.0));
        assert_eq!(tracker.speed(now), None);
    }

    #[test]
    fn download_speed_uses_recent_window_and_decays_during_stalls() {
        let now = Instant::now();
        let mut tracker = DownloadSpeedTracker::default();
        tracker.record(now, 0);
        for second in 1..=10 {
            let bytes = if second <= 5 {
                second * 100
            } else {
                500 + (second - 5) * 300
            };
            tracker.record(now + Duration::from_secs(second), bytes);
        }
        assert_eq!(tracker.speed(now + Duration::from_secs(10)), Some(300.0));
        assert_eq!(
            tracker.speed(now + Duration::from_millis(10500)),
            Some(270.0)
        );
        assert_eq!(tracker.speed(now + Duration::from_secs(15)), Some(0.0));
        tracker.record(now + Duration::from_secs(15), 2000);
        tracker.record(now + Duration::from_secs(16), 2300);
        assert!(tracker.speed(now + Duration::from_secs(16)).unwrap() > 0.0);
        tracker.record(now + Duration::from_secs(21), 3800);
        assert_eq!(tracker.speed(now + Duration::from_secs(21)), Some(300.0));
    }

    #[test]
    fn download_speed_handles_duplicate_timestamps_and_counter_regression() {
        let now = Instant::now();
        let mut tracker = DownloadSpeedTracker::default();
        tracker.record(now, 0);
        tracker.record(now, 100);
        assert_eq!(tracker.speed(now), None);
        assert_eq!(tracker.speed(now + Duration::from_secs(1)), None);
        tracker.record(now + Duration::from_secs(1), 300);
        assert_eq!(tracker.speed(now + Duration::from_secs(1)), Some(200.0));
        tracker.record(now + Duration::from_millis(500), 150);
        assert_eq!(tracker.speed(now + Duration::from_secs(1)), Some(200.0));
        tracker.record(now + Duration::from_secs(2), 50);
        assert_eq!(tracker.speed(now + Duration::from_secs(2)), None);
        tracker.record(now + Duration::from_secs(3), 150);
        assert_eq!(tracker.speed(now + Duration::from_secs(3)), Some(100.0));
    }

    #[test]
    fn download_speed_coalesces_dense_samples_without_losing_recent_rates() {
        let now = Instant::now();
        let mut tracker = DownloadSpeedTracker::default();
        tracker.record(now, 0);
        for millisecond in 1..=10000 {
            let bytes = if millisecond <= 4000 {
                millisecond
            } else {
                4000 + (millisecond - 4000) * 3
            };
            tracker.record(now + Duration::from_millis(millisecond), bytes);
        }
        assert!(
            tracker.samples.len() <= 60,
            "{} retained samples",
            tracker.samples.len()
        );
        assert!((tracker.speed(now + Duration::from_secs(10)).unwrap() - 3000.0).abs() < 1.0);
    }

    #[test]
    fn download_metrics_handle_unknown_late_and_finished_totals() {
        let now = Instant::now();
        let mut row = progress_row(1);
        row.record_progress(
            DownloadProgress {
                downloaded_bytes: 100,
                total_bytes: None,
            },
            now,
        );
        row.record_progress(
            DownloadProgress {
                downloaded_bytes: 600,
                total_bytes: None,
            },
            now + Duration::from_secs(1),
        );
        assert_eq!(
            row.transfer_metrics(now + Duration::from_secs(1)),
            (Some(500.0), None)
        );
        row.record_progress(
            DownloadProgress {
                downloaded_bytes: 600,
                total_bytes: Some(1600),
            },
            now + Duration::from_secs(1),
        );
        assert_eq!(
            row.transfer_metrics(now + Duration::from_secs(1)),
            (Some(500.0), Some(Duration::from_secs(2)))
        );
        assert_eq!(
            row.transfer_metrics(now + Duration::from_secs(6)),
            (Some(0.0), None)
        );
        row.record_progress(
            DownloadProgress {
                downloaded_bytes: 1600,
                total_bytes: Some(1600),
            },
            now + Duration::from_secs(7),
        );
        assert_eq!(
            row.transfer_metrics(now + Duration::from_secs(7)).1,
            Some(Duration::ZERO)
        );
        row.record_progress(
            DownloadProgress {
                downloaded_bytes: 1700,
                total_bytes: Some(1600),
            },
            now + Duration::from_secs(8),
        );
        assert_eq!(
            row.transfer_metrics(now + Duration::from_secs(8)).1,
            Some(Duration::ZERO)
        );
        for status in [
            DownloadNoticeStatus::Connecting,
            DownloadNoticeStatus::WaitingForCredentials,
            DownloadNoticeStatus::WaitingForHostConfirmation,
            DownloadNoticeStatus::Completed,
            DownloadNoticeStatus::Failed("failed".to_owned()),
        ] {
            let mut waiting = row.clone();
            waiting.set_status(status);
            assert_eq!(
                waiting.transfer_metrics(now + Duration::from_secs(8)),
                (None, None)
            );
            assert!(waiting.speed_tracker.samples.is_empty());
        }
    }

    #[test]
    fn download_metrics_handle_empty_files_and_unrepresentable_eta() {
        let now = Instant::now();
        let mut row = progress_row(1);
        row.record_progress(
            DownloadProgress {
                downloaded_bytes: 0,
                total_bytes: Some(0),
            },
            now,
        );
        assert_eq!(
            row.transfer_metrics(now + Duration::from_secs(1)),
            (None, None)
        );
        row.record_progress(
            DownloadProgress {
                downloaded_bytes: 1,
                total_bytes: Some(u64::MAX),
            },
            now + Duration::from_secs(5),
        );
        assert_eq!(
            row.transfer_metrics(now + Duration::from_secs(5)),
            (Some(0.2), None)
        );
    }

    #[gpui::test]
    fn ytdlp_queued_stream_boundaries_reset_even_when_totals_and_counters_match(
        cx: &mut TestAppContext,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());
        let now = Instant::now();
        cx.update(|_, app| {
            view.update(app, |view, _| {
                view.download_notice_rows = vec![progress_row(7), progress_row(8)];
            })
        });
        let (tx, rx) = mpsc::channel();
        let first = DownloadProgress {
            downloaded_bytes: 100,
            total_bytes: Some(1000),
        };
        let final_progress = DownloadProgress {
            downloaded_bytes: 1000,
            total_bytes: Some(1000),
        };
        tx.send((YtDlpProgressEvent::Downloading(first), now))
            .unwrap();
        tx.send((
            YtDlpProgressEvent::Downloading(final_progress),
            now + Duration::from_secs(1),
        ))
        .unwrap();
        tx.send((
            YtDlpProgressEvent::Finished(final_progress),
            now + Duration::from_secs(1),
        ))
        .unwrap();
        // A fully resumed second stream can start with the same counter and total.
        tx.send((
            YtDlpProgressEvent::Downloading(final_progress),
            now + Duration::from_secs(2),
        ))
        .unwrap();
        let mut async_app = cx.update(|_, app| app.to_async());
        ExplorerView::drain_ytdlp_progress(&view.downgrade(), &mut async_app, 7, &rx);
        cx.read_entity(&view, |view, _| {
            let row = &view.download_notice_rows[0];
            assert_eq!(
                row.transfer_metrics(now + Duration::from_secs(2)),
                (None, None)
            );
            assert_eq!(row.speed_tracker.samples.len(), 1);
            assert_eq!(
                row.status,
                DownloadNoticeStatus::Downloading {
                    downloaded_bytes: 1000,
                    total_bytes: Some(1000)
                }
            );
            assert!(
                view.download_notice_rows[1]
                    .speed_tracker
                    .samples
                    .is_empty()
            );
        });
        tx.send((
            YtDlpProgressEvent::PostProcessing,
            now + Duration::from_secs(3),
        ))
        .unwrap();
        let mut async_app = cx.update(|_, app| app.to_async());
        ExplorerView::drain_ytdlp_progress(&view.downgrade(), &mut async_app, 7, &rx);
        cx.read_entity(&view, |view, _| {
            assert_eq!(
                view.download_notice_rows[0].status,
                DownloadNoticeStatus::Connecting
            );
            assert!(
                view.download_notice_rows[0]
                    .speed_tracker
                    .samples
                    .is_empty()
            );
        });
    }

    #[gpui::test]
    fn download_queue_uses_worker_timestamps_before_publishing_latest_progress(
        cx: &mut TestAppContext,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());
        let now = Instant::now();
        cx.update(|_, app| {
            view.update(app, |view, _| {
                view.download_notice_rows = vec![progress_row(7)]
            })
        });
        let (tx, rx) = mpsc::channel();
        for (seconds, bytes) in [(0, 0), (1, 500), (2, 1000)] {
            tx.send((
                DownloadProgress {
                    downloaded_bytes: bytes,
                    total_bytes: Some(2000),
                },
                now + Duration::from_secs(seconds),
            ))
            .unwrap();
        }
        let mut async_app = cx.update(|_, app| app.to_async());
        ExplorerView::drain_download_progress(&view.downgrade(), &mut async_app, 7, &rx);
        cx.read_entity(&view, |view, _| {
            assert_eq!(
                view.download_notice_rows[0].transfer_metrics(now + Duration::from_secs(2)),
                (Some(500.0), Some(Duration::from_secs(2)))
            );
        });
        // An empty drain preserves history; querying later still expires a stalled rate.
        let mut async_app = cx.update(|_, app| app.to_async());
        ExplorerView::drain_download_progress(&view.downgrade(), &mut async_app, 7, &rx);
        cx.read_entity(&view, |view, _| {
            assert_eq!(
                view.download_notice_rows[0].transfer_metrics(now + Duration::from_secs(7)),
                (Some(0.0), None)
            );
        });
    }

    #[test]
    fn download_names_use_the_first_free_windows_style_suffix() {
        assert_eq!(download_file_name("archive.tar.gz", 1), "archive.tar.gz");
        assert_eq!(
            download_file_name("archive.tar.gz", 2),
            "archive.tar (2).gz"
        );
    }

    #[test]
    fn ytdlp_command_appends_video_only_guard_and_url_after_custom_options() {
        let destination = PathBuf::from("downloads");
        let spec = ytdlp_command_spec(
            PathBuf::from("yt-dlp"),
            vec![
                "--cookies-from-browser".to_owned(),
                "firefox profile".to_owned(),
                "--yes-playlist".to_owned(),
                "--quiet".to_owned(),
                "--no-progress".to_owned(),
                "--progress-template".to_owned(),
                "download:user-template".to_owned(),
            ],
            "https://youtube.com/watch?v=dQw4w9WgXcQ&list=PL123",
            destination.clone(),
        );

        assert_eq!(spec.executable, Path::new("yt-dlp"));
        assert_eq!(spec.current_dir, destination);
        assert_eq!(
            spec.args,
            [
                OsString::from("--cookies-from-browser"),
                OsString::from("firefox profile"),
                OsString::from("--yes-playlist"),
                OsString::from("--quiet"),
                OsString::from("--no-progress"),
                OsString::from("--progress-template"),
                OsString::from("download:user-template"),
                OsString::from("--no-playlist"),
                OsString::from("--print"),
                OsString::from(YTDLP_OUTPUT_TEMPLATE),
                OsString::from("--no-quiet"),
                OsString::from("--newline"),
                OsString::from("--progress"),
                OsString::from("--progress-delta"),
                OsString::from("0.1"),
                OsString::from("--progress-template"),
                OsString::from(YTDLP_DOWNLOAD_PROGRESS_TEMPLATE),
                OsString::from("--progress-template"),
                OsString::from(YTDLP_POSTPROCESS_PROGRESS_TEMPLATE),
                OsString::from("--"),
                OsString::from("https://youtube.com/watch?v=dQw4w9WgXcQ&list=PL123"),
            ]
        );
    }

    #[test]
    fn ytdlp_progress_parser_uses_only_exact_totals() {
        assert_eq!(
            ytdlp_progress_event_from_line(
                "__EXPLORER_YTDLP_DOWNLOAD_PROGRESS__{\"status\":\"downloading\",\"downloaded_bytes\":25,\"total_bytes\":100,\"total_bytes_estimate\":120}\n"
            ),
            Some(YtDlpProgressEvent::Downloading(DownloadProgress {
                downloaded_bytes: 25,
                total_bytes: Some(100),
            }))
        );
        assert_eq!(
            ytdlp_progress_event_from_line(
                "2: __EXPLORER_YTDLP_DOWNLOAD_PROGRESS__{\"status\":\"downloading\",\"downloaded_bytes\":25,\"total_bytes_estimate\":120}\r\n"
            ),
            Some(YtDlpProgressEvent::Downloading(DownloadProgress {
                downloaded_bytes: 25,
                total_bytes: None,
            }))
        );
        assert_eq!(
            ytdlp_progress_event_from_line(
                "__EXPLORER_YTDLP_DOWNLOAD_PROGRESS__{\"status\":\"finished\",\"total_bytes\":100}\n"
            ),
            Some(YtDlpProgressEvent::Finished(DownloadProgress {
                downloaded_bytes: 100,
                total_bytes: Some(100),
            }))
        );
    }

    #[test]
    fn ytdlp_progress_parser_handles_postprocessing_and_ignores_bad_output() {
        assert_eq!(
            ytdlp_progress_event_from_line(
                "__EXPLORER_YTDLP_POSTPROCESS_PROGRESS__{\"status\":\"started\"}\n"
            ),
            Some(YtDlpProgressEvent::PostProcessing)
        );
        for line in [
            "[youtube] Extracting URL",
            "__EXPLORER_YTDLP_DOWNLOAD_PROGRESS__not-json",
            "__EXPLORER_YTDLP_DOWNLOAD_PROGRESS__{\"status\":\"error\",\"downloaded_bytes\":25}",
            "__EXPLORER_YTDLP_DOWNLOAD_PROGRESS__{\"status\":\"downloading\",\"downloaded_bytes\":18446744073709551616}",
            "__EXPLORER_YTDLP_POSTPROCESS_PROGRESS__{\"status\":\"unknown\"}",
        ] {
            assert_eq!(ytdlp_progress_event_from_line(line), None, "{line}");
        }
    }

    #[test]
    fn ytdlp_progress_reader_publishes_only_machine_records() {
        let output = b"[youtube] Extracting URL\n__EXPLORER_YTDLP_DOWNLOAD_PROGRESS__{\"status\":\"downloading\",\"downloaded_bytes\":10,\"total_bytes\":20}\nnoise\n__EXPLORER_YTDLP_POSTPROCESS_PROGRESS__{\"status\":\"processing\"}\n";
        let events = Mutex::new(Vec::new());
        read_ytdlp_progress(output.as_slice(), |event| {
            events.lock().unwrap().push(event)
        })
        .expect("read progress output");
        assert_eq!(
            events.into_inner().unwrap(),
            [
                YtDlpProgressEvent::Downloading(DownloadProgress {
                    downloaded_bytes: 10,
                    total_bytes: Some(20),
                }),
                YtDlpProgressEvent::PostProcessing,
            ]
        );
    }

    #[test]
    fn ytdlp_output_records_capture_final_paths_without_publishing_progress() {
        let paths = [r#"子 folder/quoted "video".mp4"#, r"nested\movie.mkv"];
        let mut lines = String::from(
            "noise\n__EXPLORER_YTDLP_OUTPUT__not-json\n__EXPLORER_YTDLP_OUTPUT__null\n__EXPLORER_YTDLP_OUTPUT__\"\"\n",
        );
        for path in paths.iter().chain(paths.iter()) {
            lines.push_str(YTDLP_OUTPUT_PREFIX);
            lines.push_str(&serde_json::to_string(path).unwrap());
            lines.push_str("\r\n");
        }
        let captured =
            read_ytdlp_progress(lines.as_bytes(), |_| panic!("output path is not progress"))
                .unwrap();
        assert_eq!(captured, paths.map(PathBuf::from));
        for value in ["[]", "42", "\"-\"", "\"NA\"", "\"bad\\u0000name\""] {
            let parsed = ytdlp_output_path_from_line(&format!("{YTDLP_OUTPUT_PREFIX}{value}"));
            assert!(parsed.is_none());
        }
    }

    #[test]
    fn ytdlp_output_paths_resolve_relative_names_and_deduplicate() {
        let directory = tempfile::tempdir().unwrap();
        let absolute = directory.path().join("movie.mkv");
        assert_eq!(
            resolve_ytdlp_output_paths(
                vec![
                    PathBuf::from("movie.mkv"),
                    absolute.clone(),
                    PathBuf::from("nested/second.mp4")
                ],
                directory.path()
            ),
            vec![absolute, directory.path().join("nested/second.mp4")]
        );
    }

    #[gpui::test]
    fn ytdlp_progress_events_update_exact_unknown_and_postprocess_states(cx: &mut TestAppContext) {
        let temp = tempfile::tempdir().expect("temp directory");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());
        cx.update(|_, app| {
            view.update(app, |view, _| {
                view.download_notice_rows = vec![DownloadNoticeRow {
                    speed_tracker: Default::default(),
                    output_paths: Vec::new(),
                    id: 7,
                    kind: DownloadNoticeKind::Video {
                        site_domain: "youtube.com".to_owned(),
                    },
                    file_name: "Video from youtube.com".to_owned(),
                    destination: temp.path().to_path_buf(),
                    status: DownloadNoticeStatus::Connecting,
                }];

                assert!(view.apply_ytdlp_progress_event(
                    7,
                    YtDlpProgressEvent::Downloading(DownloadProgress {
                        downloaded_bytes: 25,
                        total_bytes: Some(100),
                    }),
                    Instant::now(),
                ));
                assert_eq!(
                    view.download_notice_rows[0].status,
                    DownloadNoticeStatus::Downloading {
                        downloaded_bytes: 25,
                        total_bytes: Some(100),
                    }
                );

                assert!(view.apply_ytdlp_progress_event(
                    7,
                    YtDlpProgressEvent::Downloading(DownloadProgress {
                        downloaded_bytes: 50,
                        total_bytes: None,
                    }),
                    Instant::now(),
                ));
                assert_eq!(
                    view.download_notice_rows[0].status,
                    DownloadNoticeStatus::Downloading {
                        downloaded_bytes: 50,
                        total_bytes: None,
                    }
                );

                assert!(view.apply_ytdlp_progress_event(
                    7,
                    YtDlpProgressEvent::PostProcessing,
                    Instant::now()
                ));
                assert_eq!(
                    view.download_notice_rows[0].status,
                    DownloadNoticeStatus::Connecting
                );
                assert!(!view.apply_ytdlp_progress_event(
                    99,
                    YtDlpProgressEvent::PostProcessing,
                    Instant::now()
                ));
            });
        });
    }

    #[test]
    fn ytdlp_path_resolution_searches_direct_names_and_extensions() {
        let first = PathBuf::from("first-bin");
        let second = PathBuf::from("second-bin");
        let path_var = std::env::join_paths([&first, &second]).expect("join test PATH");
        let expected = second.join("yt-dlp.EXE");

        assert_eq!(
            resolve_ytdlp_executable_with(
                &path_var,
                &[OsString::from(".EXE"), OsString::from(".CMD")],
                |candidate| candidate == expected,
            ),
            Some(expected)
        );
        assert_eq!(
            resolve_ytdlp_executable_with(&path_var, &[], |_| false),
            None
        );
    }

    #[test]
    fn ytdlp_process_errors_prefer_stderr_and_bound_the_message() {
        let error = ytdlp_result_from_process(
            false,
            "exit code: 1",
            b"WARNING: preceding detail\nERROR: unavailable video\n",
        )
        .expect_err("failed yt-dlp");
        assert_eq!(
            error,
            "yt-dlp exited with exit code: 1: ERROR: unavailable video"
        );

        let empty =
            ytdlp_result_from_process(false, "exit code: 2", b" \n").expect_err("failed yt-dlp");
        assert_eq!(empty, "yt-dlp exited with exit code: 2.");

        let oversized = vec![b'x'; YTDLP_ERROR_MESSAGE_LIMIT + 100];
        let tail = read_bounded_tail(oversized.as_slice(), YTDLP_ERROR_MESSAGE_LIMIT)
            .expect("bounded tail");
        assert_eq!(tail.len(), YTDLP_ERROR_MESSAGE_LIMIT);
    }

    const YTDLP_TEST_MODE: &str = "EXPLORER_TEST_YTDLP_MODE";
    const YTDLP_TEST_READY: &str = "EXPLORER_TEST_YTDLP_READY";
    const YTDLP_TEST_DESCENDANT_READY: &str = "EXPLORER_TEST_YTDLP_DESCENDANT_READY";
    const YTDLP_TEST_DESCENDANT_FINISHED: &str = "EXPLORER_TEST_YTDLP_DESCENDANT_FINISHED";

    #[test]
    fn ytdlp_process_test_helper() {
        let Some(mode) = std::env::var_os(YTDLP_TEST_MODE) else {
            return;
        };
        if mode == "output" {
            let path = serde_json::to_string("Final media.mkv").unwrap();
            println!("{YTDLP_OUTPUT_PREFIX}{path}");
            println!("{YTDLP_OUTPUT_PREFIX}{path}");
            return;
        }
        let ready = PathBuf::from(std::env::var_os(YTDLP_TEST_READY).expect("ready path"));
        let descendant_ready = PathBuf::from(
            std::env::var_os(YTDLP_TEST_DESCENDANT_READY).expect("descendant ready path"),
        );
        let _ = std::env::var_os(YTDLP_TEST_DESCENDANT_FINISHED).expect("descendant finished path");

        #[cfg(target_os = "windows")]
        let mut descendant = {
            let mut command = Command::new("powershell.exe");
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[IO.File]::WriteAllText($env:EXPLORER_TEST_YTDLP_DESCENDANT_READY, 'ready'); Start-Sleep -Seconds 1; [IO.File]::WriteAllText($env:EXPLORER_TEST_YTDLP_DESCENDANT_FINISHED, 'finished'); Start-Sleep -Seconds 30",
            ]);
            command
        };
        #[cfg(unix)]
        let mut descendant = {
            let mut command = Command::new("/bin/sh");
            command.args([
                "-c",
                "printf ready > \"$EXPLORER_TEST_YTDLP_DESCENDANT_READY\"; sleep 1; printf finished > \"$EXPLORER_TEST_YTDLP_DESCENDANT_FINISHED\"; sleep 30",
            ]);
            command
        };
        descendant
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut descendant = descendant.spawn().expect("spawn descendant helper");
        wait_for_ytdlp_test_path(&descendant_ready, Duration::from_secs(5));
        std::fs::write(ready, b"ready").expect("mark parent ready");
        let _ = descendant.wait();
    }

    fn ytdlp_test_command(directory: &Path) -> Command {
        let ready = directory.join("parent-ready");
        let descendant_ready = directory.join("descendant-ready");
        let descendant_finished = directory.join("descendant-finished");
        let mut command = Command::new(std::env::current_exe().expect("test executable"));
        command
            .args([
                "--exact",
                "explorer::download::tests::ytdlp_process_test_helper",
                "--nocapture",
            ])
            .env(YTDLP_TEST_MODE, "parent")
            .env(YTDLP_TEST_READY, ready)
            .env(YTDLP_TEST_DESCENDANT_READY, descendant_ready)
            .env(YTDLP_TEST_DESCENDANT_FINISHED, descendant_finished)
            .current_dir(directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(target_os = "windows")]
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }

    fn wait_for_ytdlp_test_path(path: &Path, timeout: Duration) {
        let started = std::time::Instant::now();
        while !path.exists() {
            assert!(
                started.elapsed() < timeout,
                "timed out waiting for {}",
                path.display()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn ytdlp_process_returns_final_media_paths_after_reading_stdout() {
        let temp = tempfile::tempdir().unwrap();
        let control = YtDlpProcessControl::new();
        let mut command = ytdlp_test_command(temp.path());
        command
            .env(YTDLP_TEST_MODE, "output")
            .stdout(Stdio::piped());
        let result = run_ytdlp_command(&mut command, control.shared(), |_| {
            panic!("path is not progress")
        })
        .unwrap();
        let DownloadResult::Video(paths) = result else {
            panic!("expected media paths");
        };
        assert_eq!(paths, vec![PathBuf::from("Final media.mkv")]);
    }

    #[test]
    fn ytdlp_process_cancelled_before_registration_terminates_promptly() {
        let temp = tempfile::tempdir().expect("temp directory");
        let control = YtDlpProcessControl::new();
        let state = control.shared();
        control.cancel();
        let (result_tx, result_rx) = mpsc::channel();
        let directory = temp.path().to_path_buf();

        thread::spawn(move || {
            let mut command = ytdlp_test_command(&directory);
            let _ = result_tx.send(run_ytdlp_command(&mut command, state, |_| {}));
        });

        let result = result_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("pre-cancelled process should stop promptly");
        assert!(result.is_err());
        assert!(control.shared.cancelled.load(Ordering::Relaxed));
        assert!(control.shared.child.lock().unwrap().is_none());
    }

    #[test]
    fn ytdlp_process_cancel_terminates_registered_process_tree() {
        let temp = tempfile::tempdir().expect("temp directory");
        let control = YtDlpProcessControl::new();
        let state = control.shared();
        let (result_tx, result_rx) = mpsc::channel();
        let directory = temp.path().to_path_buf();

        thread::spawn(move || {
            let mut command = ytdlp_test_command(&directory);
            let _ = result_tx.send(run_ytdlp_command(&mut command, state, |_| {}));
        });

        let ready = temp.path().join("parent-ready");
        let started = std::time::Instant::now();
        while !ready.exists() {
            if let Ok(result) = result_rx.try_recv() {
                panic!("yt-dlp helper exited before becoming ready: {result:?}");
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "timed out waiting for {}",
                ready.display()
            );
            thread::sleep(Duration::from_millis(10));
        }
        control.cancel();
        let result = result_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("cancelled process should stop promptly");
        assert!(result.is_err());
        assert!(control.shared.child.lock().unwrap().is_none());

        thread::sleep(Duration::from_millis(1_250));
        assert!(!temp.path().join("descendant-finished").exists());
    }

    #[gpui::test]
    fn cancelling_one_video_keeps_other_downloads_and_excludes_it_from_completion_notice(
        cx: &mut TestAppContext,
    ) {
        let temp = tempfile::tempdir().expect("temp directory");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());
        let continuing = YtDlpProcessControl::new();
        let continuing_state = continuing.shared();
        let cancelled = YtDlpProcessControl::new();
        let cancelled_state = cancelled.shared();

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.download_notice_rows = vec![
                    DownloadNoticeRow {
                        speed_tracker: Default::default(),
                        output_paths: Vec::new(),
                        id: 1,
                        kind: DownloadNoticeKind::Video {
                            site_domain: "youtube.com".to_owned(),
                        },
                        file_name: "Video from youtube.com".to_owned(),
                        destination: temp.path().to_path_buf(),
                        status: DownloadNoticeStatus::Connecting,
                    },
                    DownloadNoticeRow {
                        speed_tracker: Default::default(),
                        output_paths: Vec::new(),
                        id: 2,
                        kind: DownloadNoticeKind::Video {
                            site_domain: "vimeo.com".to_owned(),
                        },
                        file_name: "Video from vimeo.com".to_owned(),
                        destination: temp.path().to_path_buf(),
                        status: DownloadNoticeStatus::Connecting,
                    },
                ];
                view.download_tasks = vec![(1, gpui::Task::ready(())), (2, gpui::Task::ready(()))];
                view.ytdlp_process_controls = vec![(1, continuing), (2, cancelled)];
                view.cancel_download(2, cx);

                assert_eq!(
                    view.download_notice_rows
                        .iter()
                        .map(|row| row.id)
                        .collect::<Vec<_>>(),
                    [1]
                );
                assert_eq!(
                    view.download_tasks
                        .iter()
                        .map(|(id, _)| *id)
                        .collect::<Vec<_>>(),
                    [1]
                );
                assert_eq!(view.ytdlp_process_controls[0].0, 1);
                assert!(view.operation_notice.is_none());
                assert!(cancelled_state.cancelled.load(Ordering::Relaxed));
                assert!(!continuing_state.cancelled.load(Ordering::Relaxed));

                view.remove_ytdlp_process_control(1);
                view.complete_download(1, Ok(DownloadResult::Video(Vec::new())), cx);
            });
        });

        assert!(cancelled_state.cancelled.load(Ordering::Relaxed));
        assert!(continuing_state.cancelled.load(Ordering::Relaxed));
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.download_batch_succeeded, 1);
            assert_eq!(view.download_batch_failed, 0);
            assert!(view.operation_notice.is_none());
            assert_eq!(view.download_notice_rows.len(), 1);
            assert_eq!(
                view.download_notice_rows[0].status,
                DownloadNoticeStatus::Completed
            );
        });
    }

    #[gpui::test]
    fn completed_video_downloads_remain_for_five_seconds_without_a_notice(cx: &mut TestAppContext) {
        let temp = tempfile::tempdir().expect("temp directory");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.download_notice_rows = vec![
                    DownloadNoticeRow {
                        speed_tracker: Default::default(),
                        output_paths: Vec::new(),
                        id: 1,
                        kind: DownloadNoticeKind::Video {
                            site_domain: "vimeo.com".to_owned(),
                        },
                        file_name: "Video from vimeo.com".to_owned(),
                        destination: temp.path().to_path_buf(),
                        status: DownloadNoticeStatus::Connecting,
                    },
                    DownloadNoticeRow {
                        speed_tracker: Default::default(),
                        output_paths: Vec::new(),
                        id: 2,
                        kind: DownloadNoticeKind::Video {
                            site_domain: "vimeo.com".to_owned(),
                        },
                        file_name: "Video from vimeo.com".to_owned(),
                        destination: temp.path().to_path_buf(),
                        status: DownloadNoticeStatus::Connecting,
                    },
                ];
                view.complete_download(1, Ok(DownloadResult::Video(Vec::new())), cx);
                assert!(view.operation_notice.is_none());
                view.complete_download(2, Ok(DownloadResult::Video(Vec::new())), cx);
            });
        });

        cx.read_entity(&view, |view, _| {
            assert_eq!(view.download_notice_rows.len(), 2);
            assert_eq!(view.download_batch_succeeded, 2);
            assert!(view.operation_notice.is_none());
        });
        cx.executor()
            .advance_clock(super::super::remote_ui::TRANSFER_COMPLETION_RETENTION);
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(view.download_notice_rows.is_empty());
            assert!(view.operation_notice.is_none());
        });
    }

    #[gpui::test]
    fn mixed_site_video_downloads_do_not_create_a_completion_notice(cx: &mut TestAppContext) {
        let temp = tempfile::tempdir().expect("temp directory");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.download_notice_rows = vec![
                    DownloadNoticeRow {
                        speed_tracker: Default::default(),
                        output_paths: Vec::new(),
                        id: 1,
                        kind: DownloadNoticeKind::Video {
                            site_domain: "vimeo.com".to_owned(),
                        },
                        file_name: "Video from vimeo.com".to_owned(),
                        destination: temp.path().to_path_buf(),
                        status: DownloadNoticeStatus::Connecting,
                    },
                    DownloadNoticeRow {
                        speed_tracker: Default::default(),
                        output_paths: Vec::new(),
                        id: 2,
                        kind: DownloadNoticeKind::Video {
                            site_domain: "dailymotion.com".to_owned(),
                        },
                        file_name: "Video from dailymotion.com".to_owned(),
                        destination: temp.path().to_path_buf(),
                        status: DownloadNoticeStatus::Connecting,
                    },
                ];
                view.complete_download(1, Ok(DownloadResult::Video(Vec::new())), cx);
                view.complete_download(2, Ok(DownloadResult::Video(Vec::new())), cx);
            });
        });

        cx.read_entity(&view, |view, _| {
            assert_eq!(view.download_notice_rows.len(), 2);
            assert!(view.operation_notice.is_none());
        });
    }

    #[test]
    fn streaming_download_persists_bytes_and_reports_progress() {
        let temp = tempfile::tempdir().expect("temp directory");
        let body = b"streamed body".to_vec();
        let expected_length = body.len() as u64;
        let client = FakeHttpClient::create(move |_| {
            let body = body.clone();
            async move {
                Ok(Response::builder()
                    .status(200)
                    .header(http::header::CONTENT_LENGTH, body.len())
                    .body(AsyncBody::from(body))?)
            }
        });
        let progress = Arc::new(Mutex::new(Vec::new()));
        let captured_progress = progress.clone();

        let result = futures::executor::block_on(download_url_to_temporary_file(
            client,
            ClipboardDownload {
                url: Url::parse("https://example.com/file.zip").unwrap(),
                file_name: "file.zip".to_owned(),
            },
            temp.path(),
            move |value| captured_progress.lock().unwrap().push(value),
        ))
        .and_then(PendingDownload::persist)
        .expect("download");

        let DownloadResult::File(path) = result else {
            panic!("expected file download");
        };

        assert_eq!(std::fs::read(path).unwrap(), b"streamed body");
        assert_eq!(
            progress.lock().unwrap().last().copied(),
            Some(DownloadProgress {
                downloaded_bytes: expected_length,
                total_bytes: Some(expected_length),
            })
        );
    }

    #[test]
    fn failed_and_truncated_downloads_leave_no_destination_file() {
        for (status, length) in [(404, None), (200, Some(99))] {
            let temp = tempfile::tempdir().expect("temp directory");
            let client = FakeHttpClient::create(move |_| async move {
                let mut response = Response::builder().status(status);
                if let Some(length) = length {
                    response = response.header(http::header::CONTENT_LENGTH, length);
                }
                Ok(response.body(AsyncBody::from(b"short".to_vec()))?)
            });

            let result = futures::executor::block_on(download_url_to_temporary_file(
                client,
                ClipboardDownload {
                    url: Url::parse("https://example.com/file.zip").unwrap(),
                    file_name: "file.zip".to_owned(),
                },
                temp.path(),
                |_| {},
            ));

            assert!(result.is_err());
            assert!(std::fs::read_dir(temp.path()).unwrap().next().is_none());
        }
    }

    #[test]
    fn existing_download_gets_suffixed_without_overwrite() {
        let temp = tempfile::tempdir().expect("temp directory");
        std::fs::write(temp.path().join("file.zip"), b"existing").unwrap();
        let client = FakeHttpClient::create(|_| async move {
            Ok(Response::builder()
                .status(200)
                .body(AsyncBody::from(b"new".to_vec()))?)
        });

        let result = futures::executor::block_on(download_url_to_temporary_file(
            client,
            ClipboardDownload {
                url: Url::parse("https://example.com/file.zip").unwrap(),
                file_name: "file.zip".to_owned(),
            },
            temp.path(),
            |_| {},
        ))
        .and_then(PendingDownload::persist)
        .expect("download");

        let DownloadResult::File(path) = result else {
            panic!("expected file download");
        };

        assert_eq!(path.file_name().unwrap(), "file (2).zip");
        assert_eq!(
            std::fs::read(temp.path().join("file.zip")).unwrap(),
            b"existing"
        );
        assert_eq!(std::fs::read(path).unwrap(), b"new");
    }

    #[gpui::test]
    fn clipboard_url_paste_retains_completion_without_a_notice(cx: &mut TestAppContext) {
        let temp = tempfile::tempdir().expect("temp directory");
        let destination = temp.path().to_path_buf();
        let client = FakeHttpClient::create(|request| async move {
            assert_eq!(request.uri().to_string(), "https://example.com/file.zip");
            Ok(Response::builder()
                .status(200)
                .header(http::header::CONTENT_LENGTH, 4)
                .body(AsyncBody::from(b"data".to_vec()))?)
        });
        cx.update(|app| {
            app.set_http_client(client);
            app.write_to_clipboard(ClipboardItem::new_string(
                "https://example.com/file.zip".to_owned(),
            ));
        });
        let (view, cx) = test_view_entity_at_path(cx, destination.clone());

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.remote_transfer_panel_collapsed = true;
                view.paste_clipboard(window, cx);
                assert!(!view.remote_transfer_panel_collapsed);
            });
        });
        cx.run_until_parked();
        cx.executor().advance_clock(DOWNLOAD_PROGRESS_INTERVAL);
        cx.run_until_parked();

        assert_eq!(
            std::fs::read(destination.join("file.zip")).unwrap(),
            b"data"
        );
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.download_notice_rows.len(), 1);
            assert_eq!(
                view.download_notice_rows[0].status,
                DownloadNoticeStatus::Completed
            );
            assert!(view.operation_notice.is_none());
        });
        cx.executor()
            .advance_clock(super::super::remote_ui::TRANSFER_COMPLETION_RETENTION);
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(view.download_notice_rows.is_empty());
            assert!(view.operation_notice.is_none());
        });
    }

    #[gpui::test]
    fn overlapping_downloads_share_retained_completion_rows(cx: &mut TestAppContext) {
        let temp = tempfile::tempdir().expect("temp directory");
        let destination = temp.path().to_path_buf();
        let client = FakeHttpClient::create(|request| {
            let body = request.uri().path().as_bytes().to_vec();
            async move {
                Ok(Response::builder()
                    .status(200)
                    .header(http::header::CONTENT_LENGTH, body.len())
                    .body(AsyncBody::from(body))?)
            }
        });
        cx.update(|app| {
            app.set_http_client(client);
            app.write_to_clipboard(ClipboardItem::new_string(
                "https://example.com/one.zip\n\nhttps://example.com/two.zip".to_owned(),
            ));
        });
        let (view, cx) = test_view_entity_at_path(cx, destination.clone());

        cx.update(|window, app| {
            view.update(app, |view, cx| {
                view.paste_clipboard(window, cx);
                assert_eq!(view.download_notice_rows.len(), 2);
                assert_eq!(view.download_tasks.len(), 2);
            });
        });
        cx.run_until_parked();
        cx.executor().advance_clock(DOWNLOAD_PROGRESS_INTERVAL);
        cx.run_until_parked();

        assert_eq!(
            std::fs::read(destination.join("one.zip")).unwrap(),
            b"/one.zip"
        );
        assert_eq!(
            std::fs::read(destination.join("two.zip")).unwrap(),
            b"/two.zip"
        );
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.download_notice_rows.len(), 2);
            assert!(
                view.download_notice_rows
                    .iter()
                    .all(|row| row.status == DownloadNoticeStatus::Completed)
            );
            assert!(view.operation_notice.is_none());
        });
    }

    #[gpui::test]
    fn cancelling_download_drops_partial_file_without_a_summary(cx: &mut TestAppContext) {
        let temp = tempfile::tempdir().expect("temp directory");
        let destination = temp.path().to_path_buf();
        let mut partial = NamedTempFile::new_in(&destination).expect("partial file");
        partial.write_all(b"partial").expect("partial contents");
        let (view, cx) = test_view_entity_at_path(cx, destination.clone());

        cx.update(|_, app| {
            let task = app.background_executor().spawn(async move {
                let _partial = partial;
                futures::future::pending::<()>().await;
            });
            view.update(app, |view, cx| {
                view.download_notice_rows.push(DownloadNoticeRow {
                    speed_tracker: Default::default(),
                    output_paths: Vec::new(),
                    id: 7,
                    kind: DownloadNoticeKind::File,
                    file_name: "partial.zip".to_owned(),
                    destination: destination.clone(),
                    status: DownloadNoticeStatus::Downloading {
                        downloaded_bytes: 7,
                        total_bytes: Some(100),
                    },
                });
                view.download_tasks.push((7, task));
                view.cancel_download(7, cx);
            });
        });

        cx.run_until_parked();
        assert!(std::fs::read_dir(&destination).unwrap().next().is_none());
        cx.read_entity(&view, |view, _| {
            assert!(view.download_notice_rows.is_empty());
            assert!(view.download_tasks.is_empty());
            assert!(view.operation_notice.is_none());
        });
    }

    #[gpui::test]
    fn cancelling_one_download_retains_only_the_completed_row(cx: &mut TestAppContext) {
        let temp = tempfile::tempdir().expect("temp directory");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.download_notice_rows = vec![
                    DownloadNoticeRow {
                        speed_tracker: Default::default(),
                        output_paths: Vec::new(),
                        id: 1,
                        kind: DownloadNoticeKind::File,
                        file_name: "complete.zip".to_owned(),
                        destination: temp.path().to_path_buf(),
                        status: DownloadNoticeStatus::Completed,
                    },
                    DownloadNoticeRow {
                        speed_tracker: Default::default(),
                        output_paths: Vec::new(),
                        id: 2,
                        kind: DownloadNoticeKind::File,
                        file_name: "cancel.zip".to_owned(),
                        destination: temp.path().to_path_buf(),
                        status: DownloadNoticeStatus::Connecting,
                    },
                ];
                view.download_batch_succeeded = 1;
                view.download_tasks.push((2, gpui::Task::ready(())));
                view.cancel_download(2, cx);
            });
        });

        cx.read_entity(&view, |view, _| {
            assert_eq!(view.download_notice_rows.len(), 1);
            assert_eq!(view.download_notice_rows[0].id, 1);
            assert!(view.operation_notice.is_none());
        });
    }

    #[gpui::test]
    fn completion_retention_starts_after_the_last_download_finishes(cx: &mut TestAppContext) {
        let temp = tempfile::tempdir().expect("temp directory");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.download_batch_active = true;
                view.download_notice_rows = vec![
                    DownloadNoticeRow {
                        speed_tracker: Default::default(),
                        output_paths: Vec::new(),
                        id: 1,
                        kind: DownloadNoticeKind::File,
                        file_name: "one.zip".to_owned(),
                        destination: temp.path().to_path_buf(),
                        status: DownloadNoticeStatus::Connecting,
                    },
                    DownloadNoticeRow {
                        speed_tracker: Default::default(),
                        output_paths: Vec::new(),
                        id: 2,
                        kind: DownloadNoticeKind::File,
                        file_name: "two.zip".to_owned(),
                        destination: temp.path().to_path_buf(),
                        status: DownloadNoticeStatus::Connecting,
                    },
                ];
                view.complete_download(1, Ok(DownloadResult::Video(Vec::new())), cx);
            });
        });
        cx.executor()
            .advance_clock(super::super::remote_ui::TRANSFER_COMPLETION_RETENTION);
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.download_notice_rows.len(), 2);
        });

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.complete_download(2, Ok(DownloadResult::Video(Vec::new())), cx);
            });
        });
        cx.executor().advance_clock(Duration::from_secs(4));
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.download_notice_rows.len(), 2);
        });
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(view.download_notice_rows.is_empty());
        });
    }

    #[gpui::test]
    fn new_download_during_retention_restarts_the_shared_timer(cx: &mut TestAppContext) {
        let temp = tempfile::tempdir().expect("temp directory");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.download_batch_active = true;
                view.download_notice_rows.push(DownloadNoticeRow {
                    speed_tracker: Default::default(),
                    output_paths: Vec::new(),
                    id: 1,
                    kind: DownloadNoticeKind::File,
                    file_name: "one.zip".to_owned(),
                    destination: temp.path().to_path_buf(),
                    status: DownloadNoticeStatus::Connecting,
                });
                view.complete_download(1, Ok(DownloadResult::Video(Vec::new())), cx);
            });
        });
        cx.executor().advance_clock(Duration::from_secs(4));
        cx.run_until_parked();

        cx.update(|_, app| {
            view.update(app, |view, _| {
                view.begin_download_batch_if_needed();
                view.download_notice_rows.push(DownloadNoticeRow {
                    speed_tracker: Default::default(),
                    output_paths: Vec::new(),
                    id: 2,
                    kind: DownloadNoticeKind::File,
                    file_name: "two.zip".to_owned(),
                    destination: temp.path().to_path_buf(),
                    status: DownloadNoticeStatus::Connecting,
                });
                assert_eq!(view.download_batch_succeeded, 0);
            });
        });
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.download_notice_rows.len(), 2);
        });

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.complete_download(2, Ok(DownloadResult::Video(Vec::new())), cx);
            });
        });
        cx.executor()
            .advance_clock(super::super::remote_ui::TRANSFER_COMPLETION_RETENTION);
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(view.download_notice_rows.is_empty());
        });
    }

    #[gpui::test]
    fn failure_summary_remains_while_only_successful_rows_are_retained(cx: &mut TestAppContext) {
        let temp = tempfile::tempdir().expect("temp directory");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.download_batch_active = true;
                view.download_notice_rows = vec![
                    DownloadNoticeRow {
                        speed_tracker: Default::default(),
                        output_paths: Vec::new(),
                        id: 1,
                        kind: DownloadNoticeKind::File,
                        file_name: "complete.zip".to_owned(),
                        destination: temp.path().to_path_buf(),
                        status: DownloadNoticeStatus::Connecting,
                    },
                    DownloadNoticeRow {
                        speed_tracker: Default::default(),
                        output_paths: Vec::new(),
                        id: 2,
                        kind: DownloadNoticeKind::File,
                        file_name: "failed.zip".to_owned(),
                        destination: temp.path().to_path_buf(),
                        status: DownloadNoticeStatus::Connecting,
                    },
                ];
                view.complete_download(1, Ok(DownloadResult::Video(Vec::new())), cx);
                view.complete_download(2, Err("network unavailable".to_owned()), cx);
            });
        });

        cx.read_entity(&view, |view, _| {
            assert_eq!(view.download_notice_rows.len(), 1);
            assert_eq!(view.download_notice_rows[0].id, 1);
            assert_eq!(
                view.operation_notice
                    .as_ref()
                    .map(|notice| notice.text.as_str()),
                Some("Downloaded 1 files; 1 failed: network unavailable")
            );
        });
        cx.executor()
            .advance_clock(super::super::remote_ui::TRANSFER_COMPLETION_RETENTION);
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(view.download_notice_rows.is_empty());
            assert_eq!(
                view.operation_notice
                    .as_ref()
                    .map(|notice| notice.text.as_str()),
                Some("Downloaded 1 files; 1 failed: network unavailable")
            );
        });
    }

    #[gpui::test]
    fn cancelling_completed_download_is_ignored_and_keeps_its_file(cx: &mut TestAppContext) {
        let temp = tempfile::tempdir().expect("temp directory");
        let destination = temp.path().to_path_buf();
        let completed_path = destination.join("complete.zip");
        std::fs::write(&completed_path, b"complete").expect("completed download");
        let (view, cx) = test_view_entity_at_path(cx, destination);

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.download_notice_rows.push(DownloadNoticeRow {
                    speed_tracker: Default::default(),
                    output_paths: Vec::new(),
                    id: 3,
                    kind: DownloadNoticeKind::File,
                    file_name: "complete.zip".to_owned(),
                    destination: completed_path.parent().unwrap().to_path_buf(),
                    status: DownloadNoticeStatus::Completed,
                });
                view.cancel_download(3, cx);
            });
        });

        assert_eq!(std::fs::read(completed_path).unwrap(), b"complete");
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.download_notice_rows.len(), 1);
            assert_eq!(
                view.download_notice_rows[0].status,
                DownloadNoticeStatus::Completed
            );
        });
    }
}
