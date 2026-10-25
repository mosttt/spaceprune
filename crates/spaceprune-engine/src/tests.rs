use std::{
    fs,
    sync::{Arc, Mutex},
};

use serde_json::json;
use spaceprune_config::{ConfigLoader, ResolvedConfig};
use spaceprune_plugin_api::{HandlerScanResult, PluginResult, Recommendation};
use tempfile::tempdir;

use super::*;

#[derive(Debug)]
struct FakeHandler;

fn test_config() -> ResolvedConfig {
    let temp = tempdir().unwrap();
    let path = temp.path().join("config.toml");
    fs::write(
        &path,
        r#"
schema-version = 2
name = "test"

[modules.code]
name = "Code"

[modules.code.selectors.rust]
name = "Rust"
match-tags = ["rust"]

[modules.code.tasks.build]
name = "Build output"
handler = "builtin.filesystem/paths"
tags = ["rust"]
"#,
    )
    .unwrap();
    ConfigLoader::new().load_path(path).unwrap()
}

impl Handler for FakeHandler {
    fn scan_unprivileged(
        &self,
        task: &TaskRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult> {
        let mut result = self.scan(task, progress, context)?;
        result.items[0].reason = "Host-managed scan".to_owned();
        Ok(result)
    }

    fn descriptor(&self) -> HandlerDescriptor {
        HandlerDescriptor {
            size_accounting: Default::default(),
            id: "builtin.filesystem/paths".to_owned(),
            name: "fake".to_owned(),
            description: String::new(),
            platforms: Vec::new(),
            options_schema: serde_json::Value::Null,
            recommend_schema: serde_json::Value::Null,
        }
    }

    fn validate(&self, _task: &TaskRequest) -> PluginResult<()> {
        Ok(())
    }

    fn scan(
        &self,
        _task: &TaskRequest,
        _progress: &dyn ProgressSink,
        _cancellation: &OperationContext,
    ) -> PluginResult<HandlerScanResult> {
        Ok(HandlerScanResult {
            items: vec![
                CleanupItem {
                    logical_size_applicability: Default::default(),
                    id: "one".to_owned(),
                    parent_id: None,
                    name: "one".to_owned(),
                    location: "/tmp/one".to_owned(),
                    logical_bytes: Some(100),
                    reclaimable_bytes: Some(100),
                    modified_at: None,
                    recommendation: Recommendation::Recommended,
                    reason: "test".to_owned(),
                    tags: vec!["rust".to_owned()],
                    opaque: json!({"id": 1}),
                },
                CleanupItem {
                    logical_size_applicability: Default::default(),
                    id: "optional".to_owned(),
                    parent_id: None,
                    name: "optional".to_owned(),
                    location: "/tmp/optional".to_owned(),
                    logical_bytes: Some(50),
                    reclaimable_bytes: Some(50),
                    modified_at: None,
                    recommendation: Recommendation::Optional,
                    reason: "test".to_owned(),
                    tags: Vec::new(),
                    opaque: json!({"id": 2}),
                },
                CleanupItem {
                    logical_size_applicability: Default::default(),
                    id: "unavailable".to_owned(),
                    parent_id: None,
                    name: "unavailable".to_owned(),
                    location: "/tmp/unavailable".to_owned(),
                    logical_bytes: None,
                    reclaimable_bytes: None,
                    modified_at: None,
                    recommendation: Recommendation::Unavailable,
                    reason: "test".to_owned(),
                    tags: Vec::new(),
                    opaque: json!({"id": 3}),
                },
            ],
            issues: Vec::new(),
        })
    }

    fn clean(
        &self,
        _task: &TaskRequest,
        items: &[CleanupItem],
        _progress: &dyn ProgressSink,
        _cancellation: &OperationContext,
    ) -> PluginResult<HandlerCleanResult> {
        Ok(HandlerCleanResult {
            space_changes: Vec::new(),
            outcomes: Vec::new(),
            attempted: items.len(),
            succeeded: items.len(),
            reclaimed_bytes: 100,
            unknown_reclaimed_items: Some(0),
            failures: Vec::new(),
        })
    }
}

fn filesystem_plan_group(task_id: &str, path: &std::path::Path, bytes: u64) -> PlanGroup {
    let path = path.to_string_lossy().into_owned();
    PlanGroup {
        module_id: "cache".to_owned(),
        task_id: task_id.to_owned(),
        handler_id: "builtin.filesystem/paths".to_owned(),
        request: TaskRequest {
            module_id: "cache".to_owned(),
            task_id: task_id.to_owned(),
            name: task_id.to_owned(),
            handler_id: "builtin.filesystem/paths".to_owned(),
            options: json!({}),
            recommend: json!({}),
            tags: Vec::new(),
        },
        items: vec![CleanupItem {
            logical_size_applicability: Default::default(),
            id: path.clone(),
            parent_id: None,
            name: task_id.to_owned(),
            location: path.clone(),
            logical_bytes: Some(bytes),
            reclaimable_bytes: Some(bytes),
            modified_at: None,
            recommendation: Recommendation::Recommended,
            reason: "test".to_owned(),
            tags: Vec::new(),
            opaque: json!({"path": path, "root-index": 0}),
        }],
    }
}

#[test]
fn plan_keeps_only_the_outermost_selected_filesystem_path() {
    let root = std::env::temp_dir().join("spaceprune-overlap-fixture");
    let mut groups = vec![
        filesystem_plan_group("child", &root.join("child"), 40),
        filesystem_plan_group("parent", &root, 100),
    ];

    deduplicate_filesystem_plan_items(&mut groups);

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].task_id, "parent");
    assert_eq!(groups[0].items.len(), 1);
    assert_eq!(groups[0].items[0].reclaimable_bytes, Some(100));
}

#[test]
fn large_selection_preserves_siblings_and_deduplicates_only_path_components() {
    let root = std::env::temp_dir().join("spaceprune-large-selection");
    let mut group = filesystem_plan_group("files", &root.join("cache"), 1);
    for index in 0..20_000 {
        group
            .items
            .extend(filesystem_plan_group("files", &root.join(format!("cache-{index}")), 1).items);
    }
    group
        .items
        .extend(filesystem_plan_group("files", &root.join("cache/child"), 1).items);
    let duplicate = filesystem_plan_group("duplicate", &root.join("cache-0"), 1);
    let mut groups = vec![group, duplicate];
    deduplicate_filesystem_plan_items(&mut groups);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].items.len(), 20_001);
    assert_eq!(
        groups[0].items[0].location,
        root.join("cache").to_string_lossy()
    );
    assert_eq!(
        groups[0].items.last().unwrap().location,
        root.join("cache-19999").to_string_lossy()
    );
}

#[test]
fn caller_owned_scan_retry_still_passes_engine_validation() {
    struct InvalidRetry;
    impl ScanAccess for InvalidRetry {
        fn manages_authorization(&self, _task: &TaskRequest) -> bool {
            true
        }

        fn retry(
            &self,
            task: &TaskRequest,
            mut result: HandlerScanResult,
            _progress: &dyn ProgressSink,
            _cancellation: &OperationContext,
        ) -> PluginResult<HandlerScanResult> {
            assert_eq!(task.handler_id, "builtin.filesystem/paths");
            assert_eq!(result.items[0].reason, "Host-managed scan");
            result.items.push(result.items[0].clone());
            Ok(result)
        }
    }
    let mut registry = PluginRegistry::new();
    registry.register(Arc::new(FakeHandler)).unwrap();
    let engine = EngineBuilder::new(test_config())
        .with_registry(registry)
        .build()
        .unwrap();
    let report = engine
        .scan_with_access(
            &ScanRequest::default(),
            &NullProgress,
            &OperationContext::default(),
            Some(&InvalidRetry),
        )
        .unwrap();
    assert!(report.has_errors());
    assert!(report.selectable_keys().is_empty());
    assert!(
        report.modules[0].tasks[0]
            .error
            .as_ref()
            .unwrap()
            .contains("duplicate item")
    );
}

#[test]
fn scans_plans_and_executes_recommended_items() {
    let config = test_config();
    let mut registry = PluginRegistry::new();
    registry.register(Arc::new(FakeHandler)).unwrap();
    let engine = EngineBuilder::new(config)
        .with_registry(registry)
        .build()
        .unwrap();

    let report = engine
        .scan(
            &ScanRequest {
                module: Some("code".to_owned()),
                selectors: BTreeSet::from(["rust".to_owned()]),
                tasks: BTreeSet::new(),
            },
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    let plan = report.plan_recommended().unwrap();
    assert_eq!(plan.item_count(), 1);
    let all = report.plan_selected(&report.selectable_keys()).unwrap();
    assert_eq!(all.item_count(), 2);
    let result = engine.execute(&plan, &NullProgress, &OperationContext::default());
    assert_eq!(result.succeeded(), 1);

    let error = engine
        .scan(
            &ScanRequest {
                module: Some("code".to_owned()),
                selectors: BTreeSet::new(),
                tasks: BTreeSet::from(["missing".to_owned()]),
            },
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap_err();
    assert!(matches!(error, EngineError::UnknownTask { .. }));
}

#[test]
fn cleanup_size_aggregation_counts_only_successes_and_validates_metadata() {
    let mut result = HandlerCleanResult::default();
    result.record_success(Some(0));
    result.record_success(None);
    result.record_success(Some(1024));
    result.attempted += 1;
    result.failures.push(spaceprune_plugin_api::ItemFailure {
        authorization: None,
        io_failure: None,
        item_id: "failed".to_owned(),
        message: "fixture".to_owned(),
    });
    let report = ExecutionReport {
        operation_failures: Vec::new(),
        cancelled: true,
        stop_reason: None,
        groups: vec![
            GroupExecutionReport {
                failure: None,
                size_accounting: Default::default(),
                module_id: "fixture".to_owned(),
                task_id: "partial".to_owned(),
                handler_id: "fixture/cleanup".to_owned(),
                result: Some(result),
                error: None,
            },
            GroupExecutionReport {
                failure: None,
                size_accounting: Default::default(),
                module_id: "fixture".to_owned(),
                task_id: "unprocessed".to_owned(),
                handler_id: "fixture/cleanup".to_owned(),
                result: None,
                error: Some("fixture failure".to_owned()),
            },
        ],
    };
    assert_eq!(report.succeeded(), 3);
    assert_eq!(report.reclaimed_bytes(), 1024);
    assert_eq!(report.unknown_reclaimed_items(), 1);
    assert!(report.has_errors());
    let group = filesystem_plan_group(
        "fixture",
        std::path::Path::new("/fixture/never-accessed"),
        0,
    );
    let invalid = HandlerCleanResult {
        attempted: 1,
        succeeded: 1,
        unknown_reclaimed_items: Some(2),
        ..HandlerCleanResult::default()
    };
    assert!(invalid.validate_selected(&group.items, false).is_err());
}

#[test]
fn emits_ordered_scan_and_cleanup_lifecycle_events() {
    let config = test_config();
    let mut registry = PluginRegistry::new();
    registry.register(Arc::new(FakeHandler)).unwrap();
    let event_bus = EventBus::new();
    let names = Arc::new(Mutex::new(Vec::new()));
    let target = Arc::clone(&names);
    event_bus.subscribe_fn(move |envelope| {
        let name = match &envelope.event {
            SpacePruneEvent::ScanStarted { .. } => "scan-started",
            SpacePruneEvent::TaskScanStarted { .. } => "task-scan-started",
            SpacePruneEvent::TaskScanFinished { .. } => "task-scan-finished",
            SpacePruneEvent::ScanFinished { .. } => "scan-finished",
            SpacePruneEvent::CleanupStarted { .. } => "cleanup-started",
            SpacePruneEvent::CleanupGroupStarted { .. } => "cleanup-group-started",
            SpacePruneEvent::CleanupGroupFinished { .. } => "cleanup-group-finished",
            SpacePruneEvent::CleanupFinished { .. } => "cleanup-finished",
        };
        target
            .lock()
            .expect("test event lock poisoned")
            .push((envelope.sequence, name));
    });
    let engine = EngineBuilder::new(config)
        .with_registry(registry)
        .with_event_bus(event_bus)
        .build()
        .unwrap();
    let context = OperationContext::default();
    let report = engine
        .scan(&ScanRequest::default(), &NullProgress, &context)
        .unwrap();
    let plan = report.plan_recommended().unwrap();
    engine.execute(&plan, &NullProgress, &context);

    let names = names.lock().expect("test event lock poisoned");
    assert_eq!(
        names.iter().map(|(_, name)| *name).collect::<Vec<_>>(),
        vec![
            "scan-started",
            "task-scan-started",
            "task-scan-finished",
            "scan-finished",
            "cleanup-started",
            "cleanup-group-started",
            "cleanup-group-finished",
            "cleanup-finished",
        ]
    );
    assert_eq!(
        names
            .iter()
            .map(|(sequence, _)| *sequence)
            .collect::<Vec<_>>(),
        (1..=8).collect::<Vec<_>>()
    );
}

#[test]
fn concurrent_plans_from_the_same_scan_are_consumed_only_once() {
    let mut registry = PluginRegistry::new();
    registry.register(Arc::new(FakeHandler)).unwrap();
    let engine = EngineBuilder::new(test_config())
        .with_registry(registry)
        .build()
        .unwrap();
    let scan = engine
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    let first = scan.plan_recommended().unwrap();
    let second = scan.plan_recommended().unwrap();
    assert_eq!(first.scan_id(), second.scan_id());
    assert_ne!(first.id(), second.id());
    let results = std::thread::scope(|scope| {
        let first =
            scope.spawn(|| engine.execute(&first, &NullProgress, &OperationContext::default()));
        let second =
            scope.spawn(|| engine.execute(&second, &NullProgress, &OperationContext::default()));
        [first.join().unwrap(), second.join().unwrap()]
    });
    assert_eq!(
        results
            .iter()
            .map(ExecutionReport::succeeded)
            .sum::<usize>(),
        1
    );
    assert_eq!(
        results.iter().filter(|result| result.has_errors()).count(),
        1
    );
}

#[test]
fn execution_journal_is_committed_before_work_and_contains_the_frozen_plan() {
    let state = tempdir().unwrap();
    let mut registry = PluginRegistry::new();
    registry.register(Arc::new(FakeHandler)).unwrap();
    let engine = EngineBuilder::new(test_config())
        .with_registry(registry)
        .with_journal_directory(state.path().to_path_buf())
        .build()
        .unwrap();
    let scan = engine
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    let plan = scan.plan_recommended().unwrap();
    let report = engine.execute(&plan, &NullProgress, &OperationContext::default());
    assert!(!report.has_errors());
    let records = fs::read_to_string(state.path().join(format!("{}.jsonl", plan.id()))).unwrap();
    let records = records
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records[0]["event"], "started");
    assert_eq!(records[0]["plan"]["digest"], plan.digest());
    assert_eq!(records[1]["event"], "group_started");
    assert_eq!(records[2]["event"], "group_finished");
    assert_eq!(records.last().unwrap()["event"], "finished");
}

#[test]
fn an_unwritable_execution_journal_prevents_destructive_dispatch() {
    let state = tempdir().unwrap();
    let blocked = state.path().join("file");
    fs::write(&blocked, b"retain").unwrap();
    let mut registry = PluginRegistry::new();
    registry.register(Arc::new(FakeHandler)).unwrap();
    let engine = EngineBuilder::new(test_config())
        .with_registry(registry)
        .with_journal_directory(blocked.clone())
        .build()
        .unwrap();
    let scan = engine
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    let plan = scan.plan_recommended().unwrap();
    let report = engine.execute(&plan, &NullProgress, &OperationContext::default());
    assert!(report.has_errors());
    assert_eq!(report.succeeded(), 0);
    assert!(
        report.groups[0]
            .error
            .as_ref()
            .unwrap()
            .contains("execution-journal-unavailable")
    );
    assert_eq!(fs::read(blocked).unwrap(), b"retain");
}

mod execution;
mod operation;

#[test]
fn missing_handler_identifies_recipe_and_module_without_task_details() {
    let config = test_config();
    let source = config.source.clone();
    let error = EngineBuilder::new(config).build().unwrap_err();
    assert!(matches!(
        &error,
        EngineError::MissingHandler { configuration, module, handler }
            if configuration == &source && module == "Code" && handler == "builtin.filesystem/paths"
    ));
    let message = error.to_string();
    assert!(message.contains(&source));
    assert!(message.contains("module “Code”"));
    assert!(message.contains("is not registered"));
    assert!(!message.contains("Build output"));
}
