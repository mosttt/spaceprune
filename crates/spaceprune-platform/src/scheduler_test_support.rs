//! Isolated subprocess fixtures; never resolve a host scheduler command.

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

pub(crate) struct Fixture {
    pub(crate) state: PathBuf,
    pub(crate) definition: PathBuf,
}

pub(crate) fn run_isolated(test: &str) -> bool {
    if std::env::var_os("SPACEPRUNE_SCHEDULER_FIXTURE").is_some() {
        return false;
    }
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let python = Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .expect("scheduler fixtures require Python 3");
    assert!(python.status.success());
    let python = String::from_utf8(python.stdout).unwrap();
    let script = root.path().join("scheduler.py");
    fs::write(&script, include_str!("../tests/fixtures/scheduler.py")).unwrap();
    for command in ["launchctl", "schtasks.exe"] {
        let path = bin.join(command);
        fs::write(&path, format!(
            "#!/bin/sh\nexec \"$SPACEPRUNE_TEST_PYTHON\" \"$SPACEPRUNE_TEST_SCRIPT\" {command} \"$@\"\n"
        )).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let id = bin.join("id");
    fs::write(&id, "#!/bin/sh\nprintf '501\\n'\n").unwrap();
    fs::set_permissions(id, fs::Permissions::from_mode(0o700)).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env("SPACEPRUNE_SCHEDULER_FIXTURE", root.path())
        .env("HOME", root.path().join("home"))
        .env("PATH", &bin)
        .env("SPACEPRUNE_TEST_PYTHON", python.trim())
        .env("SPACEPRUNE_TEST_SCRIPT", script)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

impl Fixture {
    pub(crate) fn reset() -> Self {
        let root = PathBuf::from(std::env::var_os("SPACEPRUNE_SCHEDULER_FIXTURE").unwrap());
        let state = root.join("state");
        if state.exists() {
            fs::remove_dir_all(&state).unwrap();
        }
        let home = root.join("home");
        if home.exists() {
            fs::remove_dir_all(&home).unwrap();
        }
        let definition = home.join("Library/LaunchAgents/io.spaceprune.daily.plist");
        fs::create_dir_all(definition.parent().unwrap()).unwrap();
        fs::create_dir_all(&state).unwrap();
        Self { state, definition }
    }

    pub(crate) fn fail(&self, command: &str, call: usize, mode: &str) {
        fs::write(self.state.join(format!("{command}.{call}.fail")), mode).unwrap();
    }

    pub(crate) fn trace(&self) -> String {
        fs::read_to_string(self.state.join("trace")).unwrap_or_default()
    }
}

pub(crate) fn config() -> spaceprune_config::ResolvedConfig {
    let mut config = spaceprune_config::ConfigLoader::new()
        .load_official("minimal")
        .unwrap();
    let definition = spaceprune_config::AutomationDefinition {
        name: "Daily cleanup".into(),
        modules: vec!["cache".into()],
        at: spaceprune_config::DailyTime::DEFAULT,
        enabled: true,
    };
    config.automations.insert("daily".into(), definition);
    config
}
