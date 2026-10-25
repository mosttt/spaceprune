//! Deterministic traversal regressions for the engine shared by native Go cache cleanup.

use super::*;
use rustix::fs::{Mode, OFlags, RenameFlags, mkdirat, openat, renameat_with, symlinkat};
use std::{fs::File, io::Write, os::unix::fs::PermissionsExt};

fn write_at(parent: &File, name: &str, content: &[u8]) {
    let fd = openat(
        parent,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .unwrap();
    File::from(fd).write_all(content).unwrap();
}

#[test]
fn a_descendant_moved_before_capture_survives_while_runtime_replacements_are_bounded() {
    for replacement in ["absent", "symlink", "directory"] {
        let temp = tempfile::tempdir().unwrap();
        let selected = temp.path().join("selected");
        let outside = temp.path().join("outside");
        fs::create_dir_all(selected.join("candidate")).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(selected.join("candidate/keep"), b"moved original").unwrap();
        fs::write(outside.join("keep"), b"outside target").unwrap();
        fs::write(temp.path().join("sibling"), b"unselected sibling").unwrap();
        let original = read_identity(&selected.join("candidate")).unwrap();
        let retained = File::open(&selected).unwrap();
        let outer = File::open(temp.path()).unwrap();
        let moved = Cell::new(false);
        let result = remove_selected(
            temp.path(),
            &read_identity(temp.path()).unwrap(),
            Path::new("selected"),
            &read_identity(&selected).unwrap(),
            &|path| {
                if path == Path::new("selected/candidate") && !moved.replace(true) {
                    renameat_with(
                        &retained,
                        "candidate",
                        &outer,
                        "moved",
                        RenameFlags::NOREPLACE,
                    )
                    .unwrap();
                    match replacement {
                        "symlink" => symlinkat(&outside, &retained, "candidate").unwrap(),
                        "directory" => {
                            mkdirat(&retained, "candidate", Mode::RWXU).unwrap();
                            write_at(&retained, "candidate/new", b"runtime replacement");
                        }
                        _ => {}
                    }
                }
                false
            },
            &OperationContext::default(),
        );
        assert!(moved.get(), "{replacement}");
        assert!(result.error.is_none(), "{replacement}: {:?}", result.error);
        assert!(!selected.exists(), "{replacement}");
        assert_eq!(read_identity(&temp.path().join("moved")).unwrap(), original);
        assert_eq!(
            fs::read(temp.path().join("moved/keep")).unwrap(),
            b"moved original"
        );
        assert_eq!(fs::read(outside.join("keep")).unwrap(), b"outside target");
        assert_eq!(
            fs::read(temp.path().join("sibling")).unwrap(),
            b"unselected sibling"
        );
        assert_eq!(
            result.reclaimed_estimate,
            if replacement == "directory" { 19 } else { 0 },
            "{replacement}"
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 3);
    }
}

#[test]
fn a_captured_descendant_is_no_longer_addressable_through_its_original_parent() {
    let temp = tempfile::tempdir().unwrap();
    let selected = temp.path().join("selected");
    let outside = temp.path().join("outside");
    fs::create_dir_all(selected.join("candidate")).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(selected.join("candidate/keep"), b"selected data").unwrap();
    let retained = File::open(&selected).unwrap();
    let destination = File::open(&outside).unwrap();
    let attempted = Cell::new(false);
    let result = remove_selected(
        temp.path(),
        &read_identity(temp.path()).unwrap(),
        Path::new("selected"),
        &read_identity(&selected).unwrap(),
        &|path| {
            if path == Path::new("selected/candidate/keep") && !attempted.replace(true) {
                assert_eq!(
                    renameat_with(
                        &retained,
                        "candidate",
                        &destination,
                        "moved",
                        RenameFlags::NOREPLACE,
                    ),
                    Err(rustix::io::Errno::NOENT)
                );
            }
            false
        },
        &OperationContext::default(),
    );
    assert!(attempted.get());
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(result.removed_entries, 3);
    assert_eq!(result.reclaimed_estimate, 13);
    assert!(!selected.exists());
    assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn descendants_added_during_traversal_are_included_without_expanding_the_selection() {
    let temp = tempfile::tempdir().unwrap();
    let selected = temp.path().join("selected");
    fs::create_dir_all(selected.join("candidate")).unwrap();
    fs::create_dir(selected.join("protected")).unwrap();
    fs::write(selected.join("candidate/old"), b"old").unwrap();
    fs::write(selected.join("protected/keep"), b"excluded").unwrap();
    fs::write(temp.path().join("sibling"), b"sibling").unwrap();
    let retained = File::open(&selected).unwrap();
    let inserted = Cell::new(false);
    let result = remove_selected(
        temp.path(),
        &read_identity(temp.path()).unwrap(),
        Path::new("selected"),
        &read_identity(&selected).unwrap(),
        &|path| {
            if path == Path::new("selected/candidate") && !inserted.replace(true) {
                mkdirat(&retained, "candidate/new", Mode::RWXU).unwrap();
                write_at(&retained, "candidate/new/later", b"new content");
            }
            path.starts_with("selected/protected")
        },
        &OperationContext::default(),
    );
    assert!(inserted.get());
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(result.removed_entries, 4);
    assert_eq!(result.reclaimed_estimate, 14);
    assert!(!selected.join("candidate").exists());
    assert_eq!(
        fs::read(selected.join("protected/keep")).unwrap(),
        b"excluded"
    );
    assert_eq!(fs::read(temp.path().join("sibling")).unwrap(), b"sibling");
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
}

#[test]
fn cancelling_a_captured_branch_restores_survivor_identities_and_modes() {
    let temp = tempfile::tempdir().unwrap();
    let selected = temp.path().join("selected");
    let candidate = selected.join("candidate");
    let readonly = candidate.join("readonly");
    fs::create_dir_all(&readonly).unwrap();
    fs::write(readonly.join("keep"), b"unchanged").unwrap();
    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o750)).unwrap();
    fs::set_permissions(&readonly, fs::Permissions::from_mode(0o555)).unwrap();
    fs::set_permissions(readonly.join("keep"), fs::Permissions::from_mode(0o444)).unwrap();
    let candidate_identity = read_identity(&candidate).unwrap();
    let readonly_identity = read_identity(&readonly).unwrap();
    let context = OperationContext::default();
    let cancelled = Cell::new(false);
    let result = remove_selected(
        temp.path(),
        &read_identity(temp.path()).unwrap(),
        Path::new("selected"),
        &read_identity(&selected).unwrap(),
        &|path| {
            if path == Path::new("selected/candidate/readonly") {
                cancelled.set(true);
                context.cancel();
            }
            false
        },
        &context,
    );
    assert!(cancelled.get());
    assert_eq!(
        classify(result.error.as_ref().unwrap()).kind,
        IoFailureKind::Cancelled
    );
    assert_eq!(result.removed_entries, 0);
    assert_eq!(result.reclaimed_estimate, 0);
    assert_eq!(read_identity(&candidate).unwrap(), candidate_identity);
    assert_eq!(read_identity(&readonly).unwrap(), readonly_identity);
    for (path, mode) in [
        (&candidate, 0o750),
        (&readonly, 0o555),
        (&readonly.join("keep"), 0o444),
    ] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o7777,
            mode
        );
    }
    assert_eq!(fs::read(readonly.join("keep")).unwrap(), b"unchanged");
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    // Return write access only after all assertions so TempDir can remove its fixture.
    fs::set_permissions(&readonly, fs::Permissions::from_mode(0o700)).unwrap();
}
