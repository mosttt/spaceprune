//! Public JSON schemas generated from configuration, bundle and operation observation contracts.

use schemars::{JsonSchema, schema::Schema};
use serde_json::Value;

/// The single supported persistent configuration version, including physical include files.
pub fn configuration_version_schema(_: &mut schemars::r#gen::SchemaGenerator) -> Schema {
    serde_json::from_value(
        serde_json::json!({"type": "integer", "const": crate::config::SCHEMA_VERSION}),
    )
    .expect("static version schema")
}

/// Scope variants supported by the registered filesystem handler.
pub fn filesystem_scope_schema(_: &mut schemars::r#gen::SchemaGenerator) -> Schema {
    use crate::rule::RuleScope;
    serde_json::from_value(serde_json::json!({"type": "string", "enum": [RuleScope::DirectoryChildren, RuleScope::DirectorySubtree, RuleScope::ExactFile]}))
        .expect("static filesystem scope schema")
}

fn schema<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("contract schemas contain only JSON data")
}

/// Named public artifacts. No private AI or provider contract is included.
pub fn artifacts() -> [(&'static str, Value); 12] {
    [
        (
            "recipe.schema.json",
            schema::<crate::config::RecipeDefinition>(),
        ),
        (
            "filesystem-root.schema.json",
            schema::<crate::rule::FilesystemRoot>(),
        ),
        (
            "recipe-bundle.schema.json",
            schema::<crate::bundle::RecipeBundle>(),
        ),
        (
            "storage-volume.schema.json",
            schema::<crate::storage::StorageVolume>(),
        ),
        (
            "scan-request.schema.json",
            schema::<crate::report::ScanRequest>(),
        ),
        (
            "scan-report.schema.json",
            schema::<crate::report::ScanReport>(),
        ),
        (
            "cleanup-plan.schema.json",
            schema::<crate::report::CleanupPlan>(),
        ),
        (
            "execution-report.schema.json",
            schema::<crate::report::ExecutionReport>(),
        ),
        (
            "cleanup-size-summary.schema.json",
            schema::<crate::size_summary::CleanupSizeSummary>(),
        ),
        (
            "installed-plugin.schema.json",
            schema::<crate::plugin::InstalledPlugin>(),
        ),
        (
            "plugin-catalog-entry.schema.json",
            schema::<crate::plugin::PluginCatalogEntry>(),
        ),
        (
            "progress-event.schema.json",
            schema::<crate::handler::ProgressEvent>(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn committed_artifacts_match_authoritative_types() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
        for (name, generated) in super::artifacts() {
            let committed: serde_json::Value =
                serde_json::from_slice(&std::fs::read(directory.join(name)).unwrap()).unwrap();
            assert_eq!(
                committed, generated,
                "Regenerate public contract schemas: {name}"
            );
        }
    }
}
