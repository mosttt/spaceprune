//! Native metadata traversal without following links or crossing registered boundaries.

use super::*;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn measure_roots(
    roots: &[PathBuf],
    control_directory: Option<&Path>,
    expires_at: Option<u64>,
    context: Option<&OperationContext>,
) -> Result<AggregateMetadata, HelperError> {
    use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, fstat, openat, statat};
    use std::os::unix::ffi::OsStrExt as _;

    let mut aggregate = AggregateMetadata::default();
    let mut queue = VecDeque::new();
    for root in roots {
        if context.is_some_and(OperationContext::is_stopped)
            || helper_should_stop(control_directory, expires_at)
        {
            return Ok(aggregate);
        }
        if spaceprune_access::filesystem::is_execution_artifact(root) {
            aggregate.skipped_entries = aggregate.skipped_entries.saturating_add(1);
            continue;
        }
        let descriptor = match open_root(root, context) {
            Ok(descriptor) => descriptor,
            Err(error) => {
                record_failure(&mut aggregate, error);
                continue;
            }
        };
        let metadata = match fstat(&descriptor) {
            Ok(metadata) => metadata,
            Err(error) => {
                record_failure(&mut aggregate, error.into());
                continue;
            }
        };
        match FileType::from_raw_mode(metadata.st_mode) {
            FileType::RegularFile => {
                aggregate.accessible_roots = aggregate.accessible_roots.saturating_add(1);
                aggregate.inspected_entries = aggregate.inspected_entries.saturating_add(1);
                aggregate.logical_bytes = aggregate
                    .logical_bytes
                    .saturating_add(u64::try_from(metadata.st_size).unwrap_or(0));
            }
            FileType::Directory => match openat(
                &descriptor,
                ".",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .and_then(Dir::new)
            {
                Ok(directory) => {
                    aggregate.accessible_roots = aggregate.accessible_roots.saturating_add(1);
                    queue.push_back((directory, 0_usize, metadata.st_dev));
                }
                Err(error) => {
                    record_failure(&mut aggregate, error.into());
                }
            },
            _ => aggregate.skipped_entries = aggregate.skipped_entries.saturating_add(1),
        }
    }
    while let Some((mut directory, depth, device)) = queue.pop_front() {
        while let Some(entry) = directory.read() {
            if context.is_some_and(OperationContext::is_stopped)
                || helper_should_stop(control_directory, expires_at)
            {
                return Ok(aggregate);
            }
            if aggregate.inspected_entries >= MAX_METADATA_ENTRIES as u64 {
                aggregate.skipped_entries = aggregate.skipped_entries.saturating_add(1);
                return Ok(aggregate);
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    record_failure(&mut aggregate, error.into());
                    continue;
                }
            };
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            if spaceprune_access::filesystem::is_execution_artifact(Path::new(OsStr::from_bytes(
                name.to_bytes(),
            ))) {
                aggregate.skipped_entries = aggregate.skipped_entries.saturating_add(1);
                continue;
            }
            let directory_fd = match directory.fd() {
                Ok(fd) => fd,
                Err(error) => {
                    record_failure(&mut aggregate, error.into());
                    continue;
                }
            };
            let metadata = match statat(directory_fd, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(metadata) => metadata,
                Err(error) => {
                    record_failure(&mut aggregate, error.into());
                    continue;
                }
            };
            let file_type = FileType::from_raw_mode(metadata.st_mode);
            if file_type.is_symlink() || metadata.st_dev != device {
                aggregate.skipped_entries = aggregate.skipped_entries.saturating_add(1);
                continue;
            }
            aggregate.inspected_entries = aggregate.inspected_entries.saturating_add(1);
            if file_type.is_file() {
                aggregate.logical_bytes = aggregate
                    .logical_bytes
                    .saturating_add(u64::try_from(metadata.st_size).unwrap_or(0));
            } else if file_type.is_dir() && depth < MAX_METADATA_DEPTH {
                match openat(
                    directory_fd,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                    Mode::empty(),
                )
                .and_then(|fd| {
                    if fstat(&fd)?.st_dev != device {
                        return Err(rustix::io::Errno::XDEV);
                    }
                    Dir::new(fd)
                }) {
                    Ok(children) => queue.push_back((children, depth + 1, device)),
                    Err(error) => {
                        record_failure(&mut aggregate, error.into());
                    }
                }
            } else if file_type.is_dir() {
                aggregate.skipped_entries = aggregate.skipped_entries.saturating_add(1);
            }
        }
    }
    Ok(aggregate)
}

#[cfg(target_os = "windows")]
pub(super) fn measure_roots(
    roots: &[PathBuf],
    control_directory: Option<&Path>,
    expires_at: Option<u64>,
    context: Option<&OperationContext>,
) -> Result<AggregateMetadata, HelperError> {
    let aggregate = spaceprune_windows_elevation::measure_readonly_metadata(
        roots,
        MAX_METADATA_ENTRIES as u64,
        MAX_METADATA_DEPTH,
        || {
            context.is_some_and(OperationContext::is_stopped)
                || helper_should_stop(control_directory, expires_at)
        },
    );
    Ok(AggregateMetadata {
        logical_bytes: aggregate.logical_bytes,
        inspected_entries: aggregate.inspected_entries,
        skipped_entries: aggregate.skipped_entries,
        accessible_roots: aggregate.accessible_roots,
    })
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub(super) fn measure_roots(
    _roots: &[PathBuf],
    _control_directory: Option<&Path>,
    _expires_at: Option<u64>,
    _context: Option<&OperationContext>,
) -> Result<AggregateMetadata, HelperError> {
    Err(HelperError)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn record_failure(aggregate: &mut AggregateMetadata, error: std::io::Error) {
    aggregate.skipped_entries = aggregate.skipped_entries.saturating_add(1);
    let failure = spaceprune_access::error::classify(&error);
    let priority = |failure: &spaceprune_access::IoFailure| {
        (
            failure.kind == spaceprune_access::IoFailureKind::AccessDenied,
            failure.os_code == Some(rustix::io::Errno::PERM.raw_os_error()),
        )
    };
    if aggregate
        .failure
        .as_ref()
        .is_none_or(|previous| priority(&failure) > priority(previous))
    {
        aggregate.failure = Some(failure);
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn open_root(
    path: &Path,
    context: Option<&OperationContext>,
) -> std::io::Result<std::os::fd::OwnedFd> {
    use rustix::fs::{CWD, Mode, OFlags, openat};
    #[cfg(target_os = "linux")]
    let metadata = OFlags::PATH;
    #[cfg(target_os = "macos")]
    let metadata = OFlags::from_bits_retain(libc::O_EVTONLY as u32);
    let flags = metadata | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    if !path.is_absolute() {
        return Err(rustix::io::Errno::INVAL.into());
    }
    let mut components = path.components().peekable();
    let mut parent = openat(CWD, "/", flags | OFlags::DIRECTORY, Mode::empty())?;
    while let Some(component) = components.next() {
        if let Some(context) = context {
            context.check()?;
        }
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                let directory = if components.peek().is_some() {
                    OFlags::DIRECTORY
                } else {
                    OFlags::empty()
                };
                parent = openat(&parent, name, flags | directory, Mode::empty())?;
            }
            _ => return Err(rustix::io::Errno::INVAL.into()),
        }
    }
    Ok(parent)
}
