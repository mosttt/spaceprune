//! Observe the original connection throughout work, including blocking child RPCs.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

struct Finished<'a>(&'a AtomicBool);
impl Drop for Finished<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(super) fn while_connected<T>(
    channel: &Channel,
    operation: &OperationContext,
    run: impl FnOnce() -> Result<T, PermissionOutcome>,
) -> Result<T, PermissionOutcome> {
    check_connection(channel, operation)?;
    let monitor = channel
        .monitor()
        .map_err(|error| io_outcome(error, operation))?;
    let value = supervise(monitor, operation, run)?;
    check_connection(channel, operation)?;
    Ok(value)
}

pub(super) fn supervise<T>(
    mut monitor: crate::macos::channel::Monitor,
    operation: &OperationContext,
    run: impl FnOnce() -> Result<T, PermissionOutcome>,
) -> Result<T, PermissionOutcome> {
    check(operation)?;
    if !monitor.is_alive() {
        operation.cancel();
        return Err(PermissionOutcome::Cancelled);
    }
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::Acquire) {
                if operation.is_stopped() {
                    break;
                }
                if !monitor.is_alive() {
                    operation.cancel();
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let _finished = Finished(&done);
        run()
    })
}

pub(super) fn check_connection(
    channel: &Channel,
    operation: &OperationContext,
) -> Result<(), PermissionOutcome> {
    let mut probe = channel
        .monitor()
        .map_err(|error| io_outcome(error, operation))?;
    if !probe.is_alive() {
        operation.cancel();
    }
    check(operation)
}
