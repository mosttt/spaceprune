//! Linux stdio transport, native authentication and real process ownership.
use super::*;
use spaceprune_access::process::ProcessTree;
use std::{
    io::Read,
    process::{ChildStdin, Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

use spaceprune_contracts::messages;
pub(super) struct Connection {
    process: ProcessTree,
    closed: Option<Result<(), PermissionOutcome>>,
    input: Option<ChildStdin>,
    pending_input: Option<mpsc::Receiver<io::Result<ChildStdin>>>,
    output: Option<mpsc::Receiver<io::Result<Response>>>,
    workers: Vec<std::thread::JoinHandle<()>>,
    writer_cancellation: spaceprune_access::CancellationToken,
    pub(super) alive: Arc<AtomicBool>,
    pub(super) supports_go_capture: bool,
}
impl Connection {
    pub(super) fn start(
        method: &str,
        grant: &Grant,
        progress: &dyn ProgressSink,
        operation: &OperationContext,
        requires_go_capture: bool,
    ) -> Result<Self, StartupFailure> {
        let (mut command, supports_go_capture) = Self::prepare_command(
            method,
            progress,
            operation,
            requires_go_capture,
            grant.requires_developer_handlers(),
        )?;
        let compatible = (!supports_go_capture).then(|| grant.without_go_selections());
        let mut connection = Self::launch(
            &mut command,
            compatible.as_ref().unwrap_or(grant),
            operation,
        )?;
        connection.supports_go_capture = supports_go_capture;
        Ok(connection)
    }

    pub(super) fn prepare_command(
        method: &str,
        progress: &dyn ProgressSink,
        operation: &OperationContext,
        requires_go_capture: bool,
        requires_developer_handlers: bool,
    ) -> Result<(Command, bool), StartupFailure> {
        if operation.interaction == spaceprune_access::Interaction::NonInteractive
            && method != "sudo"
        {
            return Err(PermissionOutcome::NeedsAuthorization.into());
        }
        operation.check().map_err(operation_failure)?;
        let executable = {
            let _input = spaceprune_plugin_api::ProgressInputGuard::new(progress);
            spaceprune_access::helper::ensure_cleanup_helper_with_handler_requirement(
                operation,
                requires_go_capture,
                requires_developer_handlers,
            )
            .map_err(|error| {
                component_startup_failure(error, requires_go_capture || requires_developer_handlers)
            })?
        };
        let supports_go_capture = requires_go_capture
            || spaceprune_access::helper::probe_protocol(
                &executable,
                spaceprune_access::go_cache::SUPPORT_ARGUMENT,
                operation,
            )
            .is_ok();
        operation.check().map_err(operation_failure)?;
        let program = spaceprune_readonly_protocol::client::trust::system_program(method)
            .ok_or(PermissionOutcome::AuthorizationUnavailable)?;
        if method == "sudo" {
            spaceprune_access::permission::with_cached_terminal_authorization(
                operation,
                || {
                    let mut probe = Command::new(&program);
                    probe.args(["-n", "-v"]);
                    let result = spaceprune_access::process::capture(probe, operation)
                        .map_err(operation_failure)?;
                    if result.status.success() {
                        Ok(())
                    } else {
                        Err(PermissionOutcome::NeedsAuthorization)
                    }
                },
                || {
                    use std::io::IsTerminal;
                    if !std::io::stdin().is_terminal() {
                        return Err(PermissionOutcome::NeedsAuthorization);
                    }
                    let _input = spaceprune_plugin_api::ProgressInputGuard::new(progress);
                    let mut command = Command::new(&program);
                    command.arg("-v");
                    let status = spaceprune_access::process::terminal_status(command, operation)
                        .map_err(operation_failure)?;
                    if status.success() {
                        Ok(())
                    } else {
                        Err(PermissionOutcome::Denied)
                    }
                },
            )?;
        }
        let mut command = Command::new(program);
        if method == "sudo" {
            command.args(["-n", "--"]);
        } else {
            command.arg("--disable-internal-agent");
        }
        let argument = if method == "sudo" {
            WORKER_FLAG
        } else {
            spaceprune_access::helper::HelperKind::Cleanup.session_argument()
        };
        command
            .arg(executable)
            .arg(argument)
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok((command, supports_go_capture))
    }

    pub(super) fn launch(
        command: &mut Command,
        grant: &Grant,
        operation: &OperationContext,
    ) -> Result<Self, StartupFailure> {
        let mut bytes = serde_json::to_vec(grant).map_err(|_| PermissionOutcome::PolicyBlocked)?;
        bytes.push(b'\n');
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(PermissionOutcome::PolicyBlocked.into());
        }
        let startup = operation.child(Duration::from_secs(120));
        startup.check().map_err(operation_failure)?;
        let mut process = ProcessTree::spawn(command).map_err(|cause| {
            let mut failure = error(messages::message(
                "cleanup_access.start_failed",
                &[("cause", cause.to_string())],
            ));
            failure.io_failure = Some(spaceprune_access::error::classify(&cause));
            StartupFailure {
                shutdown_unconfirmed: false,
                outcome: PermissionOutcome::AuthorizationUnavailable,
                error: failure,
            }
        })?;
        let mut input = process.child_mut().stdin.take().expect("piped input");
        let output = process.child_mut().stdout.take().expect("piped output");
        let mut errors = process.child_mut().stderr.take().expect("piped errors");
        let (tx, rx) = mpsc::sync_channel(8);
        let output_bytes = Arc::new(AtomicUsize::new(0));
        let error_bytes = output_bytes.clone();
        let error_tx = tx.clone();
        let output_limit = operation.budget.output_bytes.min(MAX_FRAME_BYTES);
        let line_limit = operation.budget.line_bytes.min(MAX_FRAME_BYTES);
        let stderr = std::thread::spawn(move || {
            let mut buffer = [0; 4096];
            loop {
                match errors.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        if error_bytes
                            .fetch_add(count, Ordering::Relaxed)
                            .saturating_add(count)
                            > output_limit
                        {
                            let _ = error_tx.send(Err(io::Error::other(messages::text(
                                "cleanup_access.output_budget",
                            ))));
                            break;
                        }
                    }
                }
            }
        });
        let writer_cancellation = startup.cancellation.clone();
        let (written, writer) = mpsc::channel();
        let write_context = startup.clone();
        let stdin = std::thread::spawn(move || {
            let result = spaceprune_access::process::write_pipe(&mut input, &bytes, &write_context)
                .map(|()| input);
            let _ = written.send(result);
        });
        let alive = Arc::new(AtomicBool::new(true));
        let reader_alive = alive.clone();
        let stdout = std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(output);
            loop {
                let reply = match worker::frame_with_limit(&mut reader, line_limit) {
                    Ok(Some(bytes)) => {
                        if output_bytes
                            .fetch_add(bytes.len(), Ordering::Relaxed)
                            .saturating_add(bytes.len())
                            > output_limit
                        {
                            Err(io::Error::other(messages::text(
                                "cleanup_access.output_budget",
                            )))
                        } else {
                            serde_json::from_slice::<Response>(&bytes).map_err(io::Error::other)
                        }
                    }
                    Ok(None) => break,
                    Err(error) => Err(error),
                };
                let failed = reply.is_err();
                if tx.send(reply).is_err() || failed {
                    break;
                }
            }
            reader_alive.store(false, Ordering::Release);
        });
        let mut session = Self {
            process,
            closed: None,
            input: None,
            pending_input: Some(writer),
            output: Some(rx),
            alive,
            workers: vec![stdin, stdout, stderr],
            writer_cancellation,
            supports_go_capture: false,
        };
        let deadline = startup.deadline;
        loop {
            startup.check().map_err(operation_failure)?;
            match session
                .output
                .as_ref()
                .expect("active response channel")
                .recv_timeout(Duration::from_millis(100))
            {
                Ok(Ok(Response {
                    operation: identity,
                    reply: Reply::Ready,
                })) if identity == operation.identity() => {
                    let input = session
                        .pending_input
                        .take()
                        .expect("startup writer")
                        .recv_timeout(Duration::from_secs(1))
                        .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?
                        .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
                    session.input = Some(input);
                    return Ok(session);
                }
                Ok(_) => return Err(PermissionOutcome::AuthorizationUnavailable.into()),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let code = session
                        .process
                        .child_mut()
                        .try_wait()
                        .ok()
                        .flatten()
                        .and_then(|status| status.code());
                    return Err(if code == Some(126) {
                        PermissionOutcome::Cancelled.into()
                    } else {
                        StartupFailure {
                            shutdown_unconfirmed: false,
                            outcome: PermissionOutcome::AuthorizationUnavailable,
                            error: error(messages::message(
                                "cleanup_access.startup_exit",
                                &[("code", format!("{code:?}"))],
                            )),
                        }
                    });
                }
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                    return Err(PermissionOutcome::TimedOut.into());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    pub(super) fn send(&mut self, bytes: &[u8], operation: &OperationContext) -> io::Result<()> {
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| io::Error::other(messages::text("cleanup_access.input_unavailable")))?;
        spaceprune_access::process::write_pipe(input, bytes, operation)
    }

    pub(super) fn receive(
        &self,
        timeout: Duration,
    ) -> Result<io::Result<Response>, mpsc::RecvTimeoutError> {
        self.output
            .as_ref()
            .expect("active response channel")
            .recv_timeout(timeout)
    }
    pub(super) fn close(&mut self) -> Result<(), PermissionOutcome> {
        if let Some(result) = self.closed {
            return result;
        }
        self.writer_cancellation.cancel();
        self.pending_input.take();
        self.input.take();
        self.output.take();
        self.alive.store(false, Ordering::Release);
        let mut result = self
            .process
            .terminate_and_wait()
            .map(|_| ())
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable);
        for worker in self.workers.drain(..) {
            result = result.and(
                worker
                    .join()
                    .map_err(|_| PermissionOutcome::AuthorizationUnavailable),
            );
        }
        self.closed = Some(result);
        result
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

pub(super) fn component_startup_failure(
    error: spaceprune_access::helper::InstallError,
    additional_capability: bool,
) -> StartupFailure {
    use spaceprune_access::helper::InstallError;
    let (outcome, message) = match error {
        InstallError::SourceMissing | InstallError::InstallerMissing => (
            PermissionOutcome::HelperMissing,
            Some("cleanup_access.bundle_missing"),
        ),
        InstallError::SourceOutdated => (
            PermissionOutcome::HelperOutdated,
            Some("cleanup_access.bundle_outdated"),
        ),
        InstallError::Authorization(outcome) => (outcome, None),
        InstallError::UnsafeDestination => (PermissionOutcome::HelperUntrusted, None),
        InstallError::Failed => (PermissionOutcome::AuthorizationUnavailable, None),
    };
    let mut failure = StartupFailure::from(outcome);
    if additional_capability
        && matches!(
            outcome,
            PermissionOutcome::HelperMissing | PermissionOutcome::HelperOutdated
        )
    {
        failure.error.code = "cleanup-capability-unavailable".into();
    }
    if let Some(message) = message {
        failure.error.message = messages::text(message).into();
    }
    failure
}
