//! Native regression for tool processes launched from desktop hosts.
#![cfg(windows)]
#![allow(unsafe_code)]

#[test]
fn captured_child_has_no_console() {
    use spaceprune_access::{CancellationToken, Interaction, OperationContext, process::capture};
    if std::env::var_os("SPACEPRUNE_CONSOLE_PROBE_CHILD").is_some() {
        // SAFETY: This API takes no arguments and only observes the current process.
        assert!(unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() }.is_null());
        println!("captured-without-console");
        return;
    }
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "captured_child_has_no_console", "--nocapture"])
        .env("SPACEPRUNE_CONSOLE_PROBE_CHILD", "1");
    let operation = OperationContext::new(
        CancellationToken::default(),
        Interaction::NonInteractive,
        std::time::Duration::from_secs(15),
    );
    let output = capture(command, &operation).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("captured-without-console"));
}
