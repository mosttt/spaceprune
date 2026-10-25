//! Signed native component registration and authenticated, fixed-capability sessions.
//!
//! Registration is not a filesystem lease or a cleanup grant. Potentially blocking native
//! interaction belongs in the authorization agent; noninteractive policy and file work run
//! in a service-owned worker. Their owners enforce cancellation and wait for actual exit.

use crate::{Interaction, OperationContext, PermissionOutcome};
use serde::{Deserialize, Serialize};
use std::path::Path;
#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
use std::path::PathBuf;

mod agent;
#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
mod agent_session;
#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
mod channel;
mod code_policy;
#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
mod credential;
#[cfg(target_os = "macos")]
mod peer;
#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
mod principal;
#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
mod terminal;
#[cfg(target_os = "macos")]
pub use agent::run_installer_from_environment;
#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
use agent::{AgentAction, AgentReply, AgentRequest};
#[cfg(target_os = "macos")]
pub use agent_session::register_from_host;

#[cfg(target_os = "macos")]
mod authorization;
#[cfg(target_os = "macos")]
mod native;
#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
pub mod service;
#[cfg(target_os = "macos")]
mod signing;

use crate::macos_package::signing_requirement;
pub use crate::macos_package::{
    AGENT_ARGUMENT, AGENT_BUNDLE, AGENT_EXECUTABLE, AGENT_PROTOCOL, CLI_IDENTIFIER,
    DESKTOP_IDENTIFIER, INSTALLER_IDENTIFIER, SUPPORT_ARGUMENT, ServiceKind,
};

/// Registration status, deliberately distinct from `PermissionOutcome::Granted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationState {
    /// The service has not been registered or has been explicitly unregistered.
    NotRegistered,
    /// ServiceManagement reports that the registered service is eligible to run.
    Enabled,
    /// A trusted registered legacy executable differs from the bundled version.
    Outdated,
    /// The user must approve or re-enable the service in System Settings.
    RequiresApproval,
    /// ServiceManagement cannot locate the service registration.
    NotFound,
}

trait RegistrationBackend {
    fn status(&self) -> Result<RegistrationState, PermissionOutcome>;
    fn register(&self, context: &OperationContext) -> Result<(), PermissionOutcome>;
    fn refresh(&self, context: &OperationContext) -> Result<(), PermissionOutcome>;
    fn open_settings(&self) -> Result<(), PermissionOutcome>;
}

fn check(context: &OperationContext) -> Result<(), PermissionOutcome> {
    context.check().map_err(|error| match error.kind() {
        std::io::ErrorKind::Interrupted => PermissionOutcome::Cancelled,
        std::io::ErrorKind::TimedOut => PermissionOutcome::TimedOut,
        _ => PermissionOutcome::AuthorizationUnavailable,
    })
}

fn ensure_with(
    backend: &impl RegistrationBackend,
    context: &OperationContext,
) -> Result<RegistrationState, PermissionOutcome> {
    ensure_registration(backend, context, false)
}

fn ensure_registration(
    backend: &impl RegistrationBackend,
    context: &OperationContext,
    refresh: bool,
) -> Result<RegistrationState, PermissionOutcome> {
    check(context)?;
    let mut state = backend.status()?;
    if state == RegistrationState::Outdated || (refresh && state == RegistrationState::Enabled) {
        if context.interaction == Interaction::NonInteractive {
            return Err(PermissionOutcome::HelperOutdated);
        }
        backend.refresh(context)?;
        check(context)?;
        state = backend.status()?;
        if state == RegistrationState::Outdated {
            return Err(PermissionOutcome::HelperOutdated);
        }
        if matches!(
            state,
            RegistrationState::NotRegistered | RegistrationState::NotFound
        ) {
            return Err(PermissionOutcome::AuthorizationUnavailable);
        }
    }
    if matches!(
        state,
        RegistrationState::NotRegistered | RegistrationState::NotFound
    ) {
        if context.interaction == Interaction::NonInteractive {
            return Err(PermissionOutcome::NeedsAuthorization);
        }
        check(context)?;
        backend.register(context)?;
        check(context)?;
        state = backend.status()?;
        if state == RegistrationState::Outdated {
            return Err(PermissionOutcome::HelperOutdated);
        }
        if matches!(
            state,
            RegistrationState::NotRegistered | RegistrationState::NotFound
        ) {
            return Err(PermissionOutcome::AuthorizationUnavailable);
        }
    }
    if state == RegistrationState::RequiresApproval
        && context.interaction == Interaction::Foreground
    {
        check(context)?;
        backend.open_settings()?;
    }
    check(context)?;
    Ok(state)
}

/// Verify the signed bundle and register its fixed service using the supported native API.
///
/// Invoke only inside the dedicated installer process: native authorization may block, and
/// the caller must stop that process when its original operation is cancelled or expires.
#[cfg(target_os = "macos")]
pub fn ensure_registered(
    kind: ServiceKind,
    context: &OperationContext,
) -> Result<RegistrationState, PermissionOutcome> {
    check(context)?;
    let backend = native::Backend::for_current_bundle(kind)?;
    ensure_with(&backend, context)
}

#[cfg(target_os = "macos")]
fn refresh_registered(
    kind: ServiceKind,
    context: &OperationContext,
) -> Result<RegistrationState, PermissionOutcome> {
    check(context)?;
    let backend = native::Backend::for_current_bundle(kind)?;
    ensure_registration(&backend, context, true)
}

#[cfg(test)]
mod tests;
