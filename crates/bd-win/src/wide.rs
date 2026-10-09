//! Path conversion helpers.
//!
//! Scans work on `\\?\` (verbatim) paths so that long paths and names the
//! Win32 path parser would rewrite — trailing dots and spaces, reserved
//! device names — reach the file system unchanged. Only the user-supplied
//! root is ever normalized; every child path is built by appending the
//! native name the file system returned (FR03).

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

const VERBATIM: [u16; 4] = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
const DEVICE: [u16; 4] = [b'\\' as u16, b'\\' as u16, b'.' as u16, b'\\' as u16];
const UNC_AFTER_VERBATIM: [u16; 4] = [b'U' as u16, b'N' as u16, b'C' as u16, b'\\' as u16];

/// NUL-terminated UTF-16 for a Win32 call. Rejects interior NULs, which
/// would silently truncate the path.
pub fn to_wide_nul(s: &OsStr) -> windows_core::Result<Vec<u16>> {
    let mut v: Vec<u16> = s.encode_wide().collect();
    if v.contains(&0) {
        return Err(windows_core::Error::from_hresult(
            windows_core::HRESULT::from_win32(crate::error::ERROR_INVALID_PARAMETER),
        ));
    }
    v.push(0);
    Ok(v)
}

/// Units up to (not including) the first NUL.
pub fn from_wide_until_nul(buf: &[u16]) -> OsString {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    OsString::from_wide(&buf[..end])
}

/// Convert a user-supplied path to an absolute verbatim path.
///
/// Already-verbatim and `\\.\` device paths are returned unchanged. Other
/// paths are made absolute with the normal Win32 rules first (resolving
/// `.` and `..`), which is correct for user input but must never be applied
/// to names read from disk.
pub fn to_verbatim(path: &Path) -> io::Result<PathBuf> {
    let units: Vec<u16> = path.as_os_str().encode_wide().collect();
    if units.starts_with(&VERBATIM) || units.starts_with(&DEVICE) {
        return Ok(path.to_path_buf());
    }
    let abs = std::path::absolute(path)?;
    let abs_units: Vec<u16> = abs.as_os_str().encode_wide().collect();
    if abs_units.starts_with(&VERBATIM) || abs_units.starts_with(&DEVICE) {
        return Ok(abs);
    }
    let mut out = Vec::with_capacity(abs_units.len() + 8);
    out.extend_from_slice(&VERBATIM);
    if abs_units.starts_with(&[b'\\' as u16, b'\\' as u16]) {
        // \\server\share\x  ->  \\?\UNC\server\share\x
        out.extend_from_slice(&UNC_AFTER_VERBATIM);
        out.extend_from_slice(&abs_units[2..]);
    } else {
        out.extend_from_slice(&abs_units);
    }
    Ok(PathBuf::from(OsString::from_wide(&out)))
}

/// A path for display only: the verbatim prefix is removed. Lossy for
/// names that are not valid Unicode; never use the result as an action
/// target.
pub fn display(path: &Path) -> String {
    let units: Vec<u16> = path.as_os_str().encode_wide().collect();
    let stripped: Vec<u16> = if units.starts_with(&VERBATIM) {
        let rest = &units[4..];
        if rest.starts_with(&UNC_AFTER_VERBATIM) {
            let mut v = vec![b'\\' as u16, b'\\' as u16];
            v.extend_from_slice(&rest[4..]);
            v
        } else {
            rest.to_vec()
        }
    } else {
        units
    };
    String::from_utf16_lossy(&stripped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_paths_are_untouched() {
        let p = Path::new(r"\\?\C:\Temp\trailing.");
        assert_eq!(to_verbatim(p).unwrap(), p);
    }

    #[test]
    fn drive_paths_gain_prefix() {
        let v = to_verbatim(Path::new(r"C:\Temp\..\Data")).unwrap();
        assert_eq!(v, Path::new(r"\\?\C:\Data"));
        assert_eq!(display(&v), r"C:\Data");
    }

    #[test]
    fn unc_paths_use_unc_form() {
        let v = to_verbatim(Path::new(r"\\server\share\dir")).unwrap();
        assert_eq!(v, Path::new(r"\\?\UNC\server\share\dir"));
        assert_eq!(display(&v), r"\\server\share\dir");
    }

    #[test]
    fn trailing_dot_survives_join_on_verbatim_root() {
        let root = to_verbatim(Path::new(r"C:\Temp")).unwrap();
        let child = root.join("name.");
        assert!(display(&child).ends_with("name."));
    }

    #[test]
    fn interior_nul_rejected() {
        assert!(to_wide_nul(OsStr::new("a\0b")).is_err());
        assert_eq!(to_wide_nul(OsStr::new("ab")).unwrap(), vec![97, 98, 0]);
    }
}
