//! Exact identity-bound deletion with fresh grant and metadata checks; uncertainty stops dispatch.

use super::*;

pub(super) fn clean(
    provider: &dyn DocumentProvider,
    task: &TaskRequest,
    items: &[CleanupItem],
    progress: &dyn ProgressSink,
    context: &OperationContext,
) -> PluginResult<HandlerCleanResult> {
    let rules = rules::CompiledRules::parse(task)?;
    let mut result = HandlerCleanResult {
        unknown_reclaimed_items: Some(0),
        ..Default::default()
    };
    for (index, item) in items.iter().enumerate() {
        let outcome = if context.is_stopped() {
            failure(
                item,
                ItemExecutionStatus::NotAttempted,
                spaceprune_plugin_api::messages::text("mobile_documents.stopped").into(),
            )
        } else {
            match prepare(provider, &rules, item, context) {
                Ok(None) => success(item, ItemExecutionStatus::AlreadyAbsent, Some(0)),
                Ok(Some(selection)) => {
                    if context.is_stopped() {
                        failure(
                            item,
                            ItemExecutionStatus::NotAttempted,
                            spaceprune_plugin_api::messages::text("mobile_documents.stopped")
                                .into(),
                        )
                    } else {
                        match provider.delete_file(
                            &selection.binding,
                            &selection.document,
                            &selection.ancestors,
                            context,
                        ) {
                            DeleteResult::Deleted => success(
                                item,
                                ItemExecutionStatus::Succeeded,
                                selection.document.logical_bytes,
                            ),
                            DeleteResult::AlreadyAbsent => {
                                success(item, ItemExecutionStatus::AlreadyAbsent, Some(0))
                            }
                            DeleteResult::Failed { message } => {
                                failure(item, ItemExecutionStatus::Failed, message)
                            }
                            DeleteResult::Uncertain { message } => {
                                context.cancel();
                                failure(item, ItemExecutionStatus::Uncertain, message)
                            }
                            DeleteResult::NotAttempted { message } => {
                                context.cancel();
                                failure(item, ItemExecutionStatus::NotAttempted, message)
                            }
                        }
                    }
                }
                Err(error) => failure(
                    item,
                    if context.is_stopped() {
                        ItemExecutionStatus::NotAttempted
                    } else {
                        ItemExecutionStatus::Failed
                    },
                    error.to_string(),
                ),
            }
        };
        if progress.cleanup_item_finished(task, &outcome).is_err() {
            context.cancel();
        }
        result.record_outcome(outcome);
        progress.record_cleanup_with_accounting(
            task,
            CleanupSizeAccounting::ProcessedBytes,
            result.succeeded,
            result.reclaimed_bytes,
            result.unknown_reclaimed_items(),
        );
        emit(
            progress,
            task,
            ProgressPhase::Clean,
            index + 1,
            Some(items.len()),
            spaceprune_plugin_api::messages::message(
                "mobile_documents.processed",
                &[("name", item.name.clone())],
            ),
        );
    }
    Ok(result)
}

fn prepare(
    provider: &dyn DocumentProvider,
    rules: &rules::CompiledRules,
    item: &CleanupItem,
    context: &OperationContext,
) -> PluginResult<Option<Selection>> {
    let invalid = |message| PluginError::new("mobile-documents-selection-changed", message);
    if item.recommendation == Recommendation::Unavailable {
        return Err(invalid(spaceprune_plugin_api::messages::text(
            "mobile_documents.unavailable",
        )));
    }
    let selection: Selection = serde_json::from_value(item.opaque.clone()).map_err(|_| {
        invalid(spaceprune_plugin_api::messages::text(
            "mobile_documents.selection_invalid",
        ))
    })?;
    if selection.binding.slot != rules.rules.resource
        || selection.document.kind != DocumentKind::File
        || selection.document.id == selection.binding.root_id
        || !rules.included(&selection.document.name)
        || selection.ancestors.len() < rules.rules.relative_directory.len()
        || !selection
            .ancestors
            .iter()
            .map(|entry| &entry.name)
            .zip(&rules.rules.relative_directory)
            .all(|(actual, expected)| actual == expected)
        || (!rules.rules.recursive
            && selection.ancestors.len() != rules.rules.relative_directory.len())
    {
        return Err(invalid(spaceprune_plugin_api::messages::text(
            "mobile_documents.scope_changed",
        )));
    }
    let current = binding(provider, &rules.rules.resource, context)?;
    if current != selection.binding {
        return Err(invalid(spaceprune_plugin_api::messages::text(
            "mobile_documents.binding_changed",
        )));
    }
    if !current.writable || !selection.document.deletable {
        return Err(invalid(spaceprune_plugin_api::messages::text(
            "mobile_documents.no_delete_grant",
        )));
    }
    let mut parent = current.root_id.clone();
    for ancestor in &selection.ancestors {
        check(context)?;
        if ancestor.kind != DocumentKind::Directory
            || ancestor.parent_id != parent
            || rules.excluded(&ancestor.name)
        {
            return Err(invalid(spaceprune_plugin_api::messages::text(
                "mobile_documents.ancestry_invalid",
            )));
        }
        let observed = provider
            .inspect(&current, &ancestor.id, context)?
            .ok_or_else(|| {
                invalid(spaceprune_plugin_api::messages::text(
                    "mobile_documents.parent_absent",
                ))
            })?;
        // Directory timestamps change when a previous selected child is deleted. Scope depends on
        // identity, parent and name, not those timestamps or an aggregate directory size.
        if observed.id != ancestor.id
            || observed.kind != DocumentKind::Directory
            || observed.parent_id != parent
            || observed.name != ancestor.name
        {
            return Err(invalid(spaceprune_plugin_api::messages::text(
                "mobile_documents.parent_changed",
            )));
        }
        parent = ancestor.id.clone();
    }
    if selection.document.parent_id != parent {
        return Err(invalid(spaceprune_plugin_api::messages::text(
            "mobile_documents.chain_changed",
        )));
    }
    check(context)?;
    let Some(observed) = provider.inspect(&current, &selection.document.id, context)? else {
        return Ok(None);
    };
    if observed != selection.document {
        return Err(invalid(spaceprune_plugin_api::messages::text(
            "mobile_documents.metadata_changed",
        )));
    }
    Ok(Some(selection))
}

fn success(item: &CleanupItem, status: ItemExecutionStatus, size: Option<u64>) -> ItemExecution {
    ItemExecution {
        item_id: item.id.clone(),
        status,
        reclaimed_estimate: size,
        reclaimed_lower_bound: 0,
        failure: None,
    }
}
fn failure(item: &CleanupItem, status: ItemExecutionStatus, message: String) -> ItemExecution {
    ItemExecution {
        item_id: item.id.clone(),
        status,
        reclaimed_estimate: if status == ItemExecutionStatus::Uncertain {
            None
        } else {
            Some(0)
        },
        reclaimed_lower_bound: 0,
        failure: Some(ItemFailure {
            item_id: item.id.clone(),
            authorization: None,
            io_failure: None,
            message,
        }),
    }
}
