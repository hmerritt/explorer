use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use futures::{
    FutureExt, StreamExt,
    channel::mpsc::{self, UnboundedReceiver},
};
use gpui::{Context, Task};
use notify::{RecursiveMode, Watcher};

use crate::explorer::view::ExplorerView;

const WATCH_REFRESH_DEBOUNCE: Duration = Duration::from_millis(1000);
const POLL_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

pub(super) struct DirectoryWatcher {
    _watcher: Option<notify::RecommendedWatcher>,
    _task: Task<()>,
}

impl DirectoryWatcher {
    pub(super) fn start(path: PathBuf, cx: &mut Context<ExplorerView>) -> Option<Self> {
        let (tx, rx) = mpsc::unbounded();
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                if let Ok(event) = result {
                    let _ = tx.unbounded_send(event.paths);
                }
            })
            .ok()?;

        watcher.watch(&path, RecursiveMode::NonRecursive).ok()?;

        let task = spawn_watcher_task(path.clone(), rx, cx);
        Some(Self {
            _watcher: Some(watcher),
            _task: task,
        })
    }

    pub(super) fn start_polling(path: PathBuf, cx: &mut Context<ExplorerView>) -> Option<Self> {
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_REFRESH_INTERVAL).await;
                let should_continue = this
                    .update(cx, |explorer, cx| {
                        if explorer.path() == path {
                            explorer.reload_async_with_entry_metadata_resolution(cx);
                            cx.notify();
                        }
                    })
                    .is_ok();

                if !should_continue {
                    break;
                }
            }
        });
        Some(Self {
            _watcher: None,
            _task: task,
        })
    }
}

fn spawn_watcher_task(
    watched_path: PathBuf,
    mut rx: UnboundedReceiver<Vec<PathBuf>>,
    cx: &mut Context<ExplorerView>,
) -> Task<()> {
    cx.spawn(async move |this, cx| {
        loop {
            let Some(paths) = rx.next().await else {
                break;
            };
            if !watched_event_paths_are_relevant(&paths, &watched_path) {
                continue;
            }

            'debounce: loop {
                let debounce = cx
                    .background_executor()
                    .timer(WATCH_REFRESH_DEBOUNCE)
                    .fuse();
                futures::pin_mut!(debounce);

                loop {
                    futures::select! {
                        paths = rx.next().fuse() => {
                            let Some(paths) = paths else {
                                return;
                            };
                            if watched_event_paths_are_relevant(&paths, &watched_path) {
                                continue 'debounce;
                            }
                        }
                        _ = debounce => break 'debounce,
                    }
                }
            }

            let should_continue = this
                .update(cx, |explorer, cx| {
                    if explorer.path() == watched_path {
                        explorer.reload_async_with_entry_metadata_resolution(cx);
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

fn watched_event_paths_are_relevant(paths: &[PathBuf], watched_path: &Path) -> bool {
    paths.is_empty()
        || paths
            .iter()
            .any(|path| watched_event_is_relevant(path, watched_path))
}

pub(super) fn watched_event_is_relevant(event_path: &Path, watched_path: &Path) -> bool {
    if event_path == watched_path {
        return true;
    }

    event_path.parent() == Some(watched_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::explorer::{
        context_menu::ContextMenuState,
        test_support::{TempDir, test_view_entity_at_path},
        view::ExplorerContentBranch,
    };
    use futures::channel::mpsc::UnboundedSender;
    use gpui::{AppContext, Entity};

    #[test]
    fn watched_event_accepts_current_directory_itself() {
        let watched = PathBuf::from("/folder");

        assert!(watched_event_is_relevant(&watched, &watched));
    }

    #[test]
    fn watched_event_accepts_direct_children_only() {
        let watched = PathBuf::from("/folder");

        assert!(watched_event_is_relevant(
            &watched.join("file.txt"),
            &watched
        ));
        assert!(!watched_event_is_relevant(
            &watched.join("child").join("nested.txt"),
            &watched
        ));
        assert!(!watched_event_is_relevant(
            &PathBuf::from("/other/file.txt"),
            &watched
        ));
    }

    #[test]
    fn watched_event_paths_accept_empty_batches() {
        let watched = PathBuf::from("/folder");

        assert!(watched_event_paths_are_relevant(&[], &watched));
    }

    #[gpui::test]
    fn watcher_refreshes_after_debounce(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        fs::write(temp.path().join("existing.txt"), b"file").unwrap();
        let watched_path = temp.path().to_path_buf();
        let (view, cx) = test_view_entity_at_path(cx, watched_path.clone());
        let tx = install_test_watcher(cx, &view, watched_path.clone());

        fs::write(watched_path.join("new.txt"), b"new").unwrap();
        send_watcher_paths(&tx, vec![watched_path.join("new.txt")]);
        cx.run_until_parked();
        assert!(!view_has_entry(cx, &view, "new.txt"));

        cx.executor().advance_clock(Duration::from_millis(999));
        cx.run_until_parked();
        assert!(!view_has_entry(cx, &view, "new.txt"));

        cx.executor().advance_clock(Duration::from_millis(1));
        cx.run_until_parked();
        assert!(view_has_entry(cx, &view, "new.txt"));
    }

    #[gpui::test]
    fn watcher_refresh_preserves_open_context_menu(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        fs::write(temp.path().join("existing.txt"), b"file").unwrap();
        let watched_path = temp.path().to_path_buf();
        let (view, cx) = test_view_entity_at_path(cx, watched_path.clone());
        let tx = install_test_watcher(cx, &view, watched_path.clone());

        cx.update(|_, app| {
            view.update(app, |view, _| {
                view.context_menu = Some(ContextMenuState::new(
                    gpui::point(gpui::px(20.0), gpui::px(20.0)),
                    Vec::new(),
                ));
            });
        });

        fs::write(watched_path.join("new.txt"), b"new").unwrap();
        send_watcher_paths(&tx, vec![watched_path.join("new.txt")]);
        cx.run_until_parked();
        cx.executor().advance_clock(WATCH_REFRESH_DEBOUNCE);
        cx.run_until_parked();

        assert!(view_has_entry(cx, &view, "new.txt"));
        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_some());
        });
    }

    #[gpui::test]
    fn manual_refresh_closes_open_context_menu(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        fs::write(temp.path().join("existing.txt"), b"file").unwrap();
        let (view, cx) = test_view_entity_at_path(cx, temp.path().to_path_buf());

        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.context_menu = Some(ContextMenuState::new(
                    gpui::point(gpui::px(20.0), gpui::px(20.0)),
                    Vec::new(),
                ));
                view.refresh_with_entry_metadata_resolution(cx);
                assert!(view.context_menu.is_none());
                assert_eq!(view.content_branch(), ExplorerContentBranch::List);
                assert!(
                    view.entries
                        .iter()
                        .any(|entry| entry.name == "existing.txt")
                );
            });
        });
        cx.run_until_parked();

        cx.read_entity(&view, |view, _| {
            assert!(view.context_menu.is_none());
        });
    }

    #[gpui::test]
    fn watcher_restarts_debounce_for_relevant_events(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        fs::write(temp.path().join("existing.txt"), b"file").unwrap();
        let watched_path = temp.path().to_path_buf();
        let (view, cx) = test_view_entity_at_path(cx, watched_path.clone());
        let tx = install_test_watcher(cx, &view, watched_path.clone());

        fs::write(watched_path.join("first.txt"), b"first").unwrap();
        send_watcher_paths(&tx, vec![watched_path.join("first.txt")]);
        cx.run_until_parked();

        cx.executor().advance_clock(Duration::from_millis(900));
        cx.run_until_parked();
        fs::write(watched_path.join("second.txt"), b"second").unwrap();
        send_watcher_paths(&tx, vec![watched_path.join("second.txt")]);
        cx.run_until_parked();

        cx.executor().advance_clock(Duration::from_millis(999));
        cx.run_until_parked();
        assert!(!view_has_entry(cx, &view, "first.txt"));
        assert!(!view_has_entry(cx, &view, "second.txt"));

        cx.executor().advance_clock(Duration::from_millis(1));
        cx.run_until_parked();
        assert!(view_has_entry(cx, &view, "first.txt"));
        assert!(view_has_entry(cx, &view, "second.txt"));
    }

    #[gpui::test]
    fn watcher_ignores_irrelevant_events_for_debounce(cx: &mut gpui::TestAppContext) {
        let temp = TempDir::new();
        fs::write(temp.path().join("existing.txt"), b"file").unwrap();
        let nested_dir = temp.path().join("child");
        fs::create_dir(&nested_dir).unwrap();
        fs::write(nested_dir.join("nested.txt"), b"nested").unwrap();
        let watched_path = temp.path().to_path_buf();
        let nested_path = nested_dir.join("nested.txt");
        let (view, cx) = test_view_entity_at_path(cx, watched_path.clone());
        let tx = install_test_watcher(cx, &view, watched_path.clone());

        fs::write(watched_path.join("new.txt"), b"new").unwrap();
        send_watcher_paths(&tx, vec![nested_path.clone()]);
        cx.run_until_parked();
        cx.executor().advance_clock(WATCH_REFRESH_DEBOUNCE);
        cx.run_until_parked();
        assert!(!view_has_entry(cx, &view, "new.txt"));

        send_watcher_paths(&tx, vec![watched_path.join("new.txt")]);
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(900));
        cx.run_until_parked();
        send_watcher_paths(&tx, vec![nested_path]);
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        assert!(view_has_entry(cx, &view, "new.txt"));
    }

    fn install_test_watcher(
        cx: &mut gpui::VisualTestContext,
        view: &Entity<ExplorerView>,
        watched_path: PathBuf,
    ) -> UnboundedSender<Vec<PathBuf>> {
        let (tx, rx) = mpsc::unbounded();
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.directory_watcher = Some(DirectoryWatcher {
                    _watcher: None,
                    _task: spawn_watcher_task(watched_path, rx, cx),
                });
            });
        });
        cx.run_until_parked();
        tx
    }

    fn send_watcher_paths(tx: &UnboundedSender<Vec<PathBuf>>, paths: Vec<PathBuf>) {
        tx.unbounded_send(paths).unwrap();
    }

    fn view_has_entry(
        cx: &mut gpui::VisualTestContext,
        view: &Entity<ExplorerView>,
        name: &str,
    ) -> bool {
        cx.read_entity(view, |view, _| {
            view.entries.iter().any(|entry| entry.name == name)
        })
    }
}
