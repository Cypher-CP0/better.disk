//! An owned Win32 handle, closed on drop.

use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_core::PCWSTR;

use crate::wide;

pub(crate) struct Handle(HANDLE);

impl Handle {
    pub(crate) fn raw(&self) -> HANDLE {
        self.0
    }

    /// Open an existing file or directory for metadata access without
    /// following a final reparse point and without blocking other
    /// processes (every share mode is granted).
    pub(crate) fn open_no_follow(path: &Path, access: u32) -> windows_core::Result<Handle> {
        Handle::open(
            path,
            access,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
    }

    pub(crate) fn open(
        path: &Path,
        access: u32,
        flags: FILE_FLAGS_AND_ATTRIBUTES,
    ) -> windows_core::Result<Handle> {
        let w = wide::to_wide_nul(path.as_os_str())?;
        // SAFETY: `w` is NUL-terminated and outlives the call.
        let h = unsafe {
            CreateFileW(
                PCWSTR(w.as_ptr()),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                flags,
                None,
            )?
        };
        Ok(Handle(h))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: we own the handle and close it once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
