//! Per-snapshot isolation, real du semantics and bounded process failure.
use super::*;
use spaceprune_plugin_api::NullProgress;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};
use tempfile::tempdir;

fn task() -> TaskRequest {
    super::super::tests::metadata_task()
}

#[test]
fn missing_snapshot_does_not_erase_measured_siblings_or_count_shared_links_only_once() {
    let temp = tempdir().unwrap();
    let paths = [1, 3].map(|number| temp.path().join(format!(".snapshots/{number}/snapshot")));
    for path in &paths {
        fs::create_dir_all(path).unwrap();
    }
    fs::write(paths[0].join("file"), [0; 4096]).unwrap();
    fs::hard_link(paths[0].join("file"), paths[1].join("same-file")).unwrap();
    let outside = temp.path().join("outside");
    fs::File::create(&outside)
        .unwrap()
        .set_len(1 << 30)
        .unwrap();
    symlink(&outside, paths[0].join("link")).unwrap();
    let mut issues = Vec::new();
    let values = measure_snapshots(
        &task(),
        "home",
        temp.path(),
        vec![1, 2, 3],
        &Options::default(),
        &NullProgress,
        &OperationContext::default(),
        &mut issues,
    );
    assert_eq!(values.len(), 2);
    for number in [1, 3] {
        assert!((4096..1 << 30).contains(&values[&number]));
    }
    assert_eq!(issues.len(), 1);
    assert!(
        issues[0]
            .location
            .as_deref()
            .unwrap()
            .ends_with("/2/snapshot")
    );
    assert_eq!(
        issues[0].io_failure.as_ref().unwrap().kind,
        spaceprune_access::IoFailureKind::NotFound
    );
}

#[test]
fn failed_partial_output_does_not_hide_a_successful_snapshot() {
    let temp = tempdir().unwrap();
    for number in [1, 2] {
        fs::create_dir_all(temp.path().join(format!(".snapshots/{number}/snapshot"))).unwrap();
    }
    let du = temp.path().join("du-fixture");
    fs::write(&du, "#!/usr/bin/env python3\nimport sys\np=sys.argv[-1]\nsys.stdout.buffer.write(('1234\\t'+p+'\\0').encode())\nif '/1/snapshot' in p:\n sys.stderr.write('read failure')\n sys.exit(1)\n").unwrap();
    fs::set_permissions(&du, fs::Permissions::from_mode(0o755)).unwrap();
    let mut issues = Vec::new();
    let values = measure_snapshots(
        &task(),
        "home",
        temp.path(),
        vec![1, 2],
        &Options {
            du: du.to_string_lossy().into_owned(),
            ..Options::default()
        },
        &NullProgress,
        &OperationContext::default(),
        &mut issues,
    );
    assert_eq!(values, BTreeMap::from([(2, 1234)]));
    assert_eq!(issues.len(), 1);
    assert!(issues[0].message.contains("read failure"));
}

#[test]
fn snapshot_timeout_keeps_completed_sizes_and_does_not_cancel_siblings() {
    let temp = tempdir().unwrap();
    let du = temp.path().join("du-fixture");
    fs::write(&du, "#!/usr/bin/env python3\nimport sys,time\np=sys.argv[-1]\nif '/2/snapshot' in p: time.sleep(10)\nsys.stdout.buffer.write(('2048\\t'+p+'\\0').encode())\n").unwrap();
    fs::set_permissions(&du, fs::Permissions::from_mode(0o755)).unwrap();
    let mut context = OperationContext::default();
    context.budget.first_output_timeout = Duration::from_millis(300);
    let mut issues = Vec::new();
    let values = measure_snapshots(
        &task(),
        "home",
        temp.path(),
        vec![1, 2, 3],
        &Options {
            du: du.to_string_lossy().into_owned(),
            ..Options::default()
        },
        &NullProgress,
        &context,
        &mut issues,
    );
    assert_eq!(values, BTreeMap::from([(1, 2048), (3, 2048)]));
    assert_eq!(issues.len(), 1);
    assert_eq!(
        issues[0].io_failure.as_ref().unwrap().kind,
        spaceprune_access::IoFailureKind::TimedOut
    );
    assert!(!context.is_stopped());
}

#[test]
fn scan_attaches_failure_to_affected_snapshot_without_changing_selection() {
    use spaceprune_plugin_api::Handler;
    let temp = tempdir().unwrap();
    let root = temp.path().join("volume");
    let path = root.join(".snapshots/1/snapshot");
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("file"), [0; 4096]).unwrap();
    let snapper = temp.path().join("snapper-fixture");
    let volume = serde_json::to_string(&root).unwrap();
    fs::write(&snapper,format!("#!/usr/bin/env python3\nimport json,sys\nif 'list-configs' in sys.argv: print(json.dumps({{'configs':[{{'config':'home','subvolume':{volume}}}]}}))\nelse: print(json.dumps({{'home':[{{'number':1,'type':'single','cleanup':'timeline','date':'2001-02-03 04:05:06'}},{{'number':2,'type':'single','cleanup':'timeline','date':'2001-02-03 05:05:06'}}]}}))\n")).unwrap();
    fs::set_permissions(&snapper, fs::Permissions::from_mode(0o755)).unwrap();
    let mut request = task();
    request.options["snapper"] = serde_json::json!(snapper);
    let result = super::super::SnapperHandler
        .scan(&request, &NullProgress, &OperationContext::default())
        .unwrap();
    assert_eq!(result.items.len(), 2);
    assert!(result.items[0].logical_bytes.unwrap() >= 4096);
    assert!(!result.items[0].reason.contains("unknown"));
    assert_eq!(result.items[1].logical_bytes, None);
    assert!(result.items[1].reason.contains("home #2"));
    assert_eq!(
        result.items[0].recommendation,
        result.items[1].recommendation
    );
    assert!(
        result
            .items
            .iter()
            .all(|item| item.reclaimable_bytes.is_none())
    );
}
