//! Atomic local package installation and scoped user-plugin removal.

use crate::{
    HostError, InstalledPlugin, MANIFEST_FILE,
    manifest::{load_manifest, validate_id},
    paths::user_plugin_root,
};
use spaceprune_contracts::plugin::is_builtin_plugin_id;
use std::{
    fs,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

/// 将本地插件目录安装到当前用户的数据目录。
pub fn install_plugin(source: &Path) -> Result<InstalledPlugin, HostError> {
    let source_manifest = if source.is_dir() {
        source.join(MANIFEST_FILE)
    } else {
        source.to_path_buf()
    };
    let plugin = load_manifest(&source_manifest)?;
    validate_install_id(&plugin.manifest.id)?;
    let target_root = user_plugin_root()?;
    let target = target_root.join(&plugin.manifest.id);
    if target.exists() {
        return Err(HostError::AlreadyInstalled(plugin.manifest.id));
    }
    fs::create_dir_all(&target_root).map_err(|source| HostError::Io {
        path: target_root.clone(),
        source,
    })?;
    let source_dir = source_manifest.parent().unwrap_or_else(|| Path::new("."));
    let staging = TempDir::new_in(&target_root).map_err(|source| HostError::Io {
        path: target_root.clone(),
        source,
    })?;
    let staged_target = staging.path().join("plugin");
    copy_tree(source_dir, &staged_target)?;
    let staged = load_manifest(&staged_target.join(MANIFEST_FILE))?;
    validate_install_id(&staged.manifest.id)?;
    fs::rename(&staged_target, &target).map_err(|source| HostError::Io {
        path: target.clone(),
        source,
    })?;
    load_manifest(&target.join(MANIFEST_FILE))
}

fn validate_install_id(id: &str) -> Result<(), HostError> {
    if is_builtin_plugin_id(id) {
        return Err(HostError::ReservedBuiltinId(id.to_owned()));
    }
    Ok(())
}

/// Remove one user plugin, preferring its canonical ID directory over manifest-based discovery.
pub fn remove_plugin(id: &str) -> Result<(), HostError> {
    validate_id(id).map_err(|message| HostError::InvalidManifest {
        path: PathBuf::from(id),
        message,
    })?;
    remove_from_user_root(&user_plugin_root()?, id)
}

pub(super) fn remove_from_user_root(root: &Path, id: &str) -> Result<(), HostError> {
    let canonical_target = root.join(id);
    let target = match fs::symlink_metadata(&canonical_target) {
        Ok(_) if canonical_target.is_dir() => canonical_target,
        Ok(_) => return Err(HostError::NotInstalled(id.to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            find_user_plugin_directory(root, id)?
        }
        Err(source) => {
            return Err(HostError::Io {
                path: canonical_target,
                source,
            });
        }
    };
    fs::remove_dir_all(&target).map_err(|source| HostError::Io {
        path: target,
        source,
    })
}

fn find_user_plugin_directory(root: &Path, id: &str) -> Result<PathBuf, HostError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(HostError::NotInstalled(id.to_owned()));
        }
        Err(source) => {
            return Err(HostError::Io {
                path: root.to_owned(),
                source,
            });
        }
    };
    let mut entries = entries
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| HostError::Io {
            path: root.to_owned(),
            source,
        })?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let kind = entry.file_type().map_err(|source| HostError::Io {
            path: entry.path(),
            source,
        })?;
        let manifest = entry.path().join(MANIFEST_FILE);
        if kind.is_dir() && manifest.is_file() && load_manifest(&manifest)?.manifest.id == id {
            // Delete the discovered child, never a canonical manifest target outside this root.
            return Ok(entry.path());
        }
    }
    Err(HostError::NotInstalled(id.to_owned()))
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), HostError> {
    fs::create_dir(target).map_err(|source_error| HostError::Io {
        path: target.to_path_buf(),
        source: source_error,
    })?;
    for entry in fs::read_dir(source).map_err(|source_error| HostError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })? {
        let entry = entry.map_err(|source_error| HostError::Io {
            path: source.to_path_buf(),
            source: source_error,
        })?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        let file_type = entry.file_type().map_err(|source_error| HostError::Io {
            path: from.clone(),
            source: source_error,
        })?;
        if file_type.is_symlink() {
            return Err(HostError::InvalidManifest {
                path: from,
                message: spaceprune_contracts::messages::text("host.package_symlinks").to_owned(),
            });
        }
        if file_type.is_dir() {
            copy_tree(&from, &to)?;
        } else if file_type.is_file() {
            fs::copy(&from, &to).map_err(|source_error| HostError::Io {
                path: from,
                source: source_error,
            })?;
        } else {
            return Err(HostError::InvalidManifest {
                path: from,
                message: spaceprune_contracts::messages::text("host.package_types").to_owned(),
            });
        }
    }
    Ok(())
}
