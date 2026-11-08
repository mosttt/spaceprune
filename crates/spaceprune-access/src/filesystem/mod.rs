//! Identity-bound native deletion shared by ordinary and elevated callers.

use crate::OperationContext;
use spaceprune_contracts::messages;

#[cfg(target_os = "linux")]
mod inspection;
#[cfg(target_os = "linux")]
pub use inspection::{DirectoryInspection, inspect_directory, inspect_entry_metadata};

/// Reserved transient execution names must never become candidates after an interrupted run.
pub fn is_execution_artifact(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        #[cfg(windows)]
        let name = name.to_ascii_lowercase();
        name.starts_with(".spaceprune-execute-")
            || name == ".spaceprune-executions"
            || name.starts_with("spaceprune-readonly-")
            || name.starts_with("spaceprune-cleanup-")
    })
}
#[cfg(unix)]
use std::fs;
use std::{io, path::Path};

#[cfg(all(test, unix))]
mod tests;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

pub use spaceprune_contracts::rule::FileIdentity;

/// Capture metadata for the entry itself, preserving symbolic-link selection semantics.
pub fn read_identity(path: &Path) -> io::Result<FileIdentity> {
    #[cfg(unix)]
    {
        identity_from_metadata(&fs::symlink_metadata(path)?)
    }
    #[cfg(windows)]
    {
        windows::identify(path)
    }
}

/// Run a synchronous domain operation against the selected directory, captured atomically.
///
/// The callback must set its child process's working directory to the first path and use the
/// second path as its directory argument. Both are valid only during the callback; no worker
/// may outlive it. Remaining contents are restored without replacing a concurrently created
/// original name. A missing selection returns `None`; identity changes never run the callback.
/// Nested mounts, unreadable directories and reserved execution artifacts are rejected before
/// dispatch. Domain commands must still respect the captured subtree during concurrent changes.
#[cfg(target_os = "linux")]
pub fn with_captured_directory<T>(
    path: &Path,
    parent_identity: &FileIdentity,
    identity: &FileIdentity,
    context: &OperationContext,
    action: impl FnOnce(&Path, &Path) -> T,
) -> io::Result<Option<T>> {
    if is_execution_artifact(path) {
        return Err(crate::error::failure(
            crate::IoFailureKind::BoundaryViolation,
            messages::text("native.artifact_new_operation"),
        ));
    }
    unix::with_directory(path, parent_identity, identity, context, action)
}

/// Capture the platform's stable identity from already observed metadata.
#[cfg(unix)]
pub fn identity_from_metadata(metadata: &fs::Metadata) -> io::Result<FileIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
            high: 0,
            birth: metadata
                .created()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|duration| u64::try_from(duration.as_nanos()).ok()),
        })
    }
}

/// Observable effects, including effects before a context or failure.
#[derive(Debug, Default)]
pub struct Removal {
    /// Whether the original selected object was already absent before execution.
    pub already_absent: bool,
    /// Number of removed entries; an interrupted subtree can have nonzero effects.
    pub removed_entries: usize,
    /// Logical bytes of ordinary files actually removed; an estimate of reclaimed space.
    pub reclaimed_estimate: u64,
    /// Failure after any recorded effects. The caller must not replay the whole item blindly.
    pub error: Option<io::Error>,
}

/// Remove an identity-bound selection within one verified root.
///
/// The relative selection may contain only normal components. The root itself is not a
/// removable selection. Exclusions apply to the original relative names throughout execution.
/// An exact file root is represented by its verified parent and one relative filename.
pub fn remove_selected(
    root: &Path,
    root_identity: &FileIdentity,
    relative: &Path,
    identity: &FileIdentity,
    excluded: &dyn Fn(&Path) -> bool,
    context: &OperationContext,
) -> Removal {
    let mut result = Removal::default();
    if is_execution_artifact(root) || is_execution_artifact(relative) {
        result.error = Some(crate::error::failure(
            crate::IoFailureKind::BoundaryViolation,
            messages::text("native.artifact_cleanup"),
        ));
        return result;
    }
    let excluded = |path: &Path| is_execution_artifact(path) || excluded(path);
    #[cfg(unix)]
    let deletion = unix::remove(
        root,
        root_identity,
        relative,
        identity,
        &excluded,
        context,
        &mut result,
    );
    #[cfg(windows)]
    let deletion = windows::remove(
        root,
        root_identity,
        relative,
        identity,
        &excluded,
        context,
        &mut result,
    );
    if let Err(error) = deletion {
        result.error = Some(error);
    }
    result
}

/// Remove an already bound Go module-cache directory with dynamic descendant capture.
/// This does not change directory modes or invoke Go. Read-only owned module directories
/// require the caller's dedicated user-namespace worker or actual host authorization.
#[cfg(target_os = "linux")]
pub fn remove_go_directory(
    binding: &spaceprune_contracts::cache::DirectoryBinding,
    context: &OperationContext,
) -> Removal {
    let mut result = Removal::default();
    let removal = (|| {
        let path = binding
            .path
            .to_path_buf()
            .filter(|path| path.is_absolute())
            .ok_or_else(|| io::Error::other(messages::text("linux.go_invalid_binding")))?;
        if is_execution_artifact(&path) {
            return Err(crate::error::failure(
                crate::IoFailureKind::BoundaryViolation,
                messages::text("native.artifact_cleanup"),
            ));
        }
        unix::remove_go_directory(
            &path,
            &binding.parent_identity,
            &binding.identity,
            context,
            &mut result,
        )
    })();
    if let Err(error) = removal {
        result.error = Some(
            if result.removed_entries > 0 && !crate::error::has_effects(&error) {
                crate::error::failure_after_effects(
                    crate::error::classify(&error).kind,
                    error.to_string(),
                    &error,
                )
            } else {
                error
            },
        );
    }
    result
}
