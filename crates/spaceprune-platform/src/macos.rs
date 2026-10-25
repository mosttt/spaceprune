//! macOS launchd user automation adapter.

mod state;

use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use spaceprune_config::ResolvedConfig;
use spaceprune_contracts::messages;

use crate::{AutomationInstall, PlatformError, automation, command_arguments};
use state::{Snapshot, write_definition};

struct LaunchAgent {
    path: PathBuf,
    domain: String,
    label: String,
}

impl LaunchAgent {
    fn new(automation_id: &str) -> Result<Self, PlatformError> {
        Ok(Self {
            path: launch_agent_path(automation_id)?,
            domain: launchd_domain()?,
            label: label(automation_id),
        })
    }

    fn target(&self) -> String {
        format!("{}/{}", self.domain, self.label)
    }

    fn unload(&self) -> Result<(), PlatformError> {
        if self.loaded()? {
            launchctl(&["bootout", &self.target()])?;
        }
        Ok(())
    }

    fn bootstrap(&self) -> Result<(), PlatformError> {
        launchctl(&["bootstrap", &self.domain, &self.path.to_string_lossy()])
    }

    fn set_disabled(&self, disabled: bool) -> Result<(), PlatformError> {
        launchctl(&[if disabled { "disable" } else { "enable" }, &self.target()])
    }

    fn mutate(
        &self,
        previous: Snapshot,
        action: impl FnOnce() -> Result<(), PlatformError>,
    ) -> Result<(), PlatformError> {
        if let Err(operation) = action() {
            if let Err(recovery) = previous.restore(self) {
                // Disabling prevents a future load; bootout also removes an already
                // loaded schedule. Do both, even when one of the commands fails.
                let disable = self.set_disabled(true);
                let unload = self.unload();
                let safety_stop = match (disable, unload) {
                    (Ok(()), Ok(())) => messages::text("platform.disabled_unloaded").to_owned(),
                    (disable, unload) => messages::message(
                        "platform.safety_results",
                        &[("disable", outcome(disable)), ("unload", outcome(unload))],
                    ),
                };
                return Err(PlatformError::Recovery {
                    operation: Box::new(operation),
                    recovery: Box::new(recovery),
                    safety_stop,
                });
            }
            return Err(operation);
        }
        Ok(())
    }
}

pub(crate) fn install(
    config: &ResolvedConfig,
    automation_id: &str,
    runner: &Path,
) -> Result<AutomationInstall, PlatformError> {
    let definition = automation(config, automation_id)?;
    let arguments = command_arguments(config, automation_id, runner)?;
    let agent = LaunchAgent::new(automation_id)?;
    let content = plist(
        &agent.label,
        &arguments,
        definition.at.hour(),
        definition.at.minute(),
    );
    let previous = Snapshot::capture(&agent)?;
    write_definition(&agent.path, content.as_bytes(), previous.permissions())?;
    agent.mutate(previous, || {
        agent.unload()?;
        // A disabled launchd service cannot be bootstrapped until it is enabled.
        agent.set_disabled(false)?;
        agent.bootstrap()
    })?;
    Ok(result(automation_id, agent.path))
}

pub(crate) fn uninstall(automation_id: &str) -> Result<AutomationInstall, PlatformError> {
    let agent = LaunchAgent::new(automation_id)?;
    let previous = Snapshot::capture(&agent)?;
    agent.mutate(previous, || {
        agent.unload()?;
        state::remove_definition(&agent.path)
    })?;
    Ok(result(automation_id, agent.path))
}

pub(crate) fn installed(automation_id: &str) -> bool {
    // Preserve the existing inexpensive inventory query. Mutations use the
    // checked query, which also notices a loaded service without its plist.
    launch_agent_path(automation_id).is_ok_and(|path| path.is_file())
}

pub(crate) fn installed_checked(automation_id: &str) -> Result<bool, PlatformError> {
    let agent = LaunchAgent::new(automation_id)?;
    let definition = state::read_definition(&agent.path)?;
    Ok(agent.loaded()? || definition.is_some())
}

fn result(automation_id: &str, path: PathBuf) -> AutomationInstall {
    AutomationInstall {
        scheduler: "launchd".to_owned(),
        identifier: label(automation_id),
        artifacts: vec![path],
    }
}

fn launch_agent_path(automation_id: &str) -> Result<PathBuf, PlatformError> {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| {
            home.join("Library/LaunchAgents")
                .join(format!("{}.plist", label(automation_id)))
        })
        .ok_or(PlatformError::ConfigDirectoryMissing)
}

fn label(automation_id: &str) -> String {
    format!("io.spaceprune.{automation_id}")
}

fn launchd_domain() -> Result<String, PlatformError> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .map_err(|source| state::io_error("id", source))?;
    if !output.status.success() {
        return Err(PlatformError::Command {
            command: "id -u".to_owned(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(format!(
        "gui/{}",
        String::from_utf8_lossy(&output.stdout).trim()
    ))
}

fn launchctl_output(arguments: &[&str]) -> Result<Output, PlatformError> {
    Command::new("launchctl")
        .args(arguments)
        .output()
        .map_err(|source| state::io_error("launchctl", source))
}

fn launchctl(arguments: &[&str]) -> Result<(), PlatformError> {
    let output = launchctl_output(arguments)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_error(arguments, &output))
    }
}

fn command_error(arguments: &[&str], output: &Output) -> PlatformError {
    PlatformError::Command {
        command: format!("launchctl {}", arguments.join(" ")),
        message: messages::message(
            "platform.command_exit",
            &[
                (
                    "message",
                    String::from_utf8_lossy(&output.stderr).trim().to_string(),
                ),
                ("status", output.status.to_string()),
            ],
        ),
    }
}

fn outcome(result: Result<(), PlatformError>) -> String {
    match result {
        Ok(()) => messages::text("platform.succeeded").into(),
        Err(error) => error.to_string(),
    }
}

fn plist(label: &str, arguments: &[String], hour: u8, minute: u8) -> String {
    let arguments = arguments
        .iter()
        .map(|argument| format!("        <string>{}</string>", xml_escape(argument)))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{}</string>
    <key>ProgramArguments</key>
    <array>
{}
    </array>
    <key>StartCalendarInterval</key>
    <dict>
        <key>Hour</key>
        <integer>{hour}</integer>
        <key>Minute</key>
        <integer>{minute}</integer>
    </dict>
</dict>
</plist>
"#,
        xml_escape(label),
        arguments
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests;
