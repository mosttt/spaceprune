//! Opt-in ordinary-user fixture: a real namespace DAC gap must respect the task's policy.
#[cfg(target_os = "linux")]
fn main() {
    use serde_json::json;
    use spaceprune_access::{NativePath, OperationContext, filesystem::read_identity, go_cache};
    use spaceprune_plugin_api::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicUsize, Ordering},
    };
    if let Some(code) = go_cache::run_if_requested() {
        std::process::exit(code);
    }
    if !std::env::args().any(|argument| argument == "--ignored" || argument == "--include-ignored")
    {
        println!(
            "ignored: requires an ordinary Linux user with a supplementary group and user namespaces; run with --ignored"
        );
        return;
    }
    assert!(!rustix::process::geteuid().is_root());
    go_cache::register_host_entrypoint().unwrap();
    let group = rustix::process::getgroups()
        .unwrap()
        .into_iter()
        .find(|group| *group != rustix::process::getegid())
        .expect("Fixture needs a supplementary group");
    struct CountOnly(AtomicUsize);
    impl CleanupAccess for CountOnly {
        fn clean_authorized(
            &self,
            _: &str,
            _: &[CleanupBatchGroup<'_>],
            _: &dyn ProgressSink,
            _: &OperationContext,
        ) -> PluginResult<Vec<PluginResult<HandlerCleanResult>>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(PluginError::new(
                "fixture-authorization",
                "Recorded request; no system authorizer invoked",
            ))
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join("keep"), b"protected by unmapped group").unwrap();
    rustix::fs::chown(&cache, None, Some(group)).unwrap();
    fs::set_permissions(&cache, fs::Permissions::from_mode(0o555)).unwrap();
    let binding = DirectoryBinding {
        path: NativePath::from_path(&cache),
        parent_identity: read_identity(temp.path()).unwrap(),
        identity: read_identity(&cache).unwrap(),
    };
    let item = CleanupItem {
        logical_size_applicability: Default::default(),
        id: "go-modcache".into(),
        parent_id: None,
        name: "Fixture".into(),
        location: cache.display().to_string(),
        logical_bytes: Some(26),
        reclaimable_bytes: Some(26),
        modified_at: None,
        recommendation: Recommendation::Recommended,
        reason: "fixture".into(),
        tags: vec![],
        opaque: json!({"go-cache-binding": binding}),
    };
    let mut registry = spaceprune_engine::PluginRegistry::new();
    spaceprune_platform_linux::register_system_handlers(&mut registry).unwrap();
    let handler = registry.get("builtin.system-linux/go-modcache").unwrap();
    let authorizations = CountOnly(AtomicUsize::new(0));
    for policy in ["never", "auto"] {
        let task: TaskRequest = serde_json::from_value(json!({
            "module_id":"fixture", "task_id":"go", "name":"Fixture",
            "handler_id":"builtin.system-linux/go-modcache", "options":{"elevation":policy},
            "recommend":{}, "tags":[],
        }))
        .unwrap();
        let results = handler.clean_batch_with_access(
            &[CleanupBatchGroup {
                task: &task,
                items: std::slice::from_ref(&item),
            }],
            &NullProgress,
            &OperationContext::default(),
            Some(&authorizations),
        );
        let result = results.into_iter().next().unwrap();
        if policy == "never" {
            let result = result.unwrap();
            assert_eq!(result.outcomes[0].status, ItemExecutionStatus::Failed);
            assert_eq!(
                result.outcomes[0]
                    .failure
                    .as_ref()
                    .unwrap()
                    .io_failure
                    .as_ref()
                    .unwrap()
                    .kind,
                spaceprune_access::IoFailureKind::AccessDenied
            );
            assert_eq!(authorizations.0.load(Ordering::SeqCst), 0);
        } else {
            assert_eq!(result.unwrap_err().code, "fixture-authorization");
            assert_eq!(authorizations.0.load(Ordering::SeqCst), 1);
        }
        assert_eq!(
            fs::read(cache.join("keep")).unwrap(),
            b"protected by unmapped group"
        );
        assert_eq!(read_identity(&cache).unwrap(), binding.identity);
        assert_eq!(
            fs::metadata(&cache).unwrap().permissions().mode() & 0o777,
            0o555
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }
    fs::set_permissions(&cache, fs::Permissions::from_mode(0o755)).unwrap();
    println!(
        "real supplementary-GID namespace gap: never=0 authorization requests; auto=1 mocked request; identity/content/mode preserved"
    );
}
#[cfg(not(target_os = "linux"))]
fn main() {}
