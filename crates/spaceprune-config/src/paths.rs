//! Platform-specific discovery of local application configuration.

use crate::{CONFIG_DIRECTORY_NAME, CONFIG_FILE_NAME, current_platform};
use std::{
    env,
    path::{Path, PathBuf},
};

fn environment_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

/// 返回当前平台按优先级排列的应用配置候选路径。
pub fn user_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    match current_platform() {
        "windows" => {
            if let Some(path) = environment_path("APPDATA") {
                push_unique(&mut paths, path.join("SpacePrune").join(CONFIG_FILE_NAME));
            } else if let Some(path) = environment_path("USERPROFILE") {
                push_unique(
                    &mut paths,
                    path.join("AppData/Roaming/SpacePrune")
                        .join(CONFIG_FILE_NAME),
                );
            }
        }
        "macos" | "ios" => {
            if let Some(path) = environment_path("XDG_CONFIG_HOME") {
                push_unique(
                    &mut paths,
                    path.join(CONFIG_DIRECTORY_NAME).join(CONFIG_FILE_NAME),
                );
            }
            if let Some(path) = environment_path("HOME") {
                push_unique(
                    &mut paths,
                    path.join("Library/Application Support/SpacePrune")
                        .join(CONFIG_FILE_NAME),
                );
            }
        }
        _ => {
            if let Some(path) = environment_path("XDG_CONFIG_HOME") {
                push_unique(
                    &mut paths,
                    path.join(CONFIG_DIRECTORY_NAME).join(CONFIG_FILE_NAME),
                );
            }
            if let Some(path) = environment_path("HOME") {
                push_unique(
                    &mut paths,
                    path.join(".config")
                        .join(CONFIG_DIRECTORY_NAME)
                        .join(CONFIG_FILE_NAME),
                );
            }
        }
    }
    paths
}

/// 返回当前已存在的应用配置，或首选的新配置路径。
pub fn user_config_path() -> Option<PathBuf> {
    let paths = user_config_paths();
    paths
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .or_else(|| paths.into_iter().next())
}

/// 返回当前已存在的应用配置目录，或首选的新配置目录。
pub fn user_config_directory() -> Option<PathBuf> {
    user_config_path().and_then(|path| path.parent().map(Path::to_path_buf))
}
