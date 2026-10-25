use super::*;
use crate::{PluginHost, manifest::load_manifest};
use spaceprune_engine::{EngineBuilder, NullProgress, PluginRegistry, ScanRequest};
use spaceprune_plugin_api::{ItemExecutionStatus, PluginError, PluginResult, Recommendation};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::PermissionsExt,
};
use tempfile::{TempDir, tempdir};

fn fixture(script: &str) -> (TempDir, InstalledPlugin) {
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join("plugin.toml"),
        r#"schema-version = 1
id = "community.fixture"
name = "Fixture"
version = "1.0.0"
protocol-version = 3
executable = "plugin"
handlers = ["cache"]
"#,
    )
    .unwrap();
    let executable = directory.path().join("plugin");
    fs::write(
        &executable,
        format!("#!/bin/sh\nset -eu\ncd \"$(dirname \"$0\")\"\n{script}\n"),
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let plugin = load_manifest(&directory.path().join("plugin.toml")).unwrap();
    (directory, plugin)
}

fn response(id: u64, result: Value) -> String {
    format!(
        "IFS= read -r request\nprintf '%s\\n' '{}'\n",
        json!({"protocol": 3, "id": id, "result": result})
    )
}

fn items() -> Vec<CleanupItem> {
    ["first", "second", "unselected"]
        .into_iter()
        .map(|id| CleanupItem {
            logical_size_applicability: Default::default(),
            id: id.into(),
            parent_id: None,
            name: id.into(),
            location: id.into(),
            logical_bytes: Some(7),
            reclaimable_bytes: Some(7),
            modified_at: None,
            recommendation: if id == "unselected" {
                Recommendation::Optional
            } else {
                Recommendation::Recommended
            },
            reason: "Fixture".into(),
            tags: vec![],
            opaque: json!({}),
        })
        .collect()
}

fn outcome(id: &str) -> ItemExecution {
    ItemExecution {
        item_id: id.into(),
        status: ItemExecutionStatus::Succeeded,
        reclaimed_estimate: Some(7),
        reclaimed_lower_bound: 0,
        failure: None,
    }
}

fn finished(id: &str) -> String {
    format!(
        "printf '%s\\n' '{}'\nIFS= read -r receipt\ncase \"$receipt\" in *'\"accepted\":true'*) ;; *) exit 0 ;; esac\n",
        json!({"protocol": 3, "request_id": 4, "event": "item_finished", "data": outcome(id)})
    )
}

fn cleanup_fixture(
    tail: &str,
) -> (
    TempDir,
    spaceprune_engine::Engine,
    spaceprune_engine::CleanupPlan,
) {
    let script = response(
        1,
        json!({"handlers": [{"id":"community.fixture/cache", "name":"Fixture", "description":"Fixture", "platforms":[], "options_schema":null, "recommend_schema":null}]}),
    ) + &response(2, Value::Null)
        + &response(3, json!({"items":items(), "issues":[]}))
        + "IFS= read -r request\nrm first\n"
        + &finished("first")
        + tail;
    let (directory, plugin) = fixture(&script);
    for name in ["first", "second", "unselected"] {
        fs::write(directory.path().join(name), b"fixture").unwrap();
    }
    let host = PluginHost {
        plugins: BTreeMap::from([(plugin.manifest.id.clone(), plugin)]),
    };
    let mut registry = PluginRegistry::new();
    host.register_all(&mut registry).unwrap();
    let config_path = directory.path().join("recipe.toml");
    fs::write(&config_path, "schema-version = 2\nname = 'Fixture'\n[modules.fixture]\nname = 'Fixture'\n[modules.fixture.tasks.cache]\nname = 'Fixture'\nhandler = 'community.fixture/cache'\n").unwrap();
    let config = spaceprune_config::ConfigLoader::new()
        .load_path(config_path)
        .unwrap();
    let engine = EngineBuilder::new(config)
        .with_registry(registry)
        .with_journal_directory(directory.path().join("journal"))
        .build()
        .unwrap();
    let scan = engine
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    let plan = scan.plan_recommended().unwrap();
    (directory, engine, plan)
}

#[test]
fn plugin_disconnect_preserves_acknowledged_cleanup_in_the_engine_and_journal() {
    let (directory, engine, plan) = cleanup_fixture("exit 0\n");
    let report = engine.execute(&plan, &NullProgress, &OperationContext::default());
    assert!(report.has_errors());
    assert_eq!(report.succeeded(), 1);
    let outcomes = &report.groups[0].result.as_ref().unwrap().outcomes;
    assert_eq!(outcomes[0].status, ItemExecutionStatus::Succeeded);
    assert_eq!(outcomes[1].status, ItemExecutionStatus::Uncertain);
    assert!(!directory.path().join("first").exists());
    assert!(directory.path().join("second").exists());
    assert!(directory.path().join("unselected").exists());
    let journal = fs::read_to_string(
        directory
            .path()
            .join("journal")
            .join(format!("{}.jsonl", plan.id())),
    )
    .unwrap();
    assert!(
        journal
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .any(
                |record| record["event"] == "item_finished" && record.to_string().contains("first")
            )
    );
    assert!(
        engine
            .execute(&plan, &NullProgress, &OperationContext::default())
            .has_errors()
    );
    assert!(directory.path().join("second").exists());
}

#[test]
fn accepted_item_receipts_match_the_final_summary_and_preserve_unselected_files() {
    let mut result = HandlerCleanResult {
        unknown_reclaimed_items: Some(0),
        ..Default::default()
    };
    result.record_outcome(outcome("first"));
    result.record_outcome(outcome("second"));
    let tail = "rm second\n".to_owned()
        + &finished("second")
        + &format!(
            "printf '%s\\n' '{}'\n",
            json!({"protocol":3,"id":4,"result":result})
        );
    let (directory, engine, plan) = cleanup_fixture(&tail);
    let report = engine.execute(&plan, &NullProgress, &OperationContext::default());
    assert!(!report.has_errors(), "{report:?}");
    assert_eq!(report.succeeded(), 2);
    assert!(!directory.path().join("second").exists());
    assert!(directory.path().join("unselected").exists());
}

struct RejectReceipt;
impl ProgressSink for RejectReceipt {
    fn emit(&self, _: ProgressEvent) {}
    fn cleanup_item_finished(&self, _: &TaskRequest, _: &ItemExecution) -> PluginResult<()> {
        Err(PluginError::new(
            "fixture-journal-failed",
            "Fixture receipt failure",
        ))
    }
}

#[test]
fn rejected_receipt_stops_the_plugin_before_the_next_deletion() {
    let (directory, engine, plan) = cleanup_fixture("rm second\n");
    let token = OperationContext::default();
    let report = engine.execute(&plan, &RejectReceipt, &token);
    assert!(report.has_errors());
    assert!(token.is_stopped());
    assert_eq!(report.succeeded(), 1);
    assert!(directory.path().join("second").exists());
    assert!(directory.path().join("unselected").exists());
}

#[test]
fn unacknowledged_or_duplicate_outcomes_cannot_complete_a_cleanup() {
    for tail in [finished("first"), finished("unselected"),
        "printf '%s\\n' '{\"protocol\":3,\"id\":4,\"result\":{\"attempted\":2,\"succeeded\":2,\"reclaimed_bytes\":14,\"failures\":[]}}'\n".into()] {
        let (directory, engine, plan) = cleanup_fixture(&tail);
        let report = engine.execute(&plan, &NullProgress, &OperationContext::default());
        assert!(report.has_errors()); assert_eq!(report.succeeded(), 1);
        assert!(directory.path().join("second").exists());
        assert!(directory.path().join("unselected").exists());
    }
}

#[test]
fn malformed_and_oversized_frames_terminate_the_session() {
    for script in [
        "printf '%s\\n' '{\"protocol\":1,\"id\":1,\"result\":null}'",
        "printf '%s\\n' '{\"protocol\":3,\"id\":9,\"result\":null}'",
        "head -c 1048577 /dev/zero",
    ] {
        let (_directory, plugin) = fixture(script);
        let plugin = ProcessPlugin::start(plugin).unwrap();
        assert!(plugin.call::<Value>("scan", json!({}), None, None).is_err());
        assert!(
            plugin
                .call::<Value>("scan", json!({}), None, None)
                .unwrap_err()
                .to_string()
                .contains("session has stopped")
        );
    }
}

#[test]
fn a_blocked_input_pipe_and_continuous_progress_obey_the_total_deadline() {
    let progress = json!({"protocol":3, "event":"progress", "request_id":1,
        "data":{"phase":"scan", "module_id":null,"task_id":null,"handler_id":null,"current":null,"total":null,"message":"working"}});
    for script in [
        "while :; do :; done".to_owned(),
        format!("IFS= read -r request\nwhile :; do printf '%s\\n' '{progress}'; done"),
    ] {
        let (_directory, plugin) = fixture(&script);
        let mut plugin = ProcessPlugin::start(plugin).unwrap();
        plugin.timeout = Duration::from_millis(200);
        let started = Instant::now();
        let error = plugin
            .call::<Value>(
                "scan",
                json!({"padding":"x".repeat(256 * 1024)}),
                None,
                None,
            )
            .unwrap_err();
        assert!(error.to_string().contains("deadline"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}

#[test]
fn cancelling_an_active_call_terminates_the_process_without_a_retry() {
    let (_directory, plugin) = fixture("IFS= read -r request\nwhile :; do :; done");
    let plugin = ProcessPlugin::start(plugin).unwrap();
    let token = OperationContext::default();
    let cancel = token.clone();
    let signal = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        cancel.cancel();
    });
    assert!(
        plugin
            .call::<Value>("scan", json!({}), None, Some(&token))
            .is_err()
    );
    signal.join().unwrap();
    assert!(
        plugin
            .call::<Value>("scan", json!({}), None, None)
            .unwrap_err()
            .to_string()
            .contains("session has stopped")
    );
}

#[test]
fn cancellation_and_deadline_reap_plugin_descendants_before_returning() {
    for cancel in [true, false] {
        let (directory, plugin) = fixture(
            "IFS= read -r request\n(printf ready > started; sleep 0.6; printf late > late) &\nwait",
        );
        let plugin = ProcessPlugin::start(plugin).unwrap();
        let context = OperationContext {
            deadline: Instant::now() + Duration::from_millis(300),
            ..OperationContext::default()
        };
        let worker_context = context.clone();
        let worker = thread::spawn(move || {
            plugin.call::<Value>("scan", json!({}), None, Some(&worker_context))
        });
        let started = Instant::now();
        while !directory.path().join("started").exists() && !worker.is_finished() {
            assert!(started.elapsed() < Duration::from_secs(3));
            thread::sleep(Duration::from_millis(5));
        }
        assert!(directory.path().join("started").exists());
        if cancel {
            context.cancel();
        }
        assert!(worker.join().unwrap().is_err());
        thread::sleep(Duration::from_millis(700));
        assert!(
            !directory.path().join("late").exists(),
            "Descendant survived the operation"
        );
    }
}

#[test]
fn caller_output_limits_cannot_be_expanded_by_plugin_defaults() {
    for line_limit in [true, false] {
        let (_directory, plugin) = fixture(&response(1, json!({"data":"x".repeat(1024)})));
        let plugin = ProcessPlugin::start(plugin).unwrap();
        let mut context = OperationContext::default();
        if line_limit {
            context.budget.line_bytes = 128;
        } else {
            context.budget.output_bytes = 128;
        }
        let error = plugin
            .call::<Value>("scan", json!({}), None, Some(&context))
            .unwrap_err();
        assert!(
            error.to_string().contains(if line_limit {
                "line exceeded"
            } else {
                "total output limit"
            }),
            "{error}"
        );
    }
}

#[test]
fn expired_setup_never_launches_the_plugin() {
    let (directory, plugin) = fixture("printf started > started");
    let host = PluginHost {
        plugins: BTreeMap::from([(plugin.manifest.id.clone(), plugin)]),
    };
    let context = OperationContext {
        deadline: Instant::now(),
        ..OperationContext::default()
    };
    let required = BTreeSet::from(["community.fixture".to_owned()]);
    assert!(
        host.register_required_in_context(&mut PluginRegistry::new(), &required, &context)
            .is_err()
    );
    assert!(!directory.path().join("started").exists());
}
