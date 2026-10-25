//! Host-side process supervision and typed receipts; native credentials never enter this module.
use super::*;
use crate::process::ProcessTree;
use std::{
    process::{Command, Stdio},
    thread,
};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Completion {
    Registered(RegistrationState),
    Authorized,
}

#[cfg(test)]
pub(super) fn launch(
    kind: ServiceKind,
    context: &OperationContext,
    command: impl FnOnce(&str) -> Command,
    verify_peer: impl Fn(&Channel, u32) -> Result<(), PermissionOutcome>,
) -> Result<RegistrationState, PermissionOutcome> {
    match launch_agent(
        kind,
        AgentAction::Register,
        None,
        context,
        command,
        verify_peer,
    )? {
        Completion::Registered(state) => Ok(state),
        Completion::Authorized => Err(PermissionOutcome::HelperUntrusted),
    }
}

pub(super) fn launch_agent(
    kind: ServiceKind,
    action: AgentAction,
    service_nonce: Option<&str>,
    context: &OperationContext,
    command: impl FnOnce(&str) -> Command,
    verify_peer: impl Fn(&Channel, u32) -> Result<(), PermissionOutcome>,
) -> Result<Completion, PermissionOutcome> {
    check(context)?;
    let endpoint = Endpoint::bind().map_err(|error| io_outcome(error, context))?;
    let mut command = command(endpoint.name());
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let process = ProcessTree::spawn(&mut command).map_err(|error| io_outcome(error, context))?;
    let mut process = super::terminal::AgentProcess::new(process);
    let result = (|| {
        let pid = process.pid();
        let mut channel = loop {
            check(context)?;
            match endpoint.accept() {
                Ok(Some(channel)) => {
                    verify_peer(&channel, pid)?;
                    process.activate_terminal(context)?;
                    break channel;
                }
                Ok(None) => {}
                Err(error) => return Err(io_outcome(error, context)),
            }
            if process
                .has_exited()
                .map_err(|error| io_outcome(error, context))?
            {
                return Err(PermissionOutcome::AuthorizationUnavailable);
            }
            thread::sleep(Duration::from_millis(10));
        };
        let call = Call {
            protocol: PROTOCOL,
            nonce: uuid::Uuid::new_v4().to_string(),
            request: AgentRequest {
                action,
                service_nonce: service_nonce.map(str::to_owned),
                ..AgentRequest::new(kind, context).map_err(|error| io_outcome(error, context))?
            },
        };
        channel
            .write(&call, context)
            .map_err(|error| io_outcome(error, context))?;
        let ready: Receipt = channel
            .read(context)
            .map_err(|error| io_outcome(error, context))?;
        if !call.matches(&ready) || !matches!(ready.event, Event::Ready) {
            return Err(PermissionOutcome::HelperUntrusted);
        }
        let result: Receipt = channel
            .read(context)
            .map_err(|error| io_outcome(error, context))?;
        if !call.matches(&result) {
            return Err(PermissionOutcome::HelperUntrusted);
        }
        let Event::Finished { reply } = result.event else {
            return Err(PermissionOutcome::HelperUntrusted);
        };
        while !process
            .has_exited()
            .map_err(|error| io_outcome(error, context))?
        {
            check(context)?;
            thread::sleep(Duration::from_millis(10));
        }
        let status = process
            .terminate_and_wait()
            .map_err(|error| io_outcome(error, context))?;
        check(context)?;
        match reply {
            AgentReply::Authorized if action == AgentAction::Authorize && status.success() => {
                Ok(Completion::Authorized)
            }
            AgentReply::Observed {
                state: RegistrationState::Enabled,
            } if matches!(action, AgentAction::Register | AgentAction::Refresh)
                && status.success() =>
            {
                Ok(Completion::Registered(RegistrationState::Enabled))
            }
            AgentReply::Observed {
                state: RegistrationState::RequiresApproval,
            } if matches!(action, AgentAction::Register | AgentAction::Refresh)
                && status.code() == Some(75) =>
            {
                Ok(Completion::Registered(RegistrationState::RequiresApproval))
            }
            AgentReply::Failed { outcome }
                if !status.success() && outcome != PermissionOutcome::Granted =>
            {
                Err(outcome)
            }
            _ => Err(PermissionOutcome::AuthorizationUnavailable),
        }
    })();
    if result.is_err() {
        use std::os::unix::process::ExitStatusExt;
        let status = process
            .terminate_and_wait()
            .map_err(|error| io_outcome(error, context))?;
        check(context)?;
        if status.signal() == Some(libc::SIGINT)
            && matches!(&result, Err(PermissionOutcome::AuthorizationUnavailable))
        {
            context.cancel();
            return Err(PermissionOutcome::Cancelled);
        }
    }
    result
}

pub(in crate::macos) fn agent_at(host: &Path) -> Option<PathBuf> {
    let directory = host.parent()?;
    let bundle = if directory.file_name().is_some_and(|name| name == "MacOS")
        && directory
            .parent()?
            .file_name()
            .is_some_and(|name| name == "Contents")
    {
        directory.parent()?.join("Library").join(AGENT_BUNDLE)
    } else {
        directory.join(AGENT_BUNDLE)
    };
    Some(bundle.join("Contents/MacOS").join(AGENT_EXECUTABLE))
}

/// Run the signed registration app as a bounded child, using kernel-authenticated IPC.
/// This only prepares a native service; it does not grant metadata or cleanup access.
#[cfg(target_os = "macos")]
pub fn register_from_host(
    kind: ServiceKind,
    context: &OperationContext,
) -> Result<RegistrationState, PermissionOutcome> {
    match run_trusted_agent(kind, AgentAction::Register, None, context)? {
        Completion::Registered(state) => Ok(state),
        Completion::Authorized => Err(PermissionOutcome::HelperUntrusted),
    }
}

#[cfg(target_os = "macos")]
pub(in crate::macos) fn refresh_from_host(
    kind: ServiceKind,
    context: &OperationContext,
) -> Result<RegistrationState, PermissionOutcome> {
    match run_trusted_agent(kind, AgentAction::Refresh, None, context)? {
        Completion::Registered(state) => Ok(state),
        Completion::Authorized => Err(PermissionOutcome::HelperUntrusted),
    }
}

#[cfg(target_os = "macos")]
pub(in crate::macos) fn authorize_from_host(
    kind: ServiceKind,
    service_nonce: &str,
    context: &OperationContext,
) -> Result<(), PermissionOutcome> {
    match run_trusted_agent(kind, AgentAction::Authorize, Some(service_nonce), context)? {
        Completion::Authorized => Ok(()),
        Completion::Registered(_) => Err(PermissionOutcome::HelperUntrusted),
    }
}

#[cfg(target_os = "macos")]
fn run_trusted_agent(
    kind: ServiceKind,
    action: AgentAction,
    service_nonce: Option<&str>,
    context: &OperationContext,
) -> Result<Completion, PermissionOutcome> {
    let bounded = context.child(Duration::from_secs(120));
    let context = &bounded;
    check(context)?;
    let team = signing::host_team()?;
    let identity = peer::current_identity().map_err(|error| io_outcome(error, context))?;
    let host = std::env::current_exe().map_err(|error| io_outcome(error, context))?;
    let executable = agent_at(&host).ok_or(PermissionOutcome::HelperMissing)?;
    let metadata = std::fs::symlink_metadata(&executable).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            PermissionOutcome::HelperMissing
        } else {
            PermissionOutcome::HelperUntrusted
        }
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    let expected_hash = signing::verify_file(&executable, &team, INSTALLER_IDENTIFIER)?;
    launch_agent(
        kind,
        action,
        service_nonce,
        context,
        |socket| {
            let mut command = Command::new(&executable);
            command.arg(AGENT_ARGUMENT).arg(socket);
            crate::process::restrict_system_environment(&mut command);
            command
        },
        |channel, pid| {
            let peer = channel.peer().ok_or(PermissionOutcome::HelperUntrusted)?;
            if peer.identity.pid != pid || !peer.identity.same_login(identity) {
                return Err(PermissionOutcome::HelperUntrusted);
            }
            peer.verify_code(&team, &[INSTALLER_IDENTIFIER], Some(&expected_hash))
        },
    )
}
