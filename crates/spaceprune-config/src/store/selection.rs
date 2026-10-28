//! Separate per-host selection preferences from the default recipe used by automation.
use super::{ConfigStore, ConfigStoreError, application::replace_file, io_error, unique_suffix};
use crate::is_valid_id;
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Selection {
    last_used: Option<String>,
    cli_override: Option<String>,
}

impl ConfigStore {
    /// Resolve a CLI override, otherwise the most recently used graphical recipe or default.
    /// Deleted selections fall back to the shared default instead of blocking startup.
    pub fn cli_recipe(&self) -> Result<Option<String>, ConfigStoreError> {
        let selection = self.read_selection()?;
        let recipes = self.recipes()?;
        for id in [selection.cli_override, selection.last_used]
            .into_iter()
            .flatten()
        {
            if recipes.iter().any(|recipe| recipe.id == id) {
                return Ok(Some(id));
            }
        }
        self.active_recipe()
    }

    /// Remember a successfully loaded graphical recipe without changing automation defaults.
    pub fn remember_used_recipe(&self, id: &str) -> Result<(), ConfigStoreError> {
        self.update_selection(Some(id), false)
    }

    /// Set a persistent CLI-only override, or resume following the graphical selection.
    pub fn save_cli_recipe(&self, id: Option<&str>) -> Result<(), ConfigStoreError> {
        self.update_selection(id, true)
    }

    fn read_selection(&self) -> Result<Selection, ConfigStoreError> {
        let path = self.path.with_extension("selection.toml");
        match fs::read_to_string(&path) {
            Ok(source) => toml::from_str(&source)
                .map_err(|error| self.invalid("recipe selection", error.to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Selection::default()),
            Err(error) => Err(io_error("read recipe selection", &path, error)),
        }
    }

    fn update_selection(&self, id: Option<&str>, cli: bool) -> Result<(), ConfigStoreError> {
        let _lock = self.lock_mutations()?;
        if let Some(id) = id
            && (!is_valid_id(id) || !self.recipes()?.iter().any(|recipe| recipe.id == id))
        {
            return Err(ConfigStoreError::UnknownRecipe {
                id: id.into(),
                directory: self.recipe_directory(),
            });
        }
        let mut selection = self.read_selection()?;
        if cli {
            selection.cli_override = id.map(str::to_owned);
        } else {
            selection.last_used = id.map(str::to_owned);
        }
        let destination = self.path.with_extension("selection.toml");
        let temporary = self
            .config_directory()
            .join(format!(".selection.{}.tmp", unique_suffix()));
        let source = toml::to_string(&selection).expect("selection contains only strings");
        fs::write(&temporary, source)
            .map_err(|error| io_error("write recipe selection", &temporary, error))?;
        replace_file(&temporary, &destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn follows_last_used_until_overridden_and_leaves_default_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(temp.path().join("config.toml"));
        for id in ["default", "first", "second"] {
            store
                .upsert_recipe(
                    id,
                    &format!("schema-version = 2\nname = '{id}'\ninclude = ['official:minimal']\n"),
                )
                .unwrap();
        }
        store.save_recipe("default").unwrap();
        store.remember_used_recipe("first").unwrap();
        assert_eq!(store.cli_recipe().unwrap().as_deref(), Some("first"));
        store.save_cli_recipe(Some("default")).unwrap();
        store.remember_used_recipe("second").unwrap();
        assert_eq!(store.cli_recipe().unwrap().as_deref(), Some("default"));
        store.save_cli_recipe(None).unwrap();
        assert_eq!(store.cli_recipe().unwrap().as_deref(), Some("second"));
        assert_eq!(store.active_recipe().unwrap().as_deref(), Some("default"));
        store.delete_recipe("second").unwrap();
        assert_eq!(store.cli_recipe().unwrap().as_deref(), Some("default"));
        assert!(store.save_cli_recipe(Some("../escape")).is_err());
    }
}
