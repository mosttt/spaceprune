//! Identity-bound process working-set maintenance, accounted separately from disk cleanup.
#![deny(missing_docs)]

use serde::{Deserialize, Serialize};
use std::io;

mod recipe;
pub use recipe::{ReleaseAccess, register};

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

/// A process selection bound to both its lifetime and executable, never its basename alone.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BoundProcess {
    /// PID and native start identity observed during this scan.
    pub identity: ProcessIdentity,
    /// Exact executable path observed for this process.
    pub executable: String,
}

/// A readable process owned by the current user.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessMemory {
    /// Stable selection binding.
    pub identity: ProcessIdentity,
    /// Executable basename.
    pub name: String,
    /// Executable path when readable and representable as Unicode; required for reusable rules.
    pub executable: Option<String>,
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
    /// Whether the selected process exited before any native advice was applied.
    pub already_absent: bool,
    /// Whether any native advice was applied, even if a later step failed.
    #[serde(default)]
    pub applied: bool,
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

/// Release exact executable-bound selections using only authority already held by this process.
pub fn release_bound(
    selected: &[BoundProcess],
    check: &dyn Fn() -> io::Result<()>,
) -> io::Result<Vec<MemoryRelease>> {
    validate_bound(selected)?;
    #[cfg(windows)]
    return selected
        .iter()
        .map(|selection| {
            if let Err(error) = check() {
                return Ok(MemoryRelease {
                    identity: selection.identity.clone(),
                    succeeded: false,
                    already_absent: false,
                    applied: false,
                    resident_reduction_bytes: None,
                    error: Some(error.to_string()),
                });
            }
            Ok(native::release_matching(
                &selection.identity,
                Some(&selection.executable),
            ))
        })
        .collect();
    #[cfg(target_os = "linux")]
    return linux::release_bound_for_user(selected, linux::current_user(), check);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = check;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Memory maintenance requires Windows or Linux",
        ))
    }
}

fn validate_bound(selected: &[BoundProcess]) -> io::Result<()> {
    if selected.is_empty()
        || selected.len() > 128
        || selected
            .iter()
            .map(|p| &p.identity)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != selected.len()
        || selected
            .iter()
            .any(|p| !std::path::Path::new(&p.executable).is_absolute())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Select 1 to 128 distinct executable-bound processes",
        ));
    }
    Ok(())
}

/// Discover one read-only, executable-bound capability per currently observable application.
pub fn discover_tasks(
    context: &spaceprune_access::OperationContext,
) -> io::Result<Vec<spaceprune_plugin_api::TaskRequest>> {
    context.check()?;
    let snapshot = inspect()?;
    let applications = snapshot
        .processes
        .into_iter()
        .filter_map(|p| p.executable)
        .collect::<std::collections::BTreeSet<_>>();
    context.check()?;
    Ok(applications
        .into_iter()
        .map(|executable| spaceprune_plugin_api::TaskRequest {
            module_id: "system-evidence".into(),
            task_id: "process-working-set".into(),
            name: std::path::Path::new(&executable)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| executable.clone()),
            handler_id: "builtin.memory/process-working-set".into(),
            options: serde_json::json!({"applications":[executable]}),
            recommend: serde_json::json!({"enabled":false}),
            tags: vec!["memory".into()],
        })
        .collect())
}
