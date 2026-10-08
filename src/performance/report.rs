use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

pub const REPORT_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Sample {
    pub action_to_submission_ms: f64,
    pub cpu_frame_work_ms: Vec<f64>,
    pub renderer_submission_ms: Vec<f64>,
    pub submission_intervals_ms: Vec<f64>,
    pub input_events: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct WorkerResult {
    pub sample: Option<Sample>,
    pub error: Option<String>,
    pub scale_factor: Option<f32>,
    pub viewport: Option<[f32; 2]>,
    pub display_backend: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Distribution {
    pub count: usize,
    pub median_ms: Option<f64>,
    pub p95_ms: Option<f64>,
}

pub fn distribution(values: impl IntoIterator<Item = f64>) -> Distribution {
    let mut values = values
        .into_iter()
        .filter(|v| v.is_finite() && *v >= 0.)
        .collect::<Vec<_>>();
    values.sort_by(f64::total_cmp);
    let percentile = |fraction: f64| {
        values
            .get(((values.len() as f64 * fraction).ceil() as usize).saturating_sub(1))
            .copied()
    };
    Distribution {
        count: values.len(),
        median_ms: if values.is_empty() {
            None
        } else if values.len() % 2 == 0 {
            let middle = values.len() / 2;
            Some(values[middle - 1] / 2. + values[middle] / 2.)
        } else {
            Some(values[values.len() / 2])
        },
        p95_ms: percentile(0.95),
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Case {
    pub samples: Vec<Sample>,
    pub errors: Vec<String>,
    pub skipped: Option<String>,
    pub action_to_submission: Distribution,
    pub cpu_frame_work: Distribution,
    pub renderer_submission: Distribution,
    pub submission_intervals: Distribution,
    pub criterion_estimates: Option<serde_json::Value>,
    pub criterion_sample_count: Option<usize>,
}

impl Case {
    pub fn summarize(&mut self) {
        self.action_to_submission =
            distribution(self.samples.iter().map(|s| s.action_to_submission_ms));
        self.cpu_frame_work = distribution(
            self.samples
                .iter()
                .flat_map(|s| s.cpu_frame_work_ms.iter().copied()),
        );
        self.renderer_submission = distribution(
            self.samples
                .iter()
                .flat_map(|s| s.renderer_submission_ms.iter().copied()),
        );
        self.submission_intervals = distribution(
            self.samples
                .iter()
                .flat_map(|s| s.submission_intervals_ms.iter().copied()),
        );
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Report {
    pub version: u32,
    pub scenario_version: u32,
    pub fixture_version: String,
    pub preset: String,
    pub metadata: BTreeMap<String, String>,
    pub cases: BTreeMap<String, Case>,
    pub incomplete_coverage: bool,
}

impl Report {
    pub fn save(&mut self, root: &Path) -> Result<(), String> {
        self.incomplete_coverage = self
            .cases
            .values()
            .any(|c| c.skipped.is_some() || !c.errors.is_empty());
        for case in self.cases.values_mut() {
            case.summarize();
        }
        fs::write(
            root.join("summary.json"),
            serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let mut markdown = format!(
            "# Explorer performance run\n\nPreset: `{}`. Incomplete coverage: **{}**.\n\n",
            self.preset, self.incomplete_coverage
        );
        if self.preset == "quick" {
            markdown.push_str(
                "Tail estimates are exploratory: five measured repetitions per UI scenario.\n\n",
            );
        }
        markdown.push_str("Action-to-submission excludes OS input delivery and physical display latency. Startup includes process launch and notification transport. CPU frame work excludes renderer submission. Submission intervals include compositor scheduling and repeat presentations; they are not GPU execution times.\n\n");
        markdown.push_str("| Scenario | Samples | Median ms | p95 ms | CPU frame median ms | Interval p95 ms | Status |\n|---|---:|---:|---:|---:|---:|---|\n");
        for (id, case) in &self.cases {
            if case.criterion_estimates.is_some() {
                continue;
            }
            let status = case.skipped.clone().unwrap_or_else(|| {
                if case.errors.is_empty() {
                    "ok".into()
                } else {
                    case.errors.join("; ")
                }
            });
            markdown.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} |\n",
                id,
                case.action_to_submission.count,
                number(case.action_to_submission.median_ms),
                number(case.action_to_submission.p95_ms),
                number(case.cpu_frame_work.median_ms),
                number(case.submission_intervals.p95_ms),
                status.replace('|', "\\|").replace('\n', " ")
            ));
        }
        if self
            .cases
            .values()
            .any(|case| case.criterion_estimates.is_some())
        {
            markdown.push_str("\n## Criterion estimates\n\n| Scenario | Samples | Median ms | Median confidence interval ms |\n|---|---:|---:|---|\n");
            for (id, case) in &self.cases {
                if let Some(estimates) = &case.criterion_estimates {
                    let interval = estimates
                        .get("median")
                        .and_then(|v| v.get("confidence_interval"));
                    let bound = |name| {
                        interval
                            .and_then(|v| v.get(name))
                            .and_then(|v| v.as_f64())
                            .map(|ns| ns / 1_000_000.)
                    };
                    markdown.push_str(&format!(
                        "| {id} | {} | {} | {} – {} |\n",
                        case.criterion_sample_count
                            .map_or_else(|| "—".into(), |n| n.to_string()),
                        number(criterion_median(case)),
                        number(bound("lower_bound")),
                        number(bound("upper_bound"))
                    ));
                }
            }
        }
        markdown.push_str("\nCriterion estimates retain their native nanosecond units in JSON and their original artifacts under `criterion/`.\n\n## Environment\n\n");
        for (key, value) in &self.metadata {
            markdown.push_str(&format!("- {key}: `{}`\n", value.replace('`', "'")));
        }
        fs::write(root.join("report.md"), markdown).map_err(|e| e.to_string())
    }
}

fn number(value: Option<f64>) -> String {
    value
        .map(|v| format!("{v:.3}"))
        .unwrap_or_else(|| "—".into())
}

pub fn load(root: &Path) -> Result<Report, String> {
    let path = if root.is_dir() {
        root.join("summary.json")
    } else {
        root.to_path_buf()
    };
    let report: Report = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    if report.version != REPORT_VERSION {
        return Err("unsupported report version".into());
    }
    Ok(report)
}

pub fn incompatibilities(before: &Report, after: &Report) -> Vec<String> {
    let mut differences = Vec::new();
    if before.scenario_version != after.scenario_version {
        differences.push("scenario version".into());
    }
    if before.fixture_version != after.fixture_version {
        differences.push("fixture version".into());
    }
    if before.preset != after.preset {
        differences.push("preset/sample policy".into());
    }
    for key in [
        "profile",
        "rustc",
        "os",
        "arch",
        "cpu",
        "display_backend",
        "scale_factor",
        "viewport",
        "refresh_rate",
        "ffmpeg",
        "ffprobe",
    ] {
        if before.metadata.get(key) != after.metadata.get(key) {
            differences.push(key.into());
        }
    }
    differences
}

pub fn compare(before: &Report, after: &Report, allow: bool) -> Result<String, String> {
    let differences = incompatibilities(before, after);
    if !allow && !differences.is_empty() {
        return Err(format!(
            "incompatible runs: {}; use --allow-incompatible to compare explicitly",
            differences.join(", ")
        ));
    }
    let mut output = format!(
        "# Explorer performance comparison\n\nEnvironment differences: {}. Positive changes mean slower. Timing changes are informational.\n\n| Scenario | Metric | Before ms | After ms | Change ms | Change % |\n|---|---|---:|---:|---:|---:|\n",
        if differences.is_empty() {
            "none".into()
        } else {
            differences.join(", ")
        }
    );
    let ids = before
        .cases
        .keys()
        .chain(after.cases.keys())
        .collect::<std::collections::BTreeSet<_>>();
    for id in ids {
        let (Some(left), Some(right)) = (before.cases.get(id), after.cases.get(id)) else {
            output.push_str(&format!(
                "| {id} | missing in {} | — | — | — | — |\n",
                if before.cases.contains_key(id) {
                    "after"
                } else {
                    "before"
                }
            ));
            continue;
        };
        if left.skipped.is_some()
            || right.skipped.is_some()
            || !left.errors.is_empty()
            || !right.errors.is_empty()
        {
            output.push_str(&format!(
                "| {id} | incomplete case; inspect run reports | — | — | — | — |\n"
            ));
            continue;
        }
        for (metric, a, b) in [
            (
                "action median",
                left.action_to_submission.median_ms,
                right.action_to_submission.median_ms,
            ),
            (
                "action p95",
                left.action_to_submission.p95_ms,
                right.action_to_submission.p95_ms,
            ),
            (
                "CPU frame median",
                left.cpu_frame_work.median_ms,
                right.cpu_frame_work.median_ms,
            ),
            (
                "submission median",
                left.renderer_submission.median_ms,
                right.renderer_submission.median_ms,
            ),
            (
                "interval p95",
                left.submission_intervals.p95_ms,
                right.submission_intervals.p95_ms,
            ),
            (
                "Criterion median",
                criterion_median(left),
                criterion_median(right),
            ),
        ] {
            if let (Some(a), Some(b)) = (a, b) {
                let percent = if a > 0. {
                    format!("{:+.2}%", (b - a) / a * 100.)
                } else {
                    "n/a".into()
                };
                output.push_str(&format!(
                    "| {id} | {metric} | {a:.3} | {b:.3} | {:+.3} | {percent} |\n",
                    b - a
                ));
            }
        }
    }
    Ok(output)
}

fn criterion_median(case: &Case) -> Option<f64> {
    case.criterion_estimates
        .as_ref()?
        .get("median")?
        .get("point_estimate")?
        .as_f64()
        .map(|ns| ns / 1_000_000.)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn report() -> Report {
        Report {
            version: REPORT_VERSION,
            scenario_version: 1,
            fixture_version: "v1".into(),
            preset: "quick".into(),
            metadata: BTreeMap::new(),
            cases: BTreeMap::new(),
            incomplete_coverage: false,
        }
    }
    #[test]
    fn percentiles_handle_empty_unsorted_and_invalid_values() {
        assert_eq!(distribution([]).median_ms, None);
        let d = distribution([4., 1., 3., 2., f64::NAN, -1., f64::INFINITY]);
        assert_eq!(d.count, 4);
        assert_eq!(d.median_ms, Some(2.5));
        assert_eq!(d.p95_ms, Some(4.));
        assert_eq!(distribution([3., 1., 2.]).median_ms, Some(2.));
        assert_eq!(distribution([1.]).median_ms, Some(1.));
        assert_eq!(distribution((1..=100).map(f64::from)).p95_ms, Some(95.));
    }
    #[test]
    fn report_round_trip_preserves_raw_samples_and_failures() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = report();
        r.cases.insert(
            "ui/test".into(),
            Case {
                samples: vec![Sample {
                    action_to_submission_ms: 12.,
                    cpu_frame_work_ms: vec![2., 3.],
                    ..Default::default()
                }],
                errors: vec!["timeout".into()],
                ..Default::default()
            },
        );
        r.save(dir.path()).unwrap();
        let read = load(dir.path()).unwrap();
        assert!(read.incomplete_coverage);
        assert_eq!(
            read.cases["ui/test"].action_to_submission.median_ms,
            Some(12.)
        );
        assert_eq!(
            read.cases["ui/test"].samples[0].cpu_frame_work_ms,
            vec![2., 3.]
        );
    }
    #[test]
    fn comparisons_require_explicit_environment_override_and_flag_missing_cases() {
        let a = report();
        let mut b = report();
        b.metadata.insert("cpu".into(), "other".into());
        b.cases.insert("new".into(), Case::default());
        assert!(compare(&a, &b, false).unwrap_err().contains("cpu"));
        assert!(compare(&a, &b, true).unwrap().contains("missing in before"));
    }
}
