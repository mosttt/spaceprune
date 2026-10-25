//! Handler requests, results, and progress data; no execution capabilities.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Current major version of the external handler protocol.
pub const PROTOCOL_VERSION: u32 = 3;

/// Handler 调用结果。
pub type PluginResult<T> = Result<T, PluginError>;

/// Handler 调用失败。
#[derive(Debug, Clone, Error, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[error("{message}")]
pub struct PluginError {
    /// Native authorization result, when this failure came from an authorization attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<crate::access::PermissionOutcome>,
    /// Structured native error, when the failure comes from an OS operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io_failure: Option<crate::access::IoFailure>,
    /// 适合程序判断的稳定错误代码。
    pub code: String,
    /// 面向用户的英文错误说明。
    pub message: String,
}

impl PluginError {
    /// 创建一个插件错误。
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            authorization: None,
            io_failure: None,
            code: code.into(),
            message: message.into(),
        }
    }
}

/// How a handler's completed selections contribute to a human-facing cleanup summary.
#[derive(
    Debug, Clone, Copy, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq,
)]
#[serde(rename_all = "snake_case")]
pub enum CleanupSizeAccounting {
    /// Processed content has a byte estimate; missing estimates remain unknown.
    #[default]
    ProcessedBytes,
    /// Snapshot groups share storage and must be counted separately from byte estimates.
    SnapshotGroups,
}

/// Handler 的静态描述。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct HandlerDescriptor {
    /// Accounting unit for cleanup summaries, independent of raw per-item size evidence.
    #[serde(default)]
    pub size_accounting: CleanupSizeAccounting,
    /// 完整 Handler ID，格式为 `插件 ID/能力 ID`。
    pub id: String,
    /// 面向用户的名称。
    pub name: String,
    /// 能力说明。
    pub description: String,
    /// 支持的平台名称；空数组表示平台中立。
    pub platforms: Vec<String>,
    /// `options` 的 JSON Schema；没有额外配置时为 `null`。
    pub options_schema: Value,
    /// `recommend` 的 JSON Schema；没有额外规则时为 `null`。
    pub recommend_schema: Value,
}

/// 传递给 Handler 的已解析任务。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct TaskRequest {
    /// 所属模块 ID。
    pub module_id: String,
    /// 任务 ID。
    pub task_id: String,
    /// 任务显示名称。
    pub name: String,
    /// 完整 Handler ID。
    pub handler_id: String,
    /// 扫描与执行选项。
    pub options: Value,
    /// 推荐规则。
    pub recommend: Value,
    /// 任务标签。
    pub tags: Vec<String>,
}

/// 清理项的推荐状态。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Recommendation {
    /// 默认勾选，并由 `--clean` 清理。
    Recommended,
    /// 默认不勾选，但用户可以手动选择。
    Optional,
    /// 仅用于展示，不能进入清理计划。
    Unavailable,
}

/// Whether a size concept applies to an observed item, independently of measurement success.
#[derive(
    Debug, Clone, Copy, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq,
)]
#[serde(rename_all = "snake_case")]
pub enum SizeApplicability {
    /// A missing measurement is unknown; a measured zero remains zero.
    #[default]
    Applicable,
    /// The domain explicitly declares this size concept inapplicable.
    NotApplicable,
}

impl SizeApplicability {
    /// Whether ordinary size measurement semantics apply.
    pub const fn is_applicable(&self) -> bool {
        matches!(self, Self::Applicable)
    }
}

/// Handler 扫描产生的一个清理项。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct CleanupItem {
    /// 任务内唯一 ID。
    pub id: String,
    /// 可选的父项 ID，用于构建展示树。
    pub parent_id: Option<String>,
    /// 显示名称。
    pub name: String,
    /// 路径或领域资源的可读位置。
    pub location: String,
    /// Domain applicability of logical size. Older reports default to ordinary measurement.
    #[serde(default, skip_serializing_if = "SizeApplicability::is_applicable")]
    pub logical_size_applicability: SizeApplicability,
    /// 逻辑占用字节数；无法计量时为 `None`。
    pub logical_bytes: Option<u64>,
    /// 预计可释放字节数；无法可靠估计时为 `None`。
    pub reclaimable_bytes: Option<u64>,
    /// Unix 时间戳形式的最后活动时间。
    pub modified_at: Option<u64>,
    /// 当前推荐状态。
    pub recommendation: Recommendation,
    /// 推荐、可选或不可用的原因。
    pub reason: String,
    /// 用于过滤和展示的标签。
    pub tags: Vec<String>,
    /// 由同一 Handler 在清理时解释的私有数据。
    pub opaque: Value,
}

/// 扫描或执行过程中的问题严重程度。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum IssueLevel {
    /// 不阻止其他结果使用的问题。
    Warning,
    /// 当前任务或动作失败。
    Error,
}

/// Handler 报告的一个问题。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct HandlerIssue {
    /// Stable diagnostic code, independent of display text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Structured native failure, used by access retry policies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io_failure: Option<crate::access::IoFailure>,
    /// 问题严重程度。
    pub level: IssueLevel,
    /// 英文问题说明。
    pub message: String,
    /// 可选的路径或资源位置。
    pub location: Option<String>,
}

/// Handler 的扫描结果。
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct HandlerScanResult {
    /// 扫描得到的扁平清理项列表，层级通过 `parent_id` 表达。
    pub items: Vec<CleanupItem>,
    /// 不阻止返回其他清理项的问题。
    pub issues: Vec<HandlerIssue>,
}

mod execution;
pub use execution::{
    HandlerCleanResult, ItemExecution, ItemExecutionStatus, ItemFailure, VolumeSpaceChange,
};

/// 进度阶段。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProgressPhase {
    /// 加载配置或插件。
    Load,
    /// 扫描资源。
    Scan,
    /// 计量容量。
    Measure,
    /// 生成计划。
    Plan,
    /// A system command is requesting authorization or running with elevated access.
    Authorize,
    /// 执行清理。
    Clean,
    /// 当前阶段完成。
    Complete,
}

/// 扫描和清理进度事件。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct ProgressEvent {
    /// 当前阶段。
    pub phase: ProgressPhase,
    /// 可选模块 ID。
    pub module_id: Option<String>,
    /// 可选任务 ID。
    pub task_id: Option<String>,
    /// 可选 Handler ID。
    pub handler_id: Option<String>,
    /// Current count reported by this step; `None` when unknown.
    pub current: Option<usize>,
    /// 总量；未知时为 `None`。
    pub total: Option<usize>,
    /// 英文进度说明。
    pub message: String,
}
