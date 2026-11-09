//! Linux process inspection and PID-file-descriptor-bound page-out advice.
use super::*;
use std::{
    ffi::CStr,
    fs::{self, File, OpenOptions},
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
};

#[path = "linux/procfs.rs"]
mod procfs;
use procfs::{private_bytes, ranges, start_ticks, status_value};

/// Current effective UID, used only by the ordinary caller before authorization.
pub fn current_user() -> u32 {
    // SAFETY: geteuid takes no pointers and has no side effects.
    unsafe { libc::geteuid() }
}

/// Whether the caller already has the kernel capability required for remote page-out advice.
pub fn can_reclaim() -> bool {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|line| line.strip_prefix("CapEff:\t"))
                .and_then(|v| u64::from_str_radix(v.trim(), 16).ok())
        })
        .is_some_and(|caps| caps & (1 << 23) != 0)
}

fn owned_fd(raw: libc::c_long) -> io::Result<OwnedFd> {
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: Only a successful descriptor-returning syscall enters this ownership boundary.
    Ok(unsafe { OwnedFd::from_raw_fd(raw as i32) })
}

fn read_at(dir: &File, name: &CStr, limit: u64) -> io::Result<String> {
    // SAFETY: The owned directory and constant NUL-terminated child name remain valid.
    let fd = owned_fd(unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    } as libc::c_long)?;
    let mut text = String::new();
    File::from(fd).take(limit + 1).read_to_string(&mut text)?;
    if text.len() as u64 > limit {
        return Err(io::Error::other("Process metadata exceeds its limit"));
    }
    Ok(text)
}

struct Process {
    pid: u32,
    pidfd: OwnedFd,
    directory: File,
}
impl Process {
    fn open(pid: u32, owner: u32) -> io::Result<Self> {
        if owner == 0 || pid <= 1 || pid == std::process::id() || pid > i32::MAX as u32 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "This process is excluded",
            ));
        }
        // SAFETY: pidfd_open takes an integer PID and zero flags; no caller pointers.
        let pidfd = owned_fd(unsafe { libc::syscall(libc::SYS_pidfd_open, pid as i32, 0u32) })?;
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(format!("/proc/{pid}"))?;
        if directory.metadata()?.uid() != owner {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Process belongs to another user",
            ));
        }
        let process = Self {
            pid,
            pidfd,
            directory,
        };
        process.check_owner(owner)?;
        process.check_alive()?;
        Ok(process)
    }
    fn check_alive(&self) -> io::Result<()> {
        // SAFETY: Signal zero checks liveness/permissions on the owned pidfd; no signal is delivered.
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.pidfd.as_raw_fd(),
                0,
                std::ptr::null::<libc::siginfo_t>(),
                0u32,
            )
        };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    fn status(&self) -> io::Result<String> {
        read_at(&self.directory, c"status", 128 * 1024)
    }
    fn check_owner(&self, owner: u32) -> io::Result<()> {
        let status = self.status()?;
        let uid = status
            .lines()
            .find_map(|s| s.strip_prefix("Uid:"))
            .ok_or_else(|| io::Error::other("Process identity unavailable"))?;
        let ids = uid
            .split_whitespace()
            .map(str::parse::<u32>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(io::Error::other)?;
        let caps = status
            .lines()
            .find_map(|s| s.strip_prefix("CapEff:"))
            .and_then(|s| u64::from_str_radix(s.trim(), 16).ok());
        if ids.len() != 4 || ids.iter().any(|id| *id != owner) || caps != Some(0) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Privileged or other-user process is excluded",
            ));
        }
        Ok(())
    }
    fn identity(&self) -> io::Result<ProcessIdentity> {
        let stat = read_at(&self.directory, c"stat", 128 * 1024)?;
        let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
        Ok(ProcessIdentity {
            pid: self.pid,
            started_at: format!("{}:{}", boot.trim(), start_ticks(&stat)?),
        })
    }
    fn executable(&self) -> io::Result<String> {
        let path = fs::read_link(format!("/proc/self/fd/{}/exe", self.directory.as_raw_fd()))?;
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(|| io::Error::other("Executable path is not valid Unicode"))
    }
    fn check_executable(&self, expected: Option<&str>) -> io::Result<()> {
        if let Some(expected) = expected
            && self.executable()? != expected
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Selected executable has changed",
            ));
        }
        Ok(())
    }
    fn resident(&self) -> io::Result<u64> {
        status_value(&self.status()?, "VmRSS:")
    }
}

pub(super) fn inspect() -> io::Result<MemorySnapshot> {
    let memory = fs::read_to_string("/proc/meminfo")?;
    let total_bytes = status_value(&memory, "MemTotal:")?;
    let available_bytes = status_value(&memory, "MemAvailable:")?;
    let owner = current_user();
    let mut processes = Vec::new();
    for entry in fs::read_dir("/proc")? {
        let Ok(entry) = entry else { continue };
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse().ok()) else {
            continue;
        };
        let result = (|| -> io::Result<ProcessMemory> {
            let process = Process::open(pid, owner)?;
            let identity = process.identity()?;
            let status = process.status()?;
            let name = status
                .lines()
                .find_map(|s| s.strip_prefix("Name:"))
                .unwrap_or("process")
                .trim()
                .to_owned();
            let resident_bytes = status_value(&status, "VmRSS:")?;
            let private_bytes =
                private_bytes(&read_at(&process.directory, c"smaps_rollup", 128 * 1024)?)?;
            process.check_alive()?;
            Ok(ProcessMemory {
                identity,
                name,
                executable: process.executable().ok(),
                resident_bytes,
                private_bytes,
            })
        })();
        if let Ok(process) = result {
            processes.push(process);
        }
    }
    processes.sort_by_key(|p| std::cmp::Reverse(p.resident_bytes));
    Ok(MemorySnapshot {
        total_bytes,
        available_bytes,
        processes,
    })
}

/// Advise only selected processes belonging to the authenticated invoking user.
/// The caller supplies cooperative cancellation/deadline checks between bounded chunks.
pub fn release_for_user(
    selected: &[ProcessIdentity],
    owner: u32,
    check: &dyn Fn() -> io::Result<()>,
) -> io::Result<Vec<MemoryRelease>> {
    if selected.is_empty()
        || selected.len() > 128
        || selected
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != selected.len()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Select between 1 and 128 distinct processes",
        ));
    }
    if !can_reclaim() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Linux process memory maintenance requires system authorization",
        ));
    }
    if current_user() != 0 && current_user() != owner {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Process owner does not match caller",
        ));
    }
    Ok(selected
        .iter()
        .map(|identity| release_one(identity, owner, None, check))
        .collect())
}

/// Release current-user selections after checking their executable path on the pinned process.
pub fn release_bound_for_user(
    selected: &[BoundProcess],
    owner: u32,
    check: &dyn Fn() -> io::Result<()>,
) -> io::Result<Vec<MemoryRelease>> {
    super::validate_bound(selected)?;
    if !can_reclaim() || (current_user() != 0 && current_user() != owner) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Linux process memory maintenance requires authorized caller capability",
        ));
    }
    Ok(selected
        .iter()
        .map(|selection| {
            release_one(
                &selection.identity,
                owner,
                Some(&selection.executable),
                check,
            )
        })
        .collect())
}

fn release_one(
    selected: &ProcessIdentity,
    owner: u32,
    executable: Option<&str>,
    check: &dyn Fn() -> io::Result<()>,
) -> MemoryRelease {
    let mut process = None;
    let mut before = None;
    let mut advised = false;
    let result = (|| -> io::Result<()> {
        check()?;
        let opened = Process::open(selected.pid, owner)?;
        if opened.identity()? != *selected {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Selected process has exited or changed",
            ));
        }
        opened.check_executable(executable)?;
        before = Some(opened.resident()?);
        let regions = ranges(&read_at(&opened.directory, c"maps", 8 * 1024 * 1024)?)?;
        process = Some(opened);
        let opened = process.as_ref().unwrap();
        for (start, end) in regions {
            let mut address = start;
            while address < end {
                check()?;
                opened.check_owner(owner)?;
                opened.check_alive()?;
                opened.check_executable(executable)?;
                let length = (end - address).min(16 * 1024 * 1024);
                let range = libc::iovec {
                    iov_base: address as *mut libc::c_void,
                    iov_len: length,
                };
                // SAFETY: iovec describes remote virtual addresses only. The live pidfd binds
                // the selected process; PAGEOUT preserves contents and never unmaps memory.
                let count = unsafe {
                    libc::syscall(
                        libc::SYS_process_madvise,
                        opened.pidfd.as_raw_fd(),
                        &range as *const libc::iovec,
                        1usize,
                        libc::MADV_PAGEOUT,
                        0u32,
                    )
                };
                if count < 0 {
                    return Err(io::Error::last_os_error());
                }
                advised |= count > 0;
                if count as usize != length {
                    return Err(io::Error::other(
                        "Kernel applied memory advice only partially",
                    ));
                }
                address += length;
            }
        }
        Ok(())
    })();
    let reduction = if advised {
        before
            .zip(process.as_ref().and_then(|p| p.resident().ok()))
            .map(|(a, b)| a.saturating_sub(b))
    } else {
        None
    };
    MemoryRelease {
        identity: selected.clone(),
        succeeded: result.is_ok(),
        applied: advised,
        resident_reduction_bytes: reduction,
        error: result.err().map(|e| e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_identity_and_different_executable_never_advise_disposable_process() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let process = Process::open(child.id(), current_user()).unwrap();
        let identity = process.identity().unwrap();
        let mut stale = identity.clone();
        stale.started_at.push_str("-stale");
        let wrong_identity = release_one(&stale, current_user(), None, &|| Ok(()));
        let wrong_executable = release_one(
            &identity,
            current_user(),
            Some("/fixture/not-this-executable"),
            &|| Ok(()),
        );
        let alive = child.try_wait().unwrap().is_none();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(alive);
        for result in [wrong_identity, wrong_executable] {
            assert!(!result.succeeded && !result.applied);
            assert_eq!(result.resident_reduction_bytes, None);
            assert!(result.error.unwrap().contains("changed"));
        }
    }
}
