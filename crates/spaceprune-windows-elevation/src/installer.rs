//! Windows Installer runs the signed package transaction; the calling process stays unelevated.
use crate::SignedFile;
use std::{
    ffi::c_void,
    os::windows::ffi::OsStrExt,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
};
use windows_sys::Win32::{
    Foundation::{ERROR_INSTALL_FAILURE, ERROR_INSTALL_USEREXIT, ERROR_INVALID_NAME},
    System::ApplicationInstallationAndServicing::{
        INSTALLLOGMODE_ACTIONDATA, INSTALLLOGMODE_ERROR, INSTALLLOGMODE_FATALEXIT,
        INSTALLLOGMODE_PROGRESS, INSTALLMESSAGE_ACTIONDATA, INSTALLMESSAGE_PROGRESS,
        INSTALLUILEVEL_NONE, INSTALLUILEVEL_UACONLY, MsiInstallProductW, MsiSetExternalUIW,
        MsiSetInternalUI,
    },
    UI::WindowsAndMessaging::{IDCANCEL, IDOK},
};

/// Install an already pinned package in a dedicated ordinary-permission worker process.
/// The caller must first match the complete package hash against its immutable build pin.
/// This changes process-global MSI UI settings and must not run in a GUI or model process.
/// Cancellation is cooperative: return only after Installer finishes or rolls back its transaction.
pub fn install_package_in_worker(
    package: &SignedFile,
    should_stop: impl Fn() -> bool + Sync,
) -> u32 {
    if should_stop() {
        return ERROR_INSTALL_USEREXIT;
    }
    let Some(path) = crate::shell_path::local_path(package.path()) else {
        return ERROR_INVALID_NAME;
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let properties: Vec<u16> = "ALLUSERS=1 REBOOT=ReallySuppress\0"
        .encode_utf16()
        .collect();
    let callback = Callback {
        should_stop: &should_stop,
    };
    // SAFETY: This isolated process owns the UI configuration. The callback and all buffers
    // stay alive until the synchronous install returns, then the callback is unregistered.
    unsafe {
        if MsiSetInternalUI(
            INSTALLUILEVEL_NONE | INSTALLUILEVEL_UACONLY,
            ptr::null_mut(),
        ) == 0
        {
            return ERROR_INSTALL_FAILURE;
        }
        MsiSetExternalUIW(
            Some(handle_message),
            (INSTALLLOGMODE_ACTIONDATA
                | INSTALLLOGMODE_PROGRESS
                | INSTALLLOGMODE_ERROR
                | INSTALLLOGMODE_FATALEXIT) as u32,
            (&raw const callback).cast(),
        );
        let result = MsiInstallProductW(path.as_ptr(), properties.as_ptr());
        MsiSetExternalUIW(None, 0, ptr::null());
        result
    }
}

struct Callback<'a> {
    should_stop: &'a (dyn Fn() -> bool + Sync),
}

unsafe extern "system" fn handle_message(context: *mut c_void, message: u32, _: *const u16) -> i32 {
    // SAFETY: install_package_in_worker registers this stack object only for its synchronous call.
    let callback = unsafe { &*context.cast::<Callback<'_>>() };
    let kind = (message & 0xff00_0000) as i32;
    if matches!(kind, INSTALLMESSAGE_ACTIONDATA | INSTALLMESSAGE_PROGRESS) {
        if catch_unwind(AssertUnwindSafe(callback.should_stop)).unwrap_or(true) {
            IDCANCEL
        } else {
            IDOK
        }
    } else {
        // Let Installer report the original failure. There is no application-owned retry dialog.
        0
    }
}
