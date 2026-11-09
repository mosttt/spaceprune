//! Recipe-independent discovery and read-only inspection of built-in system resources.

use anyhow::Result;
use spaceprune_engine::{EngineError, PluginRegistry};
use spaceprune_plugin_api::{
    HandlerScanResult, OperationContext, ProgressSink, ScanAccess, TaskRequest,
};

use crate::plugins::PolicySnapshot;

use spaceprune_contracts::plugin::DEVELOPER_TOOLS_PLUGIN_ID;
#[cfg(target_os = "linux")]
use spaceprune_contracts::plugin::SYSTEM_LINUX_PLUGIN_ID;

/// Discover available built-in lifecycle tools without loading any Recipe.
///
/// Returned tasks describe observed local tools and always disable recommendation.
/// Unsupported platforms and a disabled system plugin produce an empty inventory.
pub fn discover_system_capabilities(context: &OperationContext) -> Result<Vec<TaskRequest>> {
    Ok(probe_system_capabilities(context)?
        .into_iter()
        .filter(|probe| probe.issue.is_none())
        .map(|probe| probe.task)
        .collect())
}

/// Probe installed native tools without enumerating cleanup objects.
/// Missing tools are omitted; installed but unavailable interfaces retain their diagnostic.
/// An entry with an issue is evidence only and must not be issued as an executable capability.
pub fn probe_system_capabilities(
    context: &OperationContext,
) -> Result<Vec<spaceprune_plugin_developer_tools::CapabilityProbe>> {
    context.check()?;
    let policy = PolicySnapshot::discover()?;
    let mut tasks = Vec::new();
    #[cfg(target_os = "linux")]
    {
        if !policy.disabled().contains(SYSTEM_LINUX_PLUGIN_ID) {
            tasks.extend(
                spaceprune_plugin_system_linux::discover_system_tasks(context)?
                    .into_iter()
                    .map(|task| spaceprune_plugin_developer_tools::CapabilityProbe {
                        task,
                        issue: None,
                    }),
            );
        }
    }
    #[cfg(windows)]
    if !policy
        .disabled()
        .contains(spaceprune_contracts::plugin::SYSTEM_WINDOWS_PLUGIN_ID)
    {
        tasks.extend(
            spaceprune_plugin_system_windows::discover(context)?
                .into_iter()
                .map(|task| spaceprune_plugin_developer_tools::CapabilityProbe {
                    task,
                    issue: None,
                }),
        );
    }
    if !policy.disabled().contains(DEVELOPER_TOOLS_PLUGIN_ID) {
        tasks.extend(spaceprune_plugin_developer_tools::probe_tasks(context)?);
    }
    #[cfg(any(windows, target_os = "linux"))]
    if !policy
        .disabled()
        .contains(spaceprune_contracts::plugin::MEMORY_PLUGIN_ID)
    {
        tasks.extend(
            spaceprune_plugin_memory::discover_tasks(context)?
                .into_iter()
                .map(|task| spaceprune_plugin_developer_tools::CapabilityProbe {
                    task,
                    issue: None,
                }),
        );
    }
    policy.check_current()?;
    Ok(tasks)
}

/// Inspect a built-in system lifecycle task using the host's existing read-only access.
///
/// The caller's task is not modified. Inspection disables recommendation regardless
/// of its saved rule, so discovering a working tool is not a cleanup recommendation.
/// No cleanup handler is invoked and no default Recipe is loaded.
pub fn inspect_system_capability(
    task: &TaskRequest,
    progress: &dyn ProgressSink,
    context: &OperationContext,
    access: &dyn ScanAccess,
) -> Result<HandlerScanResult> {
    context.check()?;
    let policy = PolicySnapshot::discover()?;
    let plugin = task
        .handler_id
        .split_once('/')
        .map(|(id, _)| id)
        .unwrap_or("");
    if policy.disabled().contains(plugin) {
        return Err(EngineError::PluginDisabled(plugin.into()).into());
    }
    let mut registry = PluginRegistry::new();
    #[cfg(any(windows, target_os = "linux"))]
    crate::memory_recipe::register(&mut registry)?;
    #[cfg(target_os = "linux")]
    {
        spaceprune_plugin_system_linux::register_system_handlers(&mut registry)?;
    }
    spaceprune_plugin_developer_tools::register(&mut registry)?;
    #[cfg(windows)]
    spaceprune_plugin_system_windows::register(&mut registry)?;
    let handler = registry
        .get(&task.handler_id)
        .ok_or_else(|| EngineError::UnknownTask {
            module: task.module_id.clone(),
            task: task.task_id.clone(),
        })?;
    let mut inspection = task.clone();
    inspection.recommend = serde_json::json!({"enabled": false});
    handler.validate(&inspection)?;
    let observed = access.retry_result(
        &inspection,
        handler.scan(&inspection, progress, context),
        progress,
        context,
    )?;
    context.check()?;
    policy.check_current()?;
    Ok(observed)
}
