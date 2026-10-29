//! Automation completion combines inspection coverage with actual cleanup effects.

use serde::{Serialize, Serializer, ser::SerializeStruct};

use crate::api::{
    CleanupPlan, ExecutionReport, IoFailureKind, IssueLevel, ItemExecutionStatus,
    OperationStopReason, PermissionOutcome, ScanReport,
};

/// Final outcome of one automation; readable work can finish while other work needs attention.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRunStatus {
    /// All configured work finished without reported issues.
    Completed,
    /// Cleanup finished, but scan warnings need review.
    CompletedWithIssues,
    /// Some inspection or cleanup work still requires foreground authorization.
    NeedsAuthorization,
    /// A non-authorization error or deadline prevented completion.
    Failed,
    /// The operation was cancelled, retaining any completed effects.
    Cancelled,
}

impl AutomationRunStatus {
    /// Classify reports without interpreting translated error messages or retrying any work.
    pub fn from_reports(scan: &ScanReport, execution: &ExecutionReport) -> Self {
        if [scan.stop_reason, execution.stop_reason]
            .contains(&Some(OperationStopReason::DeadlineExceeded))
        {
            return Self::Failed;
        }
        if !execution.operation_failures.is_empty() {
            return Self::Failed;
        }
        if scan.cancelled || execution.cancelled {
            return Self::Cancelled;
        }
        let mut needs_authorization = false;
        let mut has_issues = false;
        for task in scan.modules.iter().flat_map(|module| &module.tasks) {
            if task.error.is_some() {
                return Self::Failed;
            }
            for issue in &task.issues {
                has_issues = true;
                let authorization = matches!(
                    issue.code.as_deref(),
                    Some(
                        "authorization-required"
                            | "authorization-denied"
                            | "authorization-cancelled"
                            | "authorization-system-settings"
                    )
                ) || issue
                    .io_failure
                    .as_ref()
                    .is_some_and(|failure| failure.kind == IoFailureKind::AccessDenied);
                // Broken components and rejected scopes are not repaired by another password prompt.
                if matches!(
                    issue.code.as_deref(),
                    Some(
                        "authorization-helper-missing"
                            | "authorization-helper-untrusted"
                            | "authorization-helper-outdated"
                            | "authorization-scope-rejected"
                            | "authorization-unavailable"
                            | "authorization-timeout"
                    )
                ) || (issue.level == IssueLevel::Error && !authorization)
                {
                    return Self::Failed;
                }
                needs_authorization |= authorization;
            }
        }
        for group in &execution.groups {
            if group.error.is_some() {
                return Self::Failed;
            }
            if let Some(result) = &group.result {
                for failure in &result.failures {
                    let busy_skip = failure
                        .io_failure
                        .as_ref()
                        .is_some_and(|io| io.kind == IoFailureKind::SharingViolation)
                        && matches!(
                            failure.authorization,
                            None | Some(PermissionOutcome::Granted)
                        )
                        && result.outcomes.iter().any(|outcome| {
                            outcome.item_id == failure.item_id
                                && matches!(
                                    outcome.status,
                                    ItemExecutionStatus::Failed
                                        | ItemExecutionStatus::PartiallyApplied
                                )
                        });
                    if busy_skip {
                        continue;
                    }
                    if failure
                        .io_failure
                        .as_ref()
                        .is_some_and(|failure| failure.kind == IoFailureKind::AccessDenied)
                    {
                        needs_authorization = true;
                    } else {
                        return Self::Failed;
                    }
                }
            }
        }
        if needs_authorization {
            Self::NeedsAuthorization
        } else if has_issues {
            Self::CompletedWithIssues
        } else {
            Self::Completed
        }
    }
}

/// Inspection, immutable plan and actual effects of one automation run.
#[derive(Debug, Clone, PartialEq)]
pub struct AutomationRunReport {
    /// Fresh scan of the configured automation modules.
    pub scan: ScanReport,
    /// Plan selected by the current recommendation rules.
    pub plan: CleanupPlan,
    /// Actual cleanup effects, including partial results.
    pub execution: ExecutionReport,
}

impl AutomationRunReport {
    /// Combine scan coverage and cleanup results rather than assuming `Ok` means success.
    pub fn status(&self) -> AutomationRunStatus {
        AutomationRunStatus::from_reports(&self.scan, &self.execution)
    }

    /// Number of scan diagnostics, including whole-task errors, for summary-only history.
    pub fn scan_issue_count(&self) -> usize {
        self.scan
            .modules
            .iter()
            .flat_map(|module| &module.tasks)
            .map(|task| task.issues.len() + usize::from(task.error.is_some()))
            .sum()
    }
}

impl Serialize for AutomationRunReport {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut record = serializer.serialize_struct("AutomationRunReport", 4)?;
        record.serialize_field("status", &self.status())?;
        record.serialize_field("scan", &self.scan)?;
        record.serialize_field("plan", &self.plan)?;
        record.serialize_field("execution", &self.execution)?;
        record.end()
    }
}
