use super::*;

#[cfg(target_os = "linux")]
mod cache;

fn root() -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::tempdir().expect("临时目录");
    let root = temporary.path().join("cache/data");
    fs::create_dir_all(&root).expect("测试目录");
    (temporary, fs::canonicalize(root).expect("规范路径"))
}

fn request(root: &Path) -> PreparedRequest {
    PreparedRequest::create("discovery-source-0001", &[root.to_path_buf()]).expect("IPC 请求")
}

fn navigation_request(root: &Path) -> PreparedRequest {
    PreparedRequest::create_navigation_boundary(
        "navigation-boundary-0123456789abcdef0123456789abcdef",
        root,
    )
    .expect("导航边界 IPC 请求")
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[test]
fn helper_preserves_the_original_operation_and_rejects_expired_queued_work() {
    let (_temporary, root) = root();
    fs::write(root.join("keep"), b"unchanged").unwrap();
    let mut operation = OperationContext {
        generation: 8,
        ..OperationContext::default()
    };
    operation.budget.output_bytes = 4567;
    let request = request(&root).with_operation(&operation).unwrap();
    let decoded = read_and_validate_request(&request.request_path, &request.nonce).unwrap();
    assert_eq!(decoded.context.identity, operation.identity());
    assert_eq!(decoded.context.output_bytes, 4567);
    run_helper(&request.request_path, &request.nonce).unwrap();
    let (stream, _) = request.listener.accept().unwrap();
    let response: HelperResponse = serde_json::from_reader(stream).unwrap();
    assert_eq!(response.operation, operation.identity());
    assert_eq!(response.logical_bytes, 9);

    let queued = PreparedRequest::create("recipe-source-0", std::slice::from_ref(&root))
        .unwrap()
        .with_operation(&operation)
        .unwrap();
    let mut expired = read_and_validate_request(&queued.request_path, &queued.nonce).unwrap();
    expired.context.deadline_monotonic_ns = 0;
    fs::write(&queued.request_path, serde_json::to_vec(&expired).unwrap()).unwrap();
    assert!(run_helper(&queued.request_path, &queued.nonce).is_err());
    assert!(
        queued.request_path.exists(),
        "Expired work must not be consumed as an executed request"
    );
    assert_eq!(fs::read(root.join("keep")).unwrap(), b"unchanged");
}

#[test]
fn a_matching_nonce_cannot_substitute_another_operation_generation() {
    let (_temporary, root) = root();
    let request = request(&root);
    let mut response = denied_response(&request.source_id, &request.nonce);
    response.operation = request.context.identity();
    response.operation.generation += 1;
    send_response(
        request.listener.local_addr().unwrap().port(),
        &response,
        &request.context,
    )
    .unwrap();
    assert!(request.receive_response().is_err());
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[test]
fn helper_response_respects_the_callers_output_budget() {
    let (_temporary, root) = root();
    fs::write(root.join("keep"), b"data").unwrap();
    let mut operation = OperationContext::default();
    operation.budget.output_bytes = 16;
    let request = request(&root).with_operation(&operation).unwrap();
    assert!(run_helper(&request.request_path, &request.nonce).is_err());
    assert!(root.join("keep").exists());
}

#[test]
fn an_oversized_request_file_is_rejected_before_consumption_or_traversal() {
    let (_temporary, root) = root();
    let request = request(&root);
    OpenOptions::new()
        .write(true)
        .open(&request.request_path)
        .unwrap()
        .set_len(MAX_REQUEST_BYTES as u64 + 1)
        .unwrap();
    assert!(run_helper(&request.request_path, &request.nonce).is_err());
    assert!(request.request_path.exists());
}

fn denied_response(source_id: &str, nonce: &str) -> HelperResponse {
    HelperResponse {
        schema_version: REQUEST_SCHEMA_VERSION,
        operation: OperationIdentity {
            operation_id: "fixture".into(),
            generation: 0,
        },
        source_id: source_id.to_owned(),
        nonce: nonce.to_owned(),
        outcome: HelperResponseOutcome::StillDenied,
        logical_bytes: 0,
        inspected_entries: 0,
        skipped_entries: 1,
    }
}

#[test]
fn ordinary_startup_and_malformed_nonce_do_not_run_helper() {
    assert!(matches!(
        parse_helper_invocation([OsString::from("spaceprune-desktop")]),
        HelperInvocation::NotHelper
    ));
    assert!(matches!(
        parse_helper_invocation([
            OsString::from("spaceprune-desktop"),
            OsString::from(HELPER_FLAG),
            OsString::from("request.json"),
            OsString::from("bad-nonce")
        ]),
        HelperInvocation::Invalid
    ));
}

#[test]
fn navigation_boundary_ids_are_host_generated_and_strict() {
    assert!(valid_source_id(
        "navigation-boundary-0123456789abcdef0123456789abcdef"
    ));
    assert!(!valid_source_id("navigation-boundary-0001"));
    assert!(!valid_source_id(
        "navigation-boundary-0123456789abcdef0123456789abcdeg"
    ));
}

#[test]
fn navigation_request_is_single_root_and_rejects_a_filesystem_root() {
    let (_temporary, root) = root();
    let request = navigation_request(&root);
    let decoded =
        read_and_validate_request(&request.request_path, &request.nonce).expect("读取导航请求");
    assert_eq!(
        decoded.operation,
        HelperOperation::MeasureNavigationBoundary
    );
    assert_eq!(decoded.roots.as_slice(), std::slice::from_ref(&root));
    assert!(
        PreparedRequest::create_for_operation(
            "navigation-boundary-0123456789abcdef0123456789abcdef",
            &[root.clone(), root],
            HelperOperation::MeasureNavigationBoundary,
        )
        .is_err()
    );
    #[cfg(unix)]
    assert!(
        PreparedRequest::create_navigation_boundary(
            "navigation-boundary-0123456789abcdef0123456789abcdef",
            Path::new("/"),
        )
        .is_err()
    );
}

#[test]
fn elevated_arguments_never_contain_roots_or_shell_commands() {
    let (_temporary, root) = root();
    let request = request(&root);
    let arguments = request.arguments();
    assert_eq!(arguments[0], HELPER_FLAG);
    let rendered = arguments
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(!rendered.contains(root.to_string_lossy().as_ref()));
    assert!(!rendered.contains("sh -c"));
}

#[test]
fn request_rejects_forged_source_and_contains_only_registered_roots() {
    let (_temporary, root) = root();
    assert!(PreparedRequest::create("../../root", std::slice::from_ref(&root)).is_err());
    let request = request(&root);
    let decoded =
        read_and_validate_request(&request.request_path, &request.nonce).expect("读取请求");
    assert_eq!(decoded.source_id, "discovery-source-0001");
    assert_eq!(decoded.roots, [root]);
    let json = fs::read_to_string(&request.request_path).expect("读取 JSON");
    assert!(!json.contains("response_path"));
    assert!(!json.contains("command"));
}

#[test]
fn forged_and_expired_request_is_rejected() {
    let (_temporary, root) = root();
    let request = request(&root);
    let mut decoded: HelperRequest =
        serde_json::from_slice(&fs::read(&request.request_path).expect("读取请求"))
            .expect("解析请求");
    decoded.source_id = "../../root".to_owned();
    decoded.issued_at = 1;
    decoded.expires_at = 2;
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&request.request_path)
        .expect("重写请求");
    serde_json::to_writer(&mut file, &decoded).expect("序列化");
    file.flush().expect("刷新");
    assert!(read_and_validate_request(&request.request_path, &request.nonce).is_err());
}

#[test]
fn request_filename_is_bound_to_nonce() {
    let (_temporary, root) = root();
    let request = request(&root);
    let renamed = request
        .request_path
        .parent()
        .expect("请求目录")
        .join("request-forged.json");
    fs::rename(&request.request_path, &renamed).expect("重命名请求");
    assert!(read_and_validate_request(&renamed, &request.nonce).is_err());
}

#[test]
fn denied_response_requires_matching_schema_source_and_nonce() {
    let source_id = "discovery-source-0001";
    let nonce = "a".repeat(43);
    assert_eq!(
        validate_response(denied_response(source_id, &nonce), source_id, &nonce)
            .expect("合法拒绝响应"),
        ResponseOutcome::Denied
    );

    let mut wrong_schema = denied_response(source_id, &nonce);
    wrong_schema.schema_version = REQUEST_SCHEMA_VERSION + 1;
    assert!(validate_response(wrong_schema, source_id, &nonce).is_err());
    assert!(
        validate_response(
            denied_response("discovery-source-9999", &nonce),
            source_id,
            &nonce,
        )
        .is_err()
    );
    assert!(
        validate_response(
            denied_response(source_id, &"b".repeat(43)),
            source_id,
            &nonce
        )
        .is_err()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn filesystem_metadata_scan_preserves_filters_and_never_executes_cleanup() {
    let (_temporary, root) = root();
    fs::write(root.join("old.log"), b"log content remains").unwrap();
    fs::write(root.join("keep.log"), b"retained").unwrap();
    fs::write(root.join("secret.key"), b"not returned").unwrap();
    let task = spaceprune_plugin_api::TaskRequest {
        module_id: "logs".to_owned(),
        task_id: "files".to_owned(),
        name: "Logs".to_owned(),
        handler_id: "builtin.filesystem/paths".to_owned(),
        options: serde_json::json!({"roots": [{"path": root, "include": ["*.log"], "exclude": ["keep.log"]}, {"path": root.join("optional/absent")}], "files": true, "directories": false}),
        recommend: serde_json::json!({}),
        tags: vec![],
    };
    let request = PreparedRequest::create_filesystem_scan("recipe-source-0", &task).unwrap();
    run_helper(&request.request_path, &request.nonce).unwrap();
    let (result, cancelled) = request
        .receive_scan(&std::sync::atomic::AtomicBool::new(false))
        .unwrap();
    assert!(!cancelled);
    assert_eq!(result.items.len(), 1);
    assert!(result.items[0].location.ends_with("old.log"));
    assert_eq!(
        fs::read(root.join("old.log")).unwrap(),
        b"log content remains"
    );
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("log content remains")
    );
    assert!(run_helper(&request.request_path, &request.nonce).is_err());
    let mut invalid = task;
    invalid.handler_id = "external.plugin/execute".to_owned();
    assert!(PreparedRequest::create_filesystem_scan("recipe-source-0", &invalid).is_err());
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[test]
fn helper_consumes_request_and_returns_only_aggregate_metadata() {
    let (_temporary, root) = root();
    fs::write(root.join("cache.bin"), [0_u8; 23]).expect("测试文件");
    let request = request(&root);

    run_helper(&request.request_path, &request.nonce).expect("执行 Helper");

    assert!(!request.request_path.exists());
    assert_eq!(
        request.receive_response().expect("接收响应"),
        ResponseOutcome::Measured {
            logical_bytes: 23,
            inspected_entries: 1,
            incomplete: false,
        }
    );
    assert!(run_helper(&request.request_path, &request.nonce).is_err());
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[test]
fn helper_measures_a_navigation_boundary_without_returning_names() {
    let (_temporary, root) = root();
    fs::write(root.join("private-name.bin"), [0_u8; 31]).expect("测试文件");
    let request = navigation_request(&root);

    run_helper(&request.request_path, &request.nonce).expect("执行导航 Helper");

    assert_eq!(
        request.receive_response().expect("接收导航响应"),
        ResponseOutcome::Measured {
            logical_bytes: 31,
            inspected_entries: 1,
            incomplete: false,
        }
    );
}

#[cfg(unix)]
#[test]
fn navigation_request_preserves_non_utf8_paths() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt as _};

    let temporary = tempfile::tempdir().expect("临时目录");
    let root = temporary
        .path()
        .join(OsString::from_vec(b"protected-\xff".to_vec()));
    fs::create_dir(&root).expect("测试目录");
    let root = fs::canonicalize(root).expect("规范路径");
    let request = navigation_request(&root);
    let decoded =
        read_and_validate_request(&request.request_path, &request.nonce).expect("读取导航请求");
    assert_eq!(decoded.roots, [root]);
}

#[test]
fn metadata_measurement_never_follows_symlinks_or_reads_contents() {
    let (_temporary, root) = root();
    fs::write(root.join("cache.bin"), [0_u8; 17]).expect("测试文件");
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let outside = _temporary.path().join("outside.bin");
        fs::write(&outside, [0_u8; 99]).expect("外部文件");
        symlink(outside, root.join("outside-link")).expect("符号链接");
    }
    let aggregate = measure_roots(&[root], None, None, None).unwrap();
    assert_eq!(aggregate.logical_bytes, 17);
    #[cfg(unix)]
    assert_eq!(aggregate.inspected_entries, 1);
    assert_eq!(aggregate.accessible_roots, 1);
}

#[cfg(unix)]
#[test]
fn request_directory_and_file_are_private() {
    use std::os::unix::fs::PermissionsExt as _;

    let (_temporary, root) = root();
    let request = request(&root);
    let request_mode = fs::metadata(&request.request_path)
        .expect("请求元数据")
        .permissions()
        .mode();
    let directory_mode = fs::metadata(request.request_path.parent().expect("IPC 目录"))
        .expect("目录元数据")
        .permissions()
        .mode();
    assert_eq!(request_mode & 0o077, 0);
    assert_eq!(directory_mode & 0o077, 0);
}

#[test]
fn metadata_measurement_excludes_interrupted_execution_artifacts() {
    let (_temporary, root) = root();
    fs::write(root.join("cache.bin"), [0_u8; 17]).unwrap();
    for name in [".spaceprune-execute-fixture", ".spaceprune-executions"] {
        let excluded = root.join(name);
        fs::create_dir(&excluded).unwrap();
        fs::write(excluded.join("retained.bin"), [0_u8; 99]).unwrap();
        let direct = measure_roots(&[excluded], None, None, None).unwrap();
        assert_eq!(direct.logical_bytes, 0);
        assert_eq!(direct.accessible_roots, 0);
        assert_eq!(direct.skipped_entries, 1);
    }
    let aggregate = measure_roots(&[root], None, None, None).unwrap();
    assert_eq!(aggregate.logical_bytes, 17);
    assert_eq!(aggregate.inspected_entries, 1);
    assert_eq!(aggregate.skipped_entries, 2);
    assert_eq!(aggregate.accessible_roots, 1);
}
