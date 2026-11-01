//! Read-only Windows Restart Manager diagnostics. Never shut down or restart applications.
#![allow(unsafe_code)]
use crate::{BlockingProcess, FileUsage};
use std::{os::windows::ffi::OsStrExt, path::Path};
use windows_sys::Win32::{
    Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS},
    System::RestartManager::*,
};

struct Session(u32);
impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: This handle was returned by RmStartSession and is closed once.
        unsafe {
            RmEndSession(self.0);
        }
    }
}

pub(crate) fn inspect(path: &Path) -> FileUsage {
    FileUsage {
        path: path.to_string_lossy().into_owned(),
        processes: processes(path).unwrap_or_default(),
    }
}

fn processes(path: &Path) -> Option<Vec<BlockingProcess>> {
    let filename: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if filename[..filename.len() - 1].contains(&0) {
        return None;
    }
    let mut handle = 0;
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    // SAFETY: Output buffers have the documented sizes and remain live for each call.
    unsafe {
        if RmStartSession(&mut handle, 0, key.as_mut_ptr()) != ERROR_SUCCESS {
            return None;
        }
        let session = Session(handle);
        let files = [filename.as_ptr()];
        if RmRegisterResources(
            session.0,
            1,
            files.as_ptr(),
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
        ) != ERROR_SUCCESS
        {
            return None;
        }
        let mut needed = 0;
        let mut count = 0;
        let mut reasons = 0;
        let first = RmGetList(
            session.0,
            &mut needed,
            &mut count,
            std::ptr::null_mut(),
            &mut reasons,
        );
        if first == ERROR_SUCCESS {
            return Some(Vec::new());
        }
        if first != ERROR_MORE_DATA {
            return None;
        }
        for _ in 0..2 {
            // Bound allocations and retries if processes change during inspection.
            if needed == 0 || needed > 256 {
                return None;
            }
            let mut records = vec![RM_PROCESS_INFO::default(); needed as usize];
            count = records.len() as u32;
            let status = RmGetList(
                session.0,
                &mut needed,
                &mut count,
                records.as_mut_ptr(),
                &mut reasons,
            );
            if status == ERROR_MORE_DATA {
                continue;
            }
            if status != ERROR_SUCCESS || count as usize > records.len() {
                return None;
            }
            let mut result: Vec<_> = records
                .into_iter()
                .take(count as usize)
                .filter(|record| record.Process.dwProcessId != std::process::id())
                .map(|record| {
                    let end = record
                        .strAppName
                        .iter()
                        .position(|unit| *unit == 0)
                        .unwrap_or(record.strAppName.len());
                    BlockingProcess {
                        pid: record.Process.dwProcessId,
                        name: String::from_utf16_lossy(&record.strAppName[..end]),
                    }
                })
                .collect();
            result.sort_by_key(|process| process.pid);
            result.dedup_by_key(|process| process.pid);
            return Some(result);
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::{fs::OpenOptionsExt, process::CommandExt};
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
    };

    #[test]
    #[ignore = "Child process fixture, started by the parent test"]
    fn lock_holder() {
        let Some(path) = std::env::var_os("SPACEPRUNE_USAGE_TEST_FILE") else {
            return;
        };
        let _file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(path)
            .unwrap();
        println!("usage-ready");
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
    }

    #[test]
    fn exact_blocked_child_reports_its_holder_without_stopping_it() {
        let dir = tempfile::tempdir().unwrap();
        let selected = dir.path().join("cache");
        std::fs::create_dir(&selected).unwrap();
        let locked = selected.join("held.log");
        std::fs::write(&locked, b"keep").unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "file_usage::tests::lock_holder",
                "--ignored",
                "--nocapture",
            ])
            .env("SPACEPRUNE_USAGE_TEST_FILE", &locked)
            .creation_flags(0x08000000)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        struct Guard(std::process::Child);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = Guard(child);
        let mut output = BufReader::new(child.0.stdout.take().unwrap());
        let ready = (&mut output)
            .lines()
            .any(|line| line.unwrap().contains("usage-ready"));
        assert!(ready);
        let result = crate::filesystem::remove_selected(
            dir.path(),
            &crate::filesystem::read_identity(dir.path()).unwrap(),
            Path::new("cache"),
            &crate::filesystem::read_identity(&selected).unwrap(),
            &|_| false,
            &crate::OperationContext::default(),
        );
        let error = result.error.as_ref().unwrap();
        assert_eq!(
            crate::error::classify(error).kind,
            crate::IoFailureKind::SharingViolation
        );
        let usage = crate::error::file_usage(error).unwrap();
        assert_eq!(usage.path, locked.to_string_lossy());
        assert!(
            usage
                .processes
                .iter()
                .any(|process| process.pid == child.0.id() && !process.name.is_empty()),
            "{usage:?}"
        );
        assert!(child.0.try_wait().unwrap().is_none());
        assert_eq!(std::fs::read(&locked).unwrap(), b"keep");
        child.0.stdin.take().unwrap().write_all(b"done\n").unwrap();
        assert!(child.0.wait().unwrap().success());
    }
}
