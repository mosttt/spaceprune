//! Docker CLI transport. Every engine call is pinned to a local endpoint.
use super::*;
use std::process::Command;

pub(super) trait Runner {
    fn run(
        &self,
        endpoint: Option<&str>,
        args: &[&str],
        context: &OperationContext,
    ) -> PluginResult<String>;
}

pub(super) struct Native;
impl Runner for Native {
    fn run(
        &self,
        endpoint: Option<&str>,
        args: &[&str],
        context: &OperationContext,
    ) -> PluginResult<String> {
        context.check().map_err(command_error)?;
        let mut command: Command =
            super::super::discovery::native_system_command("docker").map_err(command_error)?;
        command.env("LC_ALL", "C");
        if let Some(endpoint) = endpoint {
            // Context and custom builder environment must not override the frozen engine.
            for name in [
                "DOCKER_CONTEXT",
                "DOCKER_HOST",
                "DOCKER_TLS",
                "DOCKER_TLS_VERIFY",
                "DOCKER_CERT_PATH",
                "BUILDX_BUILDER",
                "BUILDKIT_HOST",
            ] {
                command.env_remove(name);
            }
            command.args(["--host", endpoint]);
        }
        command.args(args);
        let output = spaceprune_access::process::capture(
            command,
            &context.child(std::time::Duration::from_secs(120)),
        )
        .map_err(command_error)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let (code, key) =
                if stderr.contains("permission denied") || stderr.contains("Access is denied") {
                    ("docker-access-denied", "docker.access_denied")
                } else if stderr.contains("Cannot connect")
                    || stderr.contains("Is the docker daemon running")
                    || stderr.contains("connection refused")
                    || stderr.contains("The system cannot find the file specified")
                {
                    ("docker-daemon-unavailable", "docker.daemon_unavailable")
                } else if stderr.contains("client version") || stderr.contains("API version") {
                    ("docker-version-unsupported", "docker.version_unsupported")
                } else {
                    ("docker-command-failed", "docker.command_failed")
                };
            return Err(PluginError::new(
                code,
                messages::message(
                    key,
                    &[(
                        "error",
                        String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                    )],
                ),
            ));
        }
        String::from_utf8(output.stdout).map_err(|_| invalid())
    }
}

pub(super) fn local_endpoint(value: &str) -> bool {
    if let Some(path) = value.strip_prefix("unix://") {
        return path.starts_with('/')
            && !path.starts_with("//")
            && !path.contains(['\n', '\r', '\0'])
            && !path.split('/').any(|part| part == "..");
    }
    value.starts_with("npipe:////./pipe/")
        && value.len() > "npipe:////./pipe/".len()
        && !value["npipe:////./pipe/".len()..].contains(['/', '\\', '\n', '\r', '\0'])
}

pub(super) fn probe(runner: &dyn Runner, context: &OperationContext) -> PluginResult<Binding> {
    let selected_context = std::env::var("DOCKER_CONTEXT")
        .ok()
        .filter(|s| !s.is_empty());
    let selected_host = std::env::var("DOCKER_HOST").ok().filter(|s| !s.is_empty());
    let endpoint = if selected_context.is_none()
        && let Some(host) = selected_host
    {
        host
    } else {
        let mut args = vec!["context", "inspect"];
        if let Some(name) = selected_context.as_deref() {
            args.push(name);
        }
        let contexts: serde_json::Value =
            serde_json::from_str(&runner.run(None, &args, context)?).map_err(|_| invalid())?;
        contexts
            .as_array()
            .filter(|items| items.len() == 1)
            .and_then(|items| items[0].pointer("/Endpoints/docker/Host"))
            .and_then(|host| host.as_str())
            .ok_or_else(invalid)?
            .to_owned()
    };
    if !local_endpoint(&endpoint) {
        return Err(PluginError::new(
            "docker-remote-endpoint",
            messages::text("docker.remote_endpoint"),
        ));
    }
    let engine = engine_id(runner, &endpoint, context)?;
    Ok(Binding { endpoint, engine })
}

pub(super) fn engine_id(
    runner: &dyn Runner,
    endpoint: &str,
    context: &OperationContext,
) -> PluginResult<String> {
    if !local_endpoint(endpoint) {
        return Err(invalid());
    }
    let value: String = serde_json::from_str(&runner.run(
        Some(endpoint),
        &["info", "--format", "{{json .ID}}"],
        context,
    )?)
    .map_err(|_| invalid())?;
    if value.is_empty() {
        return Err(invalid());
    }
    Ok(value)
}

fn command_error(error: impl std::fmt::Display) -> PluginError {
    PluginError::new(
        "docker-unavailable",
        messages::message("docker.unavailable", &[("error", error.to_string())]),
    )
}

// Help probes are read-only and do not enumerate objects or bootstrap a builder.
pub(super) fn interface(
    kind: DockerKind,
    binding: &Binding,
    runner: &dyn Runner,
    context: &OperationContext,
) -> PluginResult<()> {
    let requirements: Vec<(Vec<&str>, &[&str])> = match kind {
        DockerKind::BuildCache => vec![
            (vec!["buildx", "du", "--help"], &["--format", "--builder"]),
            (
                vec!["buildx", "prune", "--help"],
                &["--filter", "--builder"],
            ),
        ],
        DockerKind::DanglingImages | DockerKind::UnusedImages => vec![
            (vec!["image", "ls", "--help"], &["--filter", "--no-trunc"]),
            (vec!["image", "rm", "--help"], &["--no-prune"]),
        ],
        DockerKind::StoppedContainers => vec![
            (vec!["container", "inspect", "--help"], &["--size"]),
            (vec!["container", "rm", "--help"], &[]),
        ],
    };
    for (args, options) in requirements {
        let help = runner
            .run(Some(&binding.endpoint), &args, context)
            .map_err(|error| {
                PluginError::new(
                    "docker-interface-unavailable",
                    messages::message("docker.interface_unavailable", &[("error", error.message)]),
                )
            })?;
        if options.iter().any(|option| !help.contains(option)) {
            return Err(PluginError::new(
                "docker-interface-unavailable",
                messages::text("docker.interface_unsupported"),
            ));
        }
    }
    Ok(())
}
