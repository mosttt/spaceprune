use super::*;
use spaceprune_access::{CancellationToken, Interaction};
use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

const FIXTURE: &str = r#"
import json, pathlib, socket, sys
print('spaceprune-metadata-v6', flush=True)
for line in sys.stdin:
    frame = json.loads(line)
    path = pathlib.Path(frame['requestPath'])
    request = json.loads(path.read_text())
    assert frame['nonce'] == request['nonce']
    path.unlink()
    response = {key: request[key] for key in ('schema_version', 'source_id', 'nonce')}
    response['operation'] = request['context']['identity']
    if request['operation'] == 'inspect-snapper-task':
        kind = 'snapshots' if request['scan_task']['options']['configs'] else 'configs'
        response.update(result={'Ok': {'kind': kind, 'items': []}}, cancelled=False)
    elif request['operation'] == 'scan-system-cache-task':
        source = request['scan_task']['source']
        response.update(result={'Ok': {'items': [{'id': source['action_id'], 'name':'Cache fixture', 'location':'fixture', 'logical_bytes':None, 'reclaimable_bytes':None, 'modified_at':None, 'recommendation':'unavailable', 'reason':'Fixture', 'tags':[], 'opaque':{'system-cache-source':source, 'go-cache-binding':source['binding']}}], 'issues':[]}}, cancelled=False)
    elif 'scan_task' in request:
        response.update(result={'Ok': {'items': [], 'issues': []}}, cancelled=False)
    else:
        response.update(outcome='measured', logical_bytes=23, inspected_entries=1, skipped_entries=0)
    with socket.create_connection(('127.0.0.1', request['response_port'])) as client:
        client.sendall(json.dumps(response).encode())
    print('0', flush=True)
    if len(sys.argv) > 1:
        break
"#;

fn operation() -> OperationContext {
    OperationContext::new(
        CancellationToken::default(),
        Interaction::Foreground,
        Duration::from_secs(5),
    )
}

fn service(script: &str, exit_after_request: bool) -> (MetadataService, Arc<AtomicUsize>) {
    let script = script.to_owned();
    let launches = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&launches);
    (
        MetadataService {
            login: Mutex::default(),
            broker: Arc::default(),
            requests: Arc::default(),
            launcher: Some(Arc::new(move |operation| {
                count.fetch_add(1, Ordering::SeqCst);
                let mut command = Command::new("python3");
                command.args(["-u", "-c", &script]);
                if exit_after_request {
                    command.arg("once");
                }
                Connection::launch(&mut command, operation).map_err(Into::into)
            })),
        },
        launches,
    )
}

fn task(handler: &str, root: &std::path::Path) -> TaskRequest {
    TaskRequest {
        module_id: "test".into(),
        task_id: "metadata".into(),
        name: "Metadata fixture".into(),
        handler_id: handler.into(),
        options: if handler == "builtin.filesystem/paths" {
            serde_json::json!({"roots": [{"path": root}], "files": true})
        } else if handler == "builtin.snapper/snapshots" {
            serde_json::json!({"scan-mode": "snapshots", "configs": ["root"]})
        } else {
            serde_json::json!({})
        },
        recommend: serde_json::json!({}),
        tags: vec![],
    }
}

#[test]
fn observations_filesystem_and_snapshot_scans_share_one_authenticated_process() {
    let root = tempfile::tempdir().unwrap();
    let (service, launches) = service(FIXTURE, false);
    assert!(!service.is_active());
    assert_eq!(launches.load(Ordering::SeqCst), 0);
    let operation = operation();
    let request = PreparedRequest::create("discovery-source-1", &[root.path().to_owned()])
        .unwrap()
        .with_operation(&operation)
        .unwrap();
    assert!(matches!(
        service.measure(&request, &operation),
        Ok(ResponseOutcome::Measured {
            logical_bytes: 23,
            ..
        })
    ));
    for handler in [
        "builtin.filesystem/paths",
        "builtin.snapper/snapshots",
        "builtin.filesystem/paths",
    ] {
        service
            .scan(&task(handler, root.path()), &operation)
            .unwrap();
    }
    service
        .scan_system_cache_with_progress(
            &spaceprune_plugin_api::SystemCacheScanRequest {
                task: task("builtin.system-linux/go-modcache", root.path()),
                source: spaceprune_plugin_api::SystemCacheSource {
                    action_id: "go-modcache".into(),
                    roots: vec![NativePath::from_path(root.path())],
                    binding: None,
                    program_available: true,
                },
            },
            &operation,
            &NullProgress,
        )
        .unwrap();
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    assert!(service.is_active());
    service.close().unwrap();
    assert!(!service.is_active());
}

#[test]
fn concurrent_checks_merge_authorization_and_serialize_the_actual_connection() {
    let root = tempfile::tempdir().unwrap();
    let (service, launches) = service(FIXTURE, false);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let service = &service;
            let root = root.path();
            scope.spawn(move || {
                service
                    .scan(&task("builtin.filesystem/paths", root), &operation())
                    .unwrap();
            });
        }
    });
    assert_eq!(launches.load(Ordering::SeqCst), 1);
}

#[test]
fn refusal_is_shared_across_resources_for_one_operation_but_new_operations_can_retry() {
    let root = tempfile::tempdir().unwrap();
    let (service, launches) = service("import sys; sys.exit(126)", false);
    let operation = operation();
    for handler in ["builtin.filesystem/paths", "builtin.snapper/snapshots"] {
        assert_eq!(
            service.scan(&task(handler, root.path()), &operation),
            Err(PermissionOutcome::Denied)
        );
    }
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    assert!(!service.is_active());
    service
        .scan(
            &task("builtin.filesystem/paths", root.path()),
            &super::tests::operation(),
        )
        .unwrap_err();
    assert_eq!(launches.load(Ordering::SeqCst), 2);
}

#[test]
fn an_exited_helper_is_reauthorized_only_on_the_next_required_check() {
    let root = tempfile::tempdir().unwrap();
    let (service, launches) = service(FIXTURE, true);
    let task = task("builtin.filesystem/paths", root.path());
    service.scan(&task, &operation()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while service.is_active() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!service.is_active());
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    service.scan(&task, &operation()).unwrap();
    assert_eq!(launches.load(Ordering::SeqCst), 2);
}

#[test]
fn legacy_handshakes_and_cancelled_calls_never_become_reusable_grants() {
    let root = tempfile::tempdir().unwrap();
    let task = task("builtin.filesystem/paths", root.path());
    for greeting in ["ready", "spaceprune-metadata-v5"] {
        let (service, launches) = service(&format!("print('{greeting}', flush=True)"), false);
        assert_eq!(
            service.scan(&task, &operation()),
            Err(PermissionOutcome::HelperOutdated)
        );
        assert!(!service.is_active());
        let cancelled = operation();
        cancelled.cancellation.cancel();
        assert_eq!(
            service.scan(&task, &cancelled),
            Err(PermissionOutcome::Cancelled)
        );
        assert_eq!(launches.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn cache_response_cannot_change_the_discovered_root_identity_or_available_program() {
    let root = tempfile::tempdir().unwrap();
    let inspected =
        spaceprune_access::filesystem::inspect_directory(root.path(), None, &operation()).unwrap();
    let source = spaceprune_plugin_api::SystemCacheScanRequest {
        task: task("builtin.system-linux/go-modcache", root.path()),
        source: spaceprune_plugin_api::SystemCacheSource {
            action_id: "go-modcache".into(),
            roots: vec![NativePath::from_path(root.path())],
            binding: Some(inspected.binding),
            program_available: true,
        },
    };
    let other_roots = serde_json::to_string(&vec![NativePath::from_path(
        &root.path().join("another-cache"),
    )])
    .unwrap();
    let replace_root =
        format!("source['roots'] = json.loads({other_roots:?}); source['binding'] = None");
    for mutation in [
        replace_root.as_str(),
        "source['binding']['identity']['inode'] = format(int(source['binding']['identity']['inode'], 16) ^ 1, '016x')",
        "source['binding'] = None",
        "source['program_available'] = False",
    ] {
        let script = FIXTURE.replace(
            "source = request['scan_task']['source']",
            &format!("source = request['scan_task']['source']\n        {mutation}"),
        );
        let (service, _) = service(&script, false);
        assert_eq!(
            service.scan_system_cache_with_progress(&source, &operation(), &NullProgress),
            Err(PermissionOutcome::PolicyBlocked),
            "{mutation}",
        );
    }
}

#[test]
fn cancellation_releases_an_in_flight_request_and_its_process() {
    let root = tempfile::tempdir().unwrap();
    let pid_file = root.path().join("pid");
    let pid_path = pid_file.clone();
    let service = MetadataService {
        login: Mutex::default(),
        broker: Arc::default(),
        requests: Arc::default(),
        launcher: Some(Arc::new(move |operation| {
            let mut command = Command::new("python3");
            command.args(["-u", "-c", "import os, pathlib, sys; pathlib.Path(sys.argv[1]).write_text(str(os.getpid())); print('spaceprune-metadata-v6', flush=True); sys.stdin.read()"]);
            command.arg(&pid_path);
            Connection::launch(&mut command, operation).map_err(Into::into)
        })),
    };
    let operation = operation();
    let cancellation = operation.cancellation.clone();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !pid_file.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            cancellation.cancel();
        });
        assert_eq!(
            service.scan(&task("builtin.filesystem/paths", root.path()), &operation),
            Err(PermissionOutcome::Cancelled)
        );
    });
    let pid = fs::read_to_string(pid_file).unwrap();
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    assert!(!service.is_active());
}

#[test]
fn terminal_output_is_suspended_once_for_snapshot_configs_with_the_same_name() {
    #[derive(Default)]
    struct Progress {
        active: std::sync::atomic::AtomicBool,
        prompts: AtomicUsize,
        scans: AtomicUsize,
    }
    impl ProgressSink for Progress {
        fn set_input_active(&self, active: bool) {
            self.active.store(active, Ordering::SeqCst);
            if active {
                self.prompts.fetch_add(1, Ordering::SeqCst);
            }
        }
        fn emit(&self, event: ProgressEvent) {
            if event.phase == ProgressPhase::Measure {
                assert!(
                    !self.active.load(Ordering::SeqCst),
                    "scanning must not remain hidden behind password input"
                );
                self.scans.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    let root = tempfile::tempdir().unwrap();
    let (service, launches) = service(FIXTURE, false);
    let progress = Progress::default();
    let mut operation = operation();
    operation.interaction = Interaction::Terminal;
    let mut task = task("builtin.snapper/snapshots", root.path());
    for config in ["root", "home"] {
        task.options["configs"] = serde_json::json!([config]);
        service
            .scan_with_progress(&task, &operation, &progress)
            .unwrap();
    }
    assert_eq!(progress.prompts.load(Ordering::SeqCst), 1);
    assert_eq!(progress.scans.load(Ordering::SeqCst), 2);
    assert_eq!(launches.load(Ordering::SeqCst), 1);
}

#[test]
fn system_inventory_requests_authorization_only_on_access_denied_and_reuses_file_access() {
    use spaceprune_plugin_api::PluginError;
    use spaceprune_plugin_snapper::SnapperInventory;
    let (service, launches) = service(FIXTURE, false);
    let operation = operation();
    let readable = service
        .retry_snapper_inventory(
            None,
            &operation,
            &NullProgress,
            Ok(SnapperInventory::Configs(vec![])),
        )
        .unwrap();
    assert!(matches!(readable, SnapperInventory::Configs(_)));
    let ordinary_failure = PluginError::new("invalid-json", "Malformed system response");
    assert_eq!(
        service
            .retry_snapper_inventory(
                None,
                &operation,
                &NullProgress,
                Err(ordinary_failure.clone())
            )
            .unwrap_err(),
        ordinary_failure
    );
    assert_eq!(launches.load(Ordering::SeqCst), 0);
    let root = tempfile::tempdir().unwrap();
    service
        .scan(&task("builtin.filesystem/paths", root.path()), &operation)
        .unwrap();
    for config in [None, Some("root"), Some("home")] {
        let mut denied = PluginError::new("snapper-permission-required", "System access denied");
        denied.io_failure = Some(spaceprune_access::IoFailure {
            kind: spaceprune_access::IoFailureKind::AccessDenied,
            os_code: None,
        });
        let inventory = service
            .retry_snapper_inventory(config, &operation, &NullProgress, Err(denied))
            .unwrap();
        assert!(matches!(
            (config, inventory),
            (None, SnapperInventory::Configs(_)) | (Some(_), SnapperInventory::Snapshots(_))
        ));
    }
    assert_eq!(launches.load(Ordering::SeqCst), 1);
}

#[path = "tests/revocation.rs"]
mod revocation;

#[test]
fn developer_scan_upgrades_only_a_legacy_connection_and_reuses_the_new_capability() {
    let root = tempfile::tempdir().unwrap();
    let launches = Arc::new(AtomicUsize::new(0));
    let count = launches.clone();
    let service = MetadataService {
        login: Mutex::default(),
        broker: Arc::default(),
        requests: Arc::default(),
        launcher: Some(Arc::new(move |operation| {
            let upgraded = count.fetch_add(1, Ordering::SeqCst) > 0;
            let script = FIXTURE.replace("source = request['scan_task']['source']", "source = request['scan_task']['source']\n        assert not request['scan_task']['task']['handler_id'].startswith('builtin.developer-tools/') or __import__('os').environ['HANDLERS_V2'] == 'yes'");
            let mut command = Command::new("python3");
            command
                .args(["-u", "-c", &script])
                .env("HANDLERS_V2", if upgraded { "yes" } else { "no" });
            let mut connection =
                Connection::launch(&mut command, operation).map_err(StartupFailure::from)?;
            connection.supports_developer_handlers = upgraded;
            Ok(connection)
        })),
    };
    let operation = operation();
    for handler in ["builtin.filesystem/paths", "builtin.snapper/snapshots"] {
        service
            .scan(&task(handler, root.path()), &operation)
            .unwrap();
    }
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    let source = spaceprune_plugin_api::SystemCacheScanRequest {
        task: task("builtin.developer-tools/go-modcache", root.path()),
        source: spaceprune_plugin_api::SystemCacheSource {
            action_id: "go-modcache".into(),
            roots: vec![NativePath::from_path(root.path())],
            binding: None,
            program_available: true,
        },
    };
    let request = PreparedRequest::create_system_cache_scan("recipe-source-0", &source).unwrap();
    assert!(request.requires_developer_handlers());
    for _ in 0..2 {
        service
            .scan_system_cache_with_progress(&source, &operation, &NullProgress)
            .unwrap();
    }
    service
        .scan(&task("builtin.filesystem/paths", root.path()), &operation)
        .unwrap();
    assert_eq!(launches.load(Ordering::SeqCst), 2);
    service.close().unwrap();
}
