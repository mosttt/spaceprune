//! Durable item receipts and in-memory evidence retained when a handler loses its final reply.

use super::*;
use spaceprune_plugin_api::{ItemExecution, ItemExecutionStatus, PluginResult};
use std::sync::Mutex;

type ItemKey = (String, String, String);
struct State {
    journal: Option<ExecutionJournal>,
    failure: Option<String>,
    outcomes: BTreeMap<ItemKey, ItemExecution>,
}

pub(super) struct ObservedProgress<'a> {
    inner: &'a dyn ProgressSink,
    context: &'a OperationContext,
    selected: BTreeMap<ItemKey, String>,
    accounting: BTreeMap<String, CleanupSizeAccounting>,
    state: Mutex<State>,
}

impl<'a> ObservedProgress<'a> {
    pub(super) fn new(
        inner: &'a dyn ProgressSink,
        context: &'a OperationContext,
        plan: &CleanupPlan,
        registry: &PluginRegistry,
        journal: Option<ExecutionJournal>,
    ) -> Self {
        Self {
            inner,
            context,
            accounting: registry
                .descriptors()
                .into_iter()
                .map(|descriptor| (descriptor.id, descriptor.size_accounting))
                .collect(),
            selected: plan
                .groups
                .iter()
                .flat_map(|group| {
                    group.items.iter().map(|item| {
                        (
                            (
                                group.module_id.clone(),
                                group.task_id.clone(),
                                item.id.clone(),
                            ),
                            group.handler_id.clone(),
                        )
                    })
                })
                .collect(),
            state: Mutex::new(State {
                journal,
                failure: None,
                outcomes: BTreeMap::new(),
            }),
        }
    }

    pub(super) fn accounting_for(&self, handler_id: &str) -> CleanupSizeAccounting {
        self.accounting.get(handler_id).copied().unwrap_or_default()
    }

    fn write(
        &self,
        record: impl FnOnce(&mut ExecutionJournal) -> std::io::Result<()>,
    ) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "execution-state-unavailable".to_owned())?;
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        if let Some(journal) = &mut state.journal
            && let Err(error) = record(journal)
        {
            let message = format!("execution-journal-write-failed: {error}");
            state.failure = Some(message.clone());
            self.context.cancel();
            return Err(message);
        }
        Ok(())
    }
    pub(super) fn group_started(&self, group: &PlanGroup) -> Result<(), String> {
        self.write(|journal| journal.group_started(group))
    }
    pub(super) fn group_finished(&self, group: &GroupExecutionReport) -> Result<(), String> {
        self.write(|journal| journal.group_finished(group))
    }
    pub(super) fn finished(&self, report: &ExecutionReport) -> Result<(), String> {
        self.write(|journal| journal.finished(report))
    }

    /// A valid final reply wins except when a transport failure has already made an item uncertain.
    pub(super) fn reconcile(
        &self,
        group: &PlanGroup,
        result: &mut Option<HandlerCleanResult>,
        error: Option<&PluginError>,
    ) {
        let state = self.state.lock().expect("execution state");
        let outcomes: BTreeMap<_, _> = group
            .items
            .iter()
            .filter_map(|item| {
                state
                    .outcomes
                    .get(&(
                        group.module_id.clone(),
                        group.task_id.clone(),
                        item.id.clone(),
                    ))
                    .map(|outcome| (item.id.as_str(), outcome))
            })
            .collect();
        let final_outcome = |id: &str| {
            result
                .as_ref()
                .and_then(|result| result.outcomes.iter().find(|outcome| outcome.item_id == id))
        };
        if error.is_none()
            && !outcomes.iter().any(|(id, observed)| {
                final_outcome(id)
                    .is_none_or(|actual| reconcile_outcome(observed, actual) != *actual)
            })
        {
            return;
        }
        let mut recovered = HandlerCleanResult {
            space_changes: result
                .as_ref()
                .map(|result| result.space_changes.clone())
                .unwrap_or_default(),
            unknown_reclaimed_items: Some(0),
            ..Default::default()
        };
        for item in &group.items {
            let outcome = match (
                outcomes.get(item.id.as_str()).copied(),
                final_outcome(&item.id),
            ) {
                (Some(observed), Some(actual)) => Some(reconcile_outcome(observed, actual)),
                (_, Some(actual)) => Some(actual.clone()),
                (Some(observed), None) => Some(observed.clone()),
                (None, None) => None,
            }
            .unwrap_or_else(|| {
                let not_started =
                    error.is_some_and(|error| error.code == "cleanup-authorization-unavailable");
                ItemExecution {
                    item_id: item.id.clone(),
                    status: if not_started {
                        ItemExecutionStatus::NotAttempted
                    } else {
                        ItemExecutionStatus::Uncertain
                    },
                    reclaimed_estimate: not_started.then_some(0),
                    reclaimed_lower_bound: 0,
                    failure: (!not_started).then(|| ItemFailure {
                        file_usage: None,
                        authorization: None,
                        io_failure: None,
                        item_id: item.id.clone(),
                        message: "Cleanup result was not confirmed; rescan before retrying".into(),
                    }),
                }
            });
            recovered.record_outcome(outcome);
        }
        *result = Some(recovered);
    }
}

impl ProgressSink for ObservedProgress<'_> {
    fn emit(&self, event: ProgressEvent) {
        self.inner.emit(event);
    }
    fn set_input_active(&self, active: bool) {
        self.inner.set_input_active(active);
    }
    fn record_cleanup(&self, task: &TaskRequest, succeeded: usize, bytes: u64, unknown: usize) {
        self.inner.record_cleanup_with_accounting(
            task,
            self.accounting_for(&task.handler_id),
            succeeded,
            bytes,
            unknown,
        );
    }
    fn cleanup_item_finished(
        &self,
        task: &TaskRequest,
        outcome: &ItemExecution,
    ) -> PluginResult<()> {
        if let Err(message) = outcome.validate() {
            self.context.cancel();
            return Err(PluginError::new("cleanup-outcome-invalid", message));
        }
        let key = (
            task.module_id.clone(),
            task.task_id.clone(),
            outcome.item_id.clone(),
        );
        if self.selected.get(&key) != Some(&task.handler_id) {
            self.context.cancel();
            return Err(PluginError::new(
                "cleanup-outcome-invalid",
                "Outcome is outside the confirmed plan",
            ));
        }
        {
            let mut state = self.state.lock().map_err(|_| {
                PluginError::new("execution-state-unavailable", "Execution state unavailable")
            })?;
            if state.outcomes.get(&key) == Some(outcome) {
                return match &state.failure {
                    Some(error) => Err(PluginError::new(
                        "execution-journal-write-failed",
                        error.clone(),
                    )),
                    None => Ok(()),
                };
            }
            state.outcomes.insert(key, outcome.clone());
        }
        self.write(|journal| journal.item_finished(task, outcome))
            .map_err(|error| PluginError::new("execution-journal-write-failed", error))?;
        let result = self.inner.cleanup_item_finished(task, outcome);
        if let Err(error) = &result {
            self.context.cancel();
            if let Ok(mut state) = self.state.lock() {
                state.failure = Some(format!("execution-receipt-failed: {error}"));
            }
        }
        result
    }
}

fn prefer_observed(observed: &ItemExecution, actual: &ItemExecution) -> bool {
    if observed == actual {
        return false;
    }
    if observed.status == ItemExecutionStatus::Uncertain {
        return true;
    }
    if actual.status == ItemExecutionStatus::Uncertain {
        return false;
    }
    if matches!(
        actual.status,
        ItemExecutionStatus::Succeeded | ItemExecutionStatus::AlreadyAbsent
    ) {
        return false;
    }
    matches!(
        observed.status,
        ItemExecutionStatus::Succeeded | ItemExecutionStatus::AlreadyAbsent
    ) || (observed.status == ItemExecutionStatus::PartiallyApplied
        && actual.status != ItemExecutionStatus::PartiallyApplied)
}

pub(crate) fn reconcile_outcome(observed: &ItemExecution, actual: &ItemExecution) -> ItemExecution {
    let mut outcome = if prefer_observed(observed, actual) {
        observed
    } else {
        actual
    }
    .clone();
    // Receipts are cumulative evidence for one selection, not additional phases to add again.
    let known = observed
        .known_reclaimed_bytes()
        .max(actual.known_reclaimed_bytes());
    if outcome.status == ItemExecutionStatus::AlreadyAbsent && known > 0 {
        outcome.status = ItemExecutionStatus::Succeeded;
    }
    if outcome.reclaimed_estimate.is_none_or(|bytes| bytes < known) {
        outcome.reclaimed_estimate = None;
        outcome.reclaimed_lower_bound = known;
    }
    outcome
}
