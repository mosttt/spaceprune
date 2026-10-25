//! Metadata keeps its existing elevated LocalSystem entry point without weakening login IPC.

use super::*;

/// Validated metadata IPC scope: an original login or an OS-verified LocalSystem process.
/// The system variant still requires a held host-process proof; its ACL is not sufficient alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataIpcIdentity(Scope);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Scope {
    Logon(logon::LogonIpcIdentity),
    LocalSystemProcess,
}

impl MetadataIpcIdentity {
    /// Original account used for the metadata request's profile and owner checks.
    pub fn account_sid(&self) -> &str {
        match &self.0 {
            Scope::Logon(identity) => identity.account_sid(),
            Scope::LocalSystemProcess => "S-1-5-18",
        }
    }

    pub(crate) fn matches_host(&self, host: &crate::LogonIdentity) -> bool {
        match &self.0 {
            Scope::Logon(identity) => {
                host.account_sid() == identity.account_sid()
                    && host.logon_sid() == Some(identity.logon_sid())
            }
            Scope::LocalSystemProcess => host.is_system_process(),
        }
    }
}

pub(crate) fn secure_created(path: &Path, kind: PrivatePathKind) -> Result<(), PrivatePathError> {
    let identity = crate::LogonIdentity::current()?;
    if identity.logon_sid().is_some() {
        return logon::secure_created(path, kind);
    }
    if !identity.is_system_process() {
        return Err(PrivatePathError::Ineligible);
    }
    let user = CurrentUserSid::load()?;
    let handle = open_plain_path(path, kind, WRITE_DAC | WRITE_OWNER)?;
    let expected = system_descriptor(kind)?;
    // SAFETY: Both the current token SID and descriptor remain held. Only this operation's
    // new metadata objects are changed; ordinary user paths retain their existing API.
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
    if validate_handle(&handle, kind)?.matches_host(&identity) {
        Ok(())
    } else {
        Err(PrivatePathError::Ineligible)
    }
}

pub(crate) fn identity(
    path: &Path,
    kind: PrivatePathKind,
) -> Result<MetadataIpcIdentity, PrivatePathError> {
    validate_handle(&open_plain_path(path, kind, 0)?, kind)
}

pub(crate) fn file_identity(
    path: &Path,
    file: &std::fs::File,
) -> Result<MetadataIpcIdentity, PrivatePathError> {
    validate_handle(&opened_file(path, file)?, PrivatePathKind::File)
}

fn system_descriptor(kind: PrivatePathKind) -> Result<ExpectedDescriptor, PrivatePathError> {
    let flags = if kind == PrivatePathKind::Directory {
        "OICI"
    } else {
        ""
    };
    ExpectedDescriptor::from_sddl(&format!(
        "D:P(A;{flags};FA;;;SY)(A;{flags};FA;;;BA)(A;{flags};RC;;;OW)"
    ))
}

fn validate_handle(
    handle: &OwnedHandle,
    kind: PrivatePathKind,
) -> Result<MetadataIpcIdentity, PrivatePathError> {
    let (security, owner, dacl) = read_security(handle)?;
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: The held OS security descriptor and writable outputs remain valid.
    if unsafe { GetSecurityDescriptorControl(security.0, &raw mut control, &raw mut revision) } == 0
        || control & SE_DACL_PROTECTED == 0
    {
        return Err(PrivatePathError::Permissions);
    }
    if sid_string(owner)? == "S-1-5-18" && same_acl(dacl, system_descriptor(kind)?.dacl()?) {
        return Ok(MetadataIpcIdentity(Scope::LocalSystemProcess));
    }
    logon::validate_handle(handle, kind).map(|identity| MetadataIpcIdentity(Scope::Logon(identity)))
}

#[cfg(test)]
mod tests;
