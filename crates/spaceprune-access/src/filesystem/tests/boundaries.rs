use super::*;
use std::ffi::OsStr;

fn temporary_directory() -> tempfile::TempDir {
    // Preserve the selected test volume without macOS /var aliases failing NOFOLLOW first.
    tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap()
}

#[test]
fn an_open_descendant_cannot_be_moved_into_a_hard_exclusion_before_deletion() {
    use rustix::fs::{RenameFlags, renameat_with};
    let temp = temporary_directory();
    let selected = temp.path().join("selected");
    fs::create_dir_all(selected.join("candidate")).unwrap();
    fs::create_dir(selected.join("protected")).unwrap();
    fs::write(selected.join("candidate/keep"), b"protected after move").unwrap();
    // A concurrent writer may retain access to the selected directory after top-level capture.
    let retained = fs::File::open(&selected).unwrap();
    let attempted = Cell::new(false);
    let moved = Cell::new(false);
    let result = remove_selected(
        temp.path(),
        &read_identity(temp.path()).unwrap(),
        Path::new("selected"),
        &read_identity(&selected).unwrap(),
        &|path| {
            if path == Path::new("selected/candidate/keep") && !attempted.replace(true) {
                match renameat_with(
                    &retained,
                    "candidate",
                    &retained,
                    "protected/moved",
                    RenameFlags::NOREPLACE,
                ) {
                    Ok(()) => moved.set(true),
                    Err(rustix::io::Errno::NOENT) => {}
                    Err(error) => panic!("Fixture move failed: {error}"),
                }
            }
            path.starts_with("selected/protected")
        },
        &OperationContext::default(),
    );
    assert!(attempted.get());
    if moved.get() {
        assert_eq!(
            fs::read(selected.join("protected/moved/keep")).unwrap(),
            b"protected after move"
        );
        assert_eq!(result.removed_entries, 0);
    } else {
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(result.reclaimed_estimate, 20);
        assert!(selected.join("protected").is_dir());
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "Requires an isolated Linux mount namespace with CAP_SYS_ADMIN"]
fn removal_preserves_same_filesystem_bind_mounts() {
    use std::{os::unix::fs::MetadataExt, process::Command};
    struct Mounted(PathBuf);
    impl Drop for Mounted {
        fn drop(&mut self) {
            let _ = Command::new("umount").arg(&self.0).status();
        }
    }
    for selection in ["selected", "selected/nested", "selected/nested/keep"] {
        let temp = temporary_directory();
        let selected = temp.path().join("selected");
        let nested = selected.join("nested");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), b"outside selection").unwrap();
        assert!(
            Command::new("mount")
                .arg("--bind")
                .arg(&outside)
                .arg(&nested)
                .status()
                .unwrap()
                .success()
        );
        let _mounted = Mounted(nested.clone());
        assert_eq!(
            fs::metadata(&selected).unwrap().dev(),
            fs::metadata(&nested).unwrap().dev()
        );
        let result = remove(
            temp.path(),
            Path::new(selection),
            &read_identity(&temp.path().join(selection)).unwrap(),
        );
        assert_eq!(
            fs::read(outside.join("keep")).unwrap(),
            b"outside selection"
        );
        assert_eq!(
            classify(result.error.as_ref().unwrap()).kind,
            IoFailureKind::BoundaryViolation
        );
        assert_eq!(result.removed_entries, 0);
        assert_eq!(result.reclaimed_estimate, 0);
    }
}

#[test]
fn descendant_restore_never_overwrites_a_new_name_after_cancellation() {
    use rustix::fs::{Mode, mkdirat};
    let temp = temporary_directory();
    let selected = temp.path().join("selected");
    fs::create_dir_all(selected.join("candidate")).unwrap();
    fs::write(selected.join("candidate/keep"), b"original").unwrap();
    let original = read_identity(&selected.join("candidate")).unwrap();
    let retained = fs::File::open(&selected).unwrap();
    let context = OperationContext::default();
    let inserted = Cell::new(false);
    let result = remove_selected(
        temp.path(),
        &read_identity(temp.path()).unwrap(),
        Path::new("selected"),
        &read_identity(&selected).unwrap(),
        &|path| {
            if path == Path::new("selected/candidate/keep") && !inserted.replace(true) {
                mkdirat(&retained, "candidate", Mode::RWXU).unwrap();
                context.cancel();
            }
            false
        },
        &context,
    );
    assert!(inserted.get());
    let error = result.error.unwrap();
    assert_eq!(classify(&error).kind, IoFailureKind::TargetChanged);
    assert!(crate::error::has_effects(&error));
    assert_eq!(result.removed_entries, 0);
    assert_eq!(result.reclaimed_estimate, 0);
    assert_ne!(
        read_identity(&selected.join("candidate")).unwrap(),
        original
    );
    let staging = fs::read_dir(temp.path())
        .unwrap()
        .map(Result::unwrap)
        .find(|entry| is_execution_artifact(&entry.path()))
        .unwrap()
        .path();
    let retained = super::recovery::survivors(&staging)
        .into_iter()
        .find(|(_, source)| source == &selected.join("candidate"))
        .unwrap()
        .0;
    assert_eq!(read_identity(&retained).unwrap(), original);
    assert_eq!(fs::read(retained.join("keep")).unwrap(), b"original");
    assert!(error.to_string().contains(staging.to_str().unwrap()));
}

#[test]
fn nested_exclusions_survive_without_revisiting_restored_directories() {
    let temp = temporary_directory();
    let selected = temp.path().join("selected");
    fs::create_dir(&selected).unwrap();
    for index in 0..100 {
        let child = selected.join(index.to_string());
        fs::create_dir(&child).unwrap();
        fs::write(child.join("keep"), b"protected").unwrap();
        fs::write(child.join("remove"), b"data").unwrap();
    }
    let visited = Cell::new(0);
    let result = remove_selected(
        temp.path(),
        &read_identity(temp.path()).unwrap(),
        Path::new("selected"),
        &read_identity(&selected).unwrap(),
        &|path| {
            if path.file_name() == Some(OsStr::new("keep")) {
                visited.set(visited.get() + 1);
                return true;
            }
            false
        },
        &OperationContext::default(),
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(visited.get(), 100);
    assert_eq!(result.removed_entries, 100);
    assert_eq!(result.reclaimed_estimate, 400);
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    for index in 0..100 {
        let child = selected.join(index.to_string());
        assert_eq!(fs::read(child.join("keep")).unwrap(), b"protected");
        assert_eq!(fs::read_dir(child).unwrap().count(), 1);
    }
}

#[test]
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos")),
    ignore = "Ancestor movement observation is not implemented on this platform"
)]
fn a_selected_file_is_not_deleted_after_its_ancestor_moves_into_an_exclusion() {
    let temp = temporary_directory();
    let base = fs::canonicalize(temp.path()).unwrap();
    let root = base.as_path();
    fs::create_dir(root.join("allowed")).unwrap();
    fs::create_dir(root.join("protected")).unwrap();
    fs::write(root.join("allowed/file"), b"protected after move").unwrap();
    let selected = read_identity(&root.join("allowed/file")).unwrap();
    let moved = Cell::new(false);
    let result = remove_selected(
        root,
        &read_identity(root).unwrap(),
        Path::new("allowed/file"),
        &selected,
        &|path| {
            if path == Path::new("allowed/file") && !moved.replace(true) {
                fs::rename(root.join("allowed"), root.join("protected/moved")).unwrap();
            }
            path.starts_with("protected")
        },
        &OperationContext::default(),
    );
    assert!(moved.get());
    assert_eq!(
        fs::read(root.join("protected/moved/file")).unwrap(),
        b"protected after move"
    );
    assert_eq!(
        classify(result.error.as_ref().unwrap()).kind,
        IoFailureKind::TargetChanged
    );
    assert_eq!(result.removed_entries, 0);
    assert_eq!(result.reclaimed_estimate, 0);
    assert_eq!(
        fs::read_dir(root.join("protected/moved")).unwrap().count(),
        1
    );
}

#[test]
fn an_empty_read_only_descendant_does_not_require_new_write_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let temp = temporary_directory();
    let selected = temp.path().join("selected");
    fs::create_dir_all(selected.join("empty")).unwrap();
    fs::set_permissions(selected.join("empty"), fs::Permissions::from_mode(0o555)).unwrap();
    let result = remove(
        temp.path(),
        Path::new("selected"),
        &read_identity(&selected).unwrap(),
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(result.removed_entries, 2);
    assert_eq!(result.reclaimed_estimate, 0);
    assert!(!selected.exists());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}
