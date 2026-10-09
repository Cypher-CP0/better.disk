//! `bd-fixtures`: create and remove Stage 0 fixture trees.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use bd_fixtures::create::{CreateOptions, create};
use bd_fixtures::sandbox::create_sandbox;
use bd_fixtures::{MANIFEST_FILE, SANDBOX_MANIFEST_FILE, clean, display, prepare_root};

#[derive(Parser)]
#[command(version, about = "Create and remove disposable Stage 0 fixture trees")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

fn parse_size(s: &str) -> Result<u64, String> {
    bd_core::bytes::parse_size(s).map_err(|e| e.to_string())
}

#[derive(Subcommand)]
enum Command {
    /// Build the scan fixture (bulk tree + edge cases) and its manifest.
    Create {
        /// New or empty folder to build in.
        root: PathBuf,
        /// Files in the bulk tree (0 to skip it).
        #[arg(long, default_value_t = 20_000)]
        bulk_files: u64,
        /// Approximate total size of the bulk tree, e.g. `1GB`, `200MiB`.
        #[arg(long, default_value = "1GB", value_parser = parse_size)]
        bulk_bytes: u64,
        #[arg(long, default_value_t = 0x00BD_5EED)]
        seed: u64,
        /// Also write diskpart scripts for an exFAT test VHDX (Windows).
        #[arg(long)]
        vhd_scripts: bool,
    },
    /// Build the recycle sandbox used by bd-spike-recycle.
    RecycleSandbox {
        /// New or empty folder to build in.
        root: PathBuf,
        /// Also write one file of this size, e.g. larger than the Recycle
        /// Bin's maximum, to test that it is blocked rather than deleted.
        #[arg(long, value_parser = parse_size)]
        large_bytes: Option<u64>,
    },
    /// Delete a fixture or sandbox (only folders carrying a marker).
    Clean { root: PathBuf },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut log = |s: &str| eprintln!("{s}");
    let result = match cli.command {
        Command::Create {
            root,
            bulk_files,
            bulk_bytes,
            seed,
            vhd_scripts,
        } => {
            let opts = CreateOptions {
                bulk_files,
                bulk_bytes,
                seed,
                vhd_scripts,
            };
            create(&root, &opts, &mut log).map(|m| {
                let root = prepare_root(&root).unwrap_or(root);
                println!(
                    "created fixture at {}: {} expectations, {} skipped cases",
                    display(&root),
                    m.expectations.len(),
                    m.skipped.len()
                );
                for s in &m.skipped {
                    println!("  skipped {}: {}", s.case, s.reason);
                }
                println!("manifest: {}", display(&root.join(MANIFEST_FILE)));
            })
        }
        Command::RecycleSandbox { root, large_bytes } => {
            create_sandbox(&root, large_bytes, &mut log).map(|m| {
                let root = prepare_root(&root).unwrap_or(root);
                println!(
                    "created recycle sandbox at {}: {} items, {} skipped",
                    display(&root),
                    m.items.len(),
                    m.skipped.len()
                );
                for s in &m.skipped {
                    println!("  skipped {}: {}", s.case, s.reason);
                }
                println!("manifest: {}", display(&root.join(SANDBOX_MANIFEST_FILE)));
            })
        }
        Command::Clean { root } => clean(&root).map(|()| println!("removed {}", root.display())),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
