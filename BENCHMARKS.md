# Benchmarks

## Local application benchmarks

The `explorer-bench` runner measures real GPUI windows alongside the existing
Criterion suites. Build in the release profile; the first build can take several
minutes. A graphical desktop is required for UI cases.

Criterion uses a separate, reusable build cache under
`target/performance-criterion-build` (beneath `CARGO_TARGET_DIR` when set).
Its first build can take additional time. Cargo also builds package binaries
alongside benchmark targets ([Cargo target selection](https://doc.rust-lang.org/cargo/commands/cargo-build.html#target-selection));
separate artifacts avoid overwriting the executing runner on Windows.

```sh
cargo run --locked --release --features benchmarks --bin explorer-bench -- list
cargo run --locked --release --features benchmarks --bin explorer-bench -- run
cargo run --locked --release --features benchmarks --bin explorer-bench -- run --preset full
```

`run` defaults to the **quick** preset: 1,000-entry browsing in Details and Large
Icons, representative search and media flows, one warm-up and five measured
repetitions, two-second scrolling workloads, and representative directory-loading
Criterion cases. Its tail estimates are exploratory. **Full** adds empty, 100,
and 10,000-entry folders, explicitly primed media cases, three warm-ups and 30
measured repetitions, five-second scrolling workloads, and all eight Criterion
suites. Full runs can be lengthy; every UI repetition starts a fresh process.

Use a literal substring of a stable scenario ID to select cases. `list` accepts
the same preset and filter options. Full-only cases require `--preset full`.

```sh
cargo run --locked --release --features benchmarks --bin explorer-bench -- list --preset full
cargo run --locked --release --features benchmarks --bin explorer-bench -- run --filter ui/open
cargo run --locked --release --features benchmarks --bin explorer-bench -- run --filter ui/hover_image
cargo run --locked --release --features benchmarks --bin explorer-bench -- run --filter criterion/navigation_pipeline
cargo run --locked --release --features benchmarks --bin explorer-bench -- run --output target/performance-before
```

Each run creates a fresh `run-<timestamp>-<pid>` directory under
`target/performance` (or `--output`). It contains `summary.json`, `report.md`, raw
`samples.jsonl`, per-worker requests/results/logs, and original Criterion
artifacts. Reports are saved after each scenario, including failures and explicit
skips. Workers time out after five minutes; each action times out after 30 seconds.
Timing regressions do not fail runs. Incorrect outcomes and execution errors do.
Missing FFmpeg or FFprobe skips video cases and marks coverage incomplete.

Compare the **actual run directories** printed by two runs:

```sh
cargo run --locked --release --features benchmarks --bin explorer-bench -- compare target/performance/run-BEFORE target/performance/run-AFTER
```

Comparison prints Markdown with absolute and percentage changes for matching
scenarios, and flags missing or incomplete cases. OS, architecture, CPU, build
profile, Rust version, display backend, scale factor, viewport, media tools,
fixture/scenario versions, and sampling preset must match. An intentionally
different environment requires `--allow-incompatible`. Commit and dirty state
are recorded but may differ: comparing revisions is the purpose of a baseline.
Criterion retains its own statistical estimates and raw artifacts.

### Coverage and timing boundaries

UI scenarios cover startup, opening a child folder, Back/Forward/Up/Refresh,
all four Details-view sort columns in both directions, local filtering,
recursive search, single/range/all selection, creating/switching/closing tabs,
and scrolling in both views. Empty folders cover startup and refresh. Media cases
cover opening a 12MP JPEG viewer, Alt-hover image/video/text/PDF/EPUB previews,
and visible image/video thumbnails in Large Icons. Details displays file icons
rather than a thumbnail grid, so it has no thumbnail-readiness scenario.

- **Action-to-submission** starts immediately before a production handler or
  GPUI input dispatch and ends after the renderer submits the first newly drawn
  scene containing the expected state. Directory completion checks the exact
  load generation, path, and entry count. Selection, sorting, search, and tab
  outcomes are checked; media checks reject loading placeholders and extraction
  failures. Startup starts before process creation and ends when the parent
  receives the directory-ready submission notification, including pipe transport.
- **CPU frame work** measures scene construction, excluding renderer submission
  and the readiness observer. **Renderer submission** includes CPU submission
  and any platform backpressure. These are not GPU execution times or physical
  display latency. Input starts inside GPUI, excluding OS event delivery.
- **Submission intervals** measure successive submissions on the relevant
  window, including replayed scenes. The interval crossing the action boundary
  is omitted. Reports keep sample counts and timing distributions, not an FPS
  claim. Refresh may require one benchmark-requested repaint after an unchanged
  directory load completes.
- Scrolling dispatches 120 wheel events of 20 logical pixels (2,400 pixels in
  each scenario's direction), paced over two/five seconds. Upward cases first
  scroll down. Bounds clamp naturally; completion requires actual movement.
  Event-loop delays can extend the elapsed workload; raw samples retain the
  input count. Alt-hover targets use recorded bounds of painted entries.

Ready-state checks and row-bound tracking add instrumentation overhead. Keep the
same harness version when comparing results. No instrumented code is included
when the `benchmarks` feature is disabled.

### Fixtures and isolation

Generated fixtures live under `target/performance-fixtures-v1`, or the Cargo
target directory selected by `CARGO_TARGET_DIR`. They are created before UI
process launch and outside measured Criterion regions. Folder fixtures include
numbered, Unicode, hidden, and long names, different file types, varied sizes
and modification times, and a nested search tree. Media is generated locally;
video generation requires `ffmpeg` and `ffprobe` on `PATH`. There are no downloads.

Each UI worker uses a dedicated settings/cache/window-state directory inside its
sample output. It does not route to a running Explorer instance. Fixed settings
disable tray behavior, remote locations, updater activity, and device/clipboard
polling; fonts, caches, rendering, loading, and Explorer handlers use production
initialization. The requested window is 1024 × 820 logical pixels; reports record
the actual viewport and scale factor. Keep the display configuration stable.

`empty` media cases begin with empty application caches. `primed` cases first
complete the same production flow, then leave/reopen it outside the measured
region. Video hover starts a new playback session and image viewers decode again;
priming does not imply those flows have a decoded-content cache. Thumbnail cases
measure navigation plus readiness of the visible thumbnails, rather than all
files in a folder. Filesystem caches are not flushed; these are not cold-disk
benchmarks. Criterion children receive their own isolated cache roots too.

To add a scenario, extend the catalog in `src/performance/mod.rs`, its preparation
and expected outcome in `ui.rs`, and, if needed, a feature-gated adapter under
`explorer::benchmark_support`. Call production code and capture readiness before
scene construction. Add tests for preparation, completion, and failures. Change
the scenario version when measurement semantics change and the fixture version
when fixture contents change. Validate with:

```sh
cargo check --locked --all-targets --features benchmarks
cargo test --locked --all-targets --features benchmarks
cargo check --locked
```

Headless unit tests validate harness logic; they do not substitute for desktop
runs. There are no CI performance jobs or automatic regression thresholds.

### Validation record — 2026-10-08

Windows real-window validation passed the complete quick preset: 54 UI cases
with five measured repetitions each, plus four representative Criterion cases.
Additional full-preset checks passed all 14 primed media cases with 30 measured
repetitions and startup in both views at every fixture size. Missing-tool skips,
comparison rejection/override, and isolation from personal settings/caches and
an independently running Explorer instance were checked.

The three Cargo validation commands above passed. Library tests reported 2,250
passed and six existing ignored tests; all eight Criterion suites also passed
their test-mode smoke runs. The entire full preset was not run.

macOS and Linux graphical desktops were unavailable on this Windows host.
Real-window validation on those platforms remains outstanding; headless tests
do not establish their rendering or input behavior.

### Large Icons layout

Large Icons supplies its exact row heights, including gaps, through GPUI's
`ListState::reset_with_sizes`. The complete scroll geometry exists before the
first ready frame; only viewport and overdraw rows construct tile elements.
Supplied sizes must match the next layout width. A subsequent width change
requires supplying updated sizes again; other GPUI lists retain their existing
measurement behavior.

Row geometry and tile heights use shared immutable arrays. Each Explorer view
keeps an LRU cache of at most 10,000 displayed filename measurements. Font changes
invalidate those measurements; sorting, filtering, navigation, and resizing can
reuse them. Current layout heights remain available independently of eviction.
Filename wrapping uses the production font, fixed tile text width, and three-line
limit. Changing the grid width repacks rows without measuring filenames again.

Unchanged redraws use a constant-size layout key. When changing visible entries,
call `invalidate_visible_entries` after replacement, reordering, or a change that
affects displayed names. Extension visibility and font changes are separate keys.
Keep the existing scroll preservation, deletion, and reveal rules when rebuilding
geometry. Benchmark completion and timing boundaries are unchanged.

Focused list validation can run independently of Explorer:

```sh
cargo test --locked --manifest-path vendor/gpui/Cargo.toml --target-dir target/gpui-list-validation --lib --features test-support elements::list::test
cargo test --locked --lib --features benchmarks large_icon
```

### Large Icons optimization measurements — 2026-10-08

Full-preset startup measurements used three warm-ups and 30 measured repetitions
per case, in the same Windows x86_64 environment, release profile, Rust 1.94.0,
1024 × 820 logical viewport, and scale factor 1. Fixture, scenario, and report
versions remain unchanged. CPU frame percentiles aggregate the observed drawing
frames, as in the original baseline; they exclude renderer submission.

| Large Icons fixture | CPU frame p95 before | After | Reduction | Startup median before | After |
|---|---:|---:|---:|---:|---:|
| 1,000 entries | 44.418 ms | 5.065 ms | 88.6% | 452.589 ms | 410.805 ms |
| 10,000 entries | 415.043 ms | 7.168 ms | 98.3% | 1,020.404 ms | 614.001 ms |

Both cases exceeded the local 80% CPU frame reduction goal. Size seeding and
shared arrays alone produced p95 values of 4.996 ms and 6.781 ms; the final cache
implementation keeps startup costs close to those results while reusing filename
metrics across subsequent actions and removing folder-sized redraw work.

Details remains the control: its startup CPU frame p95 changed from 5.189 to
5.231 ms at 1,000 entries and from 5.317 to 5.584 ms at 10,000 entries. Its final
startup medians were 409.319 and 614.081 ms, close to Large Icons. Remaining
end-to-end startup and navigation costs principally involve shared application
initialization and directory loading, outside this optimization's scope.

| Large Icons action | Entries | Median before | After | Reduction |
|---|---:|---:|---:|---:|
| Back | 1,000 | 61.908 ms | 30.366 ms | 51.0% |
| Back | 10,000 | 517.710 ms | 236.593 ms | 54.3% |
| Up | 1,000 | 60.900 ms | 31.496 ms | 48.3% |
| Up | 10,000 | 516.108 ms | 236.089 ms | 54.3% |
| New tab | 1,000 | 70.728 ms | 34.894 ms | 50.7% |
| New tab | 10,000 | 522.928 ms | 239.510 ms | 54.2% |
| Unchanged Refresh | 1,000 | 43.318 ms | 38.492 ms | 11.1% |
| Unchanged Refresh | 10,000 | 244.556 ms | 236.473 ms | 3.3% |

Full-preset scrolling uses the same 120 production wheel events over five seconds.
Final Large Icons frame work stays nearly independent of folder size:

| Entries | CPU frame p95 down | CPU frame p95 up | Submission-interval p95 down | Submission-interval p95 up |
|---|---:|---:|---:|---:|
| 100 | 2.639 ms | 2.568 ms | 9.233 ms | 9.240 ms |
| 1,000 | 2.616 ms | 2.608 ms | 9.131 ms | 9.153 ms |
| 10,000 | 2.729 ms | 2.706 ms | 9.210 ms | 9.217 ms |

Against the clean original baseline, 10,000-entry scrolling CPU frame p95 fell
from 3.893 to 2.729 ms downward (1.164 ms, 29.9%) and from 4.089 to 2.706 ms
upward (1.383 ms, 33.8%). Submission-interval p95 fell from 10.362 to 9.210 ms
downward (11.1%) and from 10.732 to 9.217 ms upward (14.1%). Renderer submission
remained approximately 0.085–0.087 ms at p95; this optimization primarily reduces
CPU scene construction rather than renderer submission.

The final full matrix passed all 46 startup, Back, Up, new-tab, Refresh, and
scrolling cases across both views and all applicable fixture sizes: 1,380 measured
samples, with no errors or skips. The preserved original startup baseline is
`target/performance/run-1791475314508559800-40628`; original navigation runs use
`target/performance-large-icons-before`, the intermediate build uses
`target/performance-large-icons-stage`, and final runs use
`target/performance-large-icons-after`.

The original executable was retained for every baseline case. Scrolling workers
whose lifetimes overlapped compilation/test CPU activity were repeated afterward
with that executable and three fresh warm-ups per affected case. Exactly 329
worker repetitions, including warm-ups, were replaced; unaffected samples and
original raw outputs remain preserved. Measurements still use empty application
caches and do not flush filesystem caches. Source requests/results and the
cleanup record live under `target/performance-large-icons-before/repeat-clean-*`.

Combined before/after summaries and comparisons live under
`target/performance-large-icons-comparison`. They contain 46 matching cases,
1,380 measured samples per revision, and no incompatible environment fields.
`all-metrics.md` includes absolute and percentage changes for median/p95 action
latency, CPU frame work, renderer submission, and submission intervals. Separate
comparisons retain the intermediate size-seeding build. Original run summaries
retain commit/dirty status and environment metadata; source directories are listed
in `sources.json`. Compare the combined summaries with:

```sh
target/release/explorer-bench compare target/performance-large-icons-comparison/before target/performance-large-icons-comparison/after
```

Final regression validation passed the complete quick preset: 54 real-window UI
cases with five measured repetitions each (270 samples), and four representative
directory-loading Criterion cases. There were no errors or skips. Its report is
`target/performance-large-icons-quick/run-1791484601748565600-42884/report.md`;
quick-run tail estimates remain exploratory.

Locked checks with all targets and benchmark features, the check without benchmark
features, and all-target tests passed. Library tests reported 2,256 passed and six
existing ignored tests; all eight Criterion suites passed their test-mode smoke
runs. The four focused vendored GPUI list tests also passed. Tests cover exact
supplied-size geometry and distant offsets, viewport-bounded construction at
10,000 entries, shared arrays, measurement reuse/LRU eviction, invalidation,
resizing, mixed/Unicode/long names, selection/reveal, rename, and deletion.
Desktop runs additionally checked production navigation, sorting, search,
selection, tabs, wheel dispatch, thumbnails, and all supported hover previews.

macOS and Linux graphical desktops were unavailable. Real-window validation on
those platforms remains outstanding. Steady viewport tile/text construction and
first-use filename measurement/row packing remain within Large Icons; shared
initialization, directory loading, and video performance were not optimized.

## Individual Criterion suites

The recursive-search benchmark suite measures scanning, cached filtering,
metadata materialization, cached and uncached full searches, and cancellation.
The navigation-pipeline benchmark measures directory entry loading for a small
Documents-like folder and empty/100/1,000/10,000-entry mixed folders, with hidden
entries both shown and hidden. The image-thumbnail benchmark
measures cold thumbnail extraction for large raster/SVG/TIFF files and parallel
JPEG batch extraction. The image-viewer benchmark measures native-resolution
opens, deferred ICC correction, and `RenderImage` construction. The properties
benchmark measures fast directory properties snapshots separately from exact
recursive totals.

Run it with:

```sh
cargo bench --features benchmarks --bench recursive_search
cargo bench --features benchmarks --bench navigation_pipeline
cargo bench --features benchmarks --bench image_thumbnails
cargo bench --features benchmarks --bench video_thumbnails
cargo bench --features benchmarks --bench image_viewer
cargo bench --features benchmarks --bench properties
cargo bench --features benchmarks --bench resumable_copy
```

The first run creates a deterministic 25,000-file fixture under
`target/recursive-search-benchmark-v3`. Save a baseline before changing the
pipeline and compare against it:

```sh
cargo bench --features benchmarks --bench recursive_search -- --save-baseline before
cargo bench --features benchmarks --bench recursive_search -- --baseline before
```

The navigation benchmark creates its fixture under
`target/navigation-pipeline-benchmark-v1`.

The image-thumbnail benchmark measures isolated Catmull-Rom RGBA resizing,
ready-for-display extraction at 128px and 400px, QOI disk-cache encoding and
decoding, batched cache writes with manifest persistence, full cache generation,
mixed-folder time-to-ready, queue cancellation, and parallel JPEG batches.
Fixtures cover opaque and transparent PNG, JPEG
(including 12MP), uncompressed/Deflate/LZW TIFF up to 48MP, pathological wide
TIFF, WebP, and SVG under `target/image-thumbnails-benchmark-v6`.

The video-thumbnail benchmark measures uncached sub-second, ordinary, long,
long-GOP Matroska, and malformed video thumbnails, a 24-video folder batch,
the 20-frame Properties strip, and Alt-hover time to first frame. It generates
MPEG-4 fixtures with FFmpeg under `target/video-thumbnails-benchmark-v2`;
`ffmpeg` and `ffprobe` must be available on `PATH`.

Set `EXPLORER_VIDEO_THUMBNAIL_BENCH_DIR` to include an optional real video
directory in the same benchmark run:

```sh
EXPLORER_VIDEO_THUMBNAIL_BENCH_DIR=/path/to/videos \
  cargo bench --profile release --features benchmarks --bench video_thumbnails
```

The image-viewer benchmark compares ICC-tagged native opens with synchronous
ICC, deferred first-ready opens, and ICC ignored; it also measures no-ICC native
opens, deferred ICC correction plus corrected `RenderImage` construction, and
`RenderImage` construction alone. Fixtures cover PNG, JPEG, TIFF, WebP, SVG,
and Display P3 ICC-tagged PNG/JPEG under `target/image-viewer-benchmark-v1`.

```sh
cargo bench --features benchmarks --bench image_viewer
```

Use the release profile when comparing shipped application performance:

```sh
cargo bench --profile release --features benchmarks --bench image_thumbnails
cargo bench --profile release --features benchmarks --bench video_thumbnails
```

The properties benchmark creates a deterministic large directory fixture under
`target/properties-benchmark-v1`.

The resumable-copy benchmark creates deterministic large-file, many-small-file,
same-size edit, shifted insert, and cancel/resume fixtures under
`target/resumable-copy-benchmark-v1`.

## Archive extraction

The archive-extraction suite measures one large file, many small files, and
many medium files through the same planning and execution pipeline used by the
application. It also compares AR, ZIP, and compressed TAR extraction and
isolates listing, planning, and progress-publication stages. Fixtures live
under `target/archive-extraction-benchmark-v2`.

```sh
cargo bench --features benchmarks --bench archive_extraction
cargo bench --features benchmarks --bench archive_extraction -- --save-baseline before
cargo bench --features benchmarks --bench archive_extraction -- --baseline before
```

Runtime diagnostics are JSONL on stderr:

```sh
cargo run -- --debug=archive
cargo run -- --debug=archive-verbose
```

Summary mode redacts archive and entry paths. Verbose mode additionally emits
path-bearing `slow_entry` records.
