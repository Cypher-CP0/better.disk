//! Build a small fixture, scan it with the platform's best reader, and
//! require every manifest check to pass (or be explicitly skipped).

use std::path::{Path, PathBuf};

use bd_core::raw::DirReader;
use bd_core::scan::{CancelToken, ScanConfig, scan};
use bd_core::tree::ScanTree;
use bd_fixtures::create::{CreateOptions, create};
use bd_fixtures::manifest::{CheckStatus, check};
use bd_fixtures::sandbox::create_sandbox;
use bd_fixtures::{clean, prepare_root};

fn temp(label: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("bd-fixtures-{label}-{}", std::process::id()));
    if p.exists() {
        let _ = clean(&p);
        let _ = std::fs::remove_dir_all(&p);
    }
    p
}

fn reader() -> Box<dyn DirReader> {
    #[cfg(windows)]
    {
        Box::new(bd_win::WinDirReader::default())
    }
    #[cfg(not(windows))]
    {
        Box::new(bd_core::stdfs::StdDirReader)
    }
}

#[test]
fn fixture_scan_matches_manifest() {
    let root = temp("roundtrip");
    let opts = CreateOptions {
        bulk_files: 300,
        bulk_bytes: 3_000_000,
        ..CreateOptions::default()
    };
    let manifest = create(&root, &opts, &mut |_| {}).expect("create fixture");
    assert!(!manifest.expectations.is_empty());

    let scan_root = prepare_root(&root).unwrap();
    let reader = reader();
    let out = scan(
        &scan_root,
        reader.as_ref(),
        &ScanConfig::default(),
        &CancelToken::new(),
        |_| {},
    );
    let tree = ScanTree::build(out);
    let results = check(&tree, &manifest, Path::new(""));
    // How directory enumeration reports allocation for sparse and
    // NTFS-compressed files is a Stage 0 hypothesis (H3, H4), measured by
    // bd-spike-scan. Report it here without failing the build.
    const HYPOTHESES: [&str; 2] = ["sparse allocation", "compressed allocation"];
    let (hypotheses, failures): (Vec<_>, Vec<_>) = results
        .iter()
        .filter(|r| r.status == CheckStatus::Fail)
        .partition(|r| HYPOTHESES.iter().any(|h| r.check.starts_with(h)));
    for h in &hypotheses {
        eprintln!("hypothesis not confirmed: {}: {}", h.check, h.detail);
    }
    clean(&root).expect("clean fixture");
    assert!(failures.is_empty(), "failed checks: {failures:#?}");
    assert!(!root.exists());
}

#[test]
fn clean_refuses_unmarked_folders() {
    let root = temp("unmarked");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("precious.txt"), b"keep me").unwrap();
    assert!(clean(&root).is_err());
    assert!(root.join("precious.txt").exists());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn create_refuses_non_empty_folders() {
    let root = temp("nonempty");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("precious.txt"), b"keep me").unwrap();
    assert!(create(&root, &CreateOptions::default(), &mut |_| {}).is_err());
    assert!(create_sandbox(&root, None, &mut |_| {}).is_err());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn sandbox_lists_items() {
    let root = temp("sandbox");
    let m = create_sandbox(&root, Some(10_000), &mut |_| {}).expect("sandbox");
    assert!(m.items.iter().any(|i| i.kind == "file"));
    assert!(m.items.iter().any(|i| i.kind == "too-large"));
    for i in &m.items {
        let p = prepare_root(&root).unwrap().join(bd_fixtures::rel(&i.path));
        assert!(std::fs::symlink_metadata(&p).is_ok(), "{} missing", i.path);
    }
    clean(&root).unwrap();
}
