use super::*;
use serde_json::json;

fn fixture() -> (ExecutionReport, CleanupPlanData) {
    let failure = json!({"item_id":"same-id", "message":"localized OS message", "io_failure":{"kind":"sharing_violation", "os_code":32}});
    let report = serde_json::from_value(json!({
        "cancelled":false, "groups":[{"module_id":"cache", "task_id":"temp", "handler_id":"builtin.filesystem/paths", "error":null,
        "result":{"attempted":1,"succeeded":0,"reclaimed_bytes":0,"failures":[failure.clone()],
        "outcomes":[{"item_id":"same-id","status":"failed","reclaimed_estimate":0,"failure":failure}]}}]
    })).unwrap();
    let plan = serde_json::from_value(json!({
        "plan_id":"plan", "scan_id":"scan", "recipe_revision":"r", "digest":"d", "config_name":"fixture",
        "groups":[{"module_id":"cache", "task_id":"temp", "handler_id":"builtin.filesystem/paths",
        "request":{"module_id":"cache","task_id":"temp","handler_id":"builtin.filesystem/paths","name":"Temp","options":{},"recommend":{},"tags":[]},
        "items":[{"id":"same-id","parent_id":null,"name":"file","location":"fixture","logical_bytes":1024,"reclaimable_bytes":512,"modified_at":null,"recommendation":"recommended","reason":"fixture","tags":[],"opaque":{}}]}]
    })).unwrap();
    (report, plan)
}

#[test]
fn locked_items_keep_logical_size_separate_from_cleaned_bytes() {
    let (report, plan) = fixture();
    let items = unfinished_cleanup_items(&report, Some(&plan));
    assert!(items[0].in_use);
    assert_eq!(items[0].estimated_bytes, Some(1024));
    assert_eq!(report.reclaimed_bytes(), 0);
    assert_eq!(report.failure_count(), 1);
    assert_eq!(
        unfinished_cleanup_items(&report, None)[0].estimated_bytes,
        None
    );
}

#[test]
fn partial_filesystem_estimates_require_comparable_confirmed_measurements() {
    let (mut report, mut plan) = fixture();
    let result = report.groups[0].result.as_mut().unwrap();
    result.outcomes[0].status = ItemExecutionStatus::PartiallyApplied;
    result.outcomes[0].reclaimed_estimate = Some(256);
    let items = unfinished_cleanup_items(&report, Some(&plan));
    assert!(items[0].partially_applied);
    assert_eq!(items[0].estimated_bytes, Some(768));
    report.groups[0].result.as_mut().unwrap().outcomes[0].reclaimed_estimate = Some(2048);
    assert_eq!(
        unfinished_cleanup_items(&report, Some(&plan))[0].estimated_bytes,
        None
    );
    report.groups[0].result.as_mut().unwrap().outcomes[0].reclaimed_estimate = None;
    assert_eq!(
        unfinished_cleanup_items(&report, Some(&plan))[0].estimated_bytes,
        None
    );
    plan.groups[0].task_id = "another-task".into();
    assert_eq!(
        unfinished_cleanup_items(&report, Some(&plan))[0].estimated_bytes,
        None
    );
}

#[test]
fn uncertain_or_authorization_failures_never_become_benign_skips() {
    let (mut report, plan) = fixture();
    report.groups[0].result.as_mut().unwrap().failures[0].authorization =
        Some(PermissionOutcome::TimedOut);
    assert!(!unfinished_cleanup_items(&report, Some(&plan))[0].in_use);
    report.groups[0].result.as_mut().unwrap().failures[0].authorization = None;
    report.groups[0].result.as_mut().unwrap().outcomes[0].status = ItemExecutionStatus::Uncertain;
    let items = unfinished_cleanup_items(&report, Some(&plan));
    assert!(!items[0].in_use);
    assert_eq!(items[0].estimated_bytes, None);
    report.groups[0].result.as_mut().unwrap().outcomes.clear();
    assert!(!unfinished_cleanup_items(&report, Some(&plan))[0].in_use);
}
