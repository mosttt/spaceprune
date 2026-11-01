use spaceprune_plugin_api::NullProgress;
use std::cell::Cell;
use std::env;
use tempfile::tempdir;

use super::*;

const METHOD: &str = if cfg!(target_os = "windows") {
    "uac"
} else if cfg!(target_os = "macos") {
    "native"
} else {
    "sudo"
};

#[derive(Default)]
pub(super) struct TotalsRecorder(pub(super) std::sync::Mutex<Vec<(usize, u64, usize)>>);

impl ProgressSink for TotalsRecorder {
    fn emit(&self, _event: ProgressEvent) {}

    fn record_cleanup(&self, _task: &TaskRequest, succeeded: usize, bytes: u64, unknown: usize) {
        self.0.lock().unwrap().push((succeeded, bytes, unknown));
    }
}

#[test]
fn cleanup_progress_keeps_measured_subtotals_and_finishes_partial_cancelled_work() {
    let temp = tempdir().unwrap();
    let mut items = Vec::new();
    for name in ["known", "unknown", "missing", "cancelled"] {
        let path = temp.path().join(name);
        fs::write(&path, b"data").unwrap();
        items.push(item(&path));
    }
    items[1].reclaimable_bytes = None;
    fs::remove_file(temp.path().join("missing")).unwrap();
    let task = task(temp.path(), "never");
    let progress = TotalsRecorder::default();
    let result = clean_local(&task, &items, &progress, &OperationContext::default()).unwrap();
    assert_eq!(result.reclaimed_bytes, 12);
    assert_eq!(result.outcomes[1].reclaimed_estimate, None);
    assert_eq!(result.outcomes[1].reclaimed_lower_bound, 4);
    assert_eq!(result.unknown_reclaimed_items(), 1);
    assert_eq!(progress.0.lock().unwrap().last(), Some(&(4, 12, 1)));
    assert!(items.iter().all(|item| !Path::new(&item.id).exists()));

    for selected in &mut items {
        fs::write(&selected.id, b"data").unwrap();
        *selected = item(Path::new(&selected.id));
    }
    let context = OperationContext::default();
    let mut updates = Vec::new();
    let result = execution_tests::clean_with_progress(&task, &items, &context, |result, item| {
        assert!(!Path::new(&item.id).exists());
        updates.push((result.succeeded, result.reclaimed_bytes));
        context.cancel();
    });
    assert_eq!(result.succeeded, 1);
    assert_eq!(result.reclaimed_bytes, 4);
    assert_eq!(updates.last(), Some(&(1, 4)));
    assert!(items[1..].iter().all(|item| Path::new(&item.id).exists()));
}

#[test]
fn worker_updates_preserve_local_success_without_double_counting_retries() {
    let temp = tempdir().unwrap();
    let task = task(temp.path(), "never");
    use spaceprune_plugin_api::{ItemExecution, ItemExecutionStatus};
    let key = |id: &str| (task.module_id.clone(), task.task_id.clone(), id.to_owned());
    let outcome = |id: &str, bytes| ItemExecution {
        item_id: id.into(),
        status: ItemExecutionStatus::Succeeded,
        reclaimed_estimate: bytes,
        reclaimed_lower_bound: 0,
        failure: None,
    };
    let previous = BTreeMap::from([
        (key("known"), outcome("known", Some(1024))),
        (key("unknown"), outcome("unknown", None)),
    ]);
    let progress = TotalsRecorder::default();
    let resumed = ResumedProgress::new(&previous, &progress);
    let next = outcome("retried", Some(2048));
    resumed.cleanup_item_finished(&task, &next).unwrap();
    resumed.record_cleanup(&task, 1, 2048, 0);
    resumed.cleanup_item_finished(&task, &next).unwrap();
    assert_eq!(*progress.0.lock().unwrap(), [(3, 3072, 1), (3, 3072, 1)]);
}

pub(super) fn task(root: &Path, method: &str) -> TaskRequest {
    serde_json::from_value(json!({
        "module_id": "fixture", "task_id": root.file_name().unwrap().to_string_lossy(),
        "name": "Filesystem fixture", "handler_id": "builtin.filesystem/paths",
        "options": {"roots": [{"path": root}], "elevation": method},
        "recommend": {}, "tags": []
    }))
    .unwrap()
}

pub(super) fn item(path: &Path) -> CleanupItem {
    let mut result = HandlerScanResult::default();
    append_item(
        &mut result,
        path,
        ScanRoot {
            path: path.parent().unwrap(),
            index: 0,
        },
        &Recommend::default(),
        0,
        None,
        &OperationContext::default(),
    );
    result.items.remove(0)
}

fn denied(item: &CleanupItem) -> PluginResult<HandlerCleanResult> {
    let mut result = HandlerCleanResult::default();
    result.record_outcome(spaceprune_plugin_api::ItemExecution {
        item_id: item.id.clone(),
        status: spaceprune_plugin_api::ItemExecutionStatus::Failed,
        reclaimed_estimate: Some(0),
        reclaimed_lower_bound: 0,
        failure: Some(ItemFailure {
            file_usage: None,
            authorization: None,
            io_failure: Some(spaceprune_access::error::classify(&io::Error::from(
                io::ErrorKind::PermissionDenied,
            ))),
            item_id: item.id.clone(),
            message: "Permission denied (os error 13)".into(),
        }),
    });
    Ok(result)
}

#[test]
fn combines_permission_failures_once_and_retries_only_confirmed_items() {
    let temp = tempdir().unwrap();
    let mut tasks = Vec::new();
    let mut items = Vec::new();
    for name in ["first", "second"] {
        let root = temp.path().join(name);
        fs::create_dir(&root).unwrap();
        fs::write(root.join("selected"), b"remove").unwrap();
        fs::write(root.join("unselected"), b"retain").unwrap();
        tasks.push(task(&root, METHOD));
        items.push(vec![item(&root.join("selected"))]);
    }
    let groups: Vec<_> = tasks
        .iter()
        .zip(&items)
        .map(|(task, items)| CleanupBatchGroup { task, items })
        .collect();
    let mut results = items
        .iter()
        .map(|items| denied(&items[0]))
        .collect::<Vec<_>>();
    let calls = Cell::new(0);
    retry_with(
        &groups,
        &mut results,
        &NullProgress,
        &OperationContext::default(),
        |method, request| {
            calls.set(calls.get() + 1);
            assert_eq!(method, METHOD);
            assert_eq!(request.groups.len(), 2);
            assert!(request.groups.iter().all(|group| group.items.len() == 1));
            Ok(Reply {
                results: request
                    .groups
                    .iter()
                    .map(|group| {
                        execution_tests::clean(
                            &group.task,
                            &group.items,
                            &OperationContext::default(),
                        )
                    })
                    .collect(),
            })
        },
    );
    assert_eq!(calls.get(), 1);
    for (result, task) in results.iter().zip(&tasks) {
        let result = result.as_ref().unwrap();
        assert_eq!(
            (result.attempted, result.succeeded, result.failures.len()),
            (1, 1, 0)
        );
        let root = Path::new(task.options["roots"][0]["path"].as_str().unwrap());
        assert!(!root.join("selected").exists());
        assert!(root.join("unselected").exists());
    }
}

#[test]
fn refusal_stops_further_authentication_and_never_mode_does_not_prompt() {
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("file"), b"keep").unwrap();
    let selected = vec![item(&temp.path().join("file"))];
    let tasks = [
        task(temp.path(), METHOD),
        task(
            temp.path(),
            if cfg!(target_os = "windows") {
                "uac"
            } else if cfg!(target_os = "macos") {
                "native"
            } else {
                "pkexec"
            },
        ),
        task(temp.path(), "never"),
    ];
    let groups: Vec<_> = tasks
        .iter()
        .map(|task| CleanupBatchGroup {
            task,
            items: &selected,
        })
        .collect();
    let mut results = tasks
        .iter()
        .map(|_| denied(&selected[0]))
        .collect::<Vec<_>>();
    let calls = Cell::new(0);
    retry_with(
        &groups,
        &mut results,
        &NullProgress,
        &OperationContext::default(),
        |_, _| {
            calls.set(calls.get() + 1);
            Err(PluginError::new(
                "cleanup-authorization-unavailable",
                "Authorization was denied",
            ))
        },
    );
    assert_eq!(calls.get(), 1);
    assert!(temp.path().join("file").exists());
    assert!(
        results[0].as_ref().unwrap().failures[0]
            .message
            .contains("denied")
    );
    assert_eq!(
        results[2].as_ref().unwrap().failures[0].message,
        "Permission denied (os error 13)"
    );
}

#[test]
fn cancellation_and_non_permission_errors_do_not_prompt() {
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("file"), b"keep").unwrap();
    let selected = vec![item(&temp.path().join("file"))];
    let task = task(temp.path(), METHOD);
    let groups = [CleanupBatchGroup {
        task: &task,
        items: &selected,
    }];
    let mut results = vec![denied(&selected[0])];
    let context = OperationContext::default();
    context.cancel();
    retry_with(&groups, &mut results, &NullProgress, &context, |_, _| {
        panic!("No authorization after cancellation")
    });
    results[0].as_mut().unwrap().failures[0].message = "Read-only file system".to_owned();
    results[0].as_mut().unwrap().failures[0].io_failure = None;
    retry_with(
        &groups,
        &mut results,
        &NullProgress,
        &OperationContext::default(),
        |_, _| panic!("No authorization for unrelated errors"),
    );
}

#[test]
fn relative_roots_are_resolved_before_the_system_changes_worker_environment() {
    let cwd = env::current_dir().unwrap();
    let temp = tempfile::tempdir_in(&cwd).unwrap();
    let path = temp.path().join("selected");
    fs::write(&path, b"selected").unwrap();
    let mut task = task(temp.path(), METHOD);
    task.options["roots"][0]["path"] = json!(temp.path().strip_prefix(&cwd).unwrap());
    let items = [item(&path)];
    let groups = [CleanupBatchGroup {
        task: &task,
        items: &items,
    }];
    let mut results = [denied(&items[0])];
    retry_with(
        &groups,
        &mut results,
        &NullProgress,
        &OperationContext::default(),
        |_, request| {
            assert_eq!(
                request.groups[0].task.options["roots"][0]["path"],
                json!(temp.path())
            );
            Ok(Reply {
                results: vec![execution_tests::clean(
                    &request.groups[0].task,
                    &request.groups[0].items,
                    &OperationContext::default(),
                )],
            })
        },
    );
    assert_eq!(results[0].as_ref().unwrap().succeeded, 1);
    assert!(!path.exists());
}

#[test]
fn partial_permission_retry_uses_original_identity_and_merges_only_remaining_effects() {
    use spaceprune_plugin_api::{ItemExecution, ItemExecutionStatus};
    for replaced in [false, true] {
        let temp = tempdir().unwrap();
        let selected = temp.path().join("selected");
        fs::create_dir(&selected).unwrap();
        fs::write(selected.join("already-removed"), b"before").unwrap();
        fs::write(selected.join("remaining"), b"after").unwrap();
        fs::write(temp.path().join("unselected"), b"retain").unwrap();
        let items = [item(&selected)];
        let task = task(temp.path(), METHOD);
        fs::remove_file(selected.join("already-removed")).unwrap();
        let mut local = HandlerCleanResult::default();
        local.record_outcome(ItemExecution {
            item_id: items[0].id.clone(),
            status: ItemExecutionStatus::PartiallyApplied,
            reclaimed_estimate: Some(6),
            reclaimed_lower_bound: 0,
            failure: Some(ItemFailure {
                file_usage: None,
                authorization: None,
                item_id: items[0].id.clone(),
                message: "Partial cleanup stopped on access denial".into(),
                io_failure: Some(spaceprune_access::error::classify(&io::Error::from(
                    io::ErrorKind::PermissionDenied,
                ))),
            }),
        });
        let mut results = [Ok(local)];
        let groups = [CleanupBatchGroup {
            task: &task,
            items: &items,
        }];
        let calls = Cell::new(0);
        retry_with(
            &groups,
            &mut results,
            &NullProgress,
            &OperationContext::default(),
            |_, request| {
                calls.set(calls.get() + 1);
                if replaced {
                    fs::rename(&selected, temp.path().join("original")).unwrap();
                    fs::create_dir(&selected).unwrap();
                    fs::write(selected.join("replacement"), b"retain").unwrap();
                }
                Ok(Reply {
                    results: vec![execution_tests::clean(
                        &request.groups[0].task,
                        &request.groups[0].items,
                        &OperationContext::default(),
                    )],
                })
            },
        );
        assert_eq!(calls.get(), 1);
        let result = results[0].as_ref().unwrap();
        assert_eq!(result.attempted, 1);
        assert_eq!(result.outcomes.len(), 1);
        assert!(temp.path().join("unselected").exists());
        if replaced {
            assert_eq!(
                result.outcomes[0].status,
                ItemExecutionStatus::PartiallyApplied
            );
            assert_eq!(result.reclaimed_bytes, 6);
            assert_eq!(result.succeeded, 0);
            assert!(selected.join("replacement").exists());
            assert!(temp.path().join("original/remaining").exists());
        } else {
            assert_eq!(result.outcomes[0].status, ItemExecutionStatus::Succeeded);
            assert_eq!(result.reclaimed_bytes, 11);
            assert_eq!(result.outcomes[0].reclaimed_estimate, Some(11));
            assert_eq!(result.succeeded, 1);
            assert!(result.failures.is_empty());
            assert!(!selected.exists());
        }
    }
}

#[test]
fn group_level_access_failure_retains_authorization_outcome_and_native_code() {
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("selected"), b"retain").unwrap();
    let items = [item(&temp.path().join("selected"))];
    let task = task(temp.path(), METHOD);
    let groups = [CleanupBatchGroup {
        task: &task,
        items: &items,
    }];
    let mut original = PluginError::new("filesystem-denied", "先前访问失败");
    original.io_failure = Some(spaceprune_access::IoFailure {
        kind: spaceprune_access::IoFailureKind::AccessDenied,
        os_code: Some(13),
    });
    let mut results = [Err(original)];
    retry_with(
        &groups,
        &mut results,
        &NullProgress,
        &OperationContext::default(),
        |_, _| {
            let mut failure = PluginError::new("cleanup-authorization-unavailable", "Abgelehnt");
            failure.authorization = Some(spaceprune_access::PermissionOutcome::Denied);
            Err(failure)
        },
    );
    let failure = results[0].as_ref().unwrap_err();
    assert_eq!(failure.code, "filesystem-denied");
    assert_eq!(failure.io_failure.as_ref().unwrap().os_code, Some(13));
    assert_eq!(
        failure.authorization,
        Some(spaceprune_access::PermissionOutcome::Denied)
    );
    assert!(failure.message.starts_with("先前访问失败"));
    assert!(failure.message.contains("Abgelehnt"));
    assert!(temp.path().join("selected").exists());
}
