use super::*;
use spaceprune_plugin_api::{ItemExecution, ItemExecutionStatus};

enum Completion {
    Disconnect,
    Panic,
    StaleFinal,
    PartialThenUncertain,
    UncertainThenStalePartial,
    Authorization,
}

struct InterruptedHandler {
    directory: PathBuf,
    completion: Completion,
}
impl Handler for InterruptedHandler {
    fn descriptor(&self) -> HandlerDescriptor {
        FakeHandler.descriptor()
    }
    fn validate(&self, task: &TaskRequest) -> PluginResult<()> {
        FakeHandler.validate(task)
    }
    fn scan(
        &self,
        task: &TaskRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult> {
        let mut scan = FakeHandler.scan(task, progress, context)?;
        scan.items
            .retain(|item| item.recommendation != Recommendation::Unavailable);
        for item in &mut scan.items {
            item.location = self.directory.join(&item.id).to_str().unwrap().to_owned();
        }
        Ok(scan)
    }
    fn clean(
        &self,
        task: &TaskRequest,
        items: &[CleanupItem],
        progress: &dyn ProgressSink,
        _: &OperationContext,
    ) -> PluginResult<HandlerCleanResult> {
        if matches!(self.completion, Completion::Authorization) {
            let mut error = PluginError::new(
                "cleanup-authorization-unavailable",
                "Autorisierung erforderlich",
            );
            error.authorization = Some(spaceprune_access::PermissionOutcome::NeedsAuthorization);
            error.io_failure = Some(spaceprune_access::IoFailure {
                kind: spaceprune_access::IoFailureKind::AccessDenied,
                os_code: Some(13),
            });
            return Err(error);
        }
        let item = &items[0];
        fs::remove_file(&item.location).unwrap();
        let uncertain_receipt = matches!(self.completion, Completion::UncertainThenStalePartial);
        let partial_receipt = matches!(self.completion, Completion::PartialThenUncertain);
        progress.cleanup_item_finished(
            task,
            &ItemExecution {
                item_id: item.id.clone(),
                status: if uncertain_receipt {
                    ItemExecutionStatus::Uncertain
                } else if partial_receipt {
                    ItemExecutionStatus::PartiallyApplied
                } else {
                    ItemExecutionStatus::Succeeded
                },
                reclaimed_estimate: (!uncertain_receipt).then_some(100),
                reclaimed_lower_bound: 0,
                failure: (uncertain_receipt || partial_receipt).then(|| ItemFailure {
                    file_usage: None,
                    authorization: None,
                    item_id: item.id.clone(),
                    io_failure: None,
                    message: "Interrupted phase".into(),
                }),
            },
        )?;
        match self.completion {
            Completion::Panic => panic!("fixture interruption after a durable item receipt"),
            Completion::Authorization => unreachable!(),
            Completion::Disconnect => Err(PluginError::new(
                "fixture-disconnected",
                "No final handler reply",
            )),
            Completion::StaleFinal
            | Completion::PartialThenUncertain
            | Completion::UncertainThenStalePartial => {
                let mut result = HandlerCleanResult {
                    unknown_reclaimed_items: Some(0),
                    ..Default::default()
                };
                for item in items {
                    result.record_outcome(ItemExecution {
                        item_id: item.id.clone(),
                        status: if item.id == items[0].id {
                            match self.completion {
                                Completion::PartialThenUncertain => ItemExecutionStatus::Uncertain,
                                Completion::UncertainThenStalePartial => {
                                    ItemExecutionStatus::PartiallyApplied
                                }
                                _ => ItemExecutionStatus::Failed,
                            }
                        } else {
                            ItemExecutionStatus::Failed
                        },
                        reclaimed_estimate: if item.id == items[0].id {
                            match self.completion {
                                Completion::PartialThenUncertain => None,
                                Completion::UncertainThenStalePartial => Some(100),
                                _ => Some(0),
                            }
                        } else {
                            Some(0)
                        },
                        reclaimed_lower_bound: 0,
                        failure: Some(ItemFailure {
                            file_usage: None,
                            authorization: None,
                            item_id: item.id.clone(),
                            io_failure: None,
                            message: "Earlier ordinary-permission result".into(),
                        }),
                    });
                }
                Ok(result)
            }
        }
    }
}

fn fixture(directory: &std::path::Path, completion: Completion) -> (Engine, CleanupPlan) {
    fs::write(directory.join("one"), b"selected").unwrap();
    fs::write(directory.join("optional"), b"must remain").unwrap();
    let mut registry = PluginRegistry::new();
    registry
        .register(Arc::new(InterruptedHandler {
            directory: directory.to_path_buf(),
            completion,
        }))
        .unwrap();
    let engine = EngineBuilder::new(test_config())
        .with_registry(registry)
        .with_journal_directory(directory.join("state"))
        .build()
        .unwrap();
    let scan = engine
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    let plan = scan.plan_selected(&scan.selectable_keys()).unwrap();
    (engine, plan)
}
fn records(directory: &std::path::Path, plan: &CleanupPlan) -> Vec<serde_json::Value> {
    fs::read_to_string(directory.join("state").join(format!("{}.jsonl", plan.id())))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn handler_disconnect_keeps_confirmed_effects_and_marks_remaining_results_uncertain() {
    let temp = tempdir().unwrap();
    let (engine, plan) = fixture(temp.path(), Completion::Disconnect);
    let report = engine.execute(&plan, &NullProgress, &OperationContext::default());
    assert!(report.has_errors());
    assert_eq!(report.succeeded(), 1);
    assert_eq!(report.failure_count(), 1);
    assert_eq!(report.reclaimed_bytes(), 100);
    let outcomes = &report.groups[0].result.as_ref().unwrap().outcomes;
    assert_eq!(outcomes[0].status, ItemExecutionStatus::Succeeded);
    assert_eq!(outcomes[1].status, ItemExecutionStatus::Uncertain);
    assert_eq!(outcomes[1].reclaimed_estimate, None);
    assert_eq!(report.unknown_reclaimed_items(), 1);
    assert!(!temp.path().join("one").exists());
    assert!(temp.path().join("optional").exists());
    let records = records(temp.path(), &plan);
    assert_eq!(records[0]["schema_version"], 4);
    assert_eq!(records[2]["event"], "item_finished");
    assert_eq!(records[2]["outcome"]["status"], "succeeded");
    assert_eq!(
        records.last().unwrap()["report"]["groups"][0]["result"]["succeeded"],
        1
    );
}

#[test]
fn group_failure_retains_native_authorization_and_os_code_without_attempting_cleanup() {
    let temp = tempdir().unwrap();
    let (engine, plan) = fixture(temp.path(), Completion::Authorization);
    let report = engine.execute(&plan, &NullProgress, &OperationContext::default());
    let group = &report.groups[0];
    let failure = group.failure.as_ref().unwrap();
    assert_eq!(group.error.as_deref(), Some("Autorisierung erforderlich"));
    assert_eq!(failure.code, "cleanup-authorization-unavailable");
    assert_eq!(
        failure.authorization,
        Some(spaceprune_access::PermissionOutcome::NeedsAuthorization)
    );
    assert_eq!(failure.io_failure.as_ref().unwrap().os_code, Some(13));
    assert!(
        group
            .result
            .as_ref()
            .unwrap()
            .outcomes
            .iter()
            .all(|outcome| outcome.status == ItemExecutionStatus::NotAttempted)
    );
    assert_eq!(report.failure_count(), 1);
    assert_eq!(report.reclaimed_bytes(), 0);
    assert_eq!(report.unknown_reclaimed_items(), 0);
    assert!(temp.path().join("one").exists());
    assert!(temp.path().join("optional").exists());
    assert_eq!(
        records(temp.path(), &plan).last().unwrap()["report"],
        serde_json::to_value(&report).unwrap()
    );
    let restored: ExecutionReport =
        serde_json::from_value(serde_json::to_value(&report).unwrap()).unwrap();
    assert_eq!(restored, report);
}

#[test]
fn interruption_leaves_a_durable_prefix_without_claiming_completion_or_replaying() {
    let temp = tempdir().unwrap();
    let (engine, plan) = fixture(temp.path(), Completion::Panic);
    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        engine.execute(&plan, &NullProgress, &OperationContext::default());
    }));
    assert!(interrupted.is_err());
    let records = records(temp.path(), &plan);
    assert_eq!(records.last().unwrap()["event"], "item_finished");
    assert!(!records.iter().any(|record| record["event"] == "finished"));
    assert!(!temp.path().join("one").exists());
    assert!(temp.path().join("optional").exists());
    let retry = engine.execute(&plan, &NullProgress, &OperationContext::default());
    assert!(retry.has_errors());
    assert_eq!(retry.succeeded(), 0);
    assert!(temp.path().join("optional").exists());
}

#[test]
fn per_item_evidence_must_match_the_selection_and_aggregate() {
    let temp = tempdir().unwrap();
    let (_, plan) = fixture(temp.path(), Completion::Disconnect);
    let group = &plan.groups()[0];
    let mut result = HandlerCleanResult {
        unknown_reclaimed_items: Some(0),
        ..Default::default()
    };
    for item in &group.items {
        result.record_outcome(ItemExecution {
            item_id: item.id.clone(),
            status: ItemExecutionStatus::Succeeded,
            reclaimed_estimate: item.reclaimable_bytes,
            reclaimed_lower_bound: 0,
            failure: None,
        });
    }
    assert!(result.validate_selected(&group.items, false).is_ok());
    let mut wrong = result.clone();
    wrong.outcomes[1].item_id = wrong.outcomes[0].item_id.clone();
    assert!(wrong.validate_selected(&group.items, false).is_err());
    let mut wrong = result.clone();
    wrong.reclaimed_bytes += 1;
    assert!(wrong.validate_selected(&group.items, false).is_err());
    result.outcomes[0].status = ItemExecutionStatus::AlreadyAbsent;
    assert!(result.validate_selected(&group.items, false).is_err());
}

#[test]
fn a_stale_failure_cannot_erase_a_confirmed_privileged_success() {
    let temp = tempdir().unwrap();
    let (engine, plan) = fixture(temp.path(), Completion::StaleFinal);
    let report = engine.execute(&plan, &NullProgress, &OperationContext::default());
    assert_eq!(report.succeeded(), 1);
    assert_eq!(report.failure_count(), 1);
    assert_eq!(report.reclaimed_bytes(), 100);
    let result = report.groups[0].result.as_ref().unwrap();
    assert_eq!(result.outcomes[0].status, ItemExecutionStatus::Succeeded);
    assert_eq!(result.failures.len(), 1);
    assert!(!temp.path().join("one").exists());
    assert!(temp.path().join("optional").exists());
}

#[test]
fn uncertain_effects_keep_prior_size_evidence_without_becoming_a_retryable_partial_failure() {
    for completion in [
        Completion::PartialThenUncertain,
        Completion::UncertainThenStalePartial,
    ] {
        let temp = tempdir().unwrap();
        let (engine, plan) = fixture(temp.path(), completion);
        let report = engine.execute(&plan, &NullProgress, &OperationContext::default());
        let result = report.groups[0].result.as_ref().unwrap();
        assert_eq!(result.outcomes[0].status, ItemExecutionStatus::Uncertain);
        assert_eq!(result.outcomes[0].reclaimed_estimate, None);
        assert_eq!(result.outcomes[0].reclaimed_lower_bound, 100);
        assert_eq!(report.reclaimed_bytes(), 100);
        assert_eq!(report.unknown_reclaimed_items(), 1);
        assert_eq!(report.succeeded(), 0);
        assert!(
            result
                .validate_selected(&plan.groups()[0].items, false)
                .is_ok()
        );
        assert!(temp.path().join("optional").exists());
        let journal = records(temp.path(), &plan);
        assert_eq!(
            journal.last().unwrap()["report"],
            serde_json::to_value(report).unwrap()
        );
    }
}

#[test]
fn native_finish_failure_is_committed_separately_without_rewriting_finished_groups() {
    use spaceprune_plugin_api::CleanupAccess;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Access<'a> {
        journal: PathBuf,
        calls: AtomicUsize,
        cancel: Option<&'a OperationContext>,
    }
    impl CleanupAccess for Access<'_> {
        fn clean_authorized(
            &self,
            _: &str,
            _: &[CleanupBatchGroup<'_>],
            _: &dyn ProgressSink,
            _: &OperationContext,
        ) -> PluginResult<Vec<PluginResult<HandlerCleanResult>>> {
            panic!("The ordinary fixture needs no native cleanup")
        }
        fn finish(&self) -> PluginResult<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let records = fs::read_to_string(&self.journal).unwrap();
            let last: serde_json::Value =
                serde_json::from_str(records.lines().last().unwrap()).unwrap();
            assert_eq!(last["event"], "group_finished");
            if let Some(operation) = self.cancel {
                operation.cancel();
            }
            let mut error = PluginError::new(
                "cleanup-stop-unconfirmed",
                "Native worker shutdown was not confirmed",
            );
            error.authorization = Some(spaceprune_access::PermissionOutcome::TimedOut);
            Err(error)
        }
    }
    for cancelled in [false, true] {
        let directory = tempdir().unwrap();
        let mut registry = PluginRegistry::new();
        registry.register(Arc::new(FakeHandler)).unwrap();
        let engine = EngineBuilder::new(test_config())
            .with_registry(registry)
            .with_journal_directory(directory.path().to_owned())
            .build()
            .unwrap();
        let operation = OperationContext::default();
        let scan = engine
            .scan(&ScanRequest::default(), &NullProgress, &operation)
            .unwrap();
        let plan = scan.plan_recommended().unwrap();
        let access = Access {
            journal: directory.path().join(format!("{}.jsonl", plan.id())),
            calls: AtomicUsize::new(0),
            cancel: cancelled.then_some(&operation),
        };
        let report = engine.execute_with_access(&plan, &NullProgress, &operation, Some(&access));
        assert_eq!(access.calls.load(Ordering::SeqCst), 1);
        assert_eq!(report.cancelled, cancelled);
        assert!(report.has_errors());
        assert_eq!(report.failure_count(), 1);
        assert_eq!((report.succeeded(), report.reclaimed_bytes()), (1, 100));
        assert_eq!(
            report.operation_failures[0].code,
            "cleanup-stop-unconfirmed"
        );
        assert_eq!(
            report.operation_failures[0].authorization,
            Some(spaceprune_access::PermissionOutcome::TimedOut)
        );
        assert!(report.groups[0].error.is_none());
        let journal = fs::read_to_string(&access.journal).unwrap();
        let records = journal
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        let group = records
            .iter()
            .find(|record| record["event"] == "group_finished")
            .unwrap();
        assert_eq!(
            group["result"],
            serde_json::to_value(&report.groups[0]).unwrap()
        );
        assert_eq!(
            records.last().unwrap()["report"],
            serde_json::to_value(&report).unwrap()
        );
        let recovered =
            crate::recover_execution(&spaceprune_plugin_api::ExecutionJournalReference {
                plan_id: plan.id().to_owned(),
                path: spaceprune_access::NativePath::from_path(&access.journal),
            })
            .unwrap();
        assert!(recovered.finished);
        assert_eq!(recovered.report, report);
    }
}
