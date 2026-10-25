use super::*;

#[test]
fn ipc_binds_original_account_and_login_without_changing_account_scoped_paths() {
    let root = tempfile::tempdir().unwrap();
    let ipc = root.path().join("ipc");
    std::fs::create_dir(&ipc).unwrap();
    secure_created(&ipc, PrivatePathKind::Directory).unwrap();
    let actual = identity(&ipc, PrivatePathKind::Directory).unwrap();
    let login = crate::LogonIdentity::current().unwrap();
    assert_eq!(actual.account_sid(), login.account_sid());
    assert_eq!(Some(actual.logon_sid()), login.logon_sid());
    let created = crate::current_process_creation_time().unwrap();
    let host = crate::HostProcess::observe_logon(std::process::id(), created, &actual).unwrap();
    assert!(host.is_running());
    let mut other_login = actual.clone();
    other_login.logon_sid = "S-1-5-5-0-0".into();
    assert!(crate::HostProcess::observe_logon(std::process::id(), created, &other_login).is_none());
    assert!(super::super::validate(&ipc, PrivatePathKind::Directory).is_err());

    let ordinary = root.path().join("settings.json");
    std::fs::write(&ordinary, b"{}").unwrap();
    super::super::harden(&ordinary, PrivatePathKind::File).unwrap();
    super::super::validate(&ordinary, PrivatePathKind::File).unwrap();
    assert!(identity(&ordinary, PrivatePathKind::File).is_err());

    let path = ipc.join("request.json");
    std::fs::write(&path, b"original").unwrap();
    secure_created(&path, PrivatePathKind::File).unwrap();
    let file = std::fs::File::open(&path).unwrap();
    assert_eq!(file_identity(&path, &file).unwrap(), actual);
    std::fs::rename(&path, ipc.join("old.json")).unwrap();
    std::fs::write(&path, b"replacement").unwrap();
    secure_created(&path, PrivatePathKind::File).unwrap();
    assert_eq!(
        file_identity(&path, &file),
        Err(PrivatePathError::Ineligible)
    );
}

#[test]
fn account_owner_without_the_login_cannot_read_ipc_or_rewrite_its_dacl() {
    use windows_sys::Win32::Security::{
        AccessCheck, Authorization::ConvertStringSidToSidW, CreateRestrictedToken,
        DISABLE_MAX_PRIVILEGE, DuplicateToken, GENERIC_MAPPING, PRIVILEGE_SET, SID_AND_ATTRIBUTES,
        SecurityImpersonation, TOKEN_DUPLICATE,
    };
    use windows_sys::Win32::Storage::FileSystem::{FILE_READ_DATA, READ_CONTROL};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("request.json");
    std::fs::write(&path, b"private").unwrap();
    secure_created(&path, PrivatePathKind::File).unwrap();
    let file = open_plain_path(&path, PrivatePathKind::File, 0).unwrap();
    let (security, _, _) = read_security(&file).unwrap();
    let mut token = ptr::null_mut();
    // SAFETY: Only duplicate/query rights on this test process's primary token are requested.
    assert_ne!(
        unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_DUPLICATE,
                &raw mut token,
            )
        },
        0
    );
    let token = OwnedHandle::new(token).unwrap();
    let login = crate::LogonIdentity::current().unwrap();
    let sid = |value: &str| {
        let mut raw = ptr::null_mut();
        // SAFETY: The SID string is terminated, output is owned and later LocalFree'd.
        assert_ne!(
            unsafe { ConvertStringSidToSidW(wide_null(OsStr::new(value)).as_ptr(), &raw mut raw) },
            0
        );
        LocalDescriptor(raw)
    };
    let logon = sid(login.logon_sid().unwrap());
    let admins = sid("S-1-5-32-544");
    let disabled = [
        SID_AND_ATTRIBUTES {
            Sid: logon.0,
            Attributes: 0,
        },
        SID_AND_ATTRIBUTES {
            Sid: admins.0,
            Attributes: 0,
        },
    ];
    let mut restricted = ptr::null_mut();
    // SAFETY: This creates only a less-privileged duplicate. The account owner SID is preserved,
    // while the current login and administrators cannot satisfy allow ACEs.
    assert_ne!(
        unsafe {
            CreateRestrictedToken(
                token.0,
                DISABLE_MAX_PRIVILEGE,
                disabled.len() as u32,
                disabled.as_ptr(),
                0,
                ptr::null(),
                0,
                ptr::null(),
                &raw mut restricted,
            )
        },
        0
    );
    let restricted = OwnedHandle::new(restricted).unwrap();
    let mut impersonation = ptr::null_mut();
    // SAFETY: AccessCheck requires an impersonation token; it is never installed on this thread.
    assert_ne!(
        unsafe { DuplicateToken(restricted.0, SecurityImpersonation, &raw mut impersonation) },
        0
    );
    let impersonation = OwnedHandle::new(impersonation).unwrap();
    for (access, allowed) in [
        (FILE_READ_DATA, false),
        (WRITE_DAC, false),
        (READ_CONTROL, true),
    ] {
        let mapping = GENERIC_MAPPING::default();
        let mut privileges = PRIVILEGE_SET::default();
        let mut length = size_of::<PRIVILEGE_SET>() as u32;
        let mut granted = 0;
        let mut status = 0;
        // SAFETY: The descriptor, token and all correctly sized writable outputs remain held.
        assert_ne!(
            unsafe {
                AccessCheck(
                    security.0,
                    impersonation.0,
                    access,
                    &mapping,
                    &raw mut privileges,
                    &raw mut length,
                    &raw mut granted,
                    &raw mut status,
                )
            },
            0
        );
        assert_eq!(status != 0, allowed, "access={access:#x}");
    }
}
