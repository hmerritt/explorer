use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

use gpui::{Context, Task};
use notify::{RecursiveMode, Watcher};

use crate::explorer::view::ExplorerView;

const WATCH_REFRESH_INTERVAL: Duration = Duration::from_millis(150);

pub(super) struct DirectoryWatcher {
    _watcher: notify::RecommendedWatcher,
    _task: Task<()>,
}

impl DirectoryWatcher {
    pub(super) fn start(
        path: PathBuf,
        recursive: bool,
        cx: &mut Context<ExplorerView>,
    ) -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                if let Ok(event) = result {
                    let _ = tx.send(event.paths);
                }
            })
            .ok()?;

        let recursive_mode = if recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        watcher.watch(&path, recursive_mode).ok()?;

        let task = spawn_watcher_task(path.clone(), recursive, rx, cx);
        Some(Self {
            _watcher: watcher,
            _task: task,
        })
    }
}

fn spawn_watcher_task(
    watched_path: PathBuf,
    recursive: bool,
    rx: Receiver<Vec<PathBuf>>,
    cx: &mut Context<ExplorerView>,
) -> Task<()> {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(WATCH_REFRESH_INTERVAL).await;

            let mut should_reload = false;
            while let Ok(paths) = rx.try_recv() {
                if paths.is_empty()
                    || paths
                        .iter()
                        .any(|path| watched_event_is_relevant(path, &watched_path, recursive))
                {
                    should_reload = true;
                }
            }

            if !should_reload {
                continue;
            }

            let should_continue = this
                .update(cx, |explorer, cx| {
                    if explorer.path() == watched_path {
                        explorer.reload();
                        explorer.refresh_search_after_external_change(cx);
                        cx.notify();
                    }
                })
                .is_ok();

            if !should_continue {
                break;
            }
        }
    })
}

pub(super) fn watched_event_is_relevant(
    event_path: &Path,
    watched_path: &Path,
    recursive: bool,
) -> bool {
    if event_path == watched_path {
        return true;
    }

    if recursive {
        event_path.starts_with(watched_path)
    } else {
        event_path.parent() == Some(watched_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watched_event_accepts_current_directory_itself() {
        let watched = PathBuf::from("/folder");

        assert!(watched_event_is_relevant(&watched, &watched, false));
    }

    #[test]
    fn watched_event_accepts_direct_children_only() {
        let watched = PathBuf::from("/folder");

        assert!(watched_event_is_relevant(
            &watched.join("file.txt"),
            &watched,
            false
        ));
        assert!(!watched_event_is_relevant(
            &watched.join("child").join("nested.txt"),
            &watched,
            false
        ));
        assert!(!watched_event_is_relevant(
            &PathBuf::from("/other/file.txt"),
            &watched,
            false
        ));
    }

    #[test]
    fn recursive_watched_event_accepts_nested_descendants() {
        let watched = PathBuf::from("/folder");

        assert!(watched_event_is_relevant(
            &watched.join("child").join("nested.txt"),
            &watched,
            true
        ));
        assert!(!watched_event_is_relevant(
            &PathBuf::from("/other/file.txt"),
            &watched,
            true
        ));
    }
}
