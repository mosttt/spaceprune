use crate::{PreparedRequest, mailbox::Mailbox};
use spaceprune_access::{Interaction, OperationContext, PermissionOutcome};
use spaceprune_windows_elevation::{ElevatedExitStatus, ElevationError};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

pub(super) struct Connection {
    mailbox: Mailbox,
    closed: Option<Result<(), PermissionOutcome>>,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<ElevatedExitStatus, ElevationError>>>,
}

impl Connection {
    pub(super) fn start(operation: &OperationContext) -> Result<Self, PermissionOutcome> {
        super::super::check(operation)?;
        if operation.interaction == Interaction::NonInteractive
            && !spaceprune_windows_elevation::is_elevated()
        {
            return Err(PermissionOutcome::NeedsAuthorization);
        }
        let image = super::super::trust::prepare_helper(operation)?;
        let mailbox = Mailbox::create(operation).map_err(|_| PermissionOutcome::PolicyBlocked)?;
        let arguments = mailbox.arguments();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&cancelled);
        let worker = std::thread::spawn(move || {
            spaceprune_windows_elevation::run_elevated_and_wait(
                image.path(),
                &arguments,
                Duration::from_secs(24 * 60 * 60),
                &stopped,
            )
        });
        let mut connection = Self {
            mailbox,
            closed: None,
            cancelled,
            worker: Some(worker),
        };
        if connection.wait(operation)? != "0" {
            return Err(PermissionOutcome::HelperOutdated);
        }
        Ok(connection)
    }

    pub(super) fn is_alive(&mut self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
    }

    pub(super) fn send(
        &mut self,
        request: &PreparedRequest,
        operation: &OperationContext,
    ) -> Result<(), PermissionOutcome> {
        super::super::check(operation)?;
        self.mailbox
            .send(request)
            .map_err(|_| PermissionOutcome::PolicyBlocked)
    }

    pub(super) fn wait(
        &mut self,
        operation: &OperationContext,
    ) -> Result<String, PermissionOutcome> {
        loop {
            super::super::check(operation)?;
            if let Some(code) = self
                .mailbox
                .poll()
                .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?
            {
                return Ok(code.to_string());
            }
            if !self.is_alive() {
                let result = self
                    .worker
                    .take()
                    .ok_or(PermissionOutcome::AuthorizationUnavailable)
                    .and_then(|worker| {
                        worker
                            .join()
                            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)
                    });
                self.closed = Some(
                    result
                        .as_ref()
                        .map_err(|failure| *failure)
                        .and_then(shutdown_result),
                );
                return Err(match result? {
                    Err(ElevationError::UserDenied) => PermissionOutcome::Denied,
                    Err(ElevationError::Cancelled) => PermissionOutcome::Cancelled,
                    Err(ElevationError::TimedOut) => PermissionOutcome::TimedOut,
                    Ok(status) if status.code() == Some(64) => PermissionOutcome::HelperOutdated,
                    _ => PermissionOutcome::AuthorizationUnavailable,
                });
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    pub(super) fn close(&mut self) -> Result<(), PermissionOutcome> {
        if let Some(result) = self.closed {
            return result;
        }
        let revoked = self
            .mailbox
            .close()
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable);
        self.cancelled.store(true, Ordering::Release);
        let result = self.worker.take().map_or(Ok(()), |worker| {
            worker
                .join()
                .map_err(|_| PermissionOutcome::AuthorizationUnavailable)
                .and_then(|result| shutdown_result(&result))
        });
        let result = result.and(revoked);
        self.closed = Some(result);
        result
    }
}

fn shutdown_result(
    result: &Result<ElevatedExitStatus, ElevationError>,
) -> Result<(), PermissionOutcome> {
    match result {
        Ok(_)
        | Err(ElevationError::Cancelled | ElevationError::UserDenied | ElevationError::TimedOut) => {
            Ok(())
        }
        Err(_) => Err(PermissionOutcome::AuthorizationUnavailable),
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
