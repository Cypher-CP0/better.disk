//! The scan fixture.
//!
//! ```text
//! <root>/
//!   .bd-fixture-root              marker (clean refuses without it)
//!   bd-fixtures.manifest.json     what a correct scan must report
//!   bulk/                         deterministic tree for totals and speed
//!   edge/                         one folder per edge case
//! ```

use std::fs::{self, File, FileTimes};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bd_core::age::AgeBucket;
use bd_core::attrs::LinkKind;
use bd_core::category::FileType;
use bd_core::time::{TICKS_PER_DAY, TICKS_PER_SECOND, Ticks, UNIX_EPOCH_TICKS};

use crate::manifest::{BulkSummary, Expectation, MANIFEST_VERSION, Manifest, Skipped};
use crate::platform;
use crate::rng::SplitMix64;
use crate::{FIXTURE_MARKER, MANIFEST_FILE, display, prepare_root, rel, tree_kind};

#[derive(Clone, Debug)]
pub struct CreateOptions {
    /// Files in `bulk/`. Zero skips the bulk tree.
    pub bulk_files: u64,
    /// Approximate total logical bytes in `bulk/`.
    pub bulk_bytes: u64,
    pub seed: u64,
    /// Write diskpart scripts for an exFAT VHDX (Windows; run as
    /// administrator to test the enumeration fallback).
    pub vhd_scripts: bool,
}

impl Default for CreateOptions {
    fn default() -> CreateOptions {
        CreateOptions {
            bulk_files: 20_000,
            bulk_bytes: 1_000_000_000,
            seed: 0x00BD_5EED,
            vhd_scripts: false,
        }
    }
}

pub(crate) fn ticks_to_system_time(t: Ticks) -> SystemTime {
    let since = t.raw() - UNIX_EPOCH_TICKS;
    if since >= 0 {
        UNIX_EPOCH + Duration::from_nanos(since as u64 * 100)
    } else {
        UNIX_EPOCH - Duration::from_nanos(since.unsigned_abs() * 100)
    }
}

pub(crate) fn set_modified(path: &Path, t: Ticks) -> io::Result<()> {
    let st = ticks_to_system_time(t);
    File::options()
        .write(true)
        .open(path)?
        .set_times(FileTimes::new().set_modified(st).set_accessed(st))
}

/// Writes deterministic, incompressible content.
pub(crate) struct Writer {
    pattern: Vec<u8>,
}

impl Writer {
    pub(crate) fn new(seed: u64) -> Writer {
        let mut pattern = vec![0u8; 1 << 20];
        SplitMix64::new(seed ^ 0xC0FF_EE00).fill(&mut pattern);
        Writer { pattern }
    }

    pub(crate) fn write(&self, path: &Path, len: u64) -> io::Result<File> {
        let mut f = File::create(path)?;
        self.write_to(&mut f, len)?;
        Ok(f)
    }

    pub(crate) fn write_to(&self, f: &mut File, len: u64) -> io::Result<()> {
        let mut left = len;
        let mut offset = (len as usize * 7919) % self.pattern.len();
        while left > 0 {
            let n = (left as usize).min(self.pattern.len() - offset);
            f.write_all(&self.pattern[offset..offset + n])?;
            left -= n as u64;
            offset = 0;
        }
        Ok(())
    }
}

struct Builder<'a> {
    root: PathBuf,
    now: Ticks,
    writer: Writer,
    expectations: Vec<Expectation>,
    skipped: Vec<Skipped>,
    log: &'a mut dyn FnMut(&str),
}

impl Builder<'_> {
    fn p(&self, r: &str) -> PathBuf {
        self.root.join(rel(r))
    }

    fn mkdir(&self, r: &str) -> io::Result<PathBuf> {
        let p = self.p(r);
        fs::create_dir_all(&p)?;
        Ok(p)
    }

    fn file(&self, r: &str, len: u64) -> io::Result<PathBuf> {
        let p = self.p(r);
        self.writer.write(&p, len)?;
        Ok(p)
    }

    /// Run one case; its expectations count only if every step worked.
    fn case(&mut self, name: &str, f: impl FnOnce(&Self) -> io::Result<Vec<Expectation>>) {
        match f(self) {
            Ok(e) => self.expectations.extend(e),
            Err(err) => {
                (self.log)(&format!("  skipped {name}: {err}"));
                self.skipped.push(Skipped {
                    case: name.to_string(),
                    reason: err.to_string(),
                });
            }
        }
    }
}

fn size_and_name(r: &str, logical: u64) -> Vec<Expectation> {
    vec![
        Expectation::FileSize {
            path: r.to_string(),
            logical,
        },
        Expectation::Exists {
            path: r.to_string(),
        },
    ]
}

/// Create the fixture at `root`, which must not exist or be empty.
pub fn create(
    root: &Path,
    opts: &CreateOptions,
    log: &mut dyn FnMut(&str),
) -> io::Result<Manifest> {
    let root = prepare_root(root)?;
    if tree_kind(&root).is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "{} already holds a fixture; run `bd-fixtures clean` first",
                display(&root)
            ),
        ));
    }
    if root.exists() && fs::read_dir(&root)?.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "{} exists and is not empty; choose a new folder",
                display(&root)
            ),
        ));
    }
    fs::create_dir_all(&root)?;
    let now = Ticks::now();
    fs::write(
        root.join(FIXTURE_MARKER),
        format!(
            "{{\"tool\":\"bd-fixtures\",\"kind\":\"fixture\",\"createdAt\":\"{}\"}}\n",
            now.to_iso8601()
        ),
    )?;

    let mut b = Builder {
        root: root.clone(),
        now,
        writer: Writer::new(opts.seed),
        expectations: Vec::new(),
        skipped: Vec::new(),
        log,
    };

    let bulk = if opts.bulk_files > 0 {
        (b.log)(&format!(
            "bulk: {} files, ~{}",
            opts.bulk_files,
            bd_core::bytes::format_decimal(u128::from(opts.bulk_bytes))
        ));
        let summary = create_bulk(&mut b, opts)?;
        b.expectations.push(Expectation::SubtreeTotals {
            path: "bulk".into(),
            files: summary.files,
            dirs: summary.dirs,
            logical: summary.logical_bytes,
        });
        Some(summary)
    } else {
        None
    };

    (b.log)("edge cases");
    create_edge(&mut b)?;

    if opts.vhd_scripts {
        write_vhd_scripts(&b)?;
    }

    let manifest = Manifest {
        version: MANIFEST_VERSION,
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        platform: std::env::consts::OS.to_string(),
        created_at_ticks: now.raw(),
        created_at: now.to_iso8601(),
        seed: opts.seed,
        bulk,
        expectations: b.expectations,
        skipped: b.skipped,
    };
    fs::write(
        root.join(MANIFEST_FILE),
        serde_json::to_string_pretty(&manifest).map_err(io::Error::other)?,
    )?;
    Ok(manifest)
}

const BULK_EXTENSIONS: [&str; 8] = ["txt", "pdf", "mp4", "zip", "mp3", "jpg", "bin", "dat"];

fn create_bulk(b: &mut Builder<'_>, opts: &CreateOptions) -> io::Result<BulkSummary> {
    let mut rng = SplitMix64::new(opts.seed);
    let files = opts.bulk_files;
    let dirs = (files / 40).max(1);

    // An 8-ary tree of folders under bulk/.
    let mut dir_paths: Vec<String> = Vec::with_capacity(dirs as usize + 1);
    dir_paths.push("bulk".to_string());
    for i in 1..=dirs {
        let parent = &dir_paths[((i - 1) / 8) as usize];
        dir_paths.push(format!("{parent}/d{i:05}"));
    }
    for d in &dir_paths {
        b.mkdir(d)?;
    }

    // Mostly small files, a long tail of large ones, scaled to the target.
    let raw: Vec<u64> = (0..files)
        .map(|_| match rng.range(0, 100) {
            0..60 => rng.range(0, 4096),
            60..90 => rng.range(4096, 256 << 10),
            90..99 => rng.range(256 << 10, 4 << 20),
            _ => rng.range(4 << 20, 64 << 20),
        })
        .collect();
    let raw_total: u128 = raw.iter().map(|&s| u128::from(s)).sum::<u128>().max(1);
    let mut logical_total = 0u64;
    let step = (files / 20).max(1);
    for (j, &r) in raw.iter().enumerate() {
        let size = (u128::from(r) * u128::from(opts.bulk_bytes) / raw_total) as u64;
        let dir = &dir_paths[rng.range(0, dirs + 1) as usize];
        let ext = BULK_EXTENSIONS[rng.range(0, BULK_EXTENSIONS.len() as u64) as usize];
        let path = b.p(&format!("{dir}/f{j:07}.{ext}"));
        let f = b.writer.write(&path, size)?;
        let age = rng.range(0, 1500) * TICKS_PER_DAY as u64
            + rng.range(0, 86_400) * TICKS_PER_SECOND as u64;
        if let Some(t) = b.now.checked_add_ticks(-(age as i64)) {
            let st = ticks_to_system_time(t);
            f.set_times(FileTimes::new().set_modified(st).set_accessed(st))?;
        }
        logical_total += size;
        if (j as u64 + 1).is_multiple_of(step) {
            (b.log)(&format!("  {} / {files} files", j + 1));
        }
    }
    Ok(BulkSummary {
        files,
        dirs,
        logical_bytes: logical_total,
    })
}

fn create_edge(b: &mut Builder<'_>) -> io::Result<()> {
    b.mkdir("edge")?;

    b.case("zero-byte file", |b| {
        b.file("edge/zero-byte.txt", 0)?;
        Ok(size_and_name("edge/zero-byte.txt", 0))
    });

    b.case("empty folder", |b| {
        b.mkdir("edge/empty")?;
        Ok(vec![Expectation::SubtreeTotals {
            path: "edge/empty".into(),
            files: 0,
            dirs: 0,
            logical: 0,
        }])
    });

    b.case("file types", |b| {
        b.mkdir("edge/types")?;
        let files: [(&str, u64, FileType); 8] = [
            ("report.pdf", 1000, FileType::Documents),
            ("clip.mp4", 2000, FileType::Videos),
            ("bundle.zip", 3000, FileType::Archives),
            ("song.mp3", 4000, FileType::Music),
            ("photo.JPG", 5000, FileType::Images),
            ("data.xyz", 6000, FileType::Other),
            ("noext", 10, FileType::Other),
            (".hidden", 20, FileType::Other),
        ];
        let mut e = Vec::new();
        for (name, len, ty) in files {
            let r = format!("edge/types/{name}");
            b.file(&r, len)?;
            e.push(Expectation::FileType {
                path: r.clone(),
                file_type: ty,
            });
            e.push(Expectation::FileSize {
                path: r,
                logical: len,
            });
        }
        e.push(Expectation::SubtreeTotals {
            path: "edge/types".into(),
            files: 8,
            dirs: 0,
            logical: files.iter().map(|f| f.1).sum(),
        });
        Ok(e)
    });

    b.case("modified-date buckets", |b| {
        b.mkdir("edge/dates")?;
        let cases: [(i64, AgeBucket); 6] = [
            (1, AgeBucket::Within7Days),
            (20, AgeBucket::Days8To30),
            (60, AgeBucket::Days31To90),
            (200, AgeBucket::Days91To365),
            (800, AgeBucket::OverOneYear),
            (-30, AgeBucket::Unknown),
        ];
        let mut e = Vec::new();
        for (days, bucket) in cases {
            let r = if days >= 0 {
                format!("edge/dates/age-{days:03}d.txt")
            } else {
                format!("edge/dates/future-{}d.txt", -days)
            };
            let p = b.file(&r, 100)?;
            let t = b
                .now
                .checked_add_days(-days)
                .ok_or_else(|| io::Error::other("date out of range"))?;
            set_modified(&p, t)?;
            e.push(Expectation::AgeBucket { path: r, bucket });
        }
        Ok(e)
    });

    b.case("hard links", |b| {
        b.mkdir("edge/hardlinks/nested")?;
        let original = b.file("edge/hardlinks/original.bin", 100_000)?;
        fs::hard_link(&original, b.p("edge/hardlinks/alias-1.bin"))?;
        fs::hard_link(&original, b.p("edge/hardlinks/nested/alias-2.bin"))?;
        Ok(vec![Expectation::HardLinkGroup {
            paths: vec![
                "edge/hardlinks/alias-1.bin".into(),
                "edge/hardlinks/nested/alias-2.bin".into(),
                "edge/hardlinks/original.bin".into(),
            ],
            logical: 100_000,
        }])
    });

    b.case("hard link written after linking", |b| {
        b.mkdir("edge/hardlinks-stale")?;
        let a = b.file("edge/hardlinks-stale/a.bin", 10_000)?;
        fs::hard_link(&a, b.p("edge/hardlinks-stale/b.bin"))?;
        let mut f = File::options().append(true).open(&a)?;
        b.writer.write_to(&mut f, 50_000)?;
        Ok(vec![Expectation::StaleHardLink {
            paths: vec![
                "edge/hardlinks-stale/a.bin".into(),
                "edge/hardlinks-stale/b.bin".into(),
            ],
            final_logical: 60_000,
        }])
    });

    b.case("sparse file", |b| {
        b.mkdir("edge/sparse")?;
        let logical = 256 << 20;
        platform::write_sparse(&b.p("edge/sparse/sparse.bin"), &[0xAB; 4096], logical)?;
        Ok(vec![Expectation::Sparse {
            path: "edge/sparse/sparse.bin".into(),
            logical,
            max_allocated: 1 << 20,
        }])
    });

    b.case("NTFS-compressed file", |b| {
        b.mkdir("edge/compressed")?;
        let data: Vec<u8> = b"Better Disk Manager compressible line.\n"
            .iter()
            .copied()
            .cycle()
            .take(8 << 20)
            .collect();
        platform::write_compressed(&b.p("edge/compressed/compressible.txt"), &data)?;
        Ok(vec![Expectation::Compressed {
            path: "edge/compressed/compressible.txt".into(),
            logical: data.len() as u64,
        }])
    });

    b.case("alternate data stream", |b| {
        b.mkdir("edge/ads")?;
        let p = b.file("edge/ads/with-stream.txt", 1000)?;
        platform::write_stream(&p, "extra", &vec![7u8; 1 << 20])?;
        Ok(vec![Expectation::AlternateStream {
            path: "edge/ads/with-stream.txt".into(),
            main_logical: 1000,
            stream_bytes: 1 << 20,
        }])
    });

    b.mkdir("edge/links")?;
    b.case("junction", |b| {
        platform::junction(&b.p("edge/links/junction-to-types"), &b.p("edge/types"))?;
        Ok(vec![Expectation::Link {
            path: "edge/links/junction-to-types".into(),
            link_kind: Some(LinkKind::MountPoint),
        }])
    });
    b.case("junction cycle", |b| {
        platform::junction(&b.p("edge/links/junction-cycle"), &b.p("edge"))?;
        Ok(vec![Expectation::Link {
            path: "edge/links/junction-cycle".into(),
            link_kind: Some(LinkKind::MountPoint),
        }])
    });
    b.case("directory symlink", |b| {
        platform::symlink_dir(
            &Path::new("..").join("types"),
            &b.p("edge/links/symlink-dir"),
        )?;
        Ok(vec![Expectation::Link {
            path: "edge/links/symlink-dir".into(),
            link_kind: Some(LinkKind::Symlink),
        }])
    });
    b.case("file symlink", |b| {
        platform::symlink_file(
            &Path::new("..").join("zero-byte.txt"),
            &b.p("edge/links/symlink-file"),
        )?;
        Ok(vec![Expectation::Link {
            path: "edge/links/symlink-file".into(),
            link_kind: Some(LinkKind::Symlink),
        }])
    });
    b.case("symlink cycle", |b| {
        platform::symlink_dir(Path::new(".."), &b.p("edge/links/symlink-cycle"))?;
        Ok(vec![Expectation::Link {
            path: "edge/links/symlink-cycle".into(),
            link_kind: Some(LinkKind::Symlink),
        }])
    });

    b.case("case-sensitive folder", |b| {
        let dir = b.mkdir("edge/case")?;
        platform::set_case_sensitive(&dir)?;
        b.file("edge/case/File.txt", 100)?;
        let lower = b.p("edge/case/file.txt");
        // On a case-insensitive folder this would open File.txt instead.
        let f = File::options().write(true).create_new(true).open(&lower)?;
        drop(f);
        b.writer.write(&lower, 200)?;
        Ok(vec![Expectation::CaseSensitivePair {
            parent: "edge/case".into(),
            names: ["File.txt".into(), "file.txt".into()],
        }])
    });

    b.mkdir("edge/names")?;
    let names: [(&str, &str, u64); 7] = [
        ("accented", "héllo wörld.txt", 11),
        ("CJK", "日本語.txt", 12),
        ("emoji (surrogate pair)", "emoji-😀.txt", 13),
        ("fullwidth tilde", "fullwidth-～.txt", 14),
        ("trailing dot", "trailing.", 15),
        ("trailing space", "trailing ", 16),
        ("reserved device name", "CON", 17),
    ];
    for (case, name, len) in names {
        b.case(&format!("name: {case}"), |b| {
            let r = format!("edge/names/{name}");
            b.file(&r, len)?;
            Ok(size_and_name(&r, len))
        });
    }
    b.case("name: not valid Unicode", |b| {
        let p = b.p("edge/names").join(platform::non_unicode_name());
        b.writer.write(&p, 18)?;
        Ok(vec![Expectation::NonUnicodeNames {
            parent: "edge/names".into(),
            count: 1,
        }])
    });

    b.case("path longer than 260 characters", |b| {
        let mut r = String::from("edge/deep");
        for i in 0..10 {
            r.push_str(&format!("/level-{i:02}-{}", "x".repeat(24)));
        }
        b.mkdir(&r)?;
        r.push_str("/deep.bin");
        b.file(&r, 12_345)?;
        Ok(size_and_name(&r, 12_345))
    });

    b.case("unreadable folder", |b| {
        b.mkdir("edge/denied")?;
        b.file("edge/denied/secret.txt", 111)?;
        let effective = platform::deny(&b.p("edge/denied"))?;
        Ok(vec![Expectation::Denied {
            path: "edge/denied".into(),
            effective,
        }])
    });

    Ok(())
}

fn write_vhd_scripts(b: &Builder<'_>) -> io::Result<()> {
    let dir = b.mkdir("vhd")?;
    let vhdx = display(&dir.join("exfat-test.vhdx"));
    fs::write(
        dir.join("create-exfat.diskpart"),
        format!(
            "rem Run as administrator: diskpart /s create-exfat.diskpart\r\n\
             create vdisk file=\"{vhdx}\" maximum=512 type=expandable\r\n\
             select vdisk file=\"{vhdx}\"\r\n\
             attach vdisk\r\n\
             create partition primary\r\n\
             format fs=exfat quick label=BDEXFAT\r\n\
             assign\r\n"
        ),
    )?;
    fs::write(
        dir.join("detach.diskpart"),
        format!("select vdisk file=\"{vhdx}\"\r\ndetach vdisk\r\n"),
    )?;
    Ok(())
}
