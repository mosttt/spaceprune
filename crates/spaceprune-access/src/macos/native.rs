//! Native APIs are runtime-selected so the macOS deployment floor is not raised to 13.
#![allow(unsafe_code)]

use super::*;
use core_foundation::bundle::CFBundle;
use objc2::{
    msg_send,
    rc::{Retained, autoreleasepool},
    runtime::{AnyClass, AnyObject, Bool},
};
use objc2_foundation::{NSError, NSString};
mod legacy;

pub(super) enum Backend {
    Modern(Retained<AnyObject>),
    Legacy(legacy::Service),
}

impl Backend {
    pub(super) fn for_current_bundle(kind: ServiceKind) -> Result<Self, PermissionOutcome> {
        let bundle = CFBundle::main_bundle()
            .path()
            .ok_or(PermissionOutcome::HelperMissing)?;
        if bundle
            .extension()
            .is_none_or(|extension| extension != "app")
        {
            return Err(PermissionOutcome::HelperMissing);
        }
        let team = signing::own_team()?;
        signing::verify_file(&bundle, &team, INSTALLER_IDENTIFIER)?;
        let executable = kind.bundled_executable(&bundle);
        let plist = bundle
            .join("Contents/Library/LaunchDaemons")
            .join(kind.plist_name());
        for path in [&executable, &plist] {
            let metadata = std::fs::symlink_metadata(path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    PermissionOutcome::HelperMissing
                } else {
                    PermissionOutcome::HelperUntrusted
                }
            })?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(PermissionOutcome::HelperUntrusted);
            }
        }
        let bundled_hash = signing::verify_file(&executable, &team, kind.label())?;
        match AnyClass::get(c"SMAppService") {
            Some(class) => autoreleasepool(|_| {
                let name = NSString::from_str(&kind.plist_name());
                // SAFETY: Availability was checked dynamically; Apple's class method returns SMAppService*.
                let service: Option<Retained<AnyObject>> =
                    unsafe { msg_send![class, daemonServiceWithPlistName: &*name] };
                service
                    .map(Self::Modern)
                    .ok_or(PermissionOutcome::AuthorizationUnavailable)
            }),
            None => Ok(Self::Legacy(legacy::Service {
                kind,
                team,
                bundled_hash,
            })),
        }
    }
}

impl RegistrationBackend for Backend {
    fn status(&self) -> Result<RegistrationState, PermissionOutcome> {
        match self {
            Self::Modern(service) => {
                // SAFETY: This retained object was created by SMAppService; status is NSInteger.
                let status: isize = unsafe { msg_send![&**service, status] };
                match status {
                    0 => Ok(RegistrationState::NotRegistered),
                    1 => Ok(RegistrationState::Enabled),
                    2 => Ok(RegistrationState::RequiresApproval),
                    3 => Ok(RegistrationState::NotFound),
                    _ => Err(PermissionOutcome::AuthorizationUnavailable),
                }
            }
            Self::Legacy(service) => service.status(),
        }
    }

    fn register(&self, context: &OperationContext) -> Result<(), PermissionOutcome> {
        match self {
            Self::Modern(service) => autoreleasepool(|_| {
                let mut error: *mut NSError = std::ptr::null_mut();
                // SAFETY: The live SMAppService object implements this BOOL/NSError** method.
                let success: Bool =
                    unsafe { msg_send![&**service, registerAndReturnError: &mut error] };
                if success.as_bool() {
                    Ok(())
                } else {
                    // LaunchDeniedByUser can leave a registered daemon requiring Settings approval.
                    // Registration state is not an access grant; the client still verifies the live peer.
                    match self.status()? {
                        RegistrationState::Enabled | RegistrationState::RequiresApproval => Ok(()),
                        _ => Err(PermissionOutcome::AuthorizationUnavailable),
                    }
                }
            }),
            Self::Legacy(service) => service.register(context),
        }
    }

    fn refresh(&self, context: &OperationContext) -> Result<(), PermissionOutcome> {
        match self {
            Self::Modern(service) => {
                autoreleasepool(|_| {
                    let mut error: *mut NSError = std::ptr::null_mut();
                    // SAFETY: The verified bundle created this SMAppService. Only its fixed job is removed.
                    let removed: Bool =
                        unsafe { msg_send![&**service, unregisterAndReturnError: &mut error] };
                    if removed.as_bool() {
                        Ok(())
                    } else {
                        Err(PermissionOutcome::AuthorizationUnavailable)
                    }
                })?;
                self.register(context)
            }
            Self::Legacy(service) => service.refresh(context),
        }
    }

    fn open_settings(&self) -> Result<(), PermissionOutcome> {
        if let Self::Modern(_) = self {
            let class = AnyClass::get(c"SMAppService")
                .ok_or(PermissionOutcome::AuthorizationUnavailable)?;
            // SAFETY: The class is available, and the documented class method returns void.
            unsafe {
                let _: () = msg_send![class, openSystemSettingsLoginItems];
            }
        }
        Ok(())
    }
}
