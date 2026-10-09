//! Windows file attribute and reparse tag values, plus the classification
//! rules built on them.
//!
//! The values are defined here (rather than imported from the `windows`
//! crate) so the classification logic stays testable on every platform.
//! Other readers translate their native metadata into these values.

pub const FILE_ATTRIBUTE_READONLY: u32 = 0x0000_0001;
pub const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0000_0002;
pub const FILE_ATTRIBUTE_SYSTEM: u32 = 0x0000_0004;
pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
pub const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x0000_0020;
pub const FILE_ATTRIBUTE_SPARSE_FILE: u32 = 0x0000_0200;
pub const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
pub const FILE_ATTRIBUTE_COMPRESSED: u32 = 0x0000_0800;
pub const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
pub const FILE_ATTRIBUTE_ENCRYPTED: u32 = 0x0000_4000;
/// Shares its value with `FILE_ATTRIBUTE_EA`. Microsoft documents that in
/// directory-enumeration results (which is where we read attributes) the bit
/// means "recall on open".
pub const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
pub const FILE_ATTRIBUTE_PINNED: u32 = 0x0008_0000;
pub const FILE_ATTRIBUTE_UNPINNED: u32 = 0x0010_0000;
pub const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

/// Junctions and volume mount points share this tag.
pub const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
pub const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
pub const IO_REPARSE_TAG_DEDUP: u32 = 0x8000_0013;
pub const IO_REPARSE_TAG_APPEXECLINK: u32 = 0x8000_001B;
/// Base cloud-files tag. Providers use `0x9000_X01A` variants.
pub const IO_REPARSE_TAG_CLOUD: u32 = 0x9000_001A;
const CLOUD_TAG_MASK: u32 = 0xFFFF_0FFF;

/// Used by readers that know an entry is a reparse point but cannot read
/// its tag (for example `std::fs` on Windows). Not a real Windows tag.
pub const REPARSE_TAG_UNKNOWN: u32 = 0x0000_0001;
/// Used by readers that know an entry is a reparse point that is *not* a
/// name surrogate, but cannot read its tag. Not a real Windows tag.
pub const REPARSE_TAG_UNKNOWN_NON_SURROGATE: u32 = 0x0000_0002;

/// Bit 29 of a reparse tag marks a *name surrogate*: the reparse point names
/// another file or directory (junctions, symlinks, volume mount points).
pub const REPARSE_NAME_SURROGATE_BIT: u32 = 0x2000_0000;

/// True when the tag marks a name surrogate.
pub fn is_name_surrogate(tag: u32) -> bool {
    tag & REPARSE_NAME_SURROGATE_BIT != 0
}

/// True for any cloud-files provider tag (OneDrive and other sync engines).
pub fn is_cloud_tag(tag: u32) -> bool {
    tag & CLOUD_TAG_MASK == IO_REPARSE_TAG_CLOUD
}

/// What kind of link a name-surrogate reparse point is. Version 1 never
/// traverses any of these (FR03).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkKind {
    /// A junction or a mounted volume; both use the mount-point tag.
    MountPoint,
    Symlink,
    /// Some other name surrogate, or a reparse point whose tag the reader
    /// could not see.
    Other,
}

impl LinkKind {
    pub fn from_tag(tag: u32) -> LinkKind {
        match tag {
            IO_REPARSE_TAG_MOUNT_POINT => LinkKind::MountPoint,
            IO_REPARSE_TAG_SYMLINK => LinkKind::Symlink,
            _ => LinkKind::Other,
        }
    }
}

/// Decide whether an entry is a link that must not be followed.
///
/// A reparse point is a link when it is a name surrogate, or when its tag is
/// unknown. Cloud-files directories are reparse points too, but they are not
/// name surrogates: they *are* the directory, so they are traversed. Skipping
/// every reparse directory would silently drop a whole OneDrive tree.
pub fn link_kind(attributes: u32, reparse_tag: u32) -> Option<LinkKind> {
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0 && reparse_tag == 0 {
        return None;
    }
    if reparse_tag == REPARSE_TAG_UNKNOWN || is_name_surrogate(reparse_tag) {
        return Some(LinkKind::from_tag(reparse_tag));
    }
    None
}

/// Cloud placeholder state derived from directory-enumeration metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Placeholder {
    /// Managed by a sync provider and fully present locally.
    Hydrated,
    /// Some or all content lives only in the cloud. Opening or reading it
    /// can trigger a download, and its allocation is not a measurement of
    /// the file's storage.
    Dehydrated,
}

pub fn placeholder_state(attributes: u32, reparse_tag: u32) -> Option<Placeholder> {
    let recall = FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
        | FILE_ATTRIBUTE_RECALL_ON_OPEN
        | FILE_ATTRIBUTE_OFFLINE;
    if attributes & recall != 0 {
        Some(Placeholder::Dehydrated)
    } else if is_cloud_tag(reparse_tag) {
        Some(Placeholder::Hydrated)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surrogate_bit_matches_documented_tags() {
        assert!(is_name_surrogate(IO_REPARSE_TAG_MOUNT_POINT));
        assert!(is_name_surrogate(IO_REPARSE_TAG_SYMLINK));
        assert!(!is_name_surrogate(IO_REPARSE_TAG_CLOUD));
        assert!(!is_name_surrogate(0x9000_601A));
        assert!(!is_name_surrogate(IO_REPARSE_TAG_DEDUP));
        assert!(!is_name_surrogate(IO_REPARSE_TAG_APPEXECLINK));
    }

    #[test]
    fn cloud_tag_variants() {
        assert!(is_cloud_tag(IO_REPARSE_TAG_CLOUD));
        assert!(is_cloud_tag(0x9000_101A));
        assert!(is_cloud_tag(0x9000_F01A));
        assert!(!is_cloud_tag(IO_REPARSE_TAG_SYMLINK));
    }

    #[test]
    fn cloud_directories_are_traversed_links_are_not() {
        let dir_reparse = FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT;
        assert_eq!(link_kind(dir_reparse, 0x9000_601A), None);
        assert_eq!(
            link_kind(dir_reparse, IO_REPARSE_TAG_MOUNT_POINT),
            Some(LinkKind::MountPoint)
        );
        assert_eq!(
            link_kind(dir_reparse, IO_REPARSE_TAG_SYMLINK),
            Some(LinkKind::Symlink)
        );
        assert_eq!(
            link_kind(dir_reparse, REPARSE_TAG_UNKNOWN),
            Some(LinkKind::Other)
        );
        assert_eq!(link_kind(FILE_ATTRIBUTE_DIRECTORY, 0), None);
    }

    #[test]
    fn placeholder_detection() {
        assert_eq!(placeholder_state(0, 0), None);
        assert_eq!(
            placeholder_state(FILE_ATTRIBUTE_REPARSE_POINT, 0x9000_601A),
            Some(Placeholder::Hydrated)
        );
        assert_eq!(
            placeholder_state(
                FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
                0x9000_601A
            ),
            Some(Placeholder::Dehydrated)
        );
        assert_eq!(
            placeholder_state(FILE_ATTRIBUTE_OFFLINE, 0),
            Some(Placeholder::Dehydrated)
        );
    }
}
