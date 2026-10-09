//! Error conversion between `windows` crate errors and core types.

use bd_core::raw::{ReadError, ReadErrorKind};

pub const ERROR_INVALID_FUNCTION: u32 = 1;
pub const ERROR_FILE_NOT_FOUND: u32 = 2;
pub const ERROR_PATH_NOT_FOUND: u32 = 3;
pub const ERROR_ACCESS_DENIED: u32 = 5;
pub const ERROR_NO_MORE_FILES: u32 = 18;
pub const ERROR_NOT_READY: u32 = 21;
pub const ERROR_NOT_SUPPORTED: u32 = 50;
pub const ERROR_DEV_NOT_EXIST: u32 = 55;
pub const ERROR_INVALID_PARAMETER: u32 = 87;
pub const ERROR_INVALID_LEVEL: u32 = 124;
pub const ERROR_FILE_INVALID: u32 = 1006;
pub const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

/// The Win32 error code inside an `HRESULT_FROM_WIN32` value, if it is one.
pub fn win32_code(e: &windows_core::Error) -> Option<u32> {
    let hr = e.code().0 as u32;
    (hr & 0xFFFF_0000 == 0x8007_0000).then_some(hr & 0xFFFF)
}

/// The OS code reported to users: the Win32 code when there is one,
/// otherwise the raw `HRESULT`.
pub fn os_code(e: &windows_core::Error) -> i32 {
    win32_code(e).map_or(e.code().0, |c| c as i32)
}

pub fn read_error(e: &windows_core::Error) -> ReadError {
    let kind = match win32_code(e) {
        Some(ERROR_ACCESS_DENIED) => ReadErrorKind::AccessDenied,
        Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) => ReadErrorKind::NotFound,
        Some(
            ERROR_NOT_READY | ERROR_DEV_NOT_EXIST | ERROR_FILE_INVALID | ERROR_DEVICE_NOT_CONNECTED,
        ) => ReadErrorKind::DeviceGone,
        _ => ReadErrorKind::Other,
    };
    ReadError {
        kind,
        os_code: Some(os_code(e)),
        message: e.message().trim_end().to_string(),
    }
}

pub fn io_error(e: windows_core::Error) -> std::io::Error {
    match win32_code(&e) {
        Some(code) => std::io::Error::from_raw_os_error(code as i32),
        None => std::io::Error::other(e),
    }
}
