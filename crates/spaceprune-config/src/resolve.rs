//! Variable-map validation and current-platform option expansion.

use crate::loader::Source;
use crate::{ConfigError, ResolvedConfig, SCHEMA_VERSION, current_platform, supported_platform};
use serde::Deserialize;
use spaceprune_contracts::messages;
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};
use toml::Value;

fn official_variable_map() -> &'static str {
    include_str!("../../../mappings/official/variables.toml")
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct VariableMapFile {
    #[serde(rename = "schema-version")]
    schema_version: u32,
    #[serde(default)]
    variables: BTreeMap<String, VariableDefinition>,
    #[serde(default)]
    platforms: BTreeMap<String, PlatformVariableMap>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlatformVariableMap {
    #[serde(default)]
    variables: BTreeMap<String, VariableDefinition>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct VariableDefinition {
    #[serde(default)]
    environment: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    fallback: Option<String>,
}

pub(super) fn load_variables(
    root_source: &Source,
    user_mapping: Option<&str>,
    local_root: Option<&Path>,
) -> Result<BTreeMap<String, String>, ConfigError> {
    let default = parse_variable_map(official_variable_map(), "official:variables")?;
    let mut definitions = active_variable_definitions(default);
    let mut source_name = "official:variables".to_owned();

    if let Some(mapping) = user_mapping {
        if mapping.trim().is_empty() {
            return Err(ConfigError::Invalid {
                source_name: root_source.key(),
                message: messages::text("config.variable_map_path").to_owned(),
            });
        }
        let path = root_source.resolve_variable_map(mapping)?;
        if local_root.is_some_and(|root| !path.starts_with(root)) {
            return Err(ConfigError::InvalidVariableMap {
                source_name: root_source.key(),
                message: messages::message(
                    "config.variable_map_escape",
                    &[
                        ("path", path.display().to_string()),
                        (
                            "root",
                            local_root.expect("checked as Some").display().to_string(),
                        ),
                    ],
                ),
            });
        }
        let text = fs::read_to_string(&path).map_err(|source| ConfigError::ReadVariableMap {
            path: path.clone(),
            source,
        })?;
        source_name = path.display().to_string();
        let user = parse_variable_map(&text, &source_name)?;
        definitions.extend(active_variable_definitions(user));
    }

    resolve_variables(&definitions, &source_name)
}

fn parse_variable_map(text: &str, source_name: &str) -> Result<VariableMapFile, ConfigError> {
    let mapping: VariableMapFile =
        toml::from_str(text).map_err(|error| ConfigError::InvalidVariableMap {
            source_name: source_name.to_owned(),
            message: error.to_string(),
        })?;
    if mapping.schema_version != SCHEMA_VERSION {
        return Err(ConfigError::InvalidVariableMap {
            source_name: source_name.to_owned(),
            message: messages::message(
                "config.unsupported_schema_build",
                &[
                    ("version", mapping.schema_version.to_string()),
                    ("supported", SCHEMA_VERSION.to_string()),
                ],
            ),
        });
    }
    for platform in mapping.platforms.keys() {
        if !supported_platform(platform) {
            return Err(ConfigError::InvalidVariableMap {
                source_name: source_name.to_owned(),
                message: messages::message(
                    "config.mapping_platform",
                    &[("platform", platform.to_string())],
                ),
            });
        }
    }
    for (name, definition) in mapping.variables.iter().chain(
        mapping
            .platforms
            .values()
            .flat_map(|platform| platform.variables.iter()),
    ) {
        validate_variable_definition(name, definition, source_name)?;
    }
    Ok(mapping)
}

fn validate_variable_definition(
    name: &str,
    definition: &VariableDefinition,
    source_name: &str,
) -> Result<(), ConfigError> {
    if !valid_variable_name(name) {
        return Err(ConfigError::InvalidVariableMap {
            source_name: source_name.to_owned(),
            message: messages::message("config.variable_name", &[("name", name.to_string())]),
        });
    }
    match (&definition.environment, &definition.value) {
        (Some(environment), None) if valid_environment_name(environment) => {}
        (Some(environment), None) => {
            return Err(ConfigError::InvalidVariableMap {
                source_name: source_name.to_owned(),
                message: messages::message(
                    "config.variable_environment",
                    &[
                        ("name", name.to_string()),
                        ("environment", environment.to_string()),
                    ],
                ),
            });
        }
        (None, Some(value)) if !value.is_empty() && definition.fallback.is_none() => {}
        (None, Some(_)) if definition.fallback.is_some() => {
            return Err(ConfigError::InvalidVariableMap {
                source_name: source_name.to_owned(),
                message: messages::message(
                    "config.variable_fallback_value",
                    &[("name", name.to_string())],
                ),
            });
        }
        (Some(_), Some(_)) => {
            return Err(ConfigError::InvalidVariableMap {
                source_name: source_name.to_owned(),
                message: messages::message("config.variable_either", &[("name", name.to_string())]),
            });
        }
        _ => {
            return Err(ConfigError::InvalidVariableMap {
                source_name: source_name.to_owned(),
                message: messages::message(
                    "config.variable_required",
                    &[("name", name.to_string())],
                ),
            });
        }
    }
    Ok(())
}

fn active_variable_definitions(
    mut mapping: VariableMapFile,
) -> BTreeMap<String, VariableDefinition> {
    if let Some(platform) = mapping.platforms.remove(current_platform()) {
        mapping.variables.extend(platform.variables);
    }
    mapping.variables
}

fn resolve_variables(
    definitions: &BTreeMap<String, VariableDefinition>,
    source_name: &str,
) -> Result<BTreeMap<String, String>, ConfigError> {
    let mut cache: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut stack = Vec::new();
    for name in definitions.keys() {
        let _ = resolve_variable(name, definitions, &mut cache, &mut stack, source_name)?;
    }
    Ok(cache
        .into_iter()
        .filter_map(|(name, value)| value.map(|value| (name, value)))
        .collect())
}

fn resolve_variable(
    name: &str,
    definitions: &BTreeMap<String, VariableDefinition>,
    cache: &mut BTreeMap<String, Option<String>>,
    stack: &mut Vec<String>,
    source_name: &str,
) -> Result<Option<String>, ConfigError> {
    if let Some(value) = cache.get(name) {
        return Ok(value.clone());
    }
    let definition = definitions
        .get(name)
        .ok_or_else(|| ConfigError::InvalidVariableMap {
            source_name: source_name.to_owned(),
            message: messages::message("config.variable_undefined", &[("name", name.to_string())]),
        })?;
    if let Some(index) = stack.iter().position(|item| item == name) {
        let mut cycle = stack[index..].to_vec();
        cycle.push(name.to_owned());
        return Err(ConfigError::InvalidVariableMap {
            source_name: source_name.to_owned(),
            message: messages::message("config.variable_cycle", &[("cycle", cycle.join(" -> "))]),
        });
    }

    stack.push(name.to_owned());
    let resolved = if let Some(environment) = &definition.environment {
        if let Some(value) = env::var_os(environment).filter(|value| !value.is_empty()) {
            Some(value.to_string_lossy().into_owned())
        } else {
            definition
                .fallback
                .as_deref()
                .map(|value| expand_mapping_value(value, definitions, cache, stack, source_name))
                .transpose()?
                .flatten()
        }
    } else {
        definition
            .value
            .as_deref()
            .map(|value| expand_mapping_value(value, definitions, cache, stack, source_name))
            .transpose()?
            .flatten()
    };
    stack.pop();
    cache.insert(name.to_owned(), resolved.clone());
    Ok(resolved)
}

fn expand_mapping_value(
    value: &str,
    definitions: &BTreeMap<String, VariableDefinition>,
    cache: &mut BTreeMap<String, Option<String>>,
    stack: &mut Vec<String>,
    source_name: &str,
) -> Result<Option<String>, ConfigError> {
    let mut output = String::with_capacity(value.len());
    let mut remainder = value;
    while let Some(start) = remainder.find("${") {
        output.push_str(&remainder[..start]);
        let expression = &remainder[start + 2..];
        let Some(end) = expression.find('}') else {
            return Err(ConfigError::InvalidVariableMap {
                source_name: source_name.to_owned(),
                message: messages::message(
                    "config.variable_unclosed",
                    &[("value", value.to_string())],
                ),
            });
        };
        let name = &expression[..end];
        if !valid_variable_name(name) {
            return Err(ConfigError::InvalidVariableMap {
                source_name: source_name.to_owned(),
                message: messages::message(
                    "config.variable_expression",
                    &[
                        ("expression", format!("${{{name}}}")),
                        ("value", value.to_string()),
                    ],
                ),
            });
        }
        let Some(replacement) = resolve_variable(name, definitions, cache, stack, source_name)?
        else {
            return Ok(None);
        };
        output.push_str(&replacement);
        remainder = &expression[end + 1..];
    }
    output.push_str(remainder);
    Ok(Some(output))
}

pub(super) fn expand_runtime_options(
    config: &mut ResolvedConfig,
    variables: &BTreeMap<String, String>,
) -> Result<(), ConfigError> {
    let source_name = config.source.clone();
    for module in config.modules.values_mut().filter(|module| module.enabled) {
        for task in module.tasks.values_mut().filter(|task| {
            task.enabled
                && (task.platforms.is_empty()
                    || task
                        .platforms
                        .iter()
                        .any(|platform| platform == current_platform()))
        }) {
            expand_table(&mut task.options, variables, &source_name)?;
            crate::roots::resolve_task_roots(task, &source_name)?;
        }
    }
    Ok(())
}

fn expand_table(
    table: &mut toml::Table,
    variables: &BTreeMap<String, String>,
    source_name: &str,
) -> Result<(), ConfigError> {
    for (_, value) in table.iter_mut() {
        expand_value(value, variables, source_name)?;
    }
    Ok(())
}

fn expand_value(
    value: &mut Value,
    variables: &BTreeMap<String, String>,
    source_name: &str,
) -> Result<(), ConfigError> {
    match value {
        Value::String(text) => *text = expand_option_string(text, variables, source_name)?,
        Value::Array(values) => {
            for value in values {
                expand_value(value, variables, source_name)?;
            }
        }
        Value::Table(table) => expand_table(table, variables, source_name)?,
        _ => {}
    }
    Ok(())
}

fn expand_option_string(
    value: &str,
    variables: &BTreeMap<String, String>,
    source_name: &str,
) -> Result<String, ConfigError> {
    let mut output = String::with_capacity(value.len());
    let mut remainder = value;
    while let Some(start) = remainder.find("${") {
        output.push_str(&remainder[..start]);
        let expression = &remainder[start + 2..];
        let Some(end) = expression.find('}') else {
            return Err(ConfigError::Invalid {
                source_name: source_name.to_owned(),
                message: messages::message(
                    "config.option_variable_unclosed",
                    &[("value", value.to_string())],
                ),
            });
        };
        let name = &expression[..end];
        if !valid_variable_name(name) {
            return Err(ConfigError::Invalid {
                source_name: source_name.to_owned(),
                message: messages::message(
                    "config.option_variable_expression",
                    &[
                        ("expression", format!("${{{name}}}")),
                        ("value", value.to_string()),
                    ],
                ),
            });
        }
        let replacement = variables.get(name).ok_or_else(|| ConfigError::Invalid {
            source_name: source_name.to_owned(),
            message: messages::message(
                "config.variable_unavailable",
                &[("name", name.to_string())],
            ),
        })?;
        output.push_str(replacement);
        remainder = &expression[end + 1..];
    }
    output.push_str(remainder);

    if output == "~" || output.starts_with("~/") || output.starts_with("~\\") {
        let home = variables.get("HOME").ok_or_else(|| ConfigError::Invalid {
            source_name: source_name.to_owned(),
            message: messages::text("config.home_required").to_owned(),
        })?;
        let suffix = output
            .strip_prefix("~/")
            .or_else(|| output.strip_prefix("~\\"))
            .unwrap_or("");
        return Ok(PathBuf::from(home)
            .join(suffix)
            .to_string_lossy()
            .into_owned());
    }
    Ok(output)
}

fn valid_variable_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn valid_environment_name(value: &str) -> bool {
    !value.is_empty() && !value.contains(['\0', '='])
}

#[cfg(test)]
mod tests {
    use crate::{ConfigLoader, current_platform};
    use std::{fs, path::PathBuf};
    use tempfile::tempdir;

    #[test]
    fn user_variable_map_overrides_defaults_and_platform_values() {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("variables.toml"),
            format!(
                r#"
schema-version = 2

[variables.HOME]
value = "/mapped-home"

[variables.CACHE_HOME]
value = "${{HOME}}/common-cache"

[variables.WINDOWS_STYLE_ENV]
environment = "ProgramFiles(x86)"

[platforms.{}.variables.CACHE_HOME]
value = "${{HOME}}/platform-cache"
"#,
                current_platform()
            ),
        )
        .unwrap();
        fs::write(
            temp.path().join("config.toml"),
            r#"
schema-version = 2
name = "mapped"
variable-map = "variables.toml"

[modules.cache]
name = "Cache"

[modules.cache.tasks.files]
name = "Files"
handler = "builtin.filesystem/paths"

[modules.cache.tasks.files.options]
roots = [
  { path = "${CACHE_HOME}/app" },
  { path = "~/direct" },
]
"#,
        )
        .unwrap();

        let config = ConfigLoader::new()
            .load_path(temp.path().join("config.toml"))
            .unwrap();
        let roots = config.modules["cache"].tasks["files"].options["roots"]
            .as_array()
            .unwrap();
        assert_eq!(
            roots[0]["path"].as_str(),
            Some("/mapped-home/platform-cache/app")
        );
        let expected_direct = PathBuf::from("/mapped-home").join("direct");
        assert_eq!(roots[1]["path"].as_str(), expected_direct.to_str());
    }

    #[test]
    fn skips_option_expansion_for_inactive_tasks() {
        let temp = tempdir().unwrap();
        let inactive = if current_platform() == "linux" {
            "windows"
        } else {
            "linux"
        };
        fs::write(
            temp.path().join("config.toml"),
            format!(
                r#"
schema-version = 2
name = "inactive"

[modules.cache]
name = "Cache"

[modules.cache.tasks.other-platform]
name = "Other platform"
handler = "builtin.filesystem/paths"
platforms = ["{inactive}"]

[modules.cache.tasks.other-platform.options]
roots = [{{ path = "${{NOT_DEFINED}}" }}]
"#
            ),
        )
        .unwrap();

        let config = ConfigLoader::new()
            .load_path(temp.path().join("config.toml"))
            .unwrap();
        assert_eq!(
            config.modules["cache"].tasks["other-platform"].options["roots"][0]["path"].as_str(),
            Some("${NOT_DEFINED}")
        );
    }

    #[test]
    fn rejects_unknown_and_cyclic_variable_references() {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("config.toml"),
            r#"
schema-version = 2
name = "missing"
[modules.cache]
name = "Cache"
[modules.cache.tasks.files]
name = "Files"
handler = "builtin.filesystem/paths"
[modules.cache.tasks.files.options]
roots = [{ path = "${NOT_DEFINED}" }]
"#,
        )
        .unwrap();
        let error = ConfigLoader::new()
            .load_path(temp.path().join("config.toml"))
            .unwrap_err();
        assert!(error.to_string().contains("NOT_DEFINED"));

        fs::write(
            temp.path().join("variables.toml"),
            r#"
schema-version = 2
[variables.A]
value = "${B}"
[variables.B]
value = "${A}"
"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("cycle.toml"),
            "schema-version = 2\nname = 'cycle'\nvariable-map = 'variables.toml'\n",
        )
        .unwrap();
        let error = ConfigLoader::new()
            .load_path(temp.path().join("cycle.toml"))
            .unwrap_err();
        assert!(error.to_string().contains("Variable reference cycle"));
    }

    #[test]
    fn variable_map_is_root_only() {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("child.toml"),
            "schema-version = 2\nname = 'child'\nvariable-map = 'variables.toml'\n",
        )
        .unwrap();
        fs::write(
            temp.path().join("root.toml"),
            "schema-version = 2\nname = 'root'\ninclude = ['child.toml']\n",
        )
        .unwrap();

        let error = ConfigLoader::new()
            .load_path(temp.path().join("root.toml"))
            .unwrap_err();
        assert!(error.to_string().contains("recipe root configuration"));
    }
}
