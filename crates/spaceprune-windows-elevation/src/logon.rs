//! Primary-token identity; never inferred from environment variables or serialized claims.

use crate::PrivatePathError;
use std::{
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
};
use windows_sys::Win32::{
    Foundation::HANDLE,
    Security::{
        GetTokenInformation, SID_AND_ATTRIBUTES, TOKEN_ELEVATION, TOKEN_GROUPS, TOKEN_QUERY,
        TOKEN_STATISTICS, TOKEN_USER, TokenElevation, TokenGroups, TokenSessionId, TokenStatistics,
        TokenUser,
    },
    System::{
        SystemServices::SE_GROUP_LOGON_ID,
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

/// Identity of the primary token of a real process, kept only in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogonIdentity {
    account_sid: String,
    logon_sid: Option<String>,
    authentication_id: u64,
    session_id: u32,
    elevated: bool,
}

impl LogonIdentity {
    /// Query the current process primary token rather than a thread impersonation token.
    pub fn current() -> Result<Self, PrivatePathError> {
        // SAFETY: The pseudo-handle is valid and is not owned or closed here.
        Self::for_process(unsafe { GetCurrentProcess() })
    }

    /// Account SID used for profile and owner checks, not for cross-logon IPC access.
    pub fn account_sid(&self) -> &str {
        &self.account_sid
    }
    /// Login SID, absent for token classes that do not carry a logon group.
    pub fn logon_sid(&self) -> Option<&str> {
        self.logon_sid.as_deref()
    }
    /// AuthenticationId LUID, with its original high and low 32-bit parts preserved.
    pub fn authentication_id(&self) -> u64 {
        self.authentication_id
    }
    /// Windows session ID from the token; not the active physical console session.
    pub fn session_id(&self) -> u32 {
        self.session_id
    }

    /// Whether the OS token is the elevated, session-zero LocalSystem process context.
    /// Missing logon groups alone never authorize an account-scoped IPC fallback.
    pub fn is_system_process(&self) -> bool {
        self.logon_sid.is_none()
            && self.account_sid == "S-1-5-18"
            && self.authentication_id == 0x3e7
            && self.session_id == 0
            && self.elevated
    }

    pub(super) fn for_process(process: HANDLE) -> Result<Self, PrivatePathError> {
        let mut raw = ptr::null_mut();
        // SAFETY: The process is held by the caller and the output pointer is writable.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut raw) } == 0 {
            return Err(PrivatePathError::Unavailable);
        }
        // SAFETY: OpenProcessToken returned an owned handle.
        let token = unsafe { OwnedHandle::from_raw_handle(raw) };
        let user = token_bytes(&token, TokenUser)?;
        if user.len() < size_of::<TOKEN_USER>() {
            return Err(PrivatePathError::Unavailable);
        }
        // SAFETY: The OS initialized a complete TOKEN_USER; the SID is held in its buffer.
        let sid = unsafe { user.as_ptr().cast::<TOKEN_USER>().read_unaligned().User.Sid };
        let account_sid = crate::private_acl::sid_string(sid)?;
        let groups = token_bytes(&token, TokenGroups)?;
        let offset = std::mem::offset_of!(TOKEN_GROUPS, Groups);
        if groups.len() < offset {
            return Err(PrivatePathError::Unavailable);
        }
        // SAFETY: The checked buffer starts with a DWORD count.
        let count = unsafe { groups.as_ptr().cast::<u32>().read_unaligned() } as usize;
        if count > (groups.len() - offset) / size_of::<SID_AND_ATTRIBUTES>() {
            return Err(PrivatePathError::Unavailable);
        }
        let mut logon_sid = None;
        for index in 0..count {
            // SAFETY: Each record lies within the initialized TOKEN_GROUPS buffer.
            let group = unsafe {
                groups
                    .as_ptr()
                    .add(offset + index * size_of::<SID_AND_ATTRIBUTES>())
                    .cast::<SID_AND_ATTRIBUTES>()
                    .read_unaligned()
            };
            if group.Attributes & SE_GROUP_LOGON_ID as u32 == SE_GROUP_LOGON_ID as u32 {
                logon_sid = Some(crate::private_acl::sid_string(group.Sid)?);
                break;
            }
        }
        let statistics = token_bytes(&token, TokenStatistics)?;
        if statistics.len() < size_of::<TOKEN_STATISTICS>() {
            return Err(PrivatePathError::Unavailable);
        }
        // SAFETY: The OS initialized a complete TOKEN_STATISTICS.
        let statistics = unsafe {
            statistics
                .as_ptr()
                .cast::<TOKEN_STATISTICS>()
                .read_unaligned()
        };
        let session = token_bytes(&token, TokenSessionId)?;
        if session.len() != size_of::<u32>() {
            return Err(PrivatePathError::Unavailable);
        }
        // SAFETY: TokenSessionId is exactly one initialized DWORD.
        let session_id = unsafe { session.as_ptr().cast::<u32>().read_unaligned() };
        let elevation = token_bytes(&token, TokenElevation)?;
        if elevation.len() != size_of::<TOKEN_ELEVATION>() {
            return Err(PrivatePathError::Unavailable);
        }
        // SAFETY: TokenElevation initialized exactly one TOKEN_ELEVATION.
        let elevated = unsafe {
            elevation
                .as_ptr()
                .cast::<TOKEN_ELEVATION>()
                .read_unaligned()
                .TokenIsElevated
                != 0
        };
        Ok(Self {
            account_sid,
            logon_sid,
            session_id,
            elevated,
            authentication_id: ((statistics.AuthenticationId.HighPart as u32 as u64) << 32)
                | u64::from(statistics.AuthenticationId.LowPart),
        })
    }
}

fn token_bytes(
    token: &OwnedHandle,
    class: windows_sys::Win32::Security::TOKEN_INFORMATION_CLASS,
) -> Result<Vec<u8>, PrivatePathError> {
    let mut needed = 0;
    // SAFETY: Query the required capacity; no output buffer is supplied yet.
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            class,
            ptr::null_mut(),
            0,
            &raw mut needed,
        );
    }
    if needed == 0 {
        return Err(PrivatePathError::Unavailable);
    }
    let mut bytes = vec![0; needed as usize];
    // SAFETY: The allocated buffer has the requested capacity and stays alive during the call.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            class,
            bytes.as_mut_ptr().cast(),
            needed,
            &raw mut needed,
        )
    } == 0
    {
        return Err(PrivatePathError::Unavailable);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_logon_group_cannot_turn_an_account_or_interactive_system_token_into_process_scope() {
        let system = LogonIdentity {
            account_sid: "S-1-5-18".into(),
            logon_sid: None,
            authentication_id: 0x3e7,
            session_id: 0,
            elevated: true,
        };
        assert!(system.is_system_process());
        let cases = [
            LogonIdentity {
                account_sid: "S-1-5-21-1-2-3-1000".into(),
                ..system.clone()
            },
            LogonIdentity {
                authentication_id: 0x1234,
                ..system.clone()
            },
            LogonIdentity {
                session_id: 1,
                ..system.clone()
            },
            LogonIdentity {
                elevated: false,
                ..system.clone()
            },
            LogonIdentity {
                logon_sid: Some("S-1-5-5-0-999".into()),
                ..system.clone()
            },
        ];
        assert!(cases.iter().all(|token| !token.is_system_process()));
    }
}
