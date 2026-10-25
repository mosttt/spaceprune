// Regression tests for atomic storage and source-preserving recipe operations.

use std::collections::BTreeMap;
use tempfile::tempdir;

use super::application::NEW_RECIPE;
use crate::{ConfigLoader, RecipeBundle};

use super::*;

#[test]
fn setting_theme_creates_the_default_layout() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(temp.path().join("config.toml"));
    store.save_theme_id("orbit").unwrap();

    let config = fs::read_to_string(store.path()).unwrap();
    assert!(config.contains("recipe = \"default\""));
    assert!(config.contains("[theme]\nid = \"orbit\""));
    assert_eq!(
        fs::read_to_string(temp.path().join("recipes/default/default.toml")).unwrap(),
        NEW_RECIPE
    );
}

#[test]
fn updates_preserve_existing_comments() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(temp.path().join("config.toml"));
    fs::write(
        store.path(),
        r#"# Personal settings
schema-version = 2
recipe = "personal"

[theme]
id = "nord" # selected theme
"#,
    )
    .unwrap();
    let recipe_directory = temp.path().join("recipes/work");
    fs::create_dir_all(&recipe_directory).unwrap();
    fs::write(
        recipe_directory.join("work.toml"),
        "schema-version = 2\nname = 'Work'\n",
    )
    .unwrap();

    store.save_theme_id("graphite").unwrap();
    store.save_recipe("work").unwrap();
    let saved = fs::read_to_string(store.path()).unwrap();
    assert!(saved.contains("# Personal settings"));
    assert!(saved.contains("recipe = \"work\""));
    assert!(saved.contains("id = \"graphite\" # selected theme"));
}

#[test]
fn automation_edits_preserve_includes_and_delete_inherited_definitions() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(temp.path().join("config.toml"));
    let bundle = RecipeBundle {
        id: "work".to_owned(),
        files: BTreeMap::from([
            ("work.toml".to_owned(), "# Keep this comment\nschema-version = 2\nname = 'Work'\ninclude = ['shared.toml']\n".to_owned()),
            ("shared.toml".to_owned(), "schema-version = 2\n[modules.cache]\nname = 'Cache'\n[automations.daily]\nname = 'Original'\nat = '03:00'\nmodules = ['cache']\n".to_owned()),
        ]),
    };
    store.commit_recipe_bundle(&bundle, None, true).unwrap();
    let definition: crate::AutomationDefinition =
        toml::from_str("name = 'Edited'\nat = '06:45'\nmodules = ['cache']\n").unwrap();
    store
        .update_recipe_automation("work", "daily", Some(&definition))
        .unwrap();
    let config = crate::ConfigLoader::new()
        .load_app_path(store.path())
        .unwrap();
    assert_eq!(config.automations["daily"], definition);
    let edited = store.export_recipe_bundle("work").unwrap();
    assert!(edited.files["work.toml"].contains("# Keep this comment"));
    assert_eq!(edited.files["shared.toml"], bundle.files["shared.toml"]);
    store
        .update_recipe_automation("work", "daily", None)
        .unwrap();
    let config = crate::ConfigLoader::new()
        .load_app_path(store.path())
        .unwrap();
    assert!(config.automations.is_empty());
    assert!(config.modules.contains_key("cache"));
}

#[test]
fn bundle_rename_rejects_file_collisions_and_preserves_contents() {
    let bundle = RecipeBundle {
        id: "work".to_owned(),
        files: BTreeMap::from([
            (
                "work.toml".to_owned(),
                "# Keep\nschema-version = 2\nname = 'Work'\ninclude = ['shared.toml']\n".to_owned(),
            ),
            ("shared.toml".to_owned(), "schema-version = 2\n".to_owned()),
        ]),
    };
    assert!(crate::rename_recipe_bundle(&bundle, "shared", None).is_err());
    assert!(crate::rename_recipe_bundle(&bundle, "../escape", None).is_err());
    let copy = crate::rename_recipe_bundle(&bundle, "copy", Some("My copy")).unwrap();
    assert!(copy.files["copy.toml"].contains("# Keep"));
    assert!(copy.files["copy.toml"].contains("My copy"));
    assert_eq!(copy.files["shared.toml"], bundle.files["shared.toml"]);
}

#[test]
fn discovers_only_id_named_recipe_directories() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(temp.path().join("config.toml"));
    let directory = temp.path().join("recipes/personal");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("personal.toml"),
        "schema-version = 2\nname = 'Personal'\n",
    )
    .unwrap();
    let ignored = temp.path().join("recipes/mismatched");
    fs::create_dir_all(&ignored).unwrap();
    fs::write(
        ignored.join("other.toml"),
        "schema-version = 2\nname = 'Other'\n",
    )
    .unwrap();

    let recipes = store.recipes().unwrap();
    assert_eq!(recipes.len(), 1);
    assert_eq!(recipes[0].id, "personal");
    assert_eq!(recipes[0].name, "Personal");
}

#[test]
fn selecting_a_missing_recipe_is_rejected() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(temp.path().join("config.toml"));
    let error = store.save_recipe("missing").unwrap_err();
    assert!(matches!(error, ConfigStoreError::UnknownRecipe { .. }));
}

#[test]
fn upsert_recipe_validates_before_replacing() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(temp.path().join("config.toml"));
    let original = "schema-version = 2\nname = 'Generated'\ninclude = ['official:minimal']\n";
    let recipe = store.upsert_recipe("generated", original).unwrap();
    assert_eq!(recipe.name, "Generated");
    assert_eq!(fs::read_to_string(&recipe.path).unwrap(), original);
    fs::write(recipe.path.parent().unwrap().join("stale.toml"), "stale").unwrap();

    let replacement = "schema-version = 2\nname = 'Replacement'\n";
    let recipe = store.upsert_recipe("generated", replacement).unwrap();
    assert_eq!(recipe.name, "Replacement");
    assert!(!recipe.path.parent().unwrap().join("stale.toml").exists());

    let error = store
        .upsert_recipe("generated", "schema-version = 2\nname = 42\n")
        .unwrap_err();
    assert!(error.to_string().contains("invalid"));
    assert_eq!(fs::read_to_string(&recipe.path).unwrap(), replacement);
}

#[test]
fn delete_recipe_rejects_the_active_recipe() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(temp.path().join("config.toml"));
    store
        .upsert_recipe("generated", "schema-version = 2\nname = 'Generated'\n")
        .unwrap();
    store.save_recipe("generated").unwrap();
    assert!(matches!(
        store.delete_recipe("generated"),
        Err(ConfigStoreError::ActiveRecipe(_))
    ));

    store
        .upsert_recipe("spare", "schema-version = 2\nname = 'Spare'\n")
        .unwrap();
    store.delete_recipe("spare").unwrap();
    assert!(!store.recipe_directory().join("spare").exists());
}

#[test]
fn conditional_delete_preserves_a_bundle_changed_by_another_store() {
    let temp = tempdir().unwrap();
    let config_path = temp.path().join("config.toml");
    let store = ConfigStore::new(&config_path);
    let original = RecipeBundle {
        id: "shared".to_owned(),
        files: BTreeMap::from([(
            "shared.toml".to_owned(),
            "schema-version = 2\nname = 'Original'\n".to_owned(),
        )]),
    };
    store.upsert_recipe_bundle(&original).unwrap();
    let expected = store.export_recipe_bundle("shared").unwrap();

    let replacement = RecipeBundle {
        id: "shared".to_owned(),
        files: BTreeMap::from([(
            "shared.toml".to_owned(),
            "schema-version = 2\nname = 'Replacement'\n".to_owned(),
        )]),
    };
    let writer = ConfigStore::new(config_path);
    let replacement_for_writer = replacement.clone();
    std::thread::spawn(move || writer.upsert_recipe_bundle(&replacement_for_writer))
        .join()
        .unwrap()
        .unwrap();

    assert!(matches!(
        store.delete_recipe_if_unchanged(&expected),
        Err(ConfigStoreError::RecipeConflict(id)) if id == "shared"
    ));
    assert_eq!(store.export_recipe_bundle("shared").unwrap(), replacement);

    let current = store.export_recipe_bundle("shared").unwrap();
    store.delete_recipe_if_unchanged(&current).unwrap();
    store.delete_recipe_if_unchanged(&current).unwrap();
    assert!(!store.recipe_directory().join("shared").exists());
}

#[test]
fn recipe_bundle_round_trips_all_files() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(temp.path().join("config.toml"));
    let bundle = RecipeBundle {
        id: "synced".to_owned(),
        files: BTreeMap::from([
            (
                "synced.toml".to_owned(),
                "schema-version = 2\nname = 'Synced'\ninclude = ['parts/cache.toml']\n".to_owned(),
            ),
            (
                "parts/cache.toml".to_owned(),
                "schema-version = 2\n[modules.cache]\nname = 'Cache'\n".to_owned(),
            ),
        ]),
    };
    let recipe = store.upsert_recipe_bundle(&bundle).unwrap();
    assert_eq!(recipe.name, "Synced");
    assert_eq!(store.export_recipe_bundle("synced").unwrap(), bundle);
}

#[test]
fn conditional_recipe_commit_activates_once_and_rejects_stale_writers() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(temp.path().join("config.toml"));
    let first = RecipeBundle {
        id: "conditional".to_owned(),
        files: BTreeMap::from([(
            "conditional.toml".to_owned(),
            "schema-version = 2\nname = 'First'\n".to_owned(),
        )]),
    };
    store
        .commit_recipe_bundle(&first, None, true)
        .expect("首次条件提交");
    assert_eq!(
        store.active_recipe().unwrap().as_deref(),
        Some("conditional")
    );
    assert!(matches!(
        store.commit_recipe_bundle(&first, None, false),
        Err(ConfigStoreError::RecipeConflict(id)) if id == "conditional"
    ));

    let second = RecipeBundle {
        id: "conditional".to_owned(),
        files: BTreeMap::from([(
            "conditional.toml".to_owned(),
            "schema-version = 2\nname = 'Second'\n".to_owned(),
        )]),
    };
    store
        .commit_recipe_bundle(&second, Some(&first), false)
        .expect("基线一致时替换");
    assert!(matches!(
        store.commit_recipe_bundle(&first, Some(&first), false),
        Err(ConfigStoreError::RecipeConflict(id)) if id == "conditional"
    ));
    assert_eq!(store.export_recipe_bundle("conditional").unwrap(), second);
}

#[test]
fn recipe_bundle_rejects_paths_outside_its_root() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(temp.path().join("config.toml"));
    let bundle = RecipeBundle {
        id: "synced".to_owned(),
        files: BTreeMap::from([
            (
                "synced.toml".to_owned(),
                "schema-version = 2\nname = 'Synced'\n".to_owned(),
            ),
            ("../outside.toml".to_owned(), "secret".to_owned()),
        ]),
    };
    assert!(
        store
            .upsert_recipe_bundle(&bundle)
            .unwrap_err()
            .to_string()
            .contains("invalid path")
    );

    for invalid in ["parts//cache.toml", "parts/", "CON.toml", "cache:old.toml"] {
        let bundle = RecipeBundle {
            id: "synced".to_owned(),
            files: BTreeMap::from([
                (
                    "synced.toml".to_owned(),
                    "schema-version = 2\nname = 'Synced'\n".to_owned(),
                ),
                (invalid.to_owned(), "x".to_owned()),
            ]),
        };
        assert!(store.upsert_recipe_bundle(&bundle).is_err(), "{invalid}");
    }

    let conflicting = RecipeBundle {
        id: "synced".to_owned(),
        files: BTreeMap::from([
            (
                "synced.toml".to_owned(),
                "schema-version = 2\nname = 'Synced'\n".to_owned(),
            ),
            ("parts".to_owned(), "x".to_owned()),
            ("parts/cache.toml".to_owned(), "x".to_owned()),
        ]),
    };
    assert!(store.upsert_recipe_bundle(&conflicting).is_err());

    let case_conflicting = RecipeBundle {
        id: "synced".to_owned(),
        files: BTreeMap::from([
            (
                "synced.toml".to_owned(),
                "schema-version = 2\nname = 'Synced'\n".to_owned(),
            ),
            ("parts/A.toml".to_owned(), "x".to_owned()),
            ("parts/a.toml".to_owned(), "x".to_owned()),
        ]),
    };
    assert!(store.upsert_recipe_bundle(&case_conflicting).is_err());
}

#[test]
fn conflicting_bundle_prefixes_leave_the_complete_existing_recipe_unchanged() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(temp.path().join("config.toml"));
    let original = RecipeBundle {
        id: "work".to_owned(),
        files: BTreeMap::from([
            (
                "work.toml".to_owned(),
                "# Keep this comment\nschema-version = 2\nname = 'Work'\n".to_owned(),
            ),
            (
                "platforms/windows.toml".to_owned(),
                "# Preserve inactive source\nschema-version = 2\nname = 'Work'\n".to_owned(),
            ),
        ]),
    };
    store.upsert_recipe_bundle(&original).unwrap();
    for (file, descendant) in [
        ("a.toml", "A.toml/x.toml"),
        ("PARTS", "parts/x.toml"),
        ("Parts/a.toml", "parts/b.toml"),
    ] {
        let mut replacement = original.clone();
        replacement.files.insert(file.to_owned(), "file".to_owned());
        replacement
            .files
            .insert(descendant.to_owned(), "descendant".to_owned());
        let error = store
            .commit_recipe_bundle(&replacement, Some(&original), false)
            .unwrap_err();
        assert!(matches!(error, ConfigStoreError::Invalid { .. }));
        assert_eq!(store.export_recipe_bundle("work").unwrap(), original);
        assert_eq!(fs::read_dir(store.recipe_directory()).unwrap().count(), 1);
    }
}

#[test]
fn legacy_theme_preferences_load_without_rewriting_the_application() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(temp.path().join("config.toml"));
    let recipe_directory = temp.path().join("recipes/personal");
    fs::create_dir_all(&recipe_directory).unwrap();
    fs::write(
        recipe_directory.join("personal.toml"),
        "schema-version = 2\nname = 'Personal'\n",
    )
    .unwrap();

    for (legacy, current) in [
        ("tokyo-night", "aurora"),
        ("nord", "aurora"),
        ("catppuccin-mocha", "orbit"),
        ("dracula", "orbit"),
        ("monokai", "orbit"),
        ("vscode-dark-modern", "graphite"),
        ("vscode-light-modern", "graphite"),
        ("quiet-light", "graphite"),
        ("solarized-dark", "forest"),
    ] {
        let original = format!(
            "# Personal settings\nschema-version = 2\nrecipe = 'personal'\n[theme]\nid = '{legacy}'\n"
        );
        fs::write(store.path(), &original).unwrap();

        assert_eq!(store.load_theme_id().unwrap(), current);
        assert_eq!(
            ConfigLoader::new()
                .load_app_path(store.path())
                .unwrap()
                .name,
            "Personal"
        );
        assert!(store.save_theme_id(legacy).is_err());
        assert_eq!(fs::read_to_string(store.path()).unwrap(), original);
    }
}

#[test]
fn unknown_themes_are_rejected_on_read_and_write() {
    let temp = tempdir().unwrap();
    let store = ConfigStore::new(temp.path().join("config.toml"));
    fs::write(
        store.path(),
        "schema-version = 2\nrecipe = 'personal'\n[theme]\nid = 'unknown'\n",
    )
    .unwrap();

    assert!(
        store
            .load_theme_id()
            .unwrap_err()
            .to_string()
            .contains("unknown theme")
    );
    assert!(
        store
            .save_theme_id("unknown")
            .unwrap_err()
            .to_string()
            .contains("unknown theme")
    );
}

#[test]
fn language_is_shared_without_loading_or_rewriting_recipe() {
    use crate::{Language, LanguagePreference};
    let temp = tempdir().unwrap();
    let path = temp.path().join("config.toml");
    fs::write(&path, "# Shared preferences\nschema-version = 2\nrecipe = 'broken-recipe'\n[theme]\nid = 'graphite'\n").unwrap();
    let store = ConfigStore::new(&path);
    assert_eq!(
        store.load_language_preference().unwrap(),
        LanguagePreference::System
    );
    store
        .save_language_preference(LanguagePreference::SimplifiedChinese)
        .unwrap();
    let second_client = ConfigStore::new(&path);
    assert_eq!(
        second_client.language_settings().unwrap().resolved,
        Language::SimplifiedChinese
    );
    let contents = fs::read_to_string(&path).unwrap();
    assert!(contents.contains("# Shared preferences"));
    assert!(contents.contains("recipe = 'broken-recipe'"));
    assert!(contents.contains("id = 'graphite'"));
    second_client
        .save_language_preference(LanguagePreference::English)
        .unwrap();
    assert_eq!(
        store.language_settings().unwrap().resolved,
        Language::English
    );
}

#[test]
fn unsupported_saved_language_does_not_invalidate_application_config() {
    use crate::{AppConfig, Language, LanguagePreference};
    let temp = tempdir().unwrap();
    let path = temp.path().join("config.toml");
    let contents = "schema-version = 2\nrecipe = 'default'\nlanguage = 'fr-FR'\n";
    fs::write(&path, contents).unwrap();
    let parsed: AppConfig = toml::from_str(contents).unwrap();
    assert_eq!(parsed.language, LanguagePreference::English);
    assert_eq!(
        ConfigStore::new(path).language_settings().unwrap().resolved,
        Language::English
    );
}
