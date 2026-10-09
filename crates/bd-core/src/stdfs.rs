//! A portable [`DirReader`] built on `std::fs`.
//!
//! Used for tests and non-Windows development, and on Windows as a speed
//! and accuracy baseline for the native reader. On Windows it cannot see
//! file identities or allocation, so its results are deduplication-
//! unverified and have unknown allocation: that is reported, never guessed.

use std::fs;
use std::path::Path;

use crate::attrs;
use crate::raw::{DirReader, RawEntry, ReadError};
use crate::time::Ticks;

#[derive(Clone, Copy, Debug, Default)]
pub struct StdDirReader;

impl DirReader for StdDirReader {
    fn name(&self) -> &'static str {
        "std"
    }

    fn read_dir(&self, dir: &Path) -> Result<Vec<RawEntry>, ReadError> {
        let mut out = Vec::new();
        for entry in fs::read_dir(dir).map_err(|e| ReadError::from_io(&e))? {
            let entry = entry.map_err(|e| ReadError::from_io(&e))?;
            // `DirEntry::metadata` does not follow links. An entry that
            // vanished between listing and stat is skipped.
            let Ok(md) = entry.metadata() else {
                continue;
            };
            out.push(raw_entry(entry.file_name(), &md));
        }
        Ok(out)
    }
}

#[cfg(windows)]
fn raw_entry(name: std::ffi::OsString, md: &fs::Metadata) -> RawEntry {
    use std::os::windows::fs::MetadataExt;
    let attributes = md.file_attributes();
    let reparse_tag = if attributes & attrs::FILE_ATTRIBUTE_REPARSE_POINT == 0 {
        0
    } else if md.file_type().is_symlink() {
        // std reports name surrogates (symlinks and junctions) as symlinks
        // but does not expose the tag itself.
        attrs::REPARSE_TAG_UNKNOWN
    } else {
        // A non-surrogate reparse point (cloud placeholder, dedup, ...).
        // Traversable; placeholder state still comes from the attributes.
        attrs::REPARSE_TAG_UNKNOWN_NON_SURROGATE
    };
    let ft = |raw: u64| i64::try_from(raw).ok().and_then(Ticks::from_filetime);
    RawEntry {
        name,
        attributes,
        reparse_tag,
        logical: md.file_size(),
        allocated: None,
        file_id: None,
        created: ft(md.creation_time()),
        modified: ft(md.last_write_time()),
        accessed: ft(md.last_access_time()),
    }
}

#[cfg(unix)]
fn raw_entry(name: std::ffi::OsString, md: &fs::Metadata) -> RawEntry {
    use std::os::unix::fs::MetadataExt;
    let ft = md.file_type();
    let mut attributes = 0;
    let mut reparse_tag = 0;
    if ft.is_dir() {
        attributes |= attrs::FILE_ATTRIBUTE_DIRECTORY;
    }
    if ft.is_symlink() {
        attributes |= attrs::FILE_ATTRIBUTE_REPARSE_POINT;
        reparse_tag = attrs::IO_REPARSE_TAG_SYMLINK;
    }
    if md.permissions().readonly() {
        attributes |= attrs::FILE_ATTRIBUTE_READONLY;
    }
    let is_file = ft.is_file();
    RawEntry {
        name,
        attributes,
        reparse_tag,
        logical: if is_file { md.len() } else { 0 },
        allocated: is_file.then(|| md.blocks() * 512),
        file_id: Some((u128::from(md.dev()) << 64) | u128::from(md.ino())),
        created: md.created().ok().and_then(Ticks::from_system_time),
        modified: md.modified().ok().and_then(Ticks::from_system_time),
        accessed: md.accessed().ok().and_then(Ticks::from_system_time),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::scan::{CancelToken, ScanConfig, ScanState, scan};
    use crate::testutil::TempDir;
    use crate::tree::{NodeKind, ScanTree, flags};

    #[test]
    fn scans_a_real_directory_with_links() {
        let tmp = TempDir::new("stdfs");
        let root = tmp.path();
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/one.txt"), vec![0u8; 10_000]).unwrap();
        fs::write(root.join("a/b/two.txt"), b"hello").unwrap();
        fs::hard_link(root.join("a/one.txt"), root.join("alias.txt")).unwrap();
        std::os::unix::fs::symlink(root.join("a"), root.join("link-to-a")).unwrap();
        // A link cycle: following it would never terminate.
        std::os::unix::fs::symlink(root, root.join("a/b/cycle")).unwrap();

        let out = scan(
            root,
            &StdDirReader,
            &ScanConfig::default(),
            &CancelToken::new(),
            |_| {},
        );
        assert_eq!(out.state, ScanState::Complete);
        let tree = ScanTree::build(out);
        let totals = tree.totals();
        assert_eq!(totals.files, 3);
        assert_eq!(totals.links, 2);
        assert_eq!(totals.logical_bytes, 10_000 + 5 + 10_000);
        assert_eq!(tree.hard_links().groups, 1);

        let alias = tree.find(Path::new("alias.txt")).unwrap();
        let original = tree.find(Path::new("a/one.txt")).unwrap();
        // "a/one.txt" sorts before "alias.txt" ordinally ('/' < 'l').
        assert!(tree.node(original).has_flag(flags::HARDLINK_PRIMARY));
        assert!(tree.node(alias).has_flag(flags::ALIAS));
        assert_eq!(
            totals.counted_bytes,
            totals.allocated_bytes_naive - u128::from(tree.node(alias).allocated().unwrap())
        );
        let link = tree.find(Path::new("link-to-a")).unwrap();
        assert!(matches!(tree.node(link).kind(), NodeKind::Link(_)));
    }

    #[test]
    fn unreadable_root_is_reported() {
        let out = scan(
            Path::new("/definitely/not/here"),
            &StdDirReader,
            &ScanConfig::default(),
            &CancelToken::new(),
            |_| {},
        );
        assert_eq!(out.state, ScanState::CompleteWithOmissions);
        assert_eq!(out.errors.len(), 1);
    }
}
