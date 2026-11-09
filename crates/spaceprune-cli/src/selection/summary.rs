//! Selected-size accounting and direct navigation to unmeasured content.

use super::*;

impl SelectionApp<'_> {
    pub(super) fn selected_summary_spans(&self) -> Vec<Span<'static>> {
        let (known, unknown) = self.selected_estimate();
        let mut spans = if self.selected.len() > self.snapshot_groups + self.memory_processes
            || self.selected.is_empty()
        {
            estimate_spans(known, unknown, self.theme)
        } else {
            Vec::new()
        };
        if self.snapshot_groups > 0 {
            if !spans.is_empty() {
                spans.push(Span::styled(
                    "  ·  ",
                    Style::new().fg(self.theme.palette.muted.tui()),
                ));
            }
            spans.push(Span::styled(
                crate::i18n::message(
                    "terminal.selection_snapshots",
                    &[("count", self.snapshot_groups.to_string())],
                ),
                Style::new().fg(self.theme.module_color().tui()),
            ));
        }
        if self.memory_processes > 0 {
            if !spans.is_empty() {
                spans.push(Span::raw("  ·  "));
            }
            spans.push(Span::styled(
                crate::i18n::message(
                    "terminal.selection_memory",
                    &[("count", self.memory_processes.to_string())],
                ),
                Style::new().fg(self.theme.module_color().tui()),
            ));
        }
        spans
    }

    pub(super) fn focus_next_unknown(&mut self) {
        let start = self.visible.get(self.cursor).map_or(0, |index| index + 1);
        let Some(index) = (start..self.rows.len()).chain(0..start).find(|index| {
            let row = &self.rows[*index];
            row.selected > 0
                && row.item_accounting == Some(CleanupSizeAccounting::ProcessedBytes)
                && row
                    .item
                    .is_some_and(|item| item.reclaimable_bytes.is_none())
        }) else {
            return;
        };
        // Explicit navigation reveals the matching item even behind a filter or collapsed parent.
        self.search.clear();
        let mut ancestor = self.rows[index].parent;
        while let Some(parent) = ancestor {
            match &self.rows[parent].kind {
                RowKind::Module(id) => {
                    self.collapsed_modules.remove(id);
                }
                RowKind::Task { module_id, task_id } => {
                    self.collapsed_tasks
                        .remove(&(module_id.clone(), task_id.clone()));
                }
                RowKind::Item(_) => {}
            }
            ancestor = self.rows[parent].parent;
        }
        self.refresh_visible_rows();
        self.cursor = self
            .visible
            .iter()
            .position(|visible| *visible == index)
            .expect("Unknown-size navigation reveals its selected leaf");
    }
}
