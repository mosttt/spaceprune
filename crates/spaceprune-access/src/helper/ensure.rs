//! Apply bundled components at the point where native access is actually required.

use super::*;
use crate::Interaction;
use std::sync::{Mutex, MutexGuard, TryLockError};

static SETUP: Mutex<()> = Mutex::new(());

/// Prepare the requested capability and continue under the same operation deadline.
/// Call only after ordinary access fails, or for a user-confirmed privileged operation.
/// Background callers only probe; interactive callers apply missing or updated bundles.
pub fn ensure_helper(
    kind: HelperKind,
    operation: &OperationContext,
) -> Result<PathBuf, PermissionOutcome> {
    ensure_helper_with_prompt(kind, operation, &|_| {})
}

/// Prepare a capability, notifying the host only for an actual native authentication step.
pub fn ensure_helper_with_prompt(
    kind: HelperKind,
    operation: &OperationContext,
    notify: &dyn Fn(bool),
) -> Result<PathBuf, PermissionOutcome> {
    ensure_helper_with_handler_requirement(kind, operation, notify, false)
}

/// Prepare only the additional handler vocabulary required by the current request.
/// Ordinary filesystem, Snapper and existing system tasks retain the original v1 contract.
pub fn ensure_helper_with_handler_requirement(
    kind: HelperKind,
    operation: &OperationContext,
    notify: &dyn Fn(bool),
    requires_developer_handlers: bool,
) -> Result<PathBuf, PermissionOutcome> {
    ensure_required_helper(kind, operation, notify, false, requires_developer_handlers)
        .map_err(install_outcome)
}

/// Prepare cleanup for its confirmed grant. Only a grant containing Go cache selections
/// requires the Go capture feature; existing filesystem/Snapper callers retain their protocol.
/// Source errors distinguish an incomplete or old application bundle from the system copy.
pub fn ensure_cleanup_helper(
    operation: &OperationContext,
    requires_go_capture: bool,
) -> Result<PathBuf, InstallError> {
    ensure_cleanup_helper_with_handler_requirement(operation, requires_go_capture, false)
}

/// Prepare a complete confirmed cleanup grant before sending its bootstrap to the helper.
pub fn ensure_cleanup_helper_with_handler_requirement(
    operation: &OperationContext,
    requires_go_capture: bool,
    requires_developer_handlers: bool,
) -> Result<PathBuf, InstallError> {
    ensure_required_helper(
        HelperKind::Cleanup,
        operation,
        &|_| {},
        requires_go_capture,
        requires_developer_handlers,
    )
}

fn required_feature(
    path: &Path,
    requires_go_capture: bool,
    requires_developer_handlers: bool,
    operation: &OperationContext,
) -> Result<(), PermissionOutcome> {
    if requires_go_capture {
        probe_protocol(path, crate::go_cache::SUPPORT_ARGUMENT, operation)?;
    }
    if requires_developer_handlers {
        probe_protocol(path, DEVELOPER_HANDLERS_SUPPORT, operation)?;
    }
    Ok(())
}

fn ensure_required_helper(
    kind: HelperKind,
    operation: &OperationContext,
    notify: &dyn Fn(bool),
    requires_go_capture: bool,
    requires_developer_handlers: bool,
) -> Result<PathBuf, InstallError> {
    if operation.interaction == Interaction::NonInteractive {
        let installed = probe_helper(kind, operation).map_err(InstallError::Authorization)?;
        required_feature(
            &installed,
            requires_go_capture,
            requires_developer_handlers,
            operation,
        )
        .map_err(InstallError::Authorization)?;
        return Ok(installed);
    }
    let _setup = lock_setup(operation).map_err(InstallError::Authorization)?;
    let host = std::env::current_exe()
        .map_err(|error| InstallError::Authorization(operation_error(error)))?;
    if let Some(installed) = compatible_helper(
        current_helper(kind, &host, operation).map_err(InstallError::Authorization)?,
        requires_go_capture,
        requires_developer_handlers,
        operation,
    )
    .map_err(InstallError::Authorization)?
    {
        return Ok(installed);
    }
    // Check the requested additional capabilities before asking for installation permission.
    // Base callers do not inherit a new plugin's component requirements.
    if requires_go_capture || requires_developer_handlers {
        let source = packaged_at(&host, kind.name()).ok_or(InstallError::SourceMissing)?;
        required_feature(
            &source,
            requires_go_capture,
            requires_developer_handlers,
            operation,
        )
        .map_err(|error| match error {
            PermissionOutcome::HelperOutdated => InstallError::SourceOutdated,
            error => InstallError::Authorization(error),
        })?;
    }
    install::install_with_prompt(&host, operation, notify)?;
    let installed = probe_helper(kind, operation).map_err(InstallError::Authorization)?;
    required_feature(
        &installed,
        requires_go_capture,
        requires_developer_handlers,
        operation,
    )
    .map_err(InstallError::Authorization)?;
    Ok(installed)
}

fn compatible_helper(
    installed: Option<PathBuf>,
    requires_go_capture: bool,
    requires_developer_handlers: bool,
    operation: &OperationContext,
) -> Result<Option<PathBuf>, PermissionOutcome> {
    let Some(path) = installed else {
        return Ok(None);
    };
    match required_feature(
        &path,
        requires_go_capture,
        requires_developer_handlers,
        operation,
    ) {
        Ok(()) => Ok(Some(path)),
        Err(PermissionOutcome::HelperOutdated) => Ok(None),
        Err(error) => Err(error),
    }
}

fn lock_setup(operation: &OperationContext) -> Result<MutexGuard<'static, ()>, PermissionOutcome> {
    // Serialize preparation before the system prompt, not only the elevated copy step.
    loop {
        operation.check().map_err(operation_error)?;
        match SETUP.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(20)),
            Err(TryLockError::Poisoned(_)) => {
                return Err(PermissionOutcome::AuthorizationUnavailable);
            }
        }
    }
}

fn current_helper(
    kind: HelperKind,
    host: &Path,
    operation: &OperationContext,
) -> Result<Option<PathBuf>, PermissionOutcome> {
    let installed = probe_helper(kind, operation);
    match &installed {
        Ok(path) => {
            let Some(bundled) = packaged_at(host, kind.name()) else {
                return installed.map(Some);
            };
            if digest_file(&bundled, operation).map_err(operation_error)?
                == digest_file(path, operation).map_err(operation_error)?
            {
                return installed.map(Some);
            }
        }
        Err(PermissionOutcome::HelperMissing | PermissionOutcome::HelperOutdated) => {}
        Err(_) => return installed.map(Some),
    }
    Ok(None)
}

/// Start graphical metadata access, combining on-demand setup with the first read session.
/// The callback must wait for the metadata handshake before returning the live connection.
/// Cleanup is never started by this installation path.
pub fn start_metadata_session<T>(
    operation: &OperationContext,
    launch: impl FnOnce(&mut Command) -> Result<T, PermissionOutcome>,
) -> Result<T, PermissionOutcome> {
    start_metadata_session_with_prompt(operation, &|_| {}, launch)
}

/// Start graphical metadata access after validating the bundle, with prompt notifications.
pub fn start_metadata_session_with_prompt<T>(
    operation: &OperationContext,
    notify: &dyn Fn(bool),
    launch: impl FnOnce(&mut Command) -> Result<T, PermissionOutcome>,
) -> Result<T, PermissionOutcome> {
    start_metadata_session_with_handler_requirement(operation, notify, false, |command, _| {
        launch(command)
    })
}

/// Start metadata with the caller-required handler set; installation remains on demand.
/// The callback receives the feature of the exact selected installed or bundled component.
pub fn start_metadata_session_with_handler_requirement<T>(
    operation: &OperationContext,
    notify: &dyn Fn(bool),
    requires_developer_handlers: bool,
    launch: impl FnOnce(&mut Command, bool) -> Result<T, PermissionOutcome>,
) -> Result<T, PermissionOutcome> {
    if operation.interaction != Interaction::Foreground {
        return Err(PermissionOutcome::PolicyBlocked);
    }
    let _setup = lock_setup(operation)?;
    let host = std::env::current_exe().map_err(operation_error)?;
    let installed = compatible_helper(
        current_helper(HelperKind::Metadata, &host, operation)?,
        false,
        requires_developer_handlers,
        operation,
    )?;
    let (mut command, supports_developer_handlers) = if let Some(helper) = installed {
        let supports = requires_developer_handlers
            || probe_protocol(&helper, DEVELOPER_HANDLERS_SUPPORT, operation).is_ok();
        let pkexec = crate::process::system_program("pkexec")
            .ok_or(PermissionOutcome::AuthorizationUnavailable)?;
        let mut command = Command::new(pkexec);
        command
            .arg("--disable-internal-agent")
            .arg(helper)
            .arg(HelperKind::Metadata.session_argument());
        (command, supports)
    } else {
        let source = packaged_at(&host, HelperKind::Metadata.name())
            .ok_or(PermissionOutcome::HelperMissing)?;
        required_feature(&source, false, requires_developer_handlers, operation)?;
        let supports = requires_developer_handlers
            || probe_protocol(&source, DEVELOPER_HANDLERS_SUPPORT, operation).is_ok();
        (
            install::metadata_command(&host, operation).map_err(install_outcome)?,
            supports,
        )
    };
    command.stderr(std::process::Stdio::null());
    let _input = crate::permission::NativePromptGuard::new(notify);
    launch(&mut command, supports_developer_handlers)
}

fn install_outcome(error: InstallError) -> PermissionOutcome {
    match error {
        InstallError::Authorization(outcome) => outcome,
        InstallError::SourceMissing | InstallError::InstallerMissing => {
            PermissionOutcome::HelperMissing
        }
        InstallError::SourceOutdated => PermissionOutcome::HelperOutdated,
        InstallError::UnsafeDestination => PermissionOutcome::HelperUntrusted,
        InstallError::Failed => PermissionOutcome::AuthorizationUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_cleanup_keeps_non_go_contract_but_cannot_receive_go_selection() {
        let temp = tempfile::tempdir().unwrap();
        let helper = temp.path().join(HelperKind::Cleanup.name());
        fs::write(&helper, format!(
            "#!/bin/sh\ncase \"$1\" in {}|{HANDLERS_SUPPORT}|{PROMPT_SUPPORT}) exit 0;; *) exit 64;; esac\n",
            HelperKind::Cleanup.support_argument())).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
        let operation = OperationContext::default();
        assert_eq!(
            probe_component(&helper, HelperKind::Cleanup, &operation),
            Ok(())
        );
        assert_eq!(required_feature(&helper, false, false, &operation), Ok(()));
        assert_eq!(
            required_feature(&helper, true, false, &operation),
            Err(PermissionOutcome::HelperOutdated)
        );
    }
    #[test]
    fn previous_v1_helpers_keep_old_tasks_and_new_requests_select_automatic_preparation() {
        let root = tempfile::tempdir().unwrap();
        let operation = OperationContext::default();
        for kind in [HelperKind::Metadata, HelperKind::Cleanup] {
            let old = root.path().join(format!("old-{}", kind.name()));
            fs::write(&old, format!("#!/bin/sh\ncase \"$1\" in {}|{HANDLERS_SUPPORT}|{PROMPT_SUPPORT}) exit 0;; *) exit 64;; esac\n", kind.support_argument())).unwrap();
            fs::set_permissions(&old, fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(probe_component(&old, kind, &operation), Ok(()));
            assert_eq!(
                compatible_helper(Some(old.clone()), false, false, &operation),
                Ok(Some(old.clone()))
            );
            // None is the production install/prepare branch, not a terminal refusal.
            assert_eq!(
                compatible_helper(Some(old.clone()), false, true, &operation),
                Ok(None)
            );
            assert_eq!(
                required_feature(&old, false, true, &operation),
                Err(PermissionOutcome::HelperOutdated)
            );
            let updated = root.path().join(format!("new-{}", kind.name()));
            fs::write(&updated, format!("#!/bin/sh\ncase \"$1\" in {}|{HANDLERS_SUPPORT}|{DEVELOPER_HANDLERS_SUPPORT}|{PROMPT_SUPPORT}) exit 0;; *) exit 64;; esac\n", kind.support_argument())).unwrap();
            fs::set_permissions(&updated, fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(probe_component(&updated, kind, &operation), Ok(()));
            assert_eq!(
                compatible_helper(Some(updated.clone()), false, true, &operation),
                Ok(Some(updated))
            );
            // Failure to provide that updated source never changes the old capability probe.
            assert_eq!(
                compatible_helper(Some(old.clone()), false, false, &operation),
                Ok(Some(old))
            );
        }
    }
}
