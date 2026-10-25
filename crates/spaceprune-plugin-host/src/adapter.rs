//! Adapt external process responses to the engine's Handler contract.

use crate::{HostError, PluginHost, process::ProcessPlugin};
use serde::Deserialize;
use serde_json::{Value, json};
use spaceprune_contracts::plugin::is_builtin_plugin_id;
use spaceprune_engine::PluginRegistry;
use spaceprune_plugin_api::{
    CleanupItem, Handler, HandlerCleanResult, HandlerDescriptor, HandlerScanResult,
    OperationContext, PluginError, PluginResult, ProgressSink, TaskRequest,
};
use std::{collections::BTreeSet, sync::Arc};

impl PluginHost {
    /// Start discovered external plugins and register their handlers; reserved IDs remain metadata only.
    pub fn register_all(&self, registry: &mut PluginRegistry) -> Result<(), HostError> {
        self.register_matching(registry, |_| true, &OperationContext::default())
    }

    /// 只启动并注册指定插件 ID，避免无关插件影响当前方案。
    pub fn register_required(
        &self,
        registry: &mut PluginRegistry,
        required: &BTreeSet<String>,
    ) -> Result<(), HostError> {
        self.register_required_in_context(registry, required, &OperationContext::default())
    }

    /// Start required plugin processes and describe them under the original caller's budget.
    pub fn register_required_in_context(
        &self,
        registry: &mut PluginRegistry,
        required: &BTreeSet<String>,
        context: &OperationContext,
    ) -> Result<(), HostError> {
        self.register_matching(registry, |id| required.contains(id), context)
    }

    fn register_matching(
        &self,
        registry: &mut PluginRegistry,
        include: impl Fn(&str) -> bool,
        context: &OperationContext,
    ) -> Result<(), HostError> {
        for plugin in self.plugins.values().filter(|plugin| {
            include(&plugin.manifest.id) && !is_builtin_plugin_id(&plugin.manifest.id)
        }) {
            context.check().map_err(|source| HostError::Io {
                path: plugin.executable.clone(),
                source,
            })?;
            let process = Arc::new(ProcessPlugin::start(plugin.clone())?);
            let descriptors: DescribeResult =
                process.call("describe", json!({}), None, Some(context))?;
            let expected: std::collections::BTreeSet<_> = plugin
                .manifest
                .handlers
                .iter()
                .map(|id| format!("{}/{}", plugin.manifest.id, id))
                .collect();
            let actual: std::collections::BTreeSet<_> = descriptors
                .handlers
                .iter()
                .map(|descriptor| descriptor.id.clone())
                .collect();
            if expected != actual {
                return Err(HostError::Protocol {
                    plugin: plugin.manifest.id.clone(),
                    message: spaceprune_contracts::messages::text("host.descriptor_mismatch")
                        .to_owned(),
                });
            }
            for descriptor in descriptors.handlers {
                registry.register(Arc::new(ProcessHandler {
                    process: Arc::clone(&process),
                    descriptor,
                }))?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct DescribeResult {
    handlers: Vec<HandlerDescriptor>,
}

#[derive(Debug)]
struct ProcessHandler {
    process: Arc<ProcessPlugin>,
    descriptor: HandlerDescriptor,
}

impl Handler for ProcessHandler {
    fn descriptor(&self) -> HandlerDescriptor {
        self.descriptor.clone()
    }

    fn validate(&self, task: &TaskRequest) -> PluginResult<()> {
        self.validate_in_context(task, &OperationContext::default())
    }

    fn validate_in_context(
        &self,
        task: &TaskRequest,
        context: &OperationContext,
    ) -> PluginResult<()> {
        let _: Value = self
            .process
            .call("validate", json!({"task": task}), None, Some(context))
            .map_err(host_to_plugin)?;
        Ok(())
    }

    fn scan(
        &self,
        task: &TaskRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult> {
        self.process
            .call(
                "scan",
                json!({"task": task, "context": {"operation": context.identity(), "interaction": context.interaction, "remaining_ms": context.deadline.saturating_duration_since(std::time::Instant::now()).as_millis()}}),
                Some(progress),
                Some(context),
            )
            .map_err(host_to_plugin)
    }

    fn clean(
        &self,
        task: &TaskRequest,
        items: &[CleanupItem],
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerCleanResult> {
        self.process
            .clean(task, items, progress, context)
            .map_err(host_to_plugin)
    }
}

fn host_to_plugin(error: HostError) -> PluginError {
    PluginError::new("external-plugin-failed", error.to_string())
}
