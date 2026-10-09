//! What a correct scan of a fixture must report, and the check against a
//! finished scan.
//!
//! Paths are `/`-separated and relative to the fixture root. Names that are
//! not valid Unicode cannot be written in JSON faithfully, so they are
//! checked by count ([`Expectation::NonUnicodeNames`]).

use std::path::Path;

use bd_core::age::{AgeBucket, bucket_for};
use bd_core::attrs::LinkKind;
use bd_core::category::FileType;
use bd_core::raw::ReadErrorKind;
use bd_core::scan::{DirStatus, NodeIndex};
use bd_core::time::Ticks;
use bd_core::tree::{NodeKind, ScanTree, flags};
use serde::{Deserialize, Serialize};

pub const MANIFEST_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: u32,
    pub tool_version: String,
    /// `windows`, `linux`, `macos`, ...
    pub platform: String,
    /// FILETIME ticks. Date-bucket expectations are relative to this, so a
    /// fixture checks the same way weeks after it was made.
    pub created_at_ticks: i64,
    pub created_at: String,
    pub seed: u64,
    pub bulk: Option<BulkSummary>,
    pub expectations: Vec<Expectation>,
    /// Cases that could not be built here, and why.
    pub skipped: Vec<Skipped>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkSummary {
    pub files: u64,
    pub dirs: u64,
    pub logical_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub case: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Expectation {
    /// A file with an exact logical size.
    FileSize {
        path: String,
        logical: u64,
    },
    /// Totals over everything below a directory (not counting itself).
    SubtreeTotals {
        path: String,
        files: u64,
        dirs: u64,
        logical: u64,
    },
    /// Names of one file. Exactly one is the primary; the rest are aliases
    /// counted at zero (FR02).
    HardLinkGroup {
        paths: Vec<String>,
        logical: u64,
    },
    /// A hard-link group written through one name after linking. On NTFS
    /// the other name's directory entry can keep the old size; the scan
    /// should flag the conflict. Observed, not asserted.
    StaleHardLink {
        paths: Vec<String>,
        final_logical: u64,
    },
    /// A link that must be recorded and not traversed (FR03).
    Link {
        path: String,
        link_kind: Option<LinkKind>,
    },
    /// A directory that cannot be listed (FR01: recorded, scan continues).
    Denied {
        path: String,
        effective: bool,
    },
    Sparse {
        path: String,
        logical: u64,
        max_allocated: u64,
    },
    Compressed {
        path: String,
        logical: u64,
    },
    /// A file with an alternate data stream. Version 1 reports the main
    /// stream only; the stream's bytes are a documented gap.
    AlternateStream {
        path: String,
        main_logical: u64,
        stream_bytes: u64,
    },
    AgeBucket {
        path: String,
        bucket: AgeBucket,
    },
    FileType {
        path: String,
        file_type: FileType,
    },
    Exists {
        path: String,
    },
    NonUnicodeNames {
        parent: String,
        count: usize,
    },
    /// Two names differing only in case, in one directory.
    CaseSensitivePair {
        parent: String,
        names: [String; 2],
    },
}

impl Expectation {
    pub fn label(&self) -> String {
        match self {
            Expectation::FileSize { path, .. } => format!("file size: {path}"),
            Expectation::SubtreeTotals { path, .. } => format!("subtree totals: {path}"),
            Expectation::HardLinkGroup { paths, .. } => {
                format!("hard-link group: {}", paths.join(", "))
            }
            Expectation::StaleHardLink { paths, .. } => {
                format!("stale hard-link sizes: {}", paths.join(", "))
            }
            Expectation::Link { path, .. } => format!("link not traversed: {path}"),
            Expectation::Denied { path, .. } => format!("denied directory: {path}"),
            Expectation::Sparse { path, .. } => format!("sparse allocation: {path}"),
            Expectation::Compressed { path, .. } => format!("compressed allocation: {path}"),
            Expectation::AlternateStream { path, .. } => format!("alternate stream: {path}"),
            Expectation::AgeBucket { path, .. } => format!("age bucket: {path}"),
            Expectation::FileType { path, .. } => format!("file type: {path}"),
            Expectation::Exists { path } => format!("exists: {path}"),
            Expectation::NonUnicodeNames { parent, .. } => {
                format!("non-Unicode names under: {parent}")
            }
            Expectation::CaseSensitivePair { parent, .. } => {
                format!("case-sensitive pair under: {parent}")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckStatus {
    Pass,
    Fail,
    /// A measurement to record, not a pass/fail assertion.
    Observe,
    /// Not checkable with this reader or environment.
    Skip,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub check: String,
    pub status: CheckStatus,
    pub detail: String,
}

impl CheckResult {
    fn new(check: String, status: CheckStatus, detail: impl Into<String>) -> CheckResult {
        CheckResult {
            check,
            status,
            detail: detail.into(),
        }
    }
}

/// Compare a scan with a fixture manifest. `base` is the fixture root
/// relative to the scan root (empty when the scan root is the fixture
/// root).
pub fn check(tree: &ScanTree, manifest: &Manifest, base: &Path) -> Vec<CheckResult> {
    let find = |p: &str| tree.find(&base.join(crate::rel(p)));
    let reference = Ticks::from_filetime(manifest.created_at_ticks).unwrap_or(Ticks::now());
    let mut out = Vec::new();

    let missing = |label: String, p: &str| {
        CheckResult::new(
            label,
            CheckStatus::Fail,
            format!("{p} not found in the scan"),
        )
    };

    for e in &manifest.expectations {
        let label = e.label();
        let result = match e {
            Expectation::FileSize { path, logical } => match find(path) {
                None => missing(label, path),
                Some(i) => {
                    let n = tree.node(i);
                    if n.is_file() && n.logical() == *logical {
                        CheckResult::new(label, CheckStatus::Pass, format!("{logical} bytes"))
                    } else {
                        CheckResult::new(
                            label,
                            CheckStatus::Fail,
                            format!(
                                "expected file of {logical} bytes, got {:?} of {}",
                                n.kind(),
                                n.logical()
                            ),
                        )
                    }
                }
            },
            Expectation::SubtreeTotals {
                path,
                files,
                dirs,
                logical,
            } => match find(path) {
                None => missing(label, path),
                Some(i) => {
                    let (f, d, l) = subtree_totals(tree, i);
                    if (f, d, l) == (*files, *dirs, *logical) {
                        CheckResult::new(
                            label,
                            CheckStatus::Pass,
                            format!("{f} files, {d} folders, {l} bytes"),
                        )
                    } else {
                        CheckResult::new(
                            label,
                            CheckStatus::Fail,
                            format!(
                                "expected {files} files, {dirs} folders, {logical} bytes; got {f}, {d}, {l}"
                            ),
                        )
                    }
                }
            },
            Expectation::HardLinkGroup { paths, logical } => {
                check_hard_links(tree, &find, label, paths, *logical)
            }
            Expectation::StaleHardLink {
                paths,
                final_logical,
            } => {
                let mut parts = Vec::new();
                for p in paths {
                    match find(p) {
                        Some(i) => {
                            let n = tree.node(i);
                            parts.push(format!(
                                "{p}: {} bytes{}",
                                n.logical(),
                                if n.has_flag(flags::SIZE_CONFLICT) {
                                    " (size conflict flagged)"
                                } else {
                                    ""
                                }
                            ));
                        }
                        None => parts.push(format!("{p}: missing")),
                    }
                }
                CheckResult::new(
                    label,
                    CheckStatus::Observe,
                    format!("true size {final_logical}; {}", parts.join("; ")),
                )
            }
            Expectation::Link { path, link_kind } => match find(path) {
                None => missing(label, path),
                Some(i) => {
                    let n = tree.node(i);
                    let traversed = tree.children(i).next().is_some();
                    match n.kind() {
                        NodeKind::Link(k) if !traversed => {
                            if link_kind.is_none_or(|want| want == k) {
                                CheckResult::new(
                                    label,
                                    CheckStatus::Pass,
                                    format!("{k:?}, not traversed"),
                                )
                            } else {
                                CheckResult::new(
                                    label,
                                    CheckStatus::Fail,
                                    format!("expected {link_kind:?}, got {k:?}"),
                                )
                            }
                        }
                        k => CheckResult::new(
                            label,
                            CheckStatus::Fail,
                            format!(
                                "expected an untraversed link, got {k:?} (children: {traversed})"
                            ),
                        ),
                    }
                }
            },
            Expectation::Denied { path, effective } => match find(path) {
                None => missing(label, path),
                Some(i) => match (tree.node(i).status(), effective) {
                    (DirStatus::Failed(ReadErrorKind::AccessDenied), _) => {
                        CheckResult::new(label, CheckStatus::Pass, "recorded as access denied")
                    }
                    (s, false) => CheckResult::new(
                        label,
                        CheckStatus::Skip,
                        format!("the denial had no effect for this user (status {s:?})"),
                    ),
                    (s, true) => CheckResult::new(
                        label,
                        CheckStatus::Fail,
                        format!("expected access denied, got {s:?}"),
                    ),
                },
            },
            Expectation::Sparse {
                path,
                logical,
                max_allocated,
            } => match find(path) {
                None => missing(label, path),
                Some(i) => {
                    let n = tree.node(i);
                    match n.allocated() {
                        _ if n.logical() != *logical => CheckResult::new(
                            label,
                            CheckStatus::Fail,
                            format!("logical {} != {logical}", n.logical()),
                        ),
                        None => CheckResult::new(
                            label,
                            CheckStatus::Skip,
                            "reader reports no allocation",
                        ),
                        Some(a) if a <= *max_allocated => CheckResult::new(
                            label,
                            CheckStatus::Pass,
                            format!("logical {logical}, allocated {a}"),
                        ),
                        Some(a) => CheckResult::new(
                            label,
                            CheckStatus::Fail,
                            format!("allocated {a} > {max_allocated}"),
                        ),
                    }
                }
            },
            Expectation::Compressed { path, logical } => match find(path) {
                None => missing(label, path),
                Some(i) => {
                    let n = tree.node(i);
                    match n.allocated() {
                        _ if n.logical() != *logical => CheckResult::new(
                            label,
                            CheckStatus::Fail,
                            format!("logical {} != {logical}", n.logical()),
                        ),
                        None => CheckResult::new(
                            label,
                            CheckStatus::Skip,
                            "reader reports no allocation",
                        ),
                        Some(a) if a < *logical => CheckResult::new(
                            label,
                            CheckStatus::Pass,
                            format!("logical {logical}, allocated {a}"),
                        ),
                        Some(a) => CheckResult::new(
                            label,
                            CheckStatus::Fail,
                            format!("allocated {a} is not below logical {logical}"),
                        ),
                    }
                }
            },
            Expectation::AlternateStream {
                path,
                main_logical,
                stream_bytes,
            } => match find(path) {
                None => missing(label, path),
                Some(i) => {
                    let n = tree.node(i);
                    let status = if n.logical() == *main_logical {
                        CheckStatus::Pass
                    } else {
                        CheckStatus::Fail
                    };
                    CheckResult::new(
                        label,
                        status,
                        format!(
                            "main stream {} bytes reported; {stream_bytes} stream bytes not counted (documented gap); allocated {:?}",
                            n.logical(),
                            n.allocated()
                        ),
                    )
                }
            },
            Expectation::AgeBucket { path, bucket } => match find(path) {
                None => missing(label, path),
                Some(i) => {
                    let got = bucket_for(reference, tree.node(i).modified());
                    let status = if got == *bucket {
                        CheckStatus::Pass
                    } else {
                        CheckStatus::Fail
                    };
                    CheckResult::new(label, status, format!("expected {bucket:?}, got {got:?}"))
                }
            },
            Expectation::FileType { path, file_type } => match find(path) {
                None => missing(label, path),
                Some(i) => {
                    let got = tree.node(i).file_type();
                    let status = if got == *file_type {
                        CheckStatus::Pass
                    } else {
                        CheckStatus::Fail
                    };
                    CheckResult::new(
                        label,
                        status,
                        format!("expected {file_type:?}, got {got:?}"),
                    )
                }
            },
            Expectation::Exists { path } => match find(path) {
                None => missing(label, path),
                Some(i) => {
                    // The native path rebuilt from the tree must reach the
                    // same file (names are lossless, FR03).
                    let p = tree.path(i);
                    if std::fs::symlink_metadata(&p).is_ok() {
                        CheckResult::new(label, CheckStatus::Pass, "found; native path resolves")
                    } else {
                        CheckResult::new(
                            label,
                            CheckStatus::Fail,
                            "found, but its rebuilt path does not resolve",
                        )
                    }
                }
            },
            Expectation::NonUnicodeNames { parent, count } => match find(parent) {
                None => missing(label, parent),
                Some(i) => {
                    let found: Vec<NodeIndex> = tree
                        .children(i)
                        .filter(|&c| tree.node(c).name().to_str().is_none())
                        .collect();
                    let resolves = found
                        .iter()
                        .all(|&c| std::fs::symlink_metadata(tree.path(c)).is_ok());
                    let status = if found.len() == *count && resolves {
                        CheckStatus::Pass
                    } else {
                        CheckStatus::Fail
                    };
                    CheckResult::new(
                        label,
                        status,
                        format!(
                            "expected {count}, found {} (paths resolve: {resolves})",
                            found.len()
                        ),
                    )
                }
            },
            Expectation::CaseSensitivePair { parent, names } => {
                let a = find(&format!("{parent}/{}", names[0]));
                let b = find(&format!("{parent}/{}", names[1]));
                match (a, b) {
                    (Some(a), Some(b)) if a != b => CheckResult::new(
                        label,
                        CheckStatus::Pass,
                        "both names present as separate entries",
                    ),
                    _ => {
                        CheckResult::new(label, CheckStatus::Fail, format!("found {a:?} and {b:?}"))
                    }
                }
            }
        };
        out.push(result);
    }

    // Every unreadable directory must be one the fixture denied on purpose.
    let denied: Vec<_> = manifest
        .expectations
        .iter()
        .filter_map(|e| match e {
            Expectation::Denied { path, .. } => find(path),
            _ => None,
        })
        .collect();
    let unexpected: Vec<String> = (0..tree.len() as NodeIndex)
        .filter(|&i| matches!(tree.node(i).status(), DirStatus::Failed(_)) && !denied.contains(&i))
        .map(|i| tree.rel_path(i).to_string_lossy().into_owned())
        .collect();
    out.push(CheckResult::new(
        "no unexpected unreadable folders".into(),
        if unexpected.is_empty() {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        if unexpected.is_empty() {
            "none".to_string()
        } else {
            unexpected.join(", ")
        },
    ));
    out
}

fn subtree_totals(tree: &ScanTree, i: NodeIndex) -> (u64, u64, u64) {
    let end = tree.node(i).end();
    let (mut files, mut dirs, mut logical) = (0, 0, 0);
    for j in i + 1..end {
        let n = tree.node(j);
        if n.is_file() {
            files += 1;
            logical += n.logical();
        } else if n.is_dir() {
            dirs += 1;
        }
    }
    (files, dirs, logical)
}

fn check_hard_links(
    tree: &ScanTree,
    find: &dyn Fn(&str) -> Option<NodeIndex>,
    label: String,
    paths: &[String],
    logical: u64,
) -> CheckResult {
    let mut nodes = Vec::new();
    for p in paths {
        match find(p) {
            Some(i) => nodes.push(tree.node(i)),
            None => {
                return CheckResult::new(
                    label,
                    CheckStatus::Fail,
                    format!("{p} not found in the scan"),
                );
            }
        }
    }
    if nodes.iter().any(|n| n.file_id().is_none()) {
        return CheckResult::new(
            label,
            CheckStatus::Skip,
            "reader provides no file identities; deduplication unverified",
        );
    }
    let same_id = nodes.windows(2).all(|w| w[0].file_id() == w[1].file_id());
    let primaries = nodes
        .iter()
        .filter(|n| n.has_flag(flags::HARDLINK_PRIMARY))
        .count();
    let aliases = nodes.iter().filter(|n| n.has_flag(flags::ALIAS)).count();
    let alias_counted_zero = nodes
        .iter()
        .filter(|n| n.has_flag(flags::ALIAS))
        .all(|n| n.counted() == Some(0));
    let sizes_ok = nodes.iter().all(|n| n.logical() == logical);
    if same_id && primaries == 1 && aliases == nodes.len() - 1 && alias_counted_zero && sizes_ok {
        CheckResult::new(
            label,
            CheckStatus::Pass,
            format!("one identity, 1 primary, {aliases} aliases counted at zero"),
        )
    } else {
        CheckResult::new(
            label,
            CheckStatus::Fail,
            format!(
                "same id: {same_id}, primaries: {primaries}, aliases: {aliases}, aliases at zero: {alias_counted_zero}, sizes match: {sizes_ok}"
            ),
        )
    }
}

pub fn summarize(results: &[CheckResult]) -> (usize, usize, usize, usize) {
    let count = |s: CheckStatus| results.iter().filter(|r| r.status == s).count();
    (
        count(CheckStatus::Pass),
        count(CheckStatus::Fail),
        count(CheckStatus::Observe),
        count(CheckStatus::Skip),
    )
}
