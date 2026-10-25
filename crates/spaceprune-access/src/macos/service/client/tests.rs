use super::*;
use std::{os::unix::net::UnixStream, thread, time::Instant};

fn pair() -> (ServiceClient, Channel, OperationContext) {
    let (host, service) = UnixStream::pair().unwrap();
    let operation = OperationContext::default();
    let client = ServiceClient::new(
        Channel::new(host).unwrap(),
        "fixture".into(),
        operation.identity(),
    );
    (client, Channel::new(service).unwrap(), operation)
}

fn stopped(operation: &OperationContext, sequence: u64) -> Reply {
    Reply {
        event_sequence: 0,
        phase: Phase::Stopped,
        version: VERSION,
        nonce: "fixture".into(),
        sequence,
        operation: operation.identity(),
        result: Err(PermissionOutcome::Cancelled),
    }
}

#[test]
fn closed_connections_retain_confirmation_failures_and_cannot_dispatch_again() {
    for mode in [
        "valid",
        "disconnect",
        "nonce",
        "sequence",
        "identity",
        "phase",
        "ordinary",
    ] {
        let (mut client, mut service, operation) = pair();
        let original = operation.clone();
        let server = thread::spawn(move || {
            let command: Command = service.read(&original).unwrap();
            assert!(matches!(command, Command::Cancel { .. }));
            if mode == "disconnect" {
                return;
            }
            let mut reply = stopped(&original, 0);
            match mode {
                "nonce" => reply.nonce = "foreign".into(),
                "sequence" => reply.sequence = 1,
                "identity" => reply.operation = OperationContext::default().identity(),
                "phase" => reply.phase = Phase::Completed,
                _ => {}
            }
            if mode == "ordinary" {
                service.write(&reply, &original).unwrap();
            } else {
                service.write_terminal(&reply, &original).unwrap();
            }
        });
        let expected = match mode {
            "valid" => Ok(()),
            "disconnect" | "ordinary" => Err(PermissionOutcome::AuthorizationUnavailable),
            _ => Err(PermissionOutcome::HelperUntrusted),
        };
        assert_eq!(client.close(), expected, "{mode}");
        assert_eq!(
            client.close(),
            expected,
            "{mode}: repeat must retain the receipt"
        );
        assert!(client.channel.is_none());
        assert_eq!(
            client.send(&serde_json::Value::Null, &operation),
            Err(expected
                .err()
                .unwrap_or(PermissionOutcome::AuthorizationUnavailable)),
            "{mode}: a withdrawn connection cannot dispatch"
        );
        server.join().unwrap();
    }
}

#[test]
fn a_live_service_without_a_stop_receipt_times_out_without_becoming_confirmed() {
    let (mut client, mut service, operation) = pair();
    let (release, wait) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let _: Command = service.read(&operation).unwrap();
        wait.recv().unwrap();
    });
    let started = Instant::now();
    assert_eq!(client.close(), Err(PermissionOutcome::TimedOut));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(client.close(), Err(PermissionOutcome::TimedOut));
    release.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn cancelling_an_inflight_request_propagates_missing_stop_confirmation() {
    for confirmed in [false, true] {
        let (mut client, mut service, operation) = pair();
        client.send(&serde_json::Value::Null, &operation).unwrap();
        let original = OperationContext {
            cancellation: Default::default(),
            ..operation.clone()
        };
        let server = thread::spawn(move || {
            assert!(matches!(
                service.read::<Command>(&original).unwrap(),
                Command::Execute { .. }
            ));
            assert!(matches!(
                service.read::<Command>(&original).unwrap(),
                Command::Cancel { .. }
            ));
            if confirmed {
                service
                    .write_terminal(&stopped(&original, 1), &original)
                    .unwrap();
            }
        });
        operation.cancel();
        let expected = if confirmed {
            PermissionOutcome::Cancelled
        } else {
            PermissionOutcome::AuthorizationUnavailable
        };
        assert_eq!(client.wait::<()>(&operation), Err(expected));
        assert_eq!(
            client.close(),
            if confirmed { Ok(()) } else { Err(expected) }
        );
        server.join().unwrap();
    }
}

#[test]
fn an_ordinary_stopped_message_is_not_a_worker_exit_receipt() {
    let (mut client, mut service, operation) = pair();
    client.send(&serde_json::Value::Null, &operation).unwrap();
    let original = operation.clone();
    let server = thread::spawn(move || {
        let _: Command = service.read(&original).unwrap();
        service.write(&stopped(&original, 1), &original).unwrap();
        assert!(matches!(
            service.read::<Command>(&original).unwrap(),
            Command::Cancel { .. }
        ));
    });
    assert_eq!(
        client.wait::<()>(&operation),
        Err(PermissionOutcome::AuthorizationUnavailable)
    );
    assert_eq!(
        client.close(),
        Err(PermissionOutcome::AuthorizationUnavailable)
    );
    server.join().unwrap();
}

#[test]
fn a_matching_unsolicited_terminal_receipt_is_preserved() {
    let (mut client, mut service, operation) = pair();
    client.send(&serde_json::Value::Null, &operation).unwrap();
    let original = operation.clone();
    let server = thread::spawn(move || {
        let _: Command = service.read(&original).unwrap();
        service
            .write_terminal(&stopped(&original, 1), &original)
            .unwrap();
    });
    assert_eq!(
        client.wait::<()>(&operation),
        Err(PermissionOutcome::Cancelled)
    );
    assert_eq!(client.close(), Ok(()));
    server.join().unwrap();
}

#[test]
fn a_handshake_failure_marks_only_unconfirmed_shutdown_as_pending() {
    for outcome in [
        PermissionOutcome::Denied,
        PermissionOutcome::Cancelled,
        PermissionOutcome::TimedOut,
    ] {
        for confirm in [true, false] {
            let (client, mut service, operation) = pair();
            let server = thread::spawn(move || {
                let _: Command = service.read(&operation).unwrap();
                if confirm {
                    service
                        .write_terminal(&stopped(&operation, 0), &operation)
                        .unwrap();
                }
            });
            let failure = client.finish_connect(Err(outcome)).err().unwrap();
            assert_eq!(failure.shutdown_unconfirmed, !confirm);
            assert_eq!(
                failure.outcome,
                if confirm {
                    outcome
                } else {
                    PermissionOutcome::AuthorizationUnavailable
                }
            );
            server.join().unwrap();
        }
    }
    assert_eq!(
        ConnectFailure::from(PermissionOutcome::NeedsAuthorization),
        ConnectFailure {
            outcome: PermissionOutcome::NeedsAuthorization,
            shutdown_unconfirmed: false,
        }
    );
}

#[test]
fn ordinary_progress_and_results_keep_the_authorized_connection_reusable() {
    let (mut client, mut service, operation) = pair();
    let original = operation.clone();
    let server = thread::spawn(move || {
        for sequence in 1..=2 {
            let Command::Execute {
                sequence: received, ..
            } = service.read(&original).unwrap()
            else {
                panic!("Expected an ordinary request");
            };
            assert_eq!(received, sequence);
            let mut reply = stopped(&original, sequence);
            reply.phase = Phase::Progress;
            reply.result = Ok(serde_json::json!("progress"));
            service.write(&reply, &original).unwrap();
            reply.phase = Phase::Completed;
            reply.event_sequence = 1;
            reply.result = Ok(serde_json::json!(sequence));
            service.write(&reply, &original).unwrap();
        }
        let _: Command = service.read(&original).unwrap();
        service
            .write_terminal(&stopped(&original, 2), &original)
            .unwrap();
    });
    for sequence in 1..=2 {
        client.send(&serde_json::Value::Null, &operation).unwrap();
        let mut progress = Vec::new();
        let result: u64 = client
            .wait_with_events(&operation, |event| {
                progress.push(event);
                Ok(())
            })
            .unwrap();
        assert_eq!(result, sequence);
        assert_eq!(progress, vec![serde_json::json!("progress")]);
        assert!(client.closed.is_none());
    }
    assert_eq!(client.close(), Ok(()));
    server.join().unwrap();
}
