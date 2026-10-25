//! Separate snapshot groups from content whose processed size is expressed in bytes.

use crate::handler::{ItemExecutionStatus, VolumeSpaceChange};
use crate::{handler::CleanupSizeAccounting, report::ExecutionReport};
use serde::{Deserialize, Serialize};

/// Display totals that keep shared-storage operations out of unknown byte counts.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct CleanupSizeSummary {
    /// Known processed bytes for byte-accounted handlers, not measured freed disk space.
    pub reclaimed_bytes: u64,
    /// Byte-accounted selections whose complete processed-size estimate is unknown.
    pub unknown_size_item_count: usize,
    /// Successfully processed snapshot selection units; a pre/post pair is one group.
    pub snapshot_group_count: usize,
    /// Completed snapshot selections already absent before this execution; they freed zero bytes.
    #[serde(default)]
    pub snapshot_already_absent_group_count: usize,
    /// Distinct snapshot deletion batches, each measured once on its own volume.
    #[serde(default)]
    pub snapshot_space_changes: Vec<VolumeSpaceChange>,
}

impl CleanupSizeSummary {
    /// Project one task's cumulative counters without changing its original outcome evidence.
    pub fn from_progress(
        accounting: CleanupSizeAccounting,
        succeeded: usize,
        reclaimed_bytes: u64,
        unknown_size_item_count: usize,
    ) -> Self {
        match accounting {
            CleanupSizeAccounting::ProcessedBytes => Self {
                reclaimed_bytes,
                unknown_size_item_count,
                snapshot_group_count: 0,
                snapshot_already_absent_group_count: 0,
                snapshot_space_changes: Vec::new(),
            },
            CleanupSizeAccounting::SnapshotGroups => Self {
                snapshot_group_count: succeeded,
                ..Self::default()
            },
        }
    }

    /// Accumulate distinct task summaries. Repeated cumulative updates should replace the task first.
    pub fn merge(&mut self, other: &Self) {
        self.reclaimed_bytes = self.reclaimed_bytes.saturating_add(other.reclaimed_bytes);
        self.unknown_size_item_count = self
            .unknown_size_item_count
            .saturating_add(other.unknown_size_item_count);
        self.snapshot_group_count = self
            .snapshot_group_count
            .saturating_add(other.snapshot_group_count);
        self.snapshot_already_absent_group_count = self
            .snapshot_already_absent_group_count
            .saturating_add(other.snapshot_already_absent_group_count);
        self.snapshot_space_changes
            .extend_from_slice(&other.snapshot_space_changes);
    }
}

impl ExecutionReport {
    /// Project byte estimates and separately counted snapshot groups for user-facing summaries.
    pub fn size_summary(&self) -> CleanupSizeSummary {
        let mut summary = CleanupSizeSummary::default();
        for group in &self.groups {
            if let Some(result) = &group.result {
                summary.merge(&CleanupSizeSummary::from_progress(
                    group.size_accounting,
                    result.succeeded,
                    result.reclaimed_bytes,
                    result.unknown_reclaimed_items(),
                ));
                if group.size_accounting == CleanupSizeAccounting::SnapshotGroups {
                    summary.snapshot_already_absent_group_count += result
                        .outcomes
                        .iter()
                        .filter(|outcome| outcome.status == ItemExecutionStatus::AlreadyAbsent)
                        .count();
                    summary
                        .snapshot_space_changes
                        .extend_from_slice(&result.space_changes);
                }
            }
        }
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{handler::HandlerCleanResult, report::GroupExecutionReport};

    #[test]
    fn snapshot_accounting_uses_declared_units_and_keeps_raw_unknown_evidence() {
        let group = |accounting, succeeded, bytes, unknown| GroupExecutionReport {
            failure: None,
            size_accounting: accounting,
            module_id: "fixture".into(),
            task_id: "cleanup".into(),
            handler_id: "fixture/any-plugin".into(),
            result: Some(HandlerCleanResult {
                succeeded,
                reclaimed_bytes: bytes,
                unknown_reclaimed_items: Some(unknown),
                ..Default::default()
            }),
            error: None,
        };
        let mut report = ExecutionReport {
            operation_failures: Vec::new(),
            groups: vec![
                group(CleanupSizeAccounting::ProcessedBytes, 3, 1024, 2),
                group(CleanupSizeAccounting::SnapshotGroups, 5, 0, 5),
            ],
            cancelled: false,
            stop_reason: None,
        };
        assert_eq!(report.unknown_reclaimed_items(), 7);
        assert_eq!(
            report.size_summary(),
            CleanupSizeSummary {
                reclaimed_bytes: 1024,
                unknown_size_item_count: 2,
                snapshot_group_count: 5,
                snapshot_already_absent_group_count: 0,
                snapshot_space_changes: Vec::new(),
            }
        );
        report.cancelled = true;
        report.groups[1].error = Some("Interrupted after confirmed groups".into());
        assert_eq!(report.size_summary().snapshot_group_count, 5);
        assert_eq!(report.size_summary().unknown_size_item_count, 2);
    }
}
