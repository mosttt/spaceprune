//! Public Snapper queries use the same native service as recipe and observation scans.

use spaceprune_access::{Interaction, OperationContext};
use spaceprune_plugin_api::{PluginError, PluginResult, linux_authorization_interaction};
use spaceprune_plugin_snapper::{SnapperConfig, SnapperInventory, SnapperSnapshotGroup};

/// Discover Snapper configurations, requesting native access only after an access denial.
pub fn snapper_configs() -> PluginResult<Vec<SnapperConfig>> {
    match inventory(None, "auto")? {
        SnapperInventory::Configs(items) => Ok(items),
        _ => Err(invalid_response()),
    }
}

/// Inspect snapshots through the host's shared metadata service.
/// `never` uses ordinary access only; noninteractive callers never open a password prompt.
pub fn snapper_snapshots(config: &str, elevation: &str) -> PluginResult<Vec<SnapperSnapshotGroup>> {
    match inventory(Some(config), elevation)? {
        SnapperInventory::Snapshots(items) => Ok(items),
        _ => Err(invalid_response()),
    }
}

fn inventory(config: Option<&str>, elevation: &str) -> PluginResult<SnapperInventory> {
    if !matches!(elevation, "auto" | "never" | "sudo" | "pkexec") {
        return Err(PluginError::new(
            "invalid-elevation",
            "elevation must be auto, never, sudo, or pkexec",
        ));
    }
    let interaction = match (linux_authorization_interaction(), elevation) {
        (Interaction::NonInteractive, _) => Interaction::NonInteractive,
        (_, "sudo") => Interaction::Terminal,
        (_, "pkexec") => Interaction::Foreground,
        (interaction, _) => interaction,
    };
    let context = OperationContext::new(
        Default::default(),
        interaction,
        std::time::Duration::from_secs(120),
    );
    #[cfg(target_os = "linux")]
    if elevation != "never" {
        return super::shared_metadata_service().inspect_snapper(config, &context);
    }
    spaceprune_plugin_snapper::snapper_inventory_unprivileged("snapper", config, &context)
}

fn invalid_response() -> PluginError {
    PluginError::new(
        "invalid-snapper-response",
        "The Snapper inventory returned the wrong metadata kind",
    )
}
