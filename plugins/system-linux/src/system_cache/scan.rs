//! Preserve ordinary discovery across a native read-only cache inspection.

use super::*;
use spaceprune_access::{IoFailure, IoFailureKind, NativePath, filesystem::inspect_directory};
use spaceprune_plugin_api::{SystemCacheScanRequest, SystemCacheSource};

const SOURCE_KEY: &str = "system-cache-source";

fn kind(task: &TaskRequest) -> PluginResult<Kind> {
    match task.handler_id.as_str() {
        "builtin.system-linux/package-cache" => Ok(Kind::PackageCache),
        "builtin.system-linux/coredumps" => Ok(Kind::Coredumps),
        _ => Err(invalid_source()),
    }
}

fn invalid_source() -> PluginError {
    PluginError::new(
        "invalid-cache-source",
        messages::text("linux.invalid_source"),
    )
}

/// Freeze only sources discovered by the ordinary handler before native authorization.
pub fn prepare(
    task: &TaskRequest,
    result: &HandlerScanResult,
) -> PluginResult<Option<SystemCacheScanRequest>> {
    if spaceprune_plugin_developer_tools::is_cache_handler(&task.handler_id) {
        return spaceprune_plugin_developer_tools::prepare(task, result);
    }
    let Some(item) = result.items.first().filter(|_| result.items.len() == 1) else {
        return Ok(None);
    };
    let source: SystemCacheSource = serde_json::from_value(
        item.opaque
            .get(SOURCE_KEY)
            .cloned()
            .ok_or_else(invalid_source)?,
    )
    .map_err(|_| invalid_source())?;
    if source.roots.is_empty() {
        return Ok(None);
    }
    if source.action_id != item.id {
        return Err(invalid_source());
    }
    let request = SystemCacheScanRequest {
        task: task.clone(),
        source,
    };
    validate(&request)?;
    Ok(Some(request))
}

/// Validate native roots against a registered domain without executing discovery commands.
pub fn validate(request: &SystemCacheScanRequest) -> PluginResult<Vec<PathBuf>> {
    if spaceprune_plugin_developer_tools::is_cache_handler(&request.task.handler_id) {
        return spaceprune_plugin_developer_tools::validate(request);
    }
    let kind = kind(&request.task)?;
    SystemHandler { kind }.validate(&request.task)?;
    let source = &request.source;
    let roots: Vec<_> = source
        .roots
        .iter()
        .map(|path| path.to_path_buf().ok_or_else(invalid_source))
        .collect::<PluginResult<_>>()?;
    if roots.len() != 1
        || roots.iter().any(|path| {
            !path.is_absolute()
                || path.file_name().is_none()
                || path
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
        })
    {
        return Err(invalid_source());
    }
    match kind {
        Kind::Coredumps => {
            if source.action_id != "systemd-coredumps"
                || roots != [PathBuf::from("/var/lib/systemd/coredump")]
                || source.binding.is_some()
            {
                return Err(invalid_source());
            }
        }
        Kind::PackageCache => {
            let manager = source
                .action_id
                .strip_prefix("package-")
                .ok_or_else(invalid_source)?;
            let root = match manager {
                "apt-get" => "/var/cache/apt/archives",
                "dnf" => "/var/cache/dnf",
                "pacman" => "/var/cache/pacman/pkg",
                "zypper" => "/var/cache/zypp/packages",
                _ => return Err(invalid_source()),
            };
            let (options, _) = parse(&request.task)?;
            let configured = if options.manager == "apt" {
                "apt-get"
            } else {
                &options.manager
            };
            if roots != [PathBuf::from(root)]
                || source.binding.is_some()
                || (configured != "auto" && configured != manager)
            {
                return Err(invalid_source());
            }
        }
        _ => return Err(invalid_source()),
    }
    Ok(roots)
}

/// Inspect the frozen source without consulting an administrator's PATH or Go environment.
pub fn inspect(
    request: &SystemCacheScanRequest,
    progress: &dyn ProgressSink,
    context: &OperationContext,
) -> PluginResult<HandlerScanResult> {
    if spaceprune_plugin_developer_tools::is_cache_handler(&request.task.handler_id) {
        return spaceprune_plugin_developer_tools::inspect(request, progress, context);
    }
    validate(request)?;
    measure(
        kind(&request.task)?,
        &request.task,
        request.source.clone(),
        None,
        progress,
        context,
    )
}

pub(super) fn ordinary(
    kind: Kind,
    task: &TaskRequest,
    progress: &dyn ProgressSink,
    context: &OperationContext,
) -> PluginResult<HandlerScanResult> {
    let (options, _) = parse(task)?;
    let action = action(kind, &options, context);
    let program_available = action.available;
    let source = SystemCacheSource {
        action_id: action.id,
        roots: action
            .paths
            .iter()
            .map(|path| NativePath::from_path(path))
            .collect(),
        binding: None,
        program_available,
    };
    measure(
        kind,
        task,
        source,
        action.unavailable_reason,
        progress,
        context,
    )
}

fn measure(
    kind: Kind,
    task: &TaskRequest,
    source: SystemCacheSource,
    unavailable_reason: Option<String>,
    progress: &dyn ProgressSink,
    context: &OperationContext,
) -> PluginResult<HandlerScanResult> {
    let (_, recommend) = parse(task)?;
    let mut bytes = 0_u64;
    let mut measured = false;
    let mut issues = Vec::new();
    let available = source.program_available;
    let roots: Vec<_> = source
        .roots
        .iter()
        .map(|path| path.to_path_buf().ok_or_else(invalid_source))
        .collect::<PluginResult<_>>()?;
    if matches!(kind, Kind::PackageCache | Kind::Coredumps) {
        for path in &roots {
            if context.is_stopped() {
                break;
            }
            progress.emit(ProgressEvent {
                phase: ProgressPhase::Measure,
                module_id: Some(task.module_id.clone()),
                task_id: Some(task.task_id.clone()),
                handler_id: Some(task.handler_id.clone()),
                current: None,
                total: None,
                message: messages::message(
                    "linux.inspect_cache",
                    &[("path", path.display().to_string())],
                ),
            });
            let failure = match inspect_directory(path, source.binding.as_ref(), context) {
                Ok(value) => {
                    bytes = bytes.saturating_add(value.observed_bytes);
                    measured = true;
                    value.failure
                }
                Err(error) => Some(spaceprune_access::error::classify(&error)),
            };
            if let Some(failure) = failure {
                issues.push(issue(path, failure));
            }
        }
    }
    let (recommendation, reason) = if !available {
        (
            Recommendation::Unavailable,
            unavailable_reason.unwrap_or_else(|| messages::text("linux.action_unavailable").into()),
        )
    } else if recommend.enabled {
        (
            Recommendation::Recommended,
            messages::text("linux.recommended").into(),
        )
    } else {
        (
            Recommendation::Optional,
            messages::text("linux.recommend_disabled").into(),
        )
    };
    let complete_bytes = (measured && issues.is_empty()).then_some(bytes);
    if !issues.is_empty() && measured {
        issues[0].message =
            messages::message("linux.measurement_partial", &[("bytes", bytes.to_string())]);
    }
    let location = if roots.is_empty() {
        kind.name().into()
    } else {
        roots
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(" + ")
    };
    Ok(HandlerScanResult {
        items: vec![CleanupItem {
            logical_size_applicability: Default::default(),
            id: source.action_id.clone(),
            parent_id: None,
            name: kind.name().into(),
            location,
            logical_bytes: complete_bytes,
            reclaimable_bytes: complete_bytes,
            modified_at: None,
            recommendation,
            reason,
            tags: vec!["system-cache".into(), kind.id().into()],
            opaque: json!({"system-cache-source": source, "go-cache-binding": source.binding}),
        }],
        issues,
    })
}

fn issue(path: &Path, failure: IoFailure) -> HandlerIssue {
    HandlerIssue {
        code: Some(
            if failure.kind == IoFailureKind::TargetChanged {
                "system-cache-identity-changed"
            } else {
                "system-cache-measurement-incomplete"
            }
            .into(),
        ),
        io_failure: Some(failure),
        level: IssueLevel::Warning,
        message: messages::text("linux.measurement_incomplete").into(),
        location: Some(path.display().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spaceprune_plugin_api::NullProgress;

    fn request(root: &Path) -> SystemCacheScanRequest {
        SystemCacheScanRequest {
            task: TaskRequest {
                module_id: "cache".into(),
                task_id: "go".into(),
                name: "Go cache".into(),
                handler_id: "builtin.system-linux/go-modcache".into(),
                options: json!({}),
                recommend: json!({}),
                tags: vec![],
            },
            source: SystemCacheSource {
                action_id: "go-modcache".into(),
                roots: vec![NativePath::from_path(root)],
                binding: None,
                program_available: true,
            },
        }
    }

    #[test]
    fn frozen_scan_recovers_native_identity_without_rediscovering_the_go_environment() {
        use std::os::unix::ffi::OsStringExt;
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary
            .path()
            .join(OsString::from_vec(b"cache-\xff".to_vec()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("data"), b"fixture").unwrap();
        let source = request(&root);
        let encoded = serde_json::to_vec(&source).unwrap();
        let source = serde_json::from_slice(&encoded).unwrap();
        let result = inspect(&source, &NullProgress, &OperationContext::default()).unwrap();
        assert!(result.issues.is_empty());
        assert_eq!(result.items[0].logical_bytes, Some(7));
        assert_eq!(result.items[0].recommendation, Recommendation::Recommended);
        let rebound = prepare(&source.task, &result).unwrap().unwrap();
        assert_eq!(rebound.source.roots, source.source.roots);
        assert_eq!(
            rebound.source.binding.unwrap().path.to_path_buf().unwrap(),
            root
        );
    }

    #[test]
    fn frozen_scan_rejects_replaced_directories_and_forged_domain_roots() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("cache");
        fs::create_dir(&root).unwrap();
        let mut source = request(&root);
        let result = inspect(&source, &NullProgress, &OperationContext::default()).unwrap();
        source = prepare(&source.task, &result).unwrap().unwrap();
        fs::rename(&root, temporary.path().join("original")).unwrap();
        fs::create_dir(&root).unwrap();
        fs::write(root.join("keep"), b"replacement").unwrap();
        let result = inspect(&source, &NullProgress, &OperationContext::default()).unwrap();
        assert_eq!(result.items[0].recommendation, Recommendation::Unavailable);
        assert_eq!(
            result.issues[0].io_failure.as_ref().unwrap().kind,
            IoFailureKind::TargetChanged
        );
        assert_eq!(fs::read(root.join("keep")).unwrap(), b"replacement");
        source.task.handler_id = "builtin.system-linux/coredumps".into();
        source.source.action_id = "systemd-coredumps".into();
        source.source.binding = None;
        assert!(validate(&source).is_err());
    }
}
