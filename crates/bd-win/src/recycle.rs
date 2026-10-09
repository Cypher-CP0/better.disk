//! Recycle-only deletion through the Windows Shell (FR10, AT13).
//!
//! Each item gets its own `IFileOperation` on a dedicated single-threaded
//! COM apartment, with flags that ask for recycling
//! (`FOFX_RECYCLEONDELETE | FOF_ALLOWUNDO`). That request is not a
//! guarantee: the Shell silently deletes permanently when an item cannot be
//! recycled (too large for the bin, bin disabled, some removable and network
//! volumes). So a progress sink inspects every `PreDeleteItem` call and
//! vetoes the deletion unless the Shell reports
//! `TSF_DELETE_RECYCLE_IF_POSSIBLE`. `PostDeleteItem` then tells us whether
//! a Recycle Bin item was created. The Recycle Bin size and item count are
//! read before and after as independent evidence.
//!
//! The verdict for each item is derived from what was observed, never from
//! what was requested. Stage 0 verifies these signals on real volumes; the
//! application must not trust them until it has.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows::Win32::Storage::FileSystem::GetVolumePathNameW;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    FILEOPERATION_FLAGS, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
    FOF_WANTNUKEWARNING, FOFX_EARLYFAILURE, FOFX_RECYCLEONDELETE, FileOperation, IFileOperation,
    IFileOperationProgressSink, IFileOperationProgressSink_Impl, IShellItem,
    SHCreateItemFromParsingName, SHQUERYRBINFO, SHQueryRecycleBinW, SIGDN_FILESYSPATH,
    TSF_DELETE_RECYCLE_IF_POSSIBLE,
};
use windows_core::{HRESULT, PCWSTR, Ref, implement};

use crate::wide;

/// How much the Shell may ask the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Profile {
    /// No Shell confirmation (the application has already confirmed);
    /// stop at the first failure.
    Strict,
    /// Let the Shell show its own confirmation.
    Confirm,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecycleOptions {
    pub profile: Profile,
    /// Veto any deletion the Shell does not mark as a recycle. Only the
    /// spike may turn this off, and only inside a sandbox.
    pub veto_permanent: bool,
    /// Ask the Shell to warn before a permanent delete
    /// (`FOF_WANTNUKEWARNING`). Stage 0 measures whether the veto alone
    /// makes this unnecessary.
    pub nuke_warning: bool,
    /// Hide the Shell progress and error UI.
    pub silent: bool,
}

impl Default for RecycleOptions {
    fn default() -> RecycleOptions {
        RecycleOptions {
            profile: Profile::Strict,
            veto_permanent: true,
            nuke_warning: true,
            silent: false,
        }
    }
}

impl RecycleOptions {
    pub fn flags(&self) -> u32 {
        let mut f: FILEOPERATION_FLAGS = FOFX_RECYCLEONDELETE | FOF_ALLOWUNDO;
        if self.nuke_warning {
            f |= FOF_WANTNUKEWARNING;
        }
        if self.profile == Profile::Strict {
            f |= FOF_NOCONFIRMATION | FOFX_EARLYFAILURE;
        }
        if self.silent {
            f |= FOF_SILENT | FOF_NOERRORUI;
        }
        f.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    /// Gone from its location and the Shell reported a Recycle Bin item.
    Recycled,
    /// The Shell would have deleted it permanently; the sink vetoed it and
    /// the item is still in place.
    Blocked,
    /// Gone, and the Shell reported no Recycle Bin item.
    PermanentlyDeleted,
    /// Still in place because the operation was cancelled (by the user in
    /// the Shell's confirmation, or by an earlier failure).
    Cancelled,
    /// Still in place after an error.
    Failed,
    /// Not present before the operation.
    Missing,
    /// The signals disagree; see the outcome's details.
    Unclear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BinState {
    pub bytes: i64,
    pub items: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PathForm {
    /// Ordinary `C:\...` path.
    Plain,
    /// `\\?\C:\...`, used when the plain form would be rewritten by the
    /// Win32 path parser (long paths, trailing dots or spaces).
    Verbatim,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemOutcome {
    #[serde(skip)]
    pub path: PathBuf,
    pub display: String,
    pub verdict: Verdict,
    pub detail: String,
    pub path_form: PathForm,
    pub exists_before: bool,
    pub exists_after: bool,
    pub pre_delete_called: bool,
    pub pre_delete_flags: Option<u32>,
    /// `TSF_DELETE_RECYCLE_IF_POSSIBLE` was present in `PreDeleteItem`.
    pub shell_will_recycle: Option<bool>,
    pub vetoed: bool,
    pub post_delete_hr: Option<i32>,
    /// Where the Shell says the item now lives in the Recycle Bin.
    pub recycle_bin_item: Option<String>,
    pub perform_hr: Option<i32>,
    pub any_aborted: Option<bool>,
    pub bin_before: Option<BinState>,
    pub bin_after: Option<BinState>,
    pub elapsed_ms: f64,
}

impl ItemOutcome {
    pub fn bin_items_delta(&self) -> Option<i64> {
        Some(self.bin_after?.items - self.bin_before?.items)
    }
}

#[derive(Debug, Default)]
struct SinkState {
    pre_delete_called: bool,
    pre_delete_flags: Option<u32>,
    vetoed: bool,
    post_delete_hr: Option<HRESULT>,
    recycle_bin_item: Option<String>,
    newly_created_reported: bool,
}

#[implement(IFileOperationProgressSink)]
struct Sink {
    state: Arc<Mutex<SinkState>>,
    veto: bool,
}

fn shell_item_path(item: &IShellItem) -> Option<String> {
    // SAFETY: GetDisplayName returns a CoTaskMem string we free once.
    unsafe {
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

#[allow(non_snake_case)]
impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> windows_core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, _hr: HRESULT) -> windows_core::Result<()> {
        Ok(())
    }
    fn PreRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows_core::Result<()> {
        Ok(())
    }
    fn PostRenameItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn PreMoveItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn PostMoveItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn PreCopyItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn PostCopyItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows_core::Result<()> {
        Ok(())
    }

    fn PreDeleteItem(&self, dwflags: u32, _item: Ref<IShellItem>) -> windows_core::Result<()> {
        let will_recycle = dwflags & (TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32) != 0;
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        st.pre_delete_called = true;
        st.pre_delete_flags = Some(dwflags);
        if self.veto && !will_recycle {
            st.vetoed = true;
            // Any failure cancels this item's deletion.
            return Err(windows::Win32::Foundation::E_ABORT.into());
        }
        Ok(())
    }

    fn PostDeleteItem(
        &self,
        _dwflags: u32,
        _item: Ref<IShellItem>,
        hrdelete: HRESULT,
        newly_created: Ref<IShellItem>,
    ) -> windows_core::Result<()> {
        let bin_item = newly_created.as_ref().and_then(shell_item_path);
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        st.post_delete_hr = Some(hrdelete);
        st.newly_created_reported = !newly_created.is_null();
        st.recycle_bin_item = bin_item;
        Ok(())
    }

    fn PreNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows_core::Result<()> {
        Ok(())
    }
    fn PostNewItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: &PCWSTR,
        _: u32,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows_core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, _: u32, _: u32) -> windows_core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows_core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows_core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows_core::Result<()> {
        Ok(())
    }
}

/// The mount path (e.g. `C:\`) of the volume holding `path`.
pub fn mount_path(path: &Path) -> windows_core::Result<OsString> {
    let w = wide::to_wide_nul(path.as_os_str())?;
    let mut buf = vec![0u16; 1024];
    // SAFETY: valid NUL-terminated input and output buffer.
    unsafe { GetVolumePathNameW(PCWSTR(w.as_ptr()), &mut buf)? };
    Ok(wide::from_wide_until_nul(&buf))
}

/// Recycle Bin size and item count for the volume mounted at `root`.
pub fn query_bin(root: &OsString) -> Option<BinState> {
    let w = wide::to_wide_nul(root).ok()?;
    let mut info = SHQUERYRBINFO {
        cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: valid input string and a correctly sized struct.
    unsafe { SHQueryRecycleBinW(PCWSTR(w.as_ptr()), &mut info) }.ok()?;
    Some(BinState {
        bytes: info.i64Size,
        items: info.i64NumItems,
    })
}

/// The form of `path` handed to the Shell.
fn shell_path(path: &Path) -> (PathForm, OsString) {
    let plain = strip_prefix(path);
    let display = plain.to_string_lossy();
    let needs_verbatim = display.encode_utf16().count() >= 260
        || display
            .split('\\')
            .any(|c| (c.ends_with('.') && c != "." && c != "..") || c.ends_with(' '))
        || plain.to_str().is_none();
    if needs_verbatim {
        (PathForm::Verbatim, path.as_os_str().to_owned())
    } else {
        (PathForm::Plain, plain)
    }
}

fn strip_prefix(path: &Path) -> OsString {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let units: Vec<u16> = path.as_os_str().encode_wide().collect();
    let prefix: [u16; 4] = [92, 92, 63, 92]; // \\?\
    let unc: [u16; 4] = [85, 78, 67, 92]; // UNC\
    if let Some(rest) = units.strip_prefix(&prefix[..]) {
        if let Some(r) = rest.strip_prefix(&unc[..]) {
            let mut v = vec![92u16, 92];
            v.extend_from_slice(r);
            return OsString::from_wide(&v);
        }
        return OsString::from_wide(rest);
    }
    path.as_os_str().to_owned()
}

struct ComApartment;

impl ComApartment {
    fn enter() -> windows_core::Result<ComApartment> {
        // SAFETY: paired with CoUninitialize in Drop on this thread.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).ok()? };
        Ok(ComApartment)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: matches the successful CoInitializeEx in `enter`.
        unsafe { CoUninitialize() };
    }
}

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn recycle_one(path: &Path, opts: &RecycleOptions) -> ItemOutcome {
    let started = Instant::now();
    let (path_form, shell_form) = shell_path(path);
    let mut out = ItemOutcome {
        path: path.to_path_buf(),
        display: wide::display(path),
        verdict: Verdict::Unclear,
        detail: String::new(),
        path_form,
        exists_before: exists(path),
        exists_after: false,
        pre_delete_called: false,
        pre_delete_flags: None,
        shell_will_recycle: None,
        vetoed: false,
        post_delete_hr: None,
        recycle_bin_item: None,
        perform_hr: None,
        any_aborted: None,
        bin_before: None,
        bin_after: None,
        elapsed_ms: 0.0,
    };
    if !out.exists_before {
        out.verdict = Verdict::Missing;
        out.detail = "not present before the operation".into();
        return out;
    }
    let root = mount_path(path).ok();
    out.bin_before = root.as_ref().and_then(query_bin);

    let state = Arc::new(Mutex::new(SinkState::default()));
    let result = run_operation(&shell_form, opts, &state);

    out.exists_after = exists(path);
    out.bin_after = root.as_ref().and_then(query_bin);
    let st = state.lock().unwrap_or_else(|p| p.into_inner());
    out.pre_delete_called = st.pre_delete_called;
    out.pre_delete_flags = st.pre_delete_flags;
    out.shell_will_recycle = st
        .pre_delete_flags
        .map(|f| f & (TSF_DELETE_RECYCLE_IF_POSSIBLE.0 as u32) != 0);
    out.vetoed = st.vetoed;
    out.post_delete_hr = st.post_delete_hr.map(|h| h.0);
    out.recycle_bin_item = st.recycle_bin_item.clone();

    match result {
        Err(Stage::Setup(e)) => {
            out.verdict = if out.exists_after {
                Verdict::Failed
            } else {
                Verdict::Unclear
            };
            out.detail = format!(
                "could not start the Shell operation: {}",
                e.message().trim_end()
            );
        }
        Ok((perform, aborted)) | Err(Stage::Perform(perform, aborted)) => {
            out.perform_hr = Some(perform.0);
            out.any_aborted = aborted;
            let post_ok = st.post_delete_hr.is_some_and(|h| h.is_ok());
            let bin_grew = out.bin_items_delta().is_some_and(|d| d > 0);
            (out.verdict, out.detail) = match (out.exists_after, st.vetoed) {
                (true, true) => (
                    Verdict::Blocked,
                    "the Shell would not have recycled this item; deletion vetoed".into(),
                ),
                (false, true) => (
                    Verdict::Unclear,
                    "vetoed, yet the item is gone; investigate before trusting the veto".into(),
                ),
                (false, false) if st.newly_created_reported && post_ok => {
                    (Verdict::Recycled, "moved to the Recycle Bin".into())
                }
                (false, false) if !st.newly_created_reported && !bin_grew && post_ok => (
                    Verdict::PermanentlyDeleted,
                    "gone, and no Recycle Bin item was reported or counted".into(),
                ),
                (false, false) => (
                    Verdict::Unclear,
                    format!(
                        "gone; bin item reported: {}, bin count delta: {:?}, post-delete hr: {:?}",
                        st.newly_created_reported,
                        out.bin_items_delta(),
                        st.post_delete_hr.map(|h| format!("{:#010x}", h.0 as u32))
                    ),
                ),
                (true, false) if aborted == Some(true) || perform.0 == E_CANCELLED_HR => (
                    Verdict::Cancelled,
                    "operation cancelled; item left in place".into(),
                ),
                (true, false) => (
                    Verdict::Failed,
                    format!(
                        "still present; PerformOperations returned {:#010x}",
                        perform.0 as u32
                    ),
                ),
            };
        }
    }
    drop(st);
    out.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    out
}

/// `HRESULT_FROM_WIN32(ERROR_CANCELLED)`.
const E_CANCELLED_HR: i32 = 0x8007_04C7_u32 as i32;

enum Stage {
    Setup(windows_core::Error),
    Perform(HRESULT, Option<bool>),
}

fn run_operation(
    shell_form: &OsString,
    opts: &RecycleOptions,
    state: &Arc<Mutex<SinkState>>,
) -> Result<(HRESULT, Option<bool>), Stage> {
    let w = wide::to_wide_nul(shell_form).map_err(Stage::Setup)?;
    // SAFETY: COM is initialized on this thread (STA) by the caller; all
    // interface pointers are reference-counted by the bindings.
    unsafe {
        let item: IShellItem =
            SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).map_err(Stage::Setup)?;
        let op: IFileOperation =
            CoCreateInstance(&FileOperation, None, CLSCTX_ALL).map_err(Stage::Setup)?;
        op.SetOperationFlags(FILEOPERATION_FLAGS(opts.flags()))
            .map_err(Stage::Setup)?;
        let sink: IFileOperationProgressSink = Sink {
            state: Arc::clone(state),
            veto: opts.veto_permanent,
        }
        .into();
        let cookie = op.Advise(&sink).map_err(Stage::Setup)?;
        if let Err(e) = op.DeleteItem(&item, None) {
            let _ = op.Unadvise(cookie);
            return Err(Stage::Setup(e));
        }
        let perform = match op.PerformOperations() {
            Ok(()) => HRESULT(0),
            Err(e) => e.code(),
        };
        let aborted = op.GetAnyOperationsAborted().ok().map(|b| b.as_bool());
        let _ = op.Unadvise(cookie);
        if perform.is_ok() {
            Ok((perform, aborted))
        } else {
            Err(Stage::Perform(perform, aborted))
        }
    }
}

/// Recycle each path in its own Shell operation, on a dedicated STA
/// thread. The caller is responsible for deciding which paths may be
/// touched at all; this function only guarantees it never asks for, or
/// allows, a permanent delete when `veto_permanent` is set.
pub fn recycle(paths: Vec<PathBuf>, opts: RecycleOptions) -> Result<Vec<ItemOutcome>, String> {
    std::thread::Builder::new()
        .name("bd-recycle-sta".into())
        .spawn(move || {
            let _com = ComApartment::enter()
                .map_err(|e| format!("COM initialization failed: {}", e.message()))?;
            Ok(paths.iter().map(|p| recycle_one(p, &opts)).collect())
        })
        .map_err(|e| e.to_string())?
        .join()
        .map_err(|_| "recycle thread panicked".to_string())?
}
