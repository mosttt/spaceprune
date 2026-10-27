use super::*;
use crate::{
    error::classify,
    filesystem::{read_identity, remove_selected},
};
use std::fs;

fn remove(
    root: &Path,
    relative: &Path,
    expected: &FileIdentity,
    excluded: &dyn Fn(&Path) -> bool,
) -> Removal {
    remove_selected(
        root,
        &read_identity(root).unwrap(),
        relative,
        expected,
        excluded,
        &OperationContext::default(),
    )
}

#[test]
fn replacement_cannot_substitute_for_the_selected_file() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("selected");
    fs::write(&path, b"original").unwrap();
    let expected = read_identity(&path).unwrap();
    fs::rename(&path, temporary.path().join("moved")).unwrap();
    fs::write(&path, b"replacement").unwrap();
    let result = remove(temporary.path(), Path::new("selected"), &expected, &|_| {
        false
    });
    assert_eq!(
        classify(result.error.as_ref().unwrap()).kind,
        IoFailureKind::TargetChanged
    );
    assert_eq!(fs::read(path).unwrap(), b"replacement");
    assert_eq!(
        fs::read(temporary.path().join("moved")).unwrap(),
        b"original"
    );
}

#[test]
fn directory_enumeration_keeps_exclusions_and_new_siblings_outside_the_selection() {
    let temporary = tempfile::tempdir().unwrap();
    let selected = temporary.path().join("selected");
    fs::create_dir_all(selected.join("nested")).unwrap();
    fs::write(selected.join("nested/remove"), b"1234567").unwrap();
    fs::write(selected.join("nested/keep"), b"retained").unwrap();
    let expected = read_identity(&selected).unwrap();
    fs::write(selected.join("new-child"), b"123").unwrap();
    fs::write(temporary.path().join("new-sibling"), b"outside").unwrap();
    let result = remove(
        temporary.path(),
        Path::new("selected"),
        &expected,
        &|relative| relative == Path::new("selected/nested/keep"),
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(result.reclaimed_estimate, 10);
    assert_eq!(result.removed_entries, 2);
    assert_eq!(fs::read(selected.join("nested/keep")).unwrap(), b"retained");
    assert_eq!(
        fs::read(temporary.path().join("new-sibling")).unwrap(),
        b"outside"
    );
    assert!(!selected.join("nested/remove").exists());
    assert!(!selected.join("new-child").exists());
}

#[test]
fn ordinary_temp_directories_are_removed_without_path_reconstruction() {
    let temporary = tempfile::tempdir().unwrap();
    for name in [
        ".das.local.jobsroot",
        "04bc-b64d-9b77-0900",
        "is-2IOH4.tmp",
        "中文 目录",
    ] {
        let selected = temporary.path().join(name);
        fs::create_dir_all(selected.join("nested")).unwrap();
        fs::write(selected.join("nested/payload.tmp"), b"payload").unwrap();
        let expected = read_identity(&selected).unwrap();
        let result = remove(temporary.path(), Path::new(name), &expected, &|_| false);
        assert!(result.error.is_none(), "{name}: {:?}", result.error);
        assert_eq!(result.removed_entries, 3);
        assert_eq!(result.reclaimed_estimate, 7);
        assert!(!selected.exists());
    }
}

#[test]
fn ordinary_directory_listing_can_reopen_the_held_delete_handle() {
    let temporary = tempfile::tempdir().unwrap();
    let selected = temporary.path().join("selected");
    fs::create_dir(&selected).unwrap();
    fs::write(selected.join("child"), b"fixture").unwrap();
    let roots = lock_ancestors(temporary.path()).expect("ancestor handles");
    let handle = open_relative(
        roots.last().unwrap(),
        std::ffi::OsStr::new("selected"),
        true,
    )
    .expect("selected delete handle");
    let listing = handles::open_listing(&handle).expect("reopen directory for enumeration");
    assert_eq!(
        handles::read_directory(&listing, true)
            .expect("enumeration")
            .unwrap(),
        vec![std::ffi::OsString::from("child")]
    );
}

#[test]
fn native_handles_prevent_renaming_a_selected_directory_while_its_children_are_processed() {
    let temporary = tempfile::tempdir().unwrap();
    let selected = temporary.path().join("selected");
    fs::create_dir(&selected).unwrap();
    fs::write(selected.join("child"), b"selected").unwrap();
    let expected = read_identity(&selected).unwrap();
    let result = remove(
        temporary.path(),
        Path::new("selected"),
        &expected,
        &|relative| {
            if relative == Path::new("selected/child") {
                assert!(fs::rename(&selected, temporary.path().join("moved")).is_err());
            }
            false
        },
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(!selected.exists());
    assert_eq!(result.reclaimed_estimate, 8);
}

#[test]
#[ignore = "Requires Windows Developer Mode or the symbolic-link privilege"]
fn selected_reparse_leaf_is_removed_without_following_its_target() {
    let temporary = tempfile::tempdir().unwrap();
    let outside = temporary.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), b"outside").unwrap();
    let link = temporary.path().join("selected");
    std::os::windows::fs::symlink_dir(&outside, &link).unwrap();
    let result = remove(
        temporary.path(),
        Path::new("selected"),
        &read_identity(&link).unwrap(),
        &|_| false,
    );
    assert!(result.error.is_none(), "{:?}", result.error);
    assert!(fs::symlink_metadata(link).is_err());
    assert_eq!(fs::read(outside.join("keep")).unwrap(), b"outside");
}
