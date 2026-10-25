//! Trusted construction and single-consumer ownership of immutable cleanup plans.

use super::*;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};

/// Process-local consumption state shared by every plan derived from one scan.
/// It is not serialized and cannot be restored as an authorization credential.
#[derive(Debug, Clone, Default)]
pub struct ScanReservation(Arc<AtomicBool>);
impl PartialEq for ScanReservation {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

pub(crate) fn new_scan_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn digest(value: &impl Serialize) -> String {
    fn ordered(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(fields) => serde_json::Value::Object(
                fields
                    .into_iter()
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .map(|(key, value)| (key, ordered(value)))
                    .collect(),
            ),
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.into_iter().map(ordered).collect())
            }
            other => other,
        }
    }
    let value = ordered(serde_json::to_value(value).expect("plan serializes"));
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&value).expect("plan JSON"))
    )
}

/// An immutable, one-use execution plan issued by the host from a scan.
/// A deserialized `CleanupPlanData` is only an observation, not this runtime capability.
///
/// ```compile_fail
/// let _: spaceprune_engine::CleanupPlan = serde_json::from_str("{}").unwrap();
/// ```
///
/// ```compile_fail
/// let observed: spaceprune_engine::CleanupPlanData = serde_json::from_str("{}").unwrap();
/// let executable: spaceprune_engine::CleanupPlan = observed.into();
/// ```
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CleanupPlan {
    #[serde(flatten)]
    report: CleanupPlanData,
    #[serde(skip)]
    reservation: ScanReservation,
}

impl std::ops::Deref for CleanupPlan {
    type Target = CleanupPlanData;
    fn deref(&self) -> &Self::Target {
        &self.report
    }
}

impl CleanupPlan {
    pub(super) fn from_scan(scan: &ScanReport, groups: Vec<PlanGroup>) -> Self {
        let revision = digest(
            &groups
                .iter()
                .map(|group| &group.request)
                .collect::<Vec<_>>(),
        );
        let mut plan = Self {
            report: CleanupPlanData {
                plan_id: uuid::Uuid::new_v4().to_string(),
                scan_id: scan.scan_id.clone(),
                recipe_revision: revision,
                digest: String::new(),
                config_name: scan.config_name.clone(),
                groups,
            },
            reservation: scan.reservation.clone(),
        };
        plan.report.digest = plan.compute_digest();
        plan
    }

    fn compute_digest(&self) -> String {
        digest(&(
            &self.plan_id,
            &self.scan_id,
            &self.recipe_revision,
            &self.config_name,
            &self.groups,
        ))
    }

    pub(super) fn reserve(&self) -> Result<(), &'static str> {
        if self.digest != self.compute_digest() {
            return Err("cleanup-plan-changed");
        }
        self.reservation
            .0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| "cleanup-scan-already-consumed")
    }

    /// Exact original groups. Callers cannot mutate a confirmed selection.
    pub fn groups(&self) -> &[PlanGroup] {
        &self.groups
    }
    /// Stable identity of this plan.
    pub fn id(&self) -> &str {
        &self.plan_id
    }
    /// Source scan identity.
    pub fn scan_id(&self) -> &str {
        &self.scan_id
    }
    /// Frozen task configuration identity.
    pub fn recipe_revision(&self) -> &str {
        &self.recipe_revision
    }
    /// Exact selection digest to bind confirmation and journal entries.
    pub fn digest(&self) -> &str {
        &self.digest
    }
}
