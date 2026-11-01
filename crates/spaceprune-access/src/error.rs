//! Structured classification of native IO errors.

use spaceprune_contracts::access::{IoFailure, IoFailureKind};
use std::io;

/// Classify native errors without inspecting their translated display messages.
pub fn classify(error: &io::Error) -> IoFailure {
    if let Some(failure) = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<NativeFailure>())
    {
        return failure.failure.clone();
    }
    let os_code = error.raw_os_error();
    #[cfg(windows)]
    let native = match os_code {
        Some(32 | 33) => Some(IoFailureKind::SharingViolation),
        Some(19) => Some(IoFailureKind::ReadOnlyVolume),
        Some(1260) => Some(IoFailureKind::PolicyBlocked),
        _ => None,
    };
    #[cfg(unix)]
    let native = if os_code == Some(rustix::io::Errno::ROFS.raw_os_error()) {
        Some(IoFailureKind::ReadOnlyVolume)
    } else {
        None
    };
    IoFailure {
        kind: native.unwrap_or_else(|| match error.kind() {
            io::ErrorKind::NotFound => IoFailureKind::NotFound,
            io::ErrorKind::PermissionDenied => IoFailureKind::AccessDenied,
            io::ErrorKind::Interrupted => IoFailureKind::Cancelled,
            io::ErrorKind::TimedOut => IoFailureKind::TimedOut,
            io::ErrorKind::Unsupported => IoFailureKind::Unsupported,
            io::ErrorKind::ReadOnlyFilesystem => IoFailureKind::ReadOnlyVolume,
            _ => IoFailureKind::Other,
        }),
        os_code,
    }
}

#[derive(Debug)]
struct NativeFailure {
    failure: IoFailure,
    message: String,
    effects: bool,
    file_usage: Option<spaceprune_contracts::access::FileUsage>,
}

impl std::fmt::Display for NativeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}
impl std::error::Error for NativeFailure {}

/// Attach a stable classification to a failure detected by a native safety primitive.
pub fn failure(kind: IoFailureKind, message: impl Into<String>) -> io::Error {
    io::Error::other(NativeFailure {
        failure: IoFailure {
            kind,
            os_code: None,
        },
        message: message.into(),
        effects: false,
        file_usage: None,
    })
}

/// Preserve the OS code when a native primitive can classify its failure more precisely.
#[cfg(unix)]
pub(crate) fn failure_with_cause(
    kind: IoFailureKind,
    message: impl Into<String>,
    cause: &io::Error,
) -> io::Error {
    io::Error::other(NativeFailure {
        failure: IoFailure {
            kind,
            os_code: cause.raw_os_error(),
        },
        message: message.into(),
        effects: false,
        file_usage: None,
    })
}

/// Report a native failure after a known persistent effect, preserving its original OS code.
pub fn failure_after_effects(
    kind: IoFailureKind,
    message: impl Into<String>,
    cause: &io::Error,
) -> io::Error {
    io::Error::other(NativeFailure {
        failure: IoFailure {
            kind,
            os_code: cause.raw_os_error(),
        },
        message: message.into(),
        effects: true,
        file_usage: file_usage(cause),
    })
}

/// Whether the primitive reports known effects, such as a selected object retained in staging.
pub fn has_effects(error: &io::Error) -> bool {
    error
        .get_ref()
        .and_then(|source| source.downcast_ref::<NativeFailure>())
        .is_some_and(|failure| failure.effects)
}

/// Read display-only file usage captured by the failing native operation.
pub fn file_usage(error: &io::Error) -> Option<spaceprune_contracts::access::FileUsage> {
    error
        .get_ref()
        .and_then(|source| source.downcast_ref::<NativeFailure>())
        .and_then(|failure| failure.file_usage.clone())
}

#[cfg(windows)]
pub(crate) fn with_file_usage(error: io::Error, path: &std::path::Path) -> io::Error {
    let failure = classify(&error);
    if failure.kind != IoFailureKind::SharingViolation {
        return error;
    }
    io::Error::new(
        error.kind(),
        NativeFailure {
            failure,
            effects: has_effects(&error),
            message: error.to_string(),
            file_usage: Some(crate::file_usage::inspect(path)),
        },
    )
}
