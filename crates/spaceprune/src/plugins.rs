//! Application plugin catalog and device-local enablement.

mod policy;
#[cfg(test)]
mod tests;

use anyhow::{Result, bail};
pub(crate) use policy::PolicySnapshot;
use spaceprune_contracts::plugin::{
    DEVELOPER_TOOLS_PLUGIN_ID, FILESYSTEM_PLUGIN_ID, SNAPPER_PLUGIN_ID, SYSTEM_LINUX_PLUGIN_ID,
    is_builtin_plugin_id,
};
pub use spaceprune_contracts::plugin::{PluginCatalogEntry, PluginKind};
use spaceprune_plugin_host::{InstalledPlugin, installed_plugin_metadata};

/// List built-in and installed capabilities without starting external plugin processes.
pub fn list_plugin_catalog() -> Result<Vec<PluginCatalogEntry>> {
    Ok(catalog(
        installed_plugin_metadata()?.iter(),
        &PolicySnapshot::discover()?,
    ))
}

/// Change a known plugin's device-local preference for subsequent operations.
/// Existing loaded sessions must be reloaded and scanned again after a change.
pub fn set_plugin_enabled(id: &str, enabled: bool) -> Result<PluginCatalogEntry> {
    set_enabled(
        installed_plugin_metadata()?.iter(),
        &PolicySnapshot::discover()?,
        id,
        enabled,
    )
}

fn set_enabled<'a>(
    plugins: impl IntoIterator<Item = &'a InstalledPlugin>,
    policy: &PolicySnapshot,
    id: &str,
    enabled: bool,
) -> Result<PluginCatalogEntry> {
    let Some(mut entry) = catalog(plugins, policy)
        .into_iter()
        .find(|entry| entry.id == id && !entry.conflicts_with_builtin)
    else {
        bail!(
            "{}",
            spaceprune_contracts::messages::message(
                "plugins.not_installed",
                &[("id", (id).to_string())]
            )
        );
    };
    if !entry.supported {
        bail!(
            "{}",
            spaceprune_contracts::messages::message(
                "plugins.unsupported_platform",
                &[("id", (id).to_string())]
            )
        );
    }
    let next = policy.set_enabled(id, enabled)?;
    entry.enabled = !next.disabled().contains(id);
    Ok(entry)
}

fn catalog<'a>(
    plugins: impl IntoIterator<Item = &'a InstalledPlugin>,
    policy: &PolicySnapshot,
) -> Vec<PluginCatalogEntry> {
    let mut entries = vec![
        builtin(
            spaceprune_contracts::plugin::SYSTEM_WINDOWS_PLUGIN_ID,
            spaceprune_contracts::messages::text("plugins.windows_name"),
            spaceprune_contracts::messages::text("plugins.windows_description"),
            &["windows"],
            &["recycle-bin", "component-store"],
        ),
        builtin(
            spaceprune_contracts::plugin::MEMORY_PLUGIN_ID,
            spaceprune_contracts::messages::text("plugins.memory_name"),
            spaceprune_contracts::messages::text("plugins.memory_description"),
            &["windows", "linux"],
            &["process-working-set"],
        ),
        builtin(
            FILESYSTEM_PLUGIN_ID,
            spaceprune_contracts::messages::text("plugins.filesystem_name"),
            spaceprune_contracts::messages::text("plugins.filesystem_description"),
            &[],
            &["paths"],
        ),
        builtin(
            SNAPPER_PLUGIN_ID,
            spaceprune_contracts::messages::text("plugins.snapshots_name"),
            spaceprune_contracts::messages::text("plugins.snapshots_description"),
            &["linux"],
            &["snapshots"],
        ),
        builtin(
            DEVELOPER_TOOLS_PLUGIN_ID,
            spaceprune_contracts::messages::text("plugins.developer_name"),
            spaceprune_contracts::messages::text("plugins.developer_description"),
            &["linux", "windows", "macos"],
            &[
                "go-modcache",
                "go-buildcache",
                "cargo-registry-cache",
                "npm-cache",
                "docker-build-cache",
                "docker-dangling-images",
                "docker-unused-images",
                "docker-stopped-containers",
            ],
        ),
        builtin(
            SYSTEM_LINUX_PLUGIN_ID,
            spaceprune_contracts::messages::text("plugins.system_name"),
            spaceprune_contracts::messages::text("plugins.system_description"),
            &["linux"],
            &[
                "package-cache",
                "journal",
                "tmpfiles",
                "coredumps",
                "go-modcache",
            ],
        ),
    ];
    #[cfg(not(target_os = "linux"))]
    {
        if let Some(developer) = entries
            .iter_mut()
            .find(|entry| entry.id == DEVELOPER_TOOLS_PLUGIN_ID)
        {
            developer
                .handlers
                .retain(|handler| handler.contains("/docker-"));
        }
    }
    for plugin in plugins {
        entries.push(PluginCatalogEntry {
            id: plugin.manifest.id.clone(),
            name: plugin.manifest.name.clone(),
            description: spaceprune_contracts::messages::text("plugins.external_description")
                .to_owned(),
            version: plugin.manifest.version.clone(),
            kind: PluginKind::External,
            conflicts_with_builtin: is_builtin_plugin_id(&plugin.manifest.id),
            platforms: plugin.manifest.platforms.clone(),
            supported: plugin.manifest.platforms.is_empty()
                || plugin
                    .manifest
                    .platforms
                    .iter()
                    .any(|platform| platform == spaceprune_config::current_platform()),
            enabled: true,
            handlers: plugin
                .manifest
                .handlers
                .iter()
                .map(|handler| format!("{}/{handler}", plugin.manifest.id))
                .collect(),
            installation: Some(plugin.clone()),
        });
    }
    for entry in &mut entries {
        entry.enabled = !entry.conflicts_with_builtin && !policy.disabled().contains(&entry.id);
    }
    entries
}

fn builtin(
    id: &str,
    name: &str,
    description: &str,
    platforms: &[&str],
    handlers: &[&str],
) -> PluginCatalogEntry {
    PluginCatalogEntry {
        id: id.to_owned(),
        name: name.to_owned(),
        description: description.to_owned(),
        version: crate::VERSION.to_owned(),
        kind: PluginKind::Builtin,
        conflicts_with_builtin: false,
        platforms: platforms
            .iter()
            .map(|platform| (*platform).to_owned())
            .collect(),
        supported: platforms.is_empty()
            || platforms.contains(&spaceprune_config::current_platform()),
        enabled: true,
        handlers: handlers
            .iter()
            .map(|handler| format!("{id}/{handler}"))
            .collect(),
        installation: None,
    }
}
