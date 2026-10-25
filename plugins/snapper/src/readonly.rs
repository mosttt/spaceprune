use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use spaceprune_plugin_api::PluginResult;

use super::{Snapshot, SnapshotGroup, group_pairs};

/// A locally discovered Snapper configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapperConfig {
    /// Configuration name accepted by Snapper.
    pub name: String,
    /// Local subvolume managed by this configuration.
    pub subvolume: PathBuf,
}

/// Metadata reported by Snapper without traversing snapshot contents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapperSnapshot {
    /// Snapshot number within its configuration.
    pub number: u64,
    /// Creation date in UTC, as reported by Snapper.
    pub date: String,
    /// Snapper snapshot type, such as single, pre, or post.
    pub snapshot_type: String,
    /// Associated pre-snapshot number for a post snapshot.
    pub pre_number: Option<u64>,
    /// Whether this snapshot is currently active.
    pub active: bool,
    /// Whether this snapshot is the default subvolume.
    pub is_default: bool,
    /// Reported used space, when available; not guaranteed reclaimable space.
    pub used_bytes: Option<u64>,
    /// Local snapshot description, which may contain private text.
    pub description: String,
    /// Snapper cleanup algorithm label.
    pub cleanup: String,
}

/// An indivisible selection containing one snapshot or a complete pre/post pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapperSnapshotGroup {
    /// All snapshot numbers that must be selected together.
    pub numbers: Vec<u64>,
    /// Latest creation date in this group, in UTC.
    pub date: String,
    /// Snapshot type, or pre/post for a paired group.
    pub snapshot_type: String,
    /// Whether a current, default, or active snapshot prevents deletion.
    pub protected: bool,
    /// Sum of reported sizes, absent when any member has unknown size.
    pub used_bytes: Option<u64>,
    /// Individual metadata records for local detail views.
    pub snapshots: Vec<SnapperSnapshot>,
}

/// Read the local host identity used to bind explicit snapshot selections.
///
/// Store this in a one-time scan result or explicit fixed selection, never a reusable rule.
/// It is not analysis evidence for the model.
pub fn snapper_host_id() -> PluginResult<String> {
    let id = std::fs::read_to_string("/etc/machine-id").map_err(|_| {
        spaceprune_plugin_api::PluginError::new(
            "snapshot-host-unavailable",
            spaceprune_plugin_api::messages::text("snapper.identify_host"),
        )
    })?;
    let id = id.trim();
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(spaceprune_plugin_api::PluginError::new(
            "snapshot-host-unavailable",
            spaceprune_plugin_api::messages::text("snapper.invalid_host"),
        ));
    }
    Ok(id.to_ascii_lowercase())
}

pub(super) fn group_summary(group: &SnapshotGroup) -> SnapperSnapshotGroup {
    SnapperSnapshotGroup {
        numbers: group.numbers(),
        date: group
            .snapshots
            .iter()
            .map(|snapshot| &snapshot.date)
            .max()
            .cloned()
            .unwrap_or_default(),
        snapshot_type: if group.snapshots.len() > 1 {
            "pre/post".to_owned()
        } else {
            group.snapshots[0].snapshot_type.clone()
        },
        protected: group
            .snapshots
            .iter()
            .any(|snapshot| snapshot.number == 0 || snapshot.active || snapshot.is_default),
        used_bytes: group.snapshots.iter().try_fold(0_u64, |sum, snapshot| {
            used_bytes(snapshot).map(|bytes| sum.saturating_add(bytes))
        }),
        snapshots: group
            .snapshots
            .iter()
            .map(|snapshot| SnapperSnapshot {
                number: snapshot.number,
                date: snapshot.date.clone(),
                snapshot_type: snapshot.snapshot_type.clone(),
                pre_number: snapshot.pre_number,
                active: snapshot.active,
                is_default: snapshot.is_default,
                used_bytes: used_bytes(snapshot),
                description: snapshot.description.clone(),
                cleanup: snapshot.cleanup.clone(),
            })
            .collect(),
    }
}

fn used_bytes(snapshot: &Snapshot) -> Option<u64> {
    snapshot.used_space.as_u64().or_else(|| {
        snapshot
            .used_space
            .as_str()
            .and_then(|value| super::core_fs::parse_bytes(value).ok())
    })
}

/// Inspect the registered system domain with ordinary privileges and the original context.
/// A native host may retry this call through the metadata service on AccessDenied.
pub fn snapper_inventory_unprivileged(
    program: &str,
    config: Option<&str>,
    context: &spaceprune_plugin_api::OperationContext,
) -> PluginResult<SnapperInventory> {
    match config {
        Some(config) => Ok(SnapperInventory::Snapshots(
            group_pairs(super::list_snapshots_cancellable(
                program, config, true, context,
            )?)
            .iter()
            .map(group_summary)
            .collect(),
        )),
        None => Ok(SnapperInventory::Configs(
            super::list_configs_cancellable(program, context)?
                .into_iter()
                .map(|(name, subvolume)| SnapperConfig { name, subvolume })
                .collect(),
        )),
    }
}

/// A typed metadata result for the registered Snapper system domain.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "items",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum SnapperInventory {
    /// Available configurations without reading their snapshot contents.
    Configs(Vec<SnapperConfig>),
    /// Snapshot groups within one previously discovered configuration.
    Snapshots(Vec<SnapperSnapshotGroup>),
}
