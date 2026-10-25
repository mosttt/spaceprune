//! Cleanup tree rows, filtering, and expansion state.

use super::*;

impl<'a> SelectionApp<'a> {
    pub(super) fn tree_prefix(&self, index: usize) -> String {
        let row = &self.rows[index];
        let Some(parent) = row.parent else {
            return String::new();
        };
        let last_child = |index: usize| {
            let row = &self.rows[index];
            let next = self.visible.partition_point(|next| *next < row.subtree_end);
            self.visible
                .get(next)
                .is_none_or(|next| *next >= self.rows[row.parent.unwrap()].subtree_end)
        };
        let branch = if last_child(index) {
            "└─ "
        } else {
            "├─ "
        };
        if matches!(row.kind, RowKind::Item(_)) {
            format!(
                "  {}{branch}",
                if last_child(parent) { "   " } else { "│  " }
            )
        } else {
            format!("  {branch}")
        }
    }

    pub(super) fn build_rows(
        &self,
        accounting: &BTreeMap<String, CleanupSizeAccounting>,
    ) -> Vec<ViewRow<'a>> {
        let mut rows = Vec::new();
        for module in &self.report.modules {
            let module_index = rows.len();
            let logical_size = module.logical_size_summary();
            rows.push(ViewRow {
                kind: RowKind::Module(module.id.clone()),
                parent: None,
                subtree_end: 0,
                label: module.name.clone(),
                logical_bytes: Some(logical_size.known_bytes),
                logical_size_applicability: logical_size.applicability(),
                unknown_logical_items: logical_size.unknown_items,
                selected: 0,
                selected_logical_bytes: 0,
                selected_unknown_logical: 0,
                selected_not_applicable_logical: 0,
                selectable: 0,
                item: None,
                item_accounting: None,
                locations: Vec::new(),
                description: module.description.clone(),
                diagnostics: module.tasks.iter().flat_map(task_diagnostics).collect(),
                search_text: String::new(),
            });
            for project in projects(module) {
                let task_index = rows.len();
                let logical_size =
                    LogicalSizeSummary::from_items(project.items.iter().map(|(_, item)| *item));
                rows.push(ViewRow {
                    kind: RowKind::Task {
                        module_id: module.id.clone(),
                        task_id: project.id.clone(),
                    },
                    parent: Some(module_index),
                    subtree_end: 0,
                    label: project.name.clone(),
                    logical_bytes: Some(logical_size.known_bytes),
                    logical_size_applicability: logical_size.applicability(),
                    unknown_logical_items: logical_size.unknown_items,
                    selected: 0,
                    selected_logical_bytes: 0,
                    selected_unknown_logical: 0,
                    selected_not_applicable_logical: 0,
                    selectable: 0,
                    item: None,
                    item_accounting: None,
                    locations: project.locations,
                    description: project.description,
                    diagnostics: project.diagnostics.clone(),
                    search_text: format!("{}\n{}", project.name, project.diagnostics.join("\n"))
                        .to_lowercase(),
                });
                for (task, item) in project.items {
                    let key = ItemKey {
                        module_id: module.id.clone(),
                        task_id: task.task_id.clone(),
                        item_id: item.id.clone(),
                    };
                    let selectable =
                        usize::from(item.recommendation != Recommendation::Unavailable);
                    rows[module_index].selectable += selectable;
                    rows[task_index].selectable += selectable;
                    rows.push(ViewRow {
                        kind: RowKind::Item(key),
                        parent: Some(task_index),
                        subtree_end: rows.len() + 1,
                        label: item.name.clone(),
                        logical_bytes: item.logical_bytes,
                        logical_size_applicability: item.logical_size_applicability,
                        unknown_logical_items: usize::from(item.has_unknown_logical_size()),
                        selected: 0,
                        selected_logical_bytes: 0,
                        selected_unknown_logical: 0,
                        selected_not_applicable_logical: 0,
                        selectable,
                        item: Some(item),
                        item_accounting: Some(
                            *accounting
                                .get(&task.handler_id)
                                .expect("Scanned handlers retain registered descriptors"),
                        ),
                        locations: Vec::new(),
                        description: String::new(),
                        diagnostics: Vec::new(),
                        search_text: [
                            module.id.as_str(),
                            module.name.as_str(),
                            task.task_id.as_str(),
                            task.name.as_str(),
                            item.name.as_str(),
                            item.location.as_str(),
                            item.reason.as_str(),
                        ]
                        .join("\n")
                        .to_lowercase(),
                    });
                }
                rows[task_index].subtree_end = rows.len();
            }
            rows[module_index].subtree_end = rows.len();
        }
        rows
    }

    pub(super) fn row_collapsed(&self, row: &ViewRow<'_>) -> bool {
        match &row.kind {
            RowKind::Module(id) => self.collapsed_modules.contains(id),
            RowKind::Task { module_id, task_id } => self
                .collapsed_tasks
                .contains(&(module_id.clone(), task_id.clone())),
            RowKind::Item(_) => false,
        }
    }

    pub(super) fn refresh_visible_rows(&mut self) {
        let query = self.search.to_lowercase();
        let searching = !query.is_empty();
        let mut matching = vec![!searching; self.rows.len()];
        if searching {
            for index in (0..self.rows.len()).rev() {
                let row = &self.rows[index];
                if !row.search_text.is_empty() {
                    matching[index] |= row.search_text.contains(&query);
                }
                if matching[index]
                    && let Some(parent) = row.parent
                {
                    matching[parent] = true;
                }
            }
        }
        let mut expanded = vec![true; self.rows.len()];
        self.visible.clear();
        for (index, row) in self.rows.iter().enumerate() {
            let parent_expanded = row.parent.is_none_or(|parent| expanded[parent]);
            expanded[index] = parent_expanded
                && ((searching && self.reveal_search_matches) || !self.row_collapsed(row));
            if parent_expanded && matching[index] {
                self.visible.push(index);
            }
        }
        self.cursor = self.cursor.min(self.visible.len().saturating_sub(1));
        self.view_dirty = false;
    }
}

struct Project<'a> {
    id: String,
    name: String,
    description: String,
    locations: Vec<String>,
    diagnostics: Vec<String>,
    items: Vec<(&'a spaceprune::api::TaskReport, &'a CleanupItem)>,
}

fn projects(module: &spaceprune::api::ModuleReport) -> Vec<Project<'_>> {
    let mut projects: Vec<Project<'_>> = Vec::new();
    for task in &module.tasks {
        if task.items.is_empty() && task.error.is_none() && task.issues.is_empty() {
            continue;
        }
        let mut configs: BTreeSet<String> = if task.handler_id == "builtin.snapper/snapshots" {
            task.items
                .iter()
                .filter_map(|item| item.opaque.get("config")?.as_str().map(str::to_owned))
                .collect()
        } else {
            BTreeSet::new()
        };
        if task.handler_id == "builtin.snapper/snapshots" {
            configs.extend(
                task.request
                    .options
                    .get("configs")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned),
            );
        }
        let groups: Vec<(String, String, Option<String>)> = if configs.is_empty() {
            vec![(task.task_id.clone(), task.name.clone(), None)]
        } else {
            configs
                .into_iter()
                .map(|config| {
                    (
                        format!("snapper/{config}"),
                        format!("Snapper · {config}"),
                        Some(config),
                    )
                })
                .collect()
        };
        for (id, name, config) in groups {
            let index = projects
                .iter()
                .position(|project| project.id == id)
                .unwrap_or_else(|| {
                    projects.push(Project {
                        id,
                        name,
                        description: task.handler_id.clone(),
                        locations: filesystem_task_locations(task),
                        diagnostics: Vec::new(),
                        items: Vec::new(),
                    });
                    projects.len() - 1
                });
            let project = &mut projects[index];
            for diagnostic in task_diagnostics(task) {
                if !project.diagnostics.contains(&diagnostic) {
                    project.diagnostics.push(diagnostic);
                }
            }
            project.items.extend(
                task.leaf_items()
                    .into_iter()
                    .filter(|item| {
                        config.as_deref().is_none_or(|config| {
                            item.opaque
                                .get("config")
                                .and_then(serde_json::Value::as_str)
                                == Some(config)
                        })
                    })
                    .map(|item| (task, item)),
            );
        }
    }
    projects
}
