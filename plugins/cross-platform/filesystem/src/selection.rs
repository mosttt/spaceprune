//! Filesystem selection responsibilities.

use super::*;

impl RootMatcher {
    pub(super) fn compile(root: &RootOptions) -> PluginResult<Self> {
        Ok(Self {
            direct_children: root.scope == spaceprune_contracts::rule::RuleScope::DirectoryChildren,
            include: compile_globs(&root.include, "include")?,
            exclude: compile_globs(&root.exclude, "exclude")?,
            has_excludes: !root.exclude.is_empty(),
            recursive: root
                .include
                .iter()
                .chain(&root.exclude)
                .any(|pattern| pattern.contains('/')),
            ancestor_globs: compile_ancestor_globs(&root.include)?,
        })
    }

    pub(super) fn included(&self, relative: &Path) -> bool {
        glob_matches(&self.include, relative)
    }

    pub(super) fn excluded(&self, relative: &Path) -> bool {
        self.has_excludes && glob_matches(&self.exclude, relative)
    }

    pub(super) fn included_by_rule(&self, relative: &Path) -> bool {
        relative
            .ancestors()
            .filter(|path| !path.as_os_str().is_empty())
            .any(|path| self.included(path))
    }

    pub(super) fn excluded_by_rule(&self, relative: &Path) -> bool {
        self.has_excludes
            && relative
                .ancestors()
                .filter(|path| !path.as_os_str().is_empty())
                .any(|path| self.excluded(path))
    }

    pub(super) fn may_contain_match(&self, relative: &Path) -> bool {
        glob_matches(&self.ancestor_globs, relative)
    }
}

pub(super) fn compile_globs(patterns: &[String], kind: &str) -> PluginResult<GlobSet> {
    let mut set = GlobSetBuilder::new();
    for pattern in patterns {
        let mut builder = GlobBuilder::new(pattern);
        builder.literal_separator(true).backslash_escape(false);
        let glob = builder.build().map_err(|error| {
            PluginError::new(
                "invalid-glob",
                spaceprune_plugin_api::messages::message(
                    "filesystem.invalid_glob",
                    &[
                        ("kind", (kind).to_string()),
                        ("pattern", (pattern).to_string()),
                        ("error", (error).to_string()),
                    ],
                ),
            )
        })?;
        set.add(glob);
    }
    set.build().map_err(|error| {
        PluginError::new(
            "invalid-glob-set",
            spaceprune_plugin_api::messages::message(
                "filesystem.compile_rules",
                &[("kind", (kind).to_string()), ("error", (error).to_string())],
            ),
        )
    })
}

fn compile_ancestor_globs(patterns: &[String]) -> PluginResult<GlobSet> {
    let mut ancestors = BTreeSet::new();
    for pattern in patterns {
        let mut nested = 0_usize;
        for (index, character) in pattern.char_indices() {
            match character {
                '{' | '[' => nested += 1,
                '}' | ']' => nested = nested.saturating_sub(1),
                '/' if nested > 0 => {
                    // Alternatives containing separators cannot be split into parent globs.
                    ancestors.insert("**".to_owned());
                    break;
                }
                '/' if index > 0 => {
                    ancestors.insert(pattern[..index].to_owned());
                }
                _ => {}
            }
        }
    }
    compile_globs(
        &ancestors.into_iter().collect::<Vec<_>>(),
        spaceprune_plugin_api::messages::text("filesystem.include_ancestor"),
    )
}

pub(super) fn glob_matches(set: &GlobSet, relative: &Path) -> bool {
    set.is_match(relative) || set.is_match(relative.join(""))
}
