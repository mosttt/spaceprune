//! UAC transport for one immutable cleanup plan, authenticated by actual named-pipe peer IDs.

use super::*;
use crate::cleanup_access::windows_bridge::{self, PipeReader, Prepared};
use spaceprune_contracts::messages;
use spaceprune_windows_elevation::{ElevatedExitStatus, ElevationError};
use std::{io::BufReader, sync::atomic::Ordering, thread::JoinHandle};

pub(super) struct Connection {
    prepared: Prepared,
    closed: Option<Result<(), PermissionOutcome>>,
    cancelled: Arc<AtomicBool>,
    process: Option<JoinHandle<Result<ElevatedExitStatus, ElevationError>>>,
    output: Option<mpsc::Receiver<io::Result<Response>>>,
    reader: Option<JoinHandle<()>>,
    reader_context: OperationContext,
    pub(super) alive: Arc<AtomicBool>,
}

impl Connection {
    pub(super) fn start(
        method: &str,
        grant: &Grant,
        progress: &dyn ProgressSink,
        operation: &OperationContext,
    ) -> Result<Self, StartupFailure> {
        operation.check().map_err(operation_failure)?;
        if method != "uac" {
            return Err(PermissionOutcome::Unsupported.into());
        }
        if operation.interaction == spaceprune_access::Interaction::NonInteractive
            && !spaceprune_windows_elevation::is_elevated()
        {
            return Err(PermissionOutcome::NeedsAuthorization.into());
        }
        let _input = spaceprune_plugin_api::ProgressInputGuard::new(progress);
        let image = windows_bridge::probe(operation)?;
        let prepared = Prepared::create(operation).map_err(operation_failure)?;
        let arguments = prepared.arguments();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stopping = cancelled.clone();
        let lifetime = operation.deadline.saturating_duration_since(Instant::now());
        let (started, pid) = mpsc::channel();
        let process = std::thread::spawn(move || {
            spaceprune_windows_elevation::run_elevated_with_started(
                image.path(),
                &arguments,
                lifetime,
                &stopping,
                |id| {
                    let _ = started.send(id);
                },
            )
        });
        let reader_context = operation.child(lifetime);
        let mut connection = Self {
            prepared,
            closed: None,
            cancelled,
            process: Some(process),
            output: None,
            reader: None,
            reader_context,
            alive: Arc::new(AtomicBool::new(false)),
        };
        let startup = operation.child(Duration::from_secs(120));
        let mut process_id = None;
        loop {
            startup.check().map_err(operation_failure)?;
            if let Ok(pid) = pid.try_recv() {
                process_id = Some(pid);
            }
            if connection
                .process
                .as_ref()
                .is_some_and(|process| process.is_finished())
            {
                return Err(connection.exit_failure());
            }
            if let Some(pid) = process_id
                && connection
                    .prepared
                    .pipe
                    .poll_connected()
                    .map_err(operation_failure)?
            {
                if connection
                    .prepared
                    .pipe
                    .peer_process_id()
                    .map_err(operation_failure)?
                    == pid
                {
                    break;
                }
                connection.prepared.pipe.disconnect();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        connection
            .start_reader(operation)
            .map_err(operation_failure)?;
        let mut bytes = serde_json::to_vec(grant).map_err(|_| PermissionOutcome::PolicyBlocked)?;
        bytes.push(b'\n');
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(PermissionOutcome::PolicyBlocked.into());
        }
        connection
            .send(&bytes, &startup)
            .map_err(operation_failure)?;
        loop {
            startup.check().map_err(operation_failure)?;
            match connection.receive(Duration::from_millis(20)) {
                Ok(Ok(Response {
                    operation: identity,
                    reply: Reply::Ready,
                })) if identity == operation.identity() => return Ok(connection),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                _ => return Err(PermissionOutcome::AuthorizationUnavailable.into()),
            }
        }
    }

    fn start_reader(&mut self, operation: &OperationContext) -> io::Result<()> {
        let input = PipeReader {
            pipe: self.prepared.pipe.try_clone()?,
            context: self.reader_context.clone(),
        };
        let (tx, rx) = mpsc::sync_channel(8);
        let alive = self.alive.clone();
        alive.store(true, Ordering::Release);
        let budget = operation.budget;
        self.reader = Some(std::thread::spawn(move || {
            let mut reader = BufReader::new(input);
            let mut total = 0_usize;
            loop {
                let response = match worker::frame_with_limit(
                    &mut reader,
                    budget.line_bytes.min(MAX_FRAME_BYTES),
                ) {
                    Ok(Some(bytes)) => {
                        total = total.saturating_add(bytes.len());
                        if total > budget.output_bytes.min(MAX_FRAME_BYTES) {
                            Err(io::Error::other(messages::text(
                                "cleanup_access.response_budget",
                            )))
                        } else {
                            serde_json::from_slice::<Response>(&bytes).map_err(io::Error::other)
                        }
                    }
                    Ok(None) => break,
                    Err(error) => Err(error),
                };
                let failed = response.is_err();
                if tx.send(response).is_err() || failed {
                    break;
                }
            }
            alive.store(false, Ordering::Release);
        }));
        self.output = Some(rx);
        Ok(())
    }

    fn exit_failure(&mut self) -> StartupFailure {
        let outcome = match self.process.take().and_then(|process| process.join().ok()) {
            Some(Err(ElevationError::UserDenied)) => PermissionOutcome::Denied,
            Some(Err(ElevationError::Cancelled)) => PermissionOutcome::Cancelled,
            Some(Err(ElevationError::TimedOut)) => PermissionOutcome::TimedOut,
            _ => PermissionOutcome::AuthorizationUnavailable,
        };
        outcome.into()
    }

    pub(super) fn send(
        &mut self,
        mut bytes: &[u8],
        operation: &OperationContext,
    ) -> io::Result<()> {
        while !bytes.is_empty() {
            operation.check()?;
            match self.prepared.pipe.try_write(bytes) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => bytes = &bytes[count..],
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub(super) fn receive(
        &self,
        timeout: Duration,
    ) -> Result<io::Result<Response>, mpsc::RecvTimeoutError> {
        self.output
            .as_ref()
            .expect("active cleanup reader")
            .recv_timeout(timeout)
    }

    pub(super) fn close(&mut self) -> Result<(), PermissionOutcome> {
        if let Some(result) = self.closed {
            return result;
        }
        self.prepared.close();
        self.reader_context.cancel();
        self.cancelled.store(true, Ordering::Release);
        self.output.take();
        let mut result = self.process.take().map_or(Ok(()), |process| {
            process
                .join()
                .map_err(|_| PermissionOutcome::AuthorizationUnavailable)
                .and_then(|result| match result {
                    Ok(_)
                    | Err(
                        ElevationError::Cancelled
                        | ElevationError::UserDenied
                        | ElevationError::TimedOut,
                    ) => Ok(()),
                    Err(_) => Err(PermissionOutcome::AuthorizationUnavailable),
                })
        });
        if let Some(reader) = self.reader.take() {
            result = result.and(
                reader
                    .join()
                    .map_err(|_| PermissionOutcome::AuthorizationUnavailable),
            );
        }
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
