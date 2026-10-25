//! Include syntax, fragment schema checks, and deterministic TOML merging.

use crate::{ConfigError, SCHEMA_VERSION, current_platform};
use spaceprune_contracts::messages;
use toml::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct IncludeSpec {
    pub(super) path: String,
    platforms: Vec<String>,
}

impl IncludeSpec {
    pub(super) fn matches_current_platform(&self) -> bool {
        self.platforms.is_empty()
            || self
                .platforms
                .iter()
                .any(|platform| platform == current_platform())
    }
}

pub(super) fn validate_fragment(value: &Value, source_name: &str) -> Result<(), ConfigError> {
    let version = value
        .get("schema-version")
        .and_then(Value::as_integer)
        .ok_or_else(|| ConfigError::Invalid {
            source_name: source_name.to_owned(),
            message: messages::message(
                "config.missing_schema",
                &[("SCHEMA_VERSION", SCHEMA_VERSION.to_string())],
            ),
        })?;
    if version != i64::from(SCHEMA_VERSION) {
        return Err(ConfigError::Invalid {
            source_name: source_name.to_owned(),
            message: messages::message(
                "config.unsupported_schema",
                &[("version", version.to_string())],
            ),
        });
    }
    Ok(())
}

pub(super) fn take_includes(
    value: &mut Value,
    source_name: &str,
) -> Result<Vec<IncludeSpec>, ConfigError> {
    let Some(table) = value.as_table_mut() else {
        return Err(ConfigError::Invalid {
            source_name: source_name.to_owned(),
            message: messages::text("config.root_table").to_owned(),
        });
    };
    let Some(include) = table.remove("include") else {
        return Ok(Vec::new());
    };
    let Some(values) = include.as_array() else {
        return Err(ConfigError::Invalid {
            source_name: source_name.to_owned(),
            message: messages::text("config.include_array").to_owned(),
        });
    };
    values
        .iter()
        .map(|value| parse_include(value, source_name))
        .collect()
}

fn parse_include(value: &Value, source_name: &str) -> Result<IncludeSpec, ConfigError> {
    if let Some(path) = value.as_str() {
        return Ok(IncludeSpec {
            path: path.to_owned(),
            platforms: Vec::new(),
        });
    }
    let table = value.as_table().ok_or_else(|| ConfigError::Invalid {
        source_name: source_name.to_owned(),
        message: messages::text("config.include_entry").to_owned(),
    })?;
    if let Some(field) = table
        .keys()
        .find(|field| !matches!(field.as_str(), "path" | "platforms"))
    {
        return Err(ConfigError::Invalid {
            source_name: source_name.to_owned(),
            message: messages::message("config.include_field", &[("field", field.to_string())]),
        });
    }
    let path = table
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .ok_or_else(|| ConfigError::Invalid {
            source_name: source_name.to_owned(),
            message: messages::text("config.include_path").to_owned(),
        })?;
    let platforms = table
        .get("platforms")
        .map(|value| parse_platforms(value, source_name))
        .transpose()?
        .unwrap_or_default();
    Ok(IncludeSpec {
        path: path.to_owned(),
        platforms,
    })
}

fn parse_platforms(value: &Value, source_name: &str) -> Result<Vec<String>, ConfigError> {
    let values = value.as_array().ok_or_else(|| ConfigError::Invalid {
        source_name: source_name.to_owned(),
        message: messages::text("config.platforms_array").to_owned(),
    })?;
    values
        .iter()
        .map(|value| {
            let platform = value.as_str().ok_or_else(|| ConfigError::Invalid {
                source_name: source_name.to_owned(),
                message: messages::text("config.platforms_strings").to_owned(),
            })?;
            if matches!(platform, "linux" | "macos" | "windows" | "android" | "ios") {
                Ok(platform.to_owned())
            } else {
                Err(ConfigError::Invalid {
                    source_name: source_name.to_owned(),
                    message: messages::message(
                        "config.include_platform",
                        &[("platform", platform.to_string())],
                    ),
                })
            }
        })
        .collect()
}

pub(super) fn merge_value(base: &mut Value, overlay: Value, path: &str) -> Result<(), String> {
    match (base, overlay) {
        (Value::Table(base), Value::Table(overlay)) => {
            for (key, value) in overlay {
                if let Some(existing) = base.get_mut(&key) {
                    let child_path = if path == "<root>" {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    merge_value(existing, value, &child_path)?;
                } else {
                    base.insert(key, value);
                }
            }
            Ok(())
        }
        (base, overlay) if same_value_kind(base, &overlay) => {
            *base = overlay;
            Ok(())
        }
        (base, overlay) => Err(messages::message(
            "config.type_conflict",
            &[
                ("path", path.to_string()),
                ("base", value_kind(base).to_string()),
                ("overlay", value_kind(&overlay).to_string()),
            ],
        )),
    }
}

fn same_value_kind(left: &Value, right: &Value) -> bool {
    std::mem::discriminant(left) == std::mem::discriminant(right)
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::String(_) => messages::text("config.type_string"),
        Value::Integer(_) => messages::text("config.type_integer"),
        Value::Float(_) => messages::text("config.type_float"),
        Value::Boolean(_) => messages::text("config.type_boolean"),
        Value::Datetime(_) => messages::text("config.type_date_time"),
        Value::Array(_) => messages::text("config.type_array"),
        Value::Table(_) => messages::text("config.type_table"),
    }
}

#[cfg(test)]
mod tests {
    use crate::{ConfigLoader, current_platform};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn included_configuration_requires_its_own_schema_version() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("config.toml");
        fs::write(
            &root,
            r#"schema-version = 2
name = "test"
include = ["personal.generated.toml"]
"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("personal.generated.toml"),
            r#"[modules.personal]
name = "Personal"
"#,
        )
        .unwrap();

        let error = ConfigLoader::new().load_path(root).unwrap_err().to_string();
        assert!(error.contains("including included files"));
        assert!(error.contains("schema-version = 2"));
    }

    #[test]
    fn include_deep_merges_tables_and_replaces_scalars() {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("base.toml"),
            r#"
schema-version = 2
name = "base"
[modules.agent]
name = "Agent"
[modules.agent.tasks.sessions]
name = "Sessions"
handler = "example.agent/sessions"
[modules.agent.tasks.sessions.recommend]
older-than = "30d"
"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("root.toml"),
            r#"
schema-version = 2
name = "root"
include = ["base.toml"]
[modules.agent.tasks.sessions.recommend]
older-than = "7d"
"#,
        )
        .unwrap();

        let config = ConfigLoader::new()
            .load_path(temp.path().join("root.toml"))
            .unwrap();
        let task = &config.modules["agent"].tasks["sessions"];
        assert_eq!(config.name, "root");
        assert_eq!(task.name, "Sessions");
        assert_eq!(task.recommend["older-than"].as_str(), Some("7d"));
    }

    #[test]
    fn detects_include_cycle() {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("a.toml"),
            "schema-version = 2\nname='a'\ninclude=['b.toml']\n",
        )
        .unwrap();
        fs::write(
            temp.path().join("b.toml"),
            "schema-version = 2\nname='b'\ninclude=['a.toml']\n",
        )
        .unwrap();

        let error = ConfigLoader::new()
            .load_path(temp.path().join("a.toml"))
            .unwrap_err();
        assert!(error.to_string().contains("include cycle"));
    }

    #[test]
    fn conditionally_includes_only_the_current_platform() {
        let temp = tempdir().unwrap();
        let active = current_platform();
        let inactive = if active == "linux" {
            "windows"
        } else {
            "linux"
        };
        fs::write(
            temp.path().join("common.toml"),
            r#"
schema-version = 2
name = "common"
[modules.cache]
name = "Cache"
[modules.cache.tasks.records]
name = "Records"
handler = "builtin.filesystem/paths"
"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("active.toml"),
            r#"
schema-version = 2
name = "active"
[modules.cache.tasks.records.options]
roots = [{ path = "./active-cache" }]
"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("root.toml"),
            format!(
                r#"
schema-version = 2
name = "root"
include = [
  "common.toml",
  {{ path = "active.toml", platforms = ["{active}"] }},
  {{ path = "missing.toml", platforms = ["{inactive}"] }},
]
"#
            ),
        )
        .unwrap();

        let config = ConfigLoader::new()
            .load_path(temp.path().join("root.toml"))
            .unwrap();
        let roots = config.modules["cache"].tasks["records"].options["roots"]
            .as_array()
            .unwrap();
        assert_eq!(roots[0]["path"].as_str(), Some("./active-cache"));
    }

    #[test]
    fn rejects_include_type_conflicts() {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("base.toml"),
            "schema-version = 2\nname='base'\n[modules.cache]\nname='cache'\norder=1\n",
        )
        .unwrap();
        fs::write(
            temp.path().join("root.toml"),
            "schema-version = 2\nname='root'\ninclude=['base.toml']\n[modules.cache]\norder='first'\n",
        )
        .unwrap();
        let error = ConfigLoader::new()
            .load_path(temp.path().join("root.toml"))
            .unwrap_err();
        assert!(error.to_string().contains("type conflict"));
    }
}
