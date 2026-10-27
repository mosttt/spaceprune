//! SpacePrune CLI 最终接口的集成测试。

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use tempfile::tempdir;

fn spaceprune() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spaceprune"));
    command.env("LC_ALL", "en_US.UTF-8");
    if cfg!(target_os = "windows") {
        command.env_remove("APPDATA").env_remove("USERPROFILE");
    } else {
        command.env_remove("XDG_CONFIG_HOME").env_remove("HOME");
    }
    command
}

fn system_text(english: &'static str, chinese: &'static str) -> &'static str {
    if cfg!(any(target_os = "windows", target_os = "macos"))
        && spaceprune::api::LanguagePreference::System.resolve()
            == spaceprune::api::Language::SimplifiedChinese
    {
        chinese
    } else {
        english
    }
}

fn spaceprune_with_home(directory: &Path) -> Command {
    let mut command = spaceprune();
    if cfg!(target_os = "windows") {
        command.env("USERPROFILE", directory).env_remove("APPDATA");
    } else {
        command.env("HOME", directory).env_remove("XDG_CONFIG_HOME");
    }
    command
}

fn default_config_directory(directory: &Path) -> PathBuf {
    if cfg!(target_os = "windows") {
        directory.join("AppData/Roaming/SpacePrune")
    } else if cfg!(any(target_os = "macos", target_os = "ios")) {
        directory.join("Library/Application Support/SpacePrune")
    } else {
        directory.join(".config/spaceprune")
    }
}

fn stored_config_path(directory: &Path) -> PathBuf {
    default_config_directory(directory).join("config.toml")
}

fn write_recipe(directory: &Path, id: &str, source: &str) -> PathBuf {
    let recipe_directory = directory.join("recipes").join(id);
    fs::create_dir_all(&recipe_directory).unwrap();
    let path = recipe_directory.join(format!("{id}.toml"));
    fs::write(&path, source).unwrap();
    path
}

fn write_app_config(directory: &Path, id: &str, recipe: &str) -> PathBuf {
    fs::create_dir_all(directory).unwrap();
    write_recipe(directory, id, recipe);
    let path = directory.join("config.toml");
    fs::write(&path, format!("schema-version = 2\nrecipe = \"{id}\"\n")).unwrap();
    path
}

#[test]
fn help_only_contains_final_interface() {
    let output = spaceprune().arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("--scan"));
    assert!(stdout.contains("--clean"));
    assert!(stdout.contains("--select"));
    assert!(stdout.contains("spaceprune scan [--module <id>]"));
    assert!(stdout.contains("spaceprune clean --yes [--module <id>]"));
    assert!(stdout.contains("spaceprune module list [--all]"));
    assert!(stdout.contains("spaceprune recipe [list|set <recipe-id>]"));
    assert!(stdout.contains("spaceprune theme [list|set <theme-id>]"));
    assert!(!stdout.contains("spaceprune prompt"));
    assert!(!stdout.contains("--delete"));
    assert!(!stdout.contains("cargo-clean"));
}

#[test]
fn root_help_does_not_load_an_explicit_configuration() {
    let output = spaceprune()
        .args(["--config", "missing-config.toml", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains(system_text("Usage: spaceprune", "用法：spaceprune"))
    );
}

#[test]
fn automation_help_does_not_load_configuration() {
    let output = spaceprune()
        .args(["--config", "missing-config.toml", "automation", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("spaceprune automation <list|install|uninstall|run>")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn automation_install_persists_the_cli_runner_and_application_config() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempdir().unwrap();
    let config = write_app_config(
        temp.path(),
        "automation-test",
        r#"schema-version = 2
name = "Automation test"

[modules.cache]
name = "Cache"

[automations.daily]
name = "Daily cleanup"
modules = ["cache"]
at = "03:00"
"#,
    );
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let systemctl = bin.join("systemctl");
    fs::write(
        &systemctl,
        r#"#!/bin/sh
case "$*" in
    "--user show spaceprune-daily.timer "*)
        printf 'LoadState=not-found\nActiveState=inactive\nUnitFileState=\n'
        ;;
    "--user daemon-reload"|"--user enable spaceprune-daily.timer"|"--user restart spaceprune-daily.timer") ;;
    *) exit 90 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&systemctl, fs::Permissions::from_mode(0o755)).unwrap();
    let xdg = temp.path().join("xdg");

    let output = spaceprune()
        .env("HOME", temp.path())
        .env("XDG_CONFIG_HOME", &xdg)
        .env("PATH", &bin)
        .args(["--config", config.to_str().unwrap()])
        .args(["automation", "install", "daily", "--output=json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let service = fs::read_to_string(xdg.join("systemd/user/spaceprune-daily.service")).unwrap();
    let runner = std::path::absolute(env!("CARGO_BIN_EXE_spaceprune")).unwrap();
    let config = config.canonicalize().unwrap();
    assert!(service.contains(&format!("ExecStart=\"{}\"", runner.display())));
    assert!(service.contains(&format!(" --config \"{}\"", config.display())));
    assert!(service.contains(" --no-progress automation run daily"));
    assert!(!service.contains("recipes/automation-test/automation-test.toml --no-progress"));
}

#[test]
fn module_help_does_not_load_configuration() {
    let output = spaceprune()
        .args(["--config", "missing-config.toml", "module", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("module list [--all]"));
}

#[test]
fn theme_help_does_not_load_configuration() {
    let temp = tempdir().unwrap();
    let output = spaceprune_with_home(temp.path())
        .args(["--config", "missing-config.toml", "theme", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("spaceprune theme\n"));
}

#[test]
fn recipe_help_does_not_load_configuration() {
    let temp = tempdir().unwrap();
    let output = spaceprune_with_home(temp.path())
        .args(["--config", "missing-config.toml", "recipe", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("spaceprune recipe set <recipe-id>"));
}

#[test]
fn missing_user_config_directory_uses_the_default_theme() {
    let output = spaceprune()
        .args(["theme", "list", "--output=json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let themes: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(themes[0]["id"], "aurora");
    assert_eq!(themes[0]["active"], true);
}

#[cfg(not(target_os = "windows"))]
#[test]
fn xdg_config_home_is_the_preferred_new_location() {
    let temp = tempdir().unwrap();
    let xdg = temp.path().join("xdg");
    let output = spaceprune()
        .env("HOME", temp.path().join("home"))
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["theme", "set", "orbit", "--output=json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(xdg.join("spaceprune/config.toml").is_file());
    assert!(
        xdg.join("spaceprune/recipes/default/default.toml")
            .is_file()
    );
}

#[cfg(not(target_os = "windows"))]
#[test]
fn an_existing_fallback_config_wins_over_an_empty_xdg_location() {
    let temp = tempdir().unwrap();
    let home = temp.path().join("home");
    let directory = default_config_directory(&home);
    write_app_config(
        &directory,
        "personal",
        "schema-version = 2\nname = 'Personal'\n",
    );

    let output = spaceprune()
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", temp.path().join("empty-xdg"))
        .args(["recipe", "list", "--output=json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let recipes: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(recipes[0]["id"], "personal");
    assert_eq!(recipes[0]["active"], true);
}

#[test]
fn theme_set_persists_and_list_reports_the_active_theme() {
    let temp = tempdir().unwrap();
    let output = spaceprune_with_home(temp.path())
        .args(["theme", "set", "orbit", "--output=json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["theme"], "orbit");
    assert!(
        fs::read_to_string(stored_config_path(temp.path()))
            .unwrap()
            .contains("[theme]\nid = \"orbit\"")
    );

    let output = spaceprune_with_home(temp.path())
        .args(["theme", "list", "--output=json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let themes: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let active = themes
        .as_array()
        .unwrap()
        .iter()
        .find(|theme| theme["active"] == true)
        .unwrap();
    assert_eq!(active["id"], "orbit");
    assert_eq!(themes.as_array().unwrap().len(), 4);

    let output = spaceprune_with_home(temp.path())
        .args(["module", "list", "--output=json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let modules: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(modules[0]["id"], "cache");
}

#[test]
fn theme_set_updates_an_explicit_config_and_preserves_it() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("config.toml");
    fs::write(
        &config,
        r#"# Keep this comment
schema-version = 2
recipe = "personal"

[theme]
id = "nord"
"#,
    )
    .unwrap();

    let output = spaceprune()
        .arg("--config")
        .arg(&config)
        .args(["theme", "set", "graphite", "--output=json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let saved = fs::read_to_string(config).unwrap();
    assert!(saved.contains("# Keep this comment"));
    assert!(saved.contains("recipe = \"personal\""));
    assert!(saved.contains("[theme]\nid = \"graphite\""));
}

#[test]
fn legacy_theme_does_not_block_cli_startup_or_rewrite_preferences() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let original = "# Personal settings\nschema-version = 2\nrecipe = 'personal'\n[theme]\nid = 'tokyo-night'\n";
    fs::write(&config, original).unwrap();

    let output = spaceprune()
        .arg("--config")
        .arg(&config)
        .args(["theme", "list", "--output=json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let themes: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(themes.as_array().unwrap().len(), 4);
    assert_eq!(themes[0]["id"], "aurora");
    assert_eq!(themes[0]["active"], true);
    assert_eq!(fs::read_to_string(&config).unwrap(), original);

    let rejected = spaceprune()
        .arg("--config")
        .arg(&config)
        .args(["theme", "set", "dracula", "--output=json"])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
}

#[test]
fn interactive_theme_picker_requires_a_terminal() {
    let temp = tempdir().unwrap();
    let output = spaceprune_with_home(temp.path())
        .arg("theme")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(system_text(
            "Interactive theme selection requires a TTY",
            "交互选择主题需要终端"
        ))
    );
}

#[test]
fn recipe_list_and_set_use_the_id_named_layout() {
    let temp = tempdir().unwrap();
    let directory = default_config_directory(temp.path());
    write_app_config(
        &directory,
        "personal",
        "schema-version = 2\nname = 'Personal'\n",
    );
    write_recipe(&directory, "work", "schema-version = 2\nname = 'Work'\n");

    let output = spaceprune_with_home(temp.path())
        .args(["recipe", "list", "--output=json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let recipes: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(recipes.as_array().unwrap().len(), 2);
    assert_eq!(recipes[0]["id"], "personal");
    assert_eq!(recipes[0]["active"], true);
    assert_eq!(recipes[1]["id"], "work");

    let output = spaceprune_with_home(temp.path())
        .args(["recipe", "set", "work", "--output=json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["recipe"], "work");
    assert!(
        fs::read_to_string(directory.join("config.toml"))
            .unwrap()
            .contains("recipe = \"work\"")
    );
}

#[test]
fn interactive_recipe_picker_requires_a_terminal() {
    let temp = tempdir().unwrap();
    let directory = default_config_directory(temp.path());
    write_app_config(
        &directory,
        "personal",
        "schema-version = 2\nname = 'Personal'\n",
    );
    let output = spaceprune_with_home(temp.path())
        .arg("recipe")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(system_text(
            "Interactive recipe selection requires a TTY",
            "交互选择方案需要终端"
        ))
    );
}

#[test]
fn module_list_filters_disabled_modules_unless_all_is_requested() {
    let temp = tempdir().unwrap();
    let config = write_app_config(
        temp.path(),
        "module-list-test",
        r#"schema-version = 2
name = "module-list-test"

[modules.active]
name = "Active"
description = "Enabled module"
order = 20

[modules.inactive]
name = "Inactive"
description = "Disabled module"
enabled = false
order = 10
"#,
    );

    let output = spaceprune()
        .args(["--config", config.to_str().unwrap(), "module", "list"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("active"));
    assert!(!stdout.contains("inactive"));

    let output = spaceprune()
        .args([
            "--config",
            config.to_str().unwrap(),
            "module",
            "list",
            "--all",
            "--output=json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let modules: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(modules.as_array().unwrap().len(), 2);
    assert_eq!(modules[0]["id"], "inactive");
    assert_eq!(modules[0]["enabled"], false);
    assert_eq!(modules[1]["id"], "active");
    assert_eq!(modules[1]["enabled"], true);
}

#[test]
fn plugin_list_groups_by_origin_and_keeps_platforms_separate() {
    let home = tempdir().unwrap();
    let config = stored_config_path(home.path());
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    for (language, headings) in [
        (
            "en",
            [
                "Built-in plugins",
                "External plugins",
                "Platforms",
                "Cross-platform",
            ],
        ),
        ("zh-CN", ["内置插件", "外部插件", "适用平台", "跨平台"]),
    ] {
        let contents =
            format!("schema-version = 2\nlanguage = \"{language}\"\nrecipe = \"missing\"\n");
        fs::write(&config, &contents).unwrap();
        let output = spaceprune_with_home(home.path())
            .env("XDG_DATA_HOME", home.path().join("data"))
            .args(["plugin", "list"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        for heading in headings {
            assert!(text.contains(heading), "{text}");
        }
        let builtin = text.find(headings[0]).unwrap();
        let external = text.find(headings[1]).unwrap();
        for id in [
            "builtin.filesystem",
            "builtin.snapper",
            "builtin.system-linux",
        ] {
            let position = text.find(id).unwrap();
            assert!(builtin < position && position < external, "{text}");
        }
        assert!(text.contains("Linux"));
        assert_eq!(fs::read_to_string(&config).unwrap(), contents);
    }
}

#[test]
fn plugin_json_matches_public_metadata_without_starting_the_executable() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("spaceprune/plugins");
    let directory = root.join("metadata fixture");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("not-an-executable"), "must not run").unwrap();
    fs::write(
        directory.join("plugin.toml"),
        r#"schema-version = 1
id = "test.metadata-contract"
name = "Metadata contract 插件"
version = "1.2.3"
protocol-version = 3
executable = "not-an-executable"
handlers = ["cache", "profiles"]
"#,
    )
    .unwrap();
    let host = spaceprune::api::PluginHost::discover_in(&[root]).unwrap();
    let plugin: &spaceprune::api::InstalledPlugin = host.get("test.metadata-contract").unwrap();
    let expected = serde_json::to_value(plugin).unwrap();
    for args in [
        vec!["plugin", "list", "--output=json"],
        vec![
            "plugin",
            "inspect",
            "test.metadata-contract",
            "--output=json",
        ],
    ] {
        let output = spaceprune_with_home(temp.path())
            .env("XDG_DATA_HOME", temp.path())
            .env("LOCALAPPDATA", temp.path().join("local"))
            .env("PROGRAMDATA", temp.path().join("system"))
            .args(&args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let actual = if args[1] == "list" {
            value
                .as_array()
                .unwrap()
                .iter()
                .find(|plugin| plugin["manifest"]["id"] == "test.metadata-contract")
                .unwrap()
        } else {
            &value
        };
        assert_eq!(actual, &expected);
        assert_eq!(actual["manifest"]["platforms"], serde_json::json!([]));
    }
}

#[test]
fn custom_module_becomes_a_command_and_emits_json() {
    let temp = tempdir().unwrap();
    let cache = temp.path().join("cache");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join("data.bin"), [0_u8; 32]).unwrap();
    let config = write_app_config(
        temp.path(),
        "test",
        &format!(
            r#"schema-version = 2
name = "test"

[modules.personal]
name = "Personal cache"

[modules.personal.selectors.all]
name = "All personal data"
match-tags = ["personal"]

[modules.personal.tasks.cache]
name = "Cache"
handler = "builtin.filesystem/paths"
tags = ["personal"]

[modules.personal.tasks.cache.options]
roots = [{{ path = {:?} }}]
"#,
            cache.to_string_lossy()
        ),
    );

    let output = spaceprune()
        .args([
            "personal",
            "--all",
            "--config",
            config.to_str().unwrap(),
            "--scan",
        ])
        .args(["--output", "json", "--no-progress"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        output.stderr.is_empty(),
        "--no-progress must not emit scan progress"
    );
    assert_eq!(value["modules"][0]["id"], "personal");
    assert_eq!(
        value["modules"][0]["tasks"][0]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let output = spaceprune()
        .args([
            "--config",
            config.to_str().unwrap(),
            "scan",
            "--module",
            "personal",
            "--selector",
            "all",
            "--task",
            "cache",
            "--output",
            "json",
            "--no-progress",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["modules"][0]["id"], "personal");
}

#[test]
fn clean_yes_executes_the_recommended_plan() {
    let temp = tempdir().unwrap();
    let cache = temp.path().join("cache");
    fs::create_dir(&cache).unwrap();
    let garbage = cache.join("garbage.bin");
    fs::write(&garbage, [0_u8; 32]).unwrap();
    let config = write_app_config(
        temp.path(),
        "clean-test",
        &format!(
            r#"schema-version = 2
name = "clean-test"

[modules.personal]
name = "Personal cache"

[modules.personal.tasks.cache]
name = "Cache"
handler = "builtin.filesystem/paths"

[modules.personal.tasks.cache.options]
roots = [{{ path = {:?} }}]
files = true
directories = false

[modules.personal.tasks.cache.recommend]
enabled = true
"#,
            cache.to_string_lossy()
        ),
    );

    let output = spaceprune()
        .args([
            "--config",
            config.to_str().unwrap(),
            "clean",
            "--module",
            "personal",
            "--task",
            "cache",
        ])
        .args(["--yes", "--output", "json", "--no-progress"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(!garbage.exists());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["execution"]["groups"][0]["result"]["succeeded"], 1);
}

#[test]
fn select_requires_a_terminal() {
    let temp = tempdir().unwrap();
    let output = spaceprune_with_home(temp.path()).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(system_text(
            "Interactive selection requires a TTY",
            "交互选择需要终端"
        ))
    );
}

#[test]
fn removed_delete_option_is_rejected() {
    let output = spaceprune().args(["--delete", "--scan"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[cfg(target_os = "linux")]
#[test]
fn authorization_status_does_not_load_a_broken_application_or_recipe() {
    let temporary = tempdir().unwrap();
    let path = temporary.path().join("config.toml");
    fs::write(&path, "not valid TOML or a recipe").unwrap();
    let output = spaceprune()
        .arg("--config")
        .arg(&path)
        .args(["authorization", "status", "--output", "json"])
        .output()
        .unwrap();
    assert!(matches!(output.status.code(), Some(0 | 1)), "{:?}", output);
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        status["helperPath"],
        "/usr/libexec/spaceprune/native-6-4/spaceprune-readonly-helper"
    );
    assert!(status["status"].is_string());
    assert!(output.stderr.is_empty());
}

#[test]
fn authorization_help_is_available_without_an_active_recipe() {
    let output = spaceprune()
        .args(["authorization", "install", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("--graphical")
    );
}

#[test]
fn redirected_scan_and_cleanup_keep_json_parseable_and_never_emit_screen_controls() {
    let temporary = tempdir().unwrap();
    let cache = temporary.path().join("cache");
    fs::create_dir(&cache).unwrap();
    let selected = cache.join("generated.log");
    fs::write(&selected, b"generated test data").unwrap();
    let config = write_app_config(
        temporary.path(),
        "progress",
        &format!(
            r#"
schema-version = 2
name = "Progress output"
[modules.fixture]
name = "Fixture"
[modules.fixture.tasks.cache]
name = "Generated cache"
handler = "builtin.filesystem/paths"
[modules.fixture.tasks.cache.options]
roots = [{{ path = {:?}, scope = "directory_subtree" }}]
files = true
directories = false
elevation = "never"
[modules.fixture.tasks.cache.recommend]
older-than = "0s"
min-size = "0 B"
"#,
            cache.to_string_lossy()
        ),
    );
    fs::write(
        &config,
        "schema-version = 2\nrecipe = 'progress'\nlanguage = 'en'\n",
    )
    .unwrap();
    for disabled in [false, true] {
        let mut command = spaceprune();
        command
            .arg("--config")
            .arg(&config)
            .args(["--scan", "--output", "json"]);
        if disabled {
            command.arg("--no-progress");
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["modules"][0]["tasks"][0]["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(selected.exists());
        assert!(!output.stderr.contains(&0x1b));
        if disabled {
            assert!(output.stderr.is_empty());
        }
    }
    let output = spaceprune()
        .arg("--config")
        .arg(&config)
        .args(["--clean", "--yes", "--output", "json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["execution"]["groups"][0]["result"]["succeeded"], 1);
    assert!(!output.stderr.contains(&0x1b));
    assert!(!selected.exists());

    for disabled in [false, true] {
        fs::write(&selected, b"generated test data").unwrap();
        let mut command = spaceprune();
        command
            .arg("--config")
            .arg(&config)
            .args(["--clean", "--yes"]);
        if disabled {
            command.arg("--no-progress");
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(stdout.matches("Cleanup complete").count(), 1);
        assert!(!stderr.contains("Cleanup finished"));
        assert!(stdout.contains("Succeeded       1 of 1 attempted"));
        assert!(stdout.contains("Processed size  19 B"));
        assert!(!stdout.contains('\u{1b}'));
        assert!(!stderr.contains('\u{1b}'));
        if disabled {
            assert!(stderr.is_empty());
            assert!(!stdout.contains("Cleanup complete ·"));
        } else {
            assert!(stderr.contains("Scan finished"));
            assert!(stderr.contains("1 check · 1 item"));
            assert!(stdout.contains("Cleanup complete · 00:"));
        }
        assert!(!selected.exists());
    }
}

#[test]
fn plugin_catalog_and_enablement_apply_to_custom_recipe_without_starting_disabled_external() {
    let temp = tempdir().unwrap();
    let plugin = temp.path().join("spaceprune/plugins/fixture");
    fs::create_dir_all(&plugin).unwrap();
    fs::write(plugin.join("plugin.toml"), "schema-version = 1\nid = 'test.disabled'\nname = 'Disabled fixture'\nversion = '1.2.3'\nprotocol-version = 3\nexecutable = 'must-not-run'\nhandlers = ['cache']\n").unwrap();
    fs::write(plugin.join("must-not-run"), "not executable").unwrap();
    let unsupported = plugin.with_file_name("unsupported");
    fs::create_dir(&unsupported).unwrap();
    let other_platform = if cfg!(target_os = "windows") {
        "linux"
    } else {
        "windows"
    };
    fs::write(unsupported.join("plugin.toml"), format!("schema-version = 1\nid = 'test.unsupported'\nname = 'Other platform'\nversion = '1.0.0'\nprotocol-version = 3\nexecutable = 'must-not-run'\nhandlers = ['cache']\nplatforms = ['{other_platform}']\n")).unwrap();
    fs::write(unsupported.join("must-not-run"), "not executable").unwrap();
    let run = |args: &[&str]| {
        spaceprune_with_home(temp.path())
            .env("XDG_DATA_HOME", temp.path())
            .env("LOCALAPPDATA", temp.path().join("local"))
            .env("PROGRAMDATA", temp.path().join("system"))
            .args(args)
            .output()
            .unwrap()
    };
    let output = run(&["plugin", "catalog", "--output=json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let entries: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        entries
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == "builtin.filesystem" && entry["enabled"] == true)
    );
    let entry = entries
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == "test.disabled")
        .unwrap();
    assert_eq!(
        entry["handlers"],
        serde_json::json!(["test.disabled/cache"])
    );
    assert!(
        entries
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == "test.unsupported" && entry["supported"] == false)
    );
    let legacy = run(&["plugin", "list", "--output=json"]);
    let legacy: serde_json::Value = serde_json::from_slice(&legacy.stdout).unwrap();
    assert!(
        !legacy
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["manifest"]["id"] == "test.unsupported")
    );
    assert!(
        !run(&["plugin", "enable", "test.unsupported", "--output=json"])
            .status
            .success()
    );
    for id in ["test.disabled", "builtin.filesystem"] {
        let output = run(&["plugin", "disable", id, "--output=json"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let entry: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(entry["enabled"], false);
    }
    let unknown = run(&["plugin", "disable", "unknown.plugin", "--output=json"]);
    assert!(!unknown.status.success());
    let contents =
        fs::read_to_string(default_config_directory(temp.path()).join("plugins.toml")).unwrap();
    assert!(!contents.contains("unknown.plugin"));
    let cache = temp.path().join("cache");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join("keep.bin"), [7_u8; 16]).unwrap();
    let config = write_app_config(
        temp.path(),
        "plugin-test",
        &format!(
            r#"schema-version = 2
name = "Plugin preference test"
[requires.plugins]
"test.disabled" = "^1.2"
[modules.cache]
name = "Cache"
[modules.cache.tasks.files]
name = "Files"
handler = "builtin.filesystem/paths"
[modules.cache.tasks.files.options]
roots = [{{ path = {:?} }}]
elevation = "never"
[modules.cache.tasks.external]
name = "External"
handler = "test.disabled/cache"
"#,
            cache.to_string_lossy()
        ),
    );
    let scan = run(&[
        "--config",
        config.to_str().unwrap(),
        "scan",
        "--output=json",
        "--no-progress",
    ]);
    assert!(
        scan.status.success(),
        "{}",
        String::from_utf8_lossy(&scan.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&scan.stdout).unwrap();
    assert!(report["modules"][0]["tasks"].as_array().unwrap().is_empty());
    assert!(
        run(&["plugin", "enable", "builtin.filesystem", "--output=json"])
            .status
            .success()
    );
    let scan = run(&[
        "--config",
        config.to_str().unwrap(),
        "scan",
        "--output=json",
        "--no-progress",
    ]);
    assert!(
        scan.status.success(),
        "{}",
        String::from_utf8_lossy(&scan.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&scan.stdout).unwrap();
    assert_eq!(report["modules"][0]["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(
        report["modules"][0]["tasks"][0]["handler_id"],
        "builtin.filesystem/paths"
    );
    assert!(cache.join("keep.bin").exists());
}

#[test]
fn plugin_reserved_ids_are_rejected_and_existing_conflicts_remain_recoverable() {
    use spaceprune::api::BUILTIN_PLUGIN_IDS;
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("must-not-run"), "not executable").unwrap();
    let write_manifest = |id: &str| {
        fs::write(source.join("plugin.toml"), format!("schema-version = 1\nid = '{id}'\nname = 'Existing conflict'\nversion = '9.8.7'\nprotocol-version = 3\nexecutable = 'must-not-run'\nhandlers = ['paths']\n")).unwrap()
    };
    let data = temp.path().join("data");
    let installed = data.join("spaceprune/plugins");
    let run = |args: &[&str]| {
        spaceprune_with_home(temp.path())
            .env("XDG_DATA_HOME", &data)
            .env("XDG_STATE_HOME", temp.path().join("state"))
            .env("LOCALAPPDATA", temp.path().join("local"))
            .env("PROGRAMDATA", temp.path().join("system"))
            .args(args)
            .output()
            .unwrap()
    };
    for id in BUILTIN_PLUGIN_IDS {
        write_manifest(id);
        let rejected = run(&[
            "plugin",
            "install",
            source.to_str().unwrap(),
            "--output=json",
        ]);
        assert!(!rejected.status.success());
        assert!(
            String::from_utf8_lossy(&rejected.stderr)
                .contains("reserved for a built-in capability")
        );
        assert!(
            !installed.exists(),
            "reserved installation wrote into the destination"
        );
    }
    for id in BUILTIN_PLUGIN_IDS {
        write_manifest(id);
        let existing = installed.join(format!("legacy-{id}"));
        fs::create_dir_all(&existing).unwrap();
        fs::copy(source.join("plugin.toml"), existing.join("plugin.toml")).unwrap();
        fs::copy(source.join("must-not-run"), existing.join("must-not-run")).unwrap();
    }
    let output = run(&["plugin", "catalog", "--output=json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let entries: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for id in BUILTIN_PLUGIN_IDS {
        let matches: Vec<_> = entries
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["id"] == id)
            .collect();
        assert_eq!(matches.len(), 2);
        let builtin = matches
            .iter()
            .find(|entry| entry["kind"] == "builtin")
            .unwrap();
        assert_eq!(builtin["conflicts_with_builtin"], false);
        assert_eq!(builtin["enabled"], true);
        let conflict = matches
            .iter()
            .find(|entry| entry["kind"] == "external")
            .unwrap();
        assert_eq!(conflict["conflicts_with_builtin"], true);
        assert_eq!(conflict["enabled"], false);
        assert_eq!(conflict["supported"], true);
        if builtin["supported"] == true {
            let enabled = run(&["plugin", "enable", id, "--output=json"]);
            assert!(
                enabled.status.success(),
                "{}",
                String::from_utf8_lossy(&enabled.stderr)
            );
            let enabled: serde_json::Value = serde_json::from_slice(&enabled.stdout).unwrap();
            assert_eq!(enabled["kind"], "builtin");
            assert_eq!(enabled["conflicts_with_builtin"], false);
        }
    }
    let old_list = run(&["plugin", "list", "--output=json"]);
    assert!(old_list.status.success());
    let old_list: serde_json::Value = serde_json::from_slice(&old_list.stdout).unwrap();
    assert!(
        old_list
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["manifest"]["id"] == "builtin.filesystem")
    );
    assert!(
        old_list
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry.get("conflicts_with_builtin").is_none())
    );
    let cache = temp.path().join("cache");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join("keep.bin"), [1_u8; 16]).unwrap();
    let config = write_app_config(
        temp.path(),
        "builtin-test",
        &format!(
            r#"schema-version = 2
name = "Builtin conflict test"
[requires.plugins]
"builtin.filesystem" = "={}"
[modules.cache]
name = "Cache"
[modules.cache.tasks.files]
name = "Files"
handler = "builtin.filesystem/paths"
[modules.cache.tasks.files.options]
roots = [{{ path = {:?} }}]
elevation = "never"
"#,
            spaceprune::VERSION,
            cache.to_string_lossy()
        ),
    );
    let scan = run(&[
        "--config",
        config.to_str().unwrap(),
        "scan",
        "--output=json",
        "--no-progress",
    ]);
    assert!(
        scan.status.success(),
        "{}",
        String::from_utf8_lossy(&scan.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&scan.stdout).unwrap();
    assert_eq!(
        report["modules"][0]["tasks"][0]["handler_id"],
        "builtin.filesystem/paths"
    );
    assert_eq!(
        report["modules"][0]["tasks"][0]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(cache.join("keep.bin").exists());
    for id in BUILTIN_PLUGIN_IDS {
        assert!(
            run(&["plugin", "remove", id, "--output=json"])
                .status
                .success()
        );
        assert!(!installed.join(format!("legacy-{id}")).exists());
    }
    let recovered = run(&["plugin", "catalog", "--output=json"]);
    assert!(recovered.status.success());
    let recovered: serde_json::Value = serde_json::from_slice(&recovered.stdout).unwrap();
    assert!(
        recovered
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["conflicts_with_builtin"] == false)
    );
    let scan_after_removal = run(&[
        "--config",
        config.to_str().unwrap(),
        "scan",
        "--output=json",
        "--no-progress",
    ]);
    assert!(
        scan_after_removal.status.success(),
        "{}",
        String::from_utf8_lossy(&scan_after_removal.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&scan_after_removal.stdout).unwrap();
    assert_eq!(
        report["modules"][0]["tasks"][0]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(cache.join("keep.bin").exists());
    write_manifest("community.unreserved");
    assert!(
        run(&[
            "plugin",
            "install",
            source.to_str().unwrap(),
            "--output=json"
        ])
        .status
        .success()
    );
    let normal = run(&["plugin", "enable", "community.unreserved", "--output=json"]);
    assert!(normal.status.success());
    let normal: serde_json::Value = serde_json::from_slice(&normal.stdout).unwrap();
    assert_eq!(normal["kind"], "external");
    assert_eq!(normal["conflicts_with_builtin"], false);
    assert_eq!(normal["enabled"], true);
    assert!(
        run(&["plugin", "remove", "community.unreserved", "--output=json"])
            .status
            .success()
    );
}

#[test]
fn shared_language_preference_overrides_system_without_loading_recipe() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("config.toml");
    fs::write(
        &config,
        "# keep comment\nschema-version = 2\nrecipe = 'missing'\n[theme]\nid = 'graphite'\n",
    )
    .unwrap();
    let run = |args: &[&str], locale: &str| {
        spaceprune()
            .env("LC_ALL", locale)
            .args(["--config", config.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap()
    };
    let system = run(&["--help"], "zh_CN.UTF-8");
    assert!(system.status.success());
    let follows_native_locale = cfg!(any(target_os = "windows", target_os = "macos"));
    let system_help = if follows_native_locale {
        system_text(
            "Inspect or change the shared application language",
            "查看或更改共享应用语言",
        )
    } else {
        "查看或更改共享应用语言"
    };
    assert!(String::from_utf8_lossy(&system.stdout).contains(system_help));
    let saved = run(&["language", "set", "en"], "zh_CN.UTF-8");
    assert!(saved.status.success());
    let english = run(&["--help"], "zh_CN.UTF-8");
    assert!(
        String::from_utf8_lossy(&english.stdout)
            .contains("Inspect or change the shared application language")
    );
    let saved = run(&["language", "set", "zh-CN"], "en_US.UTF-8");
    assert!(saved.status.success());
    let invalid = run(&["language", "set", "fr"], "en_US.UTF-8");
    assert!(!invalid.status.success());
    let error = String::from_utf8_lossy(&invalid.stderr);
    assert!(error.contains("参数值无效"));
    assert!(error.contains("system, en, zh-CN"));
    let json = run(&["language", "--output", "json"], "en_US.UTF-8");
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"preference":"zh-CN","resolved":"zh-CN"})
    );
    assert!(!temp.path().join("recipes").exists());
    let contents = fs::read_to_string(config).unwrap();
    assert!(contents.contains("# keep comment"));
    assert!(contents.contains("recipe = 'missing'"));
    assert!(contents.contains("id = 'graphite'"));
}

#[test]
fn unknown_system_and_saved_languages_fall_back_to_english() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("config.toml");
    for language in [None, Some("future-language")] {
        // Native Windows/macOS language detection does not use POSIX locale variables.
        if language.is_none() && cfg!(any(target_os = "windows", target_os = "macos")) {
            continue;
        }
        let language = language
            .map(|id| format!("language = '{id}'\n"))
            .unwrap_or_default();
        fs::write(
            &config,
            format!("schema-version = 2\nrecipe = 'missing'\n{language}"),
        )
        .unwrap();
        let output = spaceprune()
            .env("LC_ALL", "fr_FR.UTF-8")
            .args(["--config", config.to_str().unwrap(), "--help"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("Configurable and extensible cleanup executor")
        );
    }
}

#[test]
fn chinese_product_reasons_do_not_change_json_or_user_content() {
    let temp = tempdir().unwrap();
    let data = temp.path().join("literal-{reason}-缓存");
    fs::create_dir(&data).unwrap();
    fs::write(data.join("用户内容.cache"), "preserved").unwrap();
    let config = write_app_config(
        temp.path(),
        "personal",
        &format!(
            r#"schema-version = 2
name = "User recipe — 保留名称"
[modules.personal]
name = "My cache — 用户名称"
[modules.personal.tasks.files]
name = "User task"
handler = "builtin.filesystem/paths"
[modules.personal.tasks.files.options]
roots = [{{ path = {:?}, scope = "directory_children" }}]
elevation = "never"
[modules.personal.tasks.files.recommend]
enabled = false
"#,
            data.to_string_lossy()
        ),
    );
    let mut source = fs::read_to_string(&config).unwrap();
    source.push_str("language = 'zh-CN'\n");
    fs::write(&config, source).unwrap();
    let run = |output: &str| {
        spaceprune()
            .env("LC_ALL", "en_US.UTF-8")
            .args([
                "--config",
                config.to_str().unwrap(),
                "scan",
                "--no-progress",
                "--output",
                output,
            ])
            .output()
            .unwrap()
    };
    let human = run("human");
    assert!(
        human.status.success(),
        "{}",
        String::from_utf8_lossy(&human.stderr)
    );
    let text = String::from_utf8(human.stdout).unwrap();
    assert!(text.contains("配置已禁用建议清理"));
    assert!(text.contains("My cache — 用户名称"));
    assert!(text.contains("literal-{reason}-缓存"));
    let machine = run("json");
    assert!(machine.status.success());
    let report: serde_json::Value = serde_json::from_slice(&machine.stdout).unwrap();
    assert_eq!(
        report["modules"][0]["tasks"][0]["items"][0]["reason"],
        "Recommended cleanup is disabled by configuration"
    );
    assert_eq!(
        fs::read_to_string(data.join("用户内容.cache")).unwrap(),
        "preserved"
    );
}

#[test]
fn language_list_is_readonly_and_keeps_system_separate_from_languages() {
    let temp = tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let source = "schema-version = 2\nrecipe = 'missing'\nlanguage = 'zh-CN'\n";
    fs::write(&config, source).unwrap();
    let output = spaceprune()
        .args([
            "--config",
            config.to_str().unwrap(),
            "language",
            "list",
            "--output",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result["languages"],
        serde_json::json!([{"id":"en","name":"English"},{"id":"zh-CN","name":"简体中文"}])
    );
    assert_eq!(result["modes"], serde_json::json!(["system"]));
    assert_eq!(result["preference"], "zh-CN");
    assert_eq!(fs::read_to_string(&config).unwrap(), source);
    fs::remove_file(&config).unwrap();
    let empty = spaceprune()
        .args(["--config", config.to_str().unwrap(), "language", "list"])
        .output()
        .unwrap();
    assert!(empty.status.success());
    assert!(!config.exists());
}

#[test]
fn management_json_queries_do_not_open_pickers_or_modify_preferences() {
    let home = tempdir().unwrap();
    let directory = default_config_directory(home.path());
    let config = write_app_config(
        &directory,
        "personal",
        "schema-version = 2\nname = \"Personal\"\n[modules]\n",
    );
    let contents = "# Keep preferences unchanged\nschema-version = 2\nlanguage = \"zh-CN\"\nrecipe = \"personal\"\n[theme]\nid = \"graphite\"\n";
    fs::write(&config, contents).unwrap();
    for command in ["recipe", "theme"] {
        let output = spaceprune_with_home(home.path())
            .args([command, "--output", "json"])
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        if command == "recipe" {
            assert_eq!(value["recipe"], "personal");
            assert_eq!(value["name"], "Personal");
            assert_eq!(
                Path::new(value["entry"].as_str().unwrap()),
                directory
                    .join("recipes")
                    .join("personal")
                    .join("personal.toml")
            );
        } else {
            assert_eq!(value["theme"], "graphite");
        }
        assert_eq!(value["path"], config.to_str().unwrap());
        assert_eq!(fs::read_to_string(&config).unwrap(), contents);
        assert!(
            !output.stderr.contains(&0x1b),
            "read-only queries must not switch terminal screens"
        );
    }
}

#[test]
fn subcommand_help_only_advertises_applicable_options() {
    let home = tempdir().unwrap();
    let config = stored_config_path(home.path());
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let paths: &[&[&str]] = &[
        &["recipe"],
        &["recipe", "list"],
        &["recipe", "set"],
        &["theme"],
        &["theme", "list"],
        &["theme", "set"],
        &["language"],
        &["language", "list"],
        &["language", "set"],
        &["module"],
        &["module", "list"],
        &["plugin"],
        &["plugin", "list"],
        &["plugin", "catalog"],
        &["plugin", "inspect"],
        &["plugin", "install"],
        &["plugin", "remove"],
        &["plugin", "enable"],
        &["plugin", "disable"],
        &["authorization"],
        &["authorization", "status"],
        &["authorization", "install"],
        &["automation"],
        &["automation", "list"],
        &["automation", "install"],
        &["automation", "uninstall"],
        &["automation", "run"],
        &["scan"],
        &["clean"],
    ];
    for language in ["en", "zh-CN"] {
        fs::write(
            &config,
            format!("schema-version = 2\nlanguage = \"{language}\"\nrecipe = \"missing\"\n"),
        )
        .unwrap();
        for path in paths {
            let output = spaceprune_with_home(home.path())
                .args(*path)
                .arg("--help")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{language} {path:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8(output.stdout).unwrap();
            let option = |name: &str| {
                text.lines().any(|line| {
                    let line = line.trim_start();
                    line.strip_prefix("-y, ")
                        .unwrap_or(line)
                        .split_whitespace()
                        .next()
                        == Some(name)
                })
            };
            for name in ["--scan", "--clean", "--select"] {
                assert!(!option(name), "{language} {path:?}: {text}");
            }
            assert_eq!(
                option("--yes"),
                path[0] == "clean",
                "{language} {path:?}: {text}"
            );
            let progress = matches!(path[0], "scan" | "clean") || *path == ["automation", "run"];
            assert_eq!(
                option("--no-progress"),
                progress,
                "{language} {path:?}: {text}"
            );
            assert_eq!(
                option("--config"),
                !matches!(path[0], "plugin" | "authorization"),
                "{language} {path:?}: {text}"
            );
            assert!(option("--output"), "{language} {path:?}: {text}");
        }
    }
}
