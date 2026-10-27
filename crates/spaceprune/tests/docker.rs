#![cfg_attr(not(target_os = "linux"), allow(missing_docs))]
#![cfg(target_os = "linux")]
//! Public configuration → capability → scan → confirmation → cleanup, on a strict fake Engine.
use spaceprune::api::*;
use std::{collections::BTreeSet, fs, os::unix::fs::PermissionsExt, path::PathBuf};

#[test]
fn docker_lifecycle_uses_available_native_interfaces_and_exact_confirmed_objects() {
    const ENV: &str = "SPACEPRUNE_DOCKER_FIXTURE";
    let Some(root) = std::env::var_os(ENV).map(PathBuf::from) else {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        fs::write(bin.join("docker"), include_str!("fixtures/docker.py")).unwrap();
        fs::set_permissions(bin.join("docker"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(root.join("engine.json"),r#"{"endpoint":"unix:///fixture/docker.sock","engine":"fixture-engine","caches":["selected","unselected"],"volumes":["database"],"running":["cloud","postgres"]}"#).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "docker_lifecycle_uses_available_native_interfaces_and_exact_confirmed_objects",
                "--nocapture",
            ])
            .env(ENV, root)
            .env("PATH", bin)
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env_remove("DOCKER_CONTEXT")
            .env_remove("DOCKER_HOST")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    };
    set_plugin_enabled("builtin.system-linux", false).unwrap();
    let tasks = discover_system_capabilities(&OperationContext::default()).unwrap();
    assert_eq!(tasks.len(), 4);
    assert!(tasks.iter().all(|task| {
        task.handler_id
            .starts_with("builtin.developer-tools/docker-")
    }));
    // Discovery probes interfaces but never enumerates cache/image/container contents.
    let calls = fs::read_to_string(root.join("commands.jsonl")).unwrap();
    assert!(
        calls.lines().all(|line| line.contains("context")
            || line.contains("info")
            || line.contains("--help"))
    );
    let mut recipe =
        "schema-version=2\nname='Docker fixture'\n[modules.docker]\nname='Docker'\n".to_owned();
    for task in &tasks {
        recipe.push_str(&format!("[modules.docker.tasks.{}]\nname={:?}\nhandler={:?}\nplatforms=['linux','windows','macos']\n[modules.docker.tasks.{}.recommend]\nenabled=true\n",task.task_id,task.name,task.handler_id,task.task_id));
    }
    let store = ConfigStore::new(root.join("config.toml"));
    let saved = store.upsert_recipe("docker", &recipe).unwrap();
    let core = SpacePrune::load_recipe(&saved.path).unwrap();
    let scan = core
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &CancellationToken::default(),
        )
        .unwrap();
    assert!(!scan.has_errors(), "{scan:?}");
    assert_eq!(scan.items_with_keys().len(), 5);
    for (key, item) in scan.items_with_keys() {
        if key.task_id == "docker-unused-images" || key.task_id == "docker-stopped-containers" {
            assert_eq!(item.recommendation, Recommendation::Optional);
        }
    }
    assert_eq!(scan.plan_recommended().unwrap().item_count(), 3);
    let selected = BTreeSet::from([
        ItemKey {
            module_id: "docker".into(),
            task_id: "docker-build-cache".into(),
            item_id: "selected".into(),
        },
        ItemKey {
            module_id: "docker".into(),
            task_id: "docker-dangling-images".into(),
            item_id: format!("sha256:{}", "a".repeat(64)),
        },
        ItemKey {
            module_id: "docker".into(),
            task_id: "docker-unused-images".into(),
            item_id: format!("sha256:{}", "b".repeat(64)),
        },
        ItemKey {
            module_id: "docker".into(),
            task_id: "docker-stopped-containers".into(),
            item_id: "c".repeat(64),
        },
    ]);
    let plan = scan.plan_selected(&selected).unwrap();
    assert_eq!(plan.item_count(), 4);
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("engine.json")).unwrap()).unwrap();
    state["caches"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!("arrivedlater"));
    fs::write(
        root.join("engine.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    let result = core.execute(&plan, &NullProgress, &CancellationToken::default());
    assert!(!result.has_errors(), "{result:?}");
    assert_eq!(result.succeeded(), 4);
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("engine.json")).unwrap()).unwrap();
    assert_eq!(
        state["caches"],
        serde_json::json!(["unselected", "arrivedlater"])
    );
    assert_eq!(state["images"], serde_json::json!([]));
    assert_eq!(state["stopped"], serde_json::json!([]));
    assert_eq!(state["volumes"], serde_json::json!(["database"]));
    assert_eq!(state["running"], serde_json::json!(["cloud", "postgres"]));
    // Installed-but-unavailable diagnostics are distinct from a missing native executable.
    for (detail, code) in [
        (
            "Cannot connect to the Docker daemon",
            "docker-daemon-unavailable",
        ),
        (
            "permission denied while connecting to Docker",
            "docker-access-denied",
        ),
        (
            "client version is too old; minimum API version is 1.44",
            "docker-version-unsupported",
        ),
    ] {
        let mut failed = state.clone();
        failed["info_error"] = serde_json::json!(detail);
        fs::write(
            root.join("engine.json"),
            serde_json::to_vec(&failed).unwrap(),
        )
        .unwrap();
        let before = fs::read_to_string(root.join("commands.jsonl")).unwrap();
        let probes = probe_system_capabilities(&OperationContext::default()).unwrap();
        assert_eq!(probes.len(), 4);
        assert!(probes.iter().all(|probe| {
            probe.issue.as_ref().and_then(|issue| issue.code.as_deref()) == Some(code)
        }));
        assert!(
            discover_system_capabilities(&OperationContext::default())
                .unwrap()
                .is_empty()
        );
        let after = fs::read_to_string(root.join("commands.jsonl")).unwrap();
        assert!(
            after[before.len()..]
                .lines()
                .all(|line| line.contains("context") || line.contains("info"))
        );
    }
    let mut missing_buildx = state.clone();
    missing_buildx["buildx_missing"] = serde_json::json!(true);
    fs::write(
        root.join("engine.json"),
        serde_json::to_vec(&missing_buildx).unwrap(),
    )
    .unwrap();
    let probes = probe_system_capabilities(&OperationContext::default()).unwrap();
    assert_eq!(
        probes.iter().filter(|probe| probe.issue.is_some()).count(),
        1
    );
    assert_eq!(
        discover_system_capabilities(&OperationContext::default())
            .unwrap()
            .len(),
        3
    );
    let mut remote = state.clone();
    remote["endpoint"] = serde_json::json!("tcp://remote:2375");
    fs::write(
        root.join("engine.json"),
        serde_json::to_vec(&remote).unwrap(),
    )
    .unwrap();
    assert!(
        probe_system_capabilities(&OperationContext::default())
            .unwrap()
            .iter()
            .all(
                |probe| probe.issue.as_ref().and_then(|issue| issue.code.as_deref())
                    == Some("docker-remote-endpoint")
            )
    );
    // A missing CLI skips every capability, and saved tasks remain explicit unavailable evidence.
    fs::rename(root.join("bin/docker"), root.join("disabled-docker")).unwrap();
    assert!(
        discover_system_capabilities(&OperationContext::default())
            .unwrap()
            .is_empty()
    );
    let unavailable = core
        .scan(
            &ScanRequest::default(),
            &NullProgress,
            &CancellationToken::default(),
        )
        .unwrap();
    assert!(
        unavailable
            .items_with_keys()
            .iter()
            .all(
                |(_, item)| item.recommendation == Recommendation::Unavailable
                    && item.logical_bytes.is_none()
            )
    );
    assert_eq!(unavailable.plan_recommended().unwrap().item_count(), 0);
}
