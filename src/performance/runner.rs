use super::{
    Scenario,
    fixtures::{FIXTURE_VERSION, Fixtures},
    report::{self, Case, Report, WorkerResult},
    ui::WorkerRequest,
};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SUITES: &[&str] = &[
    "navigation_pipeline",
    "recursive_search",
    "archive_extraction",
    "image_thumbnails",
    "video_thumbnails",
    "image_viewer",
    "properties",
    "resumable_copy",
];
const CHILD_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerTimeout {
    Startup,
    Process,
}

fn worker_timeout(
    started: Instant,
    now: Instant,
    ready: Option<Instant>,
    startup: bool,
) -> Option<WorkerTimeout> {
    if now.duration_since(started) >= CHILD_TIMEOUT {
        Some(WorkerTimeout::Process)
    } else if startup && ready.unwrap_or(now).duration_since(started) >= super::ui::ACTION_TIMEOUT {
        Some(WorkerTimeout::Startup)
    } else {
        None
    }
}
const HELP: &str = "explorer-bench list [--preset quick|full] [--filter ID]\nexplorer-bench run [--preset quick|full] [--filter ID] [--output DIRECTORY]\nexplorer-bench compare BEFORE AFTER [--allow-incompatible]\n\nBuild/run with: cargo run --locked --release --features benchmarks --bin explorer-bench -- COMMAND\nRun creates a new timestamped directory beneath --output (default target/performance).";

pub fn main() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() || args[0] == "--help" || args[0] == "help" {
        println!("{HELP}");
        return Ok(());
    }
    match args[0].as_str() {
        "worker" => {
            if args.len() != 2 {
                return Err("internal worker expects a request file".into());
            }
            let request: WorkerRequest =
                serde_json::from_slice(&fs::read(&args[1]).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            super::ui::run(request)
        }
        "list" | "run" => {
            let options = options(&args[1..])?;
            let full = options.preset == "full";
            let scenarios = super::scenarios(full)
                .into_iter()
                .filter(|s| options.matches(&s.id))
                .collect::<Vec<_>>();
            let suites = SUITES
                .iter()
                .copied()
                .filter(|suite| {
                    (full || *suite == "navigation_pipeline")
                        && options.matches(&format!("criterion/{suite}"))
                })
                .collect::<Vec<_>>();
            if args[0] == "list" {
                for s in scenarios {
                    println!(
                        "{}{}",
                        s.id,
                        if s.video { " [FFmpeg + FFprobe]" } else { "" }
                    );
                }
                for s in suites {
                    println!("criterion/{s}");
                }
                return Ok(());
            }
            if scenarios.is_empty() && suites.is_empty() {
                return Err("filter matches no scenarios in the selected preset".into());
            }
            run(options, scenarios, suites)
        }
        "compare" => {
            if !(args.len() == 3 || args.len() == 4 && args[3] == "--allow-incompatible") {
                return Err(HELP.into());
            }
            let before = report::load(Path::new(&args[1]))?;
            let after = report::load(Path::new(&args[2]))?;
            println!("{}", report::compare(&before, &after, args.len() == 4)?);
            Ok(())
        }
        _ => Err(HELP.into()),
    }
}

struct Options {
    preset: String,
    filter: String,
    output: PathBuf,
}
impl Options {
    fn matches(&self, id: &str) -> bool {
        id.contains(&self.filter)
    }
}
fn options(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        preset: "quick".into(),
        filter: String::new(),
        output: target_dir().join("performance"),
    };
    let mut args = args.iter();
    while let Some(key) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {key}"))?;
        match key.as_str() {
            "--preset" if value == "quick" || value == "full" => options.preset = value.clone(),
            "--filter" => options.filter = value.clone(),
            "--output" => options.output = value.into(),
            _ => {
                return Err(format!(
                    "unknown option or invalid value: {key} {value}\n{HELP}"
                ));
            }
        }
    }
    Ok(options)
}

pub(super) fn target_dir() -> PathBuf {
    let path = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target"));
    if path.is_absolute() {
        path
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
    }
}

fn command_text(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "unavailable".into())
}

fn metadata() -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (key, value) in [
        ("commit", command_text("git", &["rev-parse", "HEAD"])),
        (
            "dirty_status",
            command_text("git", &["status", "--porcelain"]),
        ),
        ("rustc", command_text("rustc", &["--version"])),
        ("os", format!("{} {}", std::env::consts::OS, os_version())),
        ("arch", std::env::consts::ARCH.into()),
        ("cpu", cpu()),
        ("profile", "release".into()),
        (
            "ffmpeg",
            command_text("ffmpeg", &["-version"])
                .lines()
                .next()
                .unwrap_or("unavailable")
                .into(),
        ),
        (
            "ffprobe",
            command_text("ffprobe", &["-version"])
                .lines()
                .next()
                .unwrap_or("unavailable")
                .into(),
        ),
        (
            "refresh_rate",
            "unavailable (GPUI has no portable refresh-rate query)".into(),
        ),
    ] {
        map.insert(key.into(), value);
    }
    map
}

fn cpu() -> String {
    #[cfg(target_os = "windows")]
    {
        std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "unavailable".into())
    }
    #[cfg(target_os = "macos")]
    {
        command_text("sysctl", &["-n", "machdep.cpu.brand_string"])
    }
    #[cfg(target_os = "linux")]
    {
        fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|s| {
                s.lines().find_map(|line| {
                    line.strip_prefix("model name")
                        .and_then(|s| s.split_once(':'))
                        .map(|(_, name)| name.trim().to_owned())
                })
            })
            .unwrap_or_else(|| "unavailable".into())
    }
}
fn os_version() -> String {
    #[cfg(target_os = "windows")]
    {
        command_text("cmd", &["/c", "ver"])
    }
    #[cfg(target_os = "macos")]
    {
        command_text("sw_vers", &["-productVersion"])
    }
    #[cfg(target_os = "linux")]
    {
        command_text("uname", &["-r"])
    }
}

fn run(options: Options, scenarios: Vec<Scenario>, suites: Vec<&str>) -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Err("UI timing runs require --release; use list/compare with any profile".into());
    }
    let full = options.preset == "full";
    fs::create_dir_all(&options.output).map_err(|e| e.to_string())?;
    let root = options.output.join(format!(
        "run-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos(),
        std::process::id()
    ));
    fs::create_dir(&root).map_err(|e| e.to_string())?;
    let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let mut report = Report {
        version: report::REPORT_VERSION,
        scenario_version: super::SCENARIO_VERSION,
        fixture_version: FIXTURE_VERSION.into(),
        preset: options.preset,
        metadata: metadata(),
        cases: BTreeMap::new(),
        incomplete_coverage: false,
    };
    println!("Results: {}", root.display());
    let video =
        report.metadata["ffmpeg"] != "unavailable" && report.metadata["ffprobe"] != "unavailable";
    report.save(&root)?;
    let fixtures = match Fixtures::ensure(
        &target_dir(),
        video && (scenarios.iter().any(|s| s.video) || suites.contains(&"video_thumbnails")),
    ) {
        Ok(fixtures) => fixtures,
        Err(error) => {
            report.cases.insert(
                "setup/fixtures".into(),
                Case {
                    errors: vec![error.clone()],
                    ..Default::default()
                },
            );
            report.save(&root)?;
            return Err(error);
        }
    };
    let (warmups, measured) = if full { (3, 30) } else { (1, 5) };
    for (index, scenario) in scenarios.iter().enumerate() {
        println!("[{}/{}] {}", index + 1, scenarios.len(), scenario.id);
        let case_root = root
            .join("ui")
            .join(scenario.id.strip_prefix("ui/").unwrap());
        fs::create_dir_all(&case_root).map_err(|e| e.to_string())?;
        let mut case = Case::default();
        if scenario.video && !video {
            case.skipped = Some("FFmpeg and FFprobe must both be available on PATH".into());
        } else {
            for sample in 0..warmups + measured {
                let sample_root = case_root.join(format!("sample-{sample:02}"));
                fs::create_dir(&sample_root).map_err(|e| e.to_string())?;
                let request = WorkerRequest {
                    scenario: scenario.clone(),
                    fixture_root: fixtures.root.clone(),
                    output: sample_root.clone(),
                    scroll_seconds: if full { 5. } else { 2. },
                };
                let (result, startup_ms) = match worker(&request) {
                    Ok(result) => result,
                    Err(error) => {
                        case.errors.push(format!("iteration {sample}: {error}"));
                        break;
                    }
                };
                let raw = serde_json::json!({"scenario": scenario.id, "iteration": sample, "warmup": sample < warmups,
                    "startup_launch_to_notification_ms": startup_ms, "result": result});
                let mut file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(root.join("samples.jsonl"))
                    .map_err(|e| e.to_string())?;
                writeln!(file, "{raw}").map_err(|e| e.to_string())?;
                for (key, value) in [
                    ("display_backend", Some(result.display_backend.clone())),
                    ("scale_factor", result.scale_factor.map(|v| v.to_string())),
                    (
                        "viewport",
                        result.viewport.map(|v| format!("{}x{}", v[0], v[1])),
                    ),
                ] {
                    if let Some(value) = value {
                        if let Some(old) = report.metadata.get(key)
                            && old != &value
                        {
                            case.errors.push(format!(
                                "environment changed within run: {key} ({old} -> {value})"
                            ));
                        } else {
                            report.metadata.insert(key.into(), value);
                        }
                    }
                }
                if let Some(error) = result.error {
                    case.errors.push(format!("iteration {sample}: {error}"));
                    break;
                }
                if sample >= warmups {
                    if let Some(mut result) = result.sample {
                        if scenario.flow == "startup" {
                            let Some(elapsed) = startup_ms else {
                                case.errors
                                    .push("worker omitted startup notification".into());
                                break;
                            };
                            result.action_to_submission_ms = elapsed;
                        }
                        case.samples.push(result);
                    } else {
                        case.errors.push("worker produced no sample".into());
                        break;
                    }
                }
            }
        }
        report.cases.insert(scenario.id.clone(), case);
        report.save(&root)?;
    }
    for suite in suites {
        println!("Criterion: {suite}");
        let suite_root = root.join("criterion").join(suite);
        fs::create_dir_all(&suite_root).map_err(|e| e.to_string())?;
        let mut case = Case::default();
        if suite == "video_thumbnails" && !video {
            case.skipped = Some("FFmpeg and FFprobe must both be available on PATH".into());
        } else {
            let isolated = suite_root.join("config");
            fs::create_dir(&isolated).map_err(|e| e.to_string())?;
            let status = run_criterion(suite, &suite_root, &isolated, full);
            match &status {
                Ok(status) if status.success() => {}
                Ok(status) => case
                    .errors
                    .push(format!("Criterion failed: {status}; inspect stderr.log")),
                Err(error) => case.errors.push(error.clone()),
            }
            let found = collect_criterion(&suite_root, &suite_root, &mut report.cases)?;
            if status.is_ok_and(|status| status.success()) && found == 0 {
                case.errors.push("Criterion produced no estimates".into());
            }
        }
        report.cases.insert(format!("criterion/{suite}"), case);
        report.save(&root)?;
    }
    if report.cases.values().any(|case| !case.errors.is_empty()) {
        Err(format!(
            "run failed; partial results saved to {}",
            root.display()
        ))
    } else {
        println!("Report: {}", root.join("report.md").display());
        Ok(())
    }
}

fn benchmark_artifact(line: &str, suite: &str) -> Option<PathBuf> {
    let message: serde_json::Value = serde_json::from_str(line).ok()?;
    if message["reason"] != "compiler-artifact"
        || message["target"]["name"] != suite
        || !message["target"]["kind"]
            .as_array()?
            .iter()
            .any(|kind| kind == "bench")
    {
        return None;
    }
    message["executable"].as_str().map(PathBuf::from)
}

fn run_criterion(
    suite: &str,
    root: &Path,
    isolated: &Path,
    full: bool,
) -> Result<std::process::ExitStatus, String> {
    // Cargo builds binary targets as dependencies of benchmark targets, even
    // with `build --bench`. Keep those artifacts away from the running worker
    // executable: Windows forbids overwriting an executing program.
    let mut build_root = target_dir().join("performance-criterion-build");
    if let Ok(canonical) = fs::canonicalize(&build_root)
        && fs::canonicalize(std::env::current_exe().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?
            .starts_with(canonical)
    {
        build_root = build_root.join("nested");
    }
    let artifacts = root.join("build-artifacts.jsonl");
    let status = Command::new("cargo")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args([
            "build",
            "--locked",
            "--profile",
            "release",
            "--features",
            "benchmarks",
            "--bench",
            suite,
            "--message-format=json",
        ])
        .arg("--target-dir")
        .arg(build_root)
        .stdout(Stdio::from(
            fs::File::create(&artifacts).map_err(|e| e.to_string())?,
        ))
        .stderr(Stdio::from(
            fs::File::create(root.join("build-stderr.log")).map_err(|e| e.to_string())?,
        ))
        .status()
        .map_err(|e| format!("could not build Criterion target: {e}"))?;
    if !status.success() {
        return Err(format!(
            "Criterion build failed: {status}; inspect build-stderr.log and build-artifacts.jsonl"
        ));
    }
    let executable = BufReader::new(fs::File::open(artifacts).map_err(|e| e.to_string())?)
        .lines()
        .map_while(Result::ok)
        .find_map(|line| benchmark_artifact(&line, suite))
        .ok_or("Cargo omitted the Criterion benchmark executable")?;
    let mut command = Command::new(executable);
    command
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .arg("--bench")
        .env(super::CONFIG_ENV, isolated)
        .env("CRITERION_HOME", root);
    if !full {
        command.args([
            "documents_small|mixed_1000_hidden_",
            "--sample-size",
            "10",
            "--warm-up-time",
            "1",
            "--measurement-time",
            "2",
        ]);
    }
    command
        .stdout(Stdio::from(
            fs::File::create(root.join("stdout.log")).map_err(|e| e.to_string())?,
        ))
        .stderr(Stdio::from(
            fs::File::create(root.join("stderr.log")).map_err(|e| e.to_string())?,
        ))
        .status()
        .map_err(|e| format!("could not start Criterion: {e}"))
}

fn collect_criterion(
    root: &Path,
    path: &Path,
    cases: &mut BTreeMap<String, Case>,
) -> Result<usize, String> {
    let mut found = 0;
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            found += collect_criterion(root, &entry.path(), cases)?;
        } else if entry.file_name() == "estimates.json"
            && path.file_name().is_some_and(|n| n == "new")
        {
            let id = path
                .parent()
                .unwrap()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let estimates =
                serde_json::from_slice(&fs::read(entry.path()).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            // Criterion's benchmark.json preserves the original (unsanitized) identifier.
            let benchmark: serde_json::Value =
                serde_json::from_slice(&fs::read(path.join("benchmark.json")).unwrap_or_default())
                    .unwrap_or_default();
            let id = benchmark
                .get("full_id")
                .and_then(|v| v.as_str())
                .unwrap_or(&id);
            cases.insert(
                format!("criterion/{id}"),
                Case {
                    criterion_estimates: Some(estimates),
                    criterion_sample_count: fs::read(path.join("sample.json"))
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                        .and_then(|sample| {
                            sample
                                .get("times")
                                .and_then(|v| v.as_array())
                                .map(|v| v.len())
                        }),
                    ..Default::default()
                },
            );
            found += 1;
        }
    }
    Ok(found)
}

fn worker(request: &WorkerRequest) -> Result<(WorkerResult, Option<f64>), String> {
    let request_path = request.output.join("request.json");
    fs::write(
        &request_path,
        serde_json::to_vec(request).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let config = request.output.join("config");
    fs::create_dir(&config).map_err(|e| e.to_string())?;
    super::ui::prepare(request, &config)?;
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
        .arg("worker")
        .arg(&request_path)
        .env(super::CONFIG_ENV, &config)
        .stdout(Stdio::piped())
        .stderr(Stdio::from(
            fs::File::create(request.output.join("stderr.log")).map_err(|e| e.to_string())?,
        ));
    let started = Instant::now();
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    let stdout_path = request.output.join("stdout.log");
    let reader = thread::spawn(move || {
        let mut output = fs::File::create(stdout_path).ok();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line == super::ui::READY_LINE {
                let _ = sender.send(Instant::now());
            }
            if let Some(file) = &mut output {
                let _ = writeln!(file, "{line}");
            }
        }
    });
    let mut timeout = None;
    let mut ready_at = None;
    let startup = request.scenario.flow == "startup";
    let status = loop {
        if ready_at.is_none() {
            ready_at = receiver.try_recv().ok();
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if let Some(reason) = worker_timeout(started, Instant::now(), ready_at, startup) {
            timeout = Some(reason);
            let _ = child.kill();
            break child.wait().map_err(|e| e.to_string())?;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let completed_at = Instant::now();
    if timeout.is_none() {
        let _ = reader.join();
    }
    ready_at = ready_at.or_else(|| receiver.try_iter().next());
    timeout = timeout.or_else(|| worker_timeout(started, completed_at, ready_at, startup));
    let ready = ready_at.map(|at| at.duration_since(started).as_secs_f64() * 1_000.);
    let mut result: WorkerResult = fs::read(request.output.join("result.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if let Some(reason) = timeout {
        result.error = Some(match reason {
            WorkerTimeout::Startup => {
                "startup action timeout (30 seconds from process launch)".into()
            }
            WorkerTimeout::Process => "child-process timeout (five minutes)".into(),
        });
    } else if !status.success() {
        result
            .error
            .get_or_insert_with(|| format!("worker exited with {status}; inspect stderr.log"));
    }
    if result.error.is_none() && result.sample.is_none() {
        result.error = Some("worker exited without a result; inspect stderr.log".into());
    }
    Ok((result, ready))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parent_deadlines_include_startup_initialization_and_late_notifications() {
        let start = Instant::now();
        let limit = super::super::ui::ACTION_TIMEOUT;
        assert_eq!(
            worker_timeout(start, start + limit - Duration::from_nanos(1), None, true),
            None
        );
        assert_eq!(
            worker_timeout(start, start + limit, None, true),
            Some(WorkerTimeout::Startup)
        );
        assert_eq!(
            worker_timeout(
                start,
                start + limit,
                Some(start + Duration::from_secs(1)),
                true
            ),
            None
        );
        assert_eq!(
            worker_timeout(start, start + limit, Some(start + limit), true),
            Some(WorkerTimeout::Startup)
        );
        assert_eq!(worker_timeout(start, start + limit, None, false), None);
        assert_eq!(
            worker_timeout(
                start,
                start + CHILD_TIMEOUT - Duration::from_nanos(1),
                None,
                false
            ),
            None
        );
        assert_eq!(
            worker_timeout(start, start + CHILD_TIMEOUT, Some(start), true),
            Some(WorkerTimeout::Process)
        );
    }
    #[test]
    fn cargo_artifact_parser_selects_only_the_requested_benchmark() {
        let artifact = serde_json::json!({
            "reason": "compiler-artifact", "target": {"kind": ["bench"], "name": "navigation_pipeline"},
            "executable": "/target/release/deps/navigation_pipeline"
        });
        assert_eq!(
            benchmark_artifact(&artifact.to_string(), "navigation_pipeline"),
            Some(PathBuf::from("/target/release/deps/navigation_pipeline"))
        );
        assert!(benchmark_artifact(&artifact.to_string(), "image_viewer").is_none());
        let mut binary = artifact;
        binary["target"]["kind"] = serde_json::json!(["bin"]);
        assert!(benchmark_artifact(&binary.to_string(), "navigation_pipeline").is_none());
        assert!(benchmark_artifact("not JSON", "navigation_pipeline").is_none());
    }
    #[test]
    fn cli_rejects_unknown_options_and_invalid_presets() {
        assert!(options(&["--preset".into(), "fast".into()]).is_err());
        assert!(options(&["--filter".into()]).is_err());
        let options = options(&[
            "--preset".into(),
            "full".into(),
            "--filter".into(),
            "ui/open".into(),
        ])
        .unwrap();
        assert_eq!(options.preset, "full");
        assert!(options.matches("ui/open/details/1000"));
    }
}
