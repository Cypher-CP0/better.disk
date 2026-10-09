//! Filtered aggregation for one navigation level (FR04, FR05).
//!
//! [`summarize_children`] answers "what are the blocks at this folder, under
//! these filters?" in one pass over the folder's contiguous descendant
//! range. Filters apply to individual files; folder values are recomputed
//! from the files that survive, so a folder of many small files never
//! "matches" a minimum-size filter by itself.

use crate::age::{AccessPolicy, AgeBucket, AgeHistogram, DateMetric, bucket_for};
use crate::category::FileType;
use crate::name;
use crate::scan::NodeIndex;
use crate::tree::{Node, NodeKind, ScanTree};

/// Which size drives block area, totals and the minimum-size filter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SizeMetric {
    /// "Size on disk": counted allocation. Hard-link aliases are zero and
    /// unmeasured files are unknown.
    #[default]
    Allocated,
    /// "File size": logical length. Every hard-link name counts its own
    /// length, so this is not unique storage (FR04).
    Logical,
}

impl SizeMetric {
    pub fn value(self, node: &Node) -> Option<u64> {
        match self {
            SizeMetric::Allocated => node.counted(),
            SizeMetric::Logical => Some(node.logical()),
        }
    }
}

/// A set of age buckets, used for date filters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BucketSet(u8);

impl BucketSet {
    pub fn all() -> BucketSet {
        BucketSet(0b11_1111)
    }
    pub fn none() -> BucketSet {
        BucketSet(0)
    }
    pub fn with(mut self, b: AgeBucket) -> BucketSet {
        self.0 |= 1 << b.index();
        self
    }
    pub fn contains(self, b: AgeBucket) -> bool {
        self.0 & (1 << b.index()) != 0
    }
}

#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub file_type: Option<FileType>,
    /// Literal, case-insensitive text matched against the file name and the
    /// names of its folders. Not a pattern language.
    pub search: Option<String>,
    /// Files at least this large under the current metric. A file whose
    /// size is unknown fails a positive threshold.
    pub min_size: Option<u64>,
    /// Only files whose size is unknown under the current metric.
    pub unknown_size_only: bool,
    /// Keep files whose age bucket (for `DateMetric`) is in the set.
    pub date: Option<(DateMetric, BucketSet)>,
}

impl Filter {
    fn folded_search(&self) -> Option<String> {
        self.search
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase)
    }
}

/// View settings that affect values but not which files match.
#[derive(Clone, Copy, Debug, Default)]
pub struct ViewOptions {
    pub metric: SizeMetric,
    /// Date metric used for block shading.
    pub color: DateMetric,
    pub access: AccessPolicy,
}

/// One block at the current level: a direct child file, or a direct child
/// folder with the totals of its matching descendant files.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildSummary {
    pub node: NodeIndex,
    pub kind: NodeKind,
    /// Known bytes of matching files under the current metric.
    pub bytes: u128,
    pub matching_files: u64,
    /// Matching files whose size is unknown under the current metric.
    pub unknown_files: u64,
    /// Known bytes per age bucket of the colour metric.
    pub age: AgeHistogram,
    pub dominant_age: AgeBucket,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelSummary {
    pub parent: NodeIndex,
    /// Children with at least one matching file, largest first.
    pub children: Vec<ChildSummary>,
    /// Denominator for percentages: known bytes of the filtered result.
    pub total_bytes: u128,
    pub matching_files: u64,
    pub unknown_files: u64,
}

impl LevelSummary {
    /// Share of the filtered result, 0.0..=1.0.
    pub fn fraction(&self, child: &ChildSummary) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            child.bytes as f64 / self.total_bytes as f64
        }
    }
}

struct Accumulator {
    node: NodeIndex,
    kind: NodeKind,
    bytes: u128,
    matching: u64,
    unknown: u64,
    age: AgeHistogram,
}

impl Accumulator {
    fn new(node: NodeIndex, kind: NodeKind) -> Accumulator {
        Accumulator {
            node,
            kind,
            bytes: 0,
            matching: 0,
            unknown: 0,
            age: AgeHistogram::default(),
        }
    }

    fn finish(self) -> Option<ChildSummary> {
        (self.matching > 0).then(|| ChildSummary {
            node: self.node,
            kind: self.kind,
            bytes: self.bytes,
            matching_files: self.matching,
            unknown_files: self.unknown,
            dominant_age: self.age.dominant(),
            age: self.age,
        })
    }
}

fn file_matches(
    tree: &ScanTree,
    node: &Node,
    filter: &Filter,
    view: &ViewOptions,
    path_matches: bool,
    search: Option<&str>,
) -> bool {
    if let Some(t) = filter.file_type
        && node.file_type() != t
    {
        return false;
    }
    let size = view.metric.value(node);
    if filter.unknown_size_only && size.is_some() {
        return false;
    }
    if let Some(min) = filter.min_size
        && min > 0
        && size.is_none_or(|s| s < min)
    {
        return false;
    }
    if let Some((metric, set)) = filter.date {
        let bucket = age_bucket(tree, node, metric, view.access);
        if !set.contains(bucket) {
            return false;
        }
    }
    if let Some(term) = search
        && !path_matches
        && !node.folded().contains(term)
    {
        return false;
    }
    true
}

fn age_bucket(tree: &ScanTree, node: &Node, metric: DateMetric, access: AccessPolicy) -> AgeBucket {
    match metric {
        DateMetric::Modified => bucket_for(tree.reference_time(), node.modified()),
        DateMetric::Accessed => access.bucket(tree.reference_time(), node.accessed()),
    }
}

/// Summarise the children of `parent` under `filter`.
pub fn summarize_children(
    tree: &ScanTree,
    parent: NodeIndex,
    filter: &Filter,
    view: &ViewOptions,
) -> LevelSummary {
    let search = filter.folded_search();
    let search = search.as_deref();
    let nodes = tree.nodes();
    let start = parent + 1;
    let end = tree.node(parent).end();

    // Does a folder above the current level already match the search?
    let mut prefix_match = false;
    if let Some(term) = search {
        let mut i = parent;
        while i != 0 {
            if nodes[i as usize].folded().contains(term) {
                prefix_match = true;
                break;
            }
            i = nodes[i as usize].parent().unwrap_or(0);
        }
    }

    let mut out = Vec::new();
    let mut acc: Option<Accumulator> = None;
    let mut child_end = start;
    // (subtree end, folder or an ancestor matched the search)
    let mut stack: Vec<(NodeIndex, bool)> = Vec::new();

    let mut i = start;
    while i < end {
        let node = &nodes[i as usize];
        if i >= child_end {
            if let Some(done) = acc.take().and_then(Accumulator::finish) {
                out.push(done);
            }
            acc = Some(Accumulator::new(i, node.kind()));
            child_end = node.end();
        }
        let mut path_matches = prefix_match;
        if search.is_some() {
            while stack.last().is_some_and(|&(e, _)| e <= i) {
                stack.pop();
            }
            path_matches |= stack.last().is_some_and(|&(_, m)| m);
        }
        match node.kind() {
            NodeKind::Dir => {
                if let Some(term) = search {
                    let m = path_matches || node.folded().contains(term);
                    stack.push((node.end(), m));
                }
            }
            NodeKind::File => {
                if file_matches(tree, node, filter, view, path_matches, search) {
                    let a = acc.as_mut().expect("accumulator set at child start");
                    a.matching += 1;
                    match view.metric.value(node) {
                        Some(v) => {
                            let v = u128::from(v);
                            a.bytes += v;
                            a.age
                                .add(age_bucket(tree, node, view.color, view.access), v);
                        }
                        None => a.unknown += 1,
                    }
                }
            }
            NodeKind::Link(_) => {}
        }
        i += 1;
    }
    if let Some(done) = acc.take().and_then(Accumulator::finish) {
        out.push(done);
    }

    out.sort_by(|a, b| {
        b.bytes
            .cmp(&a.bytes)
            .then_with(|| name::ordinal_cmp(tree.node(a.node).name(), tree.node(b.node).name()))
    });
    let total_bytes = out.iter().map(|c| c.bytes).sum();
    let matching_files = out.iter().map(|c| c.matching_files).sum();
    let unknown_files = out.iter().map(|c| c.unknown_files).sum();
    LevelSummary {
        parent,
        children: out,
        total_bytes,
        matching_files,
        unknown_files,
    }
}

/// Sort key for list views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    SizeDesc,
    NameAsc,
    ModifiedDesc,
}

pub fn sort_children(tree: &ScanTree, children: &mut [ChildSummary], key: SortKey) {
    let by_name = |a: &ChildSummary, b: &ChildSummary| {
        name::ordinal_cmp(tree.node(a.node).name(), tree.node(b.node).name())
    };
    match key {
        SortKey::SizeDesc => {
            children.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| by_name(a, b)))
        }
        SortKey::NameAsc => children.sort_by(by_name),
        SortKey::ModifiedDesc => children.sort_by(|a, b| {
            let ma = tree.node(a.node).modified();
            let mb = tree.node(b.node).modified();
            mb.cmp(&ma).then_with(|| by_name(a, b))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{CancelToken, ScanConfig, scan};
    use crate::testutil::{FakeFs, dir, file_with};
    use crate::time::{TICKS_PER_DAY, Ticks};
    use std::path::Path;

    fn ticks_days_ago(reference: Ticks, d: i64) -> Option<Ticks> {
        reference.checked_add_ticks(-d * TICKS_PER_DAY)
    }

    /// root
    /// ├── Videos/            holiday.mp4 (900, 2 days), clip.mp4 (50, 400 days)
    /// ├── Docs/              notes.txt (10, 40 days), sub/ big.pdf (300, 100 days)
    /// ├── unknown.bin        size 70, allocation unknown
    /// └── readme.md          (5, 2 days)
    fn tree() -> ScanTree {
        let now = Ticks::now();
        let f = |name: &str, size: u64, days: i64| {
            file_with(name, size, Some(size), None, ticks_days_ago(now, days))
        };
        let fs = FakeFs::new()
            .with(
                "root",
                vec![
                    dir("Videos"),
                    dir("Docs"),
                    file_with("unknown.bin", 70, None, None, ticks_days_ago(now, 1)),
                    f("readme.md", 5, 2),
                ],
            )
            .with(
                "root/Videos",
                vec![f("holiday.mp4", 900, 2), f("clip.mp4", 50, 400)],
            )
            .with("root/Docs", vec![f("notes.txt", 10, 40), dir("sub")])
            .with("root/Docs/sub", vec![f("big.pdf", 300, 100)]);
        ScanTree::build(scan(
            Path::new("root"),
            &fs,
            &ScanConfig::default(),
            &CancelToken::new(),
            |_| {},
        ))
    }

    fn names(t: &ScanTree, s: &LevelSummary) -> Vec<String> {
        s.children
            .iter()
            .map(|c| t.node(c.node).display_name().into_owned())
            .collect()
    }

    #[test]
    fn unfiltered_level_sums_descendants() {
        let t = tree();
        let s = summarize_children(&t, 0, &Filter::default(), &ViewOptions::default());
        assert_eq!(
            names(&t, &s),
            ["Videos", "Docs", "readme.md", "unknown.bin"]
        );
        assert_eq!(s.children[0].bytes, 950);
        assert_eq!(s.children[1].bytes, 310);
        assert_eq!(s.total_bytes, 950 + 310 + 5);
        assert_eq!(s.matching_files, 6);
        assert_eq!(s.unknown_files, 1);
        let unknown = s.children.iter().find(|c| c.unknown_files == 1).unwrap();
        assert_eq!(unknown.bytes, 0);
    }

    #[test]
    fn logical_metric_has_no_unknowns() {
        let t = tree();
        let view = ViewOptions {
            metric: SizeMetric::Logical,
            ..ViewOptions::default()
        };
        let s = summarize_children(&t, 0, &Filter::default(), &view);
        assert_eq!(s.unknown_files, 0);
        assert_eq!(s.total_bytes, 950 + 310 + 5 + 70);
    }

    #[test]
    fn children_totals_equal_parent_total_under_any_filter() {
        // AT07-style parity: the sum over children at a level equals the
        // level total, and a folder's own level total equals its block.
        let t = tree();
        let filters = [
            Filter::default(),
            Filter {
                min_size: Some(40),
                ..Filter::default()
            },
            Filter {
                file_type: Some(FileType::Videos),
                ..Filter::default()
            },
            Filter {
                search: Some("Sub".into()),
                ..Filter::default()
            },
        ];
        for filter in &filters {
            for metric in [SizeMetric::Allocated, SizeMetric::Logical] {
                let view = ViewOptions {
                    metric,
                    ..ViewOptions::default()
                };
                let top = summarize_children(&t, 0, filter, &view);
                assert_eq!(
                    top.total_bytes,
                    top.children.iter().map(|c| c.bytes).sum::<u128>()
                );
                for block in &top.children {
                    if block.kind == NodeKind::Dir {
                        let inner = summarize_children(&t, block.node, filter, &view);
                        assert_eq!(inner.total_bytes, block.bytes, "{filter:?} {metric:?}");
                        assert_eq!(inner.matching_files, block.matching_files);
                    }
                }
            }
        }
    }

    #[test]
    fn min_size_applies_to_files_and_unknown_fails_threshold() {
        let t = tree();
        let filter = Filter {
            min_size: Some(60),
            ..Filter::default()
        };
        let s = summarize_children(&t, 0, &filter, &ViewOptions::default());
        // holiday.mp4 900, big.pdf 300; unknown.bin is unknown and excluded.
        assert_eq!(names(&t, &s), ["Videos", "Docs"]);
        assert_eq!(s.total_bytes, 1200);
        assert_eq!(s.unknown_files, 0);

        let unknown_only = Filter {
            unknown_size_only: true,
            ..Filter::default()
        };
        let s = summarize_children(&t, 0, &unknown_only, &ViewOptions::default());
        assert_eq!(names(&t, &s), ["unknown.bin"]);
        assert_eq!(s.unknown_files, 1);
    }

    #[test]
    fn search_matches_file_names_and_folder_names() {
        let t = tree();
        let by_file = Filter {
            search: Some("HOLIDAY".into()),
            ..Filter::default()
        };
        let s = summarize_children(&t, 0, &by_file, &ViewOptions::default());
        assert_eq!(s.matching_files, 1);
        assert_eq!(s.total_bytes, 900);

        let by_folder = Filter {
            search: Some("docs".into()),
            ..Filter::default()
        };
        let s = summarize_children(&t, 0, &by_folder, &ViewOptions::default());
        assert_eq!(names(&t, &s), ["Docs"]);
        assert_eq!(s.matching_files, 2);

        // Inside a matching folder, everything matches.
        let docs = t.find(Path::new("Docs")).unwrap();
        let s = summarize_children(&t, docs, &by_folder, &ViewOptions::default());
        assert_eq!(s.matching_files, 2);

        // A blank search is no search.
        let blank = Filter {
            search: Some("   ".into()),
            ..Filter::default()
        };
        let s = summarize_children(&t, 0, &blank, &ViewOptions::default());
        assert_eq!(s.matching_files, 6);
    }

    #[test]
    fn type_and_date_filters_intersect() {
        let t = tree();
        let filter = Filter {
            file_type: Some(FileType::Videos),
            date: Some((
                DateMetric::Modified,
                BucketSet::none().with(AgeBucket::OverOneYear),
            )),
            ..Filter::default()
        };
        let s = summarize_children(&t, 0, &filter, &ViewOptions::default());
        assert_eq!(s.matching_files, 1);
        assert_eq!(s.total_bytes, 50);
    }

    #[test]
    fn dominant_age_uses_bytes() {
        let t = tree();
        let s = summarize_children(&t, 0, &Filter::default(), &ViewOptions::default());
        let videos = &s.children[0];
        assert_eq!(videos.dominant_age, AgeBucket::Within7Days);
        let docs = &s.children[1];
        assert_eq!(docs.dominant_age, AgeBucket::Days91To365);
    }

    #[test]
    fn sorting_options() {
        let t = tree();
        let mut s = summarize_children(&t, 0, &Filter::default(), &ViewOptions::default());
        sort_children(&t, &mut s.children, SortKey::NameAsc);
        assert_eq!(
            names(&t, &s),
            ["Docs", "Videos", "readme.md", "unknown.bin"]
        );
    }
}
