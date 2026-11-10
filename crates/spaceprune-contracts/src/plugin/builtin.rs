//! Built-in capability declarations shared by read-only catalogs and native hosts.
use super::*;

/// Immutable metadata for a bundled plugin; this never discovers or executes code.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct BuiltinPluginSpec {
    /// Stable plugin identity.
    pub id: &'static str,
    /// Bundled version, matching the Core workspace release.
    pub version: &'static str,
    /// Supported systems; empty means platform-neutral.
    pub platforms: &'static [&'static str],
    handlers: &'static [&'static str],
}
impl BuiltinPluginSpec {
    /// Whether this declaration supports the selected system.
    pub fn supports(&self, platform: &str) -> bool {
        self.platforms.is_empty() || self.platforms.contains(&platform)
    }
    /// Fully qualified handler IDs for the selected system.
    /// Call `supports` separately before treating these as available capabilities.
    pub fn handler_ids(&self, platform: &str) -> Vec<String> {
        self.handlers
            .iter()
            .filter(|name| {
                self.id != DEVELOPER_TOOLS_PLUGIN_ID
                    || platform == "linux"
                    || name.starts_with("docker-")
            })
            .map(|name| format!("{}/{name}", self.id))
            .collect()
    }
}
const fn spec(
    id: &'static str,
    platforms: &'static [&'static str],
    handlers: &'static [&'static str],
) -> BuiltinPluginSpec {
    BuiltinPluginSpec {
        id,
        version: env!("CARGO_PKG_VERSION"),
        platforms,
        handlers,
    }
}
/// Current bundled declarations. Host policy and authorization remain separate.
pub const BUILTIN_PLUGIN_SPECS: [BuiltinPluginSpec; 7] = [
    spec(FILESYSTEM_PLUGIN_ID, &[], &["paths"]),
    spec(SNAPPER_PLUGIN_ID, &["linux"], &["snapshots"]),
    spec(
        SYSTEM_LINUX_PLUGIN_ID,
        &["linux"],
        &[
            "package-cache",
            "journal",
            "tmpfiles",
            "coredumps",
            "go-modcache",
        ],
    ),
    spec(
        DEVELOPER_TOOLS_PLUGIN_ID,
        &["linux", "windows", "macos"],
        &[
            "go-modcache",
            "go-buildcache",
            "cargo-registry-cache",
            "npm-cache",
            "docker-build-cache",
            "docker-dangling-images",
            "docker-unused-images",
            "docker-stopped-containers",
        ],
    ),
    spec(MOBILE_DOCUMENTS_PLUGIN_ID, &["android"], &["documents"]),
    spec(
        MEMORY_PLUGIN_ID,
        &["windows", "linux"],
        &["process-working-set"],
    ),
    spec(
        SYSTEM_WINDOWS_PLUGIN_ID,
        &["windows"],
        &["recycle-bin", "component-store"],
    ),
];
/// Look up a bundled declaration without loading a plugin or applying local preferences.
pub fn builtin_plugin_spec(id: &str) -> Option<&'static BuiltinPluginSpec> {
    BUILTIN_PLUGIN_SPECS.iter().find(|spec| spec.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builtin_declarations_keep_platform_boundaries_and_real_handler_names() {
        assert_eq!(
            BUILTIN_PLUGIN_IDS
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>(),
            BUILTIN_PLUGIN_SPECS.iter().map(|p| p.id).collect()
        );
        let developer = builtin_plugin_spec(DEVELOPER_TOOLS_PLUGIN_ID).unwrap();
        assert_eq!(developer.handler_ids("linux").len(), 8);
        assert_eq!(developer.handler_ids("windows").len(), 4);
        assert!(
            developer
                .handler_ids("macos")
                .iter()
                .all(|id| id.starts_with("builtin.developer-tools/docker-"))
        );
        assert!(!developer.supports("android"));
        assert_eq!(
            builtin_plugin_spec(FILESYSTEM_PLUGIN_ID)
                .unwrap()
                .handler_ids("linux"),
            ["builtin.filesystem/paths"]
        );
        assert!(
            !builtin_plugin_spec(MEMORY_PLUGIN_ID)
                .unwrap()
                .supports("macos")
        );
        assert!(
            !builtin_plugin_spec(SNAPPER_PLUGIN_ID)
                .unwrap()
                .supports("windows")
        );
    }
}
