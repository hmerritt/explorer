//! Cooperative controls shared by the existing synchronous filesystem engines.
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        Arc, Condvar, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Default)]
struct PauseState {
    requested: bool,
    workers: usize,
    waiting: usize,
}

pub(super) struct OperationControl {
    pub cancel: Arc<AtomicBool>,
    pub terminate: Arc<AtomicBool>,
    state: Mutex<PauseState>,
    pause_requested: AtomicBool,
    wake: Condvar,
    pub pause_available: AtomicBool,
    items_report: Mutex<Option<ItemsReport>>,
}
type ItemsReport = Arc<dyn Fn(usize, usize, &str) + Send + Sync>;

fn registry() -> &'static Mutex<HashMap<usize, Weak<OperationControl>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<usize, Weak<OperationControl>>>> = OnceLock::new();
    REGISTRY.get_or_init(Mutex::default)
}

thread_local! { static CURRENT: RefCell<Option<Arc<OperationControl>>> = const { RefCell::new(None) }; }

impl OperationControl {
    pub fn new() -> Arc<Self> {
        let control = Arc::new(Self {
            cancel: Arc::new(AtomicBool::new(false)),
            terminate: Arc::new(AtomicBool::new(false)),
            state: Mutex::default(),
            pause_requested: AtomicBool::new(false),
            wake: Condvar::new(),
            pause_available: AtomicBool::new(true),
            items_report: Mutex::default(),
        });
        registry().lock().unwrap().insert(
            Arc::as_ptr(&control.cancel) as usize,
            Arc::downgrade(&control),
        );
        control
    }

    pub fn pause(&self) {
        self.state.lock().unwrap().requested = true;
        self.pause_requested.store(true, Ordering::Release);
    }
    pub fn resume(&self) {
        self.state.lock().unwrap().requested = false;
        self.pause_requested.store(false, Ordering::Release);
        self.wake.notify_all();
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
        self.resume();
    }
    pub fn pausing(&self) -> bool {
        self.pause_requested.load(Ordering::Acquire)
    }
    pub fn paused(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.requested && state.workers > 0 && state.waiting == state.workers
    }
    pub fn enter(self: &Arc<Self>) -> Worker {
        self.state.lock().unwrap().workers += 1;
        let previous = CURRENT.with(|current| current.replace(Some(self.clone())));
        Worker {
            control: self.clone(),
            previous,
        }
    }
    pub fn set_items_report(&self, report: ItemsReport) {
        *self.items_report.lock().unwrap() = Some(report);
    }
    fn checkpoint(&self) -> bool {
        if !self.pause_requested.load(Ordering::Acquire) {
            return self.cancel.load(Ordering::Acquire);
        }
        let mut state = self.state.lock().unwrap();
        if state.requested && !self.cancel.load(Ordering::Acquire) {
            state.waiting += 1;
            while state.requested && !self.cancel.load(Ordering::Acquire) {
                state = self.wake.wait(state).unwrap();
            }
            state.waiting -= 1;
        }
        self.cancel.load(Ordering::Acquire)
    }
}

pub(super) fn current() -> Option<Arc<OperationControl>> {
    CURRENT.with(|current| current.borrow().clone())
}

/// Hand acknowledgment to child workers while their caller waits without doing I/O.
pub(super) struct Suspended(Arc<OperationControl>);
impl Drop for Suspended {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().workers += 1;
        CURRENT.with(|current| current.replace(Some(self.0.clone())));
    }
}
pub(super) fn suspend_current() -> Option<Suspended> {
    CURRENT
        .with(|current| current.replace(None))
        .map(|control| {
            control.state.lock().unwrap().workers -= 1;
            Suspended(control)
        })
}

/// Temporarily expose safe boundaries inside an otherwise indivisible item.
pub(super) struct BoundariesAvailable(Arc<OperationControl>, bool);
impl Drop for BoundariesAvailable {
    fn drop(&mut self) {
        self.0.pause_available.store(self.1, Ordering::Release);
    }
}
pub(super) fn boundaries_available() -> Option<BoundariesAvailable> {
    current().map(|control| {
        let previous = control.pause_available.swap(true, Ordering::AcqRel);
        BoundariesAvailable(control, previous)
    })
}

pub(super) fn report_items(done: usize, total: usize, name: &str) {
    if let Some(control) = current() {
        control
            .pause_available
            .store(total.saturating_sub(done) > 1, Ordering::Release);
        let report = control.items_report.lock().unwrap().clone();
        if let Some(report) = report {
            report(done, total, name);
        }
    }
}

impl Drop for OperationControl {
    fn drop(&mut self) {
        registry()
            .lock()
            .unwrap()
            .remove(&(Arc::as_ptr(&self.cancel) as usize));
    }
}

pub(super) struct Worker {
    control: Arc<OperationControl>,
    previous: Option<Arc<OperationControl>>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        CURRENT.with(|current| current.replace(self.previous.take()));
        self.control.state.lock().unwrap().workers -= 1;
    }
}

// Existing APIs pass the cancellation flag. Resolve its control without changing
// those APIs; the thread context keeps hot copy/index loops off the registry lock.
fn control_for(cancel: &AtomicBool) -> Option<Arc<OperationControl>> {
    if let Some(control) = current()
        && std::ptr::eq(Arc::as_ptr(&control.cancel), cancel)
    {
        return Some(control);
    }
    registry()
        .lock()
        .unwrap()
        .get(&(cancel as *const AtomicBool as usize))
        .and_then(Weak::upgrade)
}

pub(super) fn enter_for_cancel(cancel: &AtomicBool) -> Option<Worker> {
    let control = control_for(cancel);
    control.and_then(|control| {
        let already_entered = CURRENT.with(|current| {
            current
                .borrow()
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &control))
        });
        (!already_entered).then(|| control.enter())
    })
}

pub(super) fn request_cancel(cancel: &AtomicBool) {
    let control = control_for(cancel);
    if let Some(control) = control {
        control.cancel();
    } else {
        cancel.store(true, Ordering::Release);
    }
}

/// Unregistered callers retain their original cancellation behavior.
pub(super) fn cancelled(cancel: &AtomicBool) -> bool {
    let control = control_for(cancel);
    control.map_or_else(
        || cancel.load(Ordering::Acquire),
        |control| control.checkpoint(),
    )
}

pub(super) fn current_checkpoint() -> bool {
    let control = CURRENT.with(|current| current.borrow().clone());
    control.is_some_and(|control| control.checkpoint())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn pause_acknowledges_all_workers_and_cancel_wakes_them() {
        let control = OperationControl::new();
        control.pause();
        let (tx, rx) = mpsc::channel();
        std::thread::scope(|scope| {
            for _ in 0..2 {
                let control = control.clone();
                let tx = tx.clone();
                scope.spawn(move || {
                    let _worker = control.enter();
                    tx.send(()).unwrap();
                    assert!(cancelled(&control.cancel));
                });
            }
            rx.recv_timeout(Duration::from_secs(2)).unwrap();
            rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while !control.paused() {
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
            control.cancel();
        });
        assert!(!control.paused());
    }

    #[test]
    fn resume_continues_the_same_worker() {
        let control = OperationControl::new();
        control.pause();
        std::thread::scope(|scope| {
            let other = control.clone();
            let worker = scope.spawn(move || {
                let _worker = other.enter();
                cancelled(&other.cancel)
            });
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while !control.paused() {
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
            control.resume();
            assert!(!worker.join().unwrap());
        });
    }
    fn wait_paused(control: &OperationControl) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !control.paused() {
            if std::time::Instant::now() >= deadline {
                control.cancel();
                panic!("workers did not acknowledge pause");
            }
            std::thread::yield_now();
        }
    }

    #[test]
    fn copying_indexing_and_verification_resume_in_place() {
        use crate::explorer::filesystem::{
            self, ConflictChoice, FileOperationPhase, PreparedFileOperation,
        };
        for phase in [
            FileOperationPhase::Copying,
            FileOperationPhase::Indexing,
            FileOperationPhase::Verifying,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("source.bin");
            let destination = temp.path().join("destination");
            std::fs::create_dir(&destination).unwrap();
            let bytes = vec![17u8; 2 * 1024 * 1024];
            std::fs::write(&source, &bytes).unwrap();
            if phase != FileOperationPhase::Copying {
                std::fs::write(destination.join("source.bin"), vec![19u8; bytes.len()]).unwrap();
            }
            let control = OperationControl::new();
            let other = control.clone();
            let (tx, rx) = mpsc::channel();
            std::thread::scope(|scope| {
                let worker = scope.spawn(|| {
                    let _participant = other.enter();
                    let job = match filesystem::prepare_copy_paths_to_directory_for_paste(
                        std::slice::from_ref(&source),
                        &destination,
                    )
                    .unwrap()
                    {
                        PreparedFileOperation::Ready(job) => job,
                        PreparedFileOperation::Conflicts(batch) => batch.into_job(),
                    };
                    let mut requested = false;
                    filesystem::execute_file_operation_with_progress(
                        job,
                        ConflictChoice::Replace,
                        other.cancel.clone(),
                        other.terminate.clone(),
                        |progress| {
                            if progress.phase == phase && !requested {
                                requested = true;
                                other.pause();
                                tx.send(()).unwrap();
                            }
                        },
                    )
                    .unwrap()
                });
                rx.recv_timeout(Duration::from_secs(5)).unwrap();
                wait_paused(&control);
                assert!(!control.cancel.load(Ordering::Acquire));
                control.resume();
                let summary = worker.join().unwrap();
                assert_eq!(summary.destination_paths.len(), 1);
            });
            assert_eq!(
                std::fs::read(destination.join("source.bin")).unwrap(),
                bytes
            );
        }
    }

    #[test]
    fn parallel_copy_cancel_wakes_all_workers_and_retains_sources() {
        use crate::explorer::filesystem::{
            self, ConflictChoice, FileOperationError, FileOperationPhase, PreparedFileOperation,
        };
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("destination");
        std::fs::create_dir(&destination).unwrap();
        let sources: Vec<_> = (0..4)
            .map(|index| {
                let path = temp.path().join(format!("file-{index}.bin"));
                std::fs::write(&path, vec![index as u8; 4 * 1024 * 1024]).unwrap();
                path
            })
            .collect();
        let control = OperationControl::new();
        let other = control.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                let _participant = other.enter();
                let PreparedFileOperation::Ready(job) =
                    filesystem::prepare_copy_paths_to_directory_for_paste(&sources, &destination)
                        .unwrap()
                else {
                    panic!("unexpected conflict")
                };
                let mut requested = false;
                filesystem::execute_file_operation_with_progress(
                    job,
                    ConflictChoice::Replace,
                    other.cancel.clone(),
                    other.terminate.clone(),
                    |progress| {
                        if progress.phase == FileOperationPhase::Copying
                            && progress.copied_bytes > 0
                            && !requested
                        {
                            requested = true;
                            other.pause();
                            tx.send(()).unwrap();
                        }
                    },
                )
            });
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
            wait_paused(&control);
            control.cancel();
            assert!(matches!(
                worker.join().unwrap(),
                Err(FileOperationError::Cancelled)
            ));
        });
        assert!(sources.iter().all(|path| path.exists()));
    }
    #[test]
    fn cross_volume_move_keeps_source_until_verification_resumes() {
        use crate::explorer::filesystem::{
            self, ConflictChoice, FileOperationPhase, PreparedFileOperation,
        };
        let temp = tempfile::tempdir().unwrap();
        let source_directory = temp.path().join("source");
        let destination = temp.path().join("destination");
        std::fs::create_dir(&source_directory).unwrap();
        std::fs::create_dir(&destination).unwrap();
        let source = source_directory.join("file.bin");
        let bytes = vec![9u8; 1024 * 1024];
        std::fs::write(&source, &bytes).unwrap();
        std::fs::write(destination.join("file.bin"), vec![11u8; bytes.len()]).unwrap();
        let _source_volume = filesystem::set_test_path_volume_key(
            &source_directory,
            Some("operation-source-volume"),
        );
        let _destination_volume = filesystem::set_test_path_volume_key(
            &destination,
            Some("operation-destination-volume"),
        );
        let control = OperationControl::new();
        let worker_control = control.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                let _participant = worker_control.enter();
                let prepared = filesystem::prepare_move_paths_to_directory(
                    std::slice::from_ref(&source),
                    &destination,
                )
                .unwrap();
                let job = match prepared {
                    PreparedFileOperation::Ready(job) => job,
                    PreparedFileOperation::Conflicts(conflicts) => conflicts.into_job(),
                };
                let mut requested = false;
                filesystem::execute_file_operation_with_progress(
                    job,
                    ConflictChoice::Replace,
                    worker_control.cancel.clone(),
                    worker_control.terminate.clone(),
                    |progress| {
                        if progress.phase == FileOperationPhase::Verifying && !requested {
                            requested = true;
                            worker_control.pause();
                            tx.send(()).unwrap();
                        }
                    },
                )
                .unwrap()
            });
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
            wait_paused(&control);
            assert!(source.exists());
            control.resume();
            assert_eq!(
                worker.join().unwrap().moved_source_paths,
                vec![source.clone()]
            );
        });
        assert!(!source.exists());
        assert_eq!(std::fs::read(destination.join("file.bin")).unwrap(), bytes);
    }

    #[test]
    fn compression_pause_keeps_incomplete_archive_until_cancel_cleanup() {
        use crate::explorer::filesystem::{
            self, ConflictChoice, FileOperationError, FileOperationPhase, PreparedFileOperation,
        };
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("file.bin");
        std::fs::write(&source, vec![7u8; 8 * 1024 * 1024]).unwrap();
        let control = OperationControl::new();
        let worker_control = control.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                let _participant = worker_control.enter();
                let PreparedFileOperation::Ready(job) =
                    filesystem::prepare_compress_paths(std::slice::from_ref(&source)).unwrap()
                else {
                    panic!("unexpected conflict")
                };
                let mut requested = false;
                filesystem::execute_file_operation_with_progress(
                    job,
                    ConflictChoice::Replace,
                    worker_control.cancel.clone(),
                    worker_control.terminate.clone(),
                    |progress| {
                        if progress.phase == FileOperationPhase::Compressing
                            && progress.copied_bytes > 0
                            && !requested
                        {
                            requested = true;
                            worker_control.pause();
                            tx.send(()).unwrap();
                        }
                    },
                )
            });
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
            wait_paused(&control);
            let archives = || {
                std::fs::read_dir(temp.path())
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry
                            .path()
                            .extension()
                            .is_some_and(|extension| extension == "zip")
                    })
                    .count()
            };
            assert_eq!(archives(), 1);
            control.cancel();
            assert!(matches!(
                worker.join().unwrap(),
                Err(FileOperationError::Cancelled)
            ));
            assert_eq!(archives(), 0);
        });
        assert!(source.exists());
    }
}
