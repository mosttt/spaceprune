//! One verified native agent per original operation; service adoption precedes authorization completion.

use super::channel::{Channel, Endpoint};
use super::*;
use crate::OperationIdentity;
use std::{io, time::Duration};

mod host;
pub(super) use host::agent_at;
#[cfg(target_os = "macos")]
pub use host::register_from_host;
#[cfg(test)]
use host::{Completion, launch, launch_agent};
#[cfg(target_os = "macos")]
pub(super) use host::{authorize_from_host, refresh_from_host};

const PROTOCOL: u16 = AGENT_PROTOCOL;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Call {
    protocol: u16,
    nonce: String,
    pub(super) request: AgentRequest,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum Event {
    Ready,
    Finished { reply: AgentReply },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    protocol: u16,
    nonce: String,
    kind: ServiceKind,
    action: AgentAction,
    operation: OperationIdentity,
    event: Event,
}

impl Call {
    fn receipt(&self, event: Event) -> Receipt {
        Receipt {
            protocol: PROTOCOL,
            nonce: self.nonce.clone(),
            kind: self.request.kind,
            action: self.request.action,
            operation: self.request.operation.identity.clone(),
            event,
        }
    }
    fn matches(&self, receipt: &Receipt) -> bool {
        receipt.protocol == PROTOCOL
            && receipt.nonce == self.nonce
            && receipt.kind == self.request.kind
            && receipt.action == self.request.action
            && receipt.operation == self.request.operation.identity
    }
}

fn io_outcome(error: io::Error, context: &OperationContext) -> PermissionOutcome {
    if let Err(outcome) = check(context) {
        return outcome;
    }
    match error.kind() {
        io::ErrorKind::Interrupted => PermissionOutcome::Cancelled,
        io::ErrorKind::TimedOut => PermissionOutcome::TimedOut,
        io::ErrorKind::InvalidData => PermissionOutcome::PolicyBlocked,
        _ => PermissionOutcome::AuthorizationUnavailable,
    }
}

pub(super) enum AgentResult {
    Registered(RegistrationState),
    Authorized,
}

#[cfg(test)]
fn serve(
    channel: Channel,
    verify_host: impl FnOnce(&Channel) -> Result<(), PermissionOutcome>,
    execute: impl FnOnce(ServiceKind, &OperationContext) -> Result<RegistrationState, PermissionOutcome>,
    stop_process: fn() -> !,
) -> Result<i32, PermissionOutcome> {
    serve_agent(
        channel,
        verify_host,
        |action, kind, _, context| {
            if action != AgentAction::Register {
                return Err(PermissionOutcome::PolicyBlocked);
            }
            execute(kind, context).map(AgentResult::Registered)
        },
        stop_process,
    )
}

pub(super) fn serve_agent(
    mut channel: Channel,
    verify_host: impl FnOnce(&Channel) -> Result<(), PermissionOutcome>,
    execute: impl FnOnce(
        AgentAction,
        ServiceKind,
        Option<&str>,
        &OperationContext,
    ) -> Result<AgentResult, PermissionOutcome>,
    stop_process: fn() -> !,
) -> Result<i32, PermissionOutcome> {
    let startup = OperationContext::workflow(Default::default(), Interaction::NonInteractive)
        .child(Duration::from_secs(5));
    let call: Call = channel
        .read(&startup)
        .map_err(|error| io_outcome(error, &startup))?;
    verify_host(&channel)?;
    if call.protocol != PROTOCOL || uuid::Uuid::parse_str(&call.nonce).is_err() {
        return Err(PermissionOutcome::HelperOutdated);
    }
    let challenge_valid = call
        .request
        .service_nonce
        .as_ref()
        .is_some_and(|nonce| uuid::Uuid::parse_str(nonce).is_ok());
    if (call.request.action == AgentAction::Authorize && !challenge_valid)
        || (matches!(
            call.request.action,
            AgentAction::Register | AgentAction::Refresh
        ) && call.request.service_nonce.is_some())
    {
        return Err(PermissionOutcome::PolicyBlocked);
    }
    let context = call.request.context()?;
    let monitor = channel
        .monitor()
        .map_err(|error| io_outcome(error, &context))?;
    let _watchdog = super::agent::watch_host(monitor, context.clone(), stop_process)?;
    channel
        .write(&call.receipt(Event::Ready), &context)
        .map_err(|error| io_outcome(error, &context))?;
    let (reply, exit) = match execute(
        call.request.action,
        call.request.kind,
        call.request.service_nonce.as_deref(),
        &context,
    ) {
        Ok(AgentResult::Registered(state))
            if matches!(
                call.request.action,
                AgentAction::Register | AgentAction::Refresh
            ) =>
        {
            (
                AgentReply::Observed { state },
                if state == RegistrationState::Enabled {
                    0
                } else {
                    75
                },
            )
        }
        Ok(AgentResult::Authorized) if call.request.action == AgentAction::Authorize => {
            (AgentReply::Authorized, 0)
        }
        Ok(_) => return Err(PermissionOutcome::PolicyBlocked),
        Err(outcome) => (AgentReply::Failed { outcome }, 1),
    };
    channel
        .write(&call.receipt(Event::Finished { reply }), &context)
        .map_err(|error| io_outcome(error, &context))?;
    Ok(exit)
}

#[cfg(test)]
mod tests;
