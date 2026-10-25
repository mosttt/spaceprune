//! Plugins command handling and terminal projection.

use super::pad_cell;
use crate::{
    cli::{CliCommand, OutputMode, ParsedArgs, PluginCommand},
    theme::{Theme, Tone},
};
use anyhow::{Result, anyhow, bail};
use serde_json::json;
use spaceprune::api::{
    PluginCatalogEntry, PluginHost, PluginKind, install_plugin, list_plugin_catalog, remove_plugin,
    set_plugin_enabled,
};
use unicode_width::UnicodeWidthStr;

/// 执行 `spaceprune plugin` 管理命令。
pub(crate) fn run_plugin_command(parsed: &ParsedArgs, theme: Theme) -> Result<u8> {
    if parsed.config.is_some() || parsed.action.is_some() || parsed.yes || parsed.no_progress {
        bail!("{}", crate::i18n::text("commands.plugin_arguments"));
    }
    let CliCommand::Plugin(command) = &parsed.command else {
        unreachable!("plugin dispatch requires a plugin command")
    };
    match command {
        PluginCommand::Catalog => {
            let entries = list_plugin_catalog()?;
            if parsed.output == OutputMode::Json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                for entry in entries {
                    let (status, tone) = plugin_status(&entry);
                    println!(
                        "{}  {}  {}  {}",
                        theme.stdout(format!("{status:<11}"), tone),
                        theme.stdout(&entry.id, Tone::Accent),
                        entry.version,
                        entry.name
                    );
                }
            }
        }
        PluginCommand::Enable(id) | PluginCommand::Disable(id) => {
            let entry = set_plugin_enabled(id, matches!(command, PluginCommand::Enable(_)))?;
            if parsed.output == OutputMode::Json {
                println!("{}", serde_json::to_string_pretty(&entry)?);
            } else {
                println!(
                    "{}",
                    theme.stdout(
                        crate::i18n::message(
                            "commands.plugin_changed",
                            &[
                                ("p0", (entry.id).to_string()),
                                (
                                    "p1",
                                    (if entry.enabled {
                                        crate::i18n::text("commands.enabled")
                                    } else {
                                        crate::i18n::text("commands.disabled")
                                    })
                                    .to_string()
                                )
                            ]
                        ),
                        Tone::Success
                    )
                );
            }
        }
        PluginCommand::List => {
            if parsed.output == OutputMode::Json {
                // Preserve the original external-plugin metadata contract for scripts.
                let host = PluginHost::discover()?;
                println!("{}", serde_json::to_string_pretty(&host.plugins())?);
            } else {
                print_plugin_list(&list_plugin_catalog()?, theme);
            }
        }
        PluginCommand::Inspect(id) => {
            let host = PluginHost::discover()?;
            let plugin = host.get(id).ok_or_else(|| {
                anyhow!(
                    "{}",
                    crate::i18n::message("commands.plugin_not_found", &[("id", (id).to_string())])
                )
            })?;
            if parsed.output == OutputMode::Json {
                println!("{}", serde_json::to_string_pretty(plugin)?);
            } else {
                println!("{}", human_manifest(plugin, theme));
            }
        }
        PluginCommand::Install(path) => {
            let plugin = install_plugin(path)?;
            if parsed.output == OutputMode::Json {
                println!("{}", serde_json::to_string_pretty(&plugin)?);
            } else {
                println!(
                    "{}",
                    theme.stdout(
                        crate::i18n::message(
                            "commands.plugin_installed",
                            &[
                                ("p0", (plugin.manifest.id).to_string()),
                                ("p1", (plugin.manifest.version).to_string()),
                                ("p2", (plugin.manifest_path.display()).to_string())
                            ]
                        ),
                        Tone::Success
                    )
                );
            }
        }
        PluginCommand::Remove(id) => {
            remove_plugin(id)?;
            if parsed.output == OutputMode::Json {
                println!("{}", json!({"removed": id}));
            } else {
                println!(
                    "{}",
                    theme.stdout(
                        crate::i18n::message(
                            "commands.plugin_removed",
                            &[("id", (id).to_string())]
                        ),
                        Tone::Success
                    )
                );
            }
        }
    }
    Ok(0)
}

fn human_manifest(plugin: &spaceprune::api::InstalledPlugin, theme: Theme) -> String {
    format!(
        "{} {}\n{} {}\n{} {}\n{} {}\n{} {}\n{} {}\n{} {}",
        theme.stdout(crate::i18n::text("commands.plugin_label"), Tone::Accent),
        plugin.manifest.id,
        theme.stdout(crate::i18n::text("commands.name_label"), Tone::Accent),
        plugin.manifest.name,
        theme.stdout(crate::i18n::text("commands.version_label"), Tone::Accent),
        plugin.manifest.version,
        theme.stdout(crate::i18n::text("commands.protocol_label"), Tone::Accent),
        plugin.manifest.protocol_version,
        theme.stdout(crate::i18n::text("commands.handlers_label"), Tone::Accent),
        plugin.manifest.handlers.join(", "),
        theme.stdout(crate::i18n::text("commands.manifest_label"), Tone::Accent),
        plugin.manifest_path.display(),
        theme.stdout(crate::i18n::text("commands.executable_label"), Tone::Accent),
        plugin.executable.display()
    )
}

fn print_plugin_list(entries: &[PluginCatalogEntry], theme: Theme) {
    let headers = [
        crate::i18n::text("commands.name_header"),
        crate::i18n::text("plugins.id_header"),
        crate::i18n::text("plugins.platform_header"),
        crate::i18n::text("commands.status_header"),
    ];
    for (index, (kind, title)) in [
        (PluginKind::Builtin, "plugins.builtin_heading"),
        (PluginKind::External, "plugins.external_heading"),
    ]
    .into_iter()
    .enumerate()
    {
        if index > 0 {
            println!();
        }
        println!("{}", theme.stdout(crate::i18n::text(title), Tone::Accent));
        let rows = entries
            .iter()
            .filter(|entry| entry.kind == kind)
            .map(|entry| {
                let platforms = if entry.platforms.is_empty() {
                    crate::i18n::text("plugins.cross_platform").to_owned()
                } else {
                    entry
                        .platforms
                        .iter()
                        .map(|platform| match platform.as_str() {
                            "linux" => "Linux",
                            "windows" => "Windows",
                            "macos" => "macOS",
                            platform => platform,
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let (status, tone) = plugin_status(entry);
                (
                    [
                        entry.name.clone(),
                        entry.id.clone(),
                        platforms,
                        status.into(),
                    ],
                    tone,
                )
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            println!(
                "  {}",
                theme.stdout(crate::i18n::text("plugins.none_installed"), Tone::Muted)
            );
            continue;
        }
        let widths: [usize; 3] = std::array::from_fn(|column| {
            rows.iter()
                .map(|(cells, _)| cells[column].width())
                .fold(headers[column].width(), usize::max)
        });
        println!(
            "  {}  {}  {}  {}",
            theme.stdout(pad_cell(headers[0], widths[0]), Tone::Muted),
            theme.stdout(pad_cell(headers[1], widths[1]), Tone::Muted),
            theme.stdout(pad_cell(headers[2], widths[2]), Tone::Muted),
            theme.stdout(headers[3], Tone::Muted)
        );
        for (cells, tone) in rows {
            println!(
                "  {}  {}  {}  {}",
                theme.stdout(pad_cell(&cells[0], widths[0]), Tone::Accent),
                theme.stdout(pad_cell(&cells[1], widths[1]), Tone::Secondary),
                pad_cell(&cells[2], widths[2]),
                theme.stdout(&cells[3], tone)
            );
        }
    }
}

fn plugin_status(entry: &PluginCatalogEntry) -> (&'static str, Tone) {
    if entry.conflicts_with_builtin {
        (crate::i18n::text("commands.id_conflict"), Tone::Warning)
    } else if !entry.supported {
        (crate::i18n::text("commands.unsupported"), Tone::Muted)
    } else if entry.enabled {
        (crate::i18n::text("commands.enabled"), Tone::Success)
    } else {
        (crate::i18n::text("commands.disabled"), Tone::Muted)
    }
}
