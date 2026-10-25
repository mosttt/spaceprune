//! Semantic validation of resolved modules, tasks, selectors, and automations.

use crate::{
    ConfigError, MANAGEMENT_COMMAND_IDS, RESERVED_SELECTOR_IDS, ResolvedConfig, supported_platform,
};
use spaceprune_contracts::messages;

pub(super) fn validate_config(config: &ResolvedConfig) -> Result<(), ConfigError> {
    if config.usage == crate::RecipeUsage::OneShot
        && config
            .automations
            .values()
            .any(|automation| automation.enabled)
    {
        return Err(ConfigError::Invalid {
            source_name: config.source.clone(),
            message: messages::text("config.one_shot_automation").to_owned(),
        });
    }
    if config.name.trim().is_empty() {
        return Err(ConfigError::Invalid {
            source_name: config.source.clone(),
            message: messages::text("config.empty_name").to_owned(),
        });
    }
    for (module_id, module) in &config.modules {
        validate_id(
            messages::text("config.kind_module"),
            module_id,
            &config.source,
        )?;
        validate_name(
            messages::text("config.kind_module"),
            module_id,
            &module.name,
            &config.source,
        )?;
        if MANAGEMENT_COMMAND_IDS.contains(&module_id.as_str()) {
            return Err(ConfigError::Invalid {
                source_name: config.source.clone(),
                message: messages::message(
                    "config.reserved_module",
                    &[("module_id", module_id.to_string())],
                ),
            });
        }
        for (selector_id, selector) in &module.selectors {
            validate_id(
                messages::text("config.kind_selector"),
                selector_id,
                &config.source,
            )?;
            validate_name(
                messages::text("config.kind_selector"),
                selector_id,
                &selector.name,
                &config.source,
            )?;
            if RESERVED_SELECTOR_IDS.contains(&selector_id.as_str()) {
                return Err(ConfigError::Invalid {
                    source_name: config.source.clone(),
                    message: messages::message(
                        "config.reserved_selector",
                        &[("selector_id", selector_id.to_string())],
                    ),
                });
            }
            validate_unique_values(
                messages::text("config.kind_selector_tag"),
                &format!("{module_id}.{selector_id}"),
                &selector.match_tags,
                &config.source,
            )?;
        }
        for (task_id, task) in &module.tasks {
            if task.handler == "builtin.filesystem/paths"
                && let Some(roots) = task.options.get("roots").and_then(toml::Value::as_array)
            {
                for root in roots {
                    let root: crate::FilesystemRoot =
                        root.clone()
                            .try_into()
                            .map_err(|error| ConfigError::Invalid {
                                source_name: config.source.clone(),
                                message: messages::message(
                                    "config.invalid_task_root",
                                    &[
                                        ("module_id", module_id.to_string()),
                                        ("task_id", task_id.to_string()),
                                        ("error", error.to_string()),
                                    ],
                                ),
                            })?;
                    if config.usage == crate::RecipeUsage::Recurring
                        && (matches!(
                            root.scope,
                            crate::RuleScope::ExactFile | crate::RuleScope::ExactSystemObject
                        ) || !root.bindings.is_empty())
                    {
                        return Err(ConfigError::Invalid {
                            source_name: config.source.clone(),
                            message: messages::message(
                                "config.recurring_exact_selection",
                                &[
                                    ("module_id", module_id.to_string()),
                                    ("task_id", task_id.to_string()),
                                ],
                            ),
                        });
                    }
                    if (root.scope == crate::RuleScope::ExactFile
                        || config.usage == crate::RecipeUsage::OneShot)
                        && root.bindings.is_empty()
                    {
                        return Err(ConfigError::Invalid {
                            source_name: config.source.clone(),
                            message: messages::message(
                                "config.one_shot_identity",
                                &[
                                    ("module_id", module_id.to_string()),
                                    ("task_id", task_id.to_string()),
                                ],
                            ),
                        });
                    }
                }
            }
            validate_id(messages::text("config.kind_task"), task_id, &config.source)?;
            validate_name(
                messages::text("config.kind_task"),
                task_id,
                &task.name,
                &config.source,
            )?;
            validate_unique_values(
                messages::text("config.kind_task_tag"),
                &format!("{module_id}.{task_id}"),
                &task.tags,
                &config.source,
            )?;
            validate_unique_values(
                messages::text("config.kind_task_platform"),
                &format!("{module_id}.{task_id}"),
                &task.platforms,
                &config.source,
            )?;
            let Some((plugin_id, handler_id)) = task.handler.split_once('/') else {
                return Err(ConfigError::Invalid {
                    source_name: config.source.clone(),
                    message: messages::message(
                        "config.handler_separator",
                        &[
                            ("module_id", module_id.to_string()),
                            ("task_id", task_id.to_string()),
                        ],
                    ),
                });
            };
            if handler_id.contains('/') {
                return Err(ConfigError::Invalid {
                    source_name: config.source.clone(),
                    message: messages::message(
                        "config.handler_separator_count",
                        &[
                            ("module_id", module_id.to_string()),
                            ("task_id", task_id.to_string()),
                        ],
                    ),
                });
            }
            validate_id(
                messages::text("config.kind_plugin"),
                plugin_id,
                &config.source,
            )?;
            validate_id(
                messages::text("config.kind_handler"),
                handler_id,
                &config.source,
            )?;
            for platform in &task.platforms {
                if !supported_platform(platform) {
                    return Err(ConfigError::Invalid {
                        source_name: config.source.clone(),
                        message: messages::message(
                            "config.unsupported_task_platform",
                            &[
                                ("module_id", module_id.to_string()),
                                ("task_id", task_id.to_string()),
                                ("platform", platform.to_string()),
                            ],
                        ),
                    });
                }
            }
        }
    }
    for (automation_id, automation) in &config.automations {
        validate_id(
            messages::text("config.kind_automation"),
            automation_id,
            &config.source,
        )?;
        validate_name(
            messages::text("config.kind_automation"),
            automation_id,
            &automation.name,
            &config.source,
        )?;
        validate_unique_values(
            messages::text("config.kind_automation_module"),
            automation_id,
            &automation.modules,
            &config.source,
        )?;
        for module in &automation.modules {
            if !config
                .modules
                .get(module)
                .is_some_and(|definition| definition.enabled)
            {
                return Err(ConfigError::Invalid {
                    source_name: config.source.clone(),
                    message: messages::message(
                        "config.automation_module_missing",
                        &[
                            ("automation_id", automation_id.to_string()),
                            ("module", module.to_string()),
                        ],
                    ),
                });
            }
        }
    }
    for (plugin_id, requirement) in &config.requires.plugins {
        validate_id(
            messages::text("config.kind_plugin"),
            plugin_id,
            &config.source,
        )?;
        if requirement.trim().is_empty() {
            return Err(ConfigError::Invalid {
                source_name: config.source.clone(),
                message: messages::message(
                    "config.empty_plugin_version",
                    &[("plugin_id", plugin_id.to_string())],
                ),
            });
        }
    }
    Ok(())
}

fn validate_id(kind: &str, id: &str, source_name: &str) -> Result<(), ConfigError> {
    if is_valid_id(id) {
        Ok(())
    } else {
        Err(ConfigError::Invalid {
            source_name: source_name.to_owned(),
            message: messages::message(
                "config.invalid_id",
                &[("kind", kind.to_string()), ("id", id.to_string())],
            ),
        })
    }
}

fn validate_name(kind: &str, id: &str, name: &str, source_name: &str) -> Result<(), ConfigError> {
    if name.trim().is_empty() {
        Err(ConfigError::Invalid {
            source_name: source_name.to_owned(),
            message: messages::message(
                "config.empty_kind_name",
                &[("kind", kind.to_string()), ("id", id.to_string())],
            ),
        })
    } else {
        Ok(())
    }
}

fn validate_unique_values(
    kind: &str,
    owner: &str,
    values: &[String],
    source_name: &str,
) -> Result<(), ConfigError> {
    let mut seen = std::collections::BTreeSet::new();
    for value in values {
        if value.trim().is_empty() {
            return Err(ConfigError::Invalid {
                source_name: source_name.to_owned(),
                message: messages::message(
                    "config.empty_unique_value",
                    &[("kind", kind.to_string()), ("owner", owner.to_string())],
                ),
            });
        }
        if !seen.insert(value) {
            return Err(ConfigError::Invalid {
                source_name: source_name.to_owned(),
                message: messages::message(
                    "config.duplicate_unique_value",
                    &[
                        ("kind", kind.to_string()),
                        ("value", value.to_string()),
                        ("owner", owner.to_string()),
                    ],
                ),
            });
        }
    }
    Ok(())
}

/// 判断字符串是否符合 SpacePrune 的稳定 ID 语法。
///
/// ID 由小写 ASCII 字母或数字组成，`.` 与 `-` 只能作为单个分隔符出现。
pub fn is_valid_id(id: &str) -> bool {
    let mut previous_separator = true;
    for byte in id.bytes() {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            previous_separator = false;
        } else if matches!(byte, b'.' | b'-') && !previous_separator {
            previous_separator = true;
        } else {
            return false;
        }
    }
    !previous_separator
}

#[cfg(test)]
mod tests {
    use crate::{ConfigLoader, MANAGEMENT_COMMAND_IDS, RESERVED_SELECTOR_IDS, is_valid_id};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn stable_ids_require_nonempty_segments() {
        assert!(is_valid_id("community.example-cache"));
        assert!(!is_valid_id(""));
        assert!(!is_valid_id("example."));
        assert!(!is_valid_id("example..cache"));
        assert!(!is_valid_id("Example"));
    }

    #[test]
    fn management_command_ids_are_reserved_for_modules() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("config.toml");

        for id in MANAGEMENT_COMMAND_IDS {
            fs::write(
                &path,
                format!(
                    r#"schema-version = 2
name = "test"

[modules.{id}]
name = "Reserved"
"#,
                ),
            )
            .unwrap();

            let error = ConfigLoader::new()
                .load_path(&path)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("reserved for a management command"),
                "{error}"
            );
        }
    }

    #[test]
    fn fixed_option_ids_are_reserved_for_selectors() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("config.toml");

        for id in RESERVED_SELECTOR_IDS {
            fs::write(
                &path,
                format!(
                    r#"schema-version = 2
name = "test"

[modules.personal]
name = "Personal"

[modules.personal.selectors.{id}]
name = "Reserved"
"#,
                ),
            )
            .unwrap();

            let error = ConfigLoader::new()
                .load_path(&path)
                .unwrap_err()
                .to_string();
            assert!(error.contains("reserved for a fixed CLI option"), "{error}");
        }
    }

    #[test]
    fn old_versions_and_unbound_one_shot_directories_are_rejected() {
        let temp = tempdir().unwrap();
        let recipe = temp.path().join("rule.toml");
        fs::write(&recipe, "schema-version = 1\nname = 'Old'\n").unwrap();
        assert!(
            ConfigLoader::new()
                .load_path(&recipe)
                .unwrap_err()
                .to_string()
                .contains("Unsupported schema-version=1")
        );
        fs::write(&recipe, "schema-version = 2\nname = 'Unbound'\nusage = 'one_shot'\n[modules.cache]\nname = 'Cache'\n[modules.cache.tasks.files]\nname = 'Files'\nhandler = 'builtin.filesystem/paths'\n[modules.cache.tasks.files.options]\nroots = [{path = '/tmp/observed', scope = 'directory_subtree'}]\n").unwrap();
        assert!(
            ConfigLoader::new()
                .load_path(&recipe)
                .unwrap_err()
                .to_string()
                .contains("requires an observed local identity")
        );
    }
}
