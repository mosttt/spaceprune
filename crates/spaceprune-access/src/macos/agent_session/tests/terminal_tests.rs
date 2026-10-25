use super::*;

#[test]
fn an_interrupted_agent_reports_cancelled_instead_of_an_authorization_outage() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("interrupted");
    let operation = OperationContext::workflow(Default::default(), Interaction::Foreground)
        .child(Duration::from_secs(2));
    assert_eq!(
        launch(
            ServiceKind::Metadata,
            &operation,
            |endpoint| command(endpoint, "interrupt", &marker),
            verify
        ),
        Err(PermissionOutcome::Cancelled)
    );
    assert!(operation.is_stopped());
    assert_reaped(
        std::fs::read_to_string(marker)
            .unwrap()
            .trim()
            .parse()
            .unwrap(),
    );
}

#[test]
fn losing_the_host_stops_the_native_authenticator_even_during_a_blocked_import() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("agent");
    let endpoint = Endpoint::bind().unwrap();
    let mut child = ProcessTree::spawn(&mut command(
        endpoint.name(),
        "native-child-blocked",
        &marker,
    ))
    .unwrap();
    let operation = OperationContext::workflow(Default::default(), Interaction::Foreground)
        .child(Duration::from_secs(3));
    let mut channel = loop {
        check(&operation).unwrap();
        if let Some(channel) = endpoint.accept().unwrap() {
            break channel;
        }
        thread::sleep(Duration::from_millis(10));
    };
    verify(&channel, child.child_mut().id()).unwrap();
    let call = Call {
        protocol: PROTOCOL,
        nonce: uuid::Uuid::new_v4().to_string(),
        request: AgentRequest::new(ServiceKind::Metadata, &operation).unwrap(),
    };
    channel.write(&call, &operation).unwrap();
    let ready: Receipt = channel.read(&operation).unwrap();
    assert!(call.matches(&ready));
    let native_marker = marker.with_extension("native");
    while !native_marker.exists() {
        check(&operation).unwrap();
        thread::sleep(Duration::from_millis(10));
    }
    let native: i32 = std::fs::read_to_string(native_marker)
        .unwrap()
        .parse()
        .unwrap();
    drop(channel);
    while !child.has_exited().unwrap() {
        check(&operation).unwrap();
        thread::sleep(Duration::from_millis(10));
    }
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(
        child.terminate_and_wait().unwrap().signal(),
        Some(libc::SIGKILL)
    );
    assert_reaped(
        std::fs::read_to_string(marker)
            .unwrap()
            .trim()
            .parse()
            .unwrap(),
    );
    // The grandchild may await init reaping, but cannot still read input or perform work.
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{native}/stat")) {
        let state = stat.rsplit_once(") ").unwrap().1.as_bytes()[0];
        assert!(
            matches!(state, b'Z' | b'X'),
            "Native authenticator is still running"
        );
    }
}
