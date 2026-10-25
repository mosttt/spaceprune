//! Signed, administrator-owned metadata components shared by Windows hosts.

use spaceprune_access::windows_package::Component;
use spaceprune_access::{OperationContext, PermissionOutcome};
use spaceprune_windows_elevation::{TrustedExecutable, trusted_installed_executable};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn names() -> [&'static str; 2] {
    let target = if cfg!(all(target_arch = "x86_64", target_env = "msvc")) {
        "spaceprune-readonly-helper-x86_64-pc-windows-msvc.exe"
    } else if cfg!(all(target_arch = "x86_64", target_env = "gnu")) {
        "spaceprune-readonly-helper-x86_64-pc-windows-gnu.exe"
    } else if cfg!(all(target_arch = "aarch64", target_env = "msvc")) {
        "spaceprune-readonly-helper-aarch64-pc-windows-msvc.exe"
    } else {
        "spaceprune-readonly-helper.exe"
    };
    [Component::Metadata.filename(), target]
}

fn valid_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| names().iter().any(|name| value.eq_ignore_ascii_case(name)))
}

/// Check installed roles and session support without requesting UAC.
pub fn probe_helper(operation: &OperationContext) -> Result<PathBuf, PermissionOutcome> {
    trusted_helper(operation).map(|image| image.path().to_owned())
}

pub(crate) fn prepare_helper(
    operation: &OperationContext,
) -> Result<TrustedExecutable, PermissionOutcome> {
    if let Some(image) =
        spaceprune_access::windows_native::ensure_helper(Component::Metadata, operation)?
    {
        return Ok(image);
    }
    trusted_helper(operation)
}

/// Keep the verified image and its ancestors held through the subsequent UAC launch.
pub(crate) fn trusted_helper(
    operation: &OperationContext,
) -> Result<TrustedExecutable, PermissionOutcome> {
    super::check(operation)?;
    if let Some(image) =
        spaceprune_access::windows_native::pinned_helper(Component::Metadata, operation)?
    {
        return Ok(image);
    }
    let executable =
        std::env::current_exe().map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
    let directory = executable
        .parent()
        .ok_or(PermissionOutcome::HelperMissing)?;
    for name in names() {
        let path = directory.join(name);
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(PermissionOutcome::HelperUntrusted),
            Ok(_) => {}
        }
        let image =
            trusted_installed_executable(&path).ok_or(PermissionOutcome::HelperUntrusted)?;
        let mut command = Command::new(image.path());
        command
            .arg(crate::mailbox::support_argument())
            .current_dir(directory);
        let output = spaceprune_access::process::capture(
            command,
            &operation.child(std::time::Duration::from_secs(5)),
        )
        .map_err(|error| {
            if error.raw_os_error() == Some(740) {
                PermissionOutcome::HelperOutdated
            } else {
                super::operation_error(&error)
            }
        })?;
        return if output.status.success() {
            Ok(image)
        } else {
            Err(PermissionOutcome::HelperOutdated)
        };
    }
    Err(PermissionOutcome::HelperMissing)
}

pub(crate) fn current_helper() -> Result<TrustedExecutable, crate::HelperError> {
    let path = std::env::current_exe().map_err(|_| crate::HelperError)?;
    if !valid_name(&path) {
        return Err(crate::HelperError);
    }
    trusted_installed_executable(&path).ok_or(crate::HelperError)
}
