use spaceprune_access::{OperationContext, PermissionOutcome};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Stable installation identity shared by CLI and graphical hosts.
pub const HELPER_PATH: &str = "/usr/libexec/spaceprune/native-6-4/spaceprune-readonly-helper";

pub use spaceprune_access::process::{system_program, trusted_executable};

/// Validate the helper's installation without requesting authorization.
pub fn installed_helper(path: &Path) -> Result<PathBuf, PermissionOutcome> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(PermissionOutcome::HelperMissing)
        }
        Ok(metadata) if !metadata.file_type().is_symlink() => {
            trusted_executable(path).ok_or(PermissionOutcome::HelperUntrusted)
        }
        _ => Err(PermissionOutcome::HelperUntrusted),
    }
}

/// Check the current identity-bearing protocol, with a finite, cancellable probe.
pub fn probe_helper(operation: &OperationContext) -> Result<PathBuf, PermissionOutcome> {
    spaceprune_access::helper::probe_helper(
        spaceprune_access::helper::HelperKind::Metadata,
        operation,
    )
}

pub(super) fn authorize_terminal(
    sudo: &Path,
    operation: &OperationContext,
) -> Result<(), PermissionOutcome> {
    let mut command = Command::new(sudo);
    command.arg("-v");
    let status = spaceprune_access::process::terminal_status(command, operation)
        .map_err(|error| super::operation_error(&error))?;
    if status.success() {
        Ok(())
    } else {
        Err(PermissionOutcome::Denied)
    }
}
