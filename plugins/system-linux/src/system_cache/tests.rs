use super::{Kind, Options, coredump_action, executable_action, package_action};

#[test]
#[cfg(unix)]
fn a_failed_system_command_preserves_side_effect_uncertainty() {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let command = temp.path().join("partial-fixture");
    let effect = temp.path().join("effect");
    fs::write(&command, "#!/bin/sh\nprintf 'effect' > \"$1\"\nexit 9\n").unwrap();
    fs::set_permissions(&command, fs::Permissions::from_mode(0o755)).unwrap();
    let task: TaskRequest = serde_json::from_value(json!({
        "module_id": "fixture", "task_id": "fixture", "name": "Fixture",
        "handler_id": "builtin.system-linux/journal", "options": {}, "recommend": {}, "tags": [],
    }))
    .unwrap();
    let action = Action {
        program: command.to_str().unwrap().into(),
        args: vec![effect.to_str().unwrap().into()],
        id: "fixture".into(),
        paths: vec![],
        available: true,
        unavailable_reason: None,
    };
    let result = run_action(
        &action,
        &task,
        &spaceprune_plugin_api::NullProgress,
        &OperationContext::default(),
    );
    assert!(matches!(result, Err(ActionFailure::Unconfirmed(_))));
    assert_eq!(fs::read(&effect).unwrap(), b"effect");
}

#[test]
fn rejects_unknown_package_manager_as_unavailable() {
    let action = package_action("unknown-manager");
    assert!(!action.available);
}

#[test]
fn rejects_a_plan_item_for_another_action() {
    let error = executable_action(
        Kind::Journal,
        &Options::default(),
        "forged-action",
        &spaceprune_plugin_api::OperationContext::default(),
    )
    .expect_err("a mismatched action must be rejected");
    assert!(error.contains("does not match"));
}

#[test]
fn coredumps_use_systemd_tmpfiles_retention_rules() {
    let action = coredump_action(true);
    assert_eq!(action.program, "systemd-tmpfiles");
    assert_eq!(
        action.args,
        ["--clean", "--prefix=/var/lib/systemd/coredump"]
    );
}
