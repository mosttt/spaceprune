//! Unprivileged domain commands and bounded command diagnostics.

use super::*;

pub(super) fn validate_elevation(elevation: &str) -> PluginResult<()> {
    if matches!(elevation, "auto" | "never" | "sudo" | "pkexec") {
        Ok(())
    } else {
        Err(PluginError::new(
            "invalid-elevation",
            spaceprune_plugin_api::messages::text("snapper.invalid_elevation"),
        ))
    }
}

pub(super) fn elevation_method(elevation: &str, needed: bool) -> Option<&str> {
    if !needed || elevation == "never" {
        None
    } else if elevation == "auto" {
        Some(spaceprune_plugin_api::automatic_linux_authorization_method())
    } else {
        Some(elevation)
    }
}

pub(super) fn command(program: &str) -> Command {
    let command = Command::new(program);
    #[cfg(target_os = "linux")]
    let command = {
        let mut command = command;
        if spaceprune_access::process::is_elevated() {
            spaceprune_access::process::restrict_system_environment(&mut command);
        }
        command
    };
    command
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

pub(super) fn measurement_error(
    program: &str,
    stderr: &[u8],
    paths: &[PathBuf],
    search_depth: Option<usize>,
    prune_name: Option<&std::ffi::OsStr>,
    context: &OperationContext,
) -> std::io::Error {
    let message = String::from_utf8_lossy(stderr).trim().to_owned();
    let mut failure = None;
    for path in paths {
        if let Err(error) = core_fs::probe_directory_access(path, search_depth, prune_name, context)
        {
            if context.is_stopped() || error.kind() == std::io::ErrorKind::PermissionDenied {
                return error;
            }
            failure.get_or_insert(error);
        }
    }
    failure.unwrap_or_else(|| {
        std::io::Error::other(if message.is_empty() {
            spaceprune_plugin_api::messages::message(
                "snapper.program_failed",
                &[("program", (program).to_string())],
            )
        } else {
            spaceprune_plugin_api::messages::message(
                "snapper.program_failed_detail",
                &[
                    ("program", (program).to_string()),
                    ("message", (message).to_string()),
                ],
            )
        })
    })
}

pub(super) fn command_failure(action: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    if detail.is_empty() {
        spaceprune_plugin_api::messages::message(
            "snapper.command_exited",
            &[
                ("action", (action).to_string()),
                ("p0", (output.status).to_string()),
            ],
        )
    } else {
        spaceprune_plugin_api::messages::message(
            "snapper.command_failed_detail",
            &[
                ("action", (action).to_string()),
                ("detail", (detail).to_string()),
            ],
        )
    }
}
