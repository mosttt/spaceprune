//! Finite control-only shutdown never extends an execution deadline.

use super::*;

pub(super) fn shutdown_context() -> OperationContext {
    OperationContext::workflow(Default::default(), Interaction::NonInteractive)
        .child(Duration::from_secs(3))
}

pub(super) fn stop_and_confirm(
    channel: &mut Channel,
    nonce: &str,
    sequence: u64,
    operation: &OperationIdentity,
) -> Result<(), PermissionOutcome> {
    let shutdown = shutdown_context();
    // If the service already closed its receive right, a terminal reply may still be queued.
    let _ = channel.write(
        &Command::Cancel {
            nonce: nonce.into(),
            sequence,
            operation: operation.clone(),
        },
        &shutdown,
    );
    channel.reset_budget();
    let reply: Reply = channel
        .read_terminal(&shutdown)
        .map_err(|error| io_outcome(error, &shutdown))?;
    if reply.version != VERSION
        || reply.nonce != nonce
        || reply.sequence != sequence
        || reply.operation != *operation
        || reply.phase != Phase::Stopped
    {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn only_a_matching_terminal_frame_confirms_worker_exit() {
        for mode in ["nonce", "sequence", "operation", "ordinary", "valid"] {
            let (left, right) = UnixStream::pair().unwrap();
            let context = OperationContext::default();
            let operation = context.identity();
            let original = operation.clone();
            let server = std::thread::spawn(move || {
                let mut channel = Channel::new(left).unwrap();
                let _: Command = channel.read(&context).unwrap();
                let reply = Reply {
                    event_sequence: 0,
                    version: VERSION,
                    phase: Phase::Stopped,
                    nonce: if mode == "nonce" { "other" } else { "fixture" }.into(),
                    sequence: if mode == "sequence" { 1 } else { 0 },
                    operation: if mode == "operation" {
                        OperationContext::default().identity()
                    } else {
                        original
                    },
                    result: Err(PermissionOutcome::Cancelled),
                };
                if mode == "ordinary" {
                    channel.write(&reply, &context).unwrap();
                } else {
                    channel.write_terminal(&reply, &context).unwrap();
                }
            });
            let mut host = Channel::new(right).unwrap();
            let result = stop_and_confirm(&mut host, "fixture", 0, &operation);
            assert_eq!(result.is_ok(), mode == "valid");
            server.join().unwrap();
        }
    }
}
