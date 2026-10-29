#![forbid(unsafe_code)]
//! Reusable user-authorized document rules, independent of URI encoding and native UI.

mod clean;
mod provider;
mod rules;
mod scan;

pub use provider::{
    DeleteResult, Document, DocumentKind, DocumentPage, DocumentProvider, MAX_PAGE_ENTRIES,
    ResourceBinding,
};
pub use rules::{DocumentRecommendation, DocumentRules};
use serde::{Deserialize, Serialize};
use spaceprune_contracts::plugin::{MOBILE_DOCUMENTS_PLUGIN_ID, PluginCatalogEntry, PluginKind};
use spaceprune_engine::{EngineError, PluginRegistry};
use spaceprune_plugin_api::*;
use std::sync::Arc;

/// Complete handler identity used by Recipe tasks.
pub const HANDLER_ID: &str = "builtin.mobile-documents/documents";

/// Register the bundled capability with an actual host-owned native document service.
/// No URI access, external plugin discovery or desktop elevation is installed implicitly.
pub fn register(
    registry: &mut PluginRegistry,
    provider: Arc<dyn DocumentProvider>,
) -> Result<(), EngineError> {
    registry.register(Arc::new(DocumentsHandler { provider }))
}

/// Built-in catalog metadata for mobile hosts; desktop catalogs need not advertise this capability.
pub fn catalog_entry(enabled: bool) -> PluginCatalogEntry {
    PluginCatalogEntry {
        id: MOBILE_DOCUMENTS_PLUGIN_ID.into(),
        name: spaceprune_plugin_api::messages::text("mobile_documents.name").into(),
        description: spaceprune_plugin_api::messages::text("mobile_documents.description").into(),
        version: env!("CARGO_PKG_VERSION").into(),
        kind: PluginKind::Builtin,
        platforms: vec!["android".into()],
        supported: cfg!(target_os = "android"),
        conflicts_with_builtin: false,
        enabled,
        handlers: vec![HANDLER_ID.into()],
        installation: None,
    }
}

struct DocumentsHandler {
    provider: Arc<dyn DocumentProvider>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    binding: ResourceBinding,
    document: Document,
    // Observed ancestor metadata preserves exclusion and membership checks without path parsing.
    ancestors: Vec<Document>,
}

impl Handler for DocumentsHandler {
    fn descriptor(&self) -> HandlerDescriptor {
        let catalog = catalog_entry(true);
        HandlerDescriptor {
            id: HANDLER_ID.into(),
            name: catalog.name,
            description: catalog.description,
            platforms: catalog.platforms,
            size_accounting: CleanupSizeAccounting::ProcessedBytes,
            options_schema: serde_json::to_value(schemars::schema_for!(DocumentRules))
                .expect("rule schema"),
            recommend_schema: serde_json::to_value(schemars::schema_for!(DocumentRecommendation))
                .expect("recommendation schema"),
        }
    }
    fn validate(&self, task: &TaskRequest) -> PluginResult<()> {
        rules::CompiledRules::parse(task).map(|_| ())
    }
    fn scan(
        &self,
        task: &TaskRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult> {
        scan::scan(self.provider.as_ref(), task, progress, context)
    }
    fn clean(
        &self,
        task: &TaskRequest,
        items: &[CleanupItem],
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerCleanResult> {
        clean::clean(self.provider.as_ref(), task, items, progress, context)
    }
}

fn check(context: &OperationContext) -> PluginResult<()> {
    context
        .check()
        .map_err(|e| PluginError::new("operation-stopped", e.to_string()))
}
fn binding(
    provider: &dyn DocumentProvider,
    slot: &str,
    context: &OperationContext,
) -> PluginResult<ResourceBinding> {
    check(context)?;
    let binding = provider.binding(slot, context)?;
    if binding.slot != slot || binding.revision.is_empty() || binding.root_id.is_empty() {
        return Err(PluginError::new(
            "mobile-documents-binding-invalid",
            spaceprune_plugin_api::messages::text("mobile_documents.binding_invalid"),
        ));
    }
    if !binding.readable {
        return Err(PluginError::new(
            "mobile-documents-access-revoked",
            spaceprune_plugin_api::messages::text("mobile_documents.access_revoked"),
        ));
    }
    Ok(binding)
}
fn emit(
    progress: &dyn ProgressSink,
    task: &TaskRequest,
    phase: ProgressPhase,
    current: usize,
    total: Option<usize>,
    message: String,
) {
    progress.emit(ProgressEvent {
        phase,
        module_id: Some(task.module_id.clone()),
        task_id: Some(task.task_id.clone()),
        handler_id: Some(task.handler_id.clone()),
        current: Some(current),
        total,
        message,
    });
}
