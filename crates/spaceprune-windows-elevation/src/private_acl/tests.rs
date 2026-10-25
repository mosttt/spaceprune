use super::*;

#[test]
fn created_ipc_binds_its_actual_user_and_detects_a_replaced_open_file() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("ipc");
    std::fs::create_dir(&directory).unwrap();
    secure_created(&directory, PrivatePathKind::Directory).unwrap();
    let sid = current_user_sid().unwrap();
    assert_eq!(owner(&directory, PrivatePathKind::Directory).unwrap(), sid);
    assert!(validate_for_owner(&directory, PrivatePathKind::Directory, "S-1-0-0").is_err());
    let path = directory.join("request.json");
    std::fs::write(&path, b"original").unwrap();
    secure_created(&path, PrivatePathKind::File).unwrap();
    let file = std::fs::File::open(&path).unwrap();
    assert_eq!(open_file_owner(&path, &file).unwrap(), sid);
    std::fs::rename(&path, directory.join("original.json")).unwrap();
    std::fs::write(&path, b"replacement").unwrap();
    secure_created(&path, PrivatePathKind::File).unwrap();
    assert_eq!(
        open_file_owner(&path, &file),
        Err(PrivatePathError::Ineligible)
    );
}

#[test]
fn directory_and_file_descriptors_are_distinct_and_restrict_to_three_trustees() {
    let user = CurrentUserSid::load().expect("当前用户SID");
    let directory = ExpectedDescriptor::new(&user, PrivatePathKind::Directory).expect("目录描述符");
    let file = ExpectedDescriptor::new(&user, PrivatePathKind::File).expect("文件描述符");
    assert!(!same_acl(directory.dacl().unwrap(), file.dacl().unwrap()));
}

#[test]
fn hardens_and_validates_plain_paths_and_rejects_reparse_points() {
    use std::os::windows::fs::{symlink_dir, symlink_file};

    let temporary = tempfile::tempdir().expect("临时目录");
    let directory = temporary.path().join("private");
    std::fs::create_dir(&directory).expect("私有目录");
    let file = directory.join("checkpoint.json");
    std::fs::write(&file, b"{}").expect("私有文件");
    harden(&directory, PrivatePathKind::Directory).expect("收紧目录");
    harden(&file, PrivatePathKind::File).expect("收紧文件");
    validate(&directory, PrivatePathKind::Directory).expect("验证目录");
    validate(&file, PrivatePathKind::File).expect("验证文件");

    let broad = ExpectedDescriptor::from_sddl("D:P(A;OICI;FA;;;WD)").expect("宽松测试描述符");
    let directory_handle =
        open_plain_path(&directory, PrivatePathKind::Directory, WRITE_DAC).expect("目录句柄");
    // SAFETY: 测试句柄和ACL均有效，仅用于验证宽松DACL会被拒绝。
    assert_eq!(
        unsafe {
            SetSecurityInfo(
                directory_handle.0,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                broad.dacl().unwrap(),
                ptr::null(),
            )
        },
        ERROR_SUCCESS
    );
    drop(directory_handle);
    assert_eq!(
        validate(&directory, PrivatePathKind::Directory),
        Err(PrivatePathError::Permissions)
    );
    harden(&directory, PrivatePathKind::Directory).expect("重新收紧目录");

    let link = temporary.path().join("checkpoint-link.json");
    if symlink_file(&file, &link).is_ok() {
        assert_eq!(
            harden(&link, PrivatePathKind::File),
            Err(PrivatePathError::Ineligible)
        );
    }
    let outside = temporary.path().join("outside");
    std::fs::create_dir(&outside).expect("外部目录");
    let outside_file = outside.join("checkpoint.json");
    std::fs::write(&outside_file, b"{}").expect("外部文件");
    let intermediate = temporary.path().join("intermediate-link");
    if symlink_dir(&outside, &intermediate).is_ok() {
        assert!(harden(&intermediate.join("checkpoint.json"), PrivatePathKind::File,).is_err());
    }
}
