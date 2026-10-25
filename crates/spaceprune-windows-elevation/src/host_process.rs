//! Bind a native session to the invoking process, including PID reuse, account and logon identity.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::{
    Foundation::{FILETIME, HANDLE, WAIT_TIMEOUT},
    System::Threading::{
        GetCurrentProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SYNCHRONIZE, WaitForSingleObject,
    },
};

/// A held process identity used to end an elevated session when its host exits.
pub struct HostProcess {
    handle: OwnedHandle,
    identity: crate::LogonIdentity,
}

impl HostProcess {
    /// Attach only to the original process owned by the private IPC creator.
    pub fn observe(pid: u32, created: u64, owner: &str) -> Option<Self> {
        // SAFETY: The PID is an OS identifier and the returned handle is immediately owned.
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if raw.is_null() {
            return None;
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let identity = crate::LogonIdentity::for_process(raw).ok()?;
        if creation_time(raw)? != created || identity.account_sid() != owner {
            return None;
        }
        Some(Self { handle, identity })
    }

    /// Bind a temporary IPC session to the original account and login, not the elevated account.
    pub fn observe_logon(pid: u32, created: u64, ipc: &crate::LogonIpcIdentity) -> Option<Self> {
        let host = Self::observe(pid, created, ipc.account_sid())?;
        (host.identity.logon_sid() == Some(ipc.logon_sid())).then_some(host)
    }

    /// Bind metadata IPC to the original login or verify the held LocalSystem process token.
    pub fn observe_metadata(
        pid: u32,
        created: u64,
        ipc: &crate::MetadataIpcIdentity,
    ) -> Option<Self> {
        let host = Self::observe(pid, created, ipc.account_sid())?;
        ipc.matches_host(&host.identity).then_some(host)
    }

    /// Observe actual process exit and recheck its primary token against the captured identity.
    pub fn is_running(&self) -> bool {
        // SAFETY: The process handle remains owned by self; a zero wait never blocks.
        (unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) }) == WAIT_TIMEOUT
            && crate::LogonIdentity::for_process(self.handle.as_raw_handle())
                .is_ok_and(|identity| identity == self.identity)
    }
}

/// Return the current process creation identity from the OS, not the wall clock or environment.
pub fn current_process_creation_time() -> Option<u64> {
    // SAFETY: The current process pseudo-handle stays valid and must not be closed.
    creation_time(unsafe { GetCurrentProcess() })
}

fn creation_time(handle: HANDLE) -> Option<u64> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: All FILETIME outputs are initialized, writable and correctly sized.
    if unsafe {
        GetProcessTimes(
            handle,
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    } == 0
    {
        return None;
    }
    Some((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_binding_rejects_another_creation_identity_or_account() {
        let created = current_process_creation_time().unwrap();
        let owner = crate::current_user_sid().unwrap();
        assert!(
            HostProcess::observe(std::process::id(), created, &owner)
                .unwrap()
                .is_running()
        );
        assert!(HostProcess::observe(std::process::id(), created ^ 1, &owner).is_none());
        assert!(HostProcess::observe(std::process::id(), created, "S-1-0-0").is_none());
    }
}
