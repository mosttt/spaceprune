mod system_cache;
#[cfg(target_os = "linux")]
pub use system_cache::discovery::discover_system_tasks;
pub use system_cache::register as register_system_handlers;
#[cfg(target_os = "linux")]
pub use system_cache::scan::{
    inspect as inspect_system_cache, prepare as prepare_system_cache_scan,
    validate as validate_system_cache_scan,
};

/// Preserve the former platform-level inventory, including its original Go handler identity.
#[cfg(target_os = "linux")]
pub fn discover_legacy_system_tasks(
    context: &spaceprune_plugin_api::OperationContext,
) -> std::io::Result<Vec<spaceprune_plugin_api::TaskRequest>> {
    let mut tasks = discover_system_tasks(context)?;
    tasks.extend(spaceprune_plugin_developer_tools::discover_legacy_go_task(
        context,
    )?);
    Ok(tasks)
}
