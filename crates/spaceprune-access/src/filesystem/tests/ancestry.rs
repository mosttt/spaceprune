use super::*;

#[test]
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos")),
    ignore = "Ancestor movement fence still requires a verified native implementation"
)]
fn moving_any_unselected_ancestor_including_away_and_back_preserves_the_selection() {
    for mode in [
        "same-parent",
        "outside-root",
        "nested-ancestor",
        "away-and-back",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(temp.path()).unwrap();
        let root = base.join("root");
        let parent = root.join("allowed/nested");
        fs::create_dir_all(&parent).unwrap();
        fs::write(parent.join("file"), b"selected").unwrap();
        let identity = read_identity(&parent.join("file")).unwrap();
        let moved = Cell::new(false);
        let current_file = match mode {
            "same-parent" => root.join("protected/nested/file"),
            "outside-root" => base.join("outside/nested/file"),
            "nested-ancestor" => root.join("protected/file"),
            _ => parent.join("file"),
        };
        let result = remove_selected(
            &root,
            &read_identity(&root).unwrap(),
            Path::new("allowed/nested/file"),
            &identity,
            &|path| {
                if path == Path::new("allowed/nested/file") && !moved.replace(true) {
                    match mode {
                        "same-parent" => {
                            fs::rename(root.join("allowed"), root.join("protected")).unwrap()
                        }
                        "outside-root" => {
                            fs::rename(root.join("allowed"), base.join("outside")).unwrap()
                        }
                        "nested-ancestor" => fs::rename(&parent, root.join("protected")).unwrap(),
                        _ => {
                            fs::rename(root.join("allowed"), base.join("outside")).unwrap();
                            fs::rename(base.join("outside"), root.join("allowed")).unwrap();
                        }
                    }
                }
                path.starts_with("protected")
            },
            &OperationContext::default(),
        );
        assert!(moved.get());
        assert_eq!(
            classify(result.error.as_ref().unwrap()).kind,
            IoFailureKind::TargetChanged,
            "{mode}"
        );
        assert_eq!(result.removed_entries, 0, "{mode}");
        assert_eq!(result.reclaimed_estimate, 0, "{mode}");
        assert_eq!(read_identity(&current_file).unwrap(), identity, "{mode}");
        assert_eq!(fs::read(&current_file).unwrap(), b"selected", "{mode}");
        assert_eq!(
            fs::read_dir(current_file.parent().unwrap())
                .unwrap()
                .count(),
            1,
            "{mode}"
        );
    }
}

#[test]
fn unchanged_ancestry_needs_no_new_parent_write_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap().join("root");
    let ancestor = root.join("ancestor");
    let parent = ancestor.join("parent");
    fs::create_dir_all(&parent).unwrap();
    fs::write(parent.join("selected"), b"selected").unwrap();
    fs::write(root.join("sibling"), b"keep").unwrap();
    let identity = read_identity(&parent.join("selected")).unwrap();
    let ancestor_identity = read_identity(&ancestor).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o555)).unwrap();
    let result = remove(&root, Path::new("ancestor/parent/selected"), &identity);
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(result.removed_entries, 1);
    assert_eq!(result.reclaimed_estimate, 8);
    assert_eq!(read_identity(&ancestor).unwrap(), ancestor_identity);
    assert_eq!(fs::read(root.join("sibling")).unwrap(), b"keep");
    assert_eq!(fs::read_dir(parent).unwrap().count(), 0);
}
