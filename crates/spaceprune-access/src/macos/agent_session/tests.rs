use super::*;
use crate::process::ProcessTree;
use std::{io::Write, process::Command, thread, time::Instant};

const CHILD_TEST: &str = "macos::agent_session::tests::registration_child";

fn command(path: &str, mode: &str, marker: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", CHILD_TEST, "--ignored", "--nocapture"])
        .env("SPACEPRUNE_REGISTRATION_TEST_ENDPOINT", path)
        .env("SPACEPRUNE_REGISTRATION_TEST_MODE", mode)
        .env("SPACEPRUNE_REGISTRATION_TEST_MARKER", marker);
    command
}

#[cfg(target_os = "linux")]
fn verify(channel: &Channel, pid: u32) -> Result<(), PermissionOutcome> {
    let credential = rustix::net::sockopt::socket_peercred(&channel.stream)
        .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    if credential.pid.as_raw_nonzero().get() as u32 != pid
        || credential.uid != rustix::process::geteuid()
    {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn verify(channel: &Channel, pid: u32) -> Result<(), PermissionOutcome> {
    let peer = channel.peer().ok_or(PermissionOutcome::HelperUntrusted)?;
    let own = peer::current_identity().map_err(|_| PermissionOutcome::HelperUntrusted)?;
    if peer.identity.pid != pid || !peer.identity.same_login(own) {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    Ok(())
}

fn assert_reaped(pid: i32) {
    let pid = rustix::process::Pid::from_raw(pid).unwrap();
    assert_eq!(
        rustix::process::test_kill_process(pid),
        Err(rustix::io::Errno::SRCH)
    );
}

#[test]
#[ignore = "A subprocess fixture launched by the bounded registration tests"]
fn registration_child() {
    let Some(path) = std::env::var_os("SPACEPRUNE_REGISTRATION_TEST_ENDPOINT") else {
        return;
    };
    let mode = std::env::var("SPACEPRUNE_REGISTRATION_TEST_MODE").unwrap();
    let marker = PathBuf::from(std::env::var_os("SPACEPRUNE_REGISTRATION_TEST_MARKER").unwrap());
    let startup = OperationContext::workflow(Default::default(), Interaction::NonInteractive);
    let channel = Channel::connect(path.to_str().unwrap(), &startup).unwrap();
    if mode.starts_with("refresh") {
        let code = serve_agent(
            channel,
            |_| Ok(()),
            |action, kind, challenge, _| {
                assert_eq!(action, AgentAction::Refresh);
                assert_eq!(kind, ServiceKind::Metadata);
                assert!(challenge.is_none());
                std::fs::write(&marker, std::process::id().to_string()).unwrap();
                if mode == "refresh-blocked" {
                    thread::sleep(Duration::from_secs(30));
                }
                Ok(AgentResult::Registered(RegistrationState::Enabled))
            },
            || std::process::exit(75),
        )
        .unwrap_or(77);
        std::process::exit(code);
    }
    if mode.starts_with("auth") {
        let code = serve_agent(
            channel,
            |_| Ok(()),
            |action, kind, challenge, context| {
                assert_eq!(action, AgentAction::Authorize);
                assert_eq!(kind, ServiceKind::Metadata);
                assert!(challenge.is_some());
                std::fs::write(&marker, std::process::id().to_string()).unwrap();
                let _owner = OwnerMarker(marker.with_extension("released"));
                // A file is only a subprocess-test barrier, never the production credential transport.
                while !marker.with_extension("adopted").exists() {
                    check(context)?;
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(AgentResult::Authorized)
            },
            || std::process::exit(75),
        )
        .unwrap_or(77);
        std::process::exit(code);
    }
    if mode == "silent" {
        thread::sleep(Duration::from_secs(30));
        return;
    }
    if matches!(mode.as_str(), "bad-nonce" | "wrong-kind") {
        let mut channel = channel;
        let context = OperationContext::workflow(Default::default(), Interaction::NonInteractive);
        let call: Call = channel.read(&context).unwrap();
        let mut response = call.receipt(Event::Ready);
        if mode == "bad-nonce" {
            response.nonce = uuid::Uuid::new_v4().to_string();
        } else {
            response.kind = ServiceKind::Cleanup;
        }
        channel.write(&response, &context).unwrap();
        thread::sleep(Duration::from_secs(30));
        return;
    }
    let code = serve(
        channel,
        |_| {
            if mode == "untrusted-host" {
                Err(PermissionOutcome::HelperUntrusted)
            } else {
                Ok(())
            }
        },
        |kind, context| {
            assert_eq!(kind, ServiceKind::Metadata);
            assert!(context.deadline > Instant::now());
            let mut file = std::fs::File::create(&marker).unwrap();
            writeln!(file, "{}", std::process::id()).unwrap();
            if mode == "interrupt" {
                rustix::process::kill_process(
                    rustix::process::getpid(),
                    rustix::process::Signal::INT,
                )
                .unwrap();
                thread::sleep(Duration::from_secs(30));
            }
            if mode == "native-child-blocked" {
                let mut child = Command::new("sleep").arg("60").spawn().unwrap();
                std::fs::write(marker.with_extension("native"), child.id().to_string()).unwrap();
                thread::sleep(Duration::from_secs(30));
                child.kill().unwrap();
                child.wait().unwrap();
            }
            if mode == "blocked" {
                thread::sleep(Duration::from_secs(30));
            }
            if mode == "denied" {
                Err(PermissionOutcome::Denied)
            } else if mode == "approval" {
                Ok(RegistrationState::RequiresApproval)
            } else {
                Ok(RegistrationState::Enabled)
            }
        },
        if mode == "native-child-blocked" {
            super::super::agent::stop_agent
        } else {
            || std::process::exit(75)
        },
    )
    .unwrap_or(77);
    if mode == "held-open" {
        thread::sleep(Duration::from_secs(30));
    }
    std::process::exit(if mode == "bad-exit" { 1 } else { code });
}

#[cfg(target_os = "linux")]
mod terminal_tests;

#[test]
fn real_registration_children_confirm_the_original_call_and_are_reaped() {
    let root = tempfile::tempdir().unwrap();
    for (mode, expected) in [
        ("enabled", Ok(RegistrationState::Enabled)),
        ("approval", Ok(RegistrationState::RequiresApproval)),
        ("denied", Err(PermissionOutcome::Denied)),
    ] {
        let marker = root.path().join(mode);
        let context = OperationContext::workflow(Default::default(), Interaction::Foreground)
            .child(Duration::from_secs(3));
        assert_eq!(
            launch(
                ServiceKind::Metadata,
                &context,
                |path| command(path, mode, &marker),
                verify
            ),
            expected
        );
        let pid: i32 = std::fs::read_to_string(&marker)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_reaped(pid);
    }
}

#[test]
fn wrong_peer_and_stale_receipts_cannot_register_or_keep_the_child_running() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("never-register");
    for mode in ["wrong-peer", "bad-nonce", "wrong-kind"] {
        let context = OperationContext::workflow(Default::default(), Interaction::Foreground)
            .child(Duration::from_secs(2));
        let started = Instant::now();
        assert_eq!(
            launch(
                ServiceKind::Metadata,
                &context,
                |path| command(path, mode, &marker),
                |socket, pid| if mode == "wrong-peer" {
                    Err(PermissionOutcome::HelperUntrusted)
                } else {
                    verify(socket, pid)
                }
            ),
            Err(PermissionOutcome::HelperUntrusted)
        );
        assert!(!marker.exists());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}

#[test]
fn silent_or_blocked_authorizers_stop_on_deadline_or_host_cancellation() {
    let root = tempfile::tempdir().unwrap();
    let context = OperationContext::workflow(Default::default(), Interaction::Foreground)
        .child(Duration::from_millis(150));
    assert_eq!(
        launch(
            ServiceKind::Metadata,
            &context,
            |path| command(path, "silent", &root.path().join("silent")),
            verify
        ),
        Err(PermissionOutcome::TimedOut)
    );
    let marker = root.path().join("blocked");
    let context = OperationContext::workflow(Default::default(), Interaction::Foreground)
        .child(Duration::from_secs(3));
    let cancel = context.clone();
    let wait_for_marker = marker.clone();
    let canceller = thread::spawn(move || {
        let limit = Instant::now() + Duration::from_secs(2);
        while !wait_for_marker.exists() && Instant::now() < limit {
            thread::sleep(Duration::from_millis(10));
        }
        cancel.cancel();
    });
    assert_eq!(
        launch(
            ServiceKind::Metadata,
            &context,
            |path| command(path, "blocked", &marker),
            verify
        ),
        Err(PermissionOutcome::Cancelled)
    );
    canceller.join().unwrap();
    let pid: i32 = std::fs::read_to_string(&marker)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_reaped(pid);
}

#[test]
fn the_agent_itself_exits_when_its_host_channel_disappears_during_native_authorization() {
    let root = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::bind().unwrap();
    let marker = root.path().join("started");
    let mut child = ProcessTree::spawn(&mut command(endpoint.name(), "blocked", &marker)).unwrap();
    let limit = Instant::now() + Duration::from_secs(2);
    let mut channel = loop {
        if let Some(channel) = endpoint.accept().unwrap() {
            break channel;
        }
        assert!(Instant::now() < limit);
        thread::sleep(Duration::from_millis(10));
    };
    verify(&channel, child.child_mut().id()).unwrap();
    let context = OperationContext::workflow(Default::default(), Interaction::Foreground)
        .child(Duration::from_secs(3));
    let call = Call {
        protocol: PROTOCOL,
        nonce: uuid::Uuid::new_v4().to_string(),
        request: AgentRequest::new(ServiceKind::Metadata, &context).unwrap(),
    };
    channel.write(&call, &context).unwrap();
    let ready: Receipt = channel.read(&context).unwrap();
    assert!(call.matches(&ready));
    drop(channel);
    drop(endpoint);
    let limit = Instant::now() + Duration::from_secs(1);
    while !child.has_exited().unwrap() && Instant::now() < limit {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(child.has_exited().unwrap());
    assert_eq!(child.terminate_and_wait().unwrap().code(), Some(75));
}

#[test]
fn component_discovery_is_fixed_for_both_public_cli_and_app_hosts() {
    assert_eq!(
        agent_at(Path::new(
            "/Applications/SpacePrune.app/Contents/MacOS/spaceprune-desktop"
        ))
        .unwrap(),
        Path::new(
            "/Applications/SpacePrune.app/Contents/Library/SpacePruneAuthorization.app/Contents/MacOS/spaceprune-helper-installer"
        )
    );
    assert_eq!(
        agent_at(Path::new("/opt/spaceprune/spaceprune")).unwrap(),
        Path::new(
            "/opt/spaceprune/SpacePruneAuthorization.app/Contents/MacOS/spaceprune-helper-installer"
        )
    );
}

#[test]
fn a_receipt_requires_the_matching_exit_and_cannot_hide_a_lingering_child() {
    let root = tempfile::tempdir().unwrap();
    for (mode, expected) in [
        ("bad-exit", PermissionOutcome::AuthorizationUnavailable),
        ("held-open", PermissionOutcome::TimedOut),
    ] {
        let marker = root.path().join(mode);
        let context = OperationContext::workflow(Default::default(), Interaction::Foreground)
            .child(Duration::from_millis(300));
        assert_eq!(
            launch(
                ServiceKind::Metadata,
                &context,
                |name| command(name, mode, &marker),
                verify
            ),
            Err(expected)
        );
        let pid = std::fs::read_to_string(marker)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_reaped(pid);
    }
}

#[test]
fn an_agent_with_an_untrusted_host_cannot_enter_the_native_api() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("never-register");
    let context = OperationContext::workflow(Default::default(), Interaction::Foreground)
        .child(Duration::from_secs(2));
    assert!(
        launch(
            ServiceKind::Metadata,
            &context,
            |name| command(name, "untrusted-host", &marker),
            verify
        )
        .is_err()
    );
    assert!(!marker.exists());
}

#[test]
fn the_agent_enforces_the_original_deadline_even_while_the_host_stays_connected() {
    let root = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::bind().unwrap();
    let marker = root.path().join("started");
    let mut child = ProcessTree::spawn(&mut command(endpoint.name(), "blocked", &marker)).unwrap();
    let limit = Instant::now() + Duration::from_secs(2);
    let mut channel = loop {
        if let Some(channel) = endpoint.accept().unwrap() {
            break channel;
        }
        assert!(Instant::now() < limit);
        thread::sleep(Duration::from_millis(10));
    };
    verify(&channel, child.child_mut().id()).unwrap();
    let context = OperationContext::workflow(Default::default(), Interaction::Foreground)
        .child(Duration::from_millis(200));
    let call = Call {
        protocol: PROTOCOL,
        nonce: uuid::Uuid::new_v4().to_string(),
        request: AgentRequest::new(ServiceKind::Metadata, &context).unwrap(),
    };
    channel.write(&call, &context).unwrap();
    let ready: Receipt = channel.read(&context).unwrap();
    assert!(call.matches(&ready));
    while !child.has_exited().unwrap() && Instant::now() < limit {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(child.has_exited().unwrap());
    assert_eq!(child.terminate_and_wait().unwrap().code(), Some(75));
    assert!(marker.exists());
}

struct OwnerMarker(PathBuf);
impl Drop for OwnerMarker {
    fn drop(&mut self) {
        std::fs::write(&self.0, b"released").unwrap();
    }
}

#[test]
fn the_host_gets_completion_only_after_direct_service_adoption() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("authorized");
    let operation = OperationContext::workflow(Default::default(), Interaction::Foreground)
        .child(Duration::from_secs(3));
    let target = marker.clone();
    let waiter = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !target.exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!target.with_extension("released").exists());
        thread::sleep(Duration::from_millis(30));
        assert!(!target.with_extension("released").exists());
        std::fs::write(target.with_extension("adopted"), b"accepted").unwrap();
    });
    let nonce = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        launch_agent(
            ServiceKind::Metadata,
            AgentAction::Authorize,
            Some(&nonce),
            &operation,
            |name| command(name, "auth", &marker),
            verify
        ),
        Ok(Completion::Authorized)
    );
    waiter.join().unwrap();
    assert!(marker.with_extension("released").exists());
    assert_reaped(std::fs::read_to_string(marker).unwrap().parse().unwrap());
}

#[test]
fn an_authorizer_without_a_service_challenge_cannot_enter_the_native_api() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("never-start");
    let context = OperationContext::default().child(Duration::from_secs(2));
    assert!(
        launch_agent(
            ServiceKind::Metadata,
            AgentAction::Authorize,
            None,
            &context,
            |name| command(name, "auth", &marker),
            verify
        )
        .is_err()
    );
    assert!(!marker.exists());
}

#[test]
fn credential_frames_are_not_part_of_the_host_agent_protocol() {
    let context = OperationContext::default();
    let call = Call {
        protocol: PROTOCOL,
        nonce: uuid::Uuid::new_v4().to_string(),
        request: AgentRequest::new(ServiceKind::Metadata, &context).unwrap(),
    };
    let mut receipt = serde_json::to_value(call.receipt(Event::Ready)).unwrap();
    receipt["event"] = serde_json::json!("authorization");
    receipt["credential"] = serde_json::json!([7; 32].to_vec());
    assert!(serde_json::from_value::<Receipt>(receipt).is_err());
}

#[test]
fn refresh_receipts_wait_for_exit_and_blocked_updates_are_reaped() {
    let root = tempfile::tempdir().unwrap();
    for mode in ["refresh", "refresh-blocked"] {
        let marker = root.path().join(mode);
        let context = OperationContext::workflow(Default::default(), Interaction::Foreground)
            .child(Duration::from_millis(350));
        let result = launch_agent(
            ServiceKind::Metadata,
            AgentAction::Refresh,
            None,
            &context,
            |name| command(name, mode, &marker),
            verify,
        );
        assert_eq!(result.is_ok(), mode == "refresh");
        assert_reaped(std::fs::read_to_string(&marker).unwrap().parse().unwrap());
    }
}
