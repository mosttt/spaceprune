use std::fs;

use tempfile::tempdir;

use super::PluginHost;

#[test]
fn discovers_valid_manifest() {
    let temp = tempdir().unwrap();
    let directory = temp.path().join("example");
    fs::create_dir(&directory).unwrap();
    fs::write(
        directory.join("plugin.toml"),
        r#"schema-version = 1
id = "community.example"
name = "Example"
version = "1.0.0"
protocol-version = 3
executable = "example"
handlers = ["cache"]
"#,
    )
    .unwrap();
    fs::write(directory.join("example"), []).unwrap();
    let host = PluginHost::discover_in(&[temp.path().to_path_buf()]).unwrap();
    let plugin: &spaceprune_contracts::plugin::InstalledPlugin =
        host.get("community.example").unwrap();
    let manifest: super::PluginManifest = plugin.manifest.clone();
    assert!(manifest.platforms.is_empty());
    assert_eq!(
        serde_json::to_value(plugin).unwrap(),
        serde_json::json!({
            "manifest": {
                "schema-version": 1, "id": "community.example", "name": "Example",
                "version": "1.0.0", "protocol-version": 3, "executable": "example",
                "platforms": [], "handlers": ["cache"]
            },
            "manifest_path": fs::canonicalize(directory.join("plugin.toml")).unwrap(),
            "executable": fs::canonicalize(directory.join("example")).unwrap(),
        })
    );
}

#[test]
fn manifest_reexport_keeps_unknown_field_rejection() {
    let source = r#"schema-version = 1
id = "community.example"
name = "Example"
version = "1.0.0"
protocol-version = 3
executable = "example"
handlers = ["cache"]
unexpected = true
"#;
    let error = toml::from_str::<super::PluginManifest>(source).unwrap_err();
    assert!(error.to_string().contains("unknown field `unexpected`"));
}

#[test]
fn rejects_executable_outside_plugin_directory() {
    let temp = tempdir().unwrap();
    let directory = temp.path().join("example");
    fs::create_dir(&directory).unwrap();
    fs::write(temp.path().join("outside"), []).unwrap();
    fs::write(
        directory.join("plugin.toml"),
        r#"schema-version = 1
id = "community.example"
name = "Example"
version = "1.0.0"
protocol-version = 3
executable = "../outside"
handlers = ["cache"]
platforms = []
"#,
    )
    .unwrap();
    assert!(PluginHost::discover_in(&[temp.path().to_path_buf()]).is_err());
}

#[test]
fn rejects_duplicate_handlers_and_invalid_versions() {
    let temp = tempdir().unwrap();
    let directory = temp.path().join("example");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("example"), []).unwrap();
    fs::write(
        directory.join("plugin.toml"),
        r#"schema-version = 1
id = "community.example"
name = "Example"
version = "latest"
protocol-version = 3
executable = "example"
handlers = ["cache", "cache"]
platforms = []
"#,
    )
    .unwrap();
    assert!(PluginHost::discover_in(&[temp.path().to_path_buf()]).is_err());
}

#[cfg(unix)]
#[test]
fn exchanges_versioned_json_lines_with_a_plugin() {
    use std::os::unix::fs::PermissionsExt;

    use serde_json::json;
    use spaceprune_engine::{NullProgress, OperationContext, PluginRegistry};
    use spaceprune_plugin_api::TaskRequest;

    let temp = tempdir().unwrap();
    let directory = temp.path().join("example");
    fs::create_dir(&directory).unwrap();
    fs::write(
        directory.join("plugin.toml"),
        r#"schema-version = 1
id = "community.example"
name = "Example"
version = "1.0.0"
protocol-version = 3
executable = "plugin"
handlers = ["cache"]
platforms = []
"#,
    )
    .unwrap();
    let executable = directory.join("plugin");
    fs::write(
        &executable,
        r#"#!/bin/sh
IFS= read -r request
printf '%s\n' '{"protocol":3,"id":1,"result":{"handlers":[{"id":"community.example/cache","name":"Cache","description":"test","platforms":[],"options_schema":null,"recommend_schema":null}]}}'
IFS= read -r request
printf '%s\n' '{"protocol":3,"id":2,"result":null}'
IFS= read -r request
printf '%s\n' '{"protocol":3,"id":3,"result":{"items":[],"issues":[]}}'
IFS= read -r request
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&executable, permissions).unwrap();

    let host = PluginHost::discover_in(&[temp.path().to_path_buf()]).unwrap();
    let mut registry = PluginRegistry::new();
    host.register_all(&mut registry).unwrap();
    let handler = registry.get("community.example/cache").unwrap();
    let task = TaskRequest {
        module_id: "example".to_owned(),
        task_id: "cache".to_owned(),
        name: "Cache".to_owned(),
        handler_id: "community.example/cache".to_owned(),
        options: json!({}),
        recommend: json!({}),
        tags: Vec::new(),
    };
    handler.validate(&task).unwrap();
    let result = handler
        .scan(&task, &NullProgress, &OperationContext::default())
        .unwrap();
    assert!(result.items.is_empty());
}

#[test]
fn catalog_keeps_unsupported_metadata_out_of_executable_discovery() {
    let temp = tempdir().unwrap();
    let directory = temp.path().join("unsupported");
    fs::create_dir(&directory).unwrap();
    let platform = if spaceprune_config::current_platform() == "windows" {
        "linux"
    } else {
        "windows"
    };
    fs::write(directory.join("plugin.toml"), format!("schema-version = 1\nid = 'test.unsupported'\nname = 'Other platform'\nversion = '1.0.0'\nprotocol-version = 3\nexecutable = 'must-not-run'\nhandlers = ['cache']\nplatforms = ['{platform}']\n")).unwrap();
    fs::write(directory.join("must-not-run"), "not executable").unwrap();
    assert!(
        PluginHost::discover_in(&[temp.path().to_owned()])
            .unwrap()
            .plugins()
            .is_empty()
    );
    let catalog = PluginHost::discover_metadata_paths(
        &[temp.path().to_owned()],
        &spaceprune_engine::OperationContext::default(),
        false,
    )
    .unwrap();
    assert_eq!(catalog.plugins().len(), 1);
    assert_eq!(
        catalog.get("test.unsupported").unwrap().manifest.platforms,
        [platform]
    );
}

#[test]
fn reserved_external_ids_remain_metadata_but_never_register_handlers() {
    use spaceprune_contracts::plugin::BUILTIN_PLUGIN_IDS;
    let temp = tempdir().unwrap();
    for id in BUILTIN_PLUGIN_IDS {
        let directory = temp.path().join(id);
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("plugin.toml"), format!("schema-version = 1\nid = '{id}'\nname = 'Existing conflict'\nversion = '9.8.7'\nprotocol-version = 3\nexecutable = 'must-not-run'\nhandlers = ['paths']\n")).unwrap();
        fs::write(directory.join("must-not-run"), "not executable").unwrap();
    }
    let host = PluginHost::discover_in(&[temp.path().to_owned()]).unwrap();
    assert_eq!(host.plugins().len(), BUILTIN_PLUGIN_IDS.len());
    let mut registry = spaceprune_engine::PluginRegistry::new();
    host.register_all(&mut registry).unwrap();
    assert!(registry.descriptors().is_empty());
    let required = BUILTIN_PLUGIN_IDS.into_iter().map(str::to_owned).collect();
    host.register_required(&mut registry, &required).unwrap();
    assert!(registry.descriptors().is_empty());
}

#[test]
fn removal_prefers_damaged_canonical_package_then_one_matching_user_child() {
    use spaceprune_contracts::plugin::FILESYSTEM_PLUGIN_ID;
    let temp = tempdir().unwrap();
    let root = temp.path().join("user-plugins");
    fs::create_dir(&root).unwrap();
    let damaged = root.join(FILESYSTEM_PLUGIN_ID);
    fs::create_dir(&damaged).unwrap();
    fs::write(damaged.join("plugin.toml"), "broken manifest").unwrap();
    for name in ["a-manual-folder", "z-duplicate-folder"] {
        let directory = root.join(name);
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("plugin.toml"), format!("schema-version = 1\nid = '{FILESYSTEM_PLUGIN_ID}'\nname = 'Existing conflict'\nversion = '9.8.7'\nprotocol-version = 3\nexecutable = 'must-not-run'\nhandlers = ['paths']\n")).unwrap();
        fs::write(directory.join("must-not-run"), "not executable").unwrap();
    }
    let outside = temp.path().join("outside-user-root");
    fs::create_dir(&outside).unwrap();
    fs::copy(
        root.join("a-manual-folder/plugin.toml"),
        outside.join("plugin.toml"),
    )
    .unwrap();
    fs::write(outside.join("must-not-run"), "not executable").unwrap();
    super::installation::remove_from_user_root(&root, FILESYSTEM_PLUGIN_ID).unwrap();
    assert!(!damaged.exists());
    assert!(root.join("a-manual-folder").exists());
    super::installation::remove_from_user_root(&root, FILESYSTEM_PLUGIN_ID).unwrap();
    assert!(!root.join("a-manual-folder").exists());
    assert!(root.join("z-duplicate-folder").exists());
    super::installation::remove_from_user_root(&root, FILESYSTEM_PLUGIN_ID).unwrap();
    assert!(!root.join("z-duplicate-folder").exists());
    assert!(outside.join("plugin.toml").exists());
    assert!(matches!(
        super::installation::remove_from_user_root(&root, FILESYSTEM_PLUGIN_ID),
        Err(super::HostError::NotInstalled(_))
    ));
}
