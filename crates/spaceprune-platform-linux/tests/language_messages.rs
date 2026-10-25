#![cfg(target_os = "linux")]
//! Exercise product messages at public scan/error origins without system commands or authorization.

use std::{fs, sync::Mutex};

use serde_json::json;
use spaceprune_access::NativePath;
use spaceprune_engine::PluginRegistry;
use spaceprune_platform_linux::{PlatformError, inspect_system_cache, register_system_handlers};
use spaceprune_plugin_api::{
    NullProgress, OperationContext, ProgressEvent, ProgressSink, Recommendation,
    SystemCacheScanRequest, SystemCacheSource, TaskRequest, messages::set_display_language,
};

#[derive(Default)]
struct Progress(Mutex<Vec<ProgressEvent>>);

impl ProgressSink for Progress {
    fn emit(&self, event: ProgressEvent) {
        self.0.lock().unwrap().push(event);
    }
}

#[test]
fn public_linux_messages_follow_language_without_changing_observations_or_external_text() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("cache-{path}");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("keep"), b"fixture").unwrap();
    let task: TaskRequest = serde_json::from_value(json!({
        "module_id": "fixture", "task_id": "fixture", "name": "用户原始名称 {name}",
        "handler_id": "builtin.system-linux/package-cache",
        "options": {"manager": "fixture-{manager}", "elevation": "never"},
        "recommend": {}, "tags": [],
    }))
    .unwrap();
    let mut registry = PluginRegistry::new();
    register_system_handlers(&mut registry).unwrap();
    let handler = registry.get(&task.handler_id).unwrap();
    let context = OperationContext::default();
    for (language, name, reason, go_name, progress_prefix, error_prefix) in [
        (
            "en",
            "Package manager cache",
            "Unsupported package manager `fixture-{manager}`",
            "Go module cache",
            "Inspecting cache metadata at ",
            "systemctl --user show fixture failed: ",
        ),
        (
            "zh-CN",
            "软件包管理器缓存",
            "不支持的软件包管理器 `fixture-{manager}`",
            "Go 模块缓存",
            "正在检查 ",
            "systemctl --user show fixture 执行失败：",
        ),
    ] {
        set_display_language(language);
        assert_eq!(handler.descriptor().name, name);
        let result = handler.scan(&task, &NullProgress, &context).unwrap();
        assert_eq!(result.items[0].name, name);
        assert_eq!(result.items[0].reason, reason);
        assert_eq!(result.items[0].recommendation, Recommendation::Unavailable);
        assert_eq!(result.items[0].id, "package-fixture-{manager}");
        assert_eq!(task.name, "用户原始名称 {name}");

        let mut cache_task = task.clone();
        cache_task.handler_id = "builtin.system-linux/go-modcache".into();
        cache_task.options = json!({"elevation":"never"});
        let request = SystemCacheScanRequest {
            task: cache_task,
            source: SystemCacheSource {
                action_id: "go-modcache".into(),
                roots: vec![NativePath::from_path(&root)],
                binding: None,
                program_available: true,
            },
        };
        let progress = Progress::default();
        let result = inspect_system_cache(&request, &progress, &context).unwrap();
        assert!(result.issues.is_empty());
        assert_eq!(result.items[0].name, go_name);
        assert_eq!(result.items[0].logical_bytes, Some(7));
        assert_eq!(result.items[0].location, root.display().to_string());
        let events = progress.0.lock().unwrap();
        assert!(events[0].message.starts_with(progress_prefix));
        assert!(events[0].message.contains(&root.display().to_string()));

        let external = "RAW {message}: permission denied";
        let error = PlatformError::Systemctl {
            action: "show fixture".into(),
            message: external.into(),
        };
        assert_eq!(error.to_string(), format!("{error_prefix}{external}"));
    }
    set_display_language("en");
    assert_eq!(fs::read(root.join("keep")).unwrap(), b"fixture");
}
