//! Native session reuse and operation-scoped denial suppression, without UI or disclosure policy.

use crate::{
    CapabilityKind, CapabilityRequest, CapabilityScope, Interaction, OperationContext,
    PermissionOutcome,
};
use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

/// Notify a host only while a native authenticator can accept user input.
/// Component probing and cached authorization must happen before creating this guard.
pub struct NativePromptGuard<'a>(&'a dyn Fn(bool));

impl<'a> NativePromptGuard<'a> {
    /// Suspend host output until the native prompt finishes, including on failure.
    pub fn new(notify: &'a dyn Fn(bool)) -> Self {
        notify(true);
        Self(notify)
    }
}

impl Drop for NativePromptGuard<'_> {
    fn drop(&mut self) {
        self.0(false);
    }
}

/// Try existing terminal authorization first and ask once only when the native result requires it.
/// Graphical authorization uses its own platform adapter; noninteractive callers never prompt.
pub fn with_cached_terminal_authorization<T>(
    operation: &OperationContext,
    mut cached: impl FnMut() -> Result<T, PermissionOutcome>,
    authorize: impl FnOnce() -> Result<(), PermissionOutcome>,
) -> Result<T, PermissionOutcome> {
    check_operation(operation)?;
    match cached() {
        Err(PermissionOutcome::NeedsAuthorization)
            if operation.interaction == Interaction::Terminal =>
        {
            check_operation(operation)?;
            authorize()?;
            check_operation(operation)?;
            cached()
        }
        outcome => outcome,
    }
}

/// A real native connection owned by its platform backend. It is never serialized.
#[derive(Clone)]
pub struct NativeSession {
    handle: Arc<dyn Any + Send + Sync>,
    expires_at: Instant,
    noninteractive: bool,
    alive: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl NativeSession {
    /// Wrap an established native connection. A successful prompt without a handshake is not a session.
    pub fn new<T: Any + Send + Sync>(
        handle: Arc<T>,
        expires_at: Instant,
        noninteractive: bool,
    ) -> Self {
        Self {
            handle,
            expires_at,
            noninteractive,
            alive: None,
        }
    }

    /// Attach a nonblocking native liveness check. An expired process is never a grant.
    pub fn with_liveness(mut self, alive: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        self.alive = Some(Arc::new(alive));
        self
    }

    /// Access the native owner when explicitly closing a revoked cached session.
    pub fn handle<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.handle.clone().downcast().ok()
    }

    fn is_valid(&self) -> bool {
        Instant::now() < self.expires_at && self.alive.as_ref().is_none_or(|alive| alive())
    }
}

/// A scoped in-memory lease. Possession does not replace per-request native identity checks.
#[derive(Clone)]
pub struct CapabilityLease {
    subject: String,
    kind: CapabilityKind,
    scope: CapabilityScope,
    epoch: u64,
    session: NativeSession,
}

impl CapabilityLease {
    /// Access only the platform backend's expected native connection type.
    pub fn native<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.session.handle.clone().downcast().ok()
    }
    /// Check subject, capability, exact request scope, lifetime and revocation before each use.
    pub fn covers(&self, request: &CapabilityRequest, broker: &PermissionBroker) -> bool {
        self.subject == request.subject
            && self.kind == request.kind
            && self.scope == request.scope
            && self.session.is_valid()
            && broker.epoch() == self.epoch
            && (request.interaction != Interaction::NonInteractive || self.session.noninteractive)
    }
}

/// A local, non-deserializable proof that the host confirmed a particular execution scope.
pub struct ExecutionGrant {
    operation_id: String,
    scope: CapabilityScope,
    plan_digest: String,
    expires_at: Instant,
}

impl ExecutionGrant {
    /// Called by the trusted executor after consuming the user's immutable confirmed plan.
    pub fn for_confirmed_plan(
        operation: &OperationContext,
        scope: CapabilityScope,
        plan_digest: &str,
    ) -> Self {
        Self {
            operation_id: operation.id.clone(),
            scope,
            plan_digest: plan_digest.to_owned(),
            expires_at: operation.deadline,
        }
    }
    fn permits(&self, request: &CapabilityRequest) -> bool {
        self.operation_id == request.operation_id
            && self.scope == request.scope
            && self.plan_digest.len() == 64
            && Instant::now() < self.expires_at
    }
}

type SessionKey = (String, CapabilityKind);
type DenialKey = (String, String, u64, CapabilityKind);

enum Slot {
    Authorizing,
    Ready(NativeSession),
}
#[derive(Default)]
struct State {
    epoch: u64,
    sessions: BTreeMap<SessionKey, Slot>,
    denied: BTreeSet<DenialKey>,
    settings_pending: BTreeSet<DenialKey>,
}

/// Serializes native prompts, reuses real connections and remembers denials or pending settings.
#[derive(Default)]
pub struct PermissionBroker {
    state: Mutex<State>,
    changed: Condvar,
}

impl PermissionBroker {
    /// Acquire a scoped lease. The callback establishes only the requested native capability.
    /// Slow OS interaction runs without holding the broker's global state lock.
    pub fn acquire(
        &self,
        request: &CapabilityRequest,
        operation: &OperationContext,
        execution: Option<&ExecutionGrant>,
        authorize: impl FnOnce(
            &CapabilityRequest,
            &OperationContext,
        ) -> Result<NativeSession, PermissionOutcome>,
    ) -> Result<CapabilityLease, PermissionOutcome> {
        if request.operation_id != operation.id
            || request.interaction != operation.interaction
            || request.subject.is_empty()
        {
            return Err(PermissionOutcome::PolicyBlocked);
        }
        if matches!(
            request.kind,
            CapabilityKind::FilesystemDelete | CapabilityKind::SystemLifecycle
        ) && !execution.is_some_and(|grant| grant.permits(request))
        {
            return Err(PermissionOutcome::PolicyBlocked);
        }
        let key = (request.subject.clone(), request.kind);
        let denied = (
            request.subject.clone(),
            operation.id.clone(),
            operation.generation,
            request.kind,
        );
        let epoch;
        loop {
            check_operation(operation)?;
            let mut state = self
                .state
                .lock()
                .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
            if state.denied.contains(&denied) {
                return Err(PermissionOutcome::Denied);
            }
            if state.settings_pending.contains(&denied) {
                return Err(PermissionOutcome::NeedsSystemSettings);
            }
            match state.sessions.get(&key) {
                Some(Slot::Ready(session)) if session.is_valid() => {
                    if request.interaction == Interaction::NonInteractive && !session.noninteractive
                    {
                        return Err(PermissionOutcome::NeedsAuthorization);
                    }
                    return Ok(CapabilityLease {
                        subject: request.subject.clone(),
                        kind: request.kind,
                        scope: request.scope.clone(),
                        epoch: state.epoch,
                        session: session.clone(),
                    });
                }
                Some(Slot::Authorizing) => {
                    drop(
                        self.changed
                            .wait_timeout(state, Duration::from_millis(20))
                            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?,
                    );
                    continue;
                }
                _ => {}
            }
            epoch = state.epoch;
            state.sessions.insert(key.clone(), Slot::Authorizing);
            break;
        }
        let outcome = authorize(request, operation).and_then(|session| {
            check_operation(operation)?;
            if !session.is_valid() {
                return Err(PermissionOutcome::AuthorizationUnavailable);
            }
            if request.interaction == Interaction::NonInteractive && !session.noninteractive {
                return Err(PermissionOutcome::NeedsAuthorization);
            }
            Ok(session)
        });
        let mut state = self
            .state
            .lock()
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
        if state.epoch != epoch {
            self.changed.notify_all();
            return Err(PermissionOutcome::AuthorizationUnavailable);
        }
        state.sessions.remove(&key);
        let result = match outcome {
            Ok(session) => {
                state.sessions.insert(key, Slot::Ready(session.clone()));
                Ok(CapabilityLease {
                    subject: request.subject.clone(),
                    kind: request.kind,
                    scope: request.scope.clone(),
                    epoch,
                    session,
                })
            }
            Err(outcome) => {
                if outcome == PermissionOutcome::Denied {
                    state.denied.insert(denied);
                } else if outcome == PermissionOutcome::NeedsSystemSettings {
                    state.settings_pending.insert(denied);
                }
                Err(outcome)
            }
        };
        self.changed.notify_all();
        result
    }

    fn epoch(&self) -> u64 {
        self.state
            .lock()
            .map(|state| state.epoch)
            .unwrap_or(u64::MAX)
    }

    /// Report actual connection availability without requesting authorization.
    pub fn is_active(&self, subject: &str, kind: CapabilityKind) -> bool {
        self.state.lock().is_ok_and(|state| {
            matches!(state.sessions.get(&(subject.to_owned(), kind)), Some(Slot::Ready(session)) if session.is_valid())
        })
    }

    /// Release all native connections and invalidate existing leases on logout or backend failure.
    pub fn revoke(&self) {
        let _ = self.revoke_sessions();
    }

    /// Invalidate leases and return cached native sessions for explicit platform shutdown.
    /// Owners must first stop in-flight users; handles are released outside the broker lock.
    pub fn revoke_sessions(&self) -> Result<Vec<NativeSession>, PermissionOutcome> {
        let sessions = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
            state.epoch = state.epoch.wrapping_add(1);
            state.settings_pending.clear();
            std::mem::take(&mut state.sessions)
        };
        self.changed.notify_all();
        Ok(sessions
            .into_values()
            .filter_map(|slot| match slot {
                Slot::Ready(session) => Some(session),
                Slot::Authorizing => None,
            })
            .collect())
    }

    /// Export refusal facts only; no native connection, credential or positive grant is persisted.
    pub fn denied_capabilities(
        &self,
        subject: &str,
        operation: &OperationContext,
    ) -> Vec<CapabilityKind> {
        self.state
            .lock()
            .map(|state| {
                state
                    .denied
                    .iter()
                    .filter(|(user, id, generation, _)| {
                        user == subject
                            && id == &operation.id
                            && *generation == operation.generation
                    })
                    .map(|(_, _, _, kind)| *kind)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Restore negative facts into a resumed operation without restoring any native capability.
    pub fn restore_denials(
        &self,
        subject: &str,
        operation: &OperationContext,
        denied: &[CapabilityKind],
    ) {
        if let Ok(mut state) = self.state.lock() {
            state.denied.extend(denied.iter().map(|kind| {
                (
                    subject.to_owned(),
                    operation.id.clone(),
                    operation.generation,
                    *kind,
                )
            }));
        }
    }
}

fn check_operation(operation: &OperationContext) -> Result<(), PermissionOutcome> {
    operation.check().map_err(|error| {
        if error.kind() == std::io::ErrorKind::Interrupted {
            PermissionOutcome::Cancelled
        } else {
            PermissionOutcome::TimedOut
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CancellationToken;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        thread,
    };

    #[test]
    fn cached_authentication_is_used_before_asking_and_expiry_prompts_once() {
        use std::cell::Cell;
        let operation = OperationContext::new(
            CancellationToken::default(),
            Interaction::Terminal,
            Duration::from_secs(2),
        );
        assert!(
            with_cached_terminal_authorization(
                &operation,
                || Ok(()),
                || panic!("Cached authorization needs no password")
            )
            .is_ok()
        );
        let probes = Cell::new(0);
        let asks = Cell::new(0);
        assert!(
            with_cached_terminal_authorization(
                &operation,
                || {
                    probes.set(probes.get() + 1);
                    if probes.get() == 1 {
                        Err(PermissionOutcome::NeedsAuthorization)
                    } else {
                        Ok(())
                    }
                },
                || {
                    asks.set(asks.get() + 1);
                    Ok(())
                }
            )
            .is_ok()
        );
        assert_eq!((probes.get(), asks.get()), (2, 1));
    }

    #[test]
    fn noninteractive_or_unavailable_authorization_does_not_ask_or_retry() {
        for interaction in [
            Interaction::NonInteractive,
            Interaction::Foreground,
            Interaction::Terminal,
        ] {
            let operation = OperationContext::new(
                CancellationToken::default(),
                interaction,
                Duration::from_secs(2),
            );
            let reason = if interaction == Interaction::Terminal {
                PermissionOutcome::AuthorizationUnavailable
            } else {
                PermissionOutcome::NeedsAuthorization
            };
            assert_eq!(
                with_cached_terminal_authorization::<()>(
                    &operation,
                    || Err(reason),
                    || panic!("No terminal prompt is allowed")
                ),
                Err(reason)
            );
        }
    }

    fn context() -> OperationContext {
        OperationContext::new(
            CancellationToken::default(),
            Interaction::Foreground,
            Duration::from_secs(2),
        )
    }
    fn request(context: &OperationContext) -> CapabilityRequest {
        CapabilityRequest {
            subject: "fixture-user/login".into(),
            operation_id: context.id.clone(),
            kind: CapabilityKind::MetadataRead,
            scope: CapabilityScope::Filesystem {
                roots: vec![crate::NativePath::from_path(std::path::Path::new(
                    "/fixture",
                ))],
            },
            interaction: context.interaction,
            reason_code: "scan".into(),
        }
    }

    #[test]
    fn concurrent_requests_share_one_native_session_and_revocation_invalidates_leases() {
        let broker = PermissionBroker::default();
        let operation = context();
        let request = request(&operation);
        let calls = AtomicUsize::new(0);
        thread::scope(|scope| {
            let workers = (0..10)
                .map(|_| {
                    scope.spawn(|| {
                        broker
                            .acquire(&request, &operation, None, |_, _| {
                                calls.fetch_add(1, Ordering::SeqCst);
                                thread::sleep(Duration::from_millis(30));
                                Ok(NativeSession::new(
                                    Arc::new(7_u32),
                                    Instant::now() + Duration::from_secs(30),
                                    false,
                                ))
                            })
                            .unwrap()
                    })
                })
                .collect::<Vec<_>>();
            let leases = workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert!(leases.iter().all(|lease| lease.covers(&request, &broker)));
            broker.revoke();
            assert!(leases.iter().all(|lease| !lease.covers(&request, &broker)));
        });
    }

    #[test]
    fn refusal_survives_scope_changes_and_restore_without_creating_a_positive_grant() {
        let broker = PermissionBroker::default();
        let operation = context();
        let mut request = request(&operation);
        assert_eq!(
            broker
                .acquire(&request, &operation, None, |_, _| Err(
                    PermissionOutcome::Denied
                ))
                .err(),
            Some(PermissionOutcome::Denied)
        );
        request.scope = CapabilityScope::Filesystem {
            roots: vec![crate::NativePath::from_path(std::path::Path::new(
                "/another-root",
            ))],
        };
        assert_eq!(
            broker
                .acquire(&request, &operation, None, |_, _| panic!(
                    "Denial must suppress another prompt"
                ))
                .err(),
            Some(PermissionOutcome::Denied)
        );
        let denied = broker.denied_capabilities(&request.subject, &operation);
        let restored = PermissionBroker::default();
        restored.restore_denials(&request.subject, &operation, &denied);
        assert_eq!(
            restored
                .acquire(&request, &operation, None, |_, _| panic!(
                    "Restore cannot create or request authorization"
                ))
                .err(),
            Some(PermissionOutcome::Denied)
        );
        request.kind = CapabilityKind::FilesystemDelete;
        assert_eq!(
            restored
                .acquire(&request, &operation, None, |_, _| panic!(
                    "Read permission is not deletion confirmation"
                ))
                .err(),
            Some(PermissionOutcome::PolicyBlocked)
        );
    }

    #[test]
    fn background_work_cannot_reuse_a_session_that_requires_foreground_interaction() {
        let broker = PermissionBroker::default();
        let mut operation = context();
        let mut request = request(&operation);
        broker
            .acquire(&request, &operation, None, |_, _| {
                Ok(NativeSession::new(
                    Arc::new(7_u32),
                    Instant::now() + Duration::from_secs(30),
                    false,
                ))
            })
            .unwrap();
        operation.interaction = Interaction::NonInteractive;
        request.interaction = Interaction::NonInteractive;
        assert_eq!(
            broker
                .acquire(&request, &operation, None, |_, _| panic!(
                    "Background work cannot prompt"
                ))
                .err(),
            Some(PermissionOutcome::NeedsAuthorization)
        );
    }
}
