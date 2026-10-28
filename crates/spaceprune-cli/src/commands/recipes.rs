//! Recipes command handling and terminal projection.

use super::pad_cell;
use crate::{
    cli::{CliCommand, OutputMode, ParsedArgs, RecipeCommand},
    recipe_picker::pick_recipe,
    theme::{Theme, Tone},
};
use anyhow::{Result, anyhow, bail};
use serde::Serialize;
use serde_json::json;
use spaceprune::api::ConfigStore;
use unicode_width::UnicodeWidthStr;

/// 执行 `spaceprune recipe` 管理命令。
pub(crate) fn run_recipe_command(
    parsed: &ParsedArgs,
    store: Option<&ConfigStore>,
    theme: Theme,
) -> Result<u8> {
    if parsed.action.is_some() || parsed.yes || parsed.no_progress {
        bail!("{}", crate::i18n::text("commands.recipe_arguments"));
    }
    let CliCommand::Recipe(command) = &parsed.command else {
        unreachable!("recipe dispatch requires a recipe command")
    };
    let store =
        store.ok_or_else(|| anyhow!("{}", crate::i18n::text("commands.config_path_unknown")))?;
    let recipes = store.recipes()?;

    match command {
        RecipeCommand::Follow => {
            store.save_cli_recipe(None)?;
            if parsed.output == OutputMode::Json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &json!({"recipe": store.cli_recipe()?, "following": true})
                    )?
                );
            } else {
                println!(
                    "{}",
                    theme.stdout(
                        crate::i18n::text("commands.recipe_following"),
                        Tone::Success
                    )
                );
            }
        }
        RecipeCommand::Pick if parsed.output == OutputMode::Json => {
            let active = store.cli_recipe()?;
            let recipe = recipes
                .iter()
                .find(|recipe| active.as_deref() == Some(recipe.id.as_str()));
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "recipe": active,
                    "name": recipe.map(|recipe| &recipe.name),
                    "entry": recipe.map(|recipe| &recipe.path),
                    "path": store.path(),
                }))?
            );
        }
        RecipeCommand::List => {
            let active = store.cli_recipe()?;
            #[derive(Serialize)]
            struct Entry<'a> {
                id: &'a str,
                name: &'a str,
                path: &'a std::path::Path,
                active: bool,
            }

            let entries = recipes
                .iter()
                .map(|recipe| Entry {
                    id: &recipe.id,
                    name: &recipe.name,
                    path: &recipe.path,
                    active: active.as_deref() == Some(recipe.id.as_str()),
                })
                .collect::<Vec<_>>();
            if parsed.output == OutputMode::Json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                println!(
                    "{}",
                    theme.stdout(
                        crate::i18n::message(
                            "commands.recipes_not_found",
                            &[(
                                "p0",
                                (store
                                    .path()
                                    .parent()
                                    .unwrap_or_else(|| std::path::Path::new("."))
                                    .join("recipes")
                                    .display())
                                .to_string()
                            )]
                        ),
                        Tone::Muted
                    )
                );
            } else {
                let id_width = entries
                    .iter()
                    .map(|entry| entry.id.width())
                    .fold("ID".width(), usize::max);
                let name_width = entries.iter().map(|entry| entry.name.width()).fold(
                    crate::i18n::text("commands.name_header").width(),
                    usize::max,
                );
                println!(
                    "{}  {}  {}  {}",
                    theme.stdout(
                        format!("{:<9}", crate::i18n::text("commands.status_header")),
                        Tone::Accent
                    ),
                    theme.stdout(pad_cell("ID", id_width), Tone::Accent),
                    theme.stdout(
                        pad_cell(crate::i18n::text("commands.name_header"), name_width),
                        Tone::Accent
                    ),
                    theme.stdout(crate::i18n::text("commands.path_header"), Tone::Accent)
                );
                for entry in entries {
                    let (status, tone) = if entry.active {
                        (crate::i18n::text("commands.active"), Tone::Success)
                    } else {
                        (crate::i18n::text("commands.available"), Tone::Muted)
                    };
                    println!(
                        "{}  {}  {}  {}",
                        theme.stdout(format!("{status:<9}"), tone),
                        theme.stdout(pad_cell(entry.id, id_width), Tone::Secondary),
                        pad_cell(entry.name, name_width),
                        theme.stdout(entry.path.display(), Tone::Muted)
                    );
                }
            }
        }
        RecipeCommand::Pick | RecipeCommand::Set(_) => {
            let active = if matches!(command, RecipeCommand::Pick) {
                store.cli_recipe()?
            } else {
                None
            };
            let id = match command {
                RecipeCommand::Set(id) => id.clone(),
                RecipeCommand::Pick => {
                    let Some(id) = pick_recipe(&recipes, active.as_deref(), theme)? else {
                        if parsed.output == OutputMode::Json {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&json!({
                                    "recipe": active,
                                    "cancelled": true,
                                }))?
                            );
                        } else {
                            println!(
                                "{}",
                                theme.stdout(
                                    crate::i18n::text("commands.recipe_unchanged"),
                                    Tone::Muted
                                )
                            );
                        }
                        return Ok(0);
                    };
                    id
                }
                RecipeCommand::List | RecipeCommand::Follow => {
                    unreachable!("recipe action handled separately")
                }
            };
            store.save_cli_recipe(Some(&id))?;
            let recipe = recipes
                .iter()
                .find(|recipe| recipe.id == id)
                .expect("ConfigStore only saves discovered recipes");
            if parsed.output == OutputMode::Json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "recipe": recipe.id,
                        "name": recipe.name,
                        "entry": recipe.path,
                        "path": store.path().with_extension("selection.toml"),
                    }))?
                );
            } else {
                println!(
                    "{}",
                    theme.stdout(
                        crate::i18n::message(
                            "commands.active_recipe",
                            &[
                                ("p0", (recipe.name).to_string()),
                                ("p1", (recipe.id).to_string())
                            ]
                        ),
                        Tone::Success
                    )
                );
                println!(
                    "{}",
                    theme.stdout(
                        crate::i18n::message(
                            "commands.saved_to",
                            &[(
                                "p0",
                                store
                                    .path()
                                    .with_extension("selection.toml")
                                    .display()
                                    .to_string()
                            )]
                        ),
                        Tone::Muted
                    )
                );
            }
        }
    }
    Ok(0)
}
