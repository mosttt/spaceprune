//! Manifest schema, version, identifier, and package-relative executable validation.

use crate::{HostError, InstalledPlugin, PluginManifest};
use semver::Version;
use spaceprune_config::is_valid_id;
use spaceprune_plugin_api::PROTOCOL_VERSION;
use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

pub(super) fn load_manifest(path: &Path) -> Result<InstalledPlugin, HostError> {
    let manifest_metadata = fs::symlink_metadata(path).map_err(|source| HostError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !manifest_metadata.is_file() || manifest_metadata.file_type().is_symlink() {
        return Err(HostError::InvalidManifest {
            path: path.to_path_buf(),
            message: spaceprune_contracts::messages::text("host.manifest_regular").to_owned(),
        });
    }
    let text = fs::read_to_string(path).map_err(|source| HostError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let manifest: PluginManifest =
        toml::from_str(&text).map_err(|error| HostError::InvalidManifest {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    validate_manifest(&manifest).map_err(|message| HostError::InvalidManifest {
        path: path.to_path_buf(),
        message,
    })?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let executable_path = PathBuf::from(&manifest.executable);
    if executable_path.is_absolute()
        || executable_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(HostError::InvalidManifest {
            path: path.to_path_buf(),
            message: spaceprune_contracts::messages::text("host.executable_relative").to_owned(),
        });
    }
    let executable = parent.join(&executable_path);
    let metadata = fs::symlink_metadata(&executable).map_err(|source| HostError::Io {
        path: executable.clone(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(HostError::InvalidManifest {
            path: path.to_path_buf(),
            message: spaceprune_contracts::messages::text("host.executable_regular").to_owned(),
        });
    }
    let canonical_manifest = fs::canonicalize(path).map_err(|source| HostError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let canonical_parent = fs::canonicalize(parent).map_err(|source| HostError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let canonical_executable = fs::canonicalize(&executable).map_err(|source| HostError::Io {
        path: executable.clone(),
        source,
    })?;
    if !canonical_executable.starts_with(&canonical_parent) {
        return Err(HostError::InvalidManifest {
            path: path.to_path_buf(),
            message: spaceprune_contracts::messages::text("host.executable_resolved").to_owned(),
        });
    }
    Ok(InstalledPlugin {
        manifest,
        manifest_path: canonical_manifest,
        executable: canonical_executable,
    })
}

fn validate_manifest(manifest: &PluginManifest) -> Result<(), String> {
    if manifest.schema_version != 1 {
        return Err(spaceprune_contracts::messages::message(
            "host.schema_unsupported",
            &[("p0", (manifest.schema_version).to_string())],
        ));
    }
    if manifest.protocol_version != PROTOCOL_VERSION {
        return Err(spaceprune_contracts::messages::message(
            "host.protocol_version_unsupported",
            &[("p0", (manifest.protocol_version).to_string())],
        ));
    }
    validate_id(&manifest.id)?;
    if manifest.name.trim().is_empty() {
        return Err(spaceprune_contracts::messages::text("host.empty_name").to_owned());
    }
    Version::parse(&manifest.version).map_err(|error| {
        spaceprune_contracts::messages::message(
            "host.invalid_version",
            &[("error", (error).to_string())],
        )
    })?;
    for platform in &manifest.platforms {
        if !matches!(
            platform.as_str(),
            "linux" | "macos" | "windows" | "android" | "ios"
        ) {
            return Err(spaceprune_contracts::messages::message(
                "host.unsupported_platform",
                &[("platform", (platform).to_string())],
            ));
        }
    }
    if manifest.handlers.is_empty() || manifest.executable.is_empty() {
        return Err(spaceprune_contracts::messages::text("host.empty_handlers").to_owned());
    }
    let mut handlers = BTreeSet::new();
    for id in &manifest.handlers {
        validate_id(id)?;
        if !handlers.insert(id) {
            return Err(spaceprune_contracts::messages::message(
                "host.duplicate_handler",
                &[("id", (id).to_string())],
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_id(id: &str) -> Result<(), String> {
    if is_valid_id(id) {
        Ok(())
    } else {
        Err(spaceprune_contracts::messages::message(
            "host.invalid_id",
            &[("id", (id).to_string())],
        ))
    }
}
