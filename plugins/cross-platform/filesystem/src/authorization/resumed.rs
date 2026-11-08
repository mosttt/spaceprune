//! Project cumulative per-selection receipts into live cleanup totals.

use super::*;
use spaceprune_plugin_api::{ItemExecution, ItemExecutionStatus};
use std::sync::Mutex;

type ItemKey = (String, String, String);

pub(super) struct ResumedProgress<'a> {
    previous: &'a BTreeMap<ItemKey, ItemExecution>,
    cumulative: Mutex<BTreeMap<ItemKey, ItemExecution>>,
    progress: &'a dyn ProgressSink,
}

impl<'a> ResumedProgress<'a> {
    pub(super) fn new(
        previous: &'a BTreeMap<ItemKey, ItemExecution>,
        progress: &'a dyn ProgressSink,
    ) -> Self {
        Self {
            previous,
            cumulative: Mutex::new(previous.clone()),
            progress,
        }
    }
}

impl ProgressSink for ResumedProgress<'_> {
    fn emit(&self, event: ProgressEvent) {
        self.progress.emit(event);
    }
    fn set_input_active(&self, active: bool) {
        self.progress.set_input_active(active);
    }
    fn cleanup_item_finished(
        &self,
        task: &TaskRequest,
        outcome: &ItemExecution,
    ) -> PluginResult<()> {
        let key = (
            task.module_id.clone(),
            task.task_id.clone(),
            outcome.item_id.clone(),
        );
        let mut outcome = outcome.clone();
        if let Some(previous) = self.previous.get(&key) {
            combine_resumed_outcome(previous, &mut outcome);
        }
        self.progress.cleanup_item_finished(task, &outcome)?;
        let totals = {
            let mut cumulative = self.cumulative.lock().expect("cleanup receipts");
            cumulative.insert(key, outcome);
            let mut totals = (0usize, 0u64, 0usize);
            for ((module, id, _), outcome) in cumulative.iter() {
                if module == &task.module_id && id == &task.task_id {
                    totals.0 = totals.0.saturating_add(usize::from(matches!(
                        outcome.status,
                        ItemExecutionStatus::Succeeded | ItemExecutionStatus::AlreadyAbsent
                    )));
                    totals.1 = totals.1.saturating_add(outcome.known_reclaimed_bytes());
                    totals.2 = totals
                        .2
                        .saturating_add(usize::from(outcome.has_unknown_reclaimed_size()));
                }
            }
            totals
        };
        self.progress
            .record_cleanup(task, totals.0, totals.1, totals.2);
        Ok(())
    }
    // Worker aggregates describe only its phase. Receipts above retain prior unknown effects
    // and count each original selection once, even when both phases have unknown sizes.
}
