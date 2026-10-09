//! What a directory reader returns, and the trait readers implement.

use std::ffi::OsString;
use std::fmt;
use std::path::Path;

use crate::attrs::{self, LinkKind, Placeholder};
use crate::time::Ticks;

/// A 128-bit file identity, unique within one volume. NTFS file IDs fit in
/// 64 bits; ReFS uses all 128.
pub type FileId = u128;

/// One directory entry exactly as a reader observed it.
#[derive(Clone, Debug)]
pub struct RawEntry {
    pub name: OsString,
    /// Windows `FILE_ATTRIBUTE_*` bits (see [`crate::attrs`]).
    pub attributes: u32,
    /// Reparse tag, or 0 when the entry is not a reparse point.
    pub reparse_tag: u32,
    /// Logical length (end of file) in bytes.
    pub logical: u64,
    /// Allocation as reported by the reader, or `None` when the reader
    /// cannot measure it. Never substituted with `logical` (FR02).
    pub allocated: Option<u64>,
    /// Identity on its volume, when the reader can provide one.
    pub file_id: Option<FileId>,
    pub created: Option<Ticks>,
    pub modified: Option<Ticks>,
    pub accessed: Option<Ticks>,
}

impl RawEntry {
    pub fn is_dir(&self) -> bool {
        self.attributes & attrs::FILE_ATTRIBUTE_DIRECTORY != 0
    }

    pub fn link_kind(&self) -> Option<LinkKind> {
        attrs::link_kind(self.attributes, self.reparse_tag)
    }

    pub fn placeholder(&self) -> Option<Placeholder> {
        attrs::placeholder_state(self.attributes, self.reparse_tag)
    }

    /// Directories are traversed unless they are links (FR03).
    pub fn should_traverse(&self) -> bool {
        self.is_dir() && self.link_kind().is_none()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReadErrorKind {
    AccessDenied,
    /// The directory disappeared between discovery and reading.
    NotFound,
    /// The volume went away (drive disconnected).
    DeviceGone,
    Other,
}

/// A directory that could not be read. Recorded per path; never aborts the
/// rest of the scan (FR01).
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadError {
    pub kind: ReadErrorKind,
    /// The OS error code (Win32 error or errno), when known.
    pub os_code: Option<i32>,
    pub message: String,
}

impl ReadError {
    pub fn from_io(e: &std::io::Error) -> ReadError {
        use std::io::ErrorKind as K;
        let kind = match e.kind() {
            K::PermissionDenied => ReadErrorKind::AccessDenied,
            K::NotFound => ReadErrorKind::NotFound,
            _ => ReadErrorKind::Other,
        };
        ReadError {
            kind,
            os_code: e.raw_os_error(),
            message: e.to_string(),
        }
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.os_code {
            Some(code) => write!(f, "{:?} (os error {code}): {}", self.kind, self.message),
            None => write!(f, "{:?}: {}", self.kind, self.message),
        }
    }
}

/// Reads one directory's immediate entries. Implementations must not follow
/// links, must not read file contents, and must not deliberately hydrate
/// cloud placeholders (FR01, FR03).
pub trait DirReader: Send + Sync {
    /// Short identifier used in reports, e.g. `"win-idextd"` or `"std"`.
    fn name(&self) -> &'static str;

    /// Return the entries of `dir`, excluding `.` and `..`.
    fn read_dir(&self, dir: &Path) -> Result<Vec<RawEntry>, ReadError>;
}
