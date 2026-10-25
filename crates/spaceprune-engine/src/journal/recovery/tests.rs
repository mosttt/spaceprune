use super::*;
use crate::journal::ExecutionJournal;
use crate::{CleanupPlan, ScanReport};
use serde_json::json;
use std::io::Write;

fn fixture(
    accounting: CleanupSizeAccounting,
) -> (tempfile::TempDir, CleanupPlan, ExecutionJournal) {
    let directory = tempfile::tempdir().unwrap();
    let items = ["one", "two"].map(|id| json!({"id":id,"parent_id":null,"name":id,"location":id,"logical_bytes":6,"reclaimable_bytes":6,"modified_at":null,"recommendation":"recommended","reason":"fixture","tags":[],"opaque":{}}));
    let scan: ScanReport = serde_json::from_value(json!({
        "scan_id":"fixture", "config_name":"fixture", "config_source":"fixture", "cancelled":false,
        "modules":[{"id":"module", "name":"fixture", "description":"", "tasks":[{
            "module_id":"module", "task_id":"task", "name":"fixture", "handler_id":"fixture/handler",
            "request":{"module_id":"module","task_id":"task","name":"fixture","handler_id":"fixture/handler","options":{},"recommend":{},"tags":[]},
            "items":items,
            "issues":[],"error":null
        }]}]
    })).unwrap();
    let plan = scan.plan_recommended().unwrap();
    let journal = ExecutionJournal::start(directory.path(), &plan, &[accounting]).unwrap();
    (directory, plan, journal)
}
fn outcome(id: &str, status: ItemExecutionStatus, bytes: Option<u64>) -> ItemExecution {
    ItemExecution {
        item_id: id.into(),
        status,
        reclaimed_estimate: bytes,
        reclaimed_lower_bound: 0,
        failure: (!matches!(
            status,
            ItemExecutionStatus::Succeeded
                | ItemExecutionStatus::AlreadyAbsent
                | ItemExecutionStatus::NotAttempted
        ))
        .then(|| ItemFailure {
            item_id: id.into(),
            authorization: None,
            io_failure: None,
            message: "Prior partial failure".into(),
        }),
    }
}

#[test]
fn incomplete_tail_preserves_committed_receipts_without_inventing_completion() {
    for tail in [
        b"{\"event\":".as_slice(),
        b"garbage\n",
        b"{\"event\":\"finished\",\"report\":null}\n",
    ] {
        let (_directory, plan, mut journal) = fixture(CleanupSizeAccounting::ProcessedBytes);
        journal.group_started(&plan.groups()[0]).unwrap();
        let receipt = outcome("one", ItemExecutionStatus::Succeeded, Some(6));
        journal
            .item_finished(&plan.groups()[0].request, &receipt)
            .unwrap();
        journal
            .item_finished(&plan.groups()[0].request, &receipt)
            .unwrap();
        journal.file.write_all(tail).unwrap();
        let recovered = recover_execution(journal.reference()).unwrap();
        assert!(!recovered.finished);
        assert_eq!(recovered.report.succeeded(), 1);
        assert_eq!(recovered.report.size_summary().reclaimed_bytes, 6);
        assert_eq!(recovered.report.size_summary().unknown_size_item_count, 1);
        assert_eq!(
            recovered.report.groups[0].result.as_ref().unwrap().outcomes[1].status,
            ItemExecutionStatus::Uncertain
        );
    }
}

#[test]
fn partial_work_keeps_its_native_error_and_lower_bound_but_not_a_known_final_size() {
    let (_directory, plan, mut journal) = fixture(CleanupSizeAccounting::ProcessedBytes);
    journal.group_started(&plan.groups()[0]).unwrap();
    let mut receipt = outcome("one", ItemExecutionStatus::PartiallyApplied, Some(6));
    receipt.failure.as_mut().unwrap().io_failure = Some(spaceprune_contracts::access::IoFailure {
        kind: spaceprune_contracts::access::IoFailureKind::AccessDenied,
        os_code: Some(13),
    });
    journal
        .item_finished(&plan.groups()[0].request, &receipt)
        .unwrap();
    let recovered = recover_execution(journal.reference()).unwrap();
    let actual = &recovered.report.groups[0].result.as_ref().unwrap().outcomes[0];
    assert_eq!(actual.status, ItemExecutionStatus::Uncertain);
    assert_eq!(actual.reclaimed_estimate, None);
    assert_eq!(actual.reclaimed_lower_bound, 6);
    assert_eq!(actual.failure, receipt.failure);
    assert_eq!(recovered.report.unknown_reclaimed_items(), 2);
}

#[test]
fn snapshot_accounting_and_final_space_observations_survive_recovery_once() {
    let (_directory, plan, mut journal) = fixture(CleanupSizeAccounting::SnapshotGroups);
    journal.group_started(&plan.groups()[0]).unwrap();
    let mut result = HandlerCleanResult {
        unknown_reclaimed_items: Some(0),
        ..Default::default()
    };
    for receipt in [
        outcome("one", ItemExecutionStatus::AlreadyAbsent, Some(0)),
        outcome("two", ItemExecutionStatus::Succeeded, None),
    ] {
        journal
            .item_finished(&plan.groups()[0].request, &receipt)
            .unwrap();
        result.record_outcome(receipt);
    }
    let partial = recover_execution(journal.reference()).unwrap();
    assert_eq!(partial.report.size_summary().snapshot_group_count, 2);
    assert_eq!(
        partial
            .report
            .size_summary()
            .snapshot_already_absent_group_count,
        1
    );
    assert_eq!(partial.report.size_summary().unknown_size_item_count, 0);
    result
        .space_changes
        .push(spaceprune_plugin_api::VolumeSpaceChange {
            scope: "fixture".into(),
            free_bytes_before: Some(1024),
            free_bytes_after: Some(3072),
            error: None,
        });
    let group = GroupExecutionReport {
        module_id: "module".into(),
        task_id: "task".into(),
        handler_id: "fixture/handler".into(),
        size_accounting: CleanupSizeAccounting::SnapshotGroups,
        result: Some(result),
        error: None,
        failure: None,
    };
    journal.group_finished(&group).unwrap();
    let grouped = recover_execution(journal.reference()).unwrap();
    assert!(!grouped.finished);
    assert_eq!(
        grouped.report.size_summary().snapshot_space_changes.len(),
        1
    );
    let report = ExecutionReport {
        operation_failures: Vec::new(),
        groups: vec![group],
        cancelled: false,
        stop_reason: None,
    };
    journal.finished(&report).unwrap();
    for _ in 0..2 {
        let final_record = recover_execution(journal.reference()).unwrap();
        assert!(final_record.finished);
        assert_eq!(final_record.report, report);
    }
}

#[test]
fn unstarted_work_is_zero_and_mismatched_missing_or_old_journals_are_explicit_errors() {
    let (_directory, plan, journal) = fixture(CleanupSizeAccounting::ProcessedBytes);
    let recovered = recover_execution(journal.reference()).unwrap();
    assert!(!recovered.finished);
    assert_eq!(recovered.report.unknown_reclaimed_items(), 0);
    assert!(
        recovered.report.groups[0]
            .result
            .as_ref()
            .unwrap()
            .outcomes
            .iter()
            .all(|item| item.status == ItemExecutionStatus::NotAttempted)
    );
    let mut reference = journal.reference().clone();
    reference.plan_id = "other".into();
    assert!(recover_execution(&reference).is_err());
    let path = journal.reference().path.to_path_buf().unwrap();
    std::fs::write(
        &path,
        format!(
            "{}\n",
            json!({"event":"started","schema_version":3,"plan":plan})
        ),
    )
    .unwrap();
    assert_eq!(
        recover_execution(journal.reference()).unwrap_err().kind(),
        io::ErrorKind::Unsupported
    );
    let reference = journal.reference().clone();
    drop(journal);
    std::fs::remove_file(path).unwrap();
    assert_eq!(
        recover_execution(&reference).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
}

#[test]
fn unreadable_tail_cannot_prove_that_later_selections_were_never_attempted() {
    let (_directory, _plan, mut journal) = fixture(CleanupSizeAccounting::SnapshotGroups);
    journal
        .file
        .write_all(b"{\"event\":\"group_started\"")
        .unwrap();
    let recovered = recover_execution(journal.reference()).unwrap();
    assert!(!recovered.finished);
    assert!(
        recovered.report.groups[0]
            .result
            .as_ref()
            .unwrap()
            .outcomes
            .iter()
            .all(|outcome| outcome.status == ItemExecutionStatus::Uncertain)
    );
    assert_eq!(recovered.report.size_summary().unknown_size_item_count, 0);
    assert_eq!(recovered.report.unknown_reclaimed_items(), 2);
}

#[test]
fn a_contradictory_final_report_cannot_erase_a_committed_success() {
    let (_directory, plan, mut journal) = fixture(CleanupSizeAccounting::ProcessedBytes);
    journal.group_started(&plan.groups()[0]).unwrap();
    journal
        .item_finished(
            &plan.groups()[0].request,
            &outcome("one", ItemExecutionStatus::Succeeded, Some(6)),
        )
        .unwrap();
    let mut result = HandlerCleanResult {
        unknown_reclaimed_items: Some(0),
        ..Default::default()
    };
    for id in ["one", "two"] {
        result.record_outcome(outcome(id, ItemExecutionStatus::AlreadyAbsent, Some(0)));
    }
    journal
        .finished(&ExecutionReport {
            operation_failures: Vec::new(),
            cancelled: false,
            stop_reason: None,
            groups: vec![GroupExecutionReport {
                module_id: "module".into(),
                task_id: "task".into(),
                handler_id: "fixture/handler".into(),
                size_accounting: CleanupSizeAccounting::ProcessedBytes,
                result: Some(result),
                error: None,
                failure: None,
            }],
        })
        .unwrap();
    let recovered = recover_execution(journal.reference()).unwrap();
    assert!(!recovered.finished);
    assert_eq!(recovered.report.succeeded(), 1);
    assert_eq!(recovered.report.reclaimed_bytes(), 6);
    assert_eq!(recovered.report.unknown_reclaimed_items(), 1);
}

#[test]
fn a_final_report_cannot_discard_a_committed_snapshot_space_observation() {
    let (_directory, plan, mut journal) = fixture(CleanupSizeAccounting::SnapshotGroups);
    journal.group_started(&plan.groups()[0]).unwrap();
    let mut result = HandlerCleanResult {
        unknown_reclaimed_items: Some(0),
        ..Default::default()
    };
    for id in ["one", "two"] {
        result.record_outcome(outcome(id, ItemExecutionStatus::Succeeded, None));
    }
    result
        .space_changes
        .push(spaceprune_plugin_api::VolumeSpaceChange {
            scope: "fixture".into(),
            free_bytes_before: Some(1024),
            free_bytes_after: Some(3072),
            error: None,
        });
    let group = GroupExecutionReport {
        module_id: "module".into(),
        task_id: "task".into(),
        handler_id: "fixture/handler".into(),
        size_accounting: CleanupSizeAccounting::SnapshotGroups,
        result: Some(result),
        error: None,
        failure: None,
    };
    journal.group_finished(&group).unwrap();
    let mut changed = group.clone();
    changed.result.as_mut().unwrap().space_changes.clear();
    journal
        .finished(&ExecutionReport {
            operation_failures: Vec::new(),
            groups: vec![changed],
            cancelled: false,
            stop_reason: None,
        })
        .unwrap();
    let recovered = recover_execution(journal.reference()).unwrap();
    assert!(!recovered.finished);
    assert_eq!(recovered.report.groups, vec![group]);
}
