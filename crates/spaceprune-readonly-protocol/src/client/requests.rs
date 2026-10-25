//! A close boundary for native requests, including authorization and connection queue waits.

use spaceprune_access::{OperationContext, PermissionOutcome};
use std::{
    collections::BTreeSet,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

#[derive(Default)]
struct State {
    closing: bool,
    shutdown_failure: Option<PermissionOutcome>,
    active: Vec<Arc<OperationContext>>,
    seen_operations: BTreeSet<(String, u64)>,
    revoked_operations: BTreeSet<(String, u64)>,
}

#[derive(Default)]
pub(super) struct Requests {
    state: Mutex<State>,
    changed: Condvar,
}

impl Requests {
    pub(super) fn enter(
        &self,
        operation: &OperationContext,
    ) -> Result<ActiveRequest<'_>, PermissionOutcome> {
        let mut state = self.state.lock().expect("metadata requests lock poisoned");
        loop {
            super::check(operation)?;
            if state
                .revoked_operations
                .contains(&(operation.id.clone(), operation.generation))
            {
                return Err(PermissionOutcome::Cancelled);
            }
            if let Some(failure) = state.shutdown_failure {
                return Err(failure);
            }
            if !state.closing {
                break;
            }
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(20))
                .expect("metadata requests lock poisoned")
                .0;
        }
        let operation = Arc::new(OperationContext {
            cancellation: operation.cancellation.child(),
            ..operation.clone()
        });
        state
            .seen_operations
            .insert((operation.id.clone(), operation.generation));
        state.active.push(Arc::clone(&operation));
        Ok(ActiveRequest {
            requests: self,
            operation,
        })
    }

    pub(super) fn check_operation(
        &self,
        operation: &OperationContext,
    ) -> Result<(), PermissionOutcome> {
        let state = self.state.lock().expect("metadata requests lock poisoned");
        if state
            .revoked_operations
            .contains(&(operation.id.clone(), operation.generation))
        {
            return Err(PermissionOutcome::Cancelled);
        }
        state.shutdown_failure.map_or(Ok(()), Err)
    }

    pub(super) fn reject_operation(&self, operation: &OperationContext) {
        self.state
            .lock()
            .expect("metadata requests lock poisoned")
            .revoked_operations
            .insert((operation.id.clone(), operation.generation));
    }

    pub(super) fn check(&self) -> Result<(), PermissionOutcome> {
        self.state
            .lock()
            .expect("metadata requests lock poisoned")
            .shutdown_failure
            .map_or(Ok(()), Err)
    }

    pub(super) fn record_shutdown(&self, result: Result<(), PermissionOutcome>) {
        if let Err(failure) = result {
            self.state
                .lock()
                .expect("metadata requests lock poisoned")
                .shutdown_failure
                .get_or_insert(failure);
        }
    }

    pub(super) fn close(
        &self,
        release_connections: impl FnOnce() -> Result<(), PermissionOutcome>,
    ) -> Result<(), PermissionOutcome> {
        self.close_inner(false, release_connections)
    }

    pub(super) fn revoke_login(
        &self,
        release_connections: impl FnOnce() -> Result<(), PermissionOutcome>,
    ) -> Result<(), PermissionOutcome> {
        self.close_inner(true, release_connections)
    }

    fn close_inner(
        &self,
        revoke_operations: bool,
        release_connections: impl FnOnce() -> Result<(), PermissionOutcome>,
    ) -> Result<(), PermissionOutcome> {
        let mut state = self.state.lock().expect("metadata requests lock poisoned");
        while state.closing {
            state = self
                .changed
                .wait(state)
                .expect("metadata requests lock poisoned");
        }
        state.closing = true;
        if revoke_operations {
            let seen = std::mem::take(&mut state.seen_operations);
            state.revoked_operations.extend(seen);
        }
        for operation in &state.active {
            operation.cancel();
        }
        while !state.active.is_empty() {
            state = self
                .changed
                .wait(state)
                .expect("metadata requests lock poisoned");
        }
        // Requests release their native leases before their registration. Revoke only after
        // that barrier so no stale authorization can reenter the broker during close.
        drop(state);
        self.record_shutdown(release_connections());
        let mut state = self.state.lock().expect("metadata requests lock poisoned");
        state.closing = false;
        self.changed.notify_all();
        state.shutdown_failure.map_or(Ok(()), Err)
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn active_count(&self) -> usize {
        self.state.lock().unwrap().active.len()
    }
}

pub(super) struct ActiveRequest<'a> {
    requests: &'a Requests,
    operation: Arc<OperationContext>,
}

impl ActiveRequest<'_> {
    pub(super) fn operation(&self) -> &OperationContext {
        &self.operation
    }
}

impl Drop for ActiveRequest<'_> {
    fn drop(&mut self) {
        let mut state = self
            .requests
            .state
            .lock()
            .expect("metadata requests lock poisoned");
        state
            .active
            .retain(|operation| !Arc::ptr_eq(operation, &self.operation));
        self.requests.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spaceprune_access::{CancellationToken, Interaction};
    use std::{sync::mpsc, thread, time::Instant};

    fn operation() -> OperationContext {
        let mut operation = OperationContext::new(
            CancellationToken::default(),
            Interaction::NonInteractive,
            Duration::from_secs(5),
        );
        operation.generation = 7;
        operation.budget.output_bytes = 12_345;
        operation
    }

    #[test]
    fn close_waits_for_old_work_and_new_requests_keep_their_original_context() {
        let requests = Requests::default();
        let original = operation();
        let active = requests.enter(&original).unwrap();
        let (finished, completion) = mpsc::channel();
        thread::scope(|scope| {
            let close = scope.spawn(|| {
                requests.close(|| {
                    finished.send(()).unwrap();
                    Ok(())
                })
            });
            let deadline = Instant::now() + Duration::from_secs(2);
            while !active.operation().is_stopped() {
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(5));
            }
            assert!(!original.is_stopped());
            assert!(completion.try_recv().is_err());
            let (entered, entering) = mpsc::channel();
            let requests = &requests;
            let original = &original;
            let next = scope.spawn(move || {
                let active = requests.enter(original).unwrap();
                entered.send(()).unwrap();
                assert_eq!(active.operation().identity(), original.identity());
                assert_eq!(active.operation().deadline, original.deadline);
                assert_eq!(active.operation().interaction, Interaction::NonInteractive);
                assert_eq!(active.operation().budget.output_bytes, 12_345);
                assert!(!active.operation().is_stopped());
            });
            assert!(entering.recv_timeout(Duration::from_millis(50)).is_err());
            drop(active);
            completion.recv_timeout(Duration::from_secs(2)).unwrap();
            close.join().unwrap().unwrap();
            next.join().unwrap();
        });
        assert!(!original.is_stopped());
    }

    #[test]
    fn caller_cancellation_and_deadlines_still_apply_while_close_is_draining() {
        let requests = Requests::default();
        let original = operation();
        let active = requests.enter(&original).unwrap();
        thread::scope(|scope| {
            let close = scope.spawn(|| requests.close(|| Ok(())));
            let deadline = Instant::now() + Duration::from_secs(2);
            while !active.operation().is_stopped() {
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(5));
            }
            let cancelled = operation();
            let signal = cancelled.cancellation.clone();
            scope.spawn(move || {
                thread::sleep(Duration::from_millis(20));
                signal.cancel();
            });
            assert_eq!(
                requests.enter(&cancelled).err(),
                Some(PermissionOutcome::Cancelled)
            );
            let expired = OperationContext {
                deadline: Instant::now() + Duration::from_millis(30),
                ..operation()
            };
            assert_eq!(
                requests.enter(&expired).err(),
                Some(PermissionOutcome::TimedOut)
            );
            drop(active);
            close.join().unwrap().unwrap();
        });
        let parent = operation();
        let active = requests.enter(&parent).unwrap();
        parent.cancel();
        assert!(active.operation().is_stopped());
    }
    #[test]
    fn login_revocation_rejects_old_identity_even_after_ordinary_close() {
        let requests = Requests::default();
        let original = operation();
        drop(requests.enter(&original).unwrap());
        requests.revoke_login(|| Ok(())).unwrap();
        requests.close(|| Ok(())).unwrap();
        assert_eq!(
            requests.check_operation(&original),
            Err(PermissionOutcome::Cancelled)
        );
        assert_eq!(
            requests.enter(&original).err(),
            Some(PermissionOutcome::Cancelled)
        );
        let next_generation = OperationContext {
            generation: original.generation + 1,
            ..original.clone()
        };
        assert!(requests.enter(&next_generation).is_ok());
        assert!(!original.is_stopped());
    }
}
