//! Single-consumer cleanup orchestration and authoritative result aggregation.

use super::journal::ExecutionJournal;
use super::*;
mod observed;
use observed::ObservedProgress;
pub(crate) use observed::reconcile_outcome;

impl Engine {
    /// Report a host policy refusal without authorizing or attempting any cleanup.
    pub fn reject_execution(&self, plan: &CleanupPlan, failure: PluginError) -> ExecutionReport {
        let mut report = rejected(plan, &failure.to_string());
        if report.groups.is_empty() {
            report.operation_failures.push(failure.clone());
        }
        for group in &mut report.groups {
            group.failure = Some(failure.clone());
            group.size_accounting = self
                .registry
                .get(&group.handler_id)
                .map(|handler| handler.descriptor().size_accounting)
                .unwrap_or_default();
        }
        report
    }

    /// 执行清理计划并汇总所有任务组结果。
    pub fn execute(
        &self,
        plan: &CleanupPlan,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> ExecutionReport {
        self.execute_with_access(plan, progress, context, None)
    }

    /// Execute with a host-owned native session for the exact confirmed plan.
    pub fn execute_with_access(
        &self,
        plan: &CleanupPlan,
        progress: &dyn ProgressSink,
        context: &OperationContext,
        access: Option<&dyn spaceprune_plugin_api::CleanupAccess>,
    ) -> ExecutionReport {
        if let Some(group) = plan
            .groups
            .iter()
            .find(|group| handler_disabled(&self.disabled_plugins, &group.handler_id))
        {
            return self.reject_execution(
                plan,
                PluginError::new(
                    "plugin-disabled",
                    spaceprune_contracts::messages::message(
                        "engine.handler_disabled",
                        &[("p0", (group.handler_id).to_string())],
                    ),
                ),
            );
        }
        if let Err(error) = plan.reserve() {
            return rejected(plan, error);
        }
        let accounting: Vec<_> = plan
            .groups
            .iter()
            .map(|group| {
                self.registry
                    .get(&group.handler_id)
                    .map(|handler| handler.descriptor().size_accounting)
                    .unwrap_or_default()
            })
            .collect();
        let journal = match self
            .journal_directory
            .as_deref()
            .map(|directory| ExecutionJournal::start(directory, plan, &accounting))
            .transpose()
        {
            Ok(journal) => journal,
            Err(error) => {
                return rejected(plan, &format!("execution-journal-unavailable: {error}"));
            }
        };
        if let Some(journal) = &journal
            && let Err(error) = progress.execution_journal_started(journal.reference())
        {
            return rejected(plan, &format!("execution-journal-binding-failed: {error}"));
        }
        let observed = ObservedProgress::new(progress, context, plan, &self.registry, journal);
        let progress: &dyn ProgressSink = &observed;
        let operation_id = self.event_bus.next_operation_id();
        self.event_bus.emit(
            context,
            SpacePruneEvent::CleanupStarted {
                operation_id,
                group_count: plan.groups.len(),
                item_count: plan.item_count(),
            },
        );
        let mut reports = vec![None; plan.groups.len()];
        let mut attempted_total = 0_usize;
        for (index, first) in plan.groups.iter().enumerate() {
            if context.is_stopped() {
                break;
            }
            if reports[index].is_some() {
                continue;
            }
            let handler = self.registry.get(&first.handler_id);
            let batched = handler
                .as_ref()
                .is_some_and(|handler| handler.supports_batch_cleanup());
            let indices: Vec<_> = if batched {
                (index..plan.groups.len())
                    .filter(|&next| plan.groups[next].handler_id == first.handler_id)
                    .collect()
            } else {
                vec![index]
            };
            let mut valid = Vec::new();
            for &next in &indices {
                let group = &plan.groups[next];
                self.event_bus.emit(
                    context,
                    SpacePruneEvent::CleanupGroupStarted {
                        operation_id,
                        module_id: group.module_id.clone(),
                        task_id: group.task_id.clone(),
                        handler_id: group.handler_id.clone(),
                        item_count: group.items.len(),
                    },
                );
                progress.emit(ProgressEvent {
                    phase: ProgressPhase::Clean,
                    module_id: Some(group.module_id.clone()),
                    task_id: Some(group.task_id.clone()),
                    handler_id: Some(group.handler_id.clone()),
                    current: None,
                    total: Some(group.items.len()),
                    message: spaceprune_contracts::messages::message(
                        "engine.cleaning_items",
                        &[("p0", (group.items.len()).to_string())],
                    ),
                });
                let error = validate_plan_group(group)
                    .err()
                    .or_else(|| {
                        handler.is_none().then(|| {
                            spaceprune_contracts::messages::message(
                                "engine.handler_missing",
                                &[("p0", (group.handler_id).to_string())],
                            )
                        })
                    })
                    .or_else(|| observed.group_started(group).err());
                if error.is_none() {
                    valid.push(next);
                }
                reports[next] = Some(GroupExecutionReport {
                    failure: None,
                    size_accounting: observed.accounting_for(&group.handler_id),
                    module_id: group.module_id.clone(),
                    task_id: group.task_id.clone(),
                    handler_id: group.handler_id.clone(),
                    result: None,
                    error,
                });
            }
            if let Some(handler) = handler {
                let batch: Vec<_> = valid
                    .iter()
                    .map(|&next| CleanupBatchGroup {
                        task: &plan.groups[next].request,
                        items: &plan.groups[next].items,
                    })
                    .collect();
                let results = if context.is_stopped() {
                    valid
                        .iter()
                        .map(|&next| Ok(not_attempted(&plan.groups[next])))
                        .collect()
                } else if batch.is_empty() {
                    Vec::new()
                } else {
                    handler.clean_batch_with_access(&batch, progress, context, access)
                };
                if results.len() != valid.len() {
                    for &next in &valid {
                        let report = reports[next].as_mut().expect("initialized group");
                        let error = PluginError::new(
                            "cleanup-result-invalid",
                            spaceprune_contracts::messages::text("engine.invalid_results"),
                        );
                        report.error = Some(error.to_string());
                        report.failure = Some(error.clone());
                        observed.reconcile(&plan.groups[next], &mut report.result, Some(&error));
                    }
                } else {
                    for (next, result) in valid.into_iter().zip(results) {
                        let report = reports[next].as_mut().expect("initialized group");
                        match result {
                            Ok(result) => match result
                                .validate_selected(&plan.groups[next].items, context.is_stopped())
                            {
                                Ok(()) => {
                                    report.result = Some(result);
                                    observed.reconcile(
                                        &plan.groups[next],
                                        &mut report.result,
                                        None,
                                    );
                                }
                                Err(error) => {
                                    report.error = Some(error.clone());
                                    let error = PluginError::new("cleanup-result-invalid", error);
                                    report.failure = Some(error.clone());
                                    observed.reconcile(
                                        &plan.groups[next],
                                        &mut report.result,
                                        Some(&error),
                                    );
                                }
                            },
                            Err(error) => {
                                report.error = Some(error.to_string());
                                report.failure = Some(error.clone());
                                observed.reconcile(
                                    &plan.groups[next],
                                    &mut report.result,
                                    Some(&error),
                                );
                            }
                        }
                    }
                }
            }
            for next in indices {
                let report = reports[next].as_mut().expect("initialized group");
                if let Err(error) = observed.group_finished(report) {
                    report.error = Some(format!(
                        "execution-journal-write-failed after recorded effects: {error}"
                    ));
                    report.failure = Some(PluginError::new(
                        "execution-journal-write-failed",
                        report.error.as_ref().expect("journal failure").clone(),
                    ));
                    context.cancel();
                }
                if let Some(result) = &report.result {
                    progress.record_cleanup(
                        &plan.groups[next].request,
                        result.succeeded,
                        result.reclaimed_bytes,
                        result.unknown_reclaimed_items(),
                    );
                }
                let (attempted, succeeded, failure_count, reclaimed_bytes) =
                    report.result.as_ref().map_or((0, 0, 0, 0), |result| {
                        (
                            result.attempted,
                            result.succeeded,
                            result.failures.len(),
                            result.reclaimed_bytes,
                        )
                    });
                attempted_total = attempted_total.saturating_add(attempted);
                self.event_bus.emit(
                    context,
                    SpacePruneEvent::CleanupGroupFinished {
                        operation_id,
                        module_id: report.module_id.clone(),
                        task_id: report.task_id.clone(),
                        handler_id: report.handler_id.clone(),
                        attempted,
                        succeeded,
                        failure_count,
                        reclaimed_bytes,
                        unknown_reclaimed_items: Some(
                            report
                                .result
                                .as_ref()
                                .map_or(0, HandlerCleanResult::unknown_reclaimed_items),
                        ),
                        error: report.error.clone(),
                    },
                );
            }
        }
        let operation_failures = access
            .and_then(|access| access.finish().err())
            .into_iter()
            .collect();
        let mut report = ExecutionReport {
            operation_failures,
            groups: reports
                .into_iter()
                .zip(&plan.groups)
                .map(|(report, group)| {
                    report.unwrap_or_else(|| {
                        let result = not_attempted(group);
                        GroupExecutionReport {
                            failure: None,
                            size_accounting: observed.accounting_for(&group.handler_id),
                            module_id: group.module_id.clone(),
                            task_id: group.task_id.clone(),
                            handler_id: group.handler_id.clone(),
                            result: Some(result),
                            error: None,
                        }
                    })
                })
                .collect(),
            cancelled: context.is_stopped(),
            stop_reason: context.stop_reason(),
        };
        if let Err(error) = observed.finished(&report) {
            report.operation_failures.push(PluginError::new(
                "execution-journal-finalization-failed",
                spaceprune_contracts::messages::message(
                    "engine.journal_finalize",
                    &[("error", (error).to_string())],
                ),
            ));
        }
        self.event_bus.emit(
            context,
            SpacePruneEvent::CleanupFinished {
                operation_id,
                group_count: report.groups.len(),
                attempted: attempted_total,
                succeeded: report.succeeded(),
                failure_count: report.failure_count(),
                reclaimed_bytes: report.reclaimed_bytes(),
                unknown_reclaimed_items: Some(report.unknown_reclaimed_items()),
                cancelled: report.cancelled,
                stop_reason: report.stop_reason,
            },
        );
        report
    }
}

fn not_attempted(group: &PlanGroup) -> HandlerCleanResult {
    let mut result = HandlerCleanResult {
        unknown_reclaimed_items: Some(0),
        ..Default::default()
    };
    for item in &group.items {
        result.record_outcome(spaceprune_plugin_api::ItemExecution {
            item_id: item.id.clone(),
            status: spaceprune_plugin_api::ItemExecutionStatus::NotAttempted,
            reclaimed_estimate: Some(0),
            reclaimed_lower_bound: 0,
            failure: None,
        });
    }
    result
}

fn rejected(plan: &CleanupPlan, error: &str) -> ExecutionReport {
    ExecutionReport {
        operation_failures: Vec::new(),
        cancelled: false,
        stop_reason: None,
        groups: plan
            .groups
            .iter()
            .map(|group| GroupExecutionReport {
                failure: None,
                size_accounting: Default::default(),
                module_id: group.module_id.clone(),
                task_id: group.task_id.clone(),
                handler_id: group.handler_id.clone(),
                result: None,
                error: Some(error.to_owned()),
            })
            .collect(),
    }
}
