//! A readable cleanup receipt: confirmed outcomes, byte estimates, then volume observations.

use std::{fmt::Write, time::Duration};

use spaceprune::api::{ExecutionReport, OperationStopReason, fs::format_bytes};

use super::receipt::Receipt;
use crate::theme::{Theme, Tone};

/// Keep the shared authorization cause readable without dropping per-item JSON diagnostics.
pub(super) fn failure_details(execution: &ExecutionReport) -> Vec<String> {
    let unfinished = spaceprune::api::unfinished_cleanup_items(execution, None)
        .into_iter()
        .filter(|item| item.in_use)
        .map(|item| (item.module_id, item.task_id, item.item_id))
        .collect::<std::collections::BTreeSet<_>>();
    let mut lines = execution
        .operation_failures
        .iter()
        .map(|failure| format!("- {}", failure.message))
        .collect::<Vec<_>>();
    for group in &execution.groups {
        if let Some(error) = &group.error {
            lines.push(format!("- {}.{}: {error}", group.module_id, group.task_id));
        }
        if let Some(result) = &group.result {
            for failure in &result.failures {
                if unfinished.contains(&(
                    group.module_id.clone(),
                    group.task_id.clone(),
                    failure.item_id.clone(),
                )) {
                    continue;
                }
                // Known causes already have a counted, actionable summary with their size.
                if unfinished::reason(failure) == "terminal.reason_other" {
                    lines.push(format!("- {}: {}", failure.item_id, failure.message));
                }
            }
        }
    }
    lines
}

pub(super) fn summary(
    execution: &ExecutionReport,
    elapsed: Option<Duration>,
    theme: Theme,
) -> String {
    let (title, tone) = status(execution);
    let receipt = Receipt::stdout(theme);
    let mut text = receipt.heading(title, tone, elapsed);
    let attempted = execution
        .groups
        .iter()
        .filter_map(|group| group.result.as_ref())
        .map(|result| result.attempted)
        .sum::<usize>();
    let mut row = |label: &str, value: String, tone| {
        let _ = writeln!(text, "{}", receipt.row(label, &value, tone));
    };
    row(
        crate::i18n::text("terminal.succeeded"),
        crate::i18n::message(
            "terminal.attempted_count",
            &[
                ("p0", (execution.succeeded()).to_string()),
                ("attempted", (attempted).to_string()),
            ],
        ),
        Tone::Success,
    );
    let busy = spaceprune::api::unfinished_cleanup_items(execution, None)
        .iter()
        .filter(|item| item.in_use)
        .count();
    if busy > 0 {
        row(
            crate::i18n::text("terminal.in_use_skipped"),
            count(busy, "item"),
            Tone::Warning,
        );
    }
    let failures = execution
        .failure_count()
        .saturating_sub(execution.operation_failures.len() + busy);
    row(
        crate::i18n::text("terminal.failures"),
        failures.to_string(),
        if failures > 0 {
            Tone::Error
        } else {
            Tone::Muted
        },
    );
    if !execution.operation_failures.is_empty() {
        row(
            crate::i18n::text("terminal.service_issues"),
            execution.operation_failures.len().to_string(),
            Tone::Error,
        );
    }

    let summary = execution.size_summary();
    let known = summary.reclaimed_bytes;
    let unknown = summary.unknown_size_item_count;
    // A snapshot-only receipt must not imply an unmeasured deletion freed zero bytes.
    let has_content = execution.groups.iter().any(|group| {
        group.size_accounting == spaceprune::api::CleanupSizeAccounting::ProcessedBytes
            && group.result.is_some()
    });
    if has_content || summary.snapshot_group_count > 0 || !summary.snapshot_space_changes.is_empty()
    {
        text.push_str(&receipt.rule());
    }
    if has_content {
        let size = if known == 0 && unknown > 0 {
            crate::i18n::text("terminal.unknown").to_owned()
        } else if unknown > 0 {
            crate::i18n::message(
                "terminal.at_least",
                &[("p0", (format_bytes(known)).to_string())],
            )
        } else {
            format_bytes(known)
        };
        let _ = writeln!(
            text,
            "{}",
            receipt.row(
                crate::i18n::text("terminal.processed_size"),
                &size,
                Tone::Accent
            )
        );
        if unknown > 0 {
            let _ = writeln!(
                text,
                "{}",
                receipt.row(
                    crate::i18n::text("terminal.unknown_size"),
                    &count(unknown, "item"),
                    Tone::Warning
                )
            );
        }
        let _ = writeln!(
            text,
            "  {}",
            theme.stdout(crate::i18n::text("terminal.content_estimate"), Tone::Muted)
        );
    }
    let snapshots = summary.snapshot_group_count;
    if snapshots > 0 || !summary.snapshot_space_changes.is_empty() {
        text.push('\n');
        let absent = summary.snapshot_already_absent_group_count;
        let groups = if absent > 0 {
            crate::i18n::message(
                "terminal.snapshot_absent",
                &[
                    (
                        "p0",
                        (count(snapshots.saturating_sub(absent), "group")).to_string(),
                    ),
                    ("absent", (absent).to_string()),
                ],
            )
        } else {
            crate::i18n::message(
                "terminal.snapshot_processed",
                &[("p0", (count(snapshots, "group")).to_string())],
            )
        };
        let _ = writeln!(
            text,
            "{}",
            receipt.row(
                crate::i18n::text("terminal.snapshots"),
                &groups,
                Tone::Secondary
            )
        );
        if summary.snapshot_space_changes.is_empty() {
            let (value, tone) = if snapshots > 0 && snapshots == absent {
                (crate::i18n::text("terminal.no_extra_freed"), Tone::Muted)
            } else {
                (crate::i18n::text("terminal.unavailable"), Tone::Warning)
            };
            let _ = writeln!(
                text,
                "{}",
                receipt.row(crate::i18n::text("terminal.free_space"), value, tone)
            );
        } else {
            let _ = writeln!(
                text,
                "  {}",
                theme.stdout(crate::i18n::text("terminal.volume_change"), Tone::Secondary)
            );
            for change in &summary.snapshot_space_changes {
                let (value, tone) = match (change.free_bytes_before, change.free_bytes_after) {
                    (Some(before), Some(after)) if after >= before => {
                        (format!("+{}", format_bytes(after - before)), Tone::Accent)
                    }
                    (Some(before), Some(after)) => {
                        (format!("-{}", format_bytes(before - after)), Tone::Warning)
                    }
                    _ => (
                        crate::i18n::text("terminal.unavailable").to_owned(),
                        Tone::Warning,
                    ),
                };
                let _ = writeln!(
                    text,
                    "{}",
                    receipt.row(&format!("  {}", change.scope), &value, tone)
                );
                if let Some(error) = &change.error {
                    let _ = writeln!(text, "      {}", theme.stdout(error, Tone::Warning));
                }
            }
            let _ = writeln!(
                text,
                "  {}",
                theme.stdout(crate::i18n::text("terminal.net_change"), Tone::Muted)
            );
        }
    }
    text
}

fn count(value: usize, noun: &str) -> String {
    if crate::i18n::is_chinese() {
        crate::i18n::message(
            if noun == "group" {
                "terminal.group_count"
            } else {
                "terminal.item_count"
            },
            &[("count", value.to_string())],
        )
    } else {
        format!("{value} {noun}{}", if value == 1 { "" } else { "s" })
    }
}

fn status(execution: &ExecutionReport) -> (&'static str, Tone) {
    if execution
        .operation_failures
        .iter()
        .any(|failure| failure.code == "cleanup-stop-unconfirmed")
    {
        (
            crate::i18n::text("terminal.cleanup_stop_unconfirmed"),
            Tone::Error,
        )
    } else if !execution.operation_failures.is_empty() {
        (crate::i18n::text("terminal.cleanup_errors"), Tone::Error)
    } else if execution.stop_reason == Some(OperationStopReason::DeadlineExceeded) {
        (crate::i18n::text("terminal.cleanup_timeout"), Tone::Error)
    } else if execution.cancelled {
        (
            crate::i18n::text("terminal.cleanup_cancelled"),
            Tone::Warning,
        )
    } else if execution.has_errors()
        && (execution.groups.iter().any(|group| group.error.is_some())
            || execution.failure_count()
                > spaceprune::api::unfinished_cleanup_items(execution, None)
                    .iter()
                    .filter(|item| item.in_use)
                    .count())
    {
        (crate::i18n::text("terminal.cleanup_errors"), Tone::Error)
    } else if execution.has_errors() {
        (crate::i18n::text("terminal.cleanup_in_use"), Tone::Warning)
    } else {
        (
            crate::i18n::text("terminal.cleanup_complete"),
            Tone::Success,
        )
    }
}

pub(super) fn unfinished_details(
    execution: &ExecutionReport,
    plan: &spaceprune::api::CleanupPlan,
    theme: Theme,
) -> String {
    unfinished::render(execution, plan, theme)
}

mod unfinished;

#[cfg(test)]
mod tests;
