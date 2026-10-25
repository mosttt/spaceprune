//! Isolated metadata roundtrips through the actual packaged helper executable.

#[cfg(target_os = "linux")]
mod linux {
    use spaceprune_access::{CancellationToken, Interaction, OperationContext};
    use spaceprune_readonly_protocol::{PreparedRequest, ResponseOutcome};
    use std::{
        fs,
        process::Command,
        time::{Duration, Instant},
    };

    const SOURCE: &str = "navigation-boundary-0123456789abcdef0123456789abcdef";

    #[test]
    fn packaged_helper_roundtrip_preserves_operation_identity_and_only_reads_fixture_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("keep.bin");
        fs::write(&file, b"unchanged").unwrap();
        let mut operation = OperationContext::new(
            CancellationToken::default(),
            Interaction::NonInteractive,
            Duration::from_secs(5),
        );
        operation.generation = 7;
        operation.budget.output_bytes = 1024;
        let request = PreparedRequest::create_navigation_boundary(SOURCE, directory.path())
            .unwrap()
            .with_operation(&operation)
            .unwrap();
        let status = Command::new(env!("CARGO_BIN_EXE_spaceprune-readonly-helper"))
            .args(request.arguments())
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(
            request.receive_response().unwrap(),
            ResponseOutcome::Measured {
                logical_bytes: 9,
                inspected_entries: 1,
                incomplete: false
            }
        );
        assert_eq!(fs::read(file).unwrap(), b"unchanged");
    }

    #[test]
    fn packaged_helper_cannot_renew_a_request_that_expired_while_queued() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("keep.bin");
        fs::write(&file, b"keep").unwrap();
        let prepared =
            PreparedRequest::create_navigation_boundary(SOURCE, directory.path()).unwrap();
        let operation = OperationContext::new(
            CancellationToken::default(),
            Interaction::NonInteractive,
            Duration::from_millis(250),
        );
        let request = prepared.with_operation(&operation).unwrap();
        std::thread::sleep(
            operation.deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(5),
        );
        let status = Command::new(env!("CARGO_BIN_EXE_spaceprune-readonly-helper"))
            .args(request.arguments())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(70));
        assert_eq!(fs::read(file).unwrap(), b"keep");
    }
}
