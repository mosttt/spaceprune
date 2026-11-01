//! Cleanup outcomes and shared validation for in-process and external handlers.

use super::*;
use std::collections::BTreeSet;

/// 单个清理项的执行失败。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct ItemFailure {
    /// Best-effort native file usage at failure time, absent on unsupported platforms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_usage: Option<crate::access::FileUsage>,
    /// Result of an authorization attempt for this item, distinct from its original IO failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<crate::access::PermissionOutcome>,
    /// Structured native failure; display strings are not retry decisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io_failure: Option<crate::access::IoFailure>,
    /// 任务内清理项 ID。
    pub item_id: String,
    /// 失败说明。
    pub message: String,
}

impl ItemFailure {
    /// A sharing violation is an in-use skip only when no authorization failure overrides it.
    pub fn is_in_use(&self, status: ItemExecutionStatus) -> bool {
        matches!(
            status,
            ItemExecutionStatus::Failed | ItemExecutionStatus::PartiallyApplied
        ) && self
            .io_failure
            .as_ref()
            .is_some_and(|io| io.kind == crate::access::IoFailureKind::SharingViolation)
            && matches!(
                self.authorization,
                None | Some(crate::access::PermissionOutcome::Granted)
            )
    }
}

/// Handler 的清理结果。
/// Outcome of one originally selected unit; this is distinct from the operation's terminal state.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ItemExecutionStatus {
    /// All eligible work in the selected unit completed.
    Succeeded,
    /// The selected resource had already disappeared; its additional estimate is zero.
    AlreadyAbsent,
    /// Some effects occurred before a failure or cancellation.
    PartiallyApplied,
    /// An attempted operation failed before any known effects.
    Failed,
    /// Work was dispatched but its effects could not be confirmed; never retry without reconciliation.
    Uncertain,
    /// No destructive work was attempted, for example after cancellation.
    NotAttempted,
}

/// Exact per-selection execution evidence used by reports and execution journals.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct ItemExecution {
    /// Original selected item identity within its task.
    pub item_id: String,
    /// Observable outcome of this selected unit.
    pub status: ItemExecutionStatus,
    /// Known estimate of actual processed content; None means unknown, not zero.
    pub reclaimed_estimate: Option<u64>,
    /// Known processed-size subtotal when the complete estimate is unknown; otherwise zero.
    #[serde(default)]
    pub reclaimed_lower_bound: u64,
    /// Failure or interruption that prevented full completion.
    pub failure: Option<ItemFailure>,
}

/// Aggregate and per-selection cleanup results from one handler invocation.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct HandlerCleanResult {
    /// Volume observations around distinct, non-overlapping cleanup batches.
    /// A shared batch is reported once, not once per selected item or task.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub space_changes: Vec<VolumeSpaceChange>,
    /// Per-selection outcomes, including partial effects and not-attempted work.
    #[serde(default)]
    pub outcomes: Vec<ItemExecution>,
    /// 尝试清理的项目数。
    pub attempted: usize,
    /// 成功清理的项目数。
    pub succeeded: usize,
    /// Known processed-size subtotal, not measured freed disk space.
    pub reclaimed_bytes: u64,
    /// Items with confirmed or uncertain effects whose processed-size estimate is unknown.
    /// None means the handler did not report size completeness (including older plugins).
    #[serde(default)]
    pub unknown_reclaimed_items: Option<usize>,
    /// 各清理项失败信息。
    pub failures: Vec<ItemFailure>,
}

/// Observed volume free-space change, separate from per-item processed-size estimates.
/// Concurrent filesystem activity can contribute to this change.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VolumeSpaceChange {
    /// Human-readable operation scope, such as a snapshot configuration name.
    pub scope: String,
    /// Free filesystem bytes immediately before the batch, including reserved free blocks.
    pub free_bytes_before: Option<u64>,
    /// Free bytes on the same filesystem after the batch command has exited.
    pub free_bytes_after: Option<u64>,
    /// Why measurement was unavailable; this does not change the deletion outcome.
    pub error: Option<String>,
}

impl HandlerCleanResult {
    /// Record an outcome once; repeated transport updates do not count its effects twice.
    pub fn record_outcome(&mut self, outcome: ItemExecution) {
        if self
            .outcomes
            .iter()
            .any(|previous| previous.item_id == outcome.item_id)
        {
            return;
        }
        if outcome.status != ItemExecutionStatus::NotAttempted {
            self.attempted = self.attempted.saturating_add(1);
        }
        if matches!(
            outcome.status,
            ItemExecutionStatus::Succeeded | ItemExecutionStatus::AlreadyAbsent
        ) {
            self.succeeded = self.succeeded.saturating_add(1);
        }
        if outcome.has_unknown_reclaimed_size() {
            let unknown = self.unknown_reclaimed_items.get_or_insert(0);
            *unknown = unknown.saturating_add(1);
        }
        self.reclaimed_bytes = self
            .reclaimed_bytes
            .saturating_add(outcome.known_reclaimed_bytes());
        if outcome.status != ItemExecutionStatus::NotAttempted
            && let Some(failure) = &outcome.failure
        {
            self.failures.push(failure.clone());
        }
        self.outcomes.push(outcome);
    }

    /// Record one successful item and its processed-size estimate, not measured freed space.
    pub fn record_success(&mut self, estimated_bytes: Option<u64>) {
        let unknown = self.unknown_reclaimed_items.get_or_insert(self.succeeded);
        self.attempted = self.attempted.saturating_add(1);
        self.succeeded = self.succeeded.saturating_add(1);
        if let Some(bytes) = estimated_bytes {
            self.reclaimed_bytes = self.reclaimed_bytes.saturating_add(bytes);
        } else {
            *unknown = unknown.saturating_add(1);
        }
    }

    /// Count unknown processed-size estimates, including partial and unconfirmed effects.
    pub fn unknown_reclaimed_items(&self) -> usize {
        self.unknown_reclaimed_items.unwrap_or(self.succeeded)
    }
}

impl ItemExecution {
    /// Confirmed size evidence, even when another phase has unknown effects or size.
    pub fn known_reclaimed_bytes(&self) -> u64 {
        self.reclaimed_estimate
            .unwrap_or(self.reclaimed_lower_bound)
    }

    /// Whether confirmed or uncertain effects lack a complete processed-size estimate.
    pub fn has_unknown_reclaimed_size(&self) -> bool {
        self.reclaimed_estimate.is_none()
            && matches!(
                self.status,
                ItemExecutionStatus::Succeeded
                    | ItemExecutionStatus::PartiallyApplied
                    | ItemExecutionStatus::Uncertain
            )
    }

    /// Validate status, size and failure consistency before accepting a receipt.
    pub fn validate(&self) -> Result<(), String> {
        if self.reclaimed_lower_bound > 0
            && (self.reclaimed_estimate.is_some()
                || !matches!(
                    self.status,
                    ItemExecutionStatus::Succeeded
                        | ItemExecutionStatus::PartiallyApplied
                        | ItemExecutionStatus::Uncertain
                ))
        {
            return Err("A processed-size lower bound requires unknown-size effects".into());
        }
        if self
            .failure
            .as_ref()
            .is_some_and(|failure| failure.item_id != self.item_id)
        {
            return Err("Handler outcome and failure refer to different items".into());
        }
        let successful = matches!(
            self.status,
            ItemExecutionStatus::Succeeded | ItemExecutionStatus::AlreadyAbsent
        );
        if (successful && self.failure.is_some())
            || (!successful
                && self.status != ItemExecutionStatus::NotAttempted
                && self.failure.is_none())
        {
            return Err("Handler outcome has an inconsistent failure status".into());
        }
        if matches!(
            self.status,
            ItemExecutionStatus::AlreadyAbsent | ItemExecutionStatus::NotAttempted
        ) && self.reclaimed_estimate != Some(0)
        {
            return Err("Absent or unattempted items cannot reclaim additional space".into());
        }
        Ok(())
    }
}

impl HandlerCleanResult {
    /// Validate aggregates and outcomes against the original confirmed selection.
    pub fn validate_selected(&self, items: &[CleanupItem], cancelled: bool) -> Result<(), String> {
        let measured_items = if self.outcomes.is_empty() {
            self.succeeded
        } else {
            self.attempted
        };
        if self
            .unknown_reclaimed_items
            .is_some_and(|unknown| unknown > measured_items)
        {
            return Err(
                "Handler reported more unknown-size items than processed attempts".to_owned(),
            );
        }
        if self.attempted > items.len() {
            return Err(format!(
                "Handler reported {} attempted items for a group containing {} items",
                self.attempted,
                items.len()
            ));
        }
        if self.succeeded.saturating_add(self.failures.len()) != self.attempted {
            return Err(format!(
                "Handler result is inconsistent: {} attempted, {} succeeded, {} failed",
                self.attempted,
                self.succeeded,
                self.failures.len()
            ));
        }
        if !cancelled && self.attempted != items.len() {
            return Err(format!(
                "Handler reported {} attempted items for {} selected items without cancellation",
                self.attempted,
                items.len()
            ));
        }

        let selected_ids = items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<BTreeSet<_>>();
        let mut failure_ids = BTreeSet::new();
        for failure in &self.failures {
            if !selected_ids.contains(failure.item_id.as_str()) {
                return Err(format!(
                    "Handler reported failure for unknown item ID `{}`",
                    failure.item_id
                ));
            }
            if !failure_ids.insert(failure.item_id.as_str()) {
                return Err(format!(
                    "Handler reported duplicate failures for item ID `{}`",
                    failure.item_id
                ));
            }
        }
        if !self.outcomes.is_empty() {
            if self.outcomes.len() != items.len() {
                return Err("Handler omitted selected item outcomes".into());
            }
            let mut seen = BTreeSet::new();
            let mut rebuilt = HandlerCleanResult {
                space_changes: self.space_changes.clone(),
                unknown_reclaimed_items: Some(0),
                ..Default::default()
            };
            for outcome in &self.outcomes {
                if !selected_ids.contains(outcome.item_id.as_str())
                    || !seen.insert(&outcome.item_id)
                {
                    return Err("Handler returned an unknown or repeated item outcome".into());
                }
                outcome.validate()?;
                rebuilt.record_outcome(outcome.clone());
            }
            if rebuilt != *self {
                return Err("Handler aggregate disagrees with its per-item outcomes".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod cleanup_size_tests {
    use super::*;

    #[test]
    fn unknown_size_receipt_preserves_its_subtotal_across_serialization_and_aggregation() {
        let receipt = ItemExecution {
            item_id: "partial".into(),
            status: ItemExecutionStatus::Uncertain,
            reclaimed_estimate: None,
            reclaimed_lower_bound: 6,
            failure: Some(ItemFailure {
                file_usage: None,
                authorization: None,
                item_id: "partial".into(),
                io_failure: None,
                message: "Lost retry".into(),
            }),
        };
        let restored: ItemExecution =
            serde_json::from_value(serde_json::to_value(&receipt).unwrap()).unwrap();
        assert_eq!(restored, receipt);
        assert!(restored.validate().is_ok());
        let mut result = HandlerCleanResult {
            unknown_reclaimed_items: Some(0),
            ..Default::default()
        };
        result.record_outcome(restored.clone());
        result.record_outcome(restored.clone());
        assert_eq!(
            (
                result.attempted,
                result.succeeded,
                result.reclaimed_bytes,
                result.unknown_reclaimed_items()
            ),
            (1, 0, 6, 1)
        );
        let mut invalid = restored.clone();
        invalid.reclaimed_estimate = Some(6);
        assert!(invalid.validate().is_err());
        for status in [
            ItemExecutionStatus::AlreadyAbsent,
            ItemExecutionStatus::Failed,
            ItemExecutionStatus::NotAttempted,
        ] {
            invalid = restored.clone();
            invalid.status = status;
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    fn successful_sizes_distinguish_unknown_zero_and_partial_failures() {
        let mut result = HandlerCleanResult::default();
        result.record_success(Some(0));
        assert_eq!(result.unknown_reclaimed_items(), 0);
        assert_eq!(result.reclaimed_bytes, 0);
        result.record_success(None);
        result.record_success(Some(8192));
        result.attempted += 1;
        result.failures.push(ItemFailure {
            file_usage: None,
            authorization: None,
            io_failure: None,
            item_id: "failed-unknown".to_owned(),
            message: "fixture failure".to_owned(),
        });
        assert_eq!(result.attempted, 4);
        assert_eq!(result.succeeded, 3);
        assert_eq!(result.unknown_reclaimed_items(), 1);
        assert_eq!(result.reclaimed_bytes, 8192);
        let restored: HandlerCleanResult =
            serde_json::from_value(serde_json::to_value(&result).unwrap()).unwrap();
        assert_eq!(restored, result);
    }

    #[test]
    fn legacy_results_do_not_claim_zero_size_or_complete_measurement() {
        let mut result: HandlerCleanResult = serde_json::from_value(serde_json::json!({
            "attempted": 2, "succeeded": 2, "reclaimed_bytes": 0, "failures": []
        }))
        .unwrap();
        assert_eq!(result.unknown_reclaimed_items(), 2);
        result.record_success(Some(7));
        assert_eq!(result.unknown_reclaimed_items(), 2);
        assert_eq!(result.succeeded, 3);
        assert_eq!(result.reclaimed_bytes, 7);
    }
}
