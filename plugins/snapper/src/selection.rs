//! Snapshot visibility is independent of permission to select a cleanup target.

use super::*;

pub(super) fn in_scope(group: &SnapshotGroup, options: &Options) -> bool {
    options
        .snapshot_ids
        .as_ref()
        .is_none_or(|ids| group.numbers().iter().any(|id| ids.contains(id)))
        && snapshot_matches_filters(group, options)
}

pub(super) fn visible(
    group: &SnapshotGroup,
    options: &Options,
    blocker: Option<&PluginError>,
) -> bool {
    // Retain the observed inventory for a broken fixed selection without making
    // replacement objects selectable. Protected snapshots are always visible.
    blocker.is_some() || readonly::group_summary(group).protected || in_scope(group, options)
}

pub(super) fn apply_blocker(item: &mut CleanupItem, blocker: Option<&PluginError>) {
    if let Some(error) = blocker {
        if item.recommendation != Recommendation::Unavailable {
            item.reason = spaceprune_plugin_api::messages::message(
                "snapper.not_selectable",
                &[("p0", (error.message).to_string())],
            );
        }
        item.recommendation = Recommendation::Unavailable;
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn protected_and_stale_snapshot_inventory_cannot_enter_a_cleanup_plan() {
        let temp = tempfile::tempdir().unwrap();
        let snapper = temp.path().join("snapper");
        let records = json!({"home": [
            {"number": 0, "type": "single"},
            {"number": 10, "default": true, "type": "single", "date": "2020-01-01 00:00:00"},
            {"number": 11, "active": true, "type": "pre", "date": "2020-01-02 00:00:00"},
            {"number": 12, "type": "post", "pre-number": 11, "date": "2020-01-02 01:00:00"},
            {"number": 13, "type": "single", "cleanup": "timeline", "date": "2020-01-03 00:00:00"}
        ]});
        fs::write(&snapper, format!(
            "#!/bin/sh\ncase \"$*\" in\n*list-configs*) echo '{}' ;;\n*list*) echo '{}' ;;\n*) exit 99 ;;\nesac\n",
            json!({"configs": [{"config": "home", "subvolume": temp.path()}]}), records
        )).unwrap();
        fs::set_permissions(&snapper, fs::Permissions::from_mode(0o755)).unwrap();
        let mut task = super::super::tests::metadata_task();
        task.options["snapper"] = json!(snapper);
        task.options["du"] = json!("missing-fixture-du");
        task.options["snapshot-types"] = json!(["single"]);
        task.options["cleanup-algorithms"] = json!(["timeline"]);
        task.recommend = json!({"enabled": true});
        let scan = SnapperHandler
            .scan(&task, &NullProgress, &OperationContext::default())
            .unwrap();
        assert_eq!(
            scan.items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["home:0", "home:10", "home:11+12", "home:13"]
        );
        assert!(
            scan.items[..3]
                .iter()
                .all(|item| item.recommendation == Recommendation::Unavailable)
        );
        assert_eq!(scan.items[3].recommendation, Recommendation::Recommended);

        task.options["snapshot-ids"] = json!([99]);
        task.options["snapshot-host-id"] = json!(snapper_host_id().unwrap());
        task.options["snapshot-identities"] =
            json!([{"number": 99, "type": "single", "date": "2019-01-01 00:00:00"}]);
        let stale = SnapperHandler
            .scan(&task, &NullProgress, &OperationContext::default())
            .unwrap();
        assert_eq!(stale.items.len(), 4);
        assert!(
            stale
                .items
                .iter()
                .all(|item| item.recommendation == Recommendation::Unavailable)
        );
        assert_eq!(
            stale.issues[0].code.as_deref(),
            Some("snapshot-identity-changed")
        );
        assert!(stale.items[0].reason.contains("cannot be removed"));
        assert!(stale.items[3].reason.contains("removed or replaced"));
        let report =
            spaceprune_engine::ScanReport::from_report(spaceprune_engine::ScanReportData {
                scan_id: spaceprune_engine::ScanReport::new_scan_id(),
                config_name: "stale snapshots".into(),
                config_source: "test".into(),
                cancelled: false,
                stop_reason: None,
                modules: vec![spaceprune_engine::ModuleReport {
                    id: task.module_id.clone(),
                    name: "Snapshots".into(),
                    description: String::new(),
                    tasks: vec![spaceprune_engine::TaskReport {
                        module_id: task.module_id.clone(),
                        task_id: task.task_id.clone(),
                        name: task.name.clone(),
                        handler_id: task.handler_id.clone(),
                        request: task,
                        items: stale.items,
                        issues: stale.issues,
                        error: None,
                    }],
                }],
            });
        assert!(report.selectable_keys().is_empty());
        assert_eq!(report.plan_recommended().unwrap().item_count(), 0);
    }
}
