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
    let mut entries = [
        (
            spaceprune_contracts::plugin::SYSTEM_WINDOWS_PLUGIN_ID,
            "windows",
        ),
        (spaceprune_contracts::plugin::MEMORY_PLUGIN_ID, "memory"),
        (FILESYSTEM_PLUGIN_ID, "filesystem"),
        (SNAPPER_PLUGIN_ID, "snapshots"),
        (DEVELOPER_TOOLS_PLUGIN_ID, "developer"),
        (SYSTEM_LINUX_PLUGIN_ID, "system"),
    ]
    .into_iter()
    .map(|(id, message)| {
        builtin(
            id,
            spaceprune_contracts::messages::text(&format!("plugins.{message}_name")),
            spaceprune_contracts::messages::text(&format!("plugins.{message}_description")),
        )
    })
    .collect::<Vec<_>>();
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

fn builtin(id: &str, name: &str, description: &str) -> PluginCatalogEntry {
    let spec =
        spaceprune_contracts::plugin::builtin_plugin_spec(id).expect("bundled catalog identity");
    let platform = spaceprune_config::current_platform();
    PluginCatalogEntry {
        id: id.to_owned(),
        name: name.to_owned(),
        description: description.to_owned(),
        version: spec.version.to_owned(),
        kind: PluginKind::Builtin,
        conflicts_with_builtin: false,
        platforms: spec
            .platforms
            .iter()
            .map(|platform| (*platform).to_owned())
            .collect(),
        supported: spec.supports(platform),
        enabled: true,
        handlers: spec.handler_ids(platform),
        installation: None,
    }
}
