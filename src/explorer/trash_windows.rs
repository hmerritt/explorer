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
}
#[implement(IFileOperationProgressSink)]
struct Sink {
    outcome: Arc<Mutex<Outcome>>,
    source: IShellItem,
}
impl Sink {
    fn record(
        &self,
        source: Ref<'_, IShellItem>,
        status: HRESULT,
        created: Ref<'_, IShellItem>,
        expect_created: bool,
    ) -> windows::core::Result<()> {
        // Folder operations also report descendants. Preserve the queued root's
        // destination rather than allowing the last child callback to replace it.
        let root = source.as_ref().is_some_and(|source| {
            // SAFETY: both shell descriptors remain alive on this COM worker.
            unsafe { source.Compare(&self.source, SICHINT_CANONICAL.0 as u32) }
                .is_ok_and(|order| order == 0)
        });
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
        self.outcome.lock().unwrap().item = Some(result);
        Ok(())
    }
}
#[allow(non_snake_case, unused_variables)]
impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, hrresult: windows::core::HRESULT) -> windows::core::Result<()> {
        if hrresult.is_err() {
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
    if Path::new(name).is_absolute() {
        for unit in &mut wide {
            if *unit == u16::from(b'/') {
                *unit = u16::from(b'\\');
            }
        }
        match Path::new(name).components().next() {
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

#[cfg(test)]
mod tests {
    use super::*;

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
