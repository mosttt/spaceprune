//! Shared atomic capture and restoration for native deletion and synchronous domain commands.

use super::*;
use spaceprune_contracts::messages;

/// Descendants are captured alongside the selected root, not inside previously opened subtree
/// directories. Ordinary operations through those original parent descriptors no longer resolve
/// captured names. Private staging is not isolation from an attacker with the same UID.
pub(super) struct Staging {
    pub(super) fd: OwnedFd,
    path: std::path::PathBuf,
    origins: origins::Origins,
}

impl Staging {
    pub(super) fn descendant(
        &self,
        parent: &OwnedFd,
        name: &OsStr,
        source: &Path,
        context: &OperationContext,
        action: impl FnOnce(&OsStr) -> io::Result<()>,
    ) -> io::Result<bool> {
        let captured_name = format!("entry-{}", uuid::Uuid::new_v4());
        context.check()?;
        let checkpoint = self.origins.record(&captured_name, source)?;
        match capture(parent, name, &self.fd, OsStr::new(&captured_name)) {
            Ok(()) => {}
            Err(error) => {
                self.origins.forget_after(checkpoint)?;
                return if error.kind() == io::ErrorKind::NotFound {
                    Ok(false)
                } else {
                    Err(error)
                };
            }
        }
        // DirectorySubtree authorizes runtime descendants, not a frozen list of file identities.
        // Rename is the membership boundary: inspect and delete only the atomically captured entry.
        let execution = action(OsStr::new(&captured_name));
        let retained = self.restore(OsStr::new(&captured_name), parent, name)?;
        if execution
            .as_ref()
            .err()
            .is_none_or(|error| !crate::error::has_effects(error))
        {
            self.origins.forget_after(checkpoint)?;
        }
        execution.map(|()| retained)
    }

    fn location(&self) -> String {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let current = {
            use std::os::fd::AsRawFd;
            std::fs::read_link(format!("/proc/self/fd/{}", self.fd.as_raw_fd()))
        };
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        let current = rustix::fs::getpath(&self.fd)
            .map(|path| std::path::PathBuf::from(OsStr::from_bytes(path.to_bytes())))
            .map_err(io::Error::from);
        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        )))]
        let current: io::Result<std::path::PathBuf> =
            Err(io::Error::from(io::ErrorKind::Unsupported));
        match current {
            Ok(path) => messages::message(
                "native.retained_location",
                &[("path", path.display().to_string())],
            ),
            Err(error) => messages::message(
                "native.retained_location_unknown",
                &[
                    ("path", self.path.display().to_string()),
                    ("error", error.to_string()),
                ],
            ),
        }
    }

    fn restore(&self, captured_name: &OsStr, parent: &OwnedFd, name: &OsStr) -> io::Result<bool> {
        let restoration = match statat(&self.fd, captured_name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => move_exclusive(&self.fd, captured_name, parent, name).map(|()| true),
            Err(rustix::io::Errno::NOENT) => Ok(false),
            Err(error) => Err(error.into()),
        };
        restoration.map_err(|error| {
            crate::error::failure_after_effects(
                IoFailureKind::TargetChanged,
                messages::message(
                    "native.surviving_contents",
                    &[("location", self.location()), ("error", error.to_string())],
                ),
                &error,
            )
        })
    }
}

fn capture(parent: &OwnedFd, name: &OsStr, staging: &OwnedFd, target: &OsStr) -> io::Result<()> {
    move_exclusive(parent, name, staging, target).map_err(|error| {
        if [rustix::io::Errno::BUSY, rustix::io::Errno::XDEV]
            .iter()
            .any(|code| Some(code.raw_os_error()) == error.raw_os_error())
        {
            crate::error::failure_with_cause(
                IoFailureKind::BoundaryViolation,
                messages::text("native.capture_mount"),
                &error,
            )
        } else {
            error
        }
    })
}

fn validate_private_staging(parent: &OwnedFd, name: &str, staging: &OwnedFd) -> io::Result<()> {
    let metadata = fstat(staging)?;
    if metadata.st_uid == rustix::process::geteuid().as_raw() && metadata.st_mode & 0o077 == 0 {
        return Ok(());
    }
    // Some mounts synthesize permissions and ignore mkdir's 0700 request. Reject before
    // capture, and only remove our still-matching empty directory, never a replacement.
    if statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
        .is_ok_and(|current| current.st_dev == metadata.st_dev && current.st_ino == metadata.st_ino)
    {
        let _ = unlinkat(parent, name, AtFlags::REMOVEDIR);
    }
    Err(failure(
        IoFailureKind::Unsupported,
        messages::text("native.private_staging_unavailable"),
    ))
}

pub(super) fn entry<T>(
    parent: &OwnedFd,
    name: &OsStr,
    identity: &FileIdentity,
    original_path: &Path,
    context: &OperationContext,
    action: impl FnOnce(&Staging, &OsStr) -> io::Result<T>,
) -> io::Result<Option<T>> {
    context.check()?;
    let observed = match statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !matches(identity, &observed) || !matches_birth(parent, name, identity, false)? {
        return Err(changed());
    }
    if observed.st_dev != fstat(parent)?.st_dev {
        return Err(boundary());
    }
    let staging_name = format!(".spaceprune-execute-{}", uuid::Uuid::new_v4());
    context.check()?;
    mkdirat(parent, staging_name.as_str(), Mode::RWXU)?;
    let staging = match directory(parent, OsStr::new(&staging_name)) {
        Ok(staging) => staging,
        Err(error) => {
            let _ = unlinkat(parent, staging_name.as_str(), AtFlags::REMOVEDIR);
            return Err(error);
        }
    };
    validate_private_staging(parent, &staging_name, &staging)?;
    let origins = match origins::Origins::create(&staging) {
        Ok(origins) => origins,
        Err(error) => {
            let _ = unlinkat(parent, staging_name.as_str(), AtFlags::REMOVEDIR);
            return Err(error);
        }
    };
    let staging = Staging {
        origins,
        fd: staging,
        path: original_path
            .parent()
            .ok_or_else(boundary)?
            .join(&staging_name),
    };
    let captured_name = OsStr::new("selected");
    let capture = context
        .check()
        .and_then(|()| staging.origins.record("selected", original_path))
        .and_then(|_| capture(parent, name, &staging.fd, captured_name));
    if let Err(error) = capture {
        let _ = unlinkat(&staging.fd, origins::NAME, AtFlags::empty());
        let _ = unlinkat(parent, staging_name.as_str(), AtFlags::REMOVEDIR);
        if error.kind() == io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(error);
    }
    // A competing replacement may win before rename. Check the captured inode, never the old
    // pathname, before allowing a filesystem walk or a domain command to touch any contents.
    let execution = (|| {
        if !matches(
            identity,
            &statat(&staging.fd, captured_name, AtFlags::SYMLINK_NOFOLLOW)?,
        ) || !matches_birth(&staging.fd, captured_name, identity, false)?
        {
            return Err(changed());
        }
        context.check()?;
        action(&staging, captured_name)
    })();
    staging.restore(captured_name, parent, name)?;
    if origins::has_captured_entries(&staging.fd)? {
        return match execution {
            Err(error) if crate::error::has_effects(&error) => Err(error),
            _ => Err(crate::error::failure_after_effects(
                IoFailureKind::TargetChanged,
                messages::message(
                    "native.execution_contents",
                    &[("location", staging.location())],
                ),
                &io::Error::from(io::ErrorKind::DirectoryNotEmpty),
            )),
        };
    }
    unlinkat(&staging.fd, origins::NAME, AtFlags::empty())?;
    unlinkat(parent, staging_name.as_str(), AtFlags::REMOVEDIR)?;
    execution.map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn nonprivate_staging_is_unsupported_and_its_empty_directory_is_removed() {
        let temporary = tempfile::tempdir().unwrap();
        let selected = temporary.path().join("selected");
        fs::write(&selected, b"keep selected content").unwrap();
        let previous = temporary.path().join(".spaceprune-execute-previous");
        fs::create_dir(&previous).unwrap();
        let parent = open_root(temporary.path()).unwrap();
        let name = ".spaceprune-execute-current";
        mkdirat(&parent, name, Mode::RWXU).unwrap();
        let path = temporary.path().join(name);
        // Model the observed NTFS mount's synthesized 0755 mode without mounting a filesystem.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let staging = directory(&parent, OsStr::new(name)).unwrap();
        let error = validate_private_staging(&parent, name, &staging).unwrap_err();
        assert_eq!(
            crate::error::classify(&error).kind,
            IoFailureKind::Unsupported
        );
        assert!(!crate::error::has_effects(&error));
        assert!(
            error
                .to_string()
                .contains("owner-only cleanup workspace (0700)")
        );
        assert!(!path.exists());
        assert!(previous.is_dir());
        assert_eq!(fs::read(selected).unwrap(), b"keep selected content");
    }

    #[test]
    fn failed_staging_cleanup_preserves_replacements_and_nonempty_directories() {
        for replacement in [false, true] {
            let temporary = tempfile::tempdir().unwrap();
            let parent = open_root(temporary.path()).unwrap();
            let name = ".spaceprune-execute-current";
            mkdirat(&parent, name, Mode::RWXU).unwrap();
            let path = temporary.path().join(name);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            let staging = directory(&parent, OsStr::new(name)).unwrap();
            let retained = temporary.path().join("retained");
            if replacement {
                fs::rename(&path, &retained).unwrap();
                // An empty replacement must survive even though rmdir could remove it.
                fs::create_dir(&path).unwrap();
            } else {
                fs::write(path.join("keep"), b"concurrent content").unwrap();
            }
            let error = validate_private_staging(&parent, name, &staging).unwrap_err();
            assert_eq!(
                crate::error::classify(&error).kind,
                IoFailureKind::Unsupported
            );
            assert!(path.is_dir());
            if replacement {
                assert!(retained.is_dir());
            } else {
                assert_eq!(fs::read(path.join("keep")).unwrap(), b"concurrent content");
            }
        }
    }
}
