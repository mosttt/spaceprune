use super::*;
use spaceprune::api::{GroupExecutionReport, VolumeSpaceChange};

#[test]
fn locked_files_use_a_warning_receipt_without_repeated_red_os_errors() {
    let mut report = execution(0, 0, Some(0));
    let result = report.groups[0].result.as_mut().unwrap();
    let failure = serde_json::json!({"item_id":"locked", "message":"Raw OS sharing error", "io_failure":{"kind":"sharing_violation","os_code":32}});
    result.record_outcome(serde_json::from_value(serde_json::json!({"item_id":"locked","status":"failed","reclaimed_estimate":0,"failure":failure})).unwrap());
    let (text, tone) = execution_summary(&report);
    assert!(text.contains("in-use content skipped"));
    assert!(text.contains("In use (skipped)"));
    assert!(matches!(tone, Tone::Warning));
    assert!(failure_details(&report).is_empty());
    report.groups[0].error = Some("Unexpected handler failure".into());
    assert!(matches!(status(&report).1, Tone::Error));
    assert!(
        failure_details(&report)
            .join("\n")
            .contains("Unexpected handler failure")
    );
}

fn execution_summary(execution: &ExecutionReport) -> (String, Tone) {
    (
        summary(execution, Some(Duration::from_secs(26)), Theme::default()),
        status(execution).1,
    )
}

fn execution(
    succeeded: usize,
    reclaimed_bytes: u64,
    unknown_reclaimed_items: Option<usize>,
) -> ExecutionReport {
    let mut group = GroupExecutionReport {
        failure: None,
        size_accounting: Default::default(),
        module_id: "module".to_owned(),
        task_id: "task".to_owned(),
        handler_id: "fixture/handler".to_owned(),
        result: Some(Default::default()),
        error: None,
    };
    let result = group.result.as_mut().unwrap();
    result.attempted = succeeded;
    result.succeeded = succeeded;
    result.reclaimed_bytes = reclaimed_bytes;
    result.unknown_reclaimed_items = unknown_reclaimed_items;
    ExecutionReport {
        operation_failures: Vec::new(),
        groups: vec![group],
        cancelled: false,
        stop_reason: None,
    }
}

#[test]
fn unconfirmed_shutdown_does_not_render_as_a_successful_cancellation() {
    let mut report = execution(2, 1024, Some(0));
    report.cancelled = true;
    report
        .operation_failures
        .push(spaceprune::api::PluginError::new(
            "cleanup-stop-unconfirmed",
            "Native close reply missing",
        ));
    let (summary, tone) = execution_summary(&report);
    assert!(summary.contains("Cleanup service stop unconfirmed"));
    assert!(summary.contains("2 of 2 attempted"));
    assert!(summary.contains("Service issues  1"));
    assert!(!summary.contains("Cleanup cancelled"));
    assert!(matches!(tone, Tone::Error));
}

#[test]
fn execution_summary_reports_unknown_size_without_claiming_zero_bytes() {
    let text = execution_summary(&execution(2, 0, Some(2))).0;
    assert!(text.contains("Processed size  unknown"));
    assert!(text.contains("Unknown size    2 items"));
    assert!(!text.contains("0 B"));
}

#[test]
fn execution_summary_preserves_known_zero_size() {
    let text = execution_summary(&execution(1, 0, Some(0))).0;
    assert!(text.contains("Processed size  0 B"));
    assert!(!text.contains("Unknown size"));
}

#[test]
fn snapshots_are_separate_from_unknown_content_sizes() {
    let mut snapshots = execution(5, 0, Some(5));
    snapshots.groups[0].size_accounting = spaceprune::api::CleanupSizeAccounting::SnapshotGroups;
    let summary = execution_summary(&snapshots).0;
    assert!(summary.contains("5 groups processed"));
    assert!(!summary.contains("Unknown size"));
    assert!(!summary.contains("0 B"));

    let mut mixed = execution(2, 1024, Some(1));
    mixed.groups.extend(snapshots.groups);
    let summary = execution_summary(&mixed).0;
    assert!(summary.contains("Processed size  at least 1.00 KiB\n  Unknown size    1 item"));
    assert!(summary.contains("Snapshots       5 groups processed"));
    assert!(!summary.contains("6 items"));
}

#[test]
fn already_absent_snapshots_report_zero_additional_space_but_keep_real_deletions_measured() {
    let mut snapshots = execution(2, 0, Some(0));
    snapshots.groups[0].size_accounting = spaceprune::api::CleanupSizeAccounting::SnapshotGroups;
    snapshots.groups[0].result.as_mut().unwrap().outcomes =
        serde_json::from_value(serde_json::json!([
            {"item_id":"home:1","status":"already_absent","reclaimed_estimate":0,"failure":null},
            {"item_id":"home:2","status":"already_absent","reclaimed_estimate":0,"failure":null}
        ]))
        .unwrap();
    let mut mixed = execution(4, 438304768, Some(2));
    mixed.groups.extend(snapshots.groups.clone());
    for report in [&snapshots, &mixed] {
        let summary = execution_summary(report).0;
        assert!(
            summary.contains("0 groups deleted · 2 already absent"),
            "{summary}"
        );
        assert!(summary.contains("0 B additionally freed"), "{summary}");
        assert!(!summary.contains("unavailable"), "{summary}");
    }
    let result = snapshots.groups[0].result.as_mut().unwrap();
    result.outcomes[0].status = serde_json::from_value(serde_json::json!("succeeded")).unwrap();
    result.space_changes.push(VolumeSpaceChange {
        scope: "home".into(),
        free_bytes_before: Some(1024),
        free_bytes_after: Some(3072),
        error: None,
    });
    let summary = execution_summary(&snapshots).0;
    assert!(
        summary.contains("1 group deleted · 1 already absent"),
        "{summary}"
    );
    assert!(
        summary.contains("\n    home          +2.00 KiB\n"),
        "{summary}"
    );
    assert!(!summary.contains("0 B additionally freed"), "{summary}");
}

#[test]
fn snapshot_space_change_preserves_negative_zero_and_unavailable_measurements() {
    let mut report = execution(4, 0, Some(4));
    report.groups[0].size_accounting = spaceprune::api::CleanupSizeAccounting::SnapshotGroups;
    report.groups[0].result.as_mut().unwrap().space_changes = [
        ("home", Some(1024), Some(3072)),
        ("root", Some(4096), Some(3072)),
        ("shared", Some(1024), Some(1024)),
        ("missing", None, None),
    ]
    .into_iter()
    .map(
        |(scope, free_bytes_before, free_bytes_after)| spaceprune::api::VolumeSpaceChange {
            scope: scope.to_owned(),
            free_bytes_before,
            free_bytes_after,
            error: None,
        },
    )
    .collect();
    let (summary, _) = execution_summary(&report);
    assert!(
        summary.contains("\n    home          +2.00 KiB\n"),
        "{summary}"
    );
    assert!(
        summary.contains("\n    root          -1.00 KiB\n"),
        "{summary}"
    );
    assert!(summary.contains("\n    shared        +0 B\n"), "{summary}");
    assert!(
        summary.contains("\n    missing       unavailable\n"),
        "{summary}"
    );
    assert!(!summary.contains("Unknown size"));
}

#[test]
fn deadline_failure_preserves_processed_size_and_has_a_distinct_exit_code() {
    let mut report = execution(2, 1024, Some(1));
    report.cancelled = true;
    report.stop_reason = Some(spaceprune::api::OperationStopReason::DeadlineExceeded);
    let text = execution_summary(&report).0;
    assert!(text.contains("Cleanup timed out"));
    assert!(text.contains("2 of 2 attempted"));
    assert!(text.contains("Processed size  at least 1.00 KiB"));
    assert_eq!(crate::output::stopped_exit_code(report.stop_reason), 1);
    assert_eq!(
        crate::output::stopped_exit_code(Some(spaceprune::api::OperationStopReason::Cancelled)),
        130
    );
}

#[test]
fn execution_summary_treats_unreported_sizes_as_unknown() {
    assert_eq!(
        execution_summary(&execution(2, 0, None)).0,
        execution_summary(&execution(2, 0, Some(2))).0
    );
}

#[test]
fn execution_summary_keeps_mixed_size_estimate_and_counts_for_every_status() {
    let mut report = execution(1, 1024, Some(0));
    report.groups.extend(execution(2, 0, Some(2)).groups);

    let (summary, tone) = execution_summary(&report);
    assert!(summary.contains("Cleanup complete · 00:26"));
    assert!(summary.contains("3 of 3 attempted"));
    assert!(summary.contains("Processed size  at least 1.00 KiB\n  Unknown size    2 items"));
    assert!(matches!(tone, Tone::Success));

    report.groups.push(GroupExecutionReport {
        failure: None,
        size_accounting: Default::default(),
        module_id: "module".to_owned(),
        task_id: "failed-task".to_owned(),
        handler_id: "fixture/handler".to_owned(),
        result: None,
        error: Some("fixture failure".to_owned()),
    });
    let (summary, tone) = execution_summary(&report);
    assert!(summary.contains("Cleanup finished with errors · 00:26"));
    assert!(summary.contains("3 of 3 attempted"));
    assert!(summary.contains("Processed size  at least 1.00 KiB\n  Unknown size    2 items"));
    assert!(matches!(tone, Tone::Error));

    report.cancelled = true;
    let (summary, tone) = execution_summary(&report);
    assert!(summary.contains("Cleanup cancelled · 00:26"));
    assert!(summary.contains("3 of 3 attempted"));
    assert!(summary.contains("Processed size  at least 1.00 KiB\n  Unknown size    2 items"));
    assert!(matches!(tone, Tone::Warning));
}

#[test]
fn missing_component_cause_is_printed_once_with_affected_items_and_full_json_retained() {
    let mut report = execution(0, 0, Some(0));
    let result = report.groups[0].result.as_mut().unwrap();
    result.attempted = 18;
    result.failures = (0..18)
        .map(|i| {
            serde_json::from_value(serde_json::json!({
        "item_id":format!("C:/fixture/protected-{i}"),
        "authorization":"helper_missing",
        "io_failure":{"kind":"access_denied","os_code":5},
        "message":"Access denied; protected cleanup could not start: signed component missing"
    })).unwrap()
        })
        .collect();
    let details = failure_details(&report).join("\n");
    assert_eq!(details.matches("signed component missing").count(), 1);
    assert_eq!(details.matches("C:/fixture/protected-").count(), 5);
    assert!(details.contains("18 items"));
    assert!(!details.contains("expired"));
    assert!(details.contains("--output json"));
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(
        json["groups"][0]["result"]["failures"]
            .as_array()
            .unwrap()
            .len(),
        18
    );
}
