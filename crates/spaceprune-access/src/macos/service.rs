//! Fixed-capability launchd sessions; OS credentials are never a substitute for request scope.

#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
mod client;
mod connection;
mod daemon;
mod events;
pub use events::EventSink;
use events::{EventWriter, StreamBudget};
mod handoff;
mod termination;
mod worker;
#[cfg(target_os = "macos")]
pub(super) use client::authorize_from_agent;
#[cfg(target_os = "macos")]
pub use client::{ConnectFailure, ServiceClient};
pub use daemon::AuthorizedRequest;
#[cfg(target_os = "macos")]
pub use worker::run_from_environment;

use super::{channel::Channel, credential::ExternalAuthorization, *};
use crate::{OperationIdentity, OperationTransfer, ResourceBudget};
use connection::while_connected;
use std::{io, time::Duration};

const VERSION: u16 = 4;
const IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    version: u16,
    kind: ServiceKind,
    nonce: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Cancel {
        nonce: String,
        sequence: u64,
        operation: OperationIdentity,
    },
    Prepare {
        nonce: String,
        operation: OperationTransfer,
    },
    Attach {
        nonce: String,
        target_nonce: String,
        operation: OperationTransfer,
    },
    Adopt {
        nonce: String,
        credential: ExternalAuthorization,
    },
    Execute {
        nonce: String,
        sequence: u64,
        operation: OperationTransfer,
        request: serde_json::Value,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Prepared,
    Authorized,
    Progress,
    Completed,
    Stopped,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    event_sequence: u64,
    phase: Phase,
    version: u16,
    nonce: String,
    sequence: u64,
    operation: OperationIdentity,
    result: Result<serde_json::Value, PermissionOutcome>,
}

fn io_outcome(error: io::Error, context: &OperationContext) -> PermissionOutcome {
    match context.check().err().unwrap_or(error).kind() {
        io::ErrorKind::Interrupted => PermissionOutcome::Cancelled,
        io::ErrorKind::TimedOut => PermissionOutcome::TimedOut,
        io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput => {
            PermissionOutcome::PolicyBlocked
        }
        _ => PermissionOutcome::AuthorizationUnavailable,
    }
}

fn transferred(operation: &OperationTransfer) -> Result<OperationContext, PermissionOutcome> {
    OperationContext::from_transfer(
        operation,
        Duration::from_secs(3600),
        ResourceBudget::workflow(),
    )
    .map_err(|error| match error.kind() {
        io::ErrorKind::TimedOut => PermissionOutcome::TimedOut,
        _ => PermissionOutcome::PolicyBlocked,
    })
}

fn idle_context() -> OperationContext {
    OperationContext::new(
        Default::default(),
        Interaction::NonInteractive,
        IDLE_TIMEOUT,
    )
}

// Only a peer already verified as the expected publisher and role can be classified outdated.
fn connect_current<T>(
    operation: &OperationContext,
    mut connect: impl FnMut() -> Result<T, PermissionOutcome>,
    refresh: impl FnOnce() -> Result<RegistrationState, PermissionOutcome>,
) -> Result<T, PermissionOutcome> {
    check(operation)?;
    match connect() {
        Err(PermissionOutcome::HelperOutdated)
            if operation.interaction != Interaction::NonInteractive =>
        {
            check(operation)?;
            match refresh()? {
                RegistrationState::Enabled => {}
                RegistrationState::RequiresApproval => {
                    return Err(PermissionOutcome::NeedsSystemSettings);
                }
                _ => return Err(PermissionOutcome::AuthorizationUnavailable),
            }
            check(operation)?;
            connect()
        }
        result => result,
    }
}

#[cfg(all(test, target_os = "linux"))]
mod upgrade_tests;
