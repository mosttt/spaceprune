//! Confirmed-plan validation and consumption are shared by every native transport.

use super::*;
use std::collections::BTreeSet;

pub(in crate::cleanup_access) struct PreparedPlan {
    grant: Grant,
    context: OperationContext,
    registry: spaceprune_engine::PluginRegistry,
    used: BTreeSet<(usize, usize)>,
}
pub(in crate::cleanup_access) struct PreparedBatch {
    pub(in crate::cleanup_access) groups: Vec<PlanGroup>,
    pub(in crate::cleanup_access) context: OperationContext,
}
impl PreparedPlan {
    pub(in crate::cleanup_access) fn new(
        mut grant: Grant,
        binding: Option<&OperationContext>,
    ) -> io::Result<Self> {
        let context = request_context(&grant, binding)?;
        let mut registry = spaceprune_engine::PluginRegistry::new();
        spaceprune_plugin_filesystem::register(&mut registry).map_err(io::Error::other)?;
        spaceprune_plugin_snapper::register(&mut registry).map_err(io::Error::other)?;
        spaceprune_platform::register_handlers(&mut registry).map_err(io::Error::other)?;
        #[cfg(target_os = "linux")]
        spaceprune_plugin_developer_tools::register(&mut registry).map_err(io::Error::other)?;
        for group in &mut grant.groups {
            prepare_system_programs(&mut group.request)?;
            registry
                .get(&group.handler_id)
                .ok_or_else(|| io::Error::other("Unknown cleanup handler"))?
                .validate_in_context(&group.request, &context)
                .map_err(io::Error::other)?;
        }
        Ok(Self {
            grant,
            context,
            registry,
            used: BTreeSet::new(),
        })
    }
    pub(in crate::cleanup_access) fn context(&self) -> &OperationContext {
        &self.context
    }

    pub(in crate::cleanup_access) fn prepare(
        &mut self,
        request: ExecuteRequest,
        binding: Option<&OperationContext>,
    ) -> io::Result<PreparedBatch> {
        self.context.check()?;
        if request.operation.identity != self.context.identity() {
            return Err(io::Error::other("Cleanup operation identity changed"));
        }
        let mut context = OperationContext::from_transfer(
            &request.operation,
            self.context
                .deadline
                .saturating_duration_since(Instant::now()),
            self.context.budget,
        )?;
        bind(&mut context, Some(&self.context))?;
        bind(&mut context, binding)?;
        let mut next = BTreeSet::new();
        let mut groups: Vec<PlanGroup> = Vec::new();
        for selection in &request.selection {
            let group = self
                .grant
                .groups
                .get(selection.group)
                .ok_or_else(|| io::Error::other("Unknown confirmed group"))?;
            if groups
                .first()
                .is_some_and(|first| first.handler_id != group.handler_id)
            {
                return Err(io::Error::other("Mixed cleanup handlers"));
            }
            let mut selected = group.clone();
            selected.items.clear();
            for &index in &selection.items {
                if self.used.contains(&(selection.group, index))
                    || !next.insert((selection.group, index))
                {
                    return Err(io::Error::other("Cleanup selection was already consumed"));
                }
                selected.items.push(
                    group
                        .items
                        .get(index)
                        .ok_or_else(|| io::Error::other("Unknown confirmed item"))?
                        .clone(),
                );
            }
            selected.request.options["elevation"] = serde_json::json!("never");
            groups.push(selected);
        }
        if groups.is_empty() {
            return Err(io::Error::other("Empty cleanup request"));
        }
        context.check()?;
        self.used.extend(next);
        Ok(PreparedBatch { groups, context })
    }
    pub(in crate::cleanup_access) fn execute(
        &self,
        batch: &PreparedBatch,
        sink: &dyn ProgressSink,
    ) -> Vec<PluginResult<HandlerCleanResult>> {
        let handler = self
            .registry
            .get(&batch.groups[0].handler_id)
            .expect("validated cleanup handler");
        let groups: Vec<_> = batch
            .groups
            .iter()
            .map(|group| CleanupBatchGroup {
                task: &group.request,
                items: &group.items,
            })
            .collect();
        handler.clean_batch(&groups, sink, &batch.context)
    }
}
fn bind(context: &mut OperationContext, binding: Option<&OperationContext>) -> io::Result<()> {
    if let Some(binding) = binding {
        if binding.identity() != context.identity() {
            return Err(io::Error::other("Cleanup bootstrap identity changed"));
        }
        binding.check()?;
        context.deadline = context.deadline.min(binding.deadline);
        context.budget = context.budget.restricted(binding.budget);
        context.cancellation = binding.cancellation.child();
    }
    context.check()
}

// Stdio adapters attach EOF cancellation before filesystem/handler validation begins.
pub(super) fn request_context(
    grant: &Grant,
    binding: Option<&OperationContext>,
) -> io::Result<OperationContext> {
    if grant.protocol_version != CLEANUP_PROTOCOL_VERSION
        || grant.plan_digest.len() != 64
        || grant.groups.is_empty()
        || grant.groups.iter().any(|group| {
            !supported(&group.handler_id) || group.handler_id != group.request.handler_id
        })
    {
        return Err(io::Error::other("Invalid confirmed cleanup plan"));
    }
    let mut context = OperationContext::from_transfer(
        &grant.operation,
        Duration::from_secs(3600),
        spaceprune_access::ResourceBudget::workflow(),
    )?;
    bind(&mut context, binding)?;
    Ok(context)
}
