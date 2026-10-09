//! Stage 0 query spike.
//!
//! Generates a deterministic synthetic file system (no disk I/O), scans it
//! through the real coordinator, builds the index, and measures how long
//! filtered aggregation takes at several levels (decision D1: pre-order
//! index, one linear pass per level). Use it to check the specification's
//! latency budget at a million files on the reference machine; numbers from
//! other machines are not baselines.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;

use bd_core::age::{AgeBucket, DateMetric};
use bd_core::aggregate::{BucketSet, Filter, ViewOptions, summarize_children};
use bd_core::attrs::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_REPARSE_POINT,
    IO_REPARSE_TAG_CLOUD,
};
use bd_core::bytes::format_decimal;
use bd_core::category::FileType;
use bd_core::raw::{DirReader, RawEntry, ReadError};
use bd_core::scan::{CancelToken, NodeIndex, ScanConfig, scan};
use bd_core::time::{TICKS_PER_DAY, Ticks};
use bd_core::tree::ScanTree;
use bd_fixtures::rng::SplitMix64;

#[derive(Parser)]
#[command(version, about = "Stage 0 query spike (synthetic index, no disk I/O)")]
struct Cli {
    /// Approximate number of files.
    #[arg(long, default_value_t = 1_000_000)]
    files: u64,
    /// Average files per folder.
    #[arg(long, default_value_t = 40)]
    files_per_dir: u64,
    /// Subfolders per folder.
    #[arg(long, default_value_t = 8)]
    fanout: u64,
    #[arg(long, default_value_t = 2)]
    workers: usize,
    /// Timed repetitions per query (the specification measures 30).
    #[arg(long, default_value_t = 30)]
    repeat: usize,
    /// p95 latency budget per query, in milliseconds. The specification's
    /// target is 250 ms for a warm filter or sort on 100,000 entries.
    #[arg(long, default_value_t = 250.0)]
    budget_ms: f64,
    /// Memory target for the index, in bytes. The specification's target
    /// is 500 MB private working set for the whole app at 1,000,000 entries.
    #[arg(long, default_value = "500MB", value_parser = parse_size)]
    memory_target: u64,
    #[arg(long, default_value_t = 0xBD)]
    seed: u64,
    #[arg(long)]
    json: Option<PathBuf>,
}

fn parse_size(s: &str) -> Result<u64, String> {
    bd_core::bytes::parse_size(s).map_err(|e| e.to_string())
}

const WORDS: [&str; 16] = [
    "report", "invoice", "holiday", "backup", "photo", "project", "notes", "draft", "final",
    "budget", "video", "track", "scan", "export", "archive", "data",
];
const EXTS: [&str; 12] = [
    "pdf", "docx", "txt", "mp4", "mkv", "zip", "7z", "mp3", "jpg", "png", "dll", "bin",
];

/// A deterministic file system computed from folder IDs. Folder `i` has
/// subfolders `fanout*i + 1 ..= fanout*i + fanout` (below `dirs`).
struct Synthetic {
    dirs: u64,
    fanout: u64,
    files_per_dir: u64,
    seed: u64,
    now: Ticks,
}

impl Synthetic {
    fn dir_id(path: &Path) -> u64 {
        path.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix('d'))
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    }
}

impl DirReader for Synthetic {
    fn name(&self) -> &'static str {
        "synthetic"
    }

    fn read_dir(&self, dir: &Path) -> Result<Vec<RawEntry>, ReadError> {
        let id = Synthetic::dir_id(dir);
        let mut rng = SplitMix64::new(self.seed ^ id.wrapping_mul(0x9E37_79B9));
        let mut out = Vec::new();
        for c in 1..=self.fanout {
            let child = self.fanout * id + c;
            if child < self.dirs {
                out.push(RawEntry {
                    name: OsString::from(format!("d{child}")),
                    attributes: FILE_ATTRIBUTE_DIRECTORY,
                    reparse_tag: 0,
                    logical: 0,
                    allocated: None,
                    file_id: None,
                    created: None,
                    modified: None,
                    accessed: None,
                });
            }
        }
        let count = rng.range(self.files_per_dir / 2, self.files_per_dir * 3 / 2 + 1);
        let mut prev: Option<RawEntry> = None;
        for j in 0..count {
            let roll = rng.range(0, 1000);
            // 0.5% extra names for the previous file (hard links).
            if roll < 5
                && let Some(p) = &prev
            {
                let mut alias = p.clone();
                alias.name = OsString::from(format!("link-{j}-{}", p.name.to_string_lossy()));
                out.push(alias);
                continue;
            }
            let logical = match rng.range(0, 100) {
                0..60 => rng.range(0, 4096),
                60..90 => rng.range(4096, 256 << 10),
                90..99 => rng.range(256 << 10, 4 << 20),
                _ => rng.range(4 << 20, 512 << 20),
            };
            let word = WORDS[rng.range(0, WORDS.len() as u64) as usize];
            let ext = EXTS[rng.range(0, EXTS.len() as u64) as usize];
            let age = rng.range(0, 2000) as i64 * TICKS_PER_DAY;
            let modified = self.now.checked_add_ticks(-age);
            let (mut attributes, mut reparse_tag) = (0, 0);
            if (5..10).contains(&roll) {
                // 0.5% dehydrated cloud placeholders.
                attributes = FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS;
                reparse_tag = IO_REPARSE_TAG_CLOUD;
            }
            let e = RawEntry {
                name: OsString::from(format!("{word}-{j:04}.{ext}")),
                attributes,
                reparse_tag,
                logical,
                // 0.2% with unknown allocation.
                allocated: (!(10..12).contains(&roll)).then(|| logical.div_ceil(4096) * 4096),
                file_id: Some((u128::from(id) << 32) | u128::from(j + 1)),
                created: modified,
                modified,
                accessed: modified,
            };
            prev = Some(e.clone());
            out.push(e);
        }
        Ok(out)
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Timing {
    level: &'static str,
    filter: &'static str,
    children: usize,
    matching_files: u64,
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
    over_budget: bool,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    platform: &'static str,
    files: u64,
    dirs: u64,
    nodes: usize,
    scan_ms: f64,
    build_ms: f64,
    memory_before: Option<u64>,
    memory_after_build: Option<u64>,
    peak_memory: Option<u64>,
    budget_ms: f64,
    timings: Vec<Timing>,
}

/// (current, peak) resident memory in bytes, where available.
fn memory() -> (Option<u64>, Option<u64>) {
    #[cfg(windows)]
    {
        bd_win::process::memory_usage().map_or((None, None), |m| {
            (Some(m.working_set), Some(m.peak_working_set))
        })
    }
    #[cfg(target_os = "linux")]
    {
        let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
            return (None, None);
        };
        let field = |name: &str| -> Option<u64> {
            let line = status.lines().find(|l| l.starts_with(name))?;
            Some(line.split_whitespace().nth(1)?.parse::<u64>().ok()? * 1024)
        };
        (field("VmRSS:"), field("VmHWM:"))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        (None, None)
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn largest_child(tree: &ScanTree, parent: NodeIndex) -> Option<NodeIndex> {
    summarize_children(tree, parent, &Filter::default(), &ViewOptions::default())
        .children
        .iter()
        .find(|c| tree.node(c.node).is_dir())
        .map(|c| c.node)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let dirs = (cli.files / cli.files_per_dir.max(1)).max(1);
    let fs = Synthetic {
        dirs,
        fanout: cli.fanout.max(1),
        files_per_dir: cli.files_per_dir.max(2),
        seed: cli.seed,
        now: Ticks::now(),
    };
    let (memory_before, _) = memory();

    eprintln!(
        "generating and scanning ~{} files in {dirs} folders...",
        cli.files
    );
    let t = Instant::now();
    let out = scan(
        Path::new("synthetic"),
        &fs,
        &ScanConfig {
            workers: cli.workers,
            ..ScanConfig::default()
        },
        &CancelToken::new(),
        |_| {},
    );
    let scan_ms = t.elapsed().as_secs_f64() * 1000.0;
    let files = out.final_progress.files;
    let t = Instant::now();
    let tree = ScanTree::build(out);
    let build_ms = t.elapsed().as_secs_f64() * 1000.0;
    let (memory_after_build, peak_memory) = memory();

    // Levels: root, two levels down, and as deep as the largest path goes.
    let mut levels: Vec<(&'static str, NodeIndex)> = vec![("root", 0)];
    let mut cur = 0;
    let mut depth = 0;
    while let Some(next) = largest_child(&tree, cur) {
        cur = next;
        depth += 1;
        if depth == 2 {
            levels.push(("depth 2", cur));
        }
    }
    if depth > 2 {
        levels.push(("deepest", cur));
    }

    let filters: Vec<(&'static str, Filter, ViewOptions)> = vec![
        ("none", Filter::default(), ViewOptions::default()),
        (
            "type: videos",
            Filter {
                file_type: Some(FileType::Videos),
                ..Filter::default()
            },
            ViewOptions::default(),
        ),
        (
            "search: common word",
            Filter {
                search: Some("report".into()),
                ..Filter::default()
            },
            ViewOptions::default(),
        ),
        (
            "search: no match",
            Filter {
                search: Some("zzzzqqq".into()),
                ..Filter::default()
            },
            ViewOptions::default(),
        ),
        (
            "min size 1 MB",
            Filter {
                min_size: Some(1_000_000),
                ..Filter::default()
            },
            ViewOptions::default(),
        ),
        (
            "modified > 90 days",
            Filter {
                date: Some((
                    DateMetric::Modified,
                    BucketSet::none()
                        .with(AgeBucket::Days91To365)
                        .with(AgeBucket::OverOneYear),
                )),
                ..Filter::default()
            },
            ViewOptions::default(),
        ),
        (
            "combined",
            Filter {
                file_type: Some(FileType::Documents),
                search: Some("final".into()),
                min_size: Some(10_000),
                date: Some((
                    DateMetric::Modified,
                    BucketSet::none().with(AgeBucket::OverOneYear),
                )),
                ..Filter::default()
            },
            ViewOptions::default(),
        ),
    ];

    let mut timings = Vec::new();
    for &(level, node) in &levels {
        for (name, filter, view) in &filters {
            let mut samples = Vec::with_capacity(cli.repeat);
            let mut last = None;
            for _ in 0..cli.repeat.max(1) {
                let t = Instant::now();
                let s = summarize_children(&tree, node, filter, view);
                samples.push(t.elapsed().as_secs_f64() * 1000.0);
                last = Some(s);
            }
            samples.sort_by(f64::total_cmp);
            let s = last.expect("at least one run");
            let max = *samples.last().unwrap();
            timings.push(Timing {
                level,
                filter: name,
                children: s.children.len(),
                matching_files: s.matching_files,
                p50_ms: percentile(&samples, 0.5),
                p95_ms: percentile(&samples, 0.95),
                max_ms: max,
                over_budget: percentile(&samples, 0.95) > cli.budget_ms,
            });
        }
    }

    let report = Report {
        platform: std::env::consts::OS,
        files,
        dirs,
        nodes: tree.len(),
        scan_ms,
        build_ms,
        memory_before,
        memory_after_build,
        peak_memory,
        budget_ms: cli.budget_ms,
        timings,
    };

    let mb = |b: Option<u64>| b.map_or("n/a".into(), |b| format_decimal(u128::from(b)));
    println!(
        "{} files, {} folders, {} nodes ({})",
        report.files, report.dirs, report.nodes, report.platform
    );
    println!(
        "scan (no I/O) {:.0} ms, tree build {:.0} ms",
        report.scan_ms, report.build_ms
    );
    println!(
        "memory: before {}, after build {}, peak {}",
        mb(report.memory_before),
        mb(report.memory_after_build),
        mb(report.peak_memory)
    );
    println!();
    println!(
        "{:<9} {:<20} {:>9} {:>10} {:>9} {:>9} {:>9}",
        "level", "filter", "children", "matches", "p50 ms", "p95 ms", "max ms"
    );
    for t in &report.timings {
        println!(
            "{:<9} {:<20} {:>9} {:>10} {:>9.2} {:>9.2} {:>9.2}{}",
            t.level,
            t.filter,
            t.children,
            t.matching_files,
            t.p50_ms,
            t.p95_ms,
            t.max_ms,
            if t.over_budget { "  OVER BUDGET" } else { "" }
        );
    }
    let over = report.timings.iter().filter(|t| t.over_budget).count();
    if let Some(peak) = report.peak_memory {
        println!(
            "peak memory {} vs target {} (index only; the app adds UI and storage): {}",
            format_decimal(u128::from(peak)),
            format_decimal(u128::from(cli.memory_target)),
            if peak <= cli.memory_target {
                "within"
            } else {
                "OVER"
            }
        );
    }
    println!();
    println!(
        "{} of {} queries over the {:.0} ms p95 budget",
        over,
        report.timings.len(),
        report.budget_ms
    );

    if let Some(path) = &cli.json {
        match serde_json::to_string_pretty(&report) {
            Ok(text) => {
                if let Err(e) = std::fs::write(path, text) {
                    eprintln!("error writing report: {e}");
                    return ExitCode::from(2);
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        }
    }
    if over > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
