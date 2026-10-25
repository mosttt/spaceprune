mod ancestry;
mod boundaries;
mod go_capture;
mod recovery;

use super::*;
use crate::{IoFailureKind, error::classify};
use std::{
    cell::Cell,
    os::unix::{ffi::OsStringExt, fs::symlink},
    path::PathBuf,
};

fn remove(root: &Path, relative: &Path, identity: &FileIdentity) -> Removal {
    remove_selected(
        root,
        &read_identity(root).unwrap(),
        relative,
        identity,
        &|_| false,
        &OperationContext::default(),
    )
}

#[test]
fn replacement_is_rejected_and_the_original_selection_does_not_expand() {
    let temp = tempfile::tempdir().unwrap();
    let selected = temp.path().join("selected");
    fs::write(&selected, b"original").unwrap();
    let identity = read_identity(&selected).unwrap();
    fs::rename(&selected, temp.path().join("moved")).unwrap();
    fs::write(&selected, b"replacement").unwrap();
    let result = remove(temp.path(), Path::new("selected"), &identity);
    assert_eq!(
        classify(result.error.as_ref().unwrap()).kind,
        IoFailureKind::TargetChanged
    );
    assert_eq!(fs::read(&selected).unwrap(), b"replacement");
    assert_eq!(fs::read(temp.path().join("moved")).unwrap(), b"original");
}

#[test]
fn selected_symlink_is_removed_without_reading_or_deleting_its_target() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("keep");
    let link = temp.path().join("link");
    fs::write(&target, b"keep").unwrap();
    symlink(&target, &link).unwrap();
    let result = remove(
        temp.path(),
        Path::new("link"),
        &read_identity(&link).unwrap(),
    );
    assert!(result.error.is_none());
    assert!(fs::symlink_metadata(&link).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"keep");
}

#[test]
fn replaced_directory_ancestors_report_a_boundary_without_claiming_effects() {
    for replace_root in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(temp.path()).unwrap();
        let root = base.join("root");
        let ancestor = root.join("ancestor");
        fs::create_dir_all(&ancestor).unwrap();
        fs::write(ancestor.join("selected"), b"keep").unwrap();
        let root_identity = read_identity(&root).unwrap();
        let selected_identity = read_identity(&ancestor.join("selected")).unwrap();
        let (replaced, retained, selected) = if replace_root {
            let retained = base.join("original-root");
            let selected = retained.join("ancestor/selected");
            (root.clone(), retained, selected)
        } else {
            let retained = root.join("original-ancestor");
            let selected = retained.join("selected");
            (ancestor, retained, selected)
        };
        fs::rename(&replaced, &retained).unwrap();
        symlink(&retained, &replaced).unwrap();
        let result = remove_selected(
            &root,
            &root_identity,
            Path::new("ancestor/selected"),
            &selected_identity,
            &|_| false,
            &OperationContext::default(),
        );
        let error = result.error.as_ref().unwrap();
        let failure = classify(error);
        assert_eq!(failure.kind, IoFailureKind::BoundaryViolation);
        assert!(
            [rustix::io::Errno::NOTDIR, rustix::io::Errno::LOOP]
                .iter()
                .any(|code| Some(code.raw_os_error()) == failure.os_code)
        );
        assert!(!crate::error::has_effects(error));
        assert_eq!(result.removed_entries, 0);
        assert_eq!(result.reclaimed_estimate, 0);
        assert_eq!(fs::read(selected).unwrap(), b"keep");
        assert!(fs::symlink_metadata(replaced).unwrap().is_symlink());
    }
}

#[test]
fn selected_subtree_includes_new_descendants_but_preserves_exclusions_and_siblings() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("selected");
    fs::create_dir(&directory).unwrap();
    let identity = read_identity(&directory).unwrap();
    fs::write(directory.join("new"), b"new").unwrap();
    fs::write(directory.join("keep"), b"keep").unwrap();
    fs::write(temp.path().join("sibling"), b"sibling").unwrap();
    let result = remove_selected(
        temp.path(),
        &read_identity(temp.path()).unwrap(),
        Path::new("selected"),
        &identity,
        &|path| path == Path::new("selected/keep"),
        &OperationContext::default(),
    );
    assert!(result.error.is_none());
    assert_eq!(result.reclaimed_estimate, 3);
    assert!(!directory.join("new").exists());
    assert_eq!(fs::read(directory.join("keep")).unwrap(), b"keep");
    assert_eq!(fs::read(temp.path().join("sibling")).unwrap(), b"sibling");
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
}

#[test]
fn cancellation_preserves_remaining_content_and_reports_only_observed_effects() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("selected");
    fs::create_dir(&directory).unwrap();
    for name in ["a", "b", "c"] {
        fs::write(directory.join(name), b"data").unwrap();
    }
    let context = OperationContext::default();
    let observed = Cell::new(0);
    let result = remove_selected(
        temp.path(),
        &read_identity(temp.path()).unwrap(),
        Path::new("selected"),
        &read_identity(&directory).unwrap(),
        &|path| {
            if path.components().count() == 2 {
                observed.set(observed.get() + 1);
                if observed.get() == 2 {
                    context.cancel();
                }
            }
            false
        },
        &context,
    );
    assert_eq!(
        classify(result.error.as_ref().unwrap()).kind,
        IoFailureKind::Cancelled
    );
    assert_eq!(result.removed_entries, 1);
    assert_eq!(result.reclaimed_estimate, 4);
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn deadline_restores_the_remaining_subtree_after_partial_deletion() {
    use std::time::{Duration, Instant};
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("selected");
    fs::create_dir(&directory).unwrap();
    for name in ["a", "b", "c"] {
        fs::write(directory.join(name), b"data").unwrap();
    }
    let context = OperationContext {
        deadline: Instant::now() + Duration::from_millis(100),
        ..OperationContext::default()
    };
    let observed = Cell::new(0);
    let result = remove_selected(
        temp.path(),
        &read_identity(temp.path()).unwrap(),
        Path::new("selected"),
        &read_identity(&directory).unwrap(),
        &|path| {
            if path.components().count() == 2 {
                observed.set(observed.get() + 1);
                if observed.get() == 2 {
                    std::thread::sleep(context.deadline.saturating_duration_since(Instant::now()));
                }
            }
            false
        },
        &context,
    );
    assert_eq!(
        result.error.as_ref().unwrap().kind(),
        io::ErrorKind::TimedOut
    );
    assert_eq!(result.removed_entries, 1);
    assert_eq!(result.reclaimed_estimate, 4);
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn native_non_utf8_names_are_not_reparsed_from_display_text() {
    let temp = tempfile::tempdir().unwrap();
    let names = [
        PathBuf::from(std::ffi::OsString::from_vec(vec![b'a', 0xfe])),
        PathBuf::from(std::ffi::OsString::from_vec(vec![b'a', 0xff])),
    ];
    for name in &names {
        fs::write(temp.path().join(name), b"data").unwrap();
    }
    assert_eq!(names[0].to_string_lossy(), names[1].to_string_lossy());
    let result = remove(
        temp.path(),
        &names[0],
        &read_identity(&temp.path().join(&names[0])).unwrap(),
    );
    assert!(result.error.is_none());
    assert!(!temp.path().join(&names[0]).exists());
    assert_eq!(fs::read(temp.path().join(&names[1])).unwrap(), b"data");
}

#[test]
fn a_link_in_the_selected_ancestry_cannot_redirect_deletion() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("outside");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("keep"), b"keep").unwrap();
    let root = temp.path().join("root");
    fs::create_dir(&root).unwrap();
    symlink(&target, root.join("alias")).unwrap();
    let result = remove(
        &root,
        Path::new("alias/keep"),
        &read_identity(&target.join("keep")).unwrap(),
    );
    assert!(result.error.is_some());
    assert_eq!(fs::read(target.join("keep")).unwrap(), b"keep");
}

#[test]
fn concurrent_path_replacement_never_deletes_the_replacement() {
    use std::{
        sync::{Arc, Barrier},
        thread,
    };
    let temp = tempfile::tempdir().unwrap();
    for iteration in 0..64 {
        let root = temp.path().join(iteration.to_string());
        fs::create_dir(&root).unwrap();
        let selected = root.join("selected");
        fs::write(&selected, b"original").unwrap();
        let identity = read_identity(&selected).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let worker_barrier = barrier.clone();
        let worker_root = root.clone();
        let replacer = thread::spawn(move || {
            worker_barrier.wait();
            match fs::rename(worker_root.join("selected"), worker_root.join("original")) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => panic!("Fixture rename failed: {error}"),
            }
            fs::write(worker_root.join("replacement-ready"), b"replacement").unwrap();
            fs::rename(
                worker_root.join("replacement-ready"),
                worker_root.join("selected"),
            )
            .unwrap();
        });
        barrier.wait();
        let _result = remove(&root, Path::new("selected"), &identity);
        replacer.join().unwrap();
        assert_eq!(
            fs::read(&selected).unwrap(),
            b"replacement",
            "iteration {iteration}"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn captured_domain_directory_refuses_replaced_parent_and_selected_objects() {
    for replace_parent in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("parent");
        fs::create_dir(&parent).unwrap();
        let selected = parent.join("cache");
        fs::create_dir(&selected).unwrap();
        fs::write(selected.join("keep"), b"original").unwrap();
        let parent_identity = read_identity(&parent).unwrap();
        let identity = read_identity(&selected).unwrap();
        if replace_parent {
            fs::rename(&parent, temp.path().join("original-parent")).unwrap();
            fs::create_dir(&parent).unwrap();
        } else {
            fs::rename(&selected, parent.join("original-cache")).unwrap();
        }
        fs::create_dir(&selected).unwrap();
        fs::write(selected.join("keep"), b"replacement").unwrap();
        let error = with_captured_directory(
            &selected,
            &parent_identity,
            &identity,
            &OperationContext::default(),
            |_, _| panic!("Changed objects must not reach the command"),
        )
        .unwrap_err();
        assert_eq!(classify(&error).kind, IoFailureKind::TargetChanged);
        assert_eq!(fs::read(selected.join("keep")).unwrap(), b"replacement");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn domain_child_keeps_the_captured_directory_when_original_names_are_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("parent");
    fs::create_dir(&parent).unwrap();
    let selected = parent.join("cache");
    fs::create_dir(&selected).unwrap();
    fs::write(selected.join("selected"), b"original").unwrap();
    let result = with_captured_directory(&selected, &read_identity(&parent).unwrap(),
        &read_identity(&selected).unwrap(), &OperationContext::default(), |cwd, root| {
            fs::rename(&parent, temp.path().join("moved-parent")).unwrap();
            fs::create_dir_all(&selected).unwrap();
            fs::write(selected.join("selected"), b"replacement").unwrap();
            std::process::Command::new("sh")
                .args(["-c", "test \"$(cat -- \"$BOUND/selected\")\" = original && rm -- \"$BOUND/selected\" && rmdir -- \"$BOUND\""])
                .env("BOUND", root).current_dir(cwd).status().unwrap().success()
        }).unwrap();
    assert_eq!(result, Some(true));
    assert_eq!(fs::read(selected.join("selected")).unwrap(), b"replacement");
    assert_eq!(
        fs::read_dir(temp.path().join("moved-parent"))
            .unwrap()
            .count(),
        0
    );
}

#[cfg(target_os = "linux")]
#[test]
fn captured_domain_directory_restores_survivors_without_overwriting_a_new_name() {
    for replace_name in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let selected = temp.path().join("cache");
        fs::create_dir(&selected).unwrap();
        fs::write(selected.join("keep"), b"original").unwrap();
        let identity = read_identity(&selected).unwrap();
        let context = OperationContext::default();
        let result = with_captured_directory(
            &selected,
            &read_identity(temp.path()).unwrap(),
            &identity,
            &context,
            |_, _| {
                if replace_name {
                    fs::create_dir(&selected).unwrap();
                    fs::write(selected.join("keep"), b"replacement").unwrap();
                }
                context.cancel();
            },
        );
        if replace_name {
            let error = result.unwrap_err();
            assert!(error.to_string().contains("surviving content remains"));
            assert!(crate::error::has_effects(&error));
            assert_eq!(fs::read(selected.join("keep")).unwrap(), b"replacement");
            let retained = fs::read_dir(temp.path())
                .unwrap()
                .map(Result::unwrap)
                .find(|entry| is_execution_artifact(&entry.path()))
                .unwrap()
                .path()
                .join("selected");
            assert_eq!(read_identity(&retained).unwrap(), identity);
            assert_eq!(fs::read(retained.join("keep")).unwrap(), b"original");
        } else {
            assert_eq!(result.unwrap(), Some(()));
            assert_eq!(read_identity(&selected).unwrap(), identity);
            assert_eq!(fs::read(selected.join("keep")).unwrap(), b"original");
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        }
    }
}
