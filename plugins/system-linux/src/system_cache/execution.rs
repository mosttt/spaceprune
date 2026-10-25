//! Per-item cleanup outcomes and permission recovery before a domain command starts.

use super::*;

impl SystemHandler {
    pub(super) fn clean_with_access(
        &self,
        task: &TaskRequest,
        items: &[CleanupItem],
        progress: &dyn ProgressSink,
        context: &OperationContext,
        _access: Option<&dyn spaceprune_plugin_api::CleanupAccess>,
    ) -> PluginResult<HandlerCleanResult> {
        let (options, _) = parse(task)?;
        let mut result = HandlerCleanResult {
            unknown_reclaimed_items: Some(0),
            ..HandlerCleanResult::default()
        };
        for item in items {
            use spaceprune_plugin_api::{ItemExecution, ItemExecutionStatus};
            let mut outcome = ItemExecution {
                item_id: item.id.clone(),
                status: ItemExecutionStatus::NotAttempted,
                reclaimed_estimate: Some(0),
                reclaimed_lower_bound: 0,
                failure: None,
            };
            if context.is_stopped() {
                result.record_outcome(outcome);
                continue;
            }
            let execution = executable_action(self.kind, &options, &item.id, context)
                .map_err(ActionFailure::BeforeStart)
                .and_then(|action| {
                    run_action(&action, task, progress, context)
                        .map(|()| Some(item.reclaimable_bytes))
                });
            match execution {
                Ok(executed) => {
                    outcome.status = if executed.is_some() {
                        ItemExecutionStatus::Succeeded
                    } else {
                        ItemExecutionStatus::AlreadyAbsent
                    };
                    outcome.reclaimed_estimate = executed.unwrap_or(Some(0));
                }
                Err(error) => {
                    let mut io_failure = None;
                    let message = match error {
                        ActionFailure::Native {
                            started,
                            reclaimed,
                            error,
                        } => {
                            outcome.reclaimed_lower_bound = reclaimed;
                            io_failure = Some(spaceprune_access::error::classify(&error));
                            outcome.status = if spaceprune_access::error::has_effects(&error) {
                                ItemExecutionStatus::PartiallyApplied
                            } else if started {
                                ItemExecutionStatus::Uncertain
                            } else {
                                ItemExecutionStatus::Failed
                            };
                            if started {
                                outcome.reclaimed_estimate = None;
                            }
                            error.to_string()
                        }
                        ActionFailure::BeforeStart(message) => {
                            outcome.status = ItemExecutionStatus::Failed;
                            message
                        }
                        ActionFailure::Unconfirmed(message) => {
                            outcome.status = ItemExecutionStatus::Uncertain;
                            outcome.reclaimed_estimate = None;
                            message
                        }
                    };
                    outcome.failure = Some(ItemFailure {
                        authorization: None,
                        io_failure,
                        item_id: item.id.clone(),
                        message,
                    });
                }
            }
            result.record_outcome(outcome.clone());
            if progress.cleanup_item_finished(task, &outcome).is_err() {
                context.cancel();
            }
            progress.record_cleanup(
                task,
                result.succeeded,
                result.reclaimed_bytes,
                result.unknown_reclaimed_items(),
            );
        }
        Ok(result)
    }
}
