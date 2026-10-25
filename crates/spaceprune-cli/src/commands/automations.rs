//! Automations command handling and terminal projection.

use crate::{
    cli::{AutomationCommand, CliCommand, OutputMode, ParsedArgs},
    output::emit_automation,
    runtime::{TextProgress, build_engine, operation_context, signal_cancellation},
    theme::{Theme, Tone},
};
use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::json;
use spaceprune::api::{AutomationDefinition, ResolvedConfig};

/// 执行 `spaceprune automation` 管理命令。
pub(crate) fn run_automation_command(
    parsed: ParsedArgs,
    config: ResolvedConfig,
    theme: Theme,
) -> Result<u8> {
    if parsed.action.is_some() || parsed.yes {
        bail!("{}", crate::i18n::text("commands.automation_arguments"));
    }
    let CliCommand::Automation(command) = parsed.command else {
        unreachable!("automation dispatch requires an automation command")
    };
    match command {
        AutomationCommand::List => {
            #[derive(Serialize)]
            struct Entry<'a> {
                id: &'a str,
                definition: &'a AutomationDefinition,
                installed: bool,
            }
            let entries: Vec<_> = config
                .automations
                .iter()
                .map(|(id, definition)| Entry {
                    id,
                    definition,
                    installed: spaceprune::api::automation_installed(id),
                })
                .collect();
            if parsed.output == OutputMode::Json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                println!(
                    "{}",
                    theme.stdout(crate::i18n::text("commands.no_automations"), Tone::Muted)
                );
            } else {
                for entry in entries {
                    let (status, tone) = if entry.installed {
                        (crate::i18n::text("commands.installed"), Tone::Success)
                    } else {
                        (crate::i18n::text("commands.not_installed"), Tone::Warning)
                    };
                    println!(
                        "{}",
                        crate::i18n::message(
                            "commands.daily",
                            &[
                                ("p0", (theme.stdout(status, tone)).to_string()),
                                ("p1", (theme.stdout(entry.id, Tone::Accent)).to_string()),
                                ("p2", (entry.definition.name).to_string()),
                                (
                                    "p3",
                                    (theme.stdout(entry.definition.at, Tone::Secondary))
                                        .to_string()
                                )
                            ]
                        )
                    );
                }
            }
            Ok(0)
        }
        AutomationCommand::Install(id) => {
            let runner = std::env::current_exe()?;
            let installation = spaceprune::api::install_automation(&config, &id, &runner)?;
            if parsed.output == OutputMode::Json {
                println!("{}", serde_json::to_string_pretty(&installation)?);
            } else {
                println!(
                    "{}",
                    theme.stdout(
                        crate::i18n::message(
                            "commands.automation_installed",
                            &[("id", (id).to_string())]
                        ),
                        Tone::Success
                    )
                );
            }
            Ok(0)
        }
        AutomationCommand::Uninstall(id) => {
            spaceprune::api::uninstall_automation(&id)?;
            if parsed.output == OutputMode::Json {
                println!("{}", json!({"uninstalled": id}));
            } else {
                println!(
                    "{}",
                    theme.stdout(
                        crate::i18n::message(
                            "commands.automation_uninstalled",
                            &[("id", (id).to_string())]
                        ),
                        Tone::Success
                    )
                );
            }
            Ok(0)
        }
        AutomationCommand::Run(id) => {
            let cancellation = signal_cancellation()?;
            let context = operation_context(&cancellation);
            let engine = build_engine(config, &context)?;
            let progress = TextProgress::new(
                !parsed.no_progress,
                theme,
                &cancellation,
                engine.event_bus(),
                engine.config(),
            );
            let report = engine.scan_automation_in_context(&id, &progress, &context)?;
            if report.cancelled {
                crate::output::emit_automation_scan_stop(&report, parsed.output, theme)?;
                return Ok(crate::output::stopped_exit_code(report.stop_reason));
            }
            let plan = report.plan_recommended()?;
            let execution = engine.execute_in_context(&plan, &progress, &context);
            let report = spaceprune::api::AutomationRunReport {
                scan: report,
                plan,
                execution,
            };
            emit_automation(&report, progress.cleanup_elapsed(), parsed.output, theme)?;
            Ok(if !report.execution.operation_failures.is_empty() {
                1
            } else if report.execution.cancelled {
                crate::output::stopped_exit_code(report.execution.stop_reason)
            } else {
                u8::from(report.status() != spaceprune::api::AutomationRunStatus::Completed)
            })
        }
    }
}
