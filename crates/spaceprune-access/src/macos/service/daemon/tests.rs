use super::*;
use crate::macos::principal::ProcessIdentity;
use std::{
    os::unix::net::UnixStream,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize},
    },
};

fn caller() -> Caller {
    Caller {
        identity: ProcessIdentity {
            pid: 501,
            effective_uid: 1001,
            real_uid: 1001,
            audit_session: 9,
        },
    }
}
fn start_host(
    registry: Registry,
    execute: impl Fn(
        &AuthorizedRequest,
        &OperationContext,
    ) -> Result<serde_json::Value, PermissionOutcome>
    + Send
    + 'static,
    adopt: impl Fn(&ExternalAuthorization) -> Result<(), PermissionOutcome> + Send + 'static,
    verify: impl Fn(&()) -> Result<(), PermissionOutcome> + Send + 'static,
) -> (
    Channel,
    std::thread::JoinHandle<Result<(), PermissionOutcome>>,
) {
    let (left, right) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        serve_host(
            HostSession {
                channel: Channel::new(left).unwrap(),
                kind: ServiceKind::Metadata,
                caller: caller(),
                registry,
            },
            |_, _| Ok(()),
            |_, _| Ok(()),
            move |_, credential, _| adopt(credential),
            move |authority, _| verify(authority),
            move |_, request, context, _| execute(request, context),
            |_| Ok(()),
        )
    });
    (Channel::new(right).unwrap(), server)
}

fn prepare(channel: &mut Channel, context: &OperationContext) -> String {
    let hello: Hello = channel.read(context).unwrap();
    assert_eq!(hello.kind, ServiceKind::Metadata);
    channel
        .write(
            &Command::Prepare {
                nonce: hello.nonce.clone(),
                operation: context.transfer().unwrap(),
            },
            context,
        )
        .unwrap();
    let reply: Reply = channel.read(context).unwrap();
    assert_eq!(reply.phase, Phase::Prepared);
    assert!(reply.result.is_ok());
    hello.nonce
}

fn authorize(channel: &mut Channel, context: &OperationContext, registry: Registry) -> String {
    let target_nonce = prepare(channel, context);
    let (left, right) = UnixStream::pair().unwrap();
    // This substitutes the native code/parent verifier. Production chooses this branch only
    // after checking the authorizer's actual kernel peer and both live code identities.
    let server = std::thread::spawn(move || {
        serve_authorizer(
            Channel::new(left).unwrap(),
            ServiceKind::Metadata,
            registry,
            |owner| {
                assert_eq!(owner.identity, caller().identity);
                Ok(())
            },
        )
    });
    let mut agent = Channel::new(right).unwrap();
    let hello: Hello = agent.read(context).unwrap();
    agent
        .write(
            &Command::Attach {
                nonce: hello.nonce.clone(),
                target_nonce: target_nonce.clone(),
                operation: context.transfer().unwrap(),
            },
            context,
        )
        .unwrap();
    let ready: Reply = agent.read(context).unwrap();
    assert_eq!(ready.phase, Phase::Prepared);
    assert!(ready.result.is_ok());
    agent
        .write(
            &Command::Adopt {
                nonce: hello.nonce,
                credential: ExternalAuthorization([7; 32]),
            },
            context,
        )
        .unwrap();
    let adopted: Reply = agent.read(context).unwrap();
    assert_eq!(adopted.phase, Phase::Authorized);
    assert!(adopted.result.is_ok());
    assert!(server.join().unwrap().is_ok());
    let reply: Reply = channel.read(context).unwrap();
    assert_eq!(reply.operation, context.identity());
    assert_eq!(reply.phase, Phase::Authorized);
    assert!(reply.result.is_ok());
    target_nonce
}

fn command(nonce: &str, sequence: u64, operation: &OperationContext) -> Command {
    Command::Execute {
        nonce: nonce.into(),
        sequence,
        operation: operation.transfer().unwrap(),
        request: serde_json::json!({"value": 17}),
    }
}

#[test]
fn every_request_rechecks_authority_and_revocation_prevents_dispatch() {
    let registry = Registry::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let revoked = Arc::new(AtomicBool::new(false));
    let count = Arc::clone(&calls);
    let denied = Arc::clone(&revoked);
    let (mut client, server) = start_host(
        registry.clone(),
        move |request, _| {
            assert_eq!(request.caller_uid(), 1001);
            count.fetch_add(1, Ordering::AcqRel);
            Ok(request.body().clone())
        },
        |credential| {
            assert_eq!(credential.0, [7; 32]);
            Ok(())
        },
        move |_| {
            if denied.load(Ordering::Acquire) {
                Err(PermissionOutcome::Denied)
            } else {
                Ok(())
            }
        },
    );
    let context = OperationContext::default().child(Duration::from_secs(2));
    let nonce = authorize(&mut client, &context, registry);
    client
        .write(&command(&nonce, 1, &context), &context)
        .unwrap();
    let first: Reply = client.read(&context).unwrap();
    assert_eq!(first.phase, Phase::Completed);
    assert_eq!(first.sequence, 1);
    assert_eq!(first.result.unwrap(), serde_json::json!({"value": 17}));
    revoked.store(true, Ordering::Release);
    client
        .write(&command(&nonce, 2, &context), &context)
        .unwrap();
    let second: Reply = client.read(&context).unwrap();
    assert_eq!(second.result, Err(PermissionOutcome::Denied));
    assert_eq!(calls.load(Ordering::Acquire), 1);
    drop(client);
    assert!(server.join().unwrap().is_err());
}

#[test]
fn raw_credentials_on_a_host_channel_never_replace_an_authorizer_handoff() {
    for after_preparation in [false, true] {
        let registry = Registry::default();
        let imports = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&imports);
        // Model the OS accepting cached credentials for any right. This must still not make
        // a host-supplied token valid: transport purpose and issuer identity are independent.
        let (mut client, server) = start_host(
            registry,
            |_, _| panic!("Unverified handoff cannot execute"),
            move |_| {
                count.fetch_add(1, Ordering::AcqRel);
                Ok(())
            },
            |_| Ok(()),
        );
        let context = OperationContext::default().child(Duration::from_secs(2));
        let nonce = if after_preparation {
            prepare(&mut client, &context)
        } else {
            client.read::<Hello>(&context).unwrap().nonce
        };
        client
            .write(
                &Command::Adopt {
                    nonce,
                    credential: ExternalAuthorization([7; 32]),
                },
                &context,
            )
            .unwrap();
        if let Ok(reply) = client.read::<Reply>(&context) {
            assert_eq!(reply.phase, Phase::Stopped);
            assert!(reply.result.is_err());
        }
        assert!(server.join().unwrap().is_err());
        assert_eq!(imports.load(Ordering::Acquire), 0);
    }
}

#[test]
fn a_challenge_for_another_service_cannot_trigger_authorization() {
    let cleanup_registry = Registry::default();
    let context = OperationContext::default().child(Duration::from_secs(2));
    let target = uuid::Uuid::new_v4().to_string();
    let _pending = cleanup_registry
        .prepare(&target, caller(), &context)
        .unwrap();
    let (left, right) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        serve_authorizer(
            Channel::new(left).unwrap(),
            ServiceKind::Metadata,
            Registry::default(),
            |_| panic!("Foreign challenge cannot reach issuer verification"),
        )
    });
    let mut agent = Channel::new(right).unwrap();
    let hello: Hello = agent.read(&context).unwrap();
    agent
        .write(
            &Command::Attach {
                nonce: hello.nonce,
                target_nonce: target,
                operation: context.transfer().unwrap(),
            },
            &context,
        )
        .unwrap();
    let reply: Reply = agent.read(&context).unwrap();
    assert_eq!(reply.phase, Phase::Prepared);
    assert_eq!(reply.result, Err(PermissionOutcome::HelperUntrusted));
    assert!(server.join().unwrap().is_err());
}

#[test]
fn a_prepared_handoff_cannot_outlive_its_host_or_authorizers_identity() {
    for disconnect_host in [false, true] {
        let registry = Registry::default();
        let context = OperationContext::default().child(Duration::from_secs(2));
        let (mut host, host_server) = start_host(
            registry.clone(),
            |_, _| panic!("Lost identity cannot execute"),
            |_| panic!("Lost identity cannot import a credential"),
            |_| Ok(()),
        );
        let target_nonce = prepare(&mut host, &context);
        let (left, right) = UnixStream::pair().unwrap();
        let checks = Arc::new(AtomicUsize::new(0));
        let verified = Arc::clone(&checks);
        let authorizer_server = std::thread::spawn(move || {
            serve_authorizer(
                Channel::new(left).unwrap(),
                ServiceKind::Metadata,
                registry,
                |_| {
                    if verified.fetch_add(1, Ordering::AcqRel) == 0 {
                        Ok(())
                    } else {
                        Err(PermissionOutcome::HelperUntrusted)
                    }
                },
            )
        });
        let mut agent = Channel::new(right).unwrap();
        let hello: Hello = agent.read(&context).unwrap();
        agent
            .write(
                &Command::Attach {
                    nonce: hello.nonce.clone(),
                    target_nonce,
                    operation: context.transfer().unwrap(),
                },
                &context,
            )
            .unwrap();
        let ready: Reply = agent.read(&context).unwrap();
        assert!(ready.result.is_ok());
        assert_eq!(checks.load(Ordering::Acquire), 1);
        if disconnect_host {
            drop(host);
            assert!(host_server.join().unwrap().is_err());
            // No delivery is necessary: losing the host stops an already claimed handoff.
            assert!(agent.read::<Reply>(&context).is_err());
            assert_eq!(checks.load(Ordering::Acquire), 1);
        } else {
            agent
                .write(
                    &Command::Adopt {
                        nonce: hello.nonce,
                        credential: ExternalAuthorization([7; 32]),
                    },
                    &context,
                )
                .unwrap();
            assert!(agent.read::<Reply>(&context).is_err());
            assert_eq!(checks.load(Ordering::Acquire), 2);
            drop(host);
            assert!(host_server.join().unwrap().is_err());
        }
        assert!(authorizer_server.join().unwrap().is_err());
    }
}

#[test]
fn stale_sequences_wrong_nonce_and_expired_requests_never_reach_the_handler() {
    for mode in ["sequence", "nonce", "expired"] {
        let registry = Registry::default();
        let (mut client, server) = start_host(
            registry.clone(),
            |_, _| panic!("Invalid request dispatched"),
            |_| Ok(()),
            |_| Ok(()),
        );
        let context = OperationContext::default().child(Duration::from_secs(2));
        let nonce = authorize(&mut client, &context, registry);
        let mut request = command(&nonce, 1, &context);
        let Command::Execute {
            nonce,
            sequence,
            operation,
            ..
        } = &mut request
        else {
            unreachable!()
        };
        match mode {
            "sequence" => *sequence = 0,
            "nonce" => *nonce = uuid::Uuid::new_v4().to_string(),
            _ => operation.deadline_monotonic_ns = 0,
        }
        client.write(&request, &context).unwrap();
        if let Ok(reply) = client.read::<Reply>(&context) {
            assert_eq!(reply.phase, Phase::Stopped);
            assert!(reply.result.is_err());
        }
        assert!(server.join().unwrap().is_err());
    }
}

#[test]
fn disconnecting_the_host_cancels_the_inflight_original_operation() {
    let registry = Registry::default();
    let (started, ready) = std::sync::mpsc::channel();
    let (mut client, server) = start_host(
        registry.clone(),
        move |_, context| {
            started.send(context.identity()).unwrap();
            while !context.is_stopped() {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(context.cancellation.is_cancelled());
            Ok(serde_json::Value::Null)
        },
        |_| Ok(()),
        |_| Ok(()),
    );
    let context = OperationContext::default().child(Duration::from_secs(2));
    let nonce = authorize(&mut client, &context, registry);
    client
        .write(&command(&nonce, 1, &context), &context)
        .unwrap();
    assert_eq!(
        ready.recv_timeout(Duration::from_secs(1)).unwrap(),
        context.identity()
    );
    drop(client);
    assert!(server.join().unwrap().is_err());
}

#[test]
fn a_late_os_check_cannot_start_work_after_the_host_disconnects() {
    let registry = Registry::default();
    let (entered, started) = std::sync::mpsc::channel();
    let release = Arc::new(AtomicBool::new(false));
    let verification = Arc::clone(&release);
    let (mut client, server) = start_host(
        registry.clone(),
        |_, _| panic!("Disconnected caller started work"),
        |_| Ok(()),
        move |_| {
            entered.send(()).unwrap();
            while !verification.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(())
        },
    );
    let context = OperationContext::default().child(Duration::from_secs(2));
    let nonce = authorize(&mut client, &context, registry);
    client
        .write(&command(&nonce, 1, &context), &context)
        .unwrap();
    started.recv_timeout(Duration::from_secs(1)).unwrap();
    drop(client);
    release.store(true, Ordering::Release);
    assert!(server.join().unwrap().is_err());
}

#[test]
fn a_host_stop_receipt_requires_actual_worker_reaping_even_during_blocking_calls() {
    for stage in ["initialize", "adopt", "idle", "verify", "execute"] {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("worker");
        let registry = Registry::default();
        let parent_registry = registry.clone();
        let child_marker = marker.clone();
        let (left, right) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            serve_host(
                HostSession {
                    channel: Channel::new(left).unwrap(),
                    kind: ServiceKind::Metadata,
                    caller: caller(),
                    registry: parent_registry,
                },
                |_, context| worker::launch_test(stage, &child_marker, context),
                worker::Worker::initialize,
                worker::Worker::adopt,
                worker::Worker::verify,
                worker::Worker::execute,
                worker::Worker::close,
            )
        });
        let mut host = Channel::new(right).unwrap();
        let context = OperationContext::default().child(Duration::from_secs(5));
        let mut delivery = None;
        let nonce = match stage {
            "initialize" => {
                let hello: Hello = host.read(&context).unwrap();
                host.write(
                    &Command::Prepare {
                        nonce: hello.nonce.clone(),
                        operation: context.transfer().unwrap(),
                    },
                    &context,
                )
                .unwrap();
                hello.nonce
            }
            "adopt" => {
                let nonce = prepare(&mut host, &context);
                let claim = registry
                    .claim(&nonce, &context.identity(), |_| Ok(()))
                    .unwrap();
                delivery = Some(std::thread::spawn(move || {
                    claim.deliver(ExternalAuthorization([7; 32]))
                }));
                nonce
            }
            _ => authorize(&mut host, &context, registry),
        };
        let sequence = if matches!(stage, "verify" | "execute") {
            host.write(&command(&nonce, 1, &context), &context).unwrap();
            1
        } else {
            0
        };
        if stage != "idle" {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while !marker.with_extension("stage").exists() {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        termination::stop_and_confirm(&mut host, &nonce, sequence, &context.identity()).unwrap();
        let pid = std::fs::read_to_string(&marker)
            .unwrap()
            .parse::<i32>()
            .unwrap();
        assert_eq!(
            rustix::process::test_kill_process(rustix::process::Pid::from_raw(pid).unwrap()),
            Err(rustix::io::Errno::SRCH)
        );
        assert!(server.join().unwrap().is_err());
        if let Some(delivery) = delivery {
            assert!(delivery.join().unwrap().is_err());
        }
    }
}

#[test]
fn a_failed_worker_reap_does_not_issue_a_terminal_confirmation() {
    let registry = Registry::default();
    let parent_registry = registry.clone();
    let (left, right) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        serve_host(
            HostSession {
                channel: Channel::new(left).unwrap(),
                kind: ServiceKind::Metadata,
                caller: caller(),
                registry: parent_registry,
            },
            |_, _| Ok(()),
            |_, _| Ok(()),
            |_, _, _| Ok(()),
            |_, _| Ok(()),
            |_, _, _, _| Ok(serde_json::Value::Null),
            |_| Err(PermissionOutcome::AuthorizationUnavailable),
        )
    });
    let mut host = Channel::new(right).unwrap();
    let context = OperationContext::default().child(Duration::from_secs(2));
    let nonce = authorize(&mut host, &context, registry);
    assert_eq!(
        termination::stop_and_confirm(&mut host, &nonce, 0, &context.identity()),
        Err(PermissionOutcome::AuthorizationUnavailable)
    );
    assert!(server.join().unwrap().is_err());
}

#[test]
fn real_worker_progress_arrives_in_order_and_the_next_request_does_not_cancel_the_previous_one() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("worker");
    let file = root.path().join("file");
    std::fs::write(&file, b"payload").unwrap();
    let registry = Registry::default();
    let parent_registry = registry.clone();
    let (left, right) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        serve_host(
            HostSession {
                channel: Channel::new(left).unwrap(),
                kind: ServiceKind::Metadata,
                caller: caller(),
                registry: parent_registry,
            },
            |_, context| worker::launch_test("stream", &marker, context),
            worker::Worker::initialize,
            worker::Worker::adopt,
            worker::Worker::verify,
            worker::Worker::execute,
            worker::Worker::close,
        )
    });
    let mut host = Channel::new(right).unwrap();
    let context = OperationContext::default().child(Duration::from_secs(8));
    let nonce = authorize(&mut host, &context, registry);
    for sequence in 1..=8 {
        host.reset_budget();
        host.write(
            &Command::Execute {
                nonce: nonce.clone(),
                sequence,
                operation: context.transfer().unwrap(),
                request: serde_json::json!({"path": file}),
            },
            &context,
        )
        .unwrap();
        for (index, step) in ["first", "second"].iter().enumerate() {
            let reply: Reply = host.read(&context).unwrap();
            assert_eq!(reply.phase, Phase::Progress);
            assert_eq!(
                (reply.sequence, reply.event_sequence),
                (sequence, index as u64)
            );
            assert_eq!(reply.operation, context.identity());
            assert_eq!(reply.result.unwrap()["step"], *step);
        }
        let reply: Reply = host.read(&context).unwrap();
        assert_eq!(reply.phase, Phase::Completed);
        assert_eq!(reply.event_sequence, 2);
        assert_eq!(reply.result.unwrap()["size"], 7);
    }
    termination::stop_and_confirm(&mut host, &nonce, 8, &context.identity()).unwrap();
    assert!(server.join().unwrap().is_err());
    assert!(file.exists());
}

#[test]
fn client_cancellation_confirms_a_reaped_process_only_with_the_terminal_receipt() {
    for acknowledge in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("worker");
        let child_marker = marker.clone();
        let registry = Registry::default();
        let parent_registry = registry.clone();
        let (left, right) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            serve_host(
                HostSession {
                    channel: Channel::new(left).unwrap(),
                    kind: ServiceKind::Metadata,
                    caller: caller(),
                    registry: parent_registry,
                },
                |_, context| worker::launch_test("execute", &child_marker, context),
                worker::Worker::initialize,
                worker::Worker::adopt,
                worker::Worker::verify,
                worker::Worker::execute,
                |worker| {
                    worker.close()?;
                    if acknowledge {
                        Ok(())
                    } else {
                        Err(PermissionOutcome::AuthorizationUnavailable)
                    }
                },
            )
        });
        let context = OperationContext::default().child(Duration::from_secs(5));
        let mut host = Channel::new(right).unwrap();
        let nonce = authorize(&mut host, &context, registry);
        let mut client = client::ServiceClient::new(host, nonce, context.identity());
        client.send(&serde_json::Value::Null, &context).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !marker.with_extension("stage").exists() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        context.cancel();
        let expected = if acknowledge {
            PermissionOutcome::Cancelled
        } else {
            PermissionOutcome::AuthorizationUnavailable
        };
        assert_eq!(client.wait::<()>(&context), Err(expected));
        assert_eq!(
            client.close(),
            if acknowledge { Ok(()) } else { Err(expected) }
        );
        let pid = std::fs::read_to_string(&marker)
            .unwrap()
            .parse::<i32>()
            .unwrap();
        assert_eq!(
            rustix::process::test_kill_process(rustix::process::Pid::from_raw(pid).unwrap()),
            Err(rustix::io::Errno::SRCH)
        );
        assert!(server.join().unwrap().is_err());
    }
}
