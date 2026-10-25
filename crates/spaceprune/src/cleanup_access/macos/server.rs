//! The native executor owns the plan and rejects a new item until the previous receipt arrives.

use super::*;
use crate::cleanup_access::worker::plan::PreparedPlan;

#[derive(Default)]
pub(super) struct State {
    plan: Option<PreparedPlan>,
    pending: Option<ItemKey>,
    rejected: bool,
}
impl State {
    pub(super) fn request(
        &mut self,
        request: Request,
        operation: &OperationContext,
        events: &(dyn Fn(Reply) -> PluginResult<()> + Send + Sync),
    ) -> PluginResult<NativeReply> {
        operation
            .check()
            .map_err(|cause| error(cause.to_string()))?;
        if self.rejected {
            return Err(error("Cleanup receipt was rejected"));
        }
        match request {
            Request::Prepare { grant } => {
                if self.plan.is_some()
                    || grant
                        .groups
                        .iter()
                        .any(|group| group.handler_id != "builtin.filesystem/paths")
                {
                    return Err(error("Invalid or repeated macOS cleanup plan"));
                }
                self.plan = Some(
                    PreparedPlan::new(grant, Some(operation))
                        .map_err(|cause| error(cause.to_string()))?,
                );
                Ok(NativeReply::Ready)
            }
            Request::Execute { request } => {
                if self.pending.is_some()
                    || request.selection.len() != 1
                    || request.selection[0].items.len() != 1
                {
                    return Err(error(
                        "Cleanup requires one confirmed item and the previous receipt",
                    ));
                }
                let key = ItemKey {
                    group: request.selection[0].group,
                    item: request.selection[0].items[0],
                };
                let plan = self
                    .plan
                    .as_mut()
                    .ok_or_else(|| error("Cleanup plan is not prepared"))?;
                let batch = plan
                    .prepare(request, Some(operation))
                    .map_err(|cause| error(cause.to_string()))?;
                // The selection is consumed even if work fails or the connection is lost.
                self.pending = Some(key);
                let sink = Sink {
                    events,
                    operation: &batch.context,
                };
                let result = plan
                    .execute(&batch, &sink)
                    .pop()
                    .ok_or_else(|| error("Missing cleanup result"))??;
                result
                    .validate_selected(&batch.groups[0].items, batch.context.is_stopped())
                    .map_err(error)?;
                Ok(NativeReply::Item { result })
            }
            Request::Acknowledge { item, accepted } => {
                let plan = self
                    .plan
                    .as_ref()
                    .ok_or_else(|| error("Cleanup plan is not prepared"))?;
                if plan.context().identity() != operation.identity()
                    || self.pending.as_ref() != Some(&item)
                {
                    return Err(error("Cleanup receipt does not match the pending item"));
                }
                plan.context()
                    .check()
                    .map_err(|cause| error(cause.to_string()))?;
                if !accepted {
                    self.rejected = true;
                    return Err(error("Host could not persist the cleanup outcome"));
                }
                self.pending = None;
                Ok(NativeReply::Acknowledged)
            }
        }
    }
}
struct Sink<'a> {
    events: &'a (dyn Fn(Reply) -> PluginResult<()> + Send + Sync),
    operation: &'a OperationContext,
}
impl Sink<'_> {
    fn send(&self, reply: Reply) -> PluginResult<()> {
        let result = (self.events)(reply);
        if result.is_err() {
            self.operation.cancel();
        }
        result
    }
}
impl ProgressSink for Sink<'_> {
    fn emit(&self, event: ProgressEvent) {
        let _ = self.send(Reply::Progress { event });
    }
    fn cleanup_item_finished(
        &self,
        task: &TaskRequest,
        outcome: &spaceprune_plugin_api::ItemExecution,
    ) -> PluginResult<()> {
        self.send(Reply::ItemFinished {
            task: task.clone(),
            outcome: outcome.clone(),
        })
    }
    fn record_cleanup(&self, task: &TaskRequest, succeeded: usize, bytes: u64, unknown: usize) {
        let _ = self.send(Reply::Count {
            task: task.clone(),
            succeeded,
            bytes,
            unknown,
        });
    }
}
