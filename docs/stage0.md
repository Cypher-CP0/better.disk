# Stage 0 runbook

Stage 0 answers one question per spike with a measurement. Run it on the
specification's baseline machine where possible: Windows 11 x64, four CPU
cores, 16 GB RAM, an SSD, Microsoft Defender on, release builds. Record
everything in [`stage0-findings.md`](stage0-findings.md).

Never point the recycle spike at real files. It refuses to run outside a
sandbox created by `bd-fixtures recycle-sandbox`, and that is deliberate.

## 0. Prepare

```powershell
git clone <repo> better.disk; cd better.disk
cargo test                       # core tests
cd app; npm ci; npm test; cd ..  # UI tests
```

Optional, for the link cases: turn on Developer Mode (Settings → System →
For developers) or run from an elevated terminal, so the fixture tool can
create symbolic links. Junctions never need it.

## 1. Run the automated measurements

```powershell
powershell -ExecutionPolicy Bypass -File scripts\stage0.ps1 -ScanSystemDrive
```

This builds the spikes, creates a 100,000-file fixture under
`%TEMP%\bd-stage0`, and writes JSON reports to `stage0-out\`. Copy the
printed tables into the findings. The sections below explain what each
part checks and what to do by hand.

## 2. Enumeration (AT01–AT05, AT08, AT17)

### 2.1 The fixture and its manifest

`bd-fixtures create` builds a bulk tree plus one folder per edge case and
writes `bd-fixtures.manifest.json` describing what a correct scan reports:

| Folder | Checks |
|---|---|
| `bulk/` | exact file, folder and byte totals (AT01) |
| `edge/hardlinks/` | three names, one identity, one primary, aliases counted at zero (AT02) |
| `edge/hardlinks-stale/` | a link written through one name after linking; whether the other name's size is stale (observed) |
| `edge/sparse/`, `edge/compressed/` | allocation below logical size (AT03) |
| `edge/ads/` | alternate data stream bytes are not counted (documented gap) |
| `edge/links/` | junction, junction cycle, symlinks, symlink cycle: recorded, never traversed (AT04) |
| `edge/dates/` | one file per date bucket plus a future date (AT08) |
| `edge/types/` | file-type classification |
| `edge/names/` | accented, CJK, emoji, fullwidth, trailing dot, trailing space, `CON`, and a name that is not valid Unicode (AT17) |
| `edge/deep/` | a path longer than 260 characters (AT17) |
| `edge/case/` | case-sensitive siblings, when the folder can be made case-sensitive (AT17) |
| `edge/denied/` | a folder the current user cannot list (AT06, scan half) |

`bd-spike-scan <fixture>` checks every expectation and exits non-zero on a
failure. `Observe` lines are measurements to record; `Skip` lines say why a
case could not be checked (for example, symlinks without Developer Mode).

### 2.2 Hypotheses to confirm

`--verify all` re-queries files one at a time (metadata access only, links
and placeholders never opened) and compares with what enumeration said.

| ID | Hypothesis | Where to look |
|---|---|---|
| H1 | For files that are not hard-linked, enumeration's size and allocation equal the per-file values. | `verify`: logical and allocated mismatches outside hard-link groups should be 0. |
| H2 | Hard-link names can carry stale sizes in their directory entries. | `stale hard-link sizes` observation; `logical mismatches in hard-link groups`. |
| H3 | `GetCompressedFileSizeW` equals the logical size for ordinary files and differs only for compressed or sparse files. | `compressed size == logical / == allocated / other`. |
| H4 | `AllocationSize` is cluster-rounded and is the right "size on disk" for ordinary files. | compare `allocated` to `bytes_per_cluster` in the report. |
| H5 | The native reader is faster than `std::fs` and meets ≥ 5,000 entries/s. | `scan-fixture-native` vs `scan-fixture-std`; worker sweep. |
| H6 | First progress ≤ 1 s, cancellation ≤ 2 s. | the targets table in every scan report. |

### 2.3 Real data

* **System drive** (`-ScanSystemDrive`, read-only, paths redacted): record
  entries/s, unreadable folders, unaccounted bytes (volume used minus
  counted), memory, and the verify sample.
* **OneDrive (AT05).** Pick a OneDrive folder with online-only files. Note
  each file's status icon (cloud = online-only) before and after:
  `cargo run --release -p bd-spike-scan -- "$env:OneDrive" --json onedrive.report.json`.
  Pass: no file changes from online-only to available; the report shows
  dehydrated placeholders with their allocation excluded. Run it again
  with `--follow-dir-reparse` and compare file and folder counts: if they
  differ, the directory open mode affects how OneDrive populates listings
  (decision D5). Do not use
  `--verify` here; it skips placeholders anyway, but keep the run pure.
* **exFAT / FAT32 (enumeration fallback).** Scan a USB stick, or create a
  test VHDX: `bd-fixtures create <dir> --vhd-scripts`, then as
  administrator `diskpart /s <dir>\vhd\create-exfat.diskpart`. The report's
  reader should say `win-full` and hard links should be "unverified".
  Detach with `diskpart /s <dir>\vhd\detach.diskpart`.
* **Mounted volume (AT04).** If you have a volume mounted in a folder,
  scan its parent: the mount point must be recorded as a link and not
  entered.

## 3. Query and memory (D1)

`bd-spike-query --files 100000` and `--files 1000000` time filtered
aggregation at the root, two levels down and at the deepest level, 30
repetitions each. Targets: p95 ≤ 250 ms at 100,000 entries; ≤ 500 MB
private working set for the whole app at 1,000,000 entries (this measures
the index alone).

## 4. Recycle-only (AT13, AT14, D7)

Each run needs a fresh sandbox (items that were recycled are gone):

```powershell
$sb = "D:\bd-sandbox"           # a folder on the drive under test
target\release\bd-fixtures.exe clean $sb   # if it exists
target\release\bd-fixtures.exe recycle-sandbox $sb [--large-bytes 200MB]
target\release\bd-spike-recycle.exe $sb [options] --json recycle-<case>.report.json
```

For every item the spike prints the verdict, whether the Shell said it
would recycle (`TSF_DELETE_RECYCLE_IF_POSSIBLE` in `PreDeleteItem`),
whether the veto fired, whether a Recycle Bin item was reported, and the
change in the bin's item count. Afterwards, check the Recycle Bin by hand
and restore one item to prove it is recoverable.

| Case | Setup | Pass |
|---|---|---|
| A. Ordinary | default sandbox on an NTFS drive | every item `Recycled`; hard link's other name and the junction target survive; items restorable from the bin |
| B. In use | `--hold-open` | `in-use.txt` is `Failed` or `Cancelled` and still present |
| C. Too large for the bin | Recycle Bin properties for that drive → Custom size → smaller than `--large-bytes`; sandbox with `--large-bytes` | `large.bin` is `Blocked` and still present |
| D. Bin disabled | Recycle Bin properties → "Don't move files to the Recycle Bin" for that drive | every item `Blocked`, nothing deleted |
| E. Unsupported volume | sandbox on a FAT32/exFAT USB stick (or the test VHDX) | every item `Blocked` |
| F. Control | repeat C with `--no-veto` | `large.bin` is `PermanentlyDeleted` (proves the veto is what prevented it) |
| G. No Shell warning | repeat C with `--no-nuke-warning` | still `Blocked`, without any Shell dialog |
| H. Confirm profile | `--profile confirm`, click No in the Shell's dialog | items `Cancelled` and present |
| I. Long and unusual paths | default sandbox | `trailing.` and the long path are `Recycled`, or `Failed` with a clear reason (then Stage 1 must refuse such items up front) |

Restore the Recycle Bin settings after C and D. If any case ends with an
item gone and no Recycle Bin item, cleanup ships disabled until that is
understood (specification FR10).

## 5. Shell (WebView2 startup, canvas, accessible list)

```powershell
cd app; npm ci; npm run build:exe; cd ..
powershell -ExecutionPolicy Bypass -File scripts\measure-startup.ps1 -Runs 10
```

Reboot first for a cold-start number (the first run). Target: launch to
usable UI ≤ 3 s.

Then run `target\release\bd-app.exe` and check:

1. **Frame rate.** Items 1,500, tick *Even sizes (most blocks)*, press
   *Run 5-second frame test*. Record fps, p95 frame time and draw time at
   100 % and 200 % display scaling. Target 60 fps.
2. **Keyboard only.** Unplug or ignore the mouse. Tab reaches every
   control and the list; arrows, Page Up/Down, Home and End move through
   100,000 items; Enter activates; the details panel follows selection.
3. **Narrator** (Ctrl + Win + Enter). In each *List focus* mode, Tab into
   the list and move around. Record what Narrator says for a row (name,
   size, date, type, position "row N of M"), whether selection is
   announced, and any lag. Choose the mode for Stage 1.
4. **High contrast** (Settings → Accessibility → Contrast themes). The map
   switches to system colors with bucket names written in the blocks; the
   list, focus and selection stay visible.
5. **Selection sync.** Clicking a block selects and reveals its row;
   selecting a row outlines its block; *Smaller items* reveals the first
   aggregated item.
6. **Locked shell.** The results panel has a `security` section filled in
   automatically after startup: the page tries an ungranted core command
   (`plugin:app|version`), a command of a plugin that is not installed
   (`plugin:fs|read_dir`), a network fetch and `window.open`. All four must
   read `refused` or `blocked`. To keep the results as a file, start the
   app with `$env:BD_RESULTS_FILE = "stage0-out\app-results.json"`.

## 6. Write up

Fill in [`stage0-findings.md`](stage0-findings.md): the numbers, which
hypotheses held, which decisions in [`decisions.md`](decisions.md) change,
and any specification targets that need revising (with the measurement
that justifies it).
