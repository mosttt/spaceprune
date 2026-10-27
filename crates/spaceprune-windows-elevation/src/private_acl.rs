use std::{ffi::OsStr, mem::size_of, os::windows::ffi::OsStrExt as _, path::Path, ptr};

use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, INVALID_HANDLE_VALUE, LocalFree},
    Security::{
        ACL,
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT, SetSecurityInfo,
        },
        DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, GetSecurityDescriptorControl,
        GetSecurityDescriptorDacl, GetTokenInformation, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
        TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{
        FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx, WRITE_DAC, WRITE_OWNER,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

use crate::{PrivatePathError, PrivatePathKind};

pub(super) mod logon;
pub(super) mod metadata;
mod path;
use path::open_plain_path;

pub(super) fn harden(path: &Path, kind: PrivatePathKind) -> Result<(), PrivatePathError> {
    harden_owned(path, kind, false)
}

pub(super) fn secure_created(path: &Path, kind: PrivatePathKind) -> Result<(), PrivatePathError> {
    harden_owned(path, kind, true)
}

fn harden_owned(path: &Path, kind: PrivatePathKind, created: bool) -> Result<(), PrivatePathError> {
    let handle = open_plain_path(
        path,
        kind,
        WRITE_DAC | if created { WRITE_OWNER } else { 0 },
    )?;
    let user = CurrentUserSid::load()?;
    let expected = ExpectedDescriptor::new(&user, kind)?;
    let dacl = expected.dacl()?;
    // SAFETY: The held descriptor and SID remain valid; only newly created IPC objects change owner.
    if unsafe {
        SetSecurityInfo(
            handle.0,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION
                | if created {
                    OWNER_SECURITY_INFORMATION
                } else {
                    0
                },
            if created { user.sid() } else { ptr::null_mut() },
            ptr::null_mut(),
            dacl,
            ptr::null(),
        )
    } != ERROR_SUCCESS
    {
        return Err(PrivatePathError::Unavailable);
    }
    validate_handle(&handle, kind, Some(&user.string()?)).map(|_| ())
}

pub(super) fn current_user_sid() -> Result<String, PrivatePathError> {
    CurrentUserSid::load()?.string()
}

pub(super) fn current_logon_sid() -> Result<String, PrivatePathError> {
    crate::LogonIdentity::current()?
        .logon_sid()
        .map(str::to_owned)
        .ok_or(PrivatePathError::Unavailable)
}

pub(super) fn validate(path: &Path, kind: PrivatePathKind) -> Result<(), PrivatePathError> {
    validate_for_owner(path, kind, &current_user_sid()?)
}

pub(super) fn validate_for_owner(
    path: &Path,
    kind: PrivatePathKind,
    owner: &str,
) -> Result<(), PrivatePathError> {
    validate_handle(&open_plain_path(path, kind, 0)?, kind, Some(owner)).map(|_| ())
}

pub(super) fn owner(path: &Path, kind: PrivatePathKind) -> Result<String, PrivatePathError> {
    validate_handle(&open_plain_path(path, kind, 0)?, kind, None)
}

pub(super) fn open_file_owner(
    path: &Path,
    file: &std::fs::File,
) -> Result<String, PrivatePathError> {
    validate_handle(&opened_file(path, file)?, PrivatePathKind::File, None)
}

fn opened_file(path: &Path, file: &std::fs::File) -> Result<OwnedHandle, PrivatePathError> {
    use std::os::windows::io::IntoRawHandle;
    let actual = OwnedHandle::new(
        file.try_clone()
            .map_err(|_| PrivatePathError::Unavailable)?
            .into_raw_handle(),
    )?;
    let named = open_plain_path(path, PrivatePathKind::File, 0)?;
    let identity = |handle: &OwnedHandle| -> Result<FILE_ID_INFO, PrivatePathError> {
        let mut value = FILE_ID_INFO::default();
        // SAFETY: The held file handle and correctly sized output buffer are valid.
        if unsafe {
            GetFileInformationByHandleEx(
                handle.0,
                FileIdInfo,
                (&raw mut value).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        } == 0
        {
            return Err(PrivatePathError::Unavailable);
        }
        Ok(value)
    };
    let a = identity(&actual)?;
    let b = identity(&named)?;
    if a.VolumeSerialNumber != b.VolumeSerialNumber || a.FileId.Identifier != b.FileId.Identifier {
        return Err(PrivatePathError::Ineligible);
    }
    Ok(actual)
}

fn validate_handle(
    handle: &OwnedHandle,
    kind: PrivatePathKind,
    expected_owner: Option<&str>,
) -> Result<String, PrivatePathError> {
    let (descriptor, owner, dacl) = read_security(handle)?;
    let owner = sid_string(owner)?;
    if expected_owner.is_some_and(|expected| expected != owner) {
        return Err(PrivatePathError::Ineligible);
    }
    let expected = ExpectedDescriptor::for_sid(&owner, kind)?;
    let mut control = 0_u16;
    let mut revision = 0_u32;
    // SAFETY: The descriptor is held and the control/revision outputs are writable.
    if unsafe { GetSecurityDescriptorControl(descriptor.0, &raw mut control, &raw mut revision) }
        == 0
        || control & SE_DACL_PROTECTED == 0
        || !same_acl(dacl, expected.dacl()?)
    {
        return Err(PrivatePathError::Permissions);
    }
    Ok(owner)
}

fn read_security(
    handle: &OwnedHandle,
) -> Result<(LocalDescriptor, PSID, *mut ACL), PrivatePathError> {
    let mut owner: PSID = ptr::null_mut();
    let mut dacl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: All output pointers are valid; LocalDescriptor owns the returned allocation.
    if unsafe {
        GetSecurityInfo(
            handle.0,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &raw mut owner,
            ptr::null_mut(),
            &raw mut dacl,
            ptr::null_mut(),
            &raw mut descriptor,
        )
    } != ERROR_SUCCESS
        || descriptor.is_null()
        || owner.is_null()
        || dacl.is_null()
    {
        return Err(PrivatePathError::Unavailable);
    }
    let descriptor = LocalDescriptor(descriptor);
    Ok((descriptor, owner, dacl))
}

fn same_acl(actual: *const ACL, expected: *const ACL) -> bool {
    if actual.is_null() || expected.is_null() {
        return false;
    }
    // SAFETY: 两个 ACL 都来自已验证安全描述符，`AclSize` 覆盖完整 ACL 缓冲区。
    unsafe {
        let actual_len = usize::from((*actual).AclSize);
        let expected_len = usize::from((*expected).AclSize);
        actual_len == expected_len
            && std::slice::from_raw_parts(actual.cast::<u8>(), actual_len)
                == std::slice::from_raw_parts(expected.cast::<u8>(), expected_len)
    }
}

struct CurrentUserSid {
    token: OwnedHandle,
    bytes: Vec<u8>,
}

impl CurrentUserSid {
    fn load() -> Result<Self, PrivatePathError> {
        // SAFETY: GetCurrentProcess returns a valid borrowed pseudo-handle.
        Self::for_process(unsafe { GetCurrentProcess() })
    }

    fn for_process(process: HANDLE) -> Result<Self, PrivatePathError> {
        let mut token = ptr::null_mut();
        // SAFETY: 输出句柄指针有效。
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut token) } == 0 {
            return Err(PrivatePathError::Unavailable);
        }
        let token = OwnedHandle::new(token)?;
        let mut needed = 0_u32;
        // SAFETY: 第一次调用只查询所需长度。
        unsafe {
            GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &raw mut needed);
        }
        if needed < u32::try_from(size_of::<TOKEN_USER>()).unwrap_or(u32::MAX) {
            return Err(PrivatePathError::Unavailable);
        }
        let mut bytes =
            vec![0_u8; usize::try_from(needed).map_err(|_| PrivatePathError::Unavailable)?];
        // SAFETY: 缓冲区具有系统返回的尺寸，输出长度指针有效。
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                bytes.as_mut_ptr().cast(),
                needed,
                &raw mut needed,
            )
        } == 0
        {
            return Err(PrivatePathError::Unavailable);
        }
        Ok(Self { token, bytes })
    }

    fn sid(&self) -> PSID {
        let _ = &self.token;
        // SAFETY: `bytes` 由 `GetTokenInformation(TokenUser)` 填充并在 self 生命周期内稳定。
        unsafe {
            self.bytes
                .as_ptr()
                .cast::<TOKEN_USER>()
                .read_unaligned()
                .User
                .Sid
        }
    }

    fn string(&self) -> Result<String, PrivatePathError> {
        sid_string(self.sid())
    }
}

pub(super) fn sid_string(sid: PSID) -> Result<String, PrivatePathError> {
    let mut value = ptr::null_mut();
    // SAFETY: SID有效；系统分配的字符串由 `LocalWide` 释放。
    if unsafe { ConvertSidToStringSidW(sid, &raw mut value) } == 0 || value.is_null() {
        return Err(PrivatePathError::Unavailable);
    }
    let value = LocalWide(value);
    let mut len = 0_usize;
    // SAFETY: Windows 保证返回 NUL 结尾字符串。
    while unsafe { *value.0.add(len) } != 0 {
        len = len.checked_add(1).ok_or(PrivatePathError::Unavailable)?;
    }
    // SAFETY: 上述循环已定位终止符，范围内为有效 UTF-16 单元。
    String::from_utf16(unsafe { std::slice::from_raw_parts(value.0, len) })
        .map_err(|_| PrivatePathError::Unavailable)
}

struct ExpectedDescriptor(PSECURITY_DESCRIPTOR);

impl ExpectedDescriptor {
    fn new(user: &CurrentUserSid, kind: PrivatePathKind) -> Result<Self, PrivatePathError> {
        Self::for_sid(&user.string()?, kind)
    }

    fn for_sid(owner: &str, kind: PrivatePathKind) -> Result<Self, PrivatePathError> {
        let flags = if kind == PrivatePathKind::Directory {
            "OICI"
        } else {
            ""
        };
        let sddl = format!(
            "D:P(A;{flags};FA;;;SY)(A;{flags};FA;;;BA)(A;{flags};FA;;;{})",
            owner
        );
        Self::from_sddl(&sddl)
    }

    fn from_sddl(sddl: &str) -> Result<Self, PrivatePathError> {
        let sddl = wide_null(OsStr::new(sddl));
        let mut descriptor = ptr::null_mut();
        // SAFETY: SDDL 缓冲区 NUL 结尾；输出描述符由本类型释放。
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                ptr::null_mut(),
            )
        } == 0
            || descriptor.is_null()
        {
            return Err(PrivatePathError::Unavailable);
        }
        Ok(Self(descriptor))
    }

    fn dacl(&self) -> Result<*const ACL, PrivatePathError> {
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl = ptr::null_mut();
        // SAFETY: 描述符有效，所有输出指针可写。
        if unsafe {
            GetSecurityDescriptorDacl(self.0, &raw mut present, &raw mut dacl, &raw mut defaulted)
        } == 0
            || present == 0
            || dacl.is_null()
        {
            return Err(PrivatePathError::Unavailable);
        }
        Ok(dacl)
    }
}

impl Drop for ExpectedDescriptor {
    fn drop(&mut self) {
        // SAFETY: 描述符由 LocalAlloc 系列 API 分配且仅在此释放一次。
        let _ = unsafe { LocalFree(self.0) };
    }
}

struct LocalDescriptor(PSECURITY_DESCRIPTOR);

impl Drop for LocalDescriptor {
    fn drop(&mut self) {
        // SAFETY: 描述符由 `GetSecurityInfo` 分配且仅在此释放一次。
        let _ = unsafe { LocalFree(self.0) };
    }
}

struct LocalWide(*mut u16);

impl Drop for LocalWide {
    fn drop(&mut self) {
        // SAFETY: 字符串由 `ConvertSidToStringSidW` 分配且仅在此释放一次。
        let _ = unsafe { LocalFree(self.0.cast()) };
    }
}

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(handle: HANDLE) -> Result<Self, PrivatePathError> {
        (!handle.is_null() && handle != INVALID_HANDLE_VALUE)
            .then_some(Self(handle))
            .ok_or(PrivatePathError::Unavailable)
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: 句柄由本类型独占且只关闭一次。
        let _ = unsafe { CloseHandle(self.0) };
        self.0 = ptr::null_mut();
    }
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests;
