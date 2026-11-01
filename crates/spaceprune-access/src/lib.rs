//! Shared native operation capabilities. This crate has no UI, model, or cloud dependencies.

pub mod error;
#[cfg(windows)]
mod file_usage;
pub mod filesystem;
#[cfg(target_os = "linux")]
pub mod go_cache;
/// Linux native component installation and fixed capability endpoints.
#[cfg(target_os = "linux")]
pub mod helper;
pub mod host;
pub mod login;
/// Exclusive Windows loopback listeners for local application callbacks.
#[cfg(windows)]
pub mod loopback;
/// Signed macOS native components and capability transport; privacy and request scopes remain separate.
#[cfg(any(target_os = "macos", test))]
pub mod macos;
/// Platform-independent package descriptors for the native macOS components.
pub mod macos_package;
pub mod operation;
pub mod permission;
pub mod process;
pub mod storage;
/// Windows package sources and installed generations pinned to this host's build.
#[cfg(target_os = "windows")]
pub mod windows_native;
/// Platform-independent identity and layout of the fixed Windows native component package.
pub mod windows_package;

pub use operation::{CancellationToken, OperationContext, ResourceBudget};
pub use spaceprune_contracts::access::*;
