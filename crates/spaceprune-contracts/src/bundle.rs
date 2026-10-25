//! Source-file bundles and portable relative-path validation.

use crate::messages;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

/// 可同步的完整 Recipe bundle。
///
/// `files` 的键统一使用 `/` 分隔的 bundle 内相对路径，值为 UTF-8 文件内容。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecipeBundle {
    /// Recipe 稳定 ID，同时决定入口文件名 `<id>.toml`。
    pub id: String,
    /// bundle 内全部文件。
    pub files: BTreeMap<String, String>,
}

/// A bundle path cannot be represented unambiguously in the portable namespace.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BundlePathError {
    /// A single path contains a forbidden component or separator.
    #[error("`{path}`: {reason}")]
    InvalidPath {
        /// The rejected relative path.
        path: String,
        /// The violated portable-path rule.
        reason: &'static str,
    },
    /// Two entries or directory components differ only in ASCII case.
    #[error("{}", messages::message("config.bundle_case_conflict", &[("path", .path.to_string()), ("existing", .existing.to_string())]))]
    CaseConflict {
        /// The incoming relative path or directory prefix.
        path: String,
        /// The previously registered spelling.
        existing: String,
    },
    /// An entry also serves as a directory, including after case folding.
    #[error("{}", messages::message("config.bundle_file_directory", &[("path", .path.to_string())]))]
    FileDirectoryConflict {
        /// The conflicting path or prefix.
        path: String,
    },
}

/// Validate one `/`-separated relative path without accessing a filesystem.
///
/// Host-specific resource limits are applied separately by the caller.
pub fn validate_bundle_path(path: &str) -> Result<(), BundlePathError> {
    let invalid = |reason| BundlePathError::InvalidPath {
        path: path.to_owned(),
        reason,
    };
    if path.is_empty()
        || path.contains(['\\', ':', '<', '>', '"', '|', '?', '*', '\0'])
        || path.chars().any(char::is_control)
    {
        return Err(invalid(messages::text("config.bundle_portable_path")));
    }
    for part in path.split('/') {
        if part.is_empty() || matches!(part, "." | "..") {
            return Err(invalid(messages::text("config.bundle_relative_path")));
        }
        if part.ends_with(['.', ' ']) || windows_reserved_component(part) {
            return Err(invalid(messages::text("config.bundle_windows_component")));
        }
    }
    Ok(())
}

/// Validate the whole file/directory namespace, independently of input order.
///
/// File names and every directory prefix share one ASCII-case-folded namespace.
/// This detects `a.toml` / `A.toml/x.toml` as well as aliased directories such as
/// `Parts/a.toml` / `parts/b.toml`. Native filesystem identity checks remain the
/// responsibility of the store; this is not a filesystem authorization check.
pub fn validate_bundle_paths<'a>(
    paths: impl IntoIterator<Item = &'a str>,
) -> Result<(), BundlePathError> {
    let mut entries: BTreeMap<String, (&str, bool)> = BTreeMap::new();
    for path in paths {
        validate_bundle_path(path)?;
        let prefixes = path
            .match_indices('/')
            .map(|(end, _)| (&path[..end], false));
        for (prefix, is_file) in prefixes.chain(std::iter::once((path, true))) {
            let key = prefix.to_ascii_lowercase();
            if let Some(&(existing, existing_is_file)) = entries.get(&key) {
                if is_file != existing_is_file {
                    return Err(BundlePathError::FileDirectoryConflict {
                        path: prefix.to_owned(),
                    });
                }
                if prefix != existing || is_file {
                    return Err(BundlePathError::CaseConflict {
                        path: prefix.to_owned(),
                        existing: existing.to_owned(),
                    });
                }
            } else {
                entries.insert(key, (prefix, is_file));
            }
        }
    }
    Ok(())
}

fn windows_reserved_component(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_conflicting_namespaces_in_either_order() {
        for paths in [
            ["a.toml", "A.toml/x.toml"],
            ["parts/A.toml", "parts/a.toml"],
            ["Parts/a.toml", "parts/b.toml"],
            ["parts", "parts/nested/x.toml"],
            ["same.toml", "same.toml"],
        ] {
            assert!(validate_bundle_paths(paths).is_err(), "{paths:?}");
            assert!(
                validate_bundle_paths(paths.into_iter().rev()).is_err(),
                "{paths:?}"
            );
        }
        assert!(matches!(
            validate_bundle_paths(["a.toml", "A.toml/x.toml"]),
            Err(BundlePathError::FileDirectoryConflict { .. })
        ));
    }

    #[test]
    fn rejects_nonportable_components() {
        for path in [
            "",
            "/absolute",
            "../outside",
            "parts/../outside",
            "./file",
            "parts//file",
            "parts/",
            "C:/file",
            "parts\\file",
            "NUL.toml",
            "dir/COM1.txt",
            "LPT².txt",
            "trailing./file",
            "trailing /file",
            "line\nfeed",
            "nul\0byte",
            "wild*.toml",
        ] {
            assert!(validate_bundle_path(path).is_err(), "{path:?}");
        }
    }

    #[test]
    fn preserves_multifile_platform_sources_and_unicode() {
        let original = r##"{"id":"work","files":{"work.toml":"# comment\nschema-version = 2\nname = 'Work'\ninclude = [{ path = 'platforms/windows.toml', platforms = ['windows'] }]\n","platforms/windows.toml":"schema-version = 2\nname = 'Work'\n","说明.txt":"保留\n"}}"##;
        let bundle: RecipeBundle = serde_json::from_str(original).unwrap();
        validate_bundle_paths(bundle.files.keys().map(String::as_str)).unwrap();
        let restored: RecipeBundle =
            serde_json::from_str(&serde_json::to_string(&bundle).unwrap()).unwrap();
        assert_eq!(restored, bundle);
        assert!(restored.files["work.toml"].starts_with("# comment\n"));
        assert!(
            serde_json::from_str::<RecipeBundle>(r#"{"id":"work","files":{},"grant":true}"#)
                .is_err()
        );
    }
}
