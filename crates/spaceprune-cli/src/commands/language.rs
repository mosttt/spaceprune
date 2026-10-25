//! Shared display-language preferences, independent of the active recipe.

use crate::{
    cli::{CliCommand, LanguageCommand, OutputMode, ParsedArgs},
    i18n,
    theme::{Theme, Tone},
};
use anyhow::{Result, anyhow, bail};
use spaceprune::api::ConfigStore;

pub(crate) fn run_language_command(
    parsed: &ParsedArgs,
    store: Option<&ConfigStore>,
    theme: Theme,
) -> Result<u8> {
    if parsed.action.is_some() || parsed.yes || parsed.no_progress {
        bail!("{}", i18n::text("commands.language_arguments"));
    }
    let CliCommand::Language(command) = parsed.command else {
        unreachable!("language command dispatch")
    };
    if let LanguageCommand::Set(preference) = command {
        store
            .ok_or_else(|| anyhow!(i18n::text("commands.config_path_unknown")))?
            .save_language_preference(preference)?;
    }
    let settings = store
        .map(ConfigStore::language_settings)
        .transpose()?
        .unwrap_or_default();
    i18n::set_language(settings.resolved);
    if parsed.output == OutputMode::Json {
        if command == LanguageCommand::List {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "preference": settings.preference,
                    "resolved": settings.resolved,
                    "languages": [{"id": "en", "name": "English"}, {"id": "zh-CN", "name": "简体中文"}],
                    "modes": ["system"],
                }))?
            );
        } else {
            println!("{}", serde_json::to_string_pretty(&settings)?);
        }
    } else {
        println!(
            "{}",
            theme.stdout(
                i18n::message(
                    "commands.language_current",
                    &[
                        ("preference", settings.preference.id().into()),
                        ("resolved", settings.resolved.id().into())
                    ]
                ),
                Tone::Accent
            )
        );
        if command == LanguageCommand::List {
            println!("\n{}  English", theme.stdout("en   ", Tone::Secondary));
            println!("{}  简体中文", theme.stdout("zh-CN", Tone::Secondary));
            println!(
                "\n{}  {}",
                theme.stdout("system", Tone::Secondary),
                i18n::text("commands.follow_system")
            );
        }
    }
    Ok(0)
}
