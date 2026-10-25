//! Modules command handling and terminal projection.

use super::pad_cell;
use crate::{
    cli::{CliCommand, OutputMode, ParsedArgs},
    theme::{Theme, Tone},
};
use anyhow::{Result, bail};
use serde::Serialize;
use spaceprune::api::ResolvedConfig;
use unicode_width::UnicodeWidthStr;

/// 执行 `spaceprune module` 管理命令。
pub(crate) fn run_module_command(
    parsed: &ParsedArgs,
    config: &ResolvedConfig,
    theme: Theme,
) -> Result<u8> {
    if parsed.action.is_some() || parsed.yes || parsed.no_progress {
        bail!("{}", crate::i18n::text("commands.module_arguments"));
    }
    let CliCommand::ModuleList { all } = &parsed.command else {
        unreachable!("module dispatch requires a module command")
    };
    let all = *all;
    {
        #[derive(Serialize)]
        struct Entry<'a> {
            id: &'a str,
            name: &'a str,
            description: &'a str,
            enabled: bool,
        }

        let mut modules = config
            .modules
            .iter()
            .filter(|(_, module)| all || module.enabled)
            .collect::<Vec<_>>();
        modules.sort_by(|(left_id, left), (right_id, right)| {
            left.order
                .cmp(&right.order)
                .then_with(|| left_id.cmp(right_id))
        });
        let entries = modules
            .into_iter()
            .map(|(id, module)| Entry {
                id,
                name: &module.name,
                description: &module.description,
                enabled: module.enabled,
            })
            .collect::<Vec<_>>();

        if parsed.output == OutputMode::Json {
            println!("{}", serde_json::to_string_pretty(&entries)?);
        } else if entries.is_empty() {
            println!(
                "{}",
                theme.stdout(
                    crate::i18n::message(
                        "commands.no_modules",
                        &[(
                            "p0",
                            (if all {
                                ""
                            } else {
                                crate::i18n::text("commands.enabled_prefix")
                            })
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
                    format!("{:<8}", crate::i18n::text("commands.status_header")),
                    Tone::Accent
                ),
                theme.stdout(pad_cell("ID", id_width), Tone::Accent),
                theme.stdout(
                    pad_cell(crate::i18n::text("commands.name_header"), name_width),
                    Tone::Accent
                ),
                theme.stdout(
                    crate::i18n::text("commands.description_header"),
                    Tone::Accent
                )
            );
            for entry in entries {
                let (status, tone) = if entry.enabled {
                    (crate::i18n::text("commands.enabled"), Tone::Success)
                } else {
                    (crate::i18n::text("commands.disabled"), Tone::Muted)
                };
                println!(
                    "{}  {}  {}  {}",
                    theme.stdout(format!("{status:<8}"), tone),
                    theme.stdout(pad_cell(entry.id, id_width), Tone::Secondary),
                    pad_cell(entry.name, name_width),
                    entry.description
                );
            }
        }
        Ok(0)
    }
}
