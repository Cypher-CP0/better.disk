//! Stage 0 enumeration spike.
//!
//! Scans a folder read-only and reports what the specification needs
//! measured before Stage 1: speed per worker count, time to first progress,
//! cancel latency, memory, tree-build and query times, totals and their
//! uncertainty (unknown allocation, hard links, placeholders), and — with
//! `--verify` — how often per-file queries disagree with what directory
//! enumeration reported. Against a `bd-fixtures` tree it also checks every
//! manifest expectation and exits non-zero on a failure.
//!
//! Paths are personal data (FR11): reports redact them unless
//! `--show-paths` is given.

mod platform;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use clap::{Parser, ValueEnum};

use bd_core::aggregate::{Filter, ViewOptions, summarize_children};
use bd_core::bytes::{format_decimal, format_exact};
use bd_core::category::FileType;
use bd_core::raw::DirReader;
use bd_core::scan::{CancelToken, NO_PARENT, NodeIndex, ScanConfig, ScanOutput, ScanState, scan};
use bd_core::tree::{HardLinkSummary, NodeKind, ScanTree, TreeTotals, flags};
use bd_fixtures::manifest::{CheckResult, CheckStatus, Manifest, check, summarize};
use bd_fixtures::{MANIFEST_FILE, display, prepare_root};

use platform::Memory;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ReaderArg {
    /// Native reader on Windows (IdExtd with Full fallback), std elsewhere.
    Auto,
    /// Windows: FileIdExtdDirectoryInfo only.
    Idextd,
    /// Windows: FileFullDirectoryInfo only (no file IDs).
    Full,
    /// Portable std::fs reader (baseline).
    Std,
}

#[derive(Clone, Copy, Debug)]
enum VerifyArg {
    Count(usize),
    All,
}

fn parse_verify(s: &str) -> Result<VerifyArg, String> {
    if s.eq_ignore_ascii_case("all") {
        Ok(VerifyArg::All)
    } else {
        s.parse()
            .map(VerifyArg::Count)
            .map_err(|_| "expected a number or `all`".to_string())
    }
}

#[derive(Parser)]
#[command(version, about = "Stage 0 enumeration spike (read-only)")]
struct Cli {
    /// Folder or drive root to scan, e.g. `C:\` or a bd-fixtures tree.
    root: PathBuf,
    #[arg(long, value_enum, default_value_t = ReaderArg::Auto)]
    reader: ReaderArg,
    /// Windows: open folders without FILE_FLAG_OPEN_REPARSE_POINT (to
    /// compare listings of cloud-files folders).
    #[arg(long)]
    follow_dir_reparse: bool,
    /// Worker counts to try, e.g. `2` or `1,2,4,8`.
    #[arg(long, value_delimiter = ',', default_value = "2")]
    workers: Vec<usize>,
    /// Runs per worker count (the first is usually the cold-cache run).
    #[arg(long, default_value_t = 1)]
    repeat: usize,
    /// Cancel each run after this many milliseconds, to measure cancel
    /// latency. Ctrl+C also cancels.
    #[arg(long)]
    cancel_after_ms: Option<u64>,
    /// Re-query this many files one by one (or `all`) and compare with
    /// enumeration. Placeholders are never opened.
    #[arg(long, default_value = "0", value_parser = parse_verify)]
    verify: VerifyArg,
    /// Fixture manifest to check. Defaults to the one at the root, if any.
    #[arg(long)]
    manifest: Option<PathBuf>,
    /// Do not check a manifest even if the root has one.
    #[arg(long)]
    no_manifest: bool,
    /// Write the full report as JSON.
    #[arg(long)]
    json: Option<PathBuf>,
    /// Include paths and names in output (they are personal data).
    #[arg(long)]
    show_paths: bool,
    /// Largest items to list at the root level.
    #[arg(long, default_value_t = 10)]
    top: usize,
    /// No live progress line.
    #[arg(long)]
    quiet: bool,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RunReport {
    workers: usize,
    elapsed_ms: f64,
    first_progress_ms: Option<f64>,
    cancel_latency_ms: Option<f64>,
    state: ScanState,
    entries: u64,
    entries_per_second: f64,
    memory_after_scan: Option<Memory>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FailedDir {
    path: String,
    kind: bd_core::raw::ReadErrorKind,
    os_code: Option<i32>,
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct QueryTiming {
    root_level_ms: f64,
    search_ms: f64,
    type_filter_ms: f64,
    children_at_root: usize,
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifyReport {
    eligible_files: u64,
    sampled: u64,
    skipped_placeholders: u64,
    query_failed: u64,
    logical_mismatches: u64,
    logical_mismatches_in_hard_link_groups: u64,
    allocated_mismatches: u64,
    allocation_unknown_in_enumeration: u64,
    id_mismatches: u64,
    link_count_above_one: u64,
    /// Files whose link count exceeds the names this scan found: other
    /// names live outside the scanned folder.
    links_outside_scan: u64,
    compressed_size_equals_logical: u64,
    compressed_size_equals_allocated: u64,
    compressed_size_other: u64,
    compressed_size_unavailable: u64,
    elapsed_ms: f64,
    examples: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TopItem {
    name: String,
    kind: NodeKind,
    bytes: u128,
    files: u64,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    tool_version: &'static str,
    platform: &'static str,
    root: String,
    reader: &'static str,
    volume: Option<serde_json::Value>,
    runs: Vec<RunReport>,
    tree_build_ms: f64,
    memory_before: Option<Memory>,
    memory_after_build: Option<Memory>,
    totals: TreeTotals,
    hard_links: HardLinkSummary,
    failed_dirs: Vec<FailedDir>,
    volume_used_bytes: Option<u64>,
    /// Volume used bytes minus counted bytes. Only meaningful when the
    /// root is the volume root: system metadata, shadow copies, other
    /// users' unreadable folders.
    unaccounted_bytes: Option<i128>,
    query: QueryTiming,
    top: Vec<TopItem>,
    verify: Option<VerifyReport>,
    checks: Option<Vec<CheckResult>>,
    targets: Vec<TargetCheck>,
}

/// A specification target compared with this run. Only meaningful on the
/// documented baseline machine with a release build.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TargetCheck {
    metric: &'static str,
    target: &'static str,
    measured: String,
    met: Option<bool>,
}

fn target_checks(runs: &[RunReport], memory: Option<Memory>, entries: u64) -> Vec<TargetCheck> {
    let mut t = Vec::new();
    let first = runs
        .iter()
        .filter_map(|r| r.first_progress_ms)
        .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))));
    t.push(TargetCheck {
        metric: "first progress update",
        target: "<= 1000 ms after the scan starts",
        measured: first.map_or("not measured".into(), |v| format!("{v:.1} ms (worst run)")),
        met: first.map(|v| v <= 1000.0),
    });
    let cancel = runs
        .iter()
        .filter_map(|r| r.cancel_latency_ms)
        .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))));
    t.push(TargetCheck {
        metric: "cancellation stops scheduling",
        target: "<= 2000 ms",
        measured: cancel.map_or("not measured (use --cancel-after-ms)".into(), |v| {
            format!("{v:.1} ms (worst run)")
        }),
        met: cancel.map(|v| v <= 2000.0),
    });
    let rate = runs
        .iter()
        .filter(|r| r.state != ScanState::Cancelled)
        .map(|r| r.entries_per_second)
        .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.min(v))));
    t.push(TargetCheck {
        metric: "steady scan rate",
        target: ">= 5,000 entries/s on the 100,000-file SSD fixture",
        measured: rate.map_or("no complete run".into(), |v| {
            format!("{v:.0} entries/s (slowest run)")
        }),
        met: rate.map(|v| v >= 5000.0),
    });
    let private = memory.and_then(|m| m.private.or(Some(m.peak)));
    t.push(TargetCheck {
        metric: "memory",
        target: "<= 500 MB private working set for the whole app at 1,000,000 entries",
        measured: private.map_or("not available".into(), |b| {
            format!(
                "{} with {entries} entries (spike process, index only)",
                format_decimal(u128::from(b))
            )
        }),
        met: None,
    });
    t
}

static CURRENT_CANCEL: Mutex<Option<CancelToken>> = Mutex::new(None);

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn redact(path: &Path, show: bool) -> String {
    if show {
        display(path)
    } else {
        format!("<{} components hidden>", path.components().count())
    }
}

fn flat_rel_path(out: &ScanOutput, mut idx: NodeIndex) -> PathBuf {
    let mut chain = Vec::new();
    while idx != 0 && idx != NO_PARENT {
        let n = &out.nodes[idx as usize];
        chain.push(n.entry.name.clone());
        idx = n.parent;
    }
    chain.iter().rev().collect()
}

fn run_once(
    root: &Path,
    reader: &dyn DirReader,
    workers: usize,
    cli: &Cli,
) -> (ScanOutput, RunReport) {
    let token = CancelToken::new();
    *CURRENT_CANCEL.lock().unwrap() = Some(token.clone());
    if let Some(after) = cli.cancel_after_ms {
        let t = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(after));
            t.cancel();
        });
    }
    let config = ScanConfig {
        workers,
        ..ScanConfig::default()
    };
    let quiet = cli.quiet;
    let out = scan(root, reader, &config, &token, |p| {
        if !quiet {
            eprint!(
                "\r  {} files, {} folders, {} logical, {} errors, {:.1} s   ",
                p.files,
                p.dirs,
                format_decimal(p.logical_bytes),
                p.errors,
                p.elapsed_ms as f64 / 1000.0
            );
        }
    });
    if !quiet {
        eprintln!();
    }
    *CURRENT_CANCEL.lock().unwrap() = None;
    let p = &out.final_progress;
    let entries = p.files + p.dirs + p.links;
    let report = RunReport {
        workers,
        elapsed_ms: ms(out.elapsed),
        first_progress_ms: out.first_progress_after.map(ms),
        cancel_latency_ms: out.cancel_latency.map(ms),
        state: out.state,
        entries,
        entries_per_second: entries as f64 / out.elapsed.as_secs_f64().max(1e-9),
        memory_after_scan: platform::memory(),
    };
    (out, report)
}

fn median_ms(mut f: impl FnMut()) -> f64 {
    let mut v: Vec<f64> = (0..5)
        .map(|_| {
            let t = Instant::now();
            f();
            ms(t.elapsed())
        })
        .collect();
    v.sort_by(f64::total_cmp);
    v[2]
}

fn time_queries(tree: &ScanTree) -> QueryTiming {
    let view = ViewOptions::default();
    let mut children = 0;
    let root_level_ms = median_ms(|| {
        children = summarize_children(tree, 0, &Filter::default(), &view)
            .children
            .len();
    });
    let search = Filter {
        search: Some("report".into()),
        ..Filter::default()
    };
    let search_ms = median_ms(|| {
        let _ = summarize_children(tree, 0, &search, &view);
    });
    let videos = Filter {
        file_type: Some(FileType::Videos),
        ..Filter::default()
    };
    let type_filter_ms = median_ms(|| {
        let _ = summarize_children(tree, 0, &videos, &view);
    });
    QueryTiming {
        root_level_ms,
        search_ms,
        type_filter_ms,
        children_at_root: children,
    }
}

fn verify(tree: &ScanTree, arg: VerifyArg, show_paths: bool) -> Option<VerifyReport> {
    let wanted = match arg {
        VerifyArg::Count(0) => return None,
        VerifyArg::Count(n) => n,
        VerifyArg::All => usize::MAX,
    };
    let started = Instant::now();
    let mut r = VerifyReport::default();
    let mut names_per_id: HashMap<u128, u32> = HashMap::new();
    let mut eligible = Vec::new();
    for i in 0..tree.len() as NodeIndex {
        let n = tree.node(i);
        if !n.is_file() {
            continue;
        }
        if let Some(id) = n.file_id() {
            *names_per_id.entry(id).or_default() += 1;
        }
        if n.has_flag(flags::PLACEHOLDER) {
            r.skipped_placeholders += 1;
        } else {
            eligible.push(i);
        }
    }
    r.eligible_files = eligible.len() as u64;
    let take = wanted.min(eligible.len());
    let example = |r: &mut VerifyReport, s: String| {
        if r.examples.len() < 20 {
            r.examples.push(s);
        }
    };
    for k in 0..take {
        let i = eligible[k * eligible.len() / take];
        let n = tree.node(i);
        let path = tree.path(i);
        let label = redact(&tree.rel_path(i), show_paths);
        r.sampled += 1;
        let f = match platform::facts(&path) {
            Ok(f) => f,
            Err(e) => {
                r.query_failed += 1;
                example(&mut r, format!("{label}: query failed: {e}"));
                continue;
            }
        };
        let in_group = n.has_flag(flags::ALIAS) || n.has_flag(flags::HARDLINK_PRIMARY);
        if f.logical != n.logical() {
            r.logical_mismatches += 1;
            if in_group {
                r.logical_mismatches_in_hard_link_groups += 1;
            }
            example(
                &mut r,
                format!(
                    "{label}: enumeration logical {} vs handle {}",
                    n.logical(),
                    f.logical
                ),
            );
        }
        match (n.allocated(), f.allocated) {
            (None, _) => r.allocation_unknown_in_enumeration += 1,
            (Some(a), Some(b)) if a != b => {
                r.allocated_mismatches += 1;
                example(
                    &mut r,
                    format!("{label}: enumeration allocated {a} vs handle {b}"),
                );
            }
            _ => {}
        }
        if let (Some(a), Some(b)) = (n.file_id(), f.file_id)
            && a != b
        {
            r.id_mismatches += 1;
            example(&mut r, format!("{label}: file id differs"));
        }
        if f.link_count > 1 {
            r.link_count_above_one += 1;
            let seen = n
                .file_id()
                .and_then(|id| names_per_id.get(&id))
                .copied()
                .unwrap_or(1);
            if f.link_count > seen {
                r.links_outside_scan += 1;
            }
        }
        match f.compressed_size {
            None => r.compressed_size_unavailable += 1,
            Some(c) if c == f.logical => r.compressed_size_equals_logical += 1,
            Some(c) if Some(c) == f.allocated => r.compressed_size_equals_allocated += 1,
            Some(_) => r.compressed_size_other += 1,
        }
    }
    r.elapsed_ms = ms(started.elapsed());
    Some(r)
}

fn load_manifest(cli: &Cli, root: &Path) -> Result<Option<(Manifest, PathBuf)>, String> {
    if cli.no_manifest {
        return Ok(None);
    }
    let (path, explicit) = match &cli.manifest {
        Some(p) => (prepare_root(p).map_err(|e| e.to_string())?, true),
        None => (root.join(MANIFEST_FILE), false),
    };
    if !explicit && !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("reading manifest: {e}"))?;
    let manifest: Manifest =
        serde_json::from_str(&text).map_err(|e| format!("parsing manifest: {e}"))?;
    let fixture_root = path.parent().unwrap_or(root);
    let base = fixture_root
        .strip_prefix(root)
        .map_err(|_| "the manifest's fixture is not inside the scanned folder".to_string())?
        .to_path_buf();
    Ok(Some((manifest, base)))
}

fn print_report(r: &Report, cli: &Cli) {
    println!("Better Disk Manager - Stage 0 enumeration spike");
    println!("root:      {}", r.root);
    println!("reader:    {}", r.reader);
    println!();
    println!("run  workers  elapsed      first progress  cancel latency  entries/s     state");
    for (k, run) in r.runs.iter().enumerate() {
        println!(
            "{:<4} {:<8} {:<12} {:<15} {:<15} {:<13} {:?}",
            k + 1,
            run.workers,
            format!("{:.0} ms", run.elapsed_ms),
            run.first_progress_ms
                .map_or("-".into(), |m| format!("{m:.1} ms")),
            run.cancel_latency_ms
                .map_or("-".into(), |m| format!("{m:.1} ms")),
            format!("{:.0}", run.entries_per_second),
            run.state
        );
    }
    let t = &r.totals;
    println!();
    println!(
        "totals:    {} files ({} zero-byte), {} folders, {} links",
        t.files, t.zero_byte_files, t.dirs, t.links
    );
    println!(
        "  logical            {:>12}  ({})",
        format_decimal(t.logical_bytes),
        format_exact(t.logical_bytes)
    );
    println!(
        "  allocated (naive)  {:>12}  every hard-link name counted",
        format_decimal(t.allocated_bytes_naive)
    );
    println!(
        "  counted            {:>12}  each file identity once",
        format_decimal(t.counted_bytes)
    );
    println!("  unknown allocation: {} files", t.unknown_allocation_files);
    let h = &r.hard_links;
    println!(
        "  hard links: {} groups, {} alias names, {} groups with size conflicts, {} files without identity",
        h.groups, h.alias_names, h.groups_with_size_conflict, h.unverified_files
    );
    println!(
        "  placeholders: {} hydrated, {} dehydrated ({} reported, excluded)",
        t.hydrated_placeholders,
        t.dehydrated_placeholders,
        format_decimal(t.dehydrated_reported_bytes)
    );
    println!(
        "  unreadable folders: {}, unscanned: {}",
        t.failed_dirs, t.unscanned_dirs
    );
    for f in r.failed_dirs.iter().take(10) {
        println!("    {:?} (os {:?}): {}", f.kind, f.os_code, f.path);
    }
    if r.failed_dirs.len() > 10 {
        println!("    ... {} more", r.failed_dirs.len() - 10);
    }
    if let (Some(used), Some(un)) = (r.volume_used_bytes, r.unaccounted_bytes) {
        println!(
            "volume used {} vs counted {}: unaccounted {}{} (meaningful only for a volume root)",
            format_decimal(u128::from(used)),
            format_decimal(t.counted_bytes),
            if un < 0 { "-" } else { "" },
            format_decimal(un.unsigned_abs())
        );
    }
    println!();
    println!("tree build: {:.1} ms", r.tree_build_ms);
    let mem = |m: &Option<Memory>| {
        m.map_or("n/a".to_string(), |m| {
            format!(
                "current {}, peak {}{}",
                format_decimal(u128::from(m.current)),
                format_decimal(u128::from(m.peak)),
                m.private.map_or(String::new(), |p| format!(
                    ", private {}",
                    format_decimal(u128::from(p))
                ))
            )
        })
    };
    println!("memory before scan: {}", mem(&r.memory_before));
    println!("memory after build: {}", mem(&r.memory_after_build));
    println!(
        "root-level query (median of 5): all {:.2} ms, search {:.2} ms, type filter {:.2} ms; {} children",
        r.query.root_level_ms, r.query.search_ms, r.query.type_filter_ms, r.query.children_at_root
    );
    if !r.top.is_empty() {
        println!();
        println!("largest at root (counted allocation):");
        for item in &r.top {
            println!(
                "  {:>12}  {:>9} files  {:?}  {}",
                format_decimal(item.bytes),
                item.files,
                item.kind,
                item.name
            );
        }
    }
    if let Some(v) = &r.verify {
        println!();
        println!(
            "verify: {} of {} eligible files re-queried in {:.0} ms ({} placeholders skipped, {} failed)",
            v.sampled, v.eligible_files, v.elapsed_ms, v.skipped_placeholders, v.query_failed
        );
        println!(
            "  logical mismatches {} ({} in hard-link groups), allocated mismatches {}, unknown in enumeration {}, id mismatches {}",
            v.logical_mismatches,
            v.logical_mismatches_in_hard_link_groups,
            v.allocated_mismatches,
            v.allocation_unknown_in_enumeration,
            v.id_mismatches
        );
        println!(
            "  link count > 1: {}, with names outside the scan: {}",
            v.link_count_above_one, v.links_outside_scan
        );
        println!(
            "  compressed size == logical {}, == allocated {}, other {}, unavailable {}",
            v.compressed_size_equals_logical,
            v.compressed_size_equals_allocated,
            v.compressed_size_other,
            v.compressed_size_unavailable
        );
        for e in &v.examples {
            println!("    {e}");
        }
    }
    println!();
    println!("specification targets (baseline machine, release build):");
    for t in &r.targets {
        let verdict = match t.met {
            Some(true) => "met",
            Some(false) => "NOT MET",
            None => "record",
        };
        println!(
            "  {:<30} {:<8} {}  [target {}]",
            t.metric, verdict, t.measured, t.target
        );
    }
    if let Some(checks) = &r.checks {
        let (pass, fail, observe, skip) = summarize(checks);
        println!();
        println!("manifest checks: {pass} pass, {fail} fail, {observe} observe, {skip} skip");
        for c in checks {
            if c.status != CheckStatus::Pass || cli.show_paths {
                println!("  [{:?}] {}: {}", c.status, c.check, c.detail);
            }
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: &Cli) -> Result<ExitCode, String> {
    let root = prepare_root(&cli.root).map_err(|e| format!("bad root: {e}"))?;
    let md = std::fs::symlink_metadata(&root).map_err(|e| format!("cannot open root: {e}"))?;
    if md.file_type().is_symlink() {
        return Err(
            "the root is a link (symlink or junction); scans never follow links, so pass the target folder instead"
                .into(),
        );
    }
    if !md.is_dir() {
        return Err("the root is not a folder".into());
    }
    let reader = platform::reader(cli.reader, cli.follow_dir_reparse)?;
    let manifest = load_manifest(cli, &root)?;
    let _ = ctrlc::set_handler(|| {
        if let Some(t) = CURRENT_CANCEL.lock().unwrap().as_ref() {
            t.cancel();
        }
    });

    let volume = platform::volume(&root);
    if let Some(v) = &volume {
        for l in &v.lines {
            println!("{l}");
        }
    }

    let memory_before = platform::memory();
    let mut runs = Vec::new();
    let mut last = None;
    'outer: for &w in &cli.workers {
        for _ in 0..cli.repeat.max(1) {
            let (out, report) = run_once(&root, reader.as_ref(), w.max(1), cli);
            let cancelled = out.state == ScanState::Cancelled && cli.cancel_after_ms.is_none();
            runs.push(report);
            last = Some(out);
            if cancelled {
                break 'outer;
            }
        }
    }
    let out = last.ok_or("no runs")?;

    let failed_dirs = out
        .errors
        .iter()
        .map(|e| FailedDir {
            path: redact(&flat_rel_path(&out, e.node), cli.show_paths),
            kind: e.error.kind,
            os_code: e.error.os_code,
        })
        .collect();

    let started = Instant::now();
    let tree = ScanTree::build(out);
    let tree_build_ms = ms(started.elapsed());
    let memory_after_build = platform::memory();

    let query = time_queries(&tree);
    let level = summarize_children(&tree, 0, &Filter::default(), &ViewOptions::default());
    let top = level
        .children
        .iter()
        .take(cli.top)
        .map(|c| TopItem {
            name: if cli.show_paths {
                tree.node(c.node).display_name().into_owned()
            } else {
                format!("<item {}>", c.node)
            },
            kind: c.kind,
            bytes: c.bytes,
            files: c.matching_files,
        })
        .collect();

    let verify = verify(&tree, cli.verify, cli.show_paths);
    let checks = manifest.as_ref().map(|(m, base)| check(&tree, m, base));
    let used = volume.as_ref().and_then(|v| v.used_bytes);

    let report = Report {
        tool_version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        root: redact(&root, cli.show_paths),
        reader: reader.name(),
        volume: volume.map(|v| v.json),
        runs,
        tree_build_ms,
        memory_before,
        memory_after_build,
        totals: tree.totals().clone(),
        hard_links: tree.hard_links().clone(),
        failed_dirs,
        volume_used_bytes: used,
        unaccounted_bytes: used.map(|u| i128::from(u) - tree.totals().counted_bytes as i128),
        query,
        top,
        verify,
        checks,
        targets: Vec::new(),
    };
    let entries = report.totals.files + report.totals.dirs + report.totals.links;
    let mut report = report;
    report.targets = target_checks(&report.runs, report.memory_after_build, entries);
    print_report(&report, cli);
    if let Some(path) = &cli.json {
        let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| format!("writing report: {e}"))?;
        println!();
        println!("report written to {}", path.display());
    }
    let failed = report
        .checks
        .as_ref()
        .is_some_and(|c| c.iter().any(|r| r.status == CheckStatus::Fail));
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
