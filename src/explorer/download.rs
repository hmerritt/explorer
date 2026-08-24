use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use futures::AsyncReadExt;
use gpui::{Context, http_client::HttpClient};
use tempfile::NamedTempFile;

use crate::explorer::{
    clipboard::ClipboardDownload,
    portable_devices,
    view::{ExplorerView, OperationNotice},
};

const DOWNLOAD_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const DOWNLOAD_BUFFER_SIZE: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DownloadNoticeRow {
    pub(super) id: u64,
    pub(super) file_name: String,
    pub(super) status: DownloadNoticeStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum DownloadNoticeStatus {
    Connecting,
    Downloading {
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Completed,
    Failed(String),
}

impl DownloadNoticeStatus {
    pub(super) fn is_active(&self) -> bool {
        matches!(self, Self::Connecting | Self::Downloading { .. })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DownloadProgress {
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
}

struct PendingDownload {
    temporary: NamedTempFile,
    destination: PathBuf,
    file_name: String,
}

#[derive(Debug)]
struct DownloadResult {
    path: PathBuf,
}

impl PendingDownload {
    fn persist(mut self) -> Result<DownloadResult, String> {
        let mut index = 1usize;
        loop {
            let file_name = download_file_name(&self.file_name, index);
            let path = self.destination.join(&file_name);
            match self.temporary.persist_noclobber(&path) {
                Ok(_) => return Ok(DownloadResult { path }),
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

        if self.download_notice_rows.is_empty() {
            self.download_tasks.clear();
            self.download_batch_succeeded = 0;
            self.download_batch_failed = 0;
            self.download_batch_last_error = None;
            self.clear_operation_notice();
        }

        let id = self.next_download_id;
        self.next_download_id = self.next_download_id.wrapping_add(1);
        self.download_notice_rows.push(DownloadNoticeRow {
            id,
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

    pub(super) fn cancel_download(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(row_index) = self
            .download_notice_rows
            .iter()
            .position(|row| row.id == id && row.status.is_active())
        else {
            return;
        };

        self.download_notice_rows.remove(row_index);
        if let Some(task_index) = self
            .download_tasks
            .iter()
            .position(|(task_id, _)| *task_id == id)
        {
            let (_, task) = self.download_tasks.swap_remove(task_index);
            drop(task);
        }

        self.finish_download_batch_if_idle();
        cx.notify();
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
            Ok(result) => {
                self.download_batch_succeeded += 1;
                let final_name = result
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| self.download_notice_rows[row_index].file_name.clone());
                self.download_notice_rows[row_index].file_name = final_name;
                self.download_notice_rows[row_index].status = DownloadNoticeStatus::Completed;
                if result.path.parent() == Some(self.path.as_path()) {
                    self.reload_with_entry_metadata_resolution(cx);
                }
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
        self.download_notice_rows.clear();
        if succeeded == 0 && failed == 0 {
            self.operation_notice = None;
            return;
        }
        self.operation_notice = Some(if failed == 0 {
            let text = if succeeded == 1 {
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

        assert_eq!(std::fs::read(result.path).unwrap(), b"streamed body");
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

        assert_eq!(result.path.file_name().unwrap(), "file (2).zip");
        assert_eq!(
            std::fs::read(temp.path().join("file.zip")).unwrap(),
            b"existing"
        );
        assert_eq!(std::fs::read(result.path).unwrap(), b"new");
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
                        file_name: "complete.zip".to_owned(),
                        status: DownloadNoticeStatus::Completed,
                    },
                    DownloadNoticeRow {
                        id: 2,
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
