//! Supervise an ordinary installer child without killing an in-flight MSI transaction.
use super::{
    installer_protocol::{MAX_FRAME, Reply, Request},
    verification::check,
};
use crate::{OperationContext, PermissionOutcome};
use std::{
    io::{Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    time::Duration,
};

pub(crate) fn run(
    mut command: Command,
    request: &Request,
    operation: &OperationContext,
) -> Result<(), PermissionOutcome> {
    check(operation)?;
    let mut bytes = serde_json::to_vec(request).map_err(|_| PermissionOutcome::PolicyBlocked)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_FRAME {
        return Err(PermissionOutcome::PolicyBlocked);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
    let input = child.stdin.take();
    let mut process = InstallerProcess { child, input };
    let mut output = process
        .child
        .stdout
        .take()
        .ok_or(PermissionOutcome::AuthorizationUnavailable)?;
    let reader = std::thread::spawn(move || {
        let mut result = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = output.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            let remaining = (MAX_FRAME as usize + 1).saturating_sub(result.len());
            result.extend_from_slice(&buffer[..count.min(remaining)]);
        }
        Ok::<_, std::io::Error>(result)
    });
    let sent = process
        .input
        .as_mut()
        .ok_or(PermissionOutcome::AuthorizationUnavailable)?
        .write_all(&bytes)
        .map_err(|_| PermissionOutcome::AuthorizationUnavailable);
    if sent.is_err() {
        process.input.take();
    }
    let status = loop {
        if operation.is_stopped() {
            process.input.take();
        }
        match process.child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break Err(PermissionOutcome::AuthorizationUnavailable),
        }
    };
    // Dropping stdin asks the worker to cancel; wait for the real transaction/worker exit before
    // reading a receipt or reporting a stopped operation, including all error paths.
    drop(process);
    let output = reader
        .join()
        .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?
        .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
    check(operation)?;
    sent?;
    if !status?.success() || output.len() as u64 > MAX_FRAME {
        return Err(PermissionOutcome::AuthorizationUnavailable);
    }
    let reply: Reply =
        serde_json::from_slice(&output).map_err(|_| PermissionOutcome::HelperUntrusted)?;
    reply.validate(request).map(|_| ())
}

struct InstallerProcess {
    child: Child,
    input: Option<ChildStdin>,
}
impl Drop for InstallerProcess {
    fn drop(&mut self) {
        self.input.take();
        // Installer cancellation/rollback is cooperative. Terminating its client cannot prove
        // that the Windows Installer service has stopped modifying the installation.
        let _ = self.child.wait();
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "install_process_tests.rs"]
mod tests;
