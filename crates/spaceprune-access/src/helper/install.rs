//! Host-side installation shared by on-demand preparation and explicit repair.

use super::*;
use crate::{
    Interaction,
    process::{ProcessTree, is_elevated, system_program},
};
use std::process::Stdio;

/// Installation errors do not grant filesystem read or cleanup capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallError {
    /// A fixed executable is absent from the host's bundle.
    SourceMissing,
    /// A bundled executable does not support the required protocol.
    SourceOutdated,
    /// The destination or its ancestors are not protected real filesystem objects.
    UnsafeDestination,
    /// The bundled installer is unavailable.
    InstallerMissing,
    /// Native authentication or the operation lifetime prevented installation.
    Authorization(PermissionOutcome),
    /// Installation did not complete successfully.
    Failed,
}

/// Check protected ancestors without creating directories or following symbolic links.
pub fn destination_is_safe(destination: &Path) -> bool {
    destination
        .ancestors()
        .enumerate()
        .all(|(index, path)| match fs::symlink_metadata(path) {
            Ok(metadata) => {
                !metadata.file_type().is_symlink()
                    && (index == 0 || metadata.is_dir())
                    && metadata.uid() == 0
                    && metadata.permissions().mode() & 0o022 == 0
            }
            Err(error) => error.kind() == io::ErrorKind::NotFound,
        })
}

fn destinations_are_safe() -> bool {
    destination_is_safe(Path::new(INSTALL_DIRECTORY)) && destination_is_safe(Path::new(POLICY_PATH))
}

/// Report whether explicit installation is available, without probing or authenticating.
pub fn can_install(host: &Path, interaction: Interaction) -> bool {
    [
        HelperKind::Metadata.name(),
        HelperKind::Cleanup.name(),
        INSTALLER_NAME,
    ]
    .iter()
    .all(|name| packaged_at(host, name).is_some())
        && destinations_are_safe()
        && (is_elevated() || system_program(authorizer(interaction)).is_some())
}

fn authorizer(interaction: Interaction) -> &'static str {
    if interaction == Interaction::Foreground {
        "pkexec"
    } else {
        "sudo"
    }
}

fn prepare(
    host: &Path,
    operation: &OperationContext,
) -> Result<(PathBuf, InstallRequest), InstallError> {
    let source = |name| packaged_at(host, name).ok_or(InstallError::SourceMissing);
    let metadata = source(HelperKind::Metadata.name())?;
    let cleanup = source(HelperKind::Cleanup.name())?;
    let installer = packaged_at(host, INSTALLER_NAME).ok_or(InstallError::InstallerMissing)?;
    for (path, argument) in [
        (&metadata, HelperKind::Metadata.support_argument()),
        (&metadata, HANDLERS_SUPPORT),
        (&metadata, PROMPT_SUPPORT),
        (&cleanup, HelperKind::Cleanup.support_argument()),
        (&cleanup, HANDLERS_SUPPORT),
        (&cleanup, PROMPT_SUPPORT),
        (&installer, INSTALLER_SUPPORT),
        (&installer, HANDLERS_SUPPORT),
        (&installer, PROMPT_SUPPORT),
    ] {
        probe_protocol(path, argument, operation).map_err(|outcome| match outcome {
            PermissionOutcome::HelperOutdated => InstallError::SourceOutdated,
            outcome => InstallError::Authorization(outcome),
        })?;
    }
    let request = InstallRequest {
        operation: operation
            .transfer()
            .map_err(|error| InstallError::Authorization(operation_error(error)))?,
        source_directory: crate::NativePath::from_path(
            metadata.parent().expect("adjacent component"),
        ),
        metadata_sha256: digest_file(&metadata, operation)
            .map_err(|error| InstallError::Authorization(operation_error(error)))?,
        cleanup_sha256: digest_file(&cleanup, operation)
            .map_err(|error| InstallError::Authorization(operation_error(error)))?,
    };
    Ok((installer, request))
}

fn authorize_terminal(
    operation: &OperationContext,
    notify: &dyn Fn(bool),
) -> Result<(), InstallError> {
    let sudo = system_program("sudo").ok_or(InstallError::Authorization(
        PermissionOutcome::AuthorizationUnavailable,
    ))?;
    crate::permission::with_cached_terminal_authorization(
        operation,
        || {
            let mut command = Command::new(&sudo);
            command.args(["-n", "-v"]);
            let output = capture(command, operation).map_err(operation_error)?;
            if output.status.success() {
                Ok(())
            } else {
                Err(PermissionOutcome::NeedsAuthorization)
            }
        },
        || {
            use std::io::IsTerminal;
            if !std::io::stdin().is_terminal() {
                return Err(PermissionOutcome::NeedsAuthorization);
            }
            let mut command = Command::new(&sudo);
            command.arg("-v");
            let _input = crate::permission::NativePromptGuard::new(notify);
            let status =
                crate::process::terminal_status(command, operation).map_err(operation_error)?;
            if status.success() {
                Ok(())
            } else {
                Err(PermissionOutcome::Denied)
            }
        },
    )
    .map_err(InstallError::Authorization)
}

fn command(
    installer: &Path,
    request: &InstallRequest,
    interaction: Interaction,
    argument: &str,
) -> Result<Command, InstallError> {
    let mut command = if is_elevated() {
        Command::new(installer)
    } else {
        let program = system_program(authorizer(interaction)).ok_or(
            InstallError::Authorization(PermissionOutcome::AuthorizationUnavailable),
        )?;
        let mut command = Command::new(program);
        if interaction == Interaction::Foreground {
            command.arg("--disable-internal-agent");
        } else {
            command.args(["-n", "--"]);
        }
        command.arg(installer);
        command
    };
    let request = serde_json::to_string(request).map_err(|_| InstallError::Failed)?;
    if request.len() > MAX_REQUEST_BYTES {
        return Err(InstallError::Failed);
    }
    command
        .arg(argument)
        .arg(request)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(command)
}

pub(super) fn metadata_command(
    host: &Path,
    operation: &OperationContext,
) -> Result<Command, InstallError> {
    if !destinations_are_safe() {
        return Err(InstallError::UnsafeDestination);
    }
    let (installer, request) = prepare(host, operation)?;
    command(
        &installer,
        &request,
        operation.interaction,
        INSTALL_METADATA_ARGUMENT,
    )
}

/// Install both capability components through the bundled, narrowly scoped installer.
/// Native authorization protects both on-demand preparation and explicit setup requests.
pub fn install(host: &Path, operation: &OperationContext) -> Result<(), InstallError> {
    install_with_prompt(host, operation, &|_| {})
}

pub(super) fn install_with_prompt(
    host: &Path,
    operation: &OperationContext,
    notify: &dyn Fn(bool),
) -> Result<(), InstallError> {
    operation
        .check()
        .map_err(|error| InstallError::Authorization(operation_error(error)))?;
    if !destinations_are_safe() {
        return Err(InstallError::UnsafeDestination);
    }
    let (installer, request) = prepare(host, operation)?;
    if !is_elevated() && operation.interaction != Interaction::Foreground {
        authorize_terminal(operation, notify)?;
    }
    operation
        .check()
        .map_err(|error| InstallError::Authorization(operation_error(error)))?;
    let mut command = command(
        &installer,
        &request,
        operation.interaction,
        INSTALL_ARGUMENT,
    )?;
    let _input = (!is_elevated() && operation.interaction == Interaction::Foreground)
        .then(|| crate::permission::NativePromptGuard::new(notify));
    let mut process = ProcessTree::spawn(&mut command).map_err(|_| InstallError::Failed)?;
    // EOF reaches the elevated worker even when the caller cannot signal a root process.
    let input = process.child_mut().stdin.take();
    let result = loop {
        if let Err(error) = operation.check() {
            break Err(InstallError::Authorization(operation_error(error)));
        }
        match process.has_exited() {
            Ok(true) => break Ok(()),
            Ok(false) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break Err(InstallError::Failed),
        }
    };
    drop(input);
    if result.is_err() {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline && matches!(process.has_exited(), Ok(false)) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let status = process
        .terminate_and_wait()
        .map_err(|_| InstallError::Failed)?;
    result?;
    if !status.success() {
        return Err(match status.code() {
            Some(126) => InstallError::Authorization(PermissionOutcome::Denied),
            Some(127) => InstallError::Authorization(PermissionOutcome::AuthorizationUnavailable),
            _ => InstallError::Failed,
        });
    }
    probe_installation(operation).map_err(InstallError::Authorization)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_previous_bundle_can_prepare_non_go_installation_without_authorization() {
        let temp = tempfile::tempdir().unwrap();
        let host = temp.path().join("host");
        fs::write(&host, "fixture host").unwrap();
        let probes = [
            (
                HelperKind::Metadata.name(),
                HelperKind::Metadata.support_argument(),
            ),
            (
                HelperKind::Cleanup.name(),
                HelperKind::Cleanup.support_argument(),
            ),
            (INSTALLER_NAME, INSTALLER_SUPPORT),
        ];
        for (name, support) in probes {
            let path = temp.path().join(name);
            fs::write(&path, format!(
                "#!/bin/sh\ncase \"$1\" in {support}|{HANDLERS_SUPPORT}|{PROMPT_SUPPORT}) exit 0;; *) exit 64;; esac\n"
            )).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let operation = OperationContext::default();
        assert_eq!(
            probe_protocol(
                &temp.path().join(HelperKind::Cleanup.name()),
                crate::go_cache::SUPPORT_ARGUMENT,
                &operation
            ),
            Err(PermissionOutcome::HelperOutdated)
        );
        let (installer, request) = prepare(&host, &operation).unwrap();
        assert_eq!(installer, temp.path().join(INSTALLER_NAME));
        assert_eq!(request.source_directory.to_path_buf().unwrap(), temp.path());
    }
}
