//! Open private IPC paths by directory handles without following reparse points.

use super::{OwnedHandle, PrivatePathError, PrivatePathKind, wide_null};
use std::{
    ffi::{OsStr, OsString},
    mem::size_of,
    os::windows::ffi::OsStrExt as _,
    path::{Component, Path, PathBuf, Prefix},
    ptr,
};
use windows_sys::Wdk::{
    Foundation::OBJECT_ATTRIBUTES,
    Storage::FileSystem::{
        FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_FOR_BACKUP_INTENT,
        FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
    },
};
use windows_sys::Win32::{
    Foundation::{INVALID_HANDLE_VALUE, OBJ_CASE_INSENSITIVE, UNICODE_STRING},
    Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FILE_TRAVERSE, FileAttributeTagInfo, FileIdInfo, GetFileInformationByHandleEx, GetFileType,
        OPEN_EXISTING, READ_CONTROL, SYNCHRONIZE,
    },
    System::IO::IO_STATUS_BLOCK,
};

pub(super) fn open_plain_path(
    path: &Path,
    kind: PrivatePathKind,
    extra_access: u32,
) -> Result<OwnedHandle, PrivatePathError> {
    let (anchor, components) = split_absolute_path(path).ok_or(PrivatePathError::Ineligible)?;
    if components.is_empty() {
        return Err(PrivatePathError::Ineligible);
    }
    let mut parent = open_anchor(&anchor)?;
    let volume = volume_serial(&parent)?;
    for component in &components[..components.len() - 1] {
        parent = open_relative_private(&parent, component, PrivatePathKind::Directory, 0)?;
        if volume_serial(&parent)? != volume {
            return Err(PrivatePathError::Ineligible);
        }
    }
    let target = open_relative_private(
        &parent,
        &components[components.len() - 1],
        kind,
        READ_CONTROL | extra_access,
    )?;
    if volume_serial(&target)? != volume {
        return Err(PrivatePathError::Ineligible);
    }
    Ok(target)
}

fn split_absolute_path(path: &Path) -> Option<(PathBuf, Vec<OsString>)> {
    let mut components = path.components();
    let Component::Prefix(prefix) = components.next()? else {
        return None;
    };
    if !matches!(
        prefix.kind(),
        Prefix::Disk(_) | Prefix::VerbatimDisk(_) | Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _)
    ) || components.next() != Some(Component::RootDir)
    {
        return None;
    }
    let mut anchor = PathBuf::from(prefix.as_os_str());
    anchor.push(Path::new(r"\"));
    let names = components
        .map(|component| match component {
            Component::Normal(name) if valid_relative_name(name) => Some(name.to_owned()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some((anchor, names))
}

fn open_anchor(path: &Path) -> Result<OwnedHandle, PrivatePathError> {
    let path = wide_null(path.as_os_str());
    // SAFETY: 路径缓冲区NUL结尾并在调用期间有效；返回句柄立即由RAII管理。
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_READ_ATTRIBUTES | FILE_TRAVERSE | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    let handle = OwnedHandle::new(handle)?;
    plain_kind_matches(&handle, PrivatePathKind::Directory)
        .then_some(handle)
        .ok_or(PrivatePathError::Ineligible)
}

fn open_relative_private(
    parent: &OwnedHandle,
    name: &OsStr,
    kind: PrivatePathKind,
    extra_access: u32,
) -> Result<OwnedHandle, PrivatePathError> {
    if !valid_relative_name(name) {
        return Err(PrivatePathError::Ineligible);
    }
    let mut name = name.encode_wide().collect::<Vec<_>>();
    let byte_length = name
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|value| u16::try_from(value).ok())
        .ok_or(PrivatePathError::Ineligible)?;
    let object_name = UNICODE_STRING {
        Length: byte_length,
        MaximumLength: byte_length,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: u32::try_from(size_of::<OBJECT_ATTRIBUTES>())
            .map_err(|_| PrivatePathError::Unavailable)?,
        RootDirectory: parent.0,
        ObjectName: &raw const object_name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: ptr::null(),
        SecurityQualityOfService: ptr::null(),
    };
    let mut status = IO_STATUS_BLOCK::default();
    let mut handle = INVALID_HANDLE_VALUE;
    let (access, kind_option) = match kind {
        PrivatePathKind::Directory => (
            FILE_READ_ATTRIBUTES | FILE_TRAVERSE | SYNCHRONIZE,
            FILE_DIRECTORY_FILE,
        ),
        PrivatePathKind::File => (FILE_READ_ATTRIBUTES | SYNCHRONIZE, FILE_NON_DIRECTORY_FILE),
    };
    let access = access | extra_access;
    // SAFETY: 父句柄有效；对象名是单一已校验组件；输入输出结构体在调用期间有效。
    if unsafe {
        NtCreateFile(
            &raw mut handle,
            access,
            &raw const attributes,
            &raw mut status,
            ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT
                | FILE_OPEN_FOR_BACKUP_INTENT
                | FILE_SYNCHRONOUS_IO_NONALERT
                | kind_option,
            ptr::null(),
            0,
        )
    } < 0
    {
        return Err(PrivatePathError::Unavailable);
    }
    let handle = OwnedHandle::new(handle)?;
    plain_kind_matches(&handle, kind)
        .then_some(handle)
        .ok_or(PrivatePathError::Ineligible)
}

fn volume_serial(handle: &OwnedHandle) -> Result<u64, PrivatePathError> {
    let mut id = FILE_ID_INFO::default();
    // SAFETY: 输出结构体尺寸正确且句柄有效。
    if unsafe {
        GetFileInformationByHandleEx(
            handle.0,
            FileIdInfo,
            (&raw mut id).cast(),
            u32::try_from(size_of::<FILE_ID_INFO>()).unwrap_or(0),
        )
    } == 0
    {
        return Err(PrivatePathError::Unavailable);
    }
    Ok(id.VolumeSerialNumber)
}

fn valid_relative_name(name: &OsStr) -> bool {
    let value = name.encode_wide().collect::<Vec<_>>();
    !value.is_empty()
        && value != [b'.' as u16]
        && value != [b'.' as u16, b'.' as u16]
        && !value.iter().any(|unit| {
            *unit == 0 || *unit == b'\\' as u16 || *unit == b'/' as u16 || *unit == b':' as u16
        })
}

fn plain_kind_matches(handle: &OwnedHandle, expected: PrivatePathKind) -> bool {
    if unsafe { GetFileType(handle.0) } != windows_sys::Win32::Storage::FileSystem::FILE_TYPE_DISK {
        return false;
    }
    let mut tag = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: 输出结构体尺寸正确且句柄有效。
    if unsafe {
        GetFileInformationByHandleEx(
            handle.0,
            FileAttributeTagInfo,
            (&raw mut tag).cast(),
            u32::try_from(size_of::<FILE_ATTRIBUTE_TAG_INFO>()).unwrap_or(0),
        )
    } == 0
        || tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return false;
    }
    let directory = tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    directory == (expected == PrivatePathKind::Directory)
}
