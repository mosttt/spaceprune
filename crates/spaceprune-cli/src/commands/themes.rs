//! Themes command handling and terminal projection.

use super::pad_cell;
use crate::{
    cli::{CliCommand, OutputMode, ParsedArgs, ThemeCommand},
    theme::{Theme, Tone},
    theme_picker::pick_theme,
};
use anyhow::{Result, anyhow, bail};
use serde::Serialize;
use serde_json::json;
use spaceprune::api::ConfigStore;
use unicode_width::UnicodeWidthStr;

/// 执行 `spaceprune theme` 管理命令。
pub(crate) fn run_theme_command(
    parsed: &ParsedArgs,
    store: Option<&ConfigStore>,
    active_theme: Theme,
) -> Result<u8> {
    if parsed.action.is_some() || parsed.yes || parsed.no_progress {
        bail!("{}", crate::i18n::text("commands.theme_arguments"));
    }
    let CliCommand::Theme(command) = &parsed.command else {
        unreachable!("theme dispatch requires a theme command")
    };
    match command {
        ThemeCommand::Pick if parsed.output == OutputMode::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "theme": active_theme.id(),
                    "name": active_theme.name(),
                    "path": store.map(ConfigStore::path),
                }))?
            );
        }
        ThemeCommand::List => {
            #[derive(Serialize)]
            struct Entry {
                id: &'static str,
                name: &'static str,
                active: bool,
            }

            let entries = Theme::all()
                .iter()
                .copied()
                .map(|theme| Entry {
                    id: theme.id(),
                    name: theme.name(),
                    active: theme == active_theme,
                })
                .collect::<Vec<_>>();
            if parsed.output == OutputMode::Json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                let id_width = entries
                    .iter()
                    .map(|entry| entry.id.width())
                    .fold("ID".width(), usize::max);
                println!(
                    "{}  {}  {}  {}",
                    active_theme.stdout(
                        format!("{:<9}", crate::i18n::text("commands.status_header")),
                        Tone::Accent
                    ),
                    active_theme.stdout(crate::i18n::text("commands.preview_header"), Tone::Accent),
                    active_theme.stdout(pad_cell("ID", id_width), Tone::Accent),
                    active_theme.stdout(crate::i18n::text("commands.name_header"), Tone::Accent)
                );
                for entry in entries {
                    let theme = Theme::named(entry.id).expect("listed themes are built in");
                    let (status, tone) = if entry.active {
                        (crate::i18n::text("commands.active"), Tone::Success)
                    } else {
                        (crate::i18n::text("commands.available"), Tone::Muted)
                    };
                    println!(
                        "{}  {}{}{}      {}  {}",
                        active_theme.stdout(format!("{status:<9}"), tone),
                        theme.stdout("●", Tone::Accent),
                        theme.stdout("●", Tone::Secondary),
                        theme.stdout("●", Tone::Success),
                        theme.stdout(pad_cell(entry.id, id_width), Tone::Secondary),
                        theme.display_name()
                    );
                }
            }
        }
        ThemeCommand::Pick | ThemeCommand::Set(_) => {
            let store = store
                .ok_or_else(|| anyhow!("{}", crate::i18n::text("commands.config_path_unknown")))?;
            let theme = match command {
                ThemeCommand::Set(id) => {
                    Theme::named(id).expect("theme IDs are restricted by clap")
                }
                ThemeCommand::Pick => {
                    let Some(theme) = pick_theme(active_theme)? else {
                        if parsed.output == OutputMode::Json {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&json!({
                                    "theme": active_theme.id(),
                                    "name": active_theme.name(),
                                    "cancelled": true,
                                }))?
                            );
                        } else {
                            println!(
                                "{}",
                                active_theme.stdout(
                                    crate::i18n::text("commands.theme_unchanged"),
                                    Tone::Muted
                                )
                            );
                        }
                        return Ok(0);
                    };
                    theme
                }
                ThemeCommand::List => unreachable!("theme list is handled separately"),
            };
            store.save_theme_id(theme.id())?;
            if parsed.output == OutputMode::Json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "theme": theme.id(),
                        "name": theme.name(),
                        "path": store.path(),
                    }))?
                );
            } else {
                println!(
                    "{}",
                    theme.stdout(
                        crate::i18n::message(
                            "commands.active_theme",
                            &[
                                ("p0", (theme.display_name()).to_string()),
                                ("p1", (theme.id()).to_string())
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
                            &[("p0", (store.path().display()).to_string())]
                        ),
                        Tone::Muted
                    )
                );
            }
        }
    }
    Ok(0)
}
