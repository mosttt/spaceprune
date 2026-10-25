//! Operation identity, cancellation and finite resource limits propagated through native work.

use spaceprune_contracts::messages;
mod transfer;

use spaceprune_contracts::access::{Interaction, OperationIdentity, OperationStopReason};
use std::{
    io,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// A shared cooperative cancellation signal; cloning retains the original operation.
#[derive(Debug, Clone)]
pub struct CancellationToken {
    state: Arc<CancellationState>,
    operation_id: Arc<str>,
}

#[derive(Debug)]
struct CancellationState {
    cancelled: Arc<AtomicBool>,
    children: Mutex<Vec<Weak<CancellationState>>>,
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::from_signal(Arc::new(AtomicBool::new(false)))
    }
}

impl CancellationToken {
    /// Adopt a host cancellation signal; retain this token for the full operation identity.
    pub fn from_signal(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            state: Arc::new(CancellationState {
                cancelled,
                children: Mutex::new(Vec::new()),
            }),
            operation_id: uuid::Uuid::new_v4().to_string().into(),
        }
    }

    /// Create child work with the same identity. Cancelling it does not cancel siblings.
    pub fn child(&self) -> Self {
        self.child_with_signal(Arc::new(AtomicBool::new(false)))
    }

    /// Start a distinct operation that remains linked to this owner's cancellation.
    pub fn new_operation(&self) -> Self {
        let mut operation = self.child();
        operation.operation_id = uuid::Uuid::new_v4().to_string().into();
        operation
    }

    /// Bind a runtime's cancellation flag without losing the parent operation or cancellation.
    pub fn child_with_signal(&self, cancelled: Arc<AtomicBool>) -> Self {
        let state = Arc::new(CancellationState {
            cancelled,
            children: Mutex::new(Vec::new()),
        });
        let mut children = self
            .state
            .children
            .lock()
            .expect("cancellation children lock poisoned");
        children.retain(|child| child.strong_count() > 0);
        children.push(Arc::downgrade(&state));
        if self.is_cancelled() {
            state.cancelled.store(true, Ordering::Release);
        }
        Self {
            state,
            operation_id: Arc::clone(&self.operation_id),
        }
    }

    /// Stable identity shared by all clones and child work of this operation.
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    /// Request cancellation. Completed effects are not rolled back.
    pub fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::Release);
        let children = self
            .state
            .children
            .lock()
            .expect("cancellation children lock poisoned")
            .iter()
            .filter_map(Weak::upgrade)
            .collect::<Vec<_>>();
        for state in children {
            Self {
                state,
                operation_id: Arc::clone(&self.operation_id),
            }
            .cancel();
        }
    }

    /// Whether the caller requested cancellation.
    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire)
    }

    /// Borrow the signal for native backends. Request cancellation through `cancel()` so it
    /// reaches child signals as well; writing to the borrowed atomic does not notify children.
    pub fn signal(&self) -> &AtomicBool {
        &self.state.cancelled
    }

    /// Stop traversal at a cooperative boundary.
    pub fn check(&self) -> io::Result<()> {
        if self.is_cancelled() {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                messages::text("native.operation_cancelled"),
            ))
        } else {
            Ok(())
        }
    }
}

/// Resource limits for one native operation or controlled process.
#[derive(Debug, Clone, Copy)]
pub struct ResourceBudget {
    /// Total captured output bytes across stdout and stderr.
    pub output_bytes: usize,
    /// Maximum length of one JSONL/protocol line.
    pub line_bytes: usize,
    /// Maximum delay before any process output is observed.
    pub first_output_timeout: Duration,
    /// Maximum gap between process output chunks.
    pub idle_timeout: Duration,
}

impl Default for ResourceBudget {
    fn default() -> Self {
        Self {
            output_bytes: 8 * 1024 * 1024,
            line_bytes: 1024 * 1024,
            first_output_timeout: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(60),
        }
    }
}

impl ResourceBudget {
    /// Limits for a full scan or confirmed plan, whose result can contain many selected items.
    pub fn workflow() -> Self {
        Self {
            output_bytes: 64 * 1024 * 1024,
            line_bytes: 64 * 1024 * 1024,
            first_output_timeout: Duration::from_secs(300),
            idle_timeout: Duration::from_secs(300),
        }
    }
    /// Restrict local limits without increasing any caller-supplied budget.
    pub fn restricted(self, local: Self) -> Self {
        Self {
            output_bytes: self.output_bytes.min(local.output_bytes),
            line_bytes: self.line_bytes.min(local.line_bytes),
            first_output_timeout: self.first_output_timeout.min(local.first_output_timeout),
            idle_timeout: self.idle_timeout.min(local.idle_timeout),
        }
    }
}

/// Shared context for a single user operation, including native authorization waits.
#[derive(Debug, Clone)]
pub struct OperationContext {
    /// Stable identity used for denial suppression and events.
    pub id: String,
    /// Generation of a restarted attempt; old events cannot advance this attempt.
    pub generation: u64,
    /// Original cancellation signal, shared by all child work.
    pub cancellation: CancellationToken,
    /// Absolute monotonic deadline; child work cannot extend it.
    pub deadline: Instant,
    /// Foreground, terminal or noninteractive authorization policy.
    pub interaction: Interaction,
    /// Finite resource limits for native work.
    pub budget: ResourceBudget,
}

impl Default for OperationContext {
    fn default() -> Self {
        Self::workflow(CancellationToken::default(), Interaction::NonInteractive)
    }
}

impl OperationContext {
    /// Start a finite application workflow; narrower native calls and tool budgets remain bounded.
    pub fn workflow(cancellation: CancellationToken, interaction: Interaction) -> Self {
        Self {
            budget: ResourceBudget::workflow(),
            ..Self::new(cancellation, interaction, Duration::from_secs(3600))
        }
    }
    /// Return the stable correlation carried into events and child work.
    pub fn identity(&self) -> OperationIdentity {
        OperationIdentity {
            operation_id: self.id.clone(),
            generation: self.generation,
        }
    }

    /// Request cancellation without undoing completed effects.
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// Whether cancellation or the original deadline prevents further work.
    pub fn is_stopped(&self) -> bool {
        self.stop_reason().is_some()
    }

    /// Distinguish user cancellation from the operation's execution deadline.
    pub fn stop_reason(&self) -> Option<OperationStopReason> {
        if self.cancellation.is_cancelled() {
            Some(OperationStopReason::Cancelled)
        } else if Instant::now() >= self.deadline {
            Some(OperationStopReason::DeadlineExceeded)
        } else {
            None
        }
    }
    /// Start an operation with a caller-owned cancellation signal.
    pub fn new(
        cancellation: CancellationToken,
        interaction: Interaction,
        timeout: Duration,
    ) -> Self {
        Self {
            id: cancellation.operation_id().to_owned(),
            generation: 0,
            cancellation,
            deadline: Instant::now() + timeout,
            interaction,
            budget: ResourceBudget::default(),
        }
    }

    /// Limit a native child without renewing the caller's deadline or identity.
    pub fn child(&self, timeout: Duration) -> Self {
        Self {
            cancellation: self.cancellation.child(),
            deadline: self.deadline.min(Instant::now() + timeout),
            ..self.clone()
        }
    }

    /// Check both the original signal and total deadline.
    pub fn check(&self) -> io::Result<()> {
        self.cancellation.check()?;
        if Instant::now() >= self.deadline {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                messages::text("native.operation_deadline"),
            ))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn children_keep_identity_and_parent_cancellation_without_cancelling_siblings() {
        let parent = CancellationToken::default();
        let external = Arc::new(AtomicBool::new(false));
        let first = parent.child_with_signal(external.clone());
        let second = parent.child();
        let grandchild = second.child();
        first.cancel();
        assert!(external.load(Ordering::Acquire));
        assert!(!parent.is_cancelled());
        assert!(!second.is_cancelled());
        parent.cancel();
        assert!(second.is_cancelled());
        assert!(grandchild.is_cancelled());
        assert!(parent.child().is_cancelled());
        assert_eq!(parent.operation_id(), grandchild.operation_id());
    }

    #[test]
    fn native_child_cannot_renew_an_expired_deadline_or_generation() {
        let mut parent = OperationContext::new(
            CancellationToken::default(),
            Interaction::NonInteractive,
            Duration::ZERO,
        );
        parent.generation = 7;
        let child = parent.child(Duration::from_secs(120));
        assert_eq!(child.id, parent.id);
        assert_eq!(child.generation, 7);
        assert_eq!(child.deadline, parent.deadline);
        assert_eq!(child.check().unwrap_err().kind(), io::ErrorKind::TimedOut);
    }
}
