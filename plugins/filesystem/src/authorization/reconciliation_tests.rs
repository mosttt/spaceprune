//! Real temporary-file effects across ordinary, authorized and journal reconciliation stages.

use super::*;
use spaceprune_engine::{EngineBuilder, ScanRequest};
use spaceprune_plugin_api::{CleanupAccess, ItemExecution, ItemExecutionStatus, NullProgress};
use std::sync::atomic::{AtomicUsize, Ordering};

struct PartialOrdinary {
    known_size: bool,
}
impl Handler for PartialOrdinary {
    fn descriptor(&self) -> HandlerDescriptor {
        PathsHandler.descriptor()
    }
    fn validate(&self, task: &TaskRequest) -> PluginResult<()> {
        PathsHandler.validate(task)
    }
    fn scan(
        &self,
        task: &TaskRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult> {
        PathsHandler.scan(task, progress, context)
    }
    fn clean(
        &self,
        _: &TaskRequest,
        _: &[CleanupItem],
        _: &dyn ProgressSink,
        _: &OperationContext,
    ) -> PluginResult<HandlerCleanResult> {
        panic!("The test must enter the same batch/access path as the filesystem handler")
    }
    fn supports_batch_cleanup(&self) -> bool {
        true
    }
    fn clean_batch_with_access(
        &self,
        groups: &[CleanupBatchGroup<'_>],
        progress: &dyn ProgressSink,
        context: &OperationContext,
        access: Option<&dyn CleanupAccess>,
    ) -> Vec<PluginResult<HandlerCleanResult>> {
        let results = groups
            .iter()
            .map(|group| {
                let item = &group.items[0];
                fs::remove_file(Path::new(&item.location).join("ordinary")).unwrap();
                let mut result = HandlerCleanResult {
                    unknown_reclaimed_items: Some(0),
                    ..Default::default()
                };
                let outcome = ItemExecution {
                    item_id: item.id.clone(),
                    status: ItemExecutionStatus::PartiallyApplied,
                    reclaimed_estimate: self.known_size.then_some(6),
                    reclaimed_lower_bound: 0,
                    failure: Some(ItemFailure {
                        authorization: None,
                        item_id: item.id.clone(),
                        message: "Remaining entries require authorization".into(),
                        io_failure: Some(spaceprune_access::error::classify(&io::Error::from(
                            io::ErrorKind::PermissionDenied,
                        ))),
                    }),
                };
                progress
                    .cleanup_item_finished(group.task, &outcome)
                    .unwrap();
                result.record_outcome(outcome);
                Ok(result)
            })
            .collect();
        retry_denied_with_access(groups, results, progress, context, access)
    }
}

#[derive(Clone, Copy)]
enum Completion {
    Lost,
    UnknownSize,
    Confirmed,
    Authorization(spaceprune_access::PermissionOutcome),
}
struct Authorized {
    completion: Completion,
    calls: AtomicUsize,
}
impl CleanupAccess for Authorized {
    fn clean_authorized(
        &self,
        _: &str,
        groups: &[CleanupBatchGroup<'_>],
        progress: &dyn ProgressSink,
        _: &OperationContext,
    ) -> PluginResult<Vec<PluginResult<HandlerCleanResult>>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].items.len(), 1);
        if let Completion::Authorization(outcome) = self.completion {
            let mut failure = PluginError::new(
                "cleanup-authorization-unavailable",
                "Native authorization fixture declined this request",
            );
            failure.authorization = Some(outcome);
            return Err(failure);
        }
        let item = &groups[0].items[0];
        fs::remove_file(Path::new(&item.location).join("authorized")).unwrap();
        fs::remove_dir(&item.location).unwrap();
        let lost = matches!(self.completion, Completion::Lost);
        let outcome = ItemExecution {
            item_id: item.id.clone(),
            status: if lost {
                ItemExecutionStatus::Uncertain
            } else {
                ItemExecutionStatus::Succeeded
            },
            reclaimed_estimate: matches!(self.completion, Completion::Confirmed).then_some(9),
            reclaimed_lower_bound: 0,
            failure: lost.then(|| ItemFailure {
                authorization: None,
                item_id: item.id.clone(),
                io_failure: None,
                message: "No confirmed native outcome; rescan before retrying".into(),
            }),
        };
        progress.cleanup_item_finished(groups[0].task, &outcome)?;
        if lost {
            return Err(PluginError::new(
                "cleanup-result-unconfirmed",
                "Worker disconnected after dispatch",
            ));
        }
        let mut result = HandlerCleanResult {
            unknown_reclaimed_items: Some(0),
            ..Default::default()
        };
        result.record_outcome(outcome);
        Ok(vec![Ok(result)])
    }
}

fn verify(
    ordinary_known: bool,
    completion: Completion,
    expected_bytes: u64,
    expected_unknown: usize,
) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    fs::create_dir_all(root.join("selected")).unwrap();
    fs::write(root.join("selected/ordinary"), b"123456").unwrap();
    fs::write(root.join("selected/authorized"), b"123456789").unwrap();
    fs::write(root.join("unselected"), b"preserved").unwrap();
    let method = if cfg!(target_os = "windows") {
        "uac"
    } else if cfg!(target_os = "macos") {
        "native"
    } else {
        "sudo"
    };
    let source = format!(
        r#"schema-version = 2
name = "Interrupted cleanup"
[modules.cache]
name = "Cache"
[modules.cache.tasks.files]
name = "Files"
handler = "builtin.filesystem/paths"
[modules.cache.tasks.files.options]
roots = [{{ path = {} }}]
files = false
directories = true
elevation = "{method}"
"#,
        toml::Value::String(root.to_string_lossy().into_owned())
    );
    let config = temp.path().join("recipe.toml");
    fs::write(&config, source).unwrap();
    let mut registry = PluginRegistry::new();
    registry
        .register(Arc::new(PartialOrdinary {
            known_size: ordinary_known,
        }))
        .unwrap();
    let journal = temp.path().join("journal");
    let engine = EngineBuilder::new(
        spaceprune_config::ConfigLoader::new()
            .load_path(config)
            .unwrap(),
    )
    .with_registry(registry)
    .with_journal_directory(journal.clone())
    .build()
    .unwrap();
    let operation = OperationContext::default();
    let scan = engine
        .scan(&ScanRequest::default(), &NullProgress, &operation)
        .unwrap();
    let plan = scan.plan_selected(&scan.selectable_keys()).unwrap();
    assert_eq!(plan.item_count(), 1);
    let access = Authorized {
        completion,
        calls: AtomicUsize::new(0),
    };
    let progress = super::tests::TotalsRecorder::default();
    let report = engine.execute_with_access(&plan, &progress, &operation, Some(&access));
    assert_eq!(report.reclaimed_bytes(), expected_bytes);
    assert_eq!(report.unknown_reclaimed_items(), expected_unknown);
    let expected_succeeded = usize::from(!matches!(
        completion,
        Completion::Lost | Completion::Authorization(_)
    ));
    let totals = progress.0.lock().unwrap();
    assert!(
        totals.len()
            >= if matches!(completion, Completion::Authorization(_)) {
                1
            } else {
                2
            },
        "Dispatched native receipts and final results must be projected"
    );
    assert!(
        totals
            .iter()
            .all(|value| *value == (expected_succeeded, expected_bytes, expected_unknown))
    );
    assert_eq!(report.succeeded(), expected_succeeded);
    let outcome = &report.groups[0].result.as_ref().unwrap().outcomes[0];
    assert_eq!(
        outcome.status,
        match completion {
            Completion::Lost => ItemExecutionStatus::Uncertain,
            Completion::Authorization(_) => ItemExecutionStatus::PartiallyApplied,
            _ => ItemExecutionStatus::Succeeded,
        }
    );
    if let Completion::Authorization(authorization) = completion {
        assert!(!root.join("selected/ordinary").exists());
        assert_eq!(
            fs::read(root.join("selected/authorized")).unwrap(),
            b"123456789"
        );
        let failure = outcome.failure.as_ref().unwrap();
        assert_eq!(failure.authorization, Some(authorization));
        if matches!(
            authorization,
            spaceprune_access::PermissionOutcome::HelperMissing
                | spaceprune_access::PermissionOutcome::HelperOutdated
                | spaceprune_access::PermissionOutcome::HelperUntrusted
        ) {
            assert!(
                failure
                    .message
                    .contains("protected cleanup could not start")
            );
            assert!(
                !failure
                    .message
                    .contains("did not return a confirmed result")
            );
        }

        assert_eq!(
            failure.io_failure.as_ref().unwrap().kind,
            spaceprune_access::IoFailureKind::AccessDenied
        );
        assert!(
            failure
                .message
                .starts_with("Remaining entries require authorization")
        );
        assert!(
            failure
                .message
                .contains("Native authorization fixture declined this request")
        );
        assert_eq!(
            report.groups[0]
                .result
                .as_ref()
                .unwrap()
                .failures
                .as_slice(),
            std::slice::from_ref(failure)
        );
    } else {
        assert!(!root.join("selected").exists());
    }
    assert_eq!(fs::read(root.join("unselected")).unwrap(), b"preserved");
    let records: Vec<Value> = fs::read_to_string(journal.join(format!("{}.jsonl", plan.id())))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let last = records
        .iter()
        .rev()
        .find(|record| record["event"] == "item_finished")
        .unwrap();
    if matches!(completion, Completion::Authorization(_)) {
        assert_eq!(last["outcome"]["status"], "partially_applied");
        assert_eq!(
            last["outcome"]["reclaimed_estimate"],
            serde_json::to_value(outcome.reclaimed_estimate).unwrap()
        );
    } else {
        assert_eq!(last["outcome"], serde_json::to_value(outcome).unwrap());
    }
    assert_eq!(
        records.last().unwrap()["report"],
        serde_json::to_value(&report).unwrap()
    );
    let retry = engine.execute_with_access(&plan, &NullProgress, &operation, Some(&access));
    assert!(retry.has_errors());
    assert_eq!(access.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fs::read(root.join("unselected")).unwrap(), b"preserved");
}

#[test]
fn dispatched_retry_loss_retains_known_ordinary_bytes_as_a_lower_bound() {
    verify(true, Completion::Lost, 6, 1);
}
#[test]
fn successful_retry_with_unknown_size_does_not_erase_known_ordinary_bytes() {
    verify(true, Completion::UnknownSize, 6, 1);
}
#[test]
fn known_retry_after_unknown_partial_work_keeps_both_size_facts() {
    verify(false, Completion::Confirmed, 9, 1);
}
#[test]
fn completed_known_retries_count_each_phase_only_once() {
    verify(true, Completion::Confirmed, 15, 0);
}

#[test]
fn two_unknown_phases_still_count_as_one_unknown_selected_item() {
    verify(false, Completion::UnknownSize, 0, 1);
}

#[test]
fn authorization_outcomes_keep_partial_effects_unknown_sizes_and_original_failures() {
    use spaceprune_access::PermissionOutcome;
    for outcome in [
        PermissionOutcome::Denied,
        PermissionOutcome::Cancelled,
        PermissionOutcome::TimedOut,
        PermissionOutcome::NeedsAuthorization,
        PermissionOutcome::HelperUntrusted,
        PermissionOutcome::HelperMissing,
        PermissionOutcome::HelperOutdated,
    ] {
        verify(true, Completion::Authorization(outcome), 6, 0);
        verify(false, Completion::Authorization(outcome), 0, 1);
    }
}
