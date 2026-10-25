#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! 外部进程插件的发现、安装和 JSON Lines 协议宿主。

mod adapter;
mod catalog;
mod installation;
mod manifest;
mod paths;
mod process;

pub use catalog::{PluginHost, installed_plugin_metadata};
pub use installation::{install_plugin, remove_plugin};
pub use paths::plugin_search_paths;
/// Public plugin metadata, retained at the existing host import path.
pub use spaceprune_contracts::plugin::{InstalledPlugin, PluginManifest};

use spaceprune_engine::EngineError;
use std::path::PathBuf;
use thiserror::Error;

/// 外部插件清单的固定文件名。
pub const MANIFEST_FILE: &str = "plugin.toml";

/// 外部插件宿主错误。
#[derive(Debug, Error)]
pub enum HostError {
    /// 无法读取文件或目录。
    #[error("{}", spaceprune_contracts::messages::message("host.io", &[("path", .path.display().to_string()), ("source", .source.to_string())]))]
    Io {
        /// 相关路径。
        path: PathBuf,
        /// 底层错误。
        source: std::io::Error,
    },
    /// 插件清单无效。
    #[error("{}", spaceprune_contracts::messages::message("host.invalid_manifest", &[("path", .path.display().to_string()), ("message", .message.to_string())]))]
    InvalidManifest {
        /// 清单路径。
        path: PathBuf,
        /// 错误说明。
        message: String,
    },
    /// 外部插件协议调用失败。
    #[error("{}", spaceprune_contracts::messages::message("host.protocol", &[("plugin", .plugin.to_string()), ("message", .message.to_string())]))]
    Protocol {
        /// 插件 ID。
        plugin: String,
        /// 错误说明。
        message: String,
    },
    /// An external package attempted to claim an application-owned capability ID.
    #[error("{}", spaceprune_contracts::messages::message("host.reserved_id", &[("id", .0.to_string())]))]
    ReservedBuiltinId(String),
    /// 插件安装目标已经存在。
    #[error("{}", spaceprune_contracts::messages::message("host.already_installed", &[("id", .0.to_string())]))]
    AlreadyInstalled(String),
    /// 没有找到已安装插件。
    #[error("{}", spaceprune_contracts::messages::message("host.not_installed", &[("id", .0.to_string())]))]
    NotInstalled(String),
    /// 注册 Handler 失败。
    #[error(transparent)]
    Engine(#[from] EngineError),
}

#[cfg(test)]
mod tests;
