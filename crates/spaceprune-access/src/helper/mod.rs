//! Core-owned components shared by CLI and desktop hosts, with separate native capabilities.

mod ensure;
mod install;
mod prompt;
mod staging;
mod worker;
pub use ensure::{
    ensure_cleanup_helper, ensure_cleanup_helper_with_handler_requirement, ensure_helper,
    ensure_helper_with_handler_requirement, ensure_helper_with_prompt, start_metadata_session,
    start_metadata_session_with_handler_requirement, start_metadata_session_with_prompt,
};
pub use install::{InstallError, can_install, install};
pub use worker::run_installer_from_environment;

use crate::{
    OperationContext, PermissionOutcome,
    process::{capture, trusted_executable},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// Compatibility-specific installation owned by Core, never by an individual host package.
pub const INSTALL_DIRECTORY: &str = "/usr/libexec/spaceprune/native-6-4";
/// Shared installer coordination; ancestor and file ownership are checked before locking.
pub const INSTALL_ROOT: &str = "/usr/libexec/spaceprune";
/// Fixed system policy file for this compatibility set.
pub const POLICY_PATH: &str = "/usr/share/polkit-1/actions/com.spaceprune.native-6-4.policy";
/// Bundled installer; it accepts only this fixed component set.
pub const INSTALLER_NAME: &str = "spaceprune-helper-installer";
/// Probe the narrowly scoped installer without requesting elevation.
pub const INSTALLER_SUPPORT: &str = "--supports-native-install-v2";
/// Handler contract shared by the host and both Linux capabilities.
/// This includes builtin handler IDs and per-operation display language.
pub const HANDLERS_SUPPORT: &str = "--supports-builtin-handlers-v1";
/// Additional handler vocabulary required only by developer-tools requests and plans.
pub const DEVELOPER_HANDLERS_SUPPORT: &str = "--supports-builtin-handlers-v2";
/// Probe language-specific native prompt entrypoints without authenticating.
pub const PROMPT_SUPPORT: &str = "--supports-native-prompt-v1";
const INSTALL_ARGUMENT: &str = "--install-native-components";
const INSTALL_METADATA_ARGUMENT: &str = "--install-and-start-metadata";
/// Start only the metadata capability after installation or ordinary native authorization.
pub const METADATA_SESSION_ARGUMENT: &str = "--spaceprune-readonly-session";
const MAX_BINARY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 16 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallRequest {
    operation: crate::OperationTransfer,
    source_directory: crate::NativePath,
    metadata_sha256: String,
    cleanup_sha256: String,
}

/// Capability of one immutable helper instance; metadata cannot dispatch cleanup messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperKind {
    /// Native metadata inspection only.
    Metadata,
    /// Execution of a host-confirmed plan only.
    Cleanup,
}
impl HelperKind {
    /// Fixed basename inside a distribution and its protected installation.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Metadata => "spaceprune-readonly-helper",
            Self::Cleanup => "spaceprune-cleanup-helper",
        }
    }
    /// Capability/protocol probe; it must not display a native prompt.
    pub const fn support_argument(self) -> &'static str {
        match self {
            Self::Metadata => "--supports-scope-v6",
            Self::Cleanup => "--supports-cleanup-v4",
        }
    }
    /// Fixed root-owned installation location.
    pub fn path(self) -> PathBuf {
        Path::new(INSTALL_DIRECTORY).join(self.name())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    owner: String,
    version: String,
    metadata_sha256: String,
    cleanup_sha256: String,
}
impl Manifest {
    fn digest(&self, kind: HelperKind) -> &str {
        match kind {
            HelperKind::Metadata => &self.metadata_sha256,
            HelperKind::Cleanup => &self.cleanup_sha256,
        }
    }
}

/// Find a bundled executable without accepting a caller-selected name or helper symlink.
pub fn packaged_at(host: &Path, name: &str) -> Option<PathBuf> {
    if ![
        HelperKind::Metadata.name(),
        HelperKind::Cleanup.name(),
        INSTALLER_NAME,
    ]
    .contains(&name)
    {
        return None;
    }
    let path = fs::canonicalize(host).ok()?.parent()?.join(name);
    let metadata = fs::symlink_metadata(&path).ok()?;
    (metadata.is_file()
        && !metadata.file_type().is_symlink()
        && metadata.permissions().mode() & 0o111 != 0)
        .then_some(path)
}

fn operation_error(error: io::Error) -> PermissionOutcome {
    match error.kind() {
        io::ErrorKind::Interrupted => PermissionOutcome::Cancelled,
        io::ErrorKind::TimedOut => PermissionOutcome::TimedOut,
        _ => PermissionOutcome::AuthorizationUnavailable,
    }
}

fn digest_file(path: &Path, context: &OperationContext) -> io::Result<String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > MAX_BINARY_BYTES {
        return Err(io::Error::other("Invalid native component"));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut total = 0u64;
    loop {
        context.check()?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_BINARY_BYTES {
            return Err(io::Error::other("Native component exceeds size limit"));
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests;

/// Probe a packaged or already trusted executable without authorizing any filesystem work.
pub fn probe_protocol(
    path: &Path,
    argument: &str,
    operation: &OperationContext,
) -> Result<(), PermissionOutcome> {
    let context = operation.child(Duration::from_secs(2));
    let mut command = Command::new(path);
    command.arg(argument);
    let result = capture(command, &context).map_err(operation_error)?;
    if result.status.success() {
        Ok(())
    } else {
        Err(PermissionOutcome::HelperOutdated)
    }
}

/// Verify both transport and handler compatibility before authorization or installation.
pub fn probe_component(
    path: &Path,
    kind: HelperKind,
    operation: &OperationContext,
) -> Result<(), PermissionOutcome> {
    probe_protocol(path, kind.support_argument(), operation)?;
    probe_protocol(path, HANDLERS_SUPPORT, operation)?;
    probe_protocol(path, PROMPT_SUPPORT, operation)?;
    Ok(())
}

/// Verify the protected installation and committed artifact identity before executing it.
pub fn probe_helper(
    kind: HelperKind,
    operation: &OperationContext,
) -> Result<PathBuf, PermissionOutcome> {
    operation.check().map_err(operation_error)?;
    let path = kind.path();
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(missing_helper_outcome(Path::new(INSTALL_ROOT), kind));
        }
        Ok(metadata) if !metadata.file_type().is_symlink() => {}
        _ => return Err(PermissionOutcome::HelperUntrusted),
    }
    if !install::destination_is_safe(&path) {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    let path = trusted_executable(&path).ok_or(PermissionOutcome::HelperUntrusted)?;
    let manifest = Path::new(INSTALL_DIRECTORY).join("manifest.json");
    let metadata =
        fs::symlink_metadata(&manifest).map_err(|_| PermissionOutcome::HelperOutdated)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != 0
        || metadata.permissions().mode() & 0o022 != 0
        || metadata.len() > 4096
    {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    let manifest: Manifest = serde_json::from_slice(
        &fs::read(manifest).map_err(|_| PermissionOutcome::HelperUntrusted)?,
    )
    .map_err(|_| PermissionOutcome::HelperOutdated)?;
    if manifest.schema != 1 || manifest.owner != "spaceprune-core" {
        return Err(PermissionOutcome::HelperOutdated);
    }
    if digest_file(&path, operation).map_err(operation_error)? != manifest.digest(kind) {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    probe_component(&path, kind, operation)?;
    prompt::probe_policy(Path::new(POLICY_PATH))?;
    Ok(path)
}

// Historical paths are upgrade hints only; never execute them or grant them trust.
fn missing_helper_outcome(root: &Path, kind: HelperKind) -> PermissionOutcome {
    let previous = [
        root.join(kind.name()),
        root.join("native-5-2").join(kind.name()),
        root.join("native-5-3").join(kind.name()),
        root.join("native-6-3").join(kind.name()),
    ];
    if previous
        .iter()
        .any(|path| fs::symlink_metadata(path).is_ok())
    {
        PermissionOutcome::HelperOutdated
    } else {
        PermissionOutcome::HelperMissing
    }
}

/// Verify both capabilities without modifying installation or starting an authenticator.
pub fn probe_installation(operation: &OperationContext) -> Result<(), PermissionOutcome> {
    for kind in [HelperKind::Metadata, HelperKind::Cleanup] {
        probe_helper(kind, operation)?;
    }
    Ok(())
}
