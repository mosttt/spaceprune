//! Legacy installation uses the same signed component and native system authorization rights.
#![allow(unsafe_code)]
use super::*;
use core_foundation::{
    base::{Boolean, TCFType},
    dictionary::CFDictionaryRef,
    error::{CFError, CFErrorRef},
    string::{CFString, CFStringRef},
};
use std::os::unix::fs::{MetadataExt, PermissionsExt};

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {
    static kSMDomainSystemLaunchd: CFStringRef;
    fn SMJobBless(
        domain: CFStringRef,
        label: CFStringRef,
        authorization: security_framework_sys::authorization::AuthorizationRef,
        error: *mut CFErrorRef,
    ) -> Boolean;
    fn SMJobRemove(
        domain: CFStringRef,
        label: CFStringRef,
        authorization: security_framework_sys::authorization::AuthorizationRef,
        wait: Boolean,
        error: *mut CFErrorRef,
    ) -> Boolean;
    fn SMCopyJobDictionary(domain: CFStringRef, label: CFStringRef) -> CFDictionaryRef;
}

pub(in crate::macos) struct Service {
    pub(super) kind: ServiceKind,
    pub(super) team: String,
    pub(super) bundled_hash: Vec<u8>,
}
impl Service {
    pub(super) fn status(&self) -> Result<RegistrationState, PermissionOutcome> {
        let path = self.kind.legacy_executable();
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RegistrationState::NotRegistered);
            }
            _ => return Err(PermissionOutcome::HelperUntrusted),
        }
        for ancestor in path.ancestors() {
            let metadata = std::fs::symlink_metadata(ancestor)
                .map_err(|_| PermissionOutcome::HelperUntrusted)?;
            if metadata.file_type().is_symlink()
                || metadata.uid() != 0
                || metadata.permissions().mode() & 0o022 != 0
            {
                return Err(PermissionOutcome::HelperUntrusted);
            }
        }
        // Detect trusted old code even when it exits before accepting a connection.
        // A live peer is still checked separately after registration/update completes.
        let installed_hash = signing::verify_file(&path, &self.team, self.kind.label())?;
        let label = CFString::new(self.kind.label());
        // SAFETY: Both arguments are retained CFString values; Copy returns an owned dictionary or NULL.
        let job =
            unsafe { SMCopyJobDictionary(kSMDomainSystemLaunchd, label.as_concrete_TypeRef()) };
        if job.is_null() {
            return Ok(RegistrationState::NotRegistered);
        }
        // SAFETY: We only release the retained object returned by SMCopyJobDictionary.
        unsafe {
            core_foundation::base::CFRelease(job.cast());
        }
        Ok(if installed_hash == self.bundled_hash {
            RegistrationState::Enabled
        } else {
            RegistrationState::Outdated
        })
    }
    pub(super) fn register(&self, context: &OperationContext) -> Result<(), PermissionOutcome> {
        self.bless(&authorization::Authorization::bless_right(context)?)
    }
    pub(super) fn refresh(&self, context: &OperationContext) -> Result<(), PermissionOutcome> {
        let authorization = authorization::Authorization::refresh_rights(context)?;
        let label = CFString::new(self.kind.label());
        let mut error = std::ptr::null_mut();
        // SAFETY: Fixed installed label and both system rights belong to this verified agent.
        // The host supervises this process while ServiceManagement waits for the old job to exit.
        let removed = unsafe {
            SMJobRemove(
                kSMDomainSystemLaunchd,
                label.as_concrete_TypeRef(),
                authorization.raw(),
                1,
                &mut error,
            )
        };
        if !error.is_null() {
            // SAFETY: SMJobRemove returned an owned CFError on failure.
            drop(unsafe { CFError::wrap_under_create_rule(error) });
        }
        if removed == 0 {
            return Err(PermissionOutcome::AuthorizationUnavailable);
        }
        self.bless(&authorization)
    }
    fn bless(&self, authorization: &authorization::Authorization) -> Result<(), PermissionOutcome> {
        let label = CFString::new(self.kind.label());
        let mut error = std::ptr::null_mut();
        // SAFETY: The fixed label and native authorization reference live through the call.
        let success = unsafe {
            SMJobBless(
                kSMDomainSystemLaunchd,
                label.as_concrete_TypeRef(),
                authorization.raw(),
                &mut error,
            )
        };
        if !error.is_null() {
            // SAFETY: SMJobBless returns a retained CFError on failure.
            drop(unsafe { CFError::wrap_under_create_rule(error) });
        }
        if success != 0 {
            Ok(())
        } else {
            Err(PermissionOutcome::AuthorizationUnavailable)
        }
    }
}
