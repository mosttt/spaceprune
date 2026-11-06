#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! Pure public data contracts shared by SpacePrune hosts.
//!
//! This crate performs no filesystem, process, authorization, or network operations.
//! Parsing and editing source files, loading recipes, and executing handlers belong
//! to their respective host crates.

pub mod access;
pub mod bundle;
pub mod cache;
pub mod config;
pub mod handler;
pub mod logical_size;
pub mod plugin;
pub mod report;
pub mod rule;
pub mod schema;
pub mod size_summary;
pub mod storage;
pub mod updates;

/// Shared product display messages; execution and machine identifiers remain language independent.
pub mod messages;
