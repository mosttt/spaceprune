//! Shared native metadata authorization for CLI, recipe scans and local observation tools.

#[path = "client/connection.rs"]
mod connection;
/// Installation and updates shared by terminal and graphical hosts.
#[cfg(target_os = "linux")]
#[path = "client/install.rs"]
pub mod install;
#[path = "client/requests.rs"]
mod requests;
/// Trusted system installation and executable discovery.
#[cfg(target_os = "linux")]
#[path = "client/trust.rs"]
pub mod trust;
#[cfg(target_os = "windows")]
#[path = "client/windows_trust.rs"]
pub mod trust;

use crate::{PreparedRequest, ResponseOutcome};
use connection::{Connection, StartupFailure};
use requests::Requests;
use spaceprune_access::{
    CapabilityKind, CapabilityRequest, CapabilityScope, NativePath, OperationContext,
    PermissionOutcome,
    login::{LoginBinding, LoginRevocation},
    permission::{NativeSession, PermissionBroker},
};
use spaceprune_plugin_api::{
    HandlerScanResult, NullProgress, ProgressEvent, ProgressPhase, ProgressSink, TaskRequest,
};
/// Freeze a system source before opening a native authorization prompt.
#[cfg(target_os = "linux")]
pub use spaceprune_plugin_system_linux::prepare_system_cache_scan;
use std::{
    sync::{Arc, Mutex, MutexGuard, TryLockError},
    time::{Duration, Instant},
};

struct SharedConnection {
    slot: Mutex<Option<Connection>>,
    requests: Arc<Requests>,
}
impl SharedConnection {
    fn close(&self) -> Result<(), PermissionOutcome> {
        let result = match self.slot.lock() {
            Ok(mut slot) => slot
                .take()
                .map_or(Ok(()), |mut connection| connection.close()),
            Err(_) => Err(PermissionOutcome::AuthorizationUnavailable),
        };
        self.requests.record_shutdown(result);
        result
    }
}
impl Drop for SharedConnection {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
type TestLauncher =
    Arc<dyn Fn(&OperationContext) -> Result<Connection, StartupFailure> + Send + Sync>;

/// Owns native connections and refusal records; it never stores credentials or deletion grants.
#[derive(Default)]
pub struct MetadataService {
    login: Mutex<Option<(LoginBinding, spaceprune_access::Interaction)>>,
    broker: Arc<PermissionBroker>,
    requests: Arc<Requests>,
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    launcher: Option<TestLauncher>,
}

impl MetadataService {
    #[cfg(all(test, target_os = "macos"))]
    pub(super) fn unavailable_for_test() -> Self {
        Self {
            login: Mutex::default(),
            broker: Arc::default(),
            requests: Arc::default(),
            launcher: Some(Arc::new(|_| {
                Err(PermissionOutcome::NeedsAuthorization.into())
            })),
        }
    }

    /// Check for an actual live connection without displaying a system prompt.
    pub fn is_active(&self) -> bool {
        self.requests.check().is_ok()
            && self.login.lock().is_ok_and(|login| {
                login.as_ref().is_some_and(|(binding, _)| {
                    !binding.is_revoked()
                        && self
                            .broker
                            .is_active(binding.subject(), CapabilityKind::MetadataRead)
                })
            })
    }

    /// Cancel active native requests and confirm connection shutdown before releasing reuse.
    /// An unconfirmed shutdown remains an error on repeated close and blocks new requests.
    pub fn close(&self) -> Result<(), PermissionOutcome> {
        self.requests
            .close(|| release_connections(&self.broker, &self.requests))
    }

    fn login_context(
        &self,
        operation: &OperationContext,
    ) -> Result<(String, LoginRevocation), PermissionOutcome> {
        self.requests.check_operation(operation)?;
        let mut current = self
            .login
            .lock()
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
        if current.as_ref().is_some_and(|(binding, initial)| {
            binding.is_revoked()
                || (*initial == spaceprune_access::Interaction::NonInteractive
                    && operation.interaction != spaceprune_access::Interaction::NonInteractive)
        }) {
            self.close()?;
            current.take();
            self.requests.check_operation(operation)?;
        }
        if current.is_none() {
            let broker = Arc::downgrade(&self.broker);
            let requests = Arc::downgrade(&self.requests);
            let binding = spaceprune_access::login::bind(operation, move || {
                if let (Some(broker), Some(requests)) = (broker.upgrade(), requests.upgrade()) {
                    let _ = requests.revoke_login(|| release_connections(&broker, &requests));
                }
            })?;
            *current = Some((binding, operation.interaction));
        }
        let (binding, _) = current.as_ref().expect("login binding initialized");
        Ok((binding.subject().to_owned(), binding.revocation()))
    }

    /// Inspect an already scoped metadata request after ordinary access proved insufficient.
    pub fn measure(
        &self,
        request: &PreparedRequest,
        operation: &OperationContext,
    ) -> Result<ResponseOutcome, PermissionOutcome> {
        self.with_connection(
            request,
            operation,
            &NullProgress,
            |connection, operation| connection.measure(request, operation),
        )
    }

    /// Scan one registered built-in task after an access-denied result, preserving its rule scope.
    #[cfg(not(target_os = "macos"))]
    pub fn scan(
        &self,
        task: &TaskRequest,
        operation: &OperationContext,
    ) -> Result<HandlerScanResult, PermissionOutcome> {
        self.scan_with_progress(task, operation, &NullProgress)
    }

    /// Inspect a scoped task while suspending terminal output only during native authentication.
    pub fn scan_with_progress(
        &self,
        task: &TaskRequest,
        operation: &OperationContext,
        progress: &dyn ProgressSink,
    ) -> Result<HandlerScanResult, PermissionOutcome> {
        check(operation)?;
        let mut task = task.clone();
        if task.handler_id == "builtin.filesystem/paths" {
            let roots = task
                .options
                .get_mut("roots")
                .and_then(serde_json::Value::as_array_mut)
                .ok_or(PermissionOutcome::PolicyBlocked)?;
            for root in roots {
                let path = root
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(PermissionOutcome::PolicyBlocked)?;
                let absolute =
                    std::path::absolute(path).map_err(|_| PermissionOutcome::PolicyBlocked)?;
                root["path"] = serde_json::Value::String(
                    absolute
                        .to_str()
                        .ok_or(PermissionOutcome::PolicyBlocked)?
                        .to_owned(),
                );
            }
        }
        let request = match task.handler_id.as_str() {
            "builtin.filesystem/paths" => {
                PreparedRequest::create_filesystem_scan("recipe-source-0", &task)
            }
            #[cfg(target_os = "linux")]
            "builtin.snapper/snapshots" => {
                PreparedRequest::create_snapper_scan("recipe-source-0", &task)
            }
            #[cfg(target_os = "linux")]
            "builtin.system-linux/tmpfiles" => {
                PreparedRequest::create_tmpfiles_scan("recipe-source-0", &task)
            }
            _ => return Err(PermissionOutcome::Unsupported),
        };
        self.scan_request(&task, request, operation, progress)
    }

    /// Retry a frozen cache source through the existing metadata connection.
    #[cfg(target_os = "linux")]
    pub fn scan_system_cache_with_progress(
        &self,
        source: &spaceprune_plugin_api::SystemCacheScanRequest,
        operation: &OperationContext,
        progress: &dyn ProgressSink,
    ) -> Result<HandlerScanResult, PermissionOutcome> {
        let request = PreparedRequest::create_system_cache_scan("recipe-source-0", source);
        let result = self.scan_request(&source.task, request, operation, progress)?;
        let returned = prepare_system_cache_scan(&source.task, &result)
            .map_err(|_| PermissionOutcome::PolicyBlocked)?
            .ok_or(PermissionOutcome::PolicyBlocked)?;
        if returned.source.action_id != source.source.action_id
            || returned.source.roots != source.source.roots
            || returned.source.program_available != source.source.program_available
            || source
                .source
                .binding
                .as_ref()
                .is_some_and(|expected| returned.source.binding.as_ref() != Some(expected))
        {
            return Err(PermissionOutcome::PolicyBlocked);
        }
        Ok(result)
    }

    fn scan_request(
        &self,
        task: &TaskRequest,
        request: Result<PreparedRequest, crate::HelperError>,
        operation: &OperationContext,
        progress: &dyn ProgressSink,
    ) -> Result<HandlerScanResult, PermissionOutcome> {
        let request = request
            .and_then(|request| request.with_operation(operation))
            .map_err(|_| {
                check(operation)
                    .err()
                    .unwrap_or(PermissionOutcome::PolicyBlocked)
            })?;
        let (result, cancelled) =
            self.with_connection(&request, operation, progress, |connection, operation| {
                progress.emit(ProgressEvent {
                    phase: ProgressPhase::Measure,
                    module_id: Some(task.module_id.clone()),
                    task_id: Some(task.task_id.clone()),
                    handler_id: Some(task.handler_id.clone()),
                    current: None,
                    total: None,
                    message: spaceprune_plugin_api::messages::message(
                        "native.inspection_authorized",
                        &[("p0", task.name.clone())],
                    ),
                });
                connection.scan(&request, operation)
            })?;
        if cancelled {
            operation.cancellation.cancel();
        }
        Ok(result)
    }

    /// Read system metadata normally first; reuse or request native access only on AccessDenied.
    /// This does not walk snapshot contents or perform lifecycle operations.
    #[cfg(target_os = "linux")]
    pub fn inspect_snapper(
        &self,
        config: Option<&str>,
        operation: &OperationContext,
    ) -> spaceprune_plugin_api::PluginResult<spaceprune_plugin_snapper::SnapperInventory> {
        self.inspect_snapper_with_progress(config, operation, &NullProgress)
    }

    /// Inspect a system domain with host notifications limited to actual native authorization.
    #[cfg(target_os = "linux")]
    pub fn inspect_snapper_with_progress(
        &self,
        config: Option<&str>,
        operation: &OperationContext,
        progress: &dyn ProgressSink,
    ) -> spaceprune_plugin_api::PluginResult<spaceprune_plugin_snapper::SnapperInventory> {
        let ordinary =
            spaceprune_plugin_snapper::snapper_inventory_unprivileged("snapper", config, operation);
        self.retry_snapper_inventory(config, operation, progress, ordinary)
    }

    #[cfg(target_os = "linux")]
    fn retry_snapper_inventory(
        &self,
        config: Option<&str>,
        operation: &OperationContext,
        progress: &dyn ProgressSink,
        ordinary: spaceprune_plugin_api::PluginResult<spaceprune_plugin_snapper::SnapperInventory>,
    ) -> spaceprune_plugin_api::PluginResult<spaceprune_plugin_snapper::SnapperInventory> {
        use spaceprune_plugin_api::PluginError;
        if !ordinary.as_ref().is_err_and(|error| {
            error.io_failure.as_ref().is_some_and(|failure| {
                failure.kind == spaceprune_access::IoFailureKind::AccessDenied
            })
        }) {
            return ordinary;
        }
        let request = PreparedRequest::create_snapper_inventory(config)
            .and_then(|request| request.with_operation(operation))
            .map_err(|_| {
                PluginError::new(
                    "authorization-scope-rejected",
                    spaceprune_plugin_api::messages::text("native.invalid_inventory_scope"),
                )
            })?;
        let (inventory, cancelled) = self
            .with_connection(&request, operation, progress, |connection, operation| {
                connection.scan(&request, operation)
            })
            .map_err(|outcome| {
                PluginError::new(
                    match outcome {
                        PermissionOutcome::Denied => "authorization-denied",
                        PermissionOutcome::Cancelled => "authorization-cancelled",
                        PermissionOutcome::TimedOut => "authorization-timeout",
                        PermissionOutcome::HelperOutdated => "authorization-helper-outdated",
                        PermissionOutcome::HelperMissing => "authorization-helper-missing",
                        _ => "authorization-unavailable",
                    },
                    spaceprune_plugin_api::messages::text(match outcome {
                        PermissionOutcome::Denied => "native.metadata_declined",
                        PermissionOutcome::Cancelled => "native.metadata_cancelled",
                        PermissionOutcome::TimedOut => "native.metadata_timeout",
                        PermissionOutcome::HelperOutdated => "native.metadata_helper_outdated",
                        #[cfg(target_os = "windows")]
                        PermissionOutcome::HelperMissing
                            if matches!(
                                spaceprune_access::windows_native::embedded_identity(),
                                Ok(None)
                            ) =>
                        {
                            "native.windows_components_not_bundled"
                        }
                        PermissionOutcome::HelperMissing => "native.metadata_helper_missing",
                        PermissionOutcome::HelperUntrusted => "native.metadata_helper_untrusted",
                        PermissionOutcome::NeedsAuthorization => "native.metadata_foreground",
                        PermissionOutcome::PolicyBlocked => "native.metadata_scope",
                        _ => "native.metadata_unavailable",
                    }),
                )
            })?;
        if cancelled {
            operation.cancellation.cancel();
        }
        Ok(inventory)
    }

    fn with_connection<T>(
        &self,
        request: &PreparedRequest,
        operation: &OperationContext,
        progress: &dyn ProgressSink,
        run: impl FnOnce(&mut Connection, &OperationContext) -> Result<T, PermissionOutcome>,
    ) -> Result<T, PermissionOutcome> {
        check(operation)?;
        if request.context.identity() != operation.identity() {
            return Err(PermissionOutcome::PolicyBlocked);
        }
        let (subject, login) = self.login_context(operation)?;
        let active = self.requests.enter(operation)?;
        let operation = active.operation();
        if login.is_revoked() {
            self.requests.reject_operation(operation);
            return Err(PermissionOutcome::Cancelled);
        }
        let capability = CapabilityRequest {
            subject,
            operation_id: operation.id.clone(),
            kind: CapabilityKind::MetadataRead,
            scope: request.scope.clone(),
            interaction: operation.interaction,
            reason_code: "metadata-access-denied".to_owned(),
        };
        let lease = self
            .broker
            .acquire(&capability, operation, None, |_, operation| {
                self.requests.check()?;
                let connection = self
                    .launch(operation, progress, request.requires_developer_handlers())
                    .map_err(|failure| {
                        if failure.shutdown_unconfirmed {
                            self.requests.record_shutdown(Err(failure.outcome));
                        }
                        failure.outcome
                    })?;
                let shared = Arc::new(SharedConnection {
                    slot: Mutex::new(Some(connection)),
                    requests: Arc::clone(&self.requests),
                });
                let weak = Arc::downgrade(&shared);
                Ok(NativeSession::new(
                    shared,
                    Instant::now() + Duration::from_secs(24 * 60 * 60),
                    true,
                )
                .with_liveness(move || {
                    weak.upgrade()
                        .is_some_and(|shared| match shared.slot.try_lock() {
                            Ok(mut slot) => slot.as_mut().is_some_and(Connection::is_alive),
                            Err(TryLockError::WouldBlock) => true,
                            Err(TryLockError::Poisoned(_)) => false,
                        })
                }))
            })?;
        let shared = lease
            .native::<SharedConnection>()
            .ok_or(PermissionOutcome::AuthorizationUnavailable)?;
        let mut slot = lock(&shared, operation)?;
        if !lease.covers(&capability, &self.broker) {
            return Err(PermissionOutcome::AuthorizationUnavailable);
        }
        #[cfg(target_os = "linux")]
        if request.requires_developer_handlers()
            && slot
                .as_ref()
                .is_some_and(|connection| !connection.supports_developer_handlers)
        {
            // Prepare the new capability first; missing bundles leave an existing v1 read
            // session usable for filesystem and Snapper requests.
            #[cfg(test)]
            let mocked = self.launcher.is_some();
            #[cfg(not(test))]
            let mocked = false;
            if !mocked {
                spaceprune_access::helper::ensure_helper_with_handler_requirement(
                    spaceprune_access::helper::HelperKind::Metadata,
                    operation,
                    &|active| progress.set_input_active(active),
                    true,
                )?;
            }
            if let Some(mut previous) = slot.take() {
                let closed = previous.close();
                self.requests.record_shutdown(closed);
                closed?;
            }
            *slot = Some(self.launch(operation, progress, true).map_err(|failure| {
                if failure.shutdown_unconfirmed {
                    self.requests.record_shutdown(Err(failure.outcome));
                }
                failure.outcome
            })?);
        }
        let result = run(
            slot.as_mut()
                .ok_or(PermissionOutcome::AuthorizationUnavailable)?,
            operation,
        );
        if result.is_err() {
            let _ = request.cancel();
            if let Some(mut connection) = slot.take() {
                self.requests.record_shutdown(connection.close());
            }
        }
        result
    }

    fn launch(
        &self,
        operation: &OperationContext,
        progress: &dyn ProgressSink,
        requires_developer_handlers: bool,
    ) -> Result<Connection, StartupFailure> {
        #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
        if let Some(launcher) = &self.launcher {
            let _input = spaceprune_plugin_api::ProgressInputGuard::new(progress);
            return launcher(operation);
        }
        Connection::start(operation, progress, requires_developer_handlers)
    }
}

fn release_connections(
    broker: &PermissionBroker,
    requests: &Requests,
) -> Result<(), PermissionOutcome> {
    for session in broker.revoke_sessions()? {
        if let Some(connection) = session.handle::<SharedConnection>() {
            requests.record_shutdown(connection.close());
        }
    }
    requests.check()
}

fn lock<'a>(
    shared: &'a SharedConnection,
    operation: &OperationContext,
) -> Result<MutexGuard<'a, Option<Connection>>, PermissionOutcome> {
    loop {
        check(operation)?;
        match shared.slot.try_lock() {
            Ok(slot) => return Ok(slot),
            Err(TryLockError::Poisoned(_)) => {
                return Err(PermissionOutcome::AuthorizationUnavailable);
            }
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

fn check(operation: &OperationContext) -> Result<(), PermissionOutcome> {
    operation.check().map_err(|error| operation_error(&error))
}

fn operation_error(error: &std::io::Error) -> PermissionOutcome {
    match error.kind() {
        std::io::ErrorKind::Interrupted => PermissionOutcome::Cancelled,
        std::io::ErrorKind::TimedOut => PermissionOutcome::TimedOut,
        _ => PermissionOutcome::AuthorizationUnavailable,
    }
}

pub(super) fn request_scope(
    roots: &[std::path::PathBuf],
    task: Option<&serde_json::Value>,
) -> CapabilityScope {
    if roots.is_empty() {
        CapabilityScope::System {
            handler: task
                .and_then(|task| task.get("handler_id"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("builtin.snapper/snapshots")
                .to_owned(),
            objects: task.map(|task| vec![task.to_string()]).unwrap_or_default(),
        }
    } else {
        CapabilityScope::Filesystem {
            roots: roots
                .iter()
                .map(|root| NativePath::from_path(root))
                .collect(),
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "client/tests.rs"]
mod tests;
