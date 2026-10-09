//! The scan coordinator (FR01).
//!
//! A bounded pool of worker threads reads directories through a
//! [`DirReader`]; one collector (the calling thread) owns all scan state,
//! assigns node indices, schedules child directories and emits throttled
//! progress. Workers never touch shared scan state, so there is no locking
//! on the hot path beyond the job queue.
//!
//! Cancellation stops scheduling immediately: queued directories are marked
//! [`DirStatus::Unscanned`], in-flight reads are allowed to finish (a blocked
//! OS call cannot be interrupted), and the result is labelled
//! [`ScanState::Cancelled`] — never complete.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::attrs::{FILE_ATTRIBUTE_DIRECTORY, LinkKind};
use crate::raw::{DirReader, RawEntry, ReadError, ReadErrorKind};
use crate::time::Ticks;

/// A node index in a [`ScanOutput`]. Index 0 is the scan root.
pub type NodeIndex = u32;
pub const NO_PARENT: NodeIndex = NodeIndex::MAX;

#[derive(Clone, Debug)]
pub struct ScanConfig {
    /// Directory-reading threads. The spec starts at two and tunes with
    /// benchmarks.
    pub workers: usize,
    /// Minimum time between progress callbacks (200 ms is at most five per
    /// second). The first update is sent as soon as the root is read.
    pub progress_interval: Duration,
}

impl Default for ScanConfig {
    fn default() -> Self {
        ScanConfig {
            workers: 2,
            progress_interval: Duration::from_millis(200),
        }
    }
}

/// Cooperative cancellation shared between the UI and a running scan.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<CancelInner>);

#[derive(Debug, Default)]
struct CancelInner {
    flag: AtomicBool,
    requested_at: OnceLock<Instant>,
}

impl CancelToken {
    pub fn new() -> CancelToken {
        CancelToken::default()
    }

    pub fn cancel(&self) {
        let _ = self.0.requested_at.set(Instant::now());
        self.0.flag.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.flag.load(Ordering::SeqCst)
    }

    pub fn requested_at(&self) -> Option<Instant> {
        self.0.requested_at.get().copied()
    }
}

/// A snapshot of scan progress. Totals are unknown while discovering, so
/// there is deliberately no percentage (FR01).
#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub files: u64,
    pub dirs: u64,
    pub links: u64,
    pub logical_bytes: u128,
    /// Sum of reported allocation. Not hard-link deduplicated until the
    /// tree is built.
    pub allocated_bytes: u128,
    pub errors: u64,
    pub elapsed_ms: u64,
    /// Directory most recently read. Personal data: callers must redact it
    /// in logs by default (FR11).
    #[serde(skip)]
    pub current_dir: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanState {
    Complete,
    /// Finished, but at least one directory could not be read.
    CompleteWithOmissions,
    /// Stopped by the user; coverage is partial.
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DirStatus {
    /// A file, or anything that is not a directory.
    NotDir,
    /// A link (junction, symlink, mount point). Recorded, never followed.
    NotTraversed(LinkKind),
    /// Scheduled but not read yet. Never present in a finished output.
    Pending,
    Read,
    Failed(ReadErrorKind),
    /// Not read because the scan was cancelled first.
    Unscanned,
}

#[derive(Clone, Debug)]
pub struct FlatNode {
    pub entry: RawEntry,
    pub parent: NodeIndex,
    pub status: DirStatus,
}

#[derive(Clone, Debug)]
pub struct DirError {
    pub node: NodeIndex,
    pub error: ReadError,
}

/// Raw result of a scan in discovery order. Build a [`crate::tree::ScanTree`]
/// from it for queries.
#[derive(Debug)]
pub struct ScanOutput {
    pub root: PathBuf,
    pub reader: &'static str,
    pub nodes: Vec<FlatNode>,
    pub errors: Vec<DirError>,
    pub state: ScanState,
    /// Reference time for date buckets (FR06): when the scan started.
    pub started_at: Ticks,
    pub elapsed: Duration,
    pub first_progress_after: Option<Duration>,
    /// From the cancel request until every worker had stopped.
    pub cancel_latency: Option<Duration>,
    pub final_progress: Progress,
}

struct Job {
    node: NodeIndex,
    path: PathBuf,
}

enum JobResult {
    Read {
        node: NodeIndex,
        path: PathBuf,
        result: Result<Vec<RawEntry>, ReadError>,
    },
    /// Popped after cancellation; not read.
    Skipped { node: NodeIndex },
}

#[derive(Default)]
struct QueueState {
    jobs: VecDeque<Job>,
    closed: bool,
}

#[derive(Default)]
struct JobQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

impl JobQueue {
    fn push(&self, job: Job) {
        self.state.lock().unwrap().jobs.push_back(job);
        self.ready.notify_one();
    }

    /// Blocks until a job is available or the queue is closed.
    fn pop(&self) -> Option<Job> {
        let mut st = self.state.lock().unwrap();
        loop {
            if let Some(job) = st.jobs.pop_front() {
                return Some(job);
            }
            if st.closed {
                return None;
            }
            st = self.ready.wait(st).unwrap();
        }
    }

    fn drain(&self) -> Vec<Job> {
        self.state.lock().unwrap().jobs.drain(..).collect()
    }

    fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.ready.notify_all();
    }
}

fn root_entry(root: &Path) -> RawEntry {
    RawEntry {
        name: root
            .file_name()
            .map(OsString::from)
            .unwrap_or_else(|| root.as_os_str().to_owned()),
        attributes: FILE_ATTRIBUTE_DIRECTORY,
        reparse_tag: 0,
        logical: 0,
        allocated: None,
        file_id: None,
        created: None,
        modified: None,
        accessed: None,
    }
}

/// Scan `root` and return everything discovered.
///
/// `on_progress` runs on the calling thread at most once per
/// `config.progress_interval`, plus once at the end.
pub fn scan(
    root: &Path,
    reader: &dyn DirReader,
    config: &ScanConfig,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(&Progress),
) -> ScanOutput {
    let started_at = Ticks::now();
    let start = Instant::now();
    let workers = config.workers.max(1);

    let queue = JobQueue::default();
    let (tx, rx) = mpsc::channel::<JobResult>();

    let mut nodes: Vec<FlatNode> = vec![FlatNode {
        entry: root_entry(root),
        parent: NO_PARENT,
        status: DirStatus::Pending,
    }];
    let mut errors: Vec<DirError> = Vec::new();
    let mut progress = Progress {
        dirs: 1,
        ..Progress::default()
    };
    let mut outstanding: usize = 1;
    let mut cancel_seen = false;
    let mut first_progress_after = None;
    let mut last_progress: Option<Instant> = None;

    queue.push(Job {
        node: 0,
        path: root.to_path_buf(),
    });

    std::thread::scope(|s| {
        for _ in 0..workers {
            let tx = tx.clone();
            let queue = &queue;
            s.spawn(move || {
                while let Some(job) = queue.pop() {
                    let msg = if cancel.is_cancelled() {
                        JobResult::Skipped { node: job.node }
                    } else {
                        let result = reader.read_dir(&job.path);
                        JobResult::Read {
                            node: job.node,
                            path: job.path,
                            result,
                        }
                    };
                    if tx.send(msg).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);

        while outstanding > 0 {
            let received = rx.recv_timeout(config.progress_interval);
            if !cancel_seen && cancel.is_cancelled() {
                cancel_seen = true;
                for job in queue.drain() {
                    nodes[job.node as usize].status = DirStatus::Unscanned;
                    outstanding -= 1;
                }
            }
            match received {
                Ok(JobResult::Skipped { node }) => {
                    outstanding -= 1;
                    nodes[node as usize].status = DirStatus::Unscanned;
                }
                Ok(JobResult::Read { node, path, result }) => {
                    outstanding -= 1;
                    match result {
                        Ok(entries) => {
                            nodes[node as usize].status = DirStatus::Read;
                            for entry in entries {
                                let idx = NodeIndex::try_from(nodes.len())
                                    .expect("more than 4 billion entries");
                                let status = if let Some(link) = entry.link_kind() {
                                    progress.links += 1;
                                    DirStatus::NotTraversed(link)
                                } else if entry.is_dir() {
                                    progress.dirs += 1;
                                    if cancel_seen {
                                        DirStatus::Unscanned
                                    } else {
                                        queue.push(Job {
                                            node: idx,
                                            path: path.join(&entry.name),
                                        });
                                        outstanding += 1;
                                        DirStatus::Pending
                                    }
                                } else {
                                    progress.files += 1;
                                    progress.logical_bytes += u128::from(entry.logical);
                                    progress.allocated_bytes +=
                                        u128::from(entry.allocated.unwrap_or(0));
                                    DirStatus::NotDir
                                };
                                nodes.push(FlatNode {
                                    entry,
                                    parent: node,
                                    status,
                                });
                            }
                        }
                        Err(error) => {
                            progress.errors += 1;
                            nodes[node as usize].status = DirStatus::Failed(error.kind);
                            errors.push(DirError { node, error });
                        }
                    }
                    progress.current_dir = Some(path);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }

            let due = last_progress.is_none_or(|t| t.elapsed() >= config.progress_interval);
            if due && outstanding > 0 {
                progress.elapsed_ms = start.elapsed().as_millis() as u64;
                first_progress_after.get_or_insert_with(|| start.elapsed());
                last_progress = Some(Instant::now());
                on_progress(&progress);
            }
        }
        queue.close();
    });

    let cancelled = cancel_seen || cancel.is_cancelled();
    let cancel_latency = cancel
        .requested_at()
        .filter(|_| cancelled)
        .map(|t| t.elapsed());
    // A cancel that arrived after the last directory was read still counts:
    // the user asked to stop, and we cannot prove nothing was skipped.
    let state = if cancelled {
        ScanState::Cancelled
    } else if errors.is_empty() {
        ScanState::Complete
    } else {
        ScanState::CompleteWithOmissions
    };

    progress.elapsed_ms = start.elapsed().as_millis() as u64;
    on_progress(&progress);

    debug_assert!(nodes.iter().all(|n| n.status != DirStatus::Pending));
    ScanOutput {
        root: root.to_path_buf(),
        reader: reader.name(),
        nodes,
        errors,
        state,
        started_at,
        elapsed: start.elapsed(),
        first_progress_after,
        cancel_latency,
        final_progress: progress,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attrs::{FILE_ATTRIBUTE_REPARSE_POINT, IO_REPARSE_TAG_MOUNT_POINT};
    use crate::testutil::{FakeFs, dir, file};

    #[test]
    fn scans_every_directory() {
        let fs = FakeFs::new()
            .with("root", vec![dir("a"), dir("b"), file("top.txt", 10)])
            .with("root/a", vec![file("x", 1), file("y", 2), dir("deep")])
            .with("root/a/deep", vec![file("z", 3)])
            .with("root/b", vec![]);
        let out = scan(
            Path::new("root"),
            &fs,
            &ScanConfig::default(),
            &CancelToken::new(),
            |_| {},
        );
        assert_eq!(out.state, ScanState::Complete);
        assert_eq!(out.final_progress.files, 4);
        assert_eq!(out.final_progress.dirs, 4);
        assert_eq!(out.final_progress.logical_bytes, 16);
        assert!(out.errors.is_empty());
        assert!(out.nodes.iter().all(|n| n.status != DirStatus::Pending));
    }

    #[test]
    fn denied_directory_is_recorded_and_siblings_continue() {
        let fs = FakeFs::new()
            .with("root", vec![dir("locked"), dir("open")])
            .with("root/open", vec![file("ok", 5)])
            .deny("root/locked");
        let out = scan(
            Path::new("root"),
            &fs,
            &ScanConfig::default(),
            &CancelToken::new(),
            |_| {},
        );
        assert_eq!(out.state, ScanState::CompleteWithOmissions);
        assert_eq!(out.errors.len(), 1);
        assert_eq!(out.errors[0].error.kind, ReadErrorKind::AccessDenied);
        assert_eq!(out.final_progress.files, 1);
        let locked = out.nodes.iter().find(|n| n.entry.name == "locked").unwrap();
        assert_eq!(
            locked.status,
            DirStatus::Failed(ReadErrorKind::AccessDenied)
        );
    }

    #[test]
    fn links_are_recorded_but_not_followed() {
        let mut junction = dir("junction");
        junction.attributes |= FILE_ATTRIBUTE_REPARSE_POINT;
        junction.reparse_tag = IO_REPARSE_TAG_MOUNT_POINT;
        let mut cloud = dir("OneDrive");
        cloud.attributes |= FILE_ATTRIBUTE_REPARSE_POINT;
        cloud.reparse_tag = 0x9000_601A;
        let fs = FakeFs::new()
            .with("root", vec![junction, cloud])
            .with("root/OneDrive", vec![file("doc.docx", 7)])
            .with("root/junction", vec![file("must-not-be-read", 1)]);
        let out = scan(
            Path::new("root"),
            &fs,
            &ScanConfig::default(),
            &CancelToken::new(),
            |_| {},
        );
        assert_eq!(out.final_progress.links, 1);
        assert_eq!(out.final_progress.files, 1);
        assert!(!fs.was_read("root/junction"));
        assert!(fs.was_read("root/OneDrive"));
    }

    #[test]
    fn cancellation_marks_remaining_directories_unscanned() {
        let mut fs = FakeFs::new().with("root", (0..50).map(|i| dir(&format!("d{i}"))).collect());
        for i in 0..50 {
            fs = fs.with(&format!("root/d{i}"), vec![file("f", 1), dir("sub")]);
            fs = fs.with(&format!("root/d{i}/sub"), vec![file("g", 1)]);
        }
        let fs = fs.with_delay(Duration::from_millis(5));
        let cancel = CancelToken::new();
        let c2 = cancel.clone();
        let mut calls = 0;
        let out = scan(
            Path::new("root"),
            &fs,
            &ScanConfig {
                workers: 2,
                progress_interval: Duration::from_millis(1),
            },
            &cancel,
            |_| {
                calls += 1;
                if calls == 3 {
                    c2.cancel();
                }
            },
        );
        assert_eq!(out.state, ScanState::Cancelled);
        assert!(out.cancel_latency.is_some());
        let unscanned = out
            .nodes
            .iter()
            .filter(|n| n.status == DirStatus::Unscanned)
            .count();
        assert!(unscanned > 0, "some directories should be left unscanned");
        assert!(out.nodes.iter().all(|n| n.status != DirStatus::Pending));
        // Reads stopped well before the whole tree (101 directories) was read.
        assert!(fs.read_count() < 101);
    }

    #[test]
    fn first_progress_is_reported() {
        let fs = FakeFs::new()
            .with("root", vec![dir("a")])
            .with("root/a", vec![file("x", 1)]);
        let mut updates = 0;
        let out = scan(
            Path::new("root"),
            &fs,
            &ScanConfig::default(),
            &CancelToken::new(),
            |_| updates += 1,
        );
        assert!(updates >= 1);
        assert!(out.first_progress_after.is_some());
    }
}
