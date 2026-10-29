//! Enumerate metadata under one native grant; each selected unit is an observed regular file.

use super::*;
use std::{
    collections::{BTreeSet, VecDeque},
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) fn scan(
    provider: &dyn DocumentProvider,
    task: &TaskRequest,
    progress: &dyn ProgressSink,
    context: &OperationContext,
) -> PluginResult<HandlerScanResult> {
    let rules = rules::CompiledRules::parse(task)?;
    let binding = binding(provider, &rules.rules.resource, context)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut report = HandlerScanResult::default();
    let mut measurement = ScanMeasurement::default();
    progress.record_scan(task, measurement);
    report.items.push(CleanupItem {
        id: "resource-root".into(),
        parent_id: None,
        name: binding.name.clone(),
        location: binding.name.clone(),
        logical_size_applicability: SizeApplicability::NotApplicable,
        logical_bytes: None,
        reclaimable_bytes: None,
        modified_at: None,
        recommendation: Recommendation::Unavailable,
        reason: spaceprune_plugin_api::messages::text("mobile_documents.root_scope").into(),
        tags: vec!["document-directory".into()],
        opaque: serde_json::Value::Null,
    });
    let mut queue = VecDeque::from([(
        binding.root_id.clone(),
        "resource-root".to_owned(),
        Vec::<Document>::new(),
    )]);
    let mut seen = BTreeSet::from([binding.root_id.clone()]);
    let mut count = 0;
    let mut bytes = 0usize;
    while let Some((parent, parent_item, ancestors)) = queue.pop_front() {
        let mut cursor = None;
        let mut cursors = BTreeSet::new();
        loop {
            if context.is_stopped() {
                return Ok(report);
            }
            let page = match provider.children(&binding, &parent, cursor.as_deref(), context) {
                Ok(page) => page,
                Err(error) => {
                    report.issues.push(HandlerIssue {
                        code: Some(error.code),
                        io_failure: error.io_failure,
                        level: IssueLevel::Warning,
                        message: error.message,
                        location: Some(location(&binding, &ancestors, None)),
                    });
                    break;
                }
            };
            if page.documents.len() > MAX_PAGE_ENTRIES {
                return Err(PluginError::new(
                    "mobile-documents-page-invalid",
                    spaceprune_plugin_api::messages::text("mobile_documents.page_large"),
                ));
            }
            for document in page.documents {
                if context.is_stopped() {
                    return Ok(report);
                }
                if document.id.is_empty()
                    || document.name.is_empty()
                    || document.parent_id != parent
                {
                    return Err(PluginError::new(
                        "mobile-documents-page-invalid",
                        spaceprune_plugin_api::messages::text("mobile_documents.page_invalid"),
                    ));
                }
                if !seen.insert(document.id.clone()) {
                    report.issues.push(HandlerIssue {
                        code: Some("mobile-documents-repeated-resource".into()),
                        io_failure: None,
                        level: IssueLevel::Warning,
                        message: spaceprune_plugin_api::messages::text("mobile_documents.repeated")
                            .into(),
                        location: Some(location(&binding, &ancestors, Some(&document.name))),
                    });
                    continue;
                }
                count += 1;
                emit(
                    progress,
                    task,
                    ProgressPhase::Scan,
                    count,
                    None,
                    spaceprune_plugin_api::messages::message(
                        "mobile_documents.inspecting",
                        &[("name", document.name.clone())],
                    ),
                );
                if rules.excluded(&document.name) {
                    continue;
                }
                let directory = document.kind == DocumentKind::Directory;
                if !directory && !rules.included(&document.name) {
                    continue;
                }
                let selectable =
                    document.kind == DocumentKind::File && binding.writable && document.deletable;
                let recommendation = if !selectable {
                    Recommendation::Unavailable
                } else if rules.recommended(&document, now) {
                    Recommendation::Recommended
                } else {
                    Recommendation::Optional
                };
                let reason = if directory {
                    spaceprune_plugin_api::messages::text("mobile_documents.grouping").to_owned()
                } else if document.kind == DocumentKind::Unsupported {
                    spaceprune_plugin_api::messages::text("mobile_documents.unsupported").to_owned()
                } else if !binding.writable || !document.deletable {
                    spaceprune_plugin_api::messages::text("mobile_documents.readonly").to_owned()
                } else {
                    rules.reason(&document, now)
                };
                let id = format!("document-{count}");
                let item = CleanupItem {
                    id: id.clone(),
                    parent_id: Some(parent_item.clone()),
                    name: document.name.clone(),
                    location: location(&binding, &ancestors, Some(&document.name)),
                    logical_size_applicability: if directory {
                        SizeApplicability::NotApplicable
                    } else {
                        SizeApplicability::Applicable
                    },
                    logical_bytes: if directory {
                        None
                    } else {
                        document.logical_bytes
                    },
                    // SAF metadata cannot prove how much local storage a provider deletion frees.
                    reclaimable_bytes: None,
                    modified_at: document.modified_at,
                    recommendation,
                    reason,
                    tags: vec![
                        if directory {
                            "document-directory"
                        } else {
                            "document-file"
                        }
                        .into(),
                    ],
                    opaque: if directory {
                        serde_json::Value::Null
                    } else {
                        serde_json::to_value(Selection {
                            binding: binding.clone(),
                            document: document.clone(),
                            ancestors: ancestors.clone(),
                        })
                        .expect("document selection")
                    },
                };
                bytes =
                    bytes.saturating_add(serde_json::to_vec(&item).expect("document item").len());
                if bytes > context.budget.output_bytes {
                    return Err(PluginError::new(
                        "mobile-documents-report-too-large",
                        spaceprune_plugin_api::messages::text("mobile_documents.report_large"),
                    ));
                }
                if !directory {
                    measurement.add_item(&item);
                    progress.record_scan(task, measurement);
                }
                report.items.push(item);
                if directory && rules.rules.recursive {
                    let mut next = ancestors.clone();
                    next.push(document.clone());
                    queue.push_back((document.id, id, next));
                }
            }
            let Some(next) = page.next else {
                break;
            };
            if next.is_empty() || !cursors.insert(next.clone()) {
                return Err(PluginError::new(
                    "mobile-documents-page-invalid",
                    spaceprune_plugin_api::messages::text("mobile_documents.pagination"),
                ));
            }
            cursor = Some(next);
        }
    }
    emit(
        progress,
        task,
        ProgressPhase::Complete,
        count,
        Some(count),
        spaceprune_plugin_api::messages::text("mobile_documents.complete").into(),
    );
    Ok(report)
}

fn location(binding: &ResourceBinding, ancestors: &[Document], leaf: Option<&str>) -> String {
    std::iter::once(binding.name.as_str())
        .chain(ancestors.iter().map(|entry| entry.name.as_str()))
        .chain(leaf)
        .collect::<Vec<_>>()
        .join(" › ")
}
