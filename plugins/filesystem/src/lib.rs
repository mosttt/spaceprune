#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! 通用文件系统清理插件。
//!
//! 每个根路径拥有独立的 include 白名单和 exclude 黑名单。规则匹配相对路径，
//! 支持 `*`、`?`、`[...]` 和 `**`，并始终由 exclude 获得更高优先级。

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use serde_json::{Value, json};
use spaceprune_engine::{EngineError, PluginRegistry, fs as core_fs};
use spaceprune_plugin_api::{
    CleanupBatchGroup, CleanupItem, Handler, HandlerCleanResult, HandlerDescriptor, HandlerIssue,
    HandlerScanResult, IssueLevel, ItemFailure, OperationContext, PluginError, PluginResult,
    ProgressEvent, ProgressPhase, ProgressSink, Recommendation, TaskRequest,
};

mod enumerate;
mod execute;
mod options;
mod selection;
use enumerate::*;
use execute::clean_local;
use options::*;

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
mod authorization;

/// Resolve roots in the invoking user's environment before sealing an authorized plan.
pub fn prepare_authorized_task(task: &TaskRequest) -> PluginResult<TaskRequest> {
    let options = parse_options(task)?;
    let mut task = task.clone();
    if let Some(roots) = task.options.get_mut("roots").and_then(Value::as_array_mut) {
        for (root, configured) in roots.iter_mut().zip(&options.roots) {
            root["path"] = json!(resolve_root(&configured.path)?);
        }
    }
    Ok(task)
}

/// Run only the built-in metadata scanner for an already scoped filesystem task.
/// This entry point never loads plugins or executes cleanup operations.
pub fn scan_readonly(
    task: &TaskRequest,
    progress: &dyn ProgressSink,
    context: &OperationContext,
) -> PluginResult<HandlerScanResult> {
    if task.handler_id != "builtin.filesystem/paths" {
        return Err(PluginError::new(
            "invalid-readonly-handler",
            spaceprune_plugin_api::messages::text("filesystem.readonly_only"),
        ));
    }
    PathsHandler.validate(task)?;
    PathsHandler.scan(task, progress, context)
}

/// 将本插件的全部 Handler 注册到执行器。
pub fn register(registry: &mut PluginRegistry) -> Result<(), EngineError> {
    registry.register(Arc::new(PathsHandler))
}

#[derive(Debug)]
struct PathsHandler;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Options {
    roots: Vec<RootOptions>,
    #[serde(default = "default_true")]
    files: bool,
    #[serde(default = "default_true")]
    directories: bool,
    #[serde(default = "default_elevation")]
    elevation: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RootOptions {
    path: String,
    #[serde(default)]
    #[schemars(schema_with = "spaceprune_contracts::schema::filesystem_scope_schema")]
    scope: spaceprune_contracts::rule::RuleScope,
    #[serde(default)]
    bindings: Vec<spaceprune_contracts::rule::ObjectBinding>,
    #[serde(default = "default_include")]
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct Recommend {
    enabled: bool,
    older_than: Option<String>,
    min_size: Option<String>,
}

impl Default for Recommend {
    fn default() -> Self {
        Self {
            enabled: true,
            older_than: None,
            min_size: None,
        }
    }
}

#[derive(Debug)]
struct RootMatcher {
    include: GlobSet,
    exclude: GlobSet,
    has_excludes: bool,
    recursive: bool,
    ancestor_globs: GlobSet,
    direct_children: bool,
}

#[derive(Debug, Default)]
struct Collected {
    paths: Vec<PathBuf>,
    contains_excluded: bool,
}

impl Handler for PathsHandler {
    fn descriptor(&self) -> HandlerDescriptor {
        HandlerDescriptor {
            size_accounting: Default::default(),
            id: "builtin.filesystem/paths".to_owned(),
            name: spaceprune_plugin_api::messages::text("filesystem.name").to_owned(),
            description: spaceprune_plugin_api::messages::text("filesystem.description").to_owned(),
            platforms: Vec::new(),
            options_schema: serde_json::to_value(schemars::schema_for!(Options))
                .expect("filesystem options schema"),
            recommend_schema: serde_json::to_value(schemars::schema_for!(Recommend))
                .expect("filesystem recommendation schema"),
        }
    }

    fn validate(&self, task: &TaskRequest) -> PluginResult<()> {
        let options = parse_options(task)?;
        if !matches!(
            options.elevation.as_str(),
            "auto" | "never" | "sudo" | "pkexec" | "uac" | "native"
        ) {
            return Err(PluginError::new(
                "invalid-elevation",
                spaceprune_plugin_api::messages::text("filesystem.invalid_elevation"),
            ));
        }
        if options.roots.is_empty() {
            return Err(PluginError::new(
                "empty-roots",
                spaceprune_plugin_api::messages::text("filesystem.no_roots"),
            ));
        }
        if !options.files && !options.directories {
            return Err(PluginError::new(
                "empty-types",
                spaceprune_plugin_api::messages::text("filesystem.no_types"),
            ));
        }
        for root in &options.roots {
            validate_root(root)?;
        }
        parse_recommend(task).map(|_| ())
    }

    fn scan(
        &self,
        task: &TaskRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult> {
        let options = parse_options(task)?;
        let recommend = parse_recommend(task)?;
        let minimum = recommend
            .min_size
            .as_deref()
            .map(core_fs::parse_bytes)
            .transpose()?
            .unwrap_or(0);
        let older_than = recommend
            .older_than
            .as_deref()
            .map(core_fs::parse_duration)
            .transpose()?;
        let mut result = HandlerScanResult::default();
        let mut seen = BTreeSet::new();

        for (root_index, root_options) in options.roots.iter().enumerate() {
            if context.is_stopped() {
                break;
            }
            let root = resolve_root(&root_options.path)?;
            if let Err(error) = validate_persistent_bindings(root_options, &root) {
                result.issues.push(HandlerIssue {
                    code: None,
                    level: IssueLevel::Error,
                    message: error.to_string(),
                    location: Some(root.display().to_string()),
                    io_failure: Some(spaceprune_access::error::classify(&error)),
                });
                continue;
            }
            if fs::symlink_metadata(&root)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                continue;
            }
            let matcher = RootMatcher::compile(root_options)?;
            emit_progress(
                progress,
                task,
                ProgressPhase::Scan,
                None,
                None,
                spaceprune_plugin_api::messages::message(
                    "filesystem.check_root",
                    &[("p0", (root.display()).to_string())],
                ),
            );
            let paths = collect_selection(
                &root,
                root_options,
                &matcher,
                &options,
                &mut result.issues,
                context,
            );
            let total = paths.len();
            let mut updated = Instant::now();
            for (index, path) in paths.into_iter().enumerate() {
                if context.is_stopped() {
                    break;
                }
                if index == 0
                    || index + 1 == total
                    || updated.elapsed() >= Duration::from_millis(100)
                {
                    emit_progress(
                        progress,
                        task,
                        ProgressPhase::Measure,
                        Some(index),
                        Some(total),
                        spaceprune_plugin_api::messages::message(
                            "filesystem.measure_candidate",
                            &[("p0", (path.display()).to_string())],
                        ),
                    );
                    updated = Instant::now();
                }
                if seen.insert(path.clone()) {
                    append_item(
                        &mut result,
                        &path,
                        ScanRoot {
                            path: &root,
                            index: root_index,
                        },
                        &recommend,
                        minimum,
                        older_than,
                        context,
                    );
                }
            }
        }
        Ok(result)
    }

    fn clean(
        &self,
        task: &TaskRequest,
        items: &[CleanupItem],
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerCleanResult> {
        self.clean_batch(&[CleanupBatchGroup { task, items }], progress, context)
            .pop()
            .expect("one filesystem cleanup group")
    }

    fn supports_batch_cleanup(&self) -> bool {
        cfg!(any(
            target_os = "linux",
            target_os = "windows",
            target_os = "macos"
        ))
    }

    fn clean_batch_with_access(
        &self,
        groups: &[CleanupBatchGroup<'_>],
        progress: &dyn ProgressSink,
        context: &OperationContext,
        access: Option<&dyn spaceprune_plugin_api::CleanupAccess>,
    ) -> Vec<PluginResult<HandlerCleanResult>> {
        let results = groups
            .iter()
            .map(|group| clean_local(group.task, group.items, progress, context))
            .collect();
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
        return authorization::retry_denied_with_access(groups, results, progress, context, access);
        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
        {
            let _ = access;
            results
        }
    }

    fn clean_batch(
        &self,
        groups: &[CleanupBatchGroup<'_>],
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> Vec<PluginResult<HandlerCleanResult>> {
        groups
            .iter()
            .map(|group| clean_local(group.task, group.items, progress, context))
            .collect()
    }
}

fn emit_progress(
    progress: &dyn ProgressSink,
    task: &TaskRequest,
    phase: ProgressPhase,
    current: Option<usize>,
    total: Option<usize>,
    message: String,
) {
    progress.emit(ProgressEvent {
        phase,
        module_id: Some(task.module_id.clone()),
        task_id: Some(task.task_id.clone()),
        handler_id: Some(task.handler_id.clone()),
        current,
        total,
        message,
    });
}

#[cfg(test)]
mod tests;
