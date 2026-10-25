//! Native transport and process entrypoints. Scope and result accounting stay in shared Core code.

use super::*;
use spaceprune_access::macos::{
    ServiceKind,
    service::{self, ServiceClient},
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};

struct NativeBackend(ServiceClient);

fn service_failure(outcome: PermissionOutcome, message: &str) -> PluginError {
    super::super::session::StartupFailure {
        outcome,
        shutdown_unconfirmed: false,
        error: error(format!("{message}: {outcome:?}")),
    }
    .into_error()
}

fn shutdown_failure(outcome: PermissionOutcome) -> super::super::session::StartupFailure {
    super::super::session::StartupFailure {
        outcome,
        shutdown_unconfirmed: true,
        error: PluginError::new(
            "cleanup-result-unconfirmed",
            format!(
                "Cleanup service shutdown was not confirmed: {outcome:?}; do not retry this session"
            ),
        ),
    }
}

impl Backend for NativeBackend {
    fn request(
        &mut self,
        request: Request,
        context: &OperationContext,
        events: &mut dyn FnMut(Reply) -> PluginResult<()>,
    ) -> PluginResult<NativeReply> {
        self.0
            .send(&request, context)
            .map_err(|outcome| service_failure(outcome, "Cleanup service request failed"))?;
        self.0
            .wait_with_events(context, |event| {
                let event =
                    serde_json::from_value(event).map_err(|_| PermissionOutcome::PolicyBlocked)?;
                events(event).map_err(|_| PermissionOutcome::PolicyBlocked)
            })
            .map_err(|outcome| service_failure(outcome, "Cleanup service result failed"))
    }
}

pub(in crate::cleanup_access) struct Connection {
    input: Option<mpsc::SyncSender<Vec<u8>>>,
    output: Option<mpsc::Receiver<io::Result<Response>>>,
    worker: Option<std::thread::JoinHandle<Result<(), PermissionOutcome>>>,
    closed: Option<Result<(), PermissionOutcome>>,
    lifetime: OperationContext,
    pub(in crate::cleanup_access) alive: Arc<AtomicBool>,
}
impl Connection {
    pub(in crate::cleanup_access) fn start(
        method: &str,
        grant: &Grant,
        progress: &dyn ProgressSink,
        operation: &OperationContext,
    ) -> Result<Self, super::super::session::StartupFailure> {
        if method != "native" {
            return Err(PermissionOutcome::Unsupported.into());
        }
        operation
            .check()
            .map_err(super::super::session::operation_failure)?;
        let _input = spaceprune_plugin_api::ProgressInputGuard::new(progress);
        let service =
            ServiceClient::connect(ServiceKind::Cleanup, operation).map_err(|failure| {
                if failure.shutdown_unconfirmed {
                    shutdown_failure(failure.outcome)
                } else {
                    failure.outcome.into()
                }
            })?;
        let mut backend = NativeBackend(service);
        let prepared = backend
            .request(
                Request::Prepare {
                    grant: grant.clone(),
                },
                operation,
                &mut |_| Err(error("Unexpected cleanup preparation event")),
            )
            .map_err(|error| super::super::session::StartupFailure {
                shutdown_unconfirmed: false,
                outcome: error
                    .authorization
                    .unwrap_or(PermissionOutcome::PolicyBlocked),
                error,
            });
        let ready = match prepared {
            Ok(ready) => ready,
            Err(failure) => {
                backend.0.close().map_err(shutdown_failure)?;
                return Err(failure);
            }
        };
        if !matches!(ready, NativeReply::Ready) {
            backend.0.close().map_err(shutdown_failure)?;
            return Err(PermissionOutcome::PolicyBlocked.into());
        }
        let lifetime =
            operation.child(operation.deadline.saturating_duration_since(Instant::now()));
        let context = lifetime.clone();
        let grant = grant.clone();
        let alive = Arc::new(AtomicBool::new(true));
        let running = alive.clone();
        let (input, requests) = mpsc::sync_channel(1);
        let (responses, output) = mpsc::sync_channel(8);
        let worker = std::thread::spawn(move || {
            if let Err(failure) = driver::run(&mut backend, &grant, &requests, &responses, &context)
            {
                let _ = driver::send(
                    &responses,
                    Err(io::Error::other(failure.to_string())),
                    &context,
                );
            }
            let stopped = backend.0.close();
            running.store(false, Ordering::Release);
            stopped
        });
        Ok(Self {
            input: Some(input),
            output: Some(output),
            worker: Some(worker),
            closed: None,
            lifetime,
            alive,
        })
    }
    pub(in crate::cleanup_access) fn send(
        &mut self,
        bytes: &[u8],
        operation: &OperationContext,
    ) -> io::Result<()> {
        if bytes.len() > MAX_FRAME_BYTES || bytes.len() > operation.budget.line_bytes {
            return Err(io::Error::other(
                "Cleanup request exceeds the protocol budget",
            ));
        }
        driver::send(
            self.input
                .as_ref()
                .ok_or_else(|| io::Error::other("Cleanup session is closed"))?,
            bytes.to_vec(),
            operation,
        )
        .map_err(|failure| io::Error::other(failure.to_string()))
    }
    pub(in crate::cleanup_access) fn receive(
        &self,
        timeout: Duration,
    ) -> Result<io::Result<Response>, mpsc::RecvTimeoutError> {
        self.output
            .as_ref()
            .ok_or(mpsc::RecvTimeoutError::Disconnected)?
            .recv_timeout(timeout)
    }
    pub(in crate::cleanup_access) fn close(&mut self) -> Result<(), PermissionOutcome> {
        if let Some(result) = self.closed {
            return result;
        }
        self.lifetime.cancel();
        self.input.take();
        self.output.take();
        let result = self
            .worker
            .take()
            .ok_or(PermissionOutcome::AuthorizationUnavailable)
            .and_then(|worker| {
                worker
                    .join()
                    .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?
            });
        self.alive.store(false, Ordering::Release);
        self.closed = Some(result);
        result
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

pub(in crate::cleanup_access) fn run_if_requested() -> Option<i32> {
    let state = Mutex::new(server::State::default());
    service::run_from_environment(ServiceKind::Cleanup, |request, operation, events| {
        let request: Request = serde_json::from_value(request.body().clone())
            .map_err(|_| PermissionOutcome::PolicyBlocked)?;
        let reply = state
            .lock()
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?
            .request(request, operation, &|reply| {
                let event = serde_json::to_value(reply)
                    .map_err(|_| error("Invalid cleanup progress event"))?;
                events.emit(event).map_err(|outcome| {
                    error(format!("Cleanup progress delivery failed: {outcome:?}"))
                })
            })
            .map_err(|_| PermissionOutcome::PolicyBlocked)?;
        serde_json::to_value(reply).map_err(|_| PermissionOutcome::PolicyBlocked)
    })
}
