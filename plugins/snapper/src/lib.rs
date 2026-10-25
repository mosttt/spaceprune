#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! Read-only Snapper inventory and snapshot cleanup.
//!
//! Reported sizes do not guarantee reclaimable space because snapshots can share Btrfs blocks.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use spaceprune_engine::{EngineError, PluginRegistry, fs as core_fs};
use spaceprune_plugin_api::{
    CleanupBatchGroup, CleanupItem, Handler, HandlerCleanResult, HandlerDescriptor, HandlerIssue,
    HandlerScanResult, IssueLevel, ItemFailure, OperationContext, PluginError, PluginResult,
    ProgressEvent, ProgressPhase, ProgressSink, Recommendation, TaskRequest,
};

#[cfg(test)]
use spaceprune_plugin_api::NullProgress;

mod command;
mod identity;
mod inventory;
mod policy;
mod scan;
mod validation;
use command::*;
use identity::*;
use inventory::*;
use policy::*;
use size::measure_targets;

mod cleanup;
mod readonly;
mod selection;
mod size;
pub use readonly::{
    SnapperConfig, SnapperInventory, SnapperSnapshot, SnapperSnapshotGroup, snapper_host_id,
    snapper_inventory_unprivileged,
};

/// Register the Snapper handler with the engine.
pub fn register(registry: &mut PluginRegistry) -> Result<(), EngineError> {
    registry.register(Arc::new(SnapperHandler))
}

/// Scan only snapshot metadata without cleanup or additional authorization.
/// The privileged caller must supply trusted Snapper and du executable paths.
pub fn scan_readonly(
    task: &TaskRequest,
    progress: &dyn ProgressSink,
    context: &OperationContext,
) -> PluginResult<HandlerScanResult> {
    if task.handler_id != "builtin.snapper/snapshots"
        || !matches!(
            task.options.get("scan-mode").and_then(Value::as_str),
            None | Some("snapshots" | "rust-targets")
        )
    {
        return Err(PluginError::new(
            "invalid-readonly-handler",
            spaceprune_plugin_api::messages::text("snapper.readonly_only"),
        ));
    }
    SnapperHandler.validate(task)?;
    SnapperHandler.scan_unprivileged(task, progress, context)
}

#[derive(Debug)]
struct SnapperHandler;

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct Options {
    configs: Vec<String>,
    scan_mode: String,
    snapshot_ids: Option<Vec<u64>>,
    snapshot_host_id: Option<String>,
    snapshot_identities: Vec<SnapshotIdentity>,
    snapshot_types: Vec<String>,
    cleanup_algorithms: Vec<String>,
    relative_path: Option<PathBuf>,
    snapper: String,
    find: String,
    du: String,
    elevation: String,
    max_depth: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            configs: Vec::new(),
            scan_mode: "rust-targets".to_owned(),
            snapshot_ids: None,
            snapshot_host_id: None,
            snapshot_identities: Vec::new(),
            snapshot_types: Vec::new(),
            cleanup_algorithms: Vec::new(),
            relative_path: None,
            snapper: "snapper".to_owned(),
            find: "find".to_owned(),
            du: "du".to_owned(),
            elevation: "auto".to_owned(),
            max_depth: 20,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct Recommend {
    min_target_size: String,
    min_snapshot_size: String,
    older_than_days: u64,
    keep_latest: usize,
    enabled: bool,
}

impl Default for Recommend {
    fn default() -> Self {
        Self {
            min_target_size: "1GiB".to_owned(),
            min_snapshot_size: "0B".to_owned(),
            older_than_days: 0,
            keep_latest: 3,
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Snapshot {
    number: u64,
    #[serde(default, rename = "default")]
    is_default: bool,
    #[serde(default)]
    active: bool,
    #[serde(default, rename = "type")]
    snapshot_type: String,
    #[serde(default, rename = "pre-number")]
    pre_number: Option<u64>,
    #[serde(default)]
    date: String,
    #[serde(default)]
    cleanup: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    userdata: Value,
    #[serde(default, rename = "used-space")]
    used_space: Value,
}

#[derive(Debug)]
struct SnapshotGroup {
    snapshots: Vec<Snapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct SnapshotIdentity {
    number: u64,
    date: String,
    #[serde(rename = "type")]
    snapshot_type: String,
    #[serde(default)]
    pre_number: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotScanIdentity {
    host_id: String,
    subvolume: PathBuf,
    snapshots: Vec<SnapshotIdentity>,
}

impl From<&Snapshot> for SnapshotIdentity {
    fn from(snapshot: &Snapshot) -> Self {
        Self {
            number: snapshot.number,
            date: snapshot.date.clone(),
            snapshot_type: snapshot.snapshot_type.clone(),
            pre_number: snapshot.pre_number,
        }
    }
}

impl Handler for SnapperHandler {
    fn descriptor(&self) -> HandlerDescriptor {
        HandlerDescriptor {
            size_accounting: spaceprune_plugin_api::CleanupSizeAccounting::SnapshotGroups,
            id: "builtin.snapper/snapshots".to_owned(),
            name: spaceprune_plugin_api::messages::text("snapper.name").to_owned(),
            description: spaceprune_plugin_api::messages::text("snapper.description").to_owned(),
            platforms: vec!["linux".to_owned()],
            options_schema: json!({
                "type": "object",
                "properties": {
                    "configs": {"type": "array", "items": {"type": "string"}},
                    "scan-mode": {"enum": ["rust-targets", "snapshots"]},
                    "snapshot-ids": {"type": "array", "minItems": 1, "uniqueItems": true, "items": {"type": "integer", "minimum": 1}},
                    "snapshot-host-id": {"type": "string"},
                    "snapshot-identities": {"type": "array", "items": {"type": "object", "required": ["number", "date", "type"], "properties": {"number": {"type": "integer", "minimum": 1}, "date": {"type": "string"}, "type": {"type": "string"}, "pre-number": {"type": ["integer", "null"]}}}},
                    "snapshot-types": {"type": "array", "uniqueItems": true, "items": {"enum": ["single", "pre", "post", "pre/post"]}},
                    "cleanup-algorithms": {"type": "array", "uniqueItems": true, "items": {"type": "string", "maxLength": 128}},
                    "relative-path": {"type": "string"},
                    "snapper": {"type": "string"},
                    "find": {"type": "string"},
                    "du": {"type": "string"},
                    "elevation": {"enum": ["auto", "never", "sudo", "pkexec"]},
                    "max-depth": {"type": "integer", "minimum": 1}
                }
            }),
            recommend_schema: json!({
                "type": "object",
                "properties": {
                    "enabled": {"type": "boolean"},
                    "min-target-size": {"type": "string"},
                    "min-snapshot-size": {"type": "string"},
                    "older-than-days": {"type": "integer", "minimum": 0},
                    "keep-latest": {"type": "integer", "minimum": 0}
                }
            }),
        }
    }

    fn validate(&self, task: &TaskRequest) -> PluginResult<()> {
        validation::validate(task)
    }

    fn scan(
        &self,
        task: &TaskRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult> {
        scan::scan(task, progress, context)
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
            .expect("one cleanup group produces one result")
    }

    fn cleanup_authorization_method(&self, task: &TaskRequest) -> PluginResult<Option<String>> {
        let (options, _) = parse(task)?;
        Ok(elevation_method(&options.elevation, effective_uid() != Some(0)).map(str::to_owned))
    }

    fn supports_batch_cleanup(&self) -> bool {
        true
    }

    fn clean_batch(
        &self,
        groups: &[CleanupBatchGroup<'_>],
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> Vec<PluginResult<HandlerCleanResult>> {
        cleanup::clean_batch(groups, progress, context)
    }
}

impl SnapshotGroup {
    fn label(&self) -> String {
        self.snapshots
            .iter()
            .map(|snapshot| format!("#{}", snapshot.number))
            .collect::<Vec<_>>()
            .join(" + ")
    }

    fn numbers(&self) -> Vec<u64> {
        self.snapshots
            .iter()
            .map(|snapshot| snapshot.number)
            .collect()
    }
}

fn parse(task: &TaskRequest) -> PluginResult<(Options, Recommend)> {
    let options: Options = serde_json::from_value(task.options.clone()).map_err(|error| {
        PluginError::new(
            "invalid-options",
            spaceprune_plugin_api::messages::message(
                "snapper.invalid_options",
                &[("error", (error).to_string())],
            ),
        )
    })?;
    let mut raw_recommend = task.recommend.clone();
    if options.scan_mode == "snapshots"
        && let Some(rules) = raw_recommend.as_object_mut()
    {
        rules.entry("enabled").or_insert(json!(false));
        rules.entry("keep-latest").or_insert(json!(0));
    }
    let recommend = serde_json::from_value(raw_recommend).map_err(|error| {
        PluginError::new(
            "invalid-recommend",
            spaceprune_plugin_api::messages::message(
                "snapper.invalid_recommend",
                &[("error", (error).to_string())],
            ),
        )
    })?;
    Ok((options, recommend))
}

#[cfg(test)]
mod tests;
