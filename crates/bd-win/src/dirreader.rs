//! Native directory enumeration (FR01, FR02, FR03).
//!
//! One handle per directory, opened for `FILE_LIST_DIRECTORY` only with
//! every share mode granted and `FILE_FLAG_OPEN_REPARSE_POINT`, so a
//! directory that was replaced by a link after it was listed is read as the
//! link itself rather than followed. Entries come back in batches from
//! `GetFileInformationByHandleEx`:
//!
//! * `FileIdExtdDirectoryInfo` (preferred) carries the 128-bit file ID,
//!   the reparse tag, sizes, allocation and timestamps for every entry, so
//!   no file is ever opened.
//! * `FileFullDirectoryInfo` (fallback, e.g. FAT/exFAT and some network
//!   redirectors) has no file ID; for reparse points the `EaSize` field
//!   holds the reparse tag. Without IDs, hard links cannot be detected and
//!   the scan reports deduplication as unverified.
//!
//! The reader never reads file contents and never opens individual files,
//! so it does not hydrate cloud placeholders.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};

use bd_core::raw::{DirReader, RawEntry, ReadError};
use bd_core::time::Ticks;
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FULL_DIR_INFO, FILE_ID_EXTD_DIR_INFO,
    FILE_INFO_BY_HANDLE_CLASS, FILE_LIST_DIRECTORY, FileFullDirectoryInfo,
    FileFullDirectoryRestartInfo, FileIdExtdDirectoryInfo, FileIdExtdDirectoryRestartInfo,
    GetFileInformationByHandleEx,
};

use crate::error::{self, win32_code};
use crate::handle::Handle;

/// Which information class the reader uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EnumerationClass {
    /// Try `FileIdExtdDirectoryInfo`; fall back to `FileFullDirectoryInfo`
    /// for the rest of the scan if the volume rejects it.
    Auto,
    IdExtd,
    Full,
}

const MODE_UNKNOWN: u8 = 0;
const MODE_ID_EXTD: u8 = 1;
const MODE_FULL: u8 = 2;

/// Buffer for one batch. 64 KiB is the largest size every redirector
/// accepts; `u64` elements give the 8-byte alignment the records use.
const BUFFER_BYTES: usize = 64 * 1024;

pub struct WinDirReader {
    requested: EnumerationClass,
    mode: AtomicU8,
    open_reparse_point: bool,
}

impl WinDirReader {
    pub fn new(class: EnumerationClass) -> WinDirReader {
        let mode = match class {
            EnumerationClass::Auto => MODE_UNKNOWN,
            EnumerationClass::IdExtd => MODE_ID_EXTD,
            EnumerationClass::Full => MODE_FULL,
        };
        WinDirReader {
            requested: class,
            mode: AtomicU8::new(mode),
            open_reparse_point: true,
        }
    }

    /// Whether directories are opened with `FILE_FLAG_OPEN_REPARSE_POINT`
    /// (the default). With it, a folder replaced by a link between listing
    /// and reading is read as the link itself, never followed. Stage 0
    /// compares both modes on cloud-files folders, whose directories are
    /// reparse points too: the open mode may affect how the sync engine
    /// populates a listing.
    pub fn with_open_reparse_point(mut self, on: bool) -> WinDirReader {
        self.open_reparse_point = on;
        self
    }

    /// The class actually in use: `Auto` until the first directory is read.
    pub fn effective_class(&self) -> EnumerationClass {
        match self.mode.load(Ordering::Relaxed) {
            MODE_ID_EXTD => EnumerationClass::IdExtd,
            MODE_FULL => EnumerationClass::Full,
            _ => EnumerationClass::Auto,
        }
    }

    fn read_with(&self, handle: &Handle, full: bool) -> windows_core::Result<Vec<RawEntry>> {
        let (restart, next): (FILE_INFO_BY_HANDLE_CLASS, FILE_INFO_BY_HANDLE_CLASS) = if full {
            (FileFullDirectoryRestartInfo, FileFullDirectoryInfo)
        } else {
            (FileIdExtdDirectoryRestartInfo, FileIdExtdDirectoryInfo)
        };
        let mut buf = vec![0u64; BUFFER_BYTES / 8];
        let mut out = Vec::new();
        let mut first = true;
        loop {
            let class = if first { restart } else { next };
            // SAFETY: the buffer is valid for writes of BUFFER_BYTES and the
            // handle is an open directory handle.
            let r = unsafe {
                GetFileInformationByHandleEx(
                    handle.raw(),
                    class,
                    buf.as_mut_ptr().cast(),
                    BUFFER_BYTES as u32,
                )
            };
            if let Err(e) = r {
                match win32_code(&e) {
                    Some(error::ERROR_NO_MORE_FILES) => break,
                    // An empty volume root has no "." or ".." and reports
                    // "file not found" on the first batch.
                    Some(error::ERROR_FILE_NOT_FOUND) if first => break,
                    _ => return Err(e),
                }
            }
            first = false;
            let bytes = as_bytes(&buf);
            if full {
                parse_batch::<FILE_FULL_DIR_INFO>(bytes, &mut out);
            } else {
                parse_batch::<FILE_ID_EXTD_DIR_INFO>(bytes, &mut out);
            }
        }
        Ok(out)
    }
}

impl Default for WinDirReader {
    fn default() -> WinDirReader {
        WinDirReader::new(EnumerationClass::Auto)
    }
}

fn is_unsupported_class(e: &windows_core::Error) -> bool {
    matches!(
        win32_code(e),
        Some(
            error::ERROR_INVALID_PARAMETER
                | error::ERROR_NOT_SUPPORTED
                | error::ERROR_INVALID_LEVEL
                | error::ERROR_INVALID_FUNCTION
        )
    )
}

impl DirReader for WinDirReader {
    fn name(&self) -> &'static str {
        match (self.requested, self.effective_class()) {
            (_, EnumerationClass::IdExtd) => "win-idextd",
            (_, EnumerationClass::Full) => "win-full",
            (_, EnumerationClass::Auto) => "win-auto",
        }
    }

    fn read_dir(&self, dir: &Path) -> Result<Vec<RawEntry>, ReadError> {
        let handle = if self.open_reparse_point {
            Handle::open_no_follow(dir, FILE_LIST_DIRECTORY.0)
        } else {
            Handle::open(dir, FILE_LIST_DIRECTORY.0, FILE_FLAG_BACKUP_SEMANTICS)
        }
        .map_err(|e| error::read_error(&e))?;
        let mode = self.mode.load(Ordering::Relaxed);
        if mode == MODE_FULL {
            return self
                .read_with(&handle, true)
                .map_err(|e| error::read_error(&e));
        }
        match self.read_with(&handle, false) {
            Ok(v) => {
                if mode == MODE_UNKNOWN {
                    let _ = self.mode.compare_exchange(
                        MODE_UNKNOWN,
                        MODE_ID_EXTD,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    );
                }
                Ok(v)
            }
            Err(e) if self.requested == EnumerationClass::Auto && is_unsupported_class(&e) => {
                self.mode.store(MODE_FULL, Ordering::Relaxed);
                self.read_with(&handle, true)
                    .map_err(|e| error::read_error(&e))
            }
            Err(e) => Err(error::read_error(&e)),
        }
    }
}

fn as_bytes(buf: &[u64]) -> &[u8] {
    // SAFETY: u64 has no padding; any byte view of it is valid.
    unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), std::mem::size_of_val(buf)) }
}

/// The fields both record layouts share, plus what only one has.
struct Record {
    next: u32,
    created: i64,
    accessed: i64,
    modified: i64,
    end_of_file: i64,
    allocation: i64,
    attributes: u32,
    name_bytes: u32,
    reparse_tag: u32,
    file_id: Option<[u8; 16]>,
}

trait DirRecord: Sized {
    /// Byte offset of `FileName` within the record.
    const NAME_OFFSET: usize;
    fn record(&self) -> Record;
}

impl DirRecord for FILE_ID_EXTD_DIR_INFO {
    const NAME_OFFSET: usize = std::mem::offset_of!(FILE_ID_EXTD_DIR_INFO, FileName);
    fn record(&self) -> Record {
        Record {
            next: self.NextEntryOffset,
            created: self.CreationTime,
            accessed: self.LastAccessTime,
            modified: self.LastWriteTime,
            end_of_file: self.EndOfFile,
            allocation: self.AllocationSize,
            attributes: self.FileAttributes,
            name_bytes: self.FileNameLength,
            reparse_tag: self.ReparsePointTag,
            file_id: Some(self.FileId.Identifier),
        }
    }
}

impl DirRecord for FILE_FULL_DIR_INFO {
    const NAME_OFFSET: usize = std::mem::offset_of!(FILE_FULL_DIR_INFO, FileName);
    fn record(&self) -> Record {
        let is_reparse = self.FileAttributes & bd_core::attrs::FILE_ATTRIBUTE_REPARSE_POINT != 0;
        Record {
            next: self.NextEntryOffset,
            created: self.CreationTime,
            accessed: self.LastAccessTime,
            modified: self.LastWriteTime,
            end_of_file: self.EndOfFile,
            allocation: self.AllocationSize,
            attributes: self.FileAttributes,
            name_bytes: self.FileNameLength,
            // MS-FSCC: for reparse points, EaSize carries the reparse tag.
            reparse_tag: if is_reparse { self.EaSize } else { 0 },
            file_id: None,
        }
    }
}

fn parse_batch<T: DirRecord>(bytes: &[u8], out: &mut Vec<RawEntry>) {
    let mut offset = 0usize;
    loop {
        let Some(header_end) = offset.checked_add(T::NAME_OFFSET) else {
            return;
        };
        if header_end > bytes.len() {
            return;
        }
        // The struct's declared size includes one name unit plus padding,
        // which the last record in a full buffer need not have, so copy
        // only the bytes that exist into a zeroed value.
        let take = (bytes.len() - offset).min(std::mem::size_of::<T>());
        let mut tmp = std::mem::MaybeUninit::<T>::zeroed();
        // SAFETY: `take` bytes are in bounds of both source and
        // destination; T is a plain-integer record for which all-zero and
        // any byte pattern are valid.
        let rec = unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr().add(offset),
                tmp.as_mut_ptr().cast::<u8>(),
                take,
            );
            tmp.assume_init()
        }
        .record();
        let name_len = rec.name_bytes as usize;
        let Some(name_end) = header_end.checked_add(name_len) else {
            return;
        };
        if name_end > bytes.len() || !name_len.is_multiple_of(2) {
            return;
        }
        let units: Vec<u16> = bytes[header_end..name_end]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let is_dot = units == [b'.' as u16] || units == [b'.' as u16, b'.' as u16];
        if !is_dot {
            out.push(to_raw_entry(&rec, OsString::from_wide(&units)));
        }
        if rec.next == 0 {
            return;
        }
        offset += rec.next as usize;
    }
}

fn to_raw_entry(rec: &Record, name: OsString) -> RawEntry {
    let is_dir = rec.attributes & bd_core::attrs::FILE_ATTRIBUTE_DIRECTORY != 0;
    let file_id = rec
        .file_id
        .map(u128::from_le_bytes)
        .filter(|&id| id != 0 && id != u128::MAX);
    RawEntry {
        name,
        attributes: rec.attributes,
        reparse_tag: rec.reparse_tag,
        logical: if is_dir {
            0
        } else {
            u64::try_from(rec.end_of_file).unwrap_or(0)
        },
        allocated: if is_dir {
            None
        } else {
            u64::try_from(rec.allocation).ok()
        },
        file_id,
        created: Ticks::from_filetime(rec.created),
        modified: Ticks::from_filetime(rec.modified),
        accessed: Ticks::from_filetime(rec.accessed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bd_core::scan::{CancelToken, ScanConfig, ScanState, scan};
    use bd_core::tree::ScanTree;

    fn temp_root(label: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("bd-win-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        crate::wide::to_verbatim(&p).unwrap()
    }

    #[test]
    fn reads_entries_without_dots() {
        let root = temp_root("read");
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("a.txt"), b"hello").unwrap();
        let entries = WinDirReader::default().read_dir(&root).unwrap();
        let mut names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec![OsString::from("a.txt"), OsString::from("sub")]);
        let file = entries.iter().find(|e| e.name == "a.txt").unwrap();
        assert_eq!(file.logical, 5);
        assert!(file.modified.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn trailing_dot_names_round_trip() {
        let root = temp_root("dots");
        std::fs::write(root.join("odd."), b"x").unwrap();
        let tree = ScanTree::build(scan(
            &root,
            &WinDirReader::default(),
            &ScanConfig::default(),
            &CancelToken::new(),
            |_| {},
        ));
        assert_eq!(tree.state(), ScanState::Complete);
        let idx = tree.find(Path::new("odd.")).unwrap();
        assert!(std::fs::symlink_metadata(tree.path(idx)).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hard_links_share_an_id() {
        let root = temp_root("hardlink");
        std::fs::write(root.join("one"), vec![1u8; 9000]).unwrap();
        std::fs::hard_link(root.join("one"), root.join("two")).unwrap();
        let entries = WinDirReader::default().read_dir(&root).unwrap();
        let ids: Vec<_> = entries.iter().map(|e| e.file_id).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids[0].is_some());
        assert_eq!(ids[0], ids[1]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
