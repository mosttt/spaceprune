//! Ephemeral OS login binding and monotonic invalidation of native capability reuse.

use crate::{OperationContext, PermissionOutcome};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[cfg(target_os = "linux")]
#[path = "login/linux.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "login/windows.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "login/macos.rs"]
mod platform;

struct RevocationState {
    revoked: AtomicBool,
    on_revoke: Box<dyn Fn() + Send + Sync>,
}

/// A one-way native notification. Returning to the same login cannot revive this binding.
#[derive(Clone)]
pub struct LoginRevocation(Arc<RevocationState>);
impl LoginRevocation {
    /// Latch invalidation before asking the owner to cancel and close its native work.
    /// The callback runs on the platform monitor; it must not drop its own LoginBinding.
    pub fn invalidate(&self) {
        if !self.0.revoked.swap(true, Ordering::AcqRel) {
            (self.0.on_revoke)();
        }
    }
    /// Whether this particular binding has been invalidated, even if that login is active again.
    pub fn is_revoked(&self) -> bool {
        self.0.revoked.load(Ordering::Acquire)
    }
}

pub(super) struct NativeBinding {
    pub(super) subject: String,
    pub(super) login_bound: bool,
    pub(super) _monitor: Box<dyn Send + Sync>,
}

/// A live native login observation, never a serialized grant or proof of resource access.
pub struct LoginBinding {
    native: NativeBinding,
    revocation: LoginRevocation,
}
impl LoginBinding {
    /// OS-derived user/login identity used to key the in-memory capability broker.
    pub fn subject(&self) -> &str {
        &self.native.subject
    }
    /// Whether the OS identified a login, rather than only the existing process lifetime.
    pub fn is_login_bound(&self) -> bool {
        self.native.login_bound
    }
    /// Clone the monotonic revocation signal without extending the monitor lifetime.
    pub fn revocation(&self) -> LoginRevocation {
        self.revocation.clone()
    }
    /// Whether this binding has been irreversibly invalidated.
    pub fn is_revoked(&self) -> bool {
        self.revocation.is_revoked()
    }
}

/// Bind the current native execution context without requesting any authorization.
/// Foreground/terminal bindings require an active login; independent background work only tracks
/// that login's lifetime. Failed monitors are unavailable; confirmed no-login contexts retain
/// process-scoped operation. Owners must finish old work before replacing a revoked binding.
pub fn bind(
    operation: &OperationContext,
    on_revoke: impl Fn() + Send + Sync + 'static,
) -> Result<LoginBinding, PermissionOutcome> {
    operation.check().map_err(|error| {
        if error.kind() == std::io::ErrorKind::Interrupted {
            PermissionOutcome::Cancelled
        } else {
            PermissionOutcome::TimedOut
        }
    })?;
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        let _ = on_revoke;
        Err(PermissionOutcome::Unsupported)
    }
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    {
        let revocation = LoginRevocation(Arc::new(RevocationState {
            revoked: AtomicBool::new(false),
            on_revoke: Box::new(on_revoke),
        }));
        let native = platform::bind(operation, revocation.clone())?;
        if revocation.is_revoked() {
            return Err(PermissionOutcome::AuthorizationUnavailable);
        }
        Ok(LoginBinding { native, revocation })
    }
}
