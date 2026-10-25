//! Verify platform selection through saved bundles, loading and actual filesystem scans.

use spaceprune::api::{
    CancellationToken, ConfigLoader, ConfigStore, NullProgress, RecipeBundle, ScanRequest,
    SpacePrune, current_platform,
};
use std::{collections::BTreeMap, fs, path::Path};

#[test]
fn saved_bundle_selects_active_sources_tasks_and_variable_overrides() {
    let directory = tempfile::tempdir().unwrap();
    let active = current_platform();
    let inactive = if active == "windows" {
        "linux"
    } else {
        "windows"
    };
    let cache = directory.path().join("platform-cache");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join("active.log"), b"local fixture").unwrap();
    let bundle = RecipeBundle {
        id: "portable".into(),
        files: BTreeMap::from([
            ("portable.toml".into(), format!(r#"schema-version = 2
name = 'Portable'
variable-map = 'variables.toml'
include = [
  {{ path = 'common.toml', platforms = [] }},
  {{ path = 'active.toml', platforms = ['{active}'] }},
  {{ path = '../missing/outside.toml', platforms = ['{inactive}'] }},
]
[modules.cache.tasks.inactive]
name = 'Do not resolve or assemble'
handler = 'absent.plugin/handler'
platforms = ['{inactive}']
[modules.cache.tasks.inactive.options]
path = '${{INACTIVE_UNDEFINED}}'
"#)),
            ("common.toml".into(), "schema-version = 2\n[modules.cache]\nname = 'Cache'\n[modules.cache.tasks.common]\nname = 'Any platform'\nhandler = 'builtin.filesystem/paths'\nplatforms = []\n[modules.cache.tasks.common.options]\nroots = [{ path = '${CACHE_HOME}', include = ['*.log'] }]\nfiles = true\ndirectories = false\nelevation = 'never'\n".into()),
            ("active.toml".into(), format!("schema-version = 2\n[modules.cache.tasks.local]\nname = 'This platform'\nhandler = 'builtin.filesystem/paths'\nplatforms = ['{active}']\n[modules.cache.tasks.local.options]\nroots = [{{ path = '${{CACHE_HOME}}', include = ['*.log'] }}]\nfiles = true\ndirectories = false\nelevation = 'never'\n")),
            ("variables.toml".into(), format!("schema-version = 2\n[variables.CACHE_HOME]\nvalue = 'unused-common'\n[platforms.{active}.variables.CACHE_HOME]\nvalue = {}\n[platforms.{inactive}.variables.CACHE_HOME]\nvalue = '${{UNDEFINED_ON_THIS_HOST}}'\n", serde_json::to_string(&cache).unwrap())),
            ("unreferenced.toml".into(), "not even a valid config; never included".into()),
        ]),
    };
    let store = ConfigStore::new(directory.path().join("config.toml"));
    store.commit_recipe_bundle(&bundle, None, true).unwrap();
    let application = SpacePrune::load(Some(store.path())).unwrap();
    let scan = application
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &CancellationToken::default(),
        )
        .unwrap();
    assert!(!scan.has_errors());
    let tasks = &scan.modules[0].tasks;
    assert_eq!(
        tasks
            .iter()
            .map(|task| task.task_id.as_str())
            .collect::<Vec<_>>(),
        ["common", "local"]
    );
    for task in tasks {
        assert!(task.issues.is_empty());
        assert_eq!(task.items.len(), 1);
        assert_eq!(Path::new(&task.items[0].location), cache.join("active.log"));
    }
    let mut explicit = ScanRequest::default();
    explicit.tasks.insert("inactive".into());
    assert!(
        application
            .scan(&explicit, &NullProgress, &CancellationToken::default())
            .is_err()
    );
    assert_eq!(store.export_recipe_bundle("portable").unwrap(), bundle);

    let entry = store.recipe_directory().join("portable/portable.toml");
    let (_, _, sources) = ConfigLoader::new().load_path_with_sources(entry).unwrap();
    assert_eq!(
        sources
            .iter()
            .map(|path| path.file_name().unwrap().to_str().unwrap())
            .collect::<Vec<_>>(),
        ["common.toml", "active.toml", "portable.toml"]
    );
}

#[test]
fn empty_task_platforms_still_require_a_valid_registered_handler() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid.toml");
    fs::write(&path, "schema-version = 2\nname = 'Invalid handler'\n[modules.cache]\nname = 'Cache'\n[modules.cache.tasks.invalid]\nname = 'Invalid'\nhandler = 'builtin.filesystem/missing'\nplatforms = []\n").unwrap();
    assert!(
        SpacePrune::load_recipe(&path)
            .unwrap_err()
            .to_string()
            .contains("not registered")
    );
    let invalid_options = fs::read_to_string(&path)
        .unwrap()
        .replace("builtin.filesystem/missing", "builtin.filesystem/paths")
        + "[modules.cache.tasks.invalid.options]\nunsupported-option = true\n";
    fs::write(&path, invalid_options).unwrap();
    assert!(
        SpacePrune::load_recipe(&path).is_err(),
        "empty platforms must not skip handler validation"
    );
}
