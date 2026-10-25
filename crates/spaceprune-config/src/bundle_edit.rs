//! Source-preserving edits to complete recipe bundles.

use crate::store::{invalid, set_string};
use crate::{AutomationDefinition, ConfigStoreError, RecipeBundle, is_valid_id};
use spaceprune_contracts::messages;
use std::path::Path;
use toml_edit::{DocumentMut, Item, Table};

/// Copy a bundle under a new stable ID while preserving included files and comments.
pub fn rename_recipe_bundle(
    bundle: &RecipeBundle,
    id: &str,
    name: Option<&str>,
) -> Result<RecipeBundle, ConfigStoreError> {
    let old_entry = format!("{}.toml", bundle.id);
    let new_entry = format!("{id}.toml");
    if !is_valid_id(id) {
        return Err(invalid(
            messages::text("config.store_kind_recipe_bundle"),
            Path::new(&new_entry),
            messages::text("config.invalid_recipe_id_plain"),
        ));
    }
    let mut bundle = bundle.clone();
    if old_entry != new_entry && bundle.files.contains_key(&new_entry) {
        return Err(invalid(
            messages::text("config.store_kind_recipe_bundle"),
            Path::new(&new_entry),
            messages::text("config.entry_exists"),
        ));
    }
    let source = bundle.files.remove(&old_entry).ok_or_else(|| {
        invalid(
            messages::text("config.store_kind_recipe_bundle"),
            Path::new(&old_entry),
            messages::text("config.entry_missing"),
        )
    })?;
    let source = if let Some(name) = name {
        let mut document = source.parse::<DocumentMut>().map_err(|error| {
            invalid(
                messages::text("config.store_kind_recipe_bundle"),
                Path::new(&old_entry),
                error.to_string(),
            )
        })?;
        set_string(document.as_table_mut(), "name", name);
        document.to_string()
    } else {
        source
    };
    bundle.id = id.to_owned();
    bundle.files.insert(new_entry, source);
    Ok(bundle)
}
/// Return a copy with the named daily automation updated or removed.
/// Entry overrides preserve includes; removal covers included configuration files.
pub fn update_bundle_automation(
    bundle: &RecipeBundle,
    automation_id: &str,
    definition: Option<&AutomationDefinition>,
) -> Result<RecipeBundle, ConfigStoreError> {
    if !is_valid_id(automation_id) {
        return Err(invalid(
            messages::text("config.store_kind_automation"),
            Path::new(&bundle.id),
            messages::text("config.invalid_automation_id"),
        ));
    }
    let mut bundle = bundle.clone();
    let entry = format!("{}.toml", bundle.id);
    for (path, source) in &mut bundle.files {
        if definition.is_some() && path != &entry {
            continue;
        }
        let mut document = match source.parse::<DocumentMut>() {
            Ok(document)
                if document
                    .get("schema-version")
                    .and_then(Item::as_integer)
                    .is_some() =>
            {
                document
            }
            _ if path != &entry => continue,
            _ => {
                return Err(invalid(
                    messages::text("config.store_kind_recipe_bundle"),
                    Path::new(path),
                    messages::text("config.entry_invalid"),
                ));
            }
        };
        if let Some(definition) = definition {
            let serialized = toml::to_string(definition).map_err(|error| {
                invalid(
                    messages::text("config.store_kind_automation"),
                    Path::new(&bundle.id),
                    error.to_string(),
                )
            })?;
            let automation = serialized.parse::<DocumentMut>().map_err(|error| {
                invalid(
                    messages::text("config.store_kind_automation"),
                    Path::new(&bundle.id),
                    error.to_string(),
                )
            })?;
            let table = document
                .entry("automations")
                .or_insert_with(|| Item::Table(Table::new()))
                .as_table_like_mut()
                .ok_or_else(|| {
                    invalid(
                        messages::text("config.store_kind_automation"),
                        Path::new(&bundle.id),
                        messages::text("config.automations_table"),
                    )
                })?;
            table.insert(automation_id, Item::Table(automation.as_table().clone()));
        } else if let Some(table) = document
            .get_mut("automations")
            .and_then(Item::as_table_like_mut)
        {
            table.remove(automation_id);
        }
        *source = document.to_string();
    }
    Ok(bundle)
}
