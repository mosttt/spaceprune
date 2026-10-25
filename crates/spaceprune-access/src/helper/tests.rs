use super::*;
use crate::{CancellationToken, Interaction};
use std::os::unix::fs::symlink;

fn operation() -> OperationContext {
    OperationContext::new(
        CancellationToken::default(),
        Interaction::NonInteractive,
        Duration::from_secs(2),
    )
}

fn fixture(source: &Path, contents: &[u8]) -> InstallRequest {
    for kind in [HelperKind::Metadata, HelperKind::Cleanup] {
        let path = source.join(kind.name());
        fs::write(&path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let digest = format!("{:x}", Sha256::digest(contents));
    InstallRequest {
        operation: operation().transfer().unwrap(),
        source_directory: crate::NativePath::from_path(source),
        metadata_sha256: digest.clone(),
        cleanup_sha256: digest,
    }
}

#[test]
fn bundle_lookup_accepts_only_fixed_adjacent_regular_executables() {
    let temporary = tempfile::tempdir().unwrap();
    let host = temporary.path().join("host");
    fs::write(&host, "host").unwrap();
    fixture(temporary.path(), b"component");
    let path = temporary.path().join(HelperKind::Metadata.name());
    assert_eq!(
        packaged_at(&host, HelperKind::Metadata.name()),
        Some(path.clone())
    );
    assert!(packaged_at(&host, "../host").is_none());
    fs::remove_file(&path).unwrap();
    symlink(&host, &path).unwrap();
    assert!(packaged_at(&host, HelperKind::Metadata.name()).is_none());
}

#[test]
fn old_handler_bundle_is_rejected_even_when_the_transport_probe_succeeds() {
    let directory = tempfile::tempdir().unwrap();
    for kind in [HelperKind::Metadata, HelperKind::Cleanup] {
        let helper = directory.path().join(kind.name());
        fs::write(
            &helper,
            format!("#!/bin/sh\n[ \"$1\" = \"{}\" ]\n", kind.support_argument()),
        )
        .unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(probe_protocol(&helper, kind.support_argument(), &operation()).is_ok());
        assert_eq!(
            probe_component(&helper, kind, &operation()),
            Err(PermissionOutcome::HelperOutdated)
        );
    }
}

#[test]
fn old_prompt_bundle_is_rejected_before_starting_a_localized_session() {
    let directory = tempfile::tempdir().unwrap();
    for kind in [HelperKind::Metadata, HelperKind::Cleanup] {
        let helper = directory.path().join(kind.name());
        fs::write(
            &helper,
            format!(
                "#!/bin/sh\n[ \"$1\" = \"{}\" ] || [ \"$1\" = \"{HANDLERS_SUPPORT}\" ]\n",
                kind.support_argument()
            ),
        )
        .unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            probe_component(&helper, kind, &operation()),
            Err(PermissionOutcome::HelperOutdated)
        );
    }
}

#[test]
fn previous_installations_request_upgrade_without_launching_old_components() {
    for previous in ["", "native-5-2", "native-5-3", "native-6-3"] {
        let root = tempfile::tempdir().unwrap();
        let kind = HelperKind::Metadata;
        assert_eq!(
            missing_helper_outcome(root.path(), kind),
            PermissionOutcome::HelperMissing
        );
        let directory = root.path().join(previous);
        fs::create_dir_all(&directory).unwrap();
        let marker = root.path().join("must-not-execute");
        let content = format!("#!/bin/sh\ntouch '{}'\n", marker.display());
        let helper = directory.join(kind.name());
        fs::write(&helper, &content).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            missing_helper_outcome(root.path(), kind),
            PermissionOutcome::HelperOutdated
        );
        assert!(!marker.exists());
        assert_eq!(fs::read_to_string(&helper).unwrap(), content);
        // A historical link is still only an upgrade hint, not a launch target.
        fs::remove_file(&helper).unwrap();
        symlink(&marker, &helper).unwrap();
        assert_eq!(
            missing_helper_outcome(root.path(), kind),
            PermissionOutcome::HelperOutdated
        );
        assert!(!marker.exists());
    }
}

#[test]
fn staging_copies_without_executing_and_replacement_commits_a_complete_set() {
    let source = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let marker = source.path().join("must-not-execute");
    let content = format!("#!/bin/sh\ntouch '{}'\n", marker.display());
    let request = fixture(source.path(), content.as_bytes());
    let caller = rustix::process::getuid().as_raw();
    let destination = root.path().join("native-6-4");
    let staged = staging::stage(root.path(), &request, caller, &mut || Ok(())).unwrap();
    staging::publish(staged, &destination, false).unwrap();
    assert!(!marker.exists());
    let request = fixture(source.path(), b"replacement");
    let staged = staging::stage(root.path(), &request, caller, &mut || Ok(())).unwrap();
    assert!(staging::validate_existing(&destination).unwrap());
    staging::publish(staged, &destination, true).unwrap();
    for kind in [HelperKind::Metadata, HelperKind::Cleanup] {
        assert_eq!(
            fs::read(destination.join(kind.name())).unwrap(),
            b"replacement"
        );
    }
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(destination.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest.metadata_sha256, request.metadata_sha256);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn altered_or_symlinked_bundle_keeps_previous_installation_intact() {
    let source = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let request = fixture(source.path(), b"approved");
    let caller = rustix::process::getuid().as_raw();
    let destination = root.path().join("native-6-4");
    let staged = staging::stage(root.path(), &request, caller, &mut || Ok(())).unwrap();
    staging::publish(staged, &destination, false).unwrap();
    let cleanup = source.path().join(HelperKind::Cleanup.name());
    fs::write(&cleanup, b"changed after approval").unwrap();
    assert!(staging::stage(root.path(), &request, caller, &mut || Ok(())).is_err());
    fs::remove_file(&cleanup).unwrap();
    symlink(destination.join(HelperKind::Cleanup.name()), &cleanup).unwrap();
    assert!(staging::stage(root.path(), &request, caller, &mut || Ok(())).is_err());
    assert_eq!(
        fs::read(destination.join(HelperKind::Cleanup.name())).unwrap(),
        b"approved"
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn cancellation_discards_staging_and_does_not_publish() {
    let source = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let request = fixture(source.path(), &[7; 256 * 1024]);
    let mut checks = 0;
    let error = staging::stage(
        root.path(),
        &request,
        rustix::process::getuid().as_raw(),
        &mut || {
            checks += 1;
            if checks < 3 {
                Ok(())
            } else {
                Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
            }
        },
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    let operation = operation();
    operation.cancel();
    assert_eq!(
        install(Path::new("/missing/host"), &operation),
        Err(InstallError::Authorization(PermissionOutcome::Cancelled))
    );
}

#[test]
fn concurrent_installation_wait_is_cancellable_and_lock_releases() {
    let lock = tempfile::NamedTempFile::new().unwrap();
    let first = fs::File::open(lock.path()).unwrap();
    let second = fs::File::open(lock.path()).unwrap();
    worker::wait_for_lock(&first, &mut || Ok(())).unwrap();
    let mut checks = 0;
    assert_eq!(
        worker::wait_for_lock(&second, &mut || {
            checks += 1;
            if checks == 2 {
                Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
            } else {
                Ok(())
            }
        })
        .unwrap_err()
        .kind(),
        io::ErrorKind::Interrupted
    );
    drop(first);
    worker::wait_for_lock(&second, &mut || Ok(())).unwrap();
}

#[test]
fn installer_refuses_unmanaged_destinations_and_writable_sources() {
    let source = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let request = fixture(source.path(), b"component");
    fs::set_permissions(
        source.path().join(HelperKind::Metadata.name()),
        fs::Permissions::from_mode(0o777),
    )
    .unwrap();
    assert!(
        staging::stage(
            root.path(),
            &request,
            rustix::process::getuid().as_raw(),
            &mut || Ok(())
        )
        .is_err()
    );
    fs::write(root.path().join("user-file"), "preserve").unwrap();
    assert!(staging::validate_existing(root.path()).is_err());
    assert!(!install::destination_is_safe(
        &root.path().join("native-6-4")
    ));
}
