//! Only OS administrators may replace a protected native executable or directory.
use std::{fs::File, mem::size_of, os::windows::io::AsRawHandle, ptr};
use windows_sys::Win32::{
    Foundation::{ERROR_SUCCESS, LocalFree},
    Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
        Authorization::{GetSecurityInfo, SE_FILE_OBJECT},
        DACL_SECURITY_INFORMATION, GetAce, INHERIT_ONLY_ACE, IsWellKnownSid,
        OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, WinBuiltinAdministratorsSid,
        WinLocalSystemSid,
    },
    Storage::FileSystem::{
        DELETE, FILE_APPEND_DATA, FILE_DELETE_CHILD, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA,
        FILE_WRITE_EA, WRITE_DAC, WRITE_OWNER,
    },
    System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE},
};

pub(super) fn administrator_owned(file: &File) -> bool {
    let mut owner: PSID = ptr::null_mut();
    let mut dacl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: The file is held; outputs are writable and LocalSecurity frees the allocation.
    if unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
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
        return false;
    }
    let _descriptor = LocalSecurity(descriptor);
    if !trusted_sid(owner) {
        return false;
    }
    let write = 0x1000_0000
        | 0x4000_0000
        | DELETE
        | WRITE_DAC
        | WRITE_OWNER
        | FILE_WRITE_DATA
        | FILE_APPEND_DATA
        | FILE_WRITE_EA
        | FILE_WRITE_ATTRIBUTES
        | FILE_DELETE_CHILD;
    // SAFETY: GetSecurityInfo returns a valid ACL whose entries are accessed through GetAce.
    for index in 0..unsafe { (*dacl).AceCount } {
        let mut ace = ptr::null_mut();
        if unsafe { GetAce(dacl, u32::from(index), &raw mut ace) } == 0 || ace.is_null() {
            return false;
        }
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        if u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0
            || u32::from(header.AceType) == ACCESS_DENIED_ACE_TYPE
        {
            continue;
        }
        if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE
            || usize::from(header.AceSize) < size_of::<ACCESS_ALLOWED_ACE>()
        {
            return false;
        }
        let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        if allowed.Mask & write != 0
            && !trusted_sid((&raw const allowed.SidStart).cast_mut().cast())
        {
            return false;
        }
    }
    true
}

fn trusted_sid(sid: PSID) -> bool {
    // SAFETY: All callers pass a SID held by a validated security descriptor.
    (unsafe {
        IsWellKnownSid(sid, WinLocalSystemSid) != 0
            || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
    }) || crate::private_acl::sid_string(sid)
        .is_ok_and(|sid| sid == "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464")
}

struct LocalSecurity(PSECURITY_DESCRIPTOR);
impl Drop for LocalSecurity {
    fn drop(&mut self) {
        // SAFETY: This descriptor came from GetSecurityInfo and is freed exactly once.
        unsafe {
            LocalFree(self.0);
        }
    }
}
