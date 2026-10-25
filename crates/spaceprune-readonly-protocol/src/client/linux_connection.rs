use crate::{LINUX_SESSION_ARGUMENT, LINUX_SESSION_READY, PreparedRequest};
use spaceprune_access::{Interaction, OperationContext, PermissionOutcome, process::ProcessTree};
use std::{
    io::{BufRead, BufReader, Read},
    process::{ChildStdin, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

pub(super) struct Connection {
    pub(super) supports_developer_handlers: bool,
    tree: ProcessTree,
    input: Option<ChildStdin>,
    output: Option<mpsc::Receiver<String>>,
    reader: Option<std::thread::JoinHandle<()>>,
    interaction: Interaction,
    closed: Option<Result<(), PermissionOutcome>>,
}

impl Connection {
    pub(super) fn start(
        operation: &OperationContext,
        notify: &dyn Fn(bool),
        requires_developer_handlers: bool,
    ) -> Result<Self, PermissionOutcome> {
        super::super::check(operation)?;
        match operation.interaction {
            Interaction::Foreground => {
                spaceprune_access::helper::start_metadata_session_with_handler_requirement(
                    operation,
                    notify,
                    requires_developer_handlers,
                    |command, supports| {
                        let mut connection = Self::launch(command, operation)?;
                        connection.supports_developer_handlers = supports;
                        Ok(connection)
                    },
                )
            }
            Interaction::Terminal | Interaction::NonInteractive => {
                let helper = spaceprune_access::helper::ensure_helper_with_handler_requirement(
                    spaceprune_access::helper::HelperKind::Metadata,
                    operation,
                    notify,
                    requires_developer_handlers,
                )?;
                let supports_developer_handlers = requires_developer_handlers
                    || spaceprune_access::helper::probe_protocol(
                        &helper,
                        spaceprune_access::helper::DEVELOPER_HANDLERS_SUPPORT,
                        operation,
                    )
                    .is_ok();
                let sudo = super::super::trust::system_program("sudo")
                    .ok_or(PermissionOutcome::AuthorizationUnavailable)?;
                let mut cached = Command::new(&sudo);
                cached
                    .args(["-n", "--"])
                    .arg(&helper)
                    .arg(LINUX_SESSION_ARGUMENT)
                    .stderr(Stdio::null());
                let mut connection =
                    spaceprune_access::permission::with_cached_terminal_authorization(
                        operation,
                        || Self::launch(&mut cached, operation),
                        || {
                            let _input =
                                spaceprune_access::permission::NativePromptGuard::new(notify);
                            super::super::trust::authorize_terminal(&sudo, operation)
                        },
                    )?;
                connection.supports_developer_handlers = supports_developer_handlers;
                Ok(connection)
            }
        }
    }

    pub(super) fn launch(
        command: &mut Command,
        operation: &OperationContext,
    ) -> Result<Self, PermissionOutcome> {
        super::super::check(operation)?;
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
        let mut tree =
            ProcessTree::spawn(command).map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
        let input = tree.child_mut().stdin.take();
        let output = tree
            .child_mut()
            .stdout
            .take()
            .ok_or(PermissionOutcome::AuthorizationUnavailable)?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let reader = std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let mut line = String::new();
                if (&mut reader).take(128).read_line(&mut line).is_err()
                    || !line.ends_with('\n')
                    || sender.send(line).is_err()
                {
                    break;
                }
            }
        });
        let mut connection = Self {
            supports_developer_handlers: false,
            tree,
            input,
            output: Some(receiver),
            reader: Some(reader),
            interaction: operation.interaction,
            closed: None,
        };
        if connection.wait(operation)?.trim_end() != LINUX_SESSION_READY {
            return Err(PermissionOutcome::HelperOutdated);
        }
        Ok(connection)
    }

    pub(super) fn is_alive(&mut self) -> bool {
        matches!(self.tree.has_exited(), Ok(false))
    }

    pub(super) fn send(
        &mut self,
        request: &PreparedRequest,
        operation: &OperationContext,
    ) -> Result<(), PermissionOutcome> {
        super::super::check(operation)?;
        let frame = request
            .session_request_line()
            .map_err(|_| PermissionOutcome::PolicyBlocked)?;
        let input = self
            .input
            .as_mut()
            .ok_or(PermissionOutcome::AuthorizationUnavailable)?;
        spaceprune_access::process::write_pipe(input, &frame, operation)
            .map_err(|cause| super::super::operation_error(&cause))
    }

    pub(super) fn wait(
        &mut self,
        operation: &OperationContext,
    ) -> Result<String, PermissionOutcome> {
        let response_deadline = std::time::Instant::now() + operation.budget.first_output_timeout;
        loop {
            super::super::check(operation)?;
            if std::time::Instant::now() >= response_deadline {
                return Err(PermissionOutcome::TimedOut);
            }
            match self
                .output
                .as_ref()
                .expect("active metadata response channel")
                .recv_timeout(Duration::from_millis(20))
            {
                Ok(line) => return Ok(line),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if self
                        .tree
                        .has_exited()
                        .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?
                    {
                        let status = self
                            .tree
                            .terminate_and_wait()
                            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
                        return Err(match (self.interaction, status.code()) {
                            (Interaction::Terminal | Interaction::NonInteractive, Some(1)) => {
                                PermissionOutcome::NeedsAuthorization
                            }
                            (_, Some(126)) => PermissionOutcome::Denied,
                            (_, Some(124)) => PermissionOutcome::TimedOut,
                            _ => PermissionOutcome::AuthorizationUnavailable,
                        });
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
    pub(super) fn close(&mut self) -> Result<(), PermissionOutcome> {
        if let Some(result) = self.closed {
            return result;
        }
        // EOF releases an elevated helper even when the caller cannot signal its process group.
        self.input.take();
        self.output.take();
        let mut result = self
            .tree
            .terminate_and_wait()
            .map(|_| ())
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable);
        if let Some(reader) = self.reader.take() {
            result = result.and(
                reader
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
