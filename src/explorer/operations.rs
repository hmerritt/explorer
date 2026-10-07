//! Session-wide FIFO scheduling. Views submit intent; only the head prepares work.
use super::{
    filesystem::{
        self, ConflictChoice, FileConflictBatch, FileOperationError, FileOperationKind,
        FileOperationProgress, FileOperationSummary, PreparedFileOperation,
    },
    formatting::{format_size, format_transfer_rate, format_transfer_remaining},
    operation_control::OperationControl,
    remote_transfer::{self, State as ServerState},
    trash::{self, BatchResult, RecoveryChoice, RecoveryRequest, TrashItemId},
    view::ExplorerView,
};
use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext, ClickEvent, Context, Entity, FocusHandle,
    Focusable, Global, IntoElement, Render, SharedString, Task, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(super) enum Request {
    Files {
        sources: Vec<PathBuf>,
        destination: PathBuf,
        kind: FileOperationKind,
        paste: bool,
    },
    Drop {
        dragged: super::drag_drop::DraggedEntries,
        destination: super::drag_drop::DropDestination,
        directory: PathBuf,
        modifiers: gpui::Modifiers,
    },
    Compress(Vec<PathBuf>),
    Extract {
        sources: Vec<PathBuf>,
        destination: PathBuf,
    },
    Portable {
        sources: Vec<PathBuf>,
        destination: PathBuf,
        moving: bool,
    },
    Trash(Vec<PathBuf>),
    Delete(Vec<PathBuf>),
    Recover(RecoveryRequest),
    Purge(Vec<TrashItemId>),
    Server(u64),
    ServerCleanup(u64),
    DiscardCopy(Vec<(PathBuf, PathBuf)>),
}

impl Request {
    fn title(&self) -> &'static str {
        match self {
            Self::Drop { .. } => "File operation",
            Self::Files { kind, .. } => kind.progress_title(),
            Self::Compress(_) => "Compressing",
            Self::Extract { .. } => "Extracting",
            Self::Portable { moving: true, .. } => "Moving",
            Self::Portable { .. } | Self::Server(_) => "Transferring",
            Self::Trash(_) => "Moving to bin",
            Self::ServerCleanup(_) | Self::DiscardCopy(_) => "Discarding saved progress",
            Self::Delete(_) | Self::Purge(_) => "Deleting",
            Self::Recover(_) => "Restoring",
        }
    }
    pub(super) fn locations(&self) -> (Vec<PathBuf>, Option<PathBuf>) {
        match self {
            Self::Files {
                sources,
                destination,
                ..
            }
            | Self::Extract {
                sources,
                destination,
            }
            | Self::Portable {
                sources,
                destination,
                ..
            } => (sources.clone(), Some(destination.clone())),
            Self::Drop {
                dragged,
                destination,
                directory,
                ..
            } => (dragged.paths.clone(), Some(destination.resolve(directory))),
            Self::Compress(paths) => (
                paths.clone(),
                paths.first().and_then(|p| p.parent().map(PathBuf::from)),
            ),
            Self::Trash(paths) => (paths.clone(), Some(trash::root())),
            Self::Delete(paths) => (paths.clone(), None),
            Self::Recover(request) => (
                request.ids.iter().map(trash::item_path).collect(),
                request.directory.clone(),
            ),
            Self::Purge(ids) => (ids.iter().map(trash::item_path).collect(), None),
            Self::DiscardCopy(targets) => (
                targets.iter().map(|(_, target)| target.clone()).collect(),
                None,
            ),
            Self::Server(id) | Self::ServerCleanup(id) => remote_transfer::snapshots()
                .into_iter()
                .find(|s| s.id == *id)
                .map(|s| (s.source_reveal.paths, Some(s.destination_reveal.directory)))
                .unwrap_or_default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum State {
    Queued,
    Preparing,
    Running,
    Pausing,
    Paused,
    Attention,
    Cancelling,
    Completed,
    Cancelled,
    Failed,
}
impl State {
    fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }
    fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Preparing => "Preparing…",
            Self::Running => "Running",
            Self::Pausing => "Pausing…",
            Self::Paused => "Paused",
            Self::Attention => "Needs attention",
            Self::Cancelling => "Cancelling…",
            Self::Completed => "Completed",
            Self::Cancelled => "Cancelled",
            Self::Failed => "Failed",
        }
    }
}

enum Event {
    Progress(FileOperationProgress),
    Items {
        done: usize,
        total: usize,
        name: String,
    },
    Conflict(FileConflictBatch),
    RecoveryConflict,
    ChangedTargets,
    RetainedTargets(Vec<(PathBuf, PathBuf)>),
    Done(Result<Outcome, String>),
}

pub(super) enum Outcome {
    Files(FileOperationSummary),
    Trash(Option<super::file_commands::FileOperationUndo>),
    Deleted {
        paths: Vec<PathBuf>,
        failures: Vec<String>,
    },
    Portable {
        destinations: Vec<PathBuf>,
        moved: Vec<PathBuf>,
        failures: Vec<String>,
    },
    Bin(BatchResult),
    Cancelled,
    Cleaned,
}

#[derive(Clone, Copy)]
enum Resolution {
    Replace,
    Skip,
    KeepBoth,
    Confirm,
}

struct Job {
    id: u64,
    request: Request,
    sources: Vec<PathBuf>,
    destination: Option<PathBuf>,
    origin: Option<Entity<ExplorerView>>,
    origin_path: PathBuf,
    state: State,
    control: Arc<OperationControl>,
    progress: Option<FileOperationProgress>,
    message: String,
    done: usize,
    total: usize,
    current: String,
    events: Option<mpsc::Receiver<Event>>,
    resolution: Option<mpsc::Sender<Resolution>>,
    conflict: Option<FileConflictBatch>,
    recovery_conflict: bool,
    changed_targets: bool,
    worker: Option<Task<()>>,
    rate: RateTracker,
    details: bool,
    copy_verify: bool,
    identities: Vec<Option<TargetIdentity>>,
    selection_before: Vec<PathBuf>,
    after_delete: Option<PathBuf>,
    retained_targets: Vec<(PathBuf, PathBuf)>,
    server_snapshot: Option<remote_transfer::JobSnapshot>,
    resume_after_pause: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TargetIdentity {
    identity: Option<String>,
    directory: bool,
    size: Option<u64>,
    modified: Option<std::time::SystemTime>,
}
impl TargetIdentity {
    fn read(path: &std::path::Path) -> Option<Self> {
        if super::remote_fs::is_remote(path) {
            let metadata = super::remote_fs::metadata(path).ok()?;
            let directory = metadata.is_dir();
            return Some(Self {
                identity: None,
                directory,
                size: if directory { None } else { metadata.size },
                modified: metadata
                    .mtime
                    .map(|seconds| std::time::UNIX_EPOCH + Duration::from_secs(seconds.into())),
            });
        }
        if super::portable_devices::is_portable_path(path) {
            let metadata = super::portable_devices::metadata(path)?;
            return Some(Self {
                identity: Some(path.display().to_string()),
                directory: metadata.is_directory,
                size: metadata.size,
                modified: metadata.modified,
            });
        }
        let metadata = std::fs::symlink_metadata(path).ok()?;
        Some(Self {
            identity: trash::fingerprint(path).ok(),
            directory: metadata.is_dir(),
            size: (!metadata.is_dir()).then_some(metadata.len()),
            modified: metadata.modified().ok(),
        })
    }
    fn captured(path: &std::path::Path, entries: &[super::entry::FileEntry]) -> Option<Self> {
        if super::remote_fs::is_remote(path) {
            let entry = entries.iter().find(|entry| entry.path == path)?;
            return Some(Self {
                identity: None,
                directory: entry.is_real_directory(),
                size: entry.size,
                modified: entry.modified,
            });
        }
        Self::read(path)
    }
}

pub(super) struct Operations {
    jobs: Vec<Job>,
    next_id: u64,
    managed_servers: Vec<u64>,
    active: Option<u64>,
    window: Option<AnyWindowHandle>,
    dialog: Option<AnyWindowHandle>,
    poll: Option<Task<()>>,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    previous_quit_policy: Option<bool>,
}
struct OperationsGlobal(Entity<Operations>);
impl Global for OperationsGlobal {}
#[derive(Default)]
pub(super) struct OperationsRevision {
    pub filesystem: u64,
    pub outstanding: usize,
    managed_servers: Vec<u64>,
}
impl Global for OperationsRevision {}

pub(super) fn manager(cx: &mut App) -> Entity<Operations> {
    if let Some(global) = cx.try_global::<OperationsGlobal>() {
        return global.0.clone();
    }
    initialize(cx);
    cx.global::<OperationsGlobal>().0.clone()
}

pub(crate) fn initialize(cx: &mut App) {
    if cx.has_global::<OperationsGlobal>() {
        return;
    }
    cx.set_global(OperationsRevision::default());
    let entity = cx.new(|cx: &mut Context<Operations>| {
        let mut manager = Operations {
            jobs: Vec::new(),
            next_id: 1,
            managed_servers: Vec::new(),
            active: None,
            window: None,
            dialog: None,
            poll: None,
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            previous_quit_policy: None,
        };
        manager.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                if this.update(cx, |manager, cx| manager.tick(cx)).is_err() {
                    break;
                }
            }
        }));
        manager
    });
    cx.set_global(OperationsGlobal(entity.clone()));
    entity.update(cx, |manager, cx| {
        for snapshot in remote_transfer::snapshots() {
            if matches!(
                snapshot.state,
                ServerState::Completed | ServerState::Cancelled
            ) && snapshot.auto_dismiss
            {
                continue;
            }
            let id = manager.insert(Request::Server(snapshot.id), None, PathBuf::new(), true, cx);
            let job = manager.jobs.iter_mut().find(|j| j.id == id).unwrap();
            job.state = match snapshot.state {
                ServerState::Completed => State::Completed,
                ServerState::Cancelled => State::Cancelled,
                _ => State::Paused,
            };
            job.message = "Recovered transfer. Resume or discard saved progress.".into();
        }
        if let Some(job) = manager.jobs.iter().find(|j| !j.state.terminal()) {
            manager.active = Some(job.id);
        }
        manager.notify(false, cx);
    });
    cx.on_app_quit(|cx| {
        if let Some(global) = cx.try_global::<OperationsGlobal>() {
            let entity = global.0.clone();
            entity.update(cx, |manager, cx| {
                let settings = sftp_settings(cx);
                for job in &manager.jobs {
                    job.control.cancel();
                    if let Request::Server(id) = job.request {
                        remote_transfer::control(id, "pause", settings);
                    }
                }
            });
        }
        async {}
    })
    .detach();
}

fn sftp_settings(cx: &App) -> crate::settings::SftpSettings {
    cx.try_global::<crate::settings::SettingsState>()
        .map(|s| s.value.sftp)
        .unwrap_or_default()
}

pub(super) fn outstanding(cx: &App) -> usize {
    cx.try_global::<OperationsRevision>()
        .map_or(0, |revision| revision.outstanding)
}

pub(super) fn manages_server(id: u64, cx: &App) -> bool {
    cx.try_global::<OperationsRevision>()
        .is_some_and(|revision| revision.managed_servers.contains(&id))
}

pub(super) fn show(cx: &mut App) {
    manager(cx).update(cx, |manager, cx| manager.open_window(cx));
}
pub(super) fn submit(
    request: Request,
    origin: Entity<ExplorerView>,
    path: PathBuf,
    verify: bool,
    entries: &[super::entry::FileEntry],
    selection_before: Vec<PathBuf>,
    after_delete: Option<PathBuf>,
    cx: &mut App,
) {
    let sources = request.locations().0;
    let identities = if matches!(request, Request::Delete(_) | Request::Trash(_)) {
        sources
            .iter()
            .map(|path| TargetIdentity::captured(path, entries))
            .collect()
    } else {
        Vec::new()
    };
    manager(cx).update(cx, |manager, cx| {
        let first = manager.jobs.iter().all(|j| j.state.terminal());
        let id = manager.insert(request, Some(origin), path, verify, cx);
        let job = manager.jobs.iter_mut().find(|j| j.id == id).unwrap();
        job.identities = identities;
        job.selection_before = selection_before;
        job.after_delete = after_delete;
        manager.start_next(cx);
        if first {
            manager.open_window(cx);
        }
        manager.notify(false, cx);
    });
}

pub(super) fn control_server(id: u64, action: &'static str, cx: &mut App) {
    manager(cx).update(cx, |manager, cx| {
        if let Some(job) = manager.jobs.iter().rev().find(|j| matches!(j.request, Request::Server(server) | Request::ServerCleanup(server) if server == id) && !j.state.terminal()) {
            let job_id = job.id;
            manager.control(job_id, action, cx);
        }
    });
}

impl Operations {
    fn insert(
        &mut self,
        request: Request,
        origin: Option<Entity<ExplorerView>>,
        origin_path: PathBuf,
        copy_verify: bool,
        cx: &App,
    ) -> u64 {
        if let Request::Server(id) | Request::ServerCleanup(id) = request {
            if !self.managed_servers.contains(&id) {
                self.managed_servers.push(id);
            }
        }
        let server_snapshot = match request {
            Request::Server(id) | Request::ServerCleanup(id) => remote_transfer::snapshots()
                .into_iter()
                .find(|s| s.id == id),
            _ => None,
        };
        let id = self.next_id;
        self.next_id += 1;
        let (sources, destination) = request.locations();
        let total = sources.len();
        let identities = Vec::new();
        let _ = cx;
        self.jobs.push(Job {
            id,
            request,
            sources,
            destination,
            origin,
            origin_path,
            state: State::Queued,
            control: OperationControl::new(),
            progress: None,
            message: String::new(),
            done: 0,
            total,
            current: String::new(),
            events: None,
            resolution: None,
            conflict: None,
            recovery_conflict: false,
            changed_targets: false,
            worker: None,
            rate: RateTracker::default(),
            details: false,
            copy_verify,
            identities,
            selection_before: Vec::new(),
            after_delete: None,
            retained_targets: Vec::new(),
            server_snapshot,
            resume_after_pause: false,
        });
        id
    }
    fn notify(&mut self, filesystem_changed: bool, cx: &mut Context<Self>) {
        let count = self.jobs.iter().filter(|j| !j.state.terminal()).count();
        let managed_servers = self.managed_servers.clone();
        cx.update_global::<OperationsRevision, _>(|revision, _| {
            revision.managed_servers = managed_servers;
            revision.outstanding = count;
            if filesystem_changed {
                revision.filesystem += 1;
            }
        });
        self.update_lifecycle(cx);
        cx.notify();
    }
    fn start_next(&mut self, cx: &mut Context<Self>) {
        if self.active.is_some() {
            return;
        }
        let Some(job) = self.jobs.iter_mut().find(|j| !j.state.terminal()) else {
            return;
        };
        self.active = Some(job.id);
        if job.state == State::Paused {
            return;
        }
        job.state = State::Preparing;
        if let Request::Server(id) | Request::ServerCleanup(id) = job.request {
            let action = if matches!(job.request, Request::ServerCleanup(_)) {
                "discard"
            } else {
                "resume"
            };
            remote_transfer::control(id, action, sftp_settings(cx));
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (resolution_tx, resolution_rx) = mpsc::channel();
        job.events = Some(rx);
        job.resolution = Some(resolution_tx);
        let request = job.request.clone();
        let control = job.control.clone();
        let verify = job.copy_verify;
        let identities = job.identities.clone();
        let (finished_tx, finished_rx) = futures::channel::oneshot::channel();
        let spawn = std::thread::Builder::new()
            .name(format!("explorer-operation-{}", job.id))
            .spawn(move || {
                let _worker = control.enter();
                let report_tx = tx.clone();
                control.set_items_report(Arc::new(move |done, total, name| {
                    let _ = report_tx.send(Event::Items {
                        done,
                        total,
                        name: name.into(),
                    });
                }));
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(request, identities, verify, &control, &tx, &resolution_rx)
                }))
                .unwrap_or_else(|_| Err("File operation worker panicked.".into()));
                let _ = tx.send(Event::Done(result));
                let _ = finished_tx.send(());
            });
        if let Err(error) = spawn {
            job.state = State::Attention;
            job.message = format!("Could not start worker: {error}");
            return;
        }
        job.worker = Some(cx.spawn(async move |this, cx| {
            let _ = finished_rx.await;
            let _ = this.update(cx, |manager, cx| manager.tick(cx));
        }));
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active else {
            return;
        };
        let Some(index) = self.jobs.iter().position(|j| j.id == id) else {
            return;
        };
        if let Request::Server(server) | Request::ServerCleanup(server) = self.jobs[index].request {
            if let Some(snapshot) = remote_transfer::snapshots()
                .into_iter()
                .find(|s| s.id == server)
            {
                let job = &mut self.jobs[index];
                // Recovered queued jobs remain paused until explicitly resumed.
                if job.resume_after_pause
                    && snapshot.state == ServerState::Paused
                    && !remote_transfer::is_running(server)
                {
                    job.resume_after_pause = false;
                    job.rate = RateTracker::default();
                    remote_transfer::control(server, "resume", sftp_settings(cx));
                    job.state = State::Preparing;
                    cx.notify();
                    return;
                }
                if job.state == State::Paused && snapshot.state == ServerState::Paused {
                    return;
                }
                let previous_state = job.state;
                job.server_snapshot = Some(snapshot.clone());
                job.message = snapshot.message.clone();
                if !snapshot.warnings.is_empty() {
                    job.message
                        .push_str(&format!("\n{}", snapshot.warnings.join("\n")));
                }
                job.current = snapshot.title();
                job.total = snapshot.files();
                job.done = snapshot.current;
                job.state = match snapshot.state {
                    ServerState::Paused => State::Paused,
                    ServerState::Attention => State::Attention,
                    ServerState::Completed => State::Completed,
                    ServerState::Cancelled => State::Cancelled,
                    ServerState::Queued | ServerState::Preparing | ServerState::Connecting => {
                        State::Preparing
                    }
                    _ if job.state == State::Pausing => State::Pausing,
                    _ if job.state == State::Cancelling => State::Cancelling,
                    _ => State::Running,
                };
                if matches!(snapshot.state, ServerState::Transferring) {
                    job.progress = Some(FileOperationProgress {
                        kind: FileOperationKind::Copy,
                        phase: filesystem::FileOperationPhase::Copying,
                        total_bytes: snapshot.total,
                        copied_bytes: snapshot.bytes,
                        verified_bytes: 0,
                        work_total_bytes: snapshot.total,
                        work_completed_bytes: snapshot.bytes,
                        total_files: snapshot.files(),
                        completed_files: snapshot.current,
                        current_item: None,
                        cancellable: true,
                    });
                } else if matches!(
                    job.state,
                    State::Preparing | State::Completed | State::Cancelled
                ) {
                    job.progress = None;
                }
                job.rate
                    .record(job.progress.as_ref(), job.state, Instant::now());
                if matches!(job.state, State::Paused | State::Attention)
                    && remote_transfer::is_running(server)
                {
                    job.state = State::Pausing;
                }
                if job.state.terminal() && remote_transfer::is_running(server) {
                    return;
                }
                if job.state.terminal() {
                    if let Some(origin) = job.origin.take() {
                        cx.defer(move |cx| {
                            origin.update(cx, |view, cx| {
                                view.remove_cut_paths(&snapshot.moved_sources);
                                view.refresh_with_entry_metadata_resolution(cx);
                                view.emit_filesystem_changed(cx);
                            })
                        });
                    }
                    self.active = None;
                    self.notify(true, cx);
                    self.start_next(cx);
                } else if matches!(job.state, State::Paused | State::Attention)
                    && previous_state != job.state
                {
                    if let Some(origin) = job.origin.clone() {
                        let moved = snapshot.moved_sources.clone();
                        cx.defer(move |cx| {
                            origin.update(cx, |view, cx| {
                                view.remove_cut_paths(&moved);
                                view.refresh_with_entry_metadata_resolution(cx);
                                view.emit_filesystem_changed(cx);
                            })
                        });
                    }
                    self.notify(true, cx);
                } else {
                    cx.notify();
                }
            } else if matches!(self.jobs[index].request, Request::ServerCleanup(_)) {
                for job in &mut self.jobs {
                    if job.server_snapshot.as_ref().is_some_and(|s| s.id == server) {
                        job.server_snapshot = None;
                    }
                }
                self.jobs[index].state = State::Completed;
                self.active = None;
                self.notify(true, cx);
                self.start_next(cx);
            } else {
                self.jobs[index].state = State::Attention;
                self.jobs[index].message =
                    "The saved transfer is unavailable. Cancel this operation to continue.".into();
                cx.notify();
            }
            return;
        }
        let mut events = Vec::new();
        if let Some(rx) = &self.jobs[index].events {
            events.extend(rx.try_iter());
        }
        for event in events {
            let job = &mut self.jobs[index];
            match event {
                Event::Progress(progress) => {
                    job.done = progress.completed_files;
                    job.total = progress.total_files;
                    job.current = progress
                        .current_item
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
                    if !matches!(job.state, State::Cancelling | State::Attention) {
                        job.state = State::Running;
                    }
                    if matches!(
                        progress.kind,
                        FileOperationKind::Compress | FileOperationKind::Extract
                    ) && progress.total_files > 0
                        && progress.completed_files >= progress.total_files
                    {
                        job.control
                            .pause_available
                            .store(false, std::sync::atomic::Ordering::Release);
                    }
                    job.progress = Some(progress);
                }
                Event::Items { done, total, name } => {
                    job.done = done;
                    job.total = total;
                    job.current = name;
                    if !matches!(job.state, State::Cancelling | State::Attention) {
                        job.state = State::Running;
                    }
                }
                Event::Conflict(conflicts) => {
                    job.state = State::Attention;
                    job.message = "Files already exist in the destination.".into();
                    job.conflict = Some(conflicts);
                }
                Event::RecoveryConflict => {
                    job.state = State::Attention;
                    job.message = "Restore destinations already contain these items.".into();
                    job.recovery_conflict = true;
                }
                Event::ChangedTargets => {
                    job.state = State::Attention;
                    job.message = "Selected items changed while queued. Confirm the current items before deleting.".into();
                    job.changed_targets = true;
                }
                Event::RetainedTargets(targets) => job.retained_targets = targets,
                Event::Done(result) => {
                    job.worker = None;
                    job.events = None;
                    job.resolution = None;
                    match result {
                        Ok(outcome) => {
                            let cancelled = job
                                .control
                                .cancel
                                .load(std::sync::atomic::Ordering::Acquire)
                                || matches!(outcome, Outcome::Cancelled)
                                || matches!(&outcome, Outcome::Bin(result) if result.cancelled);
                            let failure = match &outcome {
                                Outcome::Deleted { failures, .. }
                                | Outcome::Portable { failures, .. }
                                    if !failures.is_empty() =>
                                {
                                    Some(failures.join("\n"))
                                }
                                Outcome::Bin(result) if !result.failures.is_empty() => {
                                    Some(result.failures.join("\n"))
                                }
                                Outcome::Trash(Some(
                                    super::file_commands::FileOperationUndo::Trash(
                                        super::file_commands::TrashUndo::Native {
                                            failures, ..
                                        },
                                    ),
                                )) if !failures.is_empty() => Some(failures.join("\n")),
                                _ => None,
                            };
                            job.state = if failure.is_some() {
                                State::Attention
                            } else if cancelled {
                                State::Cancelled
                            } else {
                                State::Completed
                            };
                            if job.state == State::Completed {
                                job.retained_targets.clear();
                                job.done = job.total;
                            }
                            if let Some(message) = failure {
                                job.message = message;
                            }
                            if let Some(origin) = job.origin.take() {
                                let path = job.origin_path.clone();
                                let sources = job.sources.clone();
                                let selection = job.selection_before.clone();
                                let after = job.after_delete.clone();
                                cx.defer(move |cx| {
                                    origin.update(cx, |view, cx| {
                                        view.complete_queued_operation(
                                            outcome, &path, &sources, &selection, after, cx,
                                        )
                                    })
                                });
                            }
                        }
                        Err(error) => {
                            job.state = if job.state == State::Cancelling {
                                State::Cancelled
                            } else {
                                State::Attention
                            };
                            job.message = error;
                            if let Some(origin) = job.origin.take() {
                                cx.defer(move |cx| {
                                    origin.update(cx, |view, cx| {
                                        view.refresh_with_entry_metadata_resolution(cx);
                                        view.emit_filesystem_changed(cx);
                                    })
                                });
                            }
                        }
                    }
                    if let Some(handle) = self.dialog.take() {
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                    }
                    if job.state.terminal() {
                        self.active = None;
                    }
                    self.notify(true, cx);
                }
            }
        }
        if self.jobs[index].state == State::Completed {
            if let Request::DiscardCopy(targets) = &self.jobs[index].request {
                let targets = targets.clone();
                for job in &mut self.jobs {
                    job.retained_targets
                        .retain(|target| !targets.contains(target));
                }
            }
        }
        let job = &mut self.jobs[index];
        if !job.state.terminal() && !matches!(job.state, State::Attention | State::Cancelling) {
            if job.control.pausing() {
                job.state = if job.control.paused() {
                    State::Paused
                } else {
                    State::Pausing
                };
            } else if matches!(job.state, State::Paused | State::Pausing) {
                job.state = State::Running;
            }
        }
        job.rate
            .record(job.progress.as_ref(), job.state, Instant::now());
        self.start_next(cx);
        cx.notify();
    }

    pub(super) fn control(&mut self, id: u64, action: &str, cx: &mut Context<Self>) {
        if action == "resolve" {
            self.open_attention(id, cx);
            return;
        }
        if (action == "discard" || action == "resume_saved")
            && self.jobs.iter().any(|j| j.id == id && j.state.terminal())
        {
            let Some(job) = self.jobs.iter().find(|j| j.id == id && j.state.terminal()) else {
                return;
            };
            let request = if let Request::Server(server) = job.request {
                if self.jobs.iter().any(|j| !j.state.terminal() && matches!(j.request, Request::Server(other) | Request::ServerCleanup(other) if other == server)) {return;}
                if action == "resume_saved" {
                    Request::Server(server)
                } else {
                    Request::ServerCleanup(server)
                }
            } else if action == "discard" {
                Request::DiscardCopy(job.retained_targets.clone())
            } else {
                return;
            };
            self.insert(request, None, PathBuf::new(), true, cx);
            self.start_next(cx);
            self.notify(false, cx);
            return;
        }
        if action == "discard" {
            if let Some(index) = self
                .jobs
                .iter()
                .position(|j| j.id == id && matches!(j.state, State::Paused | State::Attention))
            {
                if let Request::Server(server) = self.jobs[index].request {
                    if remote_transfer::is_running(server) {
                        return;
                    }
                    self.jobs[index].state = State::Cancelled;
                    self.jobs[index].origin = None;
                    if self.active == Some(id) {
                        self.active = None;
                    }
                    self.insert(
                        Request::ServerCleanup(server),
                        None,
                        PathBuf::new(),
                        true,
                        cx,
                    );
                    self.start_next(cx);
                    self.notify(false, cx);
                    return;
                }
            }
        }
        let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) else {
            return;
        };
        if action == "details" {
            job.details = !job.details;
            cx.notify();
            return;
        }
        if action == "dismiss" && job.state.terminal() {
            self.jobs.retain(|j| j.id != id);
            self.notify(false, cx);
            return;
        }
        if let Request::Server(server) | Request::ServerCleanup(server) = job.request {
            if action == "resume"
                && self.active == Some(id)
                && job.state == State::Pausing
                && remote_transfer::is_running(server)
            {
                job.resume_after_pause = true;
                cx.notify();
                return;
            }
            if action == "cancel" {
                job.resume_after_pause = false;
            }
            if action == "cancel" && self.active != Some(id) {
                remote_transfer::control(server, "cancel", sftp_settings(cx));
                job.state = State::Cancelled;
                job.origin = None;
            } else if self.active == Some(id) {
                let action =
                    if matches!(job.request, Request::ServerCleanup(_)) && action == "resume" {
                        "discard"
                    } else {
                        action
                    };
                remote_transfer::control(server, action, sftp_settings(cx));
                job.state = match action {
                    "pause" => State::Pausing,
                    "cancel" => State::Cancelling,
                    _ => State::Running,
                };
            }
            self.notify(false, cx);
            return;
        }
        match action {
            "pause"
                if self.active == Some(id)
                    && matches!(job.state, State::Preparing | State::Running)
                    && job
                        .control
                        .pause_available
                        .load(std::sync::atomic::Ordering::Acquire) =>
            {
                job.control.pause();
                job.state = State::Pausing;
            }
            "resume"
                if self.active == Some(id)
                    && matches!(job.state, State::Paused | State::Pausing) =>
            {
                job.control.resume();
                job.state = State::Running;
                job.rate = RateTracker::default();
            }
            "cancel" => {
                job.control.cancel();
                if job.worker.is_some() {
                    job.state = State::Cancelling;
                } else {
                    job.origin = None;
                    job.state = if job.state == State::Attention {
                        State::Failed
                    } else {
                        State::Cancelled
                    };
                    if self.active == Some(id) {
                        self.active = None;
                    }
                }
            }
            "terminate" => {
                job.control
                    .terminate
                    .store(true, std::sync::atomic::Ordering::Release);
                job.control.cancel();
                job.state = State::Cancelling;
            }
            "replace" | "skip" | "keep" | "confirm" if job.state == State::Attention => {
                if let Some(tx) = &job.resolution {
                    let choice = match action {
                        "replace" => Resolution::Replace,
                        "keep" => Resolution::KeepBoth,
                        "confirm" => Resolution::Confirm,
                        _ => Resolution::Skip,
                    };
                    let _ = tx.send(choice);
                    job.conflict = None;
                    job.recovery_conflict = false;
                    job.changed_targets = false;
                    job.message.clear();
                    job.state = State::Preparing;
                }
            }
            _ => {}
        }
        self.start_next(cx);
        self.notify(false, cx);
    }

    pub(super) fn attention_closed(&mut self) {
        self.dialog = None;
    }
    fn open_attention(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(handle) = self.dialog
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }
        let Some(job) = self.jobs.iter().find(|j| j.id == id) else {
            return;
        };
        let Some(origin) = job.origin.clone() else {
            return;
        };
        let kind = if let Some(conflicts) = &job.conflict {
            super::dialog::ExplorerDialogKind::FileConflict(conflicts.clone())
        } else if job.changed_targets {
            if matches!(job.request, Request::Trash(_)) {
                super::dialog::ExplorerDialogKind::Trash(super::view::PendingTrash {
                    paths: job.sources.clone(),
                })
            } else {
                super::dialog::ExplorerDialogKind::PermanentDelete(
                    super::view::PendingPermanentDelete {
                        paths: job.sources.clone(),
                    },
                )
            }
        } else {
            return;
        };
        self.dialog =
            super::dialog::open_operation_dialog(kind, origin, cx.entity().downgrade(), id, cx)
                .ok();
    }

    fn update_lifecycle(&mut self, cx: &mut Context<Self>) {
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            let unfinished = self.jobs.iter().any(|j| !j.state.terminal());
            if unfinished && self.previous_quit_policy.is_none() {
                self.previous_quit_policy = Some(cx.quit_on_last_window_closed());
                cx.set_quit_on_last_window_closed(false);
            } else if !unfinished && let Some(previous) = self.previous_quit_policy.take() {
                cx.set_quit_on_last_window_closed(previous);
                if previous && cx.windows().is_empty() {
                    cx.quit();
                }
            }
        }
    }
    fn open_window(&mut self, cx: &mut Context<Self>) {
        let existing = self.window;
        let manager = cx.entity();
        // GPUI renders a newly opened window synchronously. Open after releasing
        // this entity borrow, because the window renders the manager's snapshot.
        cx.defer(move |cx| {
            if let Some(handle) = existing
                && handle
                    .update(cx, |_, window, _| window.activate_window())
                    .is_ok()
            {
                return;
            }
            let window_manager = manager.clone();
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(540.0), px(520.0)), cx)),
                window_min_size: Some(size(px(430.0), px(300.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some("File Operations".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            match cx.open_window(options, |window, cx| {
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    focus.focus(window);
                    cx.observe(&window_manager, |_, _, cx| cx.notify()).detach();
                    OperationsWindow {
                        manager: window_manager,
                        focus,
                    }
                })
            }) {
                Ok(handle) => manager.update(cx, |manager, _| manager.window = Some(handle.into())),
                Err(error) => eprintln!("Could not open File Operations: {error}"),
            }
        });
    }
}

fn wait_resolution(
    control: &OperationControl,
    rx: &mpsc::Receiver<Resolution>,
) -> Option<Resolution> {
    loop {
        if control.cancel.load(std::sync::atomic::Ordering::Acquire) {
            return None;
        }
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(choice) => return Some(choice),
            Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn run(
    request: Request,
    identities: Vec<Option<TargetIdentity>>,
    verify: bool,
    control: &Arc<OperationControl>,
    tx: &mpsc::Sender<Event>,
    rx: &mpsc::Receiver<Resolution>,
) -> Result<Outcome, String> {
    if super::operation_control::current_checkpoint() {
        return Ok(Outcome::Cancelled);
    }
    if let Request::Delete(paths) | Request::Trash(paths) = &request {
        if !validate_targets(paths, identities, control, tx, rx)? {
            return Ok(Outcome::Cancelled);
        }
    }
    match request {
        Request::Files {
            sources,
            destination,
            kind,
            paste,
        } => {
            let prepare = || match kind {
                FileOperationKind::Move => {
                    filesystem::prepare_move_paths_to_directory(&sources, &destination)
                }
                FileOperationKind::Link => {
                    filesystem::prepare_create_links_to_directory(&sources, &destination)
                }
                _ if paste => {
                    filesystem::prepare_copy_paths_to_directory_for_paste(&sources, &destination)
                }
                _ => filesystem::prepare_copy_paths_to_directory_with_copy_names(
                    &sources,
                    &destination,
                ),
            };
            execute(prepare, verify, control, tx, rx)
        }
        Request::Drop {
            dragged,
            destination,
            directory,
            modifiers,
        } => execute(
            || {
                super::drag_drop::prepare_internal_file_drop(
                    &dragged,
                    &destination,
                    &directory,
                    modifiers,
                )
                .unwrap_or_else(|| Err("This drop target is no longer valid.".into()))
            },
            verify,
            control,
            tx,
            rx,
        ),
        Request::Compress(sources) => execute(
            || filesystem::prepare_compress_paths(&sources),
            verify,
            control,
            tx,
            rx,
        ),
        Request::Extract {
            sources,
            destination,
        } => execute(
            || filesystem::prepare_extract_archives_to_directory(&sources, &destination),
            verify,
            control,
            tx,
            rx,
        ),
        Request::Portable {
            sources,
            destination,
            moving,
        } => {
            let mut destinations = Vec::new();
            let mut moved = Vec::new();
            let mut failures = Vec::new();
            for (index, source) in sources.iter().enumerate() {
                if super::operation_control::current_checkpoint() {
                    break;
                }
                super::operation_control::report_items(index, sources.len(), &display_path(source));
                match super::portable_devices::transfer_item(source, &destination, moving) {
                    Ok(path) => {
                        destinations.push(path);
                        if moving {
                            moved.push(source.clone());
                        }
                    }
                    Err(error) => {
                        if !control.cancel.load(std::sync::atomic::Ordering::Acquire) {
                            failures.push(error);
                        }
                        break;
                    }
                }
            }
            Ok(Outcome::Portable {
                destinations,
                moved,
                failures,
            })
        }
        Request::Trash(paths) => Ok(Outcome::Trash(super::file_commands::run_trash_operation(
            paths,
        )?)),
        Request::Delete(paths) => {
            let mut completed = Vec::new();
            let mut failures = Vec::new();
            let accepted: Vec<_> = paths
                .iter()
                .map(|path| TargetIdentity::read(path))
                .collect();
            for (index, path) in paths.iter().enumerate() {
                if super::operation_control::current_checkpoint() {
                    break;
                }
                super::operation_control::report_items(index, paths.len(), &display_path(path));
                if accepted[index].is_none() || TargetIdentity::read(path) != accepted[index] {
                    failures.push(format!("{} changed before deletion.", display_path(path)));
                    break;
                }
                if let Err(error) = filesystem::remove_paths_permanently(std::slice::from_ref(path))
                {
                    failures.push(error);
                    break;
                }
                completed.push(path.clone());
            }
            Ok(Outcome::Deleted {
                paths: completed,
                failures,
            })
        }
        Request::Recover(request) => {
            let choice = if trash::has_conflicts(&request)? {
                let _ = tx.send(Event::RecoveryConflict);
                match wait_resolution(control, rx) {
                    Some(Resolution::Replace) => RecoveryChoice::Replace,
                    Some(Resolution::KeepBoth) => RecoveryChoice::KeepBoth,
                    Some(_) => RecoveryChoice::Skip,
                    None => return Ok(Outcome::Cancelled),
                }
            } else {
                RecoveryChoice::Skip
            };
            let report = |done, total, name: &str| {
                super::operation_control::report_items(done, total, name);
            };
            Ok(Outcome::Bin(trash::recover(
                request,
                choice,
                &control.cancel,
                report,
            )))
        }
        Request::Purge(ids) => {
            let report = move |done, total, name: &str| {
                super::operation_control::report_items(done, total, name);
            };
            Ok(Outcome::Bin(trash::purge(
                ids,
                control.cancel.clone(),
                report,
            )))
        }
        Request::DiscardCopy(targets) => {
            for (source, destination) in targets {
                if super::operation_control::current_checkpoint() {
                    return Ok(Outcome::Cancelled);
                }
                super::resumable_copy::cleanup_resumable_copy_progress(&source, &destination);
                if super::resumable_copy::has_saved_progress(&destination) {
                    return Err(format!(
                        "Could not discard saved progress for {}.",
                        display_path(&destination)
                    ));
                }
            }
            Ok(Outcome::Cleaned)
        }
        Request::Server(_) | Request::ServerCleanup(_) => unreachable!(),
    }
}

fn validate_targets(
    paths: &[PathBuf],
    mut identities: Vec<Option<TargetIdentity>>,
    control: &OperationControl,
    tx: &mpsc::Sender<Event>,
    rx: &mpsc::Receiver<Resolution>,
) -> Result<bool, String> {
    loop {
        let current: Vec<_> = paths
            .iter()
            .map(|path| TargetIdentity::read(path))
            .collect();
        if let Some(index) = current.iter().position(Option::is_none) {
            return Err(format!("Could not find {}.", display_path(&paths[index])));
        }
        if current == identities {
            return Ok(true);
        }
        let _ = tx.send(Event::ChangedTargets);
        if !matches!(wait_resolution(control, rx), Some(Resolution::Confirm)) {
            return Ok(false);
        }
        identities = current;
    }
}

fn execute(
    prepare: impl Fn() -> Result<PreparedFileOperation, String>,
    verify: bool,
    control: &Arc<OperationControl>,
    tx: &mpsc::Sender<Event>,
    rx: &mpsc::Receiver<Resolution>,
) -> Result<Outcome, String> {
    let (mut job, choice) = match prepare()? {
        PreparedFileOperation::Ready(job) => (job, ConflictChoice::Replace),
        PreparedFileOperation::Conflicts(conflicts) => {
            let diagnostics = conflicts.archive_diagnostics();
            if let Some(diagnostics) = &diagnostics {
                diagnostics.mark_conflict_wait_started();
            }
            let _ = tx.send(Event::Conflict(conflicts.clone()));
            let choice = wait_resolution(control, rx);
            if let Some(diagnostics) = diagnostics {
                diagnostics.mark_conflict_wait_finished();
                diagnostics.finish(if choice.is_some() {
                    "reprepared"
                } else {
                    "cancelled"
                });
            }
            let Some(choice) = choice else {
                return Ok(Outcome::Cancelled);
            };
            let fresh = match prepare()? {
                PreparedFileOperation::Ready(job) => job,
                PreparedFileOperation::Conflicts(conflicts) => conflicts.into_job(),
            };
            (
                fresh,
                if matches!(choice, Resolution::Replace) {
                    ConflictChoice::Replace
                } else {
                    ConflictChoice::Skip
                },
            )
        }
    };
    job.set_copy_verify(verify);
    if let Some(diagnostics) = job.archive_diagnostics() {
        diagnostics.mark_progress_dialog_visible();
    }
    let retained_targets = filesystem::resumable_copy_cleanup_targets(&job, choice);
    if super::operation_control::current_checkpoint() {
        return Ok(Outcome::Cancelled);
    }
    let result = filesystem::execute_file_operation_with_progress(
        job,
        choice,
        control.cancel.clone(),
        control.terminate.clone(),
        |progress| {
            let _ = tx.send(Event::Progress(progress));
        },
    );
    let _ = tx.send(Event::RetainedTargets(
        retained_targets
            .into_iter()
            .filter(|(_, destination)| super::resumable_copy::has_saved_progress(destination))
            .collect(),
    ));
    match result {
        Ok(summary) => {
            if let Some(diagnostics) = &summary.archive_diagnostics {
                diagnostics.finish("ok");
            }
            Ok(Outcome::Files(summary))
        }
        Err(FileOperationError::Cancelled) => Ok(Outcome::Cancelled),
        Err(FileOperationError::Failed(error)) => Err(error),
    }
}

#[derive(Default)]
struct RateTracker {
    key: Option<(filesystem::FileOperationPhase, u64)>,
    samples: VecDeque<(Instant, u64)>,
    speed: Option<f64>,
    remaining: Option<Duration>,
}
impl RateTracker {
    fn record(&mut self, progress: Option<&FileOperationProgress>, state: State, now: Instant) {
        let Some(p) = progress.filter(|p| {
            state == State::Running
                && matches!(
                    p.phase,
                    filesystem::FileOperationPhase::Copying
                        | filesystem::FileOperationPhase::Moving
                        | filesystem::FileOperationPhase::Verifying
                        | filesystem::FileOperationPhase::Compressing
                        | filesystem::FileOperationPhase::Extracting
                )
        }) else {
            *self = Self::default();
            return;
        };
        let key = (p.phase, p.work_total_bytes);
        if self.key != Some(key)
            || self
                .samples
                .back()
                .is_some_and(|(_, bytes)| p.work_completed_bytes < *bytes)
        {
            *self = Self::default();
            self.key = Some(key);
        }
        self.samples.push_back((now, p.work_completed_bytes));
        while self.samples.len() > 1
            && now.duration_since(self.samples[0].0) > Duration::from_secs(5)
        {
            self.samples.pop_front();
        }
        let (start, bytes) = self.samples[0];
        let seconds = now.duration_since(start).as_secs_f64();
        self.speed =
            (seconds >= 0.5).then(|| p.work_completed_bytes.saturating_sub(bytes) as f64 / seconds);
        self.remaining = self
            .speed
            .filter(|s| *s > 0.0 && p.work_total_bytes > p.work_completed_bytes)
            .and_then(|speed| {
                Duration::try_from_secs_f64(
                    p.work_total_bytes.saturating_sub(p.work_completed_bytes) as f64 / speed,
                )
                .ok()
            });
    }
}

struct OperationsWindow {
    manager: Entity<Operations>,
    focus: FocusHandle,
}
impl Focusable for OperationsWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for OperationsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let manager = self.manager.read(cx);
        let mut body = div()
            .id("file-operations-list")
            .flex()
            .flex_col()
            .gap(px(12.0))
            .overflow_y_scroll()
            .flex_1()
            .min_h(px(0.0));
        let order = manager
            .jobs
            .iter()
            .filter(|j| !j.state.terminal())
            .chain(manager.jobs.iter().filter(|j| j.state.terminal()));
        for job in order {
            let mut card = div()
                .id(SharedString::from(format!("operation-{}", job.id)))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .p(px(12.0))
                .flex_shrink_0()
                .min_w(px(0.0))
                .w_full()
                .border_1()
                .border_color(rgb(0xd0d0d0))
                .bg(rgb(0xffffff));
            card = card.child(div().text_size(px(16.0)).child(format!(
                "{} — {}",
                job.request.title(),
                job.state.label()
            )));
            let parents: std::collections::BTreeSet<_> = job
                .sources
                .iter()
                .filter_map(|path| path.parent())
                .collect();
            let source = if parents.len() == 1 {
                display_path(parents.iter().next().unwrap())
            } else {
                format!("{} locations", parents.len())
            };
            card = card.child(div().truncate().child(format!("From: {source}")));
            if let Some(destination) = &job.destination {
                card = card.child(
                    div()
                        .truncate()
                        .child(format!("To: {}", display_path(destination))),
                );
            }
            if manager.active == Some(job.id) {
                if let Some(progress) = &job.progress {
                    card = card.child(format!("{:?}", progress.phase));
                }
                card = card.child(div().truncate().child(job.current.clone()));
                let percent = job
                    .progress
                    .as_ref()
                    .and_then(FileOperationProgress::percent)
                    .or_else(|| {
                        (job.progress.is_none()
                            && job.total > 0
                            && !matches!(job.state, State::Preparing))
                        .then(|| (job.done as f32 / job.total as f32).clamp(0.0, 1.0))
                    });
                if let Some(percent) = percent {
                    card = card.child(format!("{:.0}%", percent * 100.0)).child(
                        div().h(px(16.0)).w_full().bg(rgb(0xeeeeee)).child(
                            div()
                                .h_full()
                                .w(gpui::relative(percent))
                                .bg(rgb(super::constants::EXPLORER_COPY_GREEN)),
                        ),
                    );
                } else if matches!(job.state, State::Preparing | State::Running) {
                    card = card.child(crate::loaders::linear_indeterminate(
                        "operation-active-progress",
                        crate::loaders::LinearProgressStyle::explorer_copy_green(),
                    ));
                }
                let status = if matches!(job.request, Request::Server(_)) {
                    if let Request::Server(id) = job.request {
                        job.server_snapshot
                            .as_ref()
                            .filter(|s| s.id == id)
                            .map(|s| {
                                format!(
                                    "{} of {} · {} · {}",
                                    format_size(Some(s.bytes)),
                                    format_size(Some(s.total)),
                                    job.rate.speed.map(format_transfer_rate).unwrap_or_default(),
                                    if job.state == State::Running {
                                        job.rate
                                            .remaining
                                            .map(format_transfer_remaining)
                                            .unwrap_or_else(|| "Calculating…".into())
                                    } else {
                                        job.state.label().into()
                                    }
                                )
                            })
                            .unwrap_or_default()
                    } else {
                        String::new()
                    }
                } else {
                    let eta = if job.state == State::Running {
                        job.rate
                            .remaining
                            .map(format_transfer_remaining)
                            .unwrap_or_else(|| "Calculating…".into())
                    } else {
                        job.state.label().into()
                    };
                    format!(
                        "{} of {} items · {} · {eta}",
                        job.done,
                        job.total,
                        job.rate.speed.map(format_transfer_rate).unwrap_or_default()
                    )
                };
                card = card.child(status);
            }
            if !job.message.is_empty() {
                card = card.child(div().text_color(rgb(0x595959)).child(job.message.clone()));
            }
            if job.details {
                for (index, source) in job.sources.iter().enumerate() {
                    card = card.child(
                        div()
                            .id(SharedString::from(format!(
                                "operation-{}-source-{index}",
                                job.id
                            )))
                            .debug_selector({
                                let selector = format!("operation-{}-source-{index}", job.id);
                                move || selector.clone()
                            })
                            .w_full()
                            .min_w(px(0.0))
                            .overflow_x_scroll()
                            .child(div().whitespace_nowrap().child(display_path(source))),
                    );
                }
                if let Some(destination) = &job.destination {
                    card = card.child(
                        div()
                            .id(SharedString::from(format!(
                                "operation-{}-destination",
                                job.id
                            )))
                            .w_full()
                            .min_w(px(0.0))
                            .overflow_x_scroll()
                            .child(
                                div()
                                    .whitespace_nowrap()
                                    .child(format!("Destination: {}", display_path(destination))),
                            ),
                    );
                }
                if let Some(progress) = &job.progress {
                    card = card.child(format!(
                        "Remaining: {} items, {}",
                        progress
                            .total_files
                            .saturating_sub(progress.completed_files),
                        format_size(Some(
                            progress
                                .work_total_bytes
                                .saturating_sub(progress.work_completed_bytes)
                        ))
                    ));
                }
            }
            if job.details && job.progress.is_none() {
                card = card.child(format!(
                    "Remaining: {} items",
                    job.total.saturating_sub(job.done)
                ));
            }
            let mut buttons = div().flex().flex_wrap().gap(px(8.0));
            buttons = buttons.child(self.button(
                job.id,
                "details",
                if job.details {
                    "Fewer details"
                } else {
                    "More details"
                },
                cx,
            ));
            if job.state.terminal() {
                let retained = !job.retained_targets.is_empty();
                let server_retained = job
                    .server_snapshot
                    .as_ref()
                    .is_some_and(|s| s.retained_partials);
                if retained || server_retained {
                    buttons =
                        buttons.child(self.button(job.id, "discard", "Discard saved progress", cx));
                }
                if matches!(job.request, Request::Server(_))
                    && job.state == State::Cancelled
                    && job.server_snapshot.is_some()
                {
                    buttons = buttons.child(self.button(
                        job.id,
                        "resume_saved",
                        "Resume saved transfer",
                        cx,
                    ));
                }
                buttons = buttons.child(self.button(job.id, "dismiss", "Dismiss", cx));
            } else {
                if matches!(job.state, State::Running | State::Preparing) {
                    if job
                        .control
                        .pause_available
                        .load(std::sync::atomic::Ordering::Acquire)
                    {
                        buttons = buttons.child(self.button(job.id, "pause", "Pause", cx));
                    } else {
                        buttons = buttons.child(
                            div()
                                .id("operation-pause-unavailable")
                                .px(px(10.0))
                                .py(px(5.0))
                                .text_color(rgb(0x888888))
                                .child("Pause")
                                .tooltip(super::tooltip::explorer_tooltip(
                                    "The current item must finish; no pause boundary remains.",
                                )),
                        );
                    }
                }
                if matches!(job.request, Request::Server(_))
                    && matches!(job.state, State::Paused | State::Attention)
                {
                    buttons =
                        buttons.child(self.button(job.id, "discard", "Discard saved progress", cx));
                }
                if manager.active == Some(job.id)
                    && matches!(job.state, State::Paused | State::Pausing)
                {
                    buttons = buttons.child(self.button(job.id, "resume", "Resume", cx));
                }
                if job.state == State::Attention {
                    if job.conflict.is_some() || job.changed_targets {
                        buttons = buttons.child(self.button(job.id, "resolve", "Resolve…", cx));
                    }
                    if job.conflict.is_some()
                        || job.recovery_conflict
                        || matches!(job.request, Request::Server(_))
                    {
                        buttons = buttons
                            .child(self.button(job.id, "replace", "Replace", cx))
                            .child(self.button(job.id, "skip", "Skip", cx));
                        if job.recovery_conflict || matches!(job.request, Request::Server(_)) {
                            buttons = buttons.child(self.button(job.id, "keep", "Keep both", cx));
                        }
                        if matches!(job.request, Request::Server(_)) {
                            buttons = buttons
                                .child(self.button(job.id, "resume", "Retry", cx))
                                .child(self.button(job.id, "skip_item", "Skip item", cx));
                        }
                    }
                }
                if job.state == State::Attention && matches!(job.request, Request::ServerCleanup(_))
                {
                    buttons = buttons.child(self.button(job.id, "resume", "Retry cleanup", cx));
                }
                buttons = buttons.child(self.button(
                    job.id,
                    "cancel",
                    if job.state == State::Attention && job.worker.is_none() {
                        "Skip operation"
                    } else {
                        "Cancel"
                    },
                    cx,
                ));
                if matches!(
                    job.request,
                    Request::Files {
                        kind: FileOperationKind::Copy,
                        ..
                    }
                ) && job.worker.is_some()
                {
                    buttons = buttons.child(self.button(job.id, "terminate", "Terminate", cx));
                }
            }
            card = card.child(buttons);
            body = body.child(card);
        }
        if manager.jobs.is_empty() {
            body = body.child("No file operations.");
        }
        let clear = self.manager.clone();
        let keyboard_clear = clear.clone();
        div()
            .track_focus(&self.focus)
            .tab_group()
            .tab_stop(false)
            .on_key_down(|event, window, cx| {
                if event.keystroke.key == "tab" {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev();
                    } else {
                        window.focus_next();
                    }
                    cx.stop_propagation();
                }
            })
            .size_full()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(20.0))
            .bg(rgb(0xf8f8f8))
            .text_size(px(12.0))
            .font(crate::settings::current_app_font(cx))
            .child(body)
            .child(
                div()
                    .id("clear-finished-operations")
                    .debug_selector(|| "clear-finished-operations".to_owned())
                    .tab_index(0)
                    .cursor_pointer()
                    .child("Clear finished")
                    .on_key_down(move |event, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            keyboard_clear.update(cx, |manager, cx| {
                                manager.jobs.retain(|j| !j.state.terminal());
                                manager.notify(false, cx);
                            });
                            cx.stop_propagation();
                        }
                    })
                    .on_click(move |_, _, cx| {
                        clear.update(cx, |manager, cx| {
                            manager.jobs.retain(|j| !j.state.terminal());
                            manager.notify(false, cx);
                        })
                    }),
            )
    }
}
impl OperationsWindow {
    fn button(
        &self,
        id: u64,
        action: &'static str,
        label: &'static str,
        _: &Context<Self>,
    ) -> AnyElement {
        let manager = self.manager.clone();
        let keyboard_manager = manager.clone();
        let selector = format!("operation-{id}-{action}");
        div()
            .id(SharedString::from(format!("operation-{id}-{action}")))
            .debug_selector(move || selector.clone())
            .tab_index(0)
            .cursor_pointer()
            .px(px(10.0))
            .py(px(5.0))
            .border_1()
            .border_color(rgb(0xd0d0d0))
            .bg(rgb(0xfdfdfd))
            .hover(|style| style.bg(rgb(0xe0eef9)))
            .focus(|style| style.border_color(rgb(0x0078d4)))
            .on_key_down(move |event, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    keyboard_manager.update(cx, |manager, cx| manager.control(id, action, cx));
                    cx.stop_propagation();
                }
            })
            .child(label)
            .on_click(move |_: &ClickEvent, _, cx| {
                manager.update(cx, |manager, cx| manager.control(id, action, cx))
            })
            .into_any_element()
    }
}
fn display_path(path: &std::path::Path) -> String {
    if trash::is_root(path) {
        trash::label().into()
    } else if let Some(location) = super::remote_fs::RemoteLocation::from_provider(path) {
        location.address()
    } else {
        path.display().to_string()
    }
}

impl Drop for Operations {
    fn drop(&mut self) {
        for job in &self.jobs {
            job.control.cancel();
        }
    }
}

#[cfg(test)]
pub(super) fn states_for_test(cx: &mut App) -> Vec<State> {
    manager(cx)
        .read(cx)
        .jobs
        .iter()
        .map(|job| job.state)
        .collect()
}
#[cfg(test)]
pub(super) fn control_for_test(id: u64, action: &str, cx: &mut App) {
    manager(cx).update(cx, |manager, cx| manager.control(id, action, cx));
}
#[cfg(test)]
pub(super) fn settle_for_test(cx: &mut gpui::VisualTestContext) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        cx.run_until_parked();
        let busy = cx.cx.update(|app| {
            let Some(global) = app.try_global::<OperationsGlobal>() else {
                return false;
            };
            let manager = global.0.clone();
            manager.update(app, |manager, cx| manager.tick(cx));
            manager.read(app).jobs.iter().any(|job| {
                matches!(
                    job.state,
                    State::Preparing | State::Running | State::Pausing | State::Cancelling
                )
            })
        });
        if !busy {
            cx.run_until_parked();
            return;
        }
        assert!(Instant::now() < deadline, "operation did not settle");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::test_view_entity_at_path;
    use super::*;
    use std::fs;

    fn copy(source: PathBuf, destination: PathBuf) -> Request {
        Request::Files {
            sources: vec![source],
            destination,
            kind: FileOperationKind::Copy,
            paste: true,
        }
    }

    #[gpui::test]
    fn fifo_reprepares_after_prior_job_and_cancelled_waiter_never_runs(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.txt");
        fs::write(&source, b"source").unwrap();
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).unwrap();
        let third = temp.path().join("third");
        fs::create_dir(&third).unwrap();
        let (view, cx) = test_view_entity_at_path(cx, destination.clone());
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.enqueue_operation(copy(source.clone(), destination.clone()), cx);
                view.enqueue_operation(copy(source.clone(), destination.clone()), cx);
                view.enqueue_operation(copy(source.clone(), third.clone()), cx);
            })
        });
        cx.update(|_, app| {
            manager(app).update(app, |manager, cx| {
                assert_eq!(manager.active, Some(1));
                assert_eq!(manager.jobs[1].state, State::Queued);
                manager.control(3, "cancel", cx);
            })
        });
        settle_for_test(cx);
        cx.update(|_, app| {
            manager(app).update(app, |manager, cx| {
                assert_eq!(manager.jobs[0].state, State::Completed);
                assert_eq!(manager.jobs[1].state, State::Attention);
                assert!(manager.jobs[1].conflict.is_some());
                assert_eq!(manager.jobs[2].state, State::Cancelled);
                manager.control(2, "skip", cx);
            })
        });
        settle_for_test(cx);
        assert_eq!(fs::read(destination.join("source.txt")).unwrap(), b"source");
        assert!(!third.join("source.txt").exists());
    }

    #[gpui::test]
    fn failure_blocks_later_jobs_until_acknowledged(cx: &mut gpui::TestAppContext) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.txt");
        fs::write(&source, b"source").unwrap();
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_owned());
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.enqueue_operation(
                    copy(temp.path().join("missing"), temp.path().to_owned()),
                    cx,
                );
                view.enqueue_operation(copy(source.clone(), temp.path().to_owned()), cx);
            })
        });
        settle_for_test(cx);
        cx.update(|_, app| {
            manager(app).update(app, |manager, cx| {
                assert_eq!(manager.active, Some(1));
                assert_eq!(manager.jobs[0].state, State::Attention);
                assert_eq!(manager.jobs[1].state, State::Queued);
                manager.control(1, "cancel", cx);
            })
        });
        settle_for_test(cx);
        assert!(temp.path().join("source - Copy.txt").exists());
    }

    #[gpui::test]
    fn changed_delete_target_requires_confirmation_and_cancel_preserves_it(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file.txt");
        fs::write(&path, b"before").unwrap();
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_owned());
        // Hold a synthetic recovered job so deletion cannot start yet.
        cx.update(|_, app| {
            manager(app).update(app, |manager, cx| {
                let id = manager.insert(Request::Server(0), None, PathBuf::new(), true, cx);
                manager.jobs[0].state = State::Paused;
                manager.active = Some(id);
            })
        });
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.enqueue_operation(Request::Delete(vec![path.clone()]), cx)
            })
        });
        fs::write(&path, b"a different file").unwrap();
        cx.update(|_, app| {
            manager(app).update(app, |manager, cx| {
                manager.jobs[0].state = State::Cancelled;
                manager.active = None;
                manager.start_next(cx);
            })
        });
        settle_for_test(cx);
        cx.update(|_, app| {
            manager(app).update(app, |manager, cx| {
                assert!(manager.jobs[1].changed_targets);
                assert_eq!(manager.jobs[1].state, State::Attention);
                manager.control(2, "cancel", cx);
            })
        });
        settle_for_test(cx);
        assert_eq!(fs::read(&path).unwrap(), b"a different file");
    }

    #[gpui::test]
    fn hidden_window_and_closed_origin_do_not_abandon_conflicts(cx: &mut gpui::TestAppContext) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("file.txt");
        fs::write(&source, b"new").unwrap();
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("file.txt"), b"old").unwrap();
        let (view, cx) = test_view_entity_at_path(cx, destination.clone());
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.enqueue_operation(copy(source.clone(), destination.clone()), cx)
            })
        });
        settle_for_test(cx);
        let weak = view.downgrade();
        cx.update(|_, app| {
            view.update(app, |view, cx| view.prepare_for_tab_close(cx));
            let entity = manager(app);
            let handle = entity.read(app).window.unwrap();
            handle
                .update(app, |_, window, _| window.remove_window())
                .unwrap();
        });
        drop(view);
        cx.update(|_, app| {
            manager(app).update(app, |manager, cx| {
                assert!(weak.upgrade().is_some());
                assert_eq!(manager.jobs[0].state, State::Attention);
                manager.open_attention(1, cx);
                assert!(manager.dialog.is_some());
            })
        });
        cx.update(|_, app| show(app));
        cx.run_until_parked();
        cx.update(|_, app| {
            manager(app).update(app, |manager, cx| manager.control(1, "replace", cx))
        });
        settle_for_test(cx);
        assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"new");
        cx.update(|_, app| assert_eq!(manager(app).read(app).jobs[0].state, State::Completed));
    }

    #[gpui::test]
    fn fifo_is_shared_across_views_and_undo_waits_for_drain(cx: &mut gpui::TestAppContext) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("file.txt");
        fs::write(&source, b"source").unwrap();
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).unwrap();
        let (first, cx) = test_view_entity_at_path(cx, temp.path().to_owned());
        let second = cx.update(|_, app| {
            app.new(|_| {
                ExplorerView::new_unloaded_with_settings_for_test(
                    destination.clone(),
                    None,
                    &crate::settings::ExplorerSettings::default(),
                )
            })
        });
        cx.update(|_, app| {
            first.update(app, |view, cx| {
                view.enqueue_operation(copy(source.clone(), destination.clone()), cx)
            });
            second.update(app, |view, cx| {
                view.enqueue_operation(
                    copy(destination.join("file.txt"), temp.path().to_owned()),
                    cx,
                )
            });
        });
        settle_for_test(cx);
        cx.update(|_, app| {
            assert_eq!(
                states_for_test(app),
                vec![State::Completed, State::Attention]
            );
            first.update(app, |view, cx| {
                assert_eq!(view.file_operation_undo_stack.len(), 1);
                view.undo_file_operation(cx);
                assert_eq!(view.file_operation_undo_stack.len(), 1);
            });
            control_for_test(2, "cancel", app);
        });
        settle_for_test(cx);
        cx.update(|_, app| first.update(app, |view, cx| view.undo_file_operation(cx)));
        assert!(!destination.join("file.txt").exists());
    }

    #[gpui::test]
    fn copying_to_another_location_preserves_the_origin_selection(cx: &mut gpui::TestAppContext) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.txt");
        fs::write(&source, b"source").unwrap();
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).unwrap();
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_owned());
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.restore_selection_from_paths(std::slice::from_ref(&source));
                view.enqueue_operation(copy(source.clone(), destination.clone()), cx);
            })
        });
        settle_for_test(cx);
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.selected_paths(), [source.clone()])
        });
        assert_eq!(fs::read(destination.join("source.txt")).unwrap(), b"source");
    }

    #[gpui::test]
    fn partial_delete_keeps_failed_sources_selected(cx: &mut gpui::TestAppContext) {
        let temp = tempfile::tempdir().unwrap();
        let removed = temp.path().join("a.txt");
        let failed = temp.path().join("b.txt");
        let next = temp.path().join("c.txt");
        for path in [&removed, &failed, &next] {
            fs::write(path, b"file").unwrap();
        }
        let sources = vec![removed.clone(), failed.clone()];
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_owned());
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.restore_selection_from_paths(&sources);
                fs::remove_file(&removed).unwrap();
                view.complete_queued_operation(
                    Outcome::Deleted {
                        paths: vec![removed.clone()],
                        failures: vec!["Access denied".into()],
                    },
                    temp.path(),
                    &sources,
                    &sources,
                    Some(next),
                    cx,
                );
            })
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert_eq!(view.selected_paths(), [failed.clone()])
        });
    }

    #[gpui::test]
    fn operation_window_keyboard_controls_and_long_source_details(cx: &mut gpui::TestAppContext) {
        let temp = tempfile::tempdir().unwrap();
        let (_view, cx) = test_view_entity_at_path(cx, temp.path().to_owned());
        cx.update(|_, app| {
            manager(app).update(app, |manager, cx| {
                let id = manager.insert(
                    Request::Files {
                        sources: vec![
                            PathBuf::from(format!(
                                "/first/{}/source.txt",
                                "long-folder/".repeat(35)
                            )),
                            PathBuf::from("/second/source.txt"),
                        ],
                        destination: PathBuf::from("/destination"),
                        kind: FileOperationKind::Copy,
                        paste: true,
                    },
                    None,
                    PathBuf::new(),
                    true,
                    cx,
                );
                manager.jobs[0].state = State::Paused;
                manager.active = Some(id);
                manager.open_window(cx);
                manager.notify(false, cx);
            });
        });
        cx.run_until_parked();
        let window = cx.update(|_, app| manager(app).read(app).window.unwrap());
        let mut operations_cx = gpui::VisualTestContext::from_window(window, &cx.cx);
        operations_cx.simulate_keystrokes("tab enter");
        operations_cx.update(|_, app| assert!(manager(app).read(app).jobs[0].details));
        assert!(operations_cx.debug_bounds("operation-1-source-0").is_some());
        assert!(operations_cx.debug_bounds("operation-1-source-1").is_some());
        // More details -> Resume -> Cancel, then activate without a pointer.
        operations_cx.simulate_keystrokes("tab tab enter");
        operations_cx
            .update(|_, app| assert_eq!(manager(app).read(app).jobs[0].state, State::Cancelled));
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[gpui::test]
    fn lifecycle_retains_process_until_queue_drains_and_preserves_tray_policy(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|app| {
            for initial in [true, false] {
                app.set_quit_on_last_window_closed(initial);
                manager(app).update(app, |manager, cx| {
                    let id =
                        manager.insert(Request::Delete(Vec::new()), None, PathBuf::new(), true, cx);
                    manager.jobs.last_mut().unwrap().state = State::Paused;
                    manager.notify(false, cx);
                    assert!(!cx.quit_on_last_window_closed());
                    manager
                        .jobs
                        .iter_mut()
                        .find(|job| job.id == id)
                        .unwrap()
                        .state = State::Cancelled;
                    manager.notify(false, cx);
                    assert_eq!(cx.quit_on_last_window_closed(), initial);
                });
            }
        });
    }

    #[gpui::test]
    fn queued_sftp_controls_cannot_bypass_the_active_job(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            manager(app).update(app, |manager, cx| {
                let active = manager.insert(Request::Server(0), None, PathBuf::new(), true, cx);
                manager.jobs[0].state = State::Paused;
                manager.active = Some(active);
                let waiting = manager.insert(Request::Server(1), None, PathBuf::new(), true, cx);
                manager.notify(false, cx);
                for action in ["resume", "replace", "skip", "keep"] {
                    manager.control(waiting, action, cx);
                }
                assert_eq!(manager.active, Some(active));
                assert_eq!(manager.jobs[1].state, State::Queued);
                manager.control(waiting, "cancel", cx);
                assert_eq!(manager.jobs[1].state, State::Cancelled);
                assert!(manages_server(1, cx));
                manager.control(waiting, "dismiss", cx);
                assert!(manages_server(1, cx));
            })
        });
    }

    #[test]
    fn eta_uses_a_window_and_resets_for_pause_phase_totals_and_resume_prefix() {
        let mut tracker = RateTracker::default();
        let start = Instant::now();
        let mut progress = FileOperationProgress {
            kind: FileOperationKind::Copy,
            phase: filesystem::FileOperationPhase::Copying,
            total_bytes: 1000,
            copied_bytes: 500,
            verified_bytes: 0,
            work_total_bytes: 1000,
            work_completed_bytes: 500,
            total_files: 1,
            completed_files: 0,
            current_item: None,
            cancellable: true,
        };
        tracker.record(Some(&progress), State::Running, start);
        assert!(tracker.speed.is_none());
        progress.work_completed_bytes = 600;
        tracker.record(
            Some(&progress),
            State::Running,
            start + Duration::from_secs(1),
        );
        assert_eq!(tracker.speed, Some(100.0));
        assert_eq!(tracker.remaining, Some(Duration::from_secs(4)));
        tracker.record(
            Some(&progress),
            State::Running,
            start + Duration::from_secs(7),
        );
        assert!(tracker.remaining.is_none());
        tracker.record(
            Some(&progress),
            State::Paused,
            start + Duration::from_secs(8),
        );
        assert!(tracker.speed.is_none());
        tracker.record(
            Some(&progress),
            State::Running,
            start + Duration::from_secs(9),
        );
        assert!(tracker.remaining.is_none());
        progress.work_total_bytes = 2000;
        tracker.record(
            Some(&progress),
            State::Running,
            start + Duration::from_secs(10),
        );
        assert!(tracker.speed.is_none());
        progress.phase = filesystem::FileOperationPhase::Verifying;
        tracker.record(
            Some(&progress),
            State::Running,
            start + Duration::from_secs(11),
        );
        assert!(tracker.remaining.is_none());
        tracker.record(None, State::Running, start + Duration::from_secs(12));
        assert!(tracker.speed.is_none());
    }
}
