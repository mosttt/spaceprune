//! Preserve ordinary discovery across a native read-only cache inspection.

use super::*;
use spaceprune_access::{IoFailure, IoFailureKind, NativePath, filesystem::inspect_directory};
use spaceprune_plugin_api::{SystemCacheScanRequest, SystemCacheSource};

const SOURCE_KEY: &str = "system-cache-source";

/// Freeze only sources discovered by the ordinary handler before native authorization.
pub fn prepare(
    task: &TaskRequest,
    result: &HandlerScanResult,
) -> PluginResult<Option<SystemCacheScanRequest>> {
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
    {
        let binding: Option<spaceprune_plugin_api::DirectoryBinding> = serde_json::from_value(
            item.opaque
                .get("go-cache-binding")
                .cloned()
                .ok_or_else(invalid_source)?,
        )
        .map_err(|_| invalid_source())?;
        if binding != source.binding {
            return Err(invalid_source());
        }
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
    let kind = kind(&request.task.handler_id)?;
    DeveloperHandler {
        kind,
        legacy: request.task.handler_id == LEGACY_GO_HANDLER,
    }
    .validate(&request.task)?;
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
    if source.action_id != kind.id()
        || source
            .binding
            .as_ref()
            .is_some_and(|binding| binding.path != source.roots[0])
    {
        return Err(invalid_source());
    }
    // An observed parent alias may resolve outside the conventional spelling; its bound
    // canonical identity remains the authority. Unbound recovery retains the discovered suffix.
    if source.binding.is_none()
        && ((kind == Kind::NpmCache
            && roots[0].file_name() != Some(std::ffi::OsStr::new("_cacache")))
            || (kind == Kind::CargoRegistryCache && !roots[0].ends_with("registry/cache")))
    {
        return Err(invalid_source());
    }
    Ok(roots)
}

/// Inspect the frozen source without consulting an administrator's PATH or Go environment.
pub fn inspect(
    request: &SystemCacheScanRequest,
    progress: &dyn ProgressSink,
    context: &OperationContext,
) -> PluginResult<HandlerScanResult> {
    validate(request)?;
    measure(
        kind(&request.task.handler_id)?,
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
    parse(task)?;
    let action = discovery::action(kind, context);
    let program_available = !action.paths.is_empty();
    let source = SystemCacheSource {
        action_id: action.id,
        roots: action
            .paths
            .iter()
            .map(|path| NativePath::from_path(path))
            .collect(),
        binding: action.go_binding,
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
    mut source: SystemCacheSource,
    mut unavailable_reason: Option<String>,
    progress: &dyn ProgressSink,
    context: &OperationContext,
) -> PluginResult<HandlerScanResult> {
    let (_, recommend) = parse(task)?;
    let mut bytes = 0_u64;
    let mut measured = false;
    let mut issues = Vec::new();
    let mut available = source.program_available && source.binding.is_some();
    let roots: Vec<_> = source
        .roots
        .iter()
        .map(|path| path.to_path_buf().ok_or_else(invalid_source))
        .collect::<PluginResult<_>>()?;
    {
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
                    {
                        source.binding = Some(value.binding);
                        available = source.program_available;
                        if available {
                            unavailable_reason = None;
                        }
                        if value
                            .failure
                            .as_ref()
                            .is_some_and(|failure| failure.kind == IoFailureKind::BoundaryViolation)
                        {
                            available = false;
                            unavailable_reason = Some(messages::text("linux.nested_mount").into());
                        }
                    }
                    value.failure
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    measured = true;
                    available = false;
                    source.binding = None;
                    unavailable_reason = Some(messages::text("developer.cache_missing").into());
                    None
                }
                Err(error) => {
                    {
                        available = false;
                        unavailable_reason = Some(messages::message(
                            "developer.cache_identity_unavailable",
                            &[("error", error.to_string())],
                        ));
                    }
                    Some(spaceprune_access::error::classify(&error))
                }
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
        (Recommendation::Recommended, kind.consequence().into())
    } else {
        (Recommendation::Optional, kind.consequence().into())
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
            tags: vec!["developer-cache".into(), kind.id().into()],
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
        source.task.handler_id = "builtin.developer-tools/npm-cache".into();
        source.source.action_id = "npm-cache".into();
        source.source.binding = None;
        assert!(validate(&source).is_err());
    }
}

#[cfg(test)]
mod developer_tests {
    use super::*;
    use spaceprune_plugin_api::{ItemExecutionStatus, NullProgress};

    #[test]
    fn missing_cache_is_empty_and_unselectable_then_remeasured_if_created() {
        for (kind, suffix) in [
            (Kind::NpmCache, "npm/_cacache"),
            (Kind::CargoRegistryCache, "cargo/registry/cache"),
            (Kind::GoBuildCache, "go/build"),
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path().join(suffix);
            let task = TaskRequest {
                module_id: "fixture".into(),
                task_id: "cache".into(),
                name: "Cache".into(),
                handler_id: format!("{PLUGIN_ID}/{}", kind.id()),
                options: json!({"elevation": "never"}),
                recommend: json!({}),
                tags: vec![],
            };
            let request = SystemCacheScanRequest {
                task,
                source: SystemCacheSource {
                    action_id: kind.id().into(),
                    roots: vec![NativePath::from_path(&root)],
                    binding: None,
                    program_available: true,
                },
            };
            let empty = inspect(&request, &NullProgress, &OperationContext::default()).unwrap();
            assert!(empty.issues.is_empty());
            assert_eq!(empty.items[0].logical_bytes, Some(0));
            assert_eq!(empty.items[0].reclaimable_bytes, Some(0));
            assert_eq!(empty.items[0].recommendation, Recommendation::Unavailable);
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("package"), b"fixture").unwrap();
            let measured = inspect(&request, &NullProgress, &OperationContext::default()).unwrap();
            assert!(measured.issues.is_empty());
            assert_eq!(measured.items[0].logical_bytes, Some(7));
            assert_eq!(
                measured.items[0].recommendation,
                Recommendation::Recommended
            );
        }
    }

    #[test]
    fn npm_and_cargo_cleanup_preserve_siblings_and_count_actual_removed_bytes() {
        for (kind, suffix) in [
            (Kind::NpmCache, "npm/_cacache"),
            (Kind::CargoRegistryCache, "cargo/registry/cache"),
            (Kind::GoBuildCache, "go/build"),
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path().join(suffix);
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("download"), b"cached").unwrap();
            let protected = root.parent().unwrap().join("credentials-and-source");
            fs::write(&protected, b"keep").unwrap();
            let task = TaskRequest {
                module_id: "developer".into(),
                task_id: kind.id().into(),
                name: kind.name().into(),
                handler_id: format!("{PLUGIN_ID}/{}", kind.id()),
                options: json!({"elevation":"never"}),
                recommend: json!({}),
                tags: vec![],
            };
            let request = SystemCacheScanRequest {
                task: task.clone(),
                source: SystemCacheSource {
                    action_id: kind.id().into(),
                    roots: vec![NativePath::from_path(&root)],
                    binding: None,
                    program_available: true,
                },
            };
            let scan = inspect(&request, &NullProgress, &OperationContext::default()).unwrap();
            assert_eq!(scan.items[0].logical_bytes, Some(6));
            fs::write(root.join("late-download"), b"later").unwrap();
            let handler = DeveloperHandler {
                kind,
                legacy: false,
            };
            let result = handler
                .clean(
                    &task,
                    &scan.items,
                    &NullProgress,
                    &OperationContext::default(),
                )
                .unwrap();
            assert_eq!(result.outcomes[0].status, ItemExecutionStatus::Succeeded);
            assert_eq!(result.reclaimed_bytes, 11);
            assert_eq!(result.unknown_reclaimed_items(), 0);
            assert!(!root.exists());
            assert_eq!(fs::read(protected).unwrap(), b"keep");
        }
    }

    #[test]
    fn an_observed_cargo_registry_alias_keeps_its_canonical_bound_source() {
        use std::os::unix::fs::symlink;
        let fixture = tempfile::tempdir().unwrap();
        let home = fixture.path().join("cargo");
        let registry = fixture.path().join("shared-downloads");
        fs::create_dir(&home).unwrap();
        fs::create_dir_all(registry.join("cache")).unwrap();
        fs::write(registry.join("cache/package.crate"), b"archive").unwrap();
        symlink(&registry, home.join("registry")).unwrap();
        let action = cache::scan_action(true, Some(home.join("registry/cache")));
        let source = SystemCacheSource {
            action_id: "cargo-registry-cache".into(),
            roots: action
                .paths
                .iter()
                .map(|path| NativePath::from_path(path))
                .collect(),
            binding: action.go_binding,
            program_available: true,
        };
        let task = TaskRequest {
            module_id: "fixture".into(),
            task_id: "cargo".into(),
            name: "Cargo".into(),
            handler_id: "builtin.developer-tools/cargo-registry-cache".into(),
            options: json!({"elevation":"never"}),
            recommend: json!({}),
            tags: vec![],
        };
        let request = SystemCacheScanRequest {
            task: task.clone(),
            source,
        };
        let scanned = inspect(&request, &NullProgress, &OperationContext::default()).unwrap();
        assert_eq!(scanned.items[0].logical_bytes, Some(7));
        assert_eq!(
            prepare(&task, &scanned).unwrap().unwrap().source.roots,
            request.source.roots
        );
        assert_eq!(
            fs::read(registry.join("cache/package.crate")).unwrap(),
            b"archive"
        );
    }

    #[test]
    fn new_handler_and_legacy_go_share_binding_and_cancel_before_changes() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("cache");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("keep"), b"keep").unwrap();
        for handler_id in [LEGACY_GO_HANDLER, "builtin.developer-tools/go-modcache"] {
            let request = SystemCacheScanRequest {
                task: TaskRequest {
                    module_id: "fixture".into(),
                    task_id: "go".into(),
                    name: "Go".into(),
                    handler_id: handler_id.into(),
                    options: json!({"elevation":"never"}),
                    recommend: json!({}),
                    tags: vec![],
                },
                source: SystemCacheSource {
                    action_id: "go-modcache".into(),
                    roots: vec![NativePath::from_path(&root)],
                    binding: None,
                    program_available: true,
                },
            };
            let scan = inspect(&request, &NullProgress, &OperationContext::default()).unwrap();
            let context = OperationContext::default();
            context.cancel();
            let handler = DeveloperHandler {
                kind: Kind::GoModCache,
                legacy: handler_id == LEGACY_GO_HANDLER,
            };
            let result = handler
                .clean(&request.task, &scan.items, &NullProgress, &context)
                .unwrap();
            assert_eq!(result.outcomes[0].status, ItemExecutionStatus::NotAttempted);
            assert_eq!(fs::read(root.join("keep")).unwrap(), b"keep");
        }
    }
}
