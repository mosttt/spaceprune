//! Bind reuse to the original Windows logon, not merely an account or the physical console.
#![allow(unsafe_code)]

use super::{LoginRevocation, NativeBinding};
use crate::{Interaction, OperationContext, PermissionOutcome};
use spaceprune_windows_elevation::LogonIdentity;
use std::{
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, LUID, WAIT_FAILED, WPARAM},
    Security::Authentication::Identity::{
        LsaFreeReturnBuffer, LsaGetLogonSessionData, SECURITY_LOGON_SESSION_DATA,
        SECURITY_LOGON_TYPE,
    },
    System::{
        LibraryLoader::GetModuleHandleW,
        RemoteDesktop::{
            NOTIFY_FOR_THIS_SESSION, WTSActive, WTSConnectState, WTSConnected, WTSDisconnected,
            WTSFreeMemory, WTSQuerySessionInformationW, WTSRegisterSessionNotification, WTSShadow,
            WTSUnRegisterSessionNotification,
        },
    },
    UI::WindowsAndMessaging::*,
};

pub(super) fn bind(
    operation: &OperationContext,
    revocation: LoginRevocation,
) -> Result<NativeBinding, PermissionOutcome> {
    let identity =
        LogonIdentity::current().map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
    let subject = format!(
        "windows-user:{}:logon:{:016x}:session:{}",
        identity.account_sid(),
        identity.authentication_id(),
        identity.session_id(),
    );
    if identity.logon_sid().is_none() {
        if !identity.is_system_process() {
            return Err(PermissionOutcome::AuthorizationUnavailable);
        }
        return Ok(NativeBinding {
            subject: format!("{subject}:process:{}", std::process::id()),
            login_bound: false,
            _monitor: Box::new(Monitor::headless(identity, revocation)?),
        });
    }
    // A batch/service/network logon is still a distinct LSA identity, but not a desktop.
    // Its existing unattended access does not gain a new WTS or active-console prerequisite.
    if identity.session_id() == 0 || !interactive_logon(logon_type(identity.authentication_id())?) {
        return Ok(NativeBinding {
            subject,
            login_bound: true,
            _monitor: Box::new(Monitor::headless(identity, revocation)?),
        });
    }
    let monitor = Monitor::start(
        identity,
        operation.interaction != Interaction::NonInteractive,
        operation,
        revocation,
    )?;
    Ok(NativeBinding {
        subject,
        login_bound: true,
        _monitor: Box::new(monitor),
    })
}

fn logon_type(authentication_id: u64) -> Result<u32, PermissionOutcome> {
    let id = LUID {
        LowPart: authentication_id as u32,
        HighPart: (authentication_id >> 32) as i32,
    };
    let mut raw = ptr::null_mut();
    // SAFETY: This queries the current token's own logon, with a writable LSA output pointer.
    if unsafe { LsaGetLogonSessionData(&id, &raw mut raw) } != 0 || raw.is_null() {
        return Err(PermissionOutcome::AuthorizationUnavailable);
    }
    // SAFETY: LSA owns this buffer until LsaFreeReturnBuffer; read only the documented prefix.
    let result = unsafe {
        if (*raw).Size as usize >= std::mem::offset_of!(SECURITY_LOGON_SESSION_DATA, LogonType) + 4
        {
            Ok((*raw).LogonType)
        } else {
            Err(PermissionOutcome::AuthorizationUnavailable)
        }
    };
    // SAFETY: This is the allocation returned by LSA above, released exactly once.
    unsafe { LsaFreeReturnBuffer(raw.cast()) };
    result
}

fn interactive_logon(kind: u32) -> bool {
    [
        SECURITY_LOGON_TYPE::Interactive,
        SECURITY_LOGON_TYPE::Unlock,
        SECURITY_LOGON_TYPE::NewCredentials,
        SECURITY_LOGON_TYPE::RemoteInteractive,
        SECURITY_LOGON_TYPE::CachedInteractive,
        SECURITY_LOGON_TYPE::CachedRemoteInteractive,
        SECURITY_LOGON_TYPE::CachedUnlock,
    ]
    .iter()
    .any(|value| value.0 as u32 == kind)
}

fn active(session: u32, foreground: bool) -> Result<bool, PermissionOutcome> {
    let mut buffer = ptr::null_mut();
    let mut bytes = 0;
    // SAFETY: The local-session query writes a typed DWORD buffer, freed below with WTSFreeMemory.
    if unsafe {
        WTSQuerySessionInformationW(
            ptr::null_mut(),
            session,
            WTSConnectState,
            &raw mut buffer,
            &raw mut bytes,
        )
    } == 0
    {
        return Err(PermissionOutcome::AuthorizationUnavailable);
    }
    let result = if buffer.is_null() || bytes as usize != std::mem::size_of::<i32>() {
        Err(PermissionOutcome::AuthorizationUnavailable)
    } else {
        // SAFETY: The size above is exactly the WTS_CONNECTSTATE_CLASS output layout.
        let state = unsafe { buffer.cast::<i32>().read_unaligned() };
        Ok(if foreground {
            state == WTSActive
        } else {
            [WTSActive, WTSConnected, WTSDisconnected, WTSShadow].contains(&state)
        })
    };
    // SAFETY: WTS owns this returned allocation; NULL is never dereferenced.
    unsafe { WTSFreeMemory(buffer.cast()) };
    result
}

struct Monitor {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Monitor {
    fn headless(
        identity: LogonIdentity,
        revocation: LoginRevocation,
    ) -> Result<Self, PermissionOutcome> {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("spaceprune-login-token".into())
            .spawn(move || {
                while !stopped.load(Ordering::Acquire) && !revocation.is_revoked() {
                    if !LogonIdentity::current().is_ok_and(|current| current == identity)
                        || (identity.logon_sid().is_some()
                            && logon_type(identity.authentication_id()).is_err())
                    {
                        revocation.invalidate();
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            })
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }

    fn start(
        identity: LogonIdentity,
        foreground: bool,
        operation: &OperationContext,
        revocation: LoginRevocation,
    ) -> Result<Self, PermissionOutcome> {
        let session = identity.session_id();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let (ready, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("spaceprune-login-wts".into())
            .spawn(move || {
                let mut message = MSG::default();
                let state = WindowState {
                    session,
                    foreground,
                    revocation,
                };
                let window = match Window::open(&state).and_then(|window| {
                    if active(session, foreground)?
                        && LogonIdentity::current().is_ok_and(|current| current == identity)
                    {
                        Ok(window)
                    } else {
                        Err(PermissionOutcome::AuthorizationUnavailable)
                    }
                }) {
                    Ok(window) => window,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                // Drain changes queued during the initial query before exposing this binding.
                while unsafe { PeekMessageW(&raw mut message, ptr::null_mut(), 0, 0, PM_REMOVE) }
                    != 0
                {
                    if message.message == WM_QUIT {
                        return;
                    }
                    unsafe {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                if state.revocation.is_revoked() {
                    let _ = ready.send(Err(PermissionOutcome::AuthorizationUnavailable));
                    return;
                }
                if ready.send(Ok(())).is_err() {
                    return;
                }
                while !stopped.load(Ordering::Acquire) && !state.revocation.is_revoked() {
                    // SAFETY: Wait for real WTS window messages, with a finite wakeup for owner shutdown.
                    // No thread-ID message is posted on Drop, so a recycled native thread cannot be signalled.
                    let result = unsafe {
                        MsgWaitForMultipleObjectsEx(
                            0,
                            ptr::null(),
                            100,
                            QS_ALLINPUT,
                            MWMO_INPUTAVAILABLE,
                        )
                    };
                    if result == WAIT_FAILED {
                        state.revocation.invalidate();
                        break;
                    }
                    while unsafe {
                        PeekMessageW(&raw mut message, ptr::null_mut(), 0, 0, PM_REMOVE)
                    } != 0
                    {
                        if message.message == WM_QUIT {
                            state.revocation.invalidate();
                            return;
                        }
                        // SAFETY: All window callbacks are dispatched on the creating thread.
                        unsafe {
                            TranslateMessage(&message);
                            DispatchMessageW(&message);
                        }
                    }
                    if !active(session, foreground).unwrap_or(false)
                        || !LogonIdentity::current().is_ok_and(|current| current == identity)
                    {
                        state.revocation.invalidate();
                    }
                }
                drop(window);
            })
            .map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
        let monitor = Self {
            stop,
            worker: Some(worker),
        };
        loop {
            operation.check().map_err(|error| {
                if error.kind() == std::io::ErrorKind::Interrupted {
                    PermissionOutcome::Cancelled
                } else {
                    PermissionOutcome::TimedOut
                }
            })?;
            match receiver.recv_timeout(Duration::from_millis(20)) {
                Ok(result) => {
                    result?;
                    return Ok(monitor);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err(PermissionOutcome::AuthorizationUnavailable),
            }
        }
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct WindowState {
    session: u32,
    foreground: bool,
    revocation: LoginRevocation,
}
fn invalidates(event: u32, foreground: bool) -> bool {
    event == WTS_SESSION_LOGOFF
        || (foreground && matches!(event, WTS_CONSOLE_DISCONNECT | WTS_REMOTE_DISCONNECT))
}
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    event: WPARAM,
    session: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: CreateWindowEx passes a valid CREATESTRUCTW during synchronous construction.
        let create = unsafe { &*(session as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
        }
    } else if message == WM_WTSSESSION_CHANGE {
        // SAFETY: The borrowed state outlives the window and is only accessed by its owner thread.
        let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const WindowState;
        if !state.is_null()
            && unsafe { (*state).session } == session as u32
            && invalidates(event as u32, unsafe { (*state).foreground })
        {
            unsafe {
                (*state).revocation.invalidate();
            }
        }
    }
    // SAFETY: Unhandled native window messages retain their normal Windows semantics.
    unsafe { DefWindowProcW(window, message, event, session) }
}

struct Window {
    handle: HWND,
    name: Vec<u16>,
    registered: bool,
}
impl Window {
    fn open(state: &WindowState) -> Result<Self, PermissionOutcome> {
        let name = format!("SpacePrune.Login.{}\0", uuid::Uuid::new_v4())
            .encode_utf16()
            .collect::<Vec<_>>();
        // SAFETY: The process module is a borrowed handle; class/name buffers live through removal.
        let module = unsafe { GetModuleHandleW(ptr::null()) };
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: module,
            lpszClassName: name.as_ptr(),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err(PermissionOutcome::AuthorizationUnavailable);
        }
        let mut window = Self {
            handle: ptr::null_mut(),
            name,
            registered: false,
        };
        // SAFETY: This invisible message-only window borrows state until its owning thread destroys it.
        window.handle = unsafe {
            CreateWindowExW(
                0,
                window.name.as_ptr(),
                window.name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                ptr::null_mut(),
                module,
                (state as *const WindowState).cast(),
            )
        };
        if window.handle.is_null() {
            return Err(PermissionOutcome::AuthorizationUnavailable);
        }
        // Subscribe before the initial query so a rapid away-and-back is still latched from its event.
        window.registered =
            unsafe { WTSRegisterSessionNotification(window.handle, NOTIFY_FOR_THIS_SESSION) } != 0;
        if !window.registered {
            return Err(PermissionOutcome::AuthorizationUnavailable);
        }
        Ok(window)
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: Native window and class are destroyed on their creating thread, after callbacks end.
        unsafe {
            if self.registered {
                WTSUnRegisterSessionNotification(self.handle);
            }
            if !self.handle.is_null() {
                DestroyWindow(self.handle);
            }
            UnregisterClassW(self.name.as_ptr(), GetModuleHandleW(ptr::null()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn native_window_delivery_latches_only_the_original_session_departure() {
        for foreground in [true, false] {
            let calls = Arc::new(AtomicUsize::new(0));
            let notified = Arc::clone(&calls);
            let signal = LoginRevocation(Arc::new(super::super::RevocationState {
                revoked: AtomicBool::new(false),
                on_revoke: Box::new(move || {
                    notified.fetch_add(1, Ordering::AcqRel);
                }),
            }));
            let session = LogonIdentity::current().unwrap().session_id();
            let state = WindowState {
                session,
                foreground,
                revocation: signal.clone(),
            };
            let window = Window::open(&state).unwrap();
            let event = |reason: u32, session: u32| {
                // Deliver to this test-owned window only; do not change any actual user session.
                unsafe {
                    SendMessageW(
                        window.handle,
                        WM_WTSSESSION_CHANGE,
                        reason as usize,
                        session as isize,
                    );
                }
            };
            event(WTS_CONSOLE_DISCONNECT, session.wrapping_add(1));
            event(WTS_SESSION_LOCK, session);
            event(WTS_SESSION_UNLOCK, session);
            assert_eq!(calls.load(Ordering::Acquire), 0);
            event(WTS_REMOTE_DISCONNECT, session);
            assert_eq!(signal.is_revoked(), foreground);
            event(WTS_REMOTE_CONNECT, session);
            assert_eq!(signal.is_revoked(), foreground);
            event(WTS_SESSION_LOGOFF, session);
            assert!(signal.is_revoked());
            assert_eq!(calls.load(Ordering::Acquire), 1);
        }
    }
}
