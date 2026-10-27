#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! Tool-owned cache discovery and identity-bound cleanup, separate from filesystem rules.

#[cfg(target_os = "linux")]
mod cache;
mod discovery;
mod docker;
#[cfg(target_os = "linux")]
mod execution;
#[cfg(target_os = "linux")]
mod scan;
#[cfg(target_os = "linux")]
pub use discovery::discover_legacy_go_task;
pub use discovery::{discover_tasks, probe_tasks};
#[cfg(target_os = "linux")]
pub use scan::{inspect, prepare, validate};

#[cfg(target_os = "linux")]
use serde::Deserialize;
use serde_json::json;
use spaceprune_engine::{EngineError, PluginRegistry};
use spaceprune_plugin_api::{
    CleanupItem, Handler, HandlerCleanResult, HandlerDescriptor, HandlerIssue, HandlerScanResult,
    IssueLevel, ItemFailure, OperationContext, PluginError, PluginResult, ProgressEvent,
    ProgressPhase, ProgressSink, Recommendation, TaskRequest, messages,
};
#[cfg(target_os = "linux")]
use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::{
    fs,
    path::{Path, PathBuf},
};
use std::{io, sync::Arc};

/// Read-only native capability probe, before any cache or object enumeration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CapabilityProbe {
    /// Handler configuration for this installed tool; usable only when `issue` is absent.
    pub task: TaskRequest,
    /// A native-interface, daemon, permission or endpoint diagnostic that blocks inspection.
    pub issue: Option<HandlerIssue>,
}

/// Canonical developer-tools plugin identifier.
pub const PLUGIN_ID: &str = "builtin.developer-tools";
/// Existing recipes retain their original Go lifecycle entry point.
pub const LEGACY_GO_HANDLER: &str = "builtin.system-linux/go-modcache";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(target_os = "linux")]
enum Kind {
    GoModCache,
    GoBuildCache,
    CargoRegistryCache,
    NpmCache,
}
#[cfg(target_os = "linux")]
impl Kind {
    const ALL: [Self; 4] = [
        Self::GoModCache,
        Self::GoBuildCache,
        Self::CargoRegistryCache,
        Self::NpmCache,
    ];
    const fn id(self) -> &'static str {
        match self {
            Self::GoModCache => "go-modcache",
            Self::GoBuildCache => "go-buildcache",
            Self::CargoRegistryCache => "cargo-registry-cache",
            Self::NpmCache => "npm-cache",
        }
    }
    fn name(self) -> &'static str {
        messages::text(match self {
            Self::GoModCache => "linux.go_cache",
            Self::GoBuildCache => "developer.go_build_cache",
            Self::CargoRegistryCache => "developer.cargo_registry_cache",
            Self::NpmCache => "developer.npm_cache",
        })
    }
    fn consequence(self) -> &'static str {
        messages::text(match self {
            Self::GoModCache => "developer.go_mod_consequence",
            Self::GoBuildCache => "developer.go_build_consequence",
            Self::CargoRegistryCache => "developer.cargo_consequence",
            Self::NpmCache => "developer.npm_consequence",
        })
    }
}

#[cfg(target_os = "linux")]
fn kind(handler: &str) -> PluginResult<Kind> {
    if handler == LEGACY_GO_HANDLER {
        return Ok(Kind::GoModCache);
    }
    Kind::ALL
        .into_iter()
        .find(|kind| handler == format!("{PLUGIN_ID}/{}", kind.id()))
        .ok_or_else(invalid_source)
}
/// Whether the handler is a registered developer cache or the preserved Go alias.
pub fn is_cache_handler(handler: &str) -> bool {
    matches!(
        handler,
        LEGACY_GO_HANDLER
            | "builtin.developer-tools/go-modcache"
            | "builtin.developer-tools/go-buildcache"
            | "builtin.developer-tools/cargo-registry-cache"
            | "builtin.developer-tools/npm-cache"
    )
}

/// Register the supported developer-tool lifecycle handlers.
pub fn register(registry: &mut PluginRegistry) -> Result<(), EngineError> {
    #[cfg(target_os = "linux")]
    for kind in Kind::ALL {
        registry.register(Arc::new(DeveloperHandler {
            kind,
            legacy: false,
        }))?;
    }
    docker::register(registry)?;
    Ok(())
}
/// Register only the original Go handler; discovery advertises the canonical plugin.
#[cfg(target_os = "linux")]
pub fn register_legacy_go(registry: &mut PluginRegistry) -> Result<(), EngineError> {
    registry.register(Arc::new(DeveloperHandler {
        kind: Kind::GoModCache,
        legacy: true,
    }))
}

#[derive(Debug)]
#[cfg(target_os = "linux")]
struct DeveloperHandler {
    kind: Kind,
    legacy: bool,
}
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[cfg(target_os = "linux")]
struct Options {
    elevation: String,
}
#[cfg(target_os = "linux")]
impl Default for Options {
    fn default() -> Self {
        Self {
            elevation: "auto".into(),
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[cfg(target_os = "linux")]
struct Recommend {
    enabled: bool,
}
#[cfg(target_os = "linux")]
impl Default for Recommend {
    fn default() -> Self {
        Self { enabled: true }
    }
}
#[derive(Debug)]
#[cfg(target_os = "linux")]
struct Action {
    id: String,
    paths: Vec<PathBuf>,
    go_binding: Option<cache::Binding>,
    unavailable_reason: Option<String>,
}
#[derive(Debug)]
#[cfg(target_os = "linux")]
enum ActionFailure {
    Native {
        started: bool,
        reclaimed: u64,
        error: io::Error,
    },
    BeforeStart(String),
}

#[cfg(target_os = "linux")]
impl Handler for DeveloperHandler {
    fn descriptor(&self) -> HandlerDescriptor {
        HandlerDescriptor {
            size_accounting: Default::default(),
            id: if self.legacy {
                LEGACY_GO_HANDLER.into()
            } else {
                format!("{PLUGIN_ID}/{}", self.kind.id())
            },
            name: self.kind.name().into(),
            description: self.kind.consequence().into(),
            platforms: vec!["linux".into()],
            options_schema: json!({"type":"object", "additionalProperties":false, "properties":{"elevation":{"enum":["auto","never","sudo","pkexec"]}}}),
            recommend_schema: json!({"type":"object", "additionalProperties":false, "properties":{"enabled":{"type":"boolean"}}}),
        }
    }
    fn validate(&self, task: &TaskRequest) -> PluginResult<()> {
        if kind(&task.handler_id)? != self.kind {
            return Err(invalid_source());
        }
        let (options, _) = parse(task)?;
        if !matches!(
            options.elevation.as_str(),
            "auto" | "never" | "sudo" | "pkexec"
        ) {
            return Err(PluginError::new(
                "invalid-elevation",
                messages::text("linux.invalid_elevation"),
            ));
        }
        Ok(())
    }
    fn scan(
        &self,
        task: &TaskRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult> {
        #[cfg(target_os = "linux")]
        return scan::ordinary(self.kind, task, progress, context);
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (task, progress, context);
            Err(PluginError::new(
                "unsupported-platform",
                messages::text("linux.unsupported_inspection"),
            ))
        }
    }
    fn clean(
        &self,
        task: &TaskRequest,
        items: &[CleanupItem],
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerCleanResult> {
        self.clean_with_access(task, items, progress, context, None)
    }
    fn clean_batch_with_access(
        &self,
        groups: &[spaceprune_plugin_api::CleanupBatchGroup<'_>],
        progress: &dyn ProgressSink,
        context: &OperationContext,
        access: Option<&dyn spaceprune_plugin_api::CleanupAccess>,
    ) -> Vec<PluginResult<HandlerCleanResult>> {
        groups
            .iter()
            .map(|group| self.clean_with_access(group.task, group.items, progress, context, access))
            .collect()
    }
}
#[cfg(target_os = "linux")]
fn parse(task: &TaskRequest) -> PluginResult<(Options, Recommend)> {
    // Preserve the original system-linux Go options without widening the new plugin schema.
    let mut value = task.options.clone();
    if task.handler_id == LEGACY_GO_HANDLER {
        for key in ["manager", "vacuum-time"] {
            if value.get(key).is_some_and(|field| !field.is_string()) {
                return Err(PluginError::new(
                    "invalid-options",
                    messages::text("developer.legacy_options_invalid"),
                ));
            }
            if let Some(object) = value.as_object_mut() {
                object.remove(key);
            }
        }
    }
    let options = serde_json::from_value(value).map_err(|error| {
        PluginError::new(
            "invalid-options",
            messages::message("linux.invalid_options", &[("error", error.to_string())]),
        )
    })?;
    let recommend = serde_json::from_value(task.recommend.clone()).map_err(|error| {
        PluginError::new(
            "invalid-recommend",
            messages::message("linux.invalid_recommend", &[("error", error.to_string())]),
        )
    })?;
    Ok((options, recommend))
}
#[cfg(target_os = "linux")]
fn invalid_source() -> PluginError {
    PluginError::new(
        "invalid-cache-source",
        messages::text("linux.invalid_source"),
    )
}
