//! Explicit process working-set maintenance, separate from disk cleanup and Recipes.
#![deny(missing_docs)]

use serde::{Deserialize, Serialize};
use std::io;

#[cfg(windows)]
#[allow(unsafe_code)]
#[path = "platform/windows.rs"]
mod native;

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
#[path = "platform/linux.rs"]
pub mod linux;

/// One process identity; creation time prevents PID reuse from retargeting a selection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessIdentity {
    /// Native process identifier.
    pub pid: u32,
    /// Opaque native start identity, encoded as text to preserve integer precision.
    pub started_at: String,
}

/// A readable process owned by the current user.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessMemory {
    /// Stable selection binding.
    pub identity: ProcessIdentity,
    /// Executable basename.
    pub name: String,
    /// Current physical working set; it can include shared pages.
    pub resident_bytes: u64,
    /// Native private memory: committed bytes on Windows, resident private bytes on Linux.
    /// Neither value is a releasable-space estimate.
    pub private_bytes: u64,
}

/// Read-only physical memory summary and eligible current-user processes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySnapshot {
    /// Installed usable physical memory.
    pub total_bytes: u64,
    /// Physical memory currently available to the operating system.
    pub available_bytes: u64,
    /// Current-user processes for which the required access is available.
    pub processes: Vec<ProcessMemory>,
}

/// Result for one exact selection, including failure without retrying another process.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryRelease {
    /// The originally selected process.
    pub identity: ProcessIdentity,
    /// Whether the native operation completed without a reported error.
    pub succeeded: bool,
    /// Observed reduction in this process's working set, not guaranteed global freed RAM.
    pub resident_reduction_bytes: Option<u64>,
    /// Native error text or identity mismatch, when applicable.
    pub error: Option<String>,
}

/// Inspect memory without changing processes or requesting administrator access.
pub fn inspect() -> io::Result<MemorySnapshot> {
    #[cfg(windows)]
    return native::inspect();
    #[cfg(target_os = "linux")]
    return linux::inspect();
    #[cfg(not(any(windows, target_os = "linux")))]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Memory maintenance requires Windows or Linux",
    ))
}

/// Trim only explicitly selected current-user working sets; never terminate a process.
pub fn release(selected: &[ProcessIdentity]) -> io::Result<Vec<MemoryRelease>> {
    if selected.is_empty()
        || selected.len() > 128
        || selected
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != selected.len()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Select between 1 and 128 distinct processes",
        ));
    }
    #[cfg(windows)]
    return Ok(selected
        .iter()
        .map(|identity| native::release_one(identity))
        .collect());
    #[cfg(target_os = "linux")]
    return linux::release_for_user(selected, linux::current_user(), &|| Ok(()));
    #[cfg(not(any(windows, target_os = "linux")))]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Memory maintenance requires Windows or Linux",
    ))
}
