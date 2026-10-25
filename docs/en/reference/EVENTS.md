# SpacePrune events

This guide describes the lifecycle event bus in `spaceprune-engine`. Library callers, logging, auditing, and metrics can observe scanning and cleanup without modifying handlers.

## Purpose

SpacePrune has two callback mechanisms with different meanings:

| Mechanism | Purpose | Contract | Frequency |
| --- | --- | --- | --- |
| `ProgressSink` / `ProgressEvent` | Progress bars, current file, terminal prompts | Presentation protocol | Potentially high |
| `EventBus` / `SpacePruneEvent` | Lifecycle, auditing, metrics | Integration protocol | Once per operation or task |

Do not infer lifecycle from progress text. Product display text follows the selected language; third-party plugin and raw system output remain unchanged. Event types and fields are the stable inputs listeners should use.

The event bus is a notification mechanism. Listeners cannot modify scan reports, cleanup plans, or handler return values, and should not change executor state through side effects.

## Public API

Low-level types live in `spaceprune_engine::events` and are re-exported from the `spaceprune_engine` root. Application hosts can also obtain `EventBus` and `SpacePruneEvent` through `spaceprune::api`:

- `EventBus`: cloneable, thread-safe subscription and dispatch entry point.
- `EventListener`: `Send + Sync` listener trait.
- `EventListenerId`: registration ID used to unsubscribe.
- `EventEnvelope`: sequence number, timestamp, and event body.
- `SpacePruneEvent`: stable lifecycle event enum.
- `OperationId`: bus-local ID for one scan or cleanup.

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

// Pass the same bus to EngineBuilder:
// let engine = EngineBuilder::new(config).with_event_bus(events.clone()).build()?;

assert!(events.unsubscribe(listener_id));
```

Closures are usually simplest. For complex state or reusable implementations, implement `EventListener` and register with `subscribe(Arc<dyn EventListener>)`.

## Events

Event JSON is a tagged enum with a `type` field and fixed kebab-case names.

| JSON `type` | Emitted | Key fields |
| --- | --- | --- |
| `scan-started` | Before resolving scan scope | `operation_id`, `request` |
| `task-scan-started` | Before a task's `Handler::scan` | Module, task, handler ID |
| `task-scan-finished` | After handler scan returns or a task error occurs | Item count, issue count, error |
| `scan-finished` | After scan success, failure, or cancellation | Module/task/item/issue/error counts |
| `cleanup-started` | Before executing the cleanup plan | Group count, item count |
| `cleanup-group-started` | Before a task group's `Handler::clean` | Module, task, handler ID, item count |
| `cleanup-group-finished` | After handler cleanup returns or the group fails | Attempted, succeeded, failed, reclaimed size, error |
| `cleanup-finished` | After plan success, failure, or cancellation | Aggregate statistics and cancellation state |

Scan and cleanup each receive a new `operation_id`. Task events within one `scan` call use its scan ID; group events within one `execute` call use its cleanup ID.

## EventEnvelope

Each event is wrapped in this envelope:

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

- `sequence` starts at 1 and increases monotonically within one `EventBus`.
- `emitted_at_ms` is a Unix timestamp in milliseconds.
- `event` contains the `SpacePruneEvent` body.
- Cloning an `EventBus` shares its listeners and counters; it does not create a new sequence.
- A separate new `EventBus` has an independent sequence space.

## Ordering guarantees

A normal scan has this minimum sequence:

```text
scan-started
  task-scan-started
  task-scan-finished
  ...
scan-finished
```

A normal cleanup has this minimum sequence:

```text
cleanup-started
  cleanup-group-started
  cleanup-group-finished
  ...
cleanup-finished
```

Tasks execute in the existing deterministic order: module `order`, module ID, task `order`, then task ID. Listeners run in `EventListenerId` registration order.

Dispatch takes a snapshot of the listeners first. Subscribing or unsubscribing inside a callback affects only the next event, not the recipients of the current event.

## Errors and cancellation

- Handler scan errors appear in `task-scan-finished.error`; scanning may continue with other tasks.
- Error-level handler issues contribute to `scan-finished.error_count`.
- Core errors preventing a scan report appear in `scan-finished.error`.
- Handler-wide cleanup errors appear in `cleanup-group-finished.error`.
- Individual cleanup failures contribute to `failure_count`.
- Cooperative cancellation is represented by `cancelled = true`.
- Cancellation before an unstarted task or group produces no started/finished pair for it.

Listeners have no error return value. A listener panic interrupts the caller like an ordinary Rust callback panic; the event bus does not swallow it. Callers must ensure listener reliability.

## Synchronous dispatch

Events dispatch synchronously on the scanning or cleanup thread. This means:

1. Event order matches execution order without asynchronous reordering.
2. Slow listeners directly slow scanning or cleanup.

Run quick in-memory state updates, metric increments, or channel sends synchronously. For file writes or other slow work, have the listener send to a bounded queue and perform the work on a background thread.

Callers needing blocking policy decisions must make them before `Engine::execute`; EventBus listeners do not provide that function.

Unit tests cover subscription, unsubscription, ordering, core lifecycle order, and kebab-case JSON type names.
