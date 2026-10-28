#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! SpacePrune 的平台中立执行器。
//!
//! 本 crate 负责把配置化模块和任务转换为扫描报告、选择集合、清理计划和执行报告。
//! 它不包含终端交互、提权或具体领域清理代码。

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Component, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use spaceprune_config::{ModuleDefinition, ResolvedConfig, TaskDefinition, current_platform};
pub use spaceprune_plugin_api::{
    CancellationToken, CleanupBatchGroup, CleanupItem, CleanupSizeAccounting, Handler,
    HandlerCleanResult, HandlerDescriptor, HandlerIssue, IssueLevel, ItemFailure,
    LogicalSizeSummary, NullProgress, OperationContext, OperationStopReason, PluginError,
    ProgressEvent, ProgressPhase, ProgressSink, Recommendation, ScanAccess, SizeApplicability,
    TaskRequest,
};
use thiserror::Error;

/// 扫描与清理生命周期事件。
pub mod events;
mod execute;
mod journal;
pub use journal::default_directory as default_execution_journal_directory;
pub use journal::recover_execution;
/// 跨插件复用的文件系统基础函数。
pub mod fs;
pub mod measurement;
mod plan;
mod scan;
pub use plan::{CleanupPlan, ScanReservation};
pub use spaceprune_contracts::report::{
    CleanupPlan as CleanupPlanData, ExecutionJournalReference, ExecutionReport,
    GroupExecutionReport, ItemKey, ModuleReport, PlanGroup, RecoveredExecution,
    ScanReport as ScanReportData, ScanRequest, TaskReport,
};
pub use spaceprune_contracts::size_summary::CleanupSizeSummary;

pub use events::{
    EventBus, EventEnvelope, EventListener, EventListenerId, OperationId, SpacePruneEvent,
};

/// SpacePrune 核心错误。
#[derive(Debug, Error)]
pub enum EngineError {
    /// 请求了不存在或未启用的模块。
    #[error("{}", spaceprune_contracts::messages::message("engine.unknown_module", &[("id", .0.to_string())]))]
    UnknownModule(String),
    /// 请求了当前模块不存在的选择器。
    #[error("{}", spaceprune_contracts::messages::message("engine.unknown_selector", &[("module", .module.to_string()), ("selector", .selector.to_string())]))]
    UnknownSelector {
        /// 模块 ID。
        module: String,
        /// 选择器 ID。
        selector: String,
    },
    /// 请求了当前模块不存在或不可用的任务。
    #[error("{}", spaceprune_contracts::messages::message("engine.unknown_task", &[("module", .module.to_string()), ("task", .task.to_string())]))]
    UnknownTask {
        /// 模块 ID。
        module: String,
        /// 任务 ID。
        task: String,
    },
    /// A selected task belongs to a locally disabled plugin.
    #[error("{}", spaceprune_contracts::messages::message("engine.plugin_disabled", &[("id", .0.to_string())]))]
    PluginDisabled(String),
    /// Device-local enablement changed after this session was loaded.
    #[error(
        "{}",
        spaceprune_contracts::messages::text("engine.plugin_policy_changed")
    )]
    PluginPolicyChanged,
    /// The current preference could not be read reliably.
    #[error("plugin-policy-unavailable: {0}")]
    PluginPolicyUnavailable(String),
    /// 配置引用了尚未注册的 Handler。
    #[error("{}", spaceprune_contracts::messages::message("engine.missing_handler", &[("configuration", .configuration.to_string()), ("module", .module.to_string()), ("handler", .handler.to_string())]))]
    MissingHandler {
        /// Recipe entry file.
        configuration: String,
        /// Module display name.
        module: String,
        /// Unavailable handler identifier.
        handler: String,
    },
    /// 注册了重复的 Handler ID。
    #[error("{}", spaceprune_contracts::messages::message("engine.duplicate_handler", &[("id", .0.to_string())]))]
    DuplicateHandler(String),
    /// Handler 描述 ID 与注册 ID 不一致。
    #[error("{}", spaceprune_contracts::messages::message("engine.handler_id_mismatch", &[("actual", .actual.to_string()), ("expected", .expected.to_string())]))]
    HandlerIdMismatch {
        /// 期望 ID。
        expected: String,
        /// Handler 返回的 ID。
        actual: String,
    },
    /// 任务配置无法转换为插件请求。
    #[error("{}", spaceprune_contracts::messages::message("engine.task_serialization", &[("module", .module.to_string()), ("task", .task.to_string()), ("message", .message.to_string())]))]
    TaskSerialization {
        /// 模块 ID。
        module: String,
        /// 任务 ID。
        task: String,
        /// 序列化错误。
        message: String,
    },
    /// 清理计划引用了不可用或不存在的项目。
    #[error("{}", spaceprune_contracts::messages::message("engine.invalid_selection", &[("id", .0.to_string())]))]
    InvalidSelection(String),
    /// 尝试从被取消的不完整扫描生成清理计划。
    #[error("{}", spaceprune_contracts::messages::text("engine.incomplete_scan"))]
    IncompleteScan,
    /// Handler 调用失败。
    #[error("{}", spaceprune_contracts::messages::message("engine.handler_failed", &[("handler", .handler.to_string()), ("source", .source.to_string())]))]
    Handler {
        /// Handler ID。
        handler: String,
        /// 插件错误。
        source: PluginError,
    },
}

/// 已注册 Handler 的集合。
#[derive(Default, Clone)]
pub struct PluginRegistry {
    handlers: BTreeMap<String, Arc<dyn Handler>>,
}

impl std::fmt::Debug for PluginRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginRegistry")
            .field("handlers", &self.handlers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl PluginRegistry {
    /// 创建空注册表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个静态或外部 Handler。
    pub fn register(&mut self, handler: Arc<dyn Handler>) -> Result<(), EngineError> {
        let descriptor = handler.descriptor();
        if self.handlers.contains_key(&descriptor.id) {
            return Err(EngineError::DuplicateHandler(descriptor.id));
        }
        self.handlers.insert(descriptor.id, handler);
        Ok(())
    }

    /// 使用期望 ID 注册 Handler，并检查描述是否一致。
    pub fn register_as(
        &mut self,
        expected_id: impl Into<String>,
        handler: Arc<dyn Handler>,
    ) -> Result<(), EngineError> {
        let expected = expected_id.into();
        let actual = handler.descriptor().id;
        if expected != actual {
            return Err(EngineError::HandlerIdMismatch { expected, actual });
        }
        self.register(handler)
    }

    /// 查找一个 Handler。
    pub fn get(&self, id: &str) -> Option<Arc<dyn Handler>> {
        self.handlers.get(id).cloned()
    }

    /// 返回所有 Handler 描述。
    pub fn descriptors(&self) -> Vec<HandlerDescriptor> {
        self.handlers
            .values()
            .map(|handler| handler.descriptor())
            .collect()
    }

    /// 判断 Handler 是否已经注册。
    pub fn contains(&self, id: &str) -> bool {
        self.handlers.contains_key(id)
    }
}

/// A host-owned scan plus the process-local one-use marker shared by its plans.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScanReport {
    #[serde(flatten)]
    report: ScanReportData,
    #[serde(skip)]
    pub(crate) reservation: ScanReservation,
}

impl std::ops::Deref for ScanReport {
    type Target = ScanReportData;
    fn deref(&self) -> &Self::Target {
        &self.report
    }
}

impl std::ops::DerefMut for ScanReport {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.report
    }
}

impl ScanReport {
    /// Wrap host-owned observations with a new process-local consumption marker.
    /// This does not grant OS access or replace a host's confirmation boundary.
    pub fn from_report(report: ScanReportData) -> Self {
        Self {
            report,
            reservation: ScanReservation::default(),
        }
    }

    /// Consume the host wrapper and retain only transferable observations.
    pub fn into_report(self) -> ScanReportData {
        self.report
    }

    /// Create a unique identity for a new or aggregated host-owned scan.
    pub fn new_scan_id() -> String {
        plan::new_scan_id()
    }

    /// 根据推荐项生成清理计划。
    pub fn plan_recommended(&self) -> Result<CleanupPlan, EngineError> {
        self.plan_selected(&self.recommended_keys())
    }

    /// 根据最终选择生成清理计划。
    pub fn plan_selected(&self, selected: &BTreeSet<ItemKey>) -> Result<CleanupPlan, EngineError> {
        if self.cancelled {
            return Err(EngineError::IncompleteScan);
        }
        let selectable = self.selectable_keys();
        if let Some(invalid) = selected.iter().find(|key| !selectable.contains(*key)) {
            return Err(EngineError::InvalidSelection(invalid.to_string()));
        }

        let mut groups = Vec::new();
        for module in &self.modules {
            for task in &module.tasks {
                let items: Vec<_> = task
                    .leaf_items()
                    .into_iter()
                    .filter(|item| {
                        selected.contains(&ItemKey {
                            module_id: module.id.clone(),
                            task_id: task.task_id.clone(),
                            item_id: item.id.clone(),
                        })
                    })
                    .cloned()
                    .collect();
                if !items.is_empty() {
                    groups.push(PlanGroup {
                        module_id: module.id.clone(),
                        task_id: task.task_id.clone(),
                        handler_id: task.handler_id.clone(),
                        request: task.request.clone(),
                        items,
                    });
                }
            }
        }
        deduplicate_filesystem_plan_items(&mut groups);
        Ok(CleanupPlan::from_scan(self, groups))
    }
}

fn deduplicate_filesystem_plan_items(groups: &mut Vec<PlanGroup>) {
    const FILESYSTEM_HANDLER: &str = "builtin.filesystem/paths";

    let mut candidates = groups
        .iter()
        .enumerate()
        .filter(|(_, group)| group.handler_id == FILESYSTEM_HANDLER)
        .flat_map(|(group_index, group)| {
            group
                .items
                .iter()
                .enumerate()
                .filter_map(move |(item_index, item)| {
                    let path = if let Some(native) = item.opaque.get("native-path") {
                        serde_json::from_value::<spaceprune_access::NativePath>(native.clone())
                            .ok()?
                            .to_path_buf()?
                    } else {
                        PathBuf::from(item.opaque.get("path")?.as_str()?)
                    };
                    if !path.is_absolute()
                        || path.components().any(|component| {
                            matches!(component, Component::CurDir | Component::ParentDir)
                        })
                    {
                        return None;
                    }
                    Some((group_index, item_index, path))
                })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.2
            .components()
            .count()
            .cmp(&right.2.components().count())
            .then_with(|| left.0.cmp(&right.0))
            .then_with(|| left.1.cmp(&right.1))
    });

    let mut kept_paths = HashSet::<PathBuf>::new();
    let mut duplicate_indexes = BTreeSet::new();
    for (group_index, item_index, path) in candidates {
        if path
            .ancestors()
            .any(|ancestor| kept_paths.contains(ancestor))
        {
            duplicate_indexes.insert((group_index, item_index));
        } else {
            kept_paths.insert(path);
        }
    }
    for (group_index, group) in groups.iter_mut().enumerate() {
        let mut item_index = 0_usize;
        group.items.retain(|_| {
            let keep = !duplicate_indexes.contains(&(group_index, item_index));
            item_index += 1;
            keep
        });
    }
    groups.retain(|group| !group.items.is_empty());
}

/// SpacePrune 执行器构建器。
#[derive(Debug)]
pub struct EngineBuilder {
    config: ResolvedConfig,
    registry: PluginRegistry,
    disabled_plugins: BTreeSet<String>,
    event_bus: EventBus,
    journal_directory: Option<PathBuf>,
}

impl EngineBuilder {
    /// 使用已解析配置创建构建器。
    pub fn new(config: ResolvedConfig) -> Self {
        Self {
            config,
            registry: PluginRegistry::new(),
            disabled_plugins: BTreeSet::new(),
            event_bus: EventBus::new(),
            journal_directory: None,
        }
    }

    /// 设置完整 Handler 注册表。
    pub fn with_registry(mut self, registry: PluginRegistry) -> Self {
        self.registry = registry;
        self
    }

    /// Apply host-selected enablement without changing recipe declarations.
    pub fn with_disabled_plugins(mut self, disabled: BTreeSet<String>) -> Self {
        self.disabled_plugins = disabled;
        self
    }

    /// 设置执行器使用的生命周期事件总线。
    pub fn with_event_bus(mut self, event_bus: EventBus) -> Self {
        self.event_bus = event_bus;
        self
    }

    /// Persist execution evidence in a host-owned state directory, outside recipe bundles.
    pub fn with_journal_directory(mut self, directory: PathBuf) -> Self {
        self.journal_directory = Some(directory);
        self
    }

    /// 构建执行器并校验当前平台启用任务的 Handler。
    pub fn build(self) -> Result<Engine, EngineError> {
        self.build_in_context(&OperationContext::default())
    }

    /// Assemble and validate without renewing an owner's setup deadline.
    pub fn build_in_context(self, context: &OperationContext) -> Result<Engine, EngineError> {
        for (module_id, module) in self.config.enabled_modules() {
            for (task_id, task) in module.tasks.iter().filter(|(_, task)| task.enabled) {
                if task_matches_platform(task)
                    && !handler_disabled(&self.disabled_plugins, &task.handler)
                {
                    let handler = self.registry.get(&task.handler).ok_or_else(|| {
                        EngineError::MissingHandler {
                            configuration: self.config.source.clone(),
                            module: module.name.clone(),
                            handler: task.handler.clone(),
                        }
                    })?;
                    let request = build_task_request(module_id, task_id, task)?;
                    handler
                        .validate_in_context(&request, context)
                        .map_err(|source| EngineError::Handler {
                            handler: task.handler.clone(),
                            source,
                        })?;
                }
            }
        }
        Ok(Engine {
            config: self.config,
            registry: self.registry,
            disabled_plugins: self.disabled_plugins,
            event_bus: self.event_bus,
            journal_directory: self.journal_directory,
        })
    }
}

/// SpacePrune 的可复用扫描和清理执行器。
#[derive(Debug)]
pub struct Engine {
    config: ResolvedConfig,
    registry: PluginRegistry,
    disabled_plugins: BTreeSet<String>,
    event_bus: EventBus,
    journal_directory: Option<PathBuf>,
}

impl Engine {
    /// 返回当前已解析配置。
    pub fn config(&self) -> &ResolvedConfig {
        &self.config
    }

    /// Return registered handler metadata for host presentation without scanning or cleaning.
    pub fn handler_descriptors(&self) -> Vec<HandlerDescriptor> {
        self.registry.descriptors()
    }

    /// 返回执行器共享的生命周期事件总线。
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    fn resolve_modules<'a>(
        &'a self,
        request: &ScanRequest,
    ) -> Result<Vec<(&'a str, &'a ModuleDefinition)>, EngineError> {
        if let Some(module_id) = &request.module {
            let (resolved_id, module) = self
                .config
                .modules
                .get_key_value(module_id)
                .filter(|(_, module)| module.enabled)
                .ok_or_else(|| EngineError::UnknownModule(module_id.clone()))?;
            Ok(vec![(resolved_id.as_str(), module)])
        } else {
            Ok(self.config.enabled_modules())
        }
    }
}

fn selector_tags(
    module_id: &str,
    module: &ModuleDefinition,
    selectors: &BTreeSet<String>,
) -> Result<BTreeSet<String>, EngineError> {
    let mut tags = BTreeSet::new();
    for selector_id in selectors {
        let selector = module
            .selectors
            .get(selector_id)
            .filter(|selector| selector.enabled)
            .ok_or_else(|| EngineError::UnknownSelector {
                module: module_id.to_owned(),
                selector: selector_id.clone(),
            })?;
        tags.extend(selector.match_tags.iter().cloned());
    }
    Ok(tags)
}

fn validate_task_selection(
    module_id: &str,
    module: &ModuleDefinition,
    tasks: &BTreeSet<String>,
) -> Result<(), EngineError> {
    for task_id in tasks {
        let available = module
            .tasks
            .get(task_id)
            .is_some_and(|task| task.enabled && task_matches_platform(task));
        if !available {
            return Err(EngineError::UnknownTask {
                module: module_id.to_owned(),
                task: task_id.clone(),
            });
        }
    }
    Ok(())
}

fn build_task_request(
    module_id: &str,
    task_id: &str,
    task: &TaskDefinition,
) -> Result<TaskRequest, EngineError> {
    let options =
        serde_json::to_value(&task.options).map_err(|error| EngineError::TaskSerialization {
            module: module_id.to_owned(),
            task: task_id.to_owned(),
            message: error.to_string(),
        })?;
    let recommend =
        serde_json::to_value(&task.recommend).map_err(|error| EngineError::TaskSerialization {
            module: module_id.to_owned(),
            task: task_id.to_owned(),
            message: error.to_string(),
        })?;
    Ok(TaskRequest {
        module_id: module_id.to_owned(),
        task_id: task_id.to_owned(),
        name: task.name.clone(),
        handler_id: task.handler.clone(),
        options,
        recommend,
        tags: task.tags.clone(),
    })
}

fn task_matches_platform(task: &TaskDefinition) -> bool {
    task.platforms.is_empty()
        || task
            .platforms
            .iter()
            .any(|platform| platform == current_platform())
}

fn validate_plan_group(group: &PlanGroup) -> Result<(), String> {
    if group.module_id != group.request.module_id
        || group.task_id != group.request.task_id
        || group.handler_id != group.request.handler_id
    {
        return Err("Cleanup plan group does not match its task request".to_owned());
    }

    let mut item_ids = BTreeSet::new();
    if let Some(duplicate) = group
        .items
        .iter()
        .find(|item| !item_ids.insert(item.id.as_str()))
    {
        return Err(format!(
            "Cleanup plan contains duplicate item ID `{}`",
            duplicate.id
        ));
    }
    if let Some(item) = group
        .items
        .iter()
        .find(|item| item.recommendation == Recommendation::Unavailable)
    {
        return Err(format!(
            "Cleanup plan contains unavailable item ID `{}`",
            item.id
        ));
    }
    Ok(())
}

fn validate_scan_items(items: Vec<CleanupItem>) -> Result<Vec<CleanupItem>, String> {
    let mut parents = BTreeMap::new();
    for item in &items {
        if item.id.is_empty() {
            return Err("Handler returned a cleanup item with an empty ID".to_owned());
        }
        if parents
            .insert(item.id.as_str(), item.parent_id.as_deref())
            .is_some()
        {
            return Err(format!("Handler returned duplicate item ID `{}`", item.id));
        }
    }
    for item in &items {
        let Some(parent) = item.parent_id.as_deref() else {
            continue;
        };
        if parent == item.id {
            return Err(format!("Cleanup item `{}` is its own parent", item.id));
        }
        if !parents.contains_key(parent) {
            return Err(format!(
                "Cleanup item `{}` references unknown parent `{parent}`",
                item.id
            ));
        }

        let mut ancestors = BTreeSet::from([item.id.as_str()]);
        let mut next = Some(parent);
        while let Some(id) = next {
            if !ancestors.insert(id) {
                return Err(format!("Cleanup item hierarchy contains a cycle at `{id}`"));
            }
            next = parents.get(id).copied().flatten();
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests;

fn handler_disabled(disabled: &BTreeSet<String>, handler: &str) -> bool {
    handler
        .split_once('/')
        .is_some_and(|(plugin, _)| disabled.contains(plugin))
}
