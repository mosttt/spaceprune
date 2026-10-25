#![cfg(target_os = "linux")]
//! Scheduler mutations are exercised only through an isolated command fixture.

use spaceprune_config::{ConfigLoader, ResolvedConfig};
use spaceprune_platform_linux::{
    PlatformError, automation_installed_checked, install_automation, uninstall_automation,
};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn config(root: &Path, at: &str) -> ResolvedConfig {
    let path = root.join("recipe.toml");
    fs::write(&path, format!("schema-version = 2\nname = 'Fixture'\n[automations.daily]\nname = 'Daily'\nmodules = []\nat = '{at}'\nenabled = true\n")).unwrap();
    ConfigLoader::new().load_path(&path).unwrap()
}

#[test]
fn systemd_mutation_failures_restore_definitions_and_effective_schedule() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let fake = bin.join("systemctl");
    fs::write(&fake, include_str!("fixtures/systemctl")).unwrap();
    fs::set_permissions(fake, fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "isolated_scheduler_child",
            "--nocapture",
        ])
        .env("SPACEPRUNE_AUTOMATION_TEST_ROOT", root.path())
        .env("HOME", root.path().join("home"))
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env(
            "PATH",
            std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            )))
            .unwrap(),
        )
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env_remove("XDG_RUNTIME_DIR")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "subprocess entrypoint called with a private scheduler fixture"]
fn isolated_scheduler_child() {
    let root =
        std::path::PathBuf::from(std::env::var_os("SPACEPRUNE_AUTOMATION_TEST_ROOT").unwrap());
    let runner = root.join("runner");
    fs::write(&runner, "fixture").unwrap();
    let units = root.join("config/systemd/user");
    let state = root.join("scheduler");
    let fail = |actions: &str| fs::write(root.join("fail-actions"), actions).unwrap();
    for action in ["daemon-reload", "enable", "restart"] {
        fail(&format!("{action}\n"));
        assert!(install_automation(&config(&root, "20:00"), "daily", None, &runner).is_err());
        assert!(!units.join("spaceprune-daily.service").exists());
        assert!(!units.join("spaceprune-daily.timer").exists());
        assert!(!state.join("enabled").exists());
        assert!(!state.join("active").exists());
        assert!(!state.join("effective.timer").exists());
    }
    fs::create_dir(units.join("spaceprune-daily.timer")).unwrap();
    assert!(automation_installed_checked("daily").is_err());
    fs::remove_dir(units.join("spaceprune-daily.timer")).unwrap();
    assert!(!automation_installed_checked("daily").unwrap());
    install_automation(&config(&root, "10:00"), "daily", None, &runner).unwrap();
    let service = fs::read(units.join("spaceprune-daily.service")).unwrap();
    let timer = fs::read(units.join("spaceprune-daily.timer")).unwrap();
    let restored = |runtime_enabled: bool| {
        assert_eq!(
            fs::read(units.join("spaceprune-daily.service")).unwrap(),
            service
        );
        assert_eq!(
            fs::read(units.join("spaceprune-daily.timer")).unwrap(),
            timer
        );
        assert_eq!(fs::read(state.join("effective.timer")).unwrap(), timer);
        assert!(state.join("active").exists());
        assert_eq!(state.join("enabled").exists(), !runtime_enabled);
        assert_eq!(state.join("enabled-runtime").exists(), runtime_enabled);
    };
    for action in ["show", "stop", "daemon-reload", "enable", "restart"] {
        fail(&format!("{action}\n"));
        assert!(install_automation(&config(&root, "20:00"), "daily", None, &runner).is_err());
        restored(false);
    }
    for action in ["show", "disable", "daemon-reload"] {
        fail(&format!("{action}\n"));
        assert!(uninstall_automation("daily").is_err());
        restored(false);
    }
    fs::remove_file(state.join("enabled")).unwrap();
    fs::write(state.join("enabled-runtime"), "").unwrap();
    fail("enable\n");
    assert!(install_automation(&config(&root, "20:00"), "daily", None, &runner).is_err());
    restored(true);
    fail("disable-runtime\n");
    assert!(uninstall_automation("daily").is_err());
    restored(true);
    for path in ["enabled-runtime", "active", "effective.timer"] {
        fs::remove_file(state.join(path)).unwrap();
    }
    fail("restart\n");
    assert!(install_automation(&config(&root, "20:00"), "daily", None, &runner).is_err());
    assert_eq!(
        fs::read(units.join("spaceprune-daily.timer")).unwrap(),
        timer
    );
    assert!(!state.join("active").exists());
    assert!(!state.join("enabled").exists());
    install_automation(&config(&root, "10:00"), "daily", None, &runner).unwrap();
    fail("restart\nrestart\n");
    let error = install_automation(&config(&root, "20:00"), "daily", None, &runner).unwrap_err();
    assert!(matches!(error, PlatformError::RecoveryFailed { .. }));
    assert!(
        error
            .to_string()
            .contains("restoring the previous schedule also failed")
    );
    assert!(!state.join("active").exists());
    assert!(!state.join("enabled").exists());
    assert!(error.to_string().contains("automatic scheduling disabled"));
    install_automation(&config(&root, "10:00"), "daily", None, &runner).unwrap();
    fail("restart\nrestart\ndisable\n");
    let error = install_automation(&config(&root, "20:00"), "daily", None, &runner).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("could not disable automatic scheduling")
    );
    assert!(error.to_string().contains("injected disable failure"));
    install_automation(&config(&root, "10:00"), "daily", None, &runner).unwrap();
    fs::remove_file(state.join("enabled")).unwrap();
    fs::write(state.join("enabled-runtime"), "").unwrap();
    fail("restart\nrestart\n");
    let error = install_automation(&config(&root, "20:00"), "daily", None, &runner).unwrap_err();
    assert!(error.to_string().contains("automatic scheduling disabled"));
    assert!(!state.join("active").exists());
    assert!(!state.join("enabled").exists());
    assert!(!state.join("enabled-runtime").exists());
    install_automation(&config(&root, "10:00"), "daily", None, &runner).unwrap();
    fs::remove_file(state.join("enabled")).unwrap();
    fs::write(state.join("enabled-runtime"), "").unwrap();
    uninstall_automation("daily").unwrap();
    assert!(!units.join("spaceprune-daily.timer").exists());
    assert!(!state.join("active").exists());
    assert!(!state.join("enabled").exists());
    assert!(!state.join("enabled-runtime").exists());
    uninstall_automation("daily").unwrap();
}
