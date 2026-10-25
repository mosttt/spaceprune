use super::*;
use spaceprune_plugin_api::NullProgress;
use std::{
    io::{BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
};

fn group(root: &Path, id: &str) -> PlanGroup {
    std::fs::create_dir(root).unwrap();
    std::fs::write(root.join("selected"), b"fixture").unwrap();
    std::fs::write(root.join("unselected"), b"keep").unwrap();
    let task: TaskRequest=serde_json::from_value(serde_json::json!({"module_id":"fixture","task_id":id,"name":id,"handler_id":"builtin.filesystem/paths","options":{"roots":[{"path":root}],"elevation":"pkexec"},"recommend":{},"tags":[]})).unwrap();
    let scan = spaceprune_plugin_filesystem::scan_readonly(
        &task,
        &NullProgress,
        &OperationContext::default(),
    )
    .unwrap();
    PlanGroup {
        module_id: task.module_id.clone(),
        task_id: task.task_id.clone(),
        handler_id: task.handler_id.clone(),
        request: task,
        items: scan
            .items
            .into_iter()
            .filter(|item| item.name == "selected")
            .collect(),
    }
}
fn send(stream: &mut UnixStream, value: &impl Serialize) {
    serde_json::to_writer(&mut *stream, value).unwrap();
    stream.write_all(b"\n").unwrap();
}
fn send_execution(stream: &mut UnixStream, grant: &Grant, selection: Vec<Selection>) {
    send(
        stream,
        &ExecuteRequest {
            operation: grant.operation.clone(),
            selection,
        },
    );
}
fn read(reader: &mut BufReader<UnixStream>) -> Reply {
    serde_json::from_slice::<Response>(&frame(reader).unwrap().unwrap())
        .unwrap()
        .reply
}

#[test]
fn custom_snapshot_programs_are_rejected_before_any_confirmed_batch_runs() {
    let temp = tempfile::tempdir().unwrap();
    let first = group(&temp.path().join("files"), "files");
    let mut snapshot = first.clone();
    snapshot.handler_id = "builtin.snapper/snapshots".into();
    snapshot.request.handler_id = snapshot.handler_id.clone();
    snapshot.request.options = serde_json::json!({"scan-mode":"snapshots", "configs":["root"], "snapper":temp.path().join("fake-snapper")});
    let grant = Grant {
        protocol_version: CLEANUP_PROTOCOL_VERSION,
        operation: OperationContext::default().transfer().unwrap(),
        plan_digest: "e".repeat(64),
        groups: vec![first, snapshot],
    };
    let (mut client, server) = UnixStream::pair().unwrap();
    let worker = std::thread::spawn(move || {
        serve(
            BufReader::new(server.try_clone().unwrap()),
            Box::new(server),
        )
    });
    send(&mut client, &grant);
    let failure = worker.join().unwrap().unwrap_err();
    client.shutdown(std::net::Shutdown::Both).unwrap();
    assert!(failure.to_string().contains("Custom programs"));
    assert!(temp.path().join("files/selected").exists());
    assert!(temp.path().join("files/unselected").exists());
}

#[test]
fn expired_or_different_operation_batches_cannot_delete_confirmed_files() {
    for change_identity in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let grant = Grant {
            protocol_version: CLEANUP_PROTOCOL_VERSION,
            operation: OperationContext::default().transfer().unwrap(),
            plan_digest: "a".repeat(64),
            groups: vec![group(&temp.path().join("data"), "data")],
        };
        let (mut client, server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let worker = std::thread::spawn(move || {
            serve(
                BufReader::new(server.try_clone().unwrap()),
                Box::new(server),
            )
        });
        let mut output = BufReader::new(client.try_clone().unwrap());
        send(&mut client, &grant);
        let ready: Response =
            serde_json::from_slice(&frame(&mut output).unwrap().unwrap()).unwrap();
        assert_eq!(ready.operation, grant.operation.identity);
        assert!(matches!(ready.reply, Reply::Ready));
        let mut operation = grant.operation.clone();
        if change_identity {
            operation.identity.generation += 1;
        } else {
            operation.deadline_monotonic_ns = 0;
        }
        send(
            &mut client,
            &ExecuteRequest {
                operation,
                selection: vec![Selection {
                    group: 0,
                    items: vec![0],
                }],
            },
        );
        assert!(worker.join().unwrap().is_err());
        client.shutdown(std::net::Shutdown::Both).unwrap();
        assert!(temp.path().join("data/selected").exists());
        assert!(temp.path().join("data/unselected").exists());
    }
}

#[test]
fn one_worker_serves_multiple_confirmed_batches_without_accepting_replay_or_unselected_files() {
    let temp = tempfile::tempdir().unwrap();
    let grant = Grant {
        protocol_version: CLEANUP_PROTOCOL_VERSION,
        operation: OperationContext::default().transfer().unwrap(),
        plan_digest: "a".repeat(64),
        groups: vec![
            group(&temp.path().join("one"), "one"),
            group(&temp.path().join("two"), "two"),
        ],
    };
    let (mut client, server) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let worker = std::thread::spawn(move || {
        serve(
            BufReader::new(server.try_clone().unwrap()),
            Box::new(server),
        )
    });
    let mut output = BufReader::new(client.try_clone().unwrap());
    send(&mut client, &grant);
    assert!(matches!(read(&mut output), Reply::Ready));
    for index in 0..2 {
        assert_eq!(grant.groups[index].items.len(), 1);
        send_execution(
            &mut client,
            &grant,
            vec![Selection {
                group: index,
                items: vec![0],
            }],
        );
        loop {
            match read(&mut output) {
                Reply::ItemFinished { .. } => send(&mut client, &OutcomeReceipt { accepted: true }),
                Reply::Results { results } => {
                    assert_eq!(results[0].as_ref().unwrap().succeeded, 1);
                    break;
                }
                _ => {}
            }
        }
    }
    for name in ["one", "two"] {
        assert!(!temp.path().join(name).join("selected").exists());
        assert!(temp.path().join(name).join("unselected").exists());
    }
    send_execution(
        &mut client,
        &grant,
        vec![Selection {
            group: 0,
            items: vec![0],
        }],
    );
    assert!(worker.join().unwrap().is_err());
    client.shutdown(std::net::Shutdown::Both).unwrap();
}

#[test]
fn a_failed_host_receipt_stops_the_next_confirmed_item() {
    let temp = tempfile::tempdir().unwrap();
    let grant = Grant {
        protocol_version: CLEANUP_PROTOCOL_VERSION,
        operation: OperationContext::default().transfer().unwrap(),
        plan_digest: "d".repeat(64),
        groups: vec![
            group(&temp.path().join("one"), "one"),
            group(&temp.path().join("two"), "two"),
        ],
    };
    let (mut client, server) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let worker = std::thread::spawn(move || {
        serve(
            BufReader::new(server.try_clone().unwrap()),
            Box::new(server),
        )
    });
    let mut output = BufReader::new(client.try_clone().unwrap());
    send(&mut client, &grant);
    assert!(matches!(read(&mut output), Reply::Ready));
    send_execution(
        &mut client,
        &grant,
        vec![
            Selection {
                group: 0,
                items: vec![0],
            },
            Selection {
                group: 1,
                items: vec![0],
            },
        ],
    );
    loop {
        if let Reply::ItemFinished { outcome, .. } = read(&mut output) {
            assert_eq!(
                outcome.status,
                spaceprune_plugin_api::ItemExecutionStatus::Succeeded
            );
            assert!(!temp.path().join("one/selected").exists());
            assert!(temp.path().join("two/selected").exists());
            send(&mut client, &OutcomeReceipt { accepted: false });
            break;
        }
    }
    loop {
        if let Reply::Results { results } = read(&mut output) {
            assert_eq!(results[0].as_ref().unwrap().succeeded, 1);
            assert_eq!(
                results[1].as_ref().unwrap().outcomes[0].status,
                spaceprune_plugin_api::ItemExecutionStatus::NotAttempted
            );
            break;
        }
    }
    worker.join().unwrap().unwrap();
    client.shutdown(std::net::Shutdown::Both).unwrap();
    assert!(temp.path().join("two/selected").exists());
    assert!(temp.path().join("one/unselected").exists());
}

#[test]
fn incompatible_or_closed_session_performs_no_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let grant = Grant {
        protocol_version: CLEANUP_PROTOCOL_VERSION,
        operation: OperationContext::default().transfer().unwrap(),
        plan_digest: "b".repeat(64),
        groups: vec![group(&temp.path().join("closed"), "closed")],
    };
    let mut incompatible = grant.clone();
    incompatible.protocol_version += 1;
    let mut bytes = serde_json::to_vec(&incompatible).unwrap();
    bytes.push(b'\n');
    assert!(serve(std::io::Cursor::new(bytes), Box::new(io::sink())).is_err());
    assert!(temp.path().join("closed/selected").exists());
    let (mut client, server) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let worker = std::thread::spawn(move || {
        serve(
            BufReader::new(server.try_clone().unwrap()),
            Box::new(server),
        )
    });
    let mut output = BufReader::new(client.try_clone().unwrap());
    send(&mut client, &grant);
    assert!(matches!(read(&mut output), Reply::Ready));
    client.shutdown(std::net::Shutdown::Both).unwrap();
    worker.join().unwrap().unwrap();
    assert!(temp.path().join("closed/selected").exists());
}

#[test]
fn modified_task_or_item_is_not_a_confirmed_selection() {
    let temp = tempfile::tempdir().unwrap();
    let grant = Grant {
        protocol_version: CLEANUP_PROTOCOL_VERSION,
        operation: OperationContext::default().transfer().unwrap(),
        plan_digest: "c".repeat(64),
        groups: vec![group(&temp.path().join("selection"), "selection")],
    };
    let original = &grant.groups[0];
    let mut task = original.request.clone();
    task.options["roots"][0]["path"] = serde_json::json!("/different");
    assert!(
        select(
            &grant,
            &[CleanupBatchGroup {
                task: &task,
                items: &original.items
            }]
        )
        .is_err()
    );
    let mut changed = original.items.clone();
    changed[0].opaque = serde_json::json!({});
    assert!(
        select(
            &grant,
            &[CleanupBatchGroup {
                task: &original.request,
                items: &changed
            }]
        )
        .is_err()
    );
    assert!(temp.path().join("selection/selected").exists());
}

#[test]
fn bootstrap_binding_cannot_be_replaced_or_renewed_by_a_later_grant() {
    for violation in 0..3 {
        let temp = tempfile::tempdir().unwrap();
        let original = OperationContext::default();
        let grant = Grant {
            protocol_version: CLEANUP_PROTOCOL_VERSION,
            operation: original.transfer().unwrap(),
            plan_digest: "c".repeat(64),
            groups: vec![group(&temp.path().join("data"), "data")],
        };
        let mut binding = original.clone();
        match violation {
            0 => binding.generation += 1,
            1 => binding.deadline = Instant::now(),
            _ => binding.cancel(),
        }
        let mut init = serde_json::to_vec(&grant).unwrap();
        init.push(b'\n');
        assert!(
            serve_bound(
                std::io::Cursor::new(init),
                Box::new(io::sink()),
                Some(&binding)
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read(temp.path().join("data/selected")).unwrap(),
            b"fixture"
        );
        assert_eq!(
            std::fs::read(temp.path().join("data/unselected")).unwrap(),
            b"keep"
        );
    }
}

#[test]
fn docker_in_a_mixed_plan_is_never_bootstrapped_into_native_helpers() {
    let temporary = tempfile::tempdir().unwrap();
    let files = group(&temporary.path().join("files"), "files");
    let mut docker = files.clone();
    docker.task_id = "docker".into();
    docker.handler_id = "builtin.developer-tools/docker-build-cache".into();
    docker.request.task_id = docker.task_id.clone();
    docker.request.handler_id = docker.handler_id.clone();
    docker.request.options = serde_json::json!({});
    let tasks = [&files, &docker]
        .into_iter()
        .map(|group| {
            serde_json::json!({
                "module_id":group.module_id,"task_id":group.task_id,"name":group.task_id,"handler_id":group.handler_id,
                "request":group.request,"items":group.items,"issues":[],"error":null,
            })
        })
        .collect::<Vec<_>>();
    let report = spaceprune_engine::ScanReport::from_report(
        serde_json::from_value(serde_json::json!({
            "scan_id":"fixture","config_name":"mixed","config_source":"fixture","cancelled":false,
            "modules":[{"id":"fixture","name":"fixture","description":"","tasks":tasks}],
        }))
        .unwrap(),
    );
    let plan = report.plan_selected(&report.selectable_keys()).unwrap();
    assert_eq!(plan.groups().len(), 2);
    let access = NativeCleanupAccess::new(&plan, &OperationContext::default());
    let grant = access.grant.unwrap();
    assert_eq!(grant.groups.len(), 1);
    assert_eq!(grant.groups[0].handler_id, "builtin.filesystem/paths");
    assert!(!grant.requires_developer_handlers());
    let scan = crate::native_access::NativeScanAccess::new();
    assert!(!spaceprune_plugin_api::ScanAccess::manages_authorization(
        &scan,
        &docker.request
    ));
}
