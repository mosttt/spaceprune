//! Shared cleanup protocol and acknowledged results; transports own platform authentication.
use super::*;
use std::{
    io,
    sync::{atomic::AtomicBool, mpsc},
};

use spaceprune_contracts::messages;
#[cfg(target_os = "linux")]
#[path = "session/linux.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "session/windows.rs"]
mod platform;

#[cfg(target_os = "macos")]
use super::macos as platform;

pub(super) struct Session {
    connection: platform::Connection,
    consumed: std::collections::BTreeSet<(usize, usize)>,
    #[cfg(target_os = "linux")]
    bootstrap: Option<(String, Grant)>,
    closed: Option<PluginResult<()>>,
    #[cfg(test)]
    close_failure: Option<PermissionOutcome>,
    // Leases retain this stable cell while a verified upgrade replaces the owned process.
    pub(super) alive: Arc<Mutex<Arc<AtomicBool>>>,
}

#[derive(Debug)]
pub(super) struct StartupFailure {
    pub(super) outcome: PermissionOutcome,
    pub(super) shutdown_unconfirmed: bool,
    pub(super) error: PluginError,
}

impl StartupFailure {
    pub(super) fn into_error(mut self) -> PluginError {
        self.error.authorization = Some(self.outcome);
        self.error
    }
}

impl From<PermissionOutcome> for StartupFailure {
    fn from(outcome: PermissionOutcome) -> Self {
        let message = match outcome {
            PermissionOutcome::Denied => messages::text("cleanup_access.denied"),
            PermissionOutcome::Cancelled => messages::text("cleanup_access.cancelled"),
            PermissionOutcome::TimedOut => messages::text("cleanup_access.timed_out"),
            PermissionOutcome::NeedsAuthorization => {
                messages::text("cleanup_access.interactive_required")
            }
            #[cfg(target_os = "windows")]
            PermissionOutcome::HelperMissing
                if matches!(
                    spaceprune_access::windows_native::embedded_identity(),
                    Ok(None)
                ) =>
            {
                messages::text("native.windows_components_not_bundled")
            }
            PermissionOutcome::HelperMissing | PermissionOutcome::HelperOutdated => {
                messages::text("cleanup_access.component_missing")
            }
            PermissionOutcome::HelperUntrusted => {
                messages::text("cleanup_access.component_untrusted")
            }
            _ => messages::text("cleanup_access.startup_unavailable"),
        };
        Self {
            outcome,
            shutdown_unconfirmed: false,
            error: error(messages::message(
                "cleanup_access.no_more_prompts",
                &[
                    ("message", message.to_owned()),
                    ("outcome", format!("{outcome:?}")),
                ],
            )),
        }
    }
}

pub(super) fn operation_failure(cause: io::Error) -> PermissionOutcome {
    match cause.kind() {
        io::ErrorKind::Interrupted => PermissionOutcome::Cancelled,
        io::ErrorKind::TimedOut => PermissionOutcome::TimedOut,
        _ => PermissionOutcome::AuthorizationUnavailable,
    }
}

impl Session {
    pub(super) fn start(
        method: &str,
        grant: &Grant,
        selection: &[Selection],
        progress: &dyn ProgressSink,
        operation: &OperationContext,
    ) -> Result<Self, StartupFailure> {
        #[cfg(target_os = "linux")]
        let connection = platform::Connection::start(
            method,
            grant,
            progress,
            operation,
            grant.requires_go_capture(selection),
        )?;
        #[cfg(not(target_os = "linux"))]
        let connection = {
            let _ = selection;
            platform::Connection::start(method, grant, progress, operation)?
        };
        Ok(Self {
            alive: Arc::new(Mutex::new(connection.alive.clone())),
            connection,
            consumed: Default::default(),
            #[cfg(target_os = "linux")]
            bootstrap: Some((method.to_owned(), grant.clone())),
            closed: None,
            #[cfg(test)]
            close_failure: None,
        })
    }

    #[cfg(all(test, target_os = "linux"))]
    fn launch(
        command: &mut std::process::Command,
        grant: &Grant,
        operation: &OperationContext,
    ) -> Result<Self, StartupFailure> {
        let connection = platform::Connection::launch(command, grant, operation)?;
        Ok(Self {
            alive: Arc::new(Mutex::new(connection.alive.clone())),
            connection,
            consumed: Default::default(),
            bootstrap: None,
            closed: None,
            #[cfg(test)]
            close_failure: None,
        })
    }

    pub(super) fn close(&mut self) -> PluginResult<()> {
        if let Some(result) = &self.closed {
            return result.clone();
        }
        let result = self.connection.close();
        #[cfg(test)]
        let result = result.and(self.close_failure.map_or(Ok(()), Err));
        let result = result.map_err(shutdown_failure);
        self.closed = Some(result.clone());
        result
    }

    pub(super) fn execute(
        &mut self,
        selection: &[Selection],
        groups: &[CleanupBatchGroup<'_>],
        progress: &dyn ProgressSink,
        operation: &OperationContext,
    ) -> PluginResult<Vec<PluginResult<HandlerCleanResult>>> {
        let mut bytes = serde_json::to_vec(&ExecuteRequest {
            operation: operation
                .transfer()
                .map_err(|cause| error(cause.to_string()))?,
            selection: selection.to_vec(),
        })
        .map_err(|_| error(messages::text("cleanup_access.invalid_request")))?;
        bytes.push(b'\n');
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(error(messages::text("cleanup_access.request_budget")));
        }
        operation
            .check()
            .map_err(|cause| error(cause.to_string()))?;
        #[cfg(target_os = "linux")]
        if !self.connection.supports_go_capture
            && self
                .bootstrap
                .as_ref()
                .is_some_and(|(_, grant)| grant.requires_go_capture(selection))
        {
            let (method, grant) = self.bootstrap.clone().expect("verified session bootstrap");
            // Prepare/upgrade the actual Go component before closing a usable legacy session.
            // A missing Go source does not disable later Snapper/filesystem batches.
            let command = platform::Connection::prepare_command(
                &method,
                progress,
                operation,
                true,
                grant.requires_developer_handlers(),
            )
            .map(|(command, _)| command);
            self.replace_connection(command, &grant, operation)?;
        }
        self.consume(selection)?;
        let mut confirmed = std::collections::BTreeSet::new();
        if self.connection.send(&bytes, operation).is_err() {
            return Err(self.unconfirmed(
                groups,
                &confirmed,
                progress,
                messages::text("cleanup_access.request_delivery"),
            ));
        }
        let mut last_output = Instant::now();
        let mut received_output = false;
        loop {
            if let Err(cause) = operation.check() {
                return Err(self.unconfirmed(
                    groups,
                    &confirmed,
                    progress,
                    &messages::message("cleanup_access.stopped", &[("cause", cause.to_string())]),
                ));
            }
            let response = self.connection.receive(Duration::from_millis(100));
            let idle = if received_output {
                operation.budget.idle_timeout
            } else {
                operation.budget.first_output_timeout
            };
            if last_output.elapsed() >= idle {
                return Err(self.unconfirmed(
                    groups,
                    &confirmed,
                    progress,
                    messages::text("cleanup_access.worker_timeout"),
                ));
            }
            if response.is_ok() {
                last_output = Instant::now();
                received_output = true;
            }
            if let Ok(Ok(response)) = &response
                && response.operation != operation.identity()
            {
                return Err(self.unconfirmed(
                    groups,
                    &confirmed,
                    progress,
                    messages::text("cleanup_access.response_identity"),
                ));
            }
            match response.map(|result| result.map(|response| response.reply)) {
                Ok(Ok(Reply::Progress { event })) => progress.emit(event),
                Ok(Ok(Reply::Count {
                    task,
                    succeeded,
                    bytes,
                    unknown,
                })) => progress.record_cleanup(&task, succeeded, bytes, unknown),
                Ok(Ok(Reply::ItemFinished { task, outcome })) => {
                    if !groups.iter().any(|group| {
                        group.task.module_id == task.module_id
                            && group.task.task_id == task.task_id
                            && group.task.handler_id == task.handler_id
                            && group.items.iter().any(|item| item.id == outcome.item_id)
                    }) {
                        return Err(self.unconfirmed(
                            groups,
                            &confirmed,
                            progress,
                            messages::text("cleanup_access.outcome_outside_selection"),
                        ));
                    }
                    let receipt = progress.cleanup_item_finished(&task, &outcome);
                    let accepted = receipt.is_ok();
                    if !matches!(&receipt, Err(error) if error.code == "cleanup-outcome-invalid") {
                        confirmed.insert((task.module_id, task.task_id, outcome.item_id));
                    }
                    let mut receipt =
                        serde_json::to_vec(&OutcomeReceipt { accepted }).expect("boolean receipt");
                    receipt.push(b'\n');
                    if self.connection.send(&receipt, operation).is_err() {
                        return Err(self.unconfirmed(
                            groups,
                            &confirmed,
                            progress,
                            messages::text("cleanup_access.receipt_delivery"),
                        ));
                    }
                }
                Ok(Ok(Reply::Results { results })) => return Ok(results),
                Ok(Err(cause)) => {
                    return Err(self.unconfirmed(
                        groups,
                        &confirmed,
                        progress,
                        &messages::message(
                            "cleanup_access.response_failed",
                            &[("cause", cause.to_string())],
                        ),
                    ));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                _ => {
                    return Err(self.unconfirmed(
                        groups,
                        &confirmed,
                        progress,
                        messages::text("cleanup_access.result_unconfirmed"),
                    ));
                }
            }
        }
    }
    fn consume(&mut self, selection: &[Selection]) -> PluginResult<()> {
        let mut next = std::collections::BTreeSet::new();
        for selected in selection {
            for &item in &selected.items {
                if self.consumed.contains(&(selected.group, item))
                    || !next.insert((selected.group, item))
                {
                    return Err(error(messages::text("cleanup_access.already_consumed")));
                }
            }
        }
        self.consumed.extend(next);
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn replace_connection(
        &mut self,
        prepared: Result<std::process::Command, StartupFailure>,
        grant: &Grant,
        operation: &OperationContext,
    ) -> PluginResult<()> {
        let mut command = prepared.map_err(StartupFailure::into_error)?;
        // A failed shutdown stops here. No replacement process or replay can be started.
        self.close()?;
        let mut connection = platform::Connection::launch(&mut command, grant, operation)
            .map_err(StartupFailure::into_error)?;
        connection.supports_go_capture = true;
        *self
            .alive
            .lock()
            .map_err(|_| shutdown_failure(PermissionOutcome::AuthorizationUnavailable))? =
            connection.alive.clone();
        self.connection = connection;
        self.closed = None;
        Ok(())
    }

    fn unconfirmed(
        &mut self,
        groups: &[CleanupBatchGroup<'_>],
        confirmed: &std::collections::BTreeSet<(String, String, String)>,
        progress: &dyn ProgressSink,
        reason: &str,
    ) -> PluginError {
        let authorization = self.close().err().and_then(|error| error.authorization);
        for group in groups {
            for item in group.items {
                let key = (
                    group.task.module_id.clone(),
                    group.task.task_id.clone(),
                    item.id.clone(),
                );
                if confirmed.contains(&key) {
                    continue;
                }
                let _ = progress.cleanup_item_finished(
                    group.task,
                    &spaceprune_plugin_api::ItemExecution {
                        item_id: item.id.clone(),
                        status: spaceprune_plugin_api::ItemExecutionStatus::Uncertain,
                        reclaimed_estimate: None,
                        reclaimed_lower_bound: 0,
                        failure: Some(spaceprune_plugin_api::ItemFailure {
                            authorization,
                            item_id: item.id.clone(),
                            io_failure: None,
                            message: reason.into(),
                        }),
                    },
                );
            }
        }
        let mut error = PluginError::new("cleanup-result-unconfirmed", reason);
        error.authorization = authorization;
        error
    }
}

pub(super) fn shutdown_failure(outcome: PermissionOutcome) -> PluginError {
    let mut failure = PluginError::new(
        "cleanup-stop-unconfirmed",
        messages::message(
            "cleanup_access.shutdown_unconfirmed",
            &[("outcome", format!("{outcome:?}"))],
        ),
    );
    failure.authorization = Some(outcome);
    failure
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
