//! Logical-size observations distinguish unknown measurements from domain inapplicability.

use crate::handler::{CleanupItem, SizeApplicability};

/// Shared aggregation for logical size in reports and host interfaces.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogicalSizeSummary {
    /// Sum of known applicable logical sizes, saturating at the public byte limit.
    pub known_bytes: u64,
    /// Applicable items with a missing measurement.
    pub unknown_items: usize,
    /// Items explicitly excluded from logical-size measurement by their domain.
    pub not_applicable_items: usize,
    /// Items for which logical size applies, including known zero and unknown measurements.
    pub applicable_items: usize,
}

impl LogicalSizeSummary {
    /// Aggregate the supplied items; callers choose leaf items when the report contains parents.
    pub fn from_items<'a>(items: impl IntoIterator<Item = &'a CleanupItem>) -> Self {
        let mut summary = Self::default();
        for item in items {
            if item.logical_size_applicability == SizeApplicability::NotApplicable {
                summary.not_applicable_items += 1;
            } else {
                summary.applicable_items += 1;
                match item.logical_bytes {
                    Some(bytes) => summary.known_bytes = summary.known_bytes.saturating_add(bytes),
                    None => summary.unknown_items += 1,
                }
            }
        }
        summary
    }

    /// Only a nonempty, entirely inapplicable group is N/A; an empty group remains zero bytes.
    pub fn applicability(self) -> SizeApplicability {
        if self.applicable_items == 0 && self.not_applicable_items > 0 {
            SizeApplicability::NotApplicable
        } else {
            SizeApplicability::Applicable
        }
    }
}

impl CleanupItem {
    /// Whether logical size applies but has not been measured.
    pub fn has_unknown_logical_size(&self) -> bool {
        self.logical_size_applicability.is_applicable() && self.logical_bytes.is_none()
    }

    /// Return known logical bytes only when the domain says the measurement applies.
    pub fn applicable_logical_bytes(&self) -> Option<u64> {
        self.logical_size_applicability
            .is_applicable()
            .then_some(self.logical_bytes)
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(bytes: Option<u64>) -> CleanupItem {
        serde_json::from_value(serde_json::json!({
            "id": "fixture", "parent_id": null, "name": "Fixture", "location": "/fixture",
            "logical_bytes": bytes, "reclaimable_bytes": null, "modified_at": null,
            "recommendation": "unavailable", "reason": "Fixture", "tags": [], "opaque": {},
        }))
        .unwrap()
    }

    #[test]
    fn legacy_unknown_and_measured_zero_keep_their_contract() {
        let unknown = item(None);
        assert_eq!(
            unknown.logical_size_applicability,
            SizeApplicability::Applicable
        );
        assert!(unknown.has_unknown_logical_size());
        let encoded = serde_json::to_value(&unknown).unwrap();
        assert!(encoded.get("logical_size_applicability").is_none());
        assert!(encoded["logical_bytes"].is_null());
        let zero = LogicalSizeSummary::from_items([&item(Some(0))]);
        assert_eq!(zero.known_bytes, 0);
        assert_eq!(zero.unknown_items, 0);
        assert_eq!(zero.applicability(), SizeApplicability::Applicable);
    }

    #[test]
    fn not_applicable_is_explicit_and_never_becomes_an_unknown_or_false_zero_group() {
        let mut current = item(None);
        current.logical_size_applicability = SizeApplicability::NotApplicable;
        assert!(!current.has_unknown_logical_size());
        let encoded = serde_json::to_value(&current).unwrap();
        assert_eq!(encoded["logical_size_applicability"], "not_applicable");
        assert!(encoded["logical_bytes"].is_null());
        assert_eq!(
            serde_json::from_value::<CleanupItem>(encoded).unwrap(),
            current
        );
        let summary = LogicalSizeSummary::from_items([&current]);
        assert_eq!(summary.not_applicable_items, 1);
        assert_eq!(summary.unknown_items, 0);
        assert_eq!(summary.applicability(), SizeApplicability::NotApplicable);
        let unknown = item(None);
        let known = item(Some(12));
        let mixed = LogicalSizeSummary::from_items([&current, &known, &unknown]);
        assert_eq!(mixed.known_bytes, 12);
        assert_eq!(mixed.unknown_items, 1);
        assert_eq!(mixed.not_applicable_items, 1);
        assert_eq!(mixed.applicability(), SizeApplicability::Applicable);
        assert_eq!(
            LogicalSizeSummary::default().applicability(),
            SizeApplicability::Applicable
        );
    }

    #[test]
    fn task_and_module_aggregation_use_leaf_applicability() {
        use crate::report::{ModuleReport, TaskReport};
        let mut parent = item(Some(1000));
        parent.id = "parent".into();
        let mut current = item(None);
        current.id = "current".into();
        current.parent_id = Some("parent".into());
        current.logical_size_applicability = SizeApplicability::NotApplicable;
        let task: TaskReport = serde_json::from_value(serde_json::json!({
            "module_id": "module", "task_id": "task", "name": "Task", "handler_id": "fixture",
            "request": {"module_id":"module", "task_id":"task", "name":"Task", "handler_id":"fixture", "options":{}, "recommend":{}, "tags":[]},
            "items": [parent, current], "issues": [], "error": null,
        })).unwrap();
        assert_eq!(
            task.logical_size_summary().applicability(),
            SizeApplicability::NotApplicable
        );
        assert_eq!(task.unknown_logical_items(), 0);
        assert_eq!(task.not_applicable_logical_items(), 1);
        let module = ModuleReport {
            id: "module".into(),
            name: "Module".into(),
            description: String::new(),
            tasks: vec![task],
        };
        assert_eq!(
            module.logical_size_summary().applicability(),
            SizeApplicability::NotApplicable
        );
        assert_eq!(module.unknown_logical_items(), 0);
        assert_eq!(module.not_applicable_logical_items(), 1);
    }
}
