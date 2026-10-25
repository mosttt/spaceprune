//! Bind each plugin receipt to the host's original selection before acknowledging it.

use spaceprune_plugin_api::{
    CleanupItem, HandlerCleanResult, ItemExecution, ProgressSink, TaskRequest,
};
use std::collections::BTreeMap;

pub(super) struct CleanupReceipts<'a> {
    task: &'a TaskRequest,
    items: &'a [CleanupItem],
    progress: &'a dyn ProgressSink,
    confirmed: BTreeMap<String, ItemExecution>,
}

impl<'a> CleanupReceipts<'a> {
    pub(super) fn new(
        task: &'a TaskRequest,
        items: &'a [CleanupItem],
        progress: &'a dyn ProgressSink,
    ) -> Self {
        Self {
            task,
            items,
            progress,
            confirmed: BTreeMap::new(),
        }
    }

    pub(super) fn accept(&mut self, outcome: &ItemExecution) -> Result<(), String> {
        outcome.validate()?;
        if !self.items.iter().any(|item| item.id == outcome.item_id)
            || self.confirmed.contains_key(&outcome.item_id)
        {
            return Err(
                spaceprune_contracts::messages::text("host.receipt_unknown_item").to_owned(),
            );
        }
        self.progress
            .cleanup_item_finished(self.task, outcome)
            .map_err(|error| error.to_string())?;
        self.confirmed
            .insert(outcome.item_id.clone(), outcome.clone());
        Ok(())
    }

    pub(super) fn validate_result(
        &self,
        result: &HandlerCleanResult,
        cancelled: bool,
    ) -> Result<(), String> {
        result.validate_selected(self.items, cancelled)?;
        if self.confirmed.len() != self.items.len()
            || result.outcomes.len() != self.items.len()
            || result
                .outcomes
                .iter()
                .any(|outcome| self.confirmed.get(&outcome.item_id) != Some(outcome))
        {
            return Err(spaceprune_contracts::messages::text("host.receipt_mismatch").to_owned());
        }
        Ok(())
    }
}
