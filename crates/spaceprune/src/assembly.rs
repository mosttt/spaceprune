//! 内置与外部 Handler 的统一装配。

use std::collections::BTreeSet;

use anyhow::{Context, Result, anyhow, bail};
use semver::{Version, VersionReq};
use spaceprune_config::{ResolvedConfig, current_platform};
use spaceprune_contracts::plugin::{
    DEVELOPER_TOOLS_PLUGIN_ID, FILESYSTEM_PLUGIN_ID, SYSTEM_LINUX_PLUGIN_ID, is_builtin_plugin_id,
};
use spaceprune_engine::{Engine, EngineBuilder, PluginRegistry};
use spaceprune_plugin_host::PluginHost;

use crate::VERSION;
use spaceprune_access::OperationContext;

pub(crate) fn build_engine(
    config: ResolvedConfig,
    context: &OperationContext,
    disabled: &BTreeSet<String>,
) -> Result<Engine> {
    let journal_directory = execution_journal_directory(&config);
    let mut registry = PluginRegistry::new();
    spaceprune_plugin_filesystem::register(&mut registry)?;
    #[cfg(any(windows, target_os = "linux"))]
    crate::memory_recipe::register(&mut registry)?;
    #[cfg(target_os = "linux")]
    spaceprune_plugin_snapper::register(&mut registry)?;
    #[cfg(target_os = "linux")]
    {
        spaceprune_plugin_system_linux::register_system_handlers(&mut registry)?;
    }
    spaceprune_plugin_developer_tools::register(&mut registry)?;
    #[cfg(windows)]
    spaceprune_plugin_system_windows::register(&mut registry)?;

    context.check()?;
    let host = PluginHost::discover_in_context(context)?;
    validate_plugin_requirements(&config, &host, disabled)?;
    let mut required_external = BTreeSet::new();
    for (_, module) in config.enabled_modules() {
        for task in module.tasks.values().filter(|task| {
            task.enabled
                && (task.platforms.is_empty()
                    || task
                        .platforms
                        .iter()
                        .any(|platform| platform == current_platform()))
        }) {
            if let Some((id, _)) = task.handler.split_once('/')
                && !is_builtin_plugin_id(id)
                && !disabled.contains(id)
            {
                required_external.insert(id.to_owned());
            }
        }
    }
    host.register_required_in_context(&mut registry, &required_external, context)?;
    EngineBuilder::new(config)
        .with_registry(registry)
        .with_disabled_plugins(disabled.clone())
        .with_journal_directory(journal_directory)
        .build_in_context(context)
        .map_err(Into::into)
}

pub(crate) fn build_readonly_engine(
    mut config: ResolvedConfig,
    context: &OperationContext,
    disabled: &BTreeSet<String>,
) -> Result<Engine> {
    let journal_directory = execution_journal_directory(&config);
    for module in config.modules.values_mut() {
        module.tasks.retain(|_, task| {
            task.handler == "builtin.filesystem/paths"
                || task.handler.starts_with("builtin.system-linux/")
                || task.handler.starts_with("builtin.system-windows/")
                || task.handler.starts_with("builtin.developer-tools/")
                || task.handler.starts_with("builtin.memory/")
        });
    }
    config.modules.retain(|_, module| !module.tasks.is_empty());
    config.requires.plugins.retain(|id, _| {
        matches!(
            id.as_str(),
            FILESYSTEM_PLUGIN_ID
                | SYSTEM_LINUX_PLUGIN_ID
                | DEVELOPER_TOOLS_PLUGIN_ID
                | spaceprune_contracts::plugin::MEMORY_PLUGIN_ID
                | spaceprune_contracts::plugin::SYSTEM_WINDOWS_PLUGIN_ID
        )
    });

    let mut registry = PluginRegistry::new();
    spaceprune_plugin_filesystem::register(&mut registry)?;
    #[cfg(any(windows, target_os = "linux"))]
    crate::memory_recipe::register(&mut registry)?;
    #[cfg(target_os = "linux")]
    {
        spaceprune_plugin_system_linux::register_system_handlers(&mut registry)?;
    }
    spaceprune_plugin_developer_tools::register(&mut registry)?;
    #[cfg(windows)]
    spaceprune_plugin_system_windows::register(&mut registry)?;
    EngineBuilder::new(config)
        .with_registry(registry)
        .with_disabled_plugins(disabled.clone())
        .with_journal_directory(journal_directory)
        .build_in_context(context)
        .map_err(Into::into)
}

fn execution_journal_directory(config: &ResolvedConfig) -> std::path::PathBuf {
    config
        .application_source
        .as_deref()
        .and_then(std::path::Path::parent)
        .map(|directory| directory.join(".spaceprune-executions"))
        .or_else(spaceprune_engine::default_execution_journal_directory)
        .unwrap_or_default()
}

fn validate_plugin_requirements(
    config: &ResolvedConfig,
    host: &PluginHost,
    disabled: &BTreeSet<String>,
) -> Result<()> {
    for (id, requirement) in &config.requires.plugins {
        if disabled.contains(id) {
            continue;
        }
        let version = if is_builtin_plugin_id(id) {
            VERSION
        } else {
            host.get(id)
                .map(|plugin| plugin.manifest.version.as_str())
                .ok_or_else(|| anyhow!("Configuration requires missing plugin `{id}`"))?
        };
        let requirement = VersionReq::parse(requirement)
            .with_context(|| format!("Plugin `{id}` has an invalid version requirement"))?;
        let version = Version::parse(version)
            .with_context(|| format!("Plugin `{id}` has invalid version `{version}`"))?;
        if !requirement.matches(&version) {
            bail!("Plugin `{id}` version {version} does not satisfy {requirement}");
        }
    }
    Ok(())
}
