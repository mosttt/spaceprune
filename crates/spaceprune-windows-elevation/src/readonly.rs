use std::{
    collections::VecDeque,
    ffi::{OsStr, OsString},
    mem::{offset_of, size_of},
    os::windows::ffi::{OsStrExt as _, OsStringExt as _},
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
    Foundation::{
        CloseHandle, ERROR_NO_MORE_FILES, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
        OBJ_CASE_INSENSITIVE, UNICODE_STRING,
    },
    Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_ATTRIBUTE_TAG_INFO, FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_BOTH_DIR_INFO, FILE_ID_INFO, FILE_LIST_DIRECTORY,
        FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FILE_STANDARD_INFO, FILE_TRAVERSE, FILE_TYPE_DISK, FileAttributeTagInfo, FileBasicInfo,
        FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo, FileIdInfo, FileStandardInfo,
        GetFileInformationByHandleEx, GetFileType, OPEN_EXISTING, ReadFile, SYNCHRONIZE,
    },
    System::IO::IO_STATUS_BLOCK,
};

use crate::{BoundedReadonlyFile, ReadonlyFileError, ReadonlyFileIdentity, ReadonlyMetadata};

const DIRECTORY_BUFFER_BYTES: usize = 64 * 1024;

pub(super) fn measure(
    roots: &[PathBuf],
    max_entries: u64,
    max_depth: usize,
    mut should_stop: impl FnMut() -> bool,
) -> ReadonlyMetadata {
    let mut aggregate = ReadonlyMetadata::default();
    let mut queue = VecDeque::new();
    for root in roots {
        if should_stop() {
            return aggregate;
        }
        if is_execution_artifact(root) {
            aggregate.skipped_entries = aggregate.skipped_entries.saturating_add(1);
            continue;
        }
        let Some((anchor, components)) = split_absolute_path(root) else {
            aggregate.skipped_entries = aggregate.skipped_entries.saturating_add(1);
            continue;
        };
        let handle =
            match open_anchor(&anchor).and_then(|anchor| open_components(anchor, &components)) {
                Ok(handle) => handle,
                Err(()) => {
                    aggregate.skipped_entries = aggregate.skipped_entries.saturating_add(1);
                    continue;
                }
            };
        match entry_info(&handle) {
            Ok(EntryInfo::File(size)) => {
                aggregate.accessible_roots = aggregate.accessible_roots.saturating_add(1);
                aggregate.inspected_entries = aggregate.inspected_entries.saturating_add(1);
                aggregate.logical_bytes = aggregate.logical_bytes.saturating_add(size);
            }
            Ok(EntryInfo::Directory) => {
                aggregate.accessible_roots = aggregate.accessible_roots.saturating_add(1);
                queue.push_back((handle, 0_usize));
            }
            Err(()) => {
                aggregate.skipped_entries = aggregate.skipped_entries.saturating_add(1);
            }
        }
    }

    while let Some((directory, depth)) = queue.pop_front() {
        let mut restart = true;
        loop {
            if should_stop() {
                return aggregate;
            }
            let names = match read_directory_batch(&directory, restart) {
                Ok(Some(names)) => names,
                Ok(None) => break,
                Err(()) => {
                    aggregate.skipped_entries = aggregate.skipped_entries.saturating_add(1);
                    break;
                }
            };
            restart = false;
            for name in names {
                if should_stop() {
                    return aggregate;
                }
                if aggregate.inspected_entries >= max_entries {
                    aggregate.skipped_entries = aggregate.skipped_entries.saturating_add(1);
                    return aggregate;
                }
                if is_execution_artifact(Path::new(&name)) {
                    aggregate.skipped_entries = aggregate.skipped_entries.saturating_add(1);
                    continue;
                }
                let handle = match open_child(&directory, &name) {
                    Ok(handle) => handle,
                    Err(()) => {
                        aggregate.skipped_entries = aggregate.skipped_entries.saturating_add(1);
                        continue;
                    }
                };
                aggregate.inspected_entries = aggregate.inspected_entries.saturating_add(1);
                match entry_info(&handle) {
                    Ok(EntryInfo::File(size)) => {
                        aggregate.logical_bytes = aggregate.logical_bytes.saturating_add(size);
                    }
                    Ok(EntryInfo::Directory) if depth < max_depth => {
                        queue.push_back((handle, depth + 1));
                    }
                    Ok(EntryInfo::Directory) | Err(()) => {
                        aggregate.skipped_entries = aggregate.skipped_entries.saturating_add(1);
                    }
                }
            }
        }
    }
    aggregate
}

// Keep aggregate inspection aligned with the reserved execution-artifact boundary.
fn is_execution_artifact(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        name.starts_with(".spaceprune-execute-") || name == ".spaceprune-executions"
    })
}

pub(super) fn identify_file(
    root: &Path,
    file: &Path,
) -> Result<ReadonlyFileIdentity, ReadonlyFileError> {
    let handle = open_file_beneath_root(root, file, false)?;
    file_identity(&handle).map_err(|()| ReadonlyFileError::Ineligible)
}

pub(super) fn read_bounded_file(
    root: &Path,
    file: &Path,
    expected: &ReadonlyFileIdentity,
    max_bytes: usize,
) -> Result<BoundedReadonlyFile, ReadonlyFileError> {
    let handle = open_file_beneath_root(root, file, true)?;
    let before = file_identity(&handle).map_err(|()| ReadonlyFileError::Ineligible)?;
    if &before != expected {
        return Err(ReadonlyFileError::Changed);
    }
    let length = usize::try_from(before.logical_bytes).map_err(|_| ReadonlyFileError::TooLarge)?;
    if length > max_bytes {
        return Err(ReadonlyFileError::TooLarge);
    }
    let mut bytes = vec![0_u8; length];
    let mut offset = 0_usize;
    while offset < bytes.len() {
        let remaining = u32::try_from(bytes.len() - offset).unwrap_or(u32::MAX);
        let mut read = 0_u32;
        // SAFETY: 文件句柄同步打开且保持有效；缓冲区剩余范围可写，读取长度不超过该范围。
        if unsafe {
            ReadFile(
                handle.0,
                bytes[offset..].as_mut_ptr(),
                remaining,
                &raw mut read,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(ReadonlyFileError::Unavailable);
        }
        if read == 0 {
            return Err(ReadonlyFileError::Changed);
        }
        offset = offset
            .checked_add(usize::try_from(read).map_err(|_| ReadonlyFileError::Unavailable)?)
            .ok_or(ReadonlyFileError::Unavailable)?;
    }
    let after = file_identity(&handle).map_err(|()| ReadonlyFileError::Ineligible)?;
    if after != before {
        return Err(ReadonlyFileError::Changed);
    }
    Ok(BoundedReadonlyFile {
        identity: after,
        bytes,
    })
}

fn open_file_beneath_root(
    root: &Path,
    file: &Path,
    read_data: bool,
) -> Result<OwnedHandle, ReadonlyFileError> {
    let (anchor, root_components) =
        split_absolute_path_allow_root(root).ok_or(ReadonlyFileError::Ineligible)?;
    let relative = file
        .strip_prefix(root)
        .map_err(|_| ReadonlyFileError::Ineligible)?;
    let relative = relative
        .components()
        .map(|component| match component {
            Component::Normal(name) if valid_relative_name(name) => Ok(name.to_owned()),
            _ => Err(ReadonlyFileError::Ineligible),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if relative.is_empty() {
        return Err(ReadonlyFileError::Ineligible);
    }
    let mut parent = open_anchor(&anchor).map_err(|()| ReadonlyFileError::Ineligible)?;
    for component in root_components {
        parent = open_relative(&parent, &component, EntryKind::Directory)
            .map_err(|()| ReadonlyFileError::Ineligible)?;
    }
    let root_volume = handle_volume_serial(&parent).map_err(|()| ReadonlyFileError::Ineligible)?;
    for component in &relative[..relative.len() - 1] {
        parent = open_relative(&parent, component, EntryKind::Directory)
            .map_err(|()| ReadonlyFileError::Ineligible)?;
        if handle_volume_serial(&parent).map_err(|()| ReadonlyFileError::Ineligible)? != root_volume
        {
            return Err(ReadonlyFileError::Ineligible);
        }
    }
    let leaf = open_relative_with_data(
        &parent,
        &relative[relative.len() - 1],
        EntryKind::File,
        read_data,
    )
    .map_err(|()| ReadonlyFileError::Ineligible)?;
    if handle_volume_serial(&leaf).map_err(|()| ReadonlyFileError::Ineligible)? != root_volume {
        return Err(ReadonlyFileError::Ineligible);
    }
    Ok(leaf)
}

fn split_absolute_path_allow_root(path: &Path) -> Option<(PathBuf, Vec<OsString>)> {
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

fn split_absolute_path(path: &Path) -> Option<(PathBuf, Vec<OsString>)> {
    let mut components = path.components();
    let Component::Prefix(prefix) = components.next()? else {
        return None;
    };
    if !matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        || components.next() != Some(Component::RootDir)
    {
        return None;
    }
    let mut anchor = PathBuf::from(prefix.as_os_str());
    anchor.push(Path::new(r"\"));
    let mut names = Vec::new();
    for component in components {
        let Component::Normal(name) = component else {
            return None;
        };
        if !valid_relative_name(name) {
            return None;
        }
        names.push(name.to_owned());
    }
    (!names.is_empty()).then_some((anchor, names))
}

fn open_anchor(path: &Path) -> Result<OwnedHandle, ()> {
    let path = wide_null(path.as_os_str());
    // SAFETY: 路径缓冲区以 NUL 结尾且在调用期间有效；返回句柄立即交给 `OwnedHandle`。
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
    matches!(entry_info(&handle), Ok(EntryInfo::Directory))
        .then_some(handle)
        .ok_or(())
}

fn open_components(mut parent: OwnedHandle, components: &[OsString]) -> Result<OwnedHandle, ()> {
    for (index, component) in components.iter().enumerate() {
        parent = if index + 1 == components.len() {
            open_child(&parent, component)?
        } else {
            open_relative(&parent, component, EntryKind::Directory)?
        };
    }
    Ok(parent)
}

pub(super) fn valid_metadata_root(path: &Path) -> bool {
    if !matches!(path.components().next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
    {
        return false;
    }
    split_absolute_path_allow_root(path)
        .and_then(|(anchor, components)| {
            open_anchor(&anchor).ok().and_then(|mut root| {
                for component in components {
                    root = open_relative(&root, &component, EntryKind::Directory)
                        .or_else(|()| open_relative(&root, &component, EntryKind::File))
                        .ok()?;
                }
                Some(root)
            })
        })
        .is_some()
}

fn open_child(parent: &OwnedHandle, name: &OsStr) -> Result<OwnedHandle, ()> {
    open_relative_with_data(parent, name, EntryKind::Directory, true)
        .or_else(|()| open_relative(parent, name, EntryKind::File))
}

fn open_relative(
    parent: &OwnedHandle,
    name: &OsStr,
    expected: EntryKind,
) -> Result<OwnedHandle, ()> {
    open_relative_with_data(parent, name, expected, false)
}

fn open_relative_with_data(
    parent: &OwnedHandle,
    name: &OsStr,
    expected: EntryKind,
    read_data: bool,
) -> Result<OwnedHandle, ()> {
    if !valid_relative_name(name) {
        return Err(());
    }
    let mut name = name.encode_wide().collect::<Vec<_>>();
    let byte_length = name.len().checked_mul(size_of::<u16>()).ok_or(())?;
    let length = u16::try_from(byte_length).map_err(|_| ())?;
    let object_name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: u32::try_from(size_of::<OBJECT_ATTRIBUTES>()).map_err(|_| ())?,
        RootDirectory: parent.0,
        ObjectName: &raw const object_name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: ptr::null(),
        SecurityQualityOfService: ptr::null(),
    };
    let mut status = IO_STATUS_BLOCK::default();
    let mut handle = INVALID_HANDLE_VALUE;
    let (access, kind_option) = match expected {
        EntryKind::Directory => (
            FILE_READ_ATTRIBUTES
                | FILE_TRAVERSE
                | SYNCHRONIZE
                | if read_data { FILE_LIST_DIRECTORY } else { 0 },
            FILE_DIRECTORY_FILE,
        ),
        EntryKind::File => (
            FILE_READ_ATTRIBUTES | SYNCHRONIZE | if read_data { FILE_READ_DATA } else { 0 },
            FILE_NON_DIRECTORY_FILE,
        ),
    };
    // SAFETY: 父句柄保持有效；对象名是单个已校验 UTF-16 组件；所有输入/输出结构体在调用期间有效。
    let result = unsafe {
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
    };
    if result < 0 {
        return Err(());
    }
    let handle = OwnedHandle::new(handle)?;
    let actual = entry_info(&handle)?;
    if matches!(
        (expected, actual),
        (EntryKind::Directory, EntryInfo::Directory) | (EntryKind::File, EntryInfo::File(_))
    ) {
        Ok(handle)
    } else {
        Err(())
    }
}

fn handle_volume_serial(handle: &OwnedHandle) -> Result<u64, ()> {
    let mut id = FILE_ID_INFO::default();
    // SAFETY: 输出指针指向尺寸正确、可写的 `FILE_ID_INFO`，句柄在调用期间有效。
    if unsafe {
        GetFileInformationByHandleEx(
            handle.0,
            FileIdInfo,
            (&raw mut id).cast(),
            u32::try_from(size_of::<FILE_ID_INFO>()).map_err(|_| ())?,
        )
    } == 0
    {
        return Err(());
    }
    Ok(id.VolumeSerialNumber)
}

fn file_identity(handle: &OwnedHandle) -> Result<ReadonlyFileIdentity, ()> {
    let EntryInfo::File(logical_bytes) = entry_info(handle)? else {
        return Err(());
    };
    let mut id = FILE_ID_INFO::default();
    // SAFETY: 输出指针指向尺寸正确、可写的 `FILE_ID_INFO`，句柄在调用期间有效。
    if unsafe {
        GetFileInformationByHandleEx(
            handle.0,
            FileIdInfo,
            (&raw mut id).cast(),
            u32::try_from(size_of::<FILE_ID_INFO>()).map_err(|_| ())?,
        )
    } == 0
    {
        return Err(());
    }
    let mut basic = FILE_BASIC_INFO::default();
    // SAFETY: 输出指针指向尺寸正确、可写的 `FILE_BASIC_INFO`，句柄在调用期间有效。
    if unsafe {
        GetFileInformationByHandleEx(
            handle.0,
            FileBasicInfo,
            (&raw mut basic).cast(),
            u32::try_from(size_of::<FILE_BASIC_INFO>()).map_err(|_| ())?,
        )
    } == 0
    {
        return Err(());
    }
    if basic.FileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        return Err(());
    }
    Ok(ReadonlyFileIdentity {
        volume_serial_number: id.VolumeSerialNumber,
        file_id: id.FileId.Identifier,
        logical_bytes,
        last_write_time: basic.LastWriteTime,
        change_time: basic.ChangeTime,
    })
}

fn entry_info(handle: &OwnedHandle) -> Result<EntryInfo, ()> {
    // SAFETY: `OwnedHandle` 在调用期间保持有效，查询不修改 Rust 内存。
    if unsafe { GetFileType(handle.0) } != FILE_TYPE_DISK {
        return Err(());
    }
    let mut tag = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: 输出指针指向尺寸正确、可写的 `FILE_ATTRIBUTE_TAG_INFO`。
    if unsafe {
        GetFileInformationByHandleEx(
            handle.0,
            FileAttributeTagInfo,
            (&raw mut tag).cast(),
            u32::try_from(size_of::<FILE_ATTRIBUTE_TAG_INFO>()).map_err(|_| ())?,
        )
    } == 0
        || tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(());
    }
    let mut standard = FILE_STANDARD_INFO::default();
    // SAFETY: 输出指针指向尺寸正确、可写的 `FILE_STANDARD_INFO`。
    if unsafe {
        GetFileInformationByHandleEx(
            handle.0,
            FileStandardInfo,
            (&raw mut standard).cast(),
            u32::try_from(size_of::<FILE_STANDARD_INFO>()).map_err(|_| ())?,
        )
    } == 0
    {
        return Err(());
    }
    let directory_attribute = tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    if directory_attribute != standard.Directory {
        return Err(());
    }
    if directory_attribute {
        Ok(EntryInfo::Directory)
    } else {
        u64::try_from(standard.EndOfFile)
            .map(EntryInfo::File)
            .map_err(|_| ())
    }
}

fn read_directory_batch(
    directory: &OwnedHandle,
    restart: bool,
) -> Result<Option<Vec<OsString>>, ()> {
    let mut buffer = vec![0_u64; DIRECTORY_BUFFER_BYTES / size_of::<u64>()];
    let class = if restart {
        FileIdBothDirectoryRestartInfo
    } else {
        FileIdBothDirectoryInfo
    };
    // SAFETY: 缓冲区按 `u64` 对齐、具有声明的可写容量，目录句柄在调用期间有效。
    if unsafe {
        GetFileInformationByHandleEx(
            directory.0,
            class,
            buffer.as_mut_ptr().cast(),
            u32::try_from(DIRECTORY_BUFFER_BYTES).map_err(|_| ())?,
        )
    } == 0
    {
        // SAFETY: 紧接失败的 Win32 调用读取线程局部错误码。
        return if unsafe { GetLastError() } == ERROR_NO_MORE_FILES {
            Ok(None)
        } else {
            Err(())
        };
    }

    let bytes = buffer.as_ptr().cast::<u8>();
    let name_offset = offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
    let mut offset = 0_usize;
    let mut names = Vec::new();
    loop {
        let header_end = offset.checked_add(name_offset).ok_or(())?;
        let structure_end = offset
            .checked_add(size_of::<FILE_ID_BOTH_DIR_INFO>())
            .ok_or(())?;
        if structure_end > DIRECTORY_BUFFER_BYTES || header_end % std::mem::align_of::<u16>() != 0 {
            return Err(());
        }
        // SAFETY: 头部范围已验证；使用非对齐读取避免依赖系统记录对齐。
        let entry =
            unsafe { ptr::read_unaligned(bytes.add(offset).cast::<FILE_ID_BOTH_DIR_INFO>()) };
        let name_bytes = usize::try_from(entry.FileNameLength).map_err(|_| ())?;
        if name_bytes == 0 || name_bytes % size_of::<u16>() != 0 {
            return Err(());
        }
        let record_end = if entry.NextEntryOffset == 0 {
            DIRECTORY_BUFFER_BYTES
        } else {
            let next = usize::try_from(entry.NextEntryOffset).map_err(|_| ())?;
            if next < name_offset {
                return Err(());
            }
            offset.checked_add(next).ok_or(())?
        };
        let name_end = header_end.checked_add(name_bytes).ok_or(())?;
        if name_end > record_end || record_end > DIRECTORY_BUFFER_BYTES {
            return Err(());
        }
        // SAFETY: 文件名边界已经按字节长度验证，起点由结构体字段偏移保证 UTF-16 对齐。
        let name = OsString::from_wide(unsafe {
            std::slice::from_raw_parts(
                bytes.add(header_end).cast::<u16>(),
                name_bytes / size_of::<u16>(),
            )
        });
        if name != "." && name != ".." {
            names.push(name);
        }
        if entry.NextEntryOffset == 0 {
            break;
        }
        offset = record_end;
    }
    Ok(Some(names))
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

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

#[derive(Clone, Copy)]
enum EntryKind {
    Directory,
    File,
}

#[derive(Clone, Copy)]
enum EntryInfo {
    Directory,
    File(u64),
}

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(handle: HANDLE) -> Result<Self, ()> {
        (!handle.is_null() && handle != INVALID_HANDLE_VALUE)
            .then_some(Self(handle))
            .ok_or(())
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: `OwnedHandle` 独占有效句柄且只在此处关闭一次。
        let _ = unsafe { CloseHandle(self.0) };
        self.0 = ptr::null_mut();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_local_drive_roots_and_rejects_unc_roots() {
        let (anchor, names) = split_absolute_path(Path::new(r"C:\cache\data")).expect("本地路径");
        assert_eq!(anchor, Path::new(r"C:\"));
        assert_eq!(names, [OsString::from("cache"), OsString::from("data")]);
        assert!(split_absolute_path(Path::new(r"\\server\share\cache\data")).is_none());
        let (anchor, names) =
            split_absolute_path_allow_root(Path::new(r"\\server\share\cache")).expect("UNC授权根");
        assert_eq!(anchor, Path::new(r"\\server\share\"));
        assert_eq!(names, [OsString::from("cache")]);
    }

    #[test]
    fn measures_regular_files_through_directory_handles() {
        let temporary = tempfile::tempdir().expect("临时目录");
        let root = temporary.path().join("cache/data");
        std::fs::create_dir_all(root.join("nested")).expect("测试目录");
        std::fs::write(root.join("one.bin"), [0_u8; 17]).expect("测试文件");
        std::fs::write(root.join("nested/two.bin"), [0_u8; 23]).expect("测试文件");
        let root = std::fs::canonicalize(root).expect("规范路径");

        let aggregate = measure(&[root], 4_096, 3, || false);

        assert_eq!(aggregate.logical_bytes, 40);
        assert_eq!(aggregate.inspected_entries, 3);
        assert_eq!(aggregate.skipped_entries, 0);
        assert_eq!(aggregate.accessible_roots, 1);
    }

    #[test]
    fn rejects_a_reparse_point_when_windows_allows_test_symlinks() {
        use std::os::windows::fs::symlink_file;

        let temporary = tempfile::tempdir().expect("临时目录");
        let root = temporary.path().join("cache/data");
        std::fs::create_dir_all(&root).expect("测试目录");
        let outside = temporary.path().join("outside.bin");
        std::fs::write(&outside, [0_u8; 99]).expect("外部文件");
        if symlink_file(&outside, root.join("outside-link")).is_err() {
            return;
        }
        let root = std::fs::canonicalize(root).expect("规范路径");

        let aggregate = measure(&[root], 4_096, 3, || false);

        assert_eq!(aggregate.logical_bytes, 0);
        assert_eq!(aggregate.inspected_entries, 0);
        assert_eq!(aggregate.skipped_entries, 1);
        assert_eq!(aggregate.accessible_roots, 1);
    }

    #[test]
    fn identifies_and_reads_one_bounded_regular_file() {
        let temporary = tempfile::tempdir().expect("临时目录");
        let root = temporary.path().join("cache");
        std::fs::create_dir(&root).expect("测试目录");
        std::fs::write(root.join("manifest.toml"), b"kind = \"cache\"\n").expect("测试文件");
        let root = std::fs::canonicalize(root).expect("规范路径");
        let file = root.join("manifest.toml");

        let identity = identify_file(&root, &file).expect("文件身份");
        let result = read_bounded_file(&root, &file, &identity, 16 * 1024).expect("完整读取");

        assert_eq!(result.identity, identity);
        assert_eq!(result.bytes, b"kind = \"cache\"\n");
    }

    #[test]
    fn bounded_read_rejects_changed_oversized_and_outside_files() {
        let temporary = tempfile::tempdir().expect("临时目录");
        let root = temporary.path().join("cache");
        std::fs::create_dir(&root).expect("测试目录");
        let file = root.join("manifest.toml");
        std::fs::write(&file, b"kind = \"cache\"\n").expect("测试文件");
        let outside = temporary.path().join("outside.toml");
        std::fs::write(&outside, b"kind = \"outside\"\n").expect("外部文件");
        let root = std::fs::canonicalize(root).expect("规范路径");
        let file = root.join("manifest.toml");
        let identity = identify_file(&root, &file).expect("文件身份");

        assert_eq!(
            read_bounded_file(&root, &file, &identity, 1),
            Err(ReadonlyFileError::TooLarge)
        );
        std::fs::write(&file, b"kind = \"changed-and-longer\"\n").expect("替换测试文件");
        assert_eq!(
            read_bounded_file(&root, &file, &identity, 16 * 1024),
            Err(ReadonlyFileError::Changed)
        );
        assert_eq!(
            identify_file(&root, &outside),
            Err(ReadonlyFileError::Ineligible)
        );
    }

    #[test]
    fn bounded_read_rejects_leaf_and_intermediate_reparse_points_when_available() {
        use std::os::windows::fs::{symlink_dir, symlink_file};

        let temporary = tempfile::tempdir().expect("临时目录");
        let root = temporary.path().join("cache");
        let outside = temporary.path().join("outside");
        std::fs::create_dir(&root).expect("测试目录");
        std::fs::create_dir(&outside).expect("外部目录");
        std::fs::write(outside.join("manifest.toml"), b"kind = \"outside\"\n").expect("外部文件");
        let leaf = root.join("leaf.toml");
        let intermediate = root.join("linked");
        if symlink_file(outside.join("manifest.toml"), &leaf).is_err()
            || symlink_dir(&outside, &intermediate).is_err()
        {
            return;
        }
        let root = std::fs::canonicalize(root).expect("规范路径");

        assert_eq!(
            identify_file(&root, &root.join("leaf.toml")),
            Err(ReadonlyFileError::Ineligible)
        );
        assert_eq!(
            identify_file(&root, &root.join("linked/manifest.toml")),
            Err(ReadonlyFileError::Ineligible)
        );
    }
}
