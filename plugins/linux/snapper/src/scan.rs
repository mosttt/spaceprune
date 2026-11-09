//! Snapshot inventory and size aggregation using the original operation context.

use super::*;

pub(super) fn scan(
    task: &TaskRequest,
    progress: &dyn ProgressSink,
    context: &OperationContext,
) -> PluginResult<HandlerScanResult> {
    let (options, recommend) = parse(task)?;
    validate_snapshot_host(&options)?;
    let scan_host_id = snapper_host_id()?;
    let minimum = core_fs::parse_bytes(if options.scan_mode == "snapshots" {
        &recommend.min_snapshot_size
    } else {
        &recommend.min_target_size
    })?;
    let available = match list_configs_cancellable(&options.snapper, context) {
        Ok(value) => value,
        Err(error) => {
            return Ok(HandlerScanResult {
                items: Vec::new(),
                issues: vec![HandlerIssue {
                    code: Some(error.code.clone()),
                    io_failure: error.io_failure.clone(),
                    level: IssueLevel::Warning,
                    message: error.message,
                    location: Some(options.snapper),
                }],
            });
        }
    };
    let mut result = HandlerScanResult::default();
    let selected: Vec<_> = if options.configs.is_empty() {
        available.into_iter().collect()
    } else {
        for name in options
            .configs
            .iter()
            .filter(|name| !available.contains_key(*name))
        {
            result.issues.push(HandlerIssue {
                code: None,
                io_failure: None,
                level: IssueLevel::Warning,
                message: spaceprune_plugin_api::messages::message(
                    "snapper.config_not_found",
                    &[("name", (name).to_string())],
                ),
                location: Some(name.clone()),
            });
        }
        options
            .configs
            .iter()
            .filter_map(|name| {
                available
                    .get(name)
                    .cloned()
                    .map(|path| (name.clone(), path))
            })
            .collect()
    };
    for (config, subvolume) in selected {
        if context.is_stopped() {
            break;
        }
        let snapshots = match list_snapshots_cancellable(
            &options.snapper,
            &config,
            options.scan_mode == "snapshots",
            context,
        ) {
            Ok(value) => value,
            Err(error) => {
                result.issues.push(HandlerIssue {
                    code: Some(error.code.clone()),
                    io_failure: error.io_failure.clone(),
                    level: IssueLevel::Warning,
                    message: error.message,
                    location: Some(config),
                });
                continue;
            }
        };
        let latest = latest_ids(&snapshots, recommend.keep_latest);
        let identity_error = validate_snapshot_identities(&options, &snapshots, None).err();
        let groups = group_pairs(snapshots);
        let selection_error = identity_error.or_else(|| {
            options
                .snapshot_ids
                .as_ref()
                .and_then(|ids| validate_selected_groups(&groups, ids).err())
        });
        if let Some(error) = &selection_error {
            result.issues.push(HandlerIssue {
                code: Some(error.code.clone()),
                io_failure: error.io_failure.clone(),
                level: IssueLevel::Error,
                message: error.message.clone(),
                location: Some(config.clone()),
            });
        }
        let logical_sizes = if options.scan_mode == "snapshots" {
            let numbers = groups
                .iter()
                .filter(|group| {
                    selection_error.is_none() && selection::visible(group, &options, None)
                })
                .flat_map(SnapshotGroup::numbers)
                .filter(|number| *number > 0)
                .collect();
            size::measure_snapshots(
                task,
                &config,
                &subvolume,
                numbers,
                &options,
                progress,
                context,
                &mut result.issues,
            )
        } else {
            BTreeMap::new()
        };
        for (index, group) in groups.iter().enumerate() {
            if context.is_stopped() {
                break;
            }
            if !selection::visible(group, &options, selection_error.as_ref()) {
                continue;
            }
            progress.emit(ProgressEvent {
                phase: ProgressPhase::Measure,
                module_id: Some(task.module_id.clone()),
                task_id: Some(task.task_id.clone()),
                handler_id: Some(task.handler_id.clone()),
                current: Some(index),
                total: Some(groups.len()),
                message: spaceprune_plugin_api::messages::message(
                    "snapper.scanning_snapshot",
                    &[
                        ("config", (config).to_string()),
                        ("p0", (group.label()).to_string()),
                    ],
                ),
            });
            if options.scan_mode == "snapshots" {
                let mut item = make_snapshot_item(&config, group, &recommend, minimum, &latest);
                selection::apply_blocker(&mut item, selection_error.as_ref());
                item.logical_bytes = group.snapshots.iter().try_fold(0_u64, |sum, snapshot| {
                    logical_sizes
                        .get(&snapshot.number)
                        .map(|bytes| sum.saturating_add(*bytes))
                });
                for snapshot in &group.snapshots {
                    let path = subvolume
                        .join(".snapshots")
                        .join(snapshot.number.to_string())
                        .join("snapshot");
                    if let Some(issue) = result.issues.iter().find(|issue| {
                        issue.code.as_deref() == Some("snapshot-logical-size-unavailable")
                            && issue.location.as_deref() == Some(path.to_string_lossy().as_ref())
                    }) {
                        item.reason.push_str("; ");
                        item.reason.push_str(&issue.message);
                    }
                }
                item.opaque["size_basis"] = json!("snapshot-apparent-size");
                item.opaque["reported_used_bytes"] =
                    json!(readonly::group_summary(group).used_bytes);
                bind_scan_identity(&mut item, group, &scan_host_id, &subvolume)?;
                result.items.push(item);
                continue;
            }
            let mut item = make_item(
                &config,
                &subvolume,
                group,
                &options,
                minimum,
                recommend.enabled,
                &latest,
                context,
                &mut result.issues,
            );
            bind_scan_identity(&mut item, group, &scan_host_id, &subvolume)?;
            result.items.push(item);
        }
    }
    Ok(result)
}
