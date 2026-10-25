//! Cross-consumer automation results retain partial effects and actionable terminal states.

use serde_json::json;
use spaceprune::api::{
    AutomationRunReport, AutomationRunStatus, ExecutionReport, HandlerIssue, OperationStopReason,
    ScanReport,
};

fn report() -> AutomationRunReport {
    let scan: ScanReport = serde_json::from_value(json!({
        "scan_id":"automation-fixture", "config_name":"fixture", "config_source":"fixture",
        "cancelled":false, "modules":[{"id":"fixture", "name":"fixture", "description":"", "tasks":[{
            "module_id":"fixture", "task_id":"restricted", "name":"Restricted", "handler_id":"builtin.filesystem/paths",
            "request":{"module_id":"fixture", "task_id":"restricted", "name":"Restricted", "handler_id":"builtin.filesystem/paths", "options":{}, "recommend":{}, "tags":[]},
            "items":[], "issues":[], "error":null
        }]}]
    })).unwrap();
    let plan = scan.plan_recommended().unwrap();
    let execution: ExecutionReport = serde_json::from_value(json!({
        "cancelled":false, "groups":[{"module_id":"fixture", "task_id":"readable", "handler_id":"builtin.filesystem/paths", "error":null,
            "result":{"attempted":1, "succeeded":1, "reclaimed_bytes":16, "unknown_reclaimed_items":0, "failures":[]}}]
    })).unwrap();
    AutomationRunReport {
        scan,
        plan,
        execution,
    }
}

fn issue(code: Option<&str>, kind: Option<&str>) -> HandlerIssue {
    serde_json::from_value(json!({
        "level":"warning", "code":code, "message":"localized display text", "location":null,
        "io_failure":kind.map(|kind| json!({"kind":kind, "os_code":null}))
    }))
    .unwrap()
}

#[test]
fn background_permission_gaps_do_not_hide_successful_partial_effects() {
    let mut report = report();
    assert_eq!(report.status(), AutomationRunStatus::Completed);
    report.scan.modules[0].tasks[0].issues = vec![
        issue(None, Some("access_denied")),
        issue(Some("authorization-required"), None),
    ];
    assert_eq!(report.status(), AutomationRunStatus::NeedsAuthorization);
    assert_eq!(report.scan_issue_count(), 2);
    let encoded = serde_json::to_value(&report).unwrap();
    assert_eq!(encoded["status"], "needs_authorization");
    assert_eq!(encoded["execution"]["groups"][0]["result"]["succeeded"], 1);
    assert_eq!(
        encoded["execution"]["groups"][0]["result"]["reclaimed_bytes"],
        16
    );
}

#[test]
fn component_scope_and_execution_failures_are_not_password_requests() {
    for code in [
        "authorization-helper-missing",
        "authorization-helper-untrusted",
        "authorization-helper-outdated",
        "authorization-scope-rejected",
        "authorization-unavailable",
        "authorization-timeout",
    ] {
        let mut report = report();
        report.scan.modules[0].tasks[0].issues =
            vec![issue(None, Some("access_denied")), issue(Some(code), None)];
        assert_eq!(report.status(), AutomationRunStatus::Failed, "{code}");
    }
    for (kind, status) in [
        ("access_denied", AutomationRunStatus::NeedsAuthorization),
        ("read_only_volume", AutomationRunStatus::Failed),
        ("boundary_violation", AutomationRunStatus::Failed),
        ("target_changed", AutomationRunStatus::Failed),
    ] {
        let mut report = report();
        report.execution.groups[0].result.as_mut().unwrap().failures.push(serde_json::from_value(json!({"item_id":"retained", "message":"Permission denied: deliberately misleading display text", "io_failure":{"kind":kind, "os_code":null}})).unwrap());
        assert_eq!(report.status(), status, "{kind}");
    }
}

#[test]
fn warnings_errors_cancellation_and_deadlines_have_distinct_outcomes() {
    let mut report = report();
    report.scan.modules[0].tasks[0]
        .issues
        .push(issue(None, None));
    assert_eq!(report.status(), AutomationRunStatus::CompletedWithIssues);
    report.scan.modules[0].tasks[0].error = Some("invalid rule".into());
    assert_eq!(report.status(), AutomationRunStatus::Failed);
    report.scan.cancelled = true;
    assert_eq!(report.status(), AutomationRunStatus::Cancelled);
    report.scan.stop_reason = Some(OperationStopReason::DeadlineExceeded);
    assert_eq!(report.status(), AutomationRunStatus::Failed);
    assert_eq!(report.execution.reclaimed_bytes(), 16);
}

#[test]
fn unconfirmed_shutdown_outweighs_cancellation_without_losing_completed_effects() {
    let mut report = report();
    report.execution.cancelled = true;
    report.execution.stop_reason = Some(OperationStopReason::Cancelled);
    report.execution.operation_failures.push(serde_json::from_value(json!({
        "code":"cleanup-stop-unconfirmed", "message":"Native close reply missing", "authorization":"timed_out"
    })).unwrap());
    assert_eq!(report.status(), AutomationRunStatus::Failed);
    assert_eq!(report.execution.succeeded(), 1);
    assert_eq!(report.execution.reclaimed_bytes(), 16);
    assert_eq!(report.execution.failure_count(), 1);
    assert_eq!(serde_json::to_value(&report).unwrap()["status"], "failed");
}
