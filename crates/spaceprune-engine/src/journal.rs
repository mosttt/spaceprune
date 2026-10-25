//! Append-only execution evidence. An unfinished journal never authorizes automatic replay.

use super::{
    CleanupPlan, CleanupSizeAccounting, ExecutionJournalReference, ExecutionReport,
    GroupExecutionReport, PlanGroup,
};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

pub(super) struct ExecutionJournal {
    file: File,
    reference: ExecutionJournalReference,
}

mod recovery;
pub use recovery::recover_execution;

impl ExecutionJournal {
    pub(super) fn start(
        directory: &Path,
        plan: &CleanupPlan,
        accounting: &[CleanupSizeAccounting],
    ) -> io::Result<Self> {
        if !directory.is_absolute() {
            return Err(io::Error::other(
                "Execution state directory must be absolute",
            ));
        }
        fs::create_dir_all(directory)?;
        let metadata = fs::symlink_metadata(directory)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(io::Error::other(
                "Execution journal directory is not a regular directory",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let path = directory.join(format!("{}.jsonl", plan.id()));
        let file = options.open(&path)?;
        let mut journal = Self {
            file,
            reference: ExecutionJournalReference {
                plan_id: plan.id().to_owned(),
                path: spaceprune_contracts::access::NativePath::from_path(&path),
            },
        };
        journal.append(
            &serde_json::json!({"event": "started", "schema_version": 4, "plan": plan, "accounting": accounting,
            "recovery": "interrupted_unless_finished_no_automatic_replay"}),
        )?;
        Ok(journal)
    }

    pub(super) fn reference(&self) -> &ExecutionJournalReference {
        &self.reference
    }

    fn append(&mut self, record: &impl Serialize) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(record)?;
        bytes.push(b'\n');
        self.file.write_all(&bytes)?;
        self.file.sync_data()
    }

    pub(super) fn group_started(&mut self, group: &PlanGroup) -> io::Result<()> {
        self.append(
            &serde_json::json!({"event": "group_started", "module_id": group.module_id,
            "task_id": group.task_id,
            "items": group.items.iter().map(|item| &item.id).collect::<Vec<_>>() }),
        )
    }

    pub(super) fn item_finished(
        &mut self,
        task: &spaceprune_plugin_api::TaskRequest,
        outcome: &spaceprune_plugin_api::ItemExecution,
    ) -> io::Result<()> {
        self.append(
            &serde_json::json!({"event": "item_finished", "module_id": task.module_id,
            "task_id": task.task_id, "handler_id": task.handler_id, "outcome": outcome}),
        )
    }

    pub(super) fn group_finished(&mut self, group: &GroupExecutionReport) -> io::Result<()> {
        self.append(&serde_json::json!({"event": "group_finished", "result": group}))
    }

    pub(super) fn finished(&mut self, report: &ExecutionReport) -> io::Result<()> {
        self.append(&serde_json::json!({"event": "finished", "report": report}))
    }
}

/// Default state location shared by local hosts; filenames are unique per plan.
pub fn default_directory() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support"));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")));
    base.map(|base| base.join("spaceprune/.spaceprune-executions"))
}
