use super::*;
use spaceprune_plugin_api::NullProgress;
use std::{path::Path, sync::mpsc};

fn fixture(root: &Path, operation: &OperationContext) -> Grant {
    std::fs::write(root.join("first"), b"1234").unwrap();
    std::fs::write(root.join("second"), b"123456").unwrap();
    std::fs::write(root.join("unselected"), b"keep").unwrap();
    let request: TaskRequest = serde_json::from_value(serde_json::json!({
        "module_id":"fixture", "task_id":"files", "name":"files", "handler_id":"builtin.filesystem/paths",
        "options":{"roots":[{"path":root}],"elevation":"native"}, "recommend":{},"tags":[]
    })).unwrap();
    let mut items = spaceprune_plugin_filesystem::scan_readonly(&request, &NullProgress, operation)
        .unwrap()
        .items;
    items.retain(|item| matches!(item.name.as_str(), "first" | "second"));
    items.sort_by(|left, right| left.name.cmp(&right.name));
    assert_eq!(items.len(), 2);
    items[1].reclaimable_bytes = None;
    let request = spaceprune_plugin_filesystem::prepare_authorized_task(&request).unwrap();
    Grant {
        protocol_version: CLEANUP_PROTOCOL_VERSION,
        operation: operation.transfer().unwrap(),
        plan_digest: "a".repeat(64),
        groups: vec![PlanGroup {
            module_id: request.module_id.clone(),
            task_id: request.task_id.clone(),
            handler_id: request.handler_id.clone(),
            request,
            items,
        }],
    }
}
fn item(operation: &OperationContext, index: usize) -> Request {
    Request::Execute {
        request: ExecuteRequest {
            operation: operation.transfer().unwrap(),
            selection: vec![Selection {
                group: 0,
                items: vec![index],
            }],
        },
    }
}
fn ack(index: usize, accepted: bool) -> Request {
    Request::Acknowledge {
        item: ItemKey {
            group: 0,
            item: index,
        },
        accepted,
    }
}

#[test]
fn native_plan_cannot_advance_without_the_matching_receipt_or_replay_a_selection() {
    let root = tempfile::tempdir().unwrap();
    let operation = OperationContext::default();
    let grant = fixture(root.path(), &operation);
    let mut state = server::State::default();
    let events = Mutex::new(Vec::new());
    let sink = |reply| {
        events.lock().unwrap().push(reply);
        Ok(())
    };
    assert!(matches!(
        state
            .request(
                Request::Prepare {
                    grant: grant.clone()
                },
                &operation,
                &sink
            )
            .unwrap(),
        NativeReply::Ready
    ));
    assert!(matches!(
        state
            .request(item(&operation, 0), &operation, &sink)
            .unwrap(),
        NativeReply::Item { .. }
    ));
    assert!(!root.path().join("first").exists());
    assert!(root.path().join("second").exists());
    assert!(
        state
            .request(item(&operation, 1), &operation, &sink)
            .is_err()
    );
    assert!(state.request(ack(1, true), &operation, &sink).is_err());
    assert!(
        state
            .request(Request::Prepare { grant }, &operation, &sink)
            .is_err()
    );
    state.request(ack(0, true), &operation, &sink).unwrap();
    std::fs::write(root.path().join("first"), b"replacement").unwrap();
    assert!(
        state
            .request(item(&operation, 0), &operation, &sink)
            .is_err()
    );
    state
        .request(item(&operation, 1), &operation, &sink)
        .unwrap();
    state.request(ack(1, true), &operation, &sink).unwrap();
    assert_eq!(
        std::fs::read(root.path().join("first")).unwrap(),
        b"replacement"
    );
    assert!(!root.path().join("second").exists());
    assert_eq!(
        std::fs::read(root.path().join("unselected")).unwrap(),
        b"keep"
    );
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, Reply::ItemFinished { .. }))
            .count(),
        2
    );
}

#[test]
fn foreign_expired_or_multi_item_native_requests_do_not_delete_files() {
    for mode in ["foreign", "expired", "batch", "outside", "non-filesystem"] {
        let root = tempfile::tempdir().unwrap();
        let operation = OperationContext::default();
        let mut grant = fixture(root.path(), &operation);
        let mut state = server::State::default();
        if mode == "non-filesystem" {
            grant.groups[0].handler_id = "builtin.snapper/snapshots".into();
            assert!(
                state
                    .request(Request::Prepare { grant }, &operation, &|_| Ok(()))
                    .is_err()
            );
        } else {
            state
                .request(Request::Prepare { grant }, &operation, &|_| Ok(()))
                .unwrap();
            let Request::Execute { mut request } = item(&operation, 0) else {
                unreachable!()
            };
            match mode {
                "foreign" => request.operation.identity.generation += 1,
                "expired" => request.operation.deadline_monotonic_ns = 0,
                "batch" => request.selection[0].items.push(1),
                _ => request.selection[0].items = vec![99],
            }
            assert!(
                state
                    .request(Request::Execute { request }, &operation, &|_| Ok(()))
                    .is_err()
            );
        }
        assert!(root.path().join("first").exists());
        assert!(root.path().join("second").exists());
        assert!(root.path().join("unselected").exists());
    }
}

struct LocalBackend(server::State);
impl Backend for LocalBackend {
    fn request(
        &mut self,
        request: Request,
        context: &OperationContext,
        events: &mut dyn FnMut(Reply) -> PluginResult<()>,
    ) -> PluginResult<NativeReply> {
        // The signed native IPC boundary is substituted here; scope and real file operations are not.
        let frames = Mutex::new(Vec::new());
        let result = self.0.request(request, context, &|reply| {
            frames.lock().unwrap().push(reply);
            Ok(())
        });
        for frame in frames.into_inner().unwrap() {
            events(frame)?;
        }
        result
    }
}

#[test]
fn batch_adapter_waits_for_durable_receipts_and_preserves_cumulative_known_and_unknown_sizes() {
    for accept in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let operation = OperationContext::default().child(Duration::from_secs(5));
        let grant = fixture(root.path(), &operation);
        let mut backend = LocalBackend(server::State::default());
        backend
            .request(
                Request::Prepare {
                    grant: grant.clone(),
                },
                &operation,
                &mut |_| Ok(()),
            )
            .unwrap();
        let (input, requests) = mpsc::sync_channel(1);
        let (responses, output) = mpsc::sync_channel(8);
        let lifetime = operation.clone();
        let worker = std::thread::spawn(move || {
            driver::run(&mut backend, &grant, &requests, &responses, &lifetime)
        });
        input
            .send(
                serde_json::to_vec(&ExecuteRequest {
                    operation: operation.transfer().unwrap(),
                    selection: vec![Selection {
                        group: 0,
                        items: vec![0, 1],
                    }],
                })
                .unwrap(),
            )
            .unwrap();
        let mut receipts = 0;
        let mut counts = Vec::new();
        loop {
            let response = output.recv_timeout(Duration::from_secs(2));
            let Ok(Ok(response)) = response else {
                assert!(!accept);
                break;
            };
            assert_eq!(response.operation, operation.identity());
            match response.reply {
                Reply::ItemFinished { outcome, .. } => {
                    receipts += 1;
                    if receipts == 1 {
                        assert!(!root.path().join("first").exists());
                        assert!(root.path().join("second").exists());
                    }
                    assert!(matches!(
                        outcome.status,
                        spaceprune_plugin_api::ItemExecutionStatus::Succeeded
                    ));
                    input
                        .send(serde_json::to_vec(&OutcomeReceipt { accepted: accept }).unwrap())
                        .unwrap();
                }
                Reply::Count {
                    succeeded,
                    bytes,
                    unknown,
                    ..
                } => counts.push((succeeded, bytes, unknown)),
                Reply::Results { results } => {
                    assert!(accept);
                    let result = results.into_iter().next().unwrap().unwrap();
                    assert_eq!(
                        (
                            result.attempted,
                            result.succeeded,
                            result.reclaimed_bytes,
                            result.unknown_reclaimed_items()
                        ),
                        (2, 2, 10, 1)
                    );
                    assert_eq!(counts, [(1, 4, 0), (2, 10, 1)]);
                    break;
                }
                _ => {}
            }
        }
        drop(input);
        assert!(worker.join().unwrap().is_err());
        assert_eq!(receipts, if accept { 2 } else { 1 });
        assert_eq!(root.path().join("second").exists(), !accept);
        assert!(root.path().join("unselected").exists());
    }
}

#[test]
fn losing_the_host_after_an_item_outcome_preserves_the_next_item() {
    let root = tempfile::tempdir().unwrap();
    let operation = OperationContext::default().child(Duration::from_secs(3));
    let grant = fixture(root.path(), &operation);
    let mut backend = LocalBackend(server::State::default());
    backend
        .request(
            Request::Prepare {
                grant: grant.clone(),
            },
            &operation,
            &mut |_| Ok(()),
        )
        .unwrap();
    let (input, requests) = mpsc::sync_channel(1);
    let (responses, output) = mpsc::sync_channel(8);
    let lifetime = operation.clone();
    let worker = std::thread::spawn(move || {
        driver::run(&mut backend, &grant, &requests, &responses, &lifetime)
    });
    input
        .send(
            serde_json::to_vec(&ExecuteRequest {
                operation: operation.transfer().unwrap(),
                selection: vec![Selection {
                    group: 0,
                    items: vec![0, 1],
                }],
            })
            .unwrap(),
        )
        .unwrap();
    while !matches!(
        output
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .reply,
        Reply::ItemFinished { .. }
    ) {}
    drop(input);
    drop(output);
    assert!(worker.join().unwrap().is_err());
    assert!(!root.path().join("first").exists());
    assert!(root.path().join("second").exists());
    assert!(root.path().join("unselected").exists());
}

#[test]
fn a_replaced_file_is_reported_without_deleting_the_replacement_or_new_siblings() {
    let root = tempfile::tempdir().unwrap();
    let operation = OperationContext::default();
    let grant = fixture(root.path(), &operation);
    std::fs::rename(root.path().join("first"), root.path().join("original")).unwrap();
    std::fs::write(root.path().join("first"), b"replacement").unwrap();
    std::fs::write(root.path().join("new-sibling"), b"new").unwrap();
    let mut state = server::State::default();
    state
        .request(Request::Prepare { grant }, &operation, &|_| Ok(()))
        .unwrap();
    let NativeReply::Item { result } = state
        .request(item(&operation, 0), &operation, &|_| Ok(()))
        .unwrap()
    else {
        panic!("Expected an item result");
    };
    assert_eq!(result.succeeded, 0);
    assert_eq!(result.reclaimed_bytes, 0);
    assert_eq!(
        std::fs::read(root.path().join("first")).unwrap(),
        b"replacement"
    );
    assert_eq!(
        std::fs::read(root.path().join("original")).unwrap(),
        b"1234"
    );
    assert!(root.path().join("new-sibling").exists());
    assert!(root.path().join("unselected").exists());
}
