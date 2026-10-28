use std::{
    io,
    mem::size_of,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    process::{Child, Command},
    ptr,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
    System::{Diagnostics::ToolHelp::*, JobObjects::*, Threading::*},
};

pub(super) struct Job(OwnedHandle);

pub(super) fn prepare(command: &mut Command, background: bool) {
    // Captured tool probes must not allocate a console when launched from a GUI host.
    // Keep the foreground authenticator's terminal behavior separate.
    command.creation_flags(CREATE_SUSPENDED | if background { CREATE_NO_WINDOW } else { 0 });
}

impl Job {
    pub(super) fn attach(child: &Child) -> io::Result<Self> {
        // SAFETY: All API buffers have their declared native layout. Every successful handle
        // has a single owner, and the suspended child cannot spawn before job assignment.
        unsafe {
            let handle = CreateJobObjectW(ptr::null(), ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(OwnedHandle::from_raw_handle(handle));
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
                || AssignProcessToJobObject(handle, child.as_raw_handle()) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let mut entry = THREADENTRY32 {
                dwSize: size_of::<THREADENTRY32>() as u32,
                ..Default::default()
            };
            let mut found = false;
            let mut next = Thread32First(snapshot, &mut entry);
            while next != 0 {
                if entry.th32OwnerProcessID == child.id() {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !thread.is_null() {
                        found = ResumeThread(thread) != u32::MAX;
                        CloseHandle(thread);
                    }
                    break;
                }
                next = Thread32Next(snapshot, &mut entry);
            }
            CloseHandle(snapshot);
            if !found {
                return Err(io::Error::other("Could not resume the contained process"));
            }
            Ok(job)
        }
    }
    pub(super) fn terminate(&self) {
        // SAFETY: OwnedHandle keeps the job alive for this call, including across worker threads.
        unsafe {
            TerminateJobObject(self.0.as_raw_handle(), 1);
        }
    }
}
