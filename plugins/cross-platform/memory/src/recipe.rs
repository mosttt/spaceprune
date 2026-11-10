//! Reusable executable rules resolve fresh process identities on every scan.
use super::*;
use serde_json::json;
use spaceprune_engine::{EngineError, PluginRegistry};
use spaceprune_plugin_api::*;
use std::{collections::BTreeSet, path::Path, sync::Arc};

/// Host authorization for an exact, executable-bound memory selection.
pub type ReleaseAccess =
    fn(&[BoundProcess], &dyn ProgressSink, &OperationContext) -> PluginResult<Vec<MemoryRelease>>;

/// Register process memory rules with the host's native authorization boundary.
pub fn register(registry: &mut PluginRegistry, release: ReleaseAccess) -> Result<(), EngineError> {
    registry.register(Arc::new(MemoryHandler { release }))
}

struct MemoryHandler {
    release: ReleaseAccess,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Options {
    applications: Vec<String>,
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Recommend {
    enabled: bool,
}
fn error(message: impl Into<String>) -> PluginError {
    PluginError::new("memory-rule-invalid", message)
}
fn options(task: &TaskRequest) -> PluginResult<(Options, Recommend)> {
    let options: Options =
        serde_json::from_value(task.options.clone()).map_err(|e| error(e.to_string()))?;
    let recommend: Recommend =
        serde_json::from_value(task.recommend.clone()).map_err(|e| error(e.to_string()))?;
    if options.applications.is_empty()
        || options.applications.len() > 128
        || options
            .applications
            .iter()
            .any(|path| !Path::new(path).is_absolute() || path.contains('\0'))
        || options.applications.iter().collect::<BTreeSet<_>>().len() != options.applications.len()
    {
        return Err(error(
            "Select distinct absolute executable paths, not names or saved PIDs",
        ));
    }
    Ok((options, recommend))
}
fn id(identity: &ProcessIdentity) -> String {
    format!("{}:{}", identity.pid, identity.started_at)
}
fn scan_items(task: &TaskRequest, snapshot: MemorySnapshot) -> PluginResult<HandlerScanResult> {
    let (options, recommend) = options(task)?;
    let items = snapshot
        .processes
        .into_iter()
        .filter_map(|process| {
            let executable = process.executable?;
            if !options.applications.contains(&executable) {
                return None;
            }
            let selected = BoundProcess {
                identity: process.identity,
                executable: executable.clone(),
            };
            Some(CleanupItem {
                id: id(&selected.identity),
                parent_id: None,
                name: process.name,
                location: executable,
                logical_size_applicability: SizeApplicability::Applicable,
                logical_bytes: Some(process.resident_bytes),
                reclaimable_bytes: None,
                modified_at: None,
                recommendation: if recommend.enabled {
                    Recommendation::Recommended
                } else {
                    Recommendation::Optional
                },
                reason: messages::text("memory.recipe_transient").into(),
                tags: vec!["memory".into()],
                opaque: serde_json::to_value(selected).expect("memory selection is serializable"),
            })
        })
        .collect();
    Ok(HandlerScanResult {
        items,
        issues: Vec::new(),
    })
}
impl Handler for MemoryHandler {
    fn descriptor(&self) -> HandlerDescriptor {
        HandlerDescriptor {
            id: "builtin.memory/process-working-set".into(),
            name: messages::text("plugins.memory_name").into(),
            description: messages::text("memory.recipe_transient").into(),
            platforms: vec!["windows".into(), "linux".into()],
            size_accounting: CleanupSizeAccounting::MemoryBytes,
            options_schema: json!({"type":"object","additionalProperties":false,"required":["applications"],"properties":{"applications":{"type":"array","minItems":1,"maxItems":128,"uniqueItems":true,"items":{"type":"string","minLength":1},"description":"Exact executable paths. Resolve fresh current-user processes on every scan; never store PIDs as reusable targets."}}}),
            recommend_schema: json!({"type":"object","additionalProperties":false,"properties":{"enabled":{"type":"boolean","default":false}}}),
        }
    }
    fn validate(&self, task: &TaskRequest) -> PluginResult<()> {
        options(task).map(|_| ())
    }
    fn scan(
        &self,
        task: &TaskRequest,
        _progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult> {
        context.check().map_err(|e| error(e.to_string()))?;
        self.validate(task)?;
        let snapshot =
            inspect().map_err(|e| PluginError::new("memory-inspection-failed", e.to_string()))?;
        context.check().map_err(|e| error(e.to_string()))?;
        scan_items(task, snapshot)
    }
    fn clean(
        &self,
        task: &TaskRequest,
        items: &[CleanupItem],
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerCleanResult> {
        let (options, _) = options(task)?;
        if items.is_empty() {
            return Ok(HandlerCleanResult::default());
        }
        let selected = items
            .iter()
            .map(|item| {
                let bound: BoundProcess = serde_json::from_value(item.opaque.clone())
                    .map_err(|e| error(e.to_string()))?;
                if item.id != id(&bound.identity)
                    || item.location != bound.executable
                    || !options.applications.contains(&bound.executable)
                    || item.recommendation == Recommendation::Unavailable
                {
                    return Err(error("Memory selection does not match its executable rule"));
                }
                Ok(bound)
            })
            .collect::<PluginResult<Vec<_>>>()?;
        validate_bound(&selected).map_err(|e| error(e.to_string()))?;
        context.check().map_err(|e| error(e.to_string()))?;
        let results = (self.release)(&selected, progress, context)?;
        if results.len() != selected.len()
            || !results
                .iter()
                .zip(&selected)
                .all(|(r, s)| r.identity == s.identity)
        {
            return Err(PluginError::new(
                "memory-result-unconfirmed",
                "Memory receipt does not match the confirmed processes",
            ));
        }
        let mut report = HandlerCleanResult {
            unknown_reclaimed_items: Some(0),
            ..Default::default()
        };
        for (item, result) in items.iter().zip(results) {
            let status = if result.already_absent {
                ItemExecutionStatus::AlreadyAbsent
            } else if result.succeeded {
                ItemExecutionStatus::Succeeded
            } else if result.applied {
                ItemExecutionStatus::PartiallyApplied
            } else {
                ItemExecutionStatus::Failed
            };
            let failure = result.error.map(|message| ItemFailure {
                item_id: item.id.clone(),
                message,
                file_usage: None,
                authorization: None,
                io_failure: None,
            });
            let outcome = ItemExecution {
                item_id: item.id.clone(),
                status,
                reclaimed_estimate: result.resident_reduction_bytes,
                reclaimed_lower_bound: 0,
                failure,
            };
            report.record_outcome(outcome.clone());
            // Keep confirmed effects even when the progress journal rejects further writes.
            if let Err(error) = progress.cleanup_item_finished(task, &outcome) {
                let _ = error;
                context.cancel();
            }
            progress.record_cleanup_with_accounting(
                task,
                CleanupSizeAccounting::MemoryBytes,
                report
                    .outcomes
                    .iter()
                    .filter(|outcome| outcome.status == ItemExecutionStatus::Succeeded)
                    .count(),
                report.reclaimed_bytes,
                report.unknown_reclaimed_items(),
            );
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests;
