//! Ordinary-user discovery only; elevated workers inspect frozen sources and never run tools.
use super::*;
use std::process::Command;

/// Discover installed tool caches without measuring them or enabling cleanup recommendations.
pub fn discover_tasks(context: &OperationContext) -> io::Result<Vec<TaskRequest>> {
    Ok(probe_tasks(context)?
        .into_iter()
        .filter(|probe| probe.issue.is_none())
        .map(|probe| probe.task)
        .collect())
}

/// Probe each installed tool independently. Missing executables are omitted, without reading caches.
pub fn probe_tasks(context: &OperationContext) -> io::Result<Vec<CapabilityProbe>> {
    let mut probes = Vec::new();
    #[cfg(target_os = "linux")]
    for kind in Kind::ALL {
        context.check()?;
        let action = action(kind, context);
        if action.paths.is_empty() {
            continue;
        }
        probes.push(CapabilityProbe {
            task: TaskRequest {
                module_id: "developer-tools".into(),
                task_id: kind.id().into(),
                name: kind.name().into(),
                handler_id: format!("{PLUGIN_ID}/{}", kind.id()),
                options: json!({}),
                recommend: json!({"enabled":false}),
                tags: vec!["developer-cache".into()],
            },
            issue: None,
        });
    }
    probes.extend(docker::probe_tasks(context));
    context.check()?;
    Ok(probes)
}

/// Preserve the original platform discovery API without advertising duplicate new capabilities.
#[cfg(target_os = "linux")]
pub fn discover_legacy_go_task(context: &OperationContext) -> io::Result<Option<TaskRequest>> {
    context.check()?;
    let observed = action(Kind::GoModCache, context);
    context.check()?;
    Ok((!observed.paths.is_empty()).then(|| TaskRequest {
        module_id: "system-linux".into(),
        task_id: "go-modcache".into(),
        name: Kind::GoModCache.name().into(),
        handler_id: LEGACY_GO_HANDLER.into(),
        options: json!({}),
        recommend: json!({"enabled":false}),
        tags: vec!["system-lifecycle".into()],
    }))
}

#[cfg(target_os = "linux")]
pub(super) fn action(kind: Kind, context: &OperationContext) -> Action {
    let program = match kind {
        Kind::GoModCache | Kind::GoBuildCache => "go",
        Kind::CargoRegistryCache => "cargo",
        Kind::NpmCache => "npm",
    };
    if !command_exists(program) {
        let mut action = cache::scan_action(false, None);
        action.id = kind.id().into();
        action.unavailable_reason = Some(messages::message(
            "developer.tool_missing",
            &[("program", program.into())],
        ));
        return action;
    }
    let path = match kind {
        Kind::GoModCache => go_path("GOMODCACHE", context),
        Kind::GoBuildCache => go_path("GOCACHE", context),
        Kind::CargoRegistryCache => cargo_cache(),
        Kind::NpmCache => npm_cache(context),
    };
    let mut action = cache::scan_action(path.is_some(), path);
    action.id = kind.id().into();
    if action.paths.is_empty() {
        action.unavailable_reason = Some(messages::text("developer.cache_unavailable").into());
    }
    action
}

#[cfg(target_os = "linux")]
fn cargo_cache() -> Option<PathBuf> {
    if !command_exists("cargo") {
        return None;
    }
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))?;
    // Cargo accepts relative CARGO_HOME; resolve it once in the invoking user's context.
    std::path::absolute(home)
        .ok()
        .map(|home| home.join("registry/cache"))
}
#[cfg(target_os = "linux")]
fn npm_cache(context: &OperationContext) -> Option<PathBuf> {
    let mut command = native_system_command("npm").ok()?;
    command
        .args([
            "config",
            "get",
            "cache",
            "--ignore-scripts",
            "--no-update-notifier",
        ])
        .current_dir("/");
    query(command, context).map(|root| root.join("_cacache"))
}
#[cfg(target_os = "linux")]
fn go_path(key: &str, context: &OperationContext) -> Option<PathBuf> {
    let mut command = native_system_command("go").ok()?;
    command
        .args(["env", key])
        .current_dir("/")
        .env("GOTOOLCHAIN", "local")
        .env("GOWORK", "off")
        .env("GO111MODULE", "off")
        .env("GOFLAGS", "");
    query(command, context)
}
#[cfg(target_os = "linux")]
fn query(command: Command, context: &OperationContext) -> Option<PathBuf> {
    let output = spaceprune_access::process::capture(
        command,
        &context.child(std::time::Duration::from_secs(30)),
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let bytes = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let path = PathBuf::from(OsString::from_vec(bytes.to_vec()));
        path.is_absolute().then_some(path)
    }
    #[cfg(not(unix))]
    {
        let _ = bytes;
        None
    }
}

pub(super) fn native_system_command(program: &str) -> io::Result<Command> {
    #[cfg(target_os = "linux")]
    if spaceprune_access::process::is_elevated() {
        return Err(io::Error::other(messages::text(
            "developer.discovery_ordinary_only",
        )));
    }
    Ok(Command::new(program))
}
pub(super) fn command_exists(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|root| {
            let path = root.join(if cfg!(windows) {
                format!("{program}.exe")
            } else {
                program.to_owned()
            });
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::metadata(path)
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            }
            #[cfg(not(unix))]
            {
                path.is_file()
            }
        })
    })
}
