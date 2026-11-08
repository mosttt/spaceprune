//! Read-only systemd policy previews. Their logical sizes are not release estimates.

use super::*;
use std::{
    collections::HashSet,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    process::Command,
    time::Duration,
};

/// Validate the single system policy accepted by the rootless metadata request.
pub fn validate_tmpfiles_task(task: &TaskRequest) -> PluginResult<()> {
    if task.handler_id != "builtin.system-linux/tmpfiles" {
        return Err(PluginError::new(
            "invalid-system-policy",
            messages::text("linux.invalid_source"),
        ));
    }
    SystemHandler {
        kind: Kind::Tmpfiles,
    }
    .validate(task)
}

/// Preview the installed system policy through the same read-only path as an ordinary scan.
pub fn inspect_tmpfiles_task(
    task: &TaskRequest,
    progress: &dyn ProgressSink,
    context: &OperationContext,
) -> PluginResult<HandlerScanResult> {
    validate_tmpfiles_task(task)?;
    scan::ordinary(Kind::Tmpfiles, task, progress, context)
}

#[derive(Debug)]
pub(super) struct TmpfilesMeasurement {
    pub(super) logical_bytes: u64,
    pub(super) is_empty: bool,
}

pub(super) fn tmpfiles(context: &OperationContext) -> Result<TmpfilesMeasurement, HandlerIssue> {
    let command = command::native_system_command("systemd-tmpfiles").map_err(io_issue)?;
    preview(command, context)
}

fn preview(
    mut command: Command,
    context: &OperationContext,
) -> Result<TmpfilesMeasurement, HandlerIssue> {
    // Keep --dry-run unconditional. Older systemd versions must fail rather than clean.
    command
        .args(["--clean", "--dry-run"])
        .env("LC_ALL", "C")
        .env("SYSTEMD_LOG_TARGET", "console")
        .env("SYSTEMD_LOG_LEVEL", "info")
        .env("SYSTEMD_COLORS", "0")
        .env("SYSTEMD_LOG_LOCATION", "0")
        .env("SYSTEMD_LOG_TIME", "0");
    let operation = context.child(Duration::from_secs(30));
    let output = spaceprune_access::process::capture(command, &operation).map_err(io_issue)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success()
        && stderr.contains("--dry-run")
        && (stderr.contains("unrecognized option") || stderr.contains("unknown option"))
    {
        return Err(diagnostic(
            "systemd-preview-unsupported",
            "linux.tmpfiles_preview_unsupported",
            None,
        ));
    }
    if !output.status.success() {
        return Err(output_issue(&stderr));
    }
    if !output.stdout.is_empty() {
        return Err(diagnostic(
            "systemd-preview-unrecognized",
            "linux.tmpfiles_preview_unrecognized",
            None,
        ));
    }
    measure_preview(&output.stderr, &operation)
}

fn measure_preview(
    output: &[u8],
    context: &OperationContext,
) -> Result<TmpfilesMeasurement, HandlerIssue> {
    context.check().map_err(io_issue)?;
    let mut bytes = 0_u64;
    let mut seen = HashSet::new();
    let mut is_empty = true;
    for line in output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        context.check().map_err(io_issue)?;
        if line.starts_with(b"Would restore access and modification time on ") {
            continue;
        }
        // These are the dry-run records for age-based clean operations. Unknown messages,
        // warnings and ambiguous path quoting keep the result unknown, never a false zero.
        let quoted = line
            .strip_prefix(b"Would remove directory ")
            .or_else(|| line.strip_prefix(b"Would remove file "))
            .or_else(|| line.strip_prefix(b"Would remove "))
            .ok_or_else(|| output_issue(&String::from_utf8_lossy(line)))?;
        let path = quoted
            .strip_prefix(b"\"")
            .and_then(|value| {
                value
                    .strip_suffix(b"\".")
                    .or_else(|| value.strip_suffix(b"\""))
            })
            .filter(|value| !value.contains(&b'"') && !value.iter().any(u8::is_ascii_control))
            .map(|value| Path::new(std::ffi::OsStr::from_bytes(value)))
            .filter(|path| {
                path.is_absolute()
                    && !path
                        .components()
                        .any(|part| matches!(part, std::path::Component::ParentDir))
            })
            .ok_or_else(|| {
                diagnostic(
                    "systemd-preview-unrecognized",
                    "linux.tmpfiles_preview_unrecognized",
                    None,
                )
            })?;
        let metadata = spaceprune_access::filesystem::inspect_entry_metadata(path, context)
            .map_err(io_issue)?;
        is_empty = false;
        // Directories and links contribute no ordinary-file bytes. Do not walk a directory:
        // the native policy separately reports eligible children and their exclusions.
        if metadata.is_file() && seen.insert((metadata.dev(), metadata.ino())) {
            bytes = bytes.saturating_add(metadata.len());
        }
    }
    context.check().map_err(io_issue)?;
    Ok(TmpfilesMeasurement {
        logical_bytes: bytes,
        is_empty,
    })
}

fn output_issue(output: &str) -> HandlerIssue {
    if output.contains("Permission denied") || output.contains("Operation not permitted") {
        return io_issue(io::Error::from(io::ErrorKind::PermissionDenied));
    }
    diagnostic(
        "systemd-preview-incomplete",
        "linux.tmpfiles_preview_incomplete",
        None,
    )
}

fn io_issue(error: io::Error) -> HandlerIssue {
    diagnostic(
        "systemd-preview-incomplete",
        "linux.tmpfiles_preview_incomplete",
        Some(spaceprune_access::error::classify(&error)),
    )
}

fn diagnostic(
    code: &str,
    message: &str,
    failure: Option<spaceprune_access::IoFailure>,
) -> HandlerIssue {
    HandlerIssue {
        code: Some(code.into()),
        io_failure: failure,
        level: IssueLevel::Warning,
        message: messages::text(message).into(),
        location: Some("systemd-tmpfiles".into()),
    }
}

#[cfg(test)]
mod tests;
