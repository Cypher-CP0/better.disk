//! The recycle sandbox: disposable items the recycle spike may touch, and
//! nothing else.
//!
//! ```text
//! <root>/
//!   .bd-recycle-sandbox           marker (the spike refuses without it)
//!   bd-sandbox.manifest.json      the items and what each one tests
//!   items/...
//! ```

use std::fs;
use std::io;
use std::path::Path;

use bd_core::time::Ticks;
use serde::{Deserialize, Serialize};

use crate::create::Writer;
use crate::manifest::Skipped;
use crate::{
    SANDBOX_MANIFEST_FILE, SANDBOX_MARKER, display, platform, prepare_root, rel, tree_kind,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxItem {
    /// `/`-separated, relative to the sandbox root.
    pub path: String,
    pub kind: String,
    /// What a correct outcome looks like.
    pub expect: String,
    /// Paths that must still exist afterwards.
    #[serde(default)]
    pub must_survive: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxManifest {
    pub version: u32,
    pub created_at: String,
    pub items: Vec<SandboxItem>,
    pub skipped: Vec<Skipped>,
}

fn item(path: &str, kind: &str, expect: &str) -> SandboxItem {
    SandboxItem {
        path: path.into(),
        kind: kind.into(),
        expect: expect.into(),
        must_survive: Vec::new(),
    }
}

pub fn create_sandbox(
    root: &Path,
    large_bytes: Option<u64>,
    log: &mut dyn FnMut(&str),
) -> io::Result<SandboxManifest> {
    let root = prepare_root(root)?;
    if tree_kind(&root).is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "{} already holds a fixture or sandbox; run `bd-fixtures clean` first",
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
    fs::create_dir_all(root.join("items"))?;
    let now = Ticks::now();
    fs::write(
        root.join(SANDBOX_MARKER),
        format!(
            "{{\"tool\":\"bd-fixtures\",\"kind\":\"recycle-sandbox\",\"createdAt\":\"{}\"}}\n",
            now.to_iso8601()
        ),
    )?;

    let w = Writer::new(0x5A_4DB0);
    let p = |r: &str| root.join(rel(r));
    let mut items = Vec::new();
    let mut skipped = Vec::new();
    let mut attempt = |case: &str, r: io::Result<SandboxItem>| match r {
        Ok(i) => items.push(i),
        Err(e) => {
            log(&format!("  skipped {case}: {e}"));
            skipped.push(Skipped {
                case: case.into(),
                reason: e.to_string(),
            });
        }
    };

    attempt(
        "small file",
        w.write(&p("items/small.txt"), 1024)
            .map(|_| item("items/small.txt", "file", "recycled")),
    );

    attempt(
        "folder",
        (|| {
            fs::create_dir_all(p("items/folder/sub"))?;
            w.write(&p("items/folder/a.txt"), 100)?;
            w.write(&p("items/folder/b.txt"), 200)?;
            w.write(&p("items/folder/sub/c.txt"), 300)?;
            Ok(item("items/folder", "folder", "recycled as one item"))
        })(),
    );

    attempt(
        "Unicode name",
        w.write(&p("items/unicode-名前-😀.txt"), 512)
            .map(|_| item("items/unicode-名前-😀.txt", "unicode-name", "recycled")),
    );

    attempt(
        "trailing dot",
        w.write(&p("items/trailing."), 64).map(|_| {
            item(
                "items/trailing.",
                "trailing-dot",
                "recycled, or Failed with a clear reason if the Shell cannot address it",
            )
        }),
    );

    attempt(
        "long path",
        (|| {
            let mut r = String::from("items/deep");
            for i in 0..10 {
                r.push_str(&format!("/level-{i:02}-{}", "y".repeat(24)));
            }
            fs::create_dir_all(p(&r))?;
            r.push_str("/long.bin");
            w.write(&p(&r), 4096)?;
            Ok(item(
                &r,
                "long-path",
                "recycled, or Failed with a clear reason if the Shell cannot address it",
            ))
        })(),
    );

    attempt(
        "hard link",
        (|| {
            w.write(&p("items/hardlink-a.bin"), 50_000)?;
            fs::hard_link(p("items/hardlink-a.bin"), p("items/hardlink-b.bin"))?;
            let mut i = item(
                "items/hardlink-a.bin",
                "hard-link",
                "recycled; the other name keeps the data",
            );
            i.must_survive = vec!["items/hardlink-b.bin".into()];
            Ok(i)
        })(),
    );

    attempt(
        "junction",
        (|| {
            fs::create_dir_all(p("items/junction-target"))?;
            w.write(&p("items/junction-target/keep.txt"), 10)?;
            platform::junction(&p("items/junction"), &p("items/junction-target"))?;
            let mut i = item(
                "items/junction",
                "junction",
                "only the link is recycled; the target and its contents survive",
            );
            i.must_survive = vec!["items/junction-target/keep.txt".into()];
            Ok(i)
        })(),
    );

    attempt(
        "read-only file",
        (|| {
            let path = p("items/readonly.txt");
            w.write(&path, 300)?;
            let mut perm = fs::metadata(&path)?.permissions();
            perm.set_readonly(true);
            fs::set_permissions(&path, perm)?;
            Ok(item(
                "items/readonly.txt",
                "read-only",
                "recycled (the Shell may ask in the confirm profile)",
            ))
        })(),
    );

    attempt(
        "file in use",
        w.write(&p("items/in-use.txt"), 200).map(|_| {
            item(
                "items/in-use.txt",
                "in-use",
                "with --hold-open: Failed or Cancelled and still present; otherwise recycled",
            )
        }),
    );

    if let Some(len) = large_bytes {
        attempt(
            "large file",
            w.write(&p("items/large.bin"), len).map(|_| {
                item(
                    "items/large.bin",
                    "too-large",
                    "Blocked when larger than the Recycle Bin allows; never permanently deleted",
                )
            }),
        );
    }

    let manifest = SandboxManifest {
        version: 1,
        created_at: now.to_iso8601(),
        items,
        skipped,
    };
    fs::write(
        root.join(SANDBOX_MANIFEST_FILE),
        serde_json::to_string_pretty(&manifest).map_err(io::Error::other)?,
    )?;
    Ok(manifest)
}
