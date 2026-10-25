use super::*;

#[derive(serde::Deserialize)]
struct Origin {
    slot: String,
    source: crate::NativePath,
}

pub(super) fn survivors(staging: &Path) -> Vec<(PathBuf, PathBuf)> {
    fs::read_to_string(staging.join("original-paths.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Origin>(line).unwrap())
        .map(|origin| {
            (
                staging.join(origin.slot),
                origin.source.to_path_buf().unwrap(),
            )
        })
        .filter(|(captured, _)| fs::symlink_metadata(captured).is_ok())
        .collect()
}

fn staging(root: &Path) -> PathBuf {
    fs::read_dir(root)
        .unwrap()
        .map(Result::unwrap)
        .find(|entry| is_execution_artifact(&entry.path()))
        .unwrap()
        .path()
}

#[test]
fn multiple_restore_conflicts_keep_each_survivors_native_original_path() {
    use rustix::fs::{Mode, mkdirat};
    let temp = tempfile::tempdir().unwrap();
    let selected = temp.path().join("selected");
    let candidate = selected.join("candidate");
    let nested_name = std::ffi::OsString::from_vec(vec![b'n', 0xff]);
    let nested = candidate.join(&nested_name);
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("keep"), b"original").unwrap();
    let candidate_identity = read_identity(&candidate).unwrap();
    let nested_identity = read_identity(&nested).unwrap();
    let retained_root = fs::File::open(&selected).unwrap();
    let retained_candidate = fs::File::open(&candidate).unwrap();
    let context = OperationContext::default();
    let interrupted = Cell::new(false);
    let result = remove_selected(
        temp.path(),
        &read_identity(temp.path()).unwrap(),
        Path::new("selected"),
        &read_identity(&selected).unwrap(),
        &|path| {
            if path
                == Path::new("selected/candidate")
                    .join(&nested_name)
                    .join("keep")
                && !interrupted.replace(true)
            {
                mkdirat(&retained_root, "candidate", Mode::RWXU).unwrap();
                mkdirat(&retained_candidate, &nested_name, Mode::RWXU).unwrap();
                context.cancel();
            }
            false
        },
        &context,
    );
    assert!(interrupted.get());
    let error = result.error.unwrap();
    assert!(crate::error::has_effects(&error));
    assert_eq!(result.removed_entries, 0);
    let retained = staging(temp.path());
    let survivors = survivors(&retained);
    assert_eq!(survivors.len(), 2);
    let original_candidate = &survivors
        .iter()
        .find(|(_, source)| source == &candidate)
        .unwrap()
        .0;
    let original_nested = &survivors
        .iter()
        .find(|(_, source)| source == &nested)
        .unwrap()
        .0;
    assert_eq!(
        read_identity(original_candidate).unwrap(),
        candidate_identity
    );
    assert_eq!(read_identity(original_nested).unwrap(), nested_identity);
    assert_eq!(fs::read(original_nested.join("keep")).unwrap(), b"original");
    assert!(original_candidate.join(&nested_name).is_dir());
    assert_ne!(read_identity(&candidate).unwrap(), candidate_identity);
    assert!(error.to_string().contains(retained.to_str().unwrap()));
}

#[test]
fn process_exit_preserves_native_sources_for_detached_objects() {
    let temp = tempfile::tempdir().unwrap();
    let selected = temp.path().join("selected");
    let native_name = std::ffi::OsString::from_vec(vec![b'd', 0xfe]);
    let nested = selected.join(&native_name);
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("keep"), b"survives exit").unwrap();
    let identity = read_identity(&nested).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "filesystem::tests::recovery::capture_exit_fixture",
            "--ignored",
        ])
        .env("SPACEPRUNE_CAPTURE_EXIT_FIXTURE", temp.path())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(73));
    let retained = staging(temp.path());
    let survivors = survivors(&retained);
    assert_eq!(survivors.len(), 2);
    let captured = &survivors
        .iter()
        .find(|(_, source)| source == &nested)
        .unwrap()
        .0;
    assert_eq!(read_identity(captured).unwrap(), identity);
    assert_eq!(fs::read(captured.join("keep")).unwrap(), b"survives exit");
}

#[test]
#[ignore = "Subprocess fixture run by process_exit_preserves_native_sources_for_detached_objects"]
fn capture_exit_fixture() {
    let Some(root) = std::env::var_os("SPACEPRUNE_CAPTURE_EXIT_FIXTURE") else {
        return;
    };
    let root = PathBuf::from(root);
    let selected = root.join("selected");
    remove_selected(
        &root,
        &read_identity(&root).unwrap(),
        Path::new("selected"),
        &read_identity(&selected).unwrap(),
        &|path| {
            if path.components().count() == 3 {
                std::process::exit(73);
            }
            false
        },
        &OperationContext::default(),
    );
    panic!("Fixture must exit while a descendant is detached");
}

#[cfg(target_os = "linux")]
#[test]
fn a_moved_parent_with_a_replacement_reports_the_actual_recovery_directory() {
    let temp = tempfile::tempdir().unwrap();
    let ancestor = temp.path().join("allowed");
    let moved = temp.path().join("protected");
    let selected = ancestor.join("selected");
    fs::create_dir_all(&selected).unwrap();
    fs::write(selected.join("keep"), b"original").unwrap();
    let identity = read_identity(&selected).unwrap();
    let error = with_captured_directory(
        &selected,
        &read_identity(&ancestor).unwrap(),
        &identity,
        &OperationContext::default(),
        |_, _| {
            fs::rename(&ancestor, &moved).unwrap();
            fs::create_dir(moved.join("selected")).unwrap();
            fs::write(moved.join("selected/keep"), b"replacement").unwrap();
        },
    )
    .unwrap_err();
    assert_eq!(classify(&error).kind, IoFailureKind::TargetChanged);
    assert!(crate::error::has_effects(&error));
    let retained = staging(&moved);
    assert!(error.to_string().contains(retained.to_str().unwrap()));
    let survivors = survivors(&retained);
    assert_eq!(survivors.len(), 1);
    assert_eq!(survivors[0].1, selected);
    assert_eq!(read_identity(&survivors[0].0).unwrap(), identity);
    assert_eq!(fs::read(survivors[0].0.join("keep")).unwrap(), b"original");
    assert_eq!(
        fs::read(moved.join("selected/keep")).unwrap(),
        b"replacement"
    );
}
