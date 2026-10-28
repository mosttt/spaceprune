use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde_json::json;
use spaceprune_config::ConfigLoader;
use spaceprune_engine::{EngineBuilder, NullProgress, OperationContext, ScanRequest};
use tempfile::tempdir;

#[test]
fn prunes_directories_that_cannot_match_a_recursive_glob() {
    let matcher = super::RootMatcher::compile(&super::RootOptions {
        scope: Default::default(),
        bindings: Vec::new(),
        path: "fixture-root".to_owned(),
        include: vec!["Desktop/*/target".to_owned()],
        exclude: Vec::new(),
    })
    .unwrap();

    assert!(matcher.may_contain_match(Path::new("Desktop")));
    assert!(matcher.may_contain_match(Path::new("Desktop/project")));
    assert!(!matcher.may_contain_match(Path::new("unrelated")));
    assert!(!matcher.may_contain_match(Path::new("Desktop/project/artifacts")));
    assert!(!matcher.may_contain_match(Path::new("Desktop/project/artifacts/private")));
    let matcher = super::RootMatcher::compile(&super::RootOptions {
        scope: Default::default(),
        bindings: Vec::new(),
        path: "fixture".into(),
        include: vec![
            "Projects/workspace/build/target".into(),
            "logs/**/*.log".into(),
        ],
        exclude: Vec::new(),
    })
    .unwrap();
    assert!(matcher.may_contain_match(Path::new("Projects")));
    assert!(matcher.may_contain_match(Path::new("Projects/workspace/build")));
    assert!(!matcher.may_contain_match(Path::new("Projects/workspace/other")));
    assert!(matcher.may_contain_match(Path::new("logs/one/two")));
}

fn engine(config: &str) -> (tempfile::TempDir, spaceprune_engine::Engine) {
    let temp = tempdir().unwrap();
    let config_file = temp.path().join("config.toml");
    fs::write(&config_file, config).unwrap();
    let config = ConfigLoader::new().load_path(config_file).unwrap();
    let mut registry = spaceprune_engine::PluginRegistry::new();
    super::register(&mut registry).unwrap();
    let engine = EngineBuilder::new(config)
        .with_registry(registry)
        .build()
        .unwrap();
    (temp, engine)
}

#[test]
fn runtime_ipc_directories_are_not_candidates_or_removed_as_descendants() {
    use spaceprune_contracts::rule::RuleScope;
    let files = tempdir().unwrap();
    for name in ["spaceprune-readonly-live", "spaceprune-cleanup-live"] {
        fs::create_dir(files.path().join(name)).unwrap();
        fs::write(files.path().join(name).join("lease"), b"active").unwrap();
    }
    fs::create_dir(files.path().join("ordinary-cache")).unwrap();
    fs::write(files.path().join("ordinary-cache/log"), b"removable").unwrap();
    let (_config, engine) = scoped_engine(files.path(), RuleScope::DirectoryChildren, vec![], true);
    let report = engine
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    let items = &report.modules[0].tasks[0].items;
    assert_eq!(items.len(), 1);
    assert!(items[0].location.ends_with("ordinary-cache"));
    let plan = report.plan_recommended().unwrap();
    // A new IPC directory inside an already selected subtree must also survive cleanup.
    let late = files.path().join("ordinary-cache/spaceprune-cleanup-later");
    fs::create_dir(&late).unwrap();
    fs::write(late.join("lease"), b"active").unwrap();
    let _ = engine.execute(&plan, &NullProgress, &OperationContext::default());
    assert!(!files.path().join("ordinary-cache/log").exists());
    for path in [
        files.path().join("spaceprune-readonly-live"),
        files.path().join("spaceprune-cleanup-live"),
        late,
    ] {
        assert_eq!(fs::read(path.join("lease")).unwrap(), b"active");
    }
}

#[test]
fn scans_include_and_exclude_globs() {
    let files = tempdir().unwrap();
    fs::create_dir_all(files.path().join("nested")).unwrap();
    fs::create_dir_all(files.path().join("private")).unwrap();
    fs::write(files.path().join("root.log"), [1_u8; 16]).unwrap();
    fs::write(files.path().join("root.tmp"), [1_u8; 16]).unwrap();
    fs::write(files.path().join("nested/child.log"), [1_u8; 16]).unwrap();
    fs::write(files.path().join("private/secret.log"), [1_u8; 16]).unwrap();
    let config = format!(
        r#"schema-version = 2
name = "test"
[modules.cache]
name = "Cache"
[modules.cache.tasks.files]
name = "Logs"
handler = "builtin.filesystem/paths"
[modules.cache.tasks.files.options]
roots = [{{ path = {}, include = ["*.log", "**/*.log"], exclude = ["private", "private/**"] }}]
"#,
        toml::Value::String(files.path().to_string_lossy().into_owned())
    );
    let (_config_dir, engine) = engine(&config);
    let report = engine
        .scan(
            &ScanRequest {
                module: Some("cache".to_owned()),
                selectors: BTreeSet::new(),
                tasks: BTreeSet::new(),
            },
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    let locations: BTreeSet<_> = report.modules[0].tasks[0]
        .items
        .iter()
        .map(|item| item.location.as_str())
        .collect();
    assert_eq!(locations.len(), 2);
    assert!(locations.iter().any(|path| path.ends_with("root.log")));
    assert!(locations.iter().any(|path| path.ends_with("child.log")));
}

#[test]
fn clean_preserves_excluded_descendants() {
    let files = tempdir().unwrap();
    fs::create_dir_all(files.path().join("cache/keep")).unwrap();
    fs::write(files.path().join("cache/remove.bin"), [1_u8; 16]).unwrap();
    fs::write(files.path().join("cache/keep/important.bin"), [1_u8; 16]).unwrap();
    let config = format!(
        r#"schema-version = 2
name = "test"
[modules.cache]
name = "Cache"
[modules.cache.tasks.files]
name = "Files"
handler = "builtin.filesystem/paths"
[modules.cache.tasks.files.options]
roots = [{{ path = {}, include = ["cache"], exclude = ["cache/keep", "cache/keep/**"] }}]
"#,
        toml::Value::String(files.path().to_string_lossy().into_owned())
    );
    let (_config_dir, engine) = engine(&config);
    let report = engine
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    let plan = report.plan_recommended().unwrap();
    let execution = engine.execute(&plan, &NullProgress, &OperationContext::default());
    assert!(!execution.has_errors());
    assert!(!files.path().join("cache/remove.bin").exists());
    assert!(files.path().join("cache/keep/important.bin").exists());
}

#[test]
fn clean_rejects_a_path_outside_the_configured_root() {
    let files = tempdir().unwrap();
    let root = files.path().join("cache");
    let outside = files.path().join("outside.bin");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("inside.bin"), [1_u8; 16]).unwrap();
    fs::write(&outside, [2_u8; 16]).unwrap();
    let config = format!(
        r#"schema-version = 2
name = "test"
[modules.cache]
name = "Cache"
[modules.cache.tasks.files]
name = "Files"
handler = "builtin.filesystem/paths"
[modules.cache.tasks.files.options]
roots = [{{ path = {} }}]
"#,
        toml::Value::String(root.to_string_lossy().into_owned())
    );
    let (_config_dir, engine) = engine(&config);
    let report = engine
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    let mut report = report;
    report.modules[0].tasks[0].items[0].opaque["path"] =
        json!(outside.to_string_lossy().into_owned());
    let plan = report.plan_recommended().unwrap();

    let execution = engine.execute(&plan, &NullProgress, &OperationContext::default());

    assert!(execution.has_errors());
    assert!(outside.exists());
    assert!(root.join("inside.bin").exists());
}

#[test]
fn cleanup_is_idempotent_when_the_root_disappears_after_scan() {
    let files = tempdir().unwrap();
    let cache = files.path().join("cache");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join("entry.bin"), [7_u8; 16]).unwrap();
    let config = format!(
        r#"schema-version = 2
name = "test"
[modules.cache]
name = "Cache"

[modules.cache.tasks.child]
name = "Child"
handler = "builtin.filesystem/paths"
[modules.cache.tasks.child.options]
roots = [{{ path = {}, include = ["*"] }}]
"#,
        toml::Value::String(cache.to_string_lossy().into_owned()),
    );
    let (_config_dir, engine) = engine(&config);
    let report = engine
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    let plan = report.plan_recommended().unwrap();
    assert_eq!(plan.item_count(), 1);
    fs::remove_dir_all(&cache).unwrap();

    let execution = engine.execute(&plan, &NullProgress, &OperationContext::default());

    assert!(!execution.has_errors());
    assert_eq!(execution.succeeded(), 1);
    assert_eq!(execution.reclaimed_bytes(), 0);
    assert_eq!(execution.unknown_reclaimed_items(), 0);
    assert_eq!(
        execution.groups[0]
            .result
            .as_ref()
            .unwrap()
            .unknown_reclaimed_items,
        Some(0)
    );
    assert!(!cache.exists());
}

#[test]
fn a_replaced_root_cannot_authorize_a_path_that_appears_later() {
    use spaceprune_access::filesystem::remove_selected;
    let files = tempdir().unwrap();
    let root = files.path().join("cache");
    fs::create_dir(&root).unwrap();
    let child = root.join("selected");
    fs::write(&child, b"original").unwrap();
    let root_identity = spaceprune_access::filesystem::read_identity(&root).unwrap();
    let identity = spaceprune_access::filesystem::read_identity(&child).unwrap();
    fs::rename(&root, files.path().join("previous")).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(&child, b"replacement").unwrap();
    let result = remove_selected(
        &root,
        &root_identity,
        Path::new("selected"),
        &identity,
        &|_| false,
        &OperationContext::default(),
    );
    assert_eq!(
        spaceprune_access::error::classify(result.error.as_ref().unwrap()).kind,
        spaceprune_access::IoFailureKind::TargetChanged
    );
    assert_eq!(fs::read(&child).unwrap(), b"replacement");
}

#[test]
fn missing_item_below_an_existing_root_is_an_idempotent_success() {
    use spaceprune_access::filesystem::remove_selected;
    let files = tempdir().unwrap();
    let child = files.path().join("already-gone.bin");
    fs::write(&child, b"original").unwrap();
    let root_identity = spaceprune_access::filesystem::read_identity(files.path()).unwrap();
    let identity = spaceprune_access::filesystem::read_identity(&child).unwrap();
    fs::remove_file(&child).unwrap();
    let result = remove_selected(
        files.path(),
        &root_identity,
        Path::new("already-gone.bin"),
        &identity,
        &|_| false,
        &OperationContext::default(),
    );
    assert!(result.error.is_none());
    assert!(result.already_absent);
    assert_eq!(result.reclaimed_estimate, 0);
}

fn scoped_engine(
    root: &Path,
    scope: spaceprune_contracts::rule::RuleScope,
    bindings: Vec<spaceprune_contracts::rule::ObjectBinding>,
    directories: bool,
) -> (tempfile::TempDir, spaceprune_engine::Engine) {
    let usage = if bindings.is_empty() {
        "recurring"
    } else {
        "one_shot"
    };
    let declaration = spaceprune_contracts::rule::FilesystemRoot {
        path: spaceprune_contracts::rule::RootSpec::Expression(root.to_str().unwrap().into()),
        scope,
        include: vec!["*".into()],
        exclude: vec!["child/keep.txt".into()],
        bindings,
    };
    let options = toml::Table::from_iter([
        (
            "roots".to_owned(),
            toml::Value::Array(vec![toml::Value::try_from(declaration).unwrap()]),
        ),
        ("files".to_owned(), toml::Value::Boolean(true)),
        ("directories".to_owned(), toml::Value::Boolean(directories)),
        (
            "elevation".to_owned(),
            toml::Value::String("never".to_owned()),
        ),
    ]);
    let mut source: toml::Table = toml::from_str(&format!(
        "schema-version = 2\nname = 'Scope fixture'\nusage = '{usage}'\n[modules.cache]\nname = 'Cache'\n[modules.cache.tasks.files]\nname = 'Files'\nhandler = 'builtin.filesystem/paths'\n"
    )).unwrap();
    source["modules"]["cache"]["tasks"]["files"]
        .as_table_mut()
        .unwrap()
        .insert("options".to_owned(), toml::Value::Table(options));
    engine(&toml::to_string(&source).unwrap())
}

fn binding(root: &Path, relative: &[&str]) -> spaceprune_contracts::rule::ObjectBinding {
    let path = relative
        .iter()
        .fold(root.to_path_buf(), |path, part| path.join(part));
    spaceprune_contracts::rule::ObjectBinding {
        relative: relative.iter().map(|part| (*part).to_owned()).collect(),
        host_id: spaceprune_access::host::identity().unwrap(),
        identity: spaceprune_access::filesystem::read_identity(&path).unwrap(),
    }
}

#[test]
fn one_shot_file_bindings_do_not_admit_new_siblings_from_broad_patterns() {
    use spaceprune_contracts::rule::RuleScope;
    let files = tempdir().unwrap();
    fs::write(files.path().join("selected.log"), b"selected").unwrap();
    fs::write(files.path().join("sibling.log"), b"keep").unwrap();
    let (_config, engine) = scoped_engine(
        files.path(),
        RuleScope::ExactFile,
        vec![binding(files.path(), &["selected.log"])],
        false,
    );
    for pass in 0..2 {
        if pass == 1 {
            fs::write(files.path().join("later.log"), b"new sibling").unwrap();
        }
        let report = engine
            .scan(
                &ScanRequest::default(),
                &NullProgress,
                &OperationContext::default(),
            )
            .unwrap();
        let items = &report.modules[0].tasks[0].items;
        assert_eq!(items.len(), 1);
        assert!(items[0].location.ends_with("selected.log"));
    }
    fs::rename(
        files.path().join("selected.log"),
        files.path().join("original.log"),
    )
    .unwrap();
    fs::create_dir(files.path().join("selected.log")).unwrap();
    fs::write(files.path().join("selected.log/retained"), b"do not expand").unwrap();
    let report = engine
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    assert!(report.modules[0].tasks[0].items.is_empty());
    assert!(!report.modules[0].tasks[0].issues.is_empty());
    assert!(files.path().join("selected.log/retained").exists());
}

#[test]
fn direct_children_keep_directory_selection_atomic_and_respect_nested_exclusions() {
    use spaceprune_contracts::rule::RuleScope;
    let files = tempdir().unwrap();
    fs::create_dir_all(files.path().join("child/nested")).unwrap();
    fs::write(files.path().join("top.txt"), b"top").unwrap();
    fs::write(files.path().join("child/nested/log.txt"), b"nested").unwrap();
    fs::write(files.path().join("child/keep.txt"), b"protected").unwrap();
    let (_config, engine) =
        scoped_engine(files.path(), RuleScope::DirectoryChildren, vec![], false);
    let report = engine
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    assert_eq!(report.modules[0].tasks[0].items.len(), 1);
    assert!(
        report.modules[0].tasks[0].items[0]
            .location
            .ends_with("top.txt")
    );
    let (_config, engine) = scoped_engine(files.path(), RuleScope::DirectoryChildren, vec![], true);
    let report = engine
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    let locations: BTreeSet<_> = report.modules[0].tasks[0]
        .items
        .iter()
        .map(|item| PathBuf::from(&item.location))
        .collect();
    assert_eq!(
        locations,
        BTreeSet::from([files.path().join("child"), files.path().join("top.txt")])
    );
    let plan = report.plan_recommended().unwrap();
    fs::write(files.path().join("child/later.log"), b"dynamic subtree").unwrap();
    fs::write(
        files.path().join("new-sibling.txt"),
        b"outside confirmation",
    )
    .unwrap();
    let cleaned = engine.execute(&plan, &NullProgress, &OperationContext::default());
    assert_eq!(cleaned.succeeded(), 2);
    assert_eq!(
        fs::read(files.path().join("child/keep.txt")).unwrap(),
        b"protected"
    );
    assert!(!files.path().join("child/nested/log.txt").exists());
    assert!(!files.path().join("child/later.log").exists());
    assert!(files.path().join("new-sibling.txt").exists());
}
