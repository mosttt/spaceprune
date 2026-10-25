use super::*;
use std::time::Instant;

const CHILD: &str = "macos::service::worker::tests::worker_child";

fn command(name: &str, mode: &str, marker: &Path) -> ProcessCommand {
    let mut command = ProcessCommand::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", CHILD, "--ignored", "--nocapture"])
        .env("SPACEPRUNE_WORKER_TEST_ENDPOINT", name)
        .env("SPACEPRUNE_WORKER_TEST_MODE", mode)
        .env("SPACEPRUNE_WORKER_TEST_MARKER", marker);
    command
}
fn verify_peer(channel: &Channel, pid: u32) -> Result<(), PermissionOutcome> {
    let peer = rustix::net::sockopt::socket_peercred(&channel.stream)
        .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    if peer.pid.as_raw_nonzero().get() as u32 != pid || peer.uid != rustix::process::geteuid() {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    Ok(())
}
pub(in crate::macos::service) fn launch(
    mode: &str,
    marker: &Path,
    context: &OperationContext,
) -> Result<Worker, PermissionOutcome> {
    Worker::launch(
        ServiceKind::Metadata,
        1234,
        context,
        |name| command(name, mode, marker),
        verify_peer,
    )
}
fn reaped(marker: &Path) {
    let pid = std::fs::read_to_string(marker)
        .unwrap()
        .parse::<i32>()
        .unwrap();
    assert_eq!(
        rustix::process::test_kill_process(rustix::process::Pid::from_raw(pid).unwrap()),
        Err(rustix::io::Errno::SRCH)
    );
}
struct FixturePolicy {
    mode: String,
    marker: PathBuf,
}
impl FixturePolicy {
    fn enter(&self, stage: &str) {
        if self.mode == stage {
            std::fs::write(self.marker.with_extension("stage"), stage).unwrap();
            // Deliberately noncooperative: the native watchdog or supervisor must stop this process.
            std::thread::sleep(Duration::from_secs(30));
        }
    }
}
impl server::AuthorityPolicy for FixturePolicy {
    type Authority = ();
    fn initialize(&self) -> Result<(), PermissionOutcome> {
        self.enter("initialize");
        Ok(())
    }
    fn adopt(&self, credential: &ExternalAuthorization) -> Result<(), PermissionOutcome> {
        assert_eq!(credential.0, [7; 32]);
        self.enter("adopt");
        Ok(())
    }
    fn verify(&self, _: &()) -> Result<(), PermissionOutcome> {
        self.enter("verify");
        if self.marker.with_extension("revoked").exists() {
            Err(PermissionOutcome::Denied)
        } else {
            Ok(())
        }
    }
}

#[test]
#[ignore = "Real child process used by the supervised service-worker tests"]
fn worker_child() {
    let Some(endpoint) = std::env::var_os("SPACEPRUNE_WORKER_TEST_ENDPOINT") else {
        return;
    };
    let mode = std::env::var("SPACEPRUNE_WORKER_TEST_MODE").unwrap();
    let marker = PathBuf::from(std::env::var_os("SPACEPRUNE_WORKER_TEST_MARKER").unwrap());
    std::fs::write(&marker, std::process::id().to_string()).unwrap();
    let policy = FixturePolicy {
        mode: mode.clone(),
        marker: marker.clone(),
    };
    let executor = FixturePolicy { mode, marker };
    let context = OperationContext::default();
    let channel = Channel::connect(endpoint.to_str().unwrap(), &context).unwrap();
    let code = server::serve(
        channel,
        ServiceKind::Metadata,
        |channel| {
            verify_peer(
                channel,
                rustix::process::getppid().unwrap().as_raw_nonzero().get() as u32,
            )
        },
        policy,
        |request, operation, events| {
            executor.enter("execute");
            if executor.mode == "stream" {
                events.emit(serde_json::json!({"step": "first"}))?;
                events.emit(serde_json::json!({"step": "second"}))?;
            }
            assert_eq!(request.caller_uid(), 1234);
            let path = request.body()["path"].as_str().unwrap();
            let size = std::fs::metadata(path).unwrap().len();
            Ok(serde_json::json!({"size": size, "operation": operation.identity()}))
        },
        || std::process::exit(75),
    )
    .map_or(77, |()| 0);
    std::process::exit(code);
}

#[test]
fn noncooperative_initialization_import_verification_and_work_are_reaped_before_return() {
    for stage in ["initialize", "adopt", "verify", "execute"] {
        for cancel in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let marker = root.path().join("worker");
            let context = OperationContext::default().child(Duration::from_millis(if cancel {
                2000
            } else {
                350
            }));
            let target = marker.with_extension("stage");
            let signal = context.clone();
            let canceller = cancel.then(|| {
                std::thread::spawn(move || {
                    let deadline = Instant::now() + Duration::from_secs(2);
                    while !target.exists() {
                        assert!(Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    signal.cancel();
                })
            });
            let began = Instant::now();
            let result = (|| {
                let mut worker = launch(stage, &marker, &context)?;
                worker.initialize(&context)?;
                worker.adopt(&ExternalAuthorization([7; 32]), &context)?;
                worker.verify(&context)?;
                worker.execute(
                    &AuthorizedRequest::from_worker(1234, serde_json::json!({"path": root.path()})),
                    &context,
                    &|_| Ok(()),
                )
            })();
            assert!(result.is_err(), "{stage}");
            if let Some(canceller) = canceller {
                canceller.join().unwrap();
            }
            assert!(began.elapsed() < Duration::from_secs(3), "{stage}");
            reaped(&marker);
        }
    }
}

#[test]
fn live_worker_reuses_its_reference_but_requires_fresh_matching_verification() {
    for invalid in ["replay", "other-operation", "revoked"] {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("worker");
        let file = root.path().join("fixture");
        std::fs::write(&file, b"one").unwrap();
        let context = OperationContext::default().child(Duration::from_secs(3));
        let mut worker = launch("normal", &marker, &context).unwrap();
        worker.initialize(&context).unwrap();
        worker
            .adopt(&ExternalAuthorization([7; 32]), &context)
            .unwrap();
        let request = AuthorizedRequest::from_worker(1234, serde_json::json!({"path": file}));
        for size in [3, 18] {
            std::fs::write(&file, vec![b'x'; size]).unwrap();
            worker.verify(&context).unwrap();
            let result = worker.execute(&request, &context, &|_| Ok(())).unwrap();
            assert_eq!(result["size"], size);
            assert_eq!(
                result["operation"],
                serde_json::to_value(context.identity()).unwrap()
            );
        }
        let result = match invalid {
            "replay" => worker.execute(&request, &context, &|_| Ok(())),
            "other-operation" => {
                worker.verify(&context).unwrap();
                worker.execute(&request, &OperationContext::default(), &|_| Ok(()))
            }
            _ => {
                std::fs::write(marker.with_extension("revoked"), b"revoked").unwrap();
                worker.verify(&context).map(|()| serde_json::Value::Null)
            }
        };
        assert!(result.is_err());
        reaped(&marker);
        assert!(file.exists());
    }
}

#[test]
fn a_worker_stops_itself_when_its_service_disappears_during_native_work() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("worker");
    let context = OperationContext::default().child(Duration::from_secs(10));
    let mut worker = launch("verify", &marker, &context).unwrap();
    worker.initialize(&context).unwrap();
    worker
        .adopt(&ExternalAuthorization([7; 32]), &context)
        .unwrap();
    let request = Request {
        version: WORKER_VERSION,
        nonce: worker.nonce.clone(),
        sequence: worker.sequence + 1,
        operation: context.transfer().unwrap(),
        action: Action::Verify,
    };
    worker
        .channel
        .as_mut()
        .unwrap()
        .write(&request, &context)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !marker.with_extension("stage").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    worker.channel.take();
    while !worker.process.has_exited().unwrap() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        worker.process.terminate_and_wait().unwrap().code(),
        Some(75)
    );
    reaped(&marker);
}

#[test]
fn legacy_launchd_dispatch_uses_only_the_fixed_service_executable_name() {
    let metadata = ServiceKind::Metadata.legacy_executable();
    assert!(legacy_daemon_entry(ServiceKind::Metadata, &metadata));
    assert!(!legacy_daemon_entry(ServiceKind::Cleanup, &metadata));
    assert!(!legacy_daemon_entry(
        ServiceKind::Metadata,
        Path::new("/usr/local/bin/spaceprune")
    ));
    assert!(!legacy_daemon_entry(
        ServiceKind::Cleanup,
        Path::new("/tmp/spaceprune-cleanup-helper")
    ));
}
