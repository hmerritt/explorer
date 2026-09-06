use std::{
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
    time::Duration,
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
    view::{ExplorerView, OperationNotice},
};

const DOWNLOAD_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
pub(super) const DOWNLOAD_BUFFER_SIZE: usize = 64 * 1024;
const YTDLP_ERROR_MESSAGE_LIMIT: usize = 4 * 1024;
const YTDLP_PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);
const YTDLP_DOWNLOAD_PROGRESS_PREFIX: &str = "__EXPLORER_YTDLP_DOWNLOAD_PROGRESS__";
const YTDLP_POSTPROCESS_PROGRESS_PREFIX: &str = "__EXPLORER_YTDLP_POSTPROCESS_PROGRESS__";
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
    pub(super) status: DownloadNoticeStatus,
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
enum DownloadResult {
    File(PathBuf),
    Video,
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
        let _cache_invalidation = crate::explorer::remote_directory_cache::DirectoryMutation::new([self.destination.join(&self.file_name)]);
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
                    Ok(location) => self.start_native_transfer(vec![location.provider_path()], self.path.clone(), false, cx),
                    Err(error) => { self.set_error_notice(error); cx.notify(); },
                }
                return;
            }
            self.enqueue_remote_download(download, cx);
            return;
        }

        self.begin_download_batch_if_needed();

        let id = self.next_download_id;
        self.next_download_id = self.next_download_id.wrapping_add(1);
        self.download_notice_rows.push(DownloadNoticeRow {
            id,
            kind: DownloadNoticeKind::File,
            file_name: download.file_name.clone(),
            status: DownloadNoticeStatus::Connecting,
        });

        let destination = self.path.clone();
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
                                let _ = progress_tx.send(progress);
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
        self.pending_remote_downloads.push_back((download, self.path.clone()));
        if self.active_remote_download.is_none() {
            self.start_next_remote_download(cx);
        }
    }

    fn start_next_remote_download(&mut self, cx: &mut Context<Self>) {
        let Some((download, destination)) = self.pending_remote_downloads.pop_front() else {
            self.remote_credentials.clear();
            self.finish_download_batch_if_idle();
            cx.notify();
            return;
        };

        let id = self.next_download_id;
        self.next_download_id = self.next_download_id.wrapping_add(1);
        self.download_notice_rows.push(DownloadNoticeRow {
            id,
            kind: DownloadNoticeKind::File,
            file_name: download.file_name.clone(),
            status: DownloadNoticeStatus::Connecting,
        });
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
            row.status = DownloadNoticeStatus::Connecting;
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
                                let _ = progress_tx.send(progress);
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
                    row.status = DownloadNoticeStatus::WaitingForCredentials;
                }
                match open_remote_credentials_dialog(cx.entity(), id, host, username, message, false, cx) {
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
            Err(RemoteDownloadError::PassphraseRequired { host, username, key_path }) => {
                match open_remote_credentials_dialog(cx.entity(), id, host, username, Some(format!("Unlock private key {}", key_path.display())), true, cx) {
                    Ok(handle) => self.active_dialog_window = Some(handle),
                    Err(error) => { self.complete_download(id, Err(error), cx); self.finish_active_remote_download(cx); },
                }
            }
            Err(RemoteDownloadError::UnknownHost(key)) => {
                if let Some(row) = self
                    .download_notice_rows
                    .iter_mut()
                    .find(|row| row.id == id)
                {
                    row.status = DownloadNoticeStatus::WaitingForHostConfirmation;
                }
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
        if super::remote_fs::reply(id, super::remote_fs::PromptReply::Credentials(credentials.clone())) { self.clear_active_dialog_window(); return; }
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
                Ok(()) => { super::remote_fs::reply(id, super::remote_fs::PromptReply::Accept); },
                Err(error) => { super::remote_fs::reply(id, super::remote_fs::PromptReply::Cancel); self.set_error_notice(error); },
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
        if super::remote_fs::reply(id, super::remote_fs::PromptReply::Cancel) { self.clear_active_dialog_window(); return; }
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
        self.download_notice_rows.push(DownloadNoticeRow {
            id,
            kind: DownloadNoticeKind::Video {
                site_domain: site_domain.clone(),
            },
            file_name: format!("Video from {site_domain}"),
            status: DownloadNoticeStatus::Connecting,
        });

        let command = ytdlp_command_spec(executable, options, url.as_str(), self.path.clone());
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
                            let _ = progress_tx.send(event);
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
        if !self.download_notice_rows.is_empty() {
            return;
        }
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
        self.remove_download_task(id);
        if self.active_remote_download.as_ref().map(|active| active.id) == Some(id) {
            if let Some(handle) = self.active_dialog_window.take() {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
            self.active_remote_download = None;
            self.start_next_remote_download(cx);
            return;
        }

        self.finish_download_batch_if_idle();
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
        progress_rx: &mpsc::Receiver<DownloadProgress>,
    ) {
        let mut latest = None;
        while let Ok(progress) = progress_rx.try_recv() {
            latest = Some(progress);
        }

        if let Some(progress) = latest {
            let _ = this.update(cx, |explorer, cx| {
                if let Some(row) = explorer
                    .download_notice_rows
                    .iter_mut()
                    .find(|row| row.id == id)
                {
                    row.status = DownloadNoticeStatus::Downloading {
                        downloaded_bytes: progress.downloaded_bytes,
                        total_bytes: progress.total_bytes,
                    };
                    cx.notify();
                }
            });
        }
    }

    fn drain_ytdlp_progress(
        this: &gpui::WeakEntity<Self>,
        cx: &mut gpui::AsyncApp,
        id: u64,
        progress_rx: &mpsc::Receiver<YtDlpProgressEvent>,
    ) {
        let mut latest = None;
        while let Ok(progress) = progress_rx.try_recv() {
            latest = Some(progress);
        }

        if let Some(progress) = latest {
            let _ = this.update(cx, |explorer, cx| {
                if explorer.apply_ytdlp_progress_event(id, progress) {
                    cx.notify();
                }
            });
        }
    }

    fn apply_ytdlp_progress_event(&mut self, id: u64, event: YtDlpProgressEvent) -> bool {
        let Some(row) = self
            .download_notice_rows
            .iter_mut()
            .find(|row| row.id == id)
        else {
            return false;
        };
        row.status = match event {
            YtDlpProgressEvent::Downloading(progress) => DownloadNoticeStatus::Downloading {
                downloaded_bytes: progress.downloaded_bytes,
                total_bytes: progress.total_bytes,
            },
            YtDlpProgressEvent::PostProcessing => DownloadNoticeStatus::Connecting,
        };
        true
    }

    fn complete_download(
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

        match result {
            Ok(DownloadResult::File(path)) => {
                self.download_batch_succeeded += 1;
                let final_name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| self.download_notice_rows[row_index].file_name.clone());
                self.download_notice_rows[row_index].file_name = final_name;
                self.download_notice_rows[row_index].status = DownloadNoticeStatus::Completed;
                if path.parent() == Some(self.path.as_path()) {
                    self.reload_with_entry_metadata_resolution(cx);
                }
                self.emit_filesystem_changed(cx);
            }
            Ok(DownloadResult::Video) => {
                self.download_batch_succeeded += 1;
                self.download_notice_rows[row_index].status = DownloadNoticeStatus::Completed;
                self.reload_with_entry_metadata_resolution(cx);
                self.emit_filesystem_changed(cx);
            }
            Err(error) => {
                self.download_batch_failed += 1;
                self.download_batch_last_error = Some(error.clone());
                self.download_notice_rows[row_index].status = DownloadNoticeStatus::Failed(error);
            }
        }

        self.finish_download_batch_if_idle();
    }

    fn finish_download_batch_if_idle(&mut self) {
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
        let last_file_name = self
            .download_notice_rows
            .last()
            .map(|row| row.file_name.clone())
            .unwrap_or_default();
        let video_sites = self
            .download_notice_rows
            .iter()
            .filter_map(|row| match &row.kind {
                DownloadNoticeKind::Video { site_domain } => Some(site_domain.clone()),
                DownloadNoticeKind::File => None,
            })
            .collect::<Vec<_>>();
        let video_batch = video_sites.len() == self.download_notice_rows.len();
        let same_video_site = video_sites.first().and_then(|first| {
            video_sites
                .iter()
                .all(|site| site == first)
                .then(|| first.clone())
        });
        self.download_notice_rows.clear();
        if succeeded == 0 && failed == 0 {
            self.operation_notice = None;
            return;
        }
        self.operation_notice = Some(if failed == 0 {
            let text = if video_batch && succeeded == 1 {
                format!(
                    "Downloaded video from {}.",
                    same_video_site.as_deref().unwrap_or("multiple sites")
                )
            } else if video_batch {
                match same_video_site.as_deref() {
                    Some(site) => format!("Downloaded {succeeded} videos from {site}."),
                    None => format!("Downloaded {succeeded} videos from multiple sites."),
                }
            } else if succeeded == 1 {
                format!("Downloaded \"{last_file_name}\".")
            } else {
                format!("Downloaded {succeeded} files.")
            };
            OperationNotice::info(text)
        } else {
            let text = match (succeeded, failed) {
                (0, 1) => format!("Download failed: {last_error}"),
                (0, failed) => format!("{failed} downloads failed: {last_error}"),
                (succeeded, failed) => {
                    format!("Downloaded {succeeded} files; {failed} failed: {last_error}")
                }
            };
            OperationNotice::error(text)
        });
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
    let mut command = Command::new(&command_spec.executable);
    let _cache_invalidation = crate::explorer::remote_directory_cache::DirectoryMutation::new([
        command_spec.current_dir.clone(),
    ]);
    command
        .args(&command_spec.args)
        .current_dir(&command_spec.current_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);

    run_ytdlp_command(&mut command, process, on_progress)
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
    stdout_reader
        .map(|reader| {
            reader
                .join()
                .map_err(|_| "Could not read yt-dlp progress output.".to_owned())?
                .map_err(|error| format!("Could not read yt-dlp progress output: {error}"))
        })
        .transpose()?;
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
}

fn read_ytdlp_progress(
    reader: impl Read,
    on_progress: impl Fn(YtDlpProgressEvent),
) -> io::Result<()> {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        if let Some(event) = ytdlp_progress_event_from_line(&String::from_utf8_lossy(&line)) {
            on_progress(event);
        }
    }
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
        return Some(YtDlpProgressEvent::Downloading(DownloadProgress {
            downloaded_bytes,
            total_bytes: record.total_bytes,
        }));
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

fn ytdlp_result_from_process(
    success: bool,
    status: &str,
    stderr: &[u8],
) -> Result<DownloadResult, String> {
    if success {
        return Ok(DownloadResult::Video);
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
    format!("…{}", &message[start..])
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
            Some(YtDlpProgressEvent::Downloading(DownloadProgress {
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

    #[gpui::test]
    fn ytdlp_progress_events_update_exact_unknown_and_postprocess_states(cx: &mut TestAppContext) {
        let temp = tempfile::tempdir().expect("temp directory");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());
        cx.update(|_, app| {
            view.update(app, |view, _| {
                view.download_notice_rows = vec![DownloadNoticeRow {
                    id: 7,
                    kind: DownloadNoticeKind::Video {
                        site_domain: "youtube.com".to_owned(),
                    },
                    file_name: "Video from youtube.com".to_owned(),
                    status: DownloadNoticeStatus::Connecting,
                }];

                assert!(view.apply_ytdlp_progress_event(
                    7,
                    YtDlpProgressEvent::Downloading(DownloadProgress {
                        downloaded_bytes: 25,
                        total_bytes: Some(100),
                    })
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
                    })
                ));
                assert_eq!(
                    view.download_notice_rows[0].status,
                    DownloadNoticeStatus::Downloading {
                        downloaded_bytes: 50,
                        total_bytes: None,
                    }
                );

                assert!(view.apply_ytdlp_progress_event(7, YtDlpProgressEvent::PostProcessing));
                assert_eq!(
                    view.download_notice_rows[0].status,
                    DownloadNoticeStatus::Connecting
                );
                assert!(!view.apply_ytdlp_progress_event(99, YtDlpProgressEvent::PostProcessing));
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
        let Some(_) = std::env::var_os(YTDLP_TEST_MODE) else {
            return;
        };
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
    fn cancelling_one_video_keeps_other_downloads_and_excludes_it_from_summary(
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
                        id: 1,
                        kind: DownloadNoticeKind::Video {
                            site_domain: "youtube.com".to_owned(),
                        },
                        file_name: "Video from youtube.com".to_owned(),
                        status: DownloadNoticeStatus::Connecting,
                    },
                    DownloadNoticeRow {
                        id: 2,
                        kind: DownloadNoticeKind::Video {
                            site_domain: "vimeo.com".to_owned(),
                        },
                        file_name: "Video from vimeo.com".to_owned(),
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
                view.complete_download(1, Ok(DownloadResult::Video), cx);
            });
        });

        assert!(cancelled_state.cancelled.load(Ordering::Relaxed));
        assert!(continuing_state.cancelled.load(Ordering::Relaxed));
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.download_batch_succeeded, 1);
            assert_eq!(view.download_batch_failed, 0);
            assert_eq!(
                view.operation_notice
                    .as_ref()
                    .map(|notice| notice.text.as_str()),
                Some("Downloaded video from youtube.com.")
            );
        });
    }

    #[gpui::test]
    fn same_site_video_downloads_share_the_existing_batch_summary(cx: &mut TestAppContext) {
        let temp = tempfile::tempdir().expect("temp directory");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.download_notice_rows = vec![
                    DownloadNoticeRow {
                        id: 1,
                        kind: DownloadNoticeKind::Video {
                            site_domain: "vimeo.com".to_owned(),
                        },
                        file_name: "Video from vimeo.com".to_owned(),
                        status: DownloadNoticeStatus::Connecting,
                    },
                    DownloadNoticeRow {
                        id: 2,
                        kind: DownloadNoticeKind::Video {
                            site_domain: "vimeo.com".to_owned(),
                        },
                        file_name: "Video from vimeo.com".to_owned(),
                        status: DownloadNoticeStatus::Connecting,
                    },
                ];
                view.complete_download(1, Ok(DownloadResult::Video), cx);
                assert!(view.operation_notice.is_none());
                view.complete_download(2, Ok(DownloadResult::Video), cx);
            });
        });

        cx.read_entity(&view, |view, _| {
            assert!(view.download_notice_rows.is_empty());
            assert_eq!(view.download_batch_succeeded, 2);
            assert_eq!(
                view.operation_notice
                    .as_ref()
                    .map(|notice| notice.text.as_str()),
                Some("Downloaded 2 videos from vimeo.com.")
            );
        });
    }

    #[gpui::test]
    fn mixed_site_video_downloads_use_a_generic_batch_summary(cx: &mut TestAppContext) {
        let temp = tempfile::tempdir().expect("temp directory");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.download_notice_rows = vec![
                    DownloadNoticeRow {
                        id: 1,
                        kind: DownloadNoticeKind::Video {
                            site_domain: "vimeo.com".to_owned(),
                        },
                        file_name: "Video from vimeo.com".to_owned(),
                        status: DownloadNoticeStatus::Connecting,
                    },
                    DownloadNoticeRow {
                        id: 2,
                        kind: DownloadNoticeKind::Video {
                            site_domain: "dailymotion.com".to_owned(),
                        },
                        file_name: "Video from dailymotion.com".to_owned(),
                        status: DownloadNoticeStatus::Connecting,
                    },
                ];
                view.complete_download(1, Ok(DownloadResult::Video), cx);
                view.complete_download(2, Ok(DownloadResult::Video), cx);
            });
        });

        cx.read_entity(&view, |view, _| {
            assert_eq!(
                view.operation_notice
                    .as_ref()
                    .map(|notice| notice.text.as_str()),
                Some("Downloaded 2 videos from multiple sites.")
            );
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
    fn clipboard_url_paste_downloads_and_reports_completion(cx: &mut TestAppContext) {
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
            view.update(app, |view, cx| view.paste_clipboard(window, cx));
        });
        cx.run_until_parked();
        cx.executor().advance_clock(DOWNLOAD_PROGRESS_INTERVAL);
        cx.run_until_parked();

        assert_eq!(
            std::fs::read(destination.join("file.zip")).unwrap(),
            b"data"
        );
        cx.read_entity(&view, |view, _| {
            assert!(view.download_notice_rows.is_empty());
            assert_eq!(
                view.operation_notice
                    .as_ref()
                    .map(|notice| notice.text.as_str()),
                Some("Downloaded \"file.zip\".")
            );
        });
    }

    #[gpui::test]
    fn overlapping_downloads_start_concurrently_and_share_a_final_summary(cx: &mut TestAppContext) {
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
            assert!(view.download_notice_rows.is_empty());
            assert_eq!(
                view.operation_notice
                    .as_ref()
                    .map(|notice| notice.text.as_str()),
                Some("Downloaded 2 files.")
            );
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
                    id: 7,
                    kind: DownloadNoticeKind::File,
                    file_name: "partial.zip".to_owned(),
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
    fn cancelling_one_download_excludes_it_from_the_batch_summary(cx: &mut TestAppContext) {
        let temp = tempfile::tempdir().expect("temp directory");
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.download_notice_rows = vec![
                    DownloadNoticeRow {
                        id: 1,
                        kind: DownloadNoticeKind::File,
                        file_name: "complete.zip".to_owned(),
                        status: DownloadNoticeStatus::Completed,
                    },
                    DownloadNoticeRow {
                        id: 2,
                        kind: DownloadNoticeKind::File,
                        file_name: "cancel.zip".to_owned(),
                        status: DownloadNoticeStatus::Connecting,
                    },
                ];
                view.download_batch_succeeded = 1;
                view.download_tasks.push((2, gpui::Task::ready(())));
                view.cancel_download(2, cx);
            });
        });

        cx.read_entity(&view, |view, _| {
            assert!(view.download_notice_rows.is_empty());
            assert_eq!(
                view.operation_notice
                    .as_ref()
                    .map(|notice| notice.text.as_str()),
                Some("Downloaded \"complete.zip\".")
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
                    id: 3,
                    kind: DownloadNoticeKind::File,
                    file_name: "complete.zip".to_owned(),
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
