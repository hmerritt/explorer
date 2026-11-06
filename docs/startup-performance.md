# Startup performance

The `startup` suite measures the real Explorer executable, through installer argument
handling, single-instance ownership/routing, GPUI setup, application initialization,
tray setup, and normal window creation. The older `ui/startup/...` cases remain
unchanged and use a different application entry point. Their results are not
interchangeable.

## Reproduce on Windows

Build both binaries with the same profile and feature set:

```powershell
cargo build --locked --release --features benchmarks --bins
target/release/explorer-bench.exe startup --filter details/1000/tray/empty
```

The default is 30 fresh-process samples per case. Without `--filter`, the suite
covers 0, 1,000, and 10,000 entries, Details and Large Icons, tray enabled/disabled,
and empty/primed application caches. The existing fixture generator is reused.
On macOS and Linux, tray cases are omitted.

Scenario settings disable the updater, hide hidden files and dotfiles, use the
fixture as the start folder, and set the media-folder view to the selected view.
The default single-instance behavior focuses an existing window. The complete
generated `config/settings.json` remains beside each sample's worker output.
Device monitoring and clipboard polling use the normal startup behavior.

Each sample also launches a secondary process to focus the existing window using
normal single-instance routing. With the Windows tray enabled it closes all
windows, verifies the primary remains resident, and launches another secondary
process to open a new window. Child processes are stopped on failures and timeouts.
The suite uses isolated settings, caches, window state, and single-instance files
under its output directory; personal settings and caches are not modified.
Normal builds reject startup-worker requests before reading settings or routing
to an existing application instance. Use a build with `--features benchmarks`.

To compare an instrumented baseline and a changed executable:

```powershell
# Before making optimizations, preserve the benchmarks-enabled executable.
New-Item -ItemType Directory -Force target/startup-baseline
Copy-Item target/release/explorer.exe target/startup-baseline/explorer.exe

# After rebuilding the changed code:
target/release/explorer-bench.exe startup --baseline target/startup-baseline/explorer.exe --filter details/1000/tray/empty
```

`--baseline` alternates executable order on successive repetitions. Both variants
get independent isolated settings and equivalent cache preparation. Use the same
machine, monitor arrangement, power mode, and background workload. Do not compare
a debug build with a release build. `--executable PATH`, `--output DIRECTORY`, and
`--repetitions N` override the defaults.

## Metrics and diagnostics

`startup.json` contains versioned raw samples, first-sample flags, failures,
scenario settings, commit/build metadata, executable SHA-256 hashes, viewport,
and scale factor. `report.md` contains medians and p95s for each launch flow.

- **First frame:** a scene submitted for the initial application window.
- **Folder frame:** a submitted scene containing the expected loaded directory.
- **Input frame:** a submitted scene after an End/Home keystroke passed through
  GPUI key dispatch and produced the expected selection. End/Home alternate
  between launches so existing windows must change selection. Empty folders
  verify that dispatch leaves selection empty.

Headline timings run from immediately before process spawn to receipt of the
milestone's loopback notification. They include executable loading, initialization,
renderer submission, scheduling, and notification transport. They exclude OS
keyboard delivery, GPU completion, compositor presentation, and physical display
latency. The input check includes a small harness scheduling delay; the folder
frame is the primary startup metric. Process-local entry-to-folder timing is
also recorded, and must not be interpreted as resident relaunch latency.

Fresh-process startup does not imply cold OS file caches. First samples are kept
in both the raw results and aggregate distributions. Primed application caches
use a separate preparation process with 250 ms settling time; this does not
promise every optional thumbnail or background job has completed.

Run diagnostics separately from headline measurements:

```powershell
target/release/explorer-bench.exe startup --filter details/1000/tray/empty --diagnostic --repetitions 5
```

Diagnostic samples write buffered `measured/trace.json` stage timings. Nested and
background spans can overlap, so their durations must not be added together.
Compare diagnostic and normal runs to assess observer overhead. The opt-in
`EXPLORER_STARTUP_TRACE` environment variable also accepts an output file path
when launching the application directly. Stage traces are flushed after initial
window creation and on graceful shutdown; renderer milestones require the
benchmarks-enabled startup suite.

For an alternating overhead comparison, run six pairs of five-sample batches
with and without `--diagnostic`, reversing their order on each pair. Combined,
this gives 30 samples per tracing mode per executable. Keep those reports
separate from the headline run; `diagnostic` in each `startup.json` identifies
the mode. Compare paired batches as well as the combined medians so normal
timing variation is visible.

If native graphics or loader work dominates, record a separate Windows Performance
Recorder trace and inspect it in Windows Performance Analyzer; profiling runs do
not belong in headline timing distributions.
