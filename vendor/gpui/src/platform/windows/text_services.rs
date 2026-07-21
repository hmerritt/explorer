use std::{cell::{Cell, RefCell}, sync::{Arc, Mutex}};

use windows::{
    Win32::{
        Foundation::{E_NOTIMPL, HWND, POINT, RECT, S_OK},
        System::{
            Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, FORMATETC, IDataObject},
            Variant::VARIANT,
        },
        UI::{TextServices::*, WindowsAndMessaging::GetWindowRect},
    },
    core::{BOOL, Error, GUID, HRESULT, IUnknown, Interface, PCWSTR, PWSTR, Ref, Result, implement},
};

use super::SystemCaretGeometry;

thread_local! {
    static THREAD_MANAGER: RefCell<Option<(ITfThreadMgr, u32)>> = const { RefCell::new(None) };
}

fn thread_manager() -> Result<(ITfThreadMgr, u32)> {
    THREAD_MANAGER.with_borrow_mut(|slot| {
        if let Some(manager) = slot.as_ref() {
            return Ok(manager.clone());
        }

        let manager: ITfThreadMgr = unsafe {
            CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)?
        };
        let client_id = unsafe { manager.Activate()? };
        *slot = Some((manager.clone(), client_id));
        Ok((manager, client_id))
    })
}

fn not_implemented<T>() -> Result<T> { Err(Error::from_hresult(E_NOTIMPL)) }

#[implement(ITfContextOwner, ITextStoreACP)]
struct CaretContextOwner {
    hwnd: isize,
    caret: Arc<Mutex<Option<RECT>>>,
    sink: RefCell<Option<ITextStoreACPSink>>,
    lock_flags: Cell<u32>,
}

impl CaretContextOwner {
    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as _)
    }
}

#[allow(non_snake_case)]
impl ITfContextOwner_Impl for CaretContextOwner_Impl {
    fn GetACPFromPoint(&self, _point: *const POINT, _flags: u32) -> Result<i32> {
        Ok(0)
    }

    fn GetTextExt(
        &self,
        _start: i32,
        _end: i32,
        rect: *mut RECT,
        clipped: *mut BOOL,
    ) -> Result<()> {
        let caret = self.caret.lock().ok().and_then(|caret| *caret).unwrap_or_default();
        if !rect.is_null() {
            unsafe { rect.write(caret) };
        }
        if !clipped.is_null() {
            unsafe { clipped.write(false.into()) };
        }
        Ok(())
    }

    fn GetScreenExt(&self) -> Result<RECT> {
        let mut rect = RECT::default();
        unsafe { GetWindowRect(self.hwnd(), &mut rect)? };
        Ok(rect)
    }

    fn GetStatus(&self) -> Result<TS_STATUS> {
        Ok(TS_STATUS::default())
    }

    fn GetWnd(&self) -> Result<HWND> {
        Ok(self.hwnd())
    }

    fn GetAttribute(&self, _attribute: *const windows::core::GUID) -> Result<VARIANT> {
        Ok(VARIANT::default())
    }
}

#[allow(non_snake_case)]
impl ITextStoreACP_Impl for CaretContextOwner_Impl {
    fn AdviseSink(&self, riid: *const GUID, punk: Ref<'_, IUnknown>, _mask: u32) -> Result<()> {
        if riid.is_null() || unsafe { *riid } != ITextStoreACPSink::IID { return not_implemented(); }
        self.sink.replace(Some(punk.ok()?.cast()?));
        Ok(())
    }
    fn UnadviseSink(&self, _punk: Ref<'_, IUnknown>) -> Result<()> { self.sink.replace(None); Ok(()) }
    fn RequestLock(&self, flags: u32) -> Result<HRESULT> {
        self.lock_flags.set(flags);
        if let Some(sink) = self.sink.borrow().as_ref() { unsafe { sink.OnLockGranted(TEXT_STORE_LOCK_FLAGS(flags))? }; }
        self.lock_flags.set(0);
        Ok(S_OK)
    }
    fn GetStatus(&self) -> Result<TS_STATUS> { Ok(TS_STATUS::default()) }
    fn QueryInsert(&self, start: i32, _end: i32, count: u32, result_start: *mut i32, result_end: *mut i32) -> Result<()> {
        unsafe {
            if !result_start.is_null() { result_start.write(start.clamp(0, 1)); }
            if !result_end.is_null() { result_end.write((start + count as i32).clamp(0, 1)); }
        }
        Ok(())
    }
    fn GetSelection(&self, _index: u32, count: u32, selection: *mut TS_SELECTION_ACP, fetched: *mut u32) -> Result<()> {
        let available = u32::from(count > 0 && !selection.is_null());
        unsafe {
            if available != 0 { selection.write(TS_SELECTION_ACP { acpStart: 0, acpEnd: 0, style: TS_SELECTIONSTYLE { ase: TS_AE_END, fInterimChar: false.into() } }); }
            if !fetched.is_null() { fetched.write(available); }
        }
        Ok(())
    }
    fn SetSelection(&self, _count: u32, _selection: *const TS_SELECTION_ACP) -> Result<()> { Ok(()) }
    fn GetText(&self, start: i32, _end: i32, plain: PWSTR, plain_capacity: u32, plain_count: *mut u32, runs: *mut TS_RUNINFO, run_capacity: u32, run_count: *mut u32, next: *mut i32) -> Result<()> {
        let chars = u32::from(start <= 0 && plain_capacity > 0 && !plain.is_null());
        unsafe {
            if chars != 0 { plain.0.write(' ' as u16); }
            if !plain_count.is_null() { plain_count.write(chars); }
            let run_items = u32::from(chars != 0 && run_capacity > 0 && !runs.is_null());
            if run_items != 0 { runs.write(TS_RUNINFO { uCount: chars, r#type: TS_RT_PLAIN }); }
            if !run_count.is_null() { run_count.write(run_items); }
            if !next.is_null() { next.write((start + chars as i32).clamp(0, 1)); }
        }
        Ok(())
    }
    fn SetText(&self, _flags: u32, start: i32, end: i32, _text: &PCWSTR, count: u32) -> Result<TS_TEXTCHANGE> { Ok(TS_TEXTCHANGE { acpStart: start, acpOldEnd: end, acpNewEnd: start + count as i32 }) }
    fn GetFormattedText(&self, _start: i32, _end: i32) -> Result<IDataObject> { not_implemented() }
    fn GetEmbedded(&self, _position: i32, _service: *const GUID, _riid: *const GUID) -> Result<IUnknown> { not_implemented() }
    fn QueryInsertEmbedded(&self, _service: *const GUID, _format: *const FORMATETC) -> Result<BOOL> { Ok(false.into()) }
    fn InsertEmbedded(&self, _flags: u32, _start: i32, _end: i32, _data: Ref<'_, IDataObject>) -> Result<TS_TEXTCHANGE> { not_implemented() }
    fn InsertTextAtSelection(&self, _flags: u32, _text: &PCWSTR, _count: u32, _start: *mut i32, _end: *mut i32, _change: *mut TS_TEXTCHANGE) -> Result<()> { not_implemented() }
    fn InsertEmbeddedAtSelection(&self, _flags: u32, _data: Ref<'_, IDataObject>, _start: *mut i32, _end: *mut i32, _change: *mut TS_TEXTCHANGE) -> Result<()> { not_implemented() }
    fn RequestSupportedAttrs(&self, _flags: u32, _count: u32, _attrs: *const GUID) -> Result<()> { Ok(()) }
    fn RequestAttrsAtPosition(&self, _position: i32, _count: u32, _attrs: *const GUID, _flags: u32) -> Result<()> { Ok(()) }
    fn RequestAttrsTransitioningAtPosition(&self, _position: i32, _count: u32, _attrs: *const GUID, _flags: u32) -> Result<()> { Ok(()) }
    fn FindNextAttrTransition(&self, start: i32, _halt: i32, _count: u32, _attrs: *const GUID, _flags: u32, next: *mut i32, found: *mut BOOL, offset: *mut i32) -> Result<()> {
        unsafe { if !next.is_null() { next.write(start); } if !found.is_null() { found.write(false.into()); } if !offset.is_null() { offset.write(0); } }
        Ok(())
    }
    fn RetrieveRequestedAttrs(&self, _count: u32, _values: *mut TS_ATTRVAL, fetched: *mut u32) -> Result<()> { if !fetched.is_null() { unsafe { fetched.write(0) }; } Ok(()) }
    fn GetEndACP(&self) -> Result<i32> { Ok(1) }
    fn GetActiveView(&self) -> Result<u32> { Ok(0) }
    fn GetACPFromPoint(&self, _view: u32, _point: *const POINT, _flags: u32) -> Result<i32> { Ok(0) }
    fn GetTextExt(&self, _view: u32, _start: i32, _end: i32, rect: *mut RECT, clipped: *mut BOOL) -> Result<()> { ITfContextOwner_Impl::GetTextExt(self, 0, 0, rect, clipped) }
    fn GetScreenExt(&self, _view: u32) -> Result<RECT> { ITfContextOwner_Impl::GetScreenExt(self) }
    fn GetWnd(&self, _view: u32) -> Result<HWND> { Ok(self.hwnd()) }
}

pub(super) struct TsfCaretContext {
    hwnd: HWND,
    thread_manager: ITfThreadMgr,
    document_manager: ITfDocumentMgr,
    _context: ITfContext,
    _owner: ITfContextOwner,
    caret: Arc<Mutex<Option<RECT>>>,
}

impl TsfCaretContext {
    pub(super) fn new(hwnd: HWND) -> Result<Self> {
        let (thread_manager, client_id) = thread_manager()?;
        let document_manager = unsafe { thread_manager.CreateDocumentMgr()? };
        let caret = Arc::new(Mutex::new(None));
        let owner: ITfContextOwner = CaretContextOwner {
            hwnd: hwnd.0 as isize,
            caret: caret.clone(),
            sink: RefCell::new(None),
            lock_flags: Cell::new(0),
        }
        .into();
        let owner_unknown: IUnknown = owner.cast()?;
        let mut context = None;
        let mut edit_cookie = 0;
        unsafe {
            document_manager.CreateContext(
                client_id,
                0,
                &owner_unknown,
                &mut context,
                &mut edit_cookie,
            )?;
        }
        let context = context.ok_or_else(windows::core::Error::from_win32)?;
        unsafe { document_manager.Push(&context)? };

        Ok(Self {
            hwnd,
            thread_manager,
            document_manager,
            _context: context,
            _owner: owner,
            caret,
        })
    }

    pub(super) fn set_caret(&self, geometry: SystemCaretGeometry) {
        let mut point = POINT { x: geometry.x, y: geometry.y };
        if !unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(self.hwnd, &mut point) }.as_bool() {
            return;
        }
        let activate = if let Ok(mut caret) = self.caret.lock() {
            let activate = caret.is_none();
            *caret = Some(RECT {
                left: point.x,
                top: point.y,
                right: point.x + geometry.width,
                bottom: point.y + geometry.height,
            });
            activate
        } else {
            false
        };
        if activate {
            unsafe { self.thread_manager.SetFocus(&self.document_manager).ok() };
        }
    }

    pub(super) fn clear_caret(&self) {
        let deactivate = self
            .caret
            .lock()
            .is_ok_and(|mut caret| caret.take().is_some());
        if deactivate {
            unsafe {
                let vtable = ITfThreadMgr::vtable(&self.thread_manager);
                let _ = (vtable.SetFocus)(
                    ITfThreadMgr::as_raw(&self.thread_manager),
                    std::ptr::null_mut(),
                )
                .ok();
            }
        }
    }
}

impl Drop for TsfCaretContext {
    fn drop(&mut self) {
        self.clear_caret();
        unsafe { self.document_manager.Pop(TF_POPF_ALL).ok() };
    }
}
