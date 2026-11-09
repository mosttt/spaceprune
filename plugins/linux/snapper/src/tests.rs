use std::collections::BTreeSet;
#[cfg(target_os = "linux")]
use std::fs;

use serde_json::{Value, json};
use spaceprune_plugin_api::Recommendation;
#[cfg(target_os = "linux")]
use tempfile::tempdir;

#[cfg(target_os = "linux")]
use super::{Options, make_item};
use super::{Snapshot, SnapshotGroup, group_pairs, latest_ids};

fn snapshot(number: u64, pre: Option<u64>) -> Snapshot {
    Snapshot {
        number,
        is_default: false,
        active: false,
        snapshot_type: String::new(),
        pre_number: pre,
        date: String::new(),
        cleanup: "timeline".to_owned(),
        description: String::new(),
        userdata: json!({}),
        used_space: Value::Null,
    }
}

#[test]
fn groups_pre_and_post_as_one_selection_unit() {
    let groups = group_pairs(vec![
        snapshot(1, None),
        snapshot(2, Some(1)),
        snapshot(3, None),
    ]);
    assert_eq!(groups.len(), 2);
    assert!(groups.iter().any(|group| group.numbers() == [1, 2]));
}

#[test]
fn keeps_orphan_post_snapshots_visible() {
    let groups = group_pairs(vec![snapshot(2, Some(1)), snapshot(3, None)]);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].numbers(), [2]);
}

#[test]
fn protects_latest_numbers() {
    assert_eq!(
        latest_ids(
            &[snapshot(1, None), snapshot(4, None), snapshot(2, None)],
            2
        ),
        [2, 4].into_iter().collect()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn manual_snapshot_is_optional_and_therefore_selectable() {
    let temp = tempdir().unwrap();
    let target = temp.path().join(".snapshots/1/snapshot/project/target");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("artifact"), [0_u8; 16]).unwrap();
    let mut manual = snapshot(1, None);
    manual.cleanup.clear();
    let group = SnapshotGroup {
        snapshots: vec![manual],
    };
    let mut issues = Vec::new();
    let item = make_item(
        "home",
        temp.path(),
        &group,
        &Options {
            elevation: "never".to_owned(),
            ..Options::default()
        },
        0,
        true,
        &BTreeSet::new(),
        &spaceprune_plugin_api::OperationContext::default(),
        &mut issues,
    );
    assert_eq!(item.recommendation, Recommendation::Optional);
    // GNU du versions differ on whether apparent size includes directory metadata.
    assert!(
        (16..=16 + fs::metadata(&target).unwrap().len()).contains(&item.logical_bytes.unwrap())
    );
    assert_eq!(item.reclaimable_bytes, None);
    assert!(issues.is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn current_snapshot_without_measurement_does_not_report_zero_bytes() {
    let temp = tempdir().unwrap();
    let mut issues = Vec::new();
    let item = make_item(
        "home",
        temp.path(),
        &SnapshotGroup {
            snapshots: vec![snapshot(0, None)],
        },
        &Options {
            elevation: "never".to_owned(),
            ..Options::default()
        },
        0,
        true,
        &BTreeSet::new(),
        &spaceprune_plugin_api::OperationContext::default(),
        &mut issues,
    );
    assert_eq!(item.recommendation, Recommendation::Unavailable);
    assert_eq!(item.logical_bytes, None);
    assert_eq!(
        item.logical_size_applicability,
        spaceprune_plugin_api::SizeApplicability::NotApplicable
    );
    assert_eq!(item.name, "home · Current system (#0)");
    assert!(!item.has_unknown_logical_size());
    assert_eq!(item.reclaimable_bytes, None);
    assert!(issues.is_empty());
}

pub(super) fn metadata_task() -> spaceprune_plugin_api::TaskRequest {
    spaceprune_plugin_api::TaskRequest {
        module_id: "snapshots".to_owned(),
        task_id: "home-history".to_owned(),
        name: "Selected snapshot history".to_owned(),
        handler_id: "builtin.snapper/snapshots".to_owned(),
        options: json!({"scan-mode": "snapshots", "configs": ["home"], "elevation": "never"}),
        recommend: json!({}),
        tags: Vec::new(),
    }
}

#[test]
fn snapshot_metadata_keeps_unknown_sizes_and_has_no_implicit_recommendations() {
    let task = metadata_task();
    let (_, recommend) = super::parse(&task).unwrap();
    assert!(!recommend.enabled);
    assert_eq!(recommend.keep_latest, 0);
    let mut entry = snapshot(1, None);
    entry.date = "2001-02-03 04:05:06".to_owned();
    let group = SnapshotGroup {
        snapshots: vec![entry],
    };
    let item = super::make_snapshot_item("home", &group, &recommend, 0, &BTreeSet::new());
    assert_eq!(item.logical_bytes, None);
    assert_eq!(item.reclaimable_bytes, None);
    assert_eq!(item.modified_at, Some(981_173_106));
    assert_eq!(item.recommendation, Recommendation::Optional);
}

#[test]
fn explicit_snapshot_rules_decide_age_size_and_retention_without_name_heuristics() {
    let mut task = metadata_task();
    task.recommend = json!({"enabled": true, "older-than-days": 30, "min-snapshot-size": "1KiB", "keep-latest": 1});
    let (_, recommend) = super::parse(&task).unwrap();
    let mut entry = snapshot(7, None);
    entry.date = "2001-02-03 04:05:06".to_owned();
    entry.used_space = json!(2048);
    entry.cleanup.clear();
    let mut group = SnapshotGroup {
        snapshots: vec![entry],
    };
    let item = super::make_snapshot_item("home", &group, &recommend, 1024, &BTreeSet::new());
    assert_eq!(item.recommendation, Recommendation::Recommended);
    let retained =
        super::make_snapshot_item("home", &group, &recommend, 1024, &[7].into_iter().collect());
    assert_eq!(retained.recommendation, Recommendation::Optional);
    group.snapshots[0].used_space = Value::Null;
    let unknown = super::make_snapshot_item("home", &group, &recommend, 1024, &BTreeSet::new());
    assert_eq!(unknown.recommendation, Recommendation::Optional);
    group.snapshots[0].active = true;
    let active = super::make_snapshot_item("home", &group, &recommend, 0, &BTreeSet::new());
    assert_eq!(active.recommendation, Recommendation::Unavailable);
}

#[test]
fn selected_snapshot_ids_require_complete_existing_pairs() {
    let groups = group_pairs(vec![
        snapshot(1, None),
        snapshot(2, Some(1)),
        snapshot(3, None),
    ]);
    assert!(super::validate_selected_groups(&groups, &[1, 2]).is_ok());
    assert_eq!(
        super::validate_selected_groups(&groups, &[1])
            .unwrap_err()
            .code,
        "incomplete-snapshot-pair"
    );
    assert_eq!(
        super::validate_selected_groups(&groups, &[99])
            .unwrap_err()
            .code,
        "snapshot-selection-changed"
    );
    let summary = super::readonly::group_summary(&groups[0]);
    assert_eq!(summary.numbers, [1, 2]);
    assert_eq!(summary.snapshot_type, "pre/post");
    assert_eq!(summary.used_bytes, None);
}

#[cfg(target_os = "linux")]
#[test]
fn snapshot_identity_rejects_another_host_and_reused_numbers() {
    use spaceprune_plugin_api::{Handler, NullProgress, OperationContext};

    let mut task = metadata_task();
    task.options["snapshot-ids"] = json!([1]);
    assert_eq!(
        super::SnapperHandler.validate(&task).unwrap_err().code,
        "missing-snapshot-identity"
    );
    task.options["snapshot-identities"] =
        json!([{"number": 1, "date": "2001-02-03 04:05:06", "type": "single"}]);
    task.options["snapshot-host-id"] = json!("another-computer");
    assert_eq!(
        super::SnapperHandler
            .scan(&task, &NullProgress, &OperationContext::default())
            .unwrap_err()
            .code,
        "snapshot-host-mismatch"
    );
    assert_eq!(
        super::SnapperHandler
            .clean(&task, &[], &NullProgress, &OperationContext::default())
            .unwrap_err()
            .code,
        "snapshot-host-mismatch"
    );
    task.options["snapshot-host-id"] = json!(super::snapper_host_id().unwrap());
    let (options, _) = super::parse(&task).unwrap();
    let mut same_number = snapshot(1, None);
    same_number.snapshot_type = "single".to_owned();
    same_number.date = "2001-02-04 04:05:06".to_owned();
    assert_eq!(
        super::validate_snapshot_identities(&options, &[same_number.clone()], None)
            .unwrap_err()
            .code,
        "snapshot-identity-changed"
    );
    same_number.date = "2001-02-03 04:05:06".to_owned();
    assert!(super::validate_snapshot_identities(&options, &[same_number], None).is_ok());
}

#[cfg(target_os = "linux")]
#[test]
fn snapshot_scan_measures_logical_sizes_but_cleanup_uses_only_confirmed_groups() {
    use spaceprune_plugin_api::{Handler, NullProgress, OperationContext};
    use std::os::unix::fs::PermissionsExt;

    let temp = tempdir().unwrap();
    let executable = temp.path().join("snapper-fixture");
    let records = temp.path().join("snapshots.json");
    let calls = temp.path().join("calls.jsonl");
    let subvolume = temp.path().join("subvolume");
    let mut logical_bytes = 0;
    for number in [1, 2] {
        let path = subvolume
            .join(".snapshots")
            .join(number.to_string())
            .join("snapshot");
        fs::create_dir_all(&path).unwrap();
        // Sparse apparent size must include holes and differs from allocated blocks.
        fs::File::create(path.join("sparse.bin"))
            .unwrap()
            .set_len(number * 1_048_576)
            .unwrap();
        logical_bytes += number * 1_048_576 + fs::metadata(&path).unwrap().len();
    }
    fs::write(&records, json!({"home": [
            {"number": 0},
            {"number": 1, "type": "pre", "date": "2001-02-03 04:05:06", "used-space": 1024},
            {"number": 2, "type": "post", "pre-number": 1, "date": "2001-02-03 04:06:06", "used-space": 2048},
            {"number": 3, "type": "single", "date": "2001-02-03 04:07:06"}
        ]}).to_string()).unwrap();
    let script = format!(
        "#!/usr/bin/env python3\nimport json, pathlib, sys\na = sys.argv[1:]\nwith open({}, 'a') as f: f.write(json.dumps(a) + '\\n')\nif 'list-configs' in a: print(json.dumps({{'configs': [{{'config': 'home', 'subvolume': {}}}]}}))\nelif 'list' in a: print(pathlib.Path({}).read_text())\nelif 'delete' not in a: sys.exit(9)\n",
        serde_json::to_string(&calls).unwrap(),
        serde_json::to_string(&subvolume).unwrap(),
        serde_json::to_string(&records).unwrap()
    );
    fs::write(&executable, script).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let mut task = metadata_task();
    task.options["snapper"] = json!(executable);
    task.options["find"] = json!("must-not-run-find");
    task.options["snapshot-ids"] = json!([1, 2]);
    task.options["snapshot-host-id"] = json!(super::snapper_host_id().unwrap());
    task.options["snapshot-identities"] = json!([
        {"number": 1, "type": "pre", "date": "2001-02-03 04:05:06"},
        {"number": 2, "type": "post", "pre-number": 1, "date": "2001-02-03 04:06:06"}
    ]);
    task.recommend = json!({"enabled": true});
    super::SnapperHandler.validate(&task).unwrap();
    let scan = super::SnapperHandler
        .scan(&task, &NullProgress, &OperationContext::default())
        .unwrap();
    assert!(scan.issues.is_empty());
    assert_eq!(scan.items.len(), 2);
    assert_eq!(scan.items[1].id, "home:1+2");
    assert!((3 * 1_048_576..=logical_bytes).contains(&scan.items[1].logical_bytes.unwrap()));
    assert_eq!(scan.items[1].reclaimable_bytes, None);
    assert_eq!(scan.items[1].opaque["reported_used_bytes"], json!(3072));
    assert_eq!(scan.items[1].recommendation, Recommendation::Recommended);
    let cleaned = super::SnapperHandler
        .clean(
            &task,
            &scan.items[1..],
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    assert_eq!(cleaned.succeeded, 1);
    // Reported snapshot usage is not a reclaimable-size estimate for shared blocks.
    assert_eq!(cleaned.reclaimed_bytes, 0);
    assert_eq!(cleaned.unknown_reclaimed_items(), 1);
    let invocations: Vec<Value> = fs::read_to_string(&calls)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        invocations.last().unwrap(),
        &json!(["-c", "home", "delete", "--sync", "1", "2"])
    );
    fs::write(
            &records,
            json!({"home": [{"number": 1, "active": true, "type": "pre", "date": "2001-02-03 04:05:06"}, {"number": 2, "type": "post", "pre-number": 1, "date": "2001-02-03 04:06:06"}]})
                .to_string(),
        )
        .unwrap();
    let changed = super::SnapperHandler
        .clean(
            &task,
            &scan.items[1..],
            &NullProgress,
            &OperationContext::default(),
        )
        .unwrap();
    assert_eq!(changed.succeeded, 0);
    assert_eq!(changed.reclaimed_bytes, 0);
    assert_eq!(changed.unknown_reclaimed_items(), 0);
    assert_eq!(changed.failures.len(), 1);
    assert!(changed.failures[0].message.contains("active snapshots"));

    task.options["du"] = json!("missing-snapshot-size-fixture");
    let scan = super::SnapperHandler
        .scan(&task, &NullProgress, &OperationContext::default())
        .unwrap();
    assert_eq!(scan.items.len(), 1);
    assert_eq!(scan.items[0].logical_bytes, None);
    assert!(scan.issues[0].message.contains("logical sizes are unknown"));
}

#[cfg(target_os = "linux")]
#[test]
fn host_managed_scan_reports_native_missing_paths_without_believing_command_permission_text() {
    use spaceprune_plugin_api::{OperationContext, ProgressEvent, ProgressPhase, ProgressSink};
    use std::os::unix::fs::PermissionsExt;

    struct NoAuthorization;
    impl ProgressSink for NoAuthorization {
        fn emit(&self, event: ProgressEvent) {
            assert_ne!(event.phase, ProgressPhase::Authorize);
        }
    }
    let temp = tempdir().unwrap();
    let executable = temp.path().join("snapper-fixture");
    fs::write(&executable, format!(
            "#!/usr/bin/python3\nimport json, sys\nif 'list-configs' in sys.argv: print(json.dumps({{'configs': [{{'config': 'home', 'subvolume': {}}}]}}))\nelse: print(json.dumps({{'home': [{{'number': 1, 'type': 'single', 'date': '2001-02-03 04:05:06'}}]}}))\n",
            serde_json::to_string(temp.path()).unwrap(),
        )).unwrap();
    let du = temp.path().join("du-fixture");
    fs::write(
        &du,
        "#!/bin/sh\nprintf 'Permission denied\\n' >&2\nexit 1\n",
    )
    .unwrap();
    for program in [&executable, &du] {
        fs::set_permissions(program, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut task = metadata_task();
    task.options["snapper"] = json!(executable);
    task.options["du"] = json!(du);
    task.options["elevation"] = json!("pkexec");
    task.options["find"] = json!(du);
    for mode in ["snapshots", "rust-targets"] {
        task.options["scan-mode"] = json!(mode);
        let result =
            super::scan_readonly(&task, &NoAuthorization, &OperationContext::default()).unwrap();
        assert_eq!(task.options["elevation"], "pkexec");
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].logical_bytes, None);
        assert!(result.issues.iter().any(|issue| {
            issue
                .io_failure
                .as_ref()
                .is_some_and(|failure| failure.kind == spaceprune_access::IoFailureKind::NotFound)
        }));
    }
}

#[test]
fn desktop_requests_graphical_host_authorization_but_domain_command_never_wraps_it() {
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return;
    }
    let elevation = super::elevation_method("auto", true);
    assert_eq!(elevation, Some("pkexec"));
    let command = super::command("snapper");
    assert_eq!(command.get_program(), "snapper");
    assert_eq!(command.get_args().count(), 0);
    assert!(super::elevation_method("never", true).is_none());
}

#[test]
fn current_system_is_not_applicable_but_real_protected_snapshots_remain_measurements() {
    let current = super::make_snapshot_item(
        "home",
        &SnapshotGroup {
            snapshots: vec![snapshot(0, None)],
        },
        &super::Recommend::default(),
        0,
        &BTreeSet::new(),
    );
    assert_eq!(
        current.logical_size_applicability,
        spaceprune_plugin_api::SizeApplicability::NotApplicable
    );
    assert_eq!(current.name, "home · Current system (#0)");
    assert_eq!(current.recommendation, Recommendation::Unavailable);
    let mut protected = snapshot(7, None);
    protected.is_default = true;
    let real = super::make_snapshot_item(
        "home",
        &SnapshotGroup {
            snapshots: vec![protected],
        },
        &super::Recommend::default(),
        0,
        &BTreeSet::new(),
    );
    assert_eq!(
        real.logical_size_applicability,
        spaceprune_plugin_api::SizeApplicability::Applicable
    );
    assert!(real.has_unknown_logical_size());
    assert_eq!(real.recommendation, Recommendation::Unavailable);
}

#[cfg(target_os = "linux")]
#[test]
fn visible_protected_snapshots_are_measured_outside_task_filters() {
    use spaceprune_plugin_api::{Handler, NullProgress, OperationContext};
    use std::os::unix::fs::PermissionsExt;

    let temp = tempdir().unwrap();
    for number in [1, 2, 8] {
        let path = temp.path().join(format!(".snapshots/{number}/snapshot"));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("content"), [7_u8; 1024]).unwrap();
    }
    // Neither #0 nor the invisible, unprotected #9 has a readable snapshot tree.
    let inventory = json!({"home": [
        {"number": 0, "type": "single"},
        {"number": 1, "type": "pre", "date": "2001-02-03 04:05:06", "cleanup": "number"},
        {"number": 2, "type": "post", "pre-number": 1, "date": "2001-02-03 04:06:06", "cleanup": "number"},
        {"number": 8, "type": "single", "default": true, "date": "2001-02-03 04:07:06"},
        {"number": 9, "type": "single", "date": "2001-02-03 04:08:06"}
    ]});
    let snapper = temp.path().join("snapper-fixture");
    fs::write(
        &snapper,
        format!(
            "#!/usr/bin/env python3\nimport sys\nprint({} if 'list-configs' in sys.argv else {})\n",
            serde_json::to_string(
                &json!({"configs": [{"config": "home", "subvolume": temp.path()}]}).to_string()
            )
            .unwrap(),
            serde_json::to_string(&inventory.to_string()).unwrap(),
        ),
    )
    .unwrap();
    fs::set_permissions(&snapper, fs::Permissions::from_mode(0o755)).unwrap();
    let mut task = metadata_task();
    task.options["snapper"] = json!(snapper);
    task.options["snapshot-types"] = json!(["pre/post"]);
    task.options["cleanup-algorithms"] = json!(["number"]);
    for exact_selection in [false, true] {
        if exact_selection {
            task.options["snapshot-ids"] = json!([1, 2]);
            task.options["snapshot-host-id"] = json!(super::snapper_host_id().unwrap());
            task.options["snapshot-identities"] = json!([
                {"number": 1, "type": "pre", "date": "2001-02-03 04:05:06"},
                {"number": 2, "type": "post", "pre-number": 1, "date": "2001-02-03 04:06:06"}
            ]);
        }
        let scan = super::SnapperHandler
            .scan(&task, &NullProgress, &OperationContext::default())
            .unwrap();
        assert!(scan.issues.is_empty(), "{:?}", scan.issues);
        assert_eq!(scan.items.len(), 3);
        let current = scan.items.iter().find(|item| item.id == "home:0").unwrap();
        assert!(!current.has_unknown_logical_size());
        let protected = scan.items.iter().find(|item| item.id == "home:8").unwrap();
        assert_eq!(protected.recommendation, Recommendation::Unavailable);
        assert!(protected.logical_bytes.is_some_and(|bytes| bytes >= 1024));
        assert_eq!(protected.reclaimable_bytes, None);
        let pair = scan
            .items
            .iter()
            .find(|item| item.id == "home:1+2")
            .unwrap();
        assert!(pair.logical_bytes.is_some_and(|bytes| bytes >= 2048));
    }
}
