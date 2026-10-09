# Better Disk Manager

A Windows disk-space explorer: scan a drive or folder, see what uses the
space as a green treemap shaded by age, compare scans over time, and move
reviewed personal files to the Recycle Bin, never deleting them
permanently.

The product and technical specifications are in [`docs/spec/`](docs/spec/).
This repository is at **Stage 0, the Windows feasibility prototype**: it
measures the risky parts (metadata reads, hard-link accounting, cloud
placeholders, WebView2 startup, accessible list behaviour, recycle-only
Shell failures) before any real UI is built. There is no cleanup feature
yet, and nothing here deletes files outside the disposable sandbox the
fixture tool creates.

## Layout

```
crates/
  bd-core/          Platform-neutral core: scan coordinator, pre-order index,
                    hard-link accounting, filtered aggregation, date buckets,
                    byte formatting/parsing, file-type mapping. Tested anywhere.
  bd-win/           Windows adapter: native directory reader, volume queries,
                    per-file verification, recycle-only Shell adapter,
                    process measurements. Empty on other platforms.
tools/
  bd-fixtures/      Builds marker-guarded fixture trees and the recycle
                    sandbox; checks scans against the fixture manifest.
spikes/
  bd-spike-scan/    Enumeration spike (read-only).
  bd-spike-query/   Query and memory benchmark on a synthetic 1M-file index.
  bd-spike-recycle/ Recycle-only spike (sandbox only).
app/                Tauri 2 + React + TypeScript shell spike: canvas treemap,
                    accessible virtual list, startup and frame measurements.
  src-tauri/        The Rust side of the shell (permissioned commands).
docs/
  stage0.md         How to run Stage 0 on Windows, step by step.
  stage0-findings.md  Measurements and what they change (fill in).
  decisions.md      Design decisions and when to revisit them.
scripts/
  stage0.ps1        Runs all Stage 0 measurements and collects reports.
  measure-startup.ps1  Repeated launch-to-first-paint timing.
```

## Requirements

* Rust 1.97.0 (pinned in `rust-toolchain.toml`; rustup installs it).
* Node.js 22.12 or later, for the app.
* Windows 10 or 11 for the native reader, the recycle spike and the app.
  The app also needs the
  [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)
  (MSVC build tools, WebView2).

On Linux and macOS the core, the fixture tool, the scan spike (with the
portable `std` reader) and the query spike all build and run, which is
enough for most core work.

## Quick start

```powershell
# Core tests (any platform)
cargo test

# Windows: everything Stage 0 measures, reports in stage0-out\
powershell -ExecutionPolicy Bypass -File scripts\stage0.ps1

# The shell spike
cd app
npm ci
npm run tauri dev          # development
npm run tauri build -- --no-bundle   # release exe for startup timing
```

Individual tools:

```powershell
cargo run --release -p bd-fixtures -- create D:\bd-fixture --bulk-files 100000 --bulk-bytes 2GB
cargo run --release -p bd-spike-scan -- D:\bd-fixture --verify all
cargo run --release -p bd-spike-scan -- C:\ --workers 1,2,4 --verify 5000 --json c-drive.report.json
cargo run --release -p bd-spike-query -- --files 1000000
cargo run --release -p bd-fixtures -- recycle-sandbox D:\bd-sandbox
cargo run --release -p bd-spike-recycle -- D:\bd-sandbox
cargo run --release -p bd-fixtures -- clean D:\bd-fixture
```

See [`docs/stage0.md`](docs/stage0.md) for the full runbook, including the
manual Recycle Bin, exFAT and Narrator checks.

## Safety rules in this repository

* Scans only read metadata. Links are recorded and never followed, files
  are never opened during a scan, and cloud placeholders are never
  hydrated on purpose.
* The recycle spike refuses to run on anything but a sandbox created by
  `bd-fixtures recycle-sandbox`, and every target must resolve inside that
  sandbox. `bd-fixtures clean` refuses to delete a folder without its
  marker file.
* Reports redact paths unless `--show-paths` is given; paths are personal
  data.

## Status

Stage 0 code is compile-checked for Windows and unit-tested on Linux; the
Windows-only parts (native reader, Shell recycling, WebView2) are exercised
by the Windows CI job and by the runbook. Record what they measure in
`docs/stage0-findings.md` before starting Stage 1.
