//! Application configuration and the resolved recipe view.

use crate::{AutomationDefinition, DEFAULT_THEME_ID, ModuleDefinition, Requirements};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};
use thiserror::Error;

/// 配置加载错误。
#[derive(Debug, Error)]
pub enum ConfigError {
    /// 无法读取配置文件。
    #[error("{}", spaceprune_contracts::messages::message("config.read", &[("path", .path.display().to_string()), ("source", .source.to_string())]))]
    Read {
        /// 配置路径。
        path: PathBuf,
        /// 底层读取错误。
        source: std::io::Error,
    },
    /// 无法读取变量映射文件。
    #[error("{}", spaceprune_contracts::messages::message("config.read_variables", &[("path", .path.display().to_string()), ("source", .source.to_string())]))]
    ReadVariableMap {
        /// 映射文件路径。
        path: PathBuf,
        /// 底层读取错误。
        source: std::io::Error,
    },
    /// TOML 语法或结构错误。
    #[error("{}", spaceprune_contracts::messages::message("config.invalid", &[("source_name", .source_name.to_string()), ("message", .message.to_string())]))]
    Invalid {
        /// 配置来源名称。
        source_name: String,
        /// 错误说明。
        message: String,
    },
    /// 变量映射语法、结构或引用错误。
    #[error("{}", spaceprune_contracts::messages::message("config.invalid_variables", &[("source_name", .source_name.to_string()), ("message", .message.to_string())]))]
    InvalidVariableMap {
        /// 映射来源名称。
        source_name: String,
        /// 错误说明。
        message: String,
    },
    /// include 引用了不存在的官方配置。
    #[error("{}", spaceprune_contracts::messages::message("config.unknown_official", &[("id", .0.to_string())]))]
    UnknownOfficial(String),
    /// include 形成循环。
    #[error("{}", spaceprune_contracts::messages::message("config.include_cycle", &[("id", .0.to_string())]))]
    IncludeCycle(String),
}

/// SpacePrune 应用入口配置。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    /// 配置 Schema 版本。
    #[serde(rename = "schema-version")]
    pub schema_version: u32,
    /// 当前启用的清理方案稳定 ID。
    pub recipe: String,
    /// Shared application display language; missing preferences follow the system.
    #[serde(default)]
    pub language: crate::LanguagePreference,
    /// 终端主题设置。
    #[serde(default)]
    pub theme: ThemeDefinition,
}

/// 完整解析后的 SpacePrune 清理配置。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResolvedConfig {
    /// 配置 Schema 版本。
    #[serde(rename = "schema-version")]
    pub schema_version: u32,
    /// 清理方案名称。
    pub name: String,
    /// Persistent recipe purpose; one-shot recipes cannot enter recurring automation.
    #[serde(default)]
    pub usage: spaceprune_contracts::rule::RecipeUsage,
    /// Recipe 配置来源，用于报告和错误定位。
    #[serde(default)]
    pub source: String,
    /// 加载本配置的应用 `config.toml`；直接加载 Recipe 或内嵌配置时为空。
    ///
    /// 这是运行时元数据，不属于 Recipe TOML，也不会进入序列化结果。
    #[serde(skip)]
    pub application_source: Option<PathBuf>,
    /// 用户变量映射文件；未设置时只使用内嵌平台映射。
    #[serde(default, rename = "variable-map")]
    pub variable_map: Option<String>,
    /// 插件版本依赖。
    #[serde(default)]
    pub requires: Requirements,
    /// 配置化模块。
    #[serde(default)]
    pub modules: BTreeMap<String, ModuleDefinition>,
    /// 自动化任务。
    #[serde(default)]
    pub automations: BTreeMap<String, AutomationDefinition>,
}

/// 终端主题配置。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ThemeDefinition {
    /// 内置主题的稳定 ID。
    #[serde(default = "default_theme_id")]
    pub id: String,
}

impl Default for ThemeDefinition {
    fn default() -> Self {
        Self {
            id: default_theme_id(),
        }
    }
}

impl ResolvedConfig {
    /// 返回按 `order` 和模块 ID 稳定排序的已启用模块。
    pub fn enabled_modules(&self) -> Vec<(&str, &ModuleDefinition)> {
        let mut modules: Vec<_> = self
            .modules
            .iter()
            .filter(|(_, module)| module.enabled)
            .map(|(id, module)| (id.as_str(), module))
            .collect();
        modules.sort_by_key(|(id, module)| (module.order, *id));
        modules
    }
}

fn default_theme_id() -> String {
    DEFAULT_THEME_ID.to_owned()
}

#[cfg(test)]
mod tests {
    use crate::{AppConfig, DEFAULT_THEME_ID};

    #[test]
    fn app_theme_defaults_and_loads() {
        let default: AppConfig =
            toml::from_str("schema-version = 2\nrecipe = 'personal'\n").unwrap();
        assert_eq!(default.theme.id, DEFAULT_THEME_ID);

        let custom: AppConfig =
            toml::from_str("schema-version = 2\nrecipe = 'personal'\n[theme]\nid = 'orbit'\n")
                .unwrap();
        assert_eq!(custom.theme.id, "orbit");
    }
}
