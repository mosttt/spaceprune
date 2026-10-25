use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn service() -> MetadataService {
    let service = MetadataService::default();
    #[cfg(target_os = "macos")]
    let service = MetadataService {
        administrator: crate::administrator::MetadataService::unavailable_for_test(),
        ..service
    };
    service
}

fn failure(code: rustix::io::Errno) -> IoFailure {
    spaceprune_access::error::classify(&std::io::Error::from_raw_os_error(code.raw_os_error()))
}

#[test]
fn settings_are_coalesced_across_scopes_and_never_create_an_access_lease() {
    let service = service();
    let operation = OperationContext::workflow(Default::default(), Interaction::Foreground);
    let opened = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for index in 0..10 {
            let service = &service;
            let operation = &operation;
            let opened = &opened;
            scope.spawn(move || {
                assert_eq!(
                    service.recover_denial_with(
                        &[PathBuf::from(format!("/fixture/root-{index}"))],
                        &failure(rustix::io::Errno::PERM),
                        operation,
                        |_| {
                            opened.fetch_add(1, Ordering::SeqCst);
                            std::thread::sleep(Duration::from_millis(20));
                            Ok(())
                        }
                    ),
                    PermissionOutcome::NeedsSystemSettings
                );
            });
        }
    });
    assert_eq!(opened.load(Ordering::SeqCst), 1);
    assert!(!service.is_active());
    service.close().unwrap();
    assert!(!service.is_active());
}

#[test]
fn background_dac_denials_and_stopped_operations_do_not_open_settings() {
    let service = service();
    let mut operation = OperationContext::default();
    let roots = [PathBuf::from("/fixture/root")];
    assert_eq!(
        service.recover_denial_with(
            &roots,
            &failure(rustix::io::Errno::ACCESS),
            &operation,
            |_| panic!("DAC denials must not be mistaken for privacy access")
        ),
        PermissionOutcome::NeedsAuthorization
    );
    operation.interaction = Interaction::NonInteractive;
    assert_eq!(
        service.recover_denial_with(
            &roots,
            &failure(rustix::io::Errno::PERM),
            &operation,
            |_| panic!("Background work cannot open settings")
        ),
        PermissionOutcome::NeedsSystemSettings
    );
    operation.cancel();
    assert_eq!(
        service.recover_denial_with(
            &roots,
            &failure(rustix::io::Errno::PERM),
            &operation,
            |_| panic!("Cancelled work cannot open settings")
        ),
        PermissionOutcome::Cancelled
    );
}

#[test]
fn returning_from_settings_reads_only_the_original_boundary_and_rechecks_revocation() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temporary = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temporary.path())
        .unwrap()
        .join("cache");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("data"), b"size only").unwrap();
    let outside = temporary.path().join("outside");
    std::fs::File::create(&outside)
        .unwrap()
        .set_len(1 << 30)
        .unwrap();
    symlink(&outside, root.join("link")).unwrap();
    let service = service();
    let operation = OperationContext::workflow(Default::default(), Interaction::Foreground);
    let roots = [root.clone()];
    assert_eq!(
        service.recover_denial_with(
            &roots,
            &failure(rustix::io::Errno::PERM),
            &operation,
            |_| Ok(())
        ),
        PermissionOutcome::NeedsSystemSettings
    );
    let request = PreparedRequest::create_navigation_boundary(
        "navigation-boundary-1234567890abcdef1234567890abcdef",
        &root,
    )
    .unwrap()
    .with_operation(&operation)
    .unwrap();
    assert_eq!(
        service.measure(&request, &operation).unwrap(),
        ResponseOutcome::Measured {
            logical_bytes: 9,
            inspected_entries: 1,
            incomplete: true,
        }
    );
    assert!(!service.is_active());
    if !rustix::process::geteuid().is_root() {
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).unwrap();
        let revoked = service.measure(&request, &operation);
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(revoked, Err(PermissionOutcome::NeedsAuthorization));
    }
    assert_eq!(std::fs::metadata(outside).unwrap().len(), 1 << 30);
    assert_eq!(std::fs::read(root.join("data")).unwrap(), b"size only");
}

#[test]
fn ordinary_scan_results_keep_their_original_rules_and_never_mean_privacy_was_granted() {
    let service = service();
    let operation = OperationContext::workflow(Default::default(), Interaction::Foreground);
    let task = TaskRequest {
        module_id: "cache".into(),
        task_id: "logs".into(),
        name: "Logs".into(),
        handler_id: "builtin.filesystem/paths".into(),
        options: serde_json::json!({"roots":[{"path":"/fixture/root","include":["*.log"],"exclude":["keep.log"]}],"elevation":"never"}),
        recommend: serde_json::json!({}),
        tags: vec![],
    };
    let mut result = HandlerScanResult::default();
    result.issues.push(spaceprune_plugin_api::HandlerIssue {
        code: None,
        location: Some("/fixture/root".into()),
        message: "localized native error".into(),
        level: spaceprune_plugin_api::IssueLevel::Warning,
        io_failure: Some(failure(rustix::io::Errno::PERM)),
    });
    let recovered = service.recover_scan(&task, &result, &operation).unwrap();
    assert_eq!(
        serde_json::to_value(recovered).unwrap(),
        serde_json::to_value(result).unwrap()
    );
    assert!(!service.is_active());
}

#[test]
fn local_verification_rejects_another_operation_and_stopped_requests() {
    let temporary = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temporary.path()).unwrap();
    let operation = OperationContext::default();
    let mut request = PreparedRequest::create("recipe-source-0", &[root])
        .unwrap()
        .with_operation(&operation)
        .unwrap();
    let service = service();
    assert_eq!(
        service.measure(&request, &OperationContext::default()),
        Err(PermissionOutcome::PolicyBlocked)
    );
    request.context.deadline = std::time::Instant::now();
    assert_eq!(
        service.measure(&request, &operation),
        Err(PermissionOutcome::TimedOut)
    );
    operation.cancel();
    assert_eq!(
        service.measure(&request, &operation),
        Err(PermissionOutcome::Cancelled)
    );
}
