use super::*;
use std::process::{Command, Stdio};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};

#[test]
#[ignore = "Spawned as an isolated child by the bounded termination test"]
fn waiting_child() {
    std::thread::sleep(Duration::from_secs(30));
}

#[test]
fn missing_terminate_rights_cannot_block_shutdown_forever_or_claim_success() {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "native::tests::waiting_child"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // SAFETY: The owned child is alive. This second handle deliberately lacks terminate rights.
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, child.id()) };
    assert!(!handle.is_null());
    let handle = ProcessHandle(handle);
    let started = Instant::now();
    let result = terminate(handle.0, CANCELLED_EXIT_CODE);
    let still_running = child.try_wait().unwrap().is_none();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(matches!(result, Err(ElevationError::Unavailable)));
    assert!(still_running);
    assert!(started.elapsed() < Duration::from_secs(15));
}
