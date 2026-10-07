//! Shell operations stay on the service's dedicated COM worker.
use super::*;
use std::{
    ffi::OsStr,
    os::windows::ffi::{OsStrExt, OsStringExt},
    sync::Arc,
};
use windows::{
    Win32::{
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree},
        UI::Shell::{
            FOF_NO_UI, FOF_RENAMEONCOLLISION, FOFX_EARLYFAILURE, FileOperation, IFileOperation,
            IFileOperationProgressSink, IFileOperationProgressSink_Impl, IShellItem,
            SHCreateItemFromParsingName, SICHINT_CANONICAL, SIGDN_FILESYSPATH,
        },
    },
    core::{HRESULT, PCWSTR, Ref, implement},
};

#[derive(Default)]
struct Outcome {
    item: Option<Result<Option<PathBuf>, String>>,
    finish_error: Option<String>,
    started: bool,
    stale: bool,
    cancelled: bool,
    reported: bool,
}

#[derive(Clone)]
struct PurgeItem {
    entry: TrashEntry,
    context: PurgeContext,
    stale_abort: Arc<AtomicBool>,
}

#[implement(IFileOperationProgressSink)]
struct Sink {
    outcome: Arc<Mutex<Outcome>>,
    source: IShellItem,
    purge: Option<PurgeItem>,
}
impl Sink {
    fn is_root(&self, source: Ref<'_, IShellItem>) -> bool {
        source.as_ref().is_some_and(|source| {
            // SAFETY: both shell descriptors remain alive on this COM worker.
            unsafe { source.Compare(&self.source, SICHINT_CANONICAL.0 as u32) }
                .is_ok_and(|order| order == 0)
        })
    }

    fn record(
        &self,
        source: Ref<'_, IShellItem>,
        status: HRESULT,
        created: Ref<'_, IShellItem>,
        expect_created: bool,
    ) -> windows::core::Result<()> {
        // Folder operations also report descendants. Preserve the queued root's
        // destination rather than allowing the last child callback to replace it.
        let root = self.is_root(source);
        if !root {
            if status.is_err() || matches!(status.0, 0x270005 | 0x27000B | 0x270010) {
                self.outcome.lock().unwrap().finish_error = Some(format!(
                    "A shell descendant operation did not complete: {status:?}"
                ));
            }
            return Ok(());
        }
        let result = if status.is_err()
            || matches!(status.0, 0x270005 | 0x27000B | 0x270010)
            || (expect_created && status.0 == 0x270003)
        {
            Err(format!("Shell item operation did not complete: {status:?}"))
        } else if let Some(item) = created.as_ref() {
            shell_path(item).map(Some)
        } else if expect_created {
            Err("The shell did not report a recovered item.".into())
        } else {
            Ok(None)
        };
        let mut outcome = self.outcome.lock().unwrap();
        outcome.item = Some(result);
        if let Some(purge) = &self.purge {
            if !outcome.reported && outcome.started && !outcome.cancelled && !outcome.stale {
                outcome.reported = true;
                drop(outcome);
                purge.context.processed(&purge.entry.name.to_string_lossy());
            }
        }
        Ok(())
    }
}
#[allow(non_snake_case, unused_variables)]
impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, hrresult: windows::core::HRESULT) -> windows::core::Result<()> {
        if hrresult.is_err() && self.purge.is_none() {
            self.outcome.lock().unwrap().finish_error =
                Some(format!("Shell operation failed: {hrresult:?}"));
        }
        Ok(())
    }
    fn PreRenameItem(
        &self,
        dwflags: u32,
        psiitem: windows::core::Ref<'_, IShellItem>,
        psznewname: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostRenameItem(
        &self,
        dwflags: u32,
        psiitem: windows::core::Ref<'_, IShellItem>,
        psznewname: &windows::core::PCWSTR,
        hrrename: windows::core::HRESULT,
        psinewlycreated: windows::core::Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreMoveItem(
        &self,
        dwflags: u32,
        psiitem: windows::core::Ref<'_, IShellItem>,
        psidestinationfolder: windows::core::Ref<'_, IShellItem>,
        psznewname: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostMoveItem(
        &self,
        dwflags: u32,
        psiitem: windows::core::Ref<'_, IShellItem>,
        psidestinationfolder: windows::core::Ref<'_, IShellItem>,
        psznewname: &windows::core::PCWSTR,
        hrmove: windows::core::HRESULT,
        psinewlycreated: windows::core::Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        self.record(psiitem, hrmove, psinewlycreated, true)
    }
    fn PreCopyItem(
        &self,
        dwflags: u32,
        psiitem: windows::core::Ref<'_, IShellItem>,
        psidestinationfolder: windows::core::Ref<'_, IShellItem>,
        psznewname: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostCopyItem(
        &self,
        dwflags: u32,
        psiitem: windows::core::Ref<'_, IShellItem>,
        psidestinationfolder: windows::core::Ref<'_, IShellItem>,
        psznewname: &windows::core::PCWSTR,
        hrcopy: windows::core::HRESULT,
        psinewlycreated: windows::core::Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        self.record(psiitem, hrcopy, psinewlycreated, true)
    }
    fn PreDeleteItem(
        &self,
        dwflags: u32,
        psiitem: windows::core::Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        if let Some(purge) = &self.purge {
            if purge.context.cancel.load(Ordering::Relaxed) {
                self.outcome.lock().unwrap().cancelled = true;
                return Err(HRESULT(0x800704C7u32 as i32).into());
            }
            if self.is_root(psiitem) {
                purge.context.report(&purge.entry.name.to_string_lossy());
                if purge.context.cancel.load(Ordering::Relaxed) {
                    self.outcome.lock().unwrap().cancelled = true;
                    return Err(HRESULT(0x800704C7u32 as i32).into());
                }
                if fingerprint(&purge.entry.payload).ok().as_ref() != Some(&purge.entry.fingerprint)
                {
                    self.outcome.lock().unwrap().stale = true;
                    purge.stale_abort.store(true, Ordering::Relaxed);
                    return Err(HRESULT(0x80004005u32 as i32).into());
                }
                self.outcome.lock().unwrap().started = true;
            }
        }
        Ok(())
    }
    fn PostDeleteItem(
        &self,
        dwflags: u32,
        psiitem: windows::core::Ref<'_, IShellItem>,
        hrdelete: windows::core::HRESULT,
        psinewlycreated: windows::core::Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        self.record(psiitem, hrdelete, psinewlycreated, false)
    }
    fn PreNewItem(
        &self,
        dwflags: u32,
        psidestinationfolder: windows::core::Ref<'_, IShellItem>,
        psznewname: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostNewItem(
        &self,
        dwflags: u32,
        psidestinationfolder: windows::core::Ref<'_, IShellItem>,
        psznewname: &windows::core::PCWSTR,
        psztemplatename: &windows::core::PCWSTR,
        dwfileattributes: u32,
        hrnew: windows::core::HRESULT,
        psinewitem: windows::core::Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, iworktotal: u32, iworksofar: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}
fn shell_parsing_name(name: &OsStr) -> Vec<u16> {
    let mut wide: Vec<_> = name.encode_wide().collect();
    // Recycle Bin namespace descriptors are opaque. Only filesystem paths use
    // Windows separators and ordinary drive/UNC prefixes at the Shell boundary.
    let native = crate::os_paths::native_path(Path::new(name));
    let path = Path::new(&native);
    if path.is_absolute() {
        wide = native.encode_wide().collect();
        match path.components().next() {
            Some(std::path::Component::Prefix(prefix)) => match prefix.kind() {
                std::path::Prefix::VerbatimDisk(_) => {
                    wide.drain(..4);
                }
                std::path::Prefix::VerbatimUNC(_, _) => {
                    wide.splice(..8, [u16::from(b'\\'), u16::from(b'\\')]);
                }
                _ => {}
            },
            _ => {}
        }
    }
    wide.push(0);
    wide
}
fn shell_item(name: &OsStr) -> Result<IShellItem, String> {
    let wide = shell_parsing_name(name);
    // SAFETY: the name is terminated; this module runs in an initialized apartment.
    unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None) }.map_err(|e| e.to_string())
}
fn shell_path(item: &IShellItem) -> Result<PathBuf, String> {
    // SAFETY: shell owns the returned CoTaskMem string, which is freed after copying.
    unsafe {
        let value = item
            .GetDisplayName(SIGDN_FILESYSPATH)
            .map_err(|e| e.to_string())?;
        let mut len = 0;
        while *value.0.add(len) != 0 {
            len += 1;
        }
        let path = PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(
            value.0, len,
        )));
        CoTaskMemFree(Some(value.0.cast()));
        Ok(path)
    }
}
fn operation(
    source: &IShellItem,
) -> Result<
    (
        IFileOperation,
        IFileOperationProgressSink,
        Arc<Mutex<Outcome>>,
    ),
    String,
> {
    let outcome = Arc::new(Mutex::new(Outcome::default()));
    let sink: IFileOperationProgressSink = Sink {
        outcome: outcome.clone(),
        source: source.clone(),
        purge: None,
    }
    .into();
    // SAFETY: COM is initialized on the worker and both objects remain alive during execution.
    let operation: IFileOperation =
        unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| e.to_string())?;
    unsafe { operation.SetOperationFlags(FOF_NO_UI | FOFX_EARLYFAILURE | FOF_RENAMEONCOLLISION) }
        .map_err(|e| e.to_string())?;
    Ok((operation, sink, outcome))
}
fn perform(
    operation: &IFileOperation,
    outcome: &Arc<Mutex<Outcome>>,
) -> Result<Option<PathBuf>, String> {
    // SAFETY: queued shell objects and the progress sink remain alive during execution.
    unsafe {
        operation.PerformOperations().map_err(|e| e.to_string())?;
        if operation
            .GetAnyOperationsAborted()
            .map_err(|e| e.to_string())?
            .as_bool()
        {
            return Err("The shell operation was aborted.".into());
        }
    }
    let mut outcome = outcome.lock().unwrap();
    if let Some(error) = outcome.finish_error.take() {
        return Err(error);
    }
    outcome
        .item
        .take()
        .ok_or_else(|| "The shell did not report an item outcome.".to_string())?
}
/// Copy from a native bin descriptor to a private stage, then move verified
/// staged data to an explicitly named destination. Collisions never overwrite.
pub(super) fn shell_transfer(
    source: &OsStr,
    destination: &Path,
    moving: bool,
) -> Result<(), String> {
    if fs::symlink_metadata(destination).is_ok() {
        return Err("The restore destination changed; source retained.".into());
    }
    let parent = destination.parent().ok_or("Invalid destination.")?;
    let name = destination.file_name().ok_or("Invalid destination name.")?;
    let name: Vec<_> = name.encode_wide().chain(Some(0)).collect();
    let action = if moving {
        "Commit recovered item"
    } else {
        "Stage recovered item"
    };
    let source = shell_item(source).map_err(|e| format!("{action}: resolve source: {e}"))?;
    let parent = shell_item(parent.as_os_str())
        .map_err(|e| format!("{action}: resolve destination folder: {e}"))?;
    let (operation, sink, outcome) =
        operation(&source).map_err(|e| format!("{action}: initialize Shell operation: {e}"))?;
    // SAFETY: all shell items and terminated names remain valid through PerformOperations.
    unsafe {
        if moving {
            operation.MoveItem(&source, &parent, PCWSTR(name.as_ptr()), &sink)
        } else {
            operation.CopyItem(&source, &parent, PCWSTR(name.as_ptr()), &sink)
        }
    }
    .map_err(|e| format!("{action}: queue Shell operation: {e}"))?;
    let actual = perform(&operation, &outcome)
        .map_err(|e| format!("{action}: {e}"))?
        .ok_or("The shell did not return a recovered destination.")?;
    if !same_path(&actual, destination) {
        // The shell may number a collision that arrived after our check. This
        // private copy belongs to this operation; remove it and retain the bin source.
        remove_payload(&actual).map_err(|e| e.to_string())?;
        return Err("The restore destination changed; source retained.".into());
    }
    Ok(())
}
pub(super) fn shell_delete(source: &OsStr) -> Result<(), String> {
    let source = shell_item(source).map_err(|e| format!("Delete bin item: resolve source: {e}"))?;
    let (operation, sink, outcome) = operation(&source)
        .map_err(|e| format!("Delete bin item: initialize Shell operation: {e}"))?;
    // SAFETY: the native descriptor and sink remain alive until execution completes.
    unsafe { operation.DeleteItem(&source, &sink) }
        .map_err(|e| format!("Delete bin item: queue Shell operation: {e}"))?;
    perform(&operation, &outcome).map_err(|e| format!("Delete bin item: {e}"))?;
    Ok(())
}

enum DeleteDisposition {
    Completed,
    Failed(String),
    Cancelled,
    Retry,
}

fn delete_disposition(
    item: &TrashEntry,
    outcome: &Outcome,
    operation_error: Option<&str>,
    aborted: bool,
    cancelled: bool,
    stale_abort: bool,
) -> DeleteDisposition {
    if outcome.stale {
        return DeleteDisposition::Failed("The bin item changed; refresh and try again.".into());
    }
    // A successful root callback must also have removed the payload. Aggregate
    // errors from a later item must not erase an earlier verified completion.
    if matches!(outcome.item, Some(Ok(_))) {
        if let Some(error) = &outcome.finish_error {
            return DeleteDisposition::Failed(error.clone());
        }
        match fs::symlink_metadata(&item.payload) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return DeleteDisposition::Completed;
            }
            Err(error) => return DeleteDisposition::Failed(error.to_string()),
            Ok(_) => {}
        }
    }
    if outcome.cancelled || (cancelled && !outcome.started) {
        return DeleteDisposition::Cancelled;
    }
    if stale_abort && !outcome.started {
        return DeleteDisposition::Retry;
    }
    if let Some(error) = &outcome.finish_error {
        return DeleteDisposition::Failed(error.clone());
    }
    if let Some(Err(error)) = &outcome.item {
        return DeleteDisposition::Failed(error.clone());
    }
    DeleteDisposition::Failed(
        operation_error
            .unwrap_or(if aborted {
                "The shell operation was aborted."
            } else if outcome.item.is_some() {
                "The item could not be removed from the Recycle Bin."
            } else {
                "The shell did not report an item outcome."
            })
            .into(),
    )
}

struct QueuedDelete {
    entry: TrashEntry,
    // Keep each per-item sink and its source descriptor alive through execution.
    _sink: IFileOperationProgressSink,
    outcome: Arc<Mutex<Outcome>>,
}

#[cfg(test)]
thread_local! {
    static DELETE_BATCH_RUNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn delete_operation() -> Result<IFileOperation, String> {
    // SAFETY: this function runs on the initialized COM worker.
    unsafe {
        let operation: IFileOperation =
            CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)
                .map_err(|error| error.to_string())?;
        operation
            .SetOperationFlags(FOF_NO_UI)
            .map_err(|error| error.to_string())?;
        Ok(operation)
    }
}

pub(super) fn shell_delete_batch(
    mut pending: Vec<TrashEntry>,
    context: PurgeContext,
) -> BatchResult {
    let mut result = BatchResult::default();
    while !pending.is_empty() {
        if context.cancel.load(Ordering::Relaxed) {
            result.cancelled = true;
            result
                .cancelled_items
                .extend(pending.iter().map(|item| item.id.clone()));
            break;
        }
        let operation = match delete_operation() {
            Ok(operation) => operation,
            Err(error) => {
                for item in &pending {
                    result.purge_failed(item, error.clone());
                    context.processed(&item.name.to_string_lossy());
                }
                break;
            }
        };
        let stale_abort = Arc::new(AtomicBool::new(false));
        let mut queued = Vec::with_capacity(pending.len());
        for (index, entry) in pending.iter().enumerate() {
            if context.cancel.load(Ordering::Relaxed) {
                result.cancelled = true;
                result
                    .cancelled_items
                    .extend(pending[index..].iter().map(|item| item.id.clone()));
                break;
            }
            let queue = || -> Result<QueuedDelete, String> {
                if fingerprint(&entry.payload).ok().as_ref() != Some(&entry.fingerprint) {
                    return Err("The bin item changed; refresh and try again.".into());
                }
                let source = shell_item(&entry.native.id)?;
                let outcome = Arc::new(Mutex::new(Outcome::default()));
                let sink: IFileOperationProgressSink = Sink {
                    outcome: outcome.clone(),
                    source: source.clone(),
                    purge: Some(PurgeItem {
                        entry: entry.clone(),
                        context: context.clone(),
                        stale_abort: stale_abort.clone(),
                    }),
                }
                .into();
                // SAFETY: the descriptor and sink are retained until the operation returns.
                unsafe { operation.DeleteItem(&source, &sink) }
                    .map_err(|error| error.to_string())?;
                Ok(QueuedDelete {
                    entry: entry.clone(),
                    _sink: sink,
                    outcome,
                })
            };
            match queue() {
                Ok(item) => queued.push(item),
                Err(error) => {
                    result.purge_failed(entry, error);
                    context.processed(&entry.name.to_string_lossy());
                }
            }
        }
        let (operation_error, aborted) =
            if queued.is_empty() || context.cancel.load(Ordering::Relaxed) {
                (None, false)
            } else {
                // Always ask about aborts, even if PerformOperations returns an error.
                // SAFETY: all queued objects and per-item sinks are still alive.
                #[cfg(test)]
                DELETE_BATCH_RUNS.with(|runs| runs.set(runs.get() + 1));
                let performed = unsafe { operation.PerformOperations() };
                let aborted = unsafe { operation.GetAnyOperationsAborted() };
                let error = performed
                    .err()
                    .map(|error| error.to_string())
                    .or_else(|| aborted.as_ref().err().map(|error| error.to_string()));
                (error, aborted.map(|value| value.as_bool()).unwrap_or(true))
            };
        pending.clear();
        let cancelled = context.cancel.load(Ordering::Relaxed);
        result.cancelled |= cancelled;
        for item in queued {
            let outcome = item.outcome.lock().unwrap();
            let disposition = delete_disposition(
                &item.entry,
                &outcome,
                operation_error.as_deref(),
                aborted,
                cancelled,
                stale_abort.load(Ordering::Relaxed),
            );
            let processed = matches!(
                disposition,
                DeleteDisposition::Completed | DeleteDisposition::Failed(_)
            );
            match disposition {
                DeleteDisposition::Completed => result.completed.push(item.entry.id.clone()),
                DeleteDisposition::Failed(error) => result.purge_failed(&item.entry, error),
                DeleteDisposition::Cancelled => result.cancelled_items.push(item.entry.id.clone()),
                DeleteDisposition::Retry => pending.push(item.entry.clone()),
            }
            let report = processed && !outcome.reported;
            drop(outcome);
            if report {
                context.processed(&item.entry.name.to_string_lossy());
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(root: &Path, name: &str) -> TrashEntry {
        let path = root.join(name);
        fs::write(&path, b"owned batch deletion fixture").unwrap();
        let mut entry = fixture_entry(name, &path, None);
        entry.native.id = path.into_os_string();
        entry
    }

    fn context(total: usize, cancel: Arc<AtomicBool>, progress: PurgeProgress) -> PurgeContext {
        PurgeContext {
            cancel,
            progress,
            done: Arc::new(AtomicUsize::new(0)),
            total,
        }
    }

    #[test]
    fn batch_preserves_success_before_a_later_abort() {
        let root = tempfile::tempdir().unwrap();
        let item = fixture(root.path(), "success.txt");
        fs::remove_file(&item.payload).unwrap();
        let outcome = Outcome {
            item: Some(Ok(None)),
            started: true,
            ..Default::default()
        };
        assert!(matches!(
            delete_disposition(&item, &outcome, Some("later failure"), true, true, true),
            DeleteDisposition::Completed
        ));
    }

    #[test]
    fn batch_rejects_descendant_failures_and_retained_payloads() {
        let root = tempfile::tempdir().unwrap();
        let item = fixture(root.path(), "retained.txt");
        let outcome = Outcome {
            item: Some(Ok(None)),
            started: true,
            ..Default::default()
        };
        assert!(matches!(
            delete_disposition(&item, &outcome, None, false, false, false),
            DeleteDisposition::Failed(_)
        ));
        fs::remove_file(&item.payload).unwrap();
        let outcome = Outcome {
            finish_error: Some("descendant failure".into()),
            ..outcome
        };
        assert!(
            matches!(delete_disposition(&item, &outcome, None, false, false, false), DeleteDisposition::Failed(error) if error == "descendant failure")
        );
    }

    #[test]
    fn batch_missing_callbacks_fail_without_retrying_system_aborts() {
        let root = tempfile::tempdir().unwrap();
        let item = fixture(root.path(), "missing.txt");
        let outcome = Outcome::default();
        assert!(
            matches!(delete_disposition(&item, &outcome, None, false, false, false), DeleteDisposition::Failed(error) if error.contains("outcome"))
        );
        assert!(
            matches!(delete_disposition(&item, &outcome, None, true, false, false), DeleteDisposition::Failed(error) if error.contains("aborted"))
        );
        assert!(
            matches!(delete_disposition(&item, &outcome, Some("execution failed"), true, false, false), DeleteDisposition::Failed(error) if error == "execution failed")
        );
    }

    #[test]
    fn batch_retries_only_unstarted_items_after_identity_abort() {
        let root = tempfile::tempdir().unwrap();
        let item = fixture(root.path(), "pending.txt");
        let untouched = Outcome {
            item: Some(Err("queue aborted".into())),
            ..Default::default()
        };
        assert!(matches!(
            delete_disposition(&item, &untouched, None, true, false, true),
            DeleteDisposition::Retry
        ));
        let started = Outcome {
            started: true,
            ..untouched
        };
        assert!(matches!(
            delete_disposition(&item, &started, None, true, false, true),
            DeleteDisposition::Failed(_)
        ));
        let stale = Outcome {
            stale: true,
            ..Default::default()
        };
        assert!(
            matches!(delete_disposition(&item, &stale, None, true, false, true), DeleteDisposition::Failed(error) if error.contains("changed"))
        );
    }

    #[test]
    fn batch_cancellation_retains_pending_items_and_item_errors() {
        let root = tempfile::tempdir().unwrap();
        let item = fixture(root.path(), "cancelled.txt");
        let outcome = Outcome::default();
        assert!(matches!(
            delete_disposition(&item, &outcome, None, true, true, true),
            DeleteDisposition::Cancelled
        ));
        let outcome = Outcome {
            started: true,
            cancelled: true,
            ..Default::default()
        };
        assert!(matches!(
            delete_disposition(&item, &outcome, None, true, true, false),
            DeleteDisposition::Cancelled
        ));
        let outcome = Outcome {
            started: true,
            item: Some(Err("access denied".into())),
            ..Default::default()
        };
        assert!(
            matches!(delete_disposition(&item, &outcome, None, true, true, false), DeleteDisposition::Failed(error) if error == "access denied")
        );
    }

    #[test]
    #[ignore = "native COM deletion callbacks; operates only on owned temporary files"]
    fn native_batch_cancellation_and_identity_changes() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap();
        let one = fixture(root.path(), "one.txt");
        let two = fixture(root.path(), "two.txt");
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_after_first = cancel.clone();
        let result = native_worker(|| {
            Ok(shell_delete_batch(
                vec![one.clone(), two.clone()],
                context(
                    2,
                    cancel,
                    Arc::new(move |done, _, _| {
                        if done == 1 {
                            cancel_after_first.store(true, Ordering::Relaxed);
                        }
                    }),
                ),
            ))
        })
        .unwrap();
        assert!(result.cancelled);
        assert_eq!(result.completed.len(), 1);
        assert_eq!(result.cancelled_items.len(), 1);
        assert!(result.failures.is_empty(), "{:?}", result.failures);
        assert_eq!(
            usize::from(one.payload.exists()) + usize::from(two.payload.exists()),
            1
        );

        let before = fixture(root.path(), "before-locked.txt");
        let locked = fixture(root.path(), "locked.txt");
        let after = fixture(root.path(), "after-locked.txt");
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&locked.payload)
            .unwrap();
        let result = native_worker(|| {
            Ok(shell_delete_batch(
                vec![before.clone(), locked.clone(), after.clone()],
                context(3, Arc::new(AtomicBool::new(false)), Arc::new(|_, _, _| {})),
            ))
        })
        .unwrap();
        assert_eq!(result.failed, vec![locked.id]);
        assert_eq!(result.completed.len(), 2);
        assert!(!before.payload.exists());
        assert!(locked.payload.exists());
        assert!(!after.payload.exists());
        assert!(!result.cancelled);
        drop(lock);

        let changed = fixture(root.path(), "changed.txt");
        let pending = fixture(root.path(), "pending.txt");
        let replacement_path = changed.payload.clone();
        let saved = root.path().join("saved.txt");
        let replace_once = Arc::new(AtomicBool::new(false));
        let result = native_worker(|| {
            Ok(shell_delete_batch(
                vec![changed.clone(), pending.clone()],
                context(
                    2,
                    Arc::new(AtomicBool::new(false)),
                    Arc::new(move |_, _, name| {
                        if name == "changed.txt" && !replace_once.swap(true, Ordering::Relaxed) {
                            fs::rename(&replacement_path, &saved).unwrap();
                            fs::write(&replacement_path, b"replacement").unwrap();
                        }
                    }),
                ),
            ))
        })
        .unwrap();
        assert_eq!(result.failed, vec![changed.id]);
        assert_eq!(result.completed, vec![pending.id]);
        assert_eq!(fs::read(changed.payload).unwrap(), b"replacement");
        assert!(!pending.payload.exists());
    }

    #[test]
    #[ignore = "native batch performance smoke test; only purges its own bin identities"]
    fn native_bin_batch_purge_performance() {
        use std::time::Instant;
        let root = tempfile::tempdir().unwrap();
        // A small baseline avoids spending minutes on the old per-item path.
        let baseline: Vec<_> = (0..16)
            .map(|index| fixture(root.path(), &format!("baseline-{index}.txt")).payload)
            .collect();
        let mut batch: Vec<_> = (0..256)
            .map(|index| fixture(root.path(), &format!("batch-{index}.txt")).payload)
            .collect();
        let folder = root.path().join("batch-folder");
        fs::create_dir_all(folder.join("nested/empty")).unwrap();
        fs::write(folder.join("nested/child.txt"), b"owned folder fixture").unwrap();
        batch.push(folder);
        let sentinel = fixture(root.path(), "unselected.txt").payload;
        let mut paths = baseline.clone();
        paths.extend(batch.iter().cloned());
        paths.push(sentinel.clone());
        native_worker(|| trash::delete_all(&paths).map_err(|error| error.to_string())).unwrap();
        let entries = list().unwrap();
        let find = |path: &Path| {
            entries
                .iter()
                .find(|item| {
                    item.original_path
                        .as_ref()
                        .is_some_and(|original| same_path(original, path))
                })
                .unwrap()
                .clone()
        };
        let baseline_ids: Vec<_> = baseline.iter().map(|path| find(path).id).collect();
        let batch_ids: Vec<_> = batch.iter().map(|path| find(path).id).collect();
        let sentinel = find(&sentinel);
        let start = Instant::now();
        native_worker(|| {
            let _guard = OPERATIONS.lock().unwrap_or_else(|error| error.into_inner());
            for id in &baseline_ids {
                let item = list_inner()?
                    .into_iter()
                    .find(|item| &item.id == id)
                    .ok_or("Missing baseline fixture")?;
                platform_purge(&item)?;
            }
            list_inner()?;
            Ok(())
        })
        .unwrap();
        let baseline_elapsed = start.elapsed();
        let reports = Arc::new(Mutex::new(Vec::new()));
        let reported = reports.clone();
        let start = Instant::now();
        let (result, operations) = native_worker(|| {
            DELETE_BATCH_RUNS.with(|runs| runs.set(0));
            let _guard = OPERATIONS.lock().unwrap_or_else(|error| error.into_inner());
            let result = purge_with(
                &NativeBackend,
                batch_ids.clone(),
                Arc::new(AtomicBool::new(false)),
                move |done, total, _| {
                    reported.lock().unwrap().push((done, total));
                },
            );
            Ok((result, DELETE_BATCH_RUNS.with(|runs| runs.get())))
        })
        .unwrap();
        let batch_elapsed = start.elapsed();
        eprintln!(
            "Old path: {} items in {baseline_elapsed:?} ({:.2} ms/item); batch path: {} items in {batch_elapsed:?} ({:.2} ms/item), {operations} Shell execution(s)",
            baseline_ids.len(),
            baseline_elapsed.as_secs_f64() * 1000.0 / baseline_ids.len() as f64,
            batch_ids.len(),
            batch_elapsed.as_secs_f64() * 1000.0 / batch_ids.len() as f64
        );
        assert!(result.failures.is_empty(), "{:?}", result.failures);
        assert_eq!(result.completed.len(), batch_ids.len());
        assert_eq!(operations, 1);
        assert!(result.undo.paths.is_empty());
        assert!(sentinel.payload.exists());
        let reports = reports.lock().unwrap();
        assert_eq!(reports.last(), Some(&(batch_ids.len(), batch_ids.len())));
        assert!(reports.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        // Only this test's sentinel is purged during cleanup.
        let cleanup = purge(
            vec![sentinel.id],
            Arc::new(AtomicBool::new(false)),
            |_, _, _| {},
        );
        assert!(cleanup.failures.is_empty(), "{:?}", cleanup.failures);
        assert_eq!(cleanup.completed.len(), 1);
    }

    #[test]
    fn shell_names_normalize_filesystem_paths_only() {
        for (input, expected) in [
            (r"C:/folder\child/é.txt", r"C:\folder\child\é.txt"),
            (r"\\?\C:\folder\é.txt", r"C:\folder\é.txt"),
            (r"\\?\UNC\server\share\folder", r"\\server\share\folder"),
            (r"\\server\share/folder", r"\\server\share\folder"),
            (
                r"::{645FF040-5081-101B-9F08-00AA002F954E}\item/opaque",
                r"::{645FF040-5081-101B-9F08-00AA002F954E}\item/opaque",
            ),
        ] {
            assert_eq!(
                shell_parsing_name(OsStr::new(input)),
                expected.encode_utf16().chain(Some(0)).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn shell_names_preserve_non_separator_utf16() {
        let input = OsString::from_wide(&[67, 58, 47, 0xd800, 47, 0xdc00]);
        assert_eq!(
            shell_parsing_name(&input),
            [67, 58, 92, 0xd800, 92, 0xdc00, 0]
        );
    }
}
