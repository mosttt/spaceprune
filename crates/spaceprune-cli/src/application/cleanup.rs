//! One scan owns selection, confirmation and the single cleanup execution.

use crate::{
    EXIT_CANCELLED,
    cli::{Action, CliCommand, OutputMode, ParsedArgs},
    output::{self, emit_execution, emit_scan, print_json_run, print_plan_summary, print_scan},
    runtime::{TextProgress, build_engine, operation_context, signal_cancellation},
    selection::{confirm, select_plan},
    theme::{Theme, Tone},
};
use anyhow::{Result, bail};
use spaceprune::api::{
    CancellationToken, CleanupPlan, ResolvedConfig, ScanReport, ScanRequest, SpacePrune,
};
use std::io::{self, IsTerminal};

pub(super) fn run(parsed: ParsedArgs, config: ResolvedConfig, theme: Theme) -> Result<u8> {
    let ParsedArgs {
        action,
        yes,
        output,
        no_progress,
        command,
        ..
    } = parsed;
    let CliCommand::Run {
        module,
        selectors,
        tasks,
    } = command
    else {
        unreachable!("management commands are dispatched before cleanup")
    };
    let action = action.unwrap_or(Action::Select);
    if yes && action != Action::Clean {
        bail!("{}", crate::i18n::text("terminal.yes_clean_only"));
    }
    if action == Action::Select && (!io::stdin().is_terminal() || !io::stderr().is_terminal()) {
        bail!("{}", crate::i18n::text("terminal.selection_tty"));
    }
    let cancellation = signal_cancellation()?;
    let context = operation_context(&cancellation);
    let engine = build_engine(config, &context)?;
    let progress = TextProgress::new(
        !no_progress,
        theme,
        &cancellation,
        engine.event_bus(),
        engine.config(),
    );
    let report = engine.scan_in_context(
        &ScanRequest {
            module,
            selectors,
            tasks,
        },
        &progress,
        &context,
    )?;
    if report.cancelled {
        emit_scan(&report, output, theme)?;
        return Ok(output::stopped_exit_code(report.stop_reason));
    }
    match action {
        Action::Scan => {
            emit_scan(&report, output, theme)?;
            Ok(u8::from(report.has_errors()))
        }
        Action::Clean => run_recommended(
            &engine,
            report,
            yes,
            output,
            &progress,
            &cancellation,
            theme,
        ),
        Action::Select => run_select(&engine, report, output, &progress, &cancellation, theme),
    }
}

fn run_recommended(
    engine: &SpacePrune,
    report: ScanReport,
    yes: bool,
    output: OutputMode,
    progress: &TextProgress,
    cancellation: &CancellationToken,
    theme: Theme,
) -> Result<u8> {
    let plan = report.plan_recommended()?;
    if output == OutputMode::Human {
        print_scan(&report, theme);
        print_plan_summary(&plan, theme);
    }
    if plan.is_empty() {
        if output == OutputMode::Json {
            print_json_run(&report, &plan, None, false)?;
        } else {
            println!(
                "{}",
                theme.stdout(
                    crate::i18n::text("terminal.no_recommendations"),
                    Tone::Muted
                )
            );
        }
        return Ok(u8::from(report.has_errors()));
    }
    if !yes {
        if !io::stdin().is_terminal() {
            bail!("{}", crate::i18n::text("terminal.clean_yes_tty"));
        }
        if !confirm(
            crate::i18n::text("terminal.confirm_recommended"),
            cancellation,
            theme,
        )? {
            if output == OutputMode::Json {
                print_json_run(&report, &plan, None, true)?;
            } else {
                println!(
                    "{}",
                    theme.stdout(crate::i18n::text("terminal.nothing_cleaned"), Tone::Warning)
                );
            }
            return Ok(if cancellation.is_cancelled() {
                EXIT_CANCELLED
            } else {
                0
            });
        }
    }
    execute_plan(
        engine,
        &report,
        &plan,
        output,
        progress,
        cancellation,
        theme,
    )
}

fn run_select(
    engine: &SpacePrune,
    report: ScanReport,
    output: OutputMode,
    progress: &TextProgress,
    cancellation: &CancellationToken,
    theme: Theme,
) -> Result<u8> {
    let Some(plan) = select_plan(&report, cancellation, theme, &engine.handler_descriptors())?
    else {
        if output == OutputMode::Json {
            let empty = report.plan_selected(&Default::default())?;
            print_json_run(&report, &empty, None, true)?;
        } else {
            println!(
                "{}",
                theme.stdout(crate::i18n::text("terminal.nothing_cleaned"), Tone::Warning)
            );
        }
        return Ok(if cancellation.is_cancelled() {
            EXIT_CANCELLED
        } else {
            0
        });
    };
    if plan.is_empty() {
        if output == OutputMode::Json {
            print_json_run(&report, &plan, None, false)?;
        } else {
            println!(
                "{}",
                theme.stdout(crate::i18n::text("terminal.no_selection"), Tone::Muted)
            );
        }
        return Ok(u8::from(report.has_errors()));
    }
    execute_plan(
        engine,
        &report,
        &plan,
        output,
        progress,
        cancellation,
        theme,
    )
}

fn execute_plan(
    engine: &SpacePrune,
    report: &ScanReport,
    plan: &CleanupPlan,
    output: OutputMode,
    progress: &TextProgress,
    cancellation: &CancellationToken,
    theme: Theme,
) -> Result<u8> {
    let execution = engine.execute_in_context(plan, progress, &operation_context(cancellation));
    emit_execution(
        report,
        plan,
        &execution,
        progress.cleanup_elapsed(),
        output,
        theme,
    )?;
    Ok(if !execution.operation_failures.is_empty() {
        1
    } else if execution.cancelled {
        output::stopped_exit_code(execution.stop_reason)
    } else {
        u8::from(report.has_errors() || execution.has_errors())
    })
}
