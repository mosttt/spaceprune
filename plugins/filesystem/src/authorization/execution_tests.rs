//! Elevated cleanup uses the same selection and native executor as ordinary cleanup.

pub(super) use crate::execute::clean_with_progress;
#[cfg(test)]
use crate::*;

#[cfg(test)]
pub(super) fn clean(
    task: &TaskRequest,
    items: &[CleanupItem],
    context: &OperationContext,
) -> HandlerCleanResult {
    clean_with_progress(task, items, context, |_, _| {})
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::tests::{item, task};
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::tempdir;

    #[test]
    fn excludes_and_unselected_siblings_remain_and_symlinks_do_not_delete_their_targets() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("cache");
        let selected = root.join("selected");
        fs::create_dir_all(&selected).unwrap();
        fs::write(selected.join("remove"), b"remove").unwrap();
        fs::write(selected.join("retain.keep"), b"retain").unwrap();
        let outside = temp.path().join("outside");
        fs::write(&outside, b"outside").unwrap();
        symlink(&outside, selected.join("link")).unwrap();
        let selected_item = item(&selected);
        let mut task = task(&root, "sudo");
        task.options["roots"][0]["exclude"] = json!(["**/*.keep"]);
        let result = clean(&task, &[selected_item], &OperationContext::default());
        assert_eq!(result.succeeded, 1);
        assert!(result.failures.is_empty());
        assert!(!selected.join("remove").exists());
        assert!(selected.join("retain.keep").exists());
        assert!(!selected.join("link").exists());
        assert!(outside.exists());
    }

    #[test]
    fn replacement_and_symlink_ancestors_are_rejected_before_any_deletion() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("cache");
        fs::create_dir(&root).unwrap();
        let path = root.join("file");
        fs::write(&path, b"original").unwrap();
        let selected = item(&path);
        let task = task(&root, "sudo");
        fs::rename(&path, root.join("original")).unwrap();
        fs::write(&path, b"replacement").unwrap();
        let result = clean(&task, &[selected], &OperationContext::default());
        assert_eq!(result.succeeded, 0);
        assert!(result.failures[0].message.contains("replaced"));
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        let alias = temp.path().join("alias");
        symlink(&root, &alias).unwrap();
        let result = clean(
            &super::super::tests::task(&alias, "sudo"),
            &[item(&alias.join("file"))],
            &OperationContext::default(),
        );
        assert_eq!(result.succeeded, 0);
        assert!(path.exists());
    }

    #[test]
    fn an_out_of_scope_item_or_missing_identity_is_not_removed() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("cache");
        fs::create_dir(&root).unwrap();
        let outside = temp.path().join("outside");
        fs::write(&outside, b"retain").unwrap();
        let result = clean(
            &task(&root, "sudo"),
            &[item(&outside)],
            &OperationContext::default(),
        );
        assert_eq!(result.succeeded, 0);
        assert!(outside.exists());
        let mut selected = item(&outside);
        selected
            .opaque
            .as_object_mut()
            .unwrap()
            .remove("filesystem-identity");
        let result = clean(
            &task(temp.path(), "sudo"),
            &[selected],
            &OperationContext::default(),
        );
        assert!(result.failures[0].message.contains("Rescan"));
        assert!(outside.exists());
    }
}
