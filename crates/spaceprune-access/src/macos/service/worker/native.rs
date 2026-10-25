//! Only a root service may launch and authenticate its same-signed worker executable.

use super::*;
use crate::macos::authorization::Authorization;

impl Worker {
    pub(in crate::macos::service) fn start(
        kind: ServiceKind,
        caller_uid: u32,
        operation: &OperationContext,
    ) -> Result<Self, PermissionOutcome> {
        if !rustix::process::geteuid().is_root() {
            return Err(PermissionOutcome::HelperUntrusted);
        }
        let team = signing::service_team(kind)?;
        let executable = std::env::current_exe().map_err(|error| io_outcome(error, operation))?;
        let expected_hash = signing::verify_file(&executable, &team, kind.label())?;
        Self::launch(
            kind,
            caller_uid,
            operation,
            |name| {
                let mut command = ProcessCommand::new(&executable);
                command.arg(kind.worker_argument()).arg(name);
                crate::process::restrict_system_environment(&mut command);
                command
            },
            |channel, pid| {
                let child = channel.peer().ok_or(PermissionOutcome::HelperUntrusted)?;
                if child.identity.pid != pid
                    || child.identity.effective_uid != 0
                    || child.identity.real_uid != 0
                {
                    return Err(PermissionOutcome::HelperUntrusted);
                }
                child.verify_code(&team, &[kind.label()], Some(&expected_hash))
            },
        )
    }
}

struct NativePolicy(ServiceKind);
impl server::AuthorityPolicy for NativePolicy {
    type Authority = Authorization;
    fn initialize(&self) -> Result<(), PermissionOutcome> {
        Authorization::ensure_access_right(self.0)
    }
    fn adopt(
        &self,
        credential: &ExternalAuthorization,
    ) -> Result<Authorization, PermissionOutcome> {
        Authorization::from_external(credential, self.0)
    }
    fn verify(&self, authority: &Authorization) -> Result<(), PermissionOutcome> {
        authority.verify_access(self.0)
    }
}

/// Dispatch the fixed launchd entrypoint or its authenticated child worker. Workers inherit no
/// user-selected executable, credential file or request path; all data uses verified Mach IPC.
pub fn run_from_environment(
    kind: ServiceKind,
    execute: impl Fn(
        &AuthorizedRequest,
        &OperationContext,
        &dyn EventSink,
    ) -> Result<serde_json::Value, PermissionOutcome>,
) -> Option<i32> {
    let mut arguments = std::env::args_os().skip(1);
    let Some(first) = arguments.next() else {
        // SMJobBless writes ProgramArguments itself and passes no custom daemon flag.
        let executable = std::env::current_exe().ok()?;
        return legacy_daemon_entry(kind, &executable)
            .then(|| daemon::run_service(kind).map_or(70, |()| 0));
    };
    if first == kind.daemon_argument() {
        return Some(if arguments.next().is_none() {
            daemon::run_service(kind).map_or(70, |()| 0)
        } else {
            64
        });
    }
    if first != kind.worker_argument() {
        return None;
    }
    let Some(name) = arguments.next().and_then(|value| value.into_string().ok()) else {
        return Some(64);
    };
    if arguments.next().is_some() {
        return Some(64);
    }
    Some(run_worker(kind, &name, execute).map_or(70, |()| 0))
}

fn run_worker(
    kind: ServiceKind,
    name: &str,
    execute: impl Fn(
        &AuthorizedRequest,
        &OperationContext,
        &dyn EventSink,
    ) -> Result<serde_json::Value, PermissionOutcome>,
) -> Result<(), PermissionOutcome> {
    if !rustix::process::geteuid().is_root() || !rustix::process::getuid().is_root() {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    let team = signing::service_team(kind)?;
    let executable = std::env::current_exe().map_err(|_| PermissionOutcome::HelperUntrusted)?;
    let expected_hash = signing::verify_file(&executable, &team, kind.label())?;
    let own = peer::current_identity().map_err(|_| PermissionOutcome::HelperUntrusted)?;
    let startup = idle_context().child(Duration::from_secs(10));
    let channel = Channel::connect(name, &startup).map_err(|error| io_outcome(error, &startup))?;
    server::serve(
        channel,
        kind,
        |channel| {
            let parent = channel.peer().ok_or(PermissionOutcome::HelperUntrusted)?;
            if !own.same_login(parent.identity)
                || rustix::process::getppid().map(|pid| pid.as_raw_nonzero().get() as u32)
                    != Some(parent.identity.pid)
            {
                return Err(PermissionOutcome::HelperUntrusted);
            }
            parent.verify_code(&team, &[kind.label()], Some(&expected_hash))
        },
        NativePolicy(kind),
        execute,
        agent::stop_agent,
    )
}
