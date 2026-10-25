//! Device-local preferences, separate from portable recipes and Desktop-only settings.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use spaceprune_engine::EngineError;
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Document {
    schema_version: u32,
    revision: u64,
    disabled: BTreeSet<String>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            schema_version: 1,
            revision: 0,
            disabled: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PolicySnapshot {
    path: Option<PathBuf>,
    document: Document,
}

impl PolicySnapshot {
    pub(crate) fn discover() -> Result<Self> {
        for path in spaceprune_config::user_config_paths() {
            let path = path.with_file_name("plugins.toml");
            match fs::metadata(&path) {
                Ok(_) => return Self::load(Some(path)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error).context("Could not inspect plugin preferences"),
            }
        }
        Self::load(
            spaceprune_config::user_config_directory()
                .map(|directory| directory.join("plugins.toml")),
        )
    }

    pub(super) fn load(path: Option<PathBuf>) -> Result<Self> {
        let document = match path.as_deref().map(fs::read_to_string) {
            Some(Ok(text)) => {
                toml::from_str::<Document>(&text).context("Plugin preferences are invalid")?
            }
            Some(Err(error)) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error).context("Could not read plugin preferences");
            }
            _ => Document::default(),
        };
        if document.schema_version != 1
            || document
                .disabled
                .iter()
                .any(|id| !spaceprune_config::is_valid_id(id))
        {
            bail!("Plugin preferences contain an unsupported schema or invalid plugin ID");
        }
        Ok(Self { path, document })
    }

    pub(crate) fn disabled(&self) -> &BTreeSet<String> {
        &self.document.disabled
    }

    pub(crate) fn check_current(&self) -> Result<(), EngineError> {
        let current = Self::load(self.path.clone())
            .map_err(|error| EngineError::PluginPolicyUnavailable(error.to_string()))?;
        if current.document != self.document {
            return Err(EngineError::PluginPolicyChanged);
        }
        Ok(())
    }

    pub(super) fn set_enabled(&self, id: &str, enabled: bool) -> Result<Self> {
        let path = self
            .path
            .as_deref()
            .context("The user plugin preferences directory could not be determined")?;
        let directory = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(directory).context("Could not create plugin preferences directory")?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join(".spaceprune-plugins.lock"))
            .context("Could not open plugin preferences lock")?;
        lock.lock().context("Could not lock plugin preferences")?;
        let mut current = Self::load(self.path.clone())?;
        let changed = if enabled {
            current.document.disabled.remove(id)
        } else {
            current.document.disabled.insert(id.to_owned())
        };
        if !changed {
            return Ok(current);
        }
        current.document.revision = current
            .document
            .revision
            .checked_add(1)
            .context("Plugin preferences revision is exhausted")?;
        let text = toml::to_string_pretty(&current.document)?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory)
            .context("Could not stage plugin preferences")?;
        temporary
            .write_all(text.as_bytes())
            .context("Could not write plugin preferences")?;
        temporary
            .as_file()
            .sync_all()
            .context("Could not flush plugin preferences")?;
        temporary
            .persist(path)
            .context("Could not save plugin preferences")?;
        Ok(current)
    }
}
