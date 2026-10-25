//! Login-specific ACLs for ephemeral IPC, separate from account-owned persistent data.

use super::*;
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, GetAce, GetLengthSid, IsValidSid,
};

/// Validated owner and login trustee of an ephemeral IPC object; not wire-authoritative data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogonIpcIdentity {
    account_sid: String,
    logon_sid: String,
}

impl LogonIpcIdentity {
    /// The original account that owns this IPC object and its profile scope.
    pub fn account_sid(&self) -> &str {
        &self.account_sid
    }
    /// The login SID allowed to access the IPC object alongside SYSTEM and administrators.
    pub fn logon_sid(&self) -> &str {
        &self.logon_sid
    }
}

pub(crate) fn secure_created(path: &Path, kind: PrivatePathKind) -> Result<(), PrivatePathError> {
    let identity = crate::LogonIdentity::current()?;
    let logon = identity.logon_sid().ok_or(PrivatePathError::Unavailable)?;
    let user = CurrentUserSid::load()?;
    let handle = open_plain_path(path, kind, WRITE_DAC | WRITE_OWNER)?;
    let expected = descriptor(logon, kind)?;
    // SAFETY: The token SID and security descriptor stay alive; this only changes new IPC objects.
    if unsafe {
        SetSecurityInfo(
            handle.0,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION
                | OWNER_SECURITY_INFORMATION,
            user.sid(),
            ptr::null_mut(),
            expected.dacl()?,
            ptr::null(),
        )
    } != ERROR_SUCCESS
    {
        return Err(PrivatePathError::Unavailable);
    }
    let actual = validate_handle(&handle, kind)?;
    if actual.account_sid != identity.account_sid() || actual.logon_sid != logon {
        return Err(PrivatePathError::Ineligible);
    }
    Ok(())
}

pub(crate) fn identity(
    path: &Path,
    kind: PrivatePathKind,
) -> Result<LogonIpcIdentity, PrivatePathError> {
    validate_handle(&open_plain_path(path, kind, 0)?, kind)
}

pub(crate) fn file_identity(
    path: &Path,
    file: &std::fs::File,
) -> Result<LogonIpcIdentity, PrivatePathError> {
    validate_handle(&opened_file(path, file)?, PrivatePathKind::File)
}

fn descriptor(logon: &str, kind: PrivatePathKind) -> Result<ExpectedDescriptor, PrivatePathError> {
    let flags = if kind == PrivatePathKind::Directory {
        "OICI"
    } else {
        ""
    };
    // OWNER RIGHTS suppresses the account owner's implicit WRITE_DAC, which would otherwise
    // let another login of that same account rewrite this login-specific DACL.
    ExpectedDescriptor::from_sddl(&format!(
        "D:P(A;{flags};FA;;;SY)(A;{flags};FA;;;BA)(A;{flags};FA;;;{logon})(A;{flags};RC;;;OW)"
    ))
}

pub(super) fn validate_handle(
    handle: &OwnedHandle,
    kind: PrivatePathKind,
) -> Result<LogonIpcIdentity, PrivatePathError> {
    let (security, owner, dacl) = read_security(handle)?;
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: The OS-owned security descriptor remains alive and outputs are writable.
    if unsafe { GetSecurityDescriptorControl(security.0, &raw mut control, &raw mut revision) } == 0
        || control & SE_DACL_PROTECTED == 0
    {
        return Err(PrivatePathError::Permissions);
    }
    let logon = logon_trustee(dacl)?;
    if !same_acl(dacl, descriptor(&logon, kind)?.dacl()?) {
        return Err(PrivatePathError::Permissions);
    }
    Ok(LogonIpcIdentity {
        account_sid: sid_string(owner)?,
        logon_sid: logon,
    })
}

fn logon_trustee(dacl: *mut ACL) -> Result<String, PrivatePathError> {
    // SAFETY: GetSecurityInfo returned a complete ACL which is held by the caller.
    if unsafe { (*dacl).AceCount } != 4 {
        return Err(PrivatePathError::Permissions);
    }
    let mut raw = ptr::null_mut();
    // SAFETY: Index 2 is within the checked ACE count; GetAce validates ACL traversal.
    if unsafe { GetAce(dacl, 2, &raw mut raw) } == 0 || raw.is_null() {
        return Err(PrivatePathError::Permissions);
    }
    // SAFETY: GetAce returned a valid ACE header within the held descriptor.
    let header = unsafe { raw.cast::<ACE_HEADER>().read_unaligned() };
    let offset = std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart);
    if header.AceType != 0 || usize::from(header.AceSize) < offset + 8 {
        return Err(PrivatePathError::Permissions);
    }
    // SAFETY: The ACE has the checked SID header size. IsValidSid checks its SID layout.
    let sid = unsafe { raw.cast::<u8>().add(offset) }.cast();
    if unsafe { IsValidSid(sid) } == 0
        || unsafe { GetLengthSid(sid) } as usize > usize::from(header.AceSize) - offset
    {
        return Err(PrivatePathError::Permissions);
    }
    let value = sid_string(sid)?;
    let mut parts = value
        .strip_prefix("S-1-5-5-")
        .ok_or(PrivatePathError::Permissions)?
        .split('-');
    if parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .is_none()
        || parts
            .next()
            .and_then(|part| part.parse::<u32>().ok())
            .is_none()
        || parts.next().is_some()
    {
        return Err(PrivatePathError::Permissions);
    }
    Ok(value)
}

#[cfg(test)]
mod tests;
