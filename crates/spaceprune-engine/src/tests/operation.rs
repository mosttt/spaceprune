use super::*;
use std::time::{Duration, Instant};

#[derive(Debug)]
struct DeadlineHandler {
    seen: Arc<Mutex<Vec<OperationContext>>>,
}

impl Handler for DeadlineHandler {
    fn descriptor(&self) -> HandlerDescriptor {
        FakeHandler.descriptor()
    }
    fn validate(&self, task: &TaskRequest) -> PluginResult<()> {
        FakeHandler.validate(task)
    }
    fn scan(
        &self,
        task: &TaskRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult> {
        self.seen.lock().unwrap().push(context.clone());
        std::thread::sleep(context.deadline.saturating_duration_since(Instant::now()));
        FakeHandler.scan(task, progress, context)
    }
    fn clean(
        &self,
        _: &TaskRequest,
        _: &[CleanupItem],
        _: &dyn ProgressSink,
        _: &OperationContext,
    ) -> PluginResult<HandlerCleanResult> {
        panic!("Incomplete scans cannot execute cleanup")
    }
}

#[test]
fn one_context_reaches_handlers_and_events_without_renewing_the_next_task() {
    let mut config = test_config();
    let tasks = &mut config.modules.get_mut("code").unwrap().tasks;
    tasks.insert("second".to_owned(), tasks["build"].clone());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut registry = PluginRegistry::new();
    registry
        .register(Arc::new(DeadlineHandler { seen: seen.clone() }))
        .unwrap();
    let engine = EngineBuilder::new(config)
        .with_registry(registry)
        .build()
        .unwrap();
    let context = OperationContext {
        generation: 7,
        deadline: Instant::now() + Duration::from_millis(100),
        budget: spaceprune_access::ResourceBudget {
            output_bytes: 4321,
            line_bytes: 1234,
            ..Default::default()
        },
        ..OperationContext::default()
    };
    let events = Arc::new(Mutex::new(Vec::new()));
    let received = events.clone();
    engine
        .event_bus()
        .subscribe_fn(move |event| received.lock().unwrap().push(event.clone()));
    let report = engine
        .scan(&ScanRequest::default(), &NullProgress, &context)
        .unwrap();
    assert!(report.cancelled);
    assert_eq!(
        report.stop_reason,
        Some(OperationStopReason::DeadlineExceeded)
    );
    assert_eq!(report.modules[0].tasks.len(), 1);
    assert!(!report.modules[0].tasks[0].items.is_empty());
    assert!(report.plan_recommended().is_err());
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].identity(), context.identity());
    assert_eq!(seen[0].deadline, context.deadline);
    assert_eq!(seen[0].interaction, context.interaction);
    assert_eq!(seen[0].budget.output_bytes, 4321);
    assert_eq!(seen[0].budget.line_bytes, 1234);
    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .all(|event| event.operation == Some(context.identity()))
    );
    assert!(matches!(
        events.last().unwrap().event,
        SpacePruneEvent::ScanFinished {
            stop_reason: Some(OperationStopReason::DeadlineExceeded),
            ..
        }
    ));
}
