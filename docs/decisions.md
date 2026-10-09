# Decisions

Decisions taken at bootstrap, before Stage 0 measurements. Each one names
what would make us revisit it. Stage 0 results go in
[`stage0-findings.md`](stage0-findings.md); if a finding contradicts a
decision here, update the decision and say why.

## D1. The active scan lives in memory as a pre-order array

**Decision.** After enumeration, the scan becomes a `ScanTree`
(`crates/bd-core/src/tree.rs`): one flat array of nodes in depth-first
pre-order, children sorted by ordinal name, and each node storing `end`,
the index one past its last descendant. Every folder's descendants are the
contiguous slice `[i + 1, end)`.

**Why.** Filtered folder totals (FR04, FR05) must be recomputed whenever a
filter changes. With the pre-order layout, one level of the map is a single
linear pass over a contiguous slice (`aggregate::summarize_children`), with
no recursion, no per-folder caches to invalidate and no SQL. SQLite remains
the persistence layer (snapshots, history, comparisons), as the
specification says; it is not on the interactive query path.

**Evidence so far.** `bd-spike-query` on a 1,000,000-file synthetic index
(Linux container, not the baseline machine): root-level queries p95 under
50 ms for every filter, deeper levels well under 1 ms, against a 250 ms
target measured at 100,000 entries. Index memory about 330 MB resident after
build.

**Revisit if** baseline numbers miss the 250 ms p95 query target or the
500 MB memory target, or Stage 2 needs incremental updates the array cannot
absorb cheaply.

## D2. Names are stored natively and losslessly; paths are rebuilt

**Decision.** Nodes store their own name and their parent index, never a
full path. Names are kept as `OsString` in memory (lossless UTF-16 on
Windows, including unpaired surrogates) and will be stored as raw UTF-16
BLOBs in SQLite. Display strings are derived (lossy) and never used as
action targets. Ordering uses UTF-16 code units (`name::ordinal_cmp`), the
same order as Windows ordinal comparison.

**Why.** FR03 requires that an unusual name (unpaired surrogate, trailing
dot or space, reserved device name, case-sensitive sibling) can never
redirect an action to a different file. Storing paths as text would lose
exactly those names. Parent links also keep memory proportional to names,
not to path depth.

**Consequences.** Native paths are rebuilt from the root (`ScanTree::path`)
on a verbatim (`\\?\`) root, so the Win32 path parser never rewrites them.
The recycle spike tests whether the Shell can address such paths at all.

## D3. Frontend types are generated from Rust

**Decision.** From Stage 1, every Tauri command's argument and result types
are generated for TypeScript from the Rust structs (ts-rs or tauri-specta;
choose when Stage 1 starts). Byte counts that can exceed 2^53 (folder and
volume totals are `u128` in the core) cross the boundary as decimal
strings.

**Why.** The UI and backend must agree on units, nullability (unknown
allocation is `null`, never 0) and enum spellings. Hand-written mirrors
drift.

**Stage 0 exception.** The shell spike hand-writes its few types
(`app/src/data/model.ts`) and sends sizes as numbers, because its synthetic
sizes stay far below 2^53.

## D4. AT06 is split across Stage 1 and Stage 2

**Decision.** Stage 1's gate checks the scan half of AT06: a denied folder
or a cancellation produces a result labelled partial (`CompleteWithOmissions`
or `Cancelled`), with the unreadable or unscanned folders recorded, and the
rest of the tree intact. Stage 2's gate checks the comparison half: missing
paths in a partial scan are not reported as removed.

**Why.** AT06's pass condition depends on saved scans and comparisons
(FR08), which arrive in Stage 2, but Stage 1's gate lists AT01 through AT08.

## D5. Scan through one directory handle per folder; never open files

**Decision.** The Windows reader (`bd-win::WinDirReader`) enumerates with
`GetFileInformationByHandleEx(FileIdExtdDirectoryInfo)` on one handle per
directory, falling back to `FileFullDirectoryInfo` (no file IDs) on volumes
that reject it. It opens directories with `FILE_LIST_DIRECTORY` only, all
share modes, and `FILE_FLAG_OPEN_REPARSE_POINT`. It never opens a file.

**Why.** Opening files is slow under Defender, can hydrate cloud
placeholders (FR03, AT05), and is not needed: the batch records carry file
ID, sizes, allocation, attributes, reparse tag and timestamps. Hard links
are detected as repeated file IDs within the scan (FR02).

**Known gap.** Directory entries can hold stale sizes for hard-linked files
(NTFS updates only the name used to write). The scan flags groups whose
members disagree (`SIZE_CONFLICT`) and counts the largest allocation once.
`bd-spike-scan --verify` measures how often this happens on real volumes.

**Revisit if** verification shows enumeration disagrees with per-file
queries for ordinary (non-hard-linked) files.

## D6. Links are recorded, never followed; cloud folders are traversed

**Decision.** A reparse point is treated as a link, and not traversed, when
its tag is a name surrogate (junctions, symlinks, volume mount points) or
unknown. Non-surrogate reparse points (cloud-files placeholders, dedup) are
the folder itself and are traversed. Dehydrated placeholders count with
unknown allocation; their reported allocation is shown separately, never
added to totals.

**Why.** Skipping every reparse folder would silently drop a whole OneDrive
tree; following surrogates would double-count and can loop forever (AT04).

## D7. Recycling is per item, on a dedicated COM thread, with a veto

**Decision.** `bd-win::recycle` runs one `IFileOperation` per item on its
own single-threaded apartment, asks for recycling
(`FOFX_RECYCLEONDELETE | FOF_ALLOWUNDO`, optionally
`FOF_WANTNUKEWARNING`), and vetoes in `PreDeleteItem` any deletion the
Shell does not mark `TSF_DELETE_RECYCLE_IF_POSSIBLE`. The verdict for each
item comes from what was observed (still present, Recycle Bin item reported,
bin count change), never from what was requested.

**Status.** Hypothesis until Stage 0 proves it on the AT13 cases (file too
large for the bin, bin disabled, unsupported volume). If it cannot be
proven, cleanup ships disabled, as the specification requires.

## D8. Toolchains and dependencies are pinned

Rust is pinned in `rust-toolchain.toml`; crates in `Cargo.lock`; npm
packages to exact versions in `app/package.json` and
`app/package-lock.json`. New versions are adopted deliberately, preferring
releases at least two weeks old. The Tauri crates are pinned to one
consistent 2.11 line (`tauri` 2.11.6, `tauri-build` 2.6.3, `@tauri-apps/api`
2.11.1, `@tauri-apps/cli` 2.11.5).
