//! Windows adapter for Better Disk Manager.
//!
//! Everything that touches a Windows API lives here, behind small safe
//! functions, so `bd-core` stays deterministic and testable on any platform.
//!
//! * [`dirreader`] — [`WinDirReader`], a [`bd_core::raw::DirReader`] built on
//!   `GetFileInformationByHandleEx` directory classes. One handle per
//!   directory, no per-file opens, links never followed.
//! * [`volume`] — volume identity, capacity, file-system capabilities and the
//!   last-access update policy.
//! * [`verify`] — per-file handle queries used by the enumeration spike to
//!   check what directory enumeration reported. Never used during a scan.
//! * [`recycle`] — the recycle-only Shell adapter (`IFileOperation` with a
//!   progress sink that vetoes anything the Shell would permanently delete).
//! * [`process`] — process memory and start-time measurements for the spikes.
//! * [`wide`] — path conversion helpers (`\\?\` paths, NUL-terminated UTF-16).
//!
//! On non-Windows targets this crate is empty.

#![cfg(windows)]

pub mod dirreader;
pub mod error;
mod handle;
pub mod process;
pub mod recycle;
pub mod verify;
pub mod volume;
pub mod wide;

pub use dirreader::{EnumerationClass, WinDirReader};
