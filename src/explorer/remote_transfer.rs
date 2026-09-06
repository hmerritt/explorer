//! Durable native SFTP jobs, independent of views and navigation.
use super::remote_fs::{self, RemoteLocation, Session, sftp_error};
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) enum Location {
    Local(PathBuf),
    Remote(RemoteLocation),
}
impl Location {
    fn from_path(path: &Path) -> io::Result<Self> {
        if remote_fs::is_remote(path) {
            return RemoteLocation::from_provider(path)
                .map(Self::Remote)
                .ok_or_else(|| io::Error::other("Invalid remote location"));
        }
        if super::portable_devices::is_portable_path(path)
            || super::archive_fs::is_archive_path(path)
        {
            return Err(io::Error::other(
                "Copy this item to a local folder before transferring it.",
            ));
        }
        Ok(Self::Local(path.to_owned()))
    }
    fn child(&self, name: &str) -> io::Result<Self> {
        match self {
            Self::Remote(loc) => Ok(Self::Remote(loc.child(name).map_err(io::Error::other)?)),
            Self::Local(path) => {
                validate_local_name(name)?;
                Ok(Self::Local(path.join(name)))
            }
        }
    }
    fn name(&self) -> io::Result<String> {
        match self {
            Self::Local(p) => p
                .file_name()
                .and_then(|s| s.to_str())
                .map(str::to_owned)
                .ok_or_else(|| io::Error::other("Filename is not valid UTF-8")),
            Self::Remote(l) => l
                .path
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| io::Error::other("Cannot transfer a server root")),
        }
    }
    fn parent(&self) -> io::Result<Self> {
        match self {
            Self::Local(p) => p
                .parent()
                .map(|p| Self::Local(p.to_owned()))
                .ok_or_else(|| io::Error::other("Missing parent")),
            Self::Remote(l) => {
                let parent = remote_fs::parent(&l.provider_path())
                    .and_then(|p| RemoteLocation::from_provider(&p))
                    .ok_or_else(|| io::Error::other("Missing remote parent"))?;
                Ok(Self::Remote(parent))
            }
        }
    }
    fn label(&self) -> String {
        match self {
            Self::Local(p) => p.display().to_string(),
            Self::Remote(l) => l.address(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Metadata {
    size: u64,
    modified: Option<u64>,
    #[serde(default)]
    nanos: Option<u32>,
    #[serde(default)]
    owner: Option<(u32, u32)>,
    mode: Option<u32>,
    kind: Kind,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
enum Kind {
    File,
    Directory,
    Link(String),
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(super) enum State {
    Queued,
    Connecting,
    Preparing,
    Transferring,
    Verifying,
    Reconnecting,
    Finalizing,
    Paused,
    Attention,
    Completed,
    Cancelled,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(super) enum Conflict {
    Ask,
    Replace,
    Skip,
    KeepBoth,
}
#[derive(Clone, Serialize, Deserialize)]
struct Item {
    #[serde(default)]
    target_seen: bool,
    #[serde(default)]
    target_before: Option<Metadata>,
    #[serde(default)]
    source_removed: bool,
    source: Location,
    destination: Location,
    metadata: Metadata,
    completed: bool,
    partial: Option<Location>,
    committing: bool,
    skipped: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Manifest {
    #[serde(default)]
    endpoint: Option<super::remote_download::RemoteEndpointKey>,
    version: u32,
    pub id: u64,
    sources: Vec<Location>,
    destination: Location,
    pub state: State,
    pub message: String,
    pub bytes: u64,
    pub total: u64,

    pub current: usize,
    items: Vec<Item>,
    planned: bool,
    move_sources: bool,
    conflict: Conflict,
    pub warnings: Vec<String>,
}
impl Manifest {
    pub fn title(&self) -> String {
        if let Some(item) = self.items.get(self.current) {
            return item.source.name().unwrap_or_else(|_| "Transfer".into());
        }
        let first = self
            .sources
            .first()
            .and_then(|s| s.name().ok())
            .unwrap_or_else(|| "Transfer".into());
        if self.sources.len() > 1 {
            format!("{first} (+{} items)", self.sources.len() - 1)
        } else {
            first
        }
    }

    pub fn files(&self) -> usize {
        self.items.len()
    }
}
struct Job {
    progress: Mutex<TransferProgress>,
    store: PathBuf,
    data: Mutex<Manifest>,
    control: AtomicU8,
    running: AtomicBool,
}

#[derive(Default)]
struct TransferProgress {
    item: Option<usize>,
    bytes: u64,
    payload: u64,
    samples: std::collections::VecDeque<(std::time::Instant, u64)>,
}
impl TransferProgress {
    fn reset_rate(&mut self, now: std::time::Instant) {
        self.payload = 0;
        self.samples.clear();
        self.samples.push_back((now, 0));
    }
    fn record(&mut self, now: std::time::Instant, item: usize, bytes: u64, payload: u64) {
        self.item = Some(item);
        self.bytes = bytes;
        self.payload = self.payload.saturating_add(payload);
        if self.samples.len() > 1
            && now.duration_since(self.samples.back().unwrap().0) < Duration::from_millis(100)
        {
            *self.samples.back_mut().unwrap() = (now, self.payload);
        } else {
            self.samples.push_back((now, self.payload));
        }
        while self.samples.len() > 1
            && now.duration_since(self.samples[0].0) > Duration::from_secs(5)
        {
            self.samples.pop_front();
        }
    }
    fn speed(&self, now: std::time::Instant) -> Option<f64> {
        let (start, bytes) = self
            .samples
            .iter()
            .find(|(t, _)| now.duration_since(*t) <= Duration::from_secs(5))?;
        let elapsed = now.duration_since(*start).as_secs_f64();
        (elapsed >= 0.5).then(|| (self.payload - bytes) as f64 / elapsed)
    }
}
fn record_progress(job: &Job, bytes: u64, payload: u64) {
    let index = job.data.lock().unwrap().current;
    job.progress
        .lock()
        .unwrap()
        .record(std::time::Instant::now(), index, bytes, payload);
}
fn complete_job(job: &Job) {
    set_state(job, State::Completed, "Transfer completed");
    COMPLETION_REVISION.fetch_add(1, Ordering::Release);
}
fn clean_completion(m: &Manifest) -> bool {
    m.state == State::Completed
        && m.warnings.is_empty()
        && m.items.iter().all(|i| i.completed && !i.skipped)
}
fn transfer_percentage(m: &Manifest, bytes: u64, total: u64) -> Option<u8> {
    if !m.planned {
        return None;
    }
    if m.state == State::Completed {
        return Some(100);
    }
    let ratio = if total > 0 {
        bytes as f64 / total as f64
    } else {
        let items = m.items.iter().filter(|i| !i.skipped).count();
        let completed = m.items.iter().filter(|i| !i.skipped && i.completed).count();
        if items == 0 {
            0.0
        } else {
            completed as f64 / items as f64
        }
    };
    Some((ratio * 100.0).clamp(0.0, 99.0) as u8)
}
#[derive(Default)]
struct Manager {
    jobs: Mutex<BTreeMap<u64, Arc<Job>>>,
}
static COMPLETION_REVISION: AtomicU64 = AtomicU64::new(0);
pub(super) fn completion_revision() -> u64 {
    COMPLETION_REVISION.load(Ordering::Acquire)
}
fn state_dir() -> io::Result<PathBuf> {
    crate::settings::config_dir()
        .map(|p| p.join("sftp-transfers"))
        .ok_or_else(|| io::Error::other("Configuration directory unavailable"))
}
fn manager() -> &'static Manager {
    static MANAGER: OnceLock<Manager> = OnceLock::new();
    MANAGER.get_or_init(|| {
        let manager = Manager::default();
        // UI unit tests must not load the user's persisted transfers. Recovery
        // tests deserialize their own manifests in isolated temporary folders.
        #[cfg(not(test))]
        if let Ok(dir) = state_dir().and_then(fs::read_dir) {
            for entry in dir.flatten().filter(|e| e.path().extension().is_some_and(|e| e == "json")) {
                if let Ok(bytes) = fs::read(entry.path()) {
                    if let Ok(mut data) = serde_json::from_slice::<Manifest>(&bytes) {
                        if !valid_manifest(&data) { continue; }
                        if clean_completion(&data) && fs::remove_file(entry.path()).is_ok() { continue; }
                        if !matches!(data.state, State::Completed | State::Cancelled) { data.state = State::Paused; data.message = "Interrupted transfer recovered. Resume to revalidate and continue.".into(); }
                        manager.jobs.lock().unwrap().insert(data.id, Arc::new(Job { progress: Mutex::default(), store: entry.path(), data: Mutex::new(data), control: AtomicU8::new(1), running: AtomicBool::new(false) }));
                    }
                }
            }
        }
        manager
    })
}
#[derive(Clone)]
pub(super) struct JobSnapshot {
    pub id: u64,
    pub state: State,
    pub message: String,
    pub current: usize,
    pub bytes: u64,
    pub total: u64,
    pub current_file_bytes: u64,
    pub current_file_total: u64,
    pub warnings: Vec<String>,
    title: String,
    files: usize,
    pub retained_partials: bool,
    pub percentage: Option<u8>,
    pub speed: Option<f64>,
    pub remaining: Option<Duration>,
}
impl JobSnapshot {
    #[cfg(test)]
    pub(super) fn for_test(state: State) -> Self {
        Self {
            id: 123,
            state,
            message: "Transferring".into(),
            current: 0,
            bytes: 512,
            total: 1024,
            current_file_bytes: 512,
            current_file_total: 1024,
            warnings: vec![],
            title: "file.txt".into(),
            files: 1,
            retained_partials: false,
            percentage: Some(50),
            speed: Some(512.0),
            remaining: Some(Duration::from_secs(1)),
        }
    }
    pub fn title(&self) -> String {
        self.title.clone()
    }
    pub fn files(&self) -> usize {
        self.files
    }
}
pub(super) fn snapshots() -> Vec<JobSnapshot> {
    manager()
        .jobs
        .lock()
        .unwrap()
        .values()
        .filter_map(|job| job_snapshot(job, std::time::Instant::now()))
        .collect()
}
fn job_snapshot(job: &Job, now: std::time::Instant) -> Option<JobSnapshot> {
    if clean_completion(&job.data.lock().unwrap()) {
        return None;
    }
    let m = job.data.lock().unwrap();
    let progress = job.progress.lock().unwrap();
    let current_bytes = if progress.item == Some(m.current) {
        progress
            .bytes
            .min(m.items.get(m.current).map_or(0, |i| i.metadata.size))
    } else {
        0
    };
    let total = m
        .items
        .iter()
        .filter(|i| !i.skipped && i.metadata.kind == Kind::File)
        .map(|i| i.metadata.size)
        .sum::<u64>();
    let bytes = m.bytes.saturating_add(current_bytes).min(total);
    let speed = (m.state == State::Transferring)
        .then(|| progress.speed(now))
        .flatten();
    let remaining = speed
        .filter(|s| *s > 0.0)
        .and_then(|s| Duration::try_from_secs_f64((total.saturating_sub(bytes)) as f64 / s).ok());
    Some(JobSnapshot {
        id: m.id,
        state: m.state,
        message: m.message.clone(),
        current: m.current,
        bytes,
        total,
        current_file_bytes: current_bytes,
        current_file_total: m.items.get(m.current).map_or(0, |i| i.metadata.size),
        percentage: transfer_percentage(&m, bytes, total),
        speed,
        remaining,
        warnings: m.warnings.clone(),
        title: m.title(),
        files: m.files(),
        retained_partials: m
            .items
            .iter()
            .any(|i| i.partial.is_some() && (!i.completed || i.skipped)),
    })
}
fn save(job: &Job) -> io::Result<()> {
    let data = job.data.lock().unwrap().clone();
    remote_fs::atomic_json(&job.store, &data).map_err(io::Error::other)
}
fn set_state(job: &Job, state: State, message: impl Into<String>) {
    let mut m = job.data.lock().unwrap();
    if m.state != state
        && !(matches!(m.state, State::Transferring | State::Finalizing)
            && matches!(state, State::Transferring | State::Finalizing))
    {
        job.progress
            .lock()
            .unwrap()
            .reset_rate(std::time::Instant::now());
    }
    m.state = state;
    m.message = message.into();
}
fn valid_manifest(m: &Manifest) -> bool {
    fn valid(l: &Location) -> bool {
        match l {
            Location::Local(p) => p.is_absolute() && !remote_fs::is_remote(p),
            Location::Remote(l) => {
                RemoteLocation::parse(&l.site).is_ok_and(|base| base.site == l.site)
                    && l.path.starts_with('/')
                    && !l.path.contains('\0')
                    && !l.path.split('/').any(|n| matches!(n, "." | ".."))
            }
        }
    }
    m.version == 1
        && m.current <= m.items.len()
        && !m.sources.is_empty()
        && m.sources.iter().chain([&m.destination]).all(valid)
        && m.items.iter().enumerate().all(|(index, i)| {
            valid(&i.source)
                && valid(&i.destination)
                && i.partial.as_ref().is_none_or(|partial| {
                    i.destination
                        .parent()
                        .and_then(|p| p.child(&format!(".explorer-{}-{index}.filepart", m.id)))
                        .is_ok_and(|expected| expected == *partial)
                })
        })
}
pub(super) fn enqueue(
    sources: Vec<PathBuf>,
    destination: PathBuf,
    move_sources: bool,
) -> Result<u64, String> {
    static IDS: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;
    let id = IDS
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |last| {
            Some(last.max(now) + 1)
        })
        .unwrap()
        .max(now)
        + 1;
    let sources = sources
        .iter()
        .map(|p| Location::from_path(p))
        .collect::<io::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    let destination = Location::from_path(&destination).map_err(|e| e.to_string())?;
    if sources.is_empty() {
        return Err("Select files or folders to transfer".into());
    }
    if !move_sources
        && sources.iter().any(|s| matches!(s, Location::Remote(_)))
        && matches!(destination, Location::Remote(_))
    {
        return Err("For remote-to-remote transfers, copy through a local folder.".into());
    }
    if let Location::Remote(dest) = &destination {
        for source in &sources {
            if let Location::Remote(source) = source {
                if source.site != dest.site {
                    return Err(
                        "Move files within the same saved server, or copy through a local folder."
                            .into(),
                    );
                }
                if source.path == dest.path
                    || dest
                        .path
                        .starts_with(&(source.path.trim_end_matches('/').to_owned() + "/"))
                {
                    return Err("Cannot move a folder into itself.".into());
                }
            }
        }
    }
    let sites: std::collections::HashSet<_> = sources
        .iter()
        .chain([&destination])
        .filter_map(|l| {
            if let Location::Remote(l) = l {
                Some(l.site.clone())
            } else {
                None
            }
        })
        .collect();
    if sites.len() != 1 {
        return Err("Transfer files between one SFTP server and a local folder.".into());
    }
    let endpoint = super::remote_download::endpoint_key(&super::clipboard::ClipboardDownload {
        url: gpui::http_client::Url::parse(sites.iter().next().unwrap())
            .map_err(|e| e.to_string())?,
        file_name: String::new(),
    })
    .ok_or("Could not resolve the SSH site configuration")?;
    let job = Arc::new(Job {
        progress: Mutex::default(),
        store: state_dir()
            .map_err(|e| e.to_string())?
            .join(format!("{id}.json")),
        data: Mutex::new(Manifest {
            endpoint: Some(endpoint),
            version: 1,
            id,
            sources,
            destination,
            state: State::Queued,
            message: "Queued".into(),
            bytes: 0,
            total: 0,
            current: 0,
            items: vec![],
            planned: false,
            move_sources,
            conflict: Conflict::Ask,
            warnings: vec![],
        }),
        control: AtomicU8::new(0),
        running: AtomicBool::new(false),
    });
    save(&job).map_err(|e| e.to_string())?;
    manager().jobs.lock().unwrap().insert(id, job.clone());
    start(job);
    Ok(id)
}
pub(super) fn control(id: u64, action: &str) {
    let Some(job) = manager().jobs.lock().unwrap().get(&id).cloned() else {
        return;
    };
    match action {
        "discard" => {
            if job.running.load(Ordering::Acquire) {
                return;
            }
            job.control.store(0, Ordering::Release);
            start_action(job, true);
        }
        "dismiss" => {
            if job.running.load(Ordering::Acquire) {
                return;
            }
            let m = job.data.lock().unwrap();
            if !matches!(m.state, State::Completed | State::Cancelled)
                || m.items
                    .iter()
                    .any(|i| i.partial.is_some() && (!i.completed || i.skipped))
            {
                return;
            }
            drop(m);
            if let Err(e) = fs::remove_file(&job.store) {
                set_state(
                    &job,
                    State::Attention,
                    format!("Could not remove saved transfer: {e}"),
                );
                return;
            }
            manager().jobs.lock().unwrap().remove(&id);
        }
        "skip_item" => {
            if job.running.load(Ordering::Acquire) {
                return;
            }
            let mut m = job.data.lock().unwrap();
            let index = m.current;
            if index >= m.items.len() {
                return;
            }
            let source = m.items[index].source.clone();
            while m.current < m.items.len()
                && locations_overlap(&source, &m.items[m.current].source)
            {
                let index = m.current;
                m.items[index].skipped = true;
                m.items[index].completed = true;
                let label = m.items[index].source.name().unwrap_or_default();
                m.warnings.push(format!("Skipped {label}"));
                m.current += 1;
            }
            drop(m);
            if let Err(e) = save(&job) {
                set_state(&job, State::Attention, e.to_string());
                return;
            }
            job.control.store(0, Ordering::Release);
            start(job);
        }
        "pause" => {
            job.control.store(1, Ordering::Release);
        }
        "cancel" => {
            job.control.store(2, Ordering::Release);
            if !job.running.load(Ordering::Acquire) {
                set_state(
                    &job,
                    State::Cancelled,
                    "Cancelled; partial data retained until discarded",
                );
                let _ = save(&job);
            }
        }
        "replace" | "skip" | "keep" | "resume" => {
            if job.running.load(Ordering::Acquire) {
                return;
            }
            {
                let mut m = job.data.lock().unwrap();
                m.conflict = match action {
                    "replace" => Conflict::Replace,
                    "skip" => Conflict::Skip,
                    "keep" => Conflict::KeepBoth,
                    _ => m.conflict,
                };
            }
            job.control.store(0, Ordering::Release);
            start(job);
        }
        _ => {}
    }
}
async fn interrupted(job: &Job) {
    while job.control.load(Ordering::Acquire) == 0 {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
fn start(job: Arc<Job>) {
    start_action(job, false);
}
fn start_action(job: Arc<Job>, discard: bool) {
    if job.running.swap(true, Ordering::AcqRel) {
        return;
    }
    remote_fs::runtime().spawn(async move {
        let data = job.data.lock().unwrap().clone();
        let site = data.sources.iter().chain([&data.destination]).find_map(|l| if let Location::Remote(l) = l { Some(l.site.clone()) } else { None });
        let Some(site) = site else { set_state(&job, State::Attention, "Missing server"); job.running.store(false, Ordering::Release); return; };
        // Two workers per server. Each owns a reusable session lane.
        let pool = pool(&site);
        let permit = tokio::select! {
            biased;
            _ = interrupted(&job) => None,
            permit = async {
                let reservation = reserve(&job).await;
                pool.semaphore.acquire().await.ok().map(|permit| (reservation, permit))
            } => permit,
        };
        let result = if let Some((_reservation, permit)) = permit {
            let lane = pool.lanes.lock().unwrap().pop().expect("permit owns lane");
            let result = tokio::select! {
                biased;
                _ = interrupted(&job) => Err(io::Error::new(io::ErrorKind::Interrupted, "Transfer interrupted")),
                result = run_with_retries(&job, &site, lane, discard) => result,
            };
            if result.is_err() { remote_fs::disconnect(&site, lane).await; }
            pool.lanes.lock().unwrap().push(lane);
            drop(permit);
            result
        } else { Err(io::Error::new(io::ErrorKind::Interrupted, "Transfer interrupted")) };
        match result {
            Ok(()) if discard => {
                match fs::remove_file(&job.store) {
                    Ok(()) => { manager().jobs.lock().unwrap().remove(&data.id); job.running.store(false, Ordering::Release); return; },
                    Err(e) => set_state(&job, State::Attention, format!("Partials removed, but could not remove saved transfer: {e}")),
                }
            },
            Ok(()) => { complete_job(&job); },
            Err(e) if job.control.load(Ordering::Acquire) != 0 => {
                let state = if job.control.load(Ordering::Acquire) == 2 { State::Cancelled } else { State::Paused };
                set_state(&job, state, "Partial data retained. Resume validates it before continuing.");
                let _ = e;
            },
            Err(e) => set_state(&job, State::Attention, e.to_string()),
        }
        if let Err(e) = save(&job) { set_state(&job, State::Attention, format!("Could not save transfer state: {e}")); }
        if clean_completion(&job.data.lock().unwrap()) {
            match fs::remove_file(&job.store) {
                Ok(()) => { manager().jobs.lock().unwrap().remove(&data.id); },
                Err(error) => set_state(&job, State::Attention, format!("Transfer completed, but saved state could not be removed: {error}")),
            }
        }
        job.running.store(false, Ordering::Release);
    });
}
struct Pool {
    semaphore: tokio::sync::Semaphore,
    lanes: Mutex<Vec<usize>>,
}
fn reservations() -> &'static Mutex<Vec<(u64, Vec<Location>)>> {
    static RESERVATIONS: OnceLock<Mutex<Vec<(u64, Vec<Location>)>>> = OnceLock::new();
    RESERVATIONS.get_or_init(Mutex::default)
}
struct Reservation(u64);
impl Drop for Reservation {
    fn drop(&mut self) {
        reservations()
            .lock()
            .unwrap()
            .retain(|(id, _)| *id != self.0);
    }
}
async fn reserve(job: &Job) -> Reservation {
    let (id, locations) = {
        let data = job.data.lock().unwrap();
        let mut locations = data.sources.clone();
        for source in &data.sources {
            locations.push(
                source
                    .name()
                    .and_then(|name| data.destination.child(&name))
                    .unwrap_or_else(|_| data.destination.clone()),
            );
        }
        (data.id, locations)
    };
    loop {
        {
            let mut active = reservations().lock().unwrap();
            if !active.iter().any(|(_, held)| {
                held.iter()
                    .any(|a| locations.iter().any(|b| locations_overlap(a, b)))
            }) {
                active.push((id, locations));
                return Reservation(id);
            }
        }
        set_state(
            job,
            State::Queued,
            "Waiting for another transfer using these files",
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
fn pool(site: &str) -> Arc<Pool> {
    static POOLS: OnceLock<Mutex<BTreeMap<String, Arc<Pool>>>> = OnceLock::new();
    POOLS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap()
        .entry(site.into())
        .or_insert_with(|| {
            Arc::new(Pool {
                semaphore: tokio::sync::Semaphore::new(2),
                lanes: Mutex::new(vec![1, 2]),
            })
        })
        .clone()
}
async fn run_with_retries(job: &Job, site: &str, lane: usize, discard: bool) -> io::Result<()> {
    let mut retries = 0;
    loop {
        set_state(job, State::Connecting, "Connecting");
        let result = async {
            let resolved = super::remote_download::endpoint_key(&super::clipboard::ClipboardDownload { url: gpui::http_client::Url::parse(site).map_err(io::Error::other)?, file_name: String::new() });
            if job.data.lock().unwrap().endpoint != resolved {
                return Err(io::Error::other("The SSH site configuration changed. Start a new transfer after reviewing the host, port, and account."));
            }
            let session = remote_fs::session(site, lane).await?;
            if job.data.lock().unwrap().endpoint != session.endpoint {
                return Err(io::Error::other("The SSH host, port, or account changed since this transfer was queued. Start a new transfer after reviewing the site configuration."));
            }
            if discard { discard_partials(job, &session).await } else { run(job, &session).await }
        }.await;
        match result {
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionRefused
                ) && retries < 5 =>
            {
                remote_fs::disconnect(site, lane).await;
                retries += 1;
                set_state(
                    job,
                    State::Reconnecting,
                    format!("Connection interrupted. Retry {retries}/5"),
                );
                save(job)?;
                tokio::time::sleep(Duration::from_secs(1 << retries)).await;
            }
            result => return result,
        }
    }
}

async fn discard_partials(job: &Job, session: &Session) -> io::Result<()> {
    set_state(job, State::Preparing, "Discarding retained partial files");
    let data = job.data.lock().unwrap().clone();
    if !valid_manifest(&data) {
        return Err(io::Error::other(
            "Invalid transfer manifest; no files were removed.",
        ));
    }
    for (index, item) in data.items.iter().enumerate() {
        if item.completed && !item.skipped {
            continue;
        }
        if let Some(partial) = &item.partial {
            if let Some(meta) = maybe_metadata(session, partial).await? {
                if meta.kind == Kind::Directory {
                    return Err(io::Error::other(
                        "A directory occupies a partial filename; it will not be removed.",
                    ));
                }
                remove(session, partial, false).await?;
            }
            job.data.lock().unwrap().items[index].partial = None;
            save(job)?;
        }
    }
    Ok(())
}

fn locations_overlap(a: &Location, b: &Location) -> bool {
    match (a, b) {
        (Location::Local(a), Location::Local(b)) => {
            #[cfg(any(windows, target_os = "macos"))]
            let (a, b) = (
                PathBuf::from(a.to_string_lossy().to_lowercase()),
                PathBuf::from(b.to_string_lossy().to_lowercase()),
            );
            a.starts_with(&b) || b.starts_with(&a)
        }
        // Conservatively serialize overlapping paths even across SSH aliases.
        (Location::Remote(a), Location::Remote(b)) => {
            a.path == b.path
                || a.path
                    .starts_with(&(b.path.trim_end_matches('/').to_owned() + "/"))
                || b.path
                    .starts_with(&(a.path.trim_end_matches('/').to_owned() + "/"))
        }
        _ => false,
    }
}

async fn metadata(session: &Session, loc: &Location) -> io::Result<Metadata> {
    match loc {
        Location::Remote(loc) => {
            let attrs = session.metadata(&loc.path).await?;
            let kind = if attrs.is_dir() {
                Kind::Directory
            } else if attrs.is_symlink() {
                let target = session
                    .raw
                    .readlink(&loc.path)
                    .await
                    .map_err(sftp_error)?
                    .files
                    .into_iter()
                    .next()
                    .ok_or_else(|| io::Error::other("Missing link target"))?
                    .filename;
                Kind::Link(target)
            } else if attrs.is_regular() {
                Kind::File
            } else {
                return Err(io::Error::other("Special files cannot be transferred"));
            };
            if kind == Kind::File && attrs.size.is_none() {
                return Err(io::Error::other("Server did not report the file size"));
            }
            Ok(Metadata {
                size: attrs.size.unwrap_or(0),
                modified: attrs.mtime.map(u64::from),
                nanos: None,
                owner: attrs.uid.zip(attrs.gid),
                mode: attrs.permissions,
                kind,
            })
        }
        Location::Local(path) => {
            let m = fs::symlink_metadata(path)?;
            let kind = if m.is_dir() {
                Kind::Directory
            } else if m.file_type().is_symlink() {
                Kind::Link(
                    fs::read_link(path)?
                        .to_str()
                        .ok_or_else(|| io::Error::other("Link target is not UTF-8"))?
                        .into(),
                )
            } else if m.is_file() {
                Kind::File
            } else {
                return Err(io::Error::other("Special files cannot be transferred"));
            };
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                Some(m.permissions().mode())
            };
            #[cfg(not(unix))]
            let mode = None;
            Ok(Metadata {
                size: m.len(),
                modified: m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|t| t.as_secs()),
                nanos: m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|t| t.subsec_nanos()),
                owner: None,
                mode,
                kind,
            })
        }
    }
}
async fn maybe_metadata(session: &Session, loc: &Location) -> io::Result<Option<Metadata>> {
    match metadata(session, loc).await {
        Ok(m) => Ok(Some(m)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
async fn children(session: &Session, loc: &Location) -> io::Result<Vec<String>> {
    match loc {
        Location::Remote(l) => Ok(session
            .list(l)
            .await?
            .into_iter()
            .map(|(name, _)| name)
            .collect()),
        Location::Local(p) => fs::read_dir(p)?
            .map(|e| {
                e?.file_name()
                    .into_string()
                    .map_err(|_| io::Error::other("Filename is not UTF-8"))
            })
            .collect(),
    }
}
async fn plan(job: &Job, session: &Session) -> io::Result<()> {
    set_state(job, State::Preparing, "Listing folders");
    let data = job.data.lock().unwrap().clone();
    let rename_only = data.move_sources
        && matches!(data.destination, Location::Remote(_))
        && data
            .sources
            .iter()
            .all(|s| matches!(s, Location::Remote(_)));
    let mut stack = Vec::new();
    for source in data.sources {
        let dest = data.destination.child(&source.name()?)?;
        stack.push((source, dest));
    }
    let mut items = Vec::new();
    let mut destinations = std::collections::HashSet::new();
    while let Some((source, destination)) = stack.pop() {
        let meta = metadata(session, &source).await?;
        let key = match &destination {
            Location::Local(_) if cfg!(windows) || cfg!(target_os = "macos") => {
                destination.label().to_lowercase()
            }
            _ => destination.label(),
        };
        if !destinations.insert(key) {
            return Err(io::Error::other(
                "Destination filename collision. Rename the conflicting source files before transferring.",
            ));
        }
        if meta.kind == Kind::Directory && !rename_only {
            for name in children(session, &source).await? {
                stack.push((source.child(&name)?, destination.child(&name)?));
            }
        }
        items.push(Item {
            target_seen: false,
            target_before: None,
            source_removed: false,
            source,
            destination,
            metadata: meta,
            completed: false,
            partial: None,
            committing: false,
            skipped: false,
        });
    }
    let mut m = job.data.lock().unwrap();
    m.total = items
        .iter()
        .filter(|i| i.metadata.kind == Kind::File)
        .map(|i| i.metadata.size)
        .sum();
    m.items = items;
    m.planned = true;
    drop(m);
    save(job)
}
async fn run(job: &Job, session: &Session) -> io::Result<()> {
    if !job.data.lock().unwrap().planned {
        plan(job, session).await?;
    }
    let rename_only = {
        let m = job.data.lock().unwrap();
        m.move_sources
            && matches!(m.destination, Location::Remote(_))
            && m.sources.iter().all(|s| matches!(s, Location::Remote(_)))
    };
    if rename_only {
        return run_remote_moves(job, session).await;
    }
    loop {
        let data = job.data.lock().unwrap().clone();
        if data.current >= data.items.len() {
            break;
        }
        let mut item = data.items[data.current].clone();
        if item.completed {
            job.data.lock().unwrap().current += 1;
            continue;
        }
        set_state(job, State::Transferring, "Transferring");
        if metadata(session, &item.source).await? != item.metadata {
            return Err(io::Error::other(
                "Source changed since this transfer was planned. Cancel and start a new transfer.",
            ));
        }
        let mut existing = if item.target_seen {
            item.target_before.clone()
        } else {
            maybe_metadata(session, &item.destination).await?
        };
        if item.metadata.kind == Kind::Directory {
            if existing.as_ref().is_some_and(|m| m.kind != Kind::Directory) {
                return Err(io::Error::other(
                    "A file occupies the destination folder name.",
                ));
            }
            if existing.is_none() {
                mkdir(session, &item.destination).await?;
            }
        } else {
            // A lost rename reply must be reconciled before any overwrite/retry.
            if item.committing && item.partial.is_some() {
                let partial = item.partial.as_ref().unwrap();
                if maybe_metadata(session, partial).await?.is_none() {
                    let final_meta = maybe_metadata(session, &item.destination).await?;
                    let matches = match (&item.metadata.kind, final_meta) {
                        (Kind::Link(target), Some(meta)) => meta.kind == Kind::Link(target.clone()),
                        (Kind::File, Some(meta)) if meta.kind == Kind::File => {
                            equal_content(
                                session,
                                &item.source,
                                &item.destination,
                                item.metadata.size,
                            )
                            .await?
                        }
                        _ => false,
                    };
                    if matches {
                        finish_item(job, item)?;
                        continue;
                    }
                    return Err(io::Error::other(
                        "Finalization outcome is uncertain. The partial is missing and the destination does not match; inspect both endpoints before starting a new transfer.",
                    ));
                }
            }
            if item.committing && data.conflict == Conflict::KeepBoth {
                item.committing = false;
            }
            if existing.is_some() && !item.committing {
                match data.conflict {
                    Conflict::Ask => {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            format!(
                                "{} already exists. Choose Replace all, Skip all, or Keep both.",
                                item.destination.name().unwrap_or_default()
                            ),
                        ));
                    }
                    Conflict::Skip => {
                        item.skipped = true;
                        finish_item(job, item)?;
                        continue;
                    }
                    Conflict::KeepBoth => {
                        let parent = item.destination.parent()?;
                        let name = item.destination.name()?;
                        let (stem, extension) = name
                            .rsplit_once('.')
                            .filter(|(stem, _)| !stem.is_empty())
                            .map_or((name.as_str(), String::new()), |(stem, ext)| {
                                (stem, format!(".{ext}"))
                            });
                        let mut available = None;
                        for suffix in 2..100_000 {
                            let candidate =
                                parent.child(&format!("{stem} ({suffix}){extension}"))?;
                            if maybe_metadata(session, &candidate).await?.is_none() {
                                available = Some(candidate);
                                break;
                            }
                        }
                        item.destination = available
                            .ok_or_else(|| io::Error::other("No available destination filename"))?;
                        existing = None;
                        item.target_seen = false;
                    }
                    Conflict::Replace => {
                        if existing.as_ref().is_some_and(|m| {
                            m.kind == Kind::Directory || matches!(m.kind, Kind::Link(_))
                        }) {
                            return Err(io::Error::other(
                                "Replacing a directory or symbolic link requires an explicit rename or delete first.",
                            ));
                        }
                    }
                }
            }
            if !item.target_seen {
                item.target_seen = true;
                item.target_before = existing.clone();
                job.data.lock().unwrap().items[data.current] = item.clone();
                save(job)?;
            }
            if item.partial.is_none() {
                item.partial = Some(
                    item.destination
                        .parent()?
                        .child(&format!(".explorer-{}-{}.filepart", data.id, data.current))?,
                );
                if maybe_metadata(session, item.partial.as_ref().unwrap())
                    .await?
                    .is_some()
                {
                    return Err(io::Error::other(
                        "An unowned partial file already exists. It will not be overwritten.",
                    ));
                }
                job.data.lock().unwrap().items[data.current] = item.clone();
                save(job)?;
            }
            let partial = item.partial.as_ref().unwrap();
            match &item.metadata.kind {
                Kind::File => transfer_file(job, session, &item).await?,
                Kind::Link(target) => {
                    if maybe_metadata(session, partial).await?.is_none() {
                        create_link(session, partial, target).await?;
                    }
                }
                Kind::Directory => unreachable!(),
            }
            if metadata(session, &item.source).await? != item.metadata {
                return Err(io::Error::other(
                    "Source changed during transfer; destination was not replaced.",
                ));
            }
            let current = maybe_metadata(session, &item.destination).await?;
            if current != existing {
                return Err(io::Error::other(
                    "Destination changed during transfer. Review it before replacing.",
                ));
            }
            item.committing = true;
            job.data.lock().unwrap().items[data.current] = item.clone();
            save(job)?;
            replace(session, partial, &item.destination, current.is_some()).await?;
        }
        finish_item(job, item)?;
    }
    set_state(job, State::Finalizing, "Finishing folders and move cleanup");
    let data = job.data.lock().unwrap().clone();
    // Apply directory times after descendants have been created.
    for item in data
        .items
        .iter()
        .rev()
        .filter(|i| i.metadata.kind == Kind::Directory && !i.skipped)
    {
        if let Err(e) = set_attributes(session, &item.destination, &item.metadata, None).await {
            job.data.lock().unwrap().warnings.push(format!(
                "{}: {e}",
                item.destination.name().unwrap_or_default()
            ));
        }
    }
    if data.move_sources {
        // Do not delete any source when the job skipped entries.
        if data.items.iter().any(|i| i.skipped) {
            job.data
                .lock()
                .unwrap()
                .warnings
                .push("Some items were skipped; sources were retained.".into());
        } else {
            for (index, item) in data.items.iter().enumerate().rev() {
                if item.source_removed {
                    continue;
                }
                if maybe_metadata(session, &item.source).await?.is_none() {
                    job.data.lock().unwrap().items[index].source_removed = true;
                    save(job)?;
                    continue;
                }
                if item.metadata.kind != Kind::Directory
                    && metadata(session, &item.source).await? != item.metadata
                {
                    return Err(io::Error::other(
                        "Source changed before move cleanup; retained source.",
                    ));
                }
                remove(session, &item.source, item.metadata.kind == Kind::Directory).await?;
                job.data.lock().unwrap().items[index].source_removed = true;
                save(job)?;
            }
        }
    }
    Ok(())
}
fn finish_item(job: &Job, mut item: Item) -> io::Result<()> {
    item.completed = true;
    let mut m = job.data.lock().unwrap();
    let index = m.current;
    if !item.skipped && item.metadata.kind == Kind::File {
        m.bytes += item.metadata.size;
    }
    m.items[index] = item;
    m.current += 1;
    drop(m);
    save(job)
}

async fn keep_both_destination(session: &Session, destination: &Location) -> io::Result<Location> {
    let parent = destination.parent()?;
    let name = destination.name()?;
    let (stem, extension) = name
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map_or((name.as_str(), String::new()), |(stem, ext)| {
            (stem, format!(".{ext}"))
        });
    for suffix in 2..100_000 {
        let candidate = parent.child(&format!("{stem} ({suffix}){extension}"))?;
        if maybe_metadata(session, &candidate).await?.is_none() {
            return Ok(candidate);
        }
    }
    Err(io::Error::other("No available destination filename"))
}

async fn run_remote_moves(job: &Job, session: &Session) -> io::Result<()> {
    loop {
        let data = job.data.lock().unwrap().clone();
        let Some(mut item) = data.items.get(data.current).cloned() else {
            return Ok(());
        };
        if item.source == item.destination {
            return Err(io::Error::other("Source and destination are the same."));
        }
        set_state(job, State::Transferring, "Moving");
        let source = maybe_metadata(session, &item.source).await?;
        let mut existing = maybe_metadata(session, &item.destination).await?;
        if item.committing && source.is_none() {
            if existing == Some(item.metadata.clone()) {
                item.source_removed = true;
                finish_item(job, item)?;
                continue;
            }
            return Err(io::Error::other(
                "Move outcome is uncertain; inspect the source and destination before continuing.",
            ));
        }
        if source != Some(item.metadata.clone()) {
            return Err(io::Error::other(
                "Source changed since the move was planned.",
            ));
        }
        if existing.is_some() {
            match data.conflict {
                Conflict::Ask => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "Destination exists. Choose Replace all, Skip conflicts, or Keep both.",
                    ));
                }
                Conflict::Skip => {
                    item.skipped = true;
                    finish_item(job, item)?;
                    continue;
                }
                Conflict::KeepBoth => {
                    item.destination = keep_both_destination(session, &item.destination).await?;
                    existing = None;
                    item.target_seen = false;
                }
                Conflict::Replace
                    if item.metadata.kind == Kind::Directory
                        || existing.as_ref().is_some_and(|m| m.kind != Kind::File) =>
                {
                    return Err(io::Error::other(
                        "Use Keep both to move this folder or link without replacing the existing destination.",
                    ));
                }
                Conflict::Replace => {}
            }
        }
        if item.target_seen && item.target_before != existing {
            return Err(io::Error::other(
                "Destination changed during the interrupted move.",
            ));
        }
        item.target_seen = true;
        item.target_before = existing.clone();
        item.committing = true;
        job.data.lock().unwrap().items[data.current] = item.clone();
        save(job)?;
        replace(session, &item.source, &item.destination, existing.is_some()).await?;
        item.source_removed = true;
        finish_item(job, item)?;
    }
}

enum Handle {
    Local(File),
    Remote(String),
}
async fn open(session: &Session, loc: &Location, write: bool, create: bool) -> io::Result<Handle> {
    match loc {
        Location::Local(p) => Ok(Handle::Local(
            OpenOptions::new()
                .read(true)
                .write(write)
                .create_new(create)
                .open(p)?,
        )),
        Location::Remote(l) => {
            let mut flags = if write {
                OpenFlags::READ | OpenFlags::WRITE
            } else {
                OpenFlags::READ
            };
            if create {
                flags |= OpenFlags::CREATE | OpenFlags::EXCLUDE;
            }
            Ok(Handle::Remote(
                session
                    .raw
                    .open(&l.path, flags, FileAttributes::empty())
                    .await
                    .map_err(sftp_error)?
                    .handle,
            ))
        }
    }
}
async fn read(
    session: &Session,
    handle: &mut Handle,
    offset: u64,
    length: usize,
) -> io::Result<Vec<u8>> {
    match handle {
        Handle::Local(file) => {
            file.seek(SeekFrom::Start(offset))?;
            let mut b = vec![0; length];
            file.read_exact(&mut b)?;
            Ok(b)
        }
        Handle::Remote(handle) => {
            let mut bytes = Vec::with_capacity(length);
            while bytes.len() < length {
                let data = session
                    .raw
                    .read(
                        &*handle,
                        offset + bytes.len() as u64,
                        (length - bytes.len()) as u32,
                    )
                    .await
                    .map_err(sftp_error)?
                    .data;
                if data.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Remote file ended early",
                    ));
                }
                bytes.extend_from_slice(&data);
            }
            Ok(bytes)
        }
    }
}
async fn write(
    session: &Session,
    handle: &mut Handle,
    offset: u64,
    bytes: Vec<u8>,
) -> io::Result<()> {
    match handle {
        Handle::Local(file) => {
            file.seek(SeekFrom::Start(offset))?;
            file.write_all(&bytes)
        }
        Handle::Remote(handle) => {
            session
                .raw
                .write(&*handle, offset, bytes)
                .await
                .map_err(sftp_error)?;
            Ok(())
        }
    }
}
async fn close(session: &Session, handle: Handle, sync: bool) -> io::Result<()> {
    match handle {
        Handle::Local(file) => {
            if sync {
                file.sync_all()
            } else {
                Ok(())
            }
        }
        Handle::Remote(handle) => {
            let flushed = if sync && session.extensions.contains_key("fsync@openssh.com") {
                session
                    .raw
                    .fsync(&handle)
                    .await
                    .map_err(sftp_error)
                    .map(|_| ())
            } else {
                Ok(())
            };
            let closed = session.raw.close(handle).await.map_err(sftp_error);
            flushed?;
            closed?;
            Ok(())
        }
    }
}
async fn transfer_file(job: &Job, session: &Session, item: &Item) -> io::Result<()> {
    let partial = item.partial.as_ref().unwrap();
    let meta = maybe_metadata(session, partial).await?;
    if meta
        .as_ref()
        .is_some_and(|m| m.kind != Kind::File || m.size > item.metadata.size)
    {
        return Err(io::Error::other(
            "Partial file is invalid; it will not be appended to.",
        ));
    }
    let offset = meta.as_ref().map_or(0, |m| m.size);
    let mut source = open(session, &item.source, false, false).await?;
    let output = open(session, partial, true, meta.is_none()).await;
    let mut output = match output {
        Ok(h) => h,
        Err(e) => {
            let _ = close(session, source, false).await;
            return Err(e);
        }
    };
    let result = async {
        if offset > 0 {
            set_state(job, State::Verifying, "Validating retained partial data");
            let mut at = 0;
            while at < offset {
                let length = (offset - at).min(session.chunk_size as u64) as usize;
                if read(session, &mut source, at, length).await?
                    != read(session, &mut output, at, length).await?
                {
                    return Err(io::Error::other(
                        "Partial content differs from the source. Cancel and start a new transfer.",
                    ));
                }
                at += length as u64;
            }
        }
        set_state(job, State::Transferring, "Transferring");
        record_progress(job, offset, 0);
        let mut at = offset;
        while at < item.metadata.size {
            // Keep memory and outstanding protocol requests bounded. Explicit
            // offsets permit multiple reads/writes without sharing a seek cursor.
            let mut ranges = Vec::new();
            let mut next = at;
            for _ in 0..8 {
                if next >= item.metadata.size {
                    break;
                }
                let length = (item.metadata.size - next).min(session.chunk_size as u64) as usize;
                ranges.push((next, length));
                next += length as u64;
            }
            if let Handle::Remote(handle) = &source {
                let reads = ranges.iter().map(|&(position, length)| {
                    let mut handle = Handle::Remote(handle.clone());
                    async move { read(session, &mut handle, position, length).await }
                });
                let results = futures::future::join_all(reads).await;
                for ((position, _), result) in ranges.iter().zip(results) {
                    write(session, &mut output, *position, result?).await?;
                }
            } else {
                let mut blocks = Vec::new();
                for &(position, length) in &ranges {
                    blocks.push((
                        position,
                        read(session, &mut source, position, length).await?,
                    ));
                }
                if let Handle::Remote(handle) = &output {
                    let results =
                        futures::future::join_all(blocks.into_iter().map(|(position, bytes)| {
                            let mut handle = Handle::Remote(handle.clone());
                            async move { write(session, &mut handle, position, bytes).await }
                        }))
                        .await;
                    for result in results {
                        result?;
                    }
                } else {
                    return Err(io::Error::other(
                        "A native transfer must have a remote endpoint",
                    ));
                }
            }
            record_progress(job, next, next - at);
            at = next;
        }
        Ok(())
    }
    .await;
    set_state(job, State::Finalizing, "Finalizing");
    let closed_source = close(session, source, false).await;
    let closed_output = close(session, output, true).await;
    result?;
    closed_source?;
    closed_output?;
    if metadata(session, partial).await?.size != item.metadata.size {
        return Err(io::Error::other("Transferred file size does not match"));
    }
    let existing = maybe_metadata(session, &item.destination).await?;
    if let Err(error) = set_attributes(session, partial, &item.metadata, existing.as_ref()).await {
        if existing.is_some() {
            return Err(io::Error::other(format!(
                "Cannot preserve destination attributes; original retained: {error}"
            )));
        }
        job.data.lock().unwrap().warnings.push(format!(
            "{}: {error}",
            item.destination.name().unwrap_or_default()
        ));
    }
    Ok(())
}
async fn equal_content(
    session: &Session,
    left: &Location,
    right: &Location,
    size: u64,
) -> io::Result<bool> {
    if metadata(session, right).await?.size != size {
        return Ok(false);
    }
    let mut hashes = Vec::new();
    for loc in [left, right] {
        let mut handle = open(session, loc, false, false).await?;
        let result = async {
            let mut hash = Sha256::new();
            let mut offset = 0;
            while offset < size {
                let length = (size - offset).min(session.chunk_size as u64) as usize;
                hash.update(read(session, &mut handle, offset, length).await?);
                offset += length as u64;
            }
            Ok::<_, io::Error>(hash.finalize())
        }
        .await;
        let closed = close(session, handle, false).await;
        hashes.push(result?);
        closed?;
    }
    Ok(hashes[0] == hashes[1])
}
async fn mkdir(session: &Session, loc: &Location) -> io::Result<()> {
    match loc {
        Location::Local(p) => fs::create_dir(p),
        Location::Remote(l) => {
            session
                .raw
                .mkdir(&l.path, FileAttributes::empty())
                .await
                .map_err(sftp_error)?;
            Ok(())
        }
    }
}
async fn remove(session: &Session, loc: &Location, directory: bool) -> io::Result<()> {
    match loc {
        Location::Local(p) => {
            if directory {
                fs::remove_dir(p)
            } else {
                fs::remove_file(p)
            }
        }
        Location::Remote(l) => {
            if directory {
                session.raw.rmdir(&l.path).await.map_err(sftp_error)?;
            } else {
                session.raw.remove(&l.path).await.map_err(sftp_error)?;
            }
            Ok(())
        }
    }
}
async fn replace(
    session: &Session,
    from: &Location,
    to: &Location,
    overwrite: bool,
) -> io::Result<()> {
    match (from, to) {
        (Location::Remote(a), Location::Remote(b)) => {
            session.replace(&a.path, &b.path, overwrite).await
        }
        (Location::Local(a), Location::Local(b)) => {
            // tempfile uses platform-specific atomic replacement and no-clobber moves.
            let temp = tempfile::TempPath::try_from_path(a.clone()).map_err(io::Error::other)?;
            let result = if overwrite {
                temp.persist(b)
            } else {
                temp.persist_noclobber(b)
            };
            match result {
                Ok(()) => {
                    #[cfg(unix)]
                    {
                        File::open(b.parent().unwrap())?.sync_all()?;
                    }
                    Ok(())
                }
                Err(e) => {
                    let err = e.error;
                    let _ = e.path.keep();
                    Err(err)
                }
            }
        }
        _ => Err(io::Error::other("Invalid finalization locations")),
    }
}
async fn create_link(session: &Session, loc: &Location, target: &str) -> io::Result<()> {
    match loc {
        Location::Remote(l) => {
            session
                .raw
                .symlink(&l.path, target)
                .await
                .map_err(sftp_error)?;
            Ok(())
        }
        Location::Local(p) => {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(target, p)
            }
            #[cfg(windows)]
            {
                let _ = (p, target);
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Download symbolic links on a Unix filesystem, or skip them.",
                ))
            }
        }
    }
}
async fn set_attributes(
    session: &Session,
    loc: &Location,
    source: &Metadata,
    existing: Option<&Metadata>,
) -> io::Result<()> {
    match loc {
        Location::Remote(l) => {
            let mut attrs = FileAttributes::empty();
            if let Some(t) = source.modified.and_then(|t| u32::try_from(t).ok()) {
                attrs.atime = Some(t);
                attrs.mtime = Some(t);
            }
            attrs.permissions = existing.and_then(|m| m.mode).map(|m| m & 0o7777);
            if let Some((uid, gid)) = existing.and_then(|m| m.owner) {
                let partial = session.metadata(&l.path).await?;
                if partial.uid != Some(uid) || partial.gid != Some(gid) {
                    attrs.uid = Some(uid);
                    attrs.gid = Some(gid);
                }
            }
            session
                .raw
                .setstat(&l.path, attrs)
                .await
                .map_err(sftp_error)?;
            if let Some(existing) = existing {
                let actual = session.metadata(&l.path).await?;
                if existing
                    .owner
                    .is_some_and(|owner| actual.uid.zip(actual.gid) != Some(owner))
                    || existing.mode.is_some_and(|mode| {
                        actual.permissions.map(|p| p & 0o7777) != Some(mode & 0o7777)
                    })
                {
                    return Err(io::Error::other(
                        "Server did not preserve ownership or permissions",
                    ));
                }
            }
            Ok(())
        }
        Location::Local(p) => {
            if let Some(t) = source.modified {
                filetime::set_file_mtime(p, filetime::FileTime::from_unix_time(t as i64, 0))?;
            }
            #[cfg(unix)]
            if let Some(mode) = existing.and_then(|m| m.mode).or(source.mode) {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(p, fs::Permissions::from_mode(mode & 0o777))?;
            }
            Ok(())
        }
    }
}
fn validate_local_name(name: &str) -> io::Result<()> {
    if name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\0']) {
        return Err(io::Error::other("Invalid local filename"));
    }
    #[cfg(windows)]
    {
        let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
        if name.contains(['\\', ':', '*', '?', '"', '<', '>', '|'])
            || name.ends_with(['.', ' '])
            || name.chars().any(|c| c < ' ')
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ["COM", "LPT"].iter().any(|prefix| {
                stem.strip_prefix(prefix).is_some_and(|s| {
                    matches!(s, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
                })
            })
        {
            return Err(io::Error::other(format!(
                "{name:?} is not a valid Windows filename. Rename it before downloading."
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_job() -> Job {
        protocol_tests::job(
            Path::new("unused-test-state"),
            Location::Remote(RemoteLocation::parse("sftp://server/folder/file.txt").unwrap()),
            Location::Local(PathBuf::from("destination")),
        )
    }
    fn item(size: u64, completed: bool, skipped: bool) -> Item {
        Item {
            target_seen: false,
            target_before: None,
            source_removed: false,
            source: Location::Remote(
                RemoteLocation::parse("sftp://server/folder/file.txt").unwrap(),
            ),
            destination: Location::Local(PathBuf::from("destination/file.txt")),
            metadata: Metadata {
                size,
                modified: None,
                nanos: None,
                owner: None,
                mode: None,
                kind: Kind::File,
            },
            completed,
            skipped,
            partial: None,
            committing: false,
        }
    }
    #[test]
    fn remote_progress_uses_validated_prefix_without_counting_it_as_speed() {
        let job = test_job();
        let now = std::time::Instant::now();
        {
            let mut m = job.data.lock().unwrap();
            m.planned = true;
            m.state = State::Transferring;
            m.items = vec![
                item(1000, true, false),
                item(4000, false, false),
                item(8000, true, true),
            ];
            m.current = 1;
            m.bytes = 1000;
        }
        {
            let mut p = job.progress.lock().unwrap();
            p.reset_rate(now);
            p.record(now, 1, 2000, 0);
            p.record(now + Duration::from_secs(1), 1, 2500, 500);
        }
        let snapshot = job_snapshot(&job, now + Duration::from_secs(1)).unwrap();
        assert_eq!(
            (snapshot.bytes, snapshot.total, snapshot.percentage),
            (3500, 5000, Some(70))
        );
        assert_eq!(snapshot.speed, Some(500.0));
        assert_eq!(snapshot.remaining, Some(Duration::from_secs(3)));
        assert_eq!(snapshot.title(), "file.txt");
        for state in [
            State::Paused,
            State::Reconnecting,
            State::Finalizing,
            State::Verifying,
        ] {
            set_state(&job, state, "Waiting");
            let snapshot = job_snapshot(&job, std::time::Instant::now()).unwrap();
            assert!(snapshot.speed.is_none());
            assert!(snapshot.remaining.is_none());
        }
    }
    #[test]
    fn remote_speed_window_expires_and_reconnect_resets_sampling() {
        let now = std::time::Instant::now();
        let mut p = TransferProgress::default();
        p.reset_rate(now);
        for second in 1..=10 {
            p.record(now + Duration::from_secs(second), 0, second * 100, 100);
        }
        assert_eq!(p.speed(now + Duration::from_secs(10)), Some(100.0));
        assert_eq!(p.speed(now + Duration::from_secs(16)), None);
        p.reset_rate(now + Duration::from_secs(20));
        p.record(now + Duration::from_secs(21), 0, 1500, 500);
        assert_eq!(p.speed(now + Duration::from_secs(21)), Some(500.0));
    }
    #[test]
    fn remote_empty_jobs_use_items_and_unplanned_jobs_have_unknown_progress() {
        let job = test_job();
        let now = std::time::Instant::now();
        assert_eq!(job_snapshot(&job, now).unwrap().percentage, None);
        {
            let mut m = job.data.lock().unwrap();
            m.planned = true;
            m.items = vec![item(0, true, false), item(0, false, false)];
            m.current = 1;
        }
        assert_eq!(job_snapshot(&job, now).unwrap().percentage, Some(50));
    }
    #[test]
    fn remote_completion_is_hidden_but_notifies_and_retains_unresolved_jobs() {
        let job = test_job();
        {
            let mut m = job.data.lock().unwrap();
            m.items = vec![item(10, true, false)];
            m.planned = true;
            m.bytes = 10;
            m.current = 1;
        }
        let before = completion_revision();
        complete_job(&job);
        assert!(completion_revision() > before);
        assert!(job_snapshot(&job, std::time::Instant::now()).is_none());
        job.data
            .lock()
            .unwrap()
            .warnings
            .push("Timestamp could not be preserved".into());
        assert!(job_snapshot(&job, std::time::Instant::now()).is_some());
        {
            let mut m = job.data.lock().unwrap();
            m.warnings.clear();
            m.items[0].skipped = true;
        }
        assert!(job_snapshot(&job, std::time::Instant::now()).is_some());
        for state in [State::Paused, State::Attention, State::Cancelled] {
            set_state(&job, state, "Review required");
            assert!(job_snapshot(&job, std::time::Instant::now()).is_some());
        }
    }
    #[test]
    fn remote_speed_window_survives_file_finalization_but_resets_on_reconnect() {
        let job = test_job();
        set_state(&job, State::Transferring, "Transferring");
        record_progress(&job, 100, 100);
        set_state(&job, State::Finalizing, "Finalizing");
        set_state(&job, State::Transferring, "Transferring");
        assert_eq!(job.progress.lock().unwrap().payload, 100);
        set_state(&job, State::Reconnecting, "Reconnecting");
        set_state(&job, State::Transferring, "Transferring");
        assert_eq!(job.progress.lock().unwrap().payload, 0);
    }
    #[test]
    fn remote_legacy_verification_flag_is_ignored_and_not_written() {
        let job = test_job();
        let mut value = serde_json::to_value(&*job.data.lock().unwrap()).unwrap();
        value["verify"] = true.into();
        let manifest: Manifest = serde_json::from_value(value).unwrap();
        assert!(
            serde_json::to_value(manifest)
                .unwrap()
                .get("verify")
                .is_none()
        );
    }
    #[test]
    fn local_children_cannot_escape_destination() {
        let root = Location::Local(PathBuf::from("destination"));
        for name in ["..", ".", "", "a/b", "a\0b"] {
            assert!(root.child(name).is_err());
        }
    }
    #[test]
    fn remote_paths_remain_typed_in_manifests() {
        let loc = Location::from_path(
            &RemoteLocation::parse("sftp://alice@server/folder")
                .unwrap()
                .provider_path(),
        )
        .unwrap();
        let json = serde_json::to_string(&loc).unwrap();
        assert!(json.contains("Remote"));
        assert!(!json.contains("explorer.sftp"));
        assert_eq!(serde_json::from_str::<Location>(&json).unwrap(), loc);
    }
}

#[cfg(test)]
#[path = "remote_transfer_tests.rs"]
mod protocol_tests;
