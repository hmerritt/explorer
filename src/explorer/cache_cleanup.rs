use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gpui::App;
use serde::{Deserialize, Serialize};

use crate::settings::{
    DEFAULT_CACHE_CLEANUP_INTERVAL_DAYS, SettingsState, config_dir,
    normalized_cache_cleanup_interval_days,
};

const CACHE_CLEANUP_STATE_FILE_NAME: &str = "cache-cleanup-state.json";
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct CacheCleanupState {
    last_run_unix_seconds: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CacheCleanupAction {
    RecordOnly,
    Cleanup,
    Skip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CacheCleanupRun {
    action: CacheCleanupAction,
    next_check: Duration,
}

pub(crate) fn initialize(cx: &mut App) {
    let Some(state_path) = cache_cleanup_state_path() else {
        return;
    };

    cx.spawn(async move |cx| {
        loop {
            let interval_days = cx
                .update(|cx| {
                    cx.try_global::<SettingsState>()
                        .map(|state| state.value.app.cache_cleanup_interval_days)
                })
                .ok()
                .flatten()
                .unwrap_or(DEFAULT_CACHE_CLEANUP_INTERVAL_DAYS);
            let state_path = state_path.clone();
            let cleanup_task = cx.background_executor().spawn(async move {
                run_scheduled_cache_cleanup(&state_path, interval_days, SystemTime::now())
            });
            let next_check = match cleanup_task.await {
                Ok(run) => run.next_check,
                Err(error) => {
                    eprintln!("Unable to run Explorer cache cleanup: {error}");
                    cache_cleanup_interval(interval_days)
                }
            };

            cx.background_executor().timer(next_check).await;
        }
    })
    .detach();
}

fn cache_cleanup_state_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(CACHE_CLEANUP_STATE_FILE_NAME))
}

fn run_scheduled_cache_cleanup(
    state_path: &Path,
    interval_days: u32,
    now: SystemTime,
) -> io::Result<CacheCleanupRun> {
    run_scheduled_cache_cleanup_with(state_path, interval_days, now, || {
        super::image_thumbnails::cleanup_stale_path_cache_entries();
        super::app_icons::cleanup_stale_path_cache_entries();
    })
}

fn run_scheduled_cache_cleanup_with(
    state_path: &Path,
    interval_days: u32,
    now: SystemTime,
    cleanup: impl FnOnce(),
) -> io::Result<CacheCleanupRun> {
    let interval = cache_cleanup_interval(interval_days);
    let (action, next_check) =
        cache_cleanup_action(load_cache_cleanup_state(state_path), now, interval);
    match action {
        CacheCleanupAction::RecordOnly => {
            save_cache_cleanup_state(state_path, now)?;
        }
        CacheCleanupAction::Cleanup => {
            cleanup();
            save_cache_cleanup_state(state_path, now)?;
        }
        CacheCleanupAction::Skip => {}
    }

    Ok(CacheCleanupRun { action, next_check })
}

fn cache_cleanup_action(
    last_run: Option<SystemTime>,
    now: SystemTime,
    interval: Duration,
) -> (CacheCleanupAction, Duration) {
    let Some(last_run) = last_run else {
        return (CacheCleanupAction::RecordOnly, interval);
    };
    match now.duration_since(last_run) {
        Ok(elapsed) if elapsed >= interval => (CacheCleanupAction::Cleanup, interval),
        Ok(elapsed) => (CacheCleanupAction::Skip, interval.saturating_sub(elapsed)),
        Err(_) => (CacheCleanupAction::Skip, interval),
    }
}

fn load_cache_cleanup_state(path: &Path) -> Option<SystemTime> {
    let state = serde_json::from_str::<CacheCleanupState>(&fs::read_to_string(path).ok()?).ok()?;
    UNIX_EPOCH.checked_add(Duration::from_secs(state.last_run_unix_seconds))
}

fn save_cache_cleanup_state(path: &Path, last_run: SystemTime) -> io::Result<()> {
    let last_run_unix_seconds = last_run
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_secs();
    let state = CacheCleanupState {
        last_run_unix_seconds,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&state).map_err(io::Error::other)?;
    fs::write(path, json)
}

fn cache_cleanup_interval(interval_days: u32) -> Duration {
    Duration::from_secs(
        u64::from(normalized_cache_cleanup_interval_days(interval_days))
            .saturating_mul(SECONDS_PER_DAY),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_cleanup_state_records_now_without_cleanup() {
        let temp = TempDir::new("missing-cleanup-state");
        let state_path = temp.path().join(CACHE_CLEANUP_STATE_FILE_NAME);
        let now = UNIX_EPOCH + Duration::from_secs(100);
        let mut cleaned = false;

        let run =
            run_scheduled_cache_cleanup_with(&state_path, 30, now, || cleaned = true).unwrap();

        assert_eq!(run.action, CacheCleanupAction::RecordOnly);
        assert_eq!(run.next_check, cache_cleanup_interval(30));
        assert!(!cleaned);
        assert_eq!(load_cache_cleanup_state(&state_path), Some(now));
    }

    #[test]
    fn corrupt_cleanup_state_records_now_without_cleanup() {
        let temp = TempDir::new("corrupt-cleanup-state");
        let state_path = temp.path().join(CACHE_CLEANUP_STATE_FILE_NAME);
        fs::create_dir_all(temp.path()).unwrap();
        fs::write(&state_path, "{").unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(200);
        let mut cleaned = false;

        let run =
            run_scheduled_cache_cleanup_with(&state_path, 30, now, || cleaned = true).unwrap();

        assert_eq!(run.action, CacheCleanupAction::RecordOnly);
        assert_eq!(run.next_check, cache_cleanup_interval(30));
        assert!(!cleaned);
        assert_eq!(load_cache_cleanup_state(&state_path), Some(now));
    }

    #[test]
    fn unreadable_cleanup_state_is_treated_as_record_only() {
        let now = UNIX_EPOCH + Duration::from_secs(200);

        assert_eq!(
            cache_cleanup_action(None, now, cache_cleanup_interval(30)),
            (CacheCleanupAction::RecordOnly, cache_cleanup_interval(30))
        );
    }

    #[test]
    fn due_cleanup_state_runs_cleanup_and_records_now() {
        let temp = TempDir::new("due-cleanup-state");
        let state_path = temp.path().join(CACHE_CLEANUP_STATE_FILE_NAME);
        let last_run = UNIX_EPOCH + Duration::from_secs(10);
        let now = last_run + cache_cleanup_interval(30);
        save_cache_cleanup_state(&state_path, last_run).unwrap();
        let mut cleaned = false;

        let run =
            run_scheduled_cache_cleanup_with(&state_path, 30, now, || cleaned = true).unwrap();

        assert_eq!(run.action, CacheCleanupAction::Cleanup);
        assert_eq!(run.next_check, cache_cleanup_interval(30));
        assert!(cleaned);
        assert_eq!(load_cache_cleanup_state(&state_path), Some(now));
    }

    #[test]
    fn recent_cleanup_state_skips_cleanup() {
        let temp = TempDir::new("recent-cleanup-state");
        let state_path = temp.path().join(CACHE_CLEANUP_STATE_FILE_NAME);
        let last_run = UNIX_EPOCH + Duration::from_secs(10);
        let now = last_run + cache_cleanup_interval(30) - Duration::from_secs(1);
        save_cache_cleanup_state(&state_path, last_run).unwrap();
        let mut cleaned = false;

        let run =
            run_scheduled_cache_cleanup_with(&state_path, 30, now, || cleaned = true).unwrap();

        assert_eq!(run.action, CacheCleanupAction::Skip);
        assert_eq!(run.next_check, Duration::from_secs(1));
        assert!(!cleaned);
        assert_eq!(load_cache_cleanup_state(&state_path), Some(last_run));
    }

    #[test]
    fn cleanup_interval_is_normalized_to_at_least_one_day() {
        assert_eq!(
            cache_cleanup_interval(0),
            Duration::from_secs(SECONDS_PER_DAY)
        );
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("explorer-{name}-{}-{nanos}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
