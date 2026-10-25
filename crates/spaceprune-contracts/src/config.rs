//! Persistent recipe components independent of include resolution and the host.

use crate::messages;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use std::{
    collections::BTreeMap,
    fmt::{self, Display},
};

/// Current public TOML configuration schema version.
pub const SCHEMA_VERSION: u32 = 2;

/// An unresolved, complete recipe declaration. Loading includes and expanding paths are host work.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RecipeDefinition {
    /// Public persistent schema version, distinct from any tool or provider protocol.
    #[serde(rename = "schema-version")]
    #[schemars(schema_with = "crate::schema::configuration_version_schema")]
    pub schema_version: u32,
    /// Human-readable recipe name.
    pub name: String,
    /// Recurring rule or explicit single-use selection.
    #[serde(default)]
    pub usage: crate::rule::RecipeUsage,
    /// Source variable map, before platform and user expansion.
    #[serde(default, rename = "variable-map")]
    pub variable_map: Option<String>,
    /// Includes remain source declarations; inactive platforms are not read by the resolver.
    #[serde(default)]
    pub include: Vec<IncludeReference>,
    /// Required handler providers and versions.
    #[serde(default)]
    pub requires: Requirements,
    /// Original modules, selectors and tasks; no flattening into a device-only rule list.
    #[serde(default)]
    pub modules: BTreeMap<String, ModuleDefinition>,
    /// User-configured recurring automation.
    #[serde(default)]
    pub automations: BTreeMap<String, AutomationDefinition>,
}

/// Source include syntax shared by schemas and configuration consumers.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(untagged, deny_unknown_fields)]
pub enum IncludeReference {
    /// An unconditional source include.
    Path(String),
    /// A platform-dependent include, skipped before any path access when inactive.
    Conditional {
        /// Source reference.
        path: String,
        /// Empty means all platforms.
        #[serde(default)]
        platforms: Vec<String>,
    },
}

/// 每日自动化使用的本地时间。
///
/// 配置格式固定为零填充的 `HH:MM`，例如 `03:05`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DailyTime {
    hour: u8,
    minute: u8,
}

impl DailyTime {
    /// 默认执行时间 `03:00`。
    pub const DEFAULT: Self = Self { hour: 3, minute: 0 };

    /// 返回小时部分。
    pub const fn hour(self) -> u8 {
        self.hour
    }

    /// 返回分钟部分。
    pub const fn minute(self) -> u8 {
        self.minute
    }

    fn parse(value: &str) -> Option<Self> {
        let (hour, minute) = value.split_once(':')?;
        if hour.len() != 2 || minute.len() != 2 {
            return None;
        }
        let hour = hour.parse().ok()?;
        let minute = minute.parse().ok()?;
        (hour < 24 && minute < 60).then_some(Self { hour, minute })
    }
}

impl Default for DailyTime {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl JsonSchema for DailyTime {
    fn schema_name() -> String {
        "DailyTime".to_owned()
    }
    fn json_schema(generator: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = <String>::json_schema(generator).into_object();
        schema.string().pattern = Some("^([01][0-9]|2[0-3]):[0-5][0-9]$".to_owned());
        schema.into()
    }
}

impl Display for DailyTime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:02}:{:02}", self.hour, self.minute)
    }
}

impl Serialize for DailyTime {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for DailyTime {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| D::Error::custom(messages::text("config.daily_time")))
    }
}

/// 清理方案依赖。
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Requirements {
    /// 插件 ID 到版本要求的映射。
    #[serde(default)]
    pub plugins: BTreeMap<String, String>,
}

/// 一个配置化模块。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModuleDefinition {
    /// 显示名称。
    pub name: String,
    /// 模块说明。
    #[serde(default)]
    pub description: String,
    /// 显示顺序。
    #[serde(default)]
    pub order: i32,
    /// 是否启用。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 配置化布尔选择器。
    #[serde(default)]
    pub selectors: BTreeMap<String, SelectorDefinition>,
    /// 模块任务。
    #[serde(default)]
    pub tasks: BTreeMap<String, TaskDefinition>,
}

/// 模块的命令行选择器。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SelectorDefinition {
    /// 选择器显示名称。
    pub name: String,
    /// 任务必须包含的全部标签。
    #[serde(default, rename = "match-tags")]
    pub match_tags: Vec<String>,
    /// 是否启用选择器。
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// 一个 Handler 任务。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TaskDefinition {
    /// 任务显示名称。
    pub name: String,
    /// 任务说明。
    #[serde(default)]
    pub description: String,
    /// 完整 Handler ID。
    pub handler: String,
    /// 是否启用。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 模块内显示顺序。
    #[serde(default)]
    pub order: i32,
    /// 用于选择器过滤的标签。
    #[serde(default)]
    pub tags: Vec<String>,
    /// 允许执行该任务的平台；空数组表示不额外限制。
    #[serde(default)]
    pub platforms: Vec<String>,
    /// Handler 扫描和清理选项。
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, serde_json::Value>")]
    pub options: toml::Table,
    /// Handler 推荐规则。
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, serde_json::Value>")]
    pub recommend: toml::Table,
}

/// 一个配置化自动化任务。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AutomationDefinition {
    /// 显示名称。
    pub name: String,
    /// 是否启用。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 要扫描的模块；空数组表示全部模块。
    #[serde(default)]
    pub modules: Vec<String>,
    /// 每日执行的本地时间。
    #[serde(default)]
    pub at: DailyTime,
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_time_uses_a_strict_zero_padded_format() {
        #[derive(serde::Deserialize)]
        struct Value {
            value: DailyTime,
        }

        let time = toml::from_str::<Value>("value = '03:05'").unwrap().value;
        assert_eq!(time.hour(), 3);
        assert_eq!(time.minute(), 5);
        assert_eq!(time.to_string(), "03:05");
        assert!(DailyTime::parse("3:05").is_none());
        assert!(DailyTime::parse("24:00").is_none());
        assert!(DailyTime::parse("03:60").is_none());
    }
}
