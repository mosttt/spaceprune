//! Portable name-based rules. Device grants and scan identities never enter Recipe options.

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use spaceprune_plugin_api::{PluginError, PluginResult, TaskRequest};

/// Reusable enumeration and matching policy for one locally bound resource slot.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct DocumentRules {
    /// Portable slot name; each installation explicitly binds it to a granted directory.
    pub resource: String,
    /// Visit descendants when true; false examines only direct children of the selected root.
    #[serde(default = "yes")]
    pub recursive: bool,
    /// Glob patterns matched against each file's display name, not a filesystem path.
    #[serde(default = "all")]
    pub include_names: Vec<String>,
    /// Name globs excluded everywhere; excluding a directory also excludes its descendants.
    #[serde(default)]
    pub exclude_names: Vec<String>,
}

/// Optional recommendation thresholds. Missing metadata never satisfies a threshold.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct DocumentRecommendation {
    /// Minimum age in whole days; zero imposes no age condition.
    pub older_than_days: u32,
    /// Minimum logical bytes; zero imposes no size condition.
    pub min_size_bytes: u64,
}

pub(crate) struct CompiledRules {
    pub rules: DocumentRules,
    pub recommend: DocumentRecommendation,
    include: GlobSet,
    exclude: GlobSet,
}

impl CompiledRules {
    pub fn parse(task: &TaskRequest) -> PluginResult<Self> {
        if task.handler_id != crate::HANDLER_ID {
            return Err(invalid(spaceprune_plugin_api::messages::text(
                "mobile_documents.wrong_handler",
            )));
        }
        let rules: DocumentRules = serde_json::from_value(task.options.clone())
            .map_err(|e| invalid(&format!("Invalid document rules: {e}")))?;
        let recommend = serde_json::from_value(task.recommend.clone())
            .map_err(|e| invalid(&format!("Invalid document recommendations: {e}")))?;
        if rules.resource.is_empty()
            || !rules
                .resource
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            || rules.include_names.is_empty()
        {
            return Err(invalid(spaceprune_plugin_api::messages::text(
                "mobile_documents.rule_required",
            )));
        }
        Ok(Self {
            include: globs(&rules.include_names)?,
            exclude: globs(&rules.exclude_names)?,
            rules,
            recommend,
        })
    }

    pub fn excluded(&self, name: &str) -> bool {
        self.exclude.is_match(name)
    }
    pub fn included(&self, name: &str) -> bool {
        self.include.is_match(name) && !self.excluded(name)
    }
    pub fn recommended(&self, document: &crate::Document, now: u64) -> bool {
        (self.recommend.min_size_bytes == 0
            || document
                .logical_bytes
                .is_some_and(|size| size >= self.recommend.min_size_bytes))
            && (self.recommend.older_than_days == 0
                || document.modified_at.is_some_and(|modified| {
                    now.checked_sub(modified).is_some_and(|age| {
                        age >= u64::from(self.recommend.older_than_days) * 86_400
                    })
                }))
    }

    pub fn reason(&self, document: &crate::Document, now: u64) -> String {
        use spaceprune_plugin_api::messages::{message, text};
        let age = if self.recommend.older_than_days == 0 {
            text("mobile_documents.age_unrestricted").to_owned()
        } else {
            message(
                "mobile_documents.age_condition",
                &[
                    ("days", self.recommend.older_than_days.to_string()),
                    (
                        "actual",
                        document
                            .modified_at
                            .and_then(|modified| now.checked_sub(modified))
                            .map(|age| (age / 86_400).to_string())
                            .unwrap_or_else(|| text("mobile_documents.unknown").into()),
                    ),
                ],
            )
        };
        let size = if self.recommend.min_size_bytes == 0 {
            text("mobile_documents.size_unrestricted").to_owned()
        } else {
            message(
                "mobile_documents.size_condition",
                &[
                    ("bytes", self.recommend.min_size_bytes.to_string()),
                    (
                        "actual",
                        document
                            .logical_bytes
                            .map(|bytes| bytes.to_string())
                            .unwrap_or_else(|| text("mobile_documents.unknown").into()),
                    ),
                ],
            )
        };
        message(
            "mobile_documents.rule_reason",
            &[
                ("patterns", self.rules.include_names.join(", ")),
                ("age", age),
                ("size", size),
                (
                    "result",
                    text(if self.recommended(document, now) {
                        "mobile_documents.matches"
                    } else {
                        "mobile_documents.optional"
                    })
                    .into(),
                ),
            ],
        )
    }
}

fn globs(patterns: &[String]) -> PluginResult<GlobSet> {
    let mut set = GlobSetBuilder::new();
    for pattern in patterns {
        if pattern.is_empty() || pattern.contains(['/', '\\', '\0']) {
            return Err(invalid(spaceprune_plugin_api::messages::text(
                "mobile_documents.name_only",
            )));
        }
        set.add(
            GlobBuilder::new(pattern)
                .literal_separator(true)
                .backslash_escape(false)
                .build()
                .map_err(|e| invalid(&format!("Invalid document name pattern: {e}")))?,
        );
    }
    set.build().map_err(|e| invalid(&e.to_string()))
}
fn invalid(message: &str) -> PluginError {
    PluginError::new("mobile-documents-rules-invalid", message)
}
fn yes() -> bool {
    true
}
fn all() -> Vec<String> {
    vec!["*".into()]
}
