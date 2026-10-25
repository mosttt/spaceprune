//! Fixed native service/authorization actions in a killable, signed app process.

use super::*;
use crate::{OperationTransfer, ResourceBudget};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum AgentAction {
    Register,
    Refresh,
    Authorize,
}

/// Installer request bound to the original host operation's remaining time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AgentRequest {
    pub(super) action: AgentAction,
    pub(super) service_nonce: Option<String>,
    /// Fixed service capability.
    pub kind: ServiceKind,
    /// Original operation identity, monotonic deadline and budgets.
    pub operation: OperationTransfer,
    /// Noninteractive operations never start a native authorization prompt.
    pub interaction: Interaction,
}

/// A registration receipt is not an authorization or execution grant.
#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum AgentReply {
    /// The verified service accepted the external authorization before the agent exited.
    Authorized,
    /// Actual status observed after the native API returned.
    Observed {
        /// Current OS service-registration state.
        state: RegistrationState,
    },
    /// Registration could not be established or verified.
    Failed {
        /// Structured outcome without native credentials or passwords.
        outcome: PermissionOutcome,
    },
}

impl AgentRequest {
    /// Preserve the caller's original identity and remaining deadline.
    pub fn new(kind: ServiceKind, context: &OperationContext) -> std::io::Result<Self> {
        Ok(Self {
            action: AgentAction::Register,
            service_nonce: None,
            kind,
            operation: context.transfer()?,
            interaction: context.interaction,
        })
    }

    pub(super) fn context(&self) -> Result<OperationContext, PermissionOutcome> {
        let mut context = OperationContext::from_transfer(
            &self.operation,
            Duration::from_secs(120),
            ResourceBudget::workflow(),
        )
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::TimedOut {
                PermissionOutcome::TimedOut
            } else {
                PermissionOutcome::PolicyBlocked
            }
        })?;
        context.interaction = self.interaction;
        Ok(context)
    }
}

#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
pub(super) struct Watchdog {
    finished: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
impl Drop for Watchdog {
    fn drop(&mut self) {
        self.finished
            .store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
pub(super) fn watch_host(
    mut monitor: super::channel::Monitor,
    context: OperationContext,
    stop_process: fn() -> !,
) -> Result<Watchdog, PermissionOutcome> {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let finished = Arc::new(AtomicBool::new(false));
    let done = Arc::clone(&finished);
    let thread = std::thread::Builder::new()
        .name("native-registration-owner".into())
        .spawn(move || {
            while !done.load(Ordering::Acquire) {
                if context.is_stopped() {
                    stop_process();
                }
                if !monitor.is_alive() {
                    stop_process();
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
        .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
    Ok(Watchdog {
        finished,
        thread: Some(thread),
    })
}

/// Dispatch only the authenticated Mach protocol in the dedicated signed registration app.
#[cfg(target_os = "macos")]
pub fn run_installer_from_environment() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    let command = args.next()?;
    if command == SUPPORT_ARGUMENT {
        return Some(if args.next().is_none() { 0 } else { 64 });
    }
    if command != AGENT_ARGUMENT {
        return None;
    }
    let Some(name) = args.next().and_then(|value| value.into_string().ok()) else {
        return Some(64);
    };
    if args.next().is_some() {
        return Some(64);
    }
    Some(run_channel(&name).unwrap_or(77))
}

#[cfg(target_os = "macos")]
fn run_channel(name: &str) -> Result<i32, PermissionOutcome> {
    let team = signing::own_team()?;
    let own = peer::current_identity().map_err(|_| PermissionOutcome::HelperUntrusted)?;
    let startup = OperationContext::workflow(Default::default(), Interaction::NonInteractive)
        .child(Duration::from_secs(5));
    let channel = super::channel::Channel::connect(name, &startup)
        .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
    super::agent_session::serve_agent(
        channel,
        |channel| {
            let parent = channel.peer().ok_or(PermissionOutcome::HelperUntrusted)?;
            if !parent.identity.same_login(own)
                || rustix::process::getppid().map(|pid| pid.as_raw_nonzero().get() as u32)
                    != Some(parent.identity.pid)
            {
                return Err(PermissionOutcome::HelperUntrusted);
            }
            parent.verify_code(&team, &[CLI_IDENTIFIER, DESKTOP_IDENTIFIER], None)
        },
        |action, kind, challenge, context| match action {
            AgentAction::Register => {
                ensure_registered(kind, context).map(super::agent_session::AgentResult::Registered)
            }
            AgentAction::Refresh => {
                refresh_registered(kind, context).map(super::agent_session::AgentResult::Registered)
            }
            AgentAction::Authorize => {
                service::authorize_from_agent(
                    kind,
                    challenge.ok_or(PermissionOutcome::PolicyBlocked)?,
                    context,
                )?;
                Ok(super::agent_session::AgentResult::Authorized)
            }
        },
        stop_agent,
    )
}

#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
#[allow(unsafe_code)]
pub(super) fn stop_agent() -> ! {
    // SAFETY: Only the dedicated, verified agent uses this watchdog. Do not run native atexit
    // handlers while another thread may be blocked in an OS authorization call.
    unsafe {
        let group = libc::getpgrp();
        if group == libc::getpid() {
            // A terminal Security child shares our isolated group. Stop it as well if
            // the host disappears; leave the host's foreground group untouched.
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
            libc::killpg(group, libc::SIGTERM);
            std::thread::sleep(Duration::from_millis(200));
            libc::killpg(group, libc::SIGKILL);
        }
        libc::_exit(75)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registration_cannot_accept_caller_selected_paths_or_reset_the_deadline() {
        let context = OperationContext::workflow(Default::default(), Interaction::Foreground);
        let request = AgentRequest::new(ServiceKind::Metadata, &context).unwrap();
        let restored = request.context().unwrap();
        assert_eq!(restored.identity(), context.identity());
        assert!(restored.deadline <= context.deadline);
        let mut value = serde_json::to_value(&request).unwrap();
        value["path"] = serde_json::json!("/tmp/not-a-service");
        assert!(serde_json::from_value::<AgentRequest>(value).is_err());
        let mut expired = request;
        expired.operation.deadline_monotonic_ns = 0;
        assert_eq!(expired.context().unwrap_err(), PermissionOutcome::TimedOut);
    }
}
