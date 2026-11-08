//! Scan-bound snapshot identities, configuration scopes and execution checks.

use super::*;

pub(super) fn parse_cleanup_item(
    item: &CleanupItem,
    options: &Options,
) -> Result<(String, Vec<u64>), String> {
    let config = item
        .opaque
        .get("config")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            spaceprune_plugin_api::messages::text("snapper.invalid_config").to_owned()
        })?;
    if !options.configs.is_empty() && !options.configs.iter().any(|value| value == config) {
        return Err(spaceprune_plugin_api::messages::message(
            "snapper.config_outside_scope",
            &[("config", (config).to_string())],
        ));
    }
    let values = item
        .opaque
        .get("numbers")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .ok_or_else(|| {
            spaceprune_plugin_api::messages::text("snapper.invalid_numbers").to_owned()
        })?;
    let mut numbers = Vec::with_capacity(values.len());
    let mut unique = BTreeSet::new();
    for value in values {
        let number = value.as_u64().filter(|number| *number > 0).ok_or_else(|| {
            spaceprune_plugin_api::messages::text("snapper.invalid_number").to_owned()
        })?;
        if !unique.insert(number) {
            return Err(spaceprune_plugin_api::messages::message(
                "snapper.duplicate_number",
                &[("number", (number).to_string())],
            ));
        }
        numbers.push(number);
    }
    if options
        .snapshot_ids
        .as_ref()
        .is_some_and(|ids| numbers.iter().any(|id| !ids.contains(id)))
    {
        return Err(
            spaceprune_plugin_api::messages::text("snapper.numbers_outside_scope").to_owned(),
        );
    }
    let expected_id = format!(
        "{config}:{}",
        numbers
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join("+")
    );
    if item.id != expected_id {
        return Err(spaceprune_plugin_api::messages::message(
            "snapper.id_mismatch",
            &[
                ("p0", (item.id).to_string()),
                ("expected_id", (expected_id).to_string()),
            ],
        ));
    }
    Ok((config.to_owned(), numbers))
}

pub(super) fn validate_live_snapshots(
    options: &Options,
    snapshots: &[Snapshot],
    numbers: &[u64],
) -> Result<(), String> {
    validate_snapshot_identities(options, snapshots, Some(numbers))
        .map_err(|error| error.to_string())?;
    let selected = numbers.iter().copied().collect::<BTreeSet<_>>();
    let Some(group) = group_pairs(snapshots.to_vec())
        .into_iter()
        .find(|group| group.numbers().into_iter().collect::<BTreeSet<_>>() == selected)
    else {
        return Err(spaceprune_plugin_api::messages::text("snapper.state_changed").to_owned());
    };
    if group
        .snapshots
        .iter()
        .any(|snapshot| snapshot.number == 0 || snapshot.active || snapshot.is_default)
    {
        return Err(spaceprune_plugin_api::messages::text("snapper.protected").to_owned());
    }
    Ok(())
}

pub(super) fn snapshot_matches_filters(group: &SnapshotGroup, options: &Options) -> bool {
    (options.snapshot_types.is_empty()
        || options
            .snapshot_types
            .contains(&readonly::group_summary(group).snapshot_type))
        && (options.cleanup_algorithms.is_empty()
            || group
                .snapshots
                .iter()
                .all(|snapshot| options.cleanup_algorithms.contains(&snapshot.cleanup)))
}

pub(super) fn bind_scan_identity(
    item: &mut CleanupItem,
    group: &SnapshotGroup,
    host_id: &str,
    subvolume: &Path,
) -> PluginResult<()> {
    let identity = SnapshotScanIdentity {
        host_id: host_id.to_owned(),
        subvolume: subvolume.to_path_buf(),
        snapshots: group.snapshots.iter().map(SnapshotIdentity::from).collect(),
    };
    item.opaque["scan_identity"] = serde_json::to_value(identity).map_err(|_| {
        PluginError::new(
            "snapshot-identity-invalid",
            spaceprune_plugin_api::messages::text("snapper.bind_failed"),
        )
    })?;
    Ok(())
}

pub(super) fn validate_scanned_item(
    options: &Options,
    item: &CleanupItem,
    numbers: &[u64],
    live: &[Snapshot],
    subvolume: Option<&Path>,
) -> Result<bool, String> {
    let absent = numbers
        .iter()
        .all(|number| !live.iter().any(|snapshot| snapshot.number == *number));
    let Some(value) = item.opaque.get("scan_identity") else {
        return if options.snapshot_ids.is_some() {
            Ok(absent)
        } else {
            Err(spaceprune_plugin_api::messages::text("snapper.missing_fresh_identity").to_owned())
        };
    };
    let identity: SnapshotScanIdentity = serde_json::from_value(value.clone()).map_err(|_| {
        spaceprune_plugin_api::messages::text("snapper.invalid_scan_identity").to_owned()
    })?;
    if identity.host_id != snapper_host_id().map_err(|error| error.to_string())?
        || subvolume != Some(identity.subvolume.as_path())
        || identity.snapshots.len() != numbers.len()
        || identity
            .snapshots
            .iter()
            .map(|snapshot| snapshot.number)
            .collect::<BTreeSet<_>>()
            != numbers.iter().copied().collect()
        || identity.snapshots.iter().any(|expected| {
            expected.date.is_empty()
                || expected.snapshot_type.is_empty()
                || (!absent
                    && !live
                        .iter()
                        .any(|snapshot| SnapshotIdentity::from(snapshot) == *expected))
        })
    {
        return Err(
            spaceprune_plugin_api::messages::text("snapper.scan_identity_changed").to_owned(),
        );
    }
    if absent {
        return Ok(true);
    }
    let selected = SnapshotGroup {
        snapshots: live
            .iter()
            .filter(|snapshot| numbers.contains(&snapshot.number))
            .cloned()
            .collect(),
    };
    if !snapshot_matches_filters(&selected, options) {
        return Err(spaceprune_plugin_api::messages::text("snapper.filter_changed").to_owned());
    }
    Ok(false)
}

pub(super) fn validate_identity_options(options: &Options) -> PluginResult<()> {
    let Some(ids) = &options.snapshot_ids else {
        return Ok(());
    };
    let identity_ids: BTreeSet<_> = options
        .snapshot_identities
        .iter()
        .map(|identity| identity.number)
        .collect();
    if options
        .snapshot_host_id
        .as_ref()
        .is_none_or(|id| id.is_empty())
        || identity_ids != ids.iter().copied().collect()
        || options.snapshot_identities.len() != ids.len()
        || options
            .snapshot_identities
            .iter()
            .any(|identity| identity.date.is_empty() || identity.snapshot_type.is_empty())
    {
        return Err(PluginError::new(
            "missing-snapshot-identity",
            spaceprune_plugin_api::messages::text("snapper.exact_identity_required"),
        ));
    }
    Ok(())
}

pub(super) fn validate_snapshot_host(options: &Options) -> PluginResult<()> {
    validate_identity_options(options)?;
    if options.snapshot_ids.is_some()
        && options.snapshot_host_id.as_deref() != Some(snapper_host_id()?.as_str())
    {
        return Err(PluginError::new(
            "snapshot-host-mismatch",
            spaceprune_plugin_api::messages::text("snapper.other_host"),
        ));
    }
    Ok(())
}

pub(super) fn validate_snapshot_identities(
    options: &Options,
    snapshots: &[Snapshot],
    selected: Option<&[u64]>,
) -> PluginResult<()> {
    for expected in options
        .snapshot_identities
        .iter()
        .filter(|identity| selected.is_none_or(|numbers| numbers.contains(&identity.number)))
    {
        if !snapshots
            .iter()
            .any(|snapshot| SnapshotIdentity::from(snapshot) == *expected)
        {
            return Err(PluginError::new(
                "snapshot-identity-changed",
                spaceprune_plugin_api::messages::text("snapper.pinned_changed"),
            ));
        }
    }
    Ok(())
}
