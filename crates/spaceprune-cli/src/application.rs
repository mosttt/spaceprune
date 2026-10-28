//! CLI startup, configuration loading and command dispatch.

mod cleanup;

use crate::{
    authorization,
    cli::{self, CliCommand, ParseResult, explicit_config_path, parse},
    commands::{
        run_automation_command, run_language_command, run_module_command, run_plugin_command,
        run_recipe_command, run_theme_command,
    },
    theme::{Theme, Tone},
};
use anyhow::{Result, anyhow};
use spaceprune::api::{ConfigLoader, ConfigStore, LanguagePreference, ResolvedConfig};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::ExitCode,
};

pub(crate) fn run_cli<I, T>(arguments: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let arguments = arguments.into_iter().map(Into::into).collect::<Vec<_>>();
    crate::i18n::set_language(LanguagePreference::System.resolve());
    if let Ok(path) = explicit_config_path(&arguments)
        && let Some(store) = ConfigStore::discover(path.as_deref())
        && let Ok(settings) = store.language_settings()
    {
        crate::i18n::set_language(settings.resolved);
    }
    if let Some(parsed) = cli::authorization_args(&arguments) {
        if parsed.output == cli::OutputMode::Json {
            spaceprune::api::set_display_language("en");
        }
        return exit_code(
            authorization::run(&parsed, Theme::default()),
            Theme::default(),
        );
    }
    let startup = load_startup(&arguments);
    let loaded_theme = startup
        .as_ref()
        .map(|(_, _, theme)| *theme)
        .unwrap_or_default();
    exit_code(
        startup.and_then(|(config_path, config_store, theme)| {
            run(arguments, config_path, config_store, theme)
        }),
        loaded_theme,
    )
}

fn exit_code(result: Result<u8>, theme: Theme) -> ExitCode {
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!(
                "{} {error:#}",
                theme.stderr(crate::i18n::text("commands.error_label"), Tone::Error)
            );
            ExitCode::from(2)
        }
    }
}

fn load_startup(arguments: &[OsString]) -> Result<(Option<PathBuf>, Option<ConfigStore>, Theme)> {
    let config_path = explicit_config_path(arguments)?;
    let config_store = ConfigStore::discover(config_path.as_deref());
    let theme_id = config_store
        .as_ref()
        .map(ConfigStore::load_theme_id)
        .transpose()?
        .unwrap_or_else(|| Theme::default().id().to_owned());
    let theme = Theme::named(&theme_id).ok_or_else(|| {
        if let Some(store) = &config_store {
            anyhow!(
                "{}",
                crate::i18n::message(
                    "commands.config_unknown_theme",
                    &[
                        ("p0", (store.path().display()).to_string()),
                        ("theme_id", (theme_id).to_string())
                    ]
                )
            )
        } else {
            anyhow!(
                "{}",
                crate::i18n::message(
                    "commands.unknown_theme",
                    &[("theme_id", (theme_id).to_string())]
                )
            )
        }
    })?;
    Ok((config_path, config_store, theme))
}

fn run(
    arguments: Vec<OsString>,
    config_path: Option<PathBuf>,
    config_store: Option<ConfigStore>,
    theme: Theme,
) -> Result<u8> {
    let bootstrap = match parse(&arguments, None, theme)? {
        ParseResult::Parsed(parsed) => parsed,
        ParseResult::Exit(code) => return Ok(code),
    };
    if bootstrap.output == cli::OutputMode::Json {
        spaceprune::api::set_display_language("en");
    }
    if matches!(&bootstrap.command, CliCommand::Language(_)) {
        return run_language_command(&bootstrap, config_store.as_ref(), theme);
    }
    if matches!(&bootstrap.command, CliCommand::Plugin(_)) {
        return run_plugin_command(&bootstrap, theme);
    }
    if matches!(&bootstrap.command, CliCommand::Theme(_)) {
        return run_theme_command(&bootstrap, config_store.as_ref(), theme);
    }
    if matches!(&bootstrap.command, CliCommand::Recipe(_)) {
        return run_recipe_command(&bootstrap, config_store.as_ref(), theme);
    }
    let config = if matches!(&bootstrap.command, CliCommand::Automation(_)) {
        load_default_recipe(config_path.as_deref())?
    } else {
        load_config(config_path.as_deref())?
    };
    let parsed = match parse(&arguments, Some(&config), theme)? {
        ParseResult::Parsed(parsed) => parsed,
        ParseResult::Exit(code) => return Ok(code),
    };
    if matches!(&parsed.command, CliCommand::ModuleList { .. }) {
        return run_module_command(&parsed, &config, theme);
    }
    if matches!(&parsed.command, CliCommand::Automation(_)) {
        return run_automation_command(parsed, config, theme);
    }
    cleanup::run(parsed, config, theme)
}

fn load_config(path: Option<&Path>) -> Result<ResolvedConfig> {
    let mut loader = ConfigLoader::new();
    if let Some(store) = ConfigStore::discover(path)
        && store.path().is_file()
    {
        let recipe = store.cli_recipe()?;
        return loader
            .load_app_path_with_recipe(store.path(), recipe.as_deref())
            .map_err(Into::into);
    }
    load_default_recipe(path)
}

fn load_default_recipe(path: Option<&Path>) -> Result<ResolvedConfig> {
    let mut loader = ConfigLoader::new();
    if let Some(path) = path {
        loader.load_app_path(path).map_err(Into::into)
    } else {
        loader.load_default().map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_recipe_override_does_not_change_automation_default() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(temp.path().join("config.toml"));
        for id in ["default", "desktop", "manual"] {
            store
                .upsert_recipe(
                    id,
                    &format!("schema-version = 2\nname = '{id}'\ninclude = ['official:minimal']\n"),
                )
                .unwrap();
        }
        store.save_recipe("default").unwrap();
        store.remember_used_recipe("desktop").unwrap();
        assert_eq!(load_config(Some(store.path())).unwrap().name, "desktop");
        store.save_cli_recipe(Some("manual")).unwrap();
        assert_eq!(load_config(Some(store.path())).unwrap().name, "manual");
        assert_eq!(
            load_default_recipe(Some(store.path())).unwrap().name,
            "default"
        );
    }
}
