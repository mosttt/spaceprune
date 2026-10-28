//! Cumulative content measurements shared by native plugins and presentation hosts.
use crate::{CleanupItem, Recommendation, SizeApplicability};
use serde::Serialize;

/// A task's latest measurement snapshot, replacing previous snapshots for the same task.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanMeasurement {
    /// Number of measured leaf candidates, including candidates with unknown size.
    pub checked_items: usize,
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
        self.checked_items = self.checked_items.saturating_add(1);
        let unknown = usize::from(
            item.logical_bytes.is_none()
                && item.logical_size_applicability == SizeApplicability::Applicable,
        );
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
