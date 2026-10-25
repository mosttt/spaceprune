//! Interactive authorization belongs to the killable agent; services only recheck OS policy.
#![allow(unsafe_code)]

use super::{ServiceKind, credential::ExternalAuthorization};
use crate::{Interaction, OperationContext, PermissionOutcome};
use core_foundation::{base::TCFType, dictionary::CFDictionary, string::CFString};
use security_framework_sys::authorization as sys;
use std::ptr;

pub(super) struct Authorization(sys::AuthorizationRef);

impl Authorization {
    pub(super) fn bless_right(context: &OperationContext) -> Result<Self, PermissionOutcome> {
        Self::request(&[c"com.apple.ServiceManagement.blesshelper"], context)
    }
    pub(super) fn refresh_rights(context: &OperationContext) -> Result<Self, PermissionOutcome> {
        Self::request(
            &[
                c"com.apple.ServiceManagement.blesshelper",
                c"com.apple.ServiceManagement.daemons.modify",
            ],
            context,
        )
    }

    fn request(
        rights: &[&std::ffi::CStr],
        context: &OperationContext,
    ) -> Result<Self, PermissionOutcome> {
        super::check(context)?;
        let authorization = Self::empty()?;
        let flags = sys::kAuthorizationFlagExtendRights
            | if context.interaction == Interaction::Foreground {
                sys::kAuthorizationFlagInteractionAllowed
            } else {
                0
            };
        // Try actual OS credentials without prompting in Terminal/NonInteractive mode.
        // No PreAuthorize or PartialRights: success must cover every requested right.
        match authorization.copy_rights(rights, flags) {
            Ok(()) => {
                super::check(context)?;
                Ok(authorization)
            }
            Err(PermissionOutcome::NeedsAuthorization)
                if context.interaction == Interaction::Terminal =>
            {
                Self::request_terminal(rights, context)
            }
            Err(outcome) => Err(outcome),
        }
    }

    fn request_terminal(
        rights: &[&std::ffi::CStr],
        context: &OperationContext,
    ) -> Result<Self, PermissionOutcome> {
        let mut command = std::process::Command::new(super::signing::system_authenticator()?);
        // The Apple tool owns getpass and its OS environment. -w keeps its reference
        // alive until we import it; neither -u (GUI) nor -P (preauthorization) is used.
        command.args(["-q", "authorize", "-c", "-w"]);
        for right in rights {
            command.arg(right.to_str().expect("fixed native authorization right"));
        }
        crate::process::restrict_system_environment(&mut command);
        super::terminal::import_from_authenticator(command, context, |form| {
            let authorization = Self::import(form)?;
            authorization.copy_rights(rights, sys::kAuthorizationFlagDefaults)?;
            Ok(authorization)
        })
    }

    fn empty() -> Result<Self, PermissionOutcome> {
        let mut value = ptr::null_mut();
        // SAFETY: No rights or interaction are requested; the output is an owned OS reference.
        status(unsafe { sys::AuthorizationCreate(ptr::null(), ptr::null(), 0, &mut value) })?;
        if value.is_null() {
            return Err(PermissionOutcome::AuthorizationUnavailable);
        }
        Ok(Self(value))
    }

    pub(super) fn request_access(
        kind: ServiceKind,
        context: &OperationContext,
    ) -> Result<Self, PermissionOutcome> {
        Self::request(
            &[kind.authorization_right(), c"system.privilege.admin"],
            context,
        )
    }

    pub(super) fn verify_access(&self, kind: ServiceKind) -> Result<(), PermissionOutcome> {
        // This rechecks OS policy without prompting. OS credentials may authorize more than one
        // right; purpose isolation is enforced by the verified agent-to-service handoff.
        self.copy_right(kind, sys::kAuthorizationFlagDefaults)
    }

    fn copy_right(&self, kind: ServiceKind, flags: u32) -> Result<(), PermissionOutcome> {
        self.copy_rights(
            &[kind.authorization_right(), c"system.privilege.admin"],
            flags,
        )
    }

    fn copy_rights(&self, names: &[&std::ffi::CStr], flags: u32) -> Result<(), PermissionOutcome> {
        let mut items: Vec<_> = names
            .iter()
            .map(|name| sys::AuthorizationItem {
                name: name.as_ptr(),
                valueLength: 0,
                value: ptr::null_mut(),
                flags: 0,
            })
            .collect();
        let rights = sys::AuthorizationItemSet {
            count: items.len() as u32,
            items: items.as_mut_ptr(),
        };
        // SAFETY: The live reference and fixed rights remain valid for the call.
        // PartialRights is never requested, and no password enters this process.
        status(unsafe {
            sys::AuthorizationCopyRights(self.0, &rights, ptr::null(), flags, ptr::null_mut())
        })
    }

    pub(super) fn externalize(&self) -> Result<ExternalAuthorization, PermissionOutcome> {
        let mut form = sys::AuthorizationExternalForm { bytes: [0; 32] };
        // SAFETY: The OS fills the fixed public AuthorizationExternalForm layout.
        status(unsafe { sys::AuthorizationMakeExternalForm(self.0, &mut form) })?;
        Ok(ExternalAuthorization(form.bytes.map(|byte| byte as u8)))
    }

    pub(super) fn from_external(
        form: &ExternalAuthorization,
        kind: ServiceKind,
    ) -> Result<Self, PermissionOutcome> {
        let authorization = Self::import(form)?;
        authorization.verify_access(kind)?;
        Ok(authorization)
    }

    fn import(form: &ExternalAuthorization) -> Result<Self, PermissionOutcome> {
        let form = sys::AuthorizationExternalForm {
            bytes: form.0.map(|byte| byte as std::ffi::c_char),
        };
        let mut value = ptr::null_mut();
        // SAFETY: Import only on an authenticated connection; a successful import is not yet
        // sufficient. Recheck fixed application and administrator policy without interaction.
        status(unsafe { sys::AuthorizationCreateFromExternalForm(&form, &mut value) })?;
        if value.is_null() {
            return Err(PermissionOutcome::AuthorizationUnavailable);
        }
        Ok(Self(value))
    }

    pub(super) fn ensure_access_right(kind: ServiceKind) -> Result<(), PermissionOutcome> {
        if !rustix::process::geteuid().is_root() {
            return Err(PermissionOutcome::HelperUntrusted);
        }
        let mut definition = ptr::null();
        // SAFETY: RightGet returns a retained public policy dictionary on success. Preserve
        // existing OS/admin policy, including denial; never replace it on service startup.
        let existing = unsafe {
            sys::AuthorizationRightGet(kind.authorization_right().as_ptr(), &mut definition)
        };
        if existing == sys::errAuthorizationSuccess {
            if definition.is_null() {
                return Err(PermissionOutcome::AuthorizationUnavailable);
            }
            // SAFETY: The successful Get call owns this retained dictionary.
            drop(unsafe {
                CFDictionary::<CFString, core_foundation::base::CFType>::wrap_under_create_rule(
                    definition,
                )
            });
            return Ok(());
        }
        if existing != sys::errAuthorizationDenied {
            return status(existing);
        }
        let authorization = Self::empty()?;
        let rule = CFString::new("authenticate-admin");
        let description = CFString::new(match kind {
            ServiceKind::Metadata => {
                "SpacePrune needs administrator approval to inspect protected file metadata."
            }
            ServiceKind::Cleanup => {
                "SpacePrune needs administrator approval to clean the confirmed selection."
            }
        });
        // SAFETY: Only a missing application right gets the documented admin default. A
        // pre-created weak application rule cannot bypass the separately required built-in
        // system.privilege.admin right, which this application never modifies.
        status(unsafe {
            sys::AuthorizationRightSet(
                authorization.0,
                kind.authorization_right().as_ptr(),
                rule.as_CFTypeRef(),
                description.as_concrete_TypeRef(),
                ptr::null_mut(),
                ptr::null(),
            )
        })
    }

    pub(super) fn raw(&self) -> sys::AuthorizationRef {
        self.0
    }
}

impl Drop for Authorization {
    fn drop(&mut self) {
        // SAFETY: This object owns the live reference. OS-owned authentication caching is retained.
        unsafe {
            sys::AuthorizationFree(self.0, sys::kAuthorizationFlagDefaults);
        }
    }
}

fn status(code: i32) -> Result<(), PermissionOutcome> {
    match code {
        sys::errAuthorizationSuccess => Ok(()),
        sys::errAuthorizationDenied => Err(PermissionOutcome::Denied),
        sys::errAuthorizationCanceled => Err(PermissionOutcome::Cancelled),
        sys::errAuthorizationInteractionNotAllowed => Err(PermissionOutcome::NeedsAuthorization),
        _ => Err(PermissionOutcome::AuthorizationUnavailable),
    }
}
