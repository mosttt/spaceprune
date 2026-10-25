//! Same-host IPC preserves a monotonic deadline, including time spent queued or authenticating.

use super::*;
use spaceprune_contracts::access::OperationTransfer;

#[cfg(unix)]
fn monotonic_ns() -> io::Result<u64> {
    let time = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    u64::try_from(time.tv_sec)
        .ok()
        .and_then(|seconds| seconds.checked_mul(1_000_000_000))
        .and_then(|value| value.checked_add(time.tv_nsec as u64))
        .ok_or_else(|| io::Error::other("System monotonic clock is outside its supported range"))
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn monotonic_ns() -> io::Result<u64> {
    use windows_sys::Win32::System::Performance::{
        QueryPerformanceCounter, QueryPerformanceFrequency,
    };
    let mut counter = 0;
    let mut frequency = 0;
    // SAFETY: Both APIs write one i64 to live stack storage and do not retain the pointers.
    if unsafe {
        QueryPerformanceCounter(&mut counter) == 0 || QueryPerformanceFrequency(&mut frequency) == 0
    } || counter < 0
        || frequency <= 0
    {
        return Err(io::Error::last_os_error());
    }
    u64::try_from((counter as u128) * 1_000_000_000 / frequency as u128)
        .map_err(|_| io::Error::other("System monotonic clock is outside its supported range"))
}

#[cfg(not(any(unix, windows)))]
fn monotonic_ns() -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Native IPC requires a system monotonic clock",
    ))
}

impl OperationContext {
    /// Export limits for a same-host process. Transport must separately deliver cancellation.
    pub fn transfer(&self) -> io::Result<OperationTransfer> {
        self.check()?;
        let now = monotonic_ns()?;
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        let language = spaceprune_contracts::messages::display_language();
        Ok(OperationTransfer {
            display_language: (language != "en").then(|| language.to_owned()),
            identity: self.identity(),
            deadline_monotonic_ns: now
                .saturating_add(remaining.as_nanos().min(u64::MAX as u128) as u64),
            output_bytes: self.budget.output_bytes as u64,
            line_bytes: self.budget.line_bytes as u64,
            first_output_timeout_ms: self
                .budget
                .first_output_timeout
                .as_millis()
                .min(u64::MAX as u128) as u64,
            idle_timeout_ms: self.budget.idle_timeout.as_millis().min(u64::MAX as u128) as u64,
        })
    }

    /// Import an operation into a noninteractive worker, restricted by the receiver's limits.
    /// The transfer is not a permission grant; caller identity and scope require separate checks.
    pub fn from_transfer(
        transfer: &OperationTransfer,
        timeout: Duration,
        budget: ResourceBudget,
    ) -> io::Result<Self> {
        let started = Instant::now();
        let remaining = transfer
            .deadline_monotonic_ns
            .saturating_sub(monotonic_ns()?);
        // Round down rather than extending work through clock conversion precision.
        let remaining = Duration::from_nanos(remaining).saturating_sub(Duration::from_micros(1));
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Operation deadline exceeded",
            ));
        }
        if transfer.identity.operation_id.is_empty() || transfer.identity.operation_id.len() > 128 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid operation identity",
            ));
        }
        let cancellation = CancellationToken {
            operation_id: transfer.identity.operation_id.clone().into(),
            ..CancellationToken::default()
        };
        // Native sessions import each request separately, including changes on a reused connection.
        spaceprune_contracts::messages::set_display_language(
            transfer.display_language.as_deref().unwrap_or("en"),
        );
        Ok(Self {
            id: transfer.identity.operation_id.clone(),
            generation: transfer.identity.generation,
            cancellation,
            interaction: Interaction::NonInteractive,
            deadline: started + remaining.min(timeout),
            budget: ResourceBudget {
                output_bytes: usize::try_from(transfer.output_bytes).unwrap_or(usize::MAX),
                line_bytes: usize::try_from(transfer.line_bytes).unwrap_or(usize::MAX),
                first_output_timeout: Duration::from_millis(transfer.first_output_timeout_ms),
                idle_timeout: Duration::from_millis(transfer.idle_timeout_ms),
            }
            .restricted(budget),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_language_crosses_requests_without_changing_identity_or_budget() {
        const CHILD: &str = "SPACEPRUNE_TEST_LANGUAGE_TRANSFER";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "operation::transfer::tests::display_language_crosses_requests_without_changing_identity_or_budget"])
                .env(CHILD, "1").output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            return;
        }
        let original = OperationContext::default();
        for language in ["zh-CN", "en", "zh-CN"] {
            spaceprune_contracts::messages::set_display_language(language);
            let wire = original.transfer().unwrap();
            let encoded = serde_json::to_value(&wire).unwrap();
            assert_eq!(encoded.get("display_language").is_some(), language != "en");
            spaceprune_contracts::messages::set_display_language("en");
            let imported = OperationContext::from_transfer(
                &wire,
                Duration::from_secs(30),
                ResourceBudget::default(),
            )
            .unwrap();
            assert_eq!(spaceprune_contracts::messages::display_language(), language);
            assert_eq!(imported.identity(), original.identity());
            assert!(imported.deadline <= original.deadline);
        }
        let mut wire = original.transfer().unwrap();
        wire.display_language = Some("future-language".into());
        OperationContext::from_transfer(&wire, Duration::from_secs(30), ResourceBudget::default())
            .unwrap();
        assert_eq!(spaceprune_contracts::messages::display_language(), "en");
    }

    #[test]
    fn queued_transfer_keeps_identity_and_does_not_renew_or_expand_limits() {
        let mut original = OperationContext::new(
            CancellationToken::default(),
            Interaction::Foreground,
            Duration::from_secs(1),
        );
        original.generation = 9;
        original.budget.output_bytes = 1234;
        let wire = original.transfer().unwrap();
        std::thread::sleep(Duration::from_millis(15));
        let imported = OperationContext::from_transfer(
            &wire,
            Duration::from_secs(10),
            ResourceBudget::default(),
        )
        .unwrap();
        assert_eq!(imported.identity(), original.identity());
        assert_eq!(
            imported.cancellation.operation_id(),
            original.cancellation.operation_id()
        );
        assert!(imported.deadline <= original.deadline);
        assert_eq!(imported.budget.output_bytes, 1234);
        assert_eq!(imported.interaction, Interaction::NonInteractive);
        let restricted = OperationContext::from_transfer(
            &wire,
            Duration::from_millis(10),
            ResourceBudget {
                output_bytes: 100,
                ..ResourceBudget::default()
            },
        )
        .unwrap();
        assert!(restricted.deadline < imported.deadline);
        assert_eq!(restricted.budget.output_bytes, 100);
        let mut expired = wire;
        expired.deadline_monotonic_ns = 0;
        assert_eq!(
            OperationContext::from_transfer(
                &expired,
                Duration::from_secs(10),
                ResourceBudget::default()
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::TimedOut
        );
    }
}
