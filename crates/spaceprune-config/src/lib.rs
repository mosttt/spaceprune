#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! SpacePrune 配置发现、include 解析和确定性合并。

mod bundle_edit;
mod language;
mod loader;
mod model;
mod parse;
mod paths;
mod resolve;
mod roots;
mod store;
mod validate;

pub use bundle_edit::{rename_recipe_bundle, update_bundle_automation};
pub use spaceprune_contracts::{
    bundle::RecipeBundle,
    config::{
        AutomationDefinition, DailyTime, ModuleDefinition, Requirements, SCHEMA_VERSION,
        SelectorDefinition, TaskDefinition,
    },
};
pub use store::{ConfigStore, ConfigStoreError, RecipeInfo};

pub use language::{Language, LanguagePreference, LanguageSettings};
pub use loader::ConfigLoader;
pub use model::{AppConfig, ConfigError, ResolvedConfig, ThemeDefinition};
pub use paths::{user_config_directory, user_config_path, user_config_paths};
pub use validate::is_valid_id;

/// Shared display-language management command.
pub const LANGUAGE_COMMAND_ID: &str = "language";

/// SpacePrune 应用入口配置文件名。
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// 用户主目录中的 SpacePrune 配置目录名。
pub const CONFIG_DIRECTORY_NAME: &str = "spaceprune";

/// 本地清理方案的统一目录名。
pub const RECIPE_DIRECTORY_NAME: &str = "recipes";

/// 首次创建配置时使用的默认清理方案目录名。
pub const DEFAULT_RECIPE_DIRECTORY_NAME: &str = "default";

/// 默认本地清理方案入口文件名。
pub const DEFAULT_RECIPE_FILE_NAME: &str = "default.toml";

/// The built-in terminal theme used when no preference is configured.
pub const DEFAULT_THEME_ID: &str = "aurora";

/// Terminal palettes share desktop theme IDs, but are configured independently.
pub const THEME_IDS: [&str; 4] = ["aurora", "orbit", "graphite", "forest"];

/// Resolve saved legacy preferences without rewriting the configuration.
fn resolve_theme_id(id: &str) -> Option<&'static str> {
    match id {
        "tokyo-night" | "nord" => Some("aurora"),
        "catppuccin-mocha" | "dracula" | "monokai" => Some("orbit"),
        "vscode-dark-modern" | "vscode-light-modern" | "quiet-light" => Some("graphite"),
        "solarized-dark" => Some("forest"),
        _ => THEME_IDS.iter().copied().find(|candidate| *candidate == id),
    }
}

/// 模块管理命令的保留 ID。
pub const MODULE_COMMAND_ID: &str = "module";

/// 插件管理命令的保留 ID。
pub const PLUGIN_COMMAND_ID: &str = "plugin";

/// 自动化管理命令的保留 ID。
pub const AUTOMATION_COMMAND_ID: &str = "automation";

/// 主题管理命令的保留 ID。
pub const THEME_COMMAND_ID: &str = "theme";

/// 清理方案管理命令的保留 ID。
pub const RECIPE_COMMAND_ID: &str = "recipe";

/// Reserved command ID for native authorization setup.
pub const AUTHORIZATION_COMMAND_ID: &str = "authorization";

/// 固定只读扫描命令的保留 ID。
pub const SCAN_COMMAND_ID: &str = "scan";

/// 固定推荐清理命令的保留 ID。
pub const CLEAN_COMMAND_ID: &str = "clean";

/// 不能用作配置模块 ID 的管理命令集合。
pub const MANAGEMENT_COMMAND_IDS: [&str; 9] = [
    AUTHORIZATION_COMMAND_ID,
    LANGUAGE_COMMAND_ID,
    MODULE_COMMAND_ID,
    PLUGIN_COMMAND_ID,
    AUTOMATION_COMMAND_ID,
    THEME_COMMAND_ID,
    RECIPE_COMMAND_ID,
    SCAN_COMMAND_ID,
    CLEAN_COMMAND_ID,
];

/// 不能用作动态选择器 ID 的固定长选项集合。
pub const RESERVED_SELECTOR_IDS: [&str; 9] = [
    "config",
    "scan",
    "clean",
    "select",
    "yes",
    "output",
    "no-progress",
    "help",
    "version",
];

fn supported_platform(platform: &str) -> bool {
    matches!(platform, "linux" | "macos" | "windows" | "android" | "ios")
}

/// 返回当前编译目标的平台名称。
pub const fn current_platform() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "android") {
        "android"
    } else if cfg!(target_os = "ios") {
        "ios"
    } else {
        "unknown"
    }
}

/// Persistent rule declaration types, shared with non-executing consumers.
pub use spaceprune_contracts::{
    config::RecipeDefinition,
    rule::{FilesystemRoot, ObjectBinding, RecipeUsage, RootSpec, RuleScope},
};
