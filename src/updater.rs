use std::{
    io,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use gpui::{App, Global};
use serde::Deserialize;
use serde_json::Value;

use crate::{settings::SettingsState, windows_squirrel::SquirrelInstallation};

const FIRST_RUN_DELAY: Duration = Duration::from_secs(30);
const SETTINGS_POLL_INTERVAL: Duration = Duration::from_secs(1);
const CHECK_TIMEOUT: Duration = Duration::from_secs(2 * 60);
const APPLY_TIMEOUT: Duration = Duration::from_secs(10 * 60);

struct UpdaterProcess;

impl Global for UpdaterProcess {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UpdateCycleResult {
    UpToDate,
    Installed,
}

fn next_check_after_cycle(
    now: Instant,
    interval: Duration,
    result: UpdateCycleResult,
) -> Option<Instant> {
    match result {
        UpdateCycleResult::UpToDate => Some(now + interval),
        UpdateCycleResult::Installed => None,
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn start(first_run: bool, cx: &mut App) {
    let installation = match crate::windows_squirrel::validated_installation() {
        Ok(installation) => installation,
        Err(error) => {
            eprintln!("Explorer self-updater disabled: {error}");
            return;
        }
    };

    // The single-instance gate runs before GPUI starts, so reaching this point means
    // this is the sole updater scheduler in the primary application process.
    cx.set_global(UpdaterProcess);
    let (result_tx, result_rx) = mpsc::channel::<io::Result<UpdateCycleResult>>();
    let initial_delay = if first_run {
        FIRST_RUN_DELAY
    } else {
        Duration::ZERO
    };

    cx.spawn(async move |cx| {
        let mut next_check = Instant::now() + initial_delay;
        let mut active = false;
        let mut scheduled_settings = cx
            .update(|cx| cx.global::<SettingsState>().value.updater.clone())
            .unwrap_or_default();

        loop {
            cx.background_executor().timer(SETTINGS_POLL_INTERVAL).await;

            while let Ok(result) = result_rx.try_recv() {
                active = false;
                match result {
                    Ok(result) => match next_check_after_cycle(
                        Instant::now(),
                        scheduled_settings.check_interval,
                        result,
                    ) {
                        Some(scheduled) => next_check = scheduled,
                        None => {
                            eprintln!(
                                "Explorer update installed; it will be used after Explorer closes and reopens"
                            );
                            return;
                        }
                    },
                    Err(error) => {
                        eprintln!("Explorer update check failed: {error}");
                        next_check = Instant::now() + scheduled_settings.check_interval;
                    }
                }
            }

            let settings = match cx.update(|cx| {
                cx.global::<SettingsState>().value.updater.clone()
            }) {
                Ok(settings) => settings,
                Err(_) => return,
            };

            if settings != scheduled_settings {
                scheduled_settings = settings.clone();
                if !active {
                    next_check = Instant::now() + settings.check_interval;
                }
            }
            if !settings.enabled || active || Instant::now() < next_check {
                continue;
            }

            active = true;
            let installation = installation.clone();
            let feed_url = settings.feed_url.clone();
            let result_tx = result_tx.clone();
            if let Err(error) = thread::Builder::new()
                .name("explorer-updater".to_owned())
                .spawn(move || {
                    let _ = result_tx.send(run_update_cycle(&installation, &feed_url));
                })
            {
                active = false;
                next_check = Instant::now() + settings.check_interval;
                eprintln!("Unable to start Explorer updater worker: {error}");
            }
        }
    })
    .detach();
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn start(_first_run: bool, _cx: &mut App) {}

fn run_update_cycle(
    installation: &SquirrelInstallation,
    feed_url: &str,
) -> io::Result<UpdateCycleResult> {
    let check_arg = format!("--checkForUpdate={feed_url}");
    let output = crate::windows_squirrel::run_update_variants(
        &installation.update_exe,
        &[&check_arg],
        CHECK_TIMEOUT,
    )?;
    if !parse_check_for_update_output(&output.stdout)? {
        return Ok(UpdateCycleResult::UpToDate);
    }

    let update_arg = format!("--update={feed_url}");
    crate::windows_squirrel::run_update_variants(
        &installation.update_exe,
        &[&update_arg],
        APPLY_TIMEOUT,
    )?;
    Ok(UpdateCycleResult::Installed)
}

#[derive(Debug, Deserialize)]
struct CheckForUpdateResponse {
    #[serde(rename = "releasesToApply", alias = "ReleasesToApply")]
    releases_to_apply: Vec<Value>,
}

fn parse_check_for_update_output(stdout: &[u8]) -> io::Result<bool> {
    let text = std::str::from_utf8(stdout)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let payload = extract_json_payload(text).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("malformed Squirrel check response: {}", text.trim()),
        )
    })?;
    let response: CheckForUpdateResponse = serde_json::from_str(payload).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("malformed Squirrel check response: {error}"),
        )
    })?;
    Ok(!response.releases_to_apply.is_empty())
}

fn extract_json_payload(output: &str) -> Option<&str> {
    let trimmed = output.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed);
    }
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('{') && line.ends_with('}'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_available_and_empty_update_responses() {
        assert!(
            parse_check_for_update_output(br#"{"releasesToApply":[{"version":"0.23.0"}]}"#)
                .unwrap()
        );
        assert!(
            !parse_check_for_update_output(b"Downloading RELEASES\r\n{\"releasesToApply\":[]}\r\n")
                .unwrap()
        );
    }

    #[test]
    fn malformed_or_incomplete_update_output_is_a_failure() {
        assert!(parse_check_for_update_output(b"no updates").is_err());
        assert!(parse_check_for_update_output(br#"{"futureReleaseEntry":null}"#).is_err());
        assert!(parse_check_for_update_output(&[0xff, 0xfe]).is_err());
    }

    #[test]
    fn successful_installation_suspends_future_checks() {
        let now = Instant::now();
        assert_eq!(
            next_check_after_cycle(now, Duration::from_secs(600), UpdateCycleResult::UpToDate),
            Some(now + Duration::from_secs(600))
        );
        assert_eq!(
            next_check_after_cycle(now, Duration::from_secs(600), UpdateCycleResult::Installed),
            None
        );
    }
}
