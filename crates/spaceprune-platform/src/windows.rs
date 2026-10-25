//! Windows Task Scheduler automation adapter.

use spaceprune_contracts::messages;
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use spaceprune_config::ResolvedConfig;

use crate::{AutomationInstall, PlatformError, automation, command_arguments};

pub(crate) fn install(
    config: &ResolvedConfig,
    automation_id: &str,
    runner: &Path,
) -> Result<AutomationInstall, PlatformError> {
    let definition = automation(config, automation_id)?;
    let task = task_name(automation_id);
    let command_line = windows_command_line(&command_arguments(config, automation_id, runner)?);
    let previous = query_task(&task)?;
    mutate(&task, previous.as_deref(), || {
        run_schtasks(&[
            "/Create",
            "/TN",
            &task,
            "/TR",
            &command_line,
            "/SC",
            "DAILY",
            "/ST",
            &definition.at.to_string(),
            "/F",
            "/HRESULT",
        ])
    })?;
    Ok(result(automation_id))
}

pub(crate) fn uninstall(automation_id: &str) -> Result<AutomationInstall, PlatformError> {
    let task = task_name(automation_id);
    if let Some(previous) = query_task(&task)? {
        mutate(&task, Some(&previous), || delete_task(&task))?;
    }
    Ok(result(automation_id))
}

pub(crate) fn installed(automation_id: &str) -> bool {
    installed_checked(automation_id).unwrap_or(false)
}

pub(crate) fn installed_checked(automation_id: &str) -> Result<bool, PlatformError> {
    query_task(&task_name(automation_id)).map(|previous| previous.is_some())
}

fn query_task(task: &str) -> Result<Option<Vec<u8>>, PlatformError> {
    let arguments = ["/Query", "/TN", task, "/XML", "/HRESULT"];
    let output = schtasks(&arguments)?;
    query_result(output.status.code(), output.stdout, |stdout| {
        command_error(
            &arguments,
            if output.stderr.is_empty() {
                stdout
            } else {
                &output.stderr
            },
            output.status.code(),
        )
    })
}

fn query_result(
    code: Option<i32>,
    xml: Vec<u8>,
    error: impl FnOnce(&[u8]) -> PlatformError,
) -> Result<Option<Vec<u8>>, PlatformError> {
    match code.map(|code| code as u32) {
        Some(0) if !xml.is_empty() => Ok(Some(xml)),
        // HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND / ERROR_PATH_NOT_FOUND).
        Some(0x8007_0002 | 0x8007_0003) => Ok(None),
        Some(0) => Err(PlatformError::Backend(
            messages::text("platform.empty_task").into(),
        )),
        _ => Err(error(&xml)),
    }
}

fn mutate(
    task: &str,
    previous: Option<&[u8]>,
    action: impl FnOnce() -> Result<(), PlatformError>,
) -> Result<(), PlatformError> {
    // Prepare the restoration input before replacing the registered task. Keep the
    // exported bytes intact: schtasks may return UTF-16 XML, including enabled state.
    let backup = previous
        .map(|xml| {
            let mut file = tempfile::Builder::new()
                .prefix("spaceprune-task-")
                .suffix(".xml")
                .tempfile()
                .map_err(|source| io_error(std::env::temp_dir(), source))?;
            file.write_all(xml)
                .map_err(|source| io_error(file.path(), source))?;
            Ok::<_, PlatformError>(file.into_temp_path())
        })
        .transpose()?;
    if let Err(operation) = action() {
        let recovery = if let Some(backup) = backup.as_ref() {
            run_schtasks(&[
                "/Create",
                "/TN",
                task,
                "/XML",
                &backup.to_string_lossy(),
                "/F",
                "/HRESULT",
            ])
        } else {
            delete_task(task)
        };
        if let Err(recovery) = recovery {
            let safety_stop = match run_schtasks(&["/Change", "/TN", task, "/DISABLE"]) {
                Ok(()) => messages::text("platform.scheduling_disabled").to_owned(),
                Err(error) => {
                    messages::message("platform.disable_failed", &[("error", error.to_string())])
                }
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

fn delete_task(task: &str) -> Result<(), PlatformError> {
    match run_schtasks(&["/Delete", "/TN", task, "/F"]) {
        Ok(()) => Ok(()),
        Err(error) => match query_task(task) {
            Ok(None) => Ok(()),
            Ok(Some(_)) => Err(error),
            Err(query) => Err(PlatformError::Backend(messages::message(
                "platform.removal_unconfirmed",
                &[("error", error.to_string()), ("query", query.to_string())],
            ))),
        },
    }
}

fn result(automation_id: &str) -> AutomationInstall {
    AutomationInstall {
        scheduler: "windows-task-scheduler".to_owned(),
        identifier: task_name(automation_id),
        artifacts: Vec::new(),
    }
}

fn task_name(automation_id: &str) -> String {
    format!(r"\SpacePrune\{automation_id}")
}

fn schtasks(arguments: &[&str]) -> Result<Output, PlatformError> {
    Command::new("schtasks.exe")
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .map_err(|source| io_error("schtasks.exe", source))
}

fn run_schtasks(arguments: &[&str]) -> Result<(), PlatformError> {
    let output = schtasks(arguments)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_error(
            arguments,
            if output.stderr.is_empty() {
                &output.stdout
            } else {
                &output.stderr
            },
            output.status.code(),
        ))
    }
}

fn command_error(arguments: &[&str], message: &[u8], code: Option<i32>) -> PlatformError {
    PlatformError::Command {
        command: format!("schtasks.exe {}", arguments.join(" ")),
        message: messages::message(
            "platform.command_exit",
            &[
                (
                    "message",
                    String::from_utf8_lossy(message).trim().to_string(),
                ),
                ("status", format!("{code:?}")),
            ],
        ),
    }
}

fn io_error(path: impl Into<PathBuf>, source: std::io::Error) -> PlatformError {
    PlatformError::Io {
        path: path.into(),
        source,
    }
}

fn windows_command_line(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| windows_quote(argument))
        .collect::<Vec<_>>()
        .join(" ")
}

fn windows_quote(value: &str) -> String {
    if !value.is_empty()
        && !value
            .bytes()
            .any(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\x0B' | b'"'))
    {
        return value.to_owned();
    }
    let mut output = String::from("\"");
    let mut backslashes = 0_usize;
    for character in value.chars() {
        if character == '\\' {
            backslashes += 1;
        } else if character == '"' {
            output.push_str(&"\\".repeat(backslashes * 2 + 1));
            output.push('"');
            backslashes = 0;
        } else {
            output.push_str(&"\\".repeat(backslashes));
            output.push(character);
            backslashes = 0;
        }
    }
    output.push_str(&"\\".repeat(backslashes * 2));
    output.push('"');
    output
}

#[cfg(test)]
mod tests;
