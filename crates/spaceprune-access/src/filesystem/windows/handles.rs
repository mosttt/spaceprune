//! Open and enumerate relative to held Windows directories; display paths never drive recursion.

use super::{attributes, boundary};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io,
    mem::{offset_of, size_of},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
    ptr,
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FILE_OPEN, FILE_OPEN_FOR_BACKUP_INTENT, FILE_OPEN_REPARSE_POINT,
            FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
        },
    },
    Win32::{
        Foundation::{
            ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE, OBJ_CASE_INSENSITIVE, RtlNtStatusToDosError,
            UNICODE_STRING,
        },
        Storage::FileSystem::*,
        System::IO::IO_STATUS_BLOCK,
    },
};

fn valid_name(name: &OsStr) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.encode_wide().any(|unit| {
            unit == 0 || unit == b':' as u16 || unit == b'/' as u16 || unit == b'\\' as u16
        })
}

pub(super) fn open_relative(parent: &File, name: &OsStr, delete: bool) -> io::Result<File> {
    if !valid_name(name) {
        return Err(boundary());
    }
    open_child(
        parent,
        name,
        FILE_READ_ATTRIBUTES | if delete { DELETE } else { 0 },
    )
}

pub(super) fn open_listing(directory: &File) -> io::Result<File> {
    // The original handle excludes delete sharing, so this read handle cannot loosen that lock.
    // It must itself share deletion because the original selected directory holds DELETE access.
    // NT relative opens do not normalize ".". Reopen the held object, not a reconstructed path.
    // SAFETY: The original file-system handle remains open; File owns the returned handle.
    let handle = unsafe {
        ReOpenFile(
            directory.as_raw_handle(),
            FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_handle(handle) })
}

fn open_child(parent: &File, name: &OsStr, access: u32) -> io::Result<File> {
    let mut name = name.encode_wide().collect::<Vec<_>>();
    let length =
        u16::try_from(name.len().checked_mul(2).ok_or_else(boundary)?).map_err(|_| boundary())?;
    let object_name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: name.as_mut_ptr(),
    };
    let object = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &raw const object_name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: ptr::null(),
        SecurityQualityOfService: ptr::null(),
    };
    let mut status = IO_STATUS_BLOCK::default();
    let mut handle = INVALID_HANDLE_VALUE;
    // SAFETY: The directory handle and single-component UTF-16 buffer remain valid. File owns
    // every successful handle; no pathname prefix is resolved again while opening a child.
    let result = unsafe {
        NtCreateFile(
            &raw mut handle,
            access | SYNCHRONIZE,
            &raw const object,
            &raw mut status,
            ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT | FILE_OPEN_FOR_BACKUP_INTENT | FILE_SYNCHRONOUS_IO_NONALERT,
            ptr::null(),
            0,
        )
    };
    if result < 0 {
        // SAFETY: Converting an NTSTATUS preserves structured Win32 errors for the caller.
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        ));
    }
    Ok(unsafe { File::from_raw_handle(handle) })
}

pub(super) fn lock_ancestors(path: &Path) -> io::Result<Vec<File>> {
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Err(boundary());
    };
    if !matches!(
        prefix.kind(),
        Prefix::Disk(_) | Prefix::VerbatimDisk(_) | Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _)
    ) || components.next() != Some(Component::RootDir)
    {
        return Err(boundary());
    }
    let mut anchor = PathBuf::from(prefix.as_os_str());
    anchor.push(Path::new(r"\"));
    let name: Vec<u16> = anchor.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: Only the local/UNC volume anchor is opened by path, then all traversal uses handles.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let mut held = vec![unsafe { File::from_raw_handle(handle) }];
    let flags = attributes(&held[0])?;
    if flags & FILE_ATTRIBUTE_REPARSE_POINT != 0 || flags & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(boundary());
    }
    for component in components {
        let Component::Normal(name) = component else {
            return Err(boundary());
        };
        let file = open_relative(held.last().expect("volume anchor"), name, false)?;
        let flags = attributes(&file)?;
        if flags & FILE_ATTRIBUTE_REPARSE_POINT != 0 || flags & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(boundary());
        }
        held.push(file);
    }
    Ok(held)
}

pub(super) fn read_directory(directory: &File, restart: bool) -> io::Result<Option<Vec<OsString>>> {
    const CAPACITY: usize = 64 * 1024;
    let mut buffer = vec![0_u64; CAPACITY / size_of::<u64>()];
    // SAFETY: The writable, aligned buffer has the declared size and the directory is held.
    if unsafe {
        GetFileInformationByHandleEx(
            directory.as_raw_handle(),
            if restart {
                FileIdBothDirectoryRestartInfo
            } else {
                FileIdBothDirectoryInfo
            },
            buffer.as_mut_ptr().cast(),
            CAPACITY as u32,
        )
    } == 0
    {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
            Ok(None)
        } else {
            Err(error)
        };
    }
    let mut offset = 0_usize;
    let mut names = Vec::new();
    let bytes = buffer.as_ptr().cast::<u8>();
    let name_offset = offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
    loop {
        if offset + size_of::<FILE_ID_BOTH_DIR_INFO>() > CAPACITY {
            return Err(boundary());
        }
        // SAFETY: The header range is validated; unaligned reads tolerate native record alignment.
        let entry = unsafe {
            bytes
                .add(offset)
                .cast::<FILE_ID_BOTH_DIR_INFO>()
                .read_unaligned()
        };
        let start = offset + name_offset;
        let end = start
            .checked_add(entry.FileNameLength as usize)
            .ok_or_else(boundary)?;
        let next = if entry.NextEntryOffset == 0 {
            CAPACITY
        } else {
            offset
                .checked_add(entry.NextEntryOffset as usize)
                .ok_or_else(boundary)?
        };
        if entry.FileNameLength == 0
            || !entry.FileNameLength.is_multiple_of(2)
            || !start.is_multiple_of(2)
            || end > next
            || next > CAPACITY
            || next <= offset
        {
            return Err(boundary());
        }
        // SAFETY: Both the UTF-16 alignment and the complete filename byte range are checked.
        let name = OsString::from_wide(unsafe {
            std::slice::from_raw_parts(
                bytes.add(start).cast::<u16>(),
                entry.FileNameLength as usize / 2,
            )
        });
        if name != "." && name != ".." {
            if !valid_name(&name) {
                return Err(boundary());
            }
            names.push(name);
        }
        if entry.NextEntryOffset == 0 {
            break;
        }
        offset = next;
    }
    Ok(Some(names))
}
