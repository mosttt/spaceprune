//! All native process handles are held through identity verification and mutation.
use super::*;
use std::{
    mem::{size_of, zeroed},
    path::Path,
    ptr::null_mut,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, HANDLE, WAIT_OBJECT_0},
    Security::{EqualSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser},
    System::{
        ProcessStatus::{
            K32EmptyWorkingSet, K32EnumProcesses, K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
            PROCESS_MEMORY_COUNTERS_EX,
        },
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::{
            GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, IsProcessCritical,
            OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
            PROCESS_SYNCHRONIZE, PROCESS_VM_READ, QueryFullProcessImageNameW, WaitForSingleObject,
        },
    },
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: This type owns exactly one successful native handle, never a pseudo handle.
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn token_user(process: HANDLE) -> io::Result<Vec<usize>> {
    let mut token = null_mut();
    // SAFETY: The process is live and token is an initialized output slot.
    check(unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) })?;
    let token = Handle(token);
    let mut bytes = 0;
    // SAFETY: Querying the required size uses a null buffer with zero capacity.
    unsafe {
        GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut bytes);
    }
    if bytes < size_of::<TOKEN_USER>() as u32 || bytes > 65_536 {
        return Err(io::Error::last_os_error());
    }
    let mut buffer = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
    // SAFETY: Word allocation provides TOKEN_USER alignment and at least the requested bytes.
    check(unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        )
    })?;
    Ok(buffer)
}
fn eligible(pid: u32) -> io::Result<Handle> {
    // SAFETY: This call has no pointer arguments.
    if pid == 0 || pid == unsafe { GetCurrentProcessId() } {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "This process is excluded",
        ));
    }
    // SAFETY: OpenProcess validates the PID; no termination, injection, or debug rights are requested.
    let raw = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ | PROCESS_SET_QUOTA,
            0,
            pid,
        )
    };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    let handle = Handle(raw);
    let mut critical = 0;
    // SAFETY: The handle is owned and critical points to writable BOOL storage.
    check(unsafe { IsProcessCritical(handle.0, &mut critical) })?;
    if critical != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Critical process is excluded",
        ));
    }
    let target = token_user(handle.0)?;
    // SAFETY: GetCurrentProcess supplies a valid pseudo handle; it is never closed.
    let current = token_user(unsafe { GetCurrentProcess() })?;
    // SAFETY: GetTokenInformation initialized aligned TOKEN_USER records and their inline SIDs.
    let same_user = unsafe {
        EqualSid(
            (*(target.as_ptr().cast::<TOKEN_USER>())).User.Sid,
            (*(current.as_ptr().cast::<TOKEN_USER>())).User.Sid,
        )
    };
    if same_user == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Process belongs to another user",
        ));
    }
    Ok(handle)
}
fn identity(handle: &Handle, pid: u32) -> io::Result<ProcessIdentity> {
    let mut times = [FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    }; 4];
    // SAFETY: Four distinct initialized FILETIME slots are passed to the OS.
    check(unsafe {
        GetProcessTimes(
            handle.0,
            &mut times[0],
            &mut times[1],
            &mut times[2],
            &mut times[3],
        )
    })?;
    let ticks = ((times[0].dwHighDateTime as u64) << 32) | times[0].dwLowDateTime as u64;
    Ok(ProcessIdentity {
        pid,
        started_at: ticks.to_string(),
    })
}
fn counters(handle: &Handle) -> io::Result<PROCESS_MEMORY_COUNTERS_EX> {
    // SAFETY: The native POD consists exclusively of integer fields.
    let mut memory: PROCESS_MEMORY_COUNTERS_EX = unsafe { zeroed() };
    memory.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    // SAFETY: EX extends the base layout and the supplied size covers the full writable allocation.
    check(unsafe {
        K32GetProcessMemoryInfo(
            handle.0,
            (&mut memory as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
            memory.cb,
        )
    })?;
    Ok(memory)
}
pub(super) fn inspect() -> io::Result<MemorySnapshot> {
    // SAFETY: All MEMORYSTATUSEX fields are integer POD values.
    let mut status: MEMORYSTATUSEX = unsafe { zeroed() };
    status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: status is a correctly sized writable record.
    check(unsafe { GlobalMemoryStatusEx(&mut status) })?;
    let mut pids = vec![0u32; 1024];
    loop {
        let mut needed = 0;
        let capacity = (pids.len() * size_of::<u32>()) as u32;
        // SAFETY: The capacity is exactly that of the writable PID allocation.
        check(unsafe { K32EnumProcesses(pids.as_mut_ptr(), capacity, &mut needed) })?;
        if needed < capacity {
            pids.truncate(needed as usize / size_of::<u32>());
            break;
        }
        if pids.len() >= 65_536 {
            return Err(io::Error::other("Process inventory exceeded its limit"));
        }
        pids.resize(pids.len() * 2, 0);
    }
    let mut processes = Vec::new();
    for pid in pids {
        let Ok(handle) = eligible(pid) else { continue };
        let Ok(identity) = identity(&handle, pid) else {
            continue;
        };
        let Ok(memory) = counters(&handle) else {
            continue;
        };
        let Ok(path_words) = name_path(&handle) else {
            continue;
        };
        let executable = String::from_utf16(&path_words).ok();
        let path = String::from_utf16_lossy(&path_words);
        let name = Path::new(&path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        processes.push(ProcessMemory {
            identity,
            name,
            executable,
            resident_bytes: memory.WorkingSetSize as u64,
            private_bytes: memory.PrivateUsage as u64,
        });
    }
    processes.sort_by_key(|process| std::cmp::Reverse(process.resident_bytes));
    Ok(MemorySnapshot {
        total_bytes: status.ullTotalPhys,
        available_bytes: status.ullAvailPhys,
        processes,
    })
}
pub(super) fn release_one(selected: &ProcessIdentity) -> MemoryRelease {
    release_matching(selected, None)
}

fn name_path(handle: &Handle) -> io::Result<Vec<u16>> {
    let mut path = vec![0u16; 32_768];
    let mut length = path.len() as u32;
    // SAFETY: The owned process handle and writable buffer remain valid for the native call.
    check(unsafe { QueryFullProcessImageNameW(handle.0, 0, path.as_mut_ptr(), &mut length) })?;
    path.truncate(length as usize);
    Ok(path)
}

fn process_absent(pid: u32) -> bool {
    // SAFETY: This read-only probe never operates on a replacement process.
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if raw.is_null() {
        return pid != 0
            && io::Error::last_os_error().raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32);
    }
    let handle = Handle(raw);
    // SAFETY: The owned process handle has SYNCHRONIZE access; zero timeout never waits.
    unsafe { WaitForSingleObject(handle.0, 0) == WAIT_OBJECT_0 }
}

pub(super) fn release_matching(
    selected: &ProcessIdentity,
    executable: Option<&str>,
) -> MemoryRelease {
    let result = (|| -> io::Result<Option<u64>> {
        let handle = eligible(selected.pid)?;
        if identity(&handle, selected.pid)? != *selected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Selected process identity has changed",
            ));
        }
        if let Some(expected) = executable {
            let observed = String::from_utf16(&name_path(&handle)?).map_err(io::Error::other)?;
            if observed != expected {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Selected executable has changed",
                ));
            }
        }
        let before = counters(&handle)?.WorkingSetSize as u64;
        // SAFETY: Identity, current-user ownership, non-critical status and required rights
        // were checked on this same owned handle; the API does not terminate the process.
        check(unsafe { K32EmptyWorkingSet(handle.0) })?;
        Ok(counters(&handle)
            .ok()
            .map(|after| before.saturating_sub(after.WorkingSetSize as u64)))
    })();
    let already_absent = result.is_err() && process_absent(selected.pid);
    MemoryRelease {
        identity: selected.clone(),
        already_absent,
        succeeded: result.is_ok(),
        applied: result.is_ok(),
        resident_reduction_bytes: if already_absent {
            Some(0)
        } else {
            result.as_ref().ok().copied().flatten()
        },
        error: if already_absent {
            None
        } else {
            result.err().map(|error| error.to_string())
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, Write},
        process::{Command, Stdio},
        time::Duration,
    };
    #[test]
    #[ignore = "spawned only by the isolated native acceptance test"]
    fn child_fixture() {
        if std::env::var_os("SPACEPRUNE_MEMORY_FIXTURE").is_none() {
            return;
        }
        let mut memory = vec![0u8; 64 * 1024 * 1024];
        for page in memory.chunks_mut(4096) {
            page[0] = 1;
        }
        println!("READY");
        std::io::stdout().flush().unwrap();
        std::thread::sleep(Duration::from_secs(30));
        std::hint::black_box(memory);
    }
    struct Fixture(std::process::Child);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    #[test]
    fn native_trim_binds_identity_and_keeps_disposable_child_running() {
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native::tests::child_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env("SPACEPRUNE_MEMORY_FIXTURE", "1")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut fixture = Fixture(child);
        let output = std::io::BufReader::new(fixture.0.stdout.take().unwrap());
        assert!(
            output
                .lines()
                .map_while(Result::ok)
                .any(|line| line == "READY")
        );
        let snapshot = inspect().unwrap();
        assert!(snapshot.total_bytes > 0 && snapshot.available_bytes <= snapshot.total_bytes);
        assert!(
            !snapshot
                .processes
                .iter()
                .any(|process| process.identity.pid == std::process::id())
        );
        let process = snapshot
            .processes
            .iter()
            .find(|process| process.identity.pid == fixture.0.id())
            .unwrap();
        let mut stale = process.identity.clone();
        stale.started_at = "0".into();
        assert!(!release_one(&stale).succeeded);
        let result = release_one(&process.identity);
        assert!(result.succeeded, "{:?}", result.error);
        assert!(fixture.0.try_wait().unwrap().is_none());
        fixture.0.kill().unwrap();
        fixture.0.wait().unwrap();
        let absent = release_one(&process.identity);
        assert!(absent.already_absent && !absent.succeeded && !absent.applied);
        assert_eq!(absent.resident_reduction_bytes, Some(0));
        assert!(absent.error.is_none());
    }
}
