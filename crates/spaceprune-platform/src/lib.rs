#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! SpacePrune 的跨平台集成入口。
//!
//! 本 crate 隔离操作系统调度器和平台专用 Handler，使 CLI 与核心执行器不依赖具体平台。

#[cfg(any(target_os = "macos", all(test, unix)))]
mod macos;
#[cfg(all(test, unix))]
mod scheduler_test_support;
#[cfg(any(target_os = "windows", all(test, unix)))]
mod windows;

#[cfg(target_os = "linux")]
pub use spaceprune_platform_linux::discover_system_tasks;

use std::path::{Path, PathBuf};

use serde::Serialize;
use spaceprune_config::{ResolvedConfig, is_valid_id};
use spaceprune_contracts::messages;
use spaceprune_engine::{EngineError, PluginRegistry};
use thiserror::Error;

/// 平台集成错误。
#[derive(Debug, Error)]
pub enum PlatformError {
    /// 配置中没有已启用的自动化。
    #[error("{}", messages::message("platform.unknown_automation", &[("id", .0.to_string())]))]
    UnknownAutomation(String),
    /// 当前平台不支持相应操作。
    #[error("{}", messages::message("platform.unsupported", &[("platform", .0.to_string())]))]
    UnsupportedPlatform(String),
    /// 自动化 ID 不符合稳定 ID 语法。
    #[error("{}", messages::message("platform.invalid_automation", &[("id", .0.to_string())]))]
    InvalidAutomationId(String),
    /// 无法确定平台配置目录。
    #[error("{}", messages::text("platform.directory_missing"))]
    ConfigDirectoryMissing,
    /// 当前配置不是从 CLI 可重新加载的应用入口加载的。
    #[error("{}", messages::message("platform.application_config_required", &[("source", .0.to_string())]))]
    ApplicationConfigMissing(String),
    /// 文件系统操作失败。
    #[error("{}", messages::message("platform.access_failed", &[("path", .path.display().to_string()), ("source", .source.to_string())]))]
    Io {
        /// 相关路径。
        path: PathBuf,
        /// 底层错误。
        source: std::io::Error,
    },
    /// 外部平台命令执行失败。
    #[error("{}", messages::message("platform.command_failed", &[("command", .command.to_string()), ("message", .message.to_string())]))]
    Command {
        /// 命令及动作。
        command: String,
        /// 系统返回的信息。
        message: String,
    },
    /// 平台专用后端失败。
    #[error("{}", messages::message("platform.backend_failed", &[("error", .0.to_string())]))]
    Backend(String),
    /// A scheduler mutation failed and the previous state could not be restored.
    #[error("{}", messages::message("platform.recovery_failed", &[("operation", .operation.to_string()), ("recovery", .recovery.to_string()), ("safety_stop", .safety_stop.to_string())]))]
    Recovery {
        /// The original mutation failure.
        operation: Box<PlatformError>,
        /// The failure encountered while restoring the previous schedule.
        recovery: Box<PlatformError>,
        /// Whether the subsequent attempt to stop automatic scheduling succeeded.
        safety_stop: String,
    },
}

/// 安装自动化后返回的跨平台描述。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AutomationInstall {
    /// 使用的系统调度器。
    pub scheduler: String,
    /// 调度器中的稳定标识。
    pub identifier: String,
    /// 安装时创建的配置文件；没有文件时为空。
    pub artifacts: Vec<PathBuf>,
}

/// 注册当前平台提供的系统清理 Handler。
pub fn register_handlers(registry: &mut PluginRegistry) -> Result<(), EngineError> {
    #[cfg(target_os = "linux")]
    {
        spaceprune_platform_linux::register_system_handlers(registry)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = registry;
        Ok(())
    }
}

/// 使用显式 CLI runner 安装当前平台的自动化任务。
///
/// `runner` 必须是已存在的绝对文件路径，并支持 `automation run <id>` CLI 接口。
pub fn install_automation(
    config: &ResolvedConfig,
    automation_id: &str,
    runner: &Path,
) -> Result<AutomationInstall, PlatformError> {
    validate_automation_id(automation_id)?;
    validate_runner(runner)?;
    #[cfg(target_os = "linux")]
    {
        let config_path = automation_config_path(config)?;
        let units = spaceprune_platform_linux::install_automation(
            config,
            automation_id,
            config_path,
            runner,
        )
        .map_err(|error| PlatformError::Backend(error.to_string()))?;
        Ok(AutomationInstall {
            scheduler: "systemd-user".to_owned(),
            identifier: format!("spaceprune-{automation_id}.timer"),
            artifacts: vec![units.service, units.timer],
        })
    }
    #[cfg(target_os = "macos")]
    {
        macos::install(config, automation_id, runner)
    }
    #[cfg(target_os = "windows")]
    {
        windows::install(config, automation_id, runner)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (config, automation_id, runner);
        Err(PlatformError::UnsupportedPlatform(
            spaceprune_config::current_platform().to_owned(),
        ))
    }
}

/// 卸载当前平台的自动化任务。
pub fn uninstall_automation(automation_id: &str) -> Result<AutomationInstall, PlatformError> {
    validate_automation_id(automation_id)?;
    #[cfg(target_os = "linux")]
    {
        let units = spaceprune_platform_linux::uninstall_automation(automation_id)
            .map_err(|error| PlatformError::Backend(error.to_string()))?;
        Ok(AutomationInstall {
            scheduler: "systemd-user".to_owned(),
            identifier: format!("spaceprune-{automation_id}.timer"),
            artifacts: vec![units.service, units.timer],
        })
    }
    #[cfg(target_os = "macos")]
    {
        macos::uninstall(automation_id)
    }
    #[cfg(target_os = "windows")]
    {
        windows::uninstall(automation_id)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = automation_id;
        Err(PlatformError::UnsupportedPlatform(
            spaceprune_config::current_platform().to_owned(),
        ))
    }
}

/// 判断当前平台是否已经安装指定自动化。
pub fn automation_installed(automation_id: &str) -> bool {
    if !is_valid_id(automation_id) {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        spaceprune_platform_linux::automation_installed(automation_id)
    }
    #[cfg(target_os = "macos")]
    {
        macos::installed(automation_id)
    }
    #[cfg(target_os = "windows")]
    {
        windows::installed(automation_id)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = automation_id;
        false
    }
}

/// Check schedule presence for mutations, preserving query failures as errors.
pub fn automation_installed_checked(automation_id: &str) -> Result<bool, PlatformError> {
    validate_automation_id(automation_id)?;
    #[cfg(target_os = "linux")]
    {
        spaceprune_platform_linux::automation_installed_checked(automation_id)
            .map_err(|error| PlatformError::Backend(error.to_string()))
    }
    #[cfg(target_os = "macos")]
    {
        macos::installed_checked(automation_id)
    }
    #[cfg(target_os = "windows")]
    {
        windows::installed_checked(automation_id)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err(PlatformError::UnsupportedPlatform(
            spaceprune_config::current_platform().to_owned(),
        ))
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(crate) fn automation<'a>(
    config: &'a ResolvedConfig,
    id: &str,
) -> Result<&'a spaceprune_config::AutomationDefinition, PlatformError> {
    config
        .automations
        .get(id)
        .filter(|automation| automation.enabled)
        .ok_or_else(|| PlatformError::UnknownAutomation(id.to_owned()))
}

fn validate_automation_id(automation_id: &str) -> Result<(), PlatformError> {
    if is_valid_id(automation_id) {
        Ok(())
    } else {
        Err(PlatformError::InvalidAutomationId(automation_id.to_owned()))
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(crate) fn command_arguments(
    config: &ResolvedConfig,
    automation_id: &str,
    runner: &Path,
) -> Result<Vec<String>, PlatformError> {
    let mut arguments = vec![runner.to_string_lossy().into_owned()];
    if let Some(path) = automation_config_path(config)? {
        arguments.extend(["--config".to_owned(), path.to_string_lossy().into_owned()]);
    }
    if let Some(recipe) = &config.application_recipe {
        arguments.extend(["--recipe".to_owned(), recipe.clone()]);
    }
    arguments.extend([
        "--no-progress".to_owned(),
        "automation".to_owned(),
        "run".to_owned(),
        automation_id.to_owned(),
    ]);
    Ok(arguments)
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn automation_config_path(config: &ResolvedConfig) -> Result<Option<&Path>, PlatformError> {
    if let Some(path) = config.application_source.as_deref() {
        Ok(Some(path))
    } else if config.source.starts_with("official:") {
        Ok(None)
    } else {
        Err(PlatformError::ApplicationConfigMissing(
            config.source.clone(),
        ))
    }
}

fn validate_runner(runner: &Path) -> Result<(), PlatformError> {
    if runner.is_absolute() && runner.is_file() {
        Ok(())
    } else {
        Err(PlatformError::Io {
            path: runner.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                messages::text("platform.runner_invalid"),
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use spaceprune_config::ConfigLoader;

    use super::{PlatformError, automation_config_path, validate_runner};

    #[test]
    fn automation_uses_only_a_reloadable_application_config() {
        assert!(validate_runner(&std::env::current_exe().unwrap()).is_ok());
        assert!(validate_runner(Path::new("relative-spaceprune")).is_err());

        let mut config = ConfigLoader::new().load_official("minimal").unwrap();
        assert_eq!(automation_config_path(&config).unwrap(), None);

        config.source = "/tmp/direct-recipe.toml".to_owned();
        assert!(matches!(
            automation_config_path(&config),
            Err(PlatformError::ApplicationConfigMissing(_))
        ));

        config.application_source = Some("/tmp/Space Prune/config.toml".into());
        config.application_recipe = Some("chosen-recipe".into());
        let args = super::command_arguments(&config, "daily", Path::new("runner")).unwrap();
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--recipe", "chosen-recipe"])
        );

        assert_eq!(
            automation_config_path(&config).unwrap(),
            Some(Path::new("/tmp/Space Prune/config.toml"))
        );
    }
}
