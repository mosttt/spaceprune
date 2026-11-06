//! One native deletion session per confirmed operation, shared by built-in handlers.
use spaceprune_access::permission::{ExecutionGrant, NativeSession, PermissionBroker};
#[cfg(any(target_os = "macos", test))]
mod macos;
mod session;
#[cfg(target_os = "windows")]
mod windows_bridge;
mod worker;

use serde::{Deserialize, Serialize};
use spaceprune_access::{
    CapabilityKind, CapabilityRequest, CapabilityScope, OperationContext, OperationIdentity,
    OperationTransfer, PermissionOutcome,
};
use spaceprune_contracts::messages;
use spaceprune_engine::{CleanupPlan, PlanGroup};
use spaceprune_plugin_api::{
    CleanupAccess, CleanupBatchGroup, HandlerCleanResult, PluginError, PluginResult, ProgressEvent,
    ProgressSink, TaskRequest,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const CLEANUP_PROTOCOL_VERSION: u32 = 4;
#[cfg(any(target_os = "linux", target_os = "windows"))]
const WORKER_FLAG: &str = "--spaceprune-cleanup-session";
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    protocol_version: u32,
    operation: OperationTransfer,
    plan_digest: String,
    groups: Vec<PlanGroup>,
}
#[cfg(target_os = "linux")]
impl Grant {
    fn requires_developer_handlers(&self) -> bool {
        self.groups
            .iter()
            .any(|group| group.handler_id.starts_with("builtin.developer-tools/"))
    }

    fn requires_go_capture(&self, selection: &[Selection]) -> bool {
        selection.iter().any(|selected| {
            !selected.items.is_empty()
                && self.groups.get(selected.group).is_some_and(|group| {
                    matches!(
                        group.handler_id.as_str(),
                        "builtin.system-linux/go-modcache" | "builtin.developer-tools/go-modcache"
                    )
                })
        })
    }

    fn without_go_selections(&self) -> Self {
        let mut grant = self.clone();
        for group in &mut grant.groups {
            if matches!(
                group.handler_id.as_str(),
                "builtin.system-linux/go-modcache" | "builtin.developer-tools/go-modcache"
            ) {
                group.items.clear();
            }
        }
        grant
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    group: usize,
    items: Vec<usize>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecuteRequest {
    operation: OperationTransfer,
    selection: Vec<Selection>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response<T = Reply> {
    operation: OperationIdentity,
    reply: T,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Reply {
    Ready,
    Progress {
        event: ProgressEvent,
    },
    Count {
        task: TaskRequest,
        succeeded: usize,
        bytes: u64,
        unknown: usize,
    },
    Results {
        results: Vec<PluginResult<HandlerCleanResult>>,
    },
    ItemFinished {
        task: TaskRequest,
        outcome: spaceprune_plugin_api::ItemExecution,
    },
    Rejected,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutcomeReceipt {
    accepted: bool,
}

#[derive(Default)]
struct NativeState {
    broker: PermissionBroker,
    connection: Mutex<Option<Arc<Mutex<session::Session>>>>,
    failure: Mutex<Option<PluginError>>,
    finished: Mutex<Option<PluginResult<()>>>,
    closing: Mutex<()>,
}

pub(crate) struct NativeCleanupAccess {
    login: Mutex<Option<spaceprune_access::login::LoginBinding>>,
    native: Arc<NativeState>,
    grant: PluginResult<Grant>,
    operation: OperationContext,
    scope: CapabilityScope,
    execution: ExecutionGrant,
}

impl NativeCleanupAccess {
    pub(crate) fn new(plan: &CleanupPlan, context: &OperationContext) -> Self {
        let operation = context.clone();
        let scope = CapabilityScope::System {
            handler: "spaceprune.confirmed-cleanup".into(),
            objects: vec![plan.digest().to_owned()],
        };
        let execution =
            ExecutionGrant::for_confirmed_plan(&operation, scope.clone(), plan.digest());
        let groups = plan
            .groups()
            .iter()
            .filter(|group| supported(&group.handler_id))
            .cloned()
            .map(|mut group| {
                // Reject executable overrides before opening an authorization dialog; the worker
                // repeats this check against the actual system installation before execution.
                worker::prepare_system_programs(&mut group.request.clone())
                    .map_err(|cause| error(cause.to_string()))?;
                if group.handler_id == "builtin.filesystem/paths" {
                    group.request =
                        spaceprune_plugin_filesystem::prepare_authorized_task(&group.request)?;
                }
                Ok(group)
            })
            .collect::<PluginResult<Vec<_>>>();
        Self {
            grant: groups.and_then(|groups| {
                Ok(Grant {
                    protocol_version: CLEANUP_PROTOCOL_VERSION,
                    operation: operation
                        .transfer()
                        .map_err(|cause| error(cause.to_string()))?,
                    plan_digest: plan.digest().into(),
                    groups,
                })
            }),
            login: Mutex::default(),
            native: Arc::default(),
            operation,
            scope,
            execution,
        }
    }
    fn login_context(&self, operation: &OperationContext) -> PluginResult<String> {
        let mut current = self
            .login
            .lock()
            .map_err(|_| error(messages::text("cleanup_access.login_unavailable")))?;
        if current.is_none() {
            let native = Arc::downgrade(&self.native);
            let cancellation = self.operation.cancellation.clone();
            *current = Some(
                spaceprune_access::login::bind(operation, move || {
                    cancellation.cancel();
                    if let Some(native) = native.upgrade() {
                        let _ = native.finish();
                    }
                })
                .map_err(|outcome| session::StartupFailure::from(outcome).into_error())?,
            );
        }
        let binding = current.as_ref().expect("cleanup login initialized");
        if binding.is_revoked() {
            self.operation.cancel();
            return Err(session::StartupFailure::from(PermissionOutcome::Cancelled).into_error());
        }
        Ok(binding.subject().to_owned())
    }
}

fn supported(handler: &str) -> bool {
    handler == "builtin.filesystem/paths"
        || (cfg!(windows) && handler == "builtin.system-windows/component-store")
        || (cfg!(target_os = "linux")
            && (handler == "builtin.snapper/snapshots"
                || handler.starts_with("builtin.system-linux/")
                || spaceprune_plugin_developer_tools::is_cache_handler(handler)))
}
fn error(message: impl Into<String>) -> PluginError {
    PluginError::new("cleanup-authorization-unavailable", message)
}

impl CleanupAccess for NativeCleanupAccess {
    fn clean_authorized(
        &self,
        method: &str,
        groups: &[CleanupBatchGroup<'_>],
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<Vec<PluginResult<HandlerCleanResult>>> {
        if let Some(result) = self
            .native
            .finished
            .lock()
            .map_err(|_| error(messages::text("cleanup_access.shutdown_unavailable")))?
            .as_ref()
        {
            return Err(result
                .as_ref()
                .err()
                .cloned()
                .unwrap_or_else(|| error(messages::text("cleanup_access.already_closed"))));
        }
        let valid_method = if cfg!(target_os = "windows") {
            method == "uac"
        } else if cfg!(target_os = "macos") {
            method == "native"
        } else {
            matches!(method, "sudo" | "pkexec")
        };
        if !valid_method
            || context.id != self.operation.id
            || context.generation != self.operation.generation
        {
            return Err(error(messages::text("cleanup_access.invalid_operation")));
        }
        let mut operation = context.clone();
        operation.deadline = operation.deadline.min(self.operation.deadline);
        operation.budget = operation.budget.restricted(self.operation.budget);
        let grant = self.grant.as_ref().map_err(Clone::clone)?;
        let selected = select(grant, groups)?;
        if let Some(failure) = self
            .native
            .failure
            .lock()
            .map_err(|_| error(messages::text("cleanup_access.state_unavailable")))?
            .as_ref()
        {
            return Err(failure.clone());
        }
        let request = CapabilityRequest {
            subject: self.login_context(&operation)?,
            operation_id: operation.id.clone(),
            kind: if cfg!(any(target_os = "windows", target_os = "macos")) {
                CapabilityKind::FilesystemDelete
            } else {
                CapabilityKind::SystemLifecycle
            },
            scope: self.scope.clone(),
            interaction: operation.interaction,
            reason_code: "confirmed-cleanup-plan".into(),
        };
        let lease =
            self.native
                .broker
                .acquire(&request, &operation, Some(&self.execution), |_, _| {
                    let mut current = self
                        .native
                        .connection
                        .lock()
                        .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
                    let connection = if let Some(connection) = current.as_ref() {
                        connection.clone()
                    } else {
                        if let Some(group) = groups.first() {
                            progress.emit(ProgressEvent {
                                phase: spaceprune_plugin_api::ProgressPhase::Authorize,
                                module_id: Some(group.task.module_id.clone()),
                                task_id: Some(group.task.task_id.clone()),
                                handler_id: Some(group.task.handler_id.clone()),
                                current: None,
                                total: None,
                                message: messages::text("cleanup_access.requesting").into(),
                            });
                        }
                        match session::Session::start(
                            method, grant, &selected, progress, &operation,
                        ) {
                            Ok(session) => {
                                let connection = Arc::new(Mutex::new(session));
                                *current = Some(connection.clone());
                                connection
                            }
                            Err(failure) => {
                                let outcome = failure.outcome;
                                if failure.shutdown_unconfirmed {
                                    *self.native.finished.lock().map_err(|_| {
                                        PermissionOutcome::AuthorizationUnavailable
                                    })? = Some(Err(session::shutdown_failure(outcome)));
                                }
                                if let Ok(mut previous) = self.native.failure.lock() {
                                    *previous = Some(failure.into_error());
                                }
                                return Err(outcome);
                            }
                        }
                    };
                    let alive = connection
                        .lock()
                        .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?
                        .alive
                        .clone();
                    Ok(
                        NativeSession::new(connection, operation.deadline, true).with_liveness(
                            move || {
                                alive.lock().is_ok_and(|current| {
                                    current.load(std::sync::atomic::Ordering::Acquire)
                                })
                            },
                        ),
                    )
                })
                .map_err(|outcome| {
                    self.native
                        .failure
                        .lock()
                        .ok()
                        .and_then(|mut failure| {
                            if failure
                                .as_ref()
                                .is_some_and(|error| error.code == "cleanup-capability-unavailable")
                            {
                                failure.take()
                            } else {
                                failure.clone()
                            }
                        })
                        .unwrap_or_else(|| session::StartupFailure::from(outcome).into_error())
                })?;
        if !lease.covers(&request, &self.native.broker) {
            return Err(error(messages::text("cleanup_access.expired")));
        }
        let connection = lease
            .native::<Mutex<session::Session>>()
            .ok_or_else(|| error(messages::text("cleanup_access.invalid_session")))?;
        let result = connection
            .lock()
            .map_err(|_| error(messages::text("cleanup_access.session_unavailable")))?
            .execute(&selected, groups, progress, &operation);
        if let Err(failure) = &result
            && failure.code != "cleanup-capability-unavailable"
            && let Ok(mut previous) = self.native.failure.lock()
        {
            *previous = Some(failure.clone());
        }
        result
    }

    fn finish(&self) -> PluginResult<()> {
        self.native.finish()
    }
}

fn select(grant: &Grant, groups: &[CleanupBatchGroup<'_>]) -> PluginResult<Vec<Selection>> {
    groups
        .iter()
        .map(|group| {
            let index = grant
                .groups
                .iter()
                .position(|sealed| sealed.request == *group.task)
                .ok_or_else(|| error(messages::text("cleanup_access.task_outside_plan")))?;
            let sealed = &grant.groups[index];
            let items = group
                .items
                .iter()
                .map(|item| {
                    sealed
                        .items
                        .iter()
                        .position(|candidate| candidate == item)
                        .ok_or_else(|| error(messages::text("cleanup_access.item_outside_plan")))
                })
                .collect::<PluginResult<Vec<_>>>()?;
            Ok(Selection {
                group: index,
                items,
            })
        })
        .collect()
}

pub(crate) fn run_if_requested() -> Option<i32> {
    #[cfg(target_os = "macos")]
    if let Some(code) = macos::run_if_requested() {
        return Some(code);
    }
    worker::run_if_requested()
}

impl NativeState {
    fn finish(&self) -> PluginResult<()> {
        let _closing = self
            .closing
            .lock()
            .map_err(|_| session::shutdown_failure(PermissionOutcome::AuthorizationUnavailable))?;
        if let Some(result) = self
            .finished
            .lock()
            .map_err(|_| error(messages::text("cleanup_access.shutdown_unavailable")))?
            .as_ref()
        {
            return result.clone();
        }
        let result = match self.connection.lock() {
            Ok(mut connection) => connection.take().map_or(Ok(()), |connection| {
                connection
                    .lock()
                    .map_err(|_| {
                        session::shutdown_failure(PermissionOutcome::AuthorizationUnavailable)
                    })?
                    .close()
            }),
            Err(_) => Err(session::shutdown_failure(
                PermissionOutcome::AuthorizationUnavailable,
            )),
        };
        let revoked = self
            .broker
            .revoke_sessions()
            .map_err(session::shutdown_failure);
        let result = result.and(revoked.map(drop));
        let mut finished = self
            .finished
            .lock()
            .map_err(|_| error(messages::text("cleanup_access.shutdown_unavailable")))?;
        if let Some(Err(error)) = finished.as_ref() {
            return Err(error.clone());
        }
        *finished = Some(result.clone());
        result
    }
}
