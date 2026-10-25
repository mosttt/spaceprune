#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! SpacePrune 的 Linux 平台集成。
//!
//! 当前实现提供 systemd user 自动化；领域清理由独立插件实现。

pub use spaceprune_plugin_system_linux::register_system_handlers;
#[cfg(target_os = "linux")]
pub use spaceprune_plugin_system_linux::{
    discover_legacy_system_tasks as discover_system_tasks, inspect_system_cache,
    prepare_system_cache_scan, validate_system_cache_scan,
};

use std::path::PathBuf;

use spaceprune_plugin_api::messages;
use thiserror::Error;

/// Linux 自动化管理错误。
#[derive(Debug, Error)]
pub enum PlatformError {
    /// 配置中没有对应自动化。
    #[error("{}", messages::message("linux.unknown_automation", &[("id", .0.to_string())]))]
    UnknownAutomation(String),
    /// 自动化 ID 不符合稳定 ID 语法。
    #[error("{}", messages::message("linux.invalid_automation_id", &[("id", .0.to_string())]))]
    InvalidAutomationId(String),
    /// 无法确定用户配置目录。
    #[error("{}", messages::text("linux.config_directory_missing"))]
    ConfigDirectoryMissing,
    /// 文件系统操作失败。
    #[error("{}", messages::message("linux.access_failed", &[("path", .path.display().to_string()), ("source", .source.to_string())]))]
    Io {
        /// 相关路径。
        path: PathBuf,
        /// 底层错误。
        source: std::io::Error,
    },
    /// systemctl 命令失败。
    #[error("{}", messages::message("linux.systemctl_failed", &[("action", .action.to_string()), ("message", .message.to_string())]))]
    Systemctl {
        /// 执行动作。
        action: String,
        /// 命令输出。
        message: String,
    },
    /// A scheduler change failed and its previous state could not be fully restored.
    #[error("{}", messages::message("linux.recovery_failed", &[("operation", .operation.to_string()), ("recovery", .recovery.to_string())]))]
    RecoveryFailed {
        /// The original failed operation.
        operation: Box<PlatformError>,
        /// Failures encountered while restoring files or scheduler state.
        recovery: String,
    },
}

/// 一组已生成的 systemd 用户单元路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationUnits {
    /// `.service` 单元路径。
    pub service: PathBuf,
    /// `.timer` 单元路径。
    pub timer: PathBuf,
}

mod automation;
pub use automation::{
    automation_installed, automation_installed_checked, install_automation, uninstall_automation,
};
