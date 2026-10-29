//! Typed native resource access. Identifiers are opaque and never decoded as filesystem paths.

use serde::{Deserialize, Serialize};
use spaceprune_plugin_api::{OperationContext, PluginResult};

/// Maximum entries in one bridge response; native adapters page larger directories.
pub const MAX_PAGE_ENTRIES: usize = 256;

/// A device-local grant snapshot. URI/bookmark storage belongs to the native host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceBinding {
    /// Stable Recipe resource slot, resolved only in this installation's private binding store.
    pub slot: String,
    /// Fresh opaque revision on every rebind or replacement grant; persist it across app restarts.
    pub revision: String,
    /// Provider-owned directory identity, not a path or a synchronized permission.
    pub root_id: String,
    /// Human-readable directory label, never used to address native resources.
    pub name: String,
    /// Whether current OS authorization permits metadata enumeration.
    pub readable: bool,
    /// Whether the current grant permits deletion, still subject to per-document capabilities.
    pub writable: bool,
}

/// Resource kinds with deliberately narrow first-version deletion semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    /// A regular document whose exact identity may be selected for deletion.
    File,
    /// A grouping/enumeration resource; never deleted by this plugin.
    Directory,
    /// Virtual or other documents that the native adapter cannot safely delete as regular files.
    Unsupported,
}

/// Metadata observed without downloading document contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    /// Provider-owned stable document identity, scoped to the supplied binding.
    pub id: String,
    /// The immediate enumerated parent; native adapters verify tree membership on every operation.
    pub parent_id: String,
    /// Display name used by name selectors, never converted to a native execution path.
    pub name: String,
    /// Native document category.
    pub kind: DocumentKind,
    /// Logical content size. Missing provider metadata is None; an actual zero is Some(0).
    pub logical_bytes: Option<u64>,
    /// Unix seconds, normalized by the adapter; unavailable/unspecified timestamps are None.
    pub modified_at: Option<u64>,
    /// Native per-document delete capability; a writable grant alone is insufficient.
    pub deletable: bool,
    /// An optional provider version marker, if one is genuinely available.
    pub version: Option<String>,
}

/// One bounded metadata batch for a single parent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentPage {
    /// At most MAX_PAGE_ENTRIES direct children of the requested parent.
    pub documents: Vec<Document>,
    /// Opaque next-page cursor for this parent; None marks complete enumeration.
    pub next: Option<String>,
}

/// Native deletion receipt. Uncertainty is distinct from a known refusal before any effects.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeleteResult {
    /// The selected document was deleted and the provider confirmed the outcome.
    Deleted,
    /// The same document identity no longer exists; no additional content was processed.
    AlreadyAbsent,
    /// The adapter confirmed that no deletion occurred.
    Failed {
        /// Safe diagnostic, without credentials or raw grant data.
        message: String,
    },
    /// Dispatch occurred, but its effects could not be confirmed. Never retry automatically.
    Uncertain {
        /// Safe diagnostic explaining why confirmation was unavailable.
        message: String,
    },
    /// Cancellation prevented dispatch. The adapter must also stop the supplied context.
    NotAttempted {
        /// Safe cancellation/timeout explanation.
        message: String,
    },
}

/// Native document service injected by the mobile host, not a model or external process plugin.
///
/// Every method must check the current OS grant and binding revision, propagate the operation's
/// cancellation/deadline, and reject resources outside the bound tree. Identifiers are not paths.
/// Call these synchronous methods from a background worker, never the UI/activity thread.
/// A directory picker and persisted URI permissions remain host responsibilities.
/// Native adapters must propagate cancellation to native work and confirm it has stopped; dropping
/// a bridge receiver alone is not cancellation. A lost deletion receipt remains Uncertain.
pub trait DocumentProvider: Send + Sync {
    /// Resolve a Recipe slot using this installation's local binding store and current OS grants.
    fn binding(&self, slot: &str, context: &OperationContext) -> PluginResult<ResourceBinding>;

    /// List direct children using only native metadata APIs; never fetch content to obtain size.
    fn children(
        &self,
        binding: &ResourceBinding,
        parent_id: &str,
        cursor: Option<&str>,
        context: &OperationContext,
    ) -> PluginResult<DocumentPage>;

    /// Re-read a document under the exact binding; None means confirmed absence, not access denial.
    fn inspect(
        &self,
        binding: &ResourceBinding,
        document_id: &str,
        context: &OperationContext,
    ) -> PluginResult<Option<Document>>;

    /// Delete only this regular file after rechecking the binding, tree membership, capabilities,
    /// and the expected metadata immediately before dispatch. Never delete a directory or root.
    /// The plugin has already checked selection/rules, but this final native check closes bridge
    /// delays. SAF does not provide an atomic compare-and-delete across arbitrary providers.
    /// An error after dispatch with unconfirmed effects must return Uncertain, not Failed.
    /// `ancestors` runs from the root's child to the immediate parent: recheck identities,
    /// names and relationships, not mutable directory sizes or timestamps.
    fn delete_file(
        &self,
        binding: &ResourceBinding,
        expected: &Document,
        ancestors: &[Document],
        context: &OperationContext,
    ) -> DeleteResult;
}
