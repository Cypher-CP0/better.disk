//! Platform-specific parts of the enumeration spike.

use bd_core::raw::DirReader;

use crate::ReaderArg;

/// Process memory in bytes.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub current: u64,
    pub peak: u64,
    /// Private commit, where the platform reports it.
    pub private: Option<u64>,
}

/// What a per-file query reports, for comparison with enumeration.
#[derive(Clone, Copy, Debug)]
pub struct Facts {
    pub logical: u64,
    pub allocated: Option<u64>,
    pub link_count: u32,
    pub file_id: Option<u128>,
    pub compressed_size: Option<u64>,
}

pub struct VolumeSummary {
    pub json: serde_json::Value,
    pub used_bytes: Option<u64>,
    pub lines: Vec<String>,
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::path::Path;

    use bd_core::bytes::format_decimal;
    use bd_win::{EnumerationClass, WinDirReader};

    pub fn reader(arg: ReaderArg, follow_dir_reparse: bool) -> Result<Box<dyn DirReader>, String> {
        let native =
            |class| Box::new(WinDirReader::new(class).with_open_reparse_point(!follow_dir_reparse));
        Ok(match arg {
            ReaderArg::Auto => native(EnumerationClass::Auto),
            ReaderArg::Idextd => native(EnumerationClass::IdExtd),
            ReaderArg::Full => native(EnumerationClass::Full),
            ReaderArg::Std => Box::new(bd_core::stdfs::StdDirReader),
        })
    }

    pub fn volume(root: &Path) -> Option<VolumeSummary> {
        let policy = bd_win::volume::access_time_policy();
        match bd_win::volume::query(root) {
            Ok(v) => {
                let reliability = policy.reliability(&v);
                let lines = vec![
                    format!(
                        "volume:    {} {} {:?}, {} total, {} used, {} free, cluster {}",
                        v.mount_path,
                        v.file_system,
                        v.drive_type,
                        format_decimal(u128::from(v.total_bytes)),
                        format_decimal(u128::from(v.used_bytes())),
                        format_decimal(u128::from(v.free_bytes)),
                        v.bytes_per_cluster
                            .map_or("unknown".into(), |c| c.to_string()),
                    ),
                    format!("           flags: {}", v.fs_flag_names.join(", ")),
                    format!(
                        "access-time policy: {} (raw {:?}) -> access dates {:?}",
                        policy.note, policy.raw, reliability
                    ),
                ];
                Some(VolumeSummary {
                    used_bytes: Some(v.used_bytes()),
                    json: serde_json::json!({
                        "volume": v,
                        "accessTimePolicy": policy,
                        "accessReliability": reliability,
                    }),
                    lines,
                })
            }
            Err(e) => Some(VolumeSummary {
                json: serde_json::json!({ "error": e.message().trim_end() }),
                used_bytes: None,
                lines: vec![format!(
                    "volume:    query failed: {}",
                    e.message().trim_end()
                )],
            }),
        }
    }

    pub fn memory() -> Option<Memory> {
        bd_win::process::memory_usage().map(|m| Memory {
            current: m.working_set,
            peak: m.peak_working_set,
            private: Some(m.private_bytes),
        })
    }

    pub fn facts(path: &Path) -> Result<Facts, String> {
        let f = bd_win::verify::handle_facts(path).map_err(|e| {
            format!(
                "{} ({})",
                e.message().trim_end(),
                bd_win::error::os_code(&e)
            )
        })?;
        Ok(Facts {
            logical: f.logical,
            allocated: Some(f.allocated),
            link_count: f.link_count,
            file_id: f.file_id,
            compressed_size: f.compressed_size,
        })
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    use std::path::Path;

    pub fn reader(arg: ReaderArg, _follow_dir_reparse: bool) -> Result<Box<dyn DirReader>, String> {
        match arg {
            ReaderArg::Auto | ReaderArg::Std => Ok(Box::new(bd_core::stdfs::StdDirReader)),
            ReaderArg::Idextd | ReaderArg::Full => {
                Err("the native Windows readers are only available on Windows".into())
            }
        }
    }

    pub fn volume(_root: &Path) -> Option<VolumeSummary> {
        None
    }

    #[cfg(target_os = "linux")]
    pub fn memory() -> Option<Memory> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let field = |name: &str| -> Option<u64> {
            let line = status.lines().find(|l| l.starts_with(name))?;
            let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
            Some(kb * 1024)
        };
        Some(Memory {
            current: field("VmRSS:")?,
            peak: field("VmHWM:")?,
            private: None,
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn memory() -> Option<Memory> {
        None
    }

    #[cfg(unix)]
    pub fn facts(path: &Path) -> Result<Facts, String> {
        use std::os::unix::fs::MetadataExt;
        let md = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        Ok(Facts {
            logical: md.len(),
            allocated: Some(md.blocks() * 512),
            link_count: md.nlink() as u32,
            file_id: Some((u128::from(md.dev()) << 64) | u128::from(md.ino())),
            compressed_size: None,
        })
    }

    #[cfg(not(unix))]
    pub fn facts(_path: &Path) -> Result<Facts, String> {
        Err("not supported on this platform".into())
    }
}

pub use imp::*;
