//! Manual, identity-bound Linux memory authorization, separate from disk cleanup plans.
mod worker;
use serde::{Deserialize, Serialize};
use spaceprune_access::{Interaction, OperationContext, OperationTransfer};
use spaceprune_plugin_memory::{BoundProcess, MemoryRelease, ProcessIdentity};
use std::{
    io::{self, Write},
    process::Command,
    time::Duration,
};

const FLAG: &str = "--spaceprune-memory-release-v1";
const SUPPORT: &str = "--supports-memory-maintenance-v2";
const BOUND_SUPPORT: &str = "--supports-bound-memory-maintenance-v2";
const BOUND_FLAG: &str = "--spaceprune-bound-memory-release-v1";
const MAX_BYTES: u64 = 64 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    operation: OperationTransfer,
    selected: Vec<ProcessIdentity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    executables: Vec<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    operation: spaceprune_access::OperationIdentity,
    results: Vec<MemoryRelease>,
}

pub(crate) fn release(
    selected: &[ProcessIdentity],
    operation: &OperationContext,
) -> anyhow::Result<Vec<MemoryRelease>> {
    release_selected(selected, &[], operation)
}

pub(crate) fn release_bound(
    selected: &[BoundProcess],
    operation: &OperationContext,
) -> anyhow::Result<Vec<MemoryRelease>> {
    let identities = selected
        .iter()
        .map(|p| p.identity.clone())
        .collect::<Vec<_>>();
    let executables = selected
        .iter()
        .map(|p| p.executable.clone())
        .collect::<Vec<_>>();
    release_selected(&identities, &executables, operation)
}

fn release_selected(
    selected: &[ProcessIdentity],
    executables: &[String],
    operation: &OperationContext,
) -> anyhow::Result<Vec<MemoryRelease>> {
    operation.check()?;
    anyhow::ensure!(
        !selected.is_empty()
            && selected.len() <= 128
            && selected
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == selected.len(),
        "Select between 1 and 128 distinct processes"
    );
    let bound = !executables.is_empty();
    anyhow::ensure!(
        !bound
            || (executables.len() == selected.len()
                && executables
                    .iter()
                    .all(|p| std::path::Path::new(p).is_absolute())),
        "Memory executable bindings are invalid"
    );
    if bound && spaceprune_plugin_memory::linux::can_reclaim() {
        let targets = selected
            .iter()
            .zip(executables)
            .map(|(identity, executable)| BoundProcess {
                identity: identity.clone(),
                executable: executable.clone(),
            })
            .collect::<Vec<_>>();
        return Ok(spaceprune_plugin_memory::linux::release_bound_for_user(
            &targets,
            spaceprune_plugin_memory::linux::current_user(),
            &|| operation.check(),
        )?);
    }
    if spaceprune_plugin_memory::linux::can_reclaim() {
        return Ok(spaceprune_plugin_memory::linux::release_for_user(
            selected,
            spaceprune_plugin_memory::linux::current_user(),
            &|| operation.check(),
        )?);
    }
    if operation.interaction != Interaction::Foreground
        && !(bound && operation.interaction == Interaction::Terminal)
    {
        anyhow::bail!(
            "{}",
            spaceprune_contracts::messages::text("memory.authorization_required")
        );
    }
    let executable =
        spaceprune_access::helper::ensure_cleanup_helper(operation, false).map_err(|_| {
            anyhow::anyhow!(spaceprune_contracts::messages::text(
                "memory.helper_unavailable"
            ))
        })?;
    spaceprune_access::helper::probe_protocol(
        &executable,
        if bound { BOUND_SUPPORT } else { SUPPORT },
        operation,
    )
    .map_err(|_| {
        anyhow::anyhow!(spaceprune_contracts::messages::text(
            "memory.helper_outdated"
        ))
    })?;
    let pkexec = spaceprune_access::process::system_program("pkexec").ok_or_else(|| {
        anyhow::anyhow!(spaceprune_contracts::messages::text(
            "memory.authorization_unavailable"
        ))
    })?;
    let request = Request {
        operation: operation.transfer()?,
        selected: selected.to_vec(),
        executables: executables.to_vec(),
    };
    let bytes = serde_json::to_vec(&request)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "Memory request exceeds its limit"
    );
    let mut frame = (bytes.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(&bytes);
    let mut command = Command::new(pkexec);
    command
        .arg("--disable-internal-agent")
        .arg(executable)
        .arg(if bound { BOUND_FLAG } else { FLAG });
    let output = spaceprune_access::process::capture_with_open_stdin(command, &frame, operation)
        .map_err(|_| {
            anyhow::anyhow!(spaceprune_contracts::messages::text(
                "memory.result_unconfirmed"
            ))
        })?;
    if !output.status.success() {
        anyhow::bail!(
            "{}",
            spaceprune_contracts::messages::text(if output.status.code() == Some(126) {
                "memory.authorization_cancelled"
            } else {
                "memory.result_unconfirmed"
            })
        );
    }
    let response: Response = serde_json::from_slice(&output.stdout)?;
    anyhow::ensure!(
        response.operation == operation.identity()
            && response.results.len() == selected.len()
            && response
                .results
                .iter()
                .zip(selected)
                .all(|(result, expected)| result.identity == *expected),
        "Memory response does not match confirmed selection"
    );
    Ok(response.results)
}

pub(crate) fn run_if_requested() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args == [SUPPORT] || args == [BOUND_SUPPORT] {
        return Some(0);
    }
    if args != [FLAG] && args != [BOUND_FLAG] {
        return None;
    }
    Some(if worker::run(args == [BOUND_FLAG]).is_ok() {
        0
    } else {
        77
    })
}
