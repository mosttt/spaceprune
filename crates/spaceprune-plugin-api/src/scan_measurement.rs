//! Cumulative content measurements shared by native plugins and presentation hosts.
use crate::{CleanupItem, CleanupSizeAccounting, Recommendation, SizeApplicability};
use serde::Serialize;

/// A task's latest measurement snapshot, replacing previous snapshots for the same task.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanMeasurement {
    /// Number of measured leaf candidates, including candidates with unknown size.
    pub checked_items: usize,
    /// Real snapshot groups, excluding the current-system reference.
    pub snapshot_groups: usize,
    /// Snapshot logical references, never an estimate of released disk space.
    pub snapshot_logical_bytes: u64,
    /// Real snapshot groups without a measured logical size.
    pub snapshot_unknown_items: usize,
    /// Inspected process working sets, separate from content and snapshots.
    pub memory_process_count: usize,
    /// Current resident RAM; shared pages may be counted in multiple processes.
    pub memory_resident_bytes: u64,
    /// Process working sets whose resident size is unavailable.
    pub memory_unknown_items: usize,
    /// Sum of known logical content sizes; not physical free-space promises.
    pub logical_bytes: u64,
    /// Candidates whose applicable logical size could not be measured.
    pub unknown_size_items: usize,
    /// Known logical bytes belonging to recommended candidates.
    pub recommended_bytes: u64,
    /// Recommended candidates whose applicable size remains unknown.
    pub recommended_unknown_items: usize,
}

impl ScanMeasurement {
    /// Add a leaf candidate once; callers exclude parent summary nodes.
    pub fn add_item(&mut self, item: &CleanupItem) {
        self.add_item_with_accounting(item, CleanupSizeAccounting::ProcessedBytes);
    }

    /// Keep logical observations in their declared size domain.
    pub fn add_item_with_accounting(
        &mut self,
        item: &CleanupItem,
        accounting: CleanupSizeAccounting,
    ) {
        self.checked_items = self.checked_items.saturating_add(1);
        let unknown = usize::from(
            item.logical_bytes.is_none()
                && item.logical_size_applicability == SizeApplicability::Applicable,
        );
        match accounting {
            CleanupSizeAccounting::SnapshotGroups => {
                if item.logical_size_applicability == SizeApplicability::Applicable {
                    self.snapshot_groups = self.snapshot_groups.saturating_add(1);
                    self.snapshot_logical_bytes = self
                        .snapshot_logical_bytes
                        .saturating_add(item.logical_bytes.unwrap_or(0));
                    self.snapshot_unknown_items =
                        self.snapshot_unknown_items.saturating_add(unknown);
                }
                return;
            }
            CleanupSizeAccounting::MemoryBytes => {
                self.memory_process_count = self.memory_process_count.saturating_add(1);
                self.memory_resident_bytes = self
                    .memory_resident_bytes
                    .saturating_add(item.logical_bytes.unwrap_or(0));
                self.memory_unknown_items = self.memory_unknown_items.saturating_add(unknown);
                return;
            }
            CleanupSizeAccounting::ProcessedBytes => {}
        }
        self.logical_bytes = self
            .logical_bytes
            .saturating_add(item.logical_bytes.unwrap_or(0));
        self.unknown_size_items = self.unknown_size_items.saturating_add(unknown);
        if item.recommendation == Recommendation::Recommended {
            self.recommended_bytes = self
                .recommended_bytes
                .saturating_add(item.logical_bytes.unwrap_or(0));
            self.recommended_unknown_items = self.recommended_unknown_items.saturating_add(unknown);
        }
    }

    /// Combine independent task snapshots without counting repeated progress updates twice.
    pub fn merge(&mut self, other: Self) {
        self.checked_items = self.checked_items.saturating_add(other.checked_items);
        self.snapshot_groups = self.snapshot_groups.saturating_add(other.snapshot_groups);
        self.snapshot_logical_bytes = self
            .snapshot_logical_bytes
            .saturating_add(other.snapshot_logical_bytes);
        self.snapshot_unknown_items = self
            .snapshot_unknown_items
            .saturating_add(other.snapshot_unknown_items);
        self.memory_process_count = self
            .memory_process_count
            .saturating_add(other.memory_process_count);
        self.memory_resident_bytes = self
            .memory_resident_bytes
            .saturating_add(other.memory_resident_bytes);
        self.memory_unknown_items = self
            .memory_unknown_items
            .saturating_add(other.memory_unknown_items);
        self.logical_bytes = self.logical_bytes.saturating_add(other.logical_bytes);
        self.unknown_size_items = self
            .unknown_size_items
            .saturating_add(other.unknown_size_items);
        self.recommended_bytes = self
            .recommended_bytes
            .saturating_add(other.recommended_bytes);
        self.recommended_unknown_items = self
            .recommended_unknown_items
            .saturating_add(other.recommended_unknown_items);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item(bytes: Option<u64>) -> CleanupItem {
        CleanupItem {
            id: "fixture".into(),
            parent_id: None,
            name: "Fixture".into(),
            location: "fixture".into(),
            logical_size_applicability: SizeApplicability::Applicable,
            logical_bytes: bytes,
            reclaimable_bytes: None,
            modified_at: None,
            recommendation: Recommendation::Recommended,
            reason: String::new(),
            tags: vec![],
            opaque: Default::default(),
        }
    }
    #[test]
    fn snapshot_and_memory_domains_do_not_inflate_file_totals_or_unknown_counts() {
        let mut files = ScanMeasurement::default();
        files.add_item(&item(Some(1024)));
        let mut snapshots = ScanMeasurement::default();
        snapshots
            .add_item_with_accounting(&item(Some(1 << 40)), CleanupSizeAccounting::SnapshotGroups);
        snapshots.add_item_with_accounting(&item(None), CleanupSizeAccounting::SnapshotGroups);
        let mut current = item(None);
        current.logical_size_applicability = SizeApplicability::NotApplicable;
        snapshots.add_item_with_accounting(&current, CleanupSizeAccounting::SnapshotGroups);
        let mut memory = ScanMeasurement::default();
        memory.add_item_with_accounting(&item(Some(8192)), CleanupSizeAccounting::MemoryBytes);
        files.merge(snapshots);
        files.merge(memory);
        assert_eq!(files.checked_items, 5);
        assert_eq!(files.logical_bytes, 1024);
        assert_eq!(files.recommended_bytes, 1024);
        assert_eq!(files.unknown_size_items, 0);
        assert_eq!(files.snapshot_groups, 2);
        assert_eq!(files.snapshot_logical_bytes, 1 << 40);
        assert_eq!(files.snapshot_unknown_items, 1);
        assert_eq!(files.memory_process_count, 1);
        assert_eq!(files.memory_resident_bytes, 8192);
    }
}
