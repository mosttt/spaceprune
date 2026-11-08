//! Read metadata through directory descriptors without following links or nested mounts.

use super::{FileIdentity, identity_from_metadata};
use crate::{IoFailure, IoFailureKind, NativePath, OperationContext, error};
use rustix::fs::{AtFlags, CWD, Dir, FileType, Mode, OFlags, fstat, openat};
use spaceprune_contracts::cache::DirectoryBinding;
use spaceprune_contracts::messages;
use std::{
    fs::File,
    io,
    os::fd::{AsRawFd, OwnedFd},
    path::{Component, Path},
};

/// An observed directory binding and its complete or partial logical file size.
#[derive(Debug)]
pub struct DirectoryInspection {
    /// Root and parent pinned before traversal.
    pub binding: DirectoryBinding,
    /// Bytes of ordinary files inspected; links and special files are not followed.
    pub observed_bytes: u64,
    /// A representative failure, prioritizing an actual access denial over other gaps.
    pub failure: Option<IoFailure>,
}

fn identity(fd: &OwnedFd) -> io::Result<FileIdentity> {
    identity_from_metadata(&File::from(fd.try_clone()?).metadata()?)
}

fn mount(fd: &OwnedFd) -> io::Result<u64> {
    if let Ok(stat) = rustix::fs::statx(
        fd,
        "",
        AtFlags::EMPTY_PATH | AtFlags::SYMLINK_NOFOLLOW,
        rustix::fs::StatxFlags::MNT_ID,
    ) && stat.stx_mask & rustix::fs::StatxFlags::MNT_ID.bits() != 0
    {
        return Ok(stat.stx_mnt_id);
    }
    // Older kernels expose the same mount identity through the held descriptor.

    std::fs::read_to_string(format!("/proc/self/fdinfo/{}", fd.as_raw_fd()))?
        .lines()
        .find_map(|line| line.strip_prefix("mnt_id:")?.trim().parse().ok())
        .ok_or_else(|| io::Error::other(messages::text("native.mount_identity")))
}

fn open_parent(path: &Path, context: &OperationContext) -> io::Result<OwnedFd> {
    if !path.is_absolute() {
        return Err(boundary());
    }
    let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut fd = openat(CWD, "/", flags, Mode::empty())?;
    for component in path.components() {
        context.check()?;
        match component {
            Component::RootDir => {}
            Component::Normal(name) => fd = openat(&fd, name, flags, Mode::empty())?,
            _ => return Err(boundary()),
        }
    }
    Ok(fd)
}

fn readable(fd: &OwnedFd) -> io::Result<Dir> {
    openat(
        fd,
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .and_then(Dir::new)
    .map_err(Into::into)
}

fn boundary() -> io::Error {
    error::failure(
        IoFailureKind::BoundaryViolation,
        messages::text("native.inspection_boundary"),
    )
}

fn changed() -> io::Error {
    error::failure(
        IoFailureKind::TargetChanged,
        messages::text("native.inspection_changed"),
    )
}

/// Read one entry's metadata through held parent descriptors without following any symlink.
/// This does not read file contents or open special devices for I/O.
pub fn inspect_entry_metadata(
    path: &Path,
    context: &OperationContext,
) -> io::Result<std::fs::Metadata> {
    context.check()?;
    let parent_path = path
        .parent()
        .filter(|_| path.file_name().is_some())
        .ok_or_else(boundary)?;
    let parent = open_parent(parent_path, context)?;
    let entry = openat(
        &parent,
        path.file_name().unwrap(),
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let metadata = File::from(entry).metadata()?;
    context.check()?;
    Ok(metadata)
}

/// Inspect one native directory, preserving its identity and original operation lifetime.
/// Ancestors require search access only. Enumeration and statistics stay relative to held descriptors.
pub fn inspect_directory(
    path: &Path,
    expected: Option<&DirectoryBinding>,
    context: &OperationContext,
) -> io::Result<DirectoryInspection> {
    context.check()?;
    let parent_path = path
        .parent()
        .filter(|_| path.file_name().is_some())
        .ok_or_else(boundary)?;
    let parent = open_parent(parent_path, context)?;
    let root = openat(
        &parent,
        path.file_name().unwrap(),
        OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut result = DirectoryInspection {
        binding: DirectoryBinding {
            path: NativePath::from_path(path),
            parent_identity: identity(&parent)?,
            identity: identity(&root)?,
        },
        observed_bytes: 0,
        failure: None,
    };
    if expected.is_some_and(|expected| *expected != result.binding) {
        return Err(changed());
    }
    let root_mount = mount(&root)?;
    let mut directories = Vec::new();
    match readable(&root) {
        Ok(directory) => directories.push(directory),
        Err(error) => record(&mut result, error),
    }
    while let Some(directory) = directories.last_mut() {
        context.check()?;
        let Some(entry) = directory.read() else {
            directories.pop();
            continue;
        };
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                record(&mut result, error.into());
                continue;
            }
        };
        if entry.file_name().to_bytes() == b"." || entry.file_name().to_bytes() == b".." {
            continue;
        }
        let inspected = (|| -> io::Result<Option<Dir>> {
            let fd = openat(
                directory.fd()?,
                entry.file_name(),
                OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?;
            let metadata = fstat(&fd)?;
            let kind = FileType::from_raw_mode(metadata.st_mode);
            if !matches!(kind, FileType::RegularFile | FileType::Directory) {
                return Ok(None);
            }
            if mount(&fd)? != root_mount {
                return Err(boundary());
            }
            if kind == FileType::Directory {
                return readable(&fd).map(Some);
            }
            result.observed_bytes = result
                .observed_bytes
                .saturating_add(u64::try_from(metadata.st_size).unwrap_or(0));
            Ok(None)
        })();
        match inspected {
            Ok(Some(child)) => directories.push(child),
            Ok(None) => {}
            Err(error) => record(&mut result, error),
        }
    }
    context.check()?;
    let current_parent = open_parent(parent_path, context)?;
    let current = openat(
        &current_parent,
        path.file_name().unwrap(),
        OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    if identity(&current_parent)? != result.binding.parent_identity
        || identity(&current)? != result.binding.identity
    {
        return Err(changed());
    }
    Ok(result)
}

fn record(result: &mut DirectoryInspection, error: io::Error) {
    let observed = error::classify(&error);
    if result.failure.as_ref().is_none_or(|previous| {
        previous.kind != IoFailureKind::AccessDenied && observed.kind == IoFailureKind::AccessDenied
    }) {
        result.failure = Some(observed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::read_identity;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };

    #[test]
    fn inspection_preserves_path_identities_and_only_needs_search_access_to_ancestors() {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().join("search-only");
        let root = parent.join("cache");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("data"), b"abc").unwrap();
        let expected = DirectoryBinding {
            path: NativePath::from_path(&root),
            parent_identity: read_identity(&parent).unwrap(),
            identity: read_identity(&root).unwrap(),
        };
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o111)).unwrap();
        let result = inspect_directory(&root, Some(&expected), &OperationContext::default());
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        let result = result.unwrap();
        assert_eq!(result.binding, expected);
        assert_eq!(result.observed_bytes, 3);
        assert!(result.failure.is_none());
    }

    #[test]
    fn inspection_does_not_follow_links_and_rejects_replaced_or_cancelled_sources() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("data"), b"abc").unwrap();
        let outside = temporary.path().join("outside");
        fs::File::create(&outside)
            .unwrap()
            .set_len(1 << 30)
            .unwrap();
        symlink(&outside, root.join("link")).unwrap();
        let result = inspect_directory(&root, None, &OperationContext::default()).unwrap();
        assert_eq!(result.observed_bytes, 3);
        assert!(result.failure.is_none());
        fs::rename(&root, temporary.path().join("original")).unwrap();
        fs::create_dir(&root).unwrap();
        let error = inspect_directory(&root, Some(&result.binding), &OperationContext::default())
            .unwrap_err();
        assert_eq!(error::classify(&error).kind, IoFailureKind::TargetChanged);
        let context = OperationContext::default();
        context.cancel();
        assert_eq!(
            inspect_directory(&root, None, &context).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(fs::metadata(outside).unwrap().len(), 1 << 30);
    }

    #[test]
    #[ignore = "Requires an isolated Linux mount namespace with CAP_SYS_ADMIN"]
    fn inspection_rejects_same_filesystem_bind_mounts() {
        use std::{os::unix::fs::MetadataExt, process::Command};
        struct Mounted(std::path::PathBuf);
        impl Drop for Mounted {
            fn drop(&mut self) {
                let _ = Command::new("umount").arg(&self.0).status();
            }
        }

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        let nested = root.join("nested");
        let outside = temporary.path().join("outside");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(root.join("data"), b"abc").unwrap();
        fs::write(outside.join("keep"), b"outside cache").unwrap();
        assert!(
            Command::new("mount")
                .arg("--bind")
                .arg(&outside)
                .arg(&nested)
                .status()
                .unwrap()
                .success()
        );
        let _mounted = Mounted(nested.clone());
        assert_eq!(
            fs::metadata(&root).unwrap().dev(),
            fs::metadata(&nested).unwrap().dev()
        );
        let result = inspect_directory(&root, None, &OperationContext::default()).unwrap();
        assert_eq!(result.observed_bytes, 3);
        assert_eq!(
            result.failure.unwrap().kind,
            IoFailureKind::BoundaryViolation
        );
        assert_eq!(fs::read(outside.join("keep")).unwrap(), b"outside cache");
    }
}
