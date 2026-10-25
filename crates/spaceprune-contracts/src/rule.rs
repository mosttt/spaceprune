//! Persistent rule intent and root location, separate from observations and execution plans.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether a recipe is a recurring rule or an explicit single-use selection.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecipeUsage {
    /// Re-enumerate the declared scope on every scan; eligible for scheduled cleanup.
    #[default]
    Recurring,
    /// Keep exact observed bindings; never eligible for recurring automation.
    OneShot,
}

/// The stable enumeration scope of a task, not its default recommendation policy.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuleScope {
    /// Enumerate immediate children; selected directories retain dynamic subtree semantics.
    DirectoryChildren,
    /// Enumerate within a bounded directory using the declared include/exclude predicates.
    #[default]
    DirectorySubtree,
    /// One exact observed file or link; only valid for one-shot recipes.
    ExactFile,
    /// Re-enumerate objects belonging to one registered system configuration.
    SystemConfiguration,
    /// One identity-bound domain object or indivisible group; only valid for one-shot recipes.
    ExactSystemObject,
}

/// A persistent location expression. It never stores a session's opaque evidence identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum RootSpec {
    /// An explicit configuration path or variable expression, resolved by the existing resolver.
    Expression(String),
    /// A validated relative location under the current user's home on the task's platform.
    UserHome {
        /// Normal relative components, without separators, traversal, or empty components.
        user_home: Vec<String>,
    },
    /// An observed absolute location bound to its originating machine.
    LocalAbsolute {
        /// Exact local absolute path; never guessed from a drive letter or similar filename.
        local_absolute: String,
        /// Originating machine identity. Another machine must explicitly rebind the rule.
        host_id: String,
    },
}

/// Stable native identity data; obtaining or checking it belongs to the access implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    /// Filesystem device or Windows volume identifier.
    #[serde(with = "hex_identity")]
    #[schemars(with = "String", regex(pattern = "^[0-9a-f]{16}$"))]
    pub device: u64,
    /// Unix inode or the lower 64 bits of a Windows file ID.
    #[serde(with = "hex_identity")]
    #[schemars(with = "String", regex(pattern = "^[0-9a-f]{16}$"))]
    pub inode: u64,
    /// Upper 64 bits of a Windows file ID; zero on Unix.
    #[serde(with = "hex_identity")]
    #[schemars(with = "String", regex(pattern = "^[0-9a-f]{16}$"))]
    pub high: u64,
    /// Stable creation marker when available.
    pub birth: Option<u64>,
}

/// A one-shot selection which must not silently bind a replacement or another device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectBinding {
    /// Exact relative selection under the configured root, or empty for an exact file root.
    pub relative: Vec<String>,
    /// Originating machine identity.
    pub host_id: String,
    /// Identity captured by the trusted host from an actual observation.
    pub identity: FileIdentity,
}

/// Typed built-in filesystem root declaration. Third-party handler options remain extensible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FilesystemRoot {
    /// Stable location expression.
    pub path: RootSpec,
    /// Enumeration or exact selection semantics.
    #[serde(default)]
    #[schemars(schema_with = "crate::schema::filesystem_scope_schema")]
    pub scope: RuleScope,
    /// Include predicates relative to this root.
    #[serde(default = "default_include")]
    pub include: Vec<String>,
    /// Hard protections, independent of default recommendation thresholds.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Required object bindings for saved one-shot selections; absent for recurring rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<ObjectBinding>,
}

fn default_include() -> Vec<String> {
    vec!["*".to_owned()]
}

// Identifiers are hex strings so Windows volume/file IDs round-trip through TOML's signed integers.
mod hex_identity {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{value:016x}"))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.len() != 16
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(D::Error::custom(
                "native identity must be 16 lowercase hexadecimal digits",
            ));
        }
        u64::from_str_radix(&value, 16).map_err(D::Error::custom)
    }
}
