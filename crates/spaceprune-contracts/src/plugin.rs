//! Public plugin identities and metadata; discovery, validation and execution remain in the host.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Reserved identity of the bundled filesystem capability.
pub const FILESYSTEM_PLUGIN_ID: &str = "builtin.filesystem";
/// Reserved identity of the bundled snapshot capability.
pub const SNAPPER_PLUGIN_ID: &str = "builtin.snapper";
/// Reserved identity of the bundled Linux system capability.
pub const SYSTEM_LINUX_PLUGIN_ID: &str = "builtin.system-linux";
/// Reserved identity of the bundled developer-tool lifecycle capability.
pub const DEVELOPER_TOOLS_PLUGIN_ID: &str = "builtin.developer-tools";
/// Reserved identity of user-authorized native document access.
pub const MOBILE_DOCUMENTS_PLUGIN_ID: &str = "builtin.mobile-documents";
/// IDs owned by the application, including native mobile document access.
pub const BUILTIN_PLUGIN_IDS: [&str; 5] = [
    FILESYSTEM_PLUGIN_ID,
    SNAPPER_PLUGIN_ID,
    SYSTEM_LINUX_PLUGIN_ID,
    DEVELOPER_TOOLS_PLUGIN_ID,
    MOBILE_DOCUMENTS_PLUGIN_ID,
];

/// Whether an ID belongs exclusively to an application-provided capability.
pub fn is_builtin_plugin_id(id: &str) -> bool {
    BUILTIN_PLUGIN_IDS.contains(&id)
}

/// An external plugin manifest, with its existing TOML/JSON field names.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    /// Manifest format version.
    #[serde(rename = "schema-version")]
    pub schema_version: u32,
    /// Globally unique plugin identifier.
    pub id: String,
    /// Human-readable plugin name.
    pub name: String,
    /// Semantic plugin version.
    pub version: String,
    /// JSON Lines protocol version.
    #[serde(rename = "protocol-version")]
    pub protocol_version: u32,
    /// Executable filename or path relative to the manifest directory.
    pub executable: String,
    /// Supported platforms; an empty list means platform-neutral.
    /// Manifest input may omit this field, but serialized metadata always includes the array.
    #[serde(default)]
    pub platforms: Vec<String>,
    /// Local handler identifiers exported by the plugin.
    pub handlers: Vec<String>,
}

/// Discovered plugin metadata serialized by CLI and GUI hosts.
#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
pub struct InstalledPlugin {
    /// Plugin manifest.
    pub manifest: PluginManifest,
    /// Absolute path to `plugin.toml`.
    pub manifest_path: PathBuf,
    /// Executable path used by the host.
    pub executable: PathBuf,
}

/// Installation origin; built-in capabilities cannot be removed separately from the application.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    /// Shipped with the application.
    Builtin,
    /// Installed as an external process plugin.
    External,
}

/// A read-only catalog entry; enumerating it never starts a plugin process.
#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
pub struct PluginCatalogEntry {
    /// Stable plugin ID used by recipe handler references.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Capability description, not model instructions.
    pub description: String,
    /// Installed semantic version.
    pub version: String,
    /// Built-in or separately installed origin.
    pub kind: PluginKind,
    /// Supported platforms; an empty list means platform-neutral.
    pub platforms: Vec<String>,
    /// Whether the plugin declares compatibility with the current platform.
    pub supported: bool,
    /// Whether an external installation uses an ID reserved for a built-in capability.
    pub conflicts_with_builtin: bool,
    /// Device-local preference; conflicting external installations are always disabled.
    pub enabled: bool,
    /// Complete handler IDs, including the plugin prefix.
    pub handlers: Vec<String>,
    /// External installation details; absent for built-in capabilities.
    pub installation: Option<InstalledPlugin>,
}
