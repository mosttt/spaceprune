//! An inherited stdio channel carries only the fixed package and original operation identity.
use super::*;
use crate::{
    Interaction, ResourceBudget,
    windows_package::{
        install_process,
        installer_protocol::{self, FLAG, Reply, Request, VERSION},
    },
};
use std::io::{BufReader, Read, Write};
use std::os::windows::process::CommandExt;

pub(super) fn install(operation: &OperationContext) -> Result<(), PermissionOutcome> {
    if operation.interaction == Interaction::NonInteractive {
        return Err(PermissionOutcome::NeedsAuthorization);
    }
    let operation = operation.child(Duration::from_secs(120));
    let identity = embedded_identity()?.ok_or(PermissionOutcome::HelperMissing)?;
    let _source = pinned_package(&operation)?;
    let request = Request {
        version: VERSION,
        host_pid: std::process::id(),
        host_created: spaceprune_windows_elevation::current_process_creation_time()
            .ok_or(PermissionOutcome::AuthorizationUnavailable)?,
        operation: operation
            .transfer()
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?,
        interaction: operation.interaction,
        package_sha256: identity.msi_sha256,
    };
    let host = std::env::current_exe().map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
    let mut command = Command::new(&host);
    command
        .arg(FLAG)
        .current_dir(host.parent().ok_or(PermissionOutcome::HelperMissing)?)
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    // This launches the same ordinary-permission host, never runas or a model-provided executable.
    install_process::run(command, &request, &operation)
}

/// Dispatch the dedicated ordinary-permission installer before application or CLI initialization.
pub fn run_installer_if_requested() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(FLAG)) {
        return None;
    }
    if args.next().is_some() {
        return Some(64);
    }
    let mut input = BufReader::new(std::io::stdin());
    let Ok(request) = installer_protocol::read_request(&mut input) else {
        return Some(65);
    };
    let result = install_in_worker(&request, input);
    let reply = Reply {
        version: VERSION,
        operation: request.operation.identity,
        package_sha256: request.package_sha256,
        result,
    };
    Some(
        if serde_json::to_writer(std::io::stdout(), &reply).is_ok()
            && std::io::stdout().flush().is_ok()
        {
            0
        } else {
            74
        },
    )
}

fn install_in_worker(
    request: &Request,
    mut input: BufReader<std::io::Stdin>,
) -> Result<installer_protocol::Installed, PermissionOutcome> {
    let identity = embedded_identity()?.ok_or(PermissionOutcome::HelperMissing)?;
    if identity.msi_sha256 != request.package_sha256 {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    let operation = OperationContext::from_transfer(
        &request.operation,
        Duration::from_secs(120),
        ResourceBudget::workflow(),
    )
    .map_err(|error| {
        if error.kind() == std::io::ErrorKind::TimedOut {
            PermissionOutcome::TimedOut
        } else {
            PermissionOutcome::PolicyBlocked
        }
    })?;
    let user = spaceprune_windows_elevation::current_user_sid()
        .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
    let host = spaceprune_windows_elevation::HostProcess::observe(
        request.host_pid,
        request.host_created,
        &user,
    )
    .ok_or(PermissionOutcome::HelperUntrusted)?;
    let cancelled = operation.clone();
    // This process exits after the synchronous Installer result; the pipe watcher has no work
    // beyond propagating EOF (including a crashed parent) into the original cancellation signal.
    std::thread::spawn(move || {
        let _ = input.read(&mut [0_u8; 1]);
        cancelled.cancel();
    });
    let package = pinned_package(&operation)?;
    if !host.is_running() {
        return Err(PermissionOutcome::Cancelled);
    }
    check(&operation)?;
    let result = spaceprune_windows_elevation::install_package_in_worker(&package, || {
        operation.is_stopped() || !host.is_running()
    });
    check(&operation)?;
    if !host.is_running() {
        return Err(PermissionOutcome::Cancelled);
    }
    installer_protocol::installer_result(result)
}
