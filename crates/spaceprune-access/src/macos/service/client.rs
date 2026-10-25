use super::*;

/// Native connection setup failure, including whether shutdown remains unconfirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectFailure {
    /// The failed native authorization or shutdown outcome.
    pub outcome: PermissionOutcome,
    /// A native connection was withdrawn without a matching worker-exit receipt.
    pub shutdown_unconfirmed: bool,
}

impl From<PermissionOutcome> for ConnectFailure {
    fn from(outcome: PermissionOutcome) -> Self {
        Self {
            outcome,
            shutdown_unconfirmed: false,
        }
    }
}

/// An authenticated connection with one fixed capability. Native credentials never enter the host.
pub struct ServiceClient {
    channel: Option<Channel>,
    nonce: String,
    sequence: u64,
    operation: OperationIdentity,
    pending: Option<(OperationIdentity, Phase)>,
    closed: Option<Result<(), PermissionOutcome>>,
}
impl ServiceClient {
    /// Prepare the signed service and ask its verified authorizer child to deliver the OS
    /// credential directly. Registration and a child success alone are not access grants.
    #[cfg(target_os = "macos")]
    pub fn connect(
        kind: ServiceKind,
        operation: &OperationContext,
    ) -> Result<Self, ConnectFailure> {
        check(operation)?;
        match register_from_host(kind, operation)? {
            RegistrationState::Enabled => {}
            RegistrationState::RequiresApproval => {
                return Err(PermissionOutcome::NeedsSystemSettings.into());
            }
            _ => return Err(PermissionOutcome::AuthorizationUnavailable.into()),
        }
        let team = signing::host_team()?;
        let host = std::env::current_exe().map_err(|error| io_outcome(error, operation))?;
        let agent = agent_session::agent_at(&host).ok_or(PermissionOutcome::HelperMissing)?;
        let (channel, hello) = super::connect_current(
            operation,
            || connect_verified(kind, &team, &agent, operation),
            || agent_session::refresh_from_host(kind, operation),
        )?;
        let bounded = operation.child(Duration::from_secs(120));
        let transfer = bounded
            .transfer()
            .map_err(|error| io_outcome(error, &bounded))?;
        let mut client = Self::new(channel, hello.nonce, operation.identity());
        let result = (|| {
            client.pending = Some((bounded.identity(), Phase::Prepared));
            client.write(
                &Command::Prepare {
                    nonce: client.nonce.clone(),
                    operation: transfer,
                },
                &bounded,
            )?;
            client.wait::<()>(&bounded)?;
            agent_session::authorize_from_host(kind, &client.nonce, &bounded)?;
            client.pending = Some((bounded.identity(), Phase::Authorized));
            client.wait::<()>(&bounded)
        })();
        client.finish_connect(result)
    }

    fn finish_connect(
        mut self,
        result: Result<(), PermissionOutcome>,
    ) -> Result<Self, ConnectFailure> {
        if let Err(outcome) = result {
            self.close().map_err(|outcome| ConnectFailure {
                outcome,
                shutdown_unconfirmed: true,
            })?;
            return Err(outcome.into());
        }
        Ok(self)
    }

    pub(super) fn new(channel: Channel, nonce: String, operation: OperationIdentity) -> Self {
        Self {
            channel: Some(channel),
            nonce,
            sequence: 0,
            operation,
            pending: None,
            closed: None,
        }
    }

    /// Liveness only. The daemon rechecks OS policy for each operation.
    #[cfg(target_os = "macos")]
    pub fn is_alive(&self) -> bool {
        self.channel.as_ref().is_some_and(Channel::is_alive)
    }

    /// Send a host-owned payload over verified Mach fragments. Per-handler scope checks remain required.
    pub fn send(
        &mut self,
        request: &impl Serialize,
        operation: &OperationContext,
    ) -> Result<(), PermissionOutcome> {
        if let Some(closed) = self.closed {
            return Err(closed
                .err()
                .unwrap_or(PermissionOutcome::AuthorizationUnavailable));
        }
        check(operation)?;
        if self.pending.is_some() {
            return Err(PermissionOutcome::PolicyBlocked);
        }
        let transfer = operation
            .transfer()
            .map_err(|error| io_outcome(error, operation))?;
        let request =
            serde_json::to_value(request).map_err(|_| PermissionOutcome::PolicyBlocked)?;
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(PermissionOutcome::PolicyBlocked)?;
        self.operation = operation.identity();
        self.pending = Some((operation.identity(), Phase::Completed));
        let command = Command::Execute {
            nonce: self.nonce.clone(),
            sequence: self.sequence,
            operation: transfer,
            request,
        };
        self.channel
            .as_mut()
            .ok_or(PermissionOutcome::AuthorizationUnavailable)?
            .reset_budget();
        self.write(&command, operation)
    }

    /// Return only the expected phase, operation and sequence; any failure closes the session.
    pub fn wait<T: serde::de::DeserializeOwned>(
        &mut self,
        operation: &OperationContext,
    ) -> Result<T, PermissionOutcome> {
        self.wait_with_events(operation, |_| Err(PermissionOutcome::PolicyBlocked))
    }

    /// Consume ordered progress before the final result. A rejected event stops and reaps work.
    pub fn wait_with_events<T: serde::de::DeserializeOwned>(
        &mut self,
        operation: &OperationContext,
        mut event: impl FnMut(serde_json::Value) -> Result<(), PermissionOutcome>,
    ) -> Result<T, PermissionOutcome> {
        let mut stopped = false;
        let result = (|| {
            let (identity, phase) = self
                .pending
                .as_ref()
                .ok_or(PermissionOutcome::PolicyBlocked)?;
            if *identity != operation.identity() {
                return Err(PermissionOutcome::PolicyBlocked);
            }
            let mut budget = StreamBudget::default();
            loop {
                let (reply, terminal): (Reply, bool) = self
                    .channel
                    .as_mut()
                    .ok_or(PermissionOutcome::AuthorizationUnavailable)?
                    .read_with_terminal(operation)
                    .map_err(|error| io_outcome(error, operation))?;
                if terminal != (reply.phase == Phase::Stopped) {
                    return Err(PermissionOutcome::HelperUntrusted);
                }
                if reply.phase == Phase::Stopped {
                    validate_reply(&reply, &self.nonce, self.sequence, Phase::Stopped, identity)?;
                    stopped = true;
                    return Err(reply.result.err().unwrap_or(PermissionOutcome::Cancelled));
                }
                let progress = reply.phase == Phase::Progress && *phase == Phase::Completed;
                validate_reply(
                    &reply,
                    &self.nonce,
                    self.sequence,
                    if progress { Phase::Progress } else { *phase },
                    identity,
                )?;
                budget.accept(reply.event_sequence, progress, &reply, operation)?;
                check(operation)?;
                if progress {
                    event(reply.result?)?;
                } else {
                    return serde_json::from_value(reply.result?)
                        .map_err(|_| PermissionOutcome::PolicyBlocked);
                }
            }
        })();
        self.pending = None;
        if stopped {
            self.channel.take();
            self.closed = Some(Ok(()));
        } else if result.is_err() {
            self.close()?;
        }
        result
    }

    fn write(
        &mut self,
        command: &Command,
        operation: &OperationContext,
    ) -> Result<(), PermissionOutcome> {
        let result = self
            .channel
            .as_mut()
            .ok_or(PermissionOutcome::AuthorizationUnavailable)?
            .write(command, operation)
            .map_err(|error| io_outcome(error, operation));
        if result.is_err() {
            self.close()?;
        }
        result
    }

    /// Withdraw this connection and confirm that its service-owned worker has been reaped.
    /// A failed confirmation is retained: another close cannot turn it into a successful stop.
    pub fn close(&mut self) -> Result<(), PermissionOutcome> {
        if let Some(result) = self.closed {
            return result;
        }
        self.pending = None;
        let result = if let Some(mut channel) = self.channel.take() {
            termination::stop_and_confirm(&mut channel, &self.nonce, self.sequence, &self.operation)
        } else {
            Err(PermissionOutcome::AuthorizationUnavailable)
        };
        self.closed = Some(result);
        result
    }
}
impl Drop for ServiceClient {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn validate_reply(
    reply: &Reply,
    nonce: &str,
    sequence: u64,
    phase: Phase,
    operation: &OperationIdentity,
) -> Result<(), PermissionOutcome> {
    if reply.version != VERSION
        || reply.nonce != nonce
        || reply.sequence != sequence
        || reply.phase != phase
        || reply.operation != *operation
    {
        Err(PermissionOutcome::HelperUntrusted)
    } else {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn connect_verified(
    kind: ServiceKind,
    team: &str,
    agent: &Path,
    operation: &OperationContext,
) -> Result<(Channel, Hello), PermissionOutcome> {
    let bundle = agent
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or(PermissionOutcome::HelperMissing)?;
    let expected_hash = signing::verify_file(&kind.bundled_executable(bundle), team, kind.label())?;
    let mut channel = Channel::connect_service(kind, &operation.child(Duration::from_secs(10)))
        .map_err(|error| io_outcome(error, operation))?;
    let peer = channel.peer().ok_or(PermissionOutcome::HelperUntrusted)?;
    if peer.identity.effective_uid != 0 || peer.identity.real_uid != 0 {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    let actual_hash = peer.verified_hash(team, &[kind.label()])?;
    if actual_hash != expected_hash {
        return Err(PermissionOutcome::HelperOutdated);
    }
    let hello: Hello = channel
        .read(operation)
        .map_err(|error| io_outcome(error, operation))?;
    if hello.kind != kind || uuid::Uuid::parse_str(&hello.nonce).is_err() {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    if hello.version != VERSION {
        return Err(PermissionOutcome::HelperOutdated);
    }
    Ok((channel, hello))
}

// Only the signed authorization executable uses this role. Native preflight binds the
// actual child to the host and purpose before any authorization dialog is requested.
#[cfg(target_os = "macos")]
pub(in crate::macos) fn authorize_from_agent(
    kind: ServiceKind,
    target_nonce: &str,
    operation: &OperationContext,
) -> Result<(), PermissionOutcome> {
    let team = signing::own_team()?;
    let executable = std::env::current_exe().map_err(|error| io_outcome(error, operation))?;
    let (mut channel, hello) = connect_verified(kind, &team, &executable, operation)?;
    channel
        .write(
            &Command::Attach {
                nonce: hello.nonce.clone(),
                target_nonce: target_nonce.into(),
                operation: operation
                    .transfer()
                    .map_err(|error| io_outcome(error, operation))?,
            },
            operation,
        )
        .map_err(|error| io_outcome(error, operation))?;
    let prepared: Reply = channel
        .read(operation)
        .map_err(|error| io_outcome(error, operation))?;
    validate_reply(
        &prepared,
        &hello.nonce,
        0,
        Phase::Prepared,
        &operation.identity(),
    )?;
    prepared.result?;
    let watchdog = agent::watch_host(
        channel
            .monitor()
            .map_err(|error| io_outcome(error, operation))?,
        operation.clone(),
        agent::stop_agent,
    )?;
    let authorization = authorization::Authorization::request_access(kind, operation)?;
    drop(watchdog);
    channel
        .write(
            &Command::Adopt {
                nonce: hello.nonce.clone(),
                credential: authorization.externalize()?,
            },
            operation,
        )
        .map_err(|error| io_outcome(error, operation))?;
    let reply: Reply = channel
        .read(operation)
        .map_err(|error| io_outcome(error, operation))?;
    validate_reply(
        &reply,
        &hello.nonce,
        0,
        Phase::Authorized,
        &operation.identity(),
    )?;
    check(operation)?;
    reply.result?;
    // A daemon-owned reference is established before releasing the agent's local one.
    drop(authorization);
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
