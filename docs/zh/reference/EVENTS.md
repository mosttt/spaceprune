# SpacePrune 事件系统

本文描述 `spaceprune-engine` 的生命周期事件总线。库调用方、日志、审计和指标组件可以在不侵入 Handler 的前提下
观察扫描与清理流程。

## 定位

SpacePrune 有两类回调，语义不同：

| 机制                             | 用途                       | 稳定性   | 频率               |
| -------------------------------- | -------------------------- | -------- | ------------------ |
| `ProgressSink` / `ProgressEvent` | 进度条、当前文件、终端提示 | 展示协议 | 可以很高           |
| `EventBus` / `SpacePruneEvent`   | 生命周期、审计和指标       | 集成协议 | 每个操作或任务一次 |

不要从进度文本推断生命周期。产品展示文本随语言设置变化，第三方插件与系统原始输出保留原文；事件类型和字段才是监听器应依赖的稳定输入。

事件总线是通知系统，不允许监听器修改扫描报告、清理计划或 Handler 返回值，也不应通过监听器副作用改变
执行器状态。

## 公共 API

底层类型位于 `spaceprune_engine::events`，也从 `spaceprune_engine` 根模块重新导出；应用宿主还可通过
`spaceprune::api` 直接取得 `EventBus` 与 `SpacePruneEvent`：

- `EventBus`：可克隆、线程安全的订阅与分发入口；
- `EventListener`：`Send + Sync` 的监听器 trait；
- `EventListenerId`：取消订阅所需的注册 ID；
- `EventEnvelope`：顺序号、时间戳和事件正文；
- `SpacePruneEvent`：稳定生命周期事件枚举；
- `OperationId`：一次扫描或清理的总线内操作 ID。

```rust,no_run
use spaceprune_engine::{EventBus, SpacePruneEvent};

let events = EventBus::new();
let listener_id = events.subscribe_fn(|envelope| {
    if let SpacePruneEvent::CleanupFinished {
        succeeded,
        reclaimed_bytes,
        unknown_reclaimed_items,
        ..
    } = &envelope.event
    {
        eprintln!(
            "cleanup finished: {succeeded} items, known size estimate {reclaimed_bytes} bytes, {} unknown sizes",
            unknown_reclaimed_items.unwrap_or(*succeeded)
        );
    }
});

// Size estimates describe processed items, not measured free-space changes.

// 将同一个总线交给 EngineBuilder：
// let engine = EngineBuilder::new(config).with_event_bus(events.clone()).build()?;

assert!(events.unsubscribe(listener_id));
```

闭包版本通常最方便。需要携带复杂状态或复用实现时，可以实现 `EventListener` 并通过
`subscribe(Arc<dyn EventListener>)` 注册。

## 事件清单

事件 JSON 使用带 `type` 的 tagged enum，类型名固定为 kebab-case。

| JSON `type`              | 触发时机                             | 关键字段                             |
| ------------------------ | ------------------------------------ | ------------------------------------ |
| `scan-started`           | 解析扫描范围前                       | `operation_id`, `request`            |
| `task-scan-started`      | 调用一个任务的 `Handler::scan` 前    | 模块、任务、Handler ID               |
| `task-scan-finished`     | Handler 扫描返回或产生任务错误后     | 项目数、问题数、错误                 |
| `scan-finished`          | 扫描成功、失败或取消后               | 模块/任务/项目/问题/错误统计         |
| `cleanup-started`        | 开始执行清理计划前                   | 任务组数、项目数                     |
| `cleanup-group-started`  | 调用一个任务组的 `Handler::clean` 前 | 模块、任务、Handler ID、项目数       |
| `cleanup-group-finished` | Handler 清理返回或任务组失败后       | 尝试数、成功数、失败数、释放量、错误 |
| `cleanup-finished`       | 清理计划成功、失败或取消后           | 汇总统计和取消状态                   |

扫描和清理各自获得新的 `operation_id`。同一次 `scan` 调用内的任务事件引用扫描 ID；同一次 `execute`
调用内的任务组事件引用清理 ID。

## EventEnvelope

每个事件由以下信封包装：

```json
{
  "sequence": 12,
  "emitted_at_ms": 1785888000123,
  "event": {
    "type": "cleanup-started",
    "operation_id": 4,
    "group_count": 2,
    "item_count": 9
  }
}
```

- `sequence` 在同一个 `EventBus` 内从 1 单调递增；
- `emitted_at_ms` 是 Unix 毫秒时间戳；
- `event` 是 `SpacePruneEvent` 正文；
- 克隆 `EventBus` 不会创建新序列，它们共享监听器和计数器；
- 新建另一个 `EventBus` 会建立独立序列空间。

## 顺序保证

一次正常扫描的最小顺序是：

```text
scan-started
  task-scan-started
  task-scan-finished
  ...
scan-finished
```

一次正常清理的最小顺序是：

```text
cleanup-started
  cleanup-group-started
  cleanup-group-finished
  ...
cleanup-finished
```

任务按模块 `order`、模块 ID、任务 `order`、任务 ID 的既有确定性顺序执行。监听器按
`EventListenerId` 注册顺序调用。

分发前会取得监听器快照。因此，监听器在自己的回调中订阅或取消订阅只影响下一条事件，不改变当前事件
的接收集合。

## 错误与取消

- Handler 的扫描错误会写入 `task-scan-finished.error`，扫描仍可继续其他任务；
- error 级 Handler issue 会计入 `scan-finished.error_count`；
- 阻止生成扫描报告的核心错误写入 `scan-finished.error`；
- Handler 整体清理错误写入 `cleanup-group-finished.error`；
- 单项清理失败计入 `failure_count`；
- 协作式取消由 `cancelled = true` 表示；
- 取消发生在尚未开始的任务或任务组之前时，不会生成对应的 started/finished 对。

监听器本身没有错误返回值。它抛出的 panic 会像普通 Rust 回调 panic 一样中断调用方；事件总线不会吞掉
panic。调用方应保证监听器可靠。

## 同步模型

事件在执行扫描或清理的线程中同步分发。这带来两个直接性质：

1. 顺序与执行顺序一致，不需要在事件中猜测异步重排；
2. 慢监听器会直接拖慢扫描或清理。

因此推荐把监听器分成两类：

- 很快的内存状态更新、指标计数或 channel send：直接同步执行；
- 文件写入或其他慢操作：监听器只向有界队列发送，实际工作由后台线程执行。

需要阻断语义的调用方必须在调用 `Engine::execute` 前自行完成策略判断；EventBus 监听器不承担该职责。

事件系统已有单元测试覆盖订阅、取消订阅、顺序、核心生命周期顺序和 kebab-case JSON 类型名。
