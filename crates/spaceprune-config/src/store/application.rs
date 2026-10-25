//! Application preferences and source-preserving config.toml persistence.

use spaceprune_contracts::messages;
use std::{fs, path::Path};
use toml_edit::{DocumentMut, Item, Table};

use super::{ConfigStore, ConfigStoreError, io_error, set_string, unique_suffix};
use crate::{
    DEFAULT_RECIPE_DIRECTORY_NAME, DEFAULT_RECIPE_FILE_NAME, DEFAULT_THEME_ID, THEME_IDS,
    is_valid_id, resolve_theme_id,
};

impl ConfigStore {
    /// Read the shared language preference without loading the active recipe.
    pub fn load_language_preference(&self) -> Result<crate::LanguagePreference, ConfigStoreError> {
        let Some(document) = self.read()? else {
            return Ok(crate::LanguagePreference::default());
        };
        Ok(match document.get("language") {
            None => crate::LanguagePreference::System,
            Some(value) => value
                .as_str()
                .and_then(crate::LanguagePreference::parse)
                .unwrap_or(crate::LanguagePreference::English),
        })
    }

    /// Resolve the shared preference and current native system language.
    pub fn language_settings(&self) -> Result<crate::LanguageSettings, ConfigStoreError> {
        self.load_language_preference()
            .map(crate::LanguagePreference::settings)
    }

    /// Save a language override while preserving unrelated application preferences.
    pub fn save_language_preference(
        &self,
        preference: crate::LanguagePreference,
    ) -> Result<(), ConfigStoreError> {
        let _lock = self.lock_mutations()?;
        let existing = self.read()?;
        let create_default_recipe = existing.is_none();
        let mut document = existing.unwrap_or_else(new_config_document);
        set_string(document.as_table_mut(), "language", preference.id());
        self.write_unlocked(document, create_default_recipe)
    }

    /// Read the terminal palette, resolving legacy IDs and defaulting absent preferences.
    pub fn load_theme_id(&self) -> Result<String, ConfigStoreError> {
        let Some(document) = self.read()? else {
            return Ok(DEFAULT_THEME_ID.to_owned());
        };
        let Some(theme) = document.get("theme") else {
            return Ok(DEFAULT_THEME_ID.to_owned());
        };
        let theme = theme.as_table_like().ok_or_else(|| {
            self.invalid(
                messages::text("config.store_kind_application_configuration"),
                messages::text("config.theme_table"),
            )
        })?;
        let Some(id) = theme.get("id") else {
            return Ok(DEFAULT_THEME_ID.to_owned());
        };
        let id = id.as_str().ok_or_else(|| {
            self.invalid(
                messages::text("config.store_kind_application_configuration"),
                messages::text("config.theme_id_string"),
            )
        })?;
        resolve_theme_id(id).map(str::to_owned).ok_or_else(|| {
            self.invalid(
                messages::text("config.store_kind_application_configuration"),
                messages::message("config.unknown_theme", &[("id", id.to_string())]),
            )
        })
    }

    /// 返回当前选择的清理方案 ID；入口配置不存在时返回 `None`。
    pub fn active_recipe(&self) -> Result<Option<String>, ConfigStoreError> {
        let Some(document) = self.read()? else {
            return Ok(None);
        };
        let recipe = document
            .get("recipe")
            .and_then(Item::as_str)
            .ok_or_else(|| {
                self.invalid(
                    messages::text("config.store_kind_application_configuration"),
                    messages::text("config.recipe_string"),
                )
            })?;
        if !is_valid_id(recipe) {
            return Err(self.invalid(
                messages::text("config.store_kind_application_configuration"),
                messages::message("config.invalid_recipe_id", &[("id", recipe.to_owned())]),
            ));
        }
        Ok(Some(recipe.to_owned()))
    }

    /// 保存主题 ID，并在首次使用时创建可直接运行的默认清理方案。
    pub fn save_theme_id(&self, id: &str) -> Result<(), ConfigStoreError> {
        if !THEME_IDS.contains(&id) {
            return Err(self.invalid(
                messages::text("config.store_kind_application_configuration"),
                messages::message("config.unknown_theme", &[("id", id.to_string())]),
            ));
        }
        let _lock = self.lock_mutations()?;
        let existing = self.read()?;
        let create_default_recipe = existing.is_none();
        let mut document = existing.unwrap_or_else(new_config_document);
        let theme_table = document
            .entry("theme")
            .or_insert_with(|| Item::Table(Table::new()))
            .as_table_like_mut()
            .ok_or_else(|| {
                self.invalid(
                    messages::text("config.store_kind_application_configuration"),
                    messages::text("config.theme_table"),
                )
            })?;
        set_string(theme_table, "id", id);
        self.write_unlocked(document, create_default_recipe)
    }

    /// 将一个已存在的清理方案设为当前方案。
    pub fn save_recipe(&self, id: &str) -> Result<(), ConfigStoreError> {
        if !is_valid_id(id) {
            return Err(self.invalid(
                messages::text("config.store_kind_application_configuration"),
                messages::message("config.invalid_recipe_id", &[("id", id.to_string())]),
            ));
        }
        let _lock = self.lock_mutations()?;
        if !self.recipes()?.iter().any(|recipe| recipe.id == id) {
            return Err(ConfigStoreError::UnknownRecipe {
                id: id.to_owned(),
                directory: self.recipe_directory(),
            });
        }
        let mut document = self.read()?.unwrap_or_else(new_config_document);
        set_string(document.as_table_mut(), "recipe", id);
        self.write_unlocked(document, false)
    }

    pub(super) fn read(&self) -> Result<Option<DocumentMut>, ConfigStoreError> {
        if !self.path.exists() {
            return Ok(None);
        }
        let source = fs::read_to_string(&self.path).map_err(|source| {
            io_error(
                messages::text("config.store_action_read_configuration"),
                &self.path,
                source,
            )
        })?;
        let document = source.parse::<DocumentMut>().map_err(|error| {
            self.invalid(
                messages::text("config.store_kind_application_configuration"),
                messages::message("config.invalid_toml", &[("error", error.to_string())]),
            )
        })?;
        Ok(Some(document))
    }

    // All callers hold the store mutation lock through commit or rollback.
    pub(super) fn write_unlocked(
        &self,
        document: DocumentMut,
        create_default_recipe: bool,
    ) -> Result<(), ConfigStoreError> {
        let directory = self.config_directory();
        fs::create_dir_all(directory).map_err(|source| {
            io_error(
                messages::text("config.store_action_create_configuration_directory"),
                directory,
                source,
            )
        })?;
        if create_default_recipe {
            self.create_default_recipe()?;
        }
        let temporary = directory.join(format!(".config.{}.tmp", unique_suffix()));
        fs::write(&temporary, document.to_string()).map_err(|source| {
            io_error(
                messages::text("config.store_action_write_configuration"),
                &temporary,
                source,
            )
        })?;
        replace_file(&temporary, &self.path)
    }

    fn create_default_recipe(&self) -> Result<(), ConfigStoreError> {
        let recipe_directory = self.recipe_directory().join(DEFAULT_RECIPE_DIRECTORY_NAME);
        fs::create_dir_all(&recipe_directory).map_err(|source| {
            io_error(
                messages::text("config.store_action_create_recipe_directory"),
                &recipe_directory,
                source,
            )
        })?;
        let recipe_path = recipe_directory.join(DEFAULT_RECIPE_FILE_NAME);
        if recipe_path.exists() {
            return Ok(());
        }
        fs::write(&recipe_path, NEW_RECIPE).map_err(|source| {
            io_error(
                messages::text("config.store_action_write_starter_recipe"),
                &recipe_path,
                source,
            )
        })
    }
}

fn replace_file(source: &Path, destination: &Path) -> Result<(), ConfigStoreError> {
    if !destination.exists() {
        return fs::rename(source, destination).map_err(|error| {
            io_error(
                messages::text("config.store_action_install_configuration"),
                destination,
                error,
            )
        });
    }
    let backup = destination.with_extension(format!("backup.{}", unique_suffix()));
    fs::rename(destination, &backup).map_err(|error| {
        io_error(
            messages::text("config.store_action_prepare_configuration_replacement"),
            destination,
            error,
        )
    })?;
    if let Err(error) = fs::rename(source, destination) {
        let _ = fs::rename(&backup, destination);
        return Err(io_error(
            messages::text("config.store_action_replace_configuration"),
            destination,
            error,
        ));
    }
    let _ = fs::remove_file(backup);
    Ok(())
}

pub(super) fn new_config_document() -> DocumentMut {
    format!("schema-version = 2\nrecipe = \"{DEFAULT_RECIPE_DIRECTORY_NAME}\"\n")
        .parse()
        .expect("the built-in starter configuration is valid TOML")
}

pub(super) const NEW_RECIPE: &str = r#"schema-version = 2
name = "SpacePrune Default"
include = ["official:minimal"]
"#;
