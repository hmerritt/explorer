# Windows startup investigation, 6 November 2026

## Environment and method

Windows 11 Pro 10.0.26300, AMD Ryzen 9 9950X3D (16 cores, 32 threads), 64 GB RAM,
NVIDIA GeForce RTX 5070 Ti (driver
32.0.16.2114), 3440×1440 at 143 Hz. A Parsec virtual display adapter was also
installed. Measurements use the optimized release profile and the `benchmarks`
feature, with isolated application state and normal OS file caches.
The GPUI viewport was 1024×820 at scale factor 1.0.

The baseline executable includes startup observers but precedes the graphics
optimization. Its SHA-256 is
`b4f3d53d3b9ee7b1d0a70dd5d633422fc0adf0ad3c4a78090a2887a9095ea1ca`.
Both variants use the normal application entry point and single-instance routing.
See [the benchmark instructions](startup-performance.md) for metric definitions.
The optimized executable SHA-256 is
`1c363e9e2e608e00fb15865e1779ab91ff4e1399a84280a835b9ba4891f6ce3d`.

## Measured comparison

For a 1,000-entry Details folder, with the tray enabled and empty application
caches, 30 samples per executable produced:

| Launch flow | Before median ms | After median ms | Before p95 ms | After p95 ms |
|---|---:|---:|---:|---:|
| Fresh process, loaded folder | 551.38 | 441.97 | 566.05 | 454.92 |
| Resident process, existing window | 131.28 | 129.79 | 136.94 | 136.10 |
| Resident process, all windows previously closed | 179.62 | 179.29 | 189.29 | 185.18 |

Fresh-process median improved **19.8%**, and p95 improved **19.6%**. The first
submitted frame improved from 524.56 to 416.73 ms median. Completion of the
keyboard selection check improved from 559.67 to 450.15 ms median.

The complete matrix contains 24 scenarios, 30 samples per executable per
scenario: **1,440 fresh-process samples and 3,600 launch-flow observations,
with zero failures**. Fresh-process median reductions ranged from 9.0% to
22.0%, saving 105–113 ms. Every fresh-process p95 improved. The slowest optimized
scenario had a 1,084 ms p95; the slowest individual optimized fresh sample was
1,100 ms. The 20% target was approached in the main case and exceeded in several
smaller-folder cases; it was not achieved across the entire matrix.

These representative cases have the tray enabled and empty application caches:

| View | Entries | Before median ms | After median ms | Median reduction | Before p95 ms | After p95 ms |
|---|---:|---:|---:|---:|---:|---:|
| Details | 0 | 515.33 | 408.78 | 20.7% | 532.22 | 417.52 |
| Large Icons | 0 | 524.74 | 416.65 | 20.6% | 565.21 | 440.78 |
| Details | 1,000 | 551.38 | 441.97 | 19.8% | 566.05 | 454.92 |
| Large Icons | 1,000 | 597.97 | 488.86 | 18.2% | 612.09 | 497.11 |
| Details | 10,000 | 769.87 | 662.23 | 14.0% | 781.07 | 683.41 |
| Large Icons | 10,000 | 1,170.65 | 1,059.56 | 9.5% | 1,186.16 | 1,079.13 |

Executables alternated order on successive repetitions. Raw samples, failures,
first-sample markers, scenario settings, and hardware metadata are retained in
[the comparison output](../target/startup-final-comparison/startup-1793976680507148100-21312/report.md)
and its adjacent `startup.json`.

The exact full-matrix command was:

```powershell
target/release/explorer-bench.exe startup --baseline target/startup-baseline-v2/explorer.exe --repetitions 30 --output target/startup-final-comparison
```

An earlier incomplete harness run omitted device-event monitoring and recorded
three folder-frame timeouts. It was excluded from the comparison. Both final
executables were rebuilt after correcting that shortcut; normal device
monitoring and clipboard polling are enabled in the reported measurements.

## Resident reopening acceptance

Five of the 36 resident-flow p95 comparisons initially increased by more than
5%, across four empty-folder scenarios. Each scenario received another 30
alternating samples per executable. None reproduced an increase above 5%.
All resident p95 changes in those repeat runs ranged from −1.3% to +4.8%; no
resident median regression exceeded 5% in either the original or repeat runs.

| Empty-folder scenario | Flow | Initial p95 change | Repeat p95 change |
|---|---|---:|---:|
| Details, tray off, empty cache | Existing window | +5.2% | −1.3% |
| Details, tray on, primed cache | Existing window | +8.6% | +1.6% |
| Large Icons, tray on, empty cache | Windows previously closed | +5.4% | +0.6% |
| Large Icons, tray on, primed cache | Existing window | +15.7% | +0.1% |
| Large Icons, tray on, primed cache | Windows previously closed | +9.8% | −0.2% |

The additional 240 fresh-process samples completed without failures. The
[repeat index](../target/startup-repeat-comparison/repeat-index.json) retains
the original flags and the locations of all four complete repeat reports.

## Stage breakdown

Thirty diagnostic launches per executable of the main 1,000-entry Details
scenario gave these mean stage durations. The first occurrence of each stage
before the loaded-folder milestone is included. Nested and background stages
overlap and must not be summed.

| Stage | Before mean ms | After mean ms |
|---|---:|---:|
| GPUI application initialization | 302.38 | 195.35 |
| Window creation, including Explorer tabs | 63.06 | 63.09 |
| Sidebar discovery, inside tab creation | 29.37 | 29.47 |
| Background directory load | 24.30 | 23.26 |
| Application initialization | 19.04 | 18.94 |
| Settings, inside application initialization | 14.27 | 14.13 |
| Tray initialization | 6.86 | 7.01 |
| Applying loaded entries | 3.91 | 4.06 |
| HTTP client construction | 2.89 | 2.91 |
| Embedded fonts | 0.68 | 0.68 |

Cache-root setup, native-icon cache setup, thumbnail-cache setup, and maintenance
scheduling each took less than 0.5 ms on average.

## Tracing overhead

Six pairs of five-sample batches alternated traced/untraced order, while each
batch also alternated baseline/optimized executable order. This provides 30
samples per executable per tracing mode, separate from headline measurements.

| Build | Trace off median ms | Trace on median ms | Observed change ms | Observed change |
|---|---:|---:|---:|---:|
| Before | 549.61 | 550.64 | +1.02 | +0.19% |
| After | 443.19 | 441.66 | −1.53 | −0.34% |

Individual paired-batch median changes ranged from −6.02 to +5.46 ms. These
results do not establish negative tracing cost: the observed changes were
smaller than batch variation. Headline results always have stage tracing off.
The [batch index](../target/startup-tracing-overhead/overhead-index.json) and
[stage/overhead calculations](../target/startup-tracing-overhead/analysis.json)
retain the diagnostic evidence.

## Native graphics diagnosis

Windows Performance Recorder could not enable the system profiling policy
(`0xc5585011`); no recording was left running. A separate native API probe timed
DXGI factory creation and two consecutive Direct3D device creations in five
fresh processes. Factory creation took approximately 7 ms, the first device
129–132 ms, and the second device 100–103 ms. These isolated native timings
identify the duplicate creation cost; the application benchmark determines the
actual end-to-end improvement.

## Retained change

The vendored GPUI backend created a real Direct3D device while testing each GPU
adapter, discarded the successful device, and created another for rendering.
It now retains the successful device, immediate context, and negotiated feature
level. Adapter order, capability checks, debug-layer flags, and device-loss
recovery remain in effect. Regression tests cover unsupported adapters, retaining
the tested result, stopping at the first supported adapter, and enumeration errors.

HTTP, sidebar, tray, and cache behavior remain unchanged. Their measured costs
did not justify introducing deferred initialization in this pass. The retained
change removes redundant work and adds no deferred work or new first-use cost.

## Remaining bottlenecks

GPUI initialization still averages about 195 ms. The difference between parent
launch-to-folder timing and process-local entry-to-folder timing averaged about
119 ms in the untraced diagnostic batches. This includes process loading and
notification transport; it is not a pure loader or display-latency measurement.

Five traced samples per executable of each 10,000-entry view found approximately
210 ms loading the directory and 41 ms applying its entries. Those stages were
similar in both views. Large Icons nevertheless submitted its loaded-folder
frame roughly 400 ms later than Details, identifying substantial remaining work
after directory application.

Separate existing UI-worker diagnostics measured a **200.83 ms CPU frame** for
10,000-entry Large Icons, compared with **2.57 ms** for Details. These counters
exclude renderer submission. Their different startup entry point means their
end-to-end timings are excluded from the headline comparison. UI preparation,
layout, and painting are the next targets for the large-folder workload; the
remaining post-application time needs finer attribution before another refactor.
See the [normal-entry diagnostic output](../target/startup-large-folder-diagnostics/startup-1793979692230778400-46092/report.md)
and [Large Icons frame diagnostic](../target/startup-render-diagnostics/run-1793979814334079500-41348/report.md).

No background work was deferred by the retained change. Optional icons and
thumbnails can still complete after the loaded-folder milestone, and the
250 ms cache-priming interval does not assert that every background job finished.

## Validation

- `cargo check --locked`
- `cargo check --locked --features benchmarks --bins`
- `cargo test --locked --all-targets`: 2,389 passed, six ignored, zero failed.
- `cargo test --locked --features benchmarks --lib performance:: --quiet`:
  19 passed, including configuration isolation and startup scenario validation.
- Both executables completed all 24 startup scenarios in a smoke run, including
  renderer milestones, consumed keyboard input, existing-window routing, and
  reopening after all windows closed to the tray.
- The legacy `ui/startup/details/1000` benchmark still completes successfully.
- All 54 quick UI benchmark cases passed, covering navigation, sorting,
  selection, tabs, scrolling, previews, and image/video thumbnails. These were
  functional checks during compilation, not headline performance measurements.
- A normal debug build rejected a startup-worker request with exit code 2,
  before creating any files in the supplied configuration directory.
- Ten real-window rapid-close checks passed, alternating Details and Large
  Icons: close after the first frame while 10,000 entries are still loading,
  verify the process stays resident, reopen through single-instance routing,
  verify the loaded folder and keyboard selection, then quit cleanly.

Hardware device-loss recovery and native macOS/Linux launches were not exercised
on this Windows machine. Physical device disconnection and unavailable mapped
network drives were not manually exercised. No tray lifecycle or shared
filesystem behavior was changed.

