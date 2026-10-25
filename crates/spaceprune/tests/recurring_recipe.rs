//! Exercise saved recurring rules through the same application facade used by CLI and Desktop.

use spaceprune::api::{
    CancellationToken, ConfigStore, ItemKey, NullProgress, Recommendation, ScanRequest, SpacePrune,
};
use std::{
    fs::{self, File, FileTimes},
    io::Write,
    path::Path,
    time::{Duration, SystemTime},
};

fn fixture_file(path: &Path, contents: &[u8], modified: SystemTime) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut file = File::create(path).unwrap();
    file.write_all(contents).unwrap();
    file.set_times(FileTimes::new().set_modified(modified))
        .unwrap();
}

#[test]
fn saved_recurring_rule_finds_new_matches_without_widening_the_confirmed_selection() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("projects");
    let now = SystemTime::now();
    let old = now - Duration::from_secs(30 * 24 * 60 * 60);
    let first = root.join("alpha/cache/first.log");
    let recent = root.join("alpha/cache/recent.log");
    let small = root.join("alpha/cache/small.log");
    let excluded = root.join("alpha/cache/keep.log");
    let unrelated = root.join("alpha/documents/notes.log");
    fixture_file(&first, &[1; 32], old);
    fixture_file(&recent, &[2; 32], now);
    fixture_file(&small, &[3; 2], old);
    fixture_file(&excluded, &[4; 32], old);
    fixture_file(&unrelated, &[5; 32], old);

    let config_path = directory.path().join("config.toml");
    let store = ConfigStore::new(&config_path);
    store.save_theme_id("aurora").unwrap();
    store
        .upsert_recipe(
            "recurring",
            &format!(
                r#"schema-version = 2
name = "Recurring project logs"
usage = "recurring"
[modules.projects]
name = "Projects"
[modules.projects.tasks.logs]
name = "Reusable log rule"
handler = "builtin.filesystem/paths"
[modules.projects.tasks.logs.options]
roots = [{{ path = {:?}, scope = "directory_subtree", include = ["*/cache/*.log"], exclude = ["*/cache/keep.log"] }}]
files = true
directories = false
elevation = "never"
[modules.projects.tasks.logs.recommend]
older-than = "7d"
min-size = "8 B"
"#,
                root.to_string_lossy()
            ),
        )
        .unwrap();
    store.save_recipe("recurring").unwrap();
    let saved = store.export_recipe_bundle("recurring").unwrap();
    let cancellation = CancellationToken::default();

    let application = SpacePrune::load(Some(&config_path)).unwrap();
    let first_scan = application
        .scan(&ScanRequest::default(), &NullProgress, &cancellation)
        .unwrap();
    assert!(!first_scan.has_errors());
    assert!(first_scan.modules[0].tasks[0].issues.is_empty());
    let items = &first_scan.modules[0].tasks[0].items;
    assert_eq!(items.len(), 3);
    for item in items {
        assert_eq!(
            item.recommendation,
            if Path::new(&item.location) == first {
                Recommendation::Recommended
            } else {
                Recommendation::Optional
            }
        );
    }
    let first_plan = first_scan.plan_recommended().unwrap();
    assert_eq!(first_plan.item_count(), 1);
    let result = application.execute(&first_plan, &NullProgress, &cancellation);
    assert!(!result.has_errors());
    assert_eq!(result.succeeded(), 1);
    assert!(!first.exists());
    assert_eq!(fs::read(&recent).unwrap(), [2; 32]);
    assert_eq!(fs::read(&small).unwrap(), [3; 2]);
    drop(application);

    let second = root.join("beta/cache/second.log");
    let second_excluded = root.join("beta/cache/keep.log");
    let nonmatching = root.join("beta/cache/notes.txt");
    fixture_file(&second, &[6; 32], old);
    fixture_file(&second_excluded, &[7; 32], old);
    fixture_file(&nonmatching, &[8; 32], old);
    let application = SpacePrune::load(Some(&config_path)).unwrap();
    let second_scan = application
        .scan(&ScanRequest::default(), &NullProgress, &cancellation)
        .unwrap();
    assert!(!second_scan.has_errors());
    let task = &second_scan.modules[0].tasks[0];
    assert!(task.issues.is_empty());
    assert_eq!(task.items.len(), 3);
    let mut selected = second_scan.recommended_keys();
    assert_eq!(selected.len(), 1);
    let new_item = task
        .items
        .iter()
        .find(|item| Path::new(&item.location) == second)
        .unwrap();
    assert_eq!(new_item.recommendation, Recommendation::Recommended);
    let optional = task
        .items
        .iter()
        .find(|item| Path::new(&item.location) == recent)
        .unwrap();
    assert_eq!(optional.recommendation, Recommendation::Optional);
    selected.insert(ItemKey {
        module_id: "projects".to_owned(),
        task_id: "logs".to_owned(),
        item_id: optional.id.clone(),
    });
    let second_plan = second_scan.plan_selected(&selected).unwrap();
    assert_eq!(second_plan.item_count(), 2);
    let after_scan = root.join("beta/cache/after-scan.log");
    fixture_file(&after_scan, &[9; 32], old);
    let result = application.execute(&second_plan, &NullProgress, &cancellation);
    assert!(!result.has_errors());
    assert_eq!(result.succeeded(), 2);
    assert!(!second.exists());
    assert!(!recent.exists());
    for (path, expected) in [
        (&small, vec![3; 2]),
        (&excluded, vec![4; 32]),
        (&unrelated, vec![5; 32]),
        (&second_excluded, vec![7; 32]),
        (&nonmatching, vec![8; 32]),
        (&after_scan, vec![9; 32]),
    ] {
        assert_eq!(fs::read(path).unwrap(), expected, "{}", path.display());
    }
    assert_eq!(store.export_recipe_bundle("recurring").unwrap(), saved);
}
