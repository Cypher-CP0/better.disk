//! Stage 0 recycle-only spike (AT13).
//!
//! Recycles the items of a `bd-fixtures recycle-sandbox` tree through the
//! Shell adapter and reports, per item, what the Shell said it would do,
//! whether the veto fired, what happened on disk, and how the Recycle Bin
//! changed. It refuses to run on anything that is not a sandbox, and every
//! target must resolve inside the sandbox's `items` folder.
//!
//! Never point this at real files. The sandbox exists so that it does not
//! matter if an item is lost.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueEnum};

use bd_fixtures::sandbox::{SandboxItem, SandboxManifest};
use bd_fixtures::{SANDBOX_MANIFEST_FILE, TreeKind, display, prepare_root, rel, tree_kind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ProfileArg {
    /// No Shell confirmation; stop at the first failure.
    Strict,
    /// Let the Shell show its own confirmation.
    Confirm,
}

#[derive(Parser)]
#[command(version, about = "Stage 0 recycle-only spike (sandbox only)")]
struct Cli {
    /// A folder created by `bd-fixtures recycle-sandbox`.
    sandbox: PathBuf,
    #[arg(long, value_enum, default_value_t = ProfileArg::Strict)]
    profile: ProfileArg,
    /// Only items of these kinds, e.g. `file,hard-link`.
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
    /// Do not veto deletions the Shell would not recycle. Sandbox items
    /// may then be permanently deleted; used to confirm the veto is what
    /// prevents it.
    #[arg(long)]
    no_veto: bool,
    /// Do not ask the Shell for its permanent-delete warning.
    #[arg(long)]
    no_nuke_warning: bool,
    /// Hide the Shell's progress and error UI.
    #[arg(long)]
    silent: bool,
    /// Hold `in-use` items open without delete sharing during the run.
    #[arg(long)]
    hold_open: bool,
    /// List what would be recycled and exit.
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    json: Option<PathBuf>,
}

/// Resolve and guard one target. The parent is canonicalized (resolving
/// any links on the way); the item itself is not, because recycling a link
/// must act on the link.
fn guarded_target(root: &Path, item: &SandboxItem) -> Result<PathBuf, String> {
    if item.path.split('/').any(|c| c == ".." || c == ".") {
        return Err(format!(
            "{}: relative components are not allowed",
            item.path
        ));
    }
    let items_dir = root.join("items");
    let target = root.join(rel(&item.path));
    let canon_items =
        std::fs::canonicalize(&items_dir).map_err(|e| format!("items folder: {e}"))?;
    let parent = target
        .parent()
        .ok_or_else(|| format!("{}: no parent", item.path))?;
    let canon_parent =
        std::fs::canonicalize(parent).map_err(|e| format!("{}: parent: {e}", item.path))?;
    if !canon_parent.starts_with(&canon_items) {
        return Err(format!(
            "{}: resolves outside the sandbox items folder",
            item.path
        ));
    }
    if target == items_dir {
        return Err("refusing to recycle the items folder itself".into());
    }
    Ok(target)
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
    let root = prepare_root(&cli.sandbox).map_err(|e| e.to_string())?;
    if tree_kind(&root) != Some(TreeKind::Sandbox) {
        return Err(format!(
            "{} is not a recycle sandbox (no marker). Create one with `bd-fixtures recycle-sandbox <folder>`.",
            display(&root)
        ));
    }
    let manifest: SandboxManifest = serde_json::from_str(
        &std::fs::read_to_string(root.join(SANDBOX_MANIFEST_FILE))
            .map_err(|e| format!("reading sandbox manifest: {e}"))?,
    )
    .map_err(|e| format!("parsing sandbox manifest: {e}"))?;

    let items: Vec<&SandboxItem> = manifest
        .items
        .iter()
        .filter(|i| cli.only.is_empty() || cli.only.iter().any(|k| k == &i.kind))
        .collect();
    let mut targets = Vec::new();
    for item in &items {
        targets.push(guarded_target(&root, item)?);
    }

    println!("sandbox: {}", display(&root));
    for (item, target) in items.iter().zip(&targets) {
        let present = std::fs::symlink_metadata(target).is_ok();
        println!(
            "  {:<13} {:<8} {}",
            item.kind,
            if present { "present" } else { "missing" },
            item.path
        );
    }
    if cli.dry_run {
        return Ok(ExitCode::SUCCESS);
    }
    if cli.no_veto {
        eprintln!(
            "warning: veto disabled; sandbox items the Shell cannot recycle will be deleted permanently"
        );
    }
    imp::recycle_and_report(cli, &root, &items, targets)
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn recycle_and_report(
        _cli: &Cli,
        _root: &Path,
        _items: &[&SandboxItem],
        _targets: Vec<PathBuf>,
    ) -> Result<ExitCode, String> {
        Err("the recycle spike uses the Windows Shell and only runs on Windows (use --dry-run elsewhere)".into())
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use bd_win::recycle::{ItemOutcome, Profile, RecycleOptions, Verdict, recycle};

    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Row<'a> {
        kind: &'a str,
        path: &'a str,
        expect: &'a str,
        outcome: &'a ItemOutcome,
        survivors_ok: bool,
        missing_survivors: Vec<String>,
    }

    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Report<'a> {
        options: RecycleOptions,
        flags: String,
        rows: Vec<Row<'a>>,
    }

    pub fn recycle_and_report(
        cli: &Cli,
        root: &Path,
        items: &[&SandboxItem],
        targets: Vec<PathBuf>,
    ) -> Result<ExitCode, String> {
        let opts = RecycleOptions {
            profile: match cli.profile {
                ProfileArg::Strict => Profile::Strict,
                ProfileArg::Confirm => Profile::Confirm,
            },
            veto_permanent: !cli.no_veto,
            nuke_warning: !cli.no_nuke_warning,
            silent: cli.silent,
        };

        // Hold in-use items open without FILE_SHARE_DELETE.
        let mut held = Vec::new();
        if cli.hold_open {
            use std::os::windows::fs::OpenOptionsExt;
            for (item, target) in items.iter().zip(&targets) {
                if item.kind == "in-use" {
                    let f = std::fs::OpenOptions::new()
                        .read(true)
                        .share_mode(0x1 /* FILE_SHARE_READ */)
                        .open(target)
                        .map_err(|e| format!("holding {} open: {e}", item.path))?;
                    held.push(f);
                }
            }
        }

        let outcomes = recycle(targets, opts)?;
        drop(held);

        let mut bad = false;
        let mut rows = Vec::new();
        for (item, outcome) in items.iter().zip(&outcomes) {
            let missing_survivors: Vec<String> = item
                .must_survive
                .iter()
                .filter(|s| std::fs::symlink_metadata(root.join(rel(s))).is_err())
                .cloned()
                .collect();
            let survivors_ok = missing_survivors.is_empty();
            let unacceptable = matches!(outcome.verdict, Verdict::Unclear)
                || (opts.veto_permanent && outcome.verdict == Verdict::PermanentlyDeleted)
                || !survivors_ok;
            bad |= unacceptable;
            rows.push(Row {
                kind: &item.kind,
                path: &item.path,
                expect: &item.expect,
                outcome,
                survivors_ok,
                missing_survivors,
            });
        }

        println!();
        println!(
            "profile {:?}, veto {}, nuke warning {}, flags {:#x}",
            opts.profile,
            opts.veto_permanent,
            opts.nuke_warning,
            opts.flags()
        );
        println!(
            "{:<13} {:<19} {:<10} {:<7} {:<9} {:<8} {:<9} detail",
            "kind", "verdict", "will-recy", "vetoed", "bin-item", "bin-dn", "survivors"
        );
        for r in &rows {
            let o = r.outcome;
            println!(
                "{:<13} {:<19} {:<10} {:<7} {:<9} {:<8} {:<9} {}",
                r.kind,
                format!("{:?}", o.verdict),
                o.shell_will_recycle.map_or("-".into(), |b| b.to_string()),
                o.vetoed,
                o.recycle_bin_item.is_some(),
                o.bin_items_delta().map_or("-".into(), |d| format!("{d:+}")),
                if r.survivors_ok { "ok" } else { "MISSING" },
                o.detail
            );
            println!("{:<13} expected: {}", "", r.expect);
        }

        if let Some(path) = &cli.json {
            let report = Report {
                options: opts,
                flags: format!("{:#x}", opts.flags()),
                rows,
            };
            let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
            std::fs::write(path, text).map_err(|e| format!("writing report: {e}"))?;
            println!("report written to {}", path.display());
        }
        Ok(if bad {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bd_fixtures::sandbox::create_sandbox;

    fn temp(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("bd-recycle-{label}-{}", std::process::id()));
        if p.exists() {
            let _ = bd_fixtures::clean(&p);
            let _ = std::fs::remove_dir_all(&p);
        }
        p
    }

    fn item(path: &str) -> SandboxItem {
        SandboxItem {
            path: path.into(),
            kind: "file".into(),
            expect: String::new(),
            must_survive: Vec::new(),
        }
    }

    #[test]
    fn guard_accepts_sandbox_items_and_rejects_escapes() {
        let root = temp("guard");
        let m = create_sandbox(&root, None, &mut |_| {}).unwrap();
        let root = prepare_root(&root).unwrap();
        for i in &m.items {
            assert!(guarded_target(&root, i).is_ok(), "{}", i.path);
        }
        assert!(guarded_target(&root, &item("items/../bd-sandbox.manifest.json")).is_err());
        assert!(guarded_target(&root, &item("bd-sandbox.manifest.json")).is_err());
        assert!(guarded_target(&root, &item("items")).is_err());
        bd_fixtures::clean(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn guard_rejects_links_that_lead_outside() {
        let root = temp("escape");
        let outside = temp("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("precious.txt"), b"keep").unwrap();
        create_sandbox(&root, None, &mut |_| {}).unwrap();
        let root = prepare_root(&root).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("items/escape")).unwrap();
        assert!(guarded_target(&root, &item("items/escape/precious.txt")).is_err());
        // The link itself is inside the sandbox and may be recycled.
        assert!(guarded_target(&root, &item("items/escape")).is_ok());
        bd_fixtures::clean(&root).unwrap();
        assert!(outside.join("precious.txt").exists());
        std::fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn refuses_folders_without_the_marker() {
        let root = temp("nomarker");
        std::fs::create_dir_all(root.join("items")).unwrap();
        let cli = Cli::parse_from(["bd-spike-recycle", root.to_str().unwrap(), "--dry-run"]);
        assert!(run(&cli).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
