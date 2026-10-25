use super::*;
use std::cell::Cell;

fn operation() -> OperationContext {
    OperationContext::workflow(Default::default(), Interaction::Foreground)
        .child(Duration::from_secs(2))
}
#[test]
fn a_trusted_outdated_service_is_refreshed_once_then_the_new_peer_is_checked() {
    let context = operation();
    let attempts = Cell::new(0);
    let updates = Cell::new(0);
    let result = connect_current(
        &context,
        || {
            attempts.set(attempts.get() + 1);
            if attempts.get() == 1 {
                Err(PermissionOutcome::HelperOutdated)
            } else {
                Ok("current")
            }
        },
        || {
            updates.set(updates.get() + 1);
            Ok(RegistrationState::Enabled)
        },
    );
    assert_eq!(result, Ok("current"));
    assert_eq!((attempts.get(), updates.get()), (2, 1));
    let attempts = Cell::new(0);
    let result: Result<(), _> = connect_current(
        &context,
        || {
            attempts.set(attempts.get() + 1);
            Err(PermissionOutcome::HelperOutdated)
        },
        || Ok(RegistrationState::Enabled),
    );
    assert_eq!(result, Err(PermissionOutcome::HelperOutdated));
    assert_eq!(attempts.get(), 2);
}
#[test]
fn untrusted_denied_and_background_services_are_not_reinstalled_as_a_fallback() {
    for outcome in [
        PermissionOutcome::HelperUntrusted,
        PermissionOutcome::Denied,
        PermissionOutcome::AuthorizationUnavailable,
    ] {
        let result: Result<(), _> = connect_current(
            &operation(),
            || Err(outcome),
            || panic!("Unexpected update"),
        );
        assert_eq!(result, Err(outcome));
    }
    let mut context = operation();
    context.interaction = Interaction::NonInteractive;
    let result: Result<(), _> = connect_current(
        &context,
        || Err(PermissionOutcome::HelperOutdated),
        || panic!("Background update"),
    );
    assert_eq!(result, Err(PermissionOutcome::HelperOutdated));
}
#[test]
fn refresh_denial_approval_and_cancellation_cannot_continue_to_file_authorization() {
    for mode in ["denied", "settings", "cancelled"] {
        let context = operation();
        let attempts = Cell::new(0);
        let result: Result<(), _> = connect_current(
            &context,
            || {
                attempts.set(attempts.get() + 1);
                Err(PermissionOutcome::HelperOutdated)
            },
            || match mode {
                "denied" => Err(PermissionOutcome::Denied),
                "settings" => Ok(RegistrationState::RequiresApproval),
                _ => {
                    context.cancel();
                    Ok(RegistrationState::Enabled)
                }
            },
        );
        assert_eq!(attempts.get(), 1);
        assert_eq!(
            result,
            Err(match mode {
                "denied" => PermissionOutcome::Denied,
                "settings" => PermissionOutcome::NeedsSystemSettings,
                _ => PermissionOutcome::Cancelled,
            })
        );
    }
}
