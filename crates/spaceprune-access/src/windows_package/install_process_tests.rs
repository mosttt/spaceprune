use super::*;
use crate::{
    CancellationToken, Interaction,
    windows_package::installer_protocol::{Installed, VERSION},
};
use std::{fs, path::Path, time::Instant};

fn operation() -> OperationContext {
    OperationContext::new(
        CancellationToken::default(),
        Interaction::Foreground,
        Duration::from_secs(5),
    )
}
fn request(operation: &OperationContext) -> Request {
    Request {
        version: VERSION,
        host_pid: std::process::id(),
        host_created: 1,
        operation: operation.transfer().unwrap(),
        interaction: operation.interaction,
        package_sha256: "b".repeat(64),
    }
}
fn reply(request: &Request) -> Reply {
    Reply {
        version: VERSION,
        operation: request.operation.identity.clone(),
        package_sha256: request.package_sha256.clone(),
        result: Ok(Installed::Ready),
    }
}
fn command(script: &str, receipt: &Reply) -> Command {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", script]).env(
        "SPACEPRUNE_TEST_INSTALL_REPLY",
        serde_json::to_string(receipt).unwrap(),
    );
    command
}
const FINISH: &str = "IFS= read -r request; printf '%s' \"$SPACEPRUNE_TEST_INSTALL_REPLY\"";

#[test]
fn real_child_receipts_require_matching_operation_package_and_successful_exit() {
    let operation = operation();
    let request = request(&operation);
    assert_eq!(
        run(command(FINISH, &reply(&request)), &request, &operation),
        Ok(())
    );
    let mut stale = reply(&request);
    stale.operation.generation += 1;
    assert_eq!(
        run(command(FINISH, &stale), &request, &operation),
        Err(PermissionOutcome::HelperUntrusted)
    );
    let mut wrong_package = reply(&request);
    wrong_package.package_sha256 = "c".repeat(64);
    assert_eq!(
        run(command(FINISH, &wrong_package), &request, &operation),
        Err(PermissionOutcome::HelperUntrusted)
    );
    assert_eq!(
        run(
            command(&format!("{FINISH}; exit 1"), &reply(&request)),
            &request,
            &operation
        ),
        Err(PermissionOutcome::AuthorizationUnavailable)
    );
    let mut refused = reply(&request);
    refused.result = Err(PermissionOutcome::Denied);
    assert_eq!(
        run(command(FINISH, &refused), &request, &operation),
        Err(PermissionOutcome::Denied)
    );
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "installer fixture did not reach its barrier"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn cancellation_closes_control_pipe_and_waits_for_transaction_exit() {
    let root = tempfile::tempdir().unwrap();
    let started = root.path().join("started");
    let exited = root.path().join("exited");
    let operation = operation();
    let request = request(&operation);
    let mut command = command(
        "IFS= read -r request; printf '%s' \"$$\" > \"$SPACEPRUNE_TEST_STARTED\"; cat >/dev/null; sleep 0.05; touch \"$SPACEPRUNE_TEST_EXITED\"; printf '%s' \"$SPACEPRUNE_TEST_INSTALL_REPLY\"",
        &reply(&request),
    );
    command
        .env("SPACEPRUNE_TEST_STARTED", &started)
        .env("SPACEPRUNE_TEST_EXITED", &exited);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            wait_for(&started);
            operation.cancel();
        });
        assert_eq!(
            run(command, &request, &operation),
            Err(PermissionOutcome::Cancelled)
        );
    });
    assert!(
        exited.exists(),
        "returning Cancelled must await the worker's final cleanup"
    );
    let pid: i32 = fs::read_to_string(started).unwrap().parse().unwrap();
    assert_eq!(
        rustix::process::test_kill_process(rustix::process::Pid::from_raw(pid).unwrap()),
        Err(rustix::io::Errno::SRCH)
    );
}

#[test]
fn deadline_closes_control_pipe_without_turning_an_old_success_into_continuation() {
    let root = tempfile::tempdir().unwrap();
    let exited = root.path().join("exited");
    let operation = OperationContext::new(
        CancellationToken::default(),
        Interaction::Terminal,
        Duration::from_millis(500),
    );
    let request = request(&operation);
    let mut command = command(
        "IFS= read -r request; cat >/dev/null; touch \"$SPACEPRUNE_TEST_EXITED\"; printf '%s' \"$SPACEPRUNE_TEST_INSTALL_REPLY\"",
        &reply(&request),
    );
    command.env("SPACEPRUNE_TEST_EXITED", &exited);
    assert_eq!(
        run(command, &request, &operation),
        Err(PermissionOutcome::TimedOut)
    );
    assert!(exited.exists());
}
