//! Retry only denied, already confirmed selections through the host's shared native service.

use super::*;
use std::collections::BTreeMap;
#[cfg(test)]
use std::io;
#[cfg(test)]
mod execution_tests;

struct RequestGroup {
    task: TaskRequest,
    items: Vec<CleanupItem>,
}

struct Request {
    groups: Vec<RequestGroup>,
}

struct Reply {
    results: Vec<HandlerCleanResult>,
}

fn permission_denied(failure: Option<&spaceprune_access::IoFailure>) -> bool {
    failure.is_some_and(|failure| failure.kind == spaceprune_access::IoFailureKind::AccessDenied)
}

mod resumed;
use resumed::ResumedProgress;

pub(super) fn retry_denied_with_access(
    groups: &[CleanupBatchGroup<'_>],
    mut results: Vec<PluginResult<HandlerCleanResult>>,
    progress: &dyn ProgressSink,
    context: &OperationContext,
    access: Option<&dyn spaceprune_plugin_api::CleanupAccess>,
) -> Vec<PluginResult<HandlerCleanResult>> {
    let Some(access) = access else {
        return results;
    };
    let previous = groups
        .iter()
        .zip(&results)
        .flat_map(|(group, result)| {
            result.as_ref().ok().into_iter().flat_map(move |result| {
                result.outcomes.iter().map(move |outcome| {
                    (
                        (
                            group.task.module_id.clone(),
                            group.task.task_id.clone(),
                            outcome.item_id.clone(),
                        ),
                        outcome.clone(),
                    )
                })
            })
        })
        .collect::<BTreeMap<_, _>>();
    retry_with(
        groups,
        &mut results,
        progress,
        context,
        |method, request| {
            let selected: Vec<_> = request
                .groups
                .iter()
                .map(|group| CleanupBatchGroup {
                    task: &group.task,
                    items: &group.items,
                })
                .collect();
            let resumed = ResumedProgress::new(&previous, progress);
            let results = access.clean_authorized(method, &selected, &resumed, context)?;
            Ok(Reply {
                results: results.into_iter().collect::<Result<Vec<_>, _>>()?,
            })
        },
    );
    results
}

fn retry_with(
    groups: &[CleanupBatchGroup<'_>],
    results: &mut [PluginResult<HandlerCleanResult>],
    progress: &dyn ProgressSink,
    context: &OperationContext,
    mut authorize: impl FnMut(&str, &Request) -> PluginResult<Reply>,
) {
    #[cfg(target_os = "linux")]
    let methods = &["sudo", "pkexec"][..];
    #[cfg(target_os = "windows")]
    let methods = &["uac"][..];
    #[cfg(target_os = "macos")]
    let methods = &["native"][..];
    for &method in methods {
        if context.is_stopped() {
            return;
        }
        let mut indices = Vec::new();
        let mut requests = Vec::new();
        for (index, (group, result)) in groups.iter().zip(results.iter()).enumerate() {
            let Ok(options) = parse_options(group.task) else {
                continue;
            };
            let selected_method = match options.elevation.as_str() {
                "auto" if cfg!(target_os = "windows") => "uac",
                "auto" if cfg!(target_os = "macos") => "native",
                "auto" => spaceprune_plugin_api::automatic_linux_authorization_method(),
                other => other,
            };
            if selected_method != method {
                continue;
            }
            let denied: BTreeSet<_> = match result {
                Ok(result) => result
                    .failures
                    .iter()
                    .filter(|failure| permission_denied(failure.io_failure.as_ref()))
                    .map(|failure| failure.item_id.as_str())
                    .collect(),
                Err(error) if permission_denied(error.io_failure.as_ref()) => {
                    group.items.iter().map(|item| item.id.as_str()).collect()
                }
                _ => continue,
            };
            let items: Vec<_> = group
                .items
                .iter()
                .filter(|item| denied.contains(item.id.as_str()))
                .filter(|item| item.opaque.get("filesystem-identity").is_some())
                .cloned()
                .collect();
            if items.is_empty() {
                continue;
            }
            let mut task = group.task.clone();
            let Some(roots) = task.options.get_mut("roots").and_then(Value::as_array_mut) else {
                continue;
            };
            let mut resolved = true;
            for (root, configured) in roots.iter_mut().zip(&options.roots) {
                match resolve_root(&configured.path) {
                    Ok(path) => root["path"] = json!(path),
                    Err(_) => {
                        resolved = false;
                        break;
                    }
                }
            }
            if !resolved {
                continue;
            }
            indices.push(index);
            requests.push(RequestGroup { task, items });
        }
        if requests.is_empty() {
            continue;
        }
        let request = Request { groups: requests };
        let count = request.groups.iter().map(|group| group.items.len()).sum();
        let first = &request.groups[0].task;
        progress.emit(ProgressEvent {
            phase: ProgressPhase::Authorize,
            module_id: Some(first.module_id.clone()),
            task_id: Some(first.task_id.clone()),
            handler_id: Some(first.handler_id.clone()),
            current: Some(0),
            total: Some(count),
            message: spaceprune_plugin_api::messages::message(
                "filesystem.authorization",
                &[("count", (count).to_string())],
            ),
        });
        match authorize(method, &request) {
            Ok(reply)
                if reply.results.len() == indices.len()
                    && reply
                        .results
                        .iter()
                        .zip(&request.groups)
                        .all(|(result, group)| {
                            let ids: BTreeSet<_> = result
                                .outcomes
                                .iter()
                                .map(|outcome| outcome.item_id.as_str())
                                .collect();
                            ids.len() == group.items.len()
                                && result.outcomes.len() == group.items.len()
                                && group
                                    .items
                                    .iter()
                                    .all(|item| ids.contains(item.id.as_str()))
                        }) =>
            {
                for ((index, _requested), elevated) in indices
                    .iter()
                    .copied()
                    .zip(&request.groups)
                    .zip(reply.results)
                {
                    if let Ok(local) = &mut results[index] {
                        merge_resumed_cleanup(local, elevated);
                    } else {
                        results[index] = Ok(elevated);
                    }
                    if let Ok(result) = &results[index] {
                        progress.record_cleanup(
                            groups[index].task,
                            result.succeeded,
                            result.reclaimed_bytes,
                            result.unknown_reclaimed_items(),
                        );
                    }
                }
            }
            outcome => {
                let reason = outcome.err().unwrap_or_else(|| {
                    PluginError::new(
                        "cleanup-result-invalid",
                        spaceprune_plugin_api::messages::text("filesystem.invalid_worker_result"),
                    )
                });
                let diagnostic = if matches!(
                    reason.authorization,
                    Some(
                        spaceprune_access::PermissionOutcome::HelperMissing
                            | spaceprune_access::PermissionOutcome::HelperOutdated
                            | spaceprune_access::PermissionOutcome::HelperUntrusted
                    )
                ) {
                    "filesystem.authorized_cleanup_not_started"
                } else {
                    "filesystem.unconfirmed_authorized_cleanup"
                };
                for (index, requested) in indices.iter().copied().zip(&request.groups) {
                    if let Ok(local) = &mut results[index] {
                        let ids: BTreeSet<_> = requested
                            .items
                            .iter()
                            .map(|item| item.id.as_str())
                            .collect();
                        for failure in &mut local.failures {
                            if ids.contains(failure.item_id.as_str()) {
                                failure.authorization = reason.authorization;
                                failure.message = spaceprune_plugin_api::messages::message(
                                    diagnostic,
                                    &[
                                        ("p0", (failure.message).to_string()),
                                        ("reason", (reason).to_string()),
                                    ],
                                );
                            }
                        }
                        for outcome in &mut local.outcomes {
                            if let Some(failure) = local
                                .failures
                                .iter()
                                .find(|failure| failure.item_id == outcome.item_id)
                            {
                                outcome.failure = Some(failure.clone());
                            }
                        }
                    } else if let Err(failure) = &mut results[index] {
                        failure.authorization = reason.authorization;
                        failure.message = spaceprune_plugin_api::messages::message(
                            diagnostic,
                            &[
                                ("p0", (failure.message).to_string()),
                                ("reason", (reason).to_string()),
                            ],
                        );
                    }
                }
                // A refusal ends authorization attempts for this batch, including other methods.
                break;
            }
        }
    }
}

fn combine_resumed_outcome(
    previous: &spaceprune_plugin_api::ItemExecution,
    next: &mut spaceprune_plugin_api::ItemExecution,
) {
    use spaceprune_plugin_api::ItemExecutionStatus;
    let known = previous
        .known_reclaimed_bytes()
        .saturating_add(next.known_reclaimed_bytes());
    next.reclaimed_estimate = previous
        .reclaimed_estimate
        .zip(next.reclaimed_estimate)
        .map(|(before, after)| before.saturating_add(after));
    next.reclaimed_lower_bound = if next.reclaimed_estimate.is_none() {
        known
    } else {
        0
    };
    if previous.status == ItemExecutionStatus::PartiallyApplied {
        next.status = match next.status {
            ItemExecutionStatus::Succeeded | ItemExecutionStatus::AlreadyAbsent => {
                ItemExecutionStatus::Succeeded
            }
            ItemExecutionStatus::Uncertain => ItemExecutionStatus::Uncertain,
            _ => ItemExecutionStatus::PartiallyApplied,
        };
        if next.status == ItemExecutionStatus::PartiallyApplied && next.failure.is_none() {
            next.failure = previous.failure.clone();
        }
    }
}

fn merge_resumed_cleanup(local: &mut HandlerCleanResult, elevated: HandlerCleanResult) {
    let mut outcomes = std::mem::take(&mut local.outcomes);
    for mut next in elevated.outcomes {
        if let Some(index) = outcomes.iter().position(|old| old.item_id == next.item_id) {
            let previous = &outcomes[index];
            combine_resumed_outcome(previous, &mut next);
            outcomes[index] = next;
        } else {
            outcomes.push(next);
        }
    }
    *local = HandlerCleanResult {
        unknown_reclaimed_items: Some(0),
        ..HandlerCleanResult::default()
    };
    for outcome in outcomes {
        local.record_outcome(outcome);
    }
}

#[cfg(test)]
mod reconciliation_tests;
#[cfg(test)]
mod tests;
