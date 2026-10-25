//! Ordered, budgeted progress uses the same authenticated conversation as its final result.

use super::*;
use std::sync::Mutex;

/// A fixed native handler's progress channel. It grants no additional operation or scope.
pub trait EventSink: Send + Sync {
    /// Publish one event under the original operation budget. Failure must stop further work.
    fn emit(&self, event: serde_json::Value) -> Result<(), PermissionOutcome>;
}
impl<F> EventSink for F
where
    F: Fn(serde_json::Value) -> Result<(), PermissionOutcome> + Send + Sync,
{
    fn emit(&self, event: serde_json::Value) -> Result<(), PermissionOutcome> {
        self(event)
    }
}

struct State<'a> {
    channel: &'a mut Channel,
    next: u64,
    bytes: usize,
}
pub(super) struct EventWriter<'a, F> {
    state: Mutex<State<'a>>,
    operation: &'a OperationContext,
    frame: F,
}
impl<'a, F> EventWriter<'a, F>
where
    F: Fn(u64, bool, Result<serde_json::Value, PermissionOutcome>) -> serde_json::Value,
{
    pub(super) fn new(channel: &'a mut Channel, operation: &'a OperationContext, frame: F) -> Self {
        Self {
            state: Mutex::new(State {
                channel,
                next: 0,
                bytes: 0,
            }),
            operation,
            frame,
        }
    }
    pub(super) fn finish(
        &self,
        result: Result<serde_json::Value, PermissionOutcome>,
    ) -> Result<(), PermissionOutcome> {
        self.write(false, result)
    }
    fn write(
        &self,
        progress: bool,
        result: Result<serde_json::Value, PermissionOutcome>,
    ) -> Result<(), PermissionOutcome> {
        let result = (|| {
            check(self.operation)?;
            let mut state = self
                .state
                .lock()
                .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
            let frame = (self.frame)(state.next, progress, result);
            let bytes = serde_json::to_vec(&frame)
                .map_err(|_| PermissionOutcome::PolicyBlocked)?
                .len();
            state.bytes = state
                .bytes
                .checked_add(bytes)
                .ok_or(PermissionOutcome::PolicyBlocked)?;
            if bytes > self.operation.budget.line_bytes
                || state.bytes > self.operation.budget.output_bytes
            {
                return Err(PermissionOutcome::PolicyBlocked);
            }
            state
                .channel
                .write(&frame, self.operation)
                .map_err(|error| io_outcome(error, self.operation))?;
            if progress {
                state.next = state
                    .next
                    .checked_add(1)
                    .ok_or(PermissionOutcome::PolicyBlocked)?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.operation.cancel();
        }
        result
    }
}
impl<F> EventSink for EventWriter<'_, F>
where
    F: Fn(u64, bool, Result<serde_json::Value, PermissionOutcome>) -> serde_json::Value
        + Send
        + Sync,
{
    fn emit(&self, event: serde_json::Value) -> Result<(), PermissionOutcome> {
        self.write(true, Ok(event))
    }
}

#[derive(Default)]
pub(super) struct StreamBudget {
    next: u64,
    bytes: usize,
}
impl StreamBudget {
    pub(super) fn accept(
        &mut self,
        index: u64,
        progress: bool,
        value: &impl Serialize,
        context: &OperationContext,
    ) -> Result<(), PermissionOutcome> {
        let size = serde_json::to_vec(value)
            .map_err(|_| PermissionOutcome::PolicyBlocked)?
            .len();
        self.bytes = self
            .bytes
            .checked_add(size)
            .ok_or(PermissionOutcome::PolicyBlocked)?;
        if index != self.next
            || size > context.budget.line_bytes
            || self.bytes > context.budget.output_bytes
        {
            return Err(PermissionOutcome::PolicyBlocked);
        }
        if progress {
            self.next = self
                .next
                .checked_add(1)
                .ok_or(PermissionOutcome::PolicyBlocked)?;
        }
        Ok(())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn streaming_output_cannot_renew_its_budget_or_replay_a_progress_frame() {
        let (left, right) = UnixStream::pair().unwrap();
        let mut sender = Channel::new(left).unwrap();
        let mut receiver = Channel::new(right).unwrap();
        let mut operation = OperationContext::default();
        operation.budget.line_bytes = 200;
        operation.budget.output_bytes = 200;
        let writer = EventWriter::new(
            &mut sender,
            &operation,
            |index, progress, result| serde_json::json!({"index":index,"progress":progress,"result":result}),
        );
        writer.emit(serde_json::json!("x".repeat(80))).unwrap();
        assert_eq!(
            writer.emit(serde_json::json!("x".repeat(80))),
            Err(PermissionOutcome::PolicyBlocked)
        );
        assert!(operation.is_stopped());
        let read = OperationContext::default();
        let frame: serde_json::Value = receiver.read(&read).unwrap();
        let mut budget = StreamBudget::default();
        budget.accept(0, true, &frame, &read).unwrap();
        assert_eq!(
            budget.accept(0, true, &frame, &read),
            Err(PermissionOutcome::PolicyBlocked)
        );
        assert_eq!(
            budget.accept(2, false, &frame, &read),
            Err(PermissionOutcome::PolicyBlocked)
        );
        budget
            .accept(1, false, &serde_json::Value::Null, &read)
            .unwrap();
    }
}
