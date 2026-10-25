//! Shared configuration store identity, mutation lock and public errors.

use spaceprune_contracts::messages;
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use toml_edit::{Item, TableLike, Value};

use crate::{RECIPE_DIRECTORY_NAME, user_config_path};

mod application;
mod recipes;

/// 应用入口配置读写错误。
#[derive(Debug, Error)]
pub enum ConfigStoreError {
    /// 文件系统操作失败。
    #[error("{}", spaceprune_contracts::messages::message("store.io", &[("action", .action.to_string()), ("path", .path.display().to_string()), ("source", .source.to_string())]))]
    Io {
        /// 正在执行的操作。
        action: &'static str,
        /// 操作目标路径。
        path: PathBuf,
        /// 底层文件系统错误。
        #[source]
        source: io::Error,
    },
    /// 配置内容无效。
    #[error("{}", spaceprune_contracts::messages::message("store.invalid", &[("kind", .kind.to_string()), ("path", .path.display().to_string()), ("message", .message.to_string())]))]
    Invalid {
        /// 配置类型。
        kind: &'static str,
        /// 配置路径。
        path: PathBuf,
        /// 错误说明。
        message: String,
    },
    /// 请求的清理方案不存在。
    #[error("{}", spaceprune_contracts::messages::message("store.unknown_recipe", &[("id", .id.to_string()), ("directory", .directory.display().to_string())]))]
    UnknownRecipe {
        /// 清理方案 ID。
        id: String,
        /// recipe 根目录。
        directory: PathBuf,
    },
    /// 不能删除当前正在使用的清理方案。
    #[error("{}", spaceprune_contracts::messages::message("store.active_recipe", &[("id", .0.to_string())]))]
    ActiveRecipe(String),
    /// Recipe 在条件写入或删除前被其他进程新增或修改。
    #[error("{}", spaceprune_contracts::messages::message("store.recipe_conflict", &[("id", .0.to_string())]))]
    RecipeConflict(String),
}

/// 一个位于 `recipes/<id>/<id>.toml` 的清理方案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeInfo {
    /// 清理方案 ID。
    pub id: String,
    /// 清理方案显示名称。
    pub name: String,
    /// 清理方案入口路径。
    pub path: PathBuf,
}

/// SpacePrune 的应用入口 `config.toml` 存储。
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    /// 使用指定应用入口配置路径创建存储。
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// 使用显式入口配置；否则选择首个存在的平台候选，均不存在时选择首选新路径。
    pub fn discover(explicit_path: Option<&Path>) -> Option<Self> {
        explicit_path
            .map(Path::to_path_buf)
            .or_else(user_config_path)
            .map(Self::new)
    }

    /// 返回应用入口配置路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 返回保存本地清理方案 bundle 的 `recipes` 目录。
    pub fn recipe_directory(&self) -> PathBuf {
        self.config_directory().join(RECIPE_DIRECTORY_NAME)
    }

    fn config_directory(&self) -> &Path {
        self.path
            .parent()
            .filter(|directory| !directory.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
    }

    fn lock_mutations(&self) -> Result<File, ConfigStoreError> {
        let directory = self.config_directory();
        fs::create_dir_all(directory).map_err(|source| {
            io_error(
                messages::text("config.store_action_create_configuration_directory"),
                directory,
                source,
            )
        })?;
        let path = directory.join(".spaceprune-store.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| {
                io_error(
                    messages::text("config.store_action_open_configuration_lock"),
                    &path,
                    source,
                )
            })?;
        file.lock().map_err(|source| {
            io_error(
                messages::text("config.store_action_lock_configuration_store"),
                &path,
                source,
            )
        })?;
        Ok(file)
    }

    fn invalid(&self, kind: &'static str, message: impl Into<String>) -> ConfigStoreError {
        invalid(kind, &self.path, message)
    }
}

fn unique_suffix() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}.{}", std::process::id(), timestamp)
}

pub(crate) fn set_string(table: &mut dyn TableLike, key: &str, value: &str) {
    let mut replacement = Value::from(value);
    if let Some(current) = table.get(key).and_then(Item::as_value) {
        *replacement.decor_mut() = current.decor().clone();
    }
    table.insert(key, Item::Value(replacement));
}

fn io_error(action: &'static str, path: &Path, source: io::Error) -> ConfigStoreError {
    ConfigStoreError::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
}

pub(crate) fn invalid(
    kind: &'static str,
    path: &Path,
    message: impl Into<String>,
) -> ConfigStoreError {
    ConfigStoreError::Invalid {
        kind,
        path: path.to_path_buf(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests;
