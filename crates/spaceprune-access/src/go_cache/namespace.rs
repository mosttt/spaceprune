//! Single-threaded bootstrap, called only from a fresh narrow worker entrypoint.
use std::io;

pub(super) fn enter() -> io::Result<()> {
    let uid = rustix::process::geteuid().as_raw();
    let gid = rustix::process::getegid().as_raw();
    if uid == 0 {
        return Err(io::Error::other(
            "The ordinary Go cache worker cannot run as host root",
        ));
    }
    // SAFETY: Only NEWUSER is requested. No descriptor table, address space or other
    // Rust thread-shared state is unshared. This entry is dispatched before host threads.
    unsafe { rustix::thread::unshare_unsafe(rustix::thread::UnshareFlags::NEWUSER) }
        .map_err(io::Error::from)?;
    std::fs::write("/proc/self/setgroups", "deny")?;
    std::fs::write("/proc/self/uid_map", format!("0 {uid} 1\n"))?;
    std::fs::write("/proc/self/gid_map", format!("0 {gid} 1\n"))?;
    // No exec follows mapping: the worker retains namespace DAC capabilities. Its UID 0
    // never reaches a general handler, authorization lease, installer or host-root session.
    Ok(())
}
