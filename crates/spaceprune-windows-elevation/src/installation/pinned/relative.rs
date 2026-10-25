//! Open a single component beneath an already pinned directory without re-resolving ancestors.
use std::{
    ffi::OsStr,
    fs::File,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle},
    },
    ptr,
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_FOR_BACKUP_INTENT,
            FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
        },
    },
    Win32::{
        Foundation::{INVALID_HANDLE_VALUE, OBJ_CASE_INSENSITIVE, UNICODE_STRING},
        Storage::FileSystem::{FILE_ATTRIBUTE_NORMAL, SYNCHRONIZE},
        System::IO::IO_STATUS_BLOCK,
    },
};

pub(super) fn open(
    parent: &File,
    name: &OsStr,
    access: u32,
    sharing: u32,
    directory: bool,
) -> Option<File> {
    let mut name = name.encode_wide().collect::<Vec<_>>();
    if name.is_empty()
        || name == [b'.' as u16]
        || name == [b'.' as u16, b'.' as u16]
        || name
            .iter()
            .any(|unit| [0, b'/' as u16, b'\\' as u16, b':' as u16].contains(unit))
    {
        return None;
    }
    let length = u16::try_from(name.len().checked_mul(size_of::<u16>())?).ok()?;
    let object_name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &raw const object_name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: ptr::null(),
        SecurityQualityOfService: ptr::null(),
    };
    let mut status = IO_STATUS_BLOCK::default();
    let mut handle = INVALID_HANDLE_VALUE;
    let kind = if directory {
        FILE_DIRECTORY_FILE
    } else {
        FILE_NON_DIRECTORY_FILE
    };
    // SAFETY: The parent handle and single-component name remain valid throughout the call.
    if unsafe {
        NtCreateFile(
            &raw mut handle,
            access | SYNCHRONIZE,
            &raw const attributes,
            &raw mut status,
            ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            sharing,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT
                | FILE_OPEN_FOR_BACKUP_INTENT
                | FILE_SYNCHRONOUS_IO_NONALERT
                | kind,
            ptr::null(),
            0,
        )
    } < 0
    {
        return None;
    }
    // SAFETY: A successful NtCreateFile returns a new owned handle, transferred to File once.
    Some(unsafe { File::from_raw_handle(handle) })
}
