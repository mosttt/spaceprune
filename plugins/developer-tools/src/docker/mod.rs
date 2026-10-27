//! Local Docker lifecycle operations; never raw deletion of engine storage.
mod cli;
mod inventory;
#[cfg(test)]
mod tests;
use super::*;
use cli::Runner;
use serde::{Deserialize, Serialize};
use spaceprune_plugin_api::{ItemExecution, ItemExecutionStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum DockerKind {
    BuildCache,
    DanglingImages,
    UnusedImages,
    StoppedContainers,
}
impl DockerKind {
    pub(super) const ALL: [Self; 4] = [
        Self::BuildCache,
        Self::DanglingImages,
        Self::UnusedImages,
        Self::StoppedContainers,
    ];
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::BuildCache => "docker-build-cache",
            Self::DanglingImages => "docker-dangling-images",
            Self::UnusedImages => "docker-unused-images",
            Self::StoppedContainers => "docker-stopped-containers",
        }
    }
    fn name(self) -> &'static str {
        messages::text(match self {
            Self::BuildCache => "docker.build_cache",
            Self::DanglingImages => "docker.dangling_images",
            Self::UnusedImages => "docker.unused_images",
            Self::StoppedContainers => "docker.stopped_containers",
        })
    }
    fn consequence(self) -> &'static str {
        messages::text(match self {
            Self::BuildCache => "docker.build_consequence",
            Self::DanglingImages => "docker.image_consequence",
            Self::UnusedImages => "docker.unused_consequence",
            Self::StoppedContainers => "docker.container_consequence",
        })
    }
    fn handler(self) -> String {
        format!("{PLUGIN_ID}/{}", self.id())
    }
    fn task(self) -> TaskRequest {
        TaskRequest {
            module_id: "developer-tools".into(),
            task_id: self.id().into(),
            name: self.name().into(),
            handler_id: self.handler(),
            options: json!({}),
            recommend: json!({"enabled":false}),
            tags: vec!["docker-lifecycle".into()],
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    endpoint: String,
    engine: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    binding: Binding,
    kind: DockerKind,
    id: String,
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DockerOptions {}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DockerRecommend {
    enabled: bool,
}

pub(super) fn register(registry: &mut PluginRegistry) -> Result<(), EngineError> {
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    for kind in DockerKind::ALL {
        registry.register(Arc::new(DockerHandler(kind)))?;
    }
    Ok(())
}
pub(super) fn probe_tasks(context: &OperationContext) -> Vec<CapabilityProbe> {
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    {
        if !super::discovery::command_exists("docker") {
            return Vec::new();
        }
        let binding = cli::probe(&cli::Native, context);
        return DockerKind::ALL
            .into_iter()
            .map(|kind| {
                let issue = binding
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|binding| cli::interface(kind, binding, &cli::Native, context))
                    .err()
                    .map(|error| HandlerIssue {
                        code: Some(error.code),
                        io_failure: None,
                        level: IssueLevel::Warning,
                        message: error.message,
                        location: Some("Docker".into()),
                    });
                CapabilityProbe {
                    task: kind.task(),
                    issue,
                }
            })
            .collect();
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    Vec::new()
}

fn invalid() -> PluginError {
    PluginError::new(
        "docker-invalid-selection",
        messages::text("docker.invalid_selection"),
    )
}
#[derive(Debug)]
struct DockerHandler(DockerKind);
impl Handler for DockerHandler {
    fn descriptor(&self) -> HandlerDescriptor {
        HandlerDescriptor {
            id: self.0.handler(),
            name: self.0.name().into(),
            description: self.0.consequence().into(),
            platforms: vec!["linux".into(), "windows".into(), "macos".into()],
            size_accounting: Default::default(),
            options_schema: json!({"type":"object","additionalProperties":false}),
            recommend_schema: json!({"type":"object","additionalProperties":false,"properties":{"enabled":{"type":"boolean","default":false}}}),
        }
    }
    fn validate(&self, task: &TaskRequest) -> PluginResult<()> {
        if !cfg!(any(
            target_os = "linux",
            target_os = "windows",
            target_os = "macos"
        )) {
            return Err(PluginError::new(
                "unsupported-platform",
                messages::text("docker.desktop_only"),
            ));
        }
        if task.handler_id != self.0.handler() {
            return Err(invalid());
        }
        serde_json::from_value::<DockerOptions>(task.options.clone()).map_err(|_| invalid())?;
        serde_json::from_value::<DockerRecommend>(task.recommend.clone()).map_err(|_| invalid())?;
        Ok(())
    }
    fn scan(
        &self,
        task: &TaskRequest,
        _: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult> {
        self.scan_with(task, context, &cli::Native)
    }
    fn clean(
        &self,
        task: &TaskRequest,
        items: &[CleanupItem],
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerCleanResult> {
        self.clean_with(task, items, progress, context, &cli::Native)
    }
}
impl DockerHandler {
    fn scan_with(
        &self,
        task: &TaskRequest,
        context: &OperationContext,
        runner: &dyn Runner,
    ) -> PluginResult<HandlerScanResult> {
        self.validate(task)?;
        let recommend: DockerRecommend =
            serde_json::from_value(task.recommend.clone()).map_err(|_| invalid())?;
        let scanned = cli::probe(runner, context).and_then(|binding| {
            cli::interface(self.0, &binding, runner, context)?;
            inventory::list(self.0, &binding, runner, context).map(|objects| (binding, objects))
        });
        match scanned {
            Ok((binding, objects)) => Ok(HandlerScanResult {
                items: objects
                    .into_iter()
                    .map(|object| CleanupItem {
                        id: object.id.clone(),
                        parent_id: None,
                        name: object.name,
                        location: format!("{} / {}", binding.endpoint, object.id),
                        logical_size_applicability: Default::default(),
                        logical_bytes: object.bytes,
                        // Shared layers and Desktop VM allocation prevent independent physical estimates.
                        reclaimable_bytes: None,
                        modified_at: None,
                        recommendation: if recommend.enabled
                            && matches!(self.0, DockerKind::BuildCache | DockerKind::DanglingImages)
                        {
                            Recommendation::Recommended
                        } else {
                            Recommendation::Optional
                        },
                        reason: self.0.consequence().into(),
                        tags: vec!["docker-lifecycle".into(), self.0.id().into()],
                        opaque: json!(Selection {
                            binding: binding.clone(),
                            kind: self.0,
                            id: object.id
                        }),
                    })
                    .collect(),
                issues: Vec::new(),
            }),
            Err(error) => {
                context
                    .check()
                    .map_err(|e| PluginError::new("operation-stopped", e.to_string()))?;
                Ok(HandlerScanResult {
                    items: vec![CleanupItem {
                        id: self.0.id().into(),
                        parent_id: None,
                        name: self.0.name().into(),
                        location: "Docker".into(),
                        logical_size_applicability: Default::default(),
                        logical_bytes: None,
                        reclaimable_bytes: None,
                        modified_at: None,
                        recommendation: Recommendation::Unavailable,
                        reason: error.message.clone(),
                        tags: vec!["docker-lifecycle".into()],
                        opaque: json!(null),
                    }],
                    issues: vec![HandlerIssue {
                        code: Some(error.code),
                        io_failure: None,
                        level: IssueLevel::Warning,
                        message: error.message,
                        location: Some("Docker".into()),
                    }],
                })
            }
        }
    }
    fn clean_with(
        &self,
        task: &TaskRequest,
        items: &[CleanupItem],
        progress: &dyn ProgressSink,
        context: &OperationContext,
        runner: &dyn Runner,
    ) -> PluginResult<HandlerCleanResult> {
        self.validate(task)?;
        let mut result = HandlerCleanResult {
            unknown_reclaimed_items: Some(0),
            ..Default::default()
        };
        for item in items {
            let mut outcome = ItemExecution {
                item_id: item.id.clone(),
                status: ItemExecutionStatus::NotAttempted,
                reclaimed_estimate: Some(0),
                reclaimed_lower_bound: 0,
                failure: None,
            };
            if !context.is_stopped() {
                let mut started = false;
                let execution = (|| {
                    let selected: Selection =
                        serde_json::from_value(item.opaque.clone()).map_err(|_| invalid())?;
                    if selected.kind != self.0
                        || selected.id != item.id
                        || item.recommendation == Recommendation::Unavailable
                        || !inventory::valid_id(self.0, &selected.id)
                    {
                        return Err(invalid());
                    }
                    // Re-resolve the caller's context as well: changing the selected engine requires a rescan.
                    if cli::probe(runner, context)? != selected.binding {
                        return Err(PluginError::new(
                            "docker-engine-changed",
                            messages::text("docker.engine_changed"),
                        ));
                    }
                    cli::interface(self.0, &selected.binding, runner, context)?;
                    let current = inventory::list(self.0, &selected.binding, runner, context)?;
                    if !current.iter().any(|object| object.id == selected.id) {
                        // Eligibility can disappear because Docker already removed a record,
                        // including during an earlier native prune in this confirmed batch.
                        if !inventory::exists(
                            self.0,
                            &selected.binding,
                            &selected.id,
                            runner,
                            context,
                        )? {
                            return Ok(ItemExecutionStatus::AlreadyAbsent);
                        }
                        return Err(PluginError::new(
                            "docker-object-changed",
                            messages::text("docker.object_changed"),
                        ));
                    }
                    context.check().map_err(|_| invalid())?;
                    let args = inventory::remove_args(self.0, &selected.id);
                    started = true;
                    runner.run(
                        Some(&selected.binding.endpoint),
                        &args.iter().map(String::as_str).collect::<Vec<_>>(),
                        context,
                    )?;
                    // Prune may skip a newly busy record. Never report a skipped operation as removal.
                    if inventory::exists(self.0, &selected.binding, &selected.id, runner, context)?
                    {
                        return Err(PluginError::new(
                            "docker-object-not-removed",
                            messages::text("docker.object_changed"),
                        ));
                    }
                    Ok(ItemExecutionStatus::Succeeded)
                })();
                match execution {
                    Ok(status) => {
                        outcome.status = status;
                        outcome.reclaimed_estimate = if status == ItemExecutionStatus::AlreadyAbsent
                        {
                            Some(0)
                        } else {
                            None
                        };
                    }
                    Err(error) => {
                        outcome.status = if started {
                            ItemExecutionStatus::Uncertain
                        } else {
                            ItemExecutionStatus::Failed
                        };
                        if started {
                            outcome.reclaimed_estimate = None;
                        }
                        outcome.failure = Some(ItemFailure {
                            authorization: None,
                            io_failure: None,
                            item_id: item.id.clone(),
                            message: error.message,
                        });
                    }
                }
            }
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
        Ok(result)
    }
}
