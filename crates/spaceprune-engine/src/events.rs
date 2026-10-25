//! 扫描与清理生命周期事件。
//!
//! 事件总线同步、按订阅 ID 顺序分发。监听器应快速返回；耗时工作应交给自己的队列。

use std::{
    collections::BTreeMap,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{OperationContext, ScanRequest};
use spaceprune_access::OperationIdentity;

/// 一次扫描或清理操作在事件流中的稳定 ID。
pub type OperationId = u64;

/// 监听器在事件总线中的注册 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventListenerId(u64);

/// SpacePrune 稳定的生命周期事件。
///
/// 事件使用面向外部集成的 kebab-case JSON 类型名。新增事件不会改变已有事件字段的语义。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SpacePruneEvent {
    /// 扫描请求即将开始。
    ScanStarted {
        /// Exact number of enabled task checks in the resolved request; absent when validation fails.
        #[serde(default)]
        task_count: Option<usize>,
        /// 本次操作 ID。
        operation_id: OperationId,
        /// 请求的模块和选择器范围。
        request: ScanRequest,
    },
    /// 一个任务即将调用 Handler 扫描。
    TaskScanStarted {
        /// 所属扫描操作 ID。
        operation_id: OperationId,
        /// 模块 ID。
        module_id: String,
        /// 任务 ID。
        task_id: String,
        /// Handler ID。
        handler_id: String,
    },
    /// 一个任务已经完成 Handler 扫描。
    TaskScanFinished {
        /// 所属扫描操作 ID。
        operation_id: OperationId,
        /// 模块 ID。
        module_id: String,
        /// 任务 ID。
        task_id: String,
        /// Handler ID。
        handler_id: String,
        /// 返回的清理项数量。
        item_count: usize,
        /// 返回的警告或错误级问题数量。
        issue_count: usize,
        /// 任务级错误。
        error: Option<String>,
    },
    /// 一次扫描已经结束。
    ScanFinished {
        /// 本次操作 ID。
        operation_id: OperationId,
        /// 返回的模块数量。
        module_count: usize,
        /// 返回的任务数量。
        task_count: usize,
        /// 返回的清理项数量。
        item_count: usize,
        /// 返回的警告或错误级问题数量。
        issue_count: usize,
        /// 任务级或扫描级错误数量。
        error_count: usize,
        /// 是否因协作式取消提前结束。
        cancelled: bool,
        /// Distinguish cancellation from deadline expiry; older events omit this field.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<spaceprune_access::OperationStopReason>,
        /// 阻止扫描生成报告的错误。
        error: Option<String>,
    },
    /// 一个清理计划即将开始执行。
    CleanupStarted {
        /// 本次操作 ID。
        operation_id: OperationId,
        /// 计划中的任务组数量。
        group_count: usize,
        /// 计划中的叶子清理项数量。
        item_count: usize,
    },
    /// 一个任务组即将调用 Handler 清理。
    CleanupGroupStarted {
        /// 所属清理操作 ID。
        operation_id: OperationId,
        /// 模块 ID。
        module_id: String,
        /// 任务 ID。
        task_id: String,
        /// Handler ID。
        handler_id: String,
        /// 任务组中的项目数量。
        item_count: usize,
    },
    /// 一个任务组已经完成 Handler 清理。
    CleanupGroupFinished {
        /// 所属清理操作 ID。
        operation_id: OperationId,
        /// 模块 ID。
        module_id: String,
        /// 任务 ID。
        task_id: String,
        /// Handler ID。
        handler_id: String,
        /// Handler 尝试的项目数量。
        attempted: usize,
        /// 成功的项目数量。
        succeeded: usize,
        /// 单项失败数量。
        failure_count: usize,
        /// Known processed-size estimate, not measured freed disk space.
        reclaimed_bytes: u64,
        /// Successful items with unknown size; absent in older event records.
        #[serde(default)]
        unknown_reclaimed_items: Option<usize>,
        /// 任务组整体错误。
        error: Option<String>,
    },
    /// 一个清理计划已经结束执行。
    CleanupFinished {
        /// 本次操作 ID。
        operation_id: OperationId,
        /// 实际产生报告的任务组数量。
        group_count: usize,
        /// Handler 尝试的项目数量。
        attempted: usize,
        /// 成功的项目数量。
        succeeded: usize,
        /// 单项或任务组整体失败数量。
        failure_count: usize,
        /// Known processed-size estimate, not measured freed disk space.
        reclaimed_bytes: u64,
        /// Successful items with unknown size; absent in older event records.
        #[serde(default)]
        unknown_reclaimed_items: Option<usize>,
        /// 是否因协作式取消提前结束。
        cancelled: bool,
        /// Distinguish cancellation from deadline expiry; older events omit this field.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<spaceprune_access::OperationStopReason>,
    },
}

/// 带有全局顺序和时间的事件信封。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventEnvelope {
    /// Host operation identity shared with native work; absent only in older event records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<OperationIdentity>,
    /// 当前事件总线内从一开始递增的顺序号。
    pub sequence: u64,
    /// 事件产生时的 Unix 毫秒时间戳。
    pub emitted_at_ms: u64,
    /// 生命周期事件正文。
    pub event: SpacePruneEvent,
}

/// 接收生命周期事件的监听器。
pub trait EventListener: Send + Sync {
    /// 同步接收一个只读事件。
    fn on_event(&self, event: &EventEnvelope);
}

impl<F> EventListener for F
where
    F: Fn(&EventEnvelope) + Send + Sync,
{
    fn on_event(&self, event: &EventEnvelope) {
        self(event);
    }
}

struct EventBusInner {
    listeners: RwLock<BTreeMap<EventListenerId, Arc<dyn EventListener>>>,
    next_listener_id: AtomicU64,
    next_sequence: AtomicU64,
    next_operation_id: AtomicU64,
}

/// 可克隆、线程安全的生命周期事件总线。
///
/// 克隆值共享监听器和计数器。分发使用监听器快照，因此监听器在回调中订阅或取消订阅
/// 只会影响下一次事件。
#[derive(Clone)]
pub struct EventBus {
    inner: Arc<EventBusInner>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self {
            inner: Arc::new(EventBusInner {
                listeners: RwLock::new(BTreeMap::new()),
                next_listener_id: AtomicU64::new(1),
                next_sequence: AtomicU64::new(1),
                next_operation_id: AtomicU64::new(1),
            }),
        }
    }
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let listener_count = self
            .inner
            .listeners
            .read()
            .expect("event listener lock poisoned")
            .len();
        formatter
            .debug_struct("EventBus")
            .field("listener_count", &listener_count)
            .finish_non_exhaustive()
    }
}

impl EventBus {
    /// 创建没有监听器的新事件总线。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册监听器，并返回用于取消订阅的 ID。
    pub fn subscribe(&self, listener: Arc<dyn EventListener>) -> EventListenerId {
        let id = EventListenerId(self.inner.next_listener_id.fetch_add(1, Ordering::Relaxed));
        self.inner
            .listeners
            .write()
            .expect("event listener lock poisoned")
            .insert(id, listener);
        id
    }

    /// 直接用闭包注册监听器。
    pub fn subscribe_fn<F>(&self, listener: F) -> EventListenerId
    where
        F: Fn(&EventEnvelope) + Send + Sync + 'static,
    {
        self.subscribe(Arc::new(listener))
    }

    /// 取消监听器；返回该 ID 是否存在。
    pub fn unsubscribe(&self, id: EventListenerId) -> bool {
        self.inner
            .listeners
            .write()
            .expect("event listener lock poisoned")
            .remove(&id)
            .is_some()
    }

    pub(crate) fn next_operation_id(&self) -> OperationId {
        self.inner.next_operation_id.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn emit(&self, context: &OperationContext, event: SpacePruneEvent) {
        let envelope = EventEnvelope {
            operation: Some(context.identity()),
            sequence: self.inner.next_sequence.fetch_add(1, Ordering::Relaxed),
            emitted_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| {
                    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
                }),
            event,
        };
        let listeners = self
            .inner
            .listeners
            .read()
            .expect("event listener lock poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for listener in listeners {
            listener.on_event(&envelope);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{EventBus, SpacePruneEvent};
    use crate::OperationContext;

    #[test]
    fn dispatches_in_sequence_and_supports_unsubscribe() {
        let bus = EventBus::new();
        let received = Arc::new(Mutex::new(Vec::new()));
        let target = Arc::clone(&received);
        let listener = bus.subscribe_fn(move |event| {
            target
                .lock()
                .expect("test event lock poisoned")
                .push(event.sequence);
        });
        bus.emit(
            &OperationContext::default(),
            SpacePruneEvent::CleanupStarted {
                operation_id: 1,
                group_count: 0,
                item_count: 0,
            },
        );
        assert!(bus.unsubscribe(listener));
        bus.emit(
            &OperationContext::default(),
            SpacePruneEvent::CleanupFinished {
                operation_id: 1,
                group_count: 0,
                attempted: 0,
                succeeded: 0,
                failure_count: 0,
                reclaimed_bytes: 0,
                unknown_reclaimed_items: Some(0),
                cancelled: false,
                stop_reason: None,
            },
        );
        assert_eq!(*received.lock().expect("test event lock poisoned"), vec![1]);
    }

    #[test]
    fn serializes_stable_kebab_case_event_types() {
        let value = serde_json::to_value(SpacePruneEvent::CleanupStarted {
            operation_id: 7,
            group_count: 2,
            item_count: 4,
        })
        .unwrap();
        assert_eq!(value["type"], "cleanup-started");
        assert_eq!(value["operation_id"], 7);
    }
}
