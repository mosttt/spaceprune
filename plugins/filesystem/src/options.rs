//! Filesystem options responsibilities.

use super::*;

pub(super) fn resolve_root(value: &str) -> PluginResult<PathBuf> {
    let path = PathBuf::from(value);
    std::path::absolute(path).map_err(|error| {
        PluginError::new(
            "current-directory-unavailable",
            spaceprune_plugin_api::messages::message(
                "filesystem.resolve_root",
                &[
                    ("value", (value).to_string()),
                    ("error", (error).to_string()),
                ],
            ),
        )
    })
}

pub(super) fn parse_options(task: &TaskRequest) -> PluginResult<Options> {
    serde_json::from_value(task.options.clone()).map_err(|error| {
        PluginError::new(
            "invalid-options",
            spaceprune_plugin_api::messages::message(
                "filesystem.invalid_options",
                &[
                    ("p0", (task.task_id).to_string()),
                    ("error", (error).to_string()),
                ],
            ),
        )
    })
}

pub(super) fn parse_recommend(task: &TaskRequest) -> PluginResult<Recommend> {
    let value: Recommend = serde_json::from_value(task.recommend.clone()).map_err(|error| {
        PluginError::new(
            "invalid-recommend",
            spaceprune_plugin_api::messages::message(
                "filesystem.invalid_recommend",
                &[
                    ("p0", (task.task_id).to_string()),
                    ("error", (error).to_string()),
                ],
            ),
        )
    })?;
    if let Some(older_than) = &value.older_than {
        core_fs::parse_duration(older_than)?;
    }
    if let Some(size) = &value.min_size {
        core_fs::parse_bytes(size)?;
    }
    Ok(value)
}

pub(super) fn validate_root(root: &RootOptions) -> PluginResult<()> {
    use spaceprune_contracts::rule::RuleScope;
    if !matches!(
        root.scope,
        RuleScope::DirectoryChildren | RuleScope::DirectorySubtree | RuleScope::ExactFile
    ) {
        return Err(PluginError::new(
            "invalid-filesystem-scope",
            spaceprune_plugin_api::messages::text("filesystem.root_scope"),
        ));
    }
    if root.scope == RuleScope::DirectoryChildren
        && root
            .include
            .iter()
            .any(|pattern| pattern.contains('/') || pattern.contains("**"))
    {
        return Err(PluginError::new(
            "invalid-filesystem-scope",
            spaceprune_plugin_api::messages::text("filesystem.children_include"),
        ));
    }
    if root.path.trim().is_empty() {
        return Err(PluginError::new(
            "empty-root-path",
            spaceprune_plugin_api::messages::text("filesystem.empty_root"),
        ));
    }
    if root.include.is_empty() {
        return Err(PluginError::new(
            "empty-include",
            spaceprune_plugin_api::messages::message(
                "filesystem.empty_include",
                &[("p0", (root.path).to_string())],
            ),
        ));
    }
    let _ = RootMatcher::compile(root)?;
    Ok(())
}

pub(super) fn validate_persistent_bindings(root: &RootOptions, path: &Path) -> std::io::Result<()> {
    use spaceprune_access::{IoFailureKind, error::failure, filesystem::read_identity};
    use spaceprune_contracts::rule::RuleScope;
    if root.scope == RuleScope::ExactFile && root.bindings.is_empty() {
        return Err(failure(
            IoFailureKind::TargetChanged,
            spaceprune_plugin_api::messages::text("filesystem.rebind_identity"),
        ));
    }
    for binding in &root.bindings {
        if binding.host_id != spaceprune_access::host::identity()? {
            return Err(failure(
                IoFailureKind::TargetChanged,
                spaceprune_plugin_api::messages::text("filesystem.rebind_host"),
            ));
        }
        let mut target = path.to_path_buf();
        for part in &binding.relative {
            if part.is_empty()
                || part.contains(['/', '\\'])
                || !matches!(
                    Path::new(part).components().next(),
                    Some(std::path::Component::Normal(_))
                )
            {
                return Err(failure(
                    IoFailureKind::BoundaryViolation,
                    spaceprune_plugin_api::messages::text("filesystem.invalid_component"),
                ));
            }
            target.push(part);
        }
        let metadata = std::fs::symlink_metadata(&target)?;
        if (root.scope == RuleScope::ExactFile) == metadata.is_dir() {
            return Err(failure(
                IoFailureKind::BoundaryViolation,
                spaceprune_plugin_api::messages::text("filesystem.scope_type"),
            ));
        }
        if root.scope == RuleScope::DirectoryChildren && binding.relative.len() > 1 {
            return Err(failure(
                IoFailureKind::BoundaryViolation,
                spaceprune_plugin_api::messages::text("filesystem.children_binding"),
            ));
        }
        if read_identity(&target)? != binding.identity {
            return Err(failure(
                IoFailureKind::TargetChanged,
                spaceprune_plugin_api::messages::text("filesystem.rebind_replaced"),
            ));
        }
    }
    Ok(())
}

pub(super) fn default_include() -> Vec<String> {
    vec!["*".to_owned()]
}

pub(super) const fn default_true() -> bool {
    true
}

pub(super) fn default_elevation() -> String {
    "auto".to_owned()
}
