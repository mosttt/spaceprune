//! Recipe inventory, bundle preparation and conditional install/delete transactions.

use spaceprune_contracts::bundle::{RecipeBundle, validate_bundle_paths};
use spaceprune_contracts::messages;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use toml_edit::{DocumentMut, Item};

use super::{
    ConfigStore, ConfigStoreError, RecipeInfo, application::new_config_document, invalid, io_error,
    set_string, unique_suffix,
};
use crate::{AutomationDefinition, ConfigLoader, is_valid_id, update_bundle_automation};

impl ConfigStore {
    /// 列出 `recipes` 下符合目录与入口文件同名约定的清理方案。
    pub fn recipes(&self) -> Result<Vec<RecipeInfo>, ConfigStoreError> {
        let recipes_path = self.recipe_directory();
        if !recipes_path.is_dir() {
            return Ok(Vec::new());
        }
        let entries = fs::read_dir(&recipes_path).map_err(|source| {
            io_error(
                messages::text("config.store_action_read_recipe_directory"),
                &recipes_path,
                source,
            )
        })?;
        let mut recipes = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| {
                io_error(
                    messages::text("config.store_action_read_recipe_directory"),
                    &recipes_path,
                    source,
                )
            })?;
            let entry_path = entry.path();
            if !entry
                .file_type()
                .map_err(|source| {
                    io_error(
                        messages::text("config.store_action_inspect_recipe_path"),
                        &entry_path,
                        source,
                    )
                })?
                .is_dir()
            {
                continue;
            }
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !is_valid_id(&id) {
                continue;
            }
            let path = entry_path.join(format!("{id}.toml"));
            if !path.is_file() {
                continue;
            }
            let source = fs::read_to_string(&path).map_err(|source| {
                io_error(
                    messages::text("config.store_action_read_recipe"),
                    &path,
                    source,
                )
            })?;
            let document = source.parse::<DocumentMut>().map_err(|error| {
                invalid(
                    messages::text("config.store_kind_recipe"),
                    &path,
                    messages::message("config.invalid_toml", &[("error", error.to_string())]),
                )
            })?;
            let name = document.get("name").and_then(Item::as_str).ok_or_else(|| {
                invalid(
                    messages::text("config.store_kind_recipe"),
                    &path,
                    messages::text("config.name_string"),
                )
            })?;
            recipes.push(RecipeInfo {
                id,
                name: name.to_owned(),
                path,
            });
        }
        recipes.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(recipes)
    }

    /// 导出一个完整 Recipe bundle，供设备级云同步使用。
    pub fn export_recipe_bundle(&self, id: &str) -> Result<RecipeBundle, ConfigStoreError> {
        if !is_valid_id(id) {
            return Err(self.invalid(
                messages::text("config.store_kind_recipe"),
                messages::message("config.invalid_recipe_id", &[("id", id.to_string())]),
            ));
        }
        let directory = self.recipe_directory().join(id);
        let entry = directory.join(format!("{id}.toml"));
        if !entry.is_file() {
            return Err(ConfigStoreError::UnknownRecipe {
                id: id.to_owned(),
                directory: self.recipe_directory(),
            });
        }
        let mut files = BTreeMap::new();
        collect_bundle_files(&directory, &directory, &mut files)?;
        Ok(RecipeBundle {
            id: id.to_owned(),
            files,
        })
    }

    /// 校验并新增或替换一个自包含 Recipe 入口 TOML。
    ///
    /// 文件保存为 `recipes/<id>/<id>.toml`。校验和替换在同一目录完成；校验失败时原文件保持不变。
    /// 本方法不自动切换当前 Recipe。
    pub fn upsert_recipe(&self, id: &str, source: &str) -> Result<RecipeInfo, ConfigStoreError> {
        self.upsert_recipe_bundle(&RecipeBundle {
            id: id.to_owned(),
            files: BTreeMap::from([(format!("{id}.toml"), source.to_owned())]),
        })
    }

    /// 校验并原子新增或替换一个完整 Recipe bundle。
    ///
    /// bundle 只能包含普通 UTF-8 相对路径，且必须包含入口 `<id>.toml`。
    pub fn upsert_recipe_bundle(
        &self,
        bundle: &RecipeBundle,
    ) -> Result<RecipeInfo, ConfigStoreError> {
        let prepared = self.prepare_recipe_bundle(bundle)?;
        let staging = prepared.staging.clone();
        let result = (|| {
            let _lock = self.lock_mutations()?;
            install_prepared_bundle(prepared, || Ok(()))
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(staging);
        }
        result
    }

    /// 条件提交完整 Recipe bundle，并可在同一可回滚事务中将其设为当前 Recipe。
    ///
    /// `expected` 为 `None` 时要求目标 Recipe 尚不存在；为 `Some` 时要求磁盘 bundle
    /// 与给定基线完全一致。条件检查和最终替换由跨进程存储锁串行化。
    pub fn commit_recipe_bundle(
        &self,
        bundle: &RecipeBundle,
        expected: Option<&RecipeBundle>,
        activate: bool,
    ) -> Result<RecipeInfo, ConfigStoreError> {
        let prepared = self.prepare_recipe_bundle(bundle)?;
        let staging = prepared.staging.clone();
        let result = (|| {
            let _lock = self.lock_mutations()?;
            let current = self.export_recipe_bundle(&bundle.id);
            let matches_expectation = match (expected, current) {
                (None, Err(ConfigStoreError::UnknownRecipe { .. })) => true,
                (Some(expected), Ok(current)) => expected == &current,
                _ => false,
            };
            if !matches_expectation {
                return Err(ConfigStoreError::RecipeConflict(bundle.id.clone()));
            }

            let document = if activate {
                let mut document = self.read()?.unwrap_or_else(new_config_document);
                set_string(document.as_table_mut(), "recipe", &bundle.id);
                Some(document)
            } else {
                None
            };
            install_prepared_bundle(prepared, || match document {
                Some(document) => self.write_unlocked(document, false),
                None => Ok(()),
            })
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(staging);
        }
        result
    }

    /// Update a daily automation and conditionally commit the complete recipe.
    pub fn update_recipe_automation(
        &self,
        recipe_id: &str,
        automation_id: &str,
        definition: Option<&AutomationDefinition>,
    ) -> Result<RecipeInfo, ConfigStoreError> {
        let expected = self.export_recipe_bundle(recipe_id)?;
        let bundle = update_bundle_automation(&expected, automation_id, definition)?;
        self.commit_recipe_bundle(&bundle, Some(&expected), false)
    }

    fn prepare_recipe_bundle(
        &self,
        bundle: &RecipeBundle,
    ) -> Result<PreparedRecipeBundle, ConfigStoreError> {
        if !is_valid_id(&bundle.id) {
            return Err(self.invalid(
                messages::text("config.store_kind_recipe_bundle"),
                messages::message("config.invalid_recipe_id", &[("id", bundle.id.to_owned())]),
            ));
        }
        let entry_name = format!("{}.toml", bundle.id);
        if !bundle.files.contains_key(&entry_name) {
            return Err(self.invalid(
                messages::text("config.store_kind_recipe_bundle"),
                messages::message(
                    "config.bundle_entry",
                    &[("entry_name", entry_name.to_string())],
                ),
            ));
        }
        validate_bundle_paths(bundle.files.keys().map(String::as_str)).map_err(|message| {
            self.invalid(
                messages::text("config.store_kind_recipe_bundle"),
                messages::message("config.bundle_paths", &[("message", message.to_string())]),
            )
        })?;

        let root = self.recipe_directory();
        fs::create_dir_all(&root).map_err(|source| {
            io_error(
                messages::text("config.store_action_create_recipe_directory"),
                &root,
                source,
            )
        })?;
        let suffix = unique_suffix();
        let staging = root.join(format!(".{}.{}.tmp", bundle.id, suffix));
        fs::create_dir(&staging).map_err(|source| {
            io_error(
                messages::text("config.store_action_create_recipe_staging_directory"),
                &staging,
                source,
            )
        })?;
        let result = (|| {
            for (relative, contents) in &bundle.files {
                let path = staging.join(relative);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|source| {
                        io_error(
                            messages::text("config.store_action_create_recipe_bundle_directory"),
                            parent,
                            source,
                        )
                    })?;
                }
                fs::write(&path, contents).map_err(|source| {
                    io_error(
                        messages::text("config.store_action_write_recipe_bundle_file"),
                        &path,
                        source,
                    )
                })?;
            }
            let entry = staging.join(&entry_name);
            let config = ConfigLoader::new().load_path(&entry).map_err(|error| {
                invalid(
                    messages::text("config.store_kind_recipe_bundle"),
                    &entry,
                    error.to_string(),
                )
            })?;
            let destination = root.join(&bundle.id);
            Ok(PreparedRecipeBundle {
                staging: staging.clone(),
                destination: destination.clone(),
                info: RecipeInfo {
                    id: bundle.id.clone(),
                    name: config.name,
                    path: destination.join(entry_name),
                },
            })
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    }

    /// 删除一个未启用的本地 Recipe bundle。
    pub fn delete_recipe(&self, id: &str) -> Result<(), ConfigStoreError> {
        if !is_valid_id(id) {
            return Err(self.invalid(
                messages::text("config.store_kind_recipe"),
                messages::message("config.invalid_recipe_id", &[("id", id.to_string())]),
            ));
        }
        let _lock = self.lock_mutations()?;
        if self.active_recipe()?.as_deref() == Some(id) {
            return Err(ConfigStoreError::ActiveRecipe(id.to_owned()));
        }
        let directory = self.recipe_directory().join(id);
        let path = directory.join(format!("{id}.toml"));
        if !path.is_file() {
            return Err(ConfigStoreError::UnknownRecipe {
                id: id.to_owned(),
                directory: self.recipe_directory(),
            });
        }
        fs::remove_dir_all(&directory).map_err(|source| {
            io_error(
                messages::text("config.store_action_delete_recipe_directory"),
                &directory,
                source,
            )
        })
    }

    /// 仅在磁盘 bundle 与 `expected` 完全一致时删除未启用的 Recipe。
    ///
    /// 条件检查和删除由跨进程存储锁串行化，避免删除预检后被其他
    /// SpacePrune 进程替换的新内容。目标已不存在时视为删除成功。
    pub fn delete_recipe_if_unchanged(
        &self,
        expected: &RecipeBundle,
    ) -> Result<(), ConfigStoreError> {
        let id = &expected.id;
        if !is_valid_id(id) {
            return Err(self.invalid(
                messages::text("config.store_kind_recipe"),
                messages::message("config.invalid_recipe_id", &[("id", id.to_string())]),
            ));
        }
        let _lock = self.lock_mutations()?;
        if self.active_recipe()?.as_deref() == Some(id.as_str()) {
            return Err(ConfigStoreError::ActiveRecipe(id.clone()));
        }
        let directory = self.recipe_directory().join(id);
        let current = match self.export_recipe_bundle(id) {
            Ok(current) => current,
            Err(ConfigStoreError::UnknownRecipe { .. }) if !directory.exists() => return Ok(()),
            Err(error) => return Err(error),
        };
        if &current != expected {
            return Err(ConfigStoreError::RecipeConflict(id.clone()));
        }
        fs::remove_dir_all(&directory).map_err(|source| {
            io_error(
                messages::text("config.store_action_delete_recipe_directory"),
                &directory,
                source,
            )
        })
    }
}

struct PreparedRecipeBundle {
    staging: PathBuf,
    destination: PathBuf,
    info: RecipeInfo,
}

fn collect_bundle_files(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<(), ConfigStoreError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|source| {
            io_error(
                messages::text("config.store_action_read_recipe_bundle"),
                directory,
                source,
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| {
            io_error(
                messages::text("config.store_action_read_recipe_bundle"),
                directory,
                source,
            )
        })?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| {
            io_error(
                messages::text("config.store_action_inspect_recipe_bundle"),
                &path,
                source,
            )
        })?;
        if file_type.is_symlink() || (!file_type.is_file() && !file_type.is_dir()) {
            return Err(invalid(
                messages::text("config.store_kind_recipe_bundle"),
                &path,
                messages::text("config.regular_files_only"),
            ));
        }
        if file_type.is_dir() {
            collect_bundle_files(root, &path, files)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("bundle files are discovered below their root")
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let contents = fs::read_to_string(&path).map_err(|source| {
            io_error(
                messages::text("config.store_action_read_recipe_bundle_file"),
                &path,
                source,
            )
        })?;
        files.insert(relative, contents);
    }
    Ok(())
}

fn install_prepared_bundle(
    prepared: PreparedRecipeBundle,
    commit_configuration: impl FnOnce() -> Result<(), ConfigStoreError>,
) -> Result<RecipeInfo, ConfigStoreError> {
    let backup = prepared.destination.exists().then(|| {
        prepared
            .destination
            .with_extension(format!("backup.{}", unique_suffix()))
    });
    if let Some(backup) = &backup {
        fs::rename(&prepared.destination, backup).map_err(|error| {
            io_error(
                messages::text("config.store_action_prepare_recipe_bundle_replacement"),
                &prepared.destination,
                error,
            )
        })?;
    }
    if let Err(error) = fs::rename(&prepared.staging, &prepared.destination) {
        if let Some(backup) = &backup {
            let _ = fs::rename(backup, &prepared.destination);
        }
        return Err(io_error(
            messages::text("config.store_action_install_recipe_bundle"),
            &prepared.destination,
            error,
        ));
    }
    if let Err(commit_error) = commit_configuration() {
        let rejected = prepared
            .destination
            .with_extension(format!("rollback.{}", unique_suffix()));
        fs::rename(&prepared.destination, &rejected).map_err(|error| {
            io_error(
                messages::text("config.store_action_roll_back_recipe_bundle"),
                &prepared.destination,
                error,
            )
        })?;
        if let Some(backup) = &backup
            && let Err(error) = fs::rename(backup, &prepared.destination)
        {
            let _ = fs::rename(&rejected, &prepared.destination);
            return Err(io_error(
                messages::text("config.store_action_restore_recipe_bundle"),
                &prepared.destination,
                error,
            ));
        }
        let _ = fs::remove_dir_all(rejected);
        return Err(commit_error);
    }
    if let Some(backup) = backup {
        let _ = fs::remove_dir_all(backup);
    }
    Ok(prepared.info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn failed_configuration_commit_rolls_recipe_directory_back() {
        let temp = tempdir().unwrap();
        let destination = temp.path().join("recipe");
        let staging = temp.path().join("staging");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("recipe.toml"), "old").unwrap();
        fs::create_dir(&staging).unwrap();
        fs::write(staging.join("recipe.toml"), "new").unwrap();
        let expected = ConfigStoreError::RecipeConflict("forced".to_owned());
        let error = install_prepared_bundle(
            PreparedRecipeBundle {
                staging,
                destination: destination.clone(),
                info: RecipeInfo {
                    id: "recipe".to_owned(),
                    name: "Recipe".to_owned(),
                    path: destination.join("recipe.toml"),
                },
            },
            || Err(expected),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ConfigStoreError::RecipeConflict(id) if id == "forced"
        ));
        assert_eq!(
            fs::read_to_string(destination.join("recipe.toml")).unwrap(),
            "old"
        );
    }
}
