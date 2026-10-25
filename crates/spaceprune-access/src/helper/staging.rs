//! Read unprivileged bundle files as data and publish a complete, verified component set.

use super::*;
use std::{io::Write, os::unix::fs::OpenOptionsExt};

pub(super) fn copy_component(
    source: &Path,
    destination: &Path,
    expected: &str,
    caller: u32,
    check: &mut impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    check()?;
    let mut input = fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(source)?;
    let metadata = input.metadata()?;
    if !metadata.is_file()
        || ![0, caller].contains(&metadata.uid())
        || metadata.permissions().mode() & 0o022 != 0
        || metadata.permissions().mode() & 0o111 == 0
        || metadata.len() > MAX_BINARY_BYTES
    {
        return Err(io::Error::other("Untrusted component source"));
    }
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut total = 0u64;
    loop {
        check()?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_BINARY_BYTES {
            return Err(io::Error::other("Native component exceeds size limit"));
        }
        hash.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
    }
    if format!("{:x}", hash.finalize()) != expected {
        return Err(io::Error::other("Component changed after approval"));
    }
    output.set_permissions(fs::Permissions::from_mode(0o755))?;
    output.sync_all()
}

pub(super) fn stage(
    root: &Path,
    request: &InstallRequest,
    caller: u32,
    check: &mut impl FnMut() -> io::Result<()>,
) -> io::Result<tempfile::TempDir> {
    let source = request
        .source_directory
        .to_path_buf()
        .filter(|path| path.is_absolute())
        .ok_or_else(|| io::Error::other("Invalid bundle directory"))?;
    let staged = tempfile::Builder::new()
        .prefix(".native-stage-")
        .tempdir_in(root)?;
    let manifest = Manifest {
        schema: 1,
        owner: "spaceprune-core".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        metadata_sha256: request.metadata_sha256.clone(),
        cleanup_sha256: request.cleanup_sha256.clone(),
    };
    for kind in [HelperKind::Metadata, HelperKind::Cleanup] {
        copy_component(
            &source.join(kind.name()),
            &staged.path().join(kind.name()),
            manifest.digest(kind),
            caller,
            check,
        )?;
    }
    check()?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(staged.path().join("manifest.json"))?;
    output.write_all(&serde_json::to_vec(&manifest)?)?;
    output.set_permissions(fs::Permissions::from_mode(0o644))?;
    output.sync_all()?;
    fs::set_permissions(staged.path(), fs::Permissions::from_mode(0o755))?;
    fs::File::open(staged.path())?.sync_all()?;
    Ok(staged)
}

pub(super) fn validate_existing(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        _ => return Err(io::Error::other("Invalid installation directory")),
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if ![
            HelperKind::Metadata.name(),
            HelperKind::Cleanup.name(),
            "manifest.json",
        ]
        .iter()
        .any(|name| entry.file_name() == *name)
            || !entry.file_type()?.is_file()
        {
            return Err(io::Error::other("Installation contains unmanaged entries"));
        }
    }
    Ok(true)
}

pub(super) fn publish(
    staged: tempfile::TempDir,
    destination: &Path,
    exists: bool,
) -> io::Result<()> {
    if exists {
        // Readers see one complete directory generation; running processes retain their old inode.
        rustix::fs::renameat_with(
            rustix::fs::CWD,
            staged.path(),
            rustix::fs::CWD,
            destination,
            rustix::fs::RenameFlags::EXCHANGE,
        )?;
    } else {
        fs::rename(staged.path(), destination)?;
    }
    fs::File::open(destination.parent().expect("fixed installation parent"))?.sync_all()?;
    // After exchange the temporary name contains only the validated, Core-owned old components.
    drop(staged);
    Ok(())
}
