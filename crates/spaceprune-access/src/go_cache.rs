//! Go module-cache cleanup in a bounded, ordinary-user worker; no Go command is executed.

mod transport;
mod worker;

use crate::{OperationContext, filesystem::Removal};
use spaceprune_contracts::{cache::DirectoryBinding, messages};
use std::io;

static LOCAL_WORKER: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Capability probe shared by CLI bootstrap and the dedicated cleanup helper.
pub const SUPPORT_ARGUMENT: &str = "--supports-go-cache-capture-v1";
/// Ordinary-user worker entrypoint. This never accepts an administrator cleanup plan.
pub const WORKER_ARGUMENT: &str = "--spaceprune-go-cache-capture-v1";

/// Dispatch before initializing a host's threads. Only this narrow worker enters a user
/// namespace; its UID 0 is never used by the host's native authorization service.
/// Library hosts may expose this entry or ship the dedicated cleanup helper beside the host.
pub fn run_if_requested() -> Option<i32> {
    let mut arguments = std::env::args_os().skip(1);
    let argument = arguments.next()?;
    if argument == SUPPORT_ARGUMENT {
        return Some(if arguments.next().is_none() { 0 } else { 64 });
    }
    if argument != WORKER_ARGUMENT {
        return None;
    }
    Some(if arguments.next().is_some() {
        64
    } else if worker::run().is_ok() {
        0
    } else {
        1
    })
}

/// Register that this host dispatches `run_if_requested` before initializing application threads.
/// Call only from an executable that implements that entrypoint; otherwise ship a helper.
pub fn register_host_entrypoint() -> io::Result<()> {
    let _ = LOCAL_WORKER.set(std::env::current_exe()?);
    Ok(())
}

/// Native result plus whether the worker stopped before confirming its effects.
#[derive(Debug)]
pub struct Cleanup {
    /// Confirmed removals and errors, including any retained recovery contents.
    pub removal: Removal,
    /// No complete result was received after work was allowed to begin; do not replay blindly.
    pub unconfirmed: bool,
}

/// Clean using ordinary filesystem access first. Only a no-effect access failure starts the
/// same-user namespace worker; missing/old components and real capability gaps remain explicit.
/// The caller owns any subsequent host elevation according to the confirmed plan's policy.
pub fn remove(binding: &DirectoryBinding, context: &OperationContext) -> Cleanup {
    let ordinary = crate::filesystem::remove_go_directory(binding, context);
    let retry = ordinary.error.as_ref().is_some_and(|error| {
        !crate::error::has_effects(error)
            && crate::error::classify(error).kind == crate::IoFailureKind::AccessDenied
    });
    if !retry || context.is_stopped() || rustix::process::geteuid().is_root() {
        return Cleanup {
            removal: ordinary,
            unconfirmed: false,
        };
    }
    match executable(context) {
        Ok(path) => transport::remove(&path, binding, context),
        Err(error) => Cleanup {
            removal: Removal {
                error: Some(error),
                ..Removal::default()
            },
            unconfirmed: false,
        },
    }
}

fn executable(context: &OperationContext) -> io::Result<std::path::PathBuf> {
    use crate::helper::{HelperKind, packaged_at, probe_protocol};
    let host = std::env::current_exe()?;
    let mut candidates = Vec::new();
    if let Some(path) = packaged_at(&host, HelperKind::Cleanup.name()) {
        candidates.push(path);
    }
    // Only explicitly integrated hosts are reexecuted; never probe arbitrary GUI executables.
    if let Some(path) = LOCAL_WORKER.get() {
        candidates.push(path.clone());
    }
    for path in candidates {
        context.check()?;
        if probe_protocol(&path, SUPPORT_ARGUMENT, context).is_ok() {
            return Ok(path);
        }
    }
    if let Ok(path) = crate::helper::probe_helper(HelperKind::Cleanup, context)
        && probe_protocol(&path, SUPPORT_ARGUMENT, context).is_ok()
    {
        return Ok(path);
    }
    context.check()?;
    Err(crate::error::failure(
        crate::IoFailureKind::Unsupported,
        messages::text("linux.go_worker_missing"),
    ))
}
