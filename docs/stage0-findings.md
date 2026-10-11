# Stage 0 findings

Fill in the **Windows baseline** columns from `scripts\stage0.ps1`,
`scripts\measure-startup.ps1` and the manual checks in
[`stage0.md`](stage0.md). The **dev container** columns were measured while
building Stage 0 in a 2-core Linux container: they show the code works and
roughly how it scales, but they are **not baseline numbers** and must not be
quoted as product performance.

## Environment

| | Windows baseline | Dev container (not baseline) |
|---|---|---|
| OS / build | Windows 11 [confirm build number] | Linux 6.18 (Firecracker VM) |
| CPU / RAM | [confirm; spec baseline is 4 cores, 16 GB] | 2 vCPU / container |
| Storage | NTFS system drive, 487 GB, cluster 4096; [confirm SSD] | overlay file system |
| Defender | [confirm on/off] | n/a |
| Build | release | release |
| Commit | 26092b1 plus a local `stage0.ps1` path fix (since committed) | initial import |

## 1. Enumeration (AT01–AT05, AT08, AT17)

| Measure | Target | Windows baseline | Dev container (not baseline) |
|---|---|---|---|
| Manifest checks on the fixture | 0 fail | 56 pass, 0 fail, 1 observe, 0 skip (native reader, after Developer Mode) | 51 pass, 0 fail, 1 observe, 1 skip (root ignores the denial); `std` reader |
| Reader used | `win-idextd` | `win-idextd` (native); `win-full` for the comparison run | `std` |
| Entries/s, 100k fixture, 2 workers | ≥ 5,000 | 1,148,416 (second run). First run was 219,275, which is unexplained; rerun to check variance | ~0.9 M/s on a warm 20k fixture (page cache) |
| Entries/s, worker sweep 1/2/4/8 | tune | ~0.80 M / ~1.5 M / ~2.2 M / ~2.9 M | 1: 0.41 M/s, 2: 0.92 M/s, 4: 1.17 M/s |
| First progress | ≤ 1 s | 0.4 ms | < 1 ms |
| Cancellation latency | ≤ 2 s | 2.3 ms, measured on the system drive (cancel at 2 s, mid-scan; 107,968 entries indexed, 28,285 folders left unscanned). The scripted fixture run never cancelled: it fired at 300 ms and the scan ends in ~70–90 ms (script now cancels at 20 ms) | (not run) |
| Tree build, 100k entries | | 29.6 ms | ~50 ms (synthetic) |
| Memory after build | ≤ 500 MB at 1M (whole app) | 305 MB peak at 1M entries (index only; see §2) | 16 MB RSS at 20k |
| `FileFullDirectoryInfo` fallback works (exFAT) | | Deferred (no working USB stick). On NTFS the `win-full` reader works: 55 pass, 1 expected skip | n/a |
| System drive: entries/s, unreadable folders, unaccounted bytes | | 16,578 entries/s; 620 unreadable folders; 41 GB unaccounted (2.68 M files, 3.0 M entries) | n/a |
| OneDrive: placeholders hydrated by the scan | none | None. 14 files, 11 dehydrated placeholders (allocation excluded), 0 hydrated after four scans. Default and `--follow-dir-reparse` give identical counts. Small folder; repeat on a larger one if available | n/a |

### Hypotheses

| ID | Hypothesis | Held? | Evidence |
|---|---|---|---|
| H1 | Enumeration size/allocation equal per-file values outside hard-link groups | Mostly | 1 logical and 1 allocated mismatch, both inside hard-link groups (100,036 files verified) |
| H2 | Hard-link names can carry stale sizes | Yes | `edge/hardlinks-stale`: `b.bin` reports 10,000 bytes against a true size of 60,000 |
| H3 | `GetCompressedFileSizeW` equals logical size for ordinary files | Yes | Equal to logical size for 100,034 of 100,036 files |
| H4 | `AllocationSize` is cluster-rounded size on disk | Not verified | Allocated values not yet checked against the 4,096-byte cluster |
| H5 | Native reader faster than `std::fs` | **No** | Second run: native 1.15 M/s vs `std` 1.20 M/s (`win-full` was fastest at 1.33 M/s). Retest before deciding |
| H6 | First progress ≤ 1 s and cancellation ≤ 2 s | Yes | First progress 0.4 ms. Cancellation 2.3 ms on the system drive |

## 2. Query and memory (D1)

| Measure | Target | Windows baseline | Dev container (not baseline) |
|---|---|---|---|
| Root-level query p95, 100k entries (worst filter) | ≤ 250 ms | 4.03 ms (search, no match) | 6.3 ms (search, no match) |
| Root-level query p95, 1M entries (worst filter) | | 39.56 ms (search, common word) | 64 ms (search, common word) |
| Deeper levels p95, 1M entries | | < 0.2 ms | < 1 ms |
| Tree build, 1M entries | | 385 ms | ~0.6 s |
| Index memory, 1M entries (peak / after build) | ≤ 500 MB whole app | 305 MB peak / 236 MB after build | 368 MB / ~305 MB RSS |

Tree build was 1.5 s at 1M entries before two changes made during Stage 0:
an in-place permutation instead of a second copy of every node, and an
ASCII fast path for search folding. Peak memory fell from 579 MB to 368 MB.

## 3. Recycle-only (AT13, AT14, D7)

Record one row per case from `stage0.md` §4.

| Case | Verdicts | Shell said "will recycle" | Veto fired | Bin item reported | Bin count Δ | Pass? |
|---|---|---|---|---|---|---|
| A. Ordinary | Recycled ×7 (file, folder, unicode, hard link, junction, read-only, in-use) | yes | no | yes | +1 each | Yes |
| B. In use (`--hold-open`) | `in-use` Cancelled, left in place (the Shell showed a "File In Use" dialog); other 8 items recycled | yes | no | no for `in-use`; yes for the rest | +0 for `in-use` | Yes |
| C. Too large for the bin | PermanentlyDeleted (2 of 2 runs, bin limit 100 MB) | yes | **no** | no | +0 | **No** |
| D. Bin disabled | Blocked ×8 (file, folder, unicode, hard link, junction, read-only, in-use, too-large); nothing deleted, confirmed by listing all 10 items afterwards | no | yes | no | +0 | Yes |
| E. FAT32/exFAT volume | Deferred (no working USB stick) | | | | | |
| F. Control (`--no-veto`) | PermanentlyDeleted (as expected) | yes | disabled | no | +0 | Yes |
| G. No Shell warning | not run | | | | | |
| H. Confirm profile, declined | 8 items Cancelled, left in place (Shell prompt declined on each); trailing dot and long path Failed as in I | - | no | no | +0 | Yes |
| I. Trailing dot / long path | Failed ×2 ("The parameter is incorrect.") | - | no | no | +0 | Partly (see note) |

Note on I: the spec allows `Failed` with a clear reason, but Stage 1 must
refuse these names before deleting.

Restore check: `small.txt` restored from the Recycle Bin matches a freshly
generated copy byte for byte (SHA256 `E0D036B1…233B6`). The fixture generator
is deterministic.

**Conclusion:** recycle-only is **not proven**. Case C failed in two runs with
the bin limited to 100 MB. The Shell reported it would recycle, the veto did
not fire, and the 200 MB file was permanently deleted. Case D shows the flag is reliable
when the bin is disabled (every item blocked), so the failure is specific to
the size limit. Cleanup ships disabled
(FR10) until a size check before deletion is implemented and tested.

## 4. Shell (WebView2, canvas, accessible list)

| Measure | Target | Windows baseline | Dev container (not baseline) |
|---|---|---|---|
| Launch to first paint, cold | ≤ 3 s | 1,337 ms on the first run (main 33 ms, setup 43 ms). [Confirm whether this was after a reboot; if not, rerun after one for the true cold number] | n/a (no process start time on Linux) |
| Launch to first paint, warm median / p95 | ≤ 3 s | Runs 2–10: median 518 ms, max 647 ms (main ~10 ms, setup ~16 ms) | Rust `main` to first paint 0.87–1.24 s (debug build, WebKitGTK, Xvfb) |
| Frame rate, 1,500 items (even sizes), 100 % | 60 fps | 100.2 fps (100 Hz display), p95 frame 10.2 ms, draw p95 1.20 ms, 1,462 blocks. Default sizes for comparison: 99.8 fps, draw p95 0.50 ms, 228 blocks | 60 fps, p95 frame 16.8 ms, draw p95 2.5 ms, 1,462 blocks (headless Chromium) |
| Frame rate, 200 % scaling | 60 fps | 99.7 fps, p95 frame 10.1 ms, draw p95 0.60 ms, 220 blocks (default sizes; the even-sizes case was not run at 200 %) | |
| Layout time, 1,500 items | | | ~3 ms |
| `level_items` IPC, 1,500 items | | | 27–37 ms, 209 KB |
| List page fetch (200 rows) | | | 32 ms (native, first page) |
| Security probe (core command, missing plugin, fetch, new window) | all refused | All refused: both commands "not allowed by ACL", fetch blocked, new window blocked (WebView2) | all refused (WebKitGTK) |
| Narrator: roving tabindex | usable | Usable | n/a |
| Narrator: active descendant | usable | Usable. **Chosen for Stage 1** | n/a |
| High contrast | usable | Usable | n/a |
| Keyboard-only through 100,000 rows | usable | Usable | arrows/Page/Home/End verified in headless Chromium |
| Selection sync (block ↔ row) | works | Works | |

### Observations so far

* A level of 100,000 items of similar size draws as a single *Smaller
  items* block: every item's share of the map is below the selectable
  minimum. That follows the specification's rules; the list is the route
  in. Worth checking with users.
* Sending every item of a level to the page (`level_items`) does not
  scale: at 100,000 items the payload is 14.7 MB and layout takes ~120 ms
  in the browser. Stage 1 should compute the capped block list (top 1,499
  plus the *Smaller items* aggregate) in Rust and send at most 1,500
  blocks, with the viewport size as an argument.
* Generated data matches bit for bit between Rust and TypeScript (same
  `mulberry32` reference values in both test suites), so the browser
  preview is a faithful stand-in for UI work.

## 5. Decisions and specification changes

| Decision / target | Change | Justifying measurement |
|---|---|---|
| Recycle safety (FR10, D7) | Stage 1 must check each item's size against the Recycle Bin maximum before the Shell call and refuse anything that won't fit. The veto alone is not enough. Refuse trailing-dot and long-path names up front. | `recycle-C.report.json`, `recycle-C2.report.json` (case C, two runs) |
| Directory open mode for OneDrive (D5) | No change needed: opening directories as reparse points or following them gives the same listing | `onedrive.report.json`, `onedrive-follow.report.json` (14 files, 4 folders, 11 dehydrated in both) |
| List focus mode | Stage 1 uses active descendant | Narrator usable in both modes on Windows; active descendant chosen |

## 6. Open questions for Stage 1

* FAT32/exFAT (scan fallback and recycle case E) is untested. A test VHDX
  needs no USB stick: `bd-fixtures create <dir> --vhd-scripts`, then as
  administrator `diskpart /s <dir>\vhd\create-exfat.diskpart`.
* Does `PreDeleteItem` ever report the size limit correctly? Case D (bin
  disabled) should show whether the flag is reliable anywhere.
* Why is the native reader not faster than `std` (H5)? Retest before
  choosing a reader for Stage 1.
* The system-drive scan found one file that enumeration reported as 0 bytes
  but the handle query measured at 87,289 bytes. Was it changing during the
  scan?
* 620 unreadable folders and 41 GB unaccounted on the system drive. Confirm
  these are protected system areas.
* Junction (resolved): in the recycled run, the junction target's `keep.txt`
  stayed in place, and only the link went to the Recycle Bin. The bin shows the
  link as a 3.12 MB item, though its target holds 10 bytes. The cause of that
  size is unexplained and should be checked in Stage 1 (or accepted as bin
  metadata).
