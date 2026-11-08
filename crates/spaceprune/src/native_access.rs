//! Adapter between engine scan failures and the shared native metadata service.

use spaceprune_plugin_api::{
    HandlerScanResult, OperationContext, PluginResult, ProgressSink, ScanAccess, TaskRequest,
};
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
use std::sync::Arc;

#[cfg(target_os = "linux")]
mod snapper;
#[cfg(target_os = "linux")]
pub use snapper::{snapper_configs, snapper_snapshots};

/// Applies native authorization only after a registered handler reports insufficient access.
pub struct NativeScanAccess {
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    service: Arc<spaceprune_readonly_protocol::client::MetadataService>,
}

impl NativeScanAccess {
    /// Use the process-wide service so independent scans can reuse the same live connection.
    pub fn new() -> Self {
        Self {
            #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
            service: shared_metadata_service(),
        }
    }

    /// Use a host-owned service shared with other local metadata consumers.
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    pub fn with_service(
        service: Arc<spaceprune_readonly_protocol::client::MetadataService>,
    ) -> Self {
        Self { service }
    }
}

/// The process-wide metadata service holds real native connections, never stored passwords.
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
pub fn shared_metadata_service() -> Arc<spaceprune_readonly_protocol::client::MetadataService> {
    static SERVICE: std::sync::OnceLock<
        Arc<spaceprune_readonly_protocol::client::MetadataService>,
    > = std::sync::OnceLock::new();
    Arc::clone(SERVICE.get_or_init(|| Arc::new(Default::default())))
}

impl ScanAccess for NativeScanAccess {
    fn manages_authorization(&self, task: &TaskRequest) -> bool {
        (cfg!(any(
            target_os = "linux",
            target_os = "windows",
            target_os = "macos"
        )) && task.handler_id == "builtin.filesystem/paths")
            || (cfg!(target_os = "linux")
                && matches!(
                    task.handler_id.as_str(),
                    "builtin.snapper/snapshots"
                        | "builtin.system-linux/package-cache"
                        | "builtin.system-linux/coredumps"
                        | "builtin.system-linux/journal"
                        | "builtin.system-linux/tmpfiles"
                        | "builtin.system-linux/go-modcache"
                        | "builtin.developer-tools/go-modcache"
                        | "builtin.developer-tools/go-buildcache"
                        | "builtin.developer-tools/cargo-registry-cache"
                        | "builtin.developer-tools/npm-cache"
                ))
    }

    fn retry(
        &self,
        task: &TaskRequest,
        result: HandlerScanResult,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult> {
        use spaceprune_access::IoFailureKind;
        if !self.manages_authorization(task)
            || context.is_stopped()
            || task
                .options
                .get("elevation")
                .and_then(serde_json::Value::as_str)
                == Some("never")
            || !result.issues.iter().any(|issue| {
                issue
                    .io_failure
                    .as_ref()
                    .is_some_and(|failure| failure.kind == IoFailureKind::AccessDenied)
            })
        {
            return Ok(result);
        }
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
        {
            use spaceprune_access::PermissionOutcome;
            use spaceprune_plugin_api::{HandlerIssue, IssueLevel, ProgressEvent, ProgressPhase};
            #[cfg(target_os = "linux")]
            let cache = if (task.handler_id.starts_with("builtin.system-linux/")
                || task.handler_id.starts_with("builtin.developer-tools/"))
                && task.handler_id != "builtin.system-linux/tmpfiles"
            {
                let Some(source) =
                    spaceprune_readonly_protocol::client::prepare_system_cache_scan(task, &result)?
                else {
                    return Ok(result);
                };
                Some(source)
            } else {
                None
            };
            let mut result = result;
            let timeout = std::time::Duration::from_secs(120);
            let operation = context.child(timeout);
            let reusing = self.service.is_active();
            progress.emit(ProgressEvent {
                phase: if reusing {
                    ProgressPhase::Measure
                } else {
                    ProgressPhase::Authorize
                },
                module_id: Some(task.module_id.clone()),
                task_id: Some(task.task_id.clone()),
                handler_id: Some(task.handler_id.clone()),
                current: None,
                total: None,
                message: if reusing {
                    spaceprune_contracts::messages::message(
                        "native.reusing_metadata",
                        &[("p0", (task.name).to_string())],
                    )
                } else {
                    spaceprune_contracts::messages::message(
                        "native.requesting_metadata",
                        &[("p0", (task.name).to_string())],
                    )
                },
            });
            #[cfg(target_os = "linux")]
            let scanned = if let Some(source) = cache {
                self.service
                    .scan_system_cache_with_progress(&source, &operation, progress)
            } else {
                self.service.scan_with_progress(task, &operation, progress)
            };
            #[cfg(target_os = "windows")]
            let scanned = self.service.scan_with_progress(task, &operation, progress);
            #[cfg(target_os = "macos")]
            let scanned = self
                .service
                .recover_scan_with_progress(task, &result, &operation, progress);
            match scanned {
                Ok(scanned) => return Ok(scanned),
                Err(error) => {
                    let (code, message) = match error {
                        PermissionOutcome::Denied => (
                            "authorization-denied",
                            spaceprune_contracts::messages::text("native.metadata_declined"),
                        ),
                        PermissionOutcome::Cancelled => (
                            "authorization-cancelled",
                            spaceprune_contracts::messages::text("native.metadata_cancelled"),
                        ),
                        PermissionOutcome::TimedOut => (
                            "authorization-timeout",
                            spaceprune_contracts::messages::text("native.metadata_timeout"),
                        ),
                        PermissionOutcome::NeedsAuthorization => (
                            "authorization-required",
                            spaceprune_contracts::messages::text("native.metadata_foreground"),
                        ),
                        PermissionOutcome::NeedsSystemSettings => (
                            "authorization-system-settings",
                            spaceprune_contracts::messages::text("native.metadata_macos_policy"),
                        ),
                        #[cfg(target_os = "windows")]
                        PermissionOutcome::HelperMissing
                            if matches!(
                                spaceprune_access::windows_native::embedded_identity(),
                                Ok(None)
                            ) =>
                        {
                            (
                                "authorization-helper-missing",
                                spaceprune_contracts::messages::text(
                                    "native.windows_components_not_bundled",
                                ),
                            )
                        }
                        PermissionOutcome::HelperMissing => (
                            "authorization-helper-missing",
                            spaceprune_contracts::messages::text("native.metadata_helper_missing"),
                        ),
                        PermissionOutcome::HelperUntrusted => (
                            "authorization-helper-untrusted",
                            spaceprune_contracts::messages::text(
                                "native.metadata_helper_untrusted",
                            ),
                        ),
                        PermissionOutcome::HelperOutdated => (
                            "authorization-helper-outdated",
                            spaceprune_contracts::messages::text("native.metadata_helper_outdated"),
                        ),
                        PermissionOutcome::PolicyBlocked => (
                            "authorization-scope-rejected",
                            spaceprune_contracts::messages::text("native.metadata_scope"),
                        ),
                        _ => (
                            "authorization-unavailable",
                            spaceprune_contracts::messages::text("native.metadata_unavailable"),
                        ),
                    };
                    result.issues.push(HandlerIssue {
                        code: Some(code.to_owned()),
                        io_failure: None,
                        level: IssueLevel::Warning,
                        message: message.to_owned(),
                        location: None,
                    });
                }
            }
            Ok(result)
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
        {
            let _ = (progress, context);
            Ok(result)
        }
    }
}

impl Default for NativeScanAccess {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn default_scan_access() -> NativeScanAccess {
    NativeScanAccess::new()
}
