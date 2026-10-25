//! One-use handoffs are tied to a verified host and original operation, not a caller's role flag.
use super::*;
use crate::macos::principal::ProcessIdentity;
use std::{
    cell::Cell,
    collections::HashMap,
    sync::{Arc, Mutex, mpsc},
};

#[derive(Clone, Copy)]
pub(super) struct Caller {
    pub(super) identity: ProcessIdentity,
    #[cfg(target_os = "macos")]
    pub(super) peer: peer::Peer,
}

#[derive(Clone, Default)]
pub(super) struct Registry(Arc<Mutex<HashMap<String, Waiting>>>);
#[derive(Clone)]
struct Waiting {
    caller: Caller,
    operation: OperationContext,
    sender: mpsc::SyncSender<Delivery>,
}
struct Delivery {
    credential: ExternalAuthorization,
    acknowledged: mpsc::SyncSender<Result<(), PermissionOutcome>>,
}

pub(super) struct Pending {
    registry: Registry,
    nonce: String,
    receiver: mpsc::Receiver<Delivery>,
    operation: OperationContext,
    finished: Cell<bool>,
}

impl Registry {
    pub(super) fn prepare(
        &self,
        nonce: &str,
        caller: Caller,
        operation: &OperationContext,
    ) -> Result<Pending, PermissionOutcome> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut waiting = self
            .0
            .lock()
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
        if waiting.contains_key(nonce) {
            return Err(PermissionOutcome::PolicyBlocked);
        }
        waiting.insert(
            nonce.into(),
            Waiting {
                caller,
                operation: operation.clone(),
                sender,
            },
        );
        Ok(Pending {
            registry: self.clone(),
            nonce: nonce.into(),
            receiver,
            operation: operation.clone(),
            finished: Cell::new(false),
        })
    }

    // Only the native listener's verified authorizer branch calls this. Tests substitute that
    // OS boundary explicitly; no deserialized role, PID, UID or "authorized" value is accepted.
    pub(super) fn claim(
        &self,
        nonce: &str,
        operation: &OperationIdentity,
        verify_agent: impl FnOnce(Caller) -> Result<(), PermissionOutcome>,
    ) -> Result<Claim, PermissionOutcome> {
        let waiting = self
            .0
            .lock()
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?
            .get(nonce)
            .cloned()
            .ok_or(PermissionOutcome::HelperUntrusted)?;
        if waiting.operation.identity() != *operation {
            return Err(PermissionOutcome::HelperUntrusted);
        }
        check(&waiting.operation)?;
        verify_agent(waiting.caller)?;
        check(&waiting.operation)?;
        let waiting = self
            .0
            .lock()
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?
            .remove(nonce)
            .ok_or(PermissionOutcome::HelperUntrusted)?;
        Ok(Claim(waiting))
    }
}

pub(super) struct Claim(Waiting);
impl Claim {
    pub(super) fn operation(&self) -> &OperationContext {
        &self.0.operation
    }
    pub(super) fn caller(&self) -> Caller {
        self.0.caller
    }
    pub(super) fn deliver(
        self,
        credential: ExternalAuthorization,
    ) -> Result<(), PermissionOutcome> {
        check(&self.0.operation)?;
        let (acknowledged, receiver) = mpsc::sync_channel(1);
        self.0
            .sender
            .try_send(Delivery {
                credential,
                acknowledged,
            })
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
        loop {
            check(&self.0.operation)?;
            match receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(PermissionOutcome::AuthorizationUnavailable);
                }
            }
        }
    }
}

impl Pending {
    pub(super) fn receive<Authority>(
        &self,
        channel: &Channel,
        adopt: impl FnOnce(&ExternalAuthorization) -> Result<Authority, PermissionOutcome>,
    ) -> Result<Authority, PermissionOutcome> {
        let mut monitor = channel
            .monitor()
            .map_err(|error| io_outcome(error, &self.operation))?;
        let delivery = loop {
            check(&self.operation)?;
            if !monitor.is_alive() {
                self.operation.cancel();
                return Err(PermissionOutcome::Cancelled);
            }
            match self.receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(delivery) => break delivery,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(PermissionOutcome::AuthorizationUnavailable);
                }
            }
        };
        let authority = adopt(&delivery.credential).and_then(|authority| {
            if !monitor.is_alive() {
                self.operation.cancel();
            }
            check(&self.operation)?;
            Ok(authority)
        });
        let acknowledgement = authority.as_ref().map(|_| ()).map_err(|outcome| *outcome);
        delivery
            .acknowledged
            .send(acknowledgement)
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
        self.finished.set(true);
        authority
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        if !self.finished.get() {
            self.operation.cancel();
        }
        if let Ok(mut waiting) = self.registry.0.lock() {
            waiting.remove(&self.nonce);
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    fn caller() -> Caller {
        Caller {
            identity: ProcessIdentity {
                pid: 501,
                effective_uid: 1001,
                real_uid: 1001,
                audit_session: 9,
            },
        }
    }
    #[test]
    fn mismatched_issuers_do_not_consume_a_challenge_but_a_valid_claim_is_one_use() {
        let registry = Registry::default();
        let operation = OperationContext::default();
        let pending = registry.prepare("fixture", caller(), &operation).unwrap();
        let mut foreign = caller().identity;
        foreign.audit_session = 10;
        assert!(
            registry
                .claim("fixture", &operation.identity(), |owner| {
                    if foreign.same_login(owner.identity) {
                        Ok(())
                    } else {
                        Err(PermissionOutcome::HelperUntrusted)
                    }
                })
                .is_err()
        );
        let claim = registry
            .claim("fixture", &operation.identity(), |_| Ok(()))
            .unwrap();
        assert!(
            registry
                .claim("fixture", &operation.identity(), |_| Ok(()))
                .is_err()
        );
        drop(pending);
        assert!(claim.operation().is_stopped());
    }
}
