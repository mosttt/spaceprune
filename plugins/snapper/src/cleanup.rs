use super::*;
use spaceprune_plugin_api::{ItemExecution, ItemExecutionStatus};
mod space;
use space::SpaceProbe;

type BatchKey = (String, String);

struct SelectedItem<'a> {
    group: usize,
    item: &'a CleanupItem,
    numbers: Vec<u64>,
}

pub(super) fn clean_batch(
    groups: &[CleanupBatchGroup<'_>],
    progress: &dyn ProgressSink,
    context: &OperationContext,
) -> Vec<PluginResult<HandlerCleanResult>> {
    let mut results = vec![
        Ok(HandlerCleanResult {
            unknown_reclaimed_items: Some(0),
            ..HandlerCleanResult::default()
        });
        groups.len()
    ];
    let mut batches: BTreeMap<BatchKey, Vec<SelectedItem<'_>>> = BTreeMap::new();
    let mut task_options: Vec<Option<Options>> = groups.iter().map(|_| None).collect();
    for (index, group) in groups.iter().enumerate() {
        if context.is_stopped() {
            break;
        }
        let options = match SnapperHandler
            .validate(group.task)
            .and_then(|()| parse(group.task))
            .and_then(|(options, _)| validate_snapshot_host(&options).map(|()| options))
        {
            Ok(options) => options,
            Err(error) => {
                if group.items.is_empty() {
                    results[index] = Err(error);
                    continue;
                }
                for item in group.items {
                    fail(
                        &mut results[index],
                        item,
                        error.clone(),
                        group.task,
                        progress,
                        context,
                    );
                }
                continue;
            }
        };
        for item in group.items {
            if context.is_stopped() {
                break;
            }
            let (config, numbers) = match parse_cleanup_item(item, &options) {
                Ok(value) => value,
                Err(message) => {
                    fail(
                        &mut results[index],
                        item,
                        PluginError::new("snapshot-cleanup-invalid-selection", message),
                        group.task,
                        progress,
                        context,
                    );
                    continue;
                }
            };
            let key = (options.snapper.clone(), config);
            batches.entry(key).or_default().push(SelectedItem {
                group: index,
                item,
                numbers,
            });
        }
        task_options[index] = Some(options);
    }

    for (key, mut selected) in batches {
        if context.is_stopped() {
            break;
        }
        let inventory =
            list_snapshots_cancellable(&key.0, &key.1, false, context).map_err(|mut error| {
                error.message = spaceprune_plugin_api::messages::message(
                    "snapper.revalidate_failed",
                    &[("error", (error).to_string())],
                );
                error
            });
        if context.is_stopped() {
            break;
        }
        let config_path = if selected
            .iter()
            .any(|entry| entry.item.opaque.get("scan_identity").is_some())
        {
            list_configs_cancellable(&key.0, context)
                .and_then(|configs| {
                    configs.get(&key.1).cloned().ok_or_else(|| {
                        PluginError::new(
                            "snapshot-config-missing",
                            spaceprune_plugin_api::messages::text("snapper.config_disappeared"),
                        )
                    })
                })
                .map(Some)
        } else {
            Ok(None)
        };
        let mut blocked = BTreeSet::new();
        selected.retain(|entry| {
            let options = task_options[entry.group]
                .as_ref()
                .expect("validated task options");
            let validation = inventory
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|snapshots| {
                    let subvolume = config_path.as_ref().map_err(Clone::clone)?;
                    validate_scanned_item(
                        options,
                        entry.item,
                        &entry.numbers,
                        snapshots,
                        subvolume.as_deref(),
                    )
                    .and_then(|absent| {
                        if !absent {
                            validate_live_snapshots(options, snapshots, &entry.numbers)?;
                        }
                        Ok(absent)
                    })
                    .map_err(|message| {
                        PluginError::new("snapshot-cleanup-invalid-selection", message)
                    })
                });
            if matches!(validation, Ok(true)) {
                record(
                    &mut results[entry.group],
                    groups[entry.group].task,
                    ItemExecution {
                        item_id: entry.item.id.clone(),
                        status: ItemExecutionStatus::AlreadyAbsent,
                        reclaimed_estimate: Some(0),
                        reclaimed_lower_bound: 0,
                        failure: None,
                    },
                    progress,
                    context,
                );
                false
            } else if let Err(message) = validation {
                blocked.extend(entry.numbers.iter().copied());
                fail(
                    &mut results[entry.group],
                    entry.item,
                    message,
                    groups[entry.group].task,
                    progress,
                    context,
                );
                false
            } else {
                true
            }
        });
        selected.retain(|entry| {
            if entry.numbers.iter().any(|id| blocked.contains(id)) {
                fail(
                    &mut results[entry.group],
                    entry.item,
                    PluginError::new(
                        "snapshot-cleanup-invalid-selection",
                        spaceprune_plugin_api::messages::text("snapper.conflicting_identity"),
                    ),
                    groups[entry.group].task,
                    progress,
                    context,
                );
                false
            } else {
                true
            }
        });
        let Some(first) = selected.first() else {
            continue;
        };
        let task = groups[first.group].task;
        let numbers: BTreeSet<_> = selected
            .iter()
            .flat_map(|entry| entry.numbers.iter().copied())
            .collect();
        emit(
            progress,
            task,
            ProgressPhase::Clean,
            selected.len(),
            spaceprune_plugin_api::messages::message(
                "snapper.deleting_sync",
                &[
                    ("p0", (numbers.len()).to_string()),
                    ("p1", (key.1).to_string()),
                ],
            ),
        );
        if context.is_stopped() {
            break;
        }
        let path = config_path.and_then(|path| match path {
            Some(path) => Ok(path),
            None => list_configs_cancellable(&key.0, context).and_then(|configs| {
                configs.get(&key.1).cloned().ok_or_else(|| {
                    PluginError::new(
                        "snapshot-config-missing",
                        spaceprune_plugin_api::messages::text("snapper.measure_config_missing"),
                    )
                })
            }),
        });
        if context.is_stopped() {
            break;
        }
        let space = SpaceProbe::begin(key.1.clone(), path.map_err(|error| error.to_string()));
        let mut deletion = command(&key.0);
        deletion.args(["-c", &key.1, "delete", "--sync"]);
        deletion.args(numbers.iter().map(u64::to_string));
        let mut operation = context.child(std::time::Duration::from_secs(300));
        operation.budget.first_output_timeout = operation
            .budget
            .first_output_timeout
            .min(std::time::Duration::from_secs(300));
        operation.budget.idle_timeout = operation
            .budget
            .idle_timeout
            .min(std::time::Duration::from_secs(300));
        let output = spaceprune_access::process::capture(deletion, &operation);
        // One observation for the shared batch; per-selection outcomes keep their original sizes.
        results[first.group]
            .as_mut()
            .expect("validated task result")
            .space_changes
            .push(space.finish());
        let failure = match &output {
            Ok(output) if output.status.success() => None,
            Ok(output) => Some(PluginError::new(
                "snapper-delete-failed",
                command_failure("snapper delete", output),
            )),
            Err(error) => {
                let mut failure = PluginError::new(
                    "snapper-delete-failed",
                    spaceprune_plugin_api::messages::message(
                        "snapper.delete_failed",
                        &[("error", (error).to_string())],
                    ),
                );
                failure.io_failure = Some(spaceprune_access::error::classify(error));
                Some(failure)
            }
        };
        // A failed batch may already have removed a prefix. Never retry deletion or ask again.
        let remaining = failure.as_ref().and_then(|_| {
            list_snapshots_cancellable(&key.0, &key.1, false, context)
                .ok()
                .map(|snapshots| {
                    snapshots
                        .into_iter()
                        .map(|snapshot| snapshot.number)
                        .collect::<BTreeSet<_>>()
                })
        });
        emit(
            progress,
            task,
            ProgressPhase::Clean,
            selected.len(),
            spaceprune_plugin_api::messages::message(
                "snapper.delete_finished",
                &[("p0", (key.1).to_string())],
            ),
        );
        for entry in selected {
            let gone = remaining
                .as_ref()
                .is_some_and(|ids| entry.numbers.iter().all(|number| !ids.contains(number)));
            if let Some(failure) = failure.as_ref().filter(|_| !gone) {
                let mut message = failure.message.clone();
                if let Some(ids) = &remaining {
                    let count = entry
                        .numbers
                        .iter()
                        .filter(|number| ids.contains(number))
                        .count();
                    message.push_str(&spaceprune_plugin_api::messages::message(
                        "snapper.remaining",
                        &[
                            ("count", (count).to_string()),
                            ("p0", (entry.numbers.len()).to_string()),
                        ],
                    ));
                } else {
                    message.push_str(spaceprune_plugin_api::messages::text(
                        "snapper.remaining_unverified",
                    ));
                }
                let status = match &remaining {
                    Some(ids) if entry.numbers.iter().any(|number| !ids.contains(number)) => {
                        ItemExecutionStatus::PartiallyApplied
                    }
                    Some(_) => ItemExecutionStatus::Failed,
                    None => ItemExecutionStatus::Uncertain,
                };
                record(
                    &mut results[entry.group],
                    groups[entry.group].task,
                    ItemExecution {
                        item_id: entry.item.id.clone(),
                        status,
                        reclaimed_estimate: (status == ItemExecutionStatus::Failed).then_some(0),
                        reclaimed_lower_bound: 0,
                        failure: Some(ItemFailure {
                            authorization: failure.authorization,
                            io_failure: failure.io_failure.clone(),
                            item_id: entry.item.id.clone(),
                            message,
                        }),
                    },
                    progress,
                    context,
                );
            } else {
                record(
                    &mut results[entry.group],
                    groups[entry.group].task,
                    ItemExecution {
                        item_id: entry.item.id.clone(),
                        status: ItemExecutionStatus::Succeeded,
                        reclaimed_estimate: entry.item.reclaimable_bytes,
                        reclaimed_lower_bound: 0,
                        failure: None,
                    },
                    progress,
                    context,
                );
            }
        }
    }
    for (group, result) in groups.iter().zip(&mut results) {
        if let Ok(result) = result {
            for item in group.items {
                if !result
                    .outcomes
                    .iter()
                    .any(|outcome| outcome.item_id == item.id)
                {
                    result.record_outcome(ItemExecution {
                        item_id: item.id.clone(),
                        status: ItemExecutionStatus::NotAttempted,
                        reclaimed_estimate: Some(0),
                        reclaimed_lower_bound: 0,
                        failure: None,
                    });
                }
            }
        }
    }
    results
}

fn record(
    result: &mut PluginResult<HandlerCleanResult>,
    task: &TaskRequest,
    outcome: ItemExecution,
    progress: &dyn ProgressSink,
    context: &OperationContext,
) {
    let result = result.as_mut().expect("validated task result");
    result.record_outcome(outcome.clone());
    if progress.cleanup_item_finished(task, &outcome).is_err() {
        context.cancel();
    }
    progress.record_cleanup(
        task,
        result.succeeded,
        result.reclaimed_bytes,
        result.unknown_reclaimed_items(),
    );
}

fn fail(
    result: &mut PluginResult<HandlerCleanResult>,
    item: &CleanupItem,
    failure: PluginError,
    task: &TaskRequest,
    progress: &dyn ProgressSink,
    context: &OperationContext,
) {
    record(
        result,
        task,
        ItemExecution {
            item_id: item.id.clone(),
            status: ItemExecutionStatus::Failed,
            reclaimed_estimate: Some(0),
            reclaimed_lower_bound: 0,
            failure: Some(ItemFailure {
                authorization: failure.authorization,
                io_failure: failure.io_failure,
                item_id: item.id.clone(),
                message: failure.message,
            }),
        },
        progress,
        context,
    );
}

fn emit(
    progress: &dyn ProgressSink,
    task: &TaskRequest,
    phase: ProgressPhase,
    count: usize,
    message: String,
) {
    progress.emit(ProgressEvent {
        phase,
        module_id: Some(task.module_id.clone()),
        task_id: Some(task.task_id.clone()),
        handler_id: Some(task.handler_id.clone()),
        current: None,
        total: Some(count),
        message,
    });
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
