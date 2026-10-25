//! Exact selection validation and shared native execution for both ordinary and elevated work.

use super::*;
use spaceprune_access::{
    NativePath,
    filesystem::{FileIdentity, Removal, remove_selected},
};
use spaceprune_plugin_api::{ItemExecution, ItemExecutionStatus};

pub(super) fn clean_local(
    task: &TaskRequest,
    items: &[CleanupItem],
    progress: &dyn ProgressSink,
    context: &OperationContext,
) -> PluginResult<HandlerCleanResult> {
    PathsHandler.validate(task)?;
    Ok(clean_observed(
        task,
        items,
        context,
        progress,
        |result, item| {
            progress.record_cleanup(
                task,
                result.succeeded,
                result.reclaimed_bytes,
                result.unknown_reclaimed_items(),
            );
            emit_progress(
                progress,
                task,
                ProgressPhase::Clean,
                Some(result.attempted),
                Some(items.len()),
                spaceprune_plugin_api::messages::message(
                    "filesystem.process_item",
                    &[("p0", (item.location).to_string())],
                ),
            );
        },
    ))
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "windows", target_os = "macos")
))]
pub(super) fn clean_with_progress(
    task: &TaskRequest,
    items: &[CleanupItem],
    context: &OperationContext,
    report: impl FnMut(&HandlerCleanResult, &CleanupItem),
) -> HandlerCleanResult {
    clean_observed(
        task,
        items,
        context,
        &spaceprune_plugin_api::NullProgress,
        report,
    )
}

fn clean_observed(
    task: &TaskRequest,
    items: &[CleanupItem],
    context: &OperationContext,
    progress: &dyn ProgressSink,
    mut report: impl FnMut(&HandlerCleanResult, &CleanupItem),
) -> HandlerCleanResult {
    let mut result = HandlerCleanResult {
        unknown_reclaimed_items: Some(0),
        ..HandlerCleanResult::default()
    };
    let mut updated = Instant::now();
    for (index, item) in items.iter().enumerate() {
        if context.is_stopped() {
            result.record_outcome(ItemExecution {
                item_id: item.id.clone(),
                status: ItemExecutionStatus::NotAttempted,
                reclaimed_estimate: Some(0),
                reclaimed_lower_bound: 0,
                failure: None,
            });
            continue;
        }
        let removal = remove_item(task, item, context);
        let (status, failure) = match removal.error {
            None => (
                if removal.already_absent {
                    ItemExecutionStatus::AlreadyAbsent
                } else {
                    ItemExecutionStatus::Succeeded
                },
                None,
            ),
            Some(error) => {
                let status = if removal.removed_entries > 0
                    || spaceprune_access::error::has_effects(&error)
                {
                    ItemExecutionStatus::PartiallyApplied
                } else if error.kind() == std::io::ErrorKind::Interrupted {
                    ItemExecutionStatus::NotAttempted
                } else {
                    ItemExecutionStatus::Failed
                };
                (
                    status,
                    Some(ItemFailure {
                        authorization: None,
                        item_id: item.id.clone(),
                        io_failure: Some(cleanup_failure(&error)),
                        message: if removal.removed_entries == 0 {
                            error.to_string()
                        } else {
                            spaceprune_plugin_api::messages::message(
                                "filesystem.partially_applied",
                                &[
                                    ("p0", (removal.removed_entries).to_string()),
                                    ("error", (error).to_string()),
                                ],
                            )
                        },
                    }),
                )
            }
        };
        let estimate = if matches!(
            status,
            ItemExecutionStatus::Failed | ItemExecutionStatus::NotAttempted
        ) {
            Some(0)
        } else if removal.already_absent || item.reclaimable_bytes.is_some() {
            Some(removal.reclaimed_estimate)
        } else {
            None
        };
        result.record_outcome(ItemExecution {
            item_id: item.id.clone(),
            status,
            reclaimed_estimate: estimate,
            reclaimed_lower_bound: if estimate.is_none() {
                removal.reclaimed_estimate
            } else {
                0
            },
            failure,
        });
        if progress
            .cleanup_item_finished(task, result.outcomes.last().expect("recorded outcome"))
            .is_err()
        {
            context.cancel();
        }
        if index == 0
            || index + 1 == items.len()
            || updated.elapsed() >= Duration::from_millis(100)
            || context.is_stopped()
        {
            report(&result, item);
            updated = Instant::now();
        }
    }
    result
}

// Darwin EPERM can be TCC, immutable flags or system policy; administrator access is not a bypass.
fn cleanup_failure(error: &std::io::Error) -> spaceprune_access::IoFailure {
    let mut failure = spaceprune_access::error::classify(error);
    if cfg!(target_os = "macos") && failure.os_code == Some(1) {
        failure.kind = spaceprune_access::IoFailureKind::PolicyBlocked;
    }
    failure
}

fn remove_item(task: &TaskRequest, item: &CleanupItem, context: &OperationContext) -> Removal {
    let prepared = (|| -> std::io::Result<_> {
        let invalid = |message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message);
        if task.handler_id != "builtin.filesystem/paths"
            || item.recommendation == Recommendation::Unavailable
        {
            return Err(invalid(spaceprune_plugin_api::messages::text(
                "filesystem.invalid_item",
            )));
        }
        let options = parse_options(task).map_err(std::io::Error::other)?;
        let native: NativePath =
            serde_json::from_value(item.opaque.get("native-path").cloned().ok_or_else(|| {
                invalid(spaceprune_plugin_api::messages::text(
                    "filesystem.missing_path",
                ))
            })?)
            .map_err(std::io::Error::other)?;
        let path = native.to_path_buf().ok_or_else(|| {
            invalid(spaceprune_plugin_api::messages::text(
                "filesystem.wrong_platform",
            ))
        })?;
        if item.opaque.get("path").and_then(Value::as_str) != Some(path.to_string_lossy().as_ref())
        {
            return Err(invalid(spaceprune_plugin_api::messages::text(
                "filesystem.path_identity",
            )));
        }
        let identity: FileIdentity =
            serde_json::from_value(item.opaque.get("filesystem-identity").cloned().ok_or_else(
                || {
                    invalid(spaceprune_plugin_api::messages::text(
                        "filesystem.missing_identity",
                    ))
                },
            )?)
            .map_err(std::io::Error::other)?;
        let root_identity: FileIdentity =
            serde_json::from_value(item.opaque.get("root-identity").cloned().ok_or_else(|| {
                invalid(spaceprune_plugin_api::messages::text(
                    "filesystem.missing_root_identity",
                ))
            })?)
            .map_err(std::io::Error::other)?;
        let index = item
            .opaque
            .get("root-index")
            .and_then(Value::as_u64)
            .and_then(|index| usize::try_from(index).ok())
            .ok_or_else(|| {
                invalid(spaceprune_plugin_api::messages::text(
                    "filesystem.missing_root_index",
                ))
            })?;
        let configured = options.roots.get(index).ok_or_else(|| {
            invalid(spaceprune_plugin_api::messages::text(
                "filesystem.unknown_root",
            ))
        })?;
        let root = resolve_root(&configured.path).map_err(std::io::Error::other)?;
        if !configured.bindings.is_empty()
            && !configured.bindings.iter().any(|binding| {
                let selected = binding
                    .relative
                    .iter()
                    .fold(root.clone(), |path, part| path.join(part));
                if configured.scope == spaceprune_contracts::rule::RuleScope::ExactFile {
                    path == selected
                } else {
                    path.starts_with(selected)
                }
            })
        {
            return Err(invalid(spaceprune_plugin_api::messages::text(
                "filesystem.outside_bindings",
            )));
        }
        let matcher = RootMatcher::compile(configured).map_err(std::io::Error::other)?;
        // The original scan records whether a configured root is itself an exact file.
        // Execution must not infer a new scope from the current type of a replaced root.
        let exact = item
            .opaque
            .get("exact-root")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                invalid(spaceprune_plugin_api::messages::text(
                    "filesystem.missing_scope",
                ))
            })?;
        let (anchor, relative) = if exact {
            if path != root {
                return Err(invalid(spaceprune_plugin_api::messages::text(
                    "filesystem.exact_no_expand",
                )));
            }
            (
                root.parent()
                    .ok_or_else(|| {
                        invalid(spaceprune_plugin_api::messages::text(
                            "filesystem.exact_no_parent",
                        ))
                    })?
                    .to_path_buf(),
                PathBuf::from(root.file_name().ok_or_else(|| {
                    invalid(spaceprune_plugin_api::messages::text(
                        "filesystem.exact_no_name",
                    ))
                })?),
            )
        } else {
            let relative = path
                .strip_prefix(&root)
                .map_err(std::io::Error::other)?
                .to_path_buf();
            (root, relative)
        };
        if relative.as_os_str().is_empty()
            || !matcher.included_by_rule(&relative)
            || matcher.excluded_by_rule(&relative)
        {
            return Err(invalid(spaceprune_plugin_api::messages::text(
                "filesystem.outside_patterns",
            )));
        }
        Ok((anchor, root_identity, relative, identity, matcher))
    })();
    match prepared {
        Ok((root, root_identity, relative, identity, matcher)) => remove_selected(
            &root,
            &root_identity,
            &relative,
            &identity,
            &|path| matcher.excluded_by_rule(path),
            context,
        ),
        Err(error) => Removal {
            error: Some(error),
            ..Removal::default()
        },
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_policy_tests {
    use super::*;
    #[test]
    fn policy_refusal_does_not_become_an_administrator_retry() {
        assert_eq!(
            cleanup_failure(&std::io::Error::from_raw_os_error(1)).kind,
            spaceprune_access::IoFailureKind::PolicyBlocked
        );
        assert_eq!(
            cleanup_failure(&std::io::Error::from_raw_os_error(13)).kind,
            spaceprune_access::IoFailureKind::AccessDenied
        );
    }
}
