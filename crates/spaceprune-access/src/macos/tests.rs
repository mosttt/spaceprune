use super::*;
use std::cell::Cell;

struct Service {
    state: Cell<RegistrationState>,
    after_registration: RegistrationState,
    registrations: Cell<usize>,
    refreshes: Cell<usize>,
    settings: Cell<usize>,
    failure: Option<PermissionOutcome>,
}
impl Service {
    fn new(state: RegistrationState, after_registration: RegistrationState) -> Self {
        Self {
            state: Cell::new(state),
            after_registration,
            registrations: Cell::new(0),
            refreshes: Cell::new(0),
            settings: Cell::new(0),
            failure: None,
        }
    }
}
impl RegistrationBackend for Service {
    fn status(&self) -> Result<RegistrationState, PermissionOutcome> {
        Ok(self.state.get())
    }
    fn register(&self, _: &OperationContext) -> Result<(), PermissionOutcome> {
        self.registrations.set(self.registrations.get() + 1);
        if let Some(outcome) = self.failure {
            return Err(outcome);
        }
        self.state.set(self.after_registration);
        Ok(())
    }
    fn refresh(&self, context: &OperationContext) -> Result<(), PermissionOutcome> {
        self.refreshes.set(self.refreshes.get() + 1);
        self.register(context)
    }
    fn open_settings(&self) -> Result<(), PermissionOutcome> {
        self.settings.set(self.settings.get() + 1);
        Ok(())
    }
}

#[test]
fn registration_rechecks_os_state_and_never_promotes_approval_to_enabled() {
    let context = OperationContext::workflow(Default::default(), Interaction::Foreground);
    let service = Service::new(
        RegistrationState::NotRegistered,
        RegistrationState::RequiresApproval,
    );
    assert_eq!(
        ensure_with(&service, &context),
        Ok(RegistrationState::RequiresApproval)
    );
    assert_eq!(service.registrations.get(), 1);
    assert_eq!(service.settings.get(), 1);
    service.state.set(RegistrationState::Enabled);
    assert_eq!(
        ensure_with(&service, &context),
        Ok(RegistrationState::Enabled)
    );
    assert_eq!(service.registrations.get(), 1);
    let broken = Service::new(
        RegistrationState::NotRegistered,
        RegistrationState::NotFound,
    );
    assert_eq!(
        ensure_with(&broken, &context),
        Err(PermissionOutcome::AuthorizationUnavailable)
    );
}

#[test]
fn noninteractive_and_cancelled_operations_do_not_launch_native_interaction() {
    let context = OperationContext::workflow(Default::default(), Interaction::NonInteractive);
    let service = Service::new(RegistrationState::NotRegistered, RegistrationState::Enabled);
    assert_eq!(
        ensure_with(&service, &context),
        Err(PermissionOutcome::NeedsAuthorization)
    );
    service.state.set(RegistrationState::RequiresApproval);
    assert_eq!(
        ensure_with(&service, &context),
        Ok(RegistrationState::RequiresApproval)
    );
    assert_eq!(service.registrations.get(), 0);
    assert_eq!(service.settings.get(), 0);
    context.cancel();
    assert_eq!(
        ensure_with(&service, &context),
        Err(PermissionOutcome::Cancelled)
    );
}

#[test]
fn native_refusal_is_preserved_without_retrying_or_opening_settings() {
    let context = OperationContext::workflow(Default::default(), Interaction::Terminal);
    let mut service = Service::new(RegistrationState::NotRegistered, RegistrationState::Enabled);
    service.failure = Some(PermissionOutcome::Denied);
    assert_eq!(
        ensure_with(&service, &context),
        Err(PermissionOutcome::Denied)
    );
    assert_eq!(service.registrations.get(), 1);
    assert_eq!(service.settings.get(), 0);

    let waiting = Service::new(
        RegistrationState::RequiresApproval,
        RegistrationState::Enabled,
    );
    assert_eq!(
        ensure_with(&waiting, &context),
        Ok(RegistrationState::RequiresApproval)
    );
    assert_eq!(
        (waiting.registrations.get(), waiting.settings.get()),
        (0, 0)
    );
}

#[test]
fn requirements_are_fixed_and_cannot_inject_a_different_publisher_or_program() {
    for kind in [ServiceKind::Metadata, ServiceKind::Cleanup] {
        let requirement = signing_requirement("A1B2C3D4E5", kind.label()).unwrap();
        assert!(requirement.contains("anchor apple generic"));
        assert!(requirement.ends_with(&format!("identifier \"{}\"", kind.label())));
        assert!(
            kind.bundled_executable(Path::new("/Applications/Native.app"))
                .ends_with(kind.label())
        );
    }
    assert_ne!(ServiceKind::Metadata.label(), ServiceKind::Cleanup.label());
    assert_eq!(
        signing_requirement("A1B2C3D4E5\" or true", INSTALLER_IDENTIFIER),
        Err(PermissionOutcome::HelperUntrusted)
    );
    assert_eq!(
        signing_requirement("A1B2C3D4E5", "com.attacker.agent"),
        Err(PermissionOutcome::HelperUntrusted)
    );
}

#[test]
fn access_rights_are_separate_fixed_capabilities() {
    assert_eq!(
        ServiceKind::Metadata.authorization_right().to_bytes(),
        b"com.spaceprune.metadata.read"
    );
    assert_eq!(
        ServiceKind::Cleanup.authorization_right().to_bytes(),
        b"com.spaceprune.cleanup.execute"
    );
}

#[test]
fn refreshing_a_service_preserves_required_system_approval_and_never_loops() {
    let context = OperationContext::workflow(Default::default(), Interaction::Foreground);
    let waiting = Service::new(
        RegistrationState::RequiresApproval,
        RegistrationState::Enabled,
    );
    assert_eq!(
        ensure_registration(&waiting, &context, true),
        Ok(RegistrationState::RequiresApproval)
    );
    assert_eq!(
        (waiting.registrations.get(), waiting.settings.get()),
        (0, 1)
    );
    let current = Service::new(RegistrationState::Enabled, RegistrationState::Enabled);
    assert_eq!(
        ensure_registration(&current, &context, true),
        Ok(RegistrationState::Enabled)
    );
    assert_eq!(current.registrations.get(), 1);
    let failed = Service::new(RegistrationState::Enabled, RegistrationState::NotFound);
    assert_eq!(
        ensure_registration(&failed, &context, true),
        Err(PermissionOutcome::AuthorizationUnavailable)
    );
    assert_eq!(failed.registrations.get(), 1);
}

#[test]
fn installed_old_code_is_updated_before_attempting_to_connect_to_a_broken_daemon() {
    let context = OperationContext::workflow(Default::default(), Interaction::Foreground);
    let service = Service::new(RegistrationState::Outdated, RegistrationState::Enabled);
    assert_eq!(
        ensure_with(&service, &context),
        Ok(RegistrationState::Enabled)
    );
    assert_eq!(
        (service.registrations.get(), service.refreshes.get()),
        (1, 1)
    );

    let unchanged = Service::new(RegistrationState::Outdated, RegistrationState::Outdated);
    assert_eq!(
        ensure_with(&unchanged, &context),
        Err(PermissionOutcome::HelperOutdated)
    );
    assert_eq!(unchanged.refreshes.get(), 1);

    let background = OperationContext::workflow(Default::default(), Interaction::NonInteractive);
    let service = Service::new(RegistrationState::Outdated, RegistrationState::Enabled);
    assert_eq!(
        ensure_with(&service, &background),
        Err(PermissionOutcome::HelperOutdated)
    );
    assert_eq!(
        (service.registrations.get(), service.refreshes.get()),
        (0, 0)
    );
}
