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
| OS / build | | Linux 6.18 (Firecracker VM) |
| CPU / RAM | | 2 vCPU / container |
| Storage | | overlay file system |
| Defender | | n/a |
| Build | release | release |
| Commit | | initial import |

## 1. Enumeration (AT01–AT05, AT08, AT17)

| Measure | Target | Windows baseline | Dev container (not baseline) |
|---|---|---|---|
| Manifest checks on the fixture | 0 fail | | 51 pass, 0 fail, 1 observe, 1 skip (root ignores the denial); `std` reader |
| Reader used | `win-idextd` | | `std` |
| Entries/s, 100k fixture, 2 workers | ≥ 5,000 | | ~0.9 M/s on a warm 20k fixture (page cache) |
| Entries/s, worker sweep 1/2/4/8 | tune | | 1: 0.41 M/s, 2: 0.92 M/s, 4: 1.17 M/s |
| First progress | ≤ 1 s | | < 1 ms |
| Cancellation latency | ≤ 2 s | | (not run) |
| Tree build, 100k entries | | | ~50 ms (synthetic) |
| Memory after build | ≤ 500 MB at 1M (whole app) | | 16 MB RSS at 20k |
| `FileFullDirectoryInfo` fallback works (exFAT) | | | n/a |
| System drive: entries/s, unreadable folders, unaccounted bytes | | | n/a |
| OneDrive: placeholders hydrated by the scan | none | | n/a |

### Hypotheses

| ID | Hypothesis | Held? | Evidence |
|---|---|---|---|
| H1 | Enumeration size/allocation equal per-file values outside hard-link groups | | |
| H2 | Hard-link names can carry stale sizes | | `edge/hardlinks-stale` observation |
| H3 | `GetCompressedFileSizeW` equals logical size for ordinary files | | |
| H4 | `AllocationSize` is cluster-rounded size on disk | | |
| H5 | Native reader faster than `std::fs` | | |
| H6 | First progress ≤ 1 s and cancellation ≤ 2 s | | |

## 2. Query and memory (D1)

| Measure | Target | Windows baseline | Dev container (not baseline) |
|---|---|---|---|
| Root-level query p95, 100k entries (worst filter) | ≤ 250 ms | | 6.3 ms (search, no match) |
| Root-level query p95, 1M entries (worst filter) | | | 64 ms (search, common word) |
| Deeper levels p95, 1M entries | | | < 1 ms |
| Tree build, 1M entries | | | ~0.6 s |
| Index memory, 1M entries (peak / after build) | ≤ 500 MB whole app | | 368 MB / ~305 MB RSS |

Tree build was 1.5 s at 1M entries before two changes made during Stage 0:
an in-place permutation instead of a second copy of every node, and an
ASCII fast path for search folding. Peak memory fell from 579 MB to 368 MB.

## 3. Recycle-only (AT13, AT14, D7)

Record one row per case from `stage0.md` §4.

| Case | Verdicts | Shell said "will recycle" | Veto fired | Bin item reported | Bin count Δ | Pass? |
|---|---|---|---|---|---|---|
| A. Ordinary | | | | | | |
| B. In use | | | | | | |
| C. Too large for the bin | | | | | | |
| D. Bin disabled | | | | | | |
| E. FAT32/exFAT volume | | | | | | |
| F. Control (`--no-veto`) | | | | | | |
| G. No Shell warning | | | | | | |
| H. Confirm profile, declined | | | | | | |
| I. Trailing dot / long path | | | | | | |

**Conclusion:** recycle-only is proven / not proven. If not proven,
cleanup ships disabled (FR10).

## 4. Shell (WebView2, canvas, accessible list)

| Measure | Target | Windows baseline | Dev container (not baseline) |
|---|---|---|---|
| Launch to first paint, cold | ≤ 3 s | | n/a (no process start time on Linux) |
| Launch to first paint, warm median / p95 | ≤ 3 s | | Rust `main` to first paint 0.87–1.24 s (debug build, WebKitGTK, Xvfb) |
| Frame rate, 1,500 items (even sizes), 100 % | 60 fps | | 60 fps, p95 frame 16.8 ms, draw p95 2.5 ms, 1,462 blocks (headless Chromium) |
| Frame rate, 200 % scaling | 60 fps | | |
| Layout time, 1,500 items | | | ~3 ms |
| `level_items` IPC, 1,500 items | | | 27–37 ms, 209 KB |
| List page fetch (200 rows) | | | 32 ms (native, first page) |
| Security probe (core command, missing plugin, fetch, new window) | all refused | | all refused (WebKitGTK) |
| Narrator: roving tabindex | usable | | n/a |
| Narrator: active descendant | usable | | n/a |
| High contrast | usable | | n/a |
| Keyboard-only through 100,000 rows | usable | | arrows/Page/Home/End verified in headless Chromium |

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
| | | |

## 6. Open questions for Stage 1

*
