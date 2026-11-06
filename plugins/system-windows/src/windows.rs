use serde::Deserialize;
use serde_json::json;
use spaceprune_engine::{EngineError, PluginRegistry};
use spaceprune_plugin_api::*;
use std::sync::Arc;

const RECYCLE: &str = "builtin.system-windows/recycle-bin";
const COMPONENTS: &str = "builtin.system-windows/component-store";
#[derive(Clone, Copy)]
enum Kind {
    Recycle,
    Components,
}
impl Kind {
    fn id(self) -> &'static str {
        match self {
            Self::Recycle => RECYCLE,
            Self::Components => COMPONENTS,
        }
    }
    fn name(self) -> &'static str {
        messages::text(match self {
            Self::Recycle => "windows.recycle_bin",
            Self::Components => "windows.component_store",
        })
    }
}
struct WindowsHandler(Kind);
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Options {
    elevation: Option<String>,
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Recommend {
    enabled: bool,
}
fn issue(error: impl std::fmt::Display) -> PluginError {
    PluginError::new("windows-maintenance-failed", error.to_string())
}
fn check(context: &OperationContext) -> PluginResult<()> {
    context.check().map_err(issue)
}

/// Register only native Windows lifecycle operations.
pub fn register(registry: &mut PluginRegistry) -> Result<(), EngineError> {
    for kind in [Kind::Recycle, Kind::Components] {
        registry.register(Arc::new(WindowsHandler(kind)))?;
    }
    Ok(())
}
/// Discover fixed Windows facilities without changing files or starting servicing.
pub fn discover(context: &OperationContext) -> PluginResult<Vec<TaskRequest>> {
    check(context)?;
    let mut kinds = vec![Kind::Recycle];
    if super::native::dism_path().is_ok() {
        kinds.push(Kind::Components);
    }
    Ok(kinds
        .into_iter()
        .map(|kind| TaskRequest {
            module_id: "system-windows".into(),
            task_id: kind.id().split('/').next_back().unwrap().into(),
            name: kind.name().into(),
            handler_id: kind.id().into(),
            options: json!({}),
            recommend: json!({"enabled": false}),
            tags: vec!["windows".into()],
        })
        .collect())
}
impl Handler for WindowsHandler {
    fn descriptor(&self) -> HandlerDescriptor {
        HandlerDescriptor {
            id: self.0.id().into(),
            name: self.0.name().into(),
            description: messages::text("windows.lifecycle_description").into(),
            platforms: vec!["windows".into()],
            size_accounting: Default::default(),
            options_schema: json!({"type":"object","additionalProperties":false,"properties":{"elevation":{"enum":["auto","never","uac"]}}}),
            recommend_schema: json!({"type":"object","additionalProperties":false,"properties":{"enabled":{"type":"boolean"}}}),
        }
    }
    fn validate(&self, task: &TaskRequest) -> PluginResult<()> {
        if task.handler_id != self.0.id() {
            return Err(issue("Handler identity changed"));
        }
        let options: Options = serde_json::from_value(task.options.clone()).map_err(issue)?;
        let _: Recommend = serde_json::from_value(task.recommend.clone()).map_err(issue)?;
        if options
            .elevation
            .as_deref()
            .is_some_and(|value| !matches!(value, "auto" | "never" | "uac"))
        {
            return Err(issue("Invalid authorization method"));
        }
        Ok(())
    }
    fn scan(
        &self,
        task: &TaskRequest,
        _: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult> {
        self.validate(task)?;
        check(context)?;
        let recommend: Recommend = serde_json::from_value(task.recommend.clone()).map_err(issue)?;
        let recommendation = if recommend.enabled {
            Recommendation::Recommended
        } else {
            Recommendation::Optional
        };
        let items = match self.0 {
            Kind::Recycle => super::native::recycle_items(context)
                .map_err(issue)?
                .into_iter()
                .map(|item| CleanupItem {
                    id: item.identity.clone(),
                    parent_id: None,
                    name: item.name.clone(),
                    location: item.location.clone(),
                    logical_bytes: item.bytes,
                    reclaimable_bytes: item.bytes,
                    logical_size_applicability: Default::default(),
                    modified_at: None,
                    recommendation,
                    reason: messages::text("windows.recycle_reason").into(),
                    tags: vec!["recycle-bin".into()],
                    opaque: serde_json::to_value(item).expect("shell record"),
                })
                .collect(),
            Kind::Components => {
                let path = super::native::dism_path().map_err(issue)?;
                vec![CleanupItem {
                    id: "online-component-cleanup".into(),
                    parent_id: None,
                    name: self.0.name().into(),
                    location: "Windows servicing".into(),
                    logical_bytes: None,
                    reclaimable_bytes: None,
                    logical_size_applicability: SizeApplicability::NotApplicable,
                    modified_at: None,
                    recommendation,
                    reason: messages::text("windows.component_reason").into(),
                    tags: vec!["windows-servicing".into()],
                    opaque: json!({"system_dism":path}),
                }]
            }
        };
        check(context)?;
        Ok(HandlerScanResult {
            items,
            ..Default::default()
        })
    }
    fn cleanup_authorization_method(&self, task: &TaskRequest) -> PluginResult<Option<String>> {
        self.validate(task)?;
        if matches!(self.0, Kind::Components) && !spaceprune_windows_elevation::is_elevated() {
            let options: Options = serde_json::from_value(task.options.clone()).map_err(issue)?;
            if options.elevation.as_deref() == Some("never") {
                return Err(issue("Windows servicing requires system authorization"));
            }
            Ok(Some("uac".into()))
        } else {
            Ok(None)
        }
    }
    fn clean(
        &self,
        task: &TaskRequest,
        items: &[CleanupItem],
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerCleanResult> {
        self.validate(task)?;
        let mut result = HandlerCleanResult {
            unknown_reclaimed_items: Some(0),
            ..Default::default()
        };
        let mut seen = std::collections::BTreeSet::new();
        for item in items {
            if !seen.insert(&item.id) {
                return Err(issue("Duplicate maintenance selection"));
            }
            match self.0 {
                Kind::Recycle => {
                    let record: super::native::RecycleItem =
                        serde_json::from_value(item.opaque.clone()).map_err(issue)?;
                    if record.identity != item.id {
                        return Err(issue("Recycle item identity changed"));
                    }
                }
                Kind::Components => {
                    if item.id != "online-component-cleanup" || items.len() != 1 {
                        return Err(issue("Invalid servicing selection"));
                    }
                    if item.opaque
                        != json!({"system_dism":super::native::dism_path().map_err(issue)?})
                    {
                        return Err(issue("Windows installation changed"));
                    }
                }
            }
        }
        for item in items {
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
            let outcome = match self.0 {
                Kind::Recycle => {
                    let record: super::native::RecycleItem =
                        serde_json::from_value(item.opaque.clone()).expect("prevalidated record");
                    super::native::delete_recycle_item(&record, context)
                }
                Kind::Components => super::native::cleanup_components(context),
            };
            let mut execution = ItemExecution {
                item_id: item.id.clone(),
                status: ItemExecutionStatus::Succeeded,
                reclaimed_estimate: None,
                reclaimed_lower_bound: 0,
                failure: None,
            };
            if let Err(error) = outcome {
                execution.status = if error.started {
                    ItemExecutionStatus::Uncertain
                } else {
                    ItemExecutionStatus::Failed
                };
                execution.failure = Some(ItemFailure {
                    item_id: item.id.clone(),
                    message: error.message,
                    io_failure: None,
                    authorization: None,
                    file_usage: None,
                });
            }
            result.record_outcome(execution.clone());
            if progress.cleanup_item_finished(task, &execution).is_err() {
                context.cancel();
            }
        }
        Ok(result)
    }
}
