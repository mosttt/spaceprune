//! Supervised one-shot transport with an EOF cancellation grace period for restoration.
use super::worker::{Reply, Request};
use super::*;
use crate::{IoFailureKind, process::ProcessTree};
use std::{
    io::BufReader,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

pub(super) fn remove(
    executable: &Path,
    binding: &DirectoryBinding,
    context: &OperationContext,
) -> super::Cleanup {
    let mut started = false;
    let result = execute(executable, binding, context, &mut started);
    match result {
        Ok(removal) => super::Cleanup {
            removal,
            unconfirmed: false,
        },
        Err(error) => super::Cleanup {
            removal: Removal {
                error: Some(if started {
                    crate::error::failure(
                        crate::error::classify(&error).kind,
                        messages::message(
                            "linux.go_worker_unconfirmed",
                            &[("error", error.to_string())],
                        ),
                    )
                } else {
                    error
                }),
                ..Removal::default()
            },
            unconfirmed: started,
        },
    }
}

fn execute(
    executable: &Path,
    binding: &DirectoryBinding,
    context: &OperationContext,
    started: &mut bool,
) -> io::Result<Removal> {
    let request = Request {
        binding: binding.clone(),
        operation: context.transfer()?,
    };
    let mut bytes = serde_json::to_vec(&request)?;
    bytes.push(b'\n');
    if bytes.len() > worker::MAX_FRAME {
        return Err(io::Error::other(
            "Go cache worker request exceeds its budget",
        ));
    }
    let mut command = Command::new(executable);
    command
        .arg(WORKER_ARGUMENT)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut process = ProcessTree::spawn(&mut command)?;
    let mut input = process.child_mut().stdin.take();
    crate::process::write_pipe(input.as_mut().expect("worker input"), &bytes, context)?;
    // Once the complete request is delivered, only a terminal report establishes no effects.
    // A lost readiness frame must not turn a possibly running cleanup into a safe-to-retry error.
    *started = true;
    let mut ready = false;
    let output = process.child_mut().stdout.take().expect("worker output");
    let (send, receive) = mpsc::sync_channel(2);
    let output_limit = context.budget.output_bytes;
    let line_limit = context.budget.line_bytes;
    let reader = std::thread::spawn(move || {
        let mut output = BufReader::new(output);
        let mut used = 0usize;
        loop {
            let result = worker::frame(&mut output, line_limit).and_then(|bytes| {
                used = used.saturating_add(bytes.len());
                if used > output_limit {
                    return Err(io::Error::other("Go cache output exceeds its budget"));
                }
                serde_json::from_slice::<Reply>(&bytes).map_err(io::Error::other)
            });
            let terminal = !matches!(result, Ok(Reply::Ready | Reply::Alive));
            if send.send(result).is_err() || terminal {
                break;
            }
        }
    });
    let mut stopping = None;
    let mut last_output = Instant::now();
    let wait = (|| {
        loop {
            if context.is_stopped() && stopping.is_none() {
                // Closing stdin lets the native traversal restore captured survivors first.
                input.take();
                stopping = Some(Instant::now() + Duration::from_secs(2));
            }
            if stopping.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(crate::error::failure(
                    IoFailureKind::Cancelled,
                    messages::text("linux.go_worker_stop_unconfirmed"),
                ));
            }
            let idle = if ready {
                context.budget.idle_timeout
            } else {
                context.budget.first_output_timeout
            };
            if stopping.is_none() && last_output.elapsed() >= idle {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Go cache worker became unresponsive",
                ));
            }
            match receive.recv_timeout(Duration::from_millis(20)) {
                Ok(Ok(Reply::Ready)) if !ready => {
                    ready = true;
                    last_output = Instant::now();
                }
                Ok(Ok(Reply::Alive)) if ready => last_output = Instant::now(),
                Ok(Ok(Reply::Finished { report })) => return Ok(report.removal()),
                Ok(Ok(Reply::Ready | Reply::Alive)) => {
                    return Err(io::Error::other("Duplicate worker readiness"));
                }
                Ok(Err(error)) => return Err(error),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other(messages::text("linux.go_worker_exit")));
                }
            }
        }
    })();
    input.take();
    let stopped = process.terminate_and_wait();
    drop(receive);
    let joined = reader.join();
    stopped?;
    joined.map_err(|_| io::Error::other("Go cache response reader failed"))?;
    wait
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    fn run_script(body: &str) -> super::super::Cleanup {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("worker");
        fs::write(&script, format!("#!/bin/sh\nread -r request\n{body}\n")).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let path = temp.path().join("cache");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), b"must survive").unwrap();
        let binding = DirectoryBinding {
            path: crate::NativePath::from_path(&path),
            parent_identity: crate::filesystem::read_identity(temp.path()).unwrap(),
            identity: crate::filesystem::read_identity(&path).unwrap(),
        };
        let result = remove(&script, &binding, &OperationContext::default());
        assert_eq!(fs::read(path.join("keep")).unwrap(), b"must survive");
        result
    }

    #[test]
    fn a_reported_namespace_denial_has_no_effects_and_keeps_its_native_classification() {
        let result = run_script(
            r#"printf '%s\n' '{"kind":"finished","report":{"already_absent":false,"removed_entries":0,"reclaimed_estimate":0,"error":{"classification":{"kind":"access_denied","os_code":1},"message":"namespace unavailable","effects":false}}}'"#,
        );
        assert!(!result.unconfirmed);
        let error = result.removal.error.unwrap();
        assert_eq!(
            crate::error::classify(&error).kind,
            IoFailureKind::AccessDenied
        );
        assert_eq!(crate::error::classify(&error).os_code, Some(1));
        assert!(!crate::error::has_effects(&error));
    }

    #[test]
    fn lost_terminal_result_is_uncertain_even_when_readiness_was_lost() {
        for body in ["exit 1", r#"printf '%s\n' '{"kind":"ready"}'; exit 1"#] {
            let result = run_script(body);
            assert!(result.unconfirmed);
            assert!(!crate::error::has_effects(
                result.removal.error.as_ref().unwrap()
            ));
        }
    }
}
