//! Platform-neutral core of Better Disk Manager.
//!
//! Everything in this crate is deterministic, OS-independent logic that the
//! Windows adapter (`bd-win`) and the application build on:
//!
//! * [`scan`] — the scan coordinator: a bounded worker pool driving a
//!   [`raw::DirReader`], throttled progress, and cancellation that keeps a
//!   clearly labelled partial result (FR01).
//! * [`tree`] — the in-memory scan index. Nodes are stored in depth-first
//!   pre-order and each node records where its subtree ends, so every folder's
//!   descendants form one contiguous slice (see `docs/decisions.md`, D1).
//!   Hard-link accounting by file identity lives here (FR02).
//! * [`aggregate`] — filtered folder aggregation for one navigation level
//!   (FR04, FR05) in a single linear pass.
//! * [`age`] — date buckets and the dominant-bucket rule (FR06).
//! * [`bytes`] — decimal display and strict size-input parsing (FR05).
//! * [`category`] — the versioned extension mapping (FR07).
//! * [`stdfs`] — a portable `std::fs` reader used for tests, non-Windows
//!   development, and as a speed baseline on Windows.

pub mod age;
pub mod aggregate;
pub mod attrs;
pub mod bytes;
pub mod category;
pub mod name;
pub mod raw;
pub mod scan;
pub mod stdfs;
pub mod time;
pub mod tree;

#[cfg(test)]
pub(crate) mod testutil;
