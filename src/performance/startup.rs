//! Real-entry-point startup experiments, intentionally separate from the legacy UI worker.
mod runtime;
pub(crate) use runtime::{attach, initialize, is_active, launch_received};

use super::{fixtures::Fixtures, report::distribution};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const REQUEST_ENV: &str = "EXPLORER_STARTUP_BENCH_REQUEST";
const TIMEOUT: Duration = Duration::from_secs(30);
const HELP: &str = "explorer-bench startup [--executable PATH] [--baseline PATH] [--repetitions 30] [--filter TEXT] [--output DIRECTORY] [--diagnostic]\nBuild BOTH binaries with: cargo build --locked --release --features benchmarks --bins\nThe baseline executable must support this startup protocol. --baseline alternates executable order per repetition.\nDefault: all 24 Windows cases; e.g. --filter details/1000/tray/empty selects one. Diagnostic runs are reported separately.";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Request {
    version: u32,
    endpoint: SocketAddr,
    token: String,
    config: PathBuf,
    path: PathBuf,
    entries: usize,
}
impl Request {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || !self.endpoint.ip().is_loopback()
            || self.endpoint.port() == 0
            || self.token.is_empty()
            || !self.config.is_absolute()
            || !self.config.join("settings.json").is_file()
            || !self.path.is_absolute()
            || !self.path.is_dir()
        {
            return Err("invalid startup benchmark request".into());
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Event {
    token: String,
    name: String,
    cycle: u64,
    entry_ms: f64,
    scale_factor: Option<f32>,
    viewport: Option<[f32; 2]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Scenario {
    id: String,
    entries: usize,
    large_icons: bool,
    tray: bool,
    primed: bool,
}
fn scenarios() -> Vec<Scenario> {
    let mut result = Vec::new();
    for entries in [0, 1_000, 10_000] {
        for large_icons in [false, true] {
            for tray in if cfg!(target_os = "windows") {
                &[false, true][..]
            } else {
                &[false][..]
            } {
                for primed in [false, true] {
                    result.push(Scenario {
                        id: format!(
                            "startup-v1/{}/{entries}/{}/{}",
                            if large_icons {
                                "large_icons"
                            } else {
                                "details"
                            },
                            if *tray { "tray" } else { "no_tray" },
                            if primed { "primed" } else { "empty" }
                        ),
                        entries,
                        large_icons,
                        tray: *tray,
                        primed,
                    });
                }
            }
        }
    }
    result
}

struct Options {
    executable: PathBuf,
    baseline: Option<PathBuf>,
    repetitions: usize,
    filter: String,
    output: PathBuf,
    diagnostic: bool,
}
fn options(args: &[String]) -> Result<Options, String> {
    let mut result = Options {
        executable: std::env::current_exe()
            .map_err(|e| e.to_string())?
            .with_file_name(if cfg!(windows) {
                "explorer.exe"
            } else {
                "explorer"
            }),
        baseline: None,
        repetitions: 30,
        filter: String::new(),
        output: super::runner::target_dir().join("performance"),
        diagnostic: false,
    };
    let mut args = args.iter();
    while let Some(key) = args.next() {
        if key == "--diagnostic" {
            result.diagnostic = true;
            continue;
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {key}"))?;
        match key.as_str() {
            "--executable" => result.executable = value.into(),
            "--baseline" => result.baseline = Some(value.into()),
            "--repetitions" => {
                result.repetitions = value.parse().map_err(|_| "invalid repetitions")?
            }
            "--filter" => result.filter = value.clone(),
            "--output" => result.output = value.into(),
            _ => return Err(format!("unknown startup option {key}")),
        }
    }
    if !(1..=1_000).contains(&result.repetitions) {
        return Err("repetitions must be 1..=1000".into());
    }
    result.executable =
        fs::canonicalize(&result.executable).map_err(|e| format!("executable: {e}"))?;
    if let Some(path) = &mut result.baseline {
        *path = fs::canonicalize(&path).map_err(|e| format!("baseline: {e}"))?;
    }
    Ok(result)
}

#[derive(Debug, Serialize, Deserialize)]
struct Milestones {
    /// Launch to loopback notification receipt, including process loading and transport.
    first_frame_ms: f64,
    folder_frame_ms: f64,
    input_frame_ms: f64,
    entry_to_folder_ms: f64,
    scale_factor: Option<f32>,
    viewport: Option<[f32; 2]>,
}
#[derive(Debug, Serialize, Deserialize)]
struct Sample {
    repetition: usize,
    first_sample: bool,
    fresh: Milestones,
    resident_window: Milestones,
    resident_closed: Option<Milestones>,
}
#[derive(Debug, Serialize, Deserialize)]
struct Case {
    scenario: Scenario,
    variant: String,
    samples: Vec<Sample>,
    failures: Vec<String>,
}
#[derive(Serialize, Deserialize)]
struct Report {
    version: u32,
    diagnostic: bool,
    metadata: BTreeMap<String, String>,
    cases: Vec<Case>,
}

pub(super) fn main(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help") {
        println!("{HELP}");
        return Ok(());
    }
    if cfg!(debug_assertions) {
        return Err("startup timing requires --release".into());
    }
    let options = options(args)?;
    let cases = scenarios()
        .into_iter()
        .filter(|s| s.id.contains(&options.filter))
        .collect::<Vec<_>>();
    if cases.is_empty() {
        return Err("startup filter matches no scenarios".into());
    }
    fs::create_dir_all(&options.output).map_err(|e| e.to_string())?;
    let root = options.output.join(format!(
        "startup-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        std::process::id()
    ));
    fs::create_dir(&root).map_err(|e| e.to_string())?;
    let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let fixture = Fixtures::ensure(&super::runner::target_dir(), false)?;
    let mut metadata = super::runner::metadata();
    metadata.insert("display_backend".into(), std::env::consts::OS.into());
    #[cfg(target_os = "windows")]
    if let Ok(output) = Command::new("powershell.exe").args(["-NoProfile", "-Command",
        "Get-CimInstance Win32_VideoController | Select-Object Name,DriverVersion,CurrentHorizontalResolution,CurrentVerticalResolution,CurrentRefreshRate | ConvertTo-Json -Compress"]).output() {
        if output.status.success() {
            metadata.insert("video_controllers".into(), String::from_utf8_lossy(&output.stdout).trim().into());
        }
    }
    for (label, exe) in [
        ("after", Some(&options.executable)),
        ("before", options.baseline.as_ref()),
    ] {
        if let Some(exe) = exe {
            metadata.insert(format!("{label}_executable"), exe.display().to_string());
            metadata.insert(
                format!("{label}_sha256"),
                format!(
                    "{:x}",
                    Sha256::digest(fs::read(exe).map_err(|e| e.to_string())?)
                ),
            );
        }
    }
    let mut report = Report {
        version: 1,
        diagnostic: options.diagnostic,
        metadata,
        cases: Vec::new(),
    };
    println!("Startup results: {}", root.display());
    for scenario in cases {
        let start = report.cases.len();
        let variants = if let Some(baseline) = &options.baseline {
            vec![("before", baseline), ("after", &options.executable)]
        } else {
            vec![("after", &options.executable)]
        };
        for (label, _) in &variants {
            report.cases.push(Case {
                scenario: scenario.clone(),
                variant: (*label).into(),
                samples: Vec::new(),
                failures: Vec::new(),
            });
        }
        for repetition in 0..options.repetitions {
            let mut order = (0..variants.len()).collect::<Vec<_>>();
            if repetition % 2 == 1 {
                order.reverse()
            }
            for index in order {
                let (label, executable) = variants[index];
                let sample_root = root.join(format!("case-{start}-{label}-{repetition}"));
                let result = run_sample(
                    executable,
                    &scenario,
                    &fixture,
                    &sample_root,
                    repetition,
                    options.diagnostic,
                );
                let case = &mut report.cases[start + index];
                match result {
                    Ok(sample) => case.samples.push(sample),
                    Err(error) => {
                        eprintln!("{} {label} sample {repetition}: {error}", scenario.id);
                        case.failures.push(format!("sample {repetition}: {error}"));
                    }
                }
                save(&report, &root)?;
                if repetition == 0 && !report.cases[start + index].failures.is_empty() {
                    return Err(
                        "first sample failed; stopping startup suite (see report.md)".into(),
                    );
                }
            }
        }
        println!("Completed {}", scenario.id);
    }
    save(&report, &root)?;
    if report.cases.iter().any(|c| !c.failures.is_empty()) {
        return Err("startup suite contains failures; see report.md".into());
    }
    Ok(())
}

fn write_settings(config: &Path, scenario: &Scenario, path: &Path) -> Result<(), String> {
    fs::create_dir_all(config).map_err(|e| e.to_string())?;
    let mut settings = crate::settings::ExplorerSettings::default();
    settings.app.start = path.to_path_buf();
    #[cfg(target_os = "windows")]
    {
        settings.app.tray = scenario.tray;
    }
    settings.updater.enabled = false;
    settings.view.mode = if scenario.large_icons {
        crate::settings::FileViewMode::LargeIcons
    } else {
        crate::settings::FileViewMode::Details
    };
    settings.view.mode_media = settings.view.mode;
    settings.view.show_dotfiles = false;
    settings.view.show_hidden = false;
    fs::write(
        config.join("settings.json"),
        serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
struct Worker {
    child: ChildGuard,
    stream: TcpStream,
    events: Receiver<(Instant, Event)>,
    token: String,
    launched: Instant,
}
impl Worker {
    fn start(
        executable: &Path,
        root: &Path,
        config: &Path,
        scenario: &Scenario,
        path: &Path,
        diagnostic: bool,
    ) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(|e| e.to_string())?;
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let token = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let request = Request {
            version: 1,
            endpoint: listener.local_addr().map_err(|e| e.to_string())?,
            token: token.clone(),
            config: config.into(),
            path: path.into(),
            entries: scenario.entries,
        };
        let request_path = root.join("request.json");
        fs::write(
            &request_path,
            serde_json::to_vec(&request).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let mut command = Command::new(executable);
        command
            .env(REQUEST_ENV, request_path)
            .env(super::CONFIG_ENV, config)
            .env_remove("EXPLORER_STARTUP_TRACE")
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                fs::File::create(root.join("stderr.log")).map_err(|e| e.to_string())?,
            ));
        if diagnostic {
            command.env("EXPLORER_STARTUP_TRACE", root.join("trace.json"));
        }
        let launched = Instant::now();
        let mut child = ChildGuard(command.spawn().map_err(|e| e.to_string())?);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.to_string()),
            }
            if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())? {
                return Err(format!("worker exited before connecting: {status}"));
            }
            if launched.elapsed() >= TIMEOUT {
                return Err(
                    "worker connection timeout (requires a benchmarks-enabled Explorer executable)"
                        .into(),
                );
            }
            thread::sleep(Duration::from_millis(2));
        };
        // Accepted sockets inherit nonblocking mode on Windows. The reader below
        // uses blocking lines(), otherwise WouldBlock looks like a disconnected worker.
        stream.set_nonblocking(false).map_err(|e| e.to_string())?;
        stream.set_nodelay(true).map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(|e| e.to_string())?;
        let reader = stream.try_clone().map_err(|e| e.to_string())?;
        let (tx, events) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(reader).lines().map_while(Result::ok) {
                let received = Instant::now();
                let Ok(event) = serde_json::from_str(&line) else {
                    break;
                };
                if tx.send((received, event)).is_err() {
                    break;
                }
            }
        });
        let worker = Self {
            child,
            stream,
            events,
            token,
            launched,
        };
        worker.receive("connected", 0)?;
        Ok(worker)
    }
    fn receive(&self, name: &str, cycle: u64) -> Result<(Instant, Event), String> {
        let (at, event) = self
            .events
            .recv_timeout(TIMEOUT)
            .map_err(|e| format!("waiting for {name}/{cycle}: {e}"))?;
        if event.token != self.token || event.name != name || event.cycle != cycle {
            return Err(format!(
                "expected {name}/{cycle}, got {}/{}, token matches: {}",
                event.name,
                event.cycle,
                event.token == self.token
            ));
        }
        Ok((at, event))
    }
    fn command(&mut self, command: &str, ack: &str, cycle: u64) -> Result<(), String> {
        writeln!(self.stream, "{command}").map_err(|e| e.to_string())?;
        self.receive(ack, cycle)?;
        Ok(())
    }
    fn milestones(&self, origin: Instant, cycle: u64) -> Result<Milestones, String> {
        let (first, _) = self.receive("first_frame", cycle)?;
        let (folder, event) = self.receive("folder_frame", cycle)?;
        let (input, _) = self.receive("input_frame", cycle)?;
        Ok(Milestones {
            first_frame_ms: first.duration_since(origin).as_secs_f64() * 1_000.,
            folder_frame_ms: folder.duration_since(origin).as_secs_f64() * 1_000.,
            input_frame_ms: input.duration_since(origin).as_secs_f64() * 1_000.,
            entry_to_folder_ms: event.entry_ms,
            scale_factor: event.scale_factor,
            viewport: event.viewport,
        })
    }
    fn quit(&mut self) -> Result<(), String> {
        writeln!(self.stream, "quit").map_err(|e| e.to_string())?;
        wait_exit(&mut self.child.0, Duration::from_secs(3))
    }
}
fn wait_exit(child: &mut Child, timeout: Duration) -> Result<(), String> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return if status.success() {
                Ok(())
            } else {
                Err(format!("child exit: {status}"))
            };
        }
        if start.elapsed() >= timeout {
            return Err("child exit timeout".into());
        }
        thread::sleep(Duration::from_millis(2));
    }
}
fn relaunch(
    worker: &mut Worker,
    executable: &Path,
    config: &Path,
    cycle: u64,
) -> Result<Milestones, String> {
    worker.command("arm", "armed", cycle - 1)?;
    let origin = Instant::now();
    let mut secondary = ChildGuard(
        Command::new(executable)
            .env(super::CONFIG_ENV, config)
            .env_remove(REQUEST_ENV)
            .env_remove("EXPLORER_STARTUP_TRACE")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?,
    );
    let result = worker.milestones(origin, cycle)?;
    wait_exit(&mut secondary.0, Duration::from_secs(3))?;
    Ok(result)
}
fn run_sample(
    executable: &Path,
    scenario: &Scenario,
    fixtures: &Fixtures,
    root: &Path,
    repetition: usize,
    diagnostic: bool,
) -> Result<Sample, String> {
    let config = root.join("config");
    let path = fixtures.mixed(scenario.entries);
    write_settings(&config, scenario, &path)?;
    if scenario.primed {
        let mut worker = Worker::start(
            executable,
            &root.join("prime"),
            &config,
            scenario,
            &path,
            false,
        )?;
        worker.milestones(worker.launched, 1)?;
        thread::sleep(Duration::from_millis(250));
        worker.quit()?;
    }
    let mut worker = Worker::start(
        executable,
        &root.join("measured"),
        &config,
        scenario,
        &path,
        diagnostic,
    )?;
    let fresh = worker.milestones(worker.launched, 1)?;
    thread::sleep(Duration::from_millis(250));
    let resident_window = relaunch(&mut worker, executable, &config, 2)?;
    let resident_closed = if scenario.tray {
        worker.command("close", "closed", 2)?;
        Some(relaunch(&mut worker, executable, &config, 3)?)
    } else {
        None
    };
    worker.quit()?;
    Ok(Sample {
        repetition,
        first_sample: repetition == 0,
        fresh,
        resident_window,
        resident_closed,
    })
}

fn save(report: &Report, root: &Path) -> Result<(), String> {
    fs::write(
        root.join("startup.json"),
        serde_json::to_vec_pretty(report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let mut text = format!(
        "# Real-entry-point startup v1\n\nDiagnostic: **{}**. Times are launch to renderer-submission notification receipt, including loopback transport; they exclude physical display latency. Keyboard probes dispatch End/Home through GPUI and wait for a matching submitted selection. Empty folders verify an empty selection.\n\nEach sample uses an isolated config and normal single-instance routing. Primed caches receive a separate preparation process and 250 ms settling time. Fresh means a new process, not cold OS caches. First samples remain in the raw report and in distributions. Entry-to-folder is process-local and is not a relaunch duration.\n\n| Case | Build | Flow | n | First sample ms | First frame median ms | Folder median ms | Folder p95 ms | Input median ms | Failures |\n|---|---|---|---:|---:|---:|---:|---:|---:|---:|\n",
        report.diagnostic
    );
    for case in &report.cases {
        for flow in ["fresh", "resident_window", "resident_closed"] {
            let samples = case
                .samples
                .iter()
                .filter_map(|s| match flow {
                    "fresh" => Some(&s.fresh),
                    "resident_window" => Some(&s.resident_window),
                    _ => s.resident_closed.as_ref(),
                })
                .collect::<Vec<_>>();
            if flow == "resident_closed" && !case.scenario.tray {
                continue;
            }
            let first = samples.first().map(|s| s.folder_frame_ms);
            let frames = distribution(samples.iter().map(|s| s.first_frame_ms));
            let folder = distribution(samples.iter().map(|s| s.folder_frame_ms));
            let input = distribution(samples.iter().map(|s| s.input_frame_ms));
            text.push_str(&format!(
                "| {} | {} | {flow} | {} | {} | {} | {} | {} | {} | {} |\n",
                case.scenario.id,
                case.variant,
                folder.count,
                number(first),
                number(frames.median_ms),
                number(folder.median_ms),
                number(folder.p95_ms),
                number(input.median_ms),
                case.failures.len()
            ));
        }
    }
    text.push_str("\n## Alternating executable comparison\n\nPositive reductions mean faster. A p95 reduction below -5% needs a repeat run to distinguish a regression from noise.\n\n| Case | Flow | Before median ms | After median ms | Median reduction | Before p95 ms | After p95 ms | p95 reduction |\n|---|---|---:|---:|---:|---:|---:|---:|\n");
    for before in report.cases.iter().filter(|c| c.variant == "before") {
        let Some(after) = report
            .cases
            .iter()
            .find(|c| c.variant == "after" && c.scenario.id == before.scenario.id)
        else {
            continue;
        };
        for flow in ["fresh", "resident_window", "resident_closed"] {
            if flow == "resident_closed" && !before.scenario.tray {
                continue;
            }
            let metric = |case: &Case| {
                distribution(case.samples.iter().filter_map(|s| match flow {
                    "fresh" => Some(s.fresh.folder_frame_ms),
                    "resident_window" => Some(s.resident_window.folder_frame_ms),
                    _ => s.resident_closed.as_ref().map(|s| s.folder_frame_ms),
                }))
            };
            let b = metric(before);
            let a = metric(after);
            let reduction = |b: Option<f64>, a: Option<f64>| {
                b.zip(a)
                    .filter(|(b, _)| *b > 0.)
                    .map(|(b, a)| format!("{:.1}%", (b - a) / b * 100.))
                    .unwrap_or_else(|| "—".into())
            };
            text.push_str(&format!(
                "| {} | {flow} | {} | {} | {} | {} | {} | {} |\n",
                before.scenario.id,
                number(b.median_ms),
                number(a.median_ms),
                reduction(b.median_ms, a.median_ms),
                number(b.p95_ms),
                number(a.p95_ms),
                reduction(b.p95_ms, a.p95_ms)
            ));
        }
    }
    text.push_str("\n## Metadata\n\n");
    for (key, value) in &report.metadata {
        text.push_str(&format!("- {key}: `{}`\n", value.replace('\n', " ")));
    }
    for case in &report.cases {
        for error in &case.failures {
            text.push_str(&format!(
                "\nFailure {} {}: {error}\n",
                case.scenario.id, case.variant
            ));
        }
    }
    fs::write(root.join("report.md"), text).map_err(|e| e.to_string())
}
fn number(value: Option<f64>) -> String {
    value
        .map(|v| format!("{v:.2}"))
        .unwrap_or_else(|| "—".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_scenarios_are_distinct_and_cover_cache_and_tray() {
        let cases = scenarios();
        let ids = cases
            .iter()
            .map(|s| &s.id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), cases.len());
        assert_eq!(cases.len(), if cfg!(windows) { 24 } else { 12 });
        assert!(cases.iter().any(|s| s.entries == 0 && s.primed));
        assert!(cases.iter().any(|s| s.entries == 10_000 && s.large_icons));
    }
    #[test]
    fn invalid_repetitions_and_unknown_flags_are_rejected() {
        for args in [
            vec!["--repetitions", "0"],
            vec!["--repetitions", "1001"],
            vec!["--repetitions"],
            vec!["--unexpected", "1"],
        ] {
            assert!(options(&args.into_iter().map(String::from).collect::<Vec<_>>()).is_err());
        }
    }
    #[test]
    fn requests_must_be_local_and_versioned() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("settings.json"), "{}").unwrap();
        let mut request = Request {
            version: 1,
            endpoint: "127.0.0.1:1234".parse().unwrap(),
            token: "test".into(),
            config: temp.path().into(),
            path: temp.path().into(),
            entries: 0,
        };
        assert!(request.validate().is_ok());
        request.endpoint = "192.0.2.1:1234".parse().unwrap();
        assert!(request.validate().is_err());
        request.endpoint = "127.0.0.1:1234".parse().unwrap();
        request.version = 2;
        assert!(request.validate().is_err());
    }
}
