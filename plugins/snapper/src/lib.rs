#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! Read-only Snapper inventory and snapshot cleanup.
//!
//! Reported sizes do not guarantee reclaimable space because snapshots can share Btrfs blocks.

#[cfg(target_os = "linux")]
include!("linux.rs");
