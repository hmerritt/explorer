use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use serde::Serialize;

const SAMPLE_INTERVAL: Duration = Duration::from_millis(250);
const STALL_THRESHOLD: Duration = Duration::from_millis(500);
const SLOW_ENTRY_LIMIT: usize = 10;
static NEXT_OPERATION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub(super) struct ArchiveDiagnostics {
    inner: Arc<Mutex<OperationState>>,
}

impl PartialEq for ArchiveDiagnostics {
    fn eq(&self, other: &Self) -> bool {
        self.operation_id() == other.operation_id()
    }
}

impl Eq for ArchiveDiagnostics {}

#[derive(Debug)]
struct OperationState {
    operation_id: u64,
    requested_at: Instant,
    active_started_at: Instant,
    progress_dialog_at: Option<Instant>,
    conflict_started_at: Option<Instant>,
    conflict_wait: Duration,
    reload: Duration,
    metadata_resolution: Duration,
    cancel_requested_at: Option<Instant>,
    archives: Vec<ArchiveState>,
    outcome: &'static str,
}

#[derive(Debug)]
struct ArchiveState {
    archive_id: usize,
    format: String,
    backend: String,
    compressed_size: u64,
    cpu_started: Option<Duration>,
    entries_listed: usize,
    entries_planned: usize,
    metrics: Arc<ArchiveMetrics>,
    phases: BTreeMap<&'static str, Duration>,
    entries: Vec<EntryMetric>,
    outcome: &'static str,
}

#[derive(Debug, Default)]
pub(super) struct ArchiveMetrics {
    pub(super) compressed_bytes_read: AtomicU64,
    pub(super) decoded_bytes: AtomicU64,
    pub(super) output_bytes_written: AtomicU64,
    pub(super) logical_output_bytes: AtomicU64,
    pub(super) entries_completed: AtomicU64,
    pub(super) entries_skipped: AtomicU64,
    pub(super) entries_replaced: AtomicU64,
    pub(super) entries_rejected: AtomicU64,
    pub(super) files: AtomicU64,
    pub(super) directories: AtomicU64,
    pub(super) zero_byte_files: AtomicU64,
    pub(super) directory_creates: AtomicU64,
    pub(super) file_creates: AtomicU64,
    pub(super) metadata_operations: AtomicU64,
    pub(super) flushes: AtomicU64,
    pub(super) progress_callbacks: AtomicU64,
    pub(super) observer_callbacks: AtomicU64,
    pub(super) sampler_wakeups: AtomicU64,
    pub(super) diagnostics_nanos: AtomicU64,
}

pub(super) struct CountingReader<R> {
    inner: R,
    metrics: Option<Arc<ArchiveMetrics>>,
}

impl<R> CountingReader<R> {
    pub(super) fn new(inner: R, diagnostics: Option<&ArchiveHandle>) -> Self {
        Self {
            inner,
            metrics: diagnostics.map(|handle| handle.metrics.clone()),
        }
    }
}

impl<R: std::io::Read> std::io::Read for CountingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        if let Some(metrics) = &self.metrics {
            metrics
                .compressed_bytes_read
                .fetch_add(read as u64, Ordering::Relaxed);
        }
        Ok(read)
    }
}

impl<R: std::io::Seek> std::io::Seek for CountingReader<R> {
    fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(position)
    }
}

#[derive(Clone, Debug)]
pub(super) struct ArchiveHandle {
    operation: ArchiveDiagnostics,
    archive_id: usize,
    metrics: Arc<ArchiveMetrics>,
}

impl PartialEq for ArchiveHandle {
    fn eq(&self, other: &Self) -> bool {
        self.operation == other.operation && self.archive_id == other.archive_id
    }
}

impl Eq for ArchiveHandle {}

#[derive(Debug)]
pub(super) struct ArchiveSampler {
    stop: Option<std::sync::mpsc::Sender<()>>,
    handle: Option<std::thread::JoinHandle<Vec<CounterSample>>>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct CounterSample {
    elapsed: Duration,
    output_bytes: u64,
    entries_completed: u64,
}

#[derive(Clone, Debug)]
struct EntryMetric {
    index: usize,
    path: PathBuf,
    size: u64,
    elapsed: Duration,
    outcome: &'static str,
}

#[derive(Serialize)]
struct JsonEvent<'a, T: Serialize> {
    schema_version: u8,
    domain: &'static str,
    event: &'static str,
    operation_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    archive_id: Option<usize>,
    outcome: &'a str,
    measurement_quality: &'static str,
    #[serde(flatten)]
    fields: T,
}

#[derive(Serialize)]
struct ArchiveSummary {
    format: String,
    backend: String,
    compressed_size_bytes: u64,
    compressed_bytes_read: u64,
    compressed_read_quality: &'static str,
    decoded_bytes: u64,
    logical_output_bytes: u64,
    physical_write_bytes: u64,
    compression_ratio: f64,
    archive_read_mib_s: f64,
    decode_mib_s: f64,
    output_write_mib_s: f64,
    files_per_second: f64,
    entries_listed: usize,
    entries_planned: usize,
    entries_completed: u64,
    entries_skipped: u64,
    entries_replaced: u64,
    entries_rejected: u64,
    files: u64,
    directories: u64,
    zero_byte_files: u64,
    size_buckets: SizeBuckets,
    directory_creates: u64,
    file_creates: u64,
    metadata_operations: u64,
    flushes: u64,
    progress_callbacks: u64,
    wall_ms: f64,
    time_to_first_output_ms: Option<f64>,
    time_to_first_completed_entry_ms: Option<f64>,
    last_output_to_finish_ms: Option<f64>,
    active_ms: f64,
    stalled_ms: f64,
    zero_progress_ms: f64,
    longest_stall_ms: f64,
    slowest_1s_mib_s: f64,
    slowest_5s_mib_s: f64,
    entry_average_ms: f64,
    entry_p50_ms: f64,
    entry_p95_ms: f64,
    entry_p99_ms: f64,
    phases_ms: BTreeMap<&'static str, f64>,
    phases_pct: BTreeMap<&'static str, f64>,
    bottleneck: &'static str,
    observer_callbacks: u64,
    sampler_wakeups: u64,
    diagnostics_overhead_ms: f64,
    slowest_entries: Vec<SlowEntrySummary>,
    cpu_time_ms: Option<f64>,
    cpu_to_wall_ratio: Option<f64>,
}

#[derive(Default, Serialize)]
struct SizeBuckets {
    zero: u64,
    bytes_1_to_4k: u64,
    bytes_4k_to_64k: u64,
    bytes_64k_to_1m: u64,
    bytes_1m_to_16m: u64,
    bytes_16m_plus: u64,
}

#[derive(Serialize)]
struct SlowEntrySummary {
    index: usize,
    size_bytes: u64,
    elapsed_ms: f64,
    outcome: &'static str,
}

#[derive(Serialize)]
struct SlowEntryVerbose<'a> {
    index: usize,
    path: &'a Path,
    size_bytes: u64,
    elapsed_ms: f64,
}

#[derive(Serialize)]
struct OperationSummary {
    archives: usize,
    request_wall_ms: f64,
    active_pipeline_ms: f64,
    conflict_wait_ms: f64,
    request_to_progress_dialog_ms: Option<f64>,
    final_reload_ms: f64,
    metadata_resolution_ms: f64,
    cancellation_response_ms: Option<f64>,
    compressed_bytes_read: u64,
    decoded_bytes: u64,
    logical_output_bytes: u64,
    physical_write_bytes: u64,
    entries_completed: u64,
}

impl ArchiveDiagnostics {
    pub(super) fn start() -> Option<Self> {
        crate::debug_options::archive_timings_enabled().then(|| {
            let now = Instant::now();
            Self {
                inner: Arc::new(Mutex::new(OperationState {
                    operation_id: NEXT_OPERATION_ID.fetch_add(1, Ordering::Relaxed),
                    requested_at: now,
                    active_started_at: now,
                    progress_dialog_at: None,
                    conflict_started_at: None,
                    conflict_wait: Duration::ZERO,
                    reload: Duration::ZERO,
                    metadata_resolution: Duration::ZERO,
                    cancel_requested_at: None,
                    archives: Vec::new(),
                    outcome: "error",
                })),
            }
        })
    }

    pub(super) fn operation_id(&self) -> u64 {
        self.inner.lock().expect("archive diagnostics").operation_id
    }

    pub(super) fn add_archive(
        &self,
        format: impl Into<String>,
        backend: impl Into<String>,
        compressed_size: u64,
        entries_listed: usize,
        entries_planned: usize,
    ) -> ArchiveHandle {
        let mut state = self.inner.lock().expect("archive diagnostics");
        let archive_id = state.archives.len() + 1;
        let metrics = Arc::new(ArchiveMetrics::default());
        state.archives.push(ArchiveState {
            archive_id,
            format: format.into(),
            backend: backend.into(),
            compressed_size,
            cpu_started: process_cpu_time(),
            entries_listed,
            entries_planned,
            metrics: metrics.clone(),
            phases: BTreeMap::new(),
            entries: Vec::new(),
            outcome: "error",
        });
        ArchiveHandle {
            operation: self.clone(),
            archive_id,
            metrics,
        }
    }

    pub(super) fn mark_conflict_wait_started(&self) {
        self.inner
            .lock()
            .expect("archive diagnostics")
            .conflict_started_at = Some(Instant::now());
    }

    pub(super) fn mark_conflict_wait_finished(&self) {
        let mut state = self.inner.lock().expect("archive diagnostics");
        if let Some(started) = state.conflict_started_at.take() {
            state.conflict_wait += started.elapsed();
        }
    }

    pub(super) fn mark_progress_dialog_visible(&self) {
        let mut state = self.inner.lock().expect("archive diagnostics");
        state.progress_dialog_at.get_or_insert_with(Instant::now);
    }

    pub(super) fn add_reload(&self, elapsed: Duration) {
        self.inner.lock().expect("archive diagnostics").reload += elapsed;
    }

    pub(super) fn add_metadata_resolution(&self, elapsed: Duration) {
        self.inner
            .lock()
            .expect("archive diagnostics")
            .metadata_resolution += elapsed;
    }

    pub(super) fn mark_cancel_requested(&self) {
        let mut state = self.inner.lock().expect("archive diagnostics");
        state.cancel_requested_at.get_or_insert_with(Instant::now);
    }

    pub(super) fn finish(&self, outcome: &'static str) {
        let (operation_id, summary) = {
            let mut state = self.inner.lock().expect("archive diagnostics");
            state.outcome = outcome;
            let now = Instant::now();
            let summary = OperationSummary {
                archives: state.archives.len(),
                request_wall_ms: ms(now.duration_since(state.requested_at)),
                active_pipeline_ms: ms(now
                    .duration_since(state.active_started_at)
                    .saturating_sub(state.conflict_wait)),
                conflict_wait_ms: ms(state.conflict_wait),
                request_to_progress_dialog_ms: state
                    .progress_dialog_at
                    .map(|at| ms(at.duration_since(state.requested_at))),
                final_reload_ms: ms(state.reload),
                metadata_resolution_ms: ms(state.metadata_resolution),
                cancellation_response_ms: state
                    .cancel_requested_at
                    .map(|requested| ms(now.duration_since(requested))),
                compressed_bytes_read: sum(&state.archives, |m| &m.compressed_bytes_read),
                decoded_bytes: sum(&state.archives, |m| &m.decoded_bytes),
                logical_output_bytes: sum(&state.archives, |m| &m.logical_output_bytes),
                physical_write_bytes: sum(&state.archives, |m| &m.output_bytes_written),
                entries_completed: sum(&state.archives, |m| &m.entries_completed),
            };
            (state.operation_id, summary)
        };
        emit(JsonEvent {
            schema_version: 1,
            domain: "archive",
            event: "operation_summary",
            operation_id,
            archive_id: None,
            outcome,
            measurement_quality: "mixed",
            fields: summary,
        });
    }
}

impl ArchiveHandle {
    pub(super) fn metrics(&self) -> &Arc<ArchiveMetrics> {
        &self.metrics
    }

    pub(super) fn phase(&self, name: &'static str, elapsed: Duration) {
        let mut state = self.operation.inner.lock().expect("archive diagnostics");
        *state.archives[self.archive_id - 1]
            .phases
            .entry(name)
            .or_default() += elapsed;
    }

    pub(super) fn record_entry(
        &self,
        path: PathBuf,
        size: u64,
        elapsed: Duration,
        outcome: &'static str,
    ) {
        let mut state = self.operation.inner.lock().expect("archive diagnostics");
        let archive = &mut state.archives[self.archive_id - 1];
        archive.entries.push(EntryMetric {
            index: archive.entries.len(),
            path,
            size,
            elapsed,
            outcome,
        });
    }

    pub(super) fn sampler(&self) -> ArchiveSampler {
        ArchiveSampler::start(self.metrics.clone())
    }

    pub(super) fn finish(&self, outcome: &'static str, samples: Vec<CounterSample>) {
        let (operation_id, archive_id, summary, verbose_entries) = {
            let mut state = self.operation.inner.lock().expect("archive diagnostics");
            let operation_id = state.operation_id;
            let archive = &mut state.archives[self.archive_id - 1];
            archive.outcome = outcome;
            let summary = archive_summary(archive, &samples);
            let mut verbose_entries = archive.entries.clone();
            verbose_entries.sort_by_key(|entry| std::cmp::Reverse(entry.elapsed));
            verbose_entries.truncate(SLOW_ENTRY_LIMIT);
            (operation_id, archive.archive_id, summary, verbose_entries)
        };
        emit(JsonEvent {
            schema_version: 1,
            domain: "archive",
            event: "archive_summary",
            operation_id,
            archive_id: Some(archive_id),
            outcome,
            measurement_quality: "mixed",
            fields: summary,
        });
        if crate::debug_options::archive_verbose_enabled() {
            for entry in verbose_entries {
                emit(JsonEvent {
                    schema_version: 1,
                    domain: "archive",
                    event: "slow_entry",
                    operation_id,
                    archive_id: Some(archive_id),
                    outcome: entry.outcome,
                    measurement_quality: "entry-completion",
                    fields: SlowEntryVerbose {
                        index: entry.index,
                        path: &entry.path,
                        size_bytes: entry.size,
                        elapsed_ms: ms(entry.elapsed),
                    },
                });
            }
        }
    }
}

impl ArchiveSampler {
    fn start(metrics: Arc<ArchiveMetrics>) -> Self {
        let (stop, receiver) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let started = Instant::now();
            let mut samples = vec![sample(&metrics, started)];
            loop {
                match receiver.recv_timeout(SAMPLE_INTERVAL) {
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        metrics.sampler_wakeups.fetch_add(1, Ordering::Relaxed);
                        samples.push(sample(&metrics, started));
                    }
                }
            }
            samples.push(sample(&metrics, started));
            samples
        });
        Self {
            stop: Some(stop),
            handle: Some(handle),
        }
    }

    pub(super) fn finish(mut self) -> Vec<CounterSample> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.handle
            .take()
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default()
    }
}

fn sample(metrics: &ArchiveMetrics, started: Instant) -> CounterSample {
    CounterSample {
        elapsed: started.elapsed(),
        output_bytes: metrics.output_bytes_written.load(Ordering::Relaxed),
        entries_completed: metrics.entries_completed.load(Ordering::Relaxed),
    }
}

fn archive_summary(archive: &ArchiveState, samples: &[CounterSample]) -> ArchiveSummary {
    let wall = samples
        .last()
        .map_or(Duration::ZERO, |sample| sample.elapsed);
    let compressed = archive
        .metrics
        .compressed_bytes_read
        .load(Ordering::Relaxed);
    let decoded = archive.metrics.decoded_bytes.load(Ordering::Relaxed);
    let output = archive.metrics.output_bytes_written.load(Ordering::Relaxed);
    let logical = archive.metrics.logical_output_bytes.load(Ordering::Relaxed);
    let completed = archive.metrics.entries_completed.load(Ordering::Relaxed);
    let (stalled, longest_stall) = stall_metrics(samples);
    let mut latencies = archive
        .entries
        .iter()
        .map(|entry| entry.elapsed.as_secs_f64())
        .collect::<Vec<_>>();
    latencies.sort_by(f64::total_cmp);
    let phases_ms = archive
        .phases
        .iter()
        .map(|(name, duration)| (*name, ms(*duration)))
        .collect();
    let phases_pct = archive
        .phases
        .iter()
        .map(|(name, duration)| {
            (
                *name,
                if wall.is_zero() {
                    0.0
                } else {
                    duration.as_secs_f64() * 100.0 / wall.as_secs_f64()
                },
            )
        })
        .collect();
    let mut slowest = archive.entries.clone();
    slowest.sort_by_key(|entry| std::cmp::Reverse(entry.elapsed));
    slowest.truncate(SLOW_ENTRY_LIMIT);
    ArchiveSummary {
        format: archive.format.clone(),
        backend: archive.backend.clone(),
        compressed_size_bytes: archive.compressed_size,
        compressed_bytes_read: compressed,
        compressed_read_quality: if compressed == 0 {
            "unavailable"
        } else {
            "direct"
        },
        decoded_bytes: decoded,
        logical_output_bytes: logical,
        physical_write_bytes: output,
        compression_ratio: ratio(logical, archive.compressed_size),
        archive_read_mib_s: mib_s(compressed, wall),
        decode_mib_s: mib_s(decoded, wall),
        output_write_mib_s: mib_s(output, wall),
        files_per_second: per_second(completed, wall),
        entries_listed: archive.entries_listed,
        entries_planned: archive.entries_planned,
        entries_completed: completed,
        entries_skipped: archive.metrics.entries_skipped.load(Ordering::Relaxed),
        entries_replaced: archive.metrics.entries_replaced.load(Ordering::Relaxed),
        entries_rejected: archive.metrics.entries_rejected.load(Ordering::Relaxed),
        files: archive.metrics.files.load(Ordering::Relaxed),
        directories: archive.metrics.directories.load(Ordering::Relaxed),
        zero_byte_files: archive.metrics.zero_byte_files.load(Ordering::Relaxed),
        size_buckets: size_buckets(&archive.entries),
        directory_creates: archive.metrics.directory_creates.load(Ordering::Relaxed),
        file_creates: archive.metrics.file_creates.load(Ordering::Relaxed),
        metadata_operations: archive.metrics.metadata_operations.load(Ordering::Relaxed),
        flushes: archive.metrics.flushes.load(Ordering::Relaxed),
        progress_callbacks: archive.metrics.progress_callbacks.load(Ordering::Relaxed),
        wall_ms: ms(wall),
        time_to_first_output_ms: samples
            .iter()
            .find(|sample| sample.output_bytes > 0)
            .map(|sample| ms(sample.elapsed)),
        time_to_first_completed_entry_ms: samples
            .iter()
            .find(|sample| sample.entries_completed > 0)
            .map(|sample| ms(sample.elapsed)),
        last_output_to_finish_ms: samples
            .windows(2)
            .rfind(|window| window[1].output_bytes > window[0].output_bytes)
            .map(|window| ms(wall.saturating_sub(window[1].elapsed))),
        active_ms: ms(wall.saturating_sub(stalled)),
        stalled_ms: ms(stalled),
        zero_progress_ms: ms(stalled),
        longest_stall_ms: ms(longest_stall),
        slowest_1s_mib_s: slowest_window_mib_s(samples, Duration::from_secs(1)),
        slowest_5s_mib_s: slowest_window_mib_s(samples, Duration::from_secs(5)),
        entry_average_ms: average_ms(&latencies),
        entry_p50_ms: percentile_ms(&latencies, 50),
        entry_p95_ms: percentile_ms(&latencies, 95),
        entry_p99_ms: percentile_ms(&latencies, 99),
        phases_ms,
        phases_pct,
        bottleneck: bottleneck(&archive.phases, stalled, wall),
        observer_callbacks: archive.metrics.observer_callbacks.load(Ordering::Relaxed),
        sampler_wakeups: archive.metrics.sampler_wakeups.load(Ordering::Relaxed),
        diagnostics_overhead_ms: archive.metrics.diagnostics_nanos.load(Ordering::Relaxed) as f64
            / 1_000_000.0,
        slowest_entries: slowest
            .into_iter()
            .map(|entry| SlowEntrySummary {
                index: entry.index,
                size_bytes: entry.size,
                elapsed_ms: ms(entry.elapsed),
                outcome: entry.outcome,
            })
            .collect(),
        cpu_time_ms: archive
            .cpu_started
            .zip(process_cpu_time())
            .map(|(started, finished)| ms(finished.saturating_sub(started))),
        cpu_to_wall_ratio: archive.cpu_started.zip(process_cpu_time()).and_then(
            |(started, finished)| {
                (!wall.is_zero())
                    .then(|| finished.saturating_sub(started).as_secs_f64() / wall.as_secs_f64())
            },
        ),
    }
}

fn stall_metrics(samples: &[CounterSample]) -> (Duration, Duration) {
    let mut stalled = Duration::ZERO;
    let mut longest = Duration::ZERO;
    for window in samples.windows(2) {
        if window[0].output_bytes == window[1].output_bytes
            && window[0].entries_completed == window[1].entries_completed
        {
            let duration = window[1].elapsed.saturating_sub(window[0].elapsed);
            if duration >= STALL_THRESHOLD || stalled > Duration::ZERO {
                stalled += duration;
                longest = longest.max(duration);
            }
        }
    }
    (stalled, longest)
}

fn slowest_window_mib_s(samples: &[CounterSample], target: Duration) -> f64 {
    samples
        .iter()
        .enumerate()
        .filter_map(|(end, sample)| {
            let start = samples[..end].iter().rposition(|candidate| {
                sample.elapsed.saturating_sub(candidate.elapsed) >= target
            })?;
            let elapsed = sample.elapsed.saturating_sub(samples[start].elapsed);
            Some(mib_s(
                sample
                    .output_bytes
                    .saturating_sub(samples[start].output_bytes),
                elapsed,
            ))
        })
        .min_by(f64::total_cmp)
        .unwrap_or(0.0)
}

fn bottleneck(
    phases: &BTreeMap<&'static str, Duration>,
    stalled: Duration,
    wall: Duration,
) -> &'static str {
    let mut candidates = [
        ("stall", stalled),
        ("decode", *phases.get("decode").unwrap_or(&Duration::ZERO)),
        (
            "input_read",
            *phases.get("input_read").unwrap_or(&Duration::ZERO),
        ),
        (
            "output_write",
            *phases.get("output_write").unwrap_or(&Duration::ZERO),
        ),
        (
            "metadata",
            *phases.get("metadata").unwrap_or(&Duration::ZERO),
        ),
        (
            "rar_merge",
            *phases.get("rar_merge").unwrap_or(&Duration::ZERO),
        ),
    ];
    candidates.sort_by_key(|(_, duration)| std::cmp::Reverse(*duration));
    if wall.is_zero() || candidates[0].1.as_secs_f64() / wall.as_secs_f64() < 0.35 {
        "mixed"
    } else {
        candidates[0].0
    }
}

fn size_buckets(entries: &[EntryMetric]) -> SizeBuckets {
    let mut buckets = SizeBuckets::default();
    for entry in entries {
        match entry.size {
            0 => buckets.zero += 1,
            1..=4096 => buckets.bytes_1_to_4k += 1,
            4097..=65536 => buckets.bytes_4k_to_64k += 1,
            65537..=1_048_576 => buckets.bytes_64k_to_1m += 1,
            1_048_577..=16_777_216 => buckets.bytes_1m_to_16m += 1,
            _ => buckets.bytes_16m_plus += 1,
        }
    }
    buckets
}

fn sum(archives: &[ArchiveState], field: impl Fn(&ArchiveMetrics) -> &AtomicU64) -> u64 {
    archives
        .iter()
        .map(|archive| field(&archive.metrics).load(Ordering::Relaxed))
        .sum()
}

fn percentile_ms(values: &[f64], percentile: usize) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let rank = ((percentile * values.len()).div_ceil(100)).max(1);
    values[rank - 1] * 1000.0
}

fn average_ms(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() * 1000.0 / values.len() as f64
    }
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn per_second(count: u64, elapsed: Duration) -> f64 {
    if elapsed.is_zero() {
        0.0
    } else {
        count as f64 / elapsed.as_secs_f64()
    }
}

fn mib_s(bytes: u64, elapsed: Duration) -> f64 {
    per_second(bytes, elapsed) / (1024.0 * 1024.0)
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn emit(value: impl Serialize) {
    if let Ok(line) = serde_json::to_string(&value) {
        eprintln!("{line}");
    }
}

#[cfg(unix)]
fn process_cpu_time() -> Option<Duration> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    let usage = unsafe { usage.assume_init() };
    let user = Duration::new(
        usage.ru_utime.tv_sec as u64,
        (usage.ru_utime.tv_usec as u32).saturating_mul(1_000),
    );
    let system = Duration::new(
        usage.ru_stime.tv_sec as u64,
        (usage.ru_stime.tv_usec as u32).saturating_mul(1_000),
    );
    Some(user + system)
}

#[cfg(target_os = "windows")]
fn process_cpu_time() -> Option<Duration> {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentProcess, GetProcessTimes},
    };

    let mut creation = unsafe { std::mem::zeroed::<FILETIME>() };
    let mut exit = unsafe { std::mem::zeroed::<FILETIME>() };
    let mut kernel = unsafe { std::mem::zeroed::<FILETIME>() };
    let mut user = unsafe { std::mem::zeroed::<FILETIME>() };
    if unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return None;
    }
    let ticks = |time: FILETIME| ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64;
    Some(Duration::from_nanos(
        ticks(kernel)
            .saturating_add(ticks(user))
            .saturating_mul(100),
    ))
}

#[cfg(not(any(unix, target_os = "windows")))]
fn process_cpu_time() -> Option<Duration> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_buckets_cover_boundaries() {
        let entries = [0, 1, 4096, 4097, 65536, 65537, 1_048_577, 16_777_217]
            .into_iter()
            .enumerate()
            .map(|(index, size)| EntryMetric {
                index,
                path: PathBuf::new(),
                size,
                elapsed: Duration::ZERO,
                outcome: "ok",
            })
            .collect::<Vec<_>>();
        let buckets = size_buckets(&entries);
        assert_eq!(buckets.zero, 1);
        assert_eq!(buckets.bytes_1_to_4k, 2);
        assert_eq!(buckets.bytes_4k_to_64k, 2);
        assert_eq!(buckets.bytes_64k_to_1m, 1);
        assert_eq!(buckets.bytes_1m_to_16m, 1);
        assert_eq!(buckets.bytes_16m_plus, 1);
    }

    #[test]
    fn bottleneck_requires_dominant_phase() {
        let phases = BTreeMap::from([
            ("decode", Duration::from_millis(700)),
            ("output_write", Duration::from_millis(100)),
        ]);
        assert_eq!(
            bottleneck(&phases, Duration::ZERO, Duration::from_secs(1)),
            "decode"
        );
        assert_eq!(
            bottleneck(&phases, Duration::ZERO, Duration::from_secs(3)),
            "mixed"
        );
    }

    #[test]
    fn json_event_has_versioned_contract_without_paths() {
        let line = serde_json::to_string(&JsonEvent {
            schema_version: 1,
            domain: "archive",
            event: "operation_summary",
            operation_id: 42,
            archive_id: None,
            outcome: "ok",
            measurement_quality: "mixed",
            fields: serde_json::json!({"archives": 1}),
        })
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["domain"], "archive");
        assert_eq!(value["operation_id"], 42);
        assert!(!line.contains("path"));
    }

    #[test]
    fn operation_ids_are_monotonic() {
        let first = ArchiveDiagnostics {
            inner: Arc::new(Mutex::new(OperationState {
                operation_id: 10,
                requested_at: Instant::now(),
                active_started_at: Instant::now(),
                progress_dialog_at: None,
                conflict_started_at: None,
                conflict_wait: Duration::ZERO,
                reload: Duration::ZERO,
                metadata_resolution: Duration::ZERO,
                cancel_requested_at: None,
                archives: Vec::new(),
                outcome: "error",
            })),
        };
        let second = ArchiveDiagnostics {
            inner: Arc::new(Mutex::new(OperationState {
                operation_id: 11,
                requested_at: Instant::now(),
                active_started_at: Instant::now(),
                progress_dialog_at: None,
                conflict_started_at: None,
                conflict_wait: Duration::ZERO,
                reload: Duration::ZERO,
                metadata_resolution: Duration::ZERO,
                cancel_requested_at: None,
                archives: Vec::new(),
                outcome: "error",
            })),
        };
        assert!(first.operation_id() < second.operation_id());
    }
}
