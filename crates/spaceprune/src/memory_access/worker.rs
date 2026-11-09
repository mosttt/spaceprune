//! A dedicated helper entrypoint accepts only one bounded, confirmed memory request.
use super::*;
use std::{
    fs::{self, File},
    io::Read,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
};

pub(super) fn run(bound: bool) -> io::Result<()> {
    if !spaceprune_access::process::is_elevated() {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    // pkexec supplies the authenticated invoking UID. Direct root invocation cannot
    // manufacture another user's authority through the serialized request body.
    let owner = std::env::var("PKEXEC_UID")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|uid| *uid != 0)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Missing authenticated memory caller",
            )
        })?;
    let self_status = fs::read_to_string("/proc/self/status")?;
    let parent = self_status
        .lines()
        .find_map(|s| s.strip_prefix("PPid:"))
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|pid| *pid > 1)
        .ok_or_else(|| io::Error::other("Missing caller process"))?;
    // pkexec replaces itself with the helper. Pin the original non-root parent directory;
    // reopening status through this descriptor cannot follow later reuse of its PID.
    let caller = File::open(format!("/proc/{parent}"))?;
    if caller.metadata()?.uid() != owner {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let mut length = [0; 4];
    io::stdin().read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as u64;
    if length > MAX_BYTES {
        return Err(io::Error::other("Memory request exceeds its limit"));
    }
    let mut bytes = vec![0; length as usize];
    io::stdin().read_exact(&mut bytes)?;
    let request: Request = serde_json::from_slice(&bytes)?;
    if bound != !request.executables.is_empty()
        || (bound && request.executables.len() != request.selected.len())
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let context = OperationContext::from_transfer(
        &request.operation,
        Duration::from_secs(180),
        spaceprune_access::ResourceBudget::default(),
    )?;
    // Drop root identity before touching a target. Kernel ptrace checks then also reject
    // a same-PID process that becomes privileged between our checks and the syscall.
    use rustix::thread::{CapabilitySet, CapabilitySets};
    rustix::thread::set_keep_capabilities(true)?;
    rustix::thread::set_thread_groups(&[])?;
    let gid = rustix::process::Gid::from_raw(caller.metadata()?.gid());
    let uid = rustix::process::Uid::from_raw(owner);
    rustix::thread::set_thread_res_gid(gid, gid, gid)?;
    rustix::thread::set_thread_res_uid(uid, uid, uid)?;
    rustix::thread::set_capabilities(
        None,
        CapabilitySets {
            effective: CapabilitySet::SYS_NICE,
            permitted: CapabilitySet::SYS_NICE,
            inheritable: CapabilitySet::empty(),
        },
    )?;
    rustix::thread::set_keep_capabilities(false)?;
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let closed = cancelled.clone();
    std::thread::spawn(move || {
        let _ = io::stdin().read(&mut [0]);
        closed.store(true, std::sync::atomic::Ordering::Release);
    });
    let check = || {
        context.check()?;
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        fs::read_to_string(format!("/proc/self/fd/{}/status", caller.as_raw_fd())).map(|_| ())
    };
    let results = if bound {
        let selected = request
            .selected
            .iter()
            .zip(&request.executables)
            .map(|(identity, executable)| BoundProcess {
                identity: identity.clone(),
                executable: executable.clone(),
            })
            .collect::<Vec<_>>();
        spaceprune_plugin_memory::linux::release_bound_for_user(&selected, owner, &check)?
    } else {
        spaceprune_plugin_memory::linux::release_for_user(&request.selected, owner, &check)?
    };
    let response = serde_json::to_vec(&Response {
        operation: context.identity(),
        results,
    })?;
    io::stdout().write_all(&response)?;
    Ok(())
}
