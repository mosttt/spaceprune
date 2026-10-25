//! Real PTY checks use a separate session, never the developer's controlling terminal.
#![allow(unsafe_code)]
use super::*;
use std::{
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
};

const FIXTURE: &str = "macos::terminal::tests::job_control::foreground_fixture";

#[test]
#[ignore = "Launched by the parent test in its own PTY/session"]
fn foreground_fixture() {
    let Some(root) = std::env::var_os("SPACEPRUNE_TERMINAL_FIXTURE") else {
        return;
    };
    let root = Path::new(&root);
    let terminal = File::open("/dev/tty").unwrap();
    let fd = terminal.as_raw_fd();
    // SAFETY: The fixture owns this isolated PTY, and the buffer is fully initialized on success.
    let mut initial = std::mem::MaybeUninit::uninit();
    assert_eq!(unsafe { libc::tcgetattr(fd, initial.as_mut_ptr()) }, 0);
    let initial = unsafe { initial.assume_init() };
    let original = unsafe { libc::tcgetpgrp(fd) };
    let marker = root.join("native-pid");
    let start = root.join("start");
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "while [ ! -f \"$1\" ]; do sleep 0.01; done; stty -echo < /dev/tty; echo $$ > \"$2\"; exec sleep 60", "fixture"])
        .arg(&start).arg(&marker).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let process = ProcessTree::spawn(&mut command).unwrap();
    let mut agent = AgentProcess::new(process);
    let context = operation();
    agent.activate_terminal(&context).unwrap();
    fs::write(start, b"ready").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !marker.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(marker.exists());
    let mut changed = std::mem::MaybeUninit::uninit();
    // SAFETY: The still-live fixture terminal initializes the complete termios value.
    assert_eq!(unsafe { libc::tcgetattr(fd, changed.as_mut_ptr()) }, 0);
    assert_eq!(unsafe { changed.assume_init() }.c_lflag & libc::ECHO, 0);
    assert_eq!(unsafe { libc::tcgetpgrp(fd) }, agent.pid() as i32);
    context.cancel();
    drop(agent);
    assert_reaped(&marker);
    let mut restored = std::mem::MaybeUninit::uninit();
    // SAFETY: Same owned PTY and initialized output buffer as above.
    assert_eq!(unsafe { libc::tcgetattr(fd, restored.as_mut_ptr()) }, 0);
    assert_eq!(unsafe { restored.assume_init() }.c_lflag, initial.c_lflag);
    assert_eq!(unsafe { libc::tcgetpgrp(fd) }, original);
    fs::write(
        root.join("restored"),
        b"foreground and echo restored after reap",
    )
    .unwrap();
}

#[test]
fn cancellation_reaps_the_authenticator_then_restores_foreground_and_echo() {
    let root = tempfile::tempdir().unwrap();
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: openpty initializes two owned descriptors; no developer terminal is used.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0
    );
    let master = unsafe { File::from_raw_fd(master) };
    let slave = unsafe { File::from_raw_fd(slave) };
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", FIXTURE, "--nocapture"])
        .env("SPACEPRUNE_TERMINAL_FIXTURE", root.path())
        .stdin(slave.try_clone().unwrap())
        .stdout(slave.try_clone().unwrap())
        .stderr(slave);
    // SAFETY: Only async-signal-safe session/terminal syscalls run between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut process = ProcessTree::spawn_authenticator(&mut command).unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    while !process.has_exited().unwrap() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let status = process.terminate_and_wait().unwrap();
    drop(master);
    assert!(
        status.success(),
        "Isolated terminal fixture failed: {status}"
    );
    assert!(root.path().join("restored").exists());
}
