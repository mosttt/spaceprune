//! Bounded domain-command execution; the host owns native authorization.

use super::*;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{env, process::Command};

#[derive(Debug)]
pub(super) enum ActionFailure {
    Native {
        started: bool,
        reclaimed: u64,
        error: io::Error,
    },
    BeforeStart(String),
    Unconfirmed(String),
}

pub(super) fn run_action(
    action: &Action,
    task: &TaskRequest,
    progress: &dyn ProgressSink,
    context: &OperationContext,
) -> Result<(), ActionFailure> {
    let program = &action.program;
    let mut operation = context.child(std::time::Duration::from_secs(300));
    operation.budget.first_output_timeout = operation
        .budget
        .first_output_timeout
        .min(std::time::Duration::from_secs(300));
    operation.budget.idle_timeout = operation
        .budget
        .idle_timeout
        .min(std::time::Duration::from_secs(300));
    operation.check().map_err(|error| ActionFailure::Native {
        started: false,
        reclaimed: 0,
        error,
    })?;
    progress.emit(ProgressEvent {
        phase: ProgressPhase::Clean,
        module_id: Some(task.module_id.clone()),
        task_id: Some(task.task_id.clone()),
        handler_id: Some(task.handler_id.clone()),
        current: None,
        total: None,
        message: messages::message("linux.running_command", &[("program", program.to_owned())]),
    });
    let mut command = native_system_command(program)
        .map_err(|cause| ActionFailure::BeforeStart(cause.to_string()))?;
    command.args(&action.args);
    let output = spaceprune_access::process::capture(command, &operation).map_err(|error| {
        ActionFailure::Unconfirmed(messages::message(
            "linux.command_unconfirmed",
            &[
                ("program", program.to_owned()),
                ("error", error.to_string()),
            ],
        ))
    })?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(ActionFailure::Unconfirmed(if stderr.is_empty() {
            messages::message(
                "linux.command_exit",
                &[
                    ("program", program.to_owned()),
                    ("status", output.status.to_string()),
                ],
            )
        } else {
            messages::message(
                "linux.command_failed",
                &[("program", program.to_owned()), ("stderr", stderr)],
            )
        }))
    }
}

pub(super) fn native_system_command(program: &str) -> io::Result<Command> {
    #[cfg(target_os = "linux")]
    if spaceprune_access::process::is_elevated() {
        let path = spaceprune_access::process::system_program(program).ok_or_else(|| {
            io::Error::other(messages::message(
                "linux.trusted_program_required",
                &[("program", program.to_owned())],
            ))
        })?;
        let mut command = Command::new(path);
        spaceprune_access::process::restrict_system_environment(&mut command);
        if program == "go" {
            command.env("GOENV", "off").env("GOTOOLCHAIN", "local");
        }
        return Ok(command);
    }
    Ok(Command::new(program))
}

pub(super) fn command_exists(program: &str) -> bool {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return is_executable(path);
    }
    env::var_os("PATH").is_some_and(|value| {
        env::split_paths(&value).any(|directory| is_executable(&directory.join(program)))
    })
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

pub(super) fn effective_uid() -> Option<u32> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}
