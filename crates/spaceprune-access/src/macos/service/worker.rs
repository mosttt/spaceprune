//! One supervised, fixed-capability process per host session. No OS authorization runs in launchd.

use super::*;
use crate::{macos::channel::Endpoint, process::ProcessTree};
use std::process::{Command as ProcessCommand, Stdio};

#[cfg(target_os = "macos")]
mod native;
mod server;
#[cfg(target_os = "macos")]
pub use native::run_from_environment;
#[cfg(all(test, target_os = "linux"))]
mod tests;
#[cfg(all(test, target_os = "linux"))]
pub(super) use tests::launch as launch_test;

const WORKER_VERSION: u16 = 2;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bootstrap {
    version: u16,
    nonce: String,
    kind: ServiceKind,
    caller_uid: u32,
    operation: OperationTransfer,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Initialize,
    Adopt { credential: ExternalAuthorization },
    Verify,
    Execute { body: serde_json::Value },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u16,
    nonce: String,
    sequence: u64,
    operation: OperationTransfer,
    action: Action,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    progress: bool,
    event_sequence: u64,
    version: u16,
    nonce: String,
    sequence: u64,
    operation: OperationIdentity,
    result: Result<serde_json::Value, PermissionOutcome>,
}

pub(super) struct Worker {
    process: ProcessTree,
    channel: Option<Channel>,
    nonce: String,
    sequence: u64,
}
impl Worker {
    fn launch(
        kind: ServiceKind,
        caller_uid: u32,
        operation: &OperationContext,
        command: impl FnOnce(&str) -> ProcessCommand,
        verify_peer: impl FnOnce(&Channel, u32) -> Result<(), PermissionOutcome>,
    ) -> Result<Self, PermissionOutcome> {
        check(operation)?;
        let endpoint = Endpoint::bind().map_err(|error| io_outcome(error, operation))?;
        let mut command = command(endpoint.name());
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut process =
            ProcessTree::spawn(&mut command).map_err(|error| io_outcome(error, operation))?;
        let mut channel = loop {
            check(operation)?;
            if let Some(channel) = endpoint
                .accept()
                .map_err(|error| io_outcome(error, operation))?
            {
                break channel;
            }
            if process
                .has_exited()
                .map_err(|error| io_outcome(error, operation))?
            {
                return Err(PermissionOutcome::AuthorizationUnavailable);
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        verify_peer(&channel, process.child_mut().id())?;
        channel.allow_service_payloads();
        let bootstrap = Bootstrap {
            version: WORKER_VERSION,
            nonce: uuid::Uuid::new_v4().to_string(),
            kind,
            caller_uid,
            operation: operation
                .transfer()
                .map_err(|error| io_outcome(error, operation))?,
        };
        channel
            .write(&bootstrap, operation)
            .map_err(|error| io_outcome(error, operation))?;
        let mut worker = Self {
            process,
            channel: Some(channel),
            nonce: bootstrap.nonce,
            sequence: 0,
        };
        worker.receive(operation, &|_| Err(PermissionOutcome::PolicyBlocked))?;
        Ok(worker)
    }

    pub(super) fn initialize(
        &mut self,
        operation: &OperationContext,
    ) -> Result<(), PermissionOutcome> {
        self.call(Action::Initialize, operation).map(|_| ())
    }

    pub(super) fn adopt(
        &mut self,
        credential: &ExternalAuthorization,
        operation: &OperationContext,
    ) -> Result<(), PermissionOutcome> {
        self.call(
            Action::Adopt {
                credential: ExternalAuthorization(credential.0),
            },
            operation,
        )
        .map(|_| ())
    }

    pub(super) fn verify(&mut self, operation: &OperationContext) -> Result<(), PermissionOutcome> {
        self.call(Action::Verify, operation).map(|_| ())
    }

    pub(super) fn execute(
        &mut self,
        request: &AuthorizedRequest,
        operation: &OperationContext,
        events: &dyn EventSink,
    ) -> Result<serde_json::Value, PermissionOutcome> {
        self.call_with_events(
            Action::Execute {
                body: request.body().clone(),
            },
            operation,
            events,
        )
    }

    fn call(
        &mut self,
        action: Action,
        operation: &OperationContext,
    ) -> Result<serde_json::Value, PermissionOutcome> {
        self.call_with_events(action, operation, &|_| {
            Err(PermissionOutcome::PolicyBlocked)
        })
    }

    fn call_with_events(
        &mut self,
        action: Action,
        operation: &OperationContext,
        events: &dyn EventSink,
    ) -> Result<serde_json::Value, PermissionOutcome> {
        let result = (|| {
            check(operation)?;
            self.sequence = self
                .sequence
                .checked_add(1)
                .ok_or(PermissionOutcome::PolicyBlocked)?;
            let request = Request {
                version: WORKER_VERSION,
                nonce: self.nonce.clone(),
                sequence: self.sequence,
                operation: operation
                    .transfer()
                    .map_err(|error| io_outcome(error, operation))?,
                action,
            };
            let channel = self
                .channel
                .as_mut()
                .ok_or(PermissionOutcome::AuthorizationUnavailable)?;
            channel.reset_budget();
            channel
                .write(&request, operation)
                .map_err(|error| io_outcome(error, operation))?;
            self.receive(operation, events)
        })();
        if result.is_err() {
            self.close()?;
        }
        result
    }

    fn receive(
        &mut self,
        operation: &OperationContext,
        events: &dyn EventSink,
    ) -> Result<serde_json::Value, PermissionOutcome> {
        let mut budget = StreamBudget::default();
        loop {
            let response: Response = self
                .channel
                .as_mut()
                .ok_or(PermissionOutcome::AuthorizationUnavailable)?
                .read(operation)
                .map_err(|error| io_outcome(error, operation))?;
            if response.version != WORKER_VERSION
                || response.nonce != self.nonce
                || response.sequence != self.sequence
                || response.operation != operation.identity()
            {
                return Err(PermissionOutcome::HelperUntrusted);
            }
            budget.accept(
                response.event_sequence,
                response.progress,
                &response,
                operation,
            )?;
            check(operation)?;
            if !response.progress {
                return response.result;
            }
            events.emit(response.result?)?;
        }
    }

    pub(super) fn close(&mut self) -> Result<(), PermissionOutcome> {
        self.channel.take();
        self.process
            .terminate_and_wait()
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
        Ok(())
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn legacy_daemon_entry(kind: ServiceKind, executable: &Path) -> bool {
    executable
        .file_name()
        .is_some_and(|name| name == kind.label())
}
