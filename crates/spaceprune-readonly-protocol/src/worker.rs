//! Validate one scoped request and execute it under the caller's original operation.

use super::*;

pub(super) fn run_helper(request_path: &Path, expected_nonce: &str) -> Result<(), HelperError> {
    run_helper_for_owner(request_path, expected_nonce, None, None)
}

pub(super) fn run_helper_for_owner(
    request_path: &Path,
    expected_nonce: &str,
    owner: Option<&str>,
    host_stopped: Option<&(dyn Fn() -> bool + Sync)>,
) -> Result<(), HelperError> {
    #[cfg(all(target_os = "windows", not(test)))]
    let _installation = if owner.is_none() {
        Some(crate::client::trust::current_helper()?)
    } else {
        None
    };
    if host_stopped.is_some_and(|stopped| stopped()) {
        return Err(HelperError);
    }
    let request_path = std::path::absolute(request_path).map_err(|_| HelperError)?;
    let request = read_and_validate_request_for_owner(&request_path, expected_nonce, owner)?;
    let context = OperationContext::from_transfer(
        &request.context,
        REQUEST_LIFETIME,
        ResourceBudget::workflow(),
    )
    .map_err(|_| HelperError)?;
    for root in &request.roots {
        context.check().map_err(|_| HelperError)?;
        if !helper_root_is_valid(root, request.operation) {
            return Err(HelperError);
        }
    }
    context.check().map_err(|_| HelperError)?;
    fs::remove_file(&request_path).map_err(|_| HelperError)?;
    let control_directory = request_path.parent().ok_or(HelperError)?;
    let cancellation = context.cancellation.clone();
    let done = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(std::sync::atomic::Ordering::Acquire) {
                if host_stopped.is_some_and(|stopped| stopped())
                    || helper_should_stop(Some(control_directory), Some(request.expires_at))
                {
                    cancellation.cancel();
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let result = execute_request(&request, control_directory, &context);
        done.store(true, std::sync::atomic::Ordering::Release);
        result
    })
}

fn execute_request(
    request: &HelperRequest,
    control_directory: &Path,
    context: &OperationContext,
) -> Result<(), HelperError> {
    let response = execute_value(request, Some(control_directory), context)?;
    send_response(request.response_port, &response, context)
}

fn execute_value(
    request: &HelperRequest,
    control_directory: Option<&Path>,
    context: &OperationContext,
) -> Result<serde_json::Value, HelperError> {
    if matches!(
        request.operation,
        HelperOperation::ScanFilesystemTask
            | HelperOperation::ScanSystemCacheTask
            | HelperOperation::ScanTmpfilesPolicy
            | HelperOperation::ScanSnapperTask
            | HelperOperation::InspectSnapperTask
    ) {
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
        {
            let task = request.scan_task.clone().ok_or(HelperError)?;
            let result = inspect_task(request.operation, task, context);
            return serde_json::to_value(TaskScanResponse {
                schema_version: REQUEST_SCHEMA_VERSION,
                operation: context.identity(),
                source_id: request.source_id.clone(),
                nonce: request.nonce.clone(),
                result,
                cancelled: context.cancellation.is_cancelled(),
            })
            .map_err(|_| HelperError);
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
        return Err(HelperError);
    }
    let aggregate = measure_roots(
        &request.roots,
        control_directory,
        Some(request.expires_at),
        Some(context),
    )?;
    let response = HelperResponse {
        schema_version: REQUEST_SCHEMA_VERSION,
        operation: context.identity(),
        source_id: request.source_id.clone(),
        nonce: request.nonce.clone(),
        outcome: if aggregate.accessible_roots == 0 && aggregate.skipped_entries > 0 {
            HelperResponseOutcome::StillDenied
        } else {
            HelperResponseOutcome::Measured
        },
        logical_bytes: aggregate.logical_bytes,
        inspected_entries: aggregate.inspected_entries,
        skipped_entries: aggregate.skipped_entries,
    };
    serde_json::to_value(response).map_err(|_| HelperError)
}

#[cfg(target_os = "macos")]
pub(super) fn run_macos_request(
    native: &spaceprune_access::macos::service::AuthorizedRequest,
    operation: &OperationContext,
) -> Result<serde_json::Value, HelperError> {
    execute_authenticated_payload(native.body(), operation)
}

#[cfg(any(target_os = "macos", test))]
fn execute_authenticated_payload(
    body: &serde_json::Value,
    operation: &OperationContext,
) -> Result<serde_json::Value, HelperError> {
    operation.check().map_err(|_| HelperError)?;
    if serde_json::to_vec(body).map_err(|_| HelperError)?.len() > MAX_REQUEST_BYTES {
        return Err(HelperError);
    }
    let request: HelperRequest = serde_json::from_value(body.clone()).map_err(|_| HelperError)?;
    let nonce = request.nonce.clone();
    let request = validate_descriptor(request, &nonce)?;
    if request.context.identity != operation.identity() {
        return Err(HelperError);
    }
    let mut context = OperationContext::from_transfer(
        &request.context,
        REQUEST_LIFETIME,
        ResourceBudget::workflow(),
    )
    .map_err(|_| HelperError)?;
    context.deadline = context.deadline.min(operation.deadline);
    context.budget = context.budget.restricted(operation.budget);
    context.cancellation = operation.cancellation.child();
    for root in &request.roots {
        context.check().map_err(|_| HelperError)?;
        if !helper_root_is_valid(root, request.operation) {
            return Err(HelperError);
        }
    }
    execute_value(&request, None, &context)
}

#[cfg(test)]
pub(super) fn read_and_validate_request(
    request_path: &Path,
    expected_nonce: &str,
) -> Result<HelperRequest, HelperError> {
    read_and_validate_request_for_owner(request_path, expected_nonce, None)
}

fn read_and_validate_request_for_owner(
    request_path: &Path,
    expected_nonce: &str,
    owner: Option<&str>,
) -> Result<HelperRequest, HelperError> {
    if !valid_nonce(expected_nonce) || !request_path_matches_nonce(request_path, expected_nonce) {
        return Err(HelperError);
    }
    let file = File::open(request_path).map_err(|_| HelperError)?;
    validate_open_ipc_file(request_path, &file)?;
    #[cfg(target_os = "windows")]
    if let Some(owner) = owner {
        let identity =
            spaceprune_windows_elevation::metadata_open_file_identity(request_path, &file)
                .map_err(|_| HelperError)?;
        if identity.account_sid() != owner {
            return Err(HelperError);
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = owner;
    let mut json = Vec::new();
    file.take(MAX_REQUEST_BYTES as u64 + 1)
        .read_to_end(&mut json)
        .map_err(|_| HelperError)?;
    if json.len() > MAX_REQUEST_BYTES {
        return Err(HelperError);
    }
    let request: HelperRequest = serde_json::from_slice(&json).map_err(|_| HelperError)?;
    validate_descriptor(request, expected_nonce)
}

fn validate_descriptor(
    request: HelperRequest,
    expected_nonce: &str,
) -> Result<HelperRequest, HelperError> {
    let now = unix_timestamp()?;
    if request.schema_version != REQUEST_SCHEMA_VERSION
        || request.nonce != expected_nonce
        || !valid_nonce(&request.nonce)
        || !valid_source_id(&request.source_id)
        || request.issued_at > now
        || request.expires_at < now
        || request.expires_at.saturating_sub(request.issued_at) > REQUEST_LIFETIME.as_secs()
        || !request_root_shape_is_valid(request.operation, &request.roots)
        || request.roots.iter().any(|root| {
            !root.is_absolute() || root.components().any(|part| part == Component::ParentDir)
        })
        || request.response_port == 0
    {
        return Err(HelperError);
    }
    if request.operation == HelperOperation::ScanFilesystemTask {
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
        {
            let task: spaceprune_plugin_api::TaskRequest =
                serde_json::from_value(request.scan_task.clone().ok_or(HelperError)?)
                    .map_err(|_| HelperError)?;
            if filesystem_scan_roots(&task)? != request.roots {
                return Err(HelperError);
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
        return Err(HelperError);
    } else if request.operation == HelperOperation::ScanSystemCacheTask {
        #[cfg(target_os = "linux")]
        {
            let source: spaceprune_plugin_api::SystemCacheScanRequest =
                serde_json::from_value(request.scan_task.clone().ok_or(HelperError)?)
                    .map_err(|_| HelperError)?;
            if spaceprune_plugin_system_linux::validate_system_cache_scan(&source)
                .map_err(|_| HelperError)?
                != request.roots
            {
                return Err(HelperError);
            }
        }
        #[cfg(not(target_os = "linux"))]
        return Err(HelperError);
    } else if request.operation == HelperOperation::ScanTmpfilesPolicy {
        #[cfg(target_os = "linux")]
        {
            let task = serde_json::from_value(request.scan_task.clone().ok_or(HelperError)?)
                .map_err(|_| HelperError)?;
            spaceprune_plugin_system_linux::validate_tmpfiles_task(&task)
                .map_err(|_| HelperError)?;
        }
        #[cfg(not(target_os = "linux"))]
        return Err(HelperError);
    } else if matches!(
        request.operation,
        HelperOperation::ScanSnapperTask | HelperOperation::InspectSnapperTask
    ) {
        #[cfg(target_os = "linux")]
        {
            let task: spaceprune_plugin_api::TaskRequest =
                serde_json::from_value(request.scan_task.clone().ok_or(HelperError)?)
                    .map_err(|_| HelperError)?;
            snapper::validate_task(&task)?;
        }
        #[cfg(not(target_os = "linux"))]
        return Err(HelperError);
    } else if request.scan_task.is_some() {
        return Err(HelperError);
    }
    Ok(request)
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn inspect_task(
    operation: HelperOperation,
    task: serde_json::Value,
    context: &OperationContext,
) -> Result<serde_json::Value, String> {
    use spaceprune_plugin_api::{NullProgress, TaskRequest};
    #[cfg(target_os = "linux")]
    if operation == HelperOperation::ScanSystemCacheTask {
        let source = serde_json::from_value(task).map_err(|error| error.to_string())?;
        return spaceprune_plugin_system_linux::inspect_system_cache(
            &source,
            &NullProgress,
            context,
        )
        .map_err(|error| error.to_string())
        .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string()));
    }
    let task: TaskRequest = serde_json::from_value(task).map_err(|error| error.to_string())?;
    match operation {
        #[cfg(target_os = "linux")]
        HelperOperation::ScanTmpfilesPolicy => {
            spaceprune_plugin_system_linux::inspect_tmpfiles_task(&task, &NullProgress, context)
                .map_err(|error| error.to_string())
                .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string()))
        }
        #[cfg(target_os = "linux")]
        HelperOperation::ScanSnapperTask => snapper::scan(&task, context)
            .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string())),
        #[cfg(target_os = "linux")]
        HelperOperation::InspectSnapperTask => snapper::inventory(&task, context)
            .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string())),
        HelperOperation::ScanFilesystemTask => {
            spaceprune_plugin_filesystem::scan_readonly(&task, &NullProgress, context)
                .map_err(|error| error.to_string())
                .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string()))
        }
        _ => Err("Unsupported native metadata operation".into()),
    }
}

pub(super) fn send_response(
    port: u16,
    response: &impl Serialize,
    context: &OperationContext,
) -> Result<(), HelperError> {
    context.check().map_err(|_| HelperError)?;
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let mut stream = TcpStream::connect_timeout(
        &address,
        Duration::from_secs(2).min(context.deadline.saturating_duration_since(Instant::now())),
    )
    .map_err(|_| HelperError)?;
    stream
        .set_write_timeout(Some(
            Duration::from_secs(10).min(context.deadline.saturating_duration_since(Instant::now())),
        ))
        .map_err(|_| HelperError)?;
    let bytes = serde_json::to_vec(response).map_err(|_| HelperError)?;
    if bytes.len() > context.budget.output_bytes.min(64 * 1024 * 1024) {
        return Err(HelperError);
    }
    stream.write_all(&bytes).map_err(|_| HelperError)?;
    stream.flush().map_err(|_| HelperError)
}

#[cfg(test)]
mod authenticated_payload_tests {
    use super::*;

    #[test]
    fn native_payload_uses_host_owned_content_and_preserves_original_identity_and_cancellation() {
        let root = tempfile::tempdir().unwrap();
        let original = fs::canonicalize(root.path()).unwrap().join("original");
        let other = root.path().join("other");
        fs::create_dir(&original).unwrap();
        fs::create_dir(&other).unwrap();
        fs::write(original.join("data"), b"native").unwrap();
        fs::write(other.join("keep"), b"not part of this request").unwrap();
        let request = PreparedRequest::create_navigation_boundary(
            "navigation-boundary-1234567890abcdef1234567890abcdef",
            &original,
        )
        .unwrap();
        let mut replaced = serde_json::to_value(&request.descriptor).unwrap();
        replaced["roots"] = serde_json::to_value(vec![
            serde_json::json!({"unix": other.as_os_str().as_encoded_bytes()}),
        ])
        .unwrap();
        fs::write(
            &request.request_path,
            serde_json::to_vec(&replaced).unwrap(),
        )
        .unwrap();
        let operation = OperationContext::default();
        let request = request.with_operation(&operation).unwrap();
        assert_eq!(request.descriptor.roots, [original]);
        let body = serde_json::to_value(&request.descriptor).unwrap();
        let response: HelperResponse =
            serde_json::from_value(execute_authenticated_payload(&body, &operation).unwrap())
                .unwrap();
        assert_eq!(response.logical_bytes, 6);
        assert_eq!(response.operation, operation.identity());
        assert_eq!(
            fs::read(other.join("keep")).unwrap(),
            b"not part of this request"
        );
        assert!(execute_authenticated_payload(&body, &OperationContext::default()).is_err());
        operation.cancel();
        assert!(execute_authenticated_payload(&body, &operation).is_err());
    }

    #[test]
    fn authenticated_payload_does_not_reset_expiry_or_accept_different_task_roots() {
        let root = tempfile::tempdir().unwrap();
        let path = fs::canonicalize(root.path()).unwrap();
        fs::write(path.join("cache.log"), b"only metadata").unwrap();
        let operation = OperationContext::default();
        let task = spaceprune_plugin_api::TaskRequest {
            module_id: "fixture".into(),
            task_id: "paths".into(),
            handler_id: "builtin.filesystem/paths".into(),
            name: "fixture".into(),
            tags: vec![],
            options: serde_json::json!({"roots":[{"path": path}], "files": true, "directories": false}),
            recommend: serde_json::json!({}),
        };
        let request = PreparedRequest::create_filesystem_scan("recipe-source-0", &task)
            .unwrap()
            .with_operation(&operation)
            .unwrap();
        let valid = serde_json::to_value(&request.descriptor).unwrap();
        let response: TaskScanResponse<serde_json::Value> =
            serde_json::from_value(execute_authenticated_payload(&valid, &operation).unwrap())
                .unwrap();
        let scan: spaceprune_plugin_api::HandlerScanResult =
            serde_json::from_value(response.result.unwrap()).unwrap();
        assert_eq!(scan.items.len(), 1);
        assert!(scan.items[0].location.ends_with("cache.log"));
        assert_eq!(fs::read(path.join("cache.log")).unwrap(), b"only metadata");
        let mut oversized = valid.clone();
        oversized["scan_task"]["name"] = serde_json::json!("x".repeat(MAX_REQUEST_BYTES));
        assert!(execute_authenticated_payload(&oversized, &operation).is_err());
        let mut body = serde_json::to_value(&request.descriptor).unwrap();
        body["context"]["deadline_monotonic_ns"] = serde_json::json!(0);
        assert!(execute_authenticated_payload(&body, &operation).is_err());
        let mut body = serde_json::to_value(&request.descriptor).unwrap();
        body["scan_task"]["options"]["roots"][0]["path"] = serde_json::json!("/different/root");
        assert!(execute_authenticated_payload(&body, &operation).is_err());
    }
}
