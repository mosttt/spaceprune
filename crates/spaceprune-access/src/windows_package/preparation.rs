//! Serialize package preparation, then recheck actual installed images before granting any session.
use super::verification::check;
use crate::{Interaction, OperationContext, PermissionOutcome};
use std::{
    collections::BTreeMap,
    sync::{Mutex, TryLockError},
    time::{Duration, Instant},
};

#[derive(Default)]
pub(crate) struct Preparation {
    failures: Mutex<BTreeMap<(String, u64), (Instant, PermissionOutcome)>>,
}

pub(crate) fn ensure<T>(
    serial: &Preparation,
    operation: &OperationContext,
    mut inspect: impl FnMut() -> Result<Option<T>, PermissionOutcome>,
    install: impl FnOnce() -> Result<(), PermissionOutcome>,
) -> Result<Option<T>, PermissionOutcome> {
    check(operation)?;
    match inspect() {
        Err(PermissionOutcome::HelperMissing | PermissionOutcome::HelperOutdated) => (),
        result => return result,
    }
    if operation.interaction == Interaction::NonInteractive {
        return Err(PermissionOutcome::NeedsAuthorization);
    }
    let mut state = loop {
        check(operation)?;
        match serial.failures.try_lock() {
            Ok(guard) => break guard,
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(20)),
            Err(TryLockError::Poisoned(_)) => {
                return Err(PermissionOutcome::AuthorizationUnavailable);
            }
        }
    };
    state.retain(|_, (deadline, _)| *deadline > Instant::now());
    let key = (operation.id.clone(), operation.generation);
    if let Some((_, failure)) = state.get(&key) {
        return Err(*failure);
    }
    match inspect() {
        Err(PermissionOutcome::HelperMissing | PermissionOutcome::HelperOutdated) => (),
        result => return result,
    }
    check(operation)?;
    let result = install().and_then(|()| {
        check(operation)?;
        inspect()?.map(Some).ok_or(PermissionOutcome::HelperMissing)
    });
    if let Err(outcome) = result {
        state.insert(key, (operation.deadline, outcome));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CancellationToken;
    use std::sync::{
        Barrier,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    fn operation() -> OperationContext {
        OperationContext::new(
            CancellationToken::default(),
            Interaction::Foreground,
            Duration::from_secs(5),
        )
    }
    #[test]
    fn concurrent_missing_components_share_one_install_and_recheck_the_files() {
        let serial = Preparation::default();
        let ready = AtomicBool::new(false);
        let installs = AtomicUsize::new(0);
        let barrier = Barrier::new(2);
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    barrier.wait();
                    let result = ensure(
                        &serial,
                        &operation(),
                        || {
                            if ready.load(Ordering::Acquire) {
                                Ok(Some("verified image"))
                            } else {
                                Err(PermissionOutcome::HelperMissing)
                            }
                        },
                        || {
                            installs.fetch_add(1, Ordering::Relaxed);
                            std::thread::sleep(Duration::from_millis(30));
                            ready.store(true, Ordering::Release);
                            Ok(())
                        },
                    );
                    assert_eq!(result, Ok(Some("verified image")));
                });
            }
        });
        assert_eq!(installs.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn account_bound_legacy_components_are_replaced_before_the_original_operation_continues() {
        use crate::windows_package::Component;
        let serial = Preparation::default();
        let operation = operation();
        let installed = AtomicBool::new(false);
        let installs = AtomicUsize::new(0);
        for (component, legacy_probe) in [
            (Component::Metadata, "--supports-windows-metadata-v6"),
            (Component::Cleanup, "--supports-cleanup-v4"),
        ] {
            let expected = component.support_argument();
            assert_ne!(expected, legacy_probe);
            let result = ensure(
                &serial,
                &operation,
                || {
                    let accepted = if installed.load(Ordering::Acquire) {
                        expected.as_str()
                    } else {
                        legacy_probe
                    };
                    if accepted == expected {
                        Ok(Some(component))
                    } else {
                        Err(PermissionOutcome::HelperOutdated)
                    }
                },
                || {
                    installs.fetch_add(1, Ordering::Relaxed);
                    installed.store(true, Ordering::Release);
                    Ok(())
                },
            );
            assert_eq!(result, Ok(Some(component)));
            assert!(!operation.is_stopped());
        }
        assert_eq!(
            installs.load(Ordering::Relaxed),
            1,
            "Both new components are installed together"
        );
    }

    #[test]
    fn untrusted_images_and_background_work_do_not_start_installation() {
        let serial = Preparation::default();
        for (operation, outcome, expected) in [
            (
                operation(),
                PermissionOutcome::HelperUntrusted,
                PermissionOutcome::HelperUntrusted,
            ),
            (
                OperationContext::default(),
                PermissionOutcome::HelperMissing,
                PermissionOutcome::NeedsAuthorization,
            ),
        ] {
            let result = ensure::<()>(
                &serial,
                &operation,
                || Err(outcome),
                || panic!("installation must not start"),
            );
            assert_eq!(result, Err(expected));
        }
    }
    #[test]
    fn installer_success_still_requires_the_actual_image_and_live_operation() {
        let serial = Preparation::default();
        let operation = operation();
        assert_eq!(
            ensure::<()>(
                &serial,
                &operation,
                || Err(PermissionOutcome::HelperMissing),
                || Ok(())
            ),
            Err(PermissionOutcome::HelperMissing)
        );
        let serial = Preparation::default();
        let probes = AtomicUsize::new(0);
        let result = ensure::<()>(
            &serial,
            &operation,
            || {
                assert!(
                    probes.fetch_add(1, Ordering::Relaxed) < 2,
                    "no inspection after cancellation"
                );
                Err(PermissionOutcome::HelperOutdated)
            },
            || {
                operation.cancel();
                Ok(())
            },
        );
        assert_eq!(result, Err(PermissionOutcome::Cancelled));
    }

    #[test]
    fn failed_installation_is_not_reprompted_for_each_root_in_the_same_operation() {
        let serial = Preparation::default();
        let first = operation();
        let attempts = AtomicUsize::new(0);
        for context in [&first, &first, &operation()] {
            assert_eq!(
                ensure::<()>(
                    &serial,
                    context,
                    || Err(PermissionOutcome::HelperMissing),
                    || {
                        attempts.fetch_add(1, Ordering::Relaxed);
                        Err(PermissionOutcome::Denied)
                    }
                ),
                Err(PermissionOutcome::Denied)
            );
        }
        assert_eq!(attempts.load(Ordering::Relaxed), 2);
    }
}
