//! Public OS capability and error data, independent of a native authorization backend.

use serde::{Deserialize, Serialize};

/// Why an operation stopped before finishing its requested scope.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationStopReason {
    /// The caller explicitly requested cancellation.
    Cancelled,
    /// The original operation's execution deadline was reached.
    DeadlineExceeded,
}

/// Stable host operation identity carried by native work and lifecycle events.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct OperationIdentity {
    /// Caller-owned operation ID; not an event sequence or observed scan ID.
    pub operation_id: String,
    /// Attempt generation supplied by the owner of this operation.
    pub generation: u64,
}

/// A same-host IPC operation budget. Correlation and limits do not grant native capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationTransfer {
    /// Product display language for this request; omission preserves English machine output.
    /// This field is presentation-only and cannot grant or widen a capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_language: Option<String>,
    /// Original operation identity, retained across native process boundaries.
    pub identity: OperationIdentity,
    /// Absolute deadline on the system monotonic clock, in nanoseconds.
    pub deadline_monotonic_ns: u64,
    /// Maximum total response bytes for this operation.
    pub output_bytes: u64,
    /// Maximum protocol line length.
    pub line_bytes: u64,
    /// Maximum wait before initial output.
    pub first_output_timeout_ms: u64,
    /// Maximum interval between output chunks.
    pub idle_timeout_ms: u64,
}

/// The operation class of a native capability; disclosure consent is a separate host policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    /// Enumerate names and metadata.
    MetadataRead,
    /// Read file bytes; this does not authorize disclosure to another process or service.
    ContentRead,
    /// Remove explicitly confirmed filesystem objects.
    FilesystemDelete,
    /// Invoke a registered system lifecycle operation.
    SystemLifecycle,
}

/// How the caller can participate in native authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interaction {
    /// A foreground graphical application.
    Foreground,
    /// An interactive terminal with the system's authentication mechanism.
    Terminal,
    /// Background operation; never launch a hidden authorization prompt.
    NonInteractive,
}

/// Stable outcomes reported by a native backend, without interpreting localized messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOutcome {
    /// An actual operation or backend handshake established the capability.
    Granted,
    /// The native backend explicitly reported refusal.
    Denied,
    /// The caller cancelled the operation.
    Cancelled,
    /// The authorization deadline elapsed.
    TimedOut,
    /// The OS requires an explicit change in system settings.
    NeedsSystemSettings,
    /// The installed helper could not be found.
    HelperMissing,
    /// The helper identity or installation permissions are not trusted.
    HelperUntrusted,
    /// The installed helper predates the required scoped identity protocol.
    HelperOutdated,
    /// Authorization did not complete; the native error does not establish a narrower cause.
    AuthorizationUnavailable,
    /// This operation needs an interactive authorization context.
    NeedsAuthorization,
    /// The backend or protocol cannot perform this operation.
    Unsupported,
    /// A system policy prevents the operation.
    PolicyBlocked,
    /// The target volume cannot be modified.
    ReadOnlyVolume,
}

/// Structured filesystem/process failure class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IoFailureKind {
    /// The exact resource is absent.
    NotFound,
    /// OS access rights were insufficient.
    AccessDenied,
    /// Another process holds an incompatible handle.
    SharingViolation,
    /// The underlying volume is read-only.
    ReadOnlyVolume,
    /// An OS policy prevents the requested operation.
    PolicyBlocked,
    /// The operation was cancelled.
    Cancelled,
    /// The operation exceeded its deadline.
    TimedOut,
    /// The selected identity or its containing boundary changed.
    TargetChanged,
    /// A symbolic link, reparse point, or mount violates the allowed traversal boundary.
    BoundaryViolation,
    /// A resource budget was exhausted.
    ResourceLimit,
    /// A backend/protocol operation is unsupported.
    Unsupported,
    /// An error with no more specific portable classification.
    Other,
}

/// A portable classification plus the unmodified native error code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IoFailure {
    /// Machine-readable category; display text is never used for authorization decisions.
    pub kind: IoFailureKind,
    /// Original OS error when available.
    pub os_code: Option<i32>,
}

/// A lossless local path representation. Display strings must not be used as execution paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "encoding", content = "units", rename_all = "snake_case")]
pub enum NativePath {
    /// Unix filename bytes, including names which are not UTF-8.
    Unix(Vec<u8>),
    /// Windows UTF-16 code units, including unpaired surrogates.
    Windows(Vec<u16>),
}

/// Host-validated resource scope, separate from a native process's ambient privileges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CapabilityScope {
    /// Explicit local resource roots. No model-supplied display path is accepted here.
    Filesystem {
        /// Exact roots of this request.
        roots: Vec<NativePath>,
    },
    /// A registered system handler and its original selected domain objects.
    System {
        /// Registered handler identity.
        handler: String,
        /// Exact configuration or object identities.
        objects: Vec<String>,
    },
}

/// Native capability request issued by a trusted local host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRequest {
    /// Local user and login-session identity established by the host.
    pub subject: String,
    /// Original operation identity; authorization retries cannot silently create a new operation.
    pub operation_id: String,
    /// The requested native capability class.
    pub kind: CapabilityKind,
    /// Explicitly registered resource scope.
    pub scope: CapabilityScope,
    /// Allowed native interaction channel.
    pub interaction: Interaction,
    /// Stable purpose code; display text is not an authority input.
    pub reason_code: String,
}

impl NativePath {
    /// Capture the native path without a lossy display conversion.
    pub fn from_path(path: &std::path::Path) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Self::Unix(path.as_os_str().as_bytes().to_vec())
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            Self::Windows(path.as_os_str().encode_wide().collect())
        }
    }

    /// Decode only paths for the current platform. Cross-device imports require rebinding.
    pub fn to_path_buf(&self) -> Option<std::path::PathBuf> {
        match self {
            #[cfg(unix)]
            Self::Unix(bytes) if !bytes.contains(&0) => {
                use std::os::unix::ffi::OsStringExt;
                Some(std::ffi::OsString::from_vec(bytes.clone()).into())
            }
            #[cfg(windows)]
            Self::Windows(units) if !units.contains(&0) => {
                use std::os::windows::ffi::OsStringExt;
                Some(std::ffi::OsString::from_wide(units).into())
            }
            _ => None,
        }
    }
}
