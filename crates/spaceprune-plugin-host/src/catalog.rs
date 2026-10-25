//! Read-only plugin discovery, path precedence, and the metadata catalog.

use crate::{
    HostError, InstalledPlugin, MANIFEST_FILE, manifest::load_manifest, paths::plugin_search_paths,
};
use spaceprune_config::current_platform;
use spaceprune_plugin_api::OperationContext;
use std::{collections::BTreeMap, fs, path::PathBuf};

/// 外部插件发现和注册入口。
#[derive(Debug, Default)]
pub struct PluginHost {
    pub(super) plugins: BTreeMap<String, InstalledPlugin>,
}

impl PluginHost {
    /// 从当前平台的标准插件目录发现可用插件。
    pub fn discover() -> Result<Self, HostError> {
        Self::discover_in(&plugin_search_paths())
    }

    /// Discover native plugin metadata while respecting the caller's operation.
    pub fn discover_in_context(context: &OperationContext) -> Result<Self, HostError> {
        Self::discover_paths_in_context(&plugin_search_paths(), context)
    }

    /// 从指定目录集合发现插件，靠前目录优先。
    pub fn discover_in(paths: &[PathBuf]) -> Result<Self, HostError> {
        Self::discover_paths_in_context(paths, &OperationContext::default())
    }

    /// Discover explicit plugin roots under one caller-owned context.
    pub fn discover_paths_in_context(
        paths: &[PathBuf],
        context: &OperationContext,
    ) -> Result<Self, HostError> {
        Self::discover_metadata_paths(paths, context, true)
    }

    pub(super) fn discover_metadata_paths(
        paths: &[PathBuf],
        context: &OperationContext,
        current_platform_only: bool,
    ) -> Result<Self, HostError> {
        let mut plugins = BTreeMap::new();
        for directory in paths {
            context.check().map_err(|source| HostError::Io {
                path: directory.clone(),
                source,
            })?;
            let entries = match fs::read_dir(directory) {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(source) => {
                    return Err(HostError::Io {
                        path: directory.clone(),
                        source,
                    });
                }
            };
            let mut entries =
                entries
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|source| HostError::Io {
                        path: directory.clone(),
                        source,
                    })?;
            entries.sort_by_key(fs::DirEntry::file_name);
            for entry in entries {
                context.check().map_err(|source| HostError::Io {
                    path: directory.clone(),
                    source,
                })?;
                let file_type = entry.file_type().map_err(|source| HostError::Io {
                    path: entry.path(),
                    source,
                })?;
                if file_type.is_symlink() {
                    continue;
                }
                if !file_type.is_dir() {
                    continue;
                }
                let manifest_path = entry.path().join(MANIFEST_FILE);
                if !manifest_path.is_file() {
                    continue;
                }
                let installed = load_manifest(&manifest_path)?;
                if !current_platform_only
                    || installed.manifest.platforms.is_empty()
                    || installed
                        .manifest
                        .platforms
                        .iter()
                        .any(|platform| platform == current_platform())
                {
                    plugins
                        .entry(installed.manifest.id.clone())
                        .or_insert(installed);
                }
            }
        }
        Ok(Self { plugins })
    }

    /// 返回所有已发现插件。
    pub fn plugins(&self) -> Vec<&InstalledPlugin> {
        self.plugins.values().collect()
    }

    /// 查找一个已发现插件。
    pub fn get(&self, id: &str) -> Option<&InstalledPlugin> {
        self.plugins.get(id)
    }
}

/// Read every installed manifest, including plugins unsupported on the current platform.
/// Unlike an executable host registry, this returns metadata only and never starts processes.
pub fn installed_plugin_metadata() -> Result<Vec<InstalledPlugin>, HostError> {
    PluginHost::discover_metadata_paths(&plugin_search_paths(), &OperationContext::default(), false)
        .map(|host| host.plugins.into_values().collect())
}
