//! Test helpers: an in-memory filesystem implementing [`DirReader`].

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use crate::attrs::FILE_ATTRIBUTE_DIRECTORY;
use crate::raw::{DirReader, FileId, RawEntry, ReadError, ReadErrorKind};
use crate::time::Ticks;

pub fn file(name: &str, logical: u64) -> RawEntry {
    RawEntry {
        name: OsString::from(name),
        attributes: 0,
        reparse_tag: 0,
        logical,
        allocated: Some(logical.div_ceil(4096) * 4096),
        file_id: None,
        created: None,
        modified: None,
        accessed: None,
    }
}

pub fn file_with(
    name: &str,
    logical: u64,
    allocated: Option<u64>,
    id: Option<FileId>,
    modified: Option<Ticks>,
) -> RawEntry {
    RawEntry {
        allocated,
        file_id: id,
        modified,
        ..file(name, logical)
    }
}

pub fn dir(name: &str) -> RawEntry {
    RawEntry {
        attributes: FILE_ATTRIBUTE_DIRECTORY,
        allocated: None,
        ..file(name, 0)
    }
}

fn key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[derive(Default)]
pub struct FakeFs {
    dirs: HashMap<String, Vec<RawEntry>>,
    denied: HashSet<String>,
    delay: Duration,
    reads: Mutex<Vec<String>>,
}

impl FakeFs {
    pub fn new() -> FakeFs {
        FakeFs::default()
    }

    pub fn with(mut self, path: &str, entries: Vec<RawEntry>) -> FakeFs {
        self.dirs.insert(path.to_string(), entries);
        self
    }

    pub fn deny(mut self, path: &str) -> FakeFs {
        self.denied.insert(path.to_string());
        self
    }

    pub fn with_delay(mut self, delay: Duration) -> FakeFs {
        self.delay = delay;
        self
    }

    pub fn was_read(&self, path: &str) -> bool {
        self.reads.lock().unwrap().iter().any(|p| p == path)
    }

    pub fn read_count(&self) -> usize {
        self.reads.lock().unwrap().len()
    }
}

impl DirReader for FakeFs {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn read_dir(&self, dir: &Path) -> Result<Vec<RawEntry>, ReadError> {
        let k = key(dir);
        self.reads.lock().unwrap().push(k.clone());
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        if self.denied.contains(&k) {
            return Err(ReadError {
                kind: ReadErrorKind::AccessDenied,
                os_code: Some(5),
                message: "Access is denied.".into(),
            });
        }
        Ok(self.dirs.get(&k).cloned().unwrap_or_default())
    }
}

/// A uniquely named directory under the system temp dir, removed on drop.
#[cfg(unix)]
pub struct TempDir(std::path::PathBuf);

#[cfg(unix)]
impl TempDir {
    pub fn new(label: &str) -> TempDir {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "bd-core-{label}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

#[cfg(unix)]
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
