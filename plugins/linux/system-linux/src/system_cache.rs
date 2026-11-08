//! Linux 软件包缓存、systemd 资源和领域清理平台能力。

mod command;
#[cfg(target_os = "linux")]
pub(crate) mod discovery;
mod execution;
pub(crate) mod measurement;
#[cfg(target_os = "linux")]
pub(crate) mod scan;
use command::{ActionFailure, command_exists, effective_uid, run_action};

#[cfg(all(test, unix))]
use std::ffi::OsString;
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::Deserialize;
use serde_json::json;
use spaceprune_engine::{EngineError, PluginRegistry, fs as core_fs};
use spaceprune_plugin_api::messages;
use spaceprune_plugin_api::{
    CleanupItem, Handler, HandlerCleanResult, HandlerDescriptor, HandlerIssue, HandlerScanResult,
    IssueLevel, ItemFailure, OperationContext, PluginError, PluginResult, ProgressEvent,
    ProgressPhase, ProgressSink, Recommendation, TaskRequest,
};

/// 将全部 Linux 系统 Handler 注册到执行器。
pub fn register(registry: &mut PluginRegistry) -> Result<(), EngineError> {
    for kind in [
        Kind::PackageCache,
        Kind::Journal,
        Kind::Tmpfiles,
        Kind::Coredumps,
    ] {
        registry.register(Arc::new(SystemHandler { kind }))?;
    }
    spaceprune_plugin_developer_tools::register_legacy_go(registry)
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    PackageCache,
    Journal,
    Tmpfiles,
    Coredumps,
}

impl Kind {
    const fn id(self) -> &'static str {
        match self {
            Self::PackageCache => "package-cache",
            Self::Journal => "journal",
            Self::Tmpfiles => "tmpfiles",
            Self::Coredumps => "coredumps",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::PackageCache => messages::text("linux.package_cache"),
            Self::Journal => messages::text("linux.journal"),
            Self::Tmpfiles => messages::text("linux.tmpfiles"),
            Self::Coredumps => messages::text("linux.coredumps"),
        }
    }
}

#[derive(Debug)]
struct SystemHandler {
    kind: Kind,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct Options {
    manager: String,
    vacuum_time: String,
    elevation: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            manager: "auto".to_owned(),
            vacuum_time: "30d".to_owned(),
            elevation: "auto".to_owned(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Recommend {
    enabled: bool,
}

impl Default for Recommend {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug)]
struct Action {
    id: String,
    paths: Vec<PathBuf>,
    program: String,
    args: Vec<String>,
    available: bool,
    unavailable_reason: Option<String>,
}

impl Handler for SystemHandler {
    fn descriptor(&self) -> HandlerDescriptor {
        HandlerDescriptor {
            size_accounting: Default::default(),
            id: format!("builtin.system-linux/{}", self.kind.id()),
            name: self.kind.name().to_owned(),
            description: messages::text("linux.handler_description").to_owned(),
            platforms: vec!["linux".to_owned()],
            options_schema: json!({
                "type": "object",
                "properties": {
                    "manager": {"type": "string"},
                    "vacuum-time": {"type": "string"},
                    "elevation": {"enum": ["auto", "never", "sudo", "pkexec"]}
                }
            }),
            recommend_schema: json!({
                "type": "object",
                "properties": {"enabled": {"type": "boolean"}}
            }),
        }
    }

    fn validate(&self, task: &TaskRequest) -> PluginResult<()> {
        let (options, _) = parse(task)?;
        if !matches!(
            options.elevation.as_str(),
            "auto" | "never" | "sudo" | "pkexec"
        ) {
            return Err(PluginError::new(
                "invalid-elevation",
                messages::text("linux.invalid_elevation"),
            ));
        }
        if matches!(self.kind, Kind::Journal) {
            core_fs::parse_duration(&options.vacuum_time)?;
        }
        Ok(())
    }

    fn scan(
        &self,
        task: &TaskRequest,
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerScanResult> {
        #[cfg(target_os = "linux")]
        return scan::ordinary(self.kind, task, progress, context);
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (task, progress, context);
            Err(PluginError::new(
                "unsupported-platform",
                messages::text("linux.unsupported_inspection"),
            ))
        }
    }

    fn cleanup_authorization_method(&self, task: &TaskRequest) -> PluginResult<Option<String>> {
        let (options, _) = parse(task)?;
        if effective_uid() == Some(0) || options.elevation == "never" {
            return Ok(None);
        }
        Ok(Some(
            match options.elevation.as_str() {
                "sudo" => "sudo",
                "pkexec" => "pkexec",
                _ => spaceprune_plugin_api::automatic_linux_authorization_method(),
            }
            .to_owned(),
        ))
    }

    fn clean(
        &self,
        task: &TaskRequest,
        items: &[CleanupItem],
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> PluginResult<HandlerCleanResult> {
        self.clean_with_access(task, items, progress, context, None)
    }

    fn clean_batch_with_access(
        &self,
        groups: &[spaceprune_plugin_api::CleanupBatchGroup<'_>],
        progress: &dyn ProgressSink,
        context: &OperationContext,
        access: Option<&dyn spaceprune_plugin_api::CleanupAccess>,
    ) -> Vec<PluginResult<HandlerCleanResult>> {
        spaceprune_plugin_api::dispatch_authorized_cleanup(self, groups, progress, context, access)
    }
}

fn parse(task: &TaskRequest) -> PluginResult<(Options, Recommend)> {
    let options = serde_json::from_value(task.options.clone()).map_err(|error| {
        PluginError::new(
            "invalid-options",
            messages::message("linux.invalid_options", &[("error", error.to_string())]),
        )
    })?;
    let recommend = serde_json::from_value(task.recommend.clone()).map_err(|error| {
        PluginError::new(
            "invalid-recommend",
            messages::message("linux.invalid_recommend", &[("error", error.to_string())]),
        )
    })?;
    Ok((options, recommend))
}

fn action(kind: Kind, options: &Options, _context: &OperationContext) -> Action {
    match kind {
        Kind::PackageCache => package_action(&options.manager),
        Kind::Journal => Action {
            id: "systemd-journal".to_owned(),
            paths: vec!["/var/log/journal".into(), "/run/log/journal".into()],
            program: "journalctl".to_owned(),
            args: vec![format!("--vacuum-time={}", options.vacuum_time)],
            available: command_exists("journalctl"),
            unavailable_reason: Some(messages::message(
                "linux.program_missing",
                &[("program", "journalctl".into())],
            )),
        },
        Kind::Tmpfiles => Action {
            id: "systemd-tmpfiles".to_owned(),
            paths: vec!["/tmp".into(), "/var/tmp".into()],
            program: "systemd-tmpfiles".to_owned(),
            args: vec!["--clean".to_owned()],
            available: command_exists("systemd-tmpfiles"),
            unavailable_reason: Some(messages::message(
                "linux.program_missing",
                &[("program", "systemd-tmpfiles".into())],
            )),
        },
        Kind::Coredumps => coredump_action(command_exists("systemd-tmpfiles")),
    }
}

fn coredump_action(available: bool) -> Action {
    Action {
        id: "systemd-coredumps".to_owned(),
        paths: vec!["/var/lib/systemd/coredump".into()],
        program: "systemd-tmpfiles".to_owned(),
        args: vec![
            "--clean".to_owned(),
            "--prefix=/var/lib/systemd/coredump".to_owned(),
        ],
        available,
        unavailable_reason: Some(messages::message(
            "linux.program_missing",
            &[("program", "systemd-tmpfiles".into())],
        )),
    }
}

fn executable_action(
    kind: Kind,
    options: &Options,
    item_id: &str,
    context: &OperationContext,
) -> Result<Action, String> {
    let action = action(kind, options, context);
    if item_id != action.id {
        return Err(messages::message(
            "linux.action_mismatch",
            &[
                ("item_id", item_id.to_owned()),
                ("action", action.id.clone()),
            ],
        ));
    }
    if !action.available {
        return Err(action
            .unavailable_reason
            .clone()
            .unwrap_or_else(|| messages::text("linux.action_unavailable").to_owned()));
    }
    Ok(action)
}

fn package_action(configured: &str) -> Action {
    let manager = if configured == "auto" {
        detect_package_manager()
    } else {
        Some(configured.to_owned())
    };
    match manager.as_deref() {
        Some("zypper") => package("zypper", "/var/cache/zypp/packages", ["clean", "--all"]),
        Some("apt") | Some("apt-get") => package("apt-get", "/var/cache/apt/archives", ["clean"]),
        Some("dnf") => package("dnf", "/var/cache/dnf", ["clean", "packages"]),
        Some("pacman") => package("pacman", "/var/cache/pacman/pkg", ["-Sc", "--noconfirm"]),
        Some(other) => Action {
            id: format!("package-{other}"),
            paths: Vec::new(),
            program: other.to_owned(),
            args: Vec::new(),
            available: false,
            unavailable_reason: Some(messages::message(
                "linux.unsupported_package_manager",
                &[("manager", other.to_owned())],
            )),
        },
        None => Action {
            id: "package-cache".to_owned(),
            paths: Vec::new(),
            program: String::new(),
            args: Vec::new(),
            available: false,
            unavailable_reason: Some(messages::text("linux.package_manager_missing").to_owned()),
        },
    }
}

fn package<const N: usize>(program: &str, path: &str, args: [&str; N]) -> Action {
    Action {
        id: format!("package-{program}"),
        paths: vec![path.into()],
        program: program.to_owned(),
        args: args.into_iter().map(str::to_owned).collect(),
        available: command_exists(program),
        unavailable_reason: Some(messages::message(
            "linux.program_missing",
            &[("program", program.to_owned())],
        )),
    }
}

fn detect_package_manager() -> Option<String> {
    let os_release = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let candidates: &[&str] = if os_release.contains("ID_LIKE=\"suse")
        || os_release.contains("ID=sles")
        || os_release.contains("ID=opensuse")
    {
        &["zypper", "apt-get", "dnf", "pacman"]
    } else if os_release.contains("debian") || os_release.contains("ubuntu") {
        &["apt-get", "zypper", "dnf", "pacman"]
    } else if os_release.contains("fedora") || os_release.contains("rhel") {
        &["dnf", "zypper", "apt-get", "pacman"]
    } else if os_release.contains("arch") {
        &["pacman", "zypper", "apt-get", "dnf"]
    } else {
        &["zypper", "apt-get", "dnf", "pacman"]
    };
    candidates
        .iter()
        .find(|program| command_exists(program))
        .map(|program| (*program).to_owned())
}

#[cfg(test)]
mod tests;
