use super::*;

#[test]
fn metadata_preserves_native_login_or_system_process_without_account_fallback() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("metadata");
    std::fs::create_dir(&directory).unwrap();
    let token = crate::LogonIdentity::current().unwrap();
    if token.logon_sid().is_none() && !token.is_system_process() {
        assert_eq!(
            secure_created(&directory, PrivatePathKind::Directory),
            Err(PrivatePathError::Ineligible)
        );
        return;
    }
    secure_created(&directory, PrivatePathKind::Directory).unwrap();
    let scope = identity(&directory, PrivatePathKind::Directory).unwrap();
    assert_eq!(scope.account_sid(), token.account_sid());
    assert!(scope.matches_host(&token));
    let created = crate::current_process_creation_time().unwrap();
    assert!(
        crate::HostProcess::observe_metadata(std::process::id(), created, &scope)
            .unwrap()
            .is_running()
    );
    assert!(
        crate::HostProcess::observe_metadata(std::process::id(), created ^ 1, &scope).is_none()
    );
    if token.is_system_process() {
        assert_eq!(scope.0, Scope::LocalSystemProcess);
        // Metadata compatibility does not add SYSTEM-without-logon cleanup support.
        assert!(logon::identity(&directory, PrivatePathKind::Directory).is_err());
        assert!(logon::secure_created(&directory, PrivatePathKind::Directory).is_err());
    } else {
        assert!(matches!(scope.0, Scope::Logon(_)));
        assert!(
            crate::HostProcess::observe_metadata(
                std::process::id(),
                created,
                &MetadataIpcIdentity(Scope::LocalSystemProcess),
            )
            .is_none()
        );
    }

    let path = directory.join("request.json");
    std::fs::write(&path, b"original").unwrap();
    secure_created(&path, PrivatePathKind::File).unwrap();
    let file = std::fs::File::open(&path).unwrap();
    assert_eq!(file_identity(&path, &file).unwrap(), scope);
    std::fs::rename(&path, directory.join("old.json")).unwrap();
    std::fs::write(&path, b"replacement").unwrap();
    secure_created(&path, PrivatePathKind::File).unwrap();
    assert_eq!(
        file_identity(&path, &file),
        Err(PrivatePathError::Ineligible)
    );

    let account_path = root.path().join("settings.json");
    std::fs::write(&account_path, b"{}").unwrap();
    super::super::harden(&account_path, PrivatePathKind::File).unwrap();
    super::super::validate(&account_path, PrivatePathKind::File).unwrap();
    assert!(identity(&account_path, PrivatePathKind::File).is_err());
}
