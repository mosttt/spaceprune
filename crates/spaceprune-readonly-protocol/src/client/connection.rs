//! Common request transfer; operating-system connection ownership stays in each backend.

#[cfg(target_os = "linux")]
#[path = "linux_connection.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "windows_connection.rs"]
mod platform;

#[cfg(target_os = "macos")]
#[path = "macos_connection.rs"]
mod platform;

use crate::{PreparedRequest, ResponseOutcome};
use spaceprune_access::{OperationContext, PermissionOutcome};

pub(super) struct Connection {
    #[cfg(target_os = "linux")]
    pub(super) supports_developer_handlers: bool,
    inner: platform::Connection,
    #[cfg(test)]
    pub(super) close_failure: Option<PermissionOutcome>,
}

#[derive(Debug)]
pub(super) struct StartupFailure {
    pub(super) outcome: PermissionOutcome,
    pub(super) shutdown_unconfirmed: bool,
}
impl From<PermissionOutcome> for StartupFailure {
    fn from(outcome: PermissionOutcome) -> Self {
        Self {
            outcome,
            shutdown_unconfirmed: false,
        }
    }
}

impl Connection {
    pub(super) fn start(
        operation: &OperationContext,
        progress: &dyn spaceprune_plugin_api::ProgressSink,
        requires_developer_handlers: bool,
    ) -> Result<Self, StartupFailure> {
        #[cfg(not(target_os = "linux"))]
        let _ = requires_developer_handlers;
        #[cfg(not(target_os = "linux"))]
        let _input = spaceprune_plugin_api::ProgressInputGuard::new(progress);
        #[cfg(target_os = "macos")]
        let inner = platform::Connection::start(operation).map_err(|failure| StartupFailure {
            outcome: failure.outcome,
            shutdown_unconfirmed: failure.shutdown_unconfirmed,
        })?;
        #[cfg(target_os = "windows")]
        let inner = platform::Connection::start(operation)?;
        #[cfg(target_os = "linux")]
        let inner = platform::Connection::start(
            operation,
            &|active| progress.set_input_active(active),
            requires_developer_handlers,
        )?;
        Ok(Self {
            #[cfg(target_os = "linux")]
            supports_developer_handlers: inner.supports_developer_handlers,
            inner,
            #[cfg(test)]
            close_failure: None,
        })
    }

    pub(super) fn close(&mut self) -> Result<(), PermissionOutcome> {
        let result = self.inner.close();
        #[cfg(test)]
        if let Some(failure) = self.close_failure {
            return Err(failure);
        }
        result
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn launch(
        command: &mut std::process::Command,
        operation: &OperationContext,
    ) -> Result<Self, PermissionOutcome> {
        platform::Connection::launch(command, operation).map(|inner| Self {
            supports_developer_handlers: false,
            inner,
            close_failure: None,
        })
    }

    pub(super) fn is_alive(&mut self) -> bool {
        self.inner.is_alive()
    }
    pub(super) fn measure(
        &mut self,
        request: &PreparedRequest,
        operation: &OperationContext,
    ) -> Result<ResponseOutcome, PermissionOutcome> {
        #[cfg(target_os = "macos")]
        return self.inner.measure(request, operation);
        #[cfg(not(target_os = "macos"))]
        {
            self.inner.send(request, operation)?;
            if self.inner.wait(operation)?.trim_end() != "0" {
                return Err(PermissionOutcome::AuthorizationUnavailable);
            }
            request
                .receive_response_with_operation(operation)
                .map_err(|_| {
                    super::check(operation)
                        .err()
                        .unwrap_or(PermissionOutcome::AuthorizationUnavailable)
                })
        }
    }

    pub(super) fn scan<T: serde::de::DeserializeOwned + Send>(
        &mut self,
        request: &PreparedRequest,
        operation: &OperationContext,
    ) -> Result<(T, bool), PermissionOutcome> {
        #[cfg(target_os = "macos")]
        return self.inner.scan(request, operation);
        #[cfg(not(target_os = "macos"))]
        {
            self.inner.send(request, operation)?;
            let stopped = operation.cancellation.child();
            std::thread::scope(|scope| {
                let response = scope.spawn(|| request.receive_task::<T>(stopped.signal()));
                let acknowledgement = self.inner.wait(operation);
                if !matches!(&acknowledgement, Ok(line) if line.trim_end() == "0") {
                    let _ = request.cancel();
                    stopped.cancel();
                }
                let result = response.join();
                super::check(operation)?;
                if acknowledgement?.trim_end() != "0" {
                    return Err(PermissionOutcome::AuthorizationUnavailable);
                }
                result
                    .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?
                    .map_err(|_| PermissionOutcome::AuthorizationUnavailable)
            })
        }
    }
}
