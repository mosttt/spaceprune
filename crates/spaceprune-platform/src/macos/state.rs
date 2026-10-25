//! Capture and restore a LaunchAgent's persisted and loaded state.

use spaceprune_contracts::messages;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use super::{LaunchAgent, command_error, launchctl, launchctl_output};
use crate::PlatformError;

pub(super) struct Definition {
    bytes: Vec<u8>,
    permissions: fs::Permissions,
}

pub(super) struct Snapshot {
    definition: Option<Definition>,
    loaded: bool,
    disabled: bool,
}

impl Snapshot {
    pub(super) fn capture(agent: &LaunchAgent) -> Result<Self, PlatformError> {
        let definition = read_definition(&agent.path)?;
        let loaded = agent.loaded()?;
        let arguments = ["print-disabled", &agent.domain];
        let output = launchctl_output(&arguments)?;
        if !output.status.success() {
            return Err(command_error(&arguments, &output));
        }
        let disabled = disabled_state(&String::from_utf8_lossy(&output.stdout), &agent.label)?;
        Ok(Self {
            definition,
            loaded,
            disabled,
        })
    }

    pub(super) fn permissions(&self) -> Option<&fs::Permissions> {
        self.definition
            .as_ref()
            .map(|definition| &definition.permissions)
    }

    pub(super) fn restore(&self, agent: &LaunchAgent) -> Result<(), PlatformError> {
        let unload = agent.unload();
        // Restore the on-disk definition even if launchd cannot currently be
        // reached; otherwise the failed update would survive the next login.
        let persisted = if let Some(definition) = &self.definition {
            write_definition(
                &agent.path,
                &definition.bytes,
                Some(&definition.permissions),
            )
        } else {
            remove_definition(&agent.path)
        };
        match (unload, persisted) {
            (Ok(()), Ok(())) => {}
            (Err(error), Ok(())) | (Ok(()), Err(error)) => return Err(error),
            (Err(unload), Err(persisted)) => {
                return Err(PlatformError::Backend(messages::message(
                    "platform.restore_definition_failed",
                    &[
                        ("unload", unload.to_string()),
                        ("persisted", persisted.to_string()),
                    ],
                )));
            }
        }
        if self.loaded {
            if self.definition.is_none() {
                return Err(PlatformError::Backend(messages::message(
                    "platform.launchagent_definition_missing",
                    &[
                        ("label", agent.label.to_string()),
                        ("path", agent.path.display().to_string()),
                    ],
                )));
            }
            agent.set_disabled(false)?;
            agent.bootstrap()?;
        }
        agent.set_disabled(self.disabled)
    }
}

impl LaunchAgent {
    pub(super) fn loaded(&self) -> Result<bool, PlatformError> {
        let target = self.target();
        let arguments = ["print", &target];
        let output = launchctl_output(&arguments)?;
        if output.status.success() {
            return Ok(true);
        }
        // Modern launchctl uses 113 for an absent service. Also require a
        // successful domain query so an unavailable login domain is not absence.
        if output.status.code() == Some(113) {
            launchctl(&["print", &self.domain])?;
            return Ok(false);
        }
        Err(command_error(&arguments, &output))
    }
}

pub(super) fn disabled_state(output: &str, label: &str) -> Result<bool, PlatformError> {
    // print-disabled has a diagnostic (not versioned API) format. Reject a
    // changed/ambiguous format before mutation rather than guessing enabledness.
    let malformed =
        || PlatformError::Backend(messages::text("platform.launchd_state_unreadable").to_owned());
    let mut lines = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    if lines.next() != Some("disabled services = {") {
        return Err(malformed());
    }
    let mut disabled = None;
    let mut closed = false;
    for line in lines.by_ref() {
        if line == "}" {
            closed = true;
            break;
        }
        let (key, value) = line.split_once("=>").ok_or_else(malformed)?;
        let key = key.trim();
        if !key.starts_with('"') || !key.ends_with('"') {
            return Err(malformed());
        }
        let value = match value.trim().trim_end_matches(',') {
            "true" => true,
            "false" => false,
            _ => return Err(malformed()),
        };
        if key == format!("\"{label}\"") && disabled.replace(value).is_some() {
            return Err(malformed());
        }
    }
    if !closed || lines.next().is_some() {
        return Err(malformed());
    }
    Ok(disabled.unwrap_or(false))
}

pub(super) fn read_definition(path: &Path) -> Result<Option<Definition>, PlatformError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(path, error)),
    };
    let permissions = fs::metadata(path)
        .map_err(|error| io_error(path, error))?
        .permissions();
    Ok(Some(Definition { bytes, permissions }))
}

pub(super) fn write_definition(
    path: &Path,
    bytes: &[u8],
    permissions: Option<&fs::Permissions>,
) -> Result<(), PlatformError> {
    let parent = path.parent().ok_or(PlatformError::ConfigDirectoryMissing)?;
    fs::create_dir_all(parent).map_err(|source| io_error(parent, source))?;
    let mut file =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| io_error(parent, source))?;
    file.write_all(bytes)
        .map_err(|source| io_error(path, source))?;
    if let Some(permissions) = permissions {
        file.as_file()
            .set_permissions(permissions.clone())
            .map_err(|source| io_error(path, source))?;
    }
    file.persist(path)
        .map_err(|error| io_error(path, error.error))?;
    Ok(())
}

pub(super) fn remove_definition(path: &Path) -> Result<(), PlatformError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(path, error)),
    }
}

pub(super) fn io_error(path: impl Into<PathBuf>, source: std::io::Error) -> PlatformError {
    PlatformError::Io {
        path: path.into(),
        source,
    }
}
