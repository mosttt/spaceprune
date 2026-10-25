//! Descriptor-relative traversal and atomic capture before checking the selected identity.

use spaceprune_contracts::messages;
#[cfg(target_os = "linux")]
mod ancestry;
#[cfg(target_os = "macos")]
#[path = "unix/ancestry_macos.rs"]
mod ancestry;
#[cfg(any(target_os = "macos", test))]
mod ancestry_generation;
mod capture;
mod origins;

use super::{FileIdentity, Removal};
use crate::{IoFailureKind, OperationContext, error::failure};
use rustix::fs::{
    AtFlags, CWD, Dir, FileType, Mode, OFlags, Stat, fstat, mkdirat, openat, statat, unlinkat,
};
use std::{
    ffi::OsStr,
    io,
    os::{fd::OwnedFd, unix::ffi::OsStrExt},
    path::{Component, Path},
};

fn changed() -> io::Error {
    failure(
        IoFailureKind::TargetChanged,
        messages::text("native.selected_changed"),
    )
}
fn boundary() -> io::Error {
    failure(
        IoFailureKind::BoundaryViolation,
        messages::text("native.cleanup_boundary"),
    )
}
// Native dev_t/ino_t widths differ across Unix targets.
#[allow(clippy::unnecessary_cast)]
fn matches(identity: &FileIdentity, metadata: &Stat) -> bool {
    identity.device == metadata.st_dev as u64 && identity.inode == metadata.st_ino as u64
}

fn matches_birth(
    parent: &OwnedFd,
    name: &OsStr,
    expected: &FileIdentity,
    empty: bool,
) -> io::Result<bool> {
    let Some(expected) = expected.birth else {
        return Ok(true);
    };
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let flags = if empty {
            AtFlags::EMPTY_PATH
        } else {
            AtFlags::SYMLINK_NOFOLLOW
        };
        let metadata = rustix::fs::statx(parent, name, flags, rustix::fs::StatxFlags::BTIME)?;
        if metadata.stx_mask & rustix::fs::StatxFlags::BTIME.bits() == 0 {
            return Ok(false);
        }
        let observed = u64::try_from(metadata.stx_btime.tv_sec)
            .ok()
            .and_then(|seconds| seconds.checked_mul(1_000_000_000))
            .and_then(|nanoseconds| nanoseconds.checked_add(metadata.stx_btime.tv_nsec.into()));
        Ok(observed == Some(expected))
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        let metadata = if empty {
            fstat(parent)?
        } else {
            statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)?
        };
        let observed = u64::try_from(metadata.st_birthtime)
            .ok()
            .and_then(|seconds| seconds.checked_mul(1_000_000_000))
            .and_then(|nanoseconds| nanoseconds.checked_add(metadata.st_birthtime_nsec as u64));
        Ok(observed == Some(expected))
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    )))]
    {
        let _ = (parent, name, expected, empty);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            messages::text("native.creation_identity"),
        ))
    }
}

fn directory(parent: &OwnedFd, name: &OsStr) -> io::Result<OwnedFd> {
    openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| match error {
        rustix::io::Errno::NOTDIR | rustix::io::Errno::LOOP => crate::error::failure_with_cause(
            IoFailureKind::BoundaryViolation,
            messages::text("native.directory_boundary_changed"),
            &error.into(),
        ),
        error => error.into(),
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn validate_ancestry(
    chain: &[OwnedFd],
    names: &[&OsStr],
    context: &OperationContext,
) -> io::Result<()> {
    for (pair, name) in chain.windows(2).zip(names) {
        context.check()?;
        let current = directory(&pair[0], name).map_err(|_| changed())?;
        let expected = fstat(&pair[1])?;
        let observed = fstat(&current)?;
        if expected.st_dev != observed.st_dev
            || expected.st_ino != observed.st_ino
            || !same_mount(&current, &pair[1])?
        {
            return Err(changed());
        }
    }
    Ok(())
}

fn open_root(path: &Path) -> io::Result<OwnedFd> {
    if !path.is_absolute() {
        return Err(boundary());
    }
    let mut fd = openat(
        CWD,
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    for part in path.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(name) => fd = directory(&fd, name)?,
            _ => return Err(boundary()),
        }
    }
    Ok(fd)
}

// NOREPLACE prevents rollback from overwriting a concurrently created name. Supported native
// kernels provide this operation; an unsupported filesystem fails instead of using stat+rename.
fn move_exclusive(from: &OwnedFd, name: &OsStr, to: &OwnedFd, target: &OsStr) -> io::Result<()> {
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
    {
        rustix::fs::renameat_with(from, name, to, target, rustix::fs::RenameFlags::NOREPLACE)
            .map_err(Into::into)
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    )))]
    {
        let _ = (from, name, to, target);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            messages::text("native.exclusive_rename"),
        ))
    }
}

pub(super) fn remove(
    root: &Path,
    root_identity: &FileIdentity,
    relative: &Path,
    identity: &FileIdentity,
    excluded: &dyn Fn(&Path) -> bool,
    context: &OperationContext,
    result: &mut Removal,
) -> io::Result<()> {
    context.check()?;
    let parent = match open_root(root) {
        Ok(fd) => fd,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            result.already_absent = true;
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    if !matches(root_identity, &fstat(&parent)?)
        || !matches_birth(&parent, OsStr::new(""), root_identity, true)?
    {
        return Err(changed());
    }
    let parts = relative
        .components()
        .map(|part| match part {
            Component::Normal(name) => Ok(name),
            _ => Err(boundary()),
        })
        .collect::<io::Result<Vec<_>>>()?;
    let (name, ancestors) = parts.split_last().ok_or_else(boundary)?;
    let mut chain = vec![parent];
    for part in ancestors {
        let parent = chain.last().expect("verified root");
        let child = match directory(parent, part) {
            Ok(fd) => fd,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                result.already_absent = true;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if !same_mount(&child, parent)? {
            return Err(boundary());
        }
        chain.push(child);
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let guard = ancestry::Guard::watch(&chain, ancestors, context)?;
    let parent = chain.last().expect("verified root");
    if excluded(relative) {
        return Err(boundary());
    }
    let removed = capture::entry(
        parent,
        name,
        identity,
        &root.join(relative),
        context,
        |staging, captured_name| {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            if let Some(guard) = &guard {
                guard.after_capture(context)?;
            }
            remove_captured(
                staging,
                captured_name,
                relative,
                &root.join(relative),
                excluded,
                context,
                result,
            )
        },
    )?;
    if removed.is_none() {
        result.already_absent = true;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(super) fn with_directory<T>(
    path: &Path,
    parent_identity: &FileIdentity,
    identity: &FileIdentity,
    context: &OperationContext,
    action: impl FnOnce(&Path, &Path) -> T,
) -> io::Result<Option<T>> {
    capture_directory(
        path,
        parent_identity,
        identity,
        context,
        |staging, captured_name| {
            use std::os::fd::AsRawFd;
            verify_domain_tree(&staging.fd, captured_name, context, false)?;
            let cwd = std::path::PathBuf::from(format!("/proc/self/fd/{}", staging.fd.as_raw_fd()));
            let selected = Path::new("/proc/self/cwd").join(captured_name);
            Ok(action(&cwd, &selected))
        },
    )
}

#[cfg(target_os = "linux")]
pub(super) fn remove_go_directory(
    path: &Path,
    parent_identity: &FileIdentity,
    identity: &FileIdentity,
    context: &OperationContext,
    result: &mut Removal,
) -> io::Result<()> {
    let removed = capture_directory(path, parent_identity, identity, context, |staging, name| {
        // This is only a permission/artifact preflight. Dynamic per-entry capture below,
        // not this walk, establishes membership at the deletion boundary.
        verify_domain_tree(&staging.fd, name, context, true)?;
        remove_captured(
            staging,
            name,
            Path::new(""),
            path,
            &|relative| super::is_execution_artifact(relative),
            context,
            result,
        )?;
        match statat(&staging.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Err(rustix::io::Errno::NOENT) => Ok(()),
            Ok(_) => Err(io::Error::other(messages::text("linux.go_cache_remains"))),
            Err(error) => Err(error.into()),
        }
    })?;
    result.already_absent = removed.is_none();
    Ok(())
}

#[cfg(target_os = "linux")]
fn capture_directory<T>(
    path: &Path,
    parent_identity: &FileIdentity,
    identity: &FileIdentity,
    context: &OperationContext,
    action: impl FnOnce(&capture::Staging, &OsStr) -> io::Result<T>,
) -> io::Result<Option<T>> {
    context.check()?;
    let parent_path = path.parent().ok_or_else(boundary)?;
    let name = path.file_name().ok_or_else(boundary)?;
    let parent = match open_root(parent_path) {
        Ok(parent) => parent,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !matches(parent_identity, &fstat(&parent)?)
        || !matches_birth(&parent, OsStr::new(""), parent_identity, true)?
    {
        return Err(changed());
    }
    let observed = match statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(observed) => observed,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if FileType::from_raw_mode(observed.st_mode) != FileType::Directory {
        return Err(changed());
    }
    capture::entry(&parent, name, identity, path, context, action)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn mount_id(fd: &OwnedFd) -> io::Result<u64> {
    use std::os::fd::AsRawFd;
    if let Ok(stat) = rustix::fs::statx(fd, "", AtFlags::EMPTY_PATH, rustix::fs::StatxFlags::MNT_ID)
        && stat.stx_mask & rustix::fs::StatxFlags::MNT_ID.bits() != 0
    {
        return Ok(stat.stx_mnt_id);
    }
    fs_mount_id(&std::fs::read_to_string(format!(
        "/proc/self/fdinfo/{}",
        fd.as_raw_fd()
    ))?)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn fs_mount_id(info: &str) -> io::Result<u64> {
    info.lines()
        .find_map(|line| line.strip_prefix("mnt_id:")?.trim().parse().ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                messages::text("native.mount_identity"),
            )
        })
}

fn same_mount(left: &OwnedFd, right: &OwnedFd) -> io::Result<bool> {
    if fstat(left)?.st_dev != fstat(right)?.st_dev {
        return Ok(false);
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        Ok(mount_id(left)? == mount_id(right)?)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        Ok(true)
    }
}

#[cfg(target_os = "linux")]
fn verify_domain_tree(
    parent: &OwnedFd,
    name: &OsStr,
    context: &OperationContext,
    require_writable: bool,
) -> io::Result<()> {
    context.check()?;
    let fd = directory(parent, name)?;
    if mount_id(&fd)? != mount_id(parent)? {
        return Err(boundary());
    }
    let metadata = fstat(&fd)?;
    // Go can restore write permissions on its own read-only module directories. A different
    // owner requires actual write/search access; check before starting a potentially partial clean.
    if require_writable
        || (!rustix::process::geteuid().is_root()
            && metadata.st_uid != rustix::process::geteuid().as_raw())
    {
        rustix::fs::accessat(
            &fd,
            ".",
            rustix::fs::Access::WRITE_OK | rustix::fs::Access::EXEC_OK,
            AtFlags::EACCESS,
        )?;
    }
    let mut entries = Dir::read_from(&fd)?;
    while let Some(entry) = entries.read() {
        context.check()?;
        let entry = entry?;
        let bytes = entry.file_name().to_bytes();
        if matches!(bytes, b"." | b"..") {
            continue;
        }
        let child = OsStr::from_bytes(bytes);
        if super::is_execution_artifact(Path::new(child)) {
            return Err(boundary());
        }
        let metadata = statat(&fd, child, AtFlags::SYMLINK_NOFOLLOW)?;
        if FileType::from_raw_mode(metadata.st_mode) == FileType::Directory {
            verify_domain_tree(&fd, child, context, require_writable)?;
        }
    }
    Ok(())
}

fn remove_captured(
    staging: &capture::Staging,
    name: &OsStr,
    relative: &Path,
    source: &Path,
    excluded: &dyn Fn(&Path) -> bool,
    context: &OperationContext,
    result: &mut Removal,
) -> io::Result<()> {
    context.check()?;
    let parent = &staging.fd;
    let metadata = match statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(rustix::io::Errno::NOENT) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.st_dev != fstat(parent)?.st_dev {
        return Err(boundary());
    }
    let kind = FileType::from_raw_mode(metadata.st_mode);
    context.check()?;
    if kind != FileType::Directory {
        unlinkat(parent, name, AtFlags::empty())?;
        result.removed_entries += 1;
        if kind == FileType::RegularFile {
            result.reclaimed_estimate = result
                .reclaimed_estimate
                .saturating_add(metadata.st_size.max(0) as u64);
        }
        return Ok(());
    }
    let fd = directory(parent, name)?;
    let opened = fstat(&fd)?;
    if opened.st_dev != metadata.st_dev || opened.st_ino != metadata.st_ino {
        return Err(changed());
    }
    if !same_mount(&fd, parent)? {
        return Err(boundary());
    }
    // Restoring a retained directory can change its readdir offset. Do not process that name
    // again, while still allowing later runtime descendants to enter this traversal.
    let mut retained_names = std::collections::HashSet::new();
    let mut entries = Dir::read_from(&fd)?;
    while let Some(entry) = entries.read() {
        context.check()?;
        let entry = entry?;
        let bytes = entry.file_name().to_bytes();
        if matches!(bytes, b"." | b"..") {
            continue;
        }
        let child = OsStr::from_bytes(bytes);
        let child_relative = relative.join(child);
        if retained_names.contains(child) || excluded(&child_relative) {
            continue;
        }
        context.check()?;
        // Removing an empty runtime descendant is already atomic. It only needs write access
        // to this captured parent, unlike moving a read-only directory across parents.
        if unlinkat(&fd, child, AtFlags::REMOVEDIR).is_ok() {
            result.removed_entries += 1;
            continue;
        }
        let child_source = source.join(child);
        if staging.descendant(&fd, child, &child_source, context, |captured_name| {
            remove_captured(
                staging,
                captured_name,
                &child_relative,
                &child_source,
                excluded,
                context,
                result,
            )
        })? {
            retained_names.insert(child.to_owned());
        }
    }
    let current = statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)?;
    if current.st_dev != opened.st_dev || current.st_ino != opened.st_ino {
        return Err(changed());
    }
    context.check()?;
    match unlinkat(parent, name, AtFlags::REMOVEDIR) {
        Ok(()) => {
            result.removed_entries += 1;
            Ok(())
        }
        Err(rustix::io::Errno::NOTEMPTY) => Ok(()),
        Err(error) => Err(error.into()),
    }
}
