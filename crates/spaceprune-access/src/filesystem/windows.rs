//! Handle-bound Windows removal. Ancestors deny delete sharing until child work terminates.

use super::{FileIdentity, Removal};
use crate::{IoFailureKind, OperationContext, error::failure};
use spaceprune_contracts::messages;
use std::{
    fs::File,
    io,
    mem::size_of,
    os::windows::io::AsRawHandle,
    path::{Component, Path},
};
use windows_sys::Win32::Storage::FileSystem::*;

mod handles;
use handles::{lock_ancestors, open_relative};

fn boundary() -> io::Error {
    failure(
        IoFailureKind::BoundaryViolation,
        messages::text("native.reparse_boundary"),
    )
}
fn changed() -> io::Error {
    failure(
        IoFailureKind::TargetChanged,
        messages::text("native.selected_changed"),
    )
}

fn identity(file: &File) -> io::Result<FileIdentity> {
    let mut value = FILE_ID_INFO::default();
    // SAFETY: The writable buffer matches the requested FileIdInfo layout and exact size.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&mut value as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut basic = FILE_BASIC_INFO::default();
    // SAFETY: The buffer matches FileBasicInfo and remains valid for this synchronous query.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileBasicInfo,
            (&mut basic as *mut FILE_BASIC_INFO).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(FileIdentity {
        device: value.VolumeSerialNumber,
        inode: u64::from_le_bytes(
            value.FileId.Identifier[..8]
                .try_into()
                .expect("file ID width"),
        ),
        high: u64::from_le_bytes(
            value.FileId.Identifier[8..]
                .try_into()
                .expect("file ID width"),
        ),
        birth: u64::try_from(basic.CreationTime).ok(),
    })
}

fn attributes(file: &File) -> io::Result<u32> {
    let mut value = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: The buffer matches FileAttributeTagInfo and remains valid for the synchronous call.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileAttributeTagInfo,
            (&mut value as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(value.FileAttributes)
}

fn byte_size(file: &File) -> io::Result<u64> {
    let mut value = FILE_STANDARD_INFO::default();
    // SAFETY: The buffer matches FileStandardInfo and the handle is owned by File.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileStandardInfo,
            (&mut value as *mut FILE_STANDARD_INFO).cast(),
            size_of::<FILE_STANDARD_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    u64::try_from(value.EndOfFile).map_err(io::Error::other)
}

fn delete(file: &File) -> io::Result<()> {
    let value = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: This operation marks this exact opened object for deletion; it never re-resolves a
    // pathname. The handle remains owned here and closes before completion is reported.
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&value as *const FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn identify(path: &Path) -> io::Result<FileIdentity> {
    if path.file_name().is_none() {
        let ancestors = lock_ancestors(path)?;
        return identity(ancestors.last().ok_or_else(boundary)?);
    }
    let parent = path.parent().ok_or_else(boundary)?;
    let ancestors = lock_ancestors(parent)?;
    identity(&open_relative(
        ancestors.last().ok_or_else(boundary)?,
        path.file_name().ok_or_else(boundary)?,
        false,
    )?)
}

pub(super) fn remove(
    root: &Path,
    root_identity: &FileIdentity,
    relative: &Path,
    expected: &FileIdentity,
    excluded: &dyn Fn(&Path) -> bool,
    context: &OperationContext,
    result: &mut Removal,
) -> io::Result<()> {
    context.check()?;
    if !root.is_absolute()
        || relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(boundary());
    }
    let root_handles = match lock_ancestors(root) {
        Ok(handles) => handles,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            result.already_absent = true;
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    if identity(root_handles.last().expect("locked root"))? != *root_identity {
        return Err(changed());
    }
    let mut nested = Vec::new();
    for component in relative.parent().unwrap_or(Path::new("")).components() {
        context.check()?;
        let parent = nested
            .last()
            .unwrap_or_else(|| root_handles.last().expect("locked root"));
        let file = open_relative(parent, component.as_os_str(), false)?;
        if attributes(&file)? & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || identity(&file)?.device != root_identity.device
        {
            return Err(boundary());
        }
        nested.push(file);
    }
    if excluded(relative) {
        return Err(boundary());
    }
    let parent = nested
        .last()
        .unwrap_or_else(|| root_handles.last().expect("locked root"));
    let selected = match open_relative(parent, relative.file_name().ok_or_else(boundary)?, true) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            result.already_absent = true;
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    if identity(&selected)? != *expected {
        return Err(changed());
    }
    remove_entry(
        selected,
        relative,
        root_identity.device,
        excluded,
        context,
        result,
    )
}

fn remove_entry(
    file: File,
    relative: &Path,
    device: u64,
    excluded: &dyn Fn(&Path) -> bool,
    context: &OperationContext,
    result: &mut Removal,
) -> io::Result<()> {
    context.check()?;
    if excluded(relative) {
        return Ok(());
    }
    if identity(&file)?.device != device {
        return Err(boundary());
    }
    let flags = attributes(&file)?;
    let directory = flags & FILE_ATTRIBUTE_DIRECTORY != 0;
    let link = flags & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    if directory && !link {
        let directory = handles::open_listing(&file)?;
        let mut restart = true;
        while let Some(names) = handles::read_directory(&directory, restart)? {
            restart = false;
            for name in names {
                context.check()?;
                let child_relative = relative.join(&name);
                if excluded(&child_relative) {
                    continue;
                }
                let child = match open_relative(&file, &name, true) {
                    Ok(file) => file,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                remove_entry(child, &child_relative, device, excluded, context, result)?;
            }
        }
    }
    let bytes = if directory || link {
        0
    } else {
        byte_size(&file)?
    };
    context.check()?;
    match delete(&file) {
        Ok(()) => {
            drop(file);
            result.removed_entries += 1;
            result.reclaimed_estimate = result.reclaimed_estimate.saturating_add(bytes);
            Ok(())
        }
        Err(error) if directory && error.kind() == io::ErrorKind::DirectoryNotEmpty => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests;
