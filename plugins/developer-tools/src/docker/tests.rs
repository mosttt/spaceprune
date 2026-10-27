use super::*;
use spaceprune_plugin_api::NullProgress;
use std::cell::{Cell, RefCell};

struct Engine {
    endpoint: String,
    identity: RefCell<String>,
    objects: RefCell<Vec<String>>,
    calls: RefCell<Vec<Vec<String>>>,
    interface_missing: Cell<bool>,
    fail_remove: Cell<bool>,
    becomes_busy: Cell<bool>,
}
impl Engine {
    fn new() -> Self {
        Self {
            endpoint: "unix:///fixture/docker.sock".into(),
            identity: RefCell::new("engine-one".into()),
            objects: RefCell::new(vec!["cacheone".into(), "cachetwo".into()]),
            calls: RefCell::new(Vec::new()),
            interface_missing: Cell::new(false),
            fail_remove: Cell::new(false),
            becomes_busy: Cell::new(false),
        }
    }
}
impl Runner for Engine {
    fn run(
        &self,
        endpoint: Option<&str>,
        args: &[&str],
        context: &OperationContext,
    ) -> PluginResult<String> {
        context.check().map_err(|_| invalid())?;
        self.calls
            .borrow_mut()
            .push(args.iter().map(|s| s.to_string()).collect());
        if args.starts_with(&["context", "inspect"]) {
            return Ok(json!([{"Endpoints":{"docker":{"Host":self.endpoint}}}]).to_string());
        }
        assert_eq!(endpoint, Some(self.endpoint.as_str()));
        if args[0] == "info" {
            return Ok(json!(*self.identity.borrow()).to_string());
        }
        if args.last() == Some(&"--help") {
            if self.interface_missing.get() && args[0] == "buildx" {
                return Err(PluginError::new(
                    "docker-command-failed",
                    "buildx is not a docker command",
                ));
            }
            return Ok("--format --builder --filter --no-trunc --no-prune --size".into());
        }
        if args.starts_with(&["buildx", "du"]) {
            assert_eq!(
                args,
                &["buildx", "du", "--builder", "default", "--format", "json"]
            );
            return Ok(self.objects.borrow().iter().map(|id|json!({"ID":id,"Size":if id=="cacheone"{"123B"}else{"1.5GB"},"Reclaimable":!self.becomes_busy.get(),"Description":"fixture"}).to_string()).collect::<Vec<_>>().join("\n"));
        }
        if args.starts_with(&["buildx", "prune"]) {
            assert_eq!(
                &args[..7],
                &[
                    "buildx",
                    "prune",
                    "--builder",
                    "default",
                    "--force",
                    "--all",
                    "--filter"
                ]
            );
            if self.fail_remove.get() {
                return Err(PluginError::new("docker-command-failed", "transport lost"));
            }
            let id = args[7]
                .strip_prefix("id=^")
                .unwrap()
                .strip_suffix('$')
                .unwrap();
            if !self.becomes_busy.get() {
                self.objects.borrow_mut().retain(|existing| existing != id);
            }
            return Ok("Total: 123B".into());
        }
        panic!("Unexpected command {args:?}");
    }
}
fn scan(engine: &Engine) -> HandlerScanResult {
    DockerHandler(DockerKind::BuildCache)
        .scan_with(
            &DockerKind::BuildCache.task(),
            &OperationContext::default(),
            engine,
        )
        .unwrap()
}
#[test]
fn exact_selected_cache_only_and_unknown_reclaimed_bytes() {
    let engine = Engine::new();
    let scanned = scan(&engine);
    assert_eq!(scanned.items[0].logical_bytes, Some(123));
    assert_eq!(scanned.items[1].logical_bytes, None);
    assert!(
        scanned
            .items
            .iter()
            .all(|item| item.recommendation == Recommendation::Optional
                && item.reclaimable_bytes.is_none())
    );
    engine.objects.borrow_mut().push("arrivedlater".into());
    let result = DockerHandler(DockerKind::BuildCache)
        .clean_with(
            &DockerKind::BuildCache.task(),
            &scanned.items[..1],
            &NullProgress,
            &OperationContext::default(),
            &engine,
        )
        .unwrap();
    assert_eq!(result.succeeded, 1);
    assert_eq!(result.unknown_reclaimed_items(), 1);
    assert_eq!(*engine.objects.borrow(), ["cachetwo", "arrivedlater"]);
}
#[test]
fn missing_interface_never_enumerates_and_does_not_disable_other_capabilities() {
    let engine = Engine::new();
    engine.interface_missing.set(true);
    let scanned = scan(&engine);
    assert_eq!(scanned.items[0].recommendation, Recommendation::Unavailable);
    assert_eq!(
        scanned.issues[0].code.as_deref(),
        Some("docker-interface-unavailable")
    );
    assert!(
        !engine
            .calls
            .borrow()
            .iter()
            .any(|args| args.starts_with(&["buildx".into(), "du".into()])
                && !args.contains(&"--help".into()))
    );
    cli::interface(
        DockerKind::DanglingImages,
        &Binding {
            endpoint: engine.endpoint.clone(),
            engine: "engine-one".into(),
        },
        &engine,
        &OperationContext::default(),
    )
    .unwrap();
}
#[test]
fn remote_endpoint_changed_engine_forged_id_and_cancel_never_remove() {
    for endpoint in [
        "tcp://localhost:2375",
        "tcp://remote:2376",
        "ssh://host",
        "npipe:////remote/pipe/docker_engine",
        "unix://relative",
    ] {
        assert!(!cli::local_endpoint(endpoint));
    }
    assert!(cli::local_endpoint(
        "npipe:////./pipe/dockerDesktopLinuxEngine"
    ));
    for case in ["engine", "id", "cancel", "in-use"] {
        let engine = Engine::new();
        let mut scanned = scan(&engine);
        let context = OperationContext::default();
        match case {
            "engine" => *engine.identity.borrow_mut() = "replacement".into(),
            "id" => {
                scanned.items[0].id = ".*".into();
                scanned.items[0].opaque["id"] = json!(".*");
            }
            "cancel" => context.cancel(),
            _ => engine.becomes_busy.set(true),
        }
        let result = DockerHandler(DockerKind::BuildCache)
            .clean_with(
                &DockerKind::BuildCache.task(),
                &scanned.items[..1],
                &NullProgress,
                &context,
                &engine,
            )
            .unwrap();
        assert_eq!(result.succeeded, 0);
        assert!(
            !engine
                .calls
                .borrow()
                .iter()
                .any(|args| args.get(1).is_some_and(|arg| arg == "prune")
                    && !args.contains(&"--help".into()))
        );
    }
}
#[test]
fn uncertain_cleanup_is_not_retried_or_reported_as_zero() {
    let engine = Engine::new();
    let scanned = scan(&engine);
    engine.fail_remove.set(true);
    let result = DockerHandler(DockerKind::BuildCache)
        .clean_with(
            &DockerKind::BuildCache.task(),
            &scanned.items[..1],
            &NullProgress,
            &OperationContext::default(),
            &engine,
        )
        .unwrap();
    assert_eq!(result.outcomes[0].status, ItemExecutionStatus::Uncertain);
    assert_eq!(result.outcomes[0].reclaimed_estimate, None);
}
#[test]
fn destructive_domain_commands_cannot_widen_to_parents_volumes_or_running_containers() {
    let image = format!("sha256:{}", "a".repeat(64));
    let container = "b".repeat(64);
    assert_eq!(
        inventory::remove_args(DockerKind::DanglingImages, &image),
        ["image", "rm", "--no-prune", &image]
    );
    assert_eq!(
        inventory::remove_args(DockerKind::StoppedContainers, &container),
        ["container", "rm", &container]
    );
    for kind in DockerKind::ALL {
        let mut task = kind.task();
        task.options = json!({"command":"system prune -af --volumes"});
        assert!(DockerHandler(kind).validate(&task).is_err());
    }
}

#[test]
fn a_prune_skipped_record_is_not_reported_removed_when_it_becomes_busy() {
    let engine = Engine::new();
    let scanned = scan(&engine);
    // A runner changes the record to busy at prune time, not during preflight.
    struct Busy<'a>(&'a Engine);
    impl Runner for Busy<'_> {
        fn run(
            &self,
            endpoint: Option<&str>,
            args: &[&str],
            context: &OperationContext,
        ) -> PluginResult<String> {
            if args.starts_with(&["buildx", "prune"]) && !args.contains(&"--help") {
                self.0.becomes_busy.set(true);
            }
            self.0.run(endpoint, args, context)
        }
    }
    let result = DockerHandler(DockerKind::BuildCache)
        .clean_with(
            &DockerKind::BuildCache.task(),
            &scanned.items[..1],
            &NullProgress,
            &OperationContext::default(),
            &Busy(&engine),
        )
        .unwrap();
    assert_eq!(result.succeeded, 0);
    assert_eq!(result.outcomes[0].status, ItemExecutionStatus::Uncertain);
    assert!(engine.objects.borrow().contains(&"cacheone".into()));
}

#[test]
fn an_already_removed_selected_object_is_an_idempotent_zero_byte_success() {
    let engine = Engine::new();
    let scanned = scan(&engine);
    engine.objects.borrow_mut().remove(0);
    let result = DockerHandler(DockerKind::BuildCache)
        .clean_with(
            &DockerKind::BuildCache.task(),
            &scanned.items[..1],
            &NullProgress,
            &OperationContext::default(),
            &engine,
        )
        .unwrap();
    assert_eq!(result.succeeded, 1);
    assert_eq!(result.unknown_reclaimed_items(), 0);
    assert_eq!(result.reclaimed_bytes, 0);
    assert_eq!(
        result.outcomes[0].status,
        ItemExecutionStatus::AlreadyAbsent
    );
    assert_eq!(result.outcomes[0].reclaimed_estimate, Some(0));
    assert!(result.outcomes[0].failure.is_none());
    assert_eq!(*engine.objects.borrow(), ["cachetwo"]);
    assert!(
        !engine
            .calls
            .borrow()
            .iter()
            .any(|args| args.get(1).is_some_and(|arg| arg == "prune")
                && !args.contains(&"--help".into()))
    );
}

#[test]
fn an_earlier_prune_removing_another_selection_does_not_create_a_failure_or_double_count() {
    let engine = Engine::new();
    let scanned = scan(&engine);
    engine.objects.borrow_mut().push("arrivedlater".into());
    struct Cascade<'a>(&'a Engine);
    impl Runner for Cascade<'_> {
        fn run(
            &self,
            endpoint: Option<&str>,
            args: &[&str],
            context: &OperationContext,
        ) -> PluginResult<String> {
            let result = self.0.run(endpoint, args, context)?;
            if args.starts_with(&["buildx", "prune"]) && !args.contains(&"--help") {
                self.0.objects.borrow_mut().retain(|id| id != "cachetwo");
            }
            Ok(result)
        }
    }
    let result = DockerHandler(DockerKind::BuildCache)
        .clean_with(
            &DockerKind::BuildCache.task(),
            &scanned.items,
            &NullProgress,
            &OperationContext::default(),
            &Cascade(&engine),
        )
        .unwrap();
    assert_eq!(result.succeeded, 2);
    assert_eq!(result.unknown_reclaimed_items(), 1);
    assert_eq!(result.outcomes[0].status, ItemExecutionStatus::Succeeded);
    assert_eq!(
        result.outcomes[1].status,
        ItemExecutionStatus::AlreadyAbsent
    );
    assert_eq!(result.outcomes[1].reclaimed_estimate, Some(0));
    assert_eq!(*engine.objects.borrow(), ["arrivedlater"]);
    let calls = engine.calls.borrow();
    let prunes = calls
        .iter()
        .filter(|args| {
            args.get(1).is_some_and(|arg| arg == "prune") && !args.contains(&"--help".into())
        })
        .collect::<Vec<_>>();
    assert_eq!(prunes.len(), 1);
    assert_eq!(prunes[0].last().unwrap(), "id=^cacheone$");
}

#[test]
fn malformed_existence_results_cannot_confirm_object_absence() {
    struct Listing(&'static str);
    impl Runner for Listing {
        fn run(&self, _: Option<&str>, _: &[&str], _: &OperationContext) -> PluginResult<String> {
            Ok(self.0.into())
        }
    }
    let binding = Binding {
        endpoint: "unix:///fixture/docker.sock".into(),
        engine: "engine-one".into(),
    };
    for (kind, output) in [
        (DockerKind::BuildCache, r#"{"Reclaimable":false}"#),
        (DockerKind::BuildCache, r#"{"ID":""}"#),
        (DockerKind::DanglingImages, "malformed-image-id"),
        (DockerKind::StoppedContainers, "malformed-container-id"),
    ] {
        assert!(
            inventory::exists(
                kind,
                &binding,
                "cacheone",
                &Listing(output),
                &OperationContext::default()
            )
            .is_err()
        );
    }
}
