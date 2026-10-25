use super::*;

fn wait(mailbox: &Mailbox) -> Result<u16, HelperError> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(code) = mailbox.poll()? {
            return Ok(code);
        }
        if Instant::now() >= deadline {
            return Err(HelperError);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn one_directory_session_serves_multiple_scoped_requests_and_rejects_replay() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("cache");
    fs::create_dir(&data).unwrap();
    fs::write(data.join("keep"), b"preserved").unwrap();
    let mut mailbox = Mailbox::create(&OperationContext::default()).unwrap();
    let arguments = mailbox.arguments();
    let bootstrap = PathBuf::from(&arguments[1]);
    let nonce = arguments[2].to_str().unwrap().to_owned();
    let executed = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&executed);
    let worker = std::thread::spawn(move || {
        serve(
            &bootstrap,
            &nonce,
            Duration::from_secs(3),
            |path, nonce, _, _| {
                count.fetch_add(1, Ordering::SeqCst);
                worker::run_helper(path, nonce)
            },
        )
    });
    assert_eq!(wait(&mailbox).unwrap(), 0);
    for _ in 0..2 {
        let request =
            PreparedRequest::create("recipe-source-0", std::slice::from_ref(&data)).unwrap();
        mailbox.send(&request).unwrap();
        assert_eq!(wait(&mailbox).unwrap(), 0);
        assert!(matches!(
            request.receive_response(),
            Ok(ResponseOutcome::Measured {
                logical_bytes: 9,
                ..
            })
        ));
    }
    assert_eq!(executed.load(Ordering::SeqCst), 2);
    let request = PreparedRequest::create("recipe-source-0", std::slice::from_ref(&data)).unwrap();
    publish(
        &mailbox.directory.path().join("command.json"),
        &CommandFrame {
            nonce: mailbox.nonce.clone(),
            sequence: 2,
            request_path: NativePath::from_path(&request.request_path),
            request_nonce: request.nonce.clone(),
        },
        true,
    )
    .unwrap();
    assert!(worker.join().unwrap().is_err());
    assert_eq!(executed.load(Ordering::SeqCst), 2);
    assert_eq!(fs::read(data.join("keep")).unwrap(), b"preserved");
}

#[test]
fn bootstrap_preserves_deadlines() {
    let mailbox = Mailbox::create(&OperationContext::default()).unwrap();
    let bootstrap = mailbox.directory.path().join("bootstrap.json");
    let mut frame: Bootstrap = read(&bootstrap, true).unwrap().unwrap();
    frame.context.deadline_monotonic_ns = 0;
    publish(&bootstrap, &frame, true).unwrap();
    assert!(
        serve(
            &bootstrap,
            &mailbox.nonce,
            Duration::from_millis(10),
            |_, _, _, _| panic!("Expired bootstrap cannot execute")
        )
        .is_err()
    );
    assert!(mailbox.poll().unwrap().is_none());
    assert!(bootstrap.exists());
}

#[test]
fn connection_revocation_exits_the_worker_and_forged_replies_cannot_grant_access() {
    let mailbox = Mailbox::create(&OperationContext::default()).unwrap();
    publish(
        &mailbox.directory.path().join("reply.json"),
        &Reply {
            version: VERSION,
            nonce: "x".repeat(43),
            sequence: 0,
            code: 0,
        },
        false,
    )
    .unwrap();
    assert!(mailbox.poll().is_err());
    fs::remove_file(mailbox.directory.path().join("reply.json")).unwrap();
    let bootstrap = mailbox.directory.path().join("bootstrap.json");
    let nonce = mailbox.nonce.clone();
    let worker = std::thread::spawn(move || {
        serve(&bootstrap, &nonce, Duration::from_secs(3), |_, _, _, _| {
            panic!("Revoked connection cannot execute")
        })
    });
    assert_eq!(wait(&mailbox).unwrap(), 0);
    assert!(
        serve(
            &mailbox.directory.path().join("bootstrap.json"),
            &mailbox.nonce,
            Duration::from_secs(3),
            |_, _, _, _| panic!("Consumed bootstrap cannot execute")
        )
        .is_err()
    );
    mailbox.close().unwrap();
    assert!(worker.join().unwrap().is_ok());
}

#[test]
fn complete_scans_stream_large_filtered_results_and_reuse_the_connection() {
    use std::sync::atomic::AtomicBool;

    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("cache");
    fs::create_dir(&data).unwrap();
    for index in 0..6_000 {
        fs::write(
            data.join(format!("{index:04}-{}.log", "sample".repeat(20))),
            b"content",
        )
        .unwrap();
    }
    fs::write(data.join("keep.log"), b"retained").unwrap();
    fs::write(data.join("secret.key"), b"not returned").unwrap();
    let operation = OperationContext::default().child(Duration::from_secs(20));
    let mut mailbox = Mailbox::create(&operation).unwrap();
    let bootstrap = mailbox.directory.path().join("bootstrap.json");
    let nonce = mailbox.nonce.clone();
    let worker = std::thread::spawn(move || {
        serve(
            &bootstrap,
            &nonce,
            Duration::from_secs(5),
            |path, nonce, _, alive| {
                worker::run_helper_for_owner(path, nonce, None, Some(&|| !alive()))
            },
        )
    });
    assert_eq!(wait(&mailbox).unwrap(), 0);
    let task = spaceprune_plugin_api::TaskRequest {
        module_id: "logs".into(),
        task_id: "files".into(),
        name: "Logs".into(),
        handler_id: "builtin.filesystem/paths".into(),
        options: serde_json::json!({"roots": [{"path": data, "include": ["*.log"], "exclude": ["keep.log"]}], "files": true, "directories": false}),
        recommend: serde_json::json!({}),
        tags: vec![],
    };
    for _ in 0..2 {
        let request = PreparedRequest::create_filesystem_scan("recipe-source-0", &task)
            .unwrap()
            .with_operation(&operation)
            .unwrap();
        mailbox.send(&request).unwrap();
        // Drain the TCP payload while the worker is producing it, before waiting for its receipt.
        let (result, cancelled) = request.receive_scan(&AtomicBool::new(false)).unwrap();
        assert_eq!(wait(&mailbox).unwrap(), 0);
        assert!(!cancelled);
        assert_eq!(result.items.len(), 6_000);
        assert!(result.items.iter().all(|item| item.location.ends_with(".log") && !item.location.ends_with("keep.log")));
        assert!(serde_json::to_vec(&result).unwrap().len() > 4 * 1024 * 1024);
    }
    mailbox.close().unwrap();
    assert!(worker.join().unwrap().is_ok());
    assert_eq!(fs::read_dir(&data).unwrap().count(), 6_002);
    assert_eq!(fs::read(data.join("keep.log")).unwrap(), b"retained");
}

#[test]
fn revoking_the_connection_stops_an_in_flight_request() {
    let root = tempfile::tempdir().unwrap();
    let mut mailbox = Mailbox::create(&OperationContext::default()).unwrap();
    let bootstrap = mailbox.directory.path().join("bootstrap.json");
    let nonce = mailbox.nonce.clone();
    let (started, ready) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        serve(
            &bootstrap,
            &nonce,
            Duration::from_secs(3),
            |_, _, _, alive| {
                started.send(()).unwrap();
                let deadline = Instant::now() + Duration::from_secs(3);
                while alive() {
                    assert!(
                        Instant::now() < deadline,
                        "Revocation must reach the active request"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(HelperError)
            },
        )
    });
    assert_eq!(wait(&mailbox).unwrap(), 0);
    let request = PreparedRequest::create("recipe-source-0", &[root.path().to_owned()]).unwrap();
    mailbox.send(&request).unwrap();
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    mailbox.close().unwrap();
    assert!(worker.join().unwrap().is_ok());
}

#[test]
fn withdrawing_only_the_lease_stops_work_even_when_the_directory_remains() {
    let root = tempfile::tempdir().unwrap();
    let mut mailbox = Mailbox::create(&OperationContext::default()).unwrap();
    let bootstrap = mailbox.directory.path().join("bootstrap.json");
    let nonce = mailbox.nonce.clone();
    let (started, ready) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        serve(
            &bootstrap,
            &nonce,
            Duration::from_secs(3),
            |_, _, _, alive| {
                started.send(()).unwrap();
                let deadline = Instant::now() + Duration::from_secs(3);
                while alive() {
                    assert!(
                        Instant::now() < deadline,
                        "Lease removal must reach active work"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(HelperError)
            },
        )
    });
    assert_eq!(wait(&mailbox).unwrap(), 0);
    let request = PreparedRequest::create("recipe-source-0", &[root.path().to_owned()]).unwrap();
    mailbox.send(&request).unwrap();
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    fs::remove_file(mailbox.directory.path().join(LEASE_FILE)).unwrap();
    assert!(worker.join().unwrap().is_ok());
    assert!(mailbox.directory.path().is_dir());
    assert!(mailbox.poll().unwrap().is_none());
    mailbox.close().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn failed_revocation_is_reported_instead_of_claiming_the_lease_was_withdrawn() {
    use std::os::unix::fs::PermissionsExt;
    if rustix::process::geteuid().is_root() {
        return;
    }
    let mailbox = Mailbox::create(&OperationContext::default()).unwrap();
    let path = mailbox.directory.path();
    fs::set_permissions(path, fs::Permissions::from_mode(0o500)).unwrap();
    let result = mailbox.close();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert!(path.join(LEASE_FILE).is_file());
    mailbox.close().unwrap();
}

#[test]
fn legacy_windows_bootstrap_is_rejected_without_changing_the_request_payload_schema() {
    let mailbox = Mailbox::create(&OperationContext::default()).unwrap();
    let path = mailbox.directory.path().join("bootstrap.json");
    let mut frame: Bootstrap = read(&path, true).unwrap().unwrap();
    assert_eq!(frame.version, 7);
    assert_eq!(REQUEST_SCHEMA_VERSION, 6);
    frame.version = 6;
    publish(&path, &frame, true).unwrap();
    assert!(
        serve(
            &path,
            &mailbox.nonce,
            Duration::from_millis(10),
            |_, _, _, _| { panic!("Legacy account-bound bootstrap cannot execute") }
        )
        .is_err()
    );
    assert!(mailbox.poll().unwrap().is_none());
}
