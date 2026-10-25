use super::*;
use std::path::Path;

const BLOCKING_FIXTURE: &str = r#"
import json, os, pathlib, socket, sys, time
root = pathlib.Path(sys.argv[1])
stage = sys.argv[2]
(root / 'pid').write_text(str(os.getpid()))
def block():
    (root / 'blocked').touch()
    time.sleep(30)
if stage == 'authorization':
    block()
print('spaceprune-metadata-v6', flush=True)
request = json.loads(pathlib.Path(json.loads(sys.stdin.readline())['requestPath']).read_text())
(root / 'request.json').write_text(json.dumps(request))
if stage == 'acknowledgement':
    block()
print('0', flush=True)
if stage == 'response-read':
    stream = socket.create_connection(('127.0.0.1', request['response_port']))
    stream.sendall(b'{')
block()
"#;

fn wait_for(message: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !ready() {
        assert!(Instant::now() < deadline, "{message}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn worker_exists(pid_file: &Path) -> bool {
    let pid = fs::read_to_string(pid_file).unwrap();
    Path::new(&format!("/proc/{pid}")).exists()
}

fn blocking_service(root: &Path, stage: &'static str) -> (MetadataService, Arc<AtomicUsize>) {
    let root = root.to_owned();
    let launches = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&launches);
    (
        MetadataService {
            launcher: Some(Arc::new(move |operation| {
                let mut command = Command::new("python3");
                if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    command.args(["-u", "-c", BLOCKING_FIXTURE]);
                    command.arg(&root).arg(stage);
                } else {
                    command.args(["-u", "-c", FIXTURE]);
                }
                Connection::launch(&mut command, operation).map_err(Into::into)
            })),
            ..MetadataService::default()
        },
        launches,
    )
}

#[test]
fn close_cancels_an_in_flight_request_before_returning() {
    let root = tempfile::tempdir().unwrap();
    let (service, _) = blocking_service(root.path(), "acknowledgement");
    let operation = operation();
    std::thread::scope(|scope| {
        let pending = scope
            .spawn(|| service.scan(&task("builtin.filesystem/paths", root.path()), &operation));
        wait_for("fixture did not receive its request", || {
            root.path().join("blocked").exists()
        });
        service.close().unwrap();
        let reclaimed = !worker_exists(&root.path().join("pid"));
        // Always release the fixture on the unfixed implementation before asserting.
        operation.cancel();
        assert_eq!(pending.join().unwrap(), Err(PermissionOutcome::Cancelled));
        assert!(
            reclaimed,
            "close returned while the metadata worker was still alive"
        );
    });
}

#[test]
fn close_stops_authorization_and_connection_queues_then_allows_fresh_reuse() {
    for stage in ["authorization", "acknowledgement"] {
        let root = tempfile::tempdir().unwrap();
        let (service, launches) = blocking_service(root.path(), stage);
        let mut operation = operation();
        operation.generation = 7;
        operation.budget.output_bytes = 131_072;
        let task = task("builtin.filesystem/paths", root.path());
        std::thread::scope(|scope| {
            let first = scope.spawn(|| service.scan(&task, &operation));
            wait_for("fixture did not reach its blocking stage", || {
                root.path().join("blocked").exists()
            });
            let queued = (0..3)
                .map(|_| scope.spawn(|| service.scan(&task, &operation)))
                .collect::<Vec<_>>();
            wait_for("concurrent requests did not enter the service", || {
                service.requests.active_count() == 4
            });
            let started = Instant::now();
            service.close().unwrap();
            assert!(started.elapsed() < Duration::from_secs(2));
            assert!(!worker_exists(&root.path().join("pid")));
            assert!(!service.is_active());
            assert_eq!(first.join().unwrap(), Err(PermissionOutcome::Cancelled));
            for pending in queued {
                assert_eq!(pending.join().unwrap(), Err(PermissionOutcome::Cancelled));
            }
        });
        assert!(!operation.cancellation.is_cancelled());
        assert_eq!(launches.load(Ordering::SeqCst), 1);
        if stage == "acknowledgement" {
            let request: serde_json::Value =
                serde_json::from_slice(&fs::read(root.path().join("request.json")).unwrap())
                    .unwrap();
            assert_eq!(request["context"]["identity"]["operation_id"], operation.id);
            assert_eq!(request["context"]["identity"]["generation"], 7);
            assert_eq!(request["scan_task"]["options"], task.options);
        }
        // A service-level close must not cancel the caller or poison the next native generation.
        for _ in 0..2 {
            service.scan(&task, &operation).unwrap();
        }
        assert_eq!(launches.load(Ordering::SeqCst), 2);
        assert!(service.is_active());
        service.close().unwrap();
    }
}

#[test]
fn close_cancels_response_accept_and_partial_reads_after_native_acknowledgement() {
    for scan in [false, true] {
        for stage in ["response-accept", "response-read"] {
            let root = tempfile::tempdir().unwrap();
            let (service, _) = blocking_service(root.path(), stage);
            let operation = operation();
            let request = PreparedRequest::create("discovery-source-1", &[root.path().to_owned()])
                .unwrap()
                .with_operation(&operation)
                .unwrap();
            std::thread::scope(|scope| {
                let pending = scope.spawn(|| {
                    if scan {
                        service
                            .scan(&task("builtin.filesystem/paths", root.path()), &operation)
                            .map(|_| ())
                    } else {
                        service.measure(&request, &operation).map(|_| ())
                    }
                });
                wait_for("fixture did not acknowledge its request", || {
                    root.path().join("blocked").exists()
                });
                let started = Instant::now();
                service.close().unwrap();
                assert_eq!(pending.join().unwrap(), Err(PermissionOutcome::Cancelled));
                assert!(started.elapsed() < Duration::from_secs(1));
                assert!(!worker_exists(&root.path().join("pid")));
                assert!(!operation.cancellation.is_cancelled());
            });
        }
    }
}

#[test]
fn close_reaps_an_idle_helper_and_preserves_operation_denials() {
    let root = tempfile::tempdir().unwrap();
    let pid_file = root.path().join("pid");
    let (service, _) = service(
        &format!(
            "import os, pathlib; pathlib.Path({:?}).write_text(str(os.getpid()))\n{FIXTURE}",
            pid_file.to_str().unwrap()
        ),
        false,
    );
    service
        .scan(&task("builtin.filesystem/paths", root.path()), &operation())
        .unwrap();
    assert!(worker_exists(&pid_file));
    service.close().unwrap();
    assert!(!worker_exists(&pid_file));

    let (service, launches) = super::service("import sys; sys.exit(126)", false);
    let operation = operation();
    for _ in 0..2 {
        assert_eq!(
            service.scan(&task("builtin.filesystem/paths", root.path()), &operation),
            Err(PermissionOutcome::Denied)
        );
        service.close().unwrap();
    }
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    assert_eq!(
        service.scan(
            &task("builtin.filesystem/paths", root.path()),
            &super::operation()
        ),
        Err(PermissionOutcome::Denied)
    );
    assert_eq!(launches.load(Ordering::SeqCst), 2);
}

fn fail_shutdown(service: &mut MetadataService, failure: PermissionOutcome) {
    let launcher = service.launcher.take().unwrap();
    service.launcher = Some(Arc::new(move |operation| {
        launcher(operation).map(|mut connection| {
            connection.close_failure = Some(failure);
            connection
        })
    }));
}

#[test]
fn failed_shutdown_survives_cache_removal_and_prevents_reauthorization() {
    for in_flight in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (mut service, launches) = if in_flight {
            blocking_service(root.path(), "acknowledgement")
        } else {
            super::service(FIXTURE, false)
        };
        fail_shutdown(&mut service, PermissionOutcome::TimedOut);
        let operation = operation();
        let task = task("builtin.filesystem/paths", root.path());
        std::thread::scope(|scope| {
            let pending = scope.spawn(|| service.scan(&task, &operation));
            if in_flight {
                wait_for("fixture did not start", || {
                    root.path().join("blocked").exists()
                });
            } else {
                wait_for("ordinary scan did not complete", || pending.is_finished());
            }
            assert_eq!(service.close(), Err(PermissionOutcome::TimedOut));
            let result = pending.join().unwrap();
            assert_eq!(result.is_err(), in_flight);
        });
        assert!(!operation.is_stopped());
        assert!(!service.is_active());
        assert_eq!(service.close(), Err(PermissionOutcome::TimedOut));
        assert_eq!(
            service.scan(&task, &super::operation()),
            Err(PermissionOutcome::TimedOut)
        );
        assert_eq!(launches.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn replacing_an_exited_connection_cannot_hide_its_failed_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let (mut service, launches) = service(FIXTURE, true);
    fail_shutdown(&mut service, PermissionOutcome::HelperUntrusted);
    let task = task("builtin.filesystem/paths", root.path());
    service.scan(&task, &operation()).unwrap();
    wait_for("fixture did not exit", || !service.is_active());
    assert_eq!(
        service.scan(&task, &operation()),
        Err(PermissionOutcome::HelperUntrusted)
    );
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    assert_eq!(service.close(), Err(PermissionOutcome::HelperUntrusted));
}

#[test]
fn only_explicit_unconfirmed_startup_shutdown_blocks_future_requests() {
    let root = tempfile::tempdir().unwrap();
    for shutdown_unconfirmed in [false, true] {
        let launches = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&launches);
        let service = MetadataService {
            launcher: Some(Arc::new(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(StartupFailure {
                    outcome: PermissionOutcome::AuthorizationUnavailable,
                    shutdown_unconfirmed,
                })
            })),
            ..MetadataService::default()
        };
        let task = task("builtin.filesystem/paths", root.path());
        for _ in 0..2 {
            assert_eq!(
                service.scan(&task, &operation()),
                Err(PermissionOutcome::AuthorizationUnavailable)
            );
            assert_eq!(
                service.close(),
                if shutdown_unconfirmed {
                    Err(PermissionOutcome::AuthorizationUnavailable)
                } else {
                    Ok(())
                }
            );
        }
        assert_eq!(
            launches.load(Ordering::SeqCst),
            if shutdown_unconfirmed { 1 } else { 2 }
        );
    }
}

fn login_revocation(service: &MetadataService) -> LoginRevocation {
    service
        .login
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .0
        .revocation()
}

#[test]
fn login_switch_cancels_native_work_and_cannot_resume_the_old_operation() {
    for stage in ["authorization", "acknowledgement", "response-read"] {
        let root = tempfile::tempdir().unwrap();
        let (service, launches) = blocking_service(root.path(), stage);
        let original = operation();
        let task = task("builtin.filesystem/paths", root.path());
        std::thread::scope(|scope| {
            let pending = scope.spawn(|| service.scan(&task, &original));
            wait_for("fixture did not start native work", || {
                root.path().join("blocked").exists()
            });
            let revoked = login_revocation(&service);
            revoked.invalidate();
            assert_eq!(pending.join().unwrap(), Err(PermissionOutcome::Cancelled));
            assert!(!worker_exists(&root.path().join("pid")));
            assert!(!service.is_active());
            assert!(revoked.is_revoked());
        });
        // The caller can still do ordinary work, but the revoked operation cannot prompt again.
        assert!(!original.is_stopped());
        assert_eq!(
            service.scan(&task, &original),
            Err(PermissionOutcome::Cancelled)
        );
        assert_eq!(launches.load(Ordering::SeqCst), 1);
        let fresh = operation();
        service.scan(&task, &fresh).unwrap();
        assert_eq!(launches.load(Ordering::SeqCst), 2);
        assert!(service.is_active());
        assert_eq!(
            service.scan(&task, &original),
            Err(PermissionOutcome::Cancelled)
        );
        service.close().unwrap();
    }
}

#[test]
fn idle_login_revocation_reaps_its_worker_and_retains_the_first_close_failure() {
    for failure in [None, Some(PermissionOutcome::TimedOut)] {
        let root = tempfile::tempdir().unwrap();
        let pid_file = root.path().join("pid");
        let (mut service, launches) = service(
            &format!(
                "import os, pathlib; pathlib.Path({:?}).write_text(str(os.getpid()))\n{FIXTURE}",
                pid_file.to_str().unwrap()
            ),
            false,
        );
        if let Some(failure) = failure {
            fail_shutdown(&mut service, failure);
        }
        let original = operation();
        let task = task("builtin.filesystem/paths", root.path());
        service.scan(&task, &original).unwrap();
        let revocation = login_revocation(&service);
        revocation.invalidate();
        assert!(!worker_exists(&pid_file));
        assert!(!service.is_active());
        assert_eq!(service.close(), failure.map_or(Ok(()), Err));
        if let Some(failure) = failure {
            assert_eq!(service.scan(&task, &operation()), Err(failure));
            revocation.invalidate();
            assert_eq!(service.close(), Err(failure));
            assert_eq!(launches.load(Ordering::SeqCst), 1);
        } else {
            service.scan(&task, &operation()).unwrap();
            assert_eq!(launches.load(Ordering::SeqCst), 2);
            service.close().unwrap();
        }
    }
}

#[test]
fn background_binding_is_reconfirmed_for_foreground_use_but_never_downgrades() {
    let root = tempfile::tempdir().unwrap();
    let (service, launches) = service(FIXTURE, false);
    let task = task("builtin.filesystem/paths", root.path());
    let mut background = operation();
    background.interaction = Interaction::NonInteractive;
    service.scan(&task, &background).unwrap();
    let previous = login_revocation(&service);
    assert_eq!(launches.load(Ordering::SeqCst), 1);

    let foreground = operation();
    service.scan(&task, &foreground).unwrap();
    assert_eq!(launches.load(Ordering::SeqCst), 2);
    assert!(
        !previous.is_revoked(),
        "ordinary replacement must not impersonate logout"
    );
    assert_eq!(
        service.login.lock().unwrap().as_ref().unwrap().1,
        Interaction::Foreground
    );

    service.scan(&task, &background).unwrap();
    assert_eq!(launches.load(Ordering::SeqCst), 2);
    assert_eq!(
        service.login.lock().unwrap().as_ref().unwrap().1,
        Interaction::Foreground
    );
    login_revocation(&service).invalidate();
    assert_eq!(
        service.scan(&task, &background),
        Err(PermissionOutcome::Cancelled)
    );
    service.close().unwrap();
}
