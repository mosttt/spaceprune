//! Platform-specific search precedence and the writable user installation root.

use crate::HostError;
use spaceprune_config::current_platform;
use std::{env, path::PathBuf};

/// 返回当前平台的插件搜索路径，按优先级排序。
pub fn plugin_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(value) = nonempty_env_path("XDG_DATA_HOME") {
        paths.push(PathBuf::from(value).join("spaceprune/plugins"));
    }
    match current_platform() {
        "windows" => {
            if let Some(value) = nonempty_env_path("LOCALAPPDATA") {
                push_unique(&mut paths, PathBuf::from(value).join("SpacePrune/plugins"));
            } else if let Some(value) = nonempty_env_path("USERPROFILE") {
                push_unique(
                    &mut paths,
                    PathBuf::from(value).join("AppData/Local/SpacePrune/plugins"),
                );
            }
            if let Some(value) = nonempty_env_path("PROGRAMDATA") {
                push_unique(&mut paths, PathBuf::from(value).join("SpacePrune/plugins"));
            }
        }
        "macos" => {
            if let Some(value) = nonempty_env_path("HOME") {
                push_unique(
                    &mut paths,
                    PathBuf::from(value).join("Library/Application Support/SpacePrune/plugins"),
                );
            }
            push_unique(
                &mut paths,
                PathBuf::from("/Library/Application Support/SpacePrune/plugins"),
            );
        }
        "ios" => {
            if let Some(value) = nonempty_env_path("HOME") {
                push_unique(
                    &mut paths,
                    PathBuf::from(value).join("Library/Application Support/SpacePrune/plugins"),
                );
            }
        }
        "android" => {
            if let Some(value) = nonempty_env_path("HOME") {
                push_unique(
                    &mut paths,
                    PathBuf::from(value).join(".local/share/spaceprune/plugins"),
                );
            }
        }
        _ => {
            if let Some(value) = nonempty_env_path("HOME") {
                push_unique(
                    &mut paths,
                    PathBuf::from(value).join(".local/share/spaceprune/plugins"),
                );
            }
            push_unique(&mut paths, PathBuf::from("/usr/lib/spaceprune/plugins"));
        }
    }
    paths
}

pub(super) fn user_plugin_root() -> Result<PathBuf, HostError> {
    if let Some(value) = nonempty_env_path("XDG_DATA_HOME") {
        return Ok(PathBuf::from(value).join("spaceprune/plugins"));
    }
    let path = match current_platform() {
        "windows" => nonempty_env_path("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("SpacePrune/plugins"))
            .or_else(|| {
                nonempty_env_path("USERPROFILE")
                    .map(PathBuf::from)
                    .map(|path| path.join("AppData/Local/SpacePrune/plugins"))
            }),
        "macos" | "ios" => nonempty_env_path("HOME")
            .map(PathBuf::from)
            .map(|path| path.join("Library/Application Support/SpacePrune/plugins")),
        _ => nonempty_env_path("HOME")
            .map(PathBuf::from)
            .map(|path| path.join(".local/share/spaceprune/plugins")),
    };
    path.ok_or_else(|| HostError::InvalidManifest {
        path: PathBuf::new(),
        message: spaceprune_contracts::messages::text("host.user_data_unknown").to_owned(),
    })
}

fn nonempty_env_path(name: &str) -> Option<std::ffi::OsString> {
    env::var_os(name).filter(|value| !value.is_empty())
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}
