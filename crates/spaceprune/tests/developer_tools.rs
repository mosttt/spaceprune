#![cfg_attr(not(target_os = "linux"), allow(missing_docs))]
#![cfg(target_os = "linux")]
//! Generated tool environments exercise both consumers' public facade without touching user caches.
use spaceprune::api::{
    CancellationToken, ConfigStore, NullProgress, OperationContext, ScanRequest, SpacePrune,
    discover_system_capabilities, set_plugin_enabled,
};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

#[test]
fn developer_lifecycle_discovers_reuses_and_respects_independent_enablement() {
    const ENV: &str = "SPACEPRUNE_DEVELOPER_FIXTURE";
    let Some(root) = std::env::var_os(ENV).map(PathBuf::from) else {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        for (program, body) in [
            ("cargo", "exit 91"),
            (
                "go",
                "case \"$1 $2\" in 'env GOMODCACHE') printf '%s\\n' \"$GOMODCACHE\";; 'env GOCACHE') printf '%s\\n' \"$GOCACHE\";; *) exit 91;; esac",
            ),
            (
                "npm",
                "if [ \"$*\" = 'config get cache --ignore-scripts --no-update-notifier' ]; then printf '%s\\n' \"$NPM_CACHE\"; else exit 91; fi",
            ),
        ] {
            let path = bin.join(program);
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "developer_lifecycle_discovers_reuses_and_respects_independent_enablement",
                "--nocapture",
            ])
            .env(ENV, root)
            .env("PATH", &bin)
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("CARGO_HOME", root.join("cargo"))
            .env("GOMODCACHE", root.join("go/mod"))
            .env("GOCACHE", root.join("go/build"))
            .env("NPM_CACHE", root.join("npm"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    };
    let caches = ["go/mod", "go/build", "cargo/registry/cache", "npm/_cacache"];
    let populate = || {
        for path in caches {
            fs::create_dir_all(root.join(path)).unwrap();
            fs::write(root.join(path).join("download"), b"cache").unwrap();
        }
    };
    populate();
    for path in [
        "cargo/credentials.toml",
        "cargo/registry/src/keep",
        "npm/.npmrc",
        "npm/node_modules/keep",
    ] {
        fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        fs::write(root.join(path), b"keep").unwrap();
    }
    let legacy_inventory =
        spaceprune_platform::discover_system_tasks(&OperationContext::default()).unwrap();
    assert_eq!(legacy_inventory.len(), 1);
    assert_eq!(
        legacy_inventory[0].handler_id,
        "builtin.system-linux/go-modcache"
    );
    set_plugin_enabled("builtin.system-linux", false).unwrap();
    let tasks = discover_system_capabilities(&OperationContext::default()).unwrap();
    assert_eq!(tasks.len(), 4);
    assert!(tasks.iter().all(
        |task| task.handler_id.starts_with("builtin.developer-tools/")
            && task.recommend["enabled"] == false
    ));
    let config = root.join("recipe-config.toml");
    let store = ConfigStore::new(&config);
    let mut recipe = "schema-version = 2\nname = 'Developer fixture'\nusage = 'recurring'\n[modules.developer]\nname = 'Developer'\n".to_owned();
    for task in tasks {
        recipe.push_str(&format!("[modules.developer.tasks.{}]\nname = {:?}\nhandler = {:?}\n[modules.developer.tasks.{}.options]\nelevation = 'never'\n",task.task_id,task.name,task.handler_id,task.task_id));
    }
    store.upsert_recipe("developer", &recipe).unwrap();
    store.save_recipe("developer").unwrap();
    for _ in 0..2 {
        let application = SpacePrune::load(Some(&config)).unwrap();
        let scan = application
            .scan(
                &ScanRequest::default(),
                &NullProgress,
                &CancellationToken::default(),
            )
            .unwrap();
        assert!(!scan.has_errors());
        let plan = scan.plan_recommended().unwrap();
        assert_eq!(plan.item_count(), 4);
        let result = application.execute(&plan, &NullProgress, &CancellationToken::default());
        assert!(!result.has_errors(), "{result:?}");
        assert_eq!(result.succeeded(), 4);
        assert!(caches.iter().all(|path| !root.join(path).exists()));
        populate();
    }
    // Removing only npm's native interface must skip that cache without touching its bytes
    // or hiding independent Go/Cargo capabilities.
    fs::rename(root.join("bin/npm"), root.join("npm-disabled")).unwrap();
    let available = discover_system_capabilities(&OperationContext::default()).unwrap();
    assert_eq!(available.len(), 3);
    let without_npm = SpacePrune::load(Some(&config))
        .unwrap()
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &CancellationToken::default(),
        )
        .unwrap();
    let npm = without_npm
        .modules
        .iter()
        .flat_map(|module| &module.tasks)
        .find(|task| task.task_id == "npm-cache")
        .unwrap();
    assert_eq!(
        npm.items[0].recommendation,
        spaceprune::api::Recommendation::Unavailable
    );
    assert_eq!(npm.items[0].logical_bytes, None);
    assert!(npm.issues.is_empty());
    assert_eq!(
        fs::read(root.join("npm/_cacache/download")).unwrap(),
        b"cache"
    );
    set_plugin_enabled("builtin.developer-tools", false).unwrap();
    assert!(
        discover_system_capabilities(&OperationContext::default())
            .unwrap()
            .is_empty()
    );
    let disabled = SpacePrune::load(Some(&config)).unwrap();
    let scan = disabled
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &CancellationToken::default(),
        )
        .unwrap();
    assert_eq!(scan.plan_recommended().unwrap().item_count(), 0);
    // Existing system-linux Go recipes retain their original contract and enablement.
    set_plugin_enabled("builtin.system-linux", true).unwrap();
    store.upsert_recipe("legacy", "schema-version=2\nname='Legacy Go'\n[modules.go]\nname='Go'\n[modules.go.tasks.cache]\nname='Go cache'\nhandler='builtin.system-linux/go-modcache'\n[modules.go.tasks.cache.options]\nmanager='auto'\nvacuum-time='30d'\nelevation='never'\n").unwrap();
    store.save_recipe("legacy").unwrap();
    let legacy = SpacePrune::load(Some(&config)).unwrap();
    let scan = legacy
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &CancellationToken::default(),
        )
        .unwrap();
    assert_eq!(scan.plan_recommended().unwrap().item_count(), 1);
    for path in [
        "cargo/credentials.toml",
        "cargo/registry/src/keep",
        "npm/.npmrc",
        "npm/node_modules/keep",
    ] {
        assert_eq!(fs::read(root.join(path)).unwrap(), b"keep");
    }
}
