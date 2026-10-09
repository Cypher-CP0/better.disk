//! Platform-specific fixture operations. Each returns an error (recorded as
//! a skipped case) when the platform or volume cannot do it.

use std::ffi::OsString;
use std::io;
use std::path::Path;

#[cfg(windows)]
pub use win::*;

#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod win {
    use super::*;
    use std::fs::{File, OpenOptions};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::process::Command;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        FILE_CASE_SENSITIVE_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FileCaseSensitiveInfo, SetFileInformationByHandle,
    };
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{
        FSCTL_SET_COMPRESSION, FSCTL_SET_REPARSE_POINT, FSCTL_SET_SPARSE,
    };

    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
    const FILE_CS_FLAG_CASE_SENSITIVE_DIR: u32 = 0x1;
    const COMPRESSION_FORMAT_DEFAULT: u16 = 1;
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    /// Everyone.
    const EVERYONE_SID: &str = "*S-1-1-0";

    fn handle(f: &File) -> HANDLE {
        HANDLE(f.as_raw_handle())
    }

    fn ioctl(f: &File, code: u32, input: &[u8]) -> io::Result<()> {
        let mut returned = 0u32;
        // SAFETY: input points to `input.len()` readable bytes; no output.
        unsafe {
            DeviceIoControl(
                handle(f),
                code,
                (!input.is_empty()).then_some(input.as_ptr().cast()),
                input.len() as u32,
                None,
                0,
                Some(&mut returned),
                None,
            )
        }
        .map_err(bd_win::error::io_error)
    }

    /// A file whose only allocated range is its first `data.len()` bytes.
    pub fn write_sparse(path: &Path, data: &[u8], logical: u64) -> io::Result<()> {
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        ioctl(&f, FSCTL_SET_SPARSE, &[])?;
        io::Write::write_all(&mut &f, data)?;
        f.set_len(logical)?;
        Ok(())
    }

    /// A file stored with NTFS compression.
    pub fn write_compressed(path: &Path, data: &[u8]) -> io::Result<()> {
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        ioctl(
            &f,
            FSCTL_SET_COMPRESSION,
            &COMPRESSION_FORMAT_DEFAULT.to_le_bytes(),
        )?;
        io::Write::write_all(&mut &f, data)?;
        f.sync_all()?;
        Ok(())
    }

    pub fn write_stream(path: &Path, stream: &str, data: &[u8]) -> io::Result<()> {
        let mut p = path.as_os_str().to_owned();
        p.push(":");
        p.push(stream);
        std::fs::write(Path::new(&p), data)
    }

    /// Make a junction (mount-point reparse point) at `link` pointing to
    /// `target`. `link` must not exist; `target` must be an absolute local
    /// path.
    pub fn junction(link: &Path, target: &Path) -> io::Result<()> {
        let plain = bd_win::wide::display(target);
        if plain.starts_with("\\\\") {
            return Err(io::Error::other(
                "junction targets must be local drive paths",
            ));
        }
        let print: Vec<u16> = std::ffi::OsStr::new(&plain).encode_wide().collect();
        let mut sub: Vec<u16> = "\\??\\".encode_utf16().collect();
        sub.extend_from_slice(&print);

        let mut path_buffer: Vec<u8> = Vec::new();
        for u in sub.iter().chain(&[0]).chain(&print).chain(&[0]) {
            path_buffer.extend_from_slice(&u.to_le_bytes());
        }
        let sub_bytes = (sub.len() * 2) as u16;
        let print_bytes = (print.len() * 2) as u16;
        let data_len = 8 + path_buffer.len();
        let mut buf = Vec::with_capacity(8 + data_len);
        buf.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
        buf.extend_from_slice(&(data_len as u16).to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes()); // SubstituteNameOffset
        buf.extend_from_slice(&sub_bytes.to_le_bytes());
        buf.extend_from_slice(&(sub_bytes + 2).to_le_bytes()); // PrintNameOffset
        buf.extend_from_slice(&print_bytes.to_le_bytes());
        buf.extend_from_slice(&path_buffer);

        std::fs::create_dir(link)?;
        let result = OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(link)
            .and_then(|f| ioctl(&f, FSCTL_SET_REPARSE_POINT, &buf));
        if result.is_err() {
            let _ = std::fs::remove_dir(link);
        }
        result
    }

    pub fn symlink_dir(target: &Path, link: &Path) -> io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    pub fn symlink_file(target: &Path, link: &Path) -> io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    /// Mark an empty directory case-sensitive. Needs the per-directory
    /// case sensitivity feature (installed with WSL).
    pub fn set_case_sensitive(dir: &Path) -> io::Result<()> {
        let f = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
            .open(dir)?;
        let info = FILE_CASE_SENSITIVE_INFO {
            Flags: FILE_CS_FLAG_CASE_SENSITIVE_DIR,
        };
        // SAFETY: the struct and size match the information class.
        unsafe {
            SetFileInformationByHandle(
                handle(&f),
                FileCaseSensitiveInfo,
                (&raw const info).cast(),
                std::mem::size_of::<FILE_CASE_SENSITIVE_INFO>() as u32,
            )
        }
        .map_err(bd_win::error::io_error)
    }

    fn icacls(dir: &Path, args: &[&str]) -> io::Result<()> {
        let status = Command::new("icacls")
            .arg(bd_win::wide::display(dir))
            .args(args)
            .stdout(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "icacls {args:?} failed: {status}"
            )))
        }
    }

    /// Deny listing `dir` to everyone. Returns whether listing now fails
    /// for the current user.
    pub fn deny(dir: &Path) -> io::Result<bool> {
        icacls(dir, &["/deny", &format!("{EVERYONE_SID}:(RD)")])?;
        Ok(std::fs::read_dir(dir).is_err())
    }

    pub fn undo_deny(dir: &Path) -> io::Result<()> {
        icacls(dir, &["/remove:d", EVERYONE_SID])
    }

    /// A name containing an unpaired surrogate: valid on NTFS, not valid
    /// Unicode.
    pub fn non_unicode_name() -> OsString {
        let mut units: Vec<u16> = "bad-".encode_utf16().collect();
        units.push(0xD800);
        units.extend(".txt".encode_utf16());
        OsString::from_wide(&units)
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;

    pub fn write_sparse(path: &Path, data: &[u8], logical: u64) -> io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        io::Write::write_all(&mut f, data)?;
        f.set_len(logical)?;
        Ok(())
    }

    pub fn write_compressed(_path: &Path, _data: &[u8]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "NTFS compression is Windows-only",
        ))
    }

    pub fn write_stream(_path: &Path, _stream: &str, _data: &[u8]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "alternate data streams are Windows-only",
        ))
    }

    pub fn junction(_link: &Path, _target: &Path) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "junctions are Windows-only",
        ))
    }

    pub fn symlink_dir(target: &Path, link: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    pub fn symlink_file(target: &Path, link: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    pub fn set_case_sensitive(_dir: &Path) -> io::Result<()> {
        // Unix file systems used for development are case-sensitive.
        Ok(())
    }

    pub fn deny(dir: &Path) -> io::Result<bool> {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000))?;
        // Root ignores permission bits; the manifest records that.
        Ok(std::fs::read_dir(dir).is_err())
    }

    pub fn undo_deny(dir: &Path) -> io::Result<()> {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))
    }

    /// A name that is not valid UTF-8.
    pub fn non_unicode_name() -> OsString {
        std::ffi::OsStr::from_bytes(b"bad-\xff.txt").to_owned()
    }
}
