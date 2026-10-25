//! Scan orchestration and access retries.

use super::*;

struct ScheduledModule<'a> {
    id: &'a str,
    definition: &'a ModuleDefinition,
    tasks: Vec<(&'a str, &'a TaskDefinition)>,
    skipped: Vec<(&'a str, &'a TaskDefinition)>,
}

impl Engine {
    /// 扫描请求范围并生成结构化报告。
    pub fn scan(
        &self,
        request: &ScanRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> Result<ScanReport, EngineError> {
        self.scan_with_access(request, progress, context, None)
    }

    /// Scan with a caller-owned read-only access policy, preserving normal report validation.
    pub fn scan_with_access(
        &self,
        request: &ScanRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
        access: Option<&dyn ScanAccess>,
    ) -> Result<ScanReport, EngineError> {
        let operation_id = self.event_bus.next_operation_id();
        let scheduled = self.schedule_scan(request);
        let task_count = scheduled
            .as_ref()
            .ok()
            .map(|modules| modules.iter().map(|module| module.tasks.len()).sum());
        self.event_bus.emit(
            context,
            SpacePruneEvent::ScanStarted {
                task_count,
                operation_id,
                request: request.clone(),
            },
        );
        let result = scheduled.and_then(|modules| {
            self.scan_operation(operation_id, modules, progress, context, access)
        });
        let (module_count, task_count, item_count, issue_count, error_count, cancelled, error) =
            match &result {
                Ok(report) => {
                    let task_count = report.modules.iter().map(|module| module.tasks.len()).sum();
                    let item_count = report
                        .modules
                        .iter()
                        .flat_map(|module| &module.tasks)
                        .map(|task| task.items.len())
                        .sum();
                    let issue_count = report
                        .modules
                        .iter()
                        .flat_map(|module| &module.tasks)
                        .map(|task| task.issues.len())
                        .sum();
                    let error_count = report
                        .modules
                        .iter()
                        .flat_map(|module| &module.tasks)
                        .map(|task| {
                            usize::from(task.error.is_some())
                                + task
                                    .issues
                                    .iter()
                                    .filter(|issue| issue.level == IssueLevel::Error)
                                    .count()
                        })
                        .sum();
                    (
                        report.modules.len(),
                        task_count,
                        item_count,
                        issue_count,
                        error_count,
                        report.cancelled,
                        None,
                    )
                }
                Err(error) => (0, 0, 0, 0, 1, context.is_stopped(), Some(error.to_string())),
            };
        self.event_bus.emit(
            context,
            SpacePruneEvent::ScanFinished {
                operation_id,
                module_count,
                task_count,
                item_count,
                issue_count,
                error_count,
                cancelled,
                stop_reason: context.stop_reason(),
                error,
            },
        );
        result
    }

    fn schedule_scan<'a>(
        &'a self,
        request: &ScanRequest,
    ) -> Result<Vec<ScheduledModule<'a>>, EngineError> {
        let mut scheduled = Vec::new();
        for (module_id, module) in self.resolve_modules(request)? {
            let required_tags = selector_tags(module_id, module, &request.selectors)?;
            validate_task_selection(module_id, module, &request.tasks)?;
            let mut tasks: Vec<_> = module
                .tasks
                .iter()
                .filter(|(_, task)| task.enabled)
                .filter(|(_, task)| task_matches_platform(task))
                .filter(|(task_id, _)| {
                    request.tasks.is_empty() || request.tasks.contains(task_id.as_str())
                })
                .filter(|(_, task)| required_tags.iter().all(|tag| task.tags.contains(tag)))
                .map(|(id, task)| (id.as_str(), task))
                .collect();
            tasks.sort_by_key(|(id, task)| (task.order, *id));
            let (skipped, tasks): (Vec<_>, Vec<_>) = tasks
                .into_iter()
                .partition(|(_, task)| handler_disabled(&self.disabled_plugins, &task.handler));
            if !request.tasks.is_empty()
                && let Some((_, task)) = skipped.first()
            {
                return Err(EngineError::PluginDisabled(
                    task.handler
                        .split_once('/')
                        .expect("validated handler")
                        .0
                        .to_owned(),
                ));
            }

            scheduled.push(ScheduledModule {
                id: module_id,
                definition: module,
                tasks,
                skipped,
            });
        }
        Ok(scheduled)
    }

    fn scan_operation(
        &self,
        operation_id: OperationId,
        modules: Vec<ScheduledModule<'_>>,
        progress: &dyn ProgressSink,
        context: &OperationContext,
        access: Option<&dyn ScanAccess>,
    ) -> Result<ScanReport, EngineError> {
        let mut reports = Vec::new();
        for ScheduledModule {
            id: module_id,
            definition: module,
            tasks,
            skipped,
        } in modules
        {
            if context.is_stopped() {
                break;
            }
            for (task_id, task) in skipped {
                progress.emit(ProgressEvent {
                    phase: ProgressPhase::Scan,
                    module_id: Some(module_id.to_owned()),
                    task_id: Some(task_id.to_owned()),
                    handler_id: Some(task.handler.clone()),
                    current: None,
                    total: None,
                    message: spaceprune_contracts::messages::message(
                        "engine.skipping_disabled",
                        &[("p0", (task.name).to_string())],
                    ),
                });
            }
            let mut task_reports = Vec::new();
            for (task_id, task) in tasks {
                if context.is_stopped() {
                    break;
                }
                let task_request = build_task_request(module_id, task_id, task)?;
                let handler = self.registry.get(&task.handler).ok_or_else(|| {
                    EngineError::MissingHandler {
                        configuration: self.config.source.clone(),
                        module: module.name.clone(),
                        handler: task.handler.clone(),
                    }
                })?;
                self.event_bus.emit(
                    context,
                    SpacePruneEvent::TaskScanStarted {
                        operation_id,
                        module_id: module_id.to_owned(),
                        task_id: task_id.to_owned(),
                        handler_id: task.handler.clone(),
                    },
                );
                progress.emit(ProgressEvent {
                    phase: ProgressPhase::Scan,
                    module_id: Some(module_id.to_owned()),
                    task_id: Some(task_id.to_owned()),
                    handler_id: Some(task.handler.clone()),
                    current: None,
                    total: None,
                    message: spaceprune_contracts::messages::message(
                        "engine.scanning_task",
                        &[("p0", (task.name).to_string())],
                    ),
                });

                let mut report = TaskReport {
                    module_id: module_id.to_owned(),
                    task_id: task_id.to_owned(),
                    name: task.name.clone(),
                    handler_id: task.handler.clone(),
                    request: task_request.clone(),
                    items: Vec::new(),
                    issues: Vec::new(),
                    error: None,
                };
                let scanned =
                    if access.is_some_and(|access| access.manages_authorization(&task_request)) {
                        handler.scan_unprivileged(&task_request, progress, context)
                    } else {
                        handler.scan(&task_request, progress, context)
                    };
                let scanned = if let Some(access) = access {
                    access.retry_result(&task_request, scanned, progress, context)
                } else {
                    scanned
                };
                match scanned {
                    Ok(mut result) => {
                        result.items.sort_by(|left, right| {
                            left.name
                                .cmp(&right.name)
                                .then_with(|| left.id.cmp(&right.id))
                        });
                        report.issues = result.issues;
                        match validate_scan_items(result.items) {
                            Ok(items) => report.items = items,
                            Err(error) => report.error = Some(error),
                        }
                    }
                    Err(error) => report.error = Some(error.to_string()),
                }
                self.event_bus.emit(
                    context,
                    SpacePruneEvent::TaskScanFinished {
                        operation_id,
                        module_id: module_id.to_owned(),
                        task_id: task_id.to_owned(),
                        handler_id: task.handler.clone(),
                        item_count: report.items.len(),
                        issue_count: report.issues.len(),
                        error: report.error.clone(),
                    },
                );
                task_reports.push(report);
            }
            deduplicate_snapshot_inventory(&mut task_reports);
            reports.push(ModuleReport {
                id: module_id.to_owned(),
                name: module.name.clone(),
                description: module.description.clone(),
                tasks: task_reports,
            });
        }

        Ok(ScanReport::from_report(ScanReportData {
            scan_id: ScanReport::new_scan_id(),
            config_name: self.config.name.clone(),
            config_source: self.config.source.clone(),
            modules: reports,
            cancelled: context.is_stopped(),
            stop_reason: context.stop_reason(),
        }))
    }
}

fn deduplicate_snapshot_inventory(tasks: &mut [TaskReport]) {
    let mut owners = BTreeMap::<String, (usize, usize, u8)>::new();
    let mut duplicates = BTreeSet::new();
    for (task_index, task) in tasks.iter().enumerate() {
        if task.handler_id != "builtin.snapper/snapshots" {
            continue;
        }
        for (item_index, item) in task.items.iter().enumerate() {
            let (Some(config), Some(identity)) =
                (item.opaque.get("config"), item.opaque.get("scan_identity"))
            else {
                continue;
            };
            let key = serde_json::json!([config, identity]).to_string();
            let priority = if item
                .opaque
                .get("protected")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                3
            } else {
                match item.recommendation {
                    Recommendation::Recommended => 2,
                    Recommendation::Optional => 1,
                    Recommendation::Unavailable => 0,
                }
            };
            let owner = owners
                .entry(key)
                .or_insert((task_index, item_index, priority));
            if (owner.0, owner.1) == (task_index, item_index) {
                continue;
            }
            if priority > owner.2 {
                duplicates.insert((owner.0, owner.1));
                *owner = (task_index, item_index, priority);
            } else {
                duplicates.insert((task_index, item_index));
            }
        }
    }
    for (task_index, task) in tasks.iter_mut().enumerate() {
        let mut item_index = 0;
        task.items.retain(|_| {
            let keep = !duplicates.contains(&(task_index, item_index));
            item_index += 1;
            keep
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task(id: &str, config: &str, recommendation: Recommendation) -> TaskReport {
        let request = TaskRequest {
            module_id: "snapshots".into(),
            task_id: id.into(),
            name: id.into(),
            handler_id: "builtin.snapper/snapshots".into(),
            options: json!({"configs": [config]}),
            recommend: json!({}),
            tags: vec![],
        };
        TaskReport {
            module_id: request.module_id.clone(),
            task_id: id.into(),
            name: id.into(),
            handler_id: request.handler_id.clone(),
            request,
            items: vec![CleanupItem {
                logical_size_applicability: Default::default(),
                id: format!("{config}:1+2"),
                parent_id: None,
                name: "paired snapshot".into(),
                location: config.into(),
                logical_bytes: None,
                reclaimable_bytes: None,
                modified_at: None,
                recommendation,
                reason: "observed inventory".into(),
                tags: vec![],
                opaque: json!({"config":config, "numbers":[1,2], "scan_identity":{
                    "host_id":"host", "subvolume":format!("/{config}"),
                    "snapshots":[{"number":1,"date":"2026-01-01","type":"pre"},{"number":2,"date":"2026-01-01","type":"post","pre-number":1}]
                }}),
            }],
            issues: vec![HandlerIssue {
                code: Some("fixture".into()),
                io_failure: None,
                level: IssueLevel::Warning,
                message: id.into(),
                location: None,
            }],
            error: None,
        }
    }

    #[test]
    fn snapshot_inventory_deduplicates_identity_preserving_rules_and_protection() {
        let mut tasks = vec![
            task("stale", "root", Recommendation::Unavailable),
            task("valid", "root", Recommendation::Optional),
            task("other-config", "home", Recommendation::Optional),
        ];
        deduplicate_snapshot_inventory(&mut tasks);
        assert!(tasks[0].items.is_empty());
        assert_eq!(tasks[1].items.len(), 1);
        assert_eq!(tasks[2].items.len(), 1);
        assert!(tasks.iter().all(|task| task.issues.len() == 1));
        let mut replacement = task("changed", "root", Recommendation::Optional);
        replacement.items[0].opaque["scan_identity"]["snapshots"][0]["date"] = json!("2026-02-01");
        tasks.push(replacement);
        deduplicate_snapshot_inventory(&mut tasks);
        assert_eq!(tasks[3].items.len(), 1);
        let mut protected = task("active", "root", Recommendation::Unavailable);
        protected.items[0].opaque["protected"] = json!(true);
        tasks.push(protected);
        deduplicate_snapshot_inventory(&mut tasks);
        assert!(tasks[1].items.is_empty());
        assert_eq!(tasks[4].items.len(), 1);
        assert_eq!(
            tasks[4].items[0].recommendation,
            Recommendation::Unavailable
        );
        assert_eq!(tasks[4].items[0].opaque["numbers"], json!([1, 2]));
    }
}
