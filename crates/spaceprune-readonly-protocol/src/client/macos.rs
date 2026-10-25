//! macOS privacy recovery stays in the original process, where TCC applies the grant.

use crate::{HelperOperation, PreparedRequest, ResponseOutcome, filesystem_scan_roots};
use spaceprune_access::{
    CapabilityKind, CapabilityRequest, CapabilityScope, Interaction, IoFailure, IoFailureKind,
    NativePath, OperationContext, PermissionOutcome, permission::PermissionBroker,
};
use spaceprune_plugin_api::{HandlerScanResult, NullProgress, ProgressSink, TaskRequest};
use std::{path::PathBuf, time::Duration};

/// Keeps ordinary-process privacy recovery separate from the authenticated administrator service.
#[derive(Default)]
pub struct MetadataService {
    broker: PermissionBroker,
    #[cfg(target_os = "macos")]
    administrator: crate::administrator::MetadataService,
}

#[cfg(target_os = "macos")]
pub(super) use crate::administrator::request_scope;

impl MetadataService {
    /// Report only the administrator connection; privacy access is still verified at the actual target.
    pub fn is_active(&self) -> bool {
        #[cfg(target_os = "macos")]
        return self.administrator.is_active();
        #[cfg(not(target_os = "macos"))]
        false
    }

    /// Release pending settings requests and report administrator-service shutdown confirmation.
    /// This cannot revoke OS-owned privacy grants.
    pub fn close(&self) -> Result<(), PermissionOutcome> {
        self.broker.revoke();
        #[cfg(target_os = "macos")]
        return self.administrator.close();
        #[cfg(not(target_os = "macos"))]
        Ok(())
    }

    /// Recover only the original filesystem task after its actual ordinary scan was denied.
    pub fn recover_scan(
        &self,
        task: &TaskRequest,
        result: &HandlerScanResult,
        operation: &OperationContext,
    ) -> Result<HandlerScanResult, PermissionOutcome> {
        self.recover_scan_with_progress(task, result, operation, &NullProgress)
    }

    /// Preserve the ordinary scan's progress/input owner while native access is requested.
    pub fn recover_scan_with_progress(
        &self,
        task: &TaskRequest,
        result: &HandlerScanResult,
        operation: &OperationContext,
        progress: &dyn ProgressSink,
    ) -> Result<HandlerScanResult, PermissionOutcome> {
        #[cfg(not(target_os = "macos"))]
        let _ = progress;
        check(operation)?;
        let roots = filesystem_scan_roots(task).map_err(|_| PermissionOutcome::PolicyBlocked)?;
        if task
            .options
            .get("elevation")
            .and_then(serde_json::Value::as_str)
            == Some("never")
        {
            return Ok(result.clone());
        }
        let denied = result
            .issues
            .iter()
            .filter_map(|issue| issue.io_failure.as_ref())
            .filter(|failure| failure.kind == IoFailureKind::AccessDenied)
            .min_by_key(|failure| failure.os_code != Some(rustix::io::Errno::PERM.raw_os_error()));
        if let Some(failure) = denied {
            #[cfg(target_os = "macos")]
            if failure.os_code == Some(rustix::io::Errno::ACCESS.raw_os_error()) {
                return self
                    .administrator
                    .scan_with_progress(task, operation, progress);
            }
            return Err(self.recover_denial(&roots, failure, operation));
        }
        Ok(result.clone())
    }

    /// Verify the exact registered boundary under the host's current OS permissions.
    pub fn measure(
        &self,
        request: &PreparedRequest,
        operation: &OperationContext,
    ) -> Result<ResponseOutcome, PermissionOutcome> {
        check(operation)?;
        if request.context.identity() != operation.identity() {
            return Err(PermissionOutcome::PolicyBlocked);
        }
        let decoded = &request.descriptor;
        if decoded.context.identity != request.context.identity()
            || !matches!(
                decoded.operation,
                HelperOperation::MeasureRegisteredSource
                    | HelperOperation::MeasureNavigationBoundary
            )
        {
            return Err(PermissionOutcome::PolicyBlocked);
        }
        let context = request.context.child(
            operation
                .deadline
                .saturating_duration_since(std::time::Instant::now()),
        );
        check(&context)?;
        let result = crate::measurement::measure_roots(&decoded.roots, None, None, Some(&context))
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
        check(&context)?;
        if let Some(failure) = &result.failure
            && failure.kind == IoFailureKind::AccessDenied
        {
            #[cfg(target_os = "macos")]
            if failure.os_code == Some(rustix::io::Errno::ACCESS.raw_os_error()) {
                return self.administrator.measure(request, &context);
            }
            return Err(self.recover_denial(&decoded.roots, failure, &context));
        }
        if result.accessible_roots == 0 {
            return Err(PermissionOutcome::PolicyBlocked);
        }
        Ok(ResponseOutcome::Measured {
            logical_bytes: result.logical_bytes,
            inspected_entries: result.inspected_entries,
            incomplete: result.skipped_entries > 0,
        })
    }

    fn recover_denial(
        &self,
        roots: &[PathBuf],
        failure: &IoFailure,
        operation: &OperationContext,
    ) -> PermissionOutcome {
        self.recover_denial_with(roots, failure, operation, open_privacy_settings)
    }

    fn recover_denial_with(
        &self,
        roots: &[PathBuf],
        failure: &IoFailure,
        operation: &OperationContext,
        open: impl FnOnce(&OperationContext) -> Result<(), PermissionOutcome>,
    ) -> PermissionOutcome {
        if let Err(outcome) = check(operation) {
            return outcome;
        }
        if roots.is_empty() || failure.kind != IoFailureKind::AccessDenied {
            return PermissionOutcome::PolicyBlocked;
        }
        // EACCES is a DAC/ACL denial. EPERM is a policy refusal, which may be TCC, sandbox
        // or another system restriction; it is never proof that FDA or root will solve it.
        if failure.os_code != Some(rustix::io::Errno::PERM.raw_os_error()) {
            return PermissionOutcome::NeedsAuthorization;
        }
        if operation.interaction == Interaction::NonInteractive {
            return PermissionOutcome::NeedsSystemSettings;
        }
        let request = CapabilityRequest {
            subject: format!("macos-uid:{}", rustix::process::geteuid().as_raw()),
            operation_id: operation.id.clone(),
            kind: CapabilityKind::MetadataRead,
            scope: CapabilityScope::Filesystem {
                roots: roots
                    .iter()
                    .map(|root| NativePath::from_path(root))
                    .collect(),
            },
            interaction: operation.interaction,
            reason_code: "metadata-policy-review".into(),
        };
        match self
            .broker
            .acquire(&request, operation, None, |_, operation| {
                open(operation)?;
                Err(PermissionOutcome::NeedsSystemSettings)
            }) {
            Err(outcome) => outcome,
            Ok(_) => PermissionOutcome::PolicyBlocked,
        }
    }
}

fn check(operation: &OperationContext) -> Result<(), PermissionOutcome> {
    operation.check().map_err(|error| operation_error(&error))
}

fn operation_error(error: &std::io::Error) -> PermissionOutcome {
    match error.kind() {
        std::io::ErrorKind::Interrupted => PermissionOutcome::Cancelled,
        std::io::ErrorKind::TimedOut => PermissionOutcome::TimedOut,
        _ => PermissionOutcome::AuthorizationUnavailable,
    }
}

fn open_privacy_settings(operation: &OperationContext) -> Result<(), PermissionOutcome> {
    #[cfg(target_os = "macos")]
    {
        let executable =
            spaceprune_access::process::trusted_executable(std::path::Path::new("/usr/bin/open"))
                .ok_or(PermissionOutcome::AuthorizationUnavailable)?;
        let mut command = std::process::Command::new(executable);
        command.arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles");
        let result =
            spaceprune_access::process::capture(command, &operation.child(Duration::from_secs(10)))
                .map_err(|error| operation_error(&error))?;
        if result.status.success() {
            Ok(())
        } else {
            Err(PermissionOutcome::AuthorizationUnavailable)
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = operation;
        Err(PermissionOutcome::Unsupported)
    }
}

#[cfg(test)]
#[path = "macos/tests.rs"]
mod tests;
