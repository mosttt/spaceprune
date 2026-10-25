//! Shared default dispatch for handlers whose system operation requires native authorization.

use super::*;

/// Group confirmed selections by native authorization method and preserve one result per group.
/// Handlers that can attempt ordinary access first may reuse this default for other capabilities.
pub fn dispatch_authorized_cleanup<H: Handler + ?Sized>(
    handler: &H,
    groups: &[CleanupBatchGroup<'_>],
    progress: &dyn ProgressSink,
    context: &OperationContext,
    access: Option<&dyn CleanupAccess>,
) -> Vec<PluginResult<HandlerCleanResult>> {
    let Some(access) = access else {
        return handler.clean_batch(groups, progress, context);
    };
    let mut batches = std::collections::BTreeMap::<Option<String>, Vec<usize>>::new();
    let mut results = vec![None; groups.len()];
    for (index, group) in groups.iter().enumerate() {
        match handler.cleanup_authorization_method(group.task) {
            Ok(method) => batches.entry(method).or_default().push(index),
            Err(error) => results[index] = Some(Err(error)),
        }
    }
    for (method, indices) in batches {
        let selected: Vec<_> = indices
            .iter()
            .map(|&index| CleanupBatchGroup {
                task: groups[index].task,
                items: groups[index].items,
            })
            .collect();
        let batch = if let Some(method) = method {
            access.clean_authorized(&method, &selected, progress, context)
        } else {
            Ok(handler.clean_batch(&selected, progress, context))
        };
        let batch = match batch {
            Ok(batch) if batch.len() == indices.len() => batch,
            Ok(_) => vec![
                Err(PluginError::new(
                    "cleanup-result-invalid",
                    "Invalid authorized cleanup result count"
                ));
                indices.len()
            ],
            Err(error) => vec![Err(error); indices.len()],
        };
        for (index, result) in indices.into_iter().zip(batch) {
            results[index] = Some(result);
        }
    }
    results
        .into_iter()
        .map(|value| value.expect("each cleanup group is assigned once"))
        .collect()
}
