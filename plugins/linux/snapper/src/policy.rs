//! Snapshot grouping, recommendation policy and visible result construction.

use super::*;

pub(super) fn latest_ids(snapshots: &[Snapshot], count: usize) -> BTreeSet<u64> {
    let mut numbers: Vec<_> = snapshots
        .iter()
        .filter(|snapshot| snapshot.number > 0)
        .map(|snapshot| snapshot.number)
        .collect();
    numbers.sort_unstable_by(|left, right| right.cmp(left));
    numbers.truncate(count);
    numbers.into_iter().collect()
}

pub(super) fn group_pairs(snapshots: Vec<Snapshot>) -> Vec<SnapshotGroup> {
    let mut by_number: BTreeMap<_, _> = snapshots
        .into_iter()
        .map(|snapshot| (snapshot.number, snapshot))
        .collect();
    let pairs: Vec<_> = by_number
        .values()
        .filter_map(|snapshot| snapshot.pre_number.map(|pre| (pre, snapshot.number)))
        .collect();
    let mut groups = Vec::new();
    for (pre, post) in pairs {
        if pre != post && by_number.contains_key(&pre) && by_number.contains_key(&post) {
            groups.push(SnapshotGroup {
                snapshots: vec![
                    by_number.remove(&pre).unwrap(),
                    by_number.remove(&post).unwrap(),
                ],
            });
        }
    }
    groups.extend(by_number.into_values().map(|snapshot| SnapshotGroup {
        snapshots: vec![snapshot],
    }));
    groups.sort_by_key(|group| group.snapshots[0].number);
    groups
}

pub(super) fn validate_selected_groups(groups: &[SnapshotGroup], ids: &[u64]) -> PluginResult<()> {
    let selected: BTreeSet<_> = ids.iter().copied().collect();
    let available: BTreeSet<_> = groups.iter().flat_map(SnapshotGroup::numbers).collect();
    if !selected.is_subset(&available) {
        return Err(PluginError::new(
            "snapshot-selection-changed",
            spaceprune_plugin_api::messages::text("snapper.selection_gone"),
        ));
    }
    for group in groups {
        let numbers = group.numbers();
        if numbers.iter().any(|id| selected.contains(id))
            && !numbers.iter().all(|id| selected.contains(id))
        {
            return Err(PluginError::new(
                "incomplete-snapshot-pair",
                spaceprune_plugin_api::messages::text("snapper.pair_required"),
            ));
        }
    }
    Ok(())
}

pub(super) fn snapshot_timestamp(date: &str) -> Option<u64> {
    humantime::parse_rfc3339_weak(&format!("{date}Z"))
        .ok()
        .and_then(core_fs::unix_timestamp)
}

pub(super) fn make_snapshot_item(
    config: &str,
    group: &SnapshotGroup,
    recommend: &Recommend,
    minimum: u64,
    latest: &BTreeSet<u64>,
) -> CleanupItem {
    let summary = readonly::group_summary(group);
    let current_system = group.snapshots.len() == 1 && group.snapshots[0].number == 0;
    let modified_at = snapshot_timestamp(&summary.date);
    let old_enough = recommend.older_than_days == 0
        || core_fs::is_older_than(
            modified_at,
            Duration::from_secs(recommend.older_than_days.saturating_mul(86_400)),
        );
    let large_enough = minimum == 0 || summary.used_bytes.is_some_and(|bytes| bytes >= minimum);
    let (recommendation, reason) = if current_system {
        (
            Recommendation::Unavailable,
            spaceprune_plugin_api::messages::text("snapper.current_system_reason"),
        )
    } else if summary.protected {
        (
            Recommendation::Unavailable,
            spaceprune_plugin_api::messages::text("snapper.protected"),
        )
    } else if group.numbers().iter().any(|number| latest.contains(number)) {
        (
            Recommendation::Optional,
            spaceprune_plugin_api::messages::text("snapper.keep_latest"),
        )
    } else if !recommend.enabled {
        (
            Recommendation::Optional,
            spaceprune_plugin_api::messages::text("snapper.recommend_disabled"),
        )
    } else if !old_enough {
        (
            Recommendation::Optional,
            spaceprune_plugin_api::messages::text("snapper.too_recent"),
        )
    } else if !large_enough {
        (
            Recommendation::Optional,
            spaceprune_plugin_api::messages::text("snapper.too_small"),
        )
    } else {
        (
            Recommendation::Recommended,
            spaceprune_plugin_api::messages::text("snapper.recommended"),
        )
    };
    CleanupItem {
        logical_size_applicability: if current_system {
            spaceprune_plugin_api::SizeApplicability::NotApplicable
        } else {
            spaceprune_plugin_api::SizeApplicability::Applicable
        },
        id: format!(
            "{config}:{}",
            summary
                .numbers
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join("+")
        ),
        parent_id: None,
        name: if current_system {
            spaceprune_plugin_api::messages::message(
                "snapper.current_system_name",
                &[("config", config.to_owned())],
            )
        } else {
            format!("{config} {}", group.label())
        },
        location: if summary.date.is_empty() {
            spaceprune_plugin_api::messages::message(
                "snapper.configuration",
                &[("config", (config).to_string())],
            )
        } else {
            summary.date
        },
        logical_bytes: None,
        reclaimable_bytes: None,
        modified_at,
        recommendation,
        reason: reason.to_owned(),
        tags: vec!["snapshots".to_owned(), "snapper".to_owned()],
        opaque: json!({"config": config, "numbers": summary.numbers, "protected": summary.protected}),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn make_item(
    config: &str,
    subvolume: &Path,
    group: &SnapshotGroup,
    options: &Options,
    minimum: u64,
    enabled: bool,
    latest: &BTreeSet<u64>,
    context: &OperationContext,
    issues: &mut Vec<HandlerIssue>,
) -> CleanupItem {
    let numbers = group.numbers();
    let current_system = group.snapshots.len() == 1 && group.snapshots[0].number == 0;
    let hard_protected = group
        .snapshots
        .iter()
        .any(|snapshot| snapshot.number == 0 || snapshot.active || snapshot.is_default);
    let soft_protected = group.snapshots.len() > 1
        || group
            .snapshots
            .iter()
            .any(|snapshot| latest.contains(&snapshot.number) || is_manual_or_important(snapshot));
    let mut bytes = 0_u64;
    let mut measured = false;
    let mut scan_failed = false;
    for snapshot in &group.snapshots {
        if snapshot.number == 0 {
            continue;
        }
        let mut root = subvolume
            .join(".snapshots")
            .join(snapshot.number.to_string())
            .join("snapshot");
        if let Some(relative) = options.relative_path.as_deref() {
            root.push(relative);
        }
        match measure_targets(&root, options, context) {
            Ok(value) => {
                bytes = bytes.saturating_add(value);
                measured = true;
            }
            Err(error) => {
                scan_failed = true;
                issues.push(HandlerIssue {
                    code: None,
                    io_failure: Some(spaceprune_access::error::classify(&error)),
                    level: IssueLevel::Warning,
                    message: spaceprune_plugin_api::messages::message(
                        "snapper.scan_failed",
                        &[
                            ("p0", (snapshot.number).to_string()),
                            ("error", (error).to_string()),
                        ],
                    ),
                    location: Some(root.to_string_lossy().into_owned()),
                });
            }
        }
    }
    let (recommendation, reason) = if current_system {
        (
            Recommendation::Unavailable,
            spaceprune_plugin_api::messages::text("snapper.current_system_reason").to_owned(),
        )
    } else if hard_protected {
        (
            Recommendation::Unavailable,
            spaceprune_plugin_api::messages::text("snapper.protected").to_owned(),
        )
    } else if scan_failed {
        (
            Recommendation::Unavailable,
            spaceprune_plugin_api::messages::text("snapper.scan_incomplete").to_owned(),
        )
    } else if soft_protected {
        (
            Recommendation::Optional,
            spaceprune_plugin_api::messages::text("snapper.manual_selection").to_owned(),
        )
    } else if enabled && bytes >= minimum {
        (
            Recommendation::Recommended,
            spaceprune_plugin_api::messages::text("snapper.build_recommended").to_owned(),
        )
    } else if !enabled {
        (
            Recommendation::Optional,
            spaceprune_plugin_api::messages::text("snapper.build_disabled").to_owned(),
        )
    } else {
        (
            Recommendation::Optional,
            spaceprune_plugin_api::messages::text("snapper.build_below_threshold").to_owned(),
        )
    };
    let descriptions = group
        .snapshots
        .iter()
        .filter(|snapshot| !snapshot.description.is_empty())
        .map(|snapshot| snapshot.description.as_str())
        .collect::<Vec<_>>()
        .join(" / ");
    let dates = group
        .snapshots
        .iter()
        .filter(|snapshot| !snapshot.date.is_empty())
        .map(|snapshot| snapshot.date.as_str())
        .collect::<Vec<_>>()
        .join(" / ");
    let label = group.label();
    CleanupItem {
        logical_size_applicability: if current_system {
            spaceprune_plugin_api::SizeApplicability::NotApplicable
        } else {
            spaceprune_plugin_api::SizeApplicability::Applicable
        },
        id: format!(
            "{config}:{}",
            numbers
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join("+")
        ),
        parent_id: None,
        name: if current_system {
            spaceprune_plugin_api::messages::message(
                "snapper.current_system_name",
                &[("config", config.to_owned())],
            )
        } else if descriptions.is_empty() {
            format!("{config} {label}")
        } else {
            format!("{config} {label} · {descriptions}")
        },
        location: if dates.is_empty() {
            spaceprune_plugin_api::messages::message(
                "snapper.configuration",
                &[("config", (config).to_string())],
            )
        } else {
            dates
        },
        logical_bytes: (measured && !scan_failed).then_some(bytes),
        reclaimable_bytes: None,
        modified_at: None,
        recommendation,
        reason,
        tags: vec!["snapshots".to_owned(), "snapper".to_owned()],
        opaque: json!({"config": config, "numbers": numbers, "protected": hard_protected}),
    }
}

pub(super) fn is_manual_or_important(snapshot: &Snapshot) -> bool {
    snapshot.cleanup.is_empty()
        || snapshot.snapshot_type == "pre"
        || snapshot.snapshot_type == "post"
        || snapshot
            .userdata
            .as_object()
            .is_some_and(|value| value.contains_key("important"))
}

pub(super) fn validate_relative_path(path: &Path) -> PluginResult<()> {
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(PluginError::new(
            "invalid-relative-path",
            spaceprune_plugin_api::messages::text("snapper.invalid_relative_path"),
        ));
    }
    Ok(())
}
