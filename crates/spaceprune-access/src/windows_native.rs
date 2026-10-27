//! Resolve only the package and installed payloads fixed by this host's build.
mod installer;
mod package;
use crate::{
    OperationContext, PermissionOutcome,
    windows_package::{
        Component, INSTALL_ROOT_NAME, PACKAGE_NAME, PackageIdentity,
        verification::{check, digest_matches},
    },
};
pub use installer::run_installer_if_requested;
pub use package::PinnedPackage;
use spaceprune_windows_elevation::{SignedFile, TrustedExecutable};
use std::{
    fs::File,
    io::{Seek, SeekFrom},
    path::Path,
    process::Command,
    time::Duration,
};

/// Prepare a missing or outdated bundled generation once, then revalidate the actual component.
/// Installation itself grants neither a metadata connection nor permission to clean any item.
pub fn ensure_helper(
    component: Component,
    operation: &OperationContext,
) -> Result<Option<TrustedExecutable>, PermissionOutcome> {
    static INSTALLING: std::sync::OnceLock<crate::windows_package::preparation::Preparation> =
        std::sync::OnceLock::new();
    crate::windows_package::preparation::ensure(
        INSTALLING.get_or_init(Default::default),
        operation,
        || pinned_helper(component, operation),
        || installer::install(operation),
    )
}

/// The build pin is immutable; adjacent manifests and runtime environment variables cannot replace it.
pub fn embedded_identity() -> Result<Option<PackageIdentity>, PermissionOutcome> {
    let target = if cfg!(target_arch = "aarch64") {
        "aarch64-pc-windows-msvc"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64-pc-windows-msvc"
    } else {
        return Err(PermissionOutcome::Unsupported);
    };
    PackageIdentity::from_embedded(
        include_bytes!(concat!(env!("OUT_DIR"), "/windows-package-identity.json")),
        target,
    )
}

/// Inspect the exact installed generation without prompting or accepting a neighboring version.
/// `None` means this is an unbundled development host and its existing discovery policy still applies.
pub fn pinned_helper(
    component: Component,
    operation: &OperationContext,
) -> Result<Option<TrustedExecutable>, PermissionOutcome> {
    check(operation)?;
    let Some(identity) = embedded_identity()? else {
        return Ok(None);
    };
    let root = spaceprune_windows_elevation::native_program_files_directory()
        .ok_or(PermissionOutcome::AuthorizationUnavailable)?;
    let path = root
        .join(INSTALL_ROOT_NAME)
        .join(identity.payload.generation_directory()?)
        .join(component.filename());
    require_file(&path)?;
    let image = spaceprune_windows_elevation::trusted_installed_executable(&path)
        .ok_or(PermissionOutcome::HelperUntrusted)?;
    verify_file(image.file(), identity.payload.digest(component), operation)?;
    let mut command = Command::new(image.path());
    command
        .arg(component.support_argument())
        .current_dir(path.parent().ok_or(PermissionOutcome::HelperMissing)?);
    let result = crate::process::capture(command, &operation.child(Duration::from_secs(5)))
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::Interrupted => PermissionOutcome::Cancelled,
            std::io::ErrorKind::TimedOut => PermissionOutcome::TimedOut,
            _ => PermissionOutcome::HelperOutdated,
        })?;
    if !result.status.success() {
        return Err(PermissionOutcome::HelperOutdated);
    }
    Ok(Some(image))
}

/// Validate the embedded or fixed adjacent MSI as signed data before installation.
pub fn pinned_package(operation: &OperationContext) -> Result<PinnedPackage, PermissionOutcome> {
    package::open(operation)
}

fn require_file(path: &Path) -> Result<(), PermissionOutcome> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(PermissionOutcome::HelperMissing)
        }
        _ => Err(PermissionOutcome::HelperUntrusted),
    }
}

fn verify_file(
    mut file: &File,
    expected: &str,
    operation: &OperationContext,
) -> Result<(), PermissionOutcome> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    digest_matches(file, expected, operation)
}
