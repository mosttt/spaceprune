//! Non-destructive receipt projections; native outcomes remain authoritative.

use crate::api::{
    CleanupPlanData, ExecutionReport, IoFailureKind, ItemExecutionStatus, PermissionOutcome,
};
use serde::Serialize;
use std::collections::BTreeMap;

/// One unfinished selection, with a scan-based estimate separate from cleaned bytes.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UnfinishedCleanupItem {
    /// Original module identity.
    pub module_id: String,
    /// Original task identity.
    pub task_id: String,
    /// Original selected item identity.
    pub item_id: String,
    /// Confirmed sharing violation, without an overriding authorization failure.
    pub in_use: bool,
    /// Some content was processed before the remaining work was blocked.
    pub partially_applied: bool,
    /// Estimated uncleaned logical content, never counted as freed space.
    pub estimated_bytes: Option<u64>,
}

/// Project unfinished items using exact plan identities and structured native results.
/// Missing/uncertain outcomes and incompatible measurement units keep an unknown size.
pub fn unfinished_cleanup_items(
    report: &ExecutionReport,
    plan: Option<&CleanupPlanData>,
) -> Vec<UnfinishedCleanupItem> {
    let planned = plan
        .into_iter()
        .flat_map(|plan| &plan.groups)
        .flat_map(|group| {
            group.items.iter().map(move |item| {
                (
                    (
                        group.module_id.as_str(),
                        group.task_id.as_str(),
                        group.handler_id.as_str(),
                        item.id.as_str(),
                    ),
                    item,
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    let mut items = Vec::new();
    for group in &report.groups {
        let Some(result) = &group.result else {
            continue;
        };
        let outcomes = result
            .outcomes
            .iter()
            .map(|outcome| (outcome.item_id.as_str(), outcome))
            .collect::<BTreeMap<_, _>>();
        for failure in &result.failures {
            let outcome = outcomes.get(failure.item_id.as_str()).copied();
            let status = outcome.map(|outcome| outcome.status);
            let in_use = matches!(
                status,
                Some(ItemExecutionStatus::Failed | ItemExecutionStatus::PartiallyApplied)
            ) && failure
                .io_failure
                .as_ref()
                .is_some_and(|io| io.kind == IoFailureKind::SharingViolation)
                && matches!(
                    failure.authorization,
                    None | Some(PermissionOutcome::Granted)
                );
            let item = planned.get(&(
                group.module_id.as_str(),
                group.task_id.as_str(),
                group.handler_id.as_str(),
                failure.item_id.as_str(),
            ));
            let estimated_bytes = item.and_then(|item| item.logical_bytes).and_then(|bytes| {
                match status {
                    Some(ItemExecutionStatus::Failed | ItemExecutionStatus::NotAttempted) => {
                        Some(bytes)
                    }
                    // Only the filesystem handler uses comparable logical-byte measurements.
                    Some(ItemExecutionStatus::PartiallyApplied)
                        if group.handler_id == "builtin.filesystem/paths" =>
                    {
                        bytes.checked_sub(outcome?.reclaimed_estimate?)
                    }
                    _ => None,
                }
            });
            items.push(UnfinishedCleanupItem {
                module_id: group.module_id.clone(),
                task_id: group.task_id.clone(),
                item_id: failure.item_id.clone(),
                in_use,
                partially_applied: status == Some(ItemExecutionStatus::PartiallyApplied),
                estimated_bytes,
            });
        }
    }
    items
}

#[cfg(test)]
mod tests;
