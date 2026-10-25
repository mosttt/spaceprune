use std::{os::unix::fs::PermissionsExt, sync::Mutex};

use spaceprune_engine::{CleanupPlan, Engine, EngineBuilder, PlanGroup, SpacePruneEvent};
use spaceprune_plugin_api::NullProgress;

use super::*;

fn fixture_plan(groups: Vec<PlanGroup>) -> CleanupPlan {
    let report = spaceprune_engine::ScanReport::from_report(spaceprune_engine::ScanReportData {
        scan_id: spaceprune_engine::ScanReport::new_scan_id(),
        config_name: "fixture".to_owned(),
        config_source: "fixture".to_owned(),
        cancelled: false,
        stop_reason: None,
        modules: groups
            .into_iter()
            .map(|group| spaceprune_engine::ModuleReport {
                id: group.module_id.clone(),
                name: "fixture".to_owned(),
                description: String::new(),
                tasks: vec![spaceprune_engine::TaskReport {
                    module_id: group.module_id,
                    task_id: group.task_id,
                    name: "fixture".to_owned(),
                    handler_id: group.handler_id,
                    request: group.request,
                    items: group.items,
                    issues: Vec::new(),
                    error: None,
                }],
            })
            .collect(),
    });
    report.plan_selected(&report.selectable_keys()).unwrap()
}

struct Fixture {
    _directory: tempfile::TempDir,
    program: PathBuf,
    records: PathBuf,
    calls: PathBuf,
    behavior: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("snapper-fixture");
        let records = directory.path().join("snapshots.json");
        let calls = directory.path().join("calls.jsonl");
        let behavior = directory.path().join("behavior.txt");
        let snapshots: Vec<_> = (0..=99)
            .map(|number| {
                json!({
                    "number": number, "type": "single", "date": "2001-02-03 04:05:06"
                })
            })
            .collect();
        fs::write(
            &records,
            json!({"home": snapshots, "root": snapshots}).to_string(),
        )
        .unwrap();
        fs::write(&behavior, "success").unwrap();
        let script = format!(
            r#"#!/usr/bin/env python3
import json, pathlib, sys
args = sys.argv[1:]
records = pathlib.Path({records})
behavior = pathlib.Path({behavior}).read_text()
with open({calls}, 'a') as target:
    target.write(json.dumps(args) + '\n')
state = json.loads(records.read_text())
if 'list' in args:
    print(json.dumps(state))
elif 'list-configs' in args:
    if behavior == 'config-error':
        sys.exit(13)
    print(json.dumps({{'configs': [{{'config': name, 'subvolume': ({subvolume} if behavior != 'missing-volume' else {subvolume} + '/missing')}} for name in state]}}))
elif 'delete' in args:
    config = args[args.index('-c') + 1]
    selected = [int(value) for value in args[args.index('--sync') + 1:]]
    removed = selected[:1] if behavior == 'partial' else selected
    state[config] = [entry for entry in state[config] if entry['number'] not in removed]
    records.write_text(json.dumps(state))
    if behavior == 'partial':
        print('fixture partial deletion failure', file=sys.stderr)
        sys.exit(9)
else:
    sys.exit(8)
"#,
            subvolume = serde_json::to_string(directory.path()).unwrap(),
            records = serde_json::to_string(&records).unwrap(),
            behavior = serde_json::to_string(&behavior).unwrap(),
            calls = serde_json::to_string(&calls).unwrap()
        );
        fs::write(&program, script).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            _directory: directory,
            program,
            records,
            calls,
            behavior,
        }
    }

    fn group(&self, index: usize, config: &str, number: u64) -> PlanGroup {
        let task_id = format!("task-{index}");
        let task = TaskRequest {
            module_id: "snapshots".to_owned(),
            task_id: task_id.clone(),
            name: task_id.clone(),
            handler_id: "builtin.snapper/snapshots".to_owned(),
            options: json!({
                "configs": [config], "scan-mode": "snapshots", "snapper": self.program,
                "elevation": "never", "snapshot-ids": [number],
                "snapshot-host-id": snapper_host_id().unwrap(),
                "snapshot-identities": [{"number": number, "type": "single", "date": "2001-02-03 04:05:06"}]
            }),
            recommend: json!({"enabled": true}),
            tags: Vec::new(),
        };
        let snapshot: Snapshot = serde_json::from_value(
            json!({"number": number, "type": "single", "date": "2001-02-03 04:05:06"}),
        )
        .unwrap();
        let (_, recommend) = parse(&task).unwrap();
        let item = make_snapshot_item(
            config,
            &SnapshotGroup {
                snapshots: vec![snapshot],
            },
            &recommend,
            0,
            &BTreeSet::new(),
        );
        PlanGroup {
            module_id: task.module_id.clone(),
            task_id,
            handler_id: task.handler_id.clone(),
            request: task,
            items: vec![item],
        }
    }

    fn calls(&self) -> Vec<Value> {
        fs::read_to_string(&self.calls)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn engine(&self) -> Engine {
        let config =
            serde_json::from_value(json!({"schema-version": 2, "name": "batch-fixture"})).unwrap();
        let mut registry = PluginRegistry::new();
        register(&mut registry).unwrap();
        EngineBuilder::new(config)
            .with_registry(registry)
            .build()
            .unwrap()
    }
}

#[test]
fn thirteen_recipe_tasks_share_one_exact_deletion_and_keep_ordered_results() {
    #[derive(Default)]
    struct Sizes(Mutex<BTreeMap<String, spaceprune_engine::CleanupSizeSummary>>);
    impl ProgressSink for Sizes {
        fn emit(&self, _: ProgressEvent) {}
        fn record_cleanup_with_accounting(
            &self,
            task: &TaskRequest,
            accounting: spaceprune_plugin_api::CleanupSizeAccounting,
            succeeded: usize,
            bytes: u64,
            unknown: usize,
        ) {
            self.0.lock().unwrap().insert(
                task.task_id.clone(),
                spaceprune_engine::CleanupSizeSummary::from_progress(
                    accounting, succeeded, bytes, unknown,
                ),
            );
        }
    }
    let fixture = Fixture::new();
    let plan = fixture_plan(
        (1..=13)
            .map(|number| fixture.group(number, "home", number as u64))
            .collect(),
    );
    let engine = fixture.engine();
    let finished = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&finished);
    engine
        .event_bus()
        .subscribe(Arc::new(move |event: &spaceprune_engine::EventEnvelope| {
            if let SpacePruneEvent::CleanupGroupFinished { task_id, .. } = &event.event {
                observed.lock().unwrap().push(task_id.clone());
            }
        }));
    let sizes = Sizes::default();
    let report = engine.execute(&plan, &sizes, &OperationContext::default());
    assert!(!report.has_errors(), "{report:?}");
    assert_eq!(report.groups.len(), 13);
    assert_eq!(report.succeeded(), 13);
    assert_eq!(report.reclaimed_bytes(), 0);
    assert_eq!(report.unknown_reclaimed_items(), 13);
    assert_eq!(report.size_summary().snapshot_group_count, 13);
    assert_eq!(report.size_summary().unknown_size_item_count, 0);
    let mut live = spaceprune_engine::CleanupSizeSummary::default();
    for summary in sizes.0.lock().unwrap().values() {
        live.merge(summary);
    }
    let mut final_sizes = report.size_summary();
    assert_eq!(final_sizes.snapshot_space_changes.len(), 1);
    let measured = final_sizes.snapshot_space_changes.pop().unwrap();
    assert_eq!(measured.scope, "home");
    assert!(measured.free_bytes_before.is_some());
    assert!(measured.free_bytes_after.is_some());
    assert!(measured.error.is_none());
    assert_eq!(live, final_sizes);
    let restored: spaceprune_engine::ExecutionReport =
        serde_json::from_value(serde_json::to_value(&report).unwrap()).unwrap();
    assert_eq!(restored.size_summary(), report.size_summary());
    let expected: Vec<_> = plan
        .groups()
        .iter()
        .map(|group| group.task_id.clone())
        .collect();
    assert_eq!(
        report
            .groups
            .iter()
            .map(|group| group.task_id.clone())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(*finished.lock().unwrap(), expected);
    let calls = fixture.calls();
    assert_eq!(
        calls.len(),
        3,
        "One shared validation, one configuration lookup and one deletion are sufficient"
    );
    let mut expected_args = vec![
        "-c".to_owned(),
        "home".to_owned(),
        "delete".to_owned(),
        "--sync".to_owned(),
    ];
    expected_args.extend((1..=13).map(|number| number.to_string()));
    assert_eq!(calls[2], json!(expected_args));
    let remaining: Value =
        serde_json::from_str(&fs::read_to_string(&fixture.records).unwrap()).unwrap();
    assert!(
        remaining["home"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["number"] == 99)
    );
    assert!(
        remaining["home"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["number"] == 0)
    );
}

#[test]
fn unavailable_space_measurement_does_not_block_an_existing_fixed_snapshot_cleanup() {
    for behavior in ["missing-volume", "config-error"] {
        let fixture = Fixture::new();
        fs::write(&fixture.behavior, behavior).unwrap();
        let plan = fixture_plan(vec![fixture.group(1, "home", 1)]);
        let report = fixture
            .engine()
            .execute(&plan, &NullProgress, &OperationContext::default());
        assert!(!report.has_errors(), "{report:?}");
        assert_eq!(report.succeeded(), 1);
        let summary = report.size_summary();
        assert_eq!(summary.unknown_size_item_count, 0);
        assert_eq!(summary.snapshot_space_changes.len(), 1);
        assert_eq!(summary.snapshot_space_changes[0].free_bytes_before, None);
        assert_eq!(summary.snapshot_space_changes[0].free_bytes_after, None);
        assert!(summary.snapshot_space_changes[0].error.is_some());
    }
}

#[test]
fn failed_batch_reports_removed_prefix_without_deletion_retries() {
    let fixture = Fixture::new();
    fs::write(&fixture.behavior, "partial").unwrap();
    let plan = fixture_plan(vec![
        fixture.group(1, "home", 1),
        fixture.group(2, "home", 2),
    ]);
    let report = fixture
        .engine()
        .execute(&plan, &NullProgress, &OperationContext::default());
    assert_eq!(report.succeeded(), 1);
    assert_eq!(report.unknown_reclaimed_items(), 1);
    assert_eq!(report.groups[0].result.as_ref().unwrap().succeeded, 1);
    assert_eq!(report.groups[1].result.as_ref().unwrap().failures.len(), 1);
    assert_eq!(
        report.groups[1]
            .result
            .as_ref()
            .unwrap()
            .unknown_reclaimed_items(),
        0
    );
    let calls = fixture.calls();
    assert_eq!(calls.len(), 4, "Failure adds only a read-only verification");
    assert_eq!(
        calls
            .iter()
            .filter(|args| args.as_array().unwrap().iter().any(|arg| arg == "delete"))
            .count(),
        1
    );
}

#[test]
fn duplicate_selections_are_deleted_once_and_conflicting_identities_are_rejected() {
    let fixture = Fixture::new();
    let mut groups = vec![fixture.group(1, "home", 1), fixture.group(2, "home", 1)];
    groups[1].request.options["snapshot-identities"][0]["date"] = json!("2001-02-04 04:05:06");
    let plan = fixture_plan(groups.clone());
    let report = fixture
        .engine()
        .execute(&plan, &NullProgress, &OperationContext::default());
    assert_eq!(report.succeeded(), 0);
    assert!(
        report
            .groups
            .iter()
            .all(|group| group.result.as_ref().unwrap().failures.len() == 1)
    );
    assert_eq!(fixture.calls().len(), 1);
    groups[1].request.options["snapshot-identities"][0]["date"] = json!("2001-02-03 04:05:06");
    let plan = fixture_plan(groups);
    let report = fixture
        .engine()
        .execute(&plan, &NullProgress, &OperationContext::default());
    assert_eq!(report.succeeded(), 2);
    assert_eq!(
        fixture.calls().last().unwrap(),
        &json!(["-c", "home", "delete", "--sync", "1"])
    );
}

struct CancelOnPhase {
    context: OperationContext,
    after_command: bool,
}

impl ProgressSink for CancelOnPhase {
    fn emit(&self, event: ProgressEvent) {
        if (!self.after_command && event.message.starts_with("Deleting "))
            || (self.after_command && event.message.starts_with("Finished snapshot deletion"))
        {
            self.context.cancel();
        }
    }
}

#[test]
fn cancellation_before_deletion_starts_no_cleanup_command() {
    let fixture = Fixture::new();
    let mut group = fixture.group(1, "home", 1);
    group.request.options["elevation"] = json!("pkexec");
    let context = OperationContext::default();
    let sink = CancelOnPhase {
        context: context.clone(),
        after_command: false,
    };
    let report = fixture
        .engine()
        .execute(&fixture_plan(vec![group]), &sink, &context);
    assert!(report.cancelled);
    assert_eq!(report.groups[0].result.as_ref().unwrap().attempted, 0);
    assert!(report.size_summary().snapshot_space_changes.is_empty());
    assert_eq!(fixture.calls().len(), 1);
}

#[test]
fn cancellation_after_one_configuration_preserves_its_result_and_skips_the_next() {
    let fixture = Fixture::new();
    let context = OperationContext::default();
    let sink = CancelOnPhase {
        context: context.clone(),
        after_command: true,
    };
    let plan = fixture_plan(vec![
        fixture.group(1, "home", 1),
        fixture.group(2, "root", 2),
    ]);
    let report = fixture.engine().execute(&plan, &sink, &context);
    assert!(report.cancelled);
    assert_eq!(report.groups[0].result.as_ref().unwrap().succeeded, 1);
    assert_eq!(report.groups[1].result.as_ref().unwrap().attempted, 0);
    assert_eq!(report.size_summary().snapshot_space_changes.len(), 1);
    assert_eq!(
        fixture
            .calls()
            .iter()
            .filter(|args| args.as_array().unwrap().iter().any(|arg| arg == "delete"))
            .count(),
        1
    );
}

fn dynamic_task(fixture: &Fixture) -> TaskRequest {
    let mut task = fixture.group(1, "home", 1).request;
    for key in ["snapshot-ids", "snapshot-host-id", "snapshot-identities"] {
        task.options.as_object_mut().unwrap().remove(key);
    }
    task.options["snapshot-types"] = json!(["single"]);
    task.options["cleanup-algorithms"] = json!([]);
    task.recommend = json!({"enabled": true, "keep-latest": 0, "older-than-days": 30, "min-snapshot-size": "0B"});
    task
}

#[test]
fn reusable_recipe_refreshes_inventory_but_confirmation_keeps_its_exact_scan() {
    let fixture = Fixture::new();
    let task = dynamic_task(&fixture);
    let initial = SnapperHandler
        .scan(&task, &NullProgress, &OperationContext::default())
        .unwrap();
    let selected = initial
        .items
        .iter()
        .find(|item| item.id == "home:1")
        .unwrap()
        .clone();
    assert!(selected.opaque.get("scan_identity").is_some());
    let mut state: Value =
        serde_json::from_str(&fs::read_to_string(&fixture.records).unwrap()).unwrap();
    state["home"][1]["date"] = json!("2002-02-03 04:05:06");
    fs::write(&fixture.records, state.to_string()).unwrap();
    let stale = SnapperHandler
        .clean(
            &task,
            std::slice::from_ref(&selected),
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    assert_eq!(stale.succeeded, 0);
    assert!(stale.failures[0].message.contains("after this scan"));
    let fresh = SnapperHandler
        .scan(&task, &NullProgress, &OperationContext::default())
        .unwrap();
    let confirmed = fresh
        .items
        .iter()
        .find(|item| item.id == "home:1")
        .unwrap()
        .clone();
    state["home"]
        .as_array_mut()
        .unwrap()
        .push(json!({"number": 100, "type": "single", "date": "2001-02-03 04:05:06"}));
    fs::write(&fixture.records, state.to_string()).unwrap();
    let cleaned = SnapperHandler
        .clean(
            &task,
            &[confirmed],
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    assert_eq!(cleaned.succeeded, 1);
    assert_eq!(
        fixture.calls().last().unwrap(),
        &json!(["-c", "home", "delete", "--sync", "1"])
    );
    let rescanned = SnapperHandler
        .scan(&task, &NullProgress, &OperationContext::default())
        .unwrap();
    assert!(rescanned.items.iter().all(|item| item.id != "home:1"));
    assert!(rescanned.items.iter().any(|item| item.id == "home:100"));
    assert!(rescanned.items.iter().any(|item| item.id == "home:2"));
    assert!(task.options.get("snapshot-ids").is_none());
    let fixed = fixture.group(2, "home", 1).request;
    let stale_fixed = SnapperHandler
        .scan(&fixed, &NullProgress, &OperationContext::default())
        .unwrap();
    assert_eq!(
        stale_fixed.issues[0].code.as_deref(),
        Some("snapshot-identity-changed")
    );
    assert!(
        stale_fixed
            .items
            .iter()
            .all(|item| item.recommendation == Recommendation::Unavailable)
    );
    assert!(
        stale_fixed.issues[0]
            .message
            .contains("create a new reusable-rule Recipe")
    );
    assert_eq!(fixed.options["snapshot-ids"], json!([1]));
}

#[test]
fn reusable_snapshot_cleanup_rejects_missing_host_config_and_changed_label_bindings() {
    let fixture = Fixture::new();
    let mut task = dynamic_task(&fixture);
    task.options["cleanup-algorithms"] = json!([""]);
    let scan = SnapperHandler
        .scan(&task, &NullProgress, &OperationContext::default())
        .unwrap();
    let selected = scan
        .items
        .iter()
        .find(|item| item.id == "home:1")
        .unwrap()
        .clone();
    let mut missing = selected.clone();
    missing
        .opaque
        .as_object_mut()
        .unwrap()
        .remove("scan_identity");
    let mut wrong_host = selected.clone();
    wrong_host.opaque["scan_identity"]["host_id"] = json!("other-host");
    let mut wrong_config = selected.clone();
    wrong_config.opaque["scan_identity"]["subvolume"] = json!("/different-config");
    for invalid in [missing, wrong_host, wrong_config] {
        let result = SnapperHandler
            .clean(
                &task,
                &[invalid],
                &NullProgress,
                &OperationContext::default(),
            )
            .unwrap();
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failures.len(), 1);
    }
    let mut state: Value =
        serde_json::from_str(&fs::read_to_string(&fixture.records).unwrap()).unwrap();
    state["home"][1]["cleanup"] = json!("timeline");
    fs::write(&fixture.records, state.to_string()).unwrap();
    let changed = SnapperHandler
        .clean(
            &task,
            &[selected],
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    assert!(
        changed.failures[0]
            .message
            .contains("cleanup algorithm filters")
    );
    assert!(
        !fixture.calls().iter().any(|args| args
            .as_array()
            .unwrap()
            .iter()
            .any(|arg| arg == "delete"))
    );
}

#[test]
fn vanished_selection_is_zero_sized_and_partial_pair_is_not_reported_as_untouched() {
    let fixture = Fixture::new();
    let fixed = fixture.group(1, "home", 1);
    let mut state: Value =
        serde_json::from_str(&fs::read_to_string(&fixture.records).unwrap()).unwrap();
    state["home"]
        .as_array_mut()
        .unwrap()
        .retain(|item| item["number"] != 1);
    fs::write(&fixture.records, state.to_string()).unwrap();
    let absent = SnapperHandler
        .clean(
            &fixed.request,
            &fixed.items,
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    assert_eq!(absent.succeeded, 1);
    assert_eq!(absent.reclaimed_bytes, 0);
    assert_eq!(absent.unknown_reclaimed_items(), 0);
    assert_eq!(
        absent.outcomes[0].status,
        ItemExecutionStatus::AlreadyAbsent
    );
    assert!(
        !fixture.calls().iter().any(|args| args
            .as_array()
            .unwrap()
            .iter()
            .any(|arg| arg == "delete"))
    );

    fs::write(
        &fixture.records,
        json!({"home": [
            {"number": 0},
            {"number": 2, "type": "pre", "date": "2001-02-03 04:05:06"},
            {"number": 3, "type": "post", "pre-number": 2, "date": "2001-02-03 04:06:06"}
        ]})
        .to_string(),
    )
    .unwrap();
    let mut task = dynamic_task(&fixture);
    task.options["snapshot-types"] = json!(["pre/post"]);
    let scan = SnapperHandler
        .scan(&task, &NullProgress, &OperationContext::default())
        .unwrap();
    let pair = scan
        .items
        .iter()
        .find(|item| item.id == "home:2+3")
        .unwrap();
    fs::write(&fixture.behavior, "partial").unwrap();
    let result = SnapperHandler
        .clean(
            &task,
            std::slice::from_ref(pair),
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    assert_eq!(result.succeeded, 0);
    assert_eq!(result.attempted, 1);
    assert_eq!(
        result.outcomes[0].status,
        ItemExecutionStatus::PartiallyApplied
    );
    assert_eq!(result.outcomes[0].reclaimed_estimate, None);
    assert_eq!(result.unknown_reclaimed_items(), 1);
    assert_eq!(
        fixture
            .calls()
            .iter()
            .filter(|args| args.as_array().unwrap().iter().any(|arg| arg == "delete"))
            .count(),
        1
    );
}

#[test]
fn reusable_filters_keep_pairs_intact_and_count_retention_is_only_recommendation() {
    let fixture = Fixture::new();
    fs::write(&fixture.records, json!({"home": [
            {"number": 0},
            {"number": 1, "type": "single", "date": "2001-02-03 04:05:06", "cleanup": "timeline"},
            {"number": 2, "type": "pre", "date": "2001-02-03 04:05:06", "cleanup": "number"},
            {"number": 3, "type": "post", "pre-number": 2, "date": "2001-02-03 04:06:06", "cleanup": "number"}
        ]}).to_string()).unwrap();
    let mut task = dynamic_task(&fixture);
    task.options["snapshot-types"] = json!(["pre/post"]);
    task.options["cleanup-algorithms"] = json!(["number"]);
    task.recommend["keep-latest"] = json!(1);
    let scan = SnapperHandler
        .scan(&task, &NullProgress, &OperationContext::default())
        .unwrap();
    assert_eq!(scan.items.len(), 2);
    assert_eq!(scan.items[0].recommendation, Recommendation::Unavailable);
    assert_eq!(scan.items[1].id, "home:2+3");
    assert_eq!(scan.items[1].recommendation, Recommendation::Optional);
    let mut split = scan.items[1].clone();
    split.id = "home:2".to_owned();
    split.opaque["numbers"] = json!([2]);
    let rejected = SnapperHandler
        .clean(&task, &[split], &NullProgress, &OperationContext::default())
        .unwrap();
    assert_eq!(rejected.succeeded, 0);
    let cleaned = SnapperHandler
        .clean(
            &task,
            &scan.items[1..],
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    assert_eq!(cleaned.succeeded, 1);
    assert_eq!(
        fixture.calls().last().unwrap(),
        &json!(["-c", "home", "delete", "--sync", "2", "3"])
    );
}

#[test]
fn inaccessible_inventory_program_keeps_native_failure_in_the_cleanup_receipt() {
    let fixture = Fixture::new();
    let group = fixture.group(0, "home", 12);
    fs::set_permissions(&fixture.program, fs::Permissions::from_mode(0o600)).unwrap();
    let reports = clean_batch(
        &[CleanupBatchGroup {
            task: &group.request,
            items: &group.items,
        }],
        &NullProgress,
        &OperationContext::default(),
    );
    let report = reports[0].as_ref().unwrap();
    assert_eq!(report.succeeded, 0);
    assert_eq!(report.reclaimed_bytes, 0);
    assert_eq!(report.unknown_reclaimed_items(), 0);
    assert_eq!(report.failures.len(), 1);
    let failure = &report.failures[0];
    assert_eq!(
        failure.io_failure.as_ref().unwrap().kind,
        spaceprune_access::IoFailureKind::AccessDenied
    );
    assert_eq!(failure.io_failure.as_ref().unwrap().os_code, Some(13));
    assert_eq!(failure.authorization, None);
    assert!(
        failure
            .message
            .starts_with("Could not revalidate selected snapshots:")
    );
    assert_eq!(report.outcomes[0].failure.as_ref(), Some(failure));
    assert!(fixture.calls().is_empty());
}
