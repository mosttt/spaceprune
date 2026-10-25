//! Darwin identity comes from the kernel Mach audit trailer, including process incarnation.
#![allow(unsafe_code)]

use super::*;
use mach2::{
    message::audit_token_t as AuditToken,
    task::task_info,
    task_info::{TASK_AUDIT_TOKEN, TASK_AUDIT_TOKEN_COUNT},
    traps::mach_task_self,
};
use std::io;

#[link(name = "bsm")]
unsafe extern "C" {
    fn audit_token_to_au32(
        token: AuditToken,
        audit_uid: *mut libc::uid_t,
        effective_uid: *mut libc::uid_t,
        effective_gid: *mut libc::gid_t,
        real_uid: *mut libc::uid_t,
        real_gid: *mut libc::gid_t,
        pid: *mut libc::pid_t,
        audit_session: *mut libc::pid_t,
        terminal: *mut libc::c_void,
    );
}

pub(super) use super::principal::ProcessIdentity;

impl ProcessIdentity {
    fn from_token(token: AuditToken) -> io::Result<ProcessIdentity> {
        let (mut pid, mut effective_uid, mut real_uid, mut audit_session) = (0, 0, 0, 0);
        // SAFETY: libbsm decodes the kernel token; unused outputs may be NULL per its API.
        unsafe {
            audit_token_to_au32(
                token,
                std::ptr::null_mut(),
                &mut effective_uid,
                std::ptr::null_mut(),
                &mut real_uid,
                std::ptr::null_mut(),
                &mut pid,
                &mut audit_session,
                std::ptr::null_mut(),
            );
        }
        if pid <= 0 {
            return Err(io::Error::other("Invalid kernel peer PID"));
        }
        Ok(ProcessIdentity {
            pid: pid as u32,
            effective_uid,
            real_uid,
            audit_session,
        })
    }
}

#[derive(Clone, Copy)]
pub(super) struct Peer {
    token: AuditToken,
    pub(super) identity: ProcessIdentity,
}

impl Peer {
    pub(super) fn from_token(token: AuditToken) -> io::Result<Self> {
        Ok(Self {
            identity: ProcessIdentity::from_token(token)?,
            token,
        })
    }

    pub(super) fn same_process(&self, other: &Self) -> bool {
        self.token == other.token
    }

    pub(super) fn check_parent(&self, expected: ProcessIdentity) -> Result<(), PermissionOutcome> {
        if !self.identity.same_login(expected) {
            return Err(PermissionOutcome::HelperUntrusted);
        }
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        // SAFETY: libproc fills a fixed public BSD process-info buffer. Callers revalidate
        // this peer's original audit token after this PID query to reject reuse/exec races.
        let written = unsafe {
            libc::proc_pidinfo(
                self.identity.pid as i32,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if written != size {
            return Err(PermissionOutcome::HelperUntrusted);
        }
        // SAFETY: Successful proc_pidinfo initialized the complete buffer above.
        let info = unsafe { info.assume_init() };
        if info.pbi_pid != self.identity.pid
            || info.pbi_ppid != expected.pid
            || info.pbi_uid != self.identity.effective_uid
            || info.pbi_ruid != self.identity.real_uid
        {
            return Err(PermissionOutcome::HelperUntrusted);
        }
        Ok(())
    }

    pub(super) fn verified_hash(
        &self,
        team: &str,
        identifiers: &[&str],
    ) -> Result<Vec<u8>, PermissionOutcome> {
        let bytes: Vec<_> = self
            .token
            .val
            .into_iter()
            .flat_map(u32::to_ne_bytes)
            .collect();
        signing::guest_hash(&bytes, team, identifiers)
    }

    pub(super) fn verify_code(
        &self,
        team: &str,
        identifiers: &[&str],
        expected_hash: Option<&[u8]>,
    ) -> Result<(), PermissionOutcome> {
        let bytes: Vec<_> = self
            .token
            .val
            .into_iter()
            .flat_map(u32::to_ne_bytes)
            .collect();
        signing::verify_guest(&bytes, team, identifiers, expected_hash)
    }
}

pub(super) fn current_identity() -> io::Result<ProcessIdentity> {
    let mut token = AuditToken { val: [0; 8] };
    let mut words = TASK_AUDIT_TOKEN_COUNT;
    // SAFETY: TASK_AUDIT_TOKEN is public Mach flavor 15; the kernel fills eight integer_t words.
    if unsafe {
        task_info(
            mach_task_self(),
            TASK_AUDIT_TOKEN,
            token.val.as_mut_ptr().cast(),
            &mut words,
        )
    } != libc::KERN_SUCCESS
    {
        return Err(io::Error::other(
            "Could not inspect current Mach audit token",
        ));
    }
    if words != TASK_AUDIT_TOKEN_COUNT {
        return Err(io::Error::other("Invalid current audit-token size"));
    }
    ProcessIdentity::from_token(token)
}
