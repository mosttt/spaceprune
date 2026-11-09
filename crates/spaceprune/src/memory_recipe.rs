//! Memory Recipe execution reuses the manual native authorization boundary.
use spaceprune_plugin_api::{OperationContext, PluginError, PluginResult, ProgressSink};
use spaceprune_plugin_memory::{BoundProcess, MemoryRelease};

pub(crate) fn register(
    registry: &mut spaceprune_engine::PluginRegistry,
) -> Result<(), spaceprune_engine::EngineError> {
    spaceprune_plugin_memory::register(registry, release)
}
fn release(
    selected: &[BoundProcess],
    progress: &dyn ProgressSink,
    context: &OperationContext,
) -> PluginResult<Vec<MemoryRelease>> {
    #[cfg(target_os = "linux")]
    {
        if !spaceprune_plugin_memory::linux::can_reclaim()
            && context.interaction == spaceprune_access::Interaction::NonInteractive
        {
            let mut error = PluginError::new(
                "memory-authorization-required",
                spaceprune_contracts::messages::text("memory.authorization_required"),
            );
            error.authorization =
                Some(spaceprune_contracts::access::PermissionOutcome::NeedsAuthorization);
            return Err(error);
        }
        let _input = (!spaceprune_plugin_memory::linux::can_reclaim()
            && context.interaction != spaceprune_access::Interaction::NonInteractive)
            .then(|| spaceprune_plugin_api::ProgressInputGuard::new(progress));
        crate::memory_access::release_bound(selected, context)
            .map_err(|error| PluginError::new("memory-release-failed", error.to_string()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = progress;
        spaceprune_plugin_memory::release_bound(selected, &|| context.check())
            .map_err(|error| PluginError::new("memory-release-failed", error.to_string()))
    }
}
