//! Public scan, plan and execution observations; no filesystem or authorization capability.

use crate::{
    access::OperationStopReason,
    handler::{
        CleanupItem, CleanupSizeAccounting, HandlerCleanResult, HandlerIssue, IssueLevel,
        PluginError, Recommendation, TaskRequest,
    },
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A local execution journal selected by the engine, never an executable cleanup capability.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionJournalReference {
    /// Identity of the plan recorded in the journal.
    pub plan_id: String,
    /// Native journal filename retained by a local host for read-only recovery.
    pub path: crate::access::NativePath,
}

/// Confirmed execution evidence recovered without resuming any work.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveredExecution {
    /// True only when a complete final report was committed to the journal.
    pub finished: bool,
    /// Known outcomes and explicit uncertainty for work without a final receipt.
    pub report: ExecutionReport,
}

/// 一次扫描的范围请求。
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct ScanRequest {
    /// 可选模块 ID；`None` 表示全部启用模块。
    pub module: Option<String>,
    /// 模块选择器 ID。
    #[serde(default)]
    pub selectors: BTreeSet<String>,
    /// 只扫描这些任务 ID；空集合表示模块中的全部任务。
    #[serde(default)]
    pub tasks: BTreeSet<String>,
}

/// 扫描报告中的全局清理项键。
#[derive(
    Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct ItemKey {
    /// 模块 ID。
    pub module_id: String,
    /// 任务 ID。
    pub task_id: String,
    /// Handler 返回的任务内项目 ID。
    pub item_id: String,
}

impl std::fmt::Display for ItemKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}::{}::{}",
            self.module_id, self.task_id, self.item_id
        )
    }
}

/// 一个任务的扫描报告。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct TaskReport {
    /// 模块 ID。
    pub module_id: String,
    /// 任务 ID。
    pub task_id: String,
    /// 任务显示名称。
    pub name: String,
    /// Handler ID。
    pub handler_id: String,
    /// 传递给 Handler 的完整任务请求。
    pub request: TaskRequest,
    /// 扫描得到的清理项。
    pub items: Vec<CleanupItem>,
    /// Handler 报告的警告或错误级问题。
    pub issues: Vec<HandlerIssue>,
    /// 阻止当前任务扫描的错误。
    pub error: Option<String>,
}

impl TaskReport {
    /// 返回所有叶子清理项。
    pub fn leaf_items(&self) -> Vec<&CleanupItem> {
        let parents: BTreeSet<_> = self
            .items
            .iter()
            .filter_map(|item| item.parent_id.as_deref())
            .collect();
        self.items
            .iter()
            .filter(|item| !parents.contains(item.id.as_str()))
            .collect()
    }

    /// Aggregate applicable logical sizes and explicit inapplicability across leaf items.
    pub fn logical_size_summary(&self) -> crate::logical_size::LogicalSizeSummary {
        crate::logical_size::LogicalSizeSummary::from_items(self.leaf_items())
    }

    /// 返回叶子项目逻辑占用之和。
    pub fn logical_bytes(&self) -> u64 {
        self.logical_size_summary().known_bytes
    }

    /// Return applicable leaf items whose logical size is unknown.
    pub fn unknown_logical_items(&self) -> usize {
        self.logical_size_summary().unknown_items
    }

    /// Return leaf items whose domain explicitly marks logical size as inapplicable.
    pub fn not_applicable_logical_items(&self) -> usize {
        self.logical_size_summary().not_applicable_items
    }

    /// 返回推荐叶子项目的预计可释放空间。
    pub fn recommended_bytes(&self) -> u64 {
        self.leaf_items()
            .into_iter()
            .filter(|item| item.recommendation == Recommendation::Recommended)
            .filter_map(|item| item.reclaimable_bytes)
            .fold(0, u64::saturating_add)
    }

    /// 返回预计可释放容量未知的推荐叶子项目数。
    pub fn unknown_recommended_items(&self) -> usize {
        self.leaf_items()
            .into_iter()
            .filter(|item| item.recommendation == Recommendation::Recommended)
            .filter(|item| item.reclaimable_bytes.is_none())
            .count()
    }
}

/// 一个模块的扫描报告。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct ModuleReport {
    /// 模块 ID。
    pub id: String,
    /// 模块显示名称。
    pub name: String,
    /// 模块说明。
    pub description: String,
    /// 任务报告。
    pub tasks: Vec<TaskReport>,
}

impl ModuleReport {
    /// Aggregate logical sizes across every task's leaf items.
    pub fn logical_size_summary(&self) -> crate::logical_size::LogicalSizeSummary {
        crate::logical_size::LogicalSizeSummary::from_items(
            self.tasks.iter().flat_map(TaskReport::leaf_items),
        )
    }

    /// 返回模块所有叶子项目的逻辑占用。
    pub fn logical_bytes(&self) -> u64 {
        self.logical_size_summary().known_bytes
    }

    /// Return applicable leaf items whose logical size is unknown.
    pub fn unknown_logical_items(&self) -> usize {
        self.logical_size_summary().unknown_items
    }

    /// Return leaf items whose logical size is not applicable.
    pub fn not_applicable_logical_items(&self) -> usize {
        self.logical_size_summary().not_applicable_items
    }

    /// 返回模块推荐项目的预计可释放空间。
    pub fn recommended_bytes(&self) -> u64 {
        self.tasks
            .iter()
            .map(TaskReport::recommended_bytes)
            .fold(0, u64::saturating_add)
    }

    /// 返回模块内预计可释放容量未知的推荐项目数。
    pub fn unknown_recommended_items(&self) -> usize {
        self.tasks
            .iter()
            .map(TaskReport::unknown_recommended_items)
            .sum()
    }
}

/// 一次 SpacePrune 扫描的完整报告。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct ScanReport {
    /// Unique identity of this observed scan, preserved in snapshots.
    pub scan_id: String,
    /// 清理方案名称。
    pub config_name: String,
    /// 配置来源。
    pub config_source: String,
    /// 实际扫描的模块。
    pub modules: Vec<ModuleReport>,
    /// 是否因协作式取消提前结束。
    pub cancelled: bool,
    /// Cause of an incomplete operation; absent in older serialized reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<OperationStopReason>,
}

impl ScanReport {
    /// 返回全部推荐叶子项目键。
    pub fn recommended_keys(&self) -> BTreeSet<ItemKey> {
        self.items_with_keys()
            .into_iter()
            .filter(|(_, item)| item.recommendation == Recommendation::Recommended)
            .map(|(key, _)| key)
            .collect()
    }

    /// 返回全部可以手动选择的叶子项目键。
    pub fn selectable_keys(&self) -> BTreeSet<ItemKey> {
        self.items_with_keys()
            .into_iter()
            .filter(|(_, item)| item.recommendation != Recommendation::Unavailable)
            .map(|(key, _)| key)
            .collect()
    }

    /// 返回报告中是否存在任务级错误或 error 级问题。
    pub fn has_errors(&self) -> bool {
        self.modules
            .iter()
            .flat_map(|module| &module.tasks)
            .any(|task| {
                task.error.is_some()
                    || task
                        .issues
                        .iter()
                        .any(|issue| issue.level == IssueLevel::Error)
            })
    }

    /// 返回全部叶子项目及其全局键。
    pub fn items_with_keys(&self) -> Vec<(ItemKey, &CleanupItem)> {
        let mut result = Vec::new();
        for module in &self.modules {
            for task in &module.tasks {
                result.extend(task.leaf_items().into_iter().map(|item| {
                    (
                        ItemKey {
                            module_id: module.id.clone(),
                            task_id: task.task_id.clone(),
                            item_id: item.id.clone(),
                        },
                        item,
                    )
                }));
            }
        }
        result
    }
}

/// 清理计划中交给一个 Handler 的项目组。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct PlanGroup {
    /// 模块 ID。
    pub module_id: String,
    /// 任务 ID。
    pub task_id: String,
    /// Handler ID。
    pub handler_id: String,
    /// 原始任务请求。
    pub request: TaskRequest,
    /// 要清理的叶子项目。
    pub items: Vec<CleanupItem>,
}

/// Serializable plan observation. It cannot authorize or execute cleanup.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
pub struct CleanupPlan {
    /// Identity of the frozen plan.
    pub plan_id: String,
    /// Identity of the scan that issued this plan.
    pub scan_id: String,
    /// Digest of the selected task definitions at planning time.
    pub recipe_revision: String,
    /// Digest of the exact selection, rules and scan identity.
    pub digest: String,
    /// 清理方案名称。
    pub config_name: String,
    /// 按 Handler 任务分组的清理项目。
    pub groups: Vec<PlanGroup>,
}

impl CleanupPlan {
    /// 返回计划中的清理项数量。
    pub fn item_count(&self) -> usize {
        self.groups
            .iter()
            .map(|group| group.items.len())
            .fold(0, usize::saturating_add)
    }

    /// 返回计划中已知的预计可释放空间。
    pub fn reclaimable_bytes(&self) -> u64 {
        self.groups
            .iter()
            .flat_map(|group| &group.items)
            .filter_map(|item| item.reclaimable_bytes)
            .fold(0, u64::saturating_add)
    }

    /// 判断计划是否为空。
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
}

/// 一个计划组的执行报告。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct GroupExecutionReport {
    /// Original structured handler failure; `error` retains the existing display text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<crate::handler::PluginError>,
    /// Registered handler accounting, retained with the report for later display.
    #[serde(default)]
    pub size_accounting: CleanupSizeAccounting,
    /// 模块 ID。
    pub module_id: String,
    /// 任务 ID。
    pub task_id: String,
    /// Handler ID。
    pub handler_id: String,
    /// Handler 返回的清理结果；调用整体失败时为 `None`。
    pub result: Option<HandlerCleanResult>,
    /// Handler 调用整体失败说明。
    pub error: Option<String>,
}

/// 一次清理计划的执行报告。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct ExecutionReport {
    /// Operation-wide finalization failures, separate from already committed item/group results.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operation_failures: Vec<PluginError>,
    /// 各任务组执行结果。
    pub groups: Vec<GroupExecutionReport>,
    /// 是否因协作式取消提前结束。
    pub cancelled: bool,
    /// Cause of interrupted execution, preserving completed per-item effects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<OperationStopReason>,
}

impl ExecutionReport {
    /// 返回是否存在整体或单项失败。
    pub fn has_errors(&self) -> bool {
        !self.operation_failures.is_empty()
            || self.groups.iter().any(|group| {
                group.error.is_some()
                    || group
                        .result
                        .as_ref()
                        .is_some_and(|result| !result.failures.is_empty())
            })
    }

    /// Count operation failures plus item failures, using a task error only when no item failure exists.
    pub fn failure_count(&self) -> usize {
        self.groups
            .iter()
            .map(|group| {
                group
                    .result
                    .as_ref()
                    .map_or(0, |result| result.failures.len())
                    .max(usize::from(group.error.is_some()))
            })
            .fold(self.operation_failures.len(), usize::saturating_add)
    }

    /// 返回成功清理的项目总数。
    pub fn succeeded(&self) -> usize {
        self.groups
            .iter()
            .filter_map(|group| group.result.as_ref())
            .map(|result| result.succeeded)
            .fold(0, usize::saturating_add)
    }

    /// Return the known processed-size estimate, not measured freed disk space.
    pub fn reclaimed_bytes(&self) -> u64 {
        self.groups
            .iter()
            .filter_map(|group| group.result.as_ref())
            .map(|result| result.reclaimed_bytes)
            .fold(0, u64::saturating_add)
    }

    /// Return the number of successful items with unknown processed-size estimates.
    pub fn unknown_reclaimed_items(&self) -> usize {
        self.groups
            .iter()
            .filter_map(|group| group.result.as_ref())
            .map(HandlerCleanResult::unknown_reclaimed_items)
            .fold(0, usize::saturating_add)
    }
}
