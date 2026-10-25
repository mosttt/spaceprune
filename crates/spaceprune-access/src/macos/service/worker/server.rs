//! Stateful worker protocol; a verified operation can be dispatched exactly once.

use super::*;

pub(super) trait AuthorityPolicy {
    type Authority;
    fn initialize(&self) -> Result<(), PermissionOutcome>;
    fn adopt(
        &self,
        credential: &ExternalAuthorization,
    ) -> Result<Self::Authority, PermissionOutcome>;
    fn verify(&self, authority: &Self::Authority) -> Result<(), PermissionOutcome>;
}

pub(super) fn serve(
    mut channel: Channel,
    kind: ServiceKind,
    verify_parent: impl FnOnce(&Channel) -> Result<(), PermissionOutcome>,
    policy: impl AuthorityPolicy,
    execute: impl Fn(
        &AuthorizedRequest,
        &OperationContext,
        &dyn EventSink,
    ) -> Result<serde_json::Value, PermissionOutcome>,
    stop_process: fn() -> !,
) -> Result<(), PermissionOutcome> {
    let startup = idle_context().child(Duration::from_secs(10));
    let bootstrap: Bootstrap = channel
        .read(&startup)
        .map_err(|error| io_outcome(error, &startup))?;
    verify_parent(&channel)?;
    if bootstrap.version != WORKER_VERSION
        || bootstrap.kind != kind
        || uuid::Uuid::parse_str(&bootstrap.nonce).is_err()
    {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    channel.allow_service_payloads();
    let context = transferred(&bootstrap.operation)?.child(Duration::from_secs(120));
    respond(
        &mut channel,
        &bootstrap.nonce,
        0,
        &context,
        Ok(serde_json::Value::Null),
    )?;
    let mut initialized = false;
    let mut authority = None;
    let mut verified: Option<OperationContext> = None;
    let mut sequence = 0_u64;
    loop {
        channel.reset_budget();
        let idle = idle_context();
        let request: Request = channel
            .read(&idle)
            .map_err(|error| io_outcome(error, &idle))?;
        sequence = sequence
            .checked_add(1)
            .ok_or(PermissionOutcome::PolicyBlocked)?;
        if request.version != WORKER_VERSION
            || request.nonce != bootstrap.nonce
            || request.sequence != sequence
        {
            return Err(PermissionOutcome::HelperUntrusted);
        }
        let mut context = transferred(&request.operation)?;
        let result = match request.action {
            Action::Initialize if !initialized && sequence == 1 => {
                if context.identity() != bootstrap.operation.identity {
                    return Err(PermissionOutcome::HelperUntrusted);
                }
                let original = transferred(&bootstrap.operation)?;
                context.deadline = context.deadline.min(original.deadline);
                context.budget = context.budget.restricted(original.budget);
                guarded(&channel, &context, stop_process, || policy.initialize()).map(|()| {
                    initialized = true;
                    serde_json::Value::Null
                })
            }
            Action::Adopt { credential } if initialized && authority.is_none() && sequence == 2 => {
                if context.identity() != bootstrap.operation.identity {
                    return Err(PermissionOutcome::HelperUntrusted);
                }
                context.deadline = context
                    .deadline
                    .min(transferred(&bootstrap.operation)?.deadline);
                let value = guarded(&channel, &context, stop_process, || {
                    policy.adopt(&credential)
                });
                value.map(|value| {
                    authority = Some(value);
                    serde_json::Value::Null
                })
            }
            Action::Verify => {
                verified = None;
                let value = authority.as_ref().ok_or(PermissionOutcome::PolicyBlocked)?;
                guarded(&channel, &context, stop_process, || policy.verify(value)).map(|()| {
                    verified = Some(context.clone());
                    serde_json::Value::Null
                })
            }
            Action::Execute { body } => {
                let allowed = verified.take().ok_or(PermissionOutcome::PolicyBlocked)?;
                if allowed.identity() != context.identity() {
                    return Err(PermissionOutcome::HelperUntrusted);
                }
                context.deadline = context.deadline.min(allowed.deadline);
                context.budget = context.budget.restricted(allowed.budget);
                let request = AuthorizedRequest::from_worker(bootstrap.caller_uid, body);
                connection::check_connection(&channel, &context)?;
                let watchdog = agent::watch_host(
                    channel
                        .monitor()
                        .map_err(|error| io_outcome(error, &context))?,
                    context.clone(),
                    stop_process,
                )?;
                let writer = EventWriter::new(
                    &mut channel,
                    &context,
                    |event_sequence, progress, result| {
                        serde_json::to_value(Response {
                            version: WORKER_VERSION,
                            nonce: bootstrap.nonce.clone(),
                            sequence,
                            operation: context.identity(),
                            event_sequence,
                            progress,
                            result,
                        })
                        .expect("native response")
                    },
                );
                let result = execute(&request, &context, &writer);
                let failed = result.is_err();
                drop(watchdog);
                writer.finish(result)?;
                if failed {
                    return Err(PermissionOutcome::AuthorizationUnavailable);
                }
                continue;
            }
            _ => return Err(PermissionOutcome::PolicyBlocked),
        };
        let failed = result.is_err();
        respond(&mut channel, &bootstrap.nonce, sequence, &context, result)?;
        if failed {
            return Err(PermissionOutcome::AuthorizationUnavailable);
        }
    }
}

fn guarded<T>(
    channel: &Channel,
    context: &OperationContext,
    stop_process: fn() -> !,
    run: impl FnOnce() -> Result<T, PermissionOutcome>,
) -> Result<T, PermissionOutcome> {
    connection::check_connection(channel, context)?;
    let _watchdog = agent::watch_host(
        channel
            .monitor()
            .map_err(|error| io_outcome(error, context))?,
        context.clone(),
        stop_process,
    )?;
    let value = run()?;
    connection::check_connection(channel, context)?;
    Ok(value)
}

fn respond(
    channel: &mut Channel,
    nonce: &str,
    sequence: u64,
    operation: &OperationContext,
    result: Result<serde_json::Value, PermissionOutcome>,
) -> Result<(), PermissionOutcome> {
    channel
        .write(
            &Response {
                progress: false,
                event_sequence: 0,
                version: WORKER_VERSION,
                nonce: nonce.into(),
                sequence,
                operation: operation.identity(),
                result,
            },
            operation,
        )
        .map_err(|error| io_outcome(error, operation))
}
