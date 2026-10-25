//! Captured pre-extraction CLI output, not generated from the types under test.
//! Source: Core 8d37e32, generated files and real sudo/PAM in an isolated container.

use spaceprune_contracts::{access::IoFailureKind, report, size_summary::CleanupSizeSummary};

#[test]
fn existing_cli_observations_roundtrip_without_runtime_state_or_wire_drift() {
    let original: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/automation-denied.json")).unwrap();
    let scan: report::ScanReport = serde_json::from_value(original["scan"].clone()).unwrap();
    let plan: report::CleanupPlan = serde_json::from_value(original["plan"].clone()).unwrap();
    let execution: report::ExecutionReport =
        serde_json::from_value(original["execution"].clone()).unwrap();
    let summary: CleanupSizeSummary =
        serde_json::from_value(original["cleanup_size_summary"].clone()).unwrap();
    assert_eq!(serde_json::to_value(&scan).unwrap(), original["scan"]);
    assert_eq!(serde_json::to_value(&plan).unwrap(), original["plan"]);
    assert_eq!(
        serde_json::to_value(&execution).unwrap(),
        original["execution"]
    );
    assert_eq!(scan.items_with_keys().len(), 1);
    assert_eq!(plan.item_count(), 1);
    assert_eq!(execution.succeeded(), 0);
    assert_eq!(execution.failure_count(), 1);
    assert_eq!(
        execution.groups[0].result.as_ref().unwrap().failures[0]
            .io_failure
            .as_ref()
            .unwrap()
            .kind,
        IoFailureKind::AccessDenied
    );
    assert_eq!(execution.size_summary(), summary);
    assert_eq!(summary.reclaimed_bytes, 0);
    let host_scan: spaceprune_engine::ScanReport =
        serde_json::from_value(original["scan"].clone()).unwrap();
    assert_eq!(serde_json::to_value(&host_scan).unwrap(), original["scan"]);
    assert_eq!(host_scan.into_report(), scan);
}
