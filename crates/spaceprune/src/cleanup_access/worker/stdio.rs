//! Bounded stdio and named-pipe cleanup frames with per-item durable acknowledgements.

use super::*;
#[cfg(target_os = "linux")]
use std::io::BufReader;
use std::{
    io::{BufRead, Read, Write},
    sync::mpsc,
};

pub(in crate::cleanup_access) fn frame(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    frame_with_limit(reader, MAX_FRAME_BYTES)
}

pub(in crate::cleanup_access) fn frame_with_limit(
    reader: &mut impl BufRead,
    limit: usize,
) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    reader
        .take((limit.saturating_add(1)) as u64)
        .read_until(b'\n', &mut bytes)?;
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() > limit || !bytes.ends_with(b"\n") {
        return Err(io::Error::other("Invalid cleanup frame"));
    }
    Ok(Some(bytes))
}

struct Sink {
    output: Mutex<(Box<dyn Write + Send>, usize)>,
    input: Mutex<mpsc::Receiver<Vec<u8>>>,
    context: OperationContext,
}
impl Sink {
    fn reply(&self, reply: &Reply) -> io::Result<()> {
        let mut out = self
            .output
            .lock()
            .map_err(|_| io::Error::other("Output unavailable"))?;
        let mut bytes = serde_json::to_vec(&Response {
            operation: self.context.identity(),
            reply,
        })?;
        bytes.push(b'\n');
        if bytes.len() > self.context.budget.line_bytes
            || out.1.saturating_add(bytes.len()) > self.context.budget.output_bytes
        {
            self.context.cancel();
            return Err(io::Error::other(
                "Cleanup response exceeded its resource budget",
            ));
        }
        out.1 += bytes.len();
        let result = write_bounded(&mut *out.0, &bytes, &self.context);
        if result.is_err() {
            self.context.cancel();
        }
        result
    }
}
impl ProgressSink for Sink {
    fn emit(&self, event: ProgressEvent) {
        let _ = self.reply(&Reply::Progress { event });
    }
    fn cleanup_item_finished(
        &self,
        task: &TaskRequest,
        outcome: &spaceprune_plugin_api::ItemExecution,
    ) -> PluginResult<()> {
        let receipt = (|| -> io::Result<()> {
            self.reply(&Reply::ItemFinished {
                task: task.clone(),
                outcome: outcome.clone(),
            })?;
            let bytes = self
                .input
                .lock()
                .map_err(|_| io::Error::other("Receipt channel unavailable"))?
                .recv_timeout(
                    Duration::from_secs(30).min(
                        self.context
                            .deadline
                            .saturating_duration_since(Instant::now()),
                    ),
                )
                .map_err(|_| io::Error::other("Host did not acknowledge the cleanup outcome"))?;
            let receipt: OutcomeReceipt = serde_json::from_slice(&bytes)?;
            if !receipt.accepted {
                return Err(io::Error::other(
                    "Host could not persist the cleanup outcome",
                ));
            }
            Ok(())
        })();
        if receipt.is_err() {
            self.context.cancel();
        }
        receipt.map_err(|error| PluginError::new("cleanup-receipt-failed", error.to_string()))
    }
    fn record_cleanup(&self, task: &TaskRequest, succeeded: usize, bytes: u64, unknown: usize) {
        let _ = self.reply(&Reply::Count {
            task: task.clone(),
            succeeded,
            bytes,
            unknown,
        });
    }
}

#[cfg(target_os = "linux")]
pub(super) fn run() -> io::Result<()> {
    serve(BufReader::new(io::stdin()), Box::new(io::stdout()))
}

#[cfg(target_os = "linux")]
pub(super) fn serve(
    input: impl BufRead + Send + 'static,
    output: Box<dyn Write + Send>,
) -> io::Result<()> {
    serve_bound(input, output, None)
}

#[cfg(target_os = "linux")]
pub(super) fn serve_bound(
    mut input: impl BufRead + Send + 'static,
    output: Box<dyn Write + Send>,
    binding: Option<&OperationContext>,
) -> io::Result<()> {
    let init = frame(&mut input)?.ok_or_else(|| io::Error::other("Missing confirmed plan"))?;
    serve_grant(input, output, binding, init)
}

pub(in crate::cleanup_access) fn serve_grant(
    mut input: impl BufRead + Send + 'static,
    output: Box<dyn Write + Send>,
    binding: Option<&OperationContext>,
    init: Vec<u8>,
) -> io::Result<()> {
    let grant: Grant = serde_json::from_slice(&init)?;
    let context = plan::request_context(&grant, binding)?;
    let cancelled = context.clone();
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        while let Ok(Some(bytes)) = frame(&mut input) {
            if tx.send(bytes).is_err() {
                break;
            }
        }
        cancelled.cancel();
    });
    let mut plan = plan::PreparedPlan::new(grant, Some(&context))?;
    let context = plan.context().clone();
    let mut sink = Sink {
        output: Mutex::new((output, 0)),
        input: Mutex::new(rx),
        context: context.clone(),
    };
    sink.reply(&Reply::Ready)?;
    while !context.is_stopped() {
        let Ok(bytes) = sink
            .input
            .lock()
            .map_err(|_| io::Error::other("Input unavailable"))?
            .recv_timeout(
                IDLE_TIMEOUT.min(context.deadline.saturating_duration_since(Instant::now())),
            )
        else {
            break;
        };
        let batch = plan.prepare(serde_json::from_slice(&bytes)?, None)?;
        let batch_sink = Sink {
            context: batch.context.clone(),
            ..sink
        };
        let results = plan.execute(&batch, &batch_sink);
        batch_sink.reply(&Reply::Results { results })?;
        if batch.context.is_stopped() {
            break;
        }
        sink = Sink {
            context: context.clone(),
            ..batch_sink
        };
    }
    Ok(())
}

fn write_bounded(
    output: &mut dyn Write,
    mut bytes: &[u8],
    context: &OperationContext,
) -> io::Result<()> {
    while !bytes.is_empty() {
        // A stopped batch still reports already-applied effects and unattempted items.
        // Cancellation only forbids waiting for further transport capacity here.
        match output.write(bytes) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(count) => bytes = &bytes[count..],
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                context.check()?;
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    output.flush()
}
