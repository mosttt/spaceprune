use super::*;
use spaceprune_plugin_api::{SystemCacheScanRequest, SystemCacheSource, TaskRequest};
use std::os::unix::fs::{PermissionsExt, symlink};

fn cache_request(root: &Path) -> PreparedRequest {
    PreparedRequest::create_system_cache_scan(
        "recipe-source-0",
        &SystemCacheScanRequest {
            task: TaskRequest {
                module_id: "cache".into(),
                task_id: "go".into(),
                name: "Go cache".into(),
                handler_id: "builtin.system-linux/go-modcache".into(),
                options: serde_json::json!({}),
                recommend: serde_json::json!({}),
                tags: vec![],
            },
            source: SystemCacheSource {
                action_id: "go-modcache".into(),
                roots: vec![spaceprune_access::NativePath::from_path(root)],
                binding: None,
                program_available: true,
            },
        },
    )
    .unwrap()
}

#[test]
fn cache_preparation_defers_physical_access_until_authorized_execution() {
    let (_temporary, root) = root();
    fs::write(root.join("keep"), b"cache content").unwrap();
    let parent = root.parent().unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o000)).unwrap();
    let prepared = cache_request(&root).with_operation(&OperationContext::default());
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
    let prepared = prepared.expect("An inaccessible ancestor must not prevent authorization");
    run_helper(&prepared.request_path, &prepared.nonce).unwrap();
    let (result, cancelled) = prepared
        .receive_scan(&std::sync::atomic::AtomicBool::new(false))
        .unwrap();
    assert!(!cancelled);
    assert!(result.issues.is_empty());
    assert_eq!(result.items[0].logical_bytes, Some(13));
    assert_eq!(fs::read(root.join("keep")).unwrap(), b"cache content");
}

#[test]
fn cache_worker_rejects_mismatched_roots_old_schema_and_new_symlink_ancestors() {
    let (_temporary, root) = root();
    fs::write(root.join("keep"), b"unchanged").unwrap();
    let request = cache_request(&root);
    let original = fs::read(&request.request_path).unwrap();
    for mutation in 0..4 {
        let mut decoded: HelperRequest = serde_json::from_slice(&original).unwrap();
        match mutation {
            0 => decoded.roots = vec![root.join("different")],
            1 => decoded.roots = vec![root.join("../escape")],
            2 => decoded.schema_version = 5,
            _ => {
                decoded.scan_task.as_mut().unwrap()["source"]["action_id"] =
                    serde_json::json!("package-pacman")
            }
        }
        fs::write(&request.request_path, serde_json::to_vec(&decoded).unwrap()).unwrap();
        assert!(run_helper(&request.request_path, &request.nonce).is_err());
        assert!(request.request_path.exists());
    }
    fs::write(&request.request_path, original).unwrap();
    let parent = root.parent().unwrap();
    let moved = parent.with_file_name("original-cache");
    fs::rename(parent, &moved).unwrap();
    symlink(&moved, parent).unwrap();
    assert!(run_helper(&request.request_path, &request.nonce).is_err());
    assert!(request.request_path.exists());
    assert_eq!(fs::read(root.join("keep")).unwrap(), b"unchanged");
}
