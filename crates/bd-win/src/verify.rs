//! Per-file handle queries used to check what enumeration reported.
//!
//! The enumeration spike opens a sample of files with
//! `FILE_READ_ATTRIBUTES` only (no data access, links not followed) and
//! compares sizes, allocation, link counts and identities with the batch
//! results. This is a measurement tool for Stage 0; the scanner itself
//! never opens files.

use std::path::Path;

use windows::Win32::Foundation::{GetLastError, SetLastError, WIN32_ERROR};
use windows::Win32::Storage::FileSystem::{
    FILE_ID_INFO, FILE_STANDARD_INFO, FileIdInfo, FileStandardInfo, GetCompressedFileSizeW,
    GetFileInformationByHandleEx, INVALID_FILE_SIZE,
};
use windows_core::PCWSTR;

use crate::handle::Handle;
use crate::wide;

const FILE_READ_ATTRIBUTES: u32 = 0x0080;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandleFacts {
    pub logical: u64,
    pub allocated: u64,
    pub link_count: u32,
    pub is_dir: bool,
    pub delete_pending: bool,
    pub volume_serial: Option<u64>,
    pub file_id: Option<u128>,
    /// `GetCompressedFileSizeW`: on-disk size for compressed and sparse
    /// files. For ordinary files it is expected to equal the logical size,
    /// not the allocation — one of the hypotheses Stage 0 checks.
    pub compressed_size: Option<u64>,
}

pub fn handle_facts(path: &Path) -> windows_core::Result<HandleFacts> {
    let handle = Handle::open_no_follow(path, FILE_READ_ATTRIBUTES)?;

    let mut std_info = FILE_STANDARD_INFO::default();
    // SAFETY: buffer matches the class and size.
    unsafe {
        GetFileInformationByHandleEx(
            handle.raw(),
            FileStandardInfo,
            (&raw mut std_info).cast(),
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )?;
    }

    let mut id_info = FILE_ID_INFO::default();
    // SAFETY: buffer matches the class and size.
    let id = unsafe {
        GetFileInformationByHandleEx(
            handle.raw(),
            FileIdInfo,
            (&raw mut id_info).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    }
    .ok()
    .map(|_| {
        (
            id_info.VolumeSerialNumber,
            u128::from_le_bytes(id_info.FileId.Identifier),
        )
    });
    drop(handle);

    Ok(HandleFacts {
        logical: u64::try_from(std_info.EndOfFile).unwrap_or(0),
        allocated: u64::try_from(std_info.AllocationSize).unwrap_or(0),
        link_count: std_info.NumberOfLinks,
        is_dir: std_info.Directory,
        delete_pending: std_info.DeletePending,
        volume_serial: id.map(|(v, _)| v),
        file_id: id.map(|(_, f)| f).filter(|&f| f != 0),
        compressed_size: compressed_size(path),
    })
}

fn compressed_size(path: &Path) -> Option<u64> {
    let w = wide::to_wide_nul(path.as_os_str()).ok()?;
    let mut high = 0u32;
    // SAFETY: valid NUL-terminated path and out-pointer. The last-error
    // value is reset first because INVALID_FILE_SIZE is also a valid low
    // DWORD of a large size.
    unsafe {
        SetLastError(WIN32_ERROR(0));
        let low = GetCompressedFileSizeW(PCWSTR(w.as_ptr()), Some(&mut high));
        if low == INVALID_FILE_SIZE && GetLastError().0 != 0 {
            return None;
        }
        Some((u64::from(high) << 32) | u64::from(low))
    }
}
