//! One namespace and one bound cache per fresh executable. No fork-time Rust callbacks.

use super::*;
use crate::{IoFailure, OperationTransfer, ResourceBudget};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, Read, Write},
    time::Duration,
};

#[path = "namespace.rs"]
#[allow(unsafe_code)]
mod namespace;

pub(super) const MAX_FRAME: usize = 64 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub binding: DirectoryBinding,
    pub operation: OperationTransfer,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Reply {
    Ready,
    Alive,
    Finished { report: Report },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Report {
    pub already_absent: bool,
    pub removed_entries: usize,
    pub reclaimed_estimate: u64,
    pub error: Option<Failure>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Failure {
    pub classification: IoFailure,
    pub message: String,
    pub effects: bool,
}
impl From<Removal> for Report {
    fn from(removal: Removal) -> Self {
        Self {
            already_absent: removal.already_absent,
            removed_entries: removal.removed_entries,
            reclaimed_estimate: removal.reclaimed_estimate,
            error: removal.error.map(|error| Failure {
                classification: crate::error::classify(&error),
                message: error.to_string(),
                effects: crate::error::has_effects(&error),
            }),
        }
    }
}
impl Report {
    pub fn removal(self) -> Removal {
        Removal {
            already_absent: self.already_absent,
            removed_entries: self.removed_entries,
            reclaimed_estimate: self.reclaimed_estimate,
            error: self.error.map(|failure| {
                let cause = failure
                    .classification
                    .os_code
                    .map(io::Error::from_raw_os_error)
                    .unwrap_or_else(|| io::Error::other(failure.message.clone()));
                if failure.effects {
                    crate::error::failure_after_effects(
                        failure.classification.kind,
                        failure.message,
                        &cause,
                    )
                } else {
                    crate::error::failure_with_cause(
                        failure.classification.kind,
                        failure.message,
                        &cause,
                    )
                }
            }),
        }
    }
}

pub(super) fn frame(input: &mut impl BufRead, limit: usize) -> io::Result<Vec<u8>> {
    let limit = limit.min(MAX_FRAME);
    let mut frame = Vec::new();
    input
        .take((limit + 1) as u64)
        .read_until(b'\n', &mut frame)?;
    if frame.len() > limit || !frame.ends_with(b"\n") {
        return Err(io::Error::other("Invalid Go cache worker frame"));
    }
    Ok(frame)
}

pub(super) fn run() -> io::Result<()> {
    let mut input = io::BufReader::new(io::stdin());
    let request: Request = serde_json::from_slice(&frame(&mut input, MAX_FRAME)?)?;
    let context = OperationContext::from_transfer(
        &request.operation,
        Duration::from_secs(3600),
        ResourceBudget::workflow(),
    )?;
    let result = namespace::enter()
        .map_err(|error| {
            crate::error::failure_with_cause(
                crate::IoFailureKind::AccessDenied,
                messages::message(
                    "linux.go_namespace_unavailable",
                    &[("error", error.to_string())],
                ),
                &error,
            )
        })
        .and_then(|()| context.check());
    if let Err(error) = result {
        return reply(Reply::Finished {
            report: Removal {
                error: Some(error),
                ..Removal::default()
            }
            .into(),
        });
    }
    // EOF is cooperative cancellation, allowing capture restoration before process termination.
    let cancellation = context.cancellation.clone();
    std::thread::spawn(move || {
        let mut byte = [0];
        let _ = input.read(&mut byte);
        cancellation.cancel();
    });
    reply(Reply::Ready)?;
    let (finished, heartbeat) = std::sync::mpsc::channel();
    let heartbeat_context = context.clone();
    let heartbeat = std::thread::spawn(move || {
        let interval = Duration::from_secs(1).min(heartbeat_context.budget.idle_timeout / 2);
        while matches!(
            heartbeat.recv_timeout(interval),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ) {
            if heartbeat_context.is_stopped() || reply(Reply::Alive).is_err() {
                heartbeat_context.cancel();
                break;
            }
        }
    });
    let removal = crate::filesystem::remove_go_directory(&request.binding, &context);
    let _ = finished.send(());
    let _ = heartbeat.join();
    reply(Reply::Finished {
        report: removal.into(),
    })
}

fn reply(reply: Reply) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(&reply)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_FRAME {
        return Err(io::Error::other(
            "Go cache worker response exceeds its budget",
        ));
    }
    io::stdout().lock().write_all(&bytes)
}
