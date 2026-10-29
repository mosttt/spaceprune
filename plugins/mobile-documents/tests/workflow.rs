//! Engine-level tests with an explicit in-memory document provider, not an Android emulation.

use spaceprune_engine::{Engine, EngineBuilder, ItemKey, PluginRegistry, ScanReport, ScanRequest};
use spaceprune_plugin_api::{
    ItemExecutionStatus, NullProgress, OperationContext, PluginError, PluginResult, Recommendation,
};
use spaceprune_plugin_mobile_documents::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

const ROOT: &str = "opaque:tree/root";
const RECIPE: &str = r#"
schema-version = 2
name = "Authorized old logs"
[requires.plugins]
"builtin.mobile-documents" = ">=0.1.3"
[modules.documents]
name = "Documents"
[modules.documents.tasks.logs]
name = "Old logs"
handler = "builtin.mobile-documents/documents"
[modules.documents.tasks.logs.options]
resource = "work-documents"
recursive = true
include-names = ["*.log"]
exclude-names = ["keep-*", "protected"]
[modules.documents.tasks.logs.recommend]
older-than-days = 1
min-size-bytes = 0
"#;

struct MemoryProvider(Mutex<Memory>);
struct Memory {
    binding: ResourceBinding,
    documents: BTreeMap<String, Document>,
    visited: Vec<String>,
    deleted: Vec<String>,
    uncertain: Option<String>,
    cancel_after_delete: bool,
}
impl MemoryProvider {
    fn new(documents: Vec<Document>) -> Arc<Self> {
        Arc::new(Self(Mutex::new(Memory {
            binding: ResourceBinding {
                slot: "work-documents".into(),
                revision: "grant-1".into(),
                root_id: ROOT.into(),
                name: "Work documents".into(),
                readable: true,
                writable: true,
            },
            documents: documents.into_iter().map(|d| (d.id.clone(), d)).collect(),
            visited: vec![],
            deleted: vec![],
            uncertain: None,
            cancel_after_delete: false,
        })))
    }
}
fn error() -> PluginError {
    PluginError::new("fixture-access-refused", "Fixture grant unavailable")
}
fn check(
    memory: &Memory,
    binding: &ResourceBinding,
    context: &OperationContext,
) -> PluginResult<()> {
    context.check().map_err(|_| error())?;
    if memory.binding != *binding || !memory.binding.readable {
        return Err(error());
    }
    Ok(())
}
impl DocumentProvider for MemoryProvider {
    fn binding(&self, slot: &str, context: &OperationContext) -> PluginResult<ResourceBinding> {
        context.check().map_err(|_| error())?;
        let state = self.0.lock().unwrap();
        if slot != state.binding.slot {
            return Err(error());
        }
        Ok(state.binding.clone())
    }
    fn children(
        &self,
        binding: &ResourceBinding,
        parent_id: &str,
        cursor: Option<&str>,
        context: &OperationContext,
    ) -> PluginResult<DocumentPage> {
        let mut state = self.0.lock().unwrap();
        check(&state, binding, context)?;
        state.visited.push(parent_id.into());
        let entries = state
            .documents
            .values()
            .filter(|d| d.parent_id == parent_id)
            .cloned()
            .collect::<Vec<_>>();
        let offset = cursor.map(|s| s.parse::<usize>().unwrap()).unwrap_or(0);
        let documents = entries
            .iter()
            .skip(offset)
            .take(2)
            .cloned()
            .collect::<Vec<_>>();
        let next = (offset + documents.len() < entries.len())
            .then(|| (offset + documents.len()).to_string());
        Ok(DocumentPage { documents, next })
    }
    fn inspect(
        &self,
        binding: &ResourceBinding,
        document_id: &str,
        context: &OperationContext,
    ) -> PluginResult<Option<Document>> {
        let state = self.0.lock().unwrap();
        check(&state, binding, context)?;
        Ok(state.documents.get(document_id).cloned())
    }
    fn delete_file(
        &self,
        binding: &ResourceBinding,
        expected: &Document,
        ancestors: &[Document],
        context: &OperationContext,
    ) -> DeleteResult {
        let mut state = self.0.lock().unwrap();
        if check(&state, binding, context).is_err() || !state.binding.writable {
            return DeleteResult::Failed {
                message: "Fixture grant revoked before dispatch".into(),
            };
        }
        let mut parent = ROOT.to_owned();
        for expected_parent in ancestors {
            let Some(actual) = state.documents.get(&expected_parent.id) else {
                return DeleteResult::Failed {
                    message: "Fixture ancestor absent".into(),
                };
            };
            if actual.parent_id != parent
                || actual.kind != DocumentKind::Directory
                || actual.name != expected_parent.name
            {
                return DeleteResult::Failed {
                    message: "Fixture ancestry changed".into(),
                };
            }
            parent = actual.id.clone();
        }
        let Some(actual) = state.documents.get(&expected.id) else {
            return DeleteResult::AlreadyAbsent;
        };
        assert_eq!(
            actual.kind,
            DocumentKind::File,
            "the handler must never dispatch a directory"
        );
        assert_ne!(actual.id, ROOT);
        if actual != expected || actual.parent_id != parent || !actual.deletable {
            return DeleteResult::Failed {
                message: "Fixture metadata changed".into(),
            };
        }
        state.deleted.push(expected.id.clone());
        if state.uncertain.as_deref() == Some(&expected.id) {
            return DeleteResult::Uncertain {
                message: "Fixture lost the deletion receipt".into(),
            };
        }
        state.documents.remove(&expected.id);
        if state.cancel_after_delete {
            context.cancel();
        }
        DeleteResult::Deleted
    }
}
fn file(id: &str, name: &str, bytes: Option<u64>) -> Document {
    Document {
        id: id.into(),
        parent_id: ROOT.into(),
        name: name.into(),
        kind: DocumentKind::File,
        logical_bytes: bytes,
        modified_at: Some(1),
        deletable: true,
        version: None,
    }
}
fn directory(id: &str, name: &str) -> Document {
    Document {
        kind: DocumentKind::Directory,
        ..file(id, name, None)
    }
}
fn engine(provider: Arc<MemoryProvider>, recipe: &str) -> (Engine, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("recipe.toml");
    std::fs::write(&path, recipe).unwrap();
    let config = spaceprune_config::ConfigLoader::new()
        .load_path(&path)
        .unwrap();
    let mut registry = PluginRegistry::new();
    register(&mut registry, provider).unwrap();
    // The fixture deliberately runs the injected handler on the test host; production metadata is Android-only.
    (
        EngineBuilder::new(config)
            .with_registry(registry)
            .with_journal_directory(root.path().join("journal"))
            .build()
            .unwrap(),
        root,
    )
}
fn scan(engine: &Engine) -> ScanReport {
    let result = engine
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    assert!(!result.cancelled);
    assert!(!result.has_errors(), "{result:?}");
    result
}
fn selected(report: &ScanReport, names: &[&str]) -> BTreeSet<ItemKey> {
    report
        .items_with_keys()
        .into_iter()
        .filter(|(_, item)| names.contains(&item.name.as_str()))
        .map(|(key, _)| key)
        .collect()
}

#[test]
fn measurement_and_rule_explanations_preserve_unknown_metadata() {
    use spaceprune_plugin_api::{ProgressEvent, ProgressSink, ScanMeasurement, TaskRequest};
    #[derive(Default)]
    struct Progress(Mutex<Vec<ScanMeasurement>>);
    impl ProgressSink for Progress {
        fn emit(&self, _: ProgressEvent) {}
        fn record_scan(&self, _: &TaskRequest, value: ScanMeasurement) {
            self.0.lock().unwrap().push(value);
        }
    }
    let provider = MemoryProvider::new(vec![
        file("known", "known.log", Some(42)),
        file("unknown", "unknown.log", None),
        file("excluded", "keep-note.log", Some(999)),
    ]);
    let (engine, _root) = engine(
        provider,
        &RECIPE.replace("min-size-bytes = 0", "min-size-bytes = 10"),
    );
    let progress = Progress::default();
    let report = engine
        .scan(
            &ScanRequest::default(),
            &progress,
            &OperationContext::default(),
        )
        .unwrap();
    let snapshots = progress.0.lock().unwrap();
    assert!(
        snapshots
            .iter()
            .any(|s| s.checked_items == 1 && s.logical_bytes == 42)
    );
    let last = snapshots.last().unwrap();
    assert_eq!(
        (
            last.checked_items,
            last.logical_bytes,
            last.unknown_size_items
        ),
        (2, 42, 1)
    );
    let known = report
        .items_with_keys()
        .into_iter()
        .find(|(_, i)| i.name == "known.log")
        .unwrap()
        .1;
    assert_eq!(known.recommendation, Recommendation::Recommended);
    assert!(
        known.reason.contains("*.log")
            && known.reason.contains("42")
            && known.reason.contains("10")
    );
    let unknown = report
        .items_with_keys()
        .into_iter()
        .find(|(_, i)| i.name == "unknown.log")
        .unwrap()
        .1;
    assert_eq!(unknown.recommendation, Recommendation::Optional);
}

#[test]
fn engine_deletes_only_confirmed_files_and_consumes_all_plans_from_the_scan_once() {
    let provider = MemoryProvider::new(vec![
        file("opaque:1/a", "old.log", Some(42)),
        file("opaque:2/b", "other.log", Some(16)),
        file("opaque:3/c", "keep-important.log", Some(200)),
    ]);
    let (engine, root) = engine(provider.clone(), RECIPE);
    let report = scan(&engine);
    let plan = report
        .plan_selected(&selected(&report, &["old.log"]))
        .unwrap();
    let alternative = report
        .plan_selected(&selected(&report, &["other.log"]))
        .unwrap();
    let result = engine.execute(&plan, &NullProgress, &OperationContext::default());
    assert!(!result.has_errors(), "{result:?}");
    assert_eq!(result.succeeded(), 1);
    assert_eq!(
        result.groups[0].result.as_ref().unwrap().reclaimed_bytes,
        42
    );
    assert!(
        engine
            .execute(&alternative, &NullProgress, &OperationContext::default())
            .has_errors()
    );
    let state = provider.0.lock().unwrap();
    assert_eq!(state.deleted, ["opaque:1/a"]);
    assert!(state.documents.contains_key("opaque:2/b"));
    assert!(state.documents.contains_key("opaque:3/c"));
    drop(state);
    let journal = root
        .path()
        .join("journal")
        .join(format!("{}.jsonl", plan.id()));
    let recovered =
        spaceprune_engine::recover_execution(&spaceprune_engine::ExecutionJournalReference {
            plan_id: plan.id().into(),
            path: spaceprune_contracts::access::NativePath::from_path(&journal),
        })
        .unwrap();
    assert!(recovered.finished);
    assert_eq!(recovered.report, result);
}

#[test]
fn reusable_rules_find_new_files_and_exclusions_prune_directories_without_serializing_grants() {
    let mut nested = file("opaque:nested", "nested.log", Some(8));
    nested.parent_id = "opaque:folder".into();
    let mut protected = file("opaque:secret", "old.log", Some(9));
    protected.parent_id = "opaque:protected".into();
    let provider = MemoryProvider::new(vec![
        directory("opaque:folder", "logs"),
        nested,
        directory("opaque:protected", "protected"),
        protected,
    ]);
    let (engine, _root) = engine(provider.clone(), RECIPE);
    let before = scan(&engine);
    assert_eq!(before.selectable_keys().len(), 1);
    provider
        .0
        .lock()
        .unwrap()
        .documents
        .insert("new".into(), file("new", "new.log", Some(3)));
    let after = scan(&engine);
    assert_eq!(after.selectable_keys().len(), 2);
    assert!(
        !provider
            .0
            .lock()
            .unwrap()
            .visited
            .iter()
            .any(|id| id == "opaque:protected")
    );
    let rules: DocumentRules =
        serde_json::from_value(after.modules[0].tasks[0].request.options.clone()).unwrap();
    let serialized = toml::to_string(&rules).unwrap();
    assert!(serialized.contains("work-documents"));
    for forbidden in ["opaque:", "grant-1", "content://", "nested.log"] {
        assert!(!serialized.contains(forbidden));
    }
    let plan = after.plan_recommended().unwrap();
    let result = engine.execute(&plan, &NullProgress, &OperationContext::default());
    assert_eq!(result.succeeded(), 2, "{result:?}");
    assert!(
        provider
            .0
            .lock()
            .unwrap()
            .documents
            .contains_key("opaque:folder")
    );
}

#[test]
fn unknown_zero_and_readonly_are_distinct_and_directories_are_not_cleanup_items() {
    let mut readonly = file("readonly", "read-only.log", Some(4));
    readonly.deletable = false;
    let provider = MemoryProvider::new(vec![
        file("unknown", "unknown.log", None),
        file("zero", "zero.log", Some(0)),
        readonly,
        directory("empty", "empty"),
    ]);
    let (engine, _root) = engine(provider, RECIPE);
    let report = scan(&engine);
    let task = &report.modules[0].tasks[0];
    let unknown = task.items.iter().find(|i| i.name == "unknown.log").unwrap();
    let zero = task.items.iter().find(|i| i.name == "zero.log").unwrap();
    assert!(unknown.has_unknown_logical_size());
    assert_eq!(zero.logical_bytes, Some(0));
    assert!(!zero.has_unknown_logical_size());
    assert_eq!(task.unknown_logical_items(), 1);
    assert_eq!(report.selectable_keys().len(), 2);
    for item in task
        .items
        .iter()
        .filter(|i| i.name == "read-only.log" || i.tags.contains(&"document-directory".to_owned()))
    {
        assert_eq!(item.recommendation, Recommendation::Unavailable);
    }
    assert!(
        report
            .plan_selected(&selected(&report, &["read-only.log"]))
            .is_err()
    );
    let result = engine.execute(
        &report.plan_recommended().unwrap(),
        &NullProgress,
        &OperationContext::default(),
    );
    let cleaned = result.groups[0].result.as_ref().unwrap();
    assert_eq!(cleaned.succeeded, 2);
    assert_eq!(cleaned.reclaimed_bytes, 0);
    assert_eq!(cleaned.unknown_reclaimed_items, Some(1));
}

#[test]
fn rebind_revocation_and_file_changes_refuse_old_selections_before_native_dispatch() {
    for change in 0..3 {
        let provider = MemoryProvider::new(vec![file("id", "old.log", Some(42))]);
        let (engine, _root) = engine(provider.clone(), RECIPE);
        let plan = scan(&engine).plan_recommended().unwrap();
        {
            let mut state = provider.0.lock().unwrap();
            match change {
                0 => state.binding.revision = "grant-2".into(),
                1 => state.binding.readable = false,
                _ => state.documents.get_mut("id").unwrap().logical_bytes = Some(200),
            }
        }
        let result = engine.execute(&plan, &NullProgress, &OperationContext::default());
        assert!(result.has_errors());
        assert_eq!(result.succeeded(), 0);
        assert!(provider.0.lock().unwrap().deleted.is_empty());
    }
}

#[test]
fn renamed_excluded_ancestor_refuses_a_previously_confirmed_child() {
    let mut child = file("child", "old.log", Some(42));
    child.parent_id = "parent".into();
    let provider = MemoryProvider::new(vec![directory("parent", "logs"), child]);
    let (engine, _root) = engine(provider.clone(), RECIPE);
    let plan = scan(&engine).plan_recommended().unwrap();
    provider
        .0
        .lock()
        .unwrap()
        .documents
        .get_mut("parent")
        .unwrap()
        .name = "protected".into();
    let result = engine.execute(&plan, &NullProgress, &OperationContext::default());
    assert!(result.has_errors());
    assert!(provider.0.lock().unwrap().deleted.is_empty());
}

#[test]
fn uncertainty_stops_later_deletes_and_the_plan_cannot_replay() {
    let provider = MemoryProvider::new(vec![
        file("a", "a.log", Some(42)),
        file("b", "b.log", Some(12)),
    ]);
    let (engine, _root) = engine(provider.clone(), RECIPE);
    let plan = scan(&engine).plan_recommended().unwrap();
    provider.0.lock().unwrap().uncertain = Some("a".into());
    let context = OperationContext::default();
    let result = engine.execute(&plan, &NullProgress, &context);
    let outcomes = &result.groups[0].result.as_ref().unwrap().outcomes;
    assert!(result.cancelled);
    assert_eq!(outcomes[0].status, ItemExecutionStatus::Uncertain);
    assert_eq!(outcomes[1].status, ItemExecutionStatus::NotAttempted);
    assert!(
        engine
            .execute(&plan, &NullProgress, &OperationContext::default())
            .has_errors()
    );
    assert_eq!(provider.0.lock().unwrap().deleted, ["a"]);
}

#[test]
fn cancellation_preserves_completed_effects_and_never_dispatches_remaining_files() {
    let provider = MemoryProvider::new(vec![
        file("a", "a.log", Some(42)),
        file("b", "b.log", Some(12)),
    ]);
    let (engine, _root) = engine(provider.clone(), RECIPE);
    let plan = scan(&engine).plan_recommended().unwrap();
    provider.0.lock().unwrap().cancel_after_delete = true;
    let result = engine.execute(&plan, &NullProgress, &OperationContext::default());
    let outcomes = &result.groups[0].result.as_ref().unwrap().outcomes;
    assert!(result.cancelled);
    assert_eq!(outcomes[0].status, ItemExecutionStatus::Succeeded);
    assert_eq!(outcomes[0].reclaimed_estimate, Some(42));
    assert_eq!(outcomes[1].status, ItemExecutionStatus::NotAttempted);
    assert_eq!(provider.0.lock().unwrap().deleted, ["a"]);
}

#[test]
fn absence_is_zero_while_readonly_grant_and_missing_threshold_metadata_remain_visible() {
    let provider = MemoryProvider::new(vec![file("absent", "absent.log", Some(40))]);
    let (engine, _root) = engine(provider.clone(), RECIPE);
    let plan = scan(&engine).plan_recommended().unwrap();
    provider.0.lock().unwrap().documents.remove("absent");
    let result = engine.execute(&plan, &NullProgress, &OperationContext::default());
    let outcome = &result.groups[0].result.as_ref().unwrap().outcomes[0];
    assert_eq!(outcome.status, ItemExecutionStatus::AlreadyAbsent);
    assert_eq!(outcome.reclaimed_estimate, Some(0));
    assert!(provider.0.lock().unwrap().deleted.is_empty());
    let mut no_time = file("no-time", "unknown-time.log", None);
    no_time.modified_at = None;
    provider
        .0
        .lock()
        .unwrap()
        .documents
        .insert("no-time".into(), no_time);
    let missing = scan(&engine);
    assert_eq!(missing.recommended_keys().len(), 0);
    assert_eq!(missing.selectable_keys().len(), 1);
    provider.0.lock().unwrap().binding.writable = false;
    let readonly = scan(&engine);
    assert_eq!(readonly.selectable_keys().len(), 0);
    assert!(
        readonly
            .items_with_keys()
            .iter()
            .any(|(_, i)| i.name == "unknown-time.log")
    );
}
