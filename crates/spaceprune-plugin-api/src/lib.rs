#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! SpacePrune 插件与执行器共享的稳定数据模型。
//!
//! 静态 Rust 插件和外部进程插件都以本 crate 中的类型表达任务、清理项、进度和执行结果。

use std::sync::atomic::{AtomicBool, Ordering};

mod scan_measurement;
pub use scan_measurement::ScanMeasurement;

pub use spaceprune_contracts::messages;

pub use spaceprune_contracts::cache::{
    DirectoryBinding, SystemCacheScanRequest, SystemCacheSource,
};

pub use spaceprune_contracts::handler::{
    CleanupItem, CleanupSizeAccounting, HandlerCleanResult, HandlerDescriptor, HandlerIssue,
    HandlerScanResult, IssueLevel, ItemExecution, ItemExecutionStatus, ItemFailure,
    PROTOCOL_VERSION, PluginError, PluginResult, ProgressEvent, ProgressPhase, Recommendation,
    SizeApplicability, TaskRequest, VolumeSpaceChange,
};
pub use spaceprune_contracts::logical_size::LogicalSizeSummary;
pub use spaceprune_contracts::report::ExecutionJournalReference;

static GRAPHICAL_LINUX_AUTHORIZATION: AtomicBool = AtomicBool::new(false);

/// Select graphical Linux authorization for a desktop host, even when it inherits a terminal.
pub fn prefer_graphical_linux_authorization() {
    GRAPHICAL_LINUX_AUTHORIZATION.store(true, Ordering::Relaxed);
}

/// Native interaction context selected by the host; background operations never open prompts.
pub fn linux_authorization_interaction() -> spaceprune_access::Interaction {
    use std::io::IsTerminal;
    if GRAPHICAL_LINUX_AUTHORIZATION.load(Ordering::Relaxed) {
        spaceprune_access::Interaction::Foreground
    } else if std::io::stdin().is_terminal() {
        spaceprune_access::Interaction::Terminal
    } else {
        spaceprune_access::Interaction::NonInteractive
    }
}

/// Return the Linux system authorization program selected by the host and terminal availability.
pub fn automatic_linux_authorization_method() -> &'static str {
    use std::io::IsTerminal;
    if GRAPHICAL_LINUX_AUTHORIZATION.load(Ordering::Relaxed) || !std::io::stdin().is_terminal() {
        "pkexec"
    } else {
        "sudo"
    }
}

/// One original task and its exact selected items in an opt-in cleanup batch.
pub struct CleanupBatchGroup<'a> {
    /// The task whose options and safety bindings apply to these items.
    pub task: &'a TaskRequest,
    /// Only the items explicitly selected for this task.
    pub items: &'a [CleanupItem],
}

/// A host-owned native session bound to the immutable confirmed cleanup plan.
/// Read-only authorization cannot satisfy this interface.
pub trait CleanupAccess: Send + Sync {
    /// Execute only the supplied subset of confirmed items after native authorization.
    /// Implementations retain refusals and never retry an uncertain deletion automatically.
    fn clean_authorized(
        &self,
        method: &str,
        groups: &[CleanupBatchGroup<'_>],
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<Vec<PluginResult<HandlerCleanResult>>>;

    /// After dispatched work returns, confirm native workers have stopped before committing the report.
    /// Implementations without persistent native work need no additional finalization.
    fn finish(&self) -> PluginResult<()> {
        Ok(())
    }
}

/// 接收进度事件的对象。
pub trait ProgressSink: Send + Sync {
    /// Replace the cumulative measurement snapshot for one scan task.
    fn record_scan(&self, _task: &TaskRequest, _measurement: ScanMeasurement) {}

    /// Bind a host's activity to its durable journal before any destructive dispatch.
    /// Existing sinks need no persistence; a participating host can reject an unsaved binding.
    fn execution_journal_started(&self, _journal: &ExecutionJournalReference) -> PluginResult<()> {
        Ok(())
    }

    /// 接收一个进度事件。
    fn emit(&self, event: ProgressEvent);

    /// Persist the latest cumulative outcome for one confirmed item before starting the next.
    /// A failed receipt stops subsequent destructive work; completed effects are not rolled back.
    fn cleanup_item_finished(
        &self,
        _task: &TaskRequest,
        _outcome: &ItemExecution,
    ) -> PluginResult<()> {
        Ok(())
    }

    /// Report cumulative successful-item size estimates for one cleanup task.
    fn record_cleanup(
        &self,
        _task: &TaskRequest,
        _succeeded: usize,
        _reclaimed_bytes: u64,
        _unknown_size_item_count: usize,
    ) {
    }

    /// Report cumulative cleanup totals with the registered handler's accounting unit.
    /// Existing sinks continue receiving the original counters unless they opt into this projection.
    fn record_cleanup_with_accounting(
        &self,
        task: &TaskRequest,
        _accounting: CleanupSizeAccounting,
        succeeded: usize,
        reclaimed_bytes: u64,
        unknown_size_item_count: usize,
    ) {
        self.record_cleanup(task, succeeded, reclaimed_bytes, unknown_size_item_count);
    }

    /// Suspend live output while a system command reads interactive input.
    ///
    /// Each `true` call must be paired with `false`, including error paths.
    /// Graphical and noninteractive sinks may leave this as a no-op.
    fn set_input_active(&self, _active: bool) {}
}

/// A host-supplied read-only retry policy for scan results that need additional access.
///
/// Implementations must preserve the task's scope and return fresh scan results, not
/// cached authorization flags. The engine validates returned item identities as usual.
pub trait ScanAccess: Send + Sync {
    /// Apply the same access policy to top-level native failures and partial scan issues.
    fn retry_result(
        &self,
        task: &TaskRequest,
        result: PluginResult<HandlerScanResult>,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult> {
        match result {
            Ok(result) => self.retry(task, result, progress, context),
            Err(error)
                if error.io_failure.as_ref().is_some_and(|failure| {
                    failure.kind == spaceprune_access::IoFailureKind::AccessDenied
                }) =>
            {
                self.retry(
                    task,
                    HandlerScanResult {
                        items: Vec::new(),
                        issues: vec![HandlerIssue {
                            code: Some(error.code),
                            level: IssueLevel::Error,
                            message: error.message,
                            location: None,
                            io_failure: error.io_failure,
                        }],
                    },
                    progress,
                    context,
                )
            }
            Err(error) => Err(error),
        }
    }

    /// Whether the host owns authorization for this task's initial scan and retries.
    fn manages_authorization(&self, _task: &TaskRequest) -> bool {
        false
    }

    /// Optionally retry a completed read-only task and return its final result.
    fn retry(
        &self,
        task: &TaskRequest,
        result: HandlerScanResult,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult>;
}

/// Keep terminal progress from overwriting a system password prompt.
pub struct ProgressInputGuard<'a> {
    progress: &'a dyn ProgressSink,
}

impl<'a> ProgressInputGuard<'a> {
    /// Suspend output until this guard is dropped.
    pub fn new(progress: &'a dyn ProgressSink) -> Self {
        progress.set_input_active(true);
        Self { progress }
    }
}

impl Drop for ProgressInputGuard<'_> {
    fn drop(&mut self) {
        self.progress.set_input_active(false);
    }
}

/// 丢弃所有进度事件的接收器。
#[derive(Debug, Default)]
pub struct NullProgress;

impl ProgressSink for NullProgress {
    fn emit(&self, _event: ProgressEvent) {}
}

pub use spaceprune_access::{CancellationToken, OperationContext, OperationStopReason};

mod cleanup_dispatch;
pub use cleanup_dispatch::dispatch_authorized_cleanup;

/// 一个可注册到 SpacePrune 执行器的清理能力。
pub trait Handler: Send + Sync {
    /// 返回 Handler 的静态描述。
    fn descriptor(&self) -> HandlerDescriptor;

    /// 校验任务的 `options` 和 `recommend`。
    fn validate(&self, task: &TaskRequest) -> PluginResult<()>;

    /// Validate under the caller's budget; process-backed validators must override this method.
    fn validate_in_context(
        &self,
        task: &TaskRequest,
        context: &OperationContext,
    ) -> PluginResult<()> {
        context
            .check()
            .map_err(|error| PluginError::new("operation-stopped", error.to_string()))?;
        self.validate(task)
    }

    /// 扫描任务并返回清理项。
    fn scan(
        &self,
        task: &TaskRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult>;

    /// Scan without requesting elevation when the host manages authorization.
    fn scan_unprivileged(
        &self,
        task: &TaskRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult> {
        self.scan(task, progress, context)
    }

    /// 清理本 Handler 扫描产生的选中项目。
    fn clean(
        &self,
        task: &TaskRequest,
        items: &[CleanupItem],
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerCleanResult>;

    /// Native elevation method required by this system handler; ordinary handlers return none.
    fn cleanup_authorization_method(&self, _task: &TaskRequest) -> PluginResult<Option<String>> {
        Ok(None)
    }

    /// Route system cleanup through the host's shared operation authorization session.
    fn clean_batch_with_access(
        &self,
        groups: &[CleanupBatchGroup<'_>],
        progress: &dyn ProgressSink,
        context: &OperationContext,
        access: Option<&dyn CleanupAccess>,
    ) -> Vec<PluginResult<HandlerCleanResult>> {
        dispatch_authorized_cleanup(self, groups, progress, context, access)
    }

    /// Whether selected groups for this handler may share a cleanup invocation.
    fn supports_batch_cleanup(&self) -> bool {
        false
    }

    /// Clean exact selections while preserving one result per input group, in order.
    ///
    /// The engine calls this only for handlers that explicitly opt in. External
    /// process plugins and other handlers retain their existing single-task calls.
    fn clean_batch(
        &self,
        groups: &[CleanupBatchGroup<'_>],
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> Vec<PluginResult<HandlerCleanResult>> {
        groups
            .iter()
            .map(|group| {
                if context.is_stopped() {
                    Ok(HandlerCleanResult::default())
                } else {
                    self.clean(group.task, group.items, progress, context)
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod progress_input_tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct InputState(Mutex<Vec<bool>>);

    impl ProgressSink for InputState {
        fn emit(&self, _event: ProgressEvent) {}

        fn set_input_active(&self, active: bool) {
            self.0.lock().unwrap().push(active);
        }
    }

    #[test]
    fn interactive_output_resumes_after_nested_prompts_and_authorization_errors() {
        fn authorize(progress: &dyn ProgressSink) -> Result<(), &'static str> {
            let _outer = ProgressInputGuard::new(progress);
            let _inner = ProgressInputGuard::new(progress);
            Err("authorization refused")
        }
        let progress = InputState::default();
        assert!(authorize(&progress).is_err());
        assert_eq!(*progress.0.lock().unwrap(), [true, true, false, false]);
    }
}
