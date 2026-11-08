//! Snapper task option and recommendation validation.

use super::*;

pub(super) fn validate(task: &TaskRequest) -> PluginResult<()> {
    let (options, recommend) = parse(task)?;
    if options.snapper.is_empty()
        || options.find.is_empty()
        || options.du.is_empty()
        || options.max_depth == 0
    {
        return Err(PluginError::new(
            "invalid-options",
            spaceprune_plugin_api::messages::text("snapper.invalid_commands"),
        ));
    }
    validate_elevation(&options.elevation)?;
    if options
        .snapshot_types
        .iter()
        .any(|value| !matches!(value.as_str(), "single" | "pre" | "post" | "pre/post"))
        || options
            .cleanup_algorithms
            .iter()
            .any(|value| value.len() > 128 || value.chars().any(char::is_control))
        || options.snapshot_types.iter().collect::<BTreeSet<_>>().len()
            != options.snapshot_types.len()
        || options
            .cleanup_algorithms
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != options.cleanup_algorithms.len()
        || (options.scan_mode != "snapshots"
            && (!options.snapshot_types.is_empty() || !options.cleanup_algorithms.is_empty()))
    {
        return Err(PluginError::new(
            "invalid-snapshot-filters",
            spaceprune_plugin_api::messages::text("snapper.invalid_filters"),
        ));
    }
    if !matches!(options.scan_mode.as_str(), "rust-targets" | "snapshots") {
        return Err(PluginError::new(
            "invalid-scan-mode",
            spaceprune_plugin_api::messages::text("snapper.invalid_mode"),
        ));
    }
    if let Some(ids) = &options.snapshot_ids
        && (options.scan_mode != "snapshots"
            || options.configs.len() != 1
            || ids.is_empty()
            || ids.contains(&0)
            || ids.iter().collect::<BTreeSet<_>>().len() != ids.len())
    {
        return Err(PluginError::new(
            "invalid-snapshot-ids",
            spaceprune_plugin_api::messages::text("snapper.invalid_exact_selection"),
        ));
    }
    core_fs::parse_bytes(&recommend.min_snapshot_size)?;
    if let Some(path) = options.relative_path.as_deref() {
        validate_relative_path(path)?;
    }
    validate_identity_options(&options)?;
    core_fs::parse_bytes(&recommend.min_target_size).map(|_| ())
}
