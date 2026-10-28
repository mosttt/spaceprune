//! 不依赖终端交互的 SpacePrune 应用接口。

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::{Result, anyhow};

pub use crate::automation::{AutomationRunReport, AutomationRunStatus};
pub use crate::cleanup_receipt::{UnfinishedCleanupItem, unfinished_cleanup_items};
pub use crate::system_capabilities::{
    discover_system_capabilities, inspect_system_capability, probe_system_capabilities,
};
use crate::{build_engine, build_readonly_engine};
pub use spaceprune_plugin_developer_tools::CapabilityProbe;

pub use crate::native_access::NativeScanAccess;
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
pub use crate::native_access::shared_metadata_service;
#[cfg(target_os = "linux")]
pub use crate::native_access::{snapper_configs, snapper_snapshots};
pub use spaceprune_access::storage::{StorageVolume, local_storage_volumes};

/// Select the product message language without changing operation semantics.
pub use spaceprune_contracts::messages::{display_language, set_display_language};

pub use spaceprune_contracts::plugin::{BUILTIN_PLUGIN_IDS, is_builtin_plugin_id};

pub use crate::plugins::{PluginCatalogEntry, PluginKind, list_plugin_catalog, set_plugin_enabled};
pub use spaceprune_config::{
    AUTHORIZATION_COMMAND_ID, AUTOMATION_COMMAND_ID, AutomationDefinition, CLEAN_COMMAND_ID,
    ConfigLoader, ConfigStore, ConfigStoreError, DEFAULT_THEME_ID, LANGUAGE_COMMAND_ID, Language,
    LanguagePreference, LanguageSettings, MODULE_COMMAND_ID, ModuleDefinition, PLUGIN_COMMAND_ID,
    RECIPE_COMMAND_ID, RecipeBundle, RecipeInfo, ResolvedConfig, SCAN_COMMAND_ID, SCHEMA_VERSION,
    SelectorDefinition, THEME_COMMAND_ID, THEME_IDS, TaskDefinition, current_platform,
    rename_recipe_bundle, update_bundle_automation,
};
pub use spaceprune_engine::{
    CancellationToken, CleanupItem, CleanupPlan, CleanupPlanData, CleanupSizeAccounting,
    CleanupSizeSummary, EngineError, EventBus, EventListenerId, ExecutionJournalReference,
    ExecutionReport, GroupExecutionReport, HandlerDescriptor, HandlerIssue, IssueLevel, ItemKey,
    ModuleReport, NullProgress, PlanGroup, ProgressEvent, ProgressPhase, ProgressSink,
    Recommendation, RecoveredExecution, ScanAccess, ScanReport, ScanReportData, ScanRequest,
    SpacePruneEvent, TaskReport, TaskRequest, recover_execution,
};
pub use spaceprune_platform::{
    AutomationInstall, PlatformError, automation_installed, automation_installed_checked,
    install_automation, uninstall_automation,
};
pub use spaceprune_plugin_api::{
    HandlerScanResult, ItemExecution, ItemExecutionStatus, LogicalSizeSummary, PluginError,
    PluginResult, SizeApplicability, VolumeSpaceChange,
};
pub use spaceprune_plugin_api::{
    linux_authorization_interaction, prefer_graphical_linux_authorization,
};

pub use spaceprune_plugin_host::{
    HostError, InstalledPlugin, PluginHost, PluginManifest, install_plugin, plugin_search_paths,
    remove_plugin,
};
#[cfg(target_os = "linux")]
pub use spaceprune_plugin_snapper::{
    SnapperConfig, SnapperInventory, SnapperSnapshot, SnapperSnapshotGroup, snapper_host_id,
};

/// 扫描报告展示和 Handler 共用的文件系统辅助函数。
pub mod fs {
    pub use spaceprune_engine::fs::{
        PathStats, format_bytes, is_older_than, latest_modified, measure_path, parse_bytes,
        parse_duration, path_stats, remove_path, unix_timestamp,
    };
}

/// 一个已经加载配置并装配全部可用 Handler 的 SpacePrune 会话。
///
/// Scans do not modify the inspected content; native permission components may be prepared on demand.
/// Cleanup runs only through [`SpacePrune::execute`] with a [`CleanupPlan`] produced from a scan.
pub struct SpacePrune {
    config_path: Option<PathBuf>,
    engine: spaceprune_engine::Engine,
    plugin_policy: crate::plugins::PolicySnapshot,
}

/// 仅装配内置只读 Handler 的扫描会话。
///
/// 此类型不提供清理执行入口，也不会发现或启动外部插件，供 AI Explorer 等不可信决策边界使用。
pub struct ReadonlySpacePrune {
    config_path: Option<PathBuf>,
    engine: spaceprune_engine::Engine,
    plugin_policy: crate::plugins::PolicySnapshot,
}

impl std::fmt::Debug for SpacePrune {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpacePrune")
            .field("config_path", &self.config_path)
            .field("config", self.engine.config())
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for ReadonlySpacePrune {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReadonlySpacePrune")
            .field("config_path", &self.config_path)
            .field("config", self.engine.config())
            .finish_non_exhaustive()
    }
}

impl SpacePrune {
    /// 从显式应用配置加载会话；传入 `None` 时使用平台默认配置或内嵌最小 Recipe。
    pub fn load(config_path: Option<&Path>) -> Result<Self> {
        Self::load_in_context(config_path, &OperationContext::default())
    }

    /// Load and validate configuration under the caller's operation, including plugin setup.
    pub fn load_in_context(config_path: Option<&Path>, context: &OperationContext) -> Result<Self> {
        context.check()?;
        let mut loader = ConfigLoader::new();
        let config = match config_path {
            Some(path) => loader.load_app_path(path)?,
            None => loader.load_default()?,
        };
        Self::from_config_in_context(config_path.map(Path::to_path_buf), config, context)
    }

    /// 从一个 Recipe 入口文件直接加载会话，不经过应用 `config.toml`。
    pub fn load_recipe(recipe_path: &Path) -> Result<Self> {
        Self::load_recipe_in_context(recipe_path, &OperationContext::default())
    }

    /// Load one Recipe under the caller's original setup and validation deadline.
    pub fn load_recipe_in_context(recipe_path: &Path, context: &OperationContext) -> Result<Self> {
        context.check()?;
        let config = ConfigLoader::new().load_path(recipe_path)?;
        Self::from_config_in_context(None, config, context)
    }

    /// 从已经解析的配置创建会话。
    pub fn from_config(config_path: Option<PathBuf>, config: ResolvedConfig) -> Result<Self> {
        Self::from_config_in_context(config_path, config, &OperationContext::default())
    }

    /// Assemble configured handlers without resetting the caller's operation limits.
    pub fn from_config_in_context(
        config_path: Option<PathBuf>,
        config: ResolvedConfig,
        context: &OperationContext,
    ) -> Result<Self> {
        context.check()?;
        Self::from_config_with_policy(
            config_path,
            config,
            context,
            crate::plugins::PolicySnapshot::discover()?,
        )
    }

    pub(crate) fn from_config_with_policy(
        config_path: Option<PathBuf>,
        config: ResolvedConfig,
        context: &OperationContext,
        plugin_policy: crate::plugins::PolicySnapshot,
    ) -> Result<Self> {
        Ok(Self {
            config_path,
            engine: build_engine(config, context, plugin_policy.disabled())?,
            plugin_policy,
        })
    }

    /// 返回显式应用配置路径；使用默认发现时为 `None`。
    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    /// 返回当前完整解析后的 Recipe 配置。
    pub fn config(&self) -> &ResolvedConfig {
        self.engine.config()
    }

    /// Return this session's registered handler metadata for consistent host presentation.
    pub fn handler_descriptors(&self) -> Vec<HandlerDescriptor> {
        self.engine.handler_descriptors()
    }

    /// 执行一次只读扫描。
    pub fn scan(
        &self,
        request: &ScanRequest,
        progress: &dyn ProgressSink,
        cancellation: &CancellationToken,
    ) -> Result<ScanReport, EngineError> {
        self.scan_in_context(request, progress, &default_operation_context(cancellation))
    }

    /// Scan under the caller's original identity, cancellation and finite resource limits.
    pub fn scan_in_context(
        &self,
        request: &ScanRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> Result<ScanReport, EngineError> {
        self.plugin_policy.check_current()?;
        self.engine.scan_with_access(
            request,
            progress,
            context,
            Some(&crate::native_access::default_scan_access()),
        )
    }

    /// Scan with a host-owned, read-only authorization policy for inaccessible task results.
    pub fn scan_with_access(
        &self,
        request: &ScanRequest,
        progress: &dyn ProgressSink,
        cancellation: &CancellationToken,
        access: &dyn ScanAccess,
    ) -> Result<ScanReport, EngineError> {
        self.scan_with_access_in_context(
            request,
            progress,
            &default_operation_context(cancellation),
            access,
        )
    }

    /// Scan with a host-owned service while retaining the caller's operation context.
    pub fn scan_with_access_in_context(
        &self,
        request: &ScanRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
        access: &dyn ScanAccess,
    ) -> Result<ScanReport, EngineError> {
        self.plugin_policy.check_current()?;
        self.engine
            .scan_with_access(request, progress, context, Some(access))
    }

    /// 执行一个由本会话扫描结果生成的清理计划。
    pub fn execute(
        &self,
        plan: &CleanupPlan,
        progress: &dyn ProgressSink,
        cancellation: &CancellationToken,
    ) -> ExecutionReport {
        self.execute_in_context(plan, progress, &default_operation_context(cancellation))
    }

    /// Execute the immutable confirmed plan under the caller's original operation limits.
    pub fn execute_in_context(
        &self,
        plan: &CleanupPlan,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> ExecutionReport {
        if let Err(error) = self.plugin_policy.check_current() {
            return self.engine.reject_execution(
                plan,
                PluginError::new(
                    match &error {
                        EngineError::PluginPolicyUnavailable(_) => "plugin-policy-unavailable",
                        _ => "plugin-policy-changed",
                    },
                    error.to_string(),
                ),
            );
        }
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
        {
            let access = crate::cleanup_access::NativeCleanupAccess::new(plan, context);
            self.engine
                .execute_with_access(plan, progress, context, Some(&access))
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
        self.engine.execute(plan, progress, context)
    }

    /// 按已启用自动化的模块范围重新执行一次只读扫描。
    pub fn scan_automation(
        &self,
        automation_id: &str,
        progress: &dyn ProgressSink,
        cancellation: &CancellationToken,
    ) -> Result<ScanReport> {
        self.scan_automation_in_context(
            automation_id,
            progress,
            &default_operation_context(cancellation),
        )
    }

    /// Scan all automation modules without renewing the caller's deadline between modules.
    pub fn scan_automation_in_context(
        &self,
        automation_id: &str,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> Result<ScanReport> {
        if self.config().usage == spaceprune_config::RecipeUsage::OneShot {
            return Err(anyhow!(
                "one_shot recipes cannot be used by recurring automation"
            ));
        }

        let automation = self
            .config()
            .automations
            .get(automation_id)
            .filter(|automation| automation.enabled)
            .ok_or_else(|| anyhow!("Enabled automation `{automation_id}` was not found"))?;
        if automation.modules.is_empty() {
            return self
                .scan_in_context(&ScanRequest::default(), progress, context)
                .map_err(Into::into);
        }
        let mut modules = Vec::new();
        let mut cancelled = false;
        for module in &automation.modules {
            let report = self.scan_in_context(
                &ScanRequest {
                    module: Some(module.clone()),
                    selectors: BTreeSet::new(),
                    tasks: BTreeSet::new(),
                },
                progress,
                context,
            )?;
            cancelled |= report.cancelled;
            modules.extend(report.into_report().modules);
            if cancelled {
                break;
            }
        }
        Ok(ScanReport::from_report(ScanReportData {
            scan_id: ScanReport::new_scan_id(),
            config_name: self.config().name.clone(),
            config_source: self.config().source.clone(),
            modules,
            cancelled,
            stop_reason: context.stop_reason(),
        }))
    }

    /// 重新扫描并执行一个自动化的推荐清理计划。
    pub fn run_automation(
        &self,
        automation_id: &str,
        progress: &dyn ProgressSink,
        cancellation: &CancellationToken,
    ) -> Result<AutomationRunReport> {
        self.run_automation_in_context(
            automation_id,
            progress,
            &default_operation_context(cancellation),
        )
    }

    /// Run a background or foreground automation under one explicitly supplied policy and budget.
    pub fn run_automation_in_context(
        &self,
        automation_id: &str,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> Result<AutomationRunReport> {
        let scan = self.scan_automation_in_context(automation_id, progress, context)?;
        let plan = scan.plan_recommended()?;
        let execution = self.execute_in_context(&plan, progress, context);
        Ok(AutomationRunReport {
            scan,
            plan,
            execution,
        })
    }

    /// 返回生命周期事件总线，供 GUI 或宿主订阅扫描与清理事件。
    pub fn event_bus(&self) -> &EventBus {
        self.engine.event_bus()
    }
}

impl ReadonlySpacePrune {
    /// 从应用配置加载内置只读扫描会话。
    pub fn load(config_path: Option<&Path>) -> Result<Self> {
        Self::load_in_context(config_path, &OperationContext::default())
    }

    /// Load and validate configuration under the caller's operation, including plugin setup.
    pub fn load_in_context(config_path: Option<&Path>, context: &OperationContext) -> Result<Self> {
        context.check()?;
        let mut loader = ConfigLoader::new();
        let config = match config_path {
            Some(path) => loader.load_app_path(path)?,
            None => loader.load_default()?,
        };
        Self::from_config_in_context(config_path.map(Path::to_path_buf), config, context)
    }

    /// 从 Recipe 入口文件加载内置只读扫描会话。
    pub fn load_recipe(recipe_path: &Path) -> Result<Self> {
        Self::load_recipe_in_context(recipe_path, &OperationContext::default())
    }

    /// Load one Recipe under the caller's original setup and validation deadline.
    pub fn load_recipe_in_context(recipe_path: &Path, context: &OperationContext) -> Result<Self> {
        context.check()?;
        let config = ConfigLoader::new().load_path(recipe_path)?;
        Self::from_config_in_context(None, config, context)
    }

    fn from_config_in_context(
        config_path: Option<PathBuf>,
        config: ResolvedConfig,
        context: &OperationContext,
    ) -> Result<Self> {
        let plugin_policy = crate::plugins::PolicySnapshot::discover()?;
        Ok(Self {
            config_path,
            engine: build_readonly_engine(config, context, plugin_policy.disabled())?,
            plugin_policy,
        })
    }

    /// 返回过滤外部插件任务后的只读配置。
    pub fn config(&self) -> &ResolvedConfig {
        self.engine.config()
    }

    /// 执行一次内置 Handler 只读扫描。
    pub fn scan(
        &self,
        request: &ScanRequest,
        progress: &dyn ProgressSink,
        cancellation: &CancellationToken,
    ) -> Result<ScanReport, EngineError> {
        self.scan_in_context(request, progress, &default_operation_context(cancellation))
    }

    /// Scan under the caller's original identity, cancellation and finite resource limits.
    pub fn scan_in_context(
        &self,
        request: &ScanRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> Result<ScanReport, EngineError> {
        self.plugin_policy.check_current()?;
        self.engine.scan_with_access(
            request,
            progress,
            context,
            Some(&crate::native_access::default_scan_access()),
        )
    }

    /// Apply the host's shared native metadata service without registering cleanup handlers.
    pub fn scan_with_access(
        &self,
        request: &ScanRequest,
        progress: &dyn ProgressSink,
        cancellation: &CancellationToken,
        access: &dyn ScanAccess,
    ) -> Result<ScanReport, EngineError> {
        self.scan_with_access_in_context(
            request,
            progress,
            &default_operation_context(cancellation),
            access,
        )
    }

    /// Scan with a host-owned service while retaining the caller's operation context.
    pub fn scan_with_access_in_context(
        &self,
        request: &ScanRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
        access: &dyn ScanAccess,
    ) -> Result<ScanReport, EngineError> {
        self.plugin_policy.check_current()?;
        self.engine
            .scan_with_access(request, progress, context, Some(access))
    }
}

/// 发现应用配置存储；显式路径为空时使用当前平台的首选位置。
pub fn discover_config_store(config_path: Option<&Path>) -> Option<ConfigStore> {
    ConfigStore::discover(config_path)
}

/// 校验 Recipe 的 Schema、include、变量、Handler 和当前平台可执行性。
pub fn validate_recipe_file(recipe_path: &Path) -> Result<ResolvedConfig> {
    let config = ConfigLoader::new().load_path(recipe_path)?;
    let policy = crate::plugins::PolicySnapshot::discover()?;
    let _ = build_engine(
        config.clone(),
        &OperationContext::default(),
        policy.disabled(),
    )?;
    Ok(config)
}

/// Native access failure classes shared by all hosts.
pub use spaceprune_access::{
    Interaction, IoFailure, IoFailureKind, NativePath, OperationContext, OperationIdentity,
    OperationStopReason, PermissionOutcome,
};

/// Native identity and local machine binding primitives for trusted hosts.
pub use spaceprune_access::filesystem::{FileIdentity, read_identity};
/// Read the local machine identity without a cloud account.
pub use spaceprune_access::host::identity as local_host_identity;
/// Persistent rule declarations and native object bindings.
pub use spaceprune_config::{ObjectBinding, RecipeDefinition, RecipeUsage, RootSpec, RuleScope};

fn default_operation_context(cancellation: &CancellationToken) -> OperationContext {
    OperationContext::workflow(
        cancellation.new_operation(),
        spaceprune_plugin_api::linux_authorization_interaction(),
    )
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs};

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn application_facade_scans_plans_and_executes() {
        let temp = tempdir().unwrap();
        let cache = temp.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let garbage = cache.join("garbage.bin");
        fs::write(&garbage, [1_u8; 16]).unwrap();
        let recipe = temp.path().join("test.toml");
        fs::write(
            &recipe,
            format!(
                r#"schema-version = 2
name = "Facade test"

[modules.personal]
name = "Personal"

[modules.personal.tasks.cache]
name = "Cache"
handler = "builtin.filesystem/paths"

[modules.personal.tasks.cache.options]
roots = [{{ path = {:?} }}]
files = true
directories = false

[modules.personal.tasks.cache.recommend]
enabled = true
"#,
                cache.to_string_lossy()
            ),
        )
        .unwrap();

        let application = SpacePrune::load_recipe(&recipe).unwrap();
        let cancellation = CancellationToken::default();
        let report = application
            .scan(
                &ScanRequest {
                    module: Some("personal".to_owned()),
                    selectors: BTreeSet::new(),
                    tasks: BTreeSet::from(["cache".to_owned()]),
                },
                &NullProgress,
                &cancellation,
            )
            .unwrap();
        let plan = report.plan_recommended().unwrap();
        assert_eq!(plan.item_count(), 1);
        let execution = application.execute(&plan, &NullProgress, &cancellation);
        assert_eq!(execution.succeeded(), 1);
        assert!(!garbage.exists());
    }

    #[test]
    fn readonly_facade_removes_external_plugin_tasks_and_has_no_execute_api() {
        let temp = tempdir().unwrap();
        let cache = temp.path().join("cache");
        fs::create_dir(&cache).unwrap();
        fs::write(cache.join("item.bin"), [1_u8; 8]).unwrap();
        let recipe = temp.path().join("readonly.toml");
        fs::write(
            &recipe,
            format!(
                r#"schema-version = 2
name = "Readonly"

[modules.cache]
name = "Cache"

[modules.cache.tasks.files]
name = "Files"
handler = "builtin.filesystem/paths"
[modules.cache.tasks.files.options]
roots = [{{ path = {:?} }}]

[modules.external]
name = "External"
[modules.external.tasks.unsafe]
name = "External task"
handler = "third-party.writer/scan"
"#,
                cache.to_string_lossy()
            ),
        )
        .unwrap();

        let readonly = ReadonlySpacePrune::load_recipe(&recipe).unwrap();
        assert_eq!(readonly.config().modules.len(), 1);
        assert!(readonly.config().modules.contains_key("cache"));
        let report = readonly
            .scan(
                &ScanRequest::default(),
                &NullProgress,
                &CancellationToken::default(),
            )
            .unwrap();
        assert_eq!(report.modules.len(), 1);
    }
}
