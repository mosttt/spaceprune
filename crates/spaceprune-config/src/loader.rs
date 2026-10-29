//! Recipe discovery, source loading, include resolution, and cache lifetime.

use crate::{
    AppConfig, ConfigError, RECIPE_DIRECTORY_NAME, ResolvedConfig, SCHEMA_VERSION, is_valid_id,
    resolve_theme_id, user_config_paths,
};
use crate::{
    parse::{merge_value, take_includes, validate_fragment},
    resolve::{expand_runtime_options, load_variables},
    validate::validate_config,
};
use spaceprune_contracts::messages;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use toml::Value;

/// 配置加载器。
#[derive(Debug, Default)]
pub struct ConfigLoader {
    cache: BTreeMap<String, (Value, Vec<PathBuf>)>,
    local_root: Option<PathBuf>,
}

impl ConfigLoader {
    /// 创建一个空加载器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 加载显式路径指向的 recipe 入口配置。
    pub fn load_path(&mut self, path: impl AsRef<Path>) -> Result<ResolvedConfig, ConfigError> {
        self.load_path_with_definition(path)
            .map(|(_, resolved)| resolved)
    }

    /// Load both the active include declarations and their device-resolved execution view.
    ///
    /// Editors use the unexpanded declarations to retain variables and portable roots. The
    /// original bundle remains authoritative for comments, includes and inactive platforms.
    pub fn load_path_with_definition(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<(crate::RecipeDefinition, ResolvedConfig), ConfigError> {
        self.load_path_with_sources(path)
            .map(|(definition, resolved, _)| (definition, resolved))
    }

    /// Load declarations, the execution view, and active local sources in merge order.
    ///
    /// Repeated includes retain their occurrences, including cached subtrees. Inactive
    /// paths are never resolved or read; embedded sources have no local editable path.
    pub fn load_path_with_sources(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<(crate::RecipeDefinition, ResolvedConfig, Vec<PathBuf>), ConfigError> {
        self.cache.clear();
        let path = normalize_path(path.as_ref())?;
        self.local_root = path.parent().map(Path::to_path_buf);
        let source = Source::File(path.clone());
        let mut stack = Vec::new();
        let (value, sources) = self.resolve(&source, &mut stack)?;
        let definition =
            value
                .clone()
                .try_into()
                .map_err(|error: toml::de::Error| ConfigError::Invalid {
                    source_name: path.display().to_string(),
                    message: error.to_string(),
                })?;
        let resolved = self.deserialize(value, path.display().to_string(), &source)?;
        Ok((definition, resolved, sources))
    }

    /// 加载应用入口及其选中的清理方案，并保留可供自动化重载的应用路径。
    pub fn load_app_path(&mut self, path: impl AsRef<Path>) -> Result<ResolvedConfig, ConfigError> {
        self.load_app_path_with_recipe(path, None)
    }

    /// Load an application configuration with a host-selected recipe, preserving validation.
    pub fn load_app_path_with_recipe(
        &mut self,
        path: impl AsRef<Path>,
        recipe: Option<&str>,
    ) -> Result<ResolvedConfig, ConfigError> {
        let path = normalize_path(path.as_ref())?;
        let source_name = path.display().to_string();
        let source = fs::read_to_string(&path).map_err(|source| ConfigError::Read {
            path: path.clone(),
            source,
        })?;
        let mut app: AppConfig = toml::from_str(&source).map_err(|error| ConfigError::Invalid {
            source_name: source_name.clone(),
            message: error.to_string(),
        })?;
        if app.schema_version != SCHEMA_VERSION {
            return Err(ConfigError::Invalid {
                source_name,
                message: messages::message(
                    "config.unsupported_schema_build",
                    &[
                        ("version", app.schema_version.to_string()),
                        ("supported", SCHEMA_VERSION.to_string()),
                    ],
                ),
            });
        }
        if let Some(recipe) = recipe {
            app.recipe = recipe.into();
        }
        if !is_valid_id(&app.recipe) {
            return Err(ConfigError::Invalid {
                source_name,
                message: messages::text("config.recipe_stable_id").to_owned(),
            });
        }
        if resolve_theme_id(&app.theme.id).is_none() {
            return Err(ConfigError::Invalid {
                source_name,
                message: messages::message(
                    "config.unknown_theme",
                    &[("id", app.theme.id.to_string())],
                ),
            });
        }
        let directory = path.parent().unwrap_or_else(|| Path::new("."));
        let recipe_path = directory
            .join(RECIPE_DIRECTORY_NAME)
            .join(&app.recipe)
            .join(format!("{}.toml", app.recipe));
        let mut config = self.load_path(recipe_path)?;
        config.application_source = Some(path);
        config.application_recipe = Some(app.recipe);
        Ok(config)
    }

    /// 加载内嵌官方配置。
    pub fn load_official(&mut self, name: &str) -> Result<ResolvedConfig, ConfigError> {
        self.cache.clear();
        self.local_root = None;
        let name = name.strip_prefix("official:").unwrap_or(name);
        let source = Source::Official(name.to_owned());
        let mut stack = Vec::new();
        let (value, _) = self.resolve(&source, &mut stack)?;
        self.deserialize(value, format!("official:{name}"), &source)
    }

    /// 加载首个存在的平台候选应用配置，否则使用内嵌最小方案。
    pub fn load_default(&mut self) -> Result<ResolvedConfig, ConfigError> {
        if let Some(path) = user_config_paths().into_iter().find(|path| path.is_file()) {
            return self.load_app_path(path);
        }
        self.load_official("minimal")
    }

    fn deserialize(
        &self,
        value: Value,
        source_name: String,
        root_source: &Source,
    ) -> Result<ResolvedConfig, ConfigError> {
        let definition: crate::RecipeDefinition =
            value.try_into().map_err(|error| ConfigError::Invalid {
                source_name: source_name.clone(),
                message: error.to_string(),
            })?;
        if definition.schema_version != SCHEMA_VERSION {
            return Err(ConfigError::Invalid {
                source_name,
                message: messages::message(
                    "config.unsupported_schema_build",
                    &[
                        ("version", definition.schema_version.to_string()),
                        ("supported", SCHEMA_VERSION.to_string()),
                    ],
                ),
            });
        }
        let mut config = ResolvedConfig {
            schema_version: definition.schema_version,
            name: definition.name,
            usage: definition.usage,
            source: source_name,
            application_source: None,
            application_recipe: None,
            variable_map: definition.variable_map,
            requires: definition.requires,
            modules: definition.modules,
            automations: definition.automations,
        };
        validate_config(&config)?;
        let variables = load_variables(
            root_source,
            config.variable_map.as_deref(),
            self.local_root.as_deref(),
        )?;
        expand_runtime_options(&mut config, &variables)?;
        Ok(config)
    }

    fn resolve(
        &mut self,
        source: &Source,
        stack: &mut Vec<String>,
    ) -> Result<(Value, Vec<PathBuf>), ConfigError> {
        let key = source.key();
        if let Some(value) = self.cache.get(&key) {
            return Ok(value.clone());
        }
        if let Some(index) = stack.iter().position(|item| item == &key) {
            let mut cycle = stack[index..].to_vec();
            cycle.push(key);
            return Err(ConfigError::IncludeCycle(cycle.join(" -> ")));
        }

        let is_root = stack.is_empty();
        stack.push(key.clone());
        let mut current = source.load()?;
        validate_fragment(&current, &key)?;
        if !is_root && current.get("variable-map").is_some() {
            return Err(ConfigError::Invalid {
                source_name: key,
                message: messages::text("config.variable_map_root_only").to_owned(),
            });
        }
        let includes = take_includes(&mut current, &key)?;
        let mut merged = Value::Table(toml::Table::new());
        let mut sources = Vec::new();
        for include in includes {
            if !include.matches_current_platform() {
                continue;
            }
            let child = source.resolve_include(&include.path)?;
            self.ensure_local_source(&child, &key)?;
            let (child_value, child_sources) = self.resolve(&child, stack)?;
            merge_value(&mut merged, child_value, "<root>").map_err(|message| {
                ConfigError::Invalid {
                    source_name: key.clone(),
                    message,
                }
            })?;
            sources.extend(child_sources);
        }
        merge_value(&mut merged, current, "<root>").map_err(|message| ConfigError::Invalid {
            source_name: key.clone(),
            message,
        })?;
        stack.pop();
        if let Source::File(path) = source {
            sources.push(path.clone());
        }
        let resolved = (merged, sources);
        self.cache.insert(key, resolved.clone());
        Ok(resolved)
    }

    fn ensure_local_source(&self, source: &Source, owner: &str) -> Result<(), ConfigError> {
        if let (Some(root), Source::File(path)) = (&self.local_root, source)
            && !path.starts_with(root)
        {
            return Err(ConfigError::Invalid {
                source_name: owner.to_owned(),
                message: messages::message(
                    "config.include_escape",
                    &[
                        ("path", path.display().to_string()),
                        ("root", root.display().to_string()),
                    ],
                ),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(super) enum Source {
    File(PathBuf),
    Official(String),
}

impl Source {
    pub(super) fn key(&self) -> String {
        match self {
            Self::File(path) => path.display().to_string(),
            Self::Official(name) => format!("official:{name}"),
        }
    }

    fn load(&self) -> Result<Value, ConfigError> {
        let (name, text) = match self {
            Self::File(path) => (
                path.display().to_string(),
                fs::read_to_string(path).map_err(|source| ConfigError::Read {
                    path: path.clone(),
                    source,
                })?,
            ),
            Self::Official(name) => (
                format!("official:{name}"),
                official_config(name)
                    .ok_or_else(|| ConfigError::UnknownOfficial(name.clone()))?
                    .to_owned(),
            ),
        };
        toml::from_str::<Value>(&text).map_err(|error| ConfigError::Invalid {
            source_name: name,
            message: error.to_string(),
        })
    }

    fn resolve_include(&self, include: &str) -> Result<Self, ConfigError> {
        if let Some(name) = include.strip_prefix("official:") {
            return Ok(Self::Official(name.to_owned()));
        }
        match self {
            Self::File(path) => {
                let parent = path.parent().unwrap_or_else(|| Path::new("."));
                normalize_path(&parent.join(include)).map(Self::File)
            }
            Self::Official(name) => Err(ConfigError::Invalid {
                source_name: format!("official:{name}"),
                message: messages::message(
                    "config.official_include",
                    &[("include", include.to_string())],
                ),
            }),
        }
    }

    pub(super) fn resolve_variable_map(&self, mapping: &str) -> Result<PathBuf, ConfigError> {
        match self {
            Self::File(path) => {
                let parent = path.parent().unwrap_or_else(|| Path::new("."));
                normalize_variable_map_path(&parent.join(mapping))
            }
            Self::Official(name) => Err(ConfigError::Invalid {
                source_name: format!("official:{name}"),
                message: messages::text("config.embedded_variable_map").to_owned(),
            }),
        }
    }
}

fn official_config(name: &str) -> Option<&'static str> {
    match name {
        "minimal" => Some(include_str!("../../../recipes/official/minimal.toml")),
        _ => None,
    }
}

fn normalize_path(path: &Path) -> Result<PathBuf, ConfigError> {
    path.canonicalize().map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })
}

fn normalize_variable_map_path(path: &Path) -> Result<PathBuf, ConfigError> {
    path.canonicalize()
        .map_err(|source| ConfigError::ReadVariableMap {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use crate::{ConfigLoader, current_platform};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn app_recipe_id_resolves_the_matching_directory_and_file() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join("recipes/personal")).unwrap();
        let app_path = temp.path().join("config.toml");
        let recipe_path = temp.path().join("recipes/personal/personal.toml");
        fs::write(&app_path, "schema-version = 2\nrecipe = 'personal'\n").unwrap();
        fs::write(&recipe_path, "schema-version = 2\nname = 'Personal'\n").unwrap();

        let config = ConfigLoader::new().load_app_path(&app_path).unwrap();
        assert_eq!(config.name, "Personal");
        assert_eq!(
            config.application_source.as_deref(),
            Some(app_path.canonicalize().unwrap().as_path())
        );
        assert_eq!(
            config.source,
            recipe_path.canonicalize().unwrap().display().to_string()
        );

        let direct = ConfigLoader::new().load_path(&recipe_path).unwrap();
        assert!(direct.application_source.is_none());
    }

    #[test]
    fn app_loader_rejects_an_unknown_theme() {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("config.toml"),
            "schema-version = 2\nrecipe = 'personal'\n[theme]\nid = 'unknown'\n",
        )
        .unwrap();

        let error = ConfigLoader::new()
            .load_app_path(temp.path().join("config.toml"))
            .unwrap_err();
        assert!(error.to_string().contains("unknown theme"));
    }

    #[test]
    fn documentation_toml_examples_are_valid() {
        let documents = [
            ("README.md", include_str!("../../../README.md")),
            (
                "docs/zh/reference/ARCHITECTURE.md",
                include_str!("../../../docs/zh/reference/ARCHITECTURE.md"),
            ),
            (
                "docs/zh/build/CI.md",
                include_str!("../../../docs/zh/build/CI.md"),
            ),
            (
                "docs/zh/usage/CONFIGURATION.md",
                include_str!("../../../docs/zh/usage/CONFIGURATION.md"),
            ),
            (
                "docs/zh/reference/EVENTS.md",
                include_str!("../../../docs/zh/reference/EVENTS.md"),
            ),
            (
                "docs/zh/reference/PLUGIN-PROTOCOL.md",
                include_str!("../../../docs/zh/reference/PLUGIN-PROTOCOL.md"),
            ),
            (
                "docs/zh/usage/RECIPE-GUIDE.md",
                include_str!("../../../docs/zh/usage/RECIPE-GUIDE.md"),
            ),
        ];
        let parsed = documents
            .into_iter()
            .map(|(path, markdown)| parse_documented_toml(path, markdown))
            .sum::<usize>();
        assert!(parsed > 0);
    }

    fn parse_documented_toml(path: &str, markdown: &str) -> usize {
        let normalized = markdown.replace("\r\n", "\n");
        let mut parsed = 0;
        for fence in ["~~~", "```"] {
            let opening = format!("{fence}toml\n");
            let closing = format!("\n{fence}");
            let mut remaining = normalized.as_str();
            while let Some((_, after_opening)) = remaining.split_once(&opening) {
                let (source, after_closing) = after_opening
                    .split_once(&closing)
                    .unwrap_or_else(|| panic!("Unclosed TOML block in {path}"));
                toml::from_str::<toml::Value>(source)
                    .unwrap_or_else(|error| panic!("Invalid TOML block in {path}: {error}"));
                parsed += 1;
                remaining = after_closing;
            }
        }
        parsed
    }

    #[test]
    fn rejects_includes_and_variable_maps_outside_recipe_root() {
        let temp = tempdir().unwrap();
        let recipe = temp.path().join("recipe");
        fs::create_dir(&recipe).unwrap();
        fs::write(
            temp.path().join("outside.toml"),
            "schema-version = 2\nname = 'outside'\n",
        )
        .unwrap();
        fs::write(
            recipe.join("recipe.toml"),
            "schema-version = 2\nname = 'recipe'\ninclude = ['../outside.toml']\n",
        )
        .unwrap();
        let error = ConfigLoader::new()
            .load_path(recipe.join("recipe.toml"))
            .unwrap_err();
        assert!(error.to_string().contains("escapes recipe root"));

        fs::write(temp.path().join("variables.toml"), "schema-version = 2\n").unwrap();
        fs::write(
            recipe.join("recipe.toml"),
            "schema-version = 2\nname = 'recipe'\nvariable-map = '../variables.toml'\n",
        )
        .unwrap();
        let error = ConfigLoader::new()
            .load_path(recipe.join("recipe.toml"))
            .unwrap_err();
        assert!(error.to_string().contains("escapes recipe root"));
    }

    #[test]
    fn loads_embedded_official_configuration() {
        let config = ConfigLoader::new().load_official("minimal").unwrap();
        assert_eq!(
            config
                .modules
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["cache"]
        );
        let task = if current_platform() == "windows" {
            &config.modules["cache"].tasks["user-temporary"]
        } else {
            &config.modules["cache"].tasks["user-cache"]
        };
        if task
            .platforms
            .iter()
            .any(|platform| platform == current_platform())
        {
            let path = task.options["roots"][0]["path"].as_str().unwrap();
            assert!(!path.contains("${"));
        }
        assert!(ConfigLoader::new().load_official("spaceprune").is_err());
    }

    #[test]
    fn clears_include_cache_between_root_loads() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(&path, "schema-version = 2\nname = 'first'\n").unwrap();
        let mut loader = ConfigLoader::new();
        assert_eq!(loader.load_path(&path).unwrap().name, "first");

        fs::write(&path, "schema-version = 2\nname = 'second'\n").unwrap();
        assert_eq!(loader.load_path(&path).unwrap().name, "second");
    }

    #[test]
    fn cached_include_subtrees_preserve_their_actual_merge_order() {
        let temp = tempdir().unwrap();
        for (name, source) in [
            (
                "root.toml",
                "schema-version = 2\nname = 'Root'\ninclude = ['branch.toml', 'override.toml', 'branch.toml']\n",
            ),
            (
                "branch.toml",
                "schema-version = 2\ninclude = ['leaf.toml']\n",
            ),
            (
                "leaf.toml",
                "schema-version = 2\n[modules.cache]\nname = 'Leaf'\n",
            ),
            (
                "override.toml",
                "schema-version = 2\n[modules.cache]\nname = 'Override'\n",
            ),
        ] {
            fs::write(temp.path().join(name), source).unwrap();
        }
        let (_, resolved, sources) = ConfigLoader::new()
            .load_path_with_sources(temp.path().join("root.toml"))
            .unwrap();
        assert_eq!(resolved.modules["cache"].name, "Leaf");
        assert_eq!(
            sources
                .iter()
                .map(|path| path.file_name().unwrap().to_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "leaf.toml",
                "branch.toml",
                "override.toml",
                "leaf.toml",
                "branch.toml",
                "root.toml"
            ]
        );
    }
}
