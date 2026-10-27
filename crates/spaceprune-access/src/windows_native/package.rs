//! Retain private extracted package bytes and their signature pin until Installer has exited.
use super::*;
use std::{fs::OpenOptions, io::Write, ops::Deref};

const EMBEDDED_MSI: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/embedded-windows-package.msi"));

/// Verified package data held until installation finishes, including any private staging directory.
pub struct PinnedPackage {
    // Drop the pinned file before removing its staging directory.
    source: SignedFile,
    _staged: Option<tempfile::TempDir>,
}

impl Deref for PinnedPackage {
    type Target = SignedFile;
    fn deref(&self) -> &SignedFile {
        &self.source
    }
}

pub(super) fn open(operation: &OperationContext) -> Result<PinnedPackage, PermissionOutcome> {
    check(operation)?;
    let identity = embedded_identity()?.ok_or(PermissionOutcome::HelperMissing)?;
    let staged = if EMBEDDED_MSI.is_empty() {
        None
    } else {
        Some(stage(EMBEDDED_MSI, &identity.msi_sha256, operation)?)
    };
    let path = match &staged {
        Some(directory) => directory.path().join(PACKAGE_NAME),
        None => std::env::current_exe()
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?
            .parent()
            .ok_or(PermissionOutcome::HelperMissing)?
            .join(PACKAGE_NAME),
    };
    require_file(&path)?;
    let source = spaceprune_windows_elevation::trusted_signed_file(&path)
        .ok_or(PermissionOutcome::HelperUntrusted)?;
    verify_file(source.file(), &identity.msi_sha256, operation)?;
    Ok(PinnedPackage {
        source,
        _staged: staged,
    })
}

fn stage(
    bytes: &[u8],
    digest: &str,
    operation: &OperationContext,
) -> Result<tempfile::TempDir, PermissionOutcome> {
    digest_matches(bytes, digest, operation)?;
    let directory = tempfile::Builder::new()
        .prefix("spaceprune-package-")
        .tempdir()
        .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
    spaceprune_windows_elevation::secure_created_logon_path(
        directory.path(),
        spaceprune_windows_elevation::PrivatePathKind::Directory,
    )
    .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    let path = directory.path().join(PACKAGE_NAME);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
    for chunk in bytes.chunks(64 * 1024) {
        check(operation)?;
        file.write_all(chunk)
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
    }
    file.sync_all()
        .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
    drop(file);
    spaceprune_windows_elevation::secure_created_logon_path(
        &path,
        spaceprune_windows_elevation::PrivatePathKind::File,
    )
    .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    check(operation)?;
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::*;
    const DIGEST: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn embedded_staging_is_exact_private_and_removed_after_use() {
        let operation = OperationContext::default();
        let directory = stage(b"abc", DIGEST, &operation).unwrap();
        let path = directory.path().to_owned();
        assert_eq!(std::fs::read(path.join(PACKAGE_NAME)).unwrap(), b"abc");
        assert!(stage(b"changed", DIGEST, &operation).is_err());
        drop(directory);
        assert!(!path.exists());
        operation.cancel();
        assert!(matches!(
            stage(b"abc", DIGEST, &operation),
            Err(PermissionOutcome::Cancelled)
        ));
    }

    #[test]
    fn embedded_signed_package_opens_without_an_adjacent_msi() {
        if EMBEDDED_MSI.is_empty() {
            return;
        }
        let package = open(&OperationContext::default()).unwrap();
        let path = package.path().to_owned();
        assert_ne!(path.parent(), std::env::current_exe().unwrap().parent());
        assert_eq!(std::fs::read(&path).unwrap(), EMBEDDED_MSI);
        drop(package);
        assert!(!path.exists());
    }
}
