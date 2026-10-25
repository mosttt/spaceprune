//! Frozen native cache sources shared by ordinary inspection and authorized retry.

use crate::{access::NativePath, handler::TaskRequest, rule::FileIdentity};
use serde::{Deserialize, Serialize};

/// Identity of a directory and its parent at inspection time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryBinding {
    /// Native execution path, never reconstructed from a display label.
    pub path: NativePath,
    /// Parent directory identity.
    pub parent_identity: FileIdentity,
    /// Selected directory identity.
    pub identity: FileIdentity,
}

/// A domain source observed by the invoking user before requesting native access.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemCacheSource {
    /// Registered action selected by ordinary discovery.
    pub action_id: String,
    /// Exact native roots to inspect without repeating discovery as an administrator.
    pub roots: Vec<NativePath>,
    /// Directory identity already observed for a Go cache, when available.
    pub binding: Option<DirectoryBinding>,
    /// Whether the invoking user's domain command was available.
    pub program_available: bool,
}

/// Read-only inspection request preserving the original rule and discovered source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemCacheScanRequest {
    /// Original registered task; no command or environment is added during retry.
    pub task: TaskRequest,
    /// Frozen ordinary discovery result.
    pub source: SystemCacheSource,
}
