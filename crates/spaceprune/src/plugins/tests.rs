use super::*;
use crate::api::{
    CancellationToken, ConfigLoader, NullProgress, OperationContext, ScanRequest, SpacePrune,
};
use spaceprune_plugin_host::PluginHost;
use std::{fs, path::Path, sync::Arc};

fn policy(directory: &Path) -> PolicySnapshot {
    PolicySnapshot::load(Some(directory.join("plugins.toml"))).unwrap()
}

fn fixture(directory: &Path) -> std::path::PathBuf {
    let cache = directory.join("cache");
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join("disposable.bin"), [1_u8; 16]).unwrap();
    let recipe = directory.join("recipe.toml");
    fs::write(
        &recipe,
        format!(
            r#"schema-version = 2
name = "Plugin policy fixture"
[modules.cache]
name = "Cache"
[modules.cache.tasks.files]
name = "Disposable files"
handler = "builtin.filesystem/paths"
[modules.cache.tasks.files.options]
roots = [{{ path = {:?} }}]
files = true
directories = false
elevation = "never"
[modules.cache.tasks.files.recommend]
enabled = true
"#,
            cache.to_string_lossy()
        ),
    )
    .unwrap();
    recipe
}

fn loaded(recipe: &Path, policy: PolicySnapshot) -> SpacePrune {
    let mut config = ConfigLoader::new().load_path(recipe).unwrap();
    config.application_source = Some(recipe.with_file_name("config.toml"));
    SpacePrune::from_config_with_policy(None, config, &OperationContext::default(), policy).unwrap()
}

#[test]
fn catalog_matches_registered_builtin_handlers_without_executing_plugins() {
    let directory = tempfile::tempdir().unwrap();
    let entries = catalog(PluginHost::default().plugins(), &policy(directory.path()));
    assert_eq!(entries.len(), 4);
    let mut registry = spaceprune_engine::PluginRegistry::new();
    spaceprune_plugin_filesystem::register(&mut registry).unwrap();
    #[cfg(target_os = "linux")]
    spaceprune_plugin_snapper::register(&mut registry).unwrap();
    spaceprune_platform::register_handlers(&mut registry).unwrap();
    spaceprune_plugin_developer_tools::register(&mut registry).unwrap();
    let expected: std::collections::BTreeSet<_> = registry
        .descriptors()
        .into_iter()
        .map(|descriptor| descriptor.id)
        .collect();
    let actual: std::collections::BTreeSet<_> = entries
        .iter()
        .filter(|entry| entry.supported)
        .flat_map(|entry| entry.handlers.clone())
        .collect();
    assert_eq!(actual, expected);
    assert!(entries.iter().all(|entry| entry.enabled
        && entry.version == crate::VERSION
        && entry.installation.is_none()));
    assert!(!directory.path().join("plugins.toml").exists());

    let external = directory.path().join("external");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("plugin.toml"), "schema-version = 1\nid = 'test.metadata'\nname = 'Metadata'\nversion = '1.2.3'\nprotocol-version = 3\nexecutable = 'must-not-run'\nhandlers = ['cache']\n").unwrap();
    fs::write(external.join("must-not-run"), "not executable").unwrap();
    let host = PluginHost::discover_in(&[directory.path().to_owned()]).unwrap();
    let entries = catalog(host.plugins(), &policy(directory.path()));
    let external = entries
        .iter()
        .find(|entry| entry.id == "test.metadata")
        .unwrap();
    assert_eq!(external.handlers, ["test.metadata/cache"]);
    assert_eq!(external.installation.as_ref(), host.get("test.metadata"));
    let disabled = set_enabled(
        host.plugins(),
        &policy(directory.path()),
        "test.metadata",
        false,
    )
    .unwrap();
    assert!(!disabled.enabled);
}

#[test]
fn unknown_plugin_never_writes_policy_and_noop_keeps_sessions_current() {
    let directory = tempfile::tempdir().unwrap();
    let policy = policy(directory.path());
    assert!(
        set_enabled(
            PluginHost::default().plugins(),
            &policy,
            "missing.plugin",
            false
        )
        .is_err()
    );
    assert!(!directory.path().join("plugins.toml").exists());
    set_enabled(
        PluginHost::default().plugins(),
        &policy,
        "builtin.filesystem",
        true,
    )
    .unwrap();
    policy.check_current().unwrap();
    assert!(!directory.path().join("plugins.toml").exists());
    let disabled = policy.set_enabled("builtin.filesystem", false).unwrap();
    disabled.set_enabled("builtin.filesystem", false).unwrap();
    disabled.check_current().unwrap();
    assert!(matches!(
        policy.check_current(),
        Err(spaceprune_engine::EngineError::PluginPolicyChanged)
    ));
}

#[test]
fn simultaneous_preferences_preserve_both_changes() {
    let directory = tempfile::tempdir().unwrap();
    let policy = Arc::new(policy(directory.path()));
    let handles: Vec<_> = ["builtin.filesystem", "builtin.snapper"]
        .into_iter()
        .map(|id| {
            let policy = Arc::clone(&policy);
            std::thread::spawn(move || policy.set_enabled(id, false).unwrap())
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let persisted = PolicySnapshot::load(Some(directory.path().join("plugins.toml"))).unwrap();
    assert_eq!(
        persisted.disabled(),
        &std::collections::BTreeSet::from([
            "builtin.filesystem".to_owned(),
            "builtin.snapper".to_owned()
        ])
    );
}

#[test]
fn policy_change_blocks_loaded_scan_and_cleanup_then_reenable_requires_new_scan() {
    let directory = tempfile::tempdir().unwrap();
    let recipe = fixture(directory.path());
    let original = policy(directory.path());
    let core = loaded(&recipe, original.clone());
    let cancellation = CancellationToken::default();
    let scan = core
        .scan(&ScanRequest::default(), &NullProgress, &cancellation)
        .unwrap();
    let plan = scan.plan_recommended().unwrap();
    assert_eq!(plan.item_count(), 1);
    let disabled = original.set_enabled("builtin.filesystem", false).unwrap();
    assert!(matches!(
        core.scan(&ScanRequest::default(), &NullProgress, &cancellation),
        Err(spaceprune_engine::EngineError::PluginPolicyChanged)
    ));
    let refused = core.execute(&plan, &NullProgress, &cancellation);
    assert_eq!(refused.succeeded(), 0);
    assert!(
        refused.groups[0]
            .error
            .as_deref()
            .unwrap()
            .contains("plugin-policy-changed")
    );
    assert!(directory.path().join("cache/disposable.bin").exists());
    let off = loaded(&recipe, disabled.clone());
    assert!(off.config().modules["cache"].tasks["files"].enabled);
    assert!(
        off.scan(&ScanRequest::default(), &NullProgress, &cancellation)
            .unwrap()
            .modules[0]
            .tasks
            .is_empty()
    );
    let rejected_old_plan = off.execute(&plan, &NullProgress, &cancellation);
    assert_eq!(
        rejected_old_plan.groups[0].failure.as_ref().unwrap().code,
        "plugin-disabled"
    );
    let enabled = disabled.set_enabled("builtin.filesystem", true).unwrap();
    assert!(
        core.execute(&plan, &NullProgress, &cancellation)
            .has_errors()
    );
    let renewed = loaded(&recipe, enabled);
    let fresh = renewed
        .scan(&ScanRequest::default(), &NullProgress, &cancellation)
        .unwrap()
        .plan_recommended()
        .unwrap();
    assert_eq!(
        renewed
            .execute(&fresh, &NullProgress, &cancellation)
            .succeeded(),
        1
    );
    assert!(!directory.path().join("cache/disposable.bin").exists());
}

#[test]
fn disabled_dependency_does_not_block_remaining_tasks_and_explicit_selection_is_clear() {
    let directory = tempfile::tempdir().unwrap();
    let recipe = fixture(directory.path());
    let mut config = ConfigLoader::new().load_path(&recipe).unwrap();
    let mut disabled_task = config.modules["cache"].tasks["files"].clone();
    disabled_task.handler = "test.missing/cache".to_owned();
    config
        .modules
        .get_mut("cache")
        .unwrap()
        .tasks
        .insert("disabled".to_owned(), disabled_task);
    config
        .requires
        .plugins
        .insert("test.missing".to_owned(), ">=900.0.0".to_owned());
    let mut snapper = config.modules["cache"].tasks["files"].clone();
    snapper.handler = "builtin.snapper/snapshots".to_owned();
    config
        .modules
        .get_mut("cache")
        .unwrap()
        .tasks
        .insert("snapshots".to_owned(), snapper);
    config
        .requires
        .plugins
        .insert("builtin.snapper".to_owned(), ">=900.0.0".to_owned());
    let policy = policy(directory.path())
        .set_enabled("test.missing", false)
        .unwrap()
        .set_enabled("builtin.snapper", false)
        .unwrap();
    let core =
        SpacePrune::from_config_with_policy(None, config, &OperationContext::default(), policy)
            .unwrap();
    let scan = core
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &CancellationToken::default(),
        )
        .unwrap();
    assert_eq!(scan.modules[0].tasks.len(), 1);
    assert_eq!(scan.modules[0].tasks[0].task_id, "files");
    let request = ScanRequest {
        module: Some("cache".to_owned()),
        tasks: ["disabled".to_owned()].into(),
        ..Default::default()
    };
    assert!(
        matches!(core.scan(&request, &NullProgress, &CancellationToken::default()), Err(spaceprune_engine::EngineError::PluginDisabled(id)) if id == "test.missing")
    );
}

#[test]
fn malformed_policy_refuses_existing_execution_without_touching_files() {
    let directory = tempfile::tempdir().unwrap();
    let recipe = fixture(directory.path());
    let core = loaded(&recipe, policy(directory.path()));
    let scan = core
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &CancellationToken::default(),
        )
        .unwrap();
    fs::write(directory.path().join("plugins.toml"), "not a valid policy").unwrap();
    let report = core.execute(
        &scan.plan_recommended().unwrap(),
        &NullProgress,
        &CancellationToken::default(),
    );
    assert!(report.has_errors());
    assert!(directory.path().join("cache/disposable.bin").exists());
}

#[test]
fn direct_recipe_and_readonly_entrypoints_share_device_preferences() {
    const TEST_HOME: &str = "SPACEPRUNE_TEST_PLUGIN_HOME";
    if let Some(directory) = std::env::var_os(TEST_HOME) {
        let directory = std::path::PathBuf::from(directory);
        let recipe = fixture(&directory);
        let before = crate::api::ReadonlySpacePrune::load_recipe(&recipe).unwrap();
        set_plugin_enabled("builtin.filesystem", false).unwrap();
        assert!(matches!(
            before.scan(
                &ScanRequest::default(),
                &NullProgress,
                &CancellationToken::default()
            ),
            Err(spaceprune_engine::EngineError::PluginPolicyChanged)
        ));
        let ordinary = SpacePrune::load_recipe(&recipe).unwrap();
        let readonly = crate::api::ReadonlySpacePrune::load_recipe(&recipe).unwrap();
        for scan in [
            ordinary
                .scan(
                    &ScanRequest::default(),
                    &NullProgress,
                    &CancellationToken::default(),
                )
                .unwrap(),
            readonly
                .scan(
                    &ScanRequest::default(),
                    &NullProgress,
                    &CancellationToken::default(),
                )
                .unwrap(),
        ] {
            assert!(scan.modules[0].tasks.is_empty());
        }
        assert!(ordinary.config().modules["cache"].tasks["files"].enabled);
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "plugins::tests::direct_recipe_and_readonly_entrypoints_share_device_preferences",
            "--nocapture",
        ])
        .env(TEST_HOME, directory.path())
        .env("HOME", directory.path())
        .env("USERPROFILE", directory.path())
        .env("XDG_CONFIG_HOME", directory.path().join("config"))
        .env("APPDATA", directory.path().join("config"))
        .env("XDG_DATA_HOME", directory.path().join("data"))
        .env("LOCALAPPDATA", directory.path().join("data"))
        .env("PROGRAMDATA", directory.path().join("system"));
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
