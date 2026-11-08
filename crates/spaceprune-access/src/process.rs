//! Bounded process capture with shared cancellation, deadlines and child-tree termination.

use crate::{IoFailureKind, OperationContext, error::failure};
use spaceprune_contracts::messages;
use std::{
    io::{self, Read},
    process::{Child, Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
mod trust;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;
#[cfg(unix)]
pub use trust::{system_program, trusted_executable};

/// Whether Linux reports this process as running with native administrator privileges.
#[cfg(target_os = "linux")]
pub fn is_elevated() -> bool {
    rustix::process::geteuid().is_root()
}

/// Give a privileged system command a fixed working directory and a minimal environment.
#[cfg(unix)]
pub fn restrict_system_environment(command: &mut Command) {
    command
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LC_ALL", "C")
        .current_dir("/");
}

/// A child process whose descendants share a native termination boundary.
pub struct ProcessTree {
    child: Child,
    terminated: bool,
    #[cfg(unix)]
    isolated_group: bool,
    #[cfg(windows)]
    job: windows::Job,
}

impl std::fmt::Debug for ProcessTree {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProcessTree")
            .field("pid", &self.child.id())
            .field("terminated", &self.terminated)
            .finish_non_exhaustive()
    }
}

impl ProcessTree {
    /// Start a caller-selected executable with argv, never through an implicit shell.
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        Self::spawn_with_group(command, true)
    }

    /// Start a finite native authenticator in the already supervised foreground group.
    pub(crate) fn spawn_authenticator(command: &mut Command) -> io::Result<Self> {
        Self::spawn_with_group(command, false)
    }

    fn spawn_with_group(command: &mut Command, isolated_group: bool) -> io::Result<Self> {
        #[cfg(unix)]
        if isolated_group {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(not(unix))]
        let _ = isolated_group;
        #[cfg(windows)]
        windows::prepare(command, isolated_group);
        let mut child = command.spawn()?;
        #[cfg(windows)]
        let job = match windows::Job::attach(&child) {
            Ok(job) => job,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let _ = &mut child;
        Ok(Self {
            child,
            terminated: false,
            #[cfg(unix)]
            isolated_group,
            #[cfg(windows)]
            job,
        })
    }

    /// Access stdio and wait APIs while retaining the native containment owner.
    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// Observe termination without reaping a Unix PID before its process group is stopped.
    pub fn has_exited(&mut self) -> io::Result<bool> {
        if self.terminated {
            return Ok(true);
        }
        #[cfg(unix)]
        {
            use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
            let pid = Pid::from_raw(self.child.id() as i32)
                .ok_or_else(|| io::Error::other(messages::text("native.invalid_child_pid")))?;
            Ok(waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            )?
            .is_some())
        }
        #[cfg(windows)]
        {
            Ok(self.child.try_wait()?.is_some())
        }
    }

    /// Stop the owned process boundary and reap the direct child before reporting termination.
    pub fn terminate_and_wait(&mut self) -> io::Result<std::process::ExitStatus> {
        if self.terminated {
            return self.child.wait();
        }
        #[cfg(unix)]
        {
            if let Some(pid) = rustix::process::Pid::from_raw(self.child.id() as i32) {
                if self.isolated_group {
                    let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::TERM);
                } else {
                    // Give a foreground authenticator time to restore terminal echo.
                    let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
                }
                let deadline = Instant::now() + Duration::from_millis(200);
                while Instant::now() < deadline && !self.has_exited()? {
                    thread::sleep(Duration::from_millis(10));
                }
                // Also stop descendants when the direct child has already exited.
                if self.isolated_group {
                    let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
                }
            }
        }
        #[cfg(windows)]
        self.job.terminate();
        let _ = self.child.kill();
        self.terminated = true;
        self.child.wait()
    }
}

/// Run a finite native authenticator in the caller's foreground terminal group.
/// The authenticator owns password input and echo settings; the host never captures them.
/// Only the direct authentication process is terminated on cancellation. Long-running work
/// must subsequently use the ordinary isolated process runner with noninteractive credentials.
pub fn terminal_status(
    mut command: Command,
    operation: &OperationContext,
) -> io::Result<std::process::ExitStatus> {
    operation.check()?;
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = ProcessTree::spawn_authenticator(&mut command)?;
    loop {
        operation.check()?;
        if child.has_exited()? {
            return child.terminate_and_wait();
        }
        thread::sleep(Duration::from_millis(20));
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        let _ = self.terminate_and_wait();
    }
}

/// Write to an owned Unix IPC pipe without allowing a full pipe to outlive the operation.
/// The caller must serialize writes to this descriptor; its original flags are restored.
#[cfg(unix)]
pub fn write_pipe(
    pipe: &mut (impl std::io::Write + std::os::fd::AsFd),
    mut bytes: &[u8],
    operation: &OperationContext,
) -> io::Result<()> {
    use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
    operation.check()?;
    let flags = fcntl_getfl(&*pipe)?;
    fcntl_setfl(&*pipe, flags | OFlags::NONBLOCK)?;
    let result = (|| {
        while !bytes.is_empty() {
            operation.check()?;
            match pipe.write(bytes) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => bytes = &bytes[count..],
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    })();
    let restored = fcntl_setfl(&*pipe, flags).map_err(io::Error::from);
    result.and(restored)
}

enum Chunk {
    Data(usize, Vec<u8>),
    Closed,
    Error(io::Error),
}

fn read_pipe(mut pipe: impl Read, stream: usize, sender: mpsc::SyncSender<Chunk>) {
    loop {
        let mut buffer = vec![0; 16 * 1024];
        match pipe.read(&mut buffer) {
            Ok(0) => {
                let _ = sender.send(Chunk::Closed);
                return;
            }
            Ok(size) => {
                buffer.truncate(size);
                if sender.send(Chunk::Data(stream, buffer)).is_err() {
                    return;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                let _ = sender.send(Chunk::Error(error));
                return;
            }
        }
    }
}

/// Capture a finite process invocation, respecting the original operation's cancellation signal.
/// Native commands with uncertain side effects must be reconciled by their domain before retry.
pub fn capture(command: Command, operation: &OperationContext) -> io::Result<Output> {
    capture_open_input(command, None, operation)
}

/// Send a bounded frame and hold stdin open until completion or cancellation.
/// Helpers can observe EOF even when the caller cannot signal an elevated child.
#[cfg(unix)]
pub fn capture_with_open_stdin(
    command: Command,
    input: &[u8],
    operation: &OperationContext,
) -> io::Result<Output> {
    capture_open_input(command, Some(input), operation)
}

fn capture_open_input(
    mut command: Command,
    input: Option<&[u8]>,
    operation: &OperationContext,
) -> io::Result<Output> {
    operation.check()?;
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut tree = ProcessTree::spawn(&mut command)?;
    let input_pipe = tree.child_mut().stdin.take();
    #[cfg(unix)]
    let mut input_pipe = input_pipe;
    #[cfg(unix)]
    if let Some(bytes) = input {
        write_pipe(input_pipe.as_mut().expect("piped stdin"), bytes, operation)?;
    }
    let stdout = tree.child_mut().stdout.take().expect("piped stdout");
    let stderr = tree.child_mut().stderr.take().expect("piped stderr");
    let (sender, receiver) = mpsc::sync_channel(8);
    let out_sender = sender.clone();
    let out_reader = thread::spawn(move || read_pipe(stdout, 0, out_sender));
    let err_reader = thread::spawn(move || read_pipe(stderr, 1, sender));
    let mut captured = [Vec::new(), Vec::new()];
    let mut line_lengths = [0_usize; 2];
    let mut closed = 0;
    let mut received_output = false;
    let mut last_output = Instant::now();
    let result = (|| {
        loop {
            operation.check()?;
            let timeout = if received_output {
                operation.budget.idle_timeout
            } else {
                operation.budget.first_output_timeout
            };
            if last_output.elapsed() >= timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    messages::text("native.output_deadline"),
                ));
            }
            match receiver.recv_timeout(Duration::from_millis(20)) {
                Ok(Chunk::Data(stream, bytes)) => {
                    let total = captured[0]
                        .len()
                        .saturating_add(captured[1].len())
                        .saturating_add(bytes.len());
                    if total > operation.budget.output_bytes {
                        return Err(failure(
                            IoFailureKind::ResourceLimit,
                            messages::text("native.output_byte_budget"),
                        ));
                    }
                    for byte in &bytes {
                        if *byte == b'\n' {
                            line_lengths[stream] = 0;
                        } else {
                            line_lengths[stream] = line_lengths[stream].saturating_add(1);
                        }
                        if line_lengths[stream] > operation.budget.line_bytes {
                            return Err(failure(
                                IoFailureKind::ResourceLimit,
                                messages::text("native.output_line_budget"),
                            ));
                        }
                    }
                    captured[stream].extend(bytes);
                    last_output = Instant::now();
                    received_output = true;
                }
                Ok(Chunk::Closed) => closed += 1,
                Ok(Chunk::Error(error)) => return Err(error),
                Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {}
            }
            if closed == 2 && tree.has_exited()? {
                return Ok(());
            }
        }
    })();
    // EOF cancels a privileged worker before waiting for its termination.
    drop(input_pipe);
    let termination = tree.terminate_and_wait();
    drop(receiver);
    let _ = out_reader.join();
    let _ = err_reader.join();
    result?;
    let status = termination?;
    let [stdout, stderr] = captured;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{CancellationToken, Interaction};

    fn operation() -> OperationContext {
        OperationContext::new(
            CancellationToken::default(),
            Interaction::NonInteractive,
            Duration::from_secs(5),
        )
    }

    #[test]
    fn terminal_authentication_keeps_the_callers_process_group() {
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        let mut child = ProcessTree::spawn_with_group(&mut command, false).unwrap();
        let pid = rustix::process::Pid::from_raw(child.child.id() as i32).unwrap();
        assert_eq!(
            rustix::process::getpgid(Some(pid)).unwrap(),
            rustix::process::getpgrp()
        );
        child.terminate_and_wait().unwrap();
    }

    #[test]
    fn terminal_authentication_cancels_without_signalling_the_callers_group() {
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        let context = operation();
        let cancellation = context.cancellation.clone();
        let cancel = thread::spawn(move || {
            thread::sleep(Duration::from_millis(80));
            cancellation.cancel();
        });
        let started = Instant::now();
        let error = terminal_status(command, &context).unwrap_err();
        cancel.join().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn cancellation_closes_cooperative_input_before_waiting_for_worker_exit() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("closed");
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                "trap '' TERM; read -r first; cat >/dev/null; printf closed > \"$1\"",
                "fixture",
            ])
            .arg(&marker);
        let context = operation();
        let cancellation = context.cancellation.clone();
        let cancel = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            cancellation.cancel();
        });
        let result = capture_with_open_stdin(command, b"start\n", &context);
        cancel.join().unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "closed");
    }

    #[test]
    fn output_limit_stops_a_noisy_child_without_unbounded_buffering() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "while :; do printf '0123456789'; done"]);
        let mut context = operation();
        context.budget.output_bytes = 4096;
        let error = capture(command, &context).unwrap_err();
        assert_eq!(
            crate::error::classify(&error).kind,
            IoFailureKind::ResourceLimit
        );
    }

    #[test]
    fn cancellation_waits_for_the_process_tree_and_preserves_the_original_signal() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30 & wait"]);
        let context = operation();
        let cancellation = context.cancellation.clone();
        let cancel = thread::spawn(move || {
            thread::sleep(Duration::from_millis(80));
            cancellation.cancel();
        });
        let started = Instant::now();
        let error = capture(command, &context).unwrap_err();
        cancel.join().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
#[cfg(unix)]
#[test]
fn full_ipc_pipe_obeys_the_deadline_and_restores_descriptor_flags() {
    let (mut writer, _reader) = std::os::unix::net::UnixStream::pair().unwrap();
    let flags = rustix::fs::fcntl_getfl(&writer).unwrap();
    let operation = OperationContext::new(
        crate::CancellationToken::default(),
        crate::Interaction::NonInteractive,
        Duration::from_millis(50),
    );
    let started = Instant::now();
    let error = write_pipe(&mut writer, &vec![0; 2 * 1024 * 1024], &operation).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert_eq!(rustix::fs::fcntl_getfl(&writer).unwrap(), flags);
    assert!(started.elapsed() < Duration::from_secs(1));
}
#[cfg(unix)]
#[test]
fn restricted_system_commands_ignore_caller_environment_and_working_directory() {
    let directory = tempfile::tempdir().unwrap();
    let mut command = Command::new(system_program("sh").unwrap());
    command
        .current_dir(directory.path())
        .env("SPACEPRUNE_UNTRUSTED_CONFIG", "fixture");
    restrict_system_environment(&mut command);
    command.args([
        "-c",
        "test -z \"$SPACEPRUNE_UNTRUSTED_CONFIG\" && test \"$PWD\" = /",
    ]);
    assert!(
        capture(command, &OperationContext::default())
            .unwrap()
            .status
            .success()
    );
}
