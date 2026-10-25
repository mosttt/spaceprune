//! systemd user schedule installation and failure recovery.

use super::{AutomationUnits, PlatformError};
use spaceprune_config::{AutomationDefinition, ResolvedConfig, is_valid_id};
use spaceprune_plugin_api::messages;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

mod recovery;
use recovery::PreviousInstallation;

/// 使用显式 runner 安装一个配置化自动化，并立即启用 timer。
pub fn install_automation(
    config: &ResolvedConfig,
    automation_id: &str,
    application_config: Option<&Path>,
    runner: &Path,
) -> Result<AutomationUnits, PlatformError> {
    validate_automation_id(automation_id)?;
    let automation = find_automation(config, automation_id)?;
    let directory = systemd_user_directory()?;
    fs::create_dir_all(&directory).map_err(|source| PlatformError::Io {
        path: directory.clone(),
        source,
    })?;
    let units = unit_paths(&directory, automation_id);
    let config_argument = systemd_config_argument(application_config);
    let service = format!(
        "[Unit]\nDescription=SpacePrune automation {automation_id}\n\n[Service]\nType=oneshot\nExecStart={}{} --no-progress automation run {}\n",
        systemd_quote(&runner.to_string_lossy()),
        config_argument,
        automation_id
    );
    let at = automation.at;
    let timer = format!(
        "[Unit]\nDescription=SpacePrune automation timer {automation_id}\n\n[Timer]\nOnCalendar=*-*-* {at}:00\nPersistent=true\nUnit=spaceprune-{automation_id}.service\n\n[Install]\nWantedBy=timers.target\n"
    );
    let previous = PreviousInstallation::capture(&units, automation_id)?;
    let unit_name = format!("spaceprune-{automation_id}.timer");
    let mut published = false;
    let result = (|| {
        if previous.active {
            systemctl(["stop", &unit_name])?;
        }
        write(&units.service, &service)?;
        write(&units.timer, &timer)?;
        published = true;
        systemctl(["daemon-reload"])?;
        systemctl(["enable", &unit_name])?;
        systemctl(["restart", &unit_name])
    })();
    if let Err(error) = result {
        return Err(previous.recover(error, &units, automation_id, published));
    }
    Ok(units)
}

/// 停止并删除一个 systemd 用户自动化。
pub fn uninstall_automation(automation_id: &str) -> Result<AutomationUnits, PlatformError> {
    validate_automation_id(automation_id)?;
    let directory = systemd_user_directory()?;
    let units = unit_paths(&directory, automation_id);
    let unit_name = format!("spaceprune-{automation_id}.timer");
    let previous = PreviousInstallation::capture(&units, automation_id)?;
    let published = previous.loaded || units.timer.is_file();
    let result = (|| {
        if published {
            disable_timer(&unit_name, true)?;
        }
        remove_if_exists(&units.service)?;
        remove_if_exists(&units.timer)?;
        systemctl(["daemon-reload"])
    })();
    if let Err(error) = result {
        return Err(previous.recover(error, &units, automation_id, published));
    }
    Ok(units)
}

/// 判断指定自动化 timer 是否已经安装。
pub fn automation_installed(automation_id: &str) -> bool {
    automation_installed_checked(automation_id).unwrap_or(false)
}

/// Check persisted schedule presence without treating inaccessible state as absence.
pub fn automation_installed_checked(automation_id: &str) -> Result<bool, PlatformError> {
    validate_automation_id(automation_id)?;
    let path = unit_paths(&systemd_user_directory()?, automation_id).timer;
    match fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(PlatformError::Io {
            path,
            source: std::io::Error::other(messages::text("linux.timer_not_file")),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(PlatformError::Io { path, source }),
    }
}

fn find_automation<'a>(
    config: &'a ResolvedConfig,
    id: &str,
) -> Result<&'a AutomationDefinition, PlatformError> {
    config
        .automations
        .get(id)
        .filter(|automation| automation.enabled)
        .ok_or_else(|| PlatformError::UnknownAutomation(id.to_owned()))
}

fn systemd_user_directory() -> Result<PathBuf, PlatformError> {
    if let Some(value) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value).join("systemd/user"));
    }
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(|value| PathBuf::from(value).join(".config/systemd/user"))
        .ok_or(PlatformError::ConfigDirectoryMissing)
}

fn unit_paths(directory: &std::path::Path, automation_id: &str) -> AutomationUnits {
    AutomationUnits {
        service: directory.join(format!("spaceprune-{automation_id}.service")),
        timer: directory.join(format!("spaceprune-{automation_id}.timer")),
    }
}

fn write(path: &std::path::Path, content: &str) -> Result<(), PlatformError> {
    fs::write(path, content).map_err(|source| PlatformError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn remove_if_exists(path: &std::path::Path) -> Result<(), PlatformError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(PlatformError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn systemctl<const N: usize>(args: [&str; N]) -> Result<(), PlatformError> {
    systemctl_output(args).map(|_| ())
}

fn disable_timer(unit: &str, stop: bool) -> Result<(), PlatformError> {
    if stop {
        systemctl(["disable", "--now", unit])?;
    } else {
        systemctl(["disable", unit])?;
    }
    // Persistent and runtime enablement have separate symlinks in systemd.
    systemctl(["disable", "--runtime", unit])
}

fn systemctl_output<const N: usize>(args: [&str; N]) -> Result<String, PlatformError> {
    let action = args.join(" ");
    let output = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .map_err(|source| PlatformError::Io {
            path: PathBuf::from("systemctl"),
            source,
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(PlatformError::Systemctl {
            action,
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

fn systemd_quote(value: &str) -> String {
    let escaped = value
        .replace('%', "%%")
        .replace('$', "$$")
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

fn systemd_config_argument(path: Option<&Path>) -> String {
    path.map_or_else(String::new, |path| {
        format!(" --config {}", systemd_quote(&path.to_string_lossy()))
    })
}

fn validate_automation_id(automation_id: &str) -> Result<(), PlatformError> {
    if is_valid_id(automation_id) {
        Ok(())
    } else {
        Err(PlatformError::InvalidAutomationId(automation_id.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{systemd_config_argument, systemd_quote};

    #[test]
    fn quotes_systemd_arguments() {
        assert_eq!(systemd_quote("a b"), "\"a b\"");
        assert_eq!(systemd_quote("a\\b"), "\"a\\\\b\"");
        assert_eq!(systemd_quote("$HOME 100%\n"), "\"$$HOME 100%%\\n\"");
    }

    #[test]
    fn service_command_uses_the_application_config() {
        assert_eq!(systemd_config_argument(None), "");
        assert_eq!(
            systemd_config_argument(Some(Path::new("/tmp/Space Prune/config.toml"))),
            " --config \"/tmp/Space Prune/config.toml\""
        );
    }
}
