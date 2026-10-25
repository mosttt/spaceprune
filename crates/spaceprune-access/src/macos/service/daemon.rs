use super::handoff::{Caller, Registry};
use super::*;
#[cfg(target_os = "macos")]
use crate::macos::channel::ServiceEndpoint;
use std::sync::atomic::Ordering;
#[cfg(target_os = "macos")]
use std::sync::{Arc, atomic::AtomicUsize};

/// A request delivered to the fixed handler only after live OS policy, Mach sender and
/// sequence verification. It is deliberately not deserializable from a host message.
pub struct AuthorizedRequest {
    caller_uid: u32,
    body: serde_json::Value,
}
impl AuthorizedRequest {
    pub(super) fn from_worker(caller_uid: u32, body: serde_json::Value) -> Self {
        Self { caller_uid, body }
    }
    /// Real invoking UID from the kernel audit trailer, never an environment variable.
    pub fn caller_uid(&self) -> u32 {
        self.caller_uid
    }
    /// Host-owned payload received on the authenticated channel. The fixed handler must
    /// still validate its operation, budget, requested roots and confirmed plan boundaries.
    pub fn body(&self) -> &serde_json::Value {
        &self.body
    }
}

/// Run only in the signed, root launchd executable for this fixed capability. The supplied
/// handler must validate its payload, original context and scope before doing any work.
#[cfg(target_os = "macos")]
pub(super) fn run_service(kind: ServiceKind) -> Result<(), PermissionOutcome> {
    if !rustix::process::geteuid().is_root() {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    let team = signing::service_team(kind)?;
    let endpoint =
        ServiceEndpoint::check_in(kind).map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
    let active: [Arc<AtomicUsize>; 2] = std::array::from_fn(|_| Arc::new(AtomicUsize::new(0)));
    let registry = Registry::default();
    let mut last_activity = std::time::Instant::now();
    loop {
        let startup = idle_context().child(Duration::from_secs(5));
        if let Ok(Some(channel)) = endpoint.accept(&startup) {
            last_activity = std::time::Instant::now();
            let peer = *channel.peer().ok_or(PermissionOutcome::HelperUntrusted)?;
            let role = if peer
                .verify_code(&team, &[CLI_IDENTIFIER, DESKTOP_IDENTIFIER], None)
                .is_ok()
            {
                0
            } else if peer
                .verify_code(&team, &[INSTALLER_IDENTIFIER], None)
                .is_ok()
            {
                1
            } else {
                continue;
            };
            // Reserve independent slots so waiting hosts cannot starve their authorizers.
            let count = &active[role];
            if count.load(Ordering::Acquire) >= 16 {
                continue;
            }
            count.fetch_add(1, Ordering::AcqRel);
            let slot = ActiveSlot(Arc::clone(count));
            let registry = registry.clone();
            let team = team.clone();
            std::thread::Builder::new()
                .name("spaceprune-native-session".into())
                .spawn(move || {
                    let _slot = slot;
                    if role == 0 {
                        let session = HostSession {
                            channel,
                            kind,
                            caller: Caller {
                                identity: peer.identity,
                                peer,
                            },
                            registry,
                        };
                        let _ = serve_host(
                            session,
                            |caller, context| {
                                worker::Worker::start(kind, caller.identity.effective_uid, context)
                            },
                            worker::Worker::initialize,
                            worker::Worker::adopt,
                            worker::Worker::verify,
                            worker::Worker::execute,
                            worker::Worker::close,
                        );
                    } else {
                        let _ = serve_authorizer(channel, kind, registry, |caller| {
                            peer.check_parent(caller.identity)?;
                            peer.verify_code(&team, &[INSTALLER_IDENTIFIER], None)?;
                            caller.peer.verify_code(
                                &team,
                                &[CLI_IDENTIFIER, DESKTOP_IDENTIFIER],
                                None,
                            )
                        });
                    }
                })
                .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
        }
        if active
            .iter()
            .all(|count| count.load(Ordering::Acquire) == 0)
            && last_activity.elapsed() >= IDLE_TIMEOUT
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(target_os = "macos")]
struct ActiveSlot(Arc<AtomicUsize>);
#[cfg(target_os = "macos")]
impl Drop for ActiveSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct HostSession {
    channel: Channel,
    kind: ServiceKind,
    caller: Caller,
    registry: Registry,
}

fn serve_host<Authority>(
    session: HostSession,
    prepare: impl FnOnce(Caller, &OperationContext) -> Result<Authority, PermissionOutcome>,
    initialize: impl Fn(&mut Authority, &OperationContext) -> Result<(), PermissionOutcome>,
    adopt: impl Fn(
        &mut Authority,
        &ExternalAuthorization,
        &OperationContext,
    ) -> Result<(), PermissionOutcome>,
    verify: impl Fn(&mut Authority, &OperationContext) -> Result<(), PermissionOutcome>,
    execute: impl Fn(
        &mut Authority,
        &AuthorizedRequest,
        &OperationContext,
        &dyn EventSink,
    ) -> Result<serde_json::Value, PermissionOutcome>,
    finish: impl FnOnce(&mut Authority) -> Result<(), PermissionOutcome>,
) -> Result<(), PermissionOutcome> {
    let HostSession {
        mut channel,
        kind,
        caller,
        registry,
    } = session;
    let startup = idle_context().child(Duration::from_secs(10));
    let nonce = greet(&mut channel, kind, &startup)?;
    let first: Command = channel
        .read(&startup)
        .map_err(|error| io_outcome(error, &startup))?;
    let Command::Prepare {
        nonce: received,
        operation,
    } = first
    else {
        return Err(PermissionOutcome::PolicyBlocked);
    };
    if received != nonce {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    let context = transferred(&operation)?.child(Duration::from_secs(120));
    let mut authority = while_connected(&channel, &context, || prepare(caller, &context))?;
    let mut sequence = 0_u64;
    let mut last_operation = context.identity();
    let result = (|| {
        while_connected(&channel, &context, || initialize(&mut authority, &context))?;
        let pending = registry.prepare(&nonce, caller, &context)?;
        reply(
            &mut channel,
            &nonce,
            Phase::Prepared,
            &context,
            Ok(serde_json::Value::Null),
        )?;
        let authorized = pending.receive(&channel, |credential| {
            while_connected(&channel, &context, || {
                adopt(&mut authority, credential, &context)
            })
        });
        reply(
            &mut channel,
            &nonce,
            Phase::Authorized,
            &context,
            authorized
                .as_ref()
                .map(|_| serde_json::Value::Null)
                .map_err(|outcome| *outcome),
        )?;
        authorized?;
        drop(pending);
        let caller_uid = caller.identity.effective_uid;
        loop {
            channel.reset_budget();
            let idle = idle_context();
            let command: Command = channel
                .read(&idle)
                .map_err(|error| io_outcome(error, &idle))?;
            if let Command::Cancel {
                nonce: received,
                sequence: received_sequence,
                operation,
            } = &command
            {
                if *received != nonce
                    || *received_sequence != sequence
                    || *operation != last_operation
                {
                    return Err(PermissionOutcome::HelperUntrusted);
                }
                return Err(PermissionOutcome::Cancelled);
            }
            let Command::Execute {
                nonce: received,
                sequence: next,
                operation,
                request: body,
            } = command
            else {
                return Err(PermissionOutcome::PolicyBlocked);
            };
            sequence = sequence
                .checked_add(1)
                .ok_or(PermissionOutcome::PolicyBlocked)?;
            if received != nonce || next != sequence {
                return Err(PermissionOutcome::HelperUntrusted);
            }
            last_operation = operation.identity.clone();
            let context = transferred(&operation)?;
            let request = AuthorizedRequest { caller_uid, body };
            while_connected(&channel, &context, || verify(&mut authority, &context))?;
            connection::check_connection(&channel, &context)?;
            let monitor = channel
                .monitor()
                .map_err(|error| io_outcome(error, &context))?;
            let writer = EventWriter::new(
                &mut channel,
                &context,
                |event_sequence, progress, result| {
                    serde_json::to_value(Reply {
                        phase: if progress {
                            Phase::Progress
                        } else {
                            Phase::Completed
                        },
                        version: VERSION,
                        nonce: nonce.clone(),
                        sequence,
                        event_sequence,
                        operation: context.identity(),
                        result,
                    })
                    .expect("native reply")
                },
            );
            let result = connection::supervise(monitor, &context, || {
                execute(&mut authority, &request, &context, &writer)
            });
            writer.finish(result)?;
        }
    })();
    // A terminal receipt is issued only after the owned worker has actually exited and been reaped.
    finish(&mut authority)?;
    drop(authority);
    let shutdown = termination::shutdown_context();
    let _ = channel.write_terminal(
        &Reply {
            phase: Phase::Stopped,
            event_sequence: 0,
            version: VERSION,
            nonce,
            sequence,
            operation: last_operation,
            result: result.map(|()| serde_json::Value::Null),
        },
        &shutdown,
    );
    result
}

fn greet(
    channel: &mut Channel,
    kind: ServiceKind,
    operation: &OperationContext,
) -> Result<String, PermissionOutcome> {
    let nonce = uuid::Uuid::new_v4().to_string();
    channel
        .write(
            &Hello {
                version: VERSION,
                kind,
                nonce: nonce.clone(),
            },
            operation,
        )
        .map_err(|error| io_outcome(error, operation))?;
    Ok(nonce)
}

fn reply(
    channel: &mut Channel,
    nonce: &str,
    phase: Phase,
    operation: &OperationContext,
    result: Result<serde_json::Value, PermissionOutcome>,
) -> Result<(), PermissionOutcome> {
    channel
        .write(
            &Reply {
                version: VERSION,
                nonce: nonce.into(),
                phase,
                event_sequence: 0,
                sequence: 0,
                operation: operation.identity(),
                result,
            },
            operation,
        )
        .map_err(|error| io_outcome(error, operation))
}

// This branch is chosen by the code-signature verifier, never by a deserialized client role.
fn serve_authorizer(
    mut channel: Channel,
    kind: ServiceKind,
    registry: Registry,
    verify_agent: impl Fn(Caller) -> Result<(), PermissionOutcome>,
) -> Result<(), PermissionOutcome> {
    let startup = idle_context().child(Duration::from_secs(10));
    let nonce = greet(&mut channel, kind, &startup)?;
    let first: Command = channel
        .read(&startup)
        .map_err(|error| io_outcome(error, &startup))?;
    let Command::Attach {
        nonce: received,
        target_nonce,
        operation,
    } = first
    else {
        return Err(PermissionOutcome::PolicyBlocked);
    };
    if received != nonce {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    let requested = transferred(&operation)?;
    let claim = registry.claim(&target_nonce, &requested.identity(), &verify_agent);
    reply(
        &mut channel,
        &nonce,
        Phase::Prepared,
        &requested,
        claim
            .as_ref()
            .map(|_| serde_json::Value::Null)
            .map_err(|outcome| *outcome),
    )?;
    let claim = claim?;
    let context = claim.operation().clone();
    let next: Command = channel
        .read(&context)
        .map_err(|error| io_outcome(error, &context))?;
    let Command::Adopt {
        nonce: received,
        credential,
    } = next
    else {
        return Err(PermissionOutcome::PolicyBlocked);
    };
    if received != nonce {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    verify_agent(claim.caller())?;
    let result = claim.deliver(credential).map(|_| serde_json::Value::Null);
    reply(&mut channel, &nonce, Phase::Authorized, &context, result)
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
