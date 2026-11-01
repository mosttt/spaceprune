//! Explain each unfinished category with its own estimate, without counting it as cleaned.
use super::*;
use spaceprune::api::{IoFailureKind, ItemFailure, PermissionOutcome, UnfinishedCleanupItem};
use std::collections::BTreeMap;

pub(super) fn reason(failure: &ItemFailure) -> &'static str {
    match failure.authorization {
        Some(PermissionOutcome::Denied | PermissionOutcome::Cancelled) => {
            "terminal.reason_authorization_denied"
        }
        Some(PermissionOutcome::TimedOut) => "terminal.reason_authorization_timeout",
        Some(
            PermissionOutcome::HelperMissing
            | PermissionOutcome::HelperOutdated
            | PermissionOutcome::HelperUntrusted
            | PermissionOutcome::AuthorizationUnavailable,
        ) => "terminal.reason_authorization_unavailable",
        Some(PermissionOutcome::Granted) | None => {
            match failure.io_failure.as_ref().map(|io| io.kind) {
                Some(IoFailureKind::AccessDenied) => "terminal.reason_access_denied",
                Some(IoFailureKind::ReadOnlyVolume) => "terminal.reason_read_only",
                _ => "terminal.reason_other",
            }
        }
        _ => "terminal.reason_other",
    }
}

pub(super) fn render(
    execution: &ExecutionReport,
    plan: &spaceprune::api::CleanupPlanData,
    theme: Theme,
) -> String {
    let items = spaceprune::api::unfinished_cleanup_items(execution, Some(plan));
    let failures: BTreeMap<_, _> = execution
        .groups
        .iter()
        .flat_map(|group| {
            group.result.iter().flat_map(move |result| {
                result.failures.iter().map(move |failure| {
                    (
                        (
                            group.module_id.as_str(),
                            group.task_id.as_str(),
                            failure.item_id.as_str(),
                        ),
                        failure,
                    )
                })
            })
        })
        .collect();
    let mut groups: BTreeMap<&str, Vec<&UnfinishedCleanupItem>> = BTreeMap::new();
    for item in &items {
        let key = if item.in_use {
            "terminal.reason_in_use"
        } else {
            failures
                .get(&(
                    item.module_id.as_str(),
                    item.task_id.as_str(),
                    item.item_id.as_str(),
                ))
                .map_or("terminal.reason_other", |failure| reason(failure))
        };
        groups.entry(key).or_default().push(item);
    }
    let mut text = String::new();
    for (reason, group) in groups {
        let known = group
            .iter()
            .filter_map(|item| item.estimated_bytes)
            .fold(0u64, u64::saturating_add);
        let unknown = group
            .iter()
            .filter(|item| item.estimated_bytes.is_none())
            .count();
        let size = crate::i18n::message(
            if unknown == 0 {
                "terminal.uncleaned_approx"
            } else if known > 0 {
                "terminal.uncleaned_at_least"
            } else {
                "terminal.uncleaned_unknown"
            },
            &[
                ("size", format_bytes(known)),
                ("count", unknown.to_string()),
            ],
        );
        let _ = writeln!(
            text,
            "{} · {} · {}{}{}",
            crate::i18n::text(reason),
            count(group.len(), "item"),
            size,
            if crate::i18n::is_chinese() {
                "；"
            } else {
                "; "
            },
            crate::i18n::text(&format!("{reason}_help"))
        );
    }
    for item in items.iter().filter(|item| item.in_use) {
        let path = item
            .file_usage
            .as_ref()
            .map_or(item.item_id.as_str(), |usage| usage.path.as_str());
        let clean = |text: &str| text.chars().filter(|c| !c.is_control()).collect::<String>();
        let _ = writeln!(text, "  {}", clean(path));
        if let Some(usage) = &item.file_usage
            && !usage.processes.is_empty()
        {
            for process in &usage.processes {
                let _ = writeln!(text, "    {} (PID {})", clean(&process.name), process.pid);
            }
        } else {
            let _ = writeln!(text, "    {}", crate::i18n::text("terminal.usage_unknown"));
        }
    }
    if !items.is_empty() {
        text.push_str(crate::i18n::text("terminal.receipt_json_details"));
    }
    theme.stdout(text, Tone::Muted)
}
