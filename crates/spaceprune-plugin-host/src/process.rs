//! Bounded JSONL transport and per-selection cleanup acknowledgements.

use crate::{HostError, InstalledPlugin};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use spaceprune_access::{ResourceBudget, process::ProcessTree};
use spaceprune_plugin_api::{
    CleanupItem, HandlerCleanResult, ItemExecution, OperationContext, PROTOCOL_VERSION,
    ProgressEvent, ProgressSink, TaskRequest,
};
use std::{
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
    process::{Command, Stdio},
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

mod receipts;
use receipts::CleanupReceipts;

const POLL: Duration = Duration::from_millis(50);
const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub(super) struct ProcessPlugin {
    id: String,
    session: Mutex<ProcessSession>,
    timeout: Duration,
    budget: ResourceBudget,
}

impl ProcessPlugin {
    pub(super) fn start(plugin: InstalledPlugin) -> Result<Self, HostError> {
        let budget = ResourceBudget {
            line_bytes: 1024 * 1024,
            output_bytes: 64 * 1024 * 1024,
            first_output_timeout: Duration::from_secs(300),
            idle_timeout: Duration::from_secs(300),
        };
        let mut command = Command::new(&plugin.executable);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = ProcessTree::spawn(&mut command).map_err(|source| HostError::Io {
            path: plugin.executable.clone(),
            source,
        })?;
        let stdin = child.child_mut().stdin.take().expect("piped plugin stdin");
        let stdout = child
            .child_mut()
            .stdout
            .take()
            .expect("piped plugin stdout");
        let (input_tx, input_rx) = mpsc::sync_channel::<WriteRequest>(1);
        let (output_tx, output_rx) = mpsc::sync_channel(16);
        let mut workers = Vec::new();
        let start = || -> io::Result<()> {
            workers.push(
                thread::Builder::new()
                    .name(format!("spaceprune-plugin-{}-stdin", plugin.manifest.id))
                    .spawn(move || {
                        let mut input = BufWriter::new(stdin);
                        while let Ok(request) = input_rx.recv() {
                            let result =
                                input.write_all(&request.bytes).and_then(|()| input.flush());
                            let failed = result.is_err();
                            let _ = request.completed.send(result);
                            if failed {
                                break;
                            }
                        }
                    })?,
            );
            workers.push(
                thread::Builder::new()
                    .name(format!("spaceprune-plugin-{}-stdout", plugin.manifest.id))
                    .spawn(move || {
                        let mut output = BufReader::new(stdout);
                        loop {
                            let mut line = Vec::new();
                            let result = (&mut output)
                                .take(budget.line_bytes as u64 + 1)
                                .read_until(b'\n', &mut line)
                                .and_then(|read| {
                                    if read > budget.line_bytes {
                                        Err(io::Error::new(
                                            io::ErrorKind::InvalidData,
                                            spaceprune_contracts::messages::text("host.line_limit"),
                                        ))
                                    } else if read > 0 && line.last() != Some(&b'\n') {
                                        Err(io::Error::new(
                                            io::ErrorKind::UnexpectedEof,
                                            spaceprune_contracts::messages::text(
                                                "host.incomplete_frame",
                                            ),
                                        ))
                                    } else {
                                        Ok(line)
                                    }
                                });
                            let finished =
                                result.as_ref().is_ok_and(Vec::is_empty) || result.is_err();
                            if output_tx.send(result).is_err() || finished {
                                break;
                            }
                        }
                    })?,
            );
            Ok(())
        };
        if let Err(source) = start() {
            let _ = child.terminate_and_wait();
            drop(input_tx);
            drop(output_rx);
            for worker in workers {
                let _ = worker.join();
            }
            return Err(HostError::Io {
                path: plugin.executable,
                source,
            });
        }
        Ok(Self {
            id: plugin.manifest.id,
            session: Mutex::new(ProcessSession {
                child,
                workers,
                input: Some(input_tx),
                output: Some(output_rx),
                next_id: 1,
            }),
            timeout: Duration::from_secs(15 * 60),
            budget,
        })
    }

    pub(super) fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
        progress: Option<&dyn ProgressSink>,
        context: Option<&OperationContext>,
    ) -> Result<T, HostError> {
        self.exchange(method, params, progress, context, None)
    }

    pub(super) fn clean(
        &self,
        task: &TaskRequest,
        items: &[CleanupItem],
        progress: &dyn ProgressSink,
        context: &OperationContext,
    ) -> Result<HandlerCleanResult, HostError> {
        let mut receipts = CleanupReceipts::new(task, items, progress);
        self.exchange(
            "clean",
            json!({"task": task, "items": items}),
            Some(progress),
            Some(context),
            Some(&mut receipts),
        )
    }

    fn exchange<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
        progress: Option<&dyn ProgressSink>,
        context: Option<&OperationContext>,
        mut receipts: Option<&mut CleanupReceipts<'_>>,
    ) -> Result<T, HostError> {
        let mut operation = context.cloned().unwrap_or_default().child(self.timeout);
        operation.budget = operation.budget.restricted(self.budget);
        // Waiting behind another request must remain cancelable and deadline-bound.
        let mut session = loop {
            operation
                .check()
                .map_err(|error| self.protocol(error.to_string()))?;
            match self.session.try_lock() {
                Ok(session) => break session,
                Err(std::sync::TryLockError::WouldBlock) => thread::sleep(POLL),
                Err(_) => {
                    return Err(
                        self.protocol(spaceprune_contracts::messages::text("host.lock_poisoned"))
                    );
                }
            }
        };
        if session.input.is_none() {
            return Err(self.protocol(spaceprune_contracts::messages::text("host.session_stopped")));
        }
        let id = session.next_id;
        session.next_id += 1;
        let result = (|| {
            let request =
                json!({"protocol": PROTOCOL_VERSION, "id": id, "method": method, "params": params});
            session
                .write(&request, &operation)
                .map_err(|error| self.protocol(error.to_string()))?;
            let mut last_response = Instant::now();
            let mut total_bytes = 0_usize;
            loop {
                operation
                    .check()
                    .map_err(|error| self.protocol(error.to_string()))?;
                let idle_limit = if total_bytes == 0 {
                    operation.budget.first_output_timeout
                } else {
                    operation.budget.idle_timeout
                };
                if last_response.elapsed() >= idle_limit {
                    return Err(
                        self.protocol(spaceprune_contracts::messages::text("host.idle_timeout"))
                    );
                }
                let line = match session
                    .output
                    .as_ref()
                    .expect("active plugin output")
                    .recv_timeout(POLL)
                {
                    Ok(Ok(line)) if !line.is_empty() => line,
                    Ok(Ok(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err(
                            self.protocol(spaceprune_contracts::messages::text("host.exited"))
                        );
                    }
                    Ok(Err(error)) => return Err(self.protocol(error.to_string())),
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                };
                last_response = Instant::now();
                if line.len() > operation.budget.line_bytes {
                    return Err(self.protocol(spaceprune_contracts::messages::text(
                        "host.caller_line_limit",
                    )));
                }
                total_bytes = total_bytes.saturating_add(line.len());
                if total_bytes > operation.budget.output_bytes {
                    return Err(
                        self.protocol(spaceprune_contracts::messages::text("host.output_limit"))
                    );
                }
                let raw: Value = serde_json::from_slice(&line).map_err(|error| {
                    self.protocol(spaceprune_contracts::messages::message(
                        "host.invalid_json",
                        &[("error", (error).to_string())],
                    ))
                })?;
                let has_result = raw.get("result").is_some();
                let response: Response = serde_json::from_value(raw).map_err(|error| {
                    self.protocol(spaceprune_contracts::messages::message(
                        "host.invalid_envelope",
                        &[("error", (error).to_string())],
                    ))
                })?;
                if response.protocol != PROTOCOL_VERSION {
                    return Err(self.protocol(spaceprune_contracts::messages::text(
                        "host.protocol_unsupported",
                    )));
                }
                if let Some(event) = &response.event {
                    if response.request_id != Some(id)
                        || response.id.is_some()
                        || has_result
                        || response.error.is_some()
                    {
                        return Err(self.protocol(spaceprune_contracts::messages::text(
                            "host.event_mismatch",
                        )));
                    }
                    let data = response.data.ok_or_else(|| {
                        self.protocol(spaceprune_contracts::messages::text(
                            "host.missing_event_data",
                        ))
                    })?;
                    match event.as_str() {
                        "progress" => {
                            let event: ProgressEvent =
                                serde_json::from_value(data).map_err(|error| {
                                    self.protocol(spaceprune_contracts::messages::message(
                                        "host.invalid_progress",
                                        &[("error", (error).to_string())],
                                    ))
                                })?;
                            if let Some(progress) = progress {
                                progress.emit(event);
                            }
                        }
                        "item_finished" => {
                            let receipts = receipts.as_deref_mut().ok_or_else(|| {
                                self.protocol(spaceprune_contracts::messages::text(
                                    "host.receipt_outside_cleanup",
                                ))
                            })?;
                            let outcome: ItemExecution =
                                serde_json::from_value(data).map_err(|error| {
                                    self.protocol(spaceprune_contracts::messages::message(
                                        "host.invalid_outcome",
                                        &[("error", (error).to_string())],
                                    ))
                                })?;
                            let accepted = receipts.accept(&outcome);
                            let receipt = json!({"protocol": PROTOCOL_VERSION, "request_id": id,
                                "event": "item_receipt", "data": {"item_id": outcome.item_id, "accepted": accepted.is_ok()}});
                            session
                                .write(&receipt, &operation)
                                .map_err(|error| self.protocol(error.to_string()))?;
                            accepted.map_err(|message| self.protocol(message))?;
                        }
                        _ => {
                            return Err(self.protocol(spaceprune_contracts::messages::text(
                                "host.unknown_event",
                            )));
                        }
                    }
                    continue;
                }
                if response.id != Some(id)
                    || response.request_id.is_some()
                    || response.data.is_some()
                {
                    return Err(self.protocol(spaceprune_contracts::messages::text(
                        "host.response_mismatch",
                    )));
                }
                if let Some(error) = response.error {
                    return Err(self.protocol(format!("{}: {}", error.code, error.message)));
                }
                if !has_result {
                    return Err(
                        self.protocol(spaceprune_contracts::messages::text("host.missing_result"))
                    );
                }
                let result = response.result.unwrap_or(Value::Null);
                if let Some(receipts) = &receipts {
                    let clean_result = serde_json::from_value(result.clone()).map_err(|error| {
                        self.protocol(spaceprune_contracts::messages::message(
                            "host.invalid_cleanup",
                            &[("error", (error).to_string())],
                        ))
                    })?;
                    receipts
                        .validate_result(&clean_result, operation.is_stopped())
                        .map_err(|message| self.protocol(message))?;
                }
                return serde_json::from_value(result).map_err(|error| {
                    self.protocol(spaceprune_contracts::messages::message(
                        "host.invalid_result",
                        &[("error", (error).to_string())],
                    ))
                });
            }
        })();
        // A failed exchange cannot be safely reused: queued frames may belong to the old call.
        if result.is_err() {
            session.terminate();
        }
        result
    }

    fn protocol(&self, message: impl Into<String>) -> HostError {
        HostError::Protocol {
            plugin: self.id.clone(),
            message: message.into(),
        }
    }
}

impl Drop for ProcessPlugin {
    fn drop(&mut self) {
        self.session
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .terminate();
    }
}

#[derive(Debug)]
struct WriteRequest {
    bytes: Vec<u8>,
    completed: mpsc::SyncSender<io::Result<()>>,
}

#[derive(Debug)]
struct ProcessSession {
    child: ProcessTree,
    workers: Vec<thread::JoinHandle<()>>,
    input: Option<mpsc::SyncSender<WriteRequest>>,
    output: Option<mpsc::Receiver<io::Result<Vec<u8>>>>,
    next_id: u64,
}

impl ProcessSession {
    fn write(&self, message: &Value, operation: &OperationContext) -> io::Result<()> {
        operation.check()?;
        let mut bytes = serde_json::to_vec(message)?;
        if bytes.len() >= MAX_REQUEST_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                spaceprune_contracts::messages::text("host.request_limit"),
            ));
        }
        bytes.push(b'\n');
        let (completed, receipt) = mpsc::sync_channel(1);
        self.input
            .as_ref()
            .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?
            .try_send(WriteRequest { bytes, completed })
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        loop {
            operation.check()?;
            match receipt.recv_timeout(POLL) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::from(io::ErrorKind::BrokenPipe));
                }
            }
        }
    }

    fn terminate(&mut self) {
        self.input = None;
        self.output = None;
        let _ = self.child.terminate_and_wait();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    protocol: u32,
    id: Option<u64>,
    result: Option<Value>,
    error: Option<ProtocolError>,
    event: Option<String>,
    request_id: Option<u64>,
    data: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtocolError {
    code: String,
    message: String,
}

#[cfg(all(test, unix))]
mod tests;
