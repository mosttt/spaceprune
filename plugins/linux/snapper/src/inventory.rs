//! Bounded Snapper inventory commands; native authorization is owned by the host.

use super::*;

#[cfg(target_os = "linux")]
mod service;

pub(super) fn list_configs_cancellable(
    snapper: &str,
    context: &OperationContext,
) -> PluginResult<BTreeMap<String, PathBuf>> {
    #[derive(Deserialize)]
    struct Envelope {
        configs: Vec<Config>,
    }
    #[derive(Deserialize)]
    struct Config {
        config: String,
        subvolume: PathBuf,
    }
    let value: Envelope = capture_json(snapper, ["--jsonout", "list-configs"], None, context)?;
    Ok(value
        .configs
        .into_iter()
        .map(|config| (config.config, config.subvolume))
        .collect())
}

pub(super) fn list_snapshots_cancellable(
    snapper: &str,
    config: &str,
    include_used_space: bool,
    context: &OperationContext,
) -> PluginResult<Vec<Snapshot>> {
    let mut args = vec!["--jsonout", "--iso", "--utc", "-c", config, "list"];
    if !include_used_space {
        args.push("--disable-used-space");
    }
    let value: BTreeMap<String, Vec<Snapshot>> =
        capture_json(snapper, &args, Some(config), context)?;
    value.get(config).cloned().ok_or_else(|| {
        PluginError::new(
            "invalid-snapper-json",
            spaceprune_plugin_api::messages::message(
                "snapper.config_not_in_output",
                &[("config", (config).to_string())],
            ),
        )
    })
}

pub(super) fn capture_json<T, I, S>(
    program: &str,
    args: I,
    config: Option<&str>,
    context: &OperationContext,
) -> PluginResult<T>
where
    T: for<'de> Deserialize<'de>,
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = command(program);
    command.env("LC_ALL", "C").args(args);
    let output = size::capture_cancellable(command, context).map_err(|error| {
        let mut failure = PluginError::new(
            "snapper-unavailable",
            spaceprune_plugin_api::messages::message(
                "snapper.execute_failed",
                &[
                    ("program", (program).to_string()),
                    ("error", (error).to_string()),
                ],
            ),
        );
        failure.io_failure = Some(spaceprune_access::error::classify(&error));
        failure
    })?;
    if !output.status.success() {
        let detail = command_failure(
            spaceprune_plugin_api::messages::text("snapper.command"),
            &output,
        );
        #[cfg(target_os = "linux")]
        if service::is_system_snapper(program)
            && let Err(mut error) = service::inspect_access(config, context)
        {
            error.message = format!("{detail}; {}", error.message);
            return Err(error);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = config;
        return Err(PluginError::new("snapper-failed", detail));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| {
        PluginError::new(
            "invalid-snapper-json",
            spaceprune_plugin_api::messages::message(
                "snapper.parse_failed",
                &[("error", (error).to_string())],
            ),
        )
    })
}
