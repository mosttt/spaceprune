use super::*;
use std::{fs, path::Path, time::Instant};

fn operation() -> OperationContext {
    OperationContext::workflow(Default::default(), Interaction::Terminal)
        .child(Duration::from_secs(2))
}

fn issuer(marker: &Path, output: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .args([
            "-c",
            "echo $$ > \"$1\"; printf '%s' \"$2\"; exec sleep 60",
            "fixture",
        ])
        .arg(marker)
        .arg(output);
    command
}

fn assert_reaped(marker: &Path) {
    let pid: i32 = fs::read_to_string(marker).unwrap().trim().parse().unwrap();
    assert_eq!(
        rustix::process::kill_process(
            rustix::process::Pid::from_raw(pid).unwrap(),
            rustix::process::Signal::CONT
        ),
        Err(rustix::io::Errno::SRCH)
    );
}

#[test]
fn a_reference_is_imported_while_its_issuer_lives_and_native_denial_is_preserved() {
    let temporary = tempfile::tempdir().unwrap();
    for reject in [false, true] {
        let marker = temporary
            .path()
            .join(if reject { "denied" } else { "granted" });
        let result = import_from_authenticator(
            issuer(&marker, "01234567890123456789012345678901"),
            &operation(),
            |reference| {
                assert_eq!(reference.0, *b"01234567890123456789012345678901");
                let pid: i32 = fs::read_to_string(&marker).unwrap().trim().parse().unwrap();
                assert!(
                    rustix::process::kill_process(
                        rustix::process::Pid::from_raw(pid).unwrap(),
                        rustix::process::Signal::CONT
                    )
                    .is_ok()
                );
                if reject {
                    Err(PermissionOutcome::Denied)
                } else {
                    Ok("imported")
                }
            },
        );
        assert_eq!(
            result,
            if reject {
                Err(PermissionOutcome::Denied)
            } else {
                Ok("imported")
            }
        );
        assert_reaped(&marker);
    }
}

#[test]
fn incomplete_native_output_cannot_outlive_cancellation_or_the_original_deadline() {
    let temporary = tempfile::tempdir().unwrap();
    for cancel in [false, true] {
        let marker = temporary
            .path()
            .join(if cancel { "cancel" } else { "timeout" });
        let context = operation().child(Duration::from_millis(180));
        let pending = context.clone();
        let started = marker.clone();
        let canceller = thread::spawn(move || {
            while !started.exists() && !pending.is_stopped() {
                thread::sleep(Duration::from_millis(2));
            }
            if cancel {
                pending.cancel();
            }
        });
        let result: Result<(), _> =
            import_from_authenticator(issuer(&marker, "partial"), &context, |_| {
                panic!("Partial reference cannot be imported")
            });
        canceller.join().unwrap();
        assert_eq!(
            result,
            Err(if cancel {
                PermissionOutcome::Cancelled
            } else {
                PermissionOutcome::TimedOut
            })
        );
        assert_reaped(&marker);
    }
}

#[test]
fn noninteractive_and_truncated_native_requests_do_not_become_grants() {
    let temporary = tempfile::tempdir().unwrap();
    let marker = temporary.path().join("never-started");
    let context = OperationContext::workflow(Default::default(), Interaction::NonInteractive);
    let result: Result<(), _> =
        import_from_authenticator(issuer(&marker, "unused"), &context, |_| {
            panic!("Background request")
        });
    assert_eq!(result, Err(PermissionOutcome::NeedsAuthorization));
    assert!(!marker.exists());
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "printf short"]);
    let result: Result<(), _> =
        import_from_authenticator(command, &operation(), |_| panic!("Truncated reference"));
    assert_eq!(result, Err(PermissionOutcome::AuthorizationUnavailable));
}

mod job_control;
