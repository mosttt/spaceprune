//! Read only committed records; no recovered object can reserve or execute a cleanup plan.

use crate::{
    CleanupPlanData, CleanupSizeAccounting, ExecutionJournalReference, ExecutionReport,
    GroupExecutionReport, HandlerCleanResult, ItemFailure, PlanGroup, RecoveredExecution,
};
use serde::Deserialize;
use spaceprune_plugin_api::{ItemExecution, ItemExecutionStatus};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{self, BufRead, BufReader},
};

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Record {
    Started {
        schema_version: u32,
        plan: CleanupPlanData,
        #[serde(default)]
        accounting: Vec<CleanupSizeAccounting>,
    },
    GroupStarted {
        module_id: String,
        task_id: String,
        items: Vec<String>,
    },
    ItemFinished {
        module_id: String,
        task_id: String,
        handler_id: String,
        outcome: ItemExecution,
    },
    GroupFinished {
        result: GroupExecutionReport,
    },
    Finished {
        report: ExecutionReport,
    },
}

#[derive(Default)]
struct GroupEvidence {
    started: bool,
    accounting: CleanupSizeAccounting,
    outcomes: BTreeMap<String, ItemExecution>,
    finished: Option<GroupExecutionReport>,
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Execution journal has no matching supported plan",
    )
}

/// Recover a matching journal's committed prefix without scanning targets or resuming deletion.
/// A missing/invalid initial record is an error; an incomplete tail preserves prior evidence.
/// Journals from unsupported formats are rejected rather than guessed or migrated.
pub fn recover_execution(reference: &ExecutionJournalReference) -> io::Result<RecoveredExecution> {
    let path = reference.path.to_path_buf().ok_or_else(invalid)?;
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line)?;
    if !line.ends_with(b"\n") {
        return Err(invalid());
    }
    let Record::Started {
        schema_version,
        plan,
        accounting,
    } = serde_json::from_slice(&line).map_err(|_| invalid())?
    else {
        return Err(invalid());
    };
    if schema_version != 4 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("Execution journal schema {schema_version} is unsupported; expected 4"),
        ));
    }
    if plan.plan_id != reference.plan_id || accounting.len() != plan.groups.len() {
        return Err(invalid());
    }
    let indices: BTreeMap<_, _> = plan
        .groups
        .iter()
        .enumerate()
        .map(|(index, group)| ((group.module_id.clone(), group.task_id.clone()), index))
        .collect();
    if indices.len() != plan.groups.len()
        || plan.groups.iter().any(|group| {
            group
                .items
                .iter()
                .map(|item| &item.id)
                .collect::<BTreeSet<_>>()
                .len()
                != group.items.len()
        })
    {
        return Err(invalid());
    }
    let mut evidence: Vec<_> = accounting
        .into_iter()
        .map(|accounting| GroupEvidence {
            accounting,
            ..Default::default()
        })
        .collect();
    let mut unreadable_tail = false;
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) if line.ends_with(b"\n") => {}
            _ => {
                unreadable_tail = true;
                break;
            }
        }
        let Ok(record) = serde_json::from_slice::<Record>(&line) else {
            unreadable_tail = true;
            break;
        };
        if let Record::Finished { report } = record {
            if report.groups.len() == plan.groups.len()
                && report
                    .groups
                    .iter()
                    .zip(&plan.groups)
                    .zip(&evidence)
                    .all(|((report, group), evidence)| valid_group(report, group, evidence))
            {
                return Ok(RecoveredExecution {
                    finished: true,
                    report,
                });
            }
            unreadable_tail = true;
            break;
        }
        if !apply_record(record, &plan.groups, &indices, &mut evidence) {
            unreadable_tail = true;
            break;
        }
    }
    let groups = plan
        .groups
        .into_iter()
        .zip(evidence)
        .map(|(group, evidence)| recover_group(group, evidence, unreadable_tail))
        .collect();
    Ok(RecoveredExecution {
        finished: false,
        report: ExecutionReport {
            operation_failures: Vec::new(),
            groups,
            cancelled: false,
            stop_reason: None,
        },
    })
}

fn recover_group(
    group: PlanGroup,
    evidence: GroupEvidence,
    unreadable_tail: bool,
) -> GroupExecutionReport {
    if let Some(finished) = evidence.finished {
        return finished;
    }
    let mut result = HandlerCleanResult {
        unknown_reclaimed_items: Some(0),
        ..Default::default()
    };
    for item in group.items {
        let previous = evidence.outcomes.get(&item.id);
        let outcome = if !evidence.started && !unreadable_tail {
            ItemExecution {
                item_id: item.id,
                status: ItemExecutionStatus::NotAttempted,
                reclaimed_estimate: Some(0),
                reclaimed_lower_bound: 0,
                failure: None,
            }
        } else if let Some(outcome) = previous.filter(|outcome| {
            matches!(
                outcome.status,
                ItemExecutionStatus::Succeeded | ItemExecutionStatus::AlreadyAbsent
            )
        }) {
            outcome.clone()
        } else {
            // A failed or partial ordinary attempt may have resumed without another receipt.
            ItemExecution {
                item_id: item.id.clone(),
                status: ItemExecutionStatus::Uncertain,
                reclaimed_estimate: None,
                reclaimed_lower_bound: previous.map_or(0, ItemExecution::known_reclaimed_bytes),
                failure: Some(
                    previous
                        .and_then(|outcome| outcome.failure.clone())
                        .unwrap_or_else(|| ItemFailure {
                            item_id: item.id,
                            authorization: None,
                            io_failure: None,
                            message:
                                "Execution ended without a final receipt; rescan before retrying"
                                    .into(),
                        }),
                ),
            }
        };
        result.record_outcome(outcome);
    }
    GroupExecutionReport {
        module_id: group.module_id,
        task_id: group.task_id,
        handler_id: group.handler_id,
        size_accounting: evidence.accounting,
        result: Some(result),
        error: None,
        failure: None,
    }
}

fn valid_group(report: &GroupExecutionReport, group: &PlanGroup, evidence: &GroupEvidence) -> bool {
    report.module_id == group.module_id
        && report.task_id == group.task_id
        && report.handler_id == group.handler_id
        && report.size_accounting == evidence.accounting
        && evidence
            .finished
            .as_ref()
            .is_none_or(|finished| finished == report)
        && (report.result.is_some() || report.error.is_some())
        && report
            .result
            .as_ref()
            .is_none_or(|result| result.validate_selected(&group.items, true).is_ok())
        && evidence.outcomes.values().all(|observed| {
            report
                .result
                .as_ref()
                .and_then(|result| {
                    result
                        .outcomes
                        .iter()
                        .find(|outcome| outcome.item_id == observed.item_id)
                })
                .is_some_and(|actual| {
                    crate::execute::reconcile_outcome(observed, actual) == *actual
                })
        })
}

fn apply_record(
    record: Record,
    groups: &[PlanGroup],
    indices: &BTreeMap<(String, String), usize>,
    evidence: &mut [GroupEvidence],
) -> bool {
    let (module_id, task_id) = match &record {
        Record::GroupStarted {
            module_id, task_id, ..
        }
        | Record::ItemFinished {
            module_id, task_id, ..
        } => (module_id, task_id),
        Record::GroupFinished { result } => (&result.module_id, &result.task_id),
        _ => return false,
    };
    let Some(&index) = indices.get(&(module_id.clone(), task_id.clone())) else {
        return false;
    };
    let state = &mut evidence[index];
    let group = &groups[index];
    match record {
        Record::GroupStarted { items, .. } => {
            if state.started
                || items
                    != group
                        .items
                        .iter()
                        .map(|item| item.id.clone())
                        .collect::<Vec<_>>()
            {
                return false;
            }
            state.started = true;
        }
        Record::ItemFinished {
            handler_id,
            outcome,
            ..
        } => {
            if !state.started
                || state.finished.is_some()
                || handler_id != group.handler_id
                || !group.items.iter().any(|item| item.id == outcome.item_id)
                || outcome.validate().is_err()
            {
                return false;
            }
            let outcome = state.outcomes.get(&outcome.item_id).map_or_else(
                || outcome.clone(),
                |previous| crate::execute::reconcile_outcome(previous, &outcome),
            );
            state.outcomes.insert(outcome.item_id.clone(), outcome);
        }
        Record::GroupFinished { result } => {
            if state.finished.is_some() || !valid_group(&result, group, state) {
                return false;
            }
            state.finished = Some(result);
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests;
