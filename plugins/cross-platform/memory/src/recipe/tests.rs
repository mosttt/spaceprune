use super::*;

fn task() -> TaskRequest {
    TaskRequest {
        module_id: "memory".into(),
        task_id: "editor".into(),
        name: "Editor".into(),
        handler_id: "builtin.memory/process-working-set".into(),
        options: json!({"applications":[exe("editor")]}),
        recommend: json!({"enabled":true}),
        tags: vec![],
    }
}
fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!(r"C:\Apps\{name}\app.exe")
    } else {
        format!("/opt/{name}/app")
    }
}
fn process(pid: u32, started: &str, path: String) -> ProcessMemory {
    ProcessMemory {
        identity: ProcessIdentity {
            pid,
            started_at: started.into(),
        },
        name: "app".into(),
        executable: Some(path),
        resident_bytes: 8192,
        private_bytes: 4096,
    }
}
fn snapshot(processes: Vec<ProcessMemory>) -> MemorySnapshot {
    MemorySnapshot {
        total_bytes: 32768,
        available_bytes: 16384,
        processes,
    }
}
struct Sink;
impl ProgressSink for Sink {
    fn emit(&self, _: ProgressEvent) {}
}
fn forbidden(
    _: &[BoundProcess],
    _: &dyn ProgressSink,
    _: &OperationContext,
) -> PluginResult<Vec<MemoryRelease>> {
    panic!("forged selection reached native execution")
}

#[test]
fn reusable_rule_resolves_fresh_pids_and_does_not_match_same_basename() {
    let first = scan_items(
        &task(),
        snapshot(vec![
            process(42, "one", exe("editor")),
            process(43, "two", exe("other")),
        ]),
    )
    .unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].reclaimable_bytes, None);
    let second = scan_items(
        &task(),
        snapshot(vec![
            process(42, "replacement", exe("other")),
            process(44, "new", exe("editor")),
        ]),
    )
    .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_ne!(first.items[0].id, second.items[0].id);
    let bound: BoundProcess = serde_json::from_value(second.items[0].opaque.clone()).unwrap();
    assert_eq!(bound.identity.pid, 44);
    assert!(
        scan_items(&task(), snapshot(vec![]))
            .unwrap()
            .items
            .is_empty()
    );
}
#[test]
fn forged_and_duplicate_selections_never_reach_execution() {
    let handler = MemoryHandler { release: forbidden };
    let item = scan_items(&task(), snapshot(vec![process(42, "one", exe("editor"))]))
        .unwrap()
        .items
        .remove(0);
    let mut forged = item.clone();
    forged.location = exe("other");
    assert!(
        handler
            .clean(&task(), &[forged], &Sink, &OperationContext::default())
            .is_err()
    );
    let mut forged = item.clone();
    forged.opaque["executable"] = json!(exe("other"));
    assert!(
        handler
            .clean(&task(), &[forged], &Sink, &OperationContext::default())
            .is_err()
    );
    assert!(
        handler
            .clean(
                &task(),
                &[item.clone(), item],
                &Sink,
                &OperationContext::default()
            )
            .is_err()
    );
    let mut invalid = task();
    invalid.options = json!({"applications":["app"]});
    assert!(handler.validate(&invalid).is_err());
}
#[test]
fn partial_native_effects_and_unknown_ram_are_preserved() {
    fn release(
        selected: &[BoundProcess],
        _: &dyn ProgressSink,
        _: &OperationContext,
    ) -> PluginResult<Vec<MemoryRelease>> {
        Ok(selected
            .iter()
            .enumerate()
            .map(|(i, p)| MemoryRelease {
                identity: p.identity.clone(),
                succeeded: i == 0,
                applied: i < 2,
                resident_reduction_bytes: if i == 0 { Some(1024) } else { None },
                error: if i == 0 {
                    None
                } else {
                    Some("cancelled or changed".into())
                },
            })
            .collect())
    }
    let items = scan_items(
        &task(),
        snapshot(
            (42..45)
                .map(|pid| process(pid, "start", exe("editor")))
                .collect(),
        ),
    )
    .unwrap()
    .items;
    let result = MemoryHandler { release }
        .clean(&task(), &items, &Sink, &OperationContext::default())
        .unwrap();
    assert_eq!(result.outcomes[0].status, ItemExecutionStatus::Succeeded);
    assert_eq!(
        result.outcomes[1].status,
        ItemExecutionStatus::PartiallyApplied
    );
    assert_eq!(result.outcomes[2].status, ItemExecutionStatus::Failed);
    assert_eq!(result.reclaimed_bytes, 1024);
    assert_eq!(result.unknown_reclaimed_items(), 1);
}
