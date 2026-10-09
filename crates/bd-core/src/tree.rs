//! The in-memory scan index.
//!
//! Nodes are stored in depth-first pre-order. Every node records `end`, the
//! index one past its last descendant, so the descendants of any folder are
//! the contiguous slice `index + 1 .. end`. Filtered folder totals become a
//! single linear pass over that slice instead of a recursive query (D1 in
//! `docs/decisions.md`). Siblings are ordered by ordinal name, which makes
//! the layout independent of worker timing.
//!
//! Building the tree also performs storage accounting (FR02):
//!
//! * hard links are recognised by file identity collisions within the scan,
//!   with no per-file handle opens;
//! * a hard-link group's allocation is counted once, on the member whose
//!   relative path is ordinally first, and the other names become aliases;
//! * dehydrated cloud placeholders have unknown counted allocation;
//! * files without an identity are flagged as deduplication-unverified.

use std::ffi::{OsStr, OsString};
use std::num::NonZeroU128;
use std::path::{Path, PathBuf};

use crate::attrs::{LinkKind, Placeholder};
use crate::category::{self, FileType};
use crate::name::{self, ORDINAL_SEPARATOR, OrdinalUnit};
use crate::raw::{FileId, RawEntry};
use crate::scan::{DirStatus, NO_PARENT, NodeIndex, ScanOutput, ScanState};
use crate::time::Ticks;

const UNKNOWN: u64 = u64::MAX;

/// Per-node flags.
pub mod flags {
    /// One of several names for the same file; its allocation is counted on
    /// another name.
    pub const ALIAS: u16 = 1 << 0;
    /// The name of a hard-link group that carries the group's allocation.
    pub const HARDLINK_PRIMARY: u16 = 1 << 1;
    /// Members of this hard-link group reported different sizes. NTFS
    /// directory entries can be stale for names other than the one most
    /// recently written through.
    pub const SIZE_CONFLICT: u16 = 1 << 2;
    /// Managed by a cloud sync provider.
    pub const PLACEHOLDER: u16 = 1 << 3;
    /// Content is (partly) online-only; allocation is unknown.
    pub const DEHYDRATED: u16 = 1 << 4;
    /// No file identity was available, so hard-link deduplication could not
    /// be verified for this file.
    pub const DEDUP_UNVERIFIED: u16 = 1 << 5;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NodeKind {
    File,
    Dir,
    Link(LinkKind),
}

#[derive(Clone, Debug)]
pub struct Node {
    name: OsString,
    folded: Box<str>,
    file_id: Option<NonZeroU128>,
    parent: NodeIndex,
    end: NodeIndex,
    logical: u64,
    allocated: u64,
    counted: u64,
    created: i64,
    modified: i64,
    accessed: i64,
    attributes: u32,
    reparse_tag: u32,
    kind: NodeKind,
    status: DirStatus,
    file_type: FileType,
    flags: u16,
}

fn opt(v: u64) -> Option<u64> {
    (v != UNKNOWN).then_some(v)
}

impl Node {
    pub fn name(&self) -> &OsStr {
        &self.name
    }
    /// Lossy display form. Never use as an action target (FR03).
    pub fn display_name(&self) -> std::borrow::Cow<'_, str> {
        self.name.to_string_lossy()
    }
    pub fn folded(&self) -> &str {
        &self.folded
    }
    pub fn parent(&self) -> Option<NodeIndex> {
        (self.parent != NO_PARENT).then_some(self.parent)
    }
    /// One past the last descendant.
    pub fn end(&self) -> NodeIndex {
        self.end
    }
    pub fn kind(&self) -> NodeKind {
        self.kind
    }
    pub fn is_file(&self) -> bool {
        self.kind == NodeKind::File
    }
    pub fn is_dir(&self) -> bool {
        self.kind == NodeKind::Dir
    }
    pub fn status(&self) -> DirStatus {
        self.status
    }
    pub fn attributes(&self) -> u32 {
        self.attributes
    }
    pub fn reparse_tag(&self) -> u32 {
        self.reparse_tag
    }
    pub fn logical(&self) -> u64 {
        self.logical
    }
    /// Allocation exactly as the reader reported it.
    pub fn allocated(&self) -> Option<u64> {
        opt(self.allocated)
    }
    /// Allocation counted toward totals: zero for aliases, unknown for
    /// dehydrated placeholders and unmeasured files.
    pub fn counted(&self) -> Option<u64> {
        opt(self.counted)
    }
    pub fn file_id(&self) -> Option<FileId> {
        self.file_id.map(NonZeroU128::get)
    }
    pub fn created(&self) -> Option<Ticks> {
        Ticks::from_filetime(self.created)
    }
    pub fn modified(&self) -> Option<Ticks> {
        Ticks::from_filetime(self.modified)
    }
    pub fn accessed(&self) -> Option<Ticks> {
        Ticks::from_filetime(self.accessed)
    }
    pub fn file_type(&self) -> FileType {
        self.file_type
    }
    pub fn flags(&self) -> u16 {
        self.flags
    }
    pub fn has_flag(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }

    fn from_raw(e: RawEntry, parent: NodeIndex, status: DirStatus) -> Node {
        let kind = match e.link_kind() {
            Some(link) => NodeKind::Link(link),
            None if e.is_dir() => NodeKind::Dir,
            None => NodeKind::File,
        };
        let mut flags = 0;
        let allocated = e.allocated.unwrap_or(UNKNOWN);
        let mut counted = if kind == NodeKind::File {
            allocated
        } else {
            UNKNOWN
        };
        match e.placeholder() {
            Some(Placeholder::Dehydrated) => {
                flags |= flags::PLACEHOLDER | flags::DEHYDRATED;
                counted = UNKNOWN;
            }
            Some(Placeholder::Hydrated) => flags |= flags::PLACEHOLDER,
            None => {}
        }
        let file_id = e.file_id.and_then(NonZeroU128::new);
        if kind == NodeKind::File && file_id.is_none() {
            flags |= flags::DEDUP_UNVERIFIED;
        }
        Node {
            folded: name::fold_for_search(&e.name),
            file_type: if kind == NodeKind::File {
                category::classify(&e.name)
            } else {
                FileType::Other
            },
            name: e.name,
            file_id,
            parent,
            end: 0,
            logical: if kind == NodeKind::File { e.logical } else { 0 },
            allocated,
            counted,
            created: e.created.map_or(0, Ticks::raw),
            modified: e.modified.map_or(0, Ticks::raw),
            accessed: e.accessed.map_or(0, Ticks::raw),
            attributes: e.attributes,
            reparse_tag: e.reparse_tag,
            kind,
            status,
            flags,
        }
    }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardLinkSummary {
    /// Groups of two or more names sharing one file identity in this scan.
    pub groups: u64,
    /// Names beyond the first in each group.
    pub alias_names: u64,
    /// Groups whose members reported different sizes.
    pub groups_with_size_conflict: u64,
    /// Files for which no identity was available.
    pub unverified_files: u64,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeTotals {
    pub files: u64,
    pub dirs: u64,
    pub links: u64,
    pub zero_byte_files: u64,
    pub logical_bytes: u128,
    /// Sum of reported allocation over files, counting every hard-link name.
    pub allocated_bytes_naive: u128,
    /// Sum of counted allocation: each file identity once.
    pub counted_bytes: u128,
    pub unknown_allocation_files: u64,
    pub hydrated_placeholders: u64,
    pub dehydrated_placeholders: u64,
    /// What the reader reported as allocation for dehydrated placeholders;
    /// excluded from `counted_bytes`.
    pub dehydrated_reported_bytes: u128,
    pub failed_dirs: u64,
    pub unscanned_dirs: u64,
}

pub struct ScanTree {
    nodes: Vec<Node>,
    root_path: PathBuf,
    reader: &'static str,
    reference_time: Ticks,
    state: ScanState,
    hard_links: HardLinkSummary,
    totals: TreeTotals,
}

impl ScanTree {
    pub fn build(output: ScanOutput) -> ScanTree {
        let ScanOutput {
            root,
            reader,
            nodes: flat,
            state,
            started_at,
            ..
        } = output;
        let n = flat.len();

        // Children in compressed-sparse-row form, sorted by ordinal name.
        let mut counts = vec![0u32; n + 1];
        for node in flat.iter().skip(1) {
            counts[node.parent as usize] += 1;
        }
        let mut offsets = vec![0usize; n + 1];
        for i in 0..n {
            offsets[i + 1] = offsets[i] + counts[i] as usize;
        }
        let mut cursor = offsets.clone();
        let mut children = vec![0u32; n.saturating_sub(1)];
        for (i, node) in flat.iter().enumerate().skip(1) {
            let p = node.parent as usize;
            children[cursor[p]] = i as u32;
            cursor[p] += 1;
        }
        for i in 0..n {
            children[offsets[i]..offsets[i + 1]].sort_by(|&a, &b| {
                name::ordinal_cmp(&flat[a as usize].entry.name, &flat[b as usize].entry.name)
            });
        }

        // Subtree sizes: a child is always discovered after its parent, so
        // walking indices downward accumulates complete subtrees.
        let mut size = vec![1u32; n];
        for i in (1..n).rev() {
            let p = flat[i].parent as usize;
            size[p] += size[i];
        }

        // Pre-order positions.
        let mut order = Vec::with_capacity(n);
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            order.push(i);
            let kids = &children[offsets[i as usize]..offsets[i as usize + 1]];
            stack.extend(kids.iter().rev());
        }
        let mut new_index = vec![0u32; n];
        for (pos, &old) in order.iter().enumerate() {
            new_index[old as usize] = pos as u32;
        }

        // Convert in discovery order, fix links to pre-order positions, then
        // permute in place (no second full copy of the nodes).
        let mut nodes: Vec<Node> = flat
            .into_iter()
            .map(|f| Node::from_raw(f.entry, f.parent, f.status))
            .collect();
        for (old, node) in nodes.iter_mut().enumerate() {
            if node.parent != NO_PARENT {
                node.parent = new_index[node.parent as usize];
            }
            node.end = new_index[old] + size[old];
        }
        let mut dest = new_index;
        for i in 0..n {
            while dest[i] as usize != i {
                let j = dest[i] as usize;
                nodes.swap(i, j);
                dest.swap(i, j);
            }
        }

        let mut tree = ScanTree {
            nodes,
            root_path: root,
            reader,
            reference_time: started_at,
            state,
            hard_links: HardLinkSummary::default(),
            totals: TreeTotals::default(),
        };
        tree.account_hard_links();
        tree.compute_totals();
        tree
    }

    fn account_hard_links(&mut self) {
        let mut ids: Vec<(u128, NodeIndex)> = Vec::with_capacity(self.nodes.len());
        ids.extend(
            self.nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.is_file() && !n.has_flag(flags::DEHYDRATED))
                .filter_map(|(i, n)| n.file_id().map(|id| (id, i as NodeIndex))),
        );
        ids.sort_unstable();

        let mut summary = HardLinkSummary {
            unverified_files: self
                .nodes
                .iter()
                .filter(|n| n.has_flag(flags::DEDUP_UNVERIFIED))
                .count() as u64,
            ..HardLinkSummary::default()
        };

        let mut start = 0;
        while start < ids.len() {
            let mut end = start + 1;
            while end < ids.len() && ids[end].0 == ids[start].0 {
                end += 1;
            }
            if end - start > 1 {
                let members: Vec<NodeIndex> = ids[start..end].iter().map(|&(_, i)| i).collect();
                self.mark_group(&members, &mut summary);
            }
            start = end;
        }
        self.hard_links = summary;
    }

    fn mark_group(&mut self, members: &[NodeIndex], summary: &mut HardLinkSummary) {
        let first = &self.nodes[members[0] as usize];
        let conflict = members.iter().any(|&m| {
            let n = &self.nodes[m as usize];
            n.logical != first.logical || n.allocated != first.allocated
        });
        let group_alloc = members
            .iter()
            .filter_map(|&m| self.nodes[m as usize].allocated())
            .max()
            .unwrap_or(UNKNOWN);
        let primary = members
            .iter()
            .copied()
            .min_by(|&a, &b| self.rel_path_units(a).cmp(&self.rel_path_units(b)))
            .expect("group has members");

        for &m in members {
            let n = &mut self.nodes[m as usize];
            if m == primary {
                n.counted = group_alloc;
                n.flags |= flags::HARDLINK_PRIMARY;
            } else {
                n.counted = 0;
                n.flags |= flags::ALIAS;
            }
            if conflict {
                n.flags |= flags::SIZE_CONFLICT;
            }
        }
        summary.groups += 1;
        summary.alias_names += members.len() as u64 - 1;
        if conflict {
            summary.groups_with_size_conflict += 1;
        }
    }

    fn compute_totals(&mut self) {
        let mut t = TreeTotals::default();
        for n in &self.nodes[1..] {
            match n.kind {
                NodeKind::Dir => {
                    t.dirs += 1;
                    match n.status {
                        DirStatus::Failed(_) => t.failed_dirs += 1,
                        DirStatus::Unscanned => t.unscanned_dirs += 1,
                        _ => {}
                    }
                }
                NodeKind::Link(_) => t.links += 1,
                NodeKind::File => {
                    t.files += 1;
                    if n.logical == 0 {
                        t.zero_byte_files += 1;
                    }
                    t.logical_bytes += u128::from(n.logical);
                    if let Some(a) = n.allocated() {
                        t.allocated_bytes_naive += u128::from(a);
                    }
                    match n.counted() {
                        Some(c) => t.counted_bytes += u128::from(c),
                        None => t.unknown_allocation_files += 1,
                    }
                    if n.has_flag(flags::DEHYDRATED) {
                        t.dehydrated_placeholders += 1;
                        t.dehydrated_reported_bytes += u128::from(n.allocated().unwrap_or(0));
                    } else if n.has_flag(flags::PLACEHOLDER) {
                        t.hydrated_placeholders += 1;
                    }
                }
            }
        }
        if let Some(root) = self.nodes.first() {
            match root.status {
                DirStatus::Failed(_) => t.failed_dirs += 1,
                DirStatus::Unscanned => t.unscanned_dirs += 1,
                _ => {}
            }
        }
        self.totals = t;
    }

    /// Relative path from the root in ordinal units, components joined by
    /// the native separator. Used for the stable hard-link primary choice.
    fn rel_path_units(&self, index: NodeIndex) -> Vec<OrdinalUnit> {
        let mut chain = Vec::new();
        let mut i = index;
        while i != 0 {
            chain.push(i);
            i = self.nodes[i as usize].parent;
        }
        let mut out = Vec::new();
        for (k, &c) in chain.iter().rev().enumerate() {
            if k > 0 {
                out.push(ORDINAL_SEPARATOR);
            }
            name::push_ordinal_units(&self.nodes[c as usize].name, &mut out);
        }
        out
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn node(&self, index: NodeIndex) -> &Node {
        &self.nodes[index as usize]
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub fn reader(&self) -> &'static str {
        self.reader
    }

    /// Scan start time; the reference for date buckets.
    pub fn reference_time(&self) -> Ticks {
        self.reference_time
    }

    pub fn state(&self) -> ScanState {
        self.state
    }

    pub fn hard_links(&self) -> &HardLinkSummary {
        &self.hard_links
    }

    pub fn totals(&self) -> &TreeTotals {
        &self.totals
    }

    /// Immediate children of a node, in ordinal name order.
    pub fn children(&self, index: NodeIndex) -> Children<'_> {
        let n = &self.nodes[index as usize];
        Children {
            nodes: &self.nodes,
            next: index + 1,
            end: n.end,
        }
    }

    /// Path relative to the scan root.
    pub fn rel_path(&self, index: NodeIndex) -> PathBuf {
        let mut chain = Vec::new();
        let mut i = index;
        while i != 0 {
            chain.push(i);
            i = self.nodes[i as usize].parent;
        }
        chain
            .iter()
            .rev()
            .map(|&c| self.nodes[c as usize].name.as_os_str())
            .collect()
    }

    /// Full native path. Built from native names, so it is a valid action
    /// target even when the display form is lossy.
    pub fn path(&self, index: NodeIndex) -> PathBuf {
        self.root_path.join(self.rel_path(index))
    }

    pub fn depth(&self, index: NodeIndex) -> usize {
        let mut d = 0;
        let mut i = index;
        while i != 0 {
            d += 1;
            i = self.nodes[i as usize].parent;
        }
        d
    }

    /// Find a node by its path relative to the root.
    pub fn find(&self, rel: &Path) -> Option<NodeIndex> {
        let mut current = 0;
        for component in rel.components() {
            let name = component.as_os_str();
            current = self
                .children(current)
                .find(|&c| self.nodes[c as usize].name == name)?;
        }
        Some(current)
    }
}

pub struct Children<'a> {
    nodes: &'a [Node],
    next: NodeIndex,
    end: NodeIndex,
}

impl Iterator for Children<'_> {
    type Item = NodeIndex;
    fn next(&mut self) -> Option<NodeIndex> {
        if self.next >= self.end {
            return None;
        }
        let i = self.next;
        self.next = self.nodes[i as usize].end;
        Some(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attrs::{
        FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_REPARSE_POINT, IO_REPARSE_TAG_SYMLINK,
    };
    use crate::scan::{CancelToken, ScanConfig, scan};
    use crate::testutil::{FakeFs, dir, file, file_with};

    fn build(fs: &FakeFs) -> ScanTree {
        ScanTree::build(scan(
            Path::new("root"),
            fs,
            &ScanConfig::default(),
            &CancelToken::new(),
            |_| {},
        ))
    }

    fn sample() -> FakeFs {
        FakeFs::new()
            .with(
                "root",
                vec![dir("b"), file("z.txt", 1), dir("a"), file("A.txt", 2)],
            )
            .with("root/a", vec![file("2", 20), dir("inner"), file("1", 10)])
            .with("root/a/inner", vec![file("deep", 5)])
            .with("root/b", vec![file("only", 7)])
    }

    #[test]
    fn preorder_ranges_are_consistent() {
        let t = build(&sample());
        assert_eq!(t.len(), 10);
        assert_eq!(t.node(0).end(), 10);
        for (i, n) in t.nodes().iter().enumerate() {
            let i = i as u32;
            assert!(n.end() > i);
            if let Some(p) = n.parent() {
                assert!(p < i);
                assert!(t.node(p).end() >= n.end());
            }
            for c in t.children(i) {
                assert_eq!(t.node(c).parent(), Some(i));
            }
        }
    }

    #[test]
    fn siblings_are_in_ordinal_order() {
        let t = build(&sample());
        let names: Vec<String> = t
            .children(0)
            .map(|c| t.node(c).display_name().into_owned())
            .collect();
        assert_eq!(names, ["A.txt", "a", "b", "z.txt"]);
        let a = t.find(Path::new("a")).unwrap();
        let names: Vec<String> = t
            .children(a)
            .map(|c| t.node(c).display_name().into_owned())
            .collect();
        assert_eq!(names, ["1", "2", "inner"]);
    }

    #[test]
    fn paths_round_trip() {
        let t = build(&sample());
        let deep = t.find(Path::new("a/inner/deep")).unwrap();
        assert_eq!(t.rel_path(deep), Path::new("a").join("inner").join("deep"));
        assert_eq!(
            t.path(deep),
            Path::new("root").join("a").join("inner").join("deep")
        );
        assert_eq!(t.depth(deep), 3);
    }

    #[test]
    fn totals() {
        let t = build(&sample());
        let tot = t.totals();
        assert_eq!(tot.files, 6);
        assert_eq!(tot.dirs, 3);
        assert_eq!(tot.logical_bytes, 45);
        assert_eq!(tot.unknown_allocation_files, 0);
    }

    #[test]
    fn hard_links_count_once_on_ordinally_first_path() {
        let id = Some(0xABCD);
        let fs = FakeFs::new()
            .with(
                "root",
                vec![
                    dir("b"),
                    dir("a"),
                    file_with("solo", 100, Some(4096), Some(1), None),
                ],
            )
            .with(
                "root/a",
                vec![file_with("link2", 9000, Some(12288), id, None)],
            )
            .with(
                "root/b",
                vec![file_with("link1", 9000, Some(12288), id, None)],
            );
        let t = build(&fs);
        let primary = t.find(Path::new("a/link2")).unwrap();
        let alias = t.find(Path::new("b/link1")).unwrap();
        assert!(t.node(primary).has_flag(flags::HARDLINK_PRIMARY));
        assert_eq!(t.node(primary).counted(), Some(12288));
        assert!(t.node(alias).has_flag(flags::ALIAS));
        assert_eq!(t.node(alias).counted(), Some(0));
        assert_eq!(t.totals().counted_bytes, 12288 + 4096);
        assert_eq!(t.totals().allocated_bytes_naive, 2 * 12288 + 4096);
        // Logical mode counts each name's length.
        assert_eq!(t.totals().logical_bytes, 9000 * 2 + 100);
        assert_eq!(t.hard_links().groups, 1);
        assert_eq!(t.hard_links().alias_names, 1);
        assert_eq!(t.hard_links().groups_with_size_conflict, 0);
    }

    #[test]
    fn stale_hard_link_sizes_are_flagged_and_max_allocation_counted() {
        let id = Some(7);
        let fs = FakeFs::new().with(
            "root",
            vec![
                file_with("old-name", 3_000, Some(4096), id, None),
                file_with("new-name", 4_000, Some(8192), id, None),
            ],
        );
        let t = build(&fs);
        assert_eq!(t.hard_links().groups_with_size_conflict, 1);
        let primary = t.find(Path::new("new-name")).unwrap();
        assert!(t.node(primary).has_flag(flags::SIZE_CONFLICT));
        assert_eq!(t.node(primary).counted(), Some(8192));
        assert_eq!(t.totals().counted_bytes, 8192);
    }

    #[test]
    fn dehydrated_placeholders_have_unknown_allocation() {
        let mut online = file("online.docx", 5_000_000);
        online.attributes = FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS;
        online.reparse_tag = 0x9000_601A;
        online.allocated = Some(0);
        let fs = FakeFs::new().with("root", vec![online, file("local.txt", 10)]);
        let t = build(&fs);
        let n = t.find(Path::new("online.docx")).unwrap();
        assert_eq!(t.node(n).counted(), None);
        assert!(t.node(n).has_flag(flags::DEHYDRATED));
        assert_eq!(t.totals().unknown_allocation_files, 1);
        assert_eq!(t.totals().dehydrated_placeholders, 1);
    }

    #[test]
    fn links_are_nodes_without_size() {
        let mut link = dir("to-elsewhere");
        link.attributes |= FILE_ATTRIBUTE_REPARSE_POINT;
        link.reparse_tag = IO_REPARSE_TAG_SYMLINK;
        let fs = FakeFs::new().with("root", vec![link]);
        let t = build(&fs);
        let l = t.find(Path::new("to-elsewhere")).unwrap();
        assert_eq!(t.node(l).kind(), NodeKind::Link(LinkKind::Symlink));
        assert_eq!(t.node(l).counted(), None);
        assert_eq!(t.totals().links, 1);
        assert_eq!(t.totals().files, 0);
    }

    #[test]
    fn files_without_ids_are_unverified() {
        let fs = FakeFs::new().with("root", vec![file("a", 1), file("b", 1)]);
        let t = build(&fs);
        assert_eq!(t.hard_links().unverified_files, 2);
    }
}
