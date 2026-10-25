use spaceprune_access::process::system_program as trusted_program;
use std::path::Path;

use spaceprune_plugin_api::{HandlerScanResult, NullProgress, OperationContext, TaskRequest};

use crate::HelperError;

pub(super) fn validate_task(task: &TaskRequest) -> Result<(), HelperError> {
    if task.handler_id != "builtin.snapper/snapshots"
        || !matches!(
            task.options
                .get("scan-mode")
                .and_then(serde_json::Value::as_str),
            None | Some("snapshots" | "rust-targets")
        )
    {
        return Err(HelperError);
    }
    for program in ["snapper", "du", "find"] {
        if let Some(value) = task.options.get(program) {
            let name = value.as_str().ok_or(HelperError)?;
            if name != program
                && !["/usr/bin", "/usr/sbin", "/bin", "/sbin"]
                    .iter()
                    .any(|directory| Path::new(name) == Path::new(directory).join(program))
            {
                return Err(HelperError);
            }
        }
    }
    if let Some(configs) = task.options.get("configs") {
        let configs = configs.as_array().ok_or(HelperError)?;
        if configs.len() > 64
            || configs.iter().any(|value| {
                value.as_str().is_none_or(|name| {
                    name.is_empty()
                        || name.len() > 128
                        || !name
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                })
            })
        {
            return Err(HelperError);
        }
    }
    Ok(())
}

pub(super) fn scan(
    task: &TaskRequest,
    context: &OperationContext,
) -> Result<HandlerScanResult, String> {
    validate_task(task).map_err(|error| error.to_string())?;
    let mut task = task.clone();
    for program in ["snapper", "du", "find"] {
        let executable = trusted_program(program)
            .ok_or_else(|| format!("A trusted system {program} executable is required"))?;
        task.options[program] = serde_json::json!(executable);
    }
    spaceprune_plugin_snapper::scan_readonly(&task, &NullProgress, context)
        .map_err(|error| error.to_string())
}

pub(super) fn inventory(
    task: &TaskRequest,
    context: &OperationContext,
) -> Result<spaceprune_plugin_snapper::SnapperInventory, String> {
    validate_task(task).map_err(|error| error.to_string())?;
    let configs = task
        .options
        .get("configs")
        .and_then(serde_json::Value::as_array)
        .ok_or("Missing configurations")?;
    if configs.len() > 1 {
        return Err("Inventory requires one configuration".to_owned());
    }
    let program = trusted_program("snapper").ok_or("A trusted Snapper executable is required")?;
    spaceprune_plugin_snapper::snapper_inventory_unprivileged(
        program.to_str().ok_or("Invalid system program path")?,
        configs.first().and_then(serde_json::Value::as_str),
        context,
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use spaceprune_access::process::trusted_executable;
    use std::{fs, os::unix::fs::PermissionsExt};

    fn task() -> TaskRequest {
        TaskRequest {
            module_id: "fixture".to_owned(),
            task_id: "snapshots".to_owned(),
            name: "Snapshot fixture".to_owned(),
            handler_id: "builtin.snapper/snapshots".to_owned(),
            options: serde_json::json!({"scan-mode": "snapshots", "configs": ["root", "home"]}),
            recommend: serde_json::json!({}),
            tags: vec![],
        }
    }

    #[test]
    fn snapshot_requests_reject_commands_plugins_and_unsupported_modes() {
        validate_task(&task()).unwrap();
        let mut targets = task();
        targets.options["scan-mode"] = serde_json::json!("rust-targets");
        validate_task(&targets).unwrap();
        for (key, value) in [
            ("scan-mode", "shell"),
            ("snapper", "/tmp/snapper"),
            ("du", "/usr/local/bin/du"),
            ("find", "sh"),
            ("du", "du --help"),
        ] {
            let mut invalid = task();
            invalid.options[key] = serde_json::json!(value);
            assert!(validate_task(&invalid).is_err());
        }
        let mut invalid = task();
        invalid.handler_id = "external/execute".to_owned();
        assert!(validate_task(&invalid).is_err());
        let mut invalid = task();
        invalid.options["configs"] = serde_json::json!(["../../root"]);
        assert!(validate_task(&invalid).is_err());
    }

    #[test]
    fn executable_trust_rejects_user_owned_or_writable_locations() {
        let temporary = tempfile::tempdir().unwrap();
        let executable = temporary.path().join("snapper");
        fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(trusted_executable(&executable).is_none());
        assert!(trusted_program("du").is_some());
    }
}
