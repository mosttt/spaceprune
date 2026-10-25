//! Frozen tool-cache identity and native execution; display paths are never authority.

use super::*;
use spaceprune_access::{NativePath, filesystem::read_identity};

pub(super) type Binding = spaceprune_plugin_api::DirectoryBinding;

pub(super) fn observe(path: &Path) -> io::Result<Binding> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(io::Error::other(messages::text(
            "developer.cache_root_not_directory",
        )));
    }
    let observed = read_identity(path)?;
    let path = fs::canonicalize(path)?;
    if spaceprune_access::filesystem::is_execution_artifact(&path) {
        return Err(io::Error::other(messages::text(
            "linux.interrupted_contents",
        )));
    }
    let parent = path
        .parent()
        .filter(|_| path.file_name().is_some())
        .ok_or_else(|| io::Error::other(messages::text("developer.cache_root_filesystem")))?;
    let identity = read_identity(&path)?;
    if identity != observed {
        return Err(io::Error::other(messages::text("developer.cache_changed")));
    }
    Ok(Binding {
        parent_identity: read_identity(parent)?,
        identity,
        path: NativePath::from_path(&path),
    })
}

pub(super) fn root(binding: &Binding) -> io::Result<PathBuf> {
    binding
        .path
        .to_path_buf()
        .filter(|path| path.is_absolute() && path.file_name().is_some())
        .ok_or_else(|| io::Error::other(messages::text("developer.cache_invalid_binding")))
}

pub(super) fn scan_action(go_available: bool, cache_path: Option<PathBuf>) -> Action {
    let mut unavailable_reason = if !go_available {
        Some(messages::message(
            "linux.program_missing",
            &[("program", "go".into())],
        ))
    } else if cache_path.is_none() {
        Some(messages::text("linux.go_path_unknown").into())
    } else {
        None
    };
    let binding = if unavailable_reason.is_none() {
        cache_path.as_ref().and_then(|path| match observe(path) {
            Ok(binding) => Some(binding),
            Err(error) => {
                unavailable_reason = Some(messages::message(
                    "developer.cache_identity_unavailable",
                    &[("error", error.to_string())],
                ));
                None
            }
        })
    } else {
        None
    };
    let path = binding
        .as_ref()
        .and_then(|binding| root(binding).ok())
        .or(cache_path);
    Action {
        id: "go-modcache".into(),
        paths: path.into_iter().collect(),
        go_binding: binding,
        unavailable_reason,
    }
}

pub(super) fn selected_action(kind: Kind, item: &CleanupItem) -> Result<Action, String> {
    let binding: Binding = item
        .opaque
        .get("go-cache-binding")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or_else(|| messages::text("developer.cache_invalid_binding").to_owned())?;
    let root = root(&binding).map_err(|error| error.to_string())?;
    if item.id != kind.id() {
        return Err(messages::text("developer.cache_selected_unavailable").into());
    }
    Ok(Action {
        id: kind.id().into(),
        paths: vec![root.clone()],
        go_binding: Some(binding),
        unavailable_reason: None,
    })
}

pub(super) fn execute(
    kind: Kind,
    action: Action,
    task: &TaskRequest,
    progress: &dyn ProgressSink,
    context: &OperationContext,
) -> Result<Option<u64>, ActionFailure> {
    #[cfg(target_os = "linux")]
    {
        let binding = action.go_binding.ok_or_else(|| {
            ActionFailure::BeforeStart(messages::text("developer.cache_invalid_binding").into())
        })?;
        progress.emit(ProgressEvent {
            phase: ProgressPhase::Clean,
            module_id: Some(task.module_id.clone()),
            task_id: Some(task.task_id.clone()),
            handler_id: Some(task.handler_id.clone()),
            current: None,
            total: None,
            message: messages::text("developer.cache_check_identity").into(),
        });
        let cleaned = if kind == Kind::GoModCache {
            spaceprune_access::go_cache::remove(&binding, context)
        } else {
            let path = root(&binding).map_err(|e| ActionFailure::BeforeStart(e.to_string()))?;
            spaceprune_access::go_cache::Cleanup {
                removal: spaceprune_access::filesystem::remove_selected(
                    path.parent().expect("bound cache parent"),
                    &binding.parent_identity,
                    Path::new(path.file_name().expect("bound cache filename")),
                    &binding.identity,
                    &|_| false,
                    context,
                ),
                unconfirmed: false,
            }
        };
        let removed = cleaned.removal;
        if let Some(error) = removed.error {
            return Err(ActionFailure::Native {
                started: cleaned.unconfirmed
                    || removed.removed_entries > 0
                    || spaceprune_access::error::has_effects(&error),
                reclaimed: removed.reclaimed_estimate,
                error,
            });
        }
        Ok((!removed.already_absent).then_some(removed.reclaimed_estimate))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (kind, action, task, progress, context);
        Err(ActionFailure::BeforeStart(
            messages::text("developer.cache_unsupported").into(),
        ))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use spaceprune_plugin_api::NullProgress;

    fn task() -> TaskRequest {
        serde_json::from_value(
            json!({"module_id":"fixture", "task_id":"go", "name":"Go cache",
            "handler_id":"builtin.system-linux/go-modcache", "options":{"elevation":"never"},
            "recommend":{}, "tags":[]}),
        )
        .unwrap()
    }

    #[test]
    fn directory_replacement_is_rejected_before_the_domain_command() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("cache");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("keep"), b"original").unwrap();
        let action = scan_action(true, Some(root.clone()));
        assert!(action.unavailable_reason.is_none());
        fs::rename(&root, temp.path().join("original")).unwrap();
        fs::create_dir(&root).unwrap();
        fs::write(root.join("keep"), b"replacement").unwrap();
        let result = execute(
            Kind::GoModCache,
            action,
            &task(),
            &NullProgress,
            &OperationContext::default(),
        );
        let Err(ActionFailure::Native {
            started: false,
            error,
            ..
        }) = result
        else {
            panic!("The command must not start: {result:?}");
        };
        assert_eq!(
            spaceprune_access::error::classify(&error).kind,
            spaceprune_access::IoFailureKind::TargetChanged
        );
        assert_eq!(fs::read(root.join("keep")).unwrap(), b"replacement");
        assert_eq!(
            fs::read(temp.path().join("original/keep")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn missing_cache_is_idempotent_and_interrupted_artifacts_prevent_execution() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("cache");
        fs::create_dir(&root).unwrap();
        let action = scan_action(true, Some(root.clone()));
        fs::remove_dir(&root).unwrap();
        assert_eq!(
            execute(
                Kind::GoModCache,
                action,
                &task(),
                &NullProgress,
                &OperationContext::default()
            )
            .unwrap(),
            None
        );
        fs::create_dir(&root).unwrap();
        let action = scan_action(true, Some(root.clone()));
        fs::create_dir(root.join(".spaceprune-execute-interrupted")).unwrap();
        fs::write(root.join(".spaceprune-execute-interrupted/keep"), b"keep").unwrap();
        let result = execute(
            Kind::GoModCache,
            action,
            &task(),
            &NullProgress,
            &OperationContext::default(),
        );
        let Err(ActionFailure::Native {
            started: false,
            error,
            ..
        }) = result
        else {
            panic!("Interrupted contents cannot be consumed: {result:?}");
        };
        assert_eq!(
            spaceprune_access::error::classify(&error).kind,
            spaceprune_access::IoFailureKind::BoundaryViolation
        );
        assert_eq!(
            fs::read(root.join(".spaceprune-execute-interrupted/keep")).unwrap(),
            b"keep"
        );
    }

    #[test]
    fn native_cleanup_counts_runtime_contents_instead_of_the_scanned_estimate() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("cache");
        fs::create_dir(&root).unwrap();
        let action = scan_action(true, Some(root.clone()));
        let item = CleanupItem {
            logical_size_applicability: Default::default(),
            id: action.id,
            parent_id: None,
            name: "Go cache".into(),
            location: root.display().to_string(),
            logical_bytes: None,
            reclaimable_bytes: None,
            modified_at: None,
            recommendation: Recommendation::Recommended,
            reason: "fixture".into(),
            tags: vec![],
            opaque: json!({"go-cache-binding": action.go_binding}),
        };
        fs::write(root.join("late-module"), b"runtime content").unwrap();
        let handler = DeveloperHandler {
            legacy: true,
            kind: Kind::GoModCache,
        };
        let cleaned = handler
            .clean_with_access(
                &task(),
                &[item],
                &NullProgress,
                &OperationContext::default(),
                None,
            )
            .unwrap();
        assert_eq!(cleaned.succeeded, 1);
        assert_eq!(cleaned.reclaimed_bytes, 15);
        assert_eq!(cleaned.unknown_reclaimed_items(), 0);
        assert!(!root.exists());
    }
}
