//! 人类可读和稳定 JSON 输出。

use std::time::Duration;

use anyhow::Result;

mod cleanup;
pub(crate) mod receipt;
use serde_json::json;
use spaceprune::api::{
    CleanupPlan, ExecutionReport, IssueLevel, OperationStopReason, Recommendation, ScanReport,
    fs::format_bytes,
};

use crate::{
    cli::OutputMode,
    theme::{Theme, Tone},
};

/// A deadline is an operation failure; only explicit cancellation uses the signal exit code.
pub(crate) fn stopped_exit_code(reason: Option<OperationStopReason>) -> u8 {
    match reason {
        Some(OperationStopReason::DeadlineExceeded) => 1,
        _ => crate::EXIT_CANCELLED,
    }
}

/// 输出一次只读扫描。
pub(crate) fn emit_scan(report: &ScanReport, output: OutputMode, theme: Theme) -> Result<()> {
    match output {
        OutputMode::Human => print_scan(report, theme),
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(report)?),
    }
    Ok(())
}

/// 输出完整人类可读扫描树。
pub(crate) fn print_scan(report: &ScanReport, theme: Theme) {
    println!(
        "{} {} {}",
        theme.stdout(crate::i18n::text("terminal.configuration"), Tone::Accent),
        report.config_name,
        theme.stdout(format!("({})", report.config_source), Tone::Muted)
    );
    if report.cancelled {
        println!(
            "{}",
            theme.stdout(
                if report.stop_reason == Some(OperationStopReason::DeadlineExceeded) {
                    crate::i18n::text("terminal.scan_timed_out_partial")
                } else {
                    crate::i18n::text("terminal.scan_cancelled_partial")
                },
                Tone::Warning
            )
        );
    }
    if report.modules.is_empty() {
        println!(
            "{}",
            theme.stdout(crate::i18n::text("terminal.no_modules"), Tone::Muted)
        );
        return;
    }
    for module in &report.modules {
        println!(
            "{}",
            crate::i18n::message(
                "terminal.module_usage",
                &[
                    ("p0", (theme.stdout(&module.name, Tone::Accent)).to_string()),
                    (
                        "p1",
                        (theme.stdout(&module.id, Tone::Secondary)).to_string()
                    ),
                    (
                        "p2",
                        (display_logical_summary(module.logical_size_summary())).to_string()
                    ),
                    (
                        "p3",
                        (display_total(
                            module.recommended_bytes(),
                            module.unknown_recommended_items()
                        ))
                        .to_string()
                    )
                ]
            )
        );
        for task in &module.tasks {
            println!(
                "{}",
                crate::i18n::message(
                    "terminal.task_usage",
                    &[
                        (
                            "p0",
                            (theme.stdout(&task.name, Tone::Secondary)).to_string()
                        ),
                        (
                            "p1",
                            (display_logical_summary(task.logical_size_summary())).to_string()
                        ),
                        (
                            "p2",
                            (display_total(
                                task.recommended_bytes(),
                                task.unknown_recommended_items()
                            ))
                            .to_string()
                        )
                    ]
                )
            );
            if let Some(error) = &task.error {
                println!(
                    "    {} {error}",
                    theme.stdout(crate::i18n::text("terminal.error_tag"), Tone::Error)
                );
            }
            for issue in &task.issues {
                let (level, tone) = match issue.level {
                    IssueLevel::Warning => (crate::i18n::text("terminal.warning"), Tone::Warning),
                    IssueLevel::Error => (crate::i18n::text("terminal.error"), Tone::Error),
                };
                println!(
                    "    {} {}",
                    theme.stdout(format!("[{level}]"), tone),
                    issue.message
                );
            }
            for item in task.leaf_items() {
                let (marker, tone) = match item.recommendation {
                    Recommendation::Recommended => {
                        (crate::i18n::text("terminal.recommended"), Tone::Success)
                    }
                    Recommendation::Optional => {
                        (crate::i18n::text("terminal.optional"), Tone::Warning)
                    }
                    Recommendation::Unavailable => {
                        (crate::i18n::text("terminal.unavailable"), Tone::Muted)
                    }
                };
                println!(
                    "    {} {}  {}  {}",
                    theme.stdout(format!("[{marker}]"), tone),
                    if item.logical_size_applicability
                        == spaceprune::api::SizeApplicability::NotApplicable
                    {
                        crate::i18n::text("terminal.not_applicable").to_owned()
                    } else {
                        display_size(item.logical_bytes)
                    },
                    item.name,
                    theme.stdout(&item.location, Tone::Muted)
                );
                println!("             {}", theme.stdout(&item.reason, Tone::Muted));
            }
        }
    }
}

/// 向 stdout 输出清理计划摘要。
pub(crate) fn print_plan_summary(plan: &CleanupPlan, theme: Theme) {
    print_plan_summary_to(plan, false, theme);
}

/// 输出清理计划摘要；交互界面可选择 stderr，保持 JSON stdout 纯净。
pub(crate) fn print_plan_summary_to(plan: &CleanupPlan, stderr: bool, theme: Theme) {
    let unknown = plan
        .groups()
        .iter()
        .flat_map(|group| &group.items)
        .filter(|item| item.reclaimable_bytes.is_none())
        .count();
    let summary = crate::i18n::message(
        "terminal.plan_summary",
        &[
            ("p0", (plan.groups().len()).to_string()),
            ("p1", (plan.item_count()).to_string()),
            ("p2", (format_bytes(plan.reclaimable_bytes())).to_string()),
            ("p3", (unknown).to_string()),
        ],
    );
    if stderr {
        eprintln!("{}", theme.stderr(summary, Tone::Accent));
    } else {
        println!("\n{}", theme.stdout(summary, Tone::Accent));
    }
}

/// Report an interrupted automation scan without constructing a cleanup plan.
pub(crate) fn emit_automation_scan_stop(
    report: &ScanReport,
    output: OutputMode,
    theme: Theme,
) -> Result<()> {
    if output == OutputMode::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "status": if report.stop_reason == Some(OperationStopReason::DeadlineExceeded) { spaceprune::api::AutomationRunStatus::Failed } else { spaceprune::api::AutomationRunStatus::Cancelled },
                "scan": report, "plan": null, "execution": null, "cleanup_size_summary": null, "cancelled": true,
            }))?
        );
        return Ok(());
    }
    emit_scan(report, output, theme)
}

/// Report automation coverage alongside its cleanup effects.
pub(crate) fn emit_automation(
    report: &spaceprune::api::AutomationRunReport,
    elapsed: Option<Duration>,
    output: OutputMode,
    theme: Theme,
) -> Result<()> {
    use spaceprune::api::AutomationRunStatus;
    if output == OutputMode::Json {
        let mut value = serde_json::to_value(report)?;
        value["cleanup_size_summary"] = serde_json::to_value(report.execution.size_summary())?;
        value["cancelled"] = json!(report.scan.cancelled || report.execution.cancelled);
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    emit_execution(
        &report.scan,
        &report.plan,
        &report.execution,
        elapsed,
        output,
        theme,
    )?;
    let (message, tone) = match report.status() {
        AutomationRunStatus::Completed => (
            crate::i18n::text("terminal.automation_complete"),
            Tone::Success,
        ),
        AutomationRunStatus::NeedsAuthorization => (
            crate::i18n::text("terminal.automation_authorization"),
            Tone::Warning,
        ),
        AutomationRunStatus::CompletedWithIssues => (
            crate::i18n::text("terminal.automation_issues"),
            Tone::Warning,
        ),
        AutomationRunStatus::Failed => {
            (crate::i18n::text("terminal.automation_failed"), Tone::Error)
        }
        AutomationRunStatus::Cancelled => (
            crate::i18n::text("terminal.automation_cancelled"),
            Tone::Warning,
        ),
    };
    println!("{}", theme.stdout(message, tone));
    for task in report.scan.modules.iter().flat_map(|module| &module.tasks) {
        for message in task
            .error
            .iter()
            .map(String::as_str)
            .chain(task.issues.iter().map(|issue| issue.message.as_str()))
        {
            eprintln!(
                "{}",
                theme.stderr(
                    format!("- {}.{}: {message}", task.module_id, task.task_id),
                    Tone::Warning
                )
            );
        }
    }
    Ok(())
}

/// Output actual cleanup effects independently of automation scan coverage.
pub(crate) fn emit_execution(
    report: &ScanReport,
    plan: &CleanupPlan,
    execution: &ExecutionReport,
    elapsed: Option<Duration>,
    output: OutputMode,
    theme: Theme,
) -> Result<()> {
    match output {
        OutputMode::Json => print_json_run(report, plan, Some(execution), false)?,
        OutputMode::Human => {
            println!("{}", cleanup::summary(execution, elapsed, theme));
            for diagnostic in cleanup::failure_details(execution) {
                eprintln!("{}", theme.stderr(diagnostic, Tone::Error));
            }
        }
    }
    Ok(())
}

/// 输出包含扫描、计划、执行和取消状态的一份 JSON 文档。
pub(crate) fn print_json_run(
    report: &ScanReport,
    plan: &CleanupPlan,
    execution: Option<&ExecutionReport>,
    cancelled: bool,
) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "scan": report,
            "plan": plan,
            "execution": execution,
            "cleanup_size_summary": execution.map(ExecutionReport::size_summary),
            "cancelled": cancelled || report.cancelled || execution.is_some_and(|value| value.cancelled),
        }))?
    );
    Ok(())
}

/// 显示一个可为空的容量。
pub(crate) fn display_size(value: Option<u64>) -> String {
    value
        .map(format_bytes)
        .unwrap_or_else(|| crate::i18n::text("terminal.unknown").to_owned())
}

fn display_logical_summary(summary: spaceprune::api::LogicalSizeSummary) -> String {
    if summary.applicability() == spaceprune::api::SizeApplicability::NotApplicable {
        crate::i18n::text("terminal.not_applicable").to_owned()
    } else {
        display_total(summary.known_bytes, summary.unknown_items)
    }
}

fn display_total(known_bytes: u64, unknown_items: usize) -> String {
    if unknown_items == 0 {
        format_bytes(known_bytes)
    } else {
        crate::i18n::message(
            "terminal.total_unknown",
            &[
                ("p0", (format_bytes(known_bytes)).to_string()),
                ("unknown_items", (unknown_items).to_string()),
            ],
        )
    }
}
