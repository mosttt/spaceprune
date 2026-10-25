//! Adapt the shared batch/receipt protocol to one-item native requests without re-counting sizes.

use super::*;
use std::sync::mpsc;

pub(super) fn run(
    backend: &mut impl Backend,
    grant: &Grant,
    input: &mpsc::Receiver<Vec<u8>>,
    output: &mpsc::SyncSender<io::Result<Response>>,
    lifetime: &OperationContext,
) -> PluginResult<()> {
    loop {
        let bytes = receive(input, lifetime, IDLE_TIMEOUT)?;
        let request: ExecuteRequest =
            serde_json::from_slice(&bytes).map_err(|_| error("Invalid cleanup request"))?;
        let mut context = OperationContext::from_transfer(
            &request.operation,
            lifetime.deadline.saturating_duration_since(Instant::now()),
            lifetime.budget,
        )
        .map_err(|cause| error(cause.to_string()))?;
        if context.identity() != lifetime.identity() {
            return Err(error("Cleanup operation identity changed"));
        }
        context.deadline = context.deadline.min(lifetime.deadline);
        context.cancellation = lifetime.cancellation.child();
        let results = batch(backend, grant, request, input, output, &context)?;
        send(
            output,
            Ok(Response {
                operation: context.identity(),
                reply: Reply::Results { results },
            }),
            &context,
        )?;
    }
}

fn batch(
    backend: &mut impl Backend,
    grant: &Grant,
    request: ExecuteRequest,
    input: &mpsc::Receiver<Vec<u8>>,
    output: &mpsc::SyncSender<io::Result<Response>>,
    operation: &OperationContext,
) -> PluginResult<Vec<PluginResult<HandlerCleanResult>>> {
    let mut results = Vec::new();
    for selection in request.selection {
        let group = grant
            .groups
            .get(selection.group)
            .ok_or_else(|| error("Unknown confirmed cleanup group"))?;
        let mut total = HandlerCleanResult {
            unknown_reclaimed_items: Some(0),
            ..Default::default()
        };
        for &index in &selection.items {
            operation
                .check()
                .map_err(|cause| error(cause.to_string()))?;
            let item = group
                .items
                .get(index)
                .ok_or_else(|| error("Unknown confirmed cleanup item"))?;
            let key = ItemKey {
                group: selection.group,
                item: index,
            };
            let mut accepted = None;
            let reply = backend.request(
                Request::Execute {
                    request: ExecuteRequest {
                        operation: operation
                            .transfer()
                            .map_err(|cause| error(cause.to_string()))?,
                        selection: vec![Selection {
                            group: key.group,
                            items: vec![key.item],
                        }],
                    },
                },
                operation,
                &mut |reply| {
                    let reply = match reply {
                        Reply::ItemFinished { task, outcome } => {
                            if accepted.is_some()
                                || task.module_id != group.request.module_id
                                || task.task_id != group.request.task_id
                                || task.handler_id != group.request.handler_id
                                || outcome.item_id != item.id
                            {
                                return Err(error(
                                    "Cleanup outcome does not match the dispatched item",
                                ));
                            }
                            send(
                                output,
                                Ok(Response {
                                    operation: operation.identity(),
                                    reply: Reply::ItemFinished {
                                        task,
                                        outcome: outcome.clone(),
                                    },
                                }),
                                operation,
                            )?;
                            let receipt: OutcomeReceipt = serde_json::from_slice(&receive(
                                input,
                                operation,
                                Duration::from_secs(30),
                            )?)
                            .map_err(|_| error("Invalid cleanup outcome receipt"))?;
                            accepted = Some(receipt.accepted);
                            total.record_outcome(outcome);
                            return Ok(());
                        }
                        Reply::Count { task, .. } => Reply::Count {
                            task,
                            succeeded: total.succeeded,
                            bytes: total.reclaimed_bytes,
                            unknown: total.unknown_reclaimed_items(),
                        },
                        Reply::Progress { mut event } => {
                            if event.current.is_some() {
                                event.current = Some(total.attempted);
                            }
                            if event.total.is_some() {
                                event.total = Some(selection.items.len());
                            }
                            Reply::Progress { event }
                        }
                        _ => return Err(error("Unexpected cleanup progress frame")),
                    };
                    send(
                        output,
                        Ok(Response {
                            operation: operation.identity(),
                            reply,
                        }),
                        operation,
                    )
                },
            )?;
            let NativeReply::Item { result } = reply else {
                return Err(error("Missing cleanup item result"));
            };
            result
                .validate_selected(std::slice::from_ref(item), operation.is_stopped())
                .map_err(error)?;
            let Some(accepted) = accepted else {
                return Err(error("Cleanup item has no durable receipt"));
            };
            if result.outcomes.len() != 1 || total.outcomes.last() != result.outcomes.first() {
                return Err(error(
                    "Cleanup final result does not match its persisted outcome",
                ));
            }
            let acknowledgement = backend.request(
                Request::Acknowledge {
                    item: key,
                    accepted,
                },
                operation,
                &mut |_| Err(error("Unexpected cleanup receipt event")),
            )?;
            if !accepted || !matches!(acknowledgement, NativeReply::Acknowledged) {
                return Err(error("Cleanup receipt was not accepted"));
            }
        }
        results.push(Ok(total));
    }
    Ok(results)
}

fn receive(
    input: &mpsc::Receiver<Vec<u8>>,
    operation: &OperationContext,
    timeout: Duration,
) -> PluginResult<Vec<u8>> {
    let deadline =
        Instant::now() + timeout.min(operation.deadline.saturating_duration_since(Instant::now()));
    loop {
        operation
            .check()
            .map_err(|cause| error(cause.to_string()))?;
        if Instant::now() >= deadline {
            return Err(error("Cleanup input timed out"));
        }
        match input.recv_timeout(Duration::from_millis(10)) {
            Ok(bytes) => return Ok(bytes),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(error("Cleanup host disconnected"));
            }
        }
    }
}

pub(super) fn send<T>(
    output: &mpsc::SyncSender<T>,
    mut value: T,
    operation: &OperationContext,
) -> PluginResult<()> {
    loop {
        operation
            .check()
            .map_err(|cause| error(cause.to_string()))?;
        match output.try_send(value) {
            Ok(()) => return Ok(()),
            Err(mpsc::TrySendError::Full(pending)) => value = pending,
            Err(mpsc::TrySendError::Disconnected(_)) => {
                return Err(error("Cleanup host disconnected"));
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
