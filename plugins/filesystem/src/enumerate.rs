//! Filesystem enumerate responsibilities.

use super::*;

pub(super) struct ScanRoot<'a> {
    pub(super) path: &'a Path,
    pub(super) index: usize,
}

/// Saved one-shot selections enumerate only their observed objects; a bound directory retains
/// its declared subtree semantics without admitting neighboring objects from a broader glob.
pub(super) fn collect_selection(
    root: &Path,
    declaration: &RootOptions,
    matcher: &RootMatcher,
    options: &Options,
    issues: &mut Vec<HandlerIssue>,
    context: &OperationContext,
) -> Vec<PathBuf> {
    if declaration.bindings.is_empty()
        || declaration
            .bindings
            .iter()
            .any(|binding| binding.relative.is_empty())
    {
        return collect_root(root, matcher, options, issues, context);
    }
    let mut paths = Vec::new();
    for binding in &declaration.bindings {
        if context.is_stopped() {
            break;
        }
        let target = binding
            .relative
            .iter()
            .fold(root.to_path_buf(), |path, part| path.join(part));
        paths.extend(collect_path(&target, root, false, matcher, options, issues, context).paths);
    }
    paths
}

pub(super) fn collect_root(
    root: &Path,
    matcher: &RootMatcher,
    options: &Options,
    issues: &mut Vec<HandlerIssue>,
    context: &OperationContext,
) -> Vec<PathBuf> {
    if spaceprune_access::filesystem::is_execution_artifact(root) {
        return Vec::new();
    }
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) => {
            issues.push(warning(
                root,
                spaceprune_plugin_api::messages::message(
                    "filesystem.read_path",
                    &[("error", (error).to_string())],
                ),
                &error,
            ));
            return Vec::new();
        }
    };
    if !metadata.is_dir() {
        let relative = root
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        return if options.files && matcher.included(&relative) && !matcher.excluded(&relative) {
            vec![root.to_path_buf()]
        } else {
            Vec::new()
        };
    }

    let mut paths = Vec::new();
    match fs::read_dir(root) {
        Ok(entries) => {
            for entry in entries {
                if context.is_stopped() {
                    break;
                }
                match entry {
                    Ok(entry) => {
                        let collected = collect_path(
                            &entry.path(),
                            root,
                            false,
                            matcher,
                            options,
                            issues,
                            context,
                        );
                        paths.extend(collected.paths);
                    }
                    Err(error) => issues.push(warning(
                        root,
                        spaceprune_plugin_api::messages::message(
                            "filesystem.read_entry",
                            &[("error", (error).to_string())],
                        ),
                        &error,
                    )),
                }
            }
        }
        Err(error) => issues.push(warning(
            root,
            spaceprune_plugin_api::messages::message(
                "filesystem.read_directory",
                &[("error", (error).to_string())],
            ),
            &error,
        )),
    }
    paths
}

pub(super) fn collect_path(
    path: &Path,
    root: &Path,
    inherited_include: bool,
    matcher: &RootMatcher,
    options: &Options,
    issues: &mut Vec<HandlerIssue>,
    context: &OperationContext,
) -> Collected {
    if spaceprune_access::filesystem::is_execution_artifact(path) {
        return Collected {
            paths: Vec::new(),
            contains_excluded: true,
        };
    }
    if context.is_stopped() {
        return Collected::default();
    }
    let relative = path.strip_prefix(root).unwrap_or(path);
    if matcher.excluded(relative) {
        return Collected {
            paths: Vec::new(),
            contains_excluded: true,
        };
    }
    let included = inherited_include || matcher.included(relative);
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            issues.push(warning(
                path,
                spaceprune_plugin_api::messages::message(
                    "filesystem.read_metadata",
                    &[("error", (error).to_string())],
                ),
                &error,
            ));
            return Collected {
                paths: Vec::new(),
                contains_excluded: included,
            };
        }
    };
    if !metadata.is_dir() {
        return Collected {
            paths: if included && options.files {
                vec![path.to_path_buf()]
            } else {
                Vec::new()
            },
            contains_excluded: false,
        };
    }
    if matcher.direct_children {
        return Collected {
            paths: if included && options.directories {
                vec![path.to_path_buf()]
            } else {
                Vec::new()
            },
            contains_excluded: false,
        };
    }
    if !included && !matcher.recursive {
        return Collected::default();
    }
    if !included && !matcher.may_contain_match(relative) {
        return Collected::default();
    }
    if included && options.directories && !matcher.has_excludes {
        return Collected {
            paths: vec![path.to_path_buf()],
            contains_excluded: false,
        };
    }

    let mut children = Vec::new();
    let mut contains_excluded = false;
    match fs::read_dir(path) {
        Ok(entries) => {
            for entry in entries {
                if context.is_stopped() {
                    break;
                }
                match entry {
                    Ok(entry) => {
                        let child = collect_path(
                            &entry.path(),
                            root,
                            included,
                            matcher,
                            options,
                            issues,
                            context,
                        );
                        contains_excluded |= child.contains_excluded;
                        children.extend(child.paths);
                    }
                    Err(error) => {
                        contains_excluded |= included;
                        issues.push(warning(
                            path,
                            spaceprune_plugin_api::messages::message(
                                "filesystem.read_entry",
                                &[("error", (error).to_string())],
                            ),
                            &error,
                        ));
                    }
                }
            }
        }
        Err(error) => {
            contains_excluded |= included;
            issues.push(warning(
                path,
                spaceprune_plugin_api::messages::message(
                    "filesystem.read_directory",
                    &[("error", (error).to_string())],
                ),
                &error,
            ));
        }
    }
    if included && options.directories && !contains_excluded {
        Collected {
            paths: vec![path.to_path_buf()],
            contains_excluded: false,
        }
    } else {
        Collected {
            paths: children,
            contains_excluded,
        }
    }
}

pub(super) fn append_item(
    result: &mut HandlerScanResult,
    path: &Path,
    root: ScanRoot<'_>,
    recommend: &Recommend,
    minimum: u64,
    older_than: Option<std::time::Duration>,
    context: &OperationContext,
) {
    use spaceprune_access::NativePath;
    let identity = spaceprune_access::filesystem::read_identity(path);
    let stats = match core_fs::path_stats_cancellable(path, context) {
        Ok(stats) => stats,
        Err(error) => {
            result.issues.push(warning(
                path,
                spaceprune_plugin_api::messages::message(
                    "filesystem.measure_failed",
                    &[("error", (error).to_string())],
                ),
                &error,
            ));
            return;
        }
    };
    let bytes = stats.logical_bytes;
    let modified = stats.latest_modified;
    let old_enough = older_than.is_none_or(|threshold| core_fs::is_older_than(modified, threshold));
    let large_enough = bytes >= minimum;
    let (recommendation, reason) = if recommend.enabled && old_enough && large_enough {
        (
            Recommendation::Recommended,
            spaceprune_plugin_api::messages::text("filesystem.matches_rules").to_owned(),
        )
    } else if !recommend.enabled {
        (
            Recommendation::Optional,
            spaceprune_plugin_api::messages::text("filesystem.recommendation_disabled").to_owned(),
        )
    } else if !old_enough {
        (
            Recommendation::Optional,
            spaceprune_plugin_api::messages::text("filesystem.recent").to_owned(),
        )
    } else {
        (
            Recommendation::Optional,
            spaceprune_plugin_api::messages::text("filesystem.below_size").to_owned(),
        )
    };
    let binding = (|| -> std::io::Result<_> {
        let identity = identity?;
        let metadata = fs::symlink_metadata(path)?;
        if spaceprune_access::filesystem::read_identity(path)? != identity {
            return Err(spaceprune_access::error::failure(
                spaceprune_access::IoFailureKind::TargetChanged,
                spaceprune_plugin_api::messages::text("filesystem.changed_during_measurement"),
            ));
        }
        let exact = !fs::symlink_metadata(root.path)?.is_dir();
        let anchor = if exact {
            root.path.parent().ok_or_else(|| {
                std::io::Error::other(spaceprune_plugin_api::messages::text(
                    "filesystem.exact_no_parent",
                ))
            })?
        } else {
            root.path
        };
        Ok((
            identity,
            spaceprune_access::filesystem::read_identity(anchor)?,
            exact,
            metadata.is_dir(),
        ))
    })();
    let (identity, root_identity, exact, directory) = match binding {
        Ok(binding) => binding,
        Err(error) => {
            result.issues.push(warning(
                path,
                spaceprune_plugin_api::messages::message(
                    "filesystem.identity_failed",
                    &[("error", (error).to_string())],
                ),
                &error,
            ));
            return;
        }
    };
    let location = path.to_string_lossy().into_owned();
    let key = if path.to_str().is_some() {
        location.clone()
    } else {
        serde_json::to_string(&NativePath::from_path(path)).expect("native path JSON")
    };
    result.items.push(CleanupItem {
        logical_size_applicability: Default::default(),
        id: key,
        parent_id: None,
        name: path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| location.clone()),
        location: location.clone(),
        logical_bytes: Some(bytes),
        reclaimable_bytes: Some(bytes),
        modified_at: modified,
        recommendation,
        reason,
        tags: vec![if directory { "directory" } else { "file" }.to_owned()],
        opaque: json!({"path": location, "native-path": NativePath::from_path(path), "root-index": root.index,
            "filesystem-identity": identity, "root-identity": root_identity, "exact-root": exact}),
    });
}

pub(super) fn warning(
    path: &Path,
    message: impl Into<String>,
    error: &std::io::Error,
) -> HandlerIssue {
    HandlerIssue {
        code: None,
        io_failure: Some(spaceprune_access::error::classify(error)),
        level: IssueLevel::Warning,
        message: message.into(),
        location: Some(path.to_string_lossy().into_owned()),
    }
}
