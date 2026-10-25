//! Local mounted-volume capacity, independent of cleanup estimates.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One mounted local filesystem; aliases share a single capacity record.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageVolume {
    /// Native volume identity or device name.
    pub id: String,
    /// Human-readable device or volume name.
    pub name: String,
    /// Mount locations for this filesystem, ordered with the shortest path first.
    pub mount_points: Vec<String>,
    /// Filesystem capacity reported by the OS, or unknown if querying failed.
    pub total_bytes: Option<u64>,
    /// Allocated capacity, excluding free blocks reserved by the OS.
    pub used_bytes: Option<u64>,
    /// Free capacity available to the current user.
    pub available_bytes: Option<u64>,
}
