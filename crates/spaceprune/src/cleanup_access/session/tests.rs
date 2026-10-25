use super::*;
use std::process::{Command, Stdio};

#[test]
fn stalled_startup_reaps_stdio_workers_and_process_descendants() {
    let directory = tempfile::tempdir().unwrap();
    let operation = OperationContext::new(
        spaceprune_access::CancellationToken::default(),
        spaceprune_access::Interaction::NonInteractive,
        Duration::from_millis(500),
    );
    let grant = Grant {
        protocol_version: CLEANUP_PROTOCOL_VERSION,
        operation: operation.transfer().unwrap(),
        plan_digest: "a".repeat(64),
        groups: vec![PlanGroup {
            module_id: "fixture".into(),
            task_id: "fixture".into(),
            handler_id: "builtin.filesystem/paths".into(),
            request: TaskRequest {
                module_id: "fixture".into(),
                task_id: "fixture".into(),
                name: "fixture".into(),
                handler_id: "builtin.filesystem/paths".into(),
                options: serde_json::json!({"padding":"x".repeat(2 * 1024 * 1024)}),
                recommend: serde_json::json!({}),
                tags: vec![],
            },
            items: vec![],
        }],
    };
    let mut command = Command::new("sh");
    command
        .args([
            "-c",
            "printf started > started; (sleep 1; printf late > late) & wait",
        ])
        .current_dir(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let started = Instant::now();
    let result = Session::launch(&mut command, &grant, &operation);
    assert!(matches!(
        result,
        Err(StartupFailure {
            outcome: PermissionOutcome::TimedOut,
            ..
        })
    ));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(directory.path().join("started").exists());
    std::thread::sleep(Duration::from_millis(1100));
    assert!(!directory.path().join("late").exists());
}

#[test]
fn unavailable_authorization_is_not_reported_as_user_refusal() {
    let failure = StartupFailure::from(PermissionOutcome::AuthorizationUnavailable);
    assert!(!failure.error.message.contains("denied"));
    assert!(!failure.error.message.contains("cancelled"));
    assert!(failure.error.message.contains("could not be established"));
}

#[test]
fn startup_diagnostics_preserve_native_outcomes_and_original_errors() {
    for outcome in [
        PermissionOutcome::Denied,
        PermissionOutcome::Cancelled,
        PermissionOutcome::TimedOut,
        PermissionOutcome::NeedsAuthorization,
        PermissionOutcome::HelperMissing,
        PermissionOutcome::HelperOutdated,
        PermissionOutcome::HelperUntrusted,
        PermissionOutcome::AuthorizationUnavailable,
    ] {
        let mut failure = StartupFailure::from(outcome);
        failure.error.message = "原生诊断，不从错误文字判断授权状态".into();
        failure.error.io_failure = Some(spaceprune_access::IoFailure {
            kind: spaceprune_access::IoFailureKind::AccessDenied,
            os_code: Some(13),
        });
        let failure = failure.into_error();
        assert_eq!(failure.authorization, Some(outcome));
        assert_eq!(failure.io_failure.as_ref().unwrap().os_code, Some(13));
        assert_eq!(failure.message, "原生诊断，不从错误文字判断授权状态");
        let decoded: PluginError =
            serde_json::from_slice(&serde_json::to_vec(&failure).unwrap()).unwrap();
        assert_eq!(decoded, failure);
    }
}

#[test]
fn noninteractive_graphical_start_returns_structured_requirement_before_launching() {
    let operation = OperationContext::new(
        spaceprune_access::CancellationToken::default(),
        spaceprune_access::Interaction::NonInteractive,
        Duration::from_secs(5),
    );
    let grant = Grant {
        protocol_version: CLEANUP_PROTOCOL_VERSION,
        operation: operation.transfer().unwrap(),
        plan_digest: "a".repeat(64),
        groups: Vec::new(),
    };
    let error = match Session::start(
        "pkexec",
        &grant,
        &[],
        &spaceprune_plugin_api::NullProgress,
        &operation,
    ) {
        Err(failure) => failure.into_error(),
        Ok(_) => panic!("Background operations must not start graphical authorization"),
    };
    assert_eq!(
        error.authorization,
        Some(PermissionOutcome::NeedsAuthorization)
    );
    assert_eq!(error.code, "cleanup-authorization-unavailable");
}

#[test]
fn startup_launch_failure_retains_the_actual_os_error() {
    let directory = tempfile::tempdir().unwrap();
    let operation = OperationContext::default();
    let grant = Grant {
        protocol_version: CLEANUP_PROTOCOL_VERSION,
        operation: operation.transfer().unwrap(),
        plan_digest: "a".repeat(64),
        groups: Vec::new(),
    };
    let mut command = Command::new(directory.path().join("missing-launcher"));
    let error = match Session::launch(&mut command, &grant, &operation) {
        Err(failure) => failure.into_error(),
        Ok(_) => panic!("The missing executable cannot launch"),
    };
    assert_eq!(
        error.authorization,
        Some(PermissionOutcome::AuthorizationUnavailable)
    );
    assert_eq!(
        error.io_failure.as_ref().unwrap().kind,
        spaceprune_access::IoFailureKind::NotFound
    );
    assert_eq!(error.io_failure.as_ref().unwrap().os_code, Some(2));
    assert!(
        error
            .message
            .starts_with("Could not start cleanup authorization:")
    );
}

fn ready_session() -> (Session, Grant, OperationContext, u32) {
    let operation = OperationContext::new(
        spaceprune_access::CancellationToken::default(),
        spaceprune_access::Interaction::NonInteractive,
        Duration::from_secs(5),
    );
    let grant = Grant {
        protocol_version: CLEANUP_PROTOCOL_VERSION,
        operation: operation.transfer().unwrap(),
        plan_digest: "a".repeat(64),
        groups: Vec::new(),
    };
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("pid");
    let mut command = Command::new("python3");
    command.args(["-u", "-c", "import json,sys,os,pathlib; pathlib.Path(sys.argv[1]).write_text(str(os.getpid())); grant=json.loads(sys.stdin.readline()); print(json.dumps({'operation':grant['operation']['identity'],'reply':{'kind':'ready'}}),flush=True); sys.stdin.read()"]);
    command
        .arg(&pid_file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let session = Session::launch(&mut command, &grant, &operation).unwrap();
    let pid = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
    (session, grant, operation, pid)
}

#[test]
fn finish_waits_for_native_connection_and_preserves_a_failed_close_on_repetition() {
    for failure in [None, Some(PermissionOutcome::TimedOut)] {
        let (mut session, grant, operation, pid) = ready_session();
        session.close_failure = failure;
        let connection = Arc::new(Mutex::new(session));
        let scope = CapabilityScope::System {
            handler: "fixture".into(),
            objects: vec![grant.plan_digest.clone()],
        };
        let execution =
            ExecutionGrant::for_confirmed_plan(&operation, scope.clone(), &grant.plan_digest);
        let access = NativeCleanupAccess {
            grant: Ok(grant),
            login: Mutex::default(),
            native: Arc::new(NativeState {
                connection: Mutex::new(Some(connection.clone())),
                ..NativeState::default()
            }),
            operation: operation.clone(),
            scope,
            execution,
        };
        let request = CapabilityRequest {
            subject: "fixture".into(),
            operation_id: operation.id.clone(),
            kind: CapabilityKind::SystemLifecycle,
            scope: access.scope.clone(),
            interaction: operation.interaction,
            reason_code: "fixture".into(),
        };
        access
            .native
            .broker
            .acquire(&request, &operation, Some(&access.execution), |_, _| {
                Ok(NativeSession::new(connection, operation.deadline, true))
            })
            .unwrap();
        let result = access.finish();
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        assert!(
            !access
                .native
                .broker
                .is_active("fixture", CapabilityKind::SystemLifecycle)
        );
        assert_eq!(result.is_err(), failure.is_some());
        if let Err(error) = &result {
            assert_eq!(error.code, "cleanup-stop-unconfirmed");
            assert_eq!(error.authorization, failure);
        }
        assert_eq!(access.finish(), result);
        assert!(!operation.is_stopped());
    }
}

#[test]
fn uncertain_cleanup_preserves_confirmed_items_and_reports_shutdown_failure() {
    #[derive(Default)]
    struct Progress(Mutex<Vec<spaceprune_plugin_api::ItemExecution>>);
    impl ProgressSink for Progress {
        fn emit(&self, _: ProgressEvent) {}
        fn cleanup_item_finished(
            &self,
            _: &TaskRequest,
            item: &spaceprune_plugin_api::ItemExecution,
        ) -> PluginResult<()> {
            self.0.lock().unwrap().push(item.clone());
            Ok(())
        }
    }
    let (mut session, _, _, _) = ready_session();
    session.close_failure = Some(PermissionOutcome::HelperUntrusted);
    let task = TaskRequest {
        module_id: "fixture".into(),
        task_id: "fixture".into(),
        name: "fixture".into(),
        handler_id: "builtin.filesystem/paths".into(),
        options: serde_json::json!({}),
        recommend: serde_json::json!({}),
        tags: vec![],
    };
    let item = |id: &str| spaceprune_plugin_api::CleanupItem {
        logical_size_applicability: Default::default(),
        id: id.into(),
        parent_id: None,
        name: id.into(),
        location: id.into(),
        logical_bytes: Some(32),
        reclaimable_bytes: Some(32),
        modified_at: None,
        recommendation: spaceprune_plugin_api::Recommendation::Recommended,
        reason: String::new(),
        tags: vec![],
        opaque: serde_json::Value::Null,
    };
    let items = [item("confirmed"), item("pending")];
    let confirmed = std::collections::BTreeSet::from([(
        "fixture".into(),
        "fixture".into(),
        "confirmed".into(),
    )]);
    let progress = Progress::default();
    let failure = session.unconfirmed(
        &[CleanupBatchGroup {
            task: &task,
            items: &items,
        }],
        &confirmed,
        &progress,
        "Execution interrupted",
    );
    assert_eq!(failure.code, "cleanup-result-unconfirmed");
    assert_eq!(
        failure.authorization,
        Some(PermissionOutcome::HelperUntrusted)
    );
    let outcomes = progress.0.lock().unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].item_id, "pending");
    assert_eq!(
        outcomes[0].status,
        spaceprune_plugin_api::ItemExecutionStatus::Uncertain
    );
    assert_eq!(
        outcomes[0].failure.as_ref().unwrap().authorization,
        failure.authorization
    );
    assert_eq!(
        session.close().unwrap_err().code,
        "cleanup-stop-unconfirmed"
    );
}

#[test]
fn login_revocation_cancels_confirmed_cleanup_and_reclaims_idle_or_running_workers() {
    for in_flight in [false, true] {
        for failure in [None, Some(PermissionOutcome::TimedOut)] {
            let (mut session, grant, operation, pid) = ready_session();
            session.close_failure = failure;
            let connection = Arc::new(Mutex::new(session));
            let scope = CapabilityScope::System {
                handler: "fixture".into(),
                objects: vec![grant.plan_digest.clone()],
            };
            let execution =
                ExecutionGrant::for_confirmed_plan(&operation, scope.clone(), &grant.plan_digest);
            let access = NativeCleanupAccess {
                grant: Ok(grant),
                login: Mutex::default(),
                native: Arc::new(NativeState {
                    connection: Mutex::new(Some(connection.clone())),
                    ..NativeState::default()
                }),
                operation: operation.clone(),
                scope,
                execution,
            };
            access.login_context(&operation).unwrap();
            let revocation = access.login.lock().unwrap().as_ref().unwrap().revocation();
            let (started, ready) = std::sync::mpsc::channel();
            std::thread::scope(|scope| {
                let pending = in_flight.then(|| {
                    scope.spawn(|| {
                        let mut connection = connection.lock().unwrap();
                        started.send(()).unwrap();
                        connection.execute(
                            &[],
                            &[],
                            &spaceprune_plugin_api::NullProgress,
                            &operation,
                        )
                    })
                });
                if in_flight {
                    ready.recv_timeout(Duration::from_secs(2)).unwrap();
                }
                revocation.invalidate();
                if let Some(pending) = pending {
                    assert!(pending.join().unwrap().is_err());
                }
            });
            assert!(operation.is_stopped());
            assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
            assert!(access.native.connection.lock().unwrap().is_none());
            let result = access.finish();
            assert_eq!(result.is_err(), failure.is_some());
            if let Err(error) = &result {
                assert_eq!(error.authorization, failure);
            }
            revocation.invalidate();
            assert_eq!(access.finish(), result);
            assert!(
                access
                    .clean_authorized(
                        "sudo",
                        &[],
                        &spaceprune_plugin_api::NullProgress,
                        &operation
                    )
                    .is_err()
            );
        }
    }
}

#[test]
fn cleanup_authorization_events_follow_display_language_without_starting_a_helper() {
    const CHILD: &str = "SPACEPRUNE_CLEANUP_LANGUAGE_FIXTURE";
    let Ok(language) = std::env::var(CHILD) else {
        for language in ["en", "zh-CN"] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "cleanup_access::session::tests::cleanup_authorization_events_follow_display_language_without_starting_a_helper",
                    "--nocapture",
                ])
                .env(CHILD, language)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    };
    messages::set_display_language(&language);
    #[derive(Default)]
    struct Progress(Mutex<Vec<ProgressEvent>>);
    impl ProgressSink for Progress {
        fn emit(&self, event: ProgressEvent) {
            self.0.lock().unwrap().push(event);
        }
    }
    let operation = OperationContext::new(
        spaceprune_access::CancellationToken::default(),
        spaceprune_access::Interaction::NonInteractive,
        Duration::from_secs(5),
    );
    let task = TaskRequest {
        module_id: "fixture-{module}".into(),
        task_id: "fixture-{task}".into(),
        name: "用户名称 {name}".into(),
        handler_id: "builtin.filesystem/paths".into(),
        options: serde_json::json!({}),
        recommend: serde_json::json!({}),
        tags: vec![],
    };
    let grant = Grant {
        protocol_version: CLEANUP_PROTOCOL_VERSION,
        operation: operation.transfer().unwrap(),
        plan_digest: "a".repeat(64),
        groups: vec![PlanGroup {
            module_id: task.module_id.clone(),
            task_id: task.task_id.clone(),
            handler_id: task.handler_id.clone(),
            request: task.clone(),
            items: vec![],
        }],
    };
    let scope = CapabilityScope::System {
        handler: "fixture".into(),
        objects: vec![grant.plan_digest.clone()],
    };
    let execution =
        ExecutionGrant::for_confirmed_plan(&operation, scope.clone(), &grant.plan_digest);
    let access = NativeCleanupAccess {
        grant: Ok(grant),
        login: Mutex::default(),
        native: Arc::default(),
        operation: operation.clone(),
        scope,
        execution,
    };
    let progress = Progress::default();
    let groups = [CleanupBatchGroup {
        task: &task,
        items: &[],
    }];
    // Noninteractive pkexec is rejected before helper installation or system authentication.
    let first = access
        .clean_authorized("pkexec", &groups, &progress, &operation)
        .unwrap_err();
    assert_eq!(
        first.authorization,
        Some(PermissionOutcome::NeedsAuthorization)
    );
    let second = access
        .clean_authorized("pkexec", &groups, &progress, &operation)
        .unwrap_err();
    assert_eq!(first, second);
    assert!(access.native.connection.lock().unwrap().is_none());
    let events = progress.0.lock().unwrap();
    assert_eq!(
        events.len(),
        1,
        "A rejected session must not request authorization again"
    );
    let event = &events[0];
    assert_eq!(event.phase, spaceprune_plugin_api::ProgressPhase::Authorize);
    assert_eq!(event.module_id.as_deref(), Some("fixture-{module}"));
    assert_eq!(event.task_id.as_deref(), Some("fixture-{task}"));
    assert_eq!(
        event.handler_id.as_deref(),
        Some("builtin.filesystem/paths")
    );
    if language == "zh-CN" {
        assert_eq!(
            event.message,
            "正在为已确认的清理请求管理员授权；后续受保护步骤将复用此会话"
        );
        assert!(first.message.contains("需要交互式管理员授权"));
        assert!(first.message.contains("本次清理不会再次弹出授权提示"));
    } else {
        assert_eq!(
            event.message,
            "Requesting administrator authorization for this confirmed cleanup; subsequent protected steps reuse this session"
        );
        assert!(
            first
                .message
                .contains("Interactive administrator authorization is required")
        );
    }
    let encoded = serde_json::to_value(event).unwrap();
    assert_eq!(encoded["phase"], "authorize");
    assert_eq!(encoded["module_id"], "fixture-{module}");
    assert_eq!(encoded["message"], event.message);
    let decoded: PluginError =
        serde_json::from_slice(&serde_json::to_vec(&first).unwrap()).unwrap();
    assert_eq!(decoded, first);
}

#[test]
fn only_confirmed_go_selections_require_the_new_capture_feature() {
    let operation = OperationContext::default();
    let group = |handler: &str, selected: bool| {
        let items = if selected {
            serde_json::json!([{
                "id":"fixture", "parent_id":null, "name":"fixture", "location":"fixture",
                "logical_bytes":null, "reclaimable_bytes":null, "modified_at":null,
                "recommendation":"recommended", "reason":"fixture", "tags":[], "opaque":{}
            }])
        } else {
            serde_json::json!([])
        };
        serde_json::from_value::<PlanGroup>(serde_json::json!({
            "module_id":"fixture", "task_id":"fixture", "handler_id":handler,
            "request":{"module_id":"fixture", "task_id":"fixture", "name":"fixture",
                "handler_id":handler, "options":{}, "recommend":{}, "tags":[]},
            "items":items
        }))
        .unwrap()
    };
    let mut grant = Grant {
        protocol_version: CLEANUP_PROTOCOL_VERSION,
        operation: operation.transfer().unwrap(),
        plan_digest: "a".repeat(64),
        groups: vec![
            group("builtin.snapper/snapshots", true),
            group("builtin.filesystem/paths", true),
        ],
    };
    assert!(!grant.requires_developer_handlers());
    assert!(!grant.requires_go_capture(&[Selection {
        group: 0,
        items: vec![0]
    }]));
    grant
        .groups
        .push(group("builtin.system-linux/go-modcache", false));
    assert!(!grant.requires_go_capture(&[Selection {
        group: 0,
        items: vec![0]
    }]));
    grant
        .groups
        .push(group("builtin.system-linux/go-modcache", true));
    assert!(!grant.requires_go_capture(&[Selection {
        group: 0,
        items: vec![0]
    }]));
    assert!(grant.requires_go_capture(&[Selection {
        group: 3,
        items: vec![0]
    }]));
    assert!(!grant.requires_go_capture(&[Selection {
        group: 3,
        items: vec![]
    }]));
    grant
        .groups
        .push(group("builtin.developer-tools/go-modcache", true));
    assert!(grant.requires_developer_handlers());
    assert!(grant.requires_go_capture(&[Selection {
        group: 4,
        items: vec![0]
    }]));
    grant
        .groups
        .push(group("builtin.developer-tools/npm-cache", true));
    assert!(!grant.requires_go_capture(&[Selection {
        group: 5,
        items: vec![0]
    }]));
    let compatible = grant.without_go_selections();
    assert_eq!(compatible.groups[0].items.len(), 1);
    assert!(compatible.groups[3].items.is_empty());
    assert!(compatible.groups[4].items.is_empty());
    assert_eq!(compatible.groups[5].items.len(), 1);
}

fn replacement_command(marker: &std::path::Path) -> Command {
    let mut command = Command::new("python3");
    command.args(["-u", "-c", "import json,sys,pathlib; pathlib.Path(sys.argv[1]).write_text('started'); grant=json.loads(sys.stdin.readline()); print(json.dumps({'operation':grant['operation']['identity'],'reply':{'kind':'ready'}}),flush=True); sys.stdin.read()"]);
    command
        .arg(marker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[test]
fn go_session_upgrade_keeps_consumed_selections_and_the_original_deadline() {
    let (mut session, grant, operation, previous_pid) = ready_session();
    let prior = [Selection {
        group: 0,
        items: vec![0],
    }];
    session.consume(&prior).unwrap();
    let lease_liveness = session.alive.clone();
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("replacement");
    let command = replacement_command(&marker);
    session
        .replace_connection(Ok(command), &grant, &operation)
        .unwrap();
    assert!(!std::path::Path::new(&format!("/proc/{previous_pid}")).exists());
    assert!(marker.is_file());
    assert!(session.connection.supports_go_capture);
    assert!(
        lease_liveness
            .lock()
            .unwrap()
            .load(std::sync::atomic::Ordering::Acquire)
    );
    assert!(session.consume(&prior).is_err());
    session
        .consume(&[Selection {
            group: 1,
            items: vec![0],
        }])
        .unwrap();
    let restored = OperationContext::from_transfer(
        &grant.operation,
        Duration::from_secs(3600),
        spaceprune_access::ResourceBudget::workflow(),
    )
    .unwrap();
    assert!(restored.deadline <= operation.deadline);
    assert_eq!(restored.identity(), operation.identity());
    session.close().unwrap();
    assert!(!operation.is_stopped());
}

#[test]
fn unconfirmed_old_session_shutdown_never_launches_a_go_replacement() {
    let (mut session, grant, operation, previous_pid) = ready_session();
    session.close_failure = Some(PermissionOutcome::TimedOut);
    let prior = [Selection {
        group: 0,
        items: vec![0],
    }];
    session.consume(&prior).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("must-not-launch");
    let command = replacement_command(&marker);
    let failure = session
        .replace_connection(Ok(command), &grant, &operation)
        .unwrap_err();
    assert_eq!(failure.code, "cleanup-stop-unconfirmed");
    assert!(!marker.exists());
    assert!(!std::path::Path::new(&format!("/proc/{previous_pid}")).exists());
    assert!(session.consume(&prior).is_err());
    assert!(!operation.is_stopped());
}

#[test]
fn missing_go_bundle_is_scoped_and_preserves_the_existing_basic_session() {
    let (mut session, grant, operation, pid) = ready_session();
    for source in [
        spaceprune_access::helper::InstallError::SourceMissing,
        spaceprune_access::helper::InstallError::SourceOutdated,
    ] {
        let failure = session
            .replace_connection(
                Err(platform::component_startup_failure(source, true)),
                &grant,
                &operation,
            )
            .unwrap_err();
        assert_eq!(failure.code, "cleanup-capability-unavailable");
        assert!(failure.message.contains("bundle") || failure.message.contains("bundled"));
        assert!(failure.message.contains("scripts/run-cli.sh"));
        assert!(
            session
                .alive
                .lock()
                .unwrap()
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert!(std::path::Path::new(&format!("/proc/{pid}")).exists());
    }
    session
        .consume(&[Selection {
            group: 0,
            items: vec![0],
        }])
        .unwrap();
    session.close().unwrap();
    assert!(!operation.is_stopped());
}
