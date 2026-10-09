//! Disposable fixture trees for Stage 0.
//!
//! Every tree this crate creates carries a marker file at its root, and
//! [`clean`] refuses to delete anything without one. The recycle sandbox has
//! its own marker so the recycle spike can refuse to touch anything else
//! (the specification: never test cleanup against a user's real files).
//!
//! * [`create::create`] builds the scan fixture: a deterministic bulk tree
//!   plus one directory per edge case, and writes a [`manifest::Manifest`]
//!   of what a correct scan must report.
//! * [`manifest::check`] compares a finished scan with that manifest.
//! * [`sandbox::create_sandbox`] builds the recycle sandbox.

use std::io;
use std::path::{Path, PathBuf};

pub mod create;
pub mod manifest;
mod platform;
pub mod rng;
pub mod sandbox;

/// Marker at the root of a scan fixture.
pub const FIXTURE_MARKER: &str = ".bd-fixture-root";
/// Marker at the root of a recycle sandbox.
pub const SANDBOX_MARKER: &str = ".bd-recycle-sandbox";
/// Scan fixture manifest file name, next to the marker.
pub const MANIFEST_FILE: &str = "bd-fixtures.manifest.json";
/// Recycle sandbox manifest file name, next to the marker.
pub const SANDBOX_MANIFEST_FILE: &str = "bd-sandbox.manifest.json";

/// Turn a user path into the form fixtures are built on: absolute, and on
/// Windows verbatim (`\\?\`), so long paths and trailing-dot names work.
pub fn prepare_root(path: &Path) -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        bd_win::wide::to_verbatim(path)
    }
    #[cfg(not(windows))]
    {
        std::path::absolute(path)
    }
}

/// Display form of a path (verbatim prefix removed on Windows).
pub fn display(path: &Path) -> String {
    #[cfg(windows)]
    {
        bd_win::wide::display(path)
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().into_owned()
    }
}

/// Which kind of tree a marker says a directory is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeKind {
    Fixture,
    Sandbox,
}

pub fn tree_kind(root: &Path) -> Option<TreeKind> {
    if root.join(FIXTURE_MARKER).is_file() {
        Some(TreeKind::Fixture)
    } else if root.join(SANDBOX_MARKER).is_file() {
        Some(TreeKind::Sandbox)
    } else {
        None
    }
}

/// Delete a fixture or sandbox tree. Refuses unless the root carries one
/// of this crate's markers. Undoes permission denials first so the delete
/// can see everything. Links inside the tree are removed, never followed.
pub fn clean(root: &Path) -> io::Result<()> {
    let root = prepare_root(root)?;
    if tree_kind(&root).is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{} has no {FIXTURE_MARKER} or {SANDBOX_MARKER} marker; refusing to delete it",
                display(&root)
            ),
        ));
    }
    let denied = root.join("edge").join("denied");
    if std::fs::symlink_metadata(&denied).is_ok() {
        platform::undo_deny(&denied)?;
    }
    std::fs::remove_dir_all(&root)
}

/// Convert a manifest path (`/`-separated, relative) to a native path.
pub fn rel(path: &str) -> PathBuf {
    path.split('/').filter(|c| !c.is_empty()).collect()
}
