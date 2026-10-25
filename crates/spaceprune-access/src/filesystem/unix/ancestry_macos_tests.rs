use super::*;
use std::fs;

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Vec<OwnedFd>) {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    fs::create_dir_all(root.join("ancestor/parent")).unwrap();
    let root_fd = open_root(&root).unwrap();
    let ancestor = directory(&root_fd, OsStr::new("ancestor")).unwrap();
    let parent = directory(&ancestor, OsStr::new("parent")).unwrap();
    (temp, root, vec![root_fd, ancestor, parent])
}

fn watch(chain: &[OwnedFd]) -> Guard<'_> {
    Guard::watch(
        chain,
        &[OsStr::new("ancestor"), OsStr::new("parent")],
        &OperationContext::default(),
    )
    .unwrap()
    .unwrap()
}

#[test]
fn generation_detects_an_ancestor_rename_without_reading_its_notification() {
    for away_and_back in [false, true] {
        let (_temp, root, chain) = fixture();
        let guard = watch(&chain);
        fs::rename(root.join("ancestor"), root.join("moved")).unwrap();
        if away_and_back {
            fs::rename(root.join("moved"), root.join("ancestor")).unwrap();
        }
        // Inspect the production FD attribute directly, independently of event delivery.
        assert_ne!(generation(&chain[0]).unwrap(), guard.generations[0]);
        let error = guard
            .after_capture(&OperationContext::default())
            .unwrap_err();
        assert_eq!(
            crate::error::classify(&error).kind,
            IoFailureKind::TargetChanged
        );
    }
}

#[test]
fn sibling_invalidation_survives_equal_raw_counters_and_repeated_checks() {
    let (_temp, root, chain) = fixture();
    fs::write(root.join("sibling"), b"keep").unwrap();
    let mut guard = watch(&chain);
    fs::rename(root.join("sibling"), root.join("renamed-sibling")).unwrap();
    // Model raw-counter equality after rollover. Only the retained parent event can reject
    // this observation; watching the ancestor's own NOTE_RENAME would miss the sibling.
    guard.generations[0] = generation(&chain[0]).unwrap();
    for _ in 0..2 {
        let error = guard
            .after_capture(&OperationContext::default())
            .unwrap_err();
        assert_eq!(
            crate::error::classify(&error).kind,
            IoFailureKind::TargetChanged
        );
    }
    assert_eq!(fs::read(root.join("renamed-sibling")).unwrap(), b"keep");
}

#[test]
fn staging_and_runtime_children_do_not_invalidate_unselected_ancestors() {
    let (_temp, root, chain) = fixture();
    let parent = root.join("ancestor/parent");
    fs::write(parent.join("selected"), b"selected").unwrap();
    let guard = watch(&chain);
    fs::create_dir(parent.join("staging")).unwrap();
    fs::rename(parent.join("selected"), parent.join("staging/captured")).unwrap();
    fs::write(parent.join("new-runtime-child"), b"keep").unwrap();
    guard.after_capture(&OperationContext::default()).unwrap();
    fs::rename(parent.join("staging/captured"), parent.join("selected")).unwrap();
    fs::remove_dir(parent.join("staging")).unwrap();
    guard.after_capture(&OperationContext::default()).unwrap();
}

#[test]
fn root_remains_an_identity_anchor_after_its_own_path_is_renamed() {
    let temp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(temp.path()).unwrap();
    let root = base.join("root");
    fs::create_dir_all(root.join("ancestor/parent")).unwrap();
    let root_fd = open_root(&root).unwrap();
    let ancestor = directory(&root_fd, OsStr::new("ancestor")).unwrap();
    let parent = directory(&ancestor, OsStr::new("parent")).unwrap();
    let chain = vec![root_fd, ancestor, parent];
    let guard = watch(&chain);
    fs::rename(&root, base.join("renamed-root")).unwrap();
    guard.after_capture(&OperationContext::default()).unwrap();
}

#[test]
fn exchanged_ancestor_is_rejected_when_supported() {
    let (_temp, root, chain) = fixture();
    fs::create_dir(root.join("peer")).unwrap();
    let guard = watch(&chain);
    let exchange = rustix::fs::renameat_with(
        &chain[0],
        "ancestor",
        &chain[0],
        "peer",
        rustix::fs::RenameFlags::EXCHANGE,
    );
    if matches!(exchange, Err(error) if error.raw_os_error() == libc::ENOTSUP) {
        // HFS rejects exchange renames. Verify that no move occurred rather than treating
        // a rejected system call as evidence for the successful-exchange protection path.
        eprintln!("RENAME_SWAP unavailable on this volume; checking rejected operation only");
        assert!(root.join("ancestor/parent").is_dir());
        assert!(!root.join("peer/parent").exists());
        guard.after_capture(&OperationContext::default()).unwrap();
        return;
    }
    exchange.unwrap();
    let error = guard
        .after_capture(&OperationContext::default())
        .unwrap_err();
    assert_eq!(
        crate::error::classify(&error).kind,
        IoFailureKind::TargetChanged
    );
}

#[test]
fn a_move_before_watch_registration_is_rejected_by_identity_revalidation() {
    let (_temp, root, chain) = fixture();
    fs::rename(root.join("ancestor"), root.join("moved")).unwrap();
    let result = Guard::watch(
        &chain,
        &[OsStr::new("ancestor"), OsStr::new("parent")],
        &OperationContext::default(),
    );
    assert!(matches!(result, Err(ref error)
        if crate::error::classify(error).kind == IoFailureKind::TargetChanged));
}

#[test]
fn queue_failure_is_not_reported_as_an_unchanged_ancestor() {
    let (_temp, _root, chain) = fixture();
    let error = check_events(&chain[0]).unwrap_err();
    assert_eq!(
        crate::error::classify(&error).kind,
        IoFailureKind::Unsupported
    );
}
