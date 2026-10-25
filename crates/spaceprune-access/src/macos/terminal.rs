//! Terminal ownership stays with the supervised agent group; native Security reads the password.

mod foreground;
use super::{check, credential::ExternalAuthorization};
use crate::{Interaction, OperationContext, PermissionOutcome, process::ProcessTree};
use std::{
    io::{self, Read},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::Duration,
};

pub(super) struct AgentProcess {
    process: ProcessTree,
    foreground: Option<foreground::Foreground>,
}

impl AgentProcess {
    pub(super) fn new(process: ProcessTree) -> Self {
        Self {
            process,
            foreground: None,
        }
    }
    pub(super) fn activate_terminal(
        &mut self,
        context: &OperationContext,
    ) -> Result<(), PermissionOutcome> {
        check(context)?;
        if context.interaction == Interaction::Terminal {
            self.foreground = Some(
                foreground::Foreground::claim(self.pid())
                    .map_err(|_| PermissionOutcome::NeedsAuthorization)?,
            );
        }
        Ok(())
    }
    pub(super) fn pid(&mut self) -> u32 {
        self.process.child_mut().id()
    }
    pub(super) fn has_exited(&mut self) -> io::Result<bool> {
        self.process.has_exited()
    }
    pub(super) fn terminate_and_wait(&mut self) -> io::Result<ExitStatus> {
        let status = self.process.terminate_and_wait()?;
        self.foreground.take();
        Ok(status)
    }
}
impl Drop for AgentProcess {
    fn drop(&mut self) {
        let _ = self.terminate_and_wait();
    }
}

// The caller is the verified authorization agent, never the GUI/CLI host. The native
// reference goes directly into Authorization Services while the issuer is still alive.
pub(super) fn import_from_authenticator<T>(
    mut command: Command,
    context: &OperationContext,
    import: impl FnOnce(&ExternalAuthorization) -> Result<T, PermissionOutcome>,
) -> Result<T, PermissionOutcome> {
    check(context)?;
    if context.interaction != Interaction::Terminal {
        return Err(PermissionOutcome::NeedsAuthorization);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child =
        ProcessTree::spawn_authenticator(&mut command).map_err(|error| outcome(error, context))?;
    let mut output = child
        .child_mut()
        .stdout
        .take()
        .ok_or(PermissionOutcome::AuthorizationUnavailable)?;
    use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
    let flags = fcntl_getfl(&output).map_err(|error| outcome(error.into(), context))?;
    fcntl_setfl(&output, flags | OFlags::NONBLOCK)
        .map_err(|error| outcome(error.into(), context))?;
    let mut reference = ExternalAuthorization([0; 32]);
    let mut filled = 0;
    while filled < reference.0.len() {
        check(context)?;
        match output.read(&mut reference.0[filled..]) {
            Ok(0) => return Err(PermissionOutcome::AuthorizationUnavailable),
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10))
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(outcome(error, context)),
        }
    }
    check(context)?;
    if child
        .has_exited()
        .map_err(|error| outcome(error, context))?
    {
        return Err(PermissionOutcome::AuthorizationUnavailable);
    }
    // security can externalize a reference even on denial. Import and native right
    // verification decide success; bytes, a password prompt or an exit code never do.
    let result = import(&reference);
    child
        .terminate_and_wait()
        .map_err(|error| outcome(error, context))?;
    check(context)?;
    result
}

fn outcome(error: io::Error, context: &OperationContext) -> PermissionOutcome {
    check(context).err().unwrap_or(match error.kind() {
        io::ErrorKind::Interrupted => PermissionOutcome::Cancelled,
        io::ErrorKind::TimedOut => PermissionOutcome::TimedOut,
        _ => PermissionOutcome::AuthorizationUnavailable,
    })
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
