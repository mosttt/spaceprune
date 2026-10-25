//! The caller's security/Quartz session is the boundary; another console user's UID is not.
#![allow(unsafe_code)]

use super::{LoginRevocation, NativeBinding};
use crate::{Interaction, OperationContext, PermissionOutcome};
use block2::RcBlock;
use core_foundation::{
    base::{CFGetTypeID, TCFType},
    boolean::{CFBoolean, CFBooleanGetTypeID, CFBooleanRef},
    dictionary::{CFDictionary, CFDictionaryGetValue, CFDictionaryRef},
    runloop::{
        CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext, CFRunLoopSourceCreate,
        kCFRunLoopDefaultMode,
    },
    string::CFStringRef,
};
use objc2::rc::autoreleasepool;
use objc2_app_kit::{
    NSWorkspace, NSWorkspaceSessionDidResignActiveNotification, NSWorkspaceWillPowerOffNotification,
};
use std::{
    fs::{File, OpenOptions},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

const ROOT_SESSION: u32 = 0x0001;
const GRAPHICS: u32 = 0x0010;
const TTY: u32 = 0x0020;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SessionGetInfo(requested: u32, session: *mut u32, attributes: *mut u32) -> i32;
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
    static kCGSessionOnConsoleKey: CFStringRef;
    static kCGSessionLoginDoneKey: CFStringRef;
}

#[derive(Clone, Copy)]
struct Session {
    id: u32,
    attributes: u32,
}
impl Session {
    fn current() -> Result<Self, PermissionOutcome> {
        let mut session = Self {
            id: 0,
            attributes: 0,
        };
        // SAFETY: The public Security API fills the two correctly sized outputs for this caller.
        if unsafe { SessionGetInfo(u32::MAX, &raw mut session.id, &raw mut session.attributes) }
            != 0
        {
            return Err(PermissionOutcome::AuthorizationUnavailable);
        }
        Ok(session)
    }
}

pub(super) fn bind(
    operation: &OperationContext,
    revocation: LoginRevocation,
) -> Result<NativeBinding, PermissionOutcome> {
    let session = Session::current()?;
    let subject = format!(
        "macos-uid:{}:ruid:{}:audit-session:{}",
        rustix::process::geteuid().as_raw(),
        rustix::process::getuid().as_raw(),
        session.id
    );
    if session.attributes & ROOT_SESSION != 0 {
        return Ok(NativeBinding {
            subject: format!("{subject}:process:{}", std::process::id()),
            login_bound: false,
            _monitor: Box::new(()),
        });
    }
    let graphic = session.attributes & GRAPHICS != 0;
    let tty = if !graphic
        && operation.interaction == Interaction::Terminal
        && session.attributes & TTY != 0
    {
        match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open("/dev/tty")
        {
            Ok(tty) => Some(tty),
            // A detached process can inherit sessionHasTTY without a controlling terminal.
            // Preserve its real audit-session binding; absence is not a live SSH connection.
            Err(error) if matches!(error.raw_os_error(), Some(libc::ENXIO | libc::ENODEV)) => None,
            Err(_) => return Err(PermissionOutcome::AuthorizationUnavailable),
        }
    } else {
        None
    };
    let monitor = Monitor::start(session, graphic, tty, operation, revocation)?;
    Ok(NativeBinding {
        subject,
        login_bound: true,
        _monitor: Box::new(monitor),
    })
}

fn graphics_active() -> Result<bool, PermissionOutcome> {
    // SAFETY: The retained dictionary describes this caller's Quartz session, not a global console UID.
    let raw = unsafe { CGSessionCopyCurrentDictionary() };
    if raw.is_null() {
        return Err(PermissionOutcome::AuthorizationUnavailable);
    }
    let dictionary = unsafe {
        CFDictionary::<*const std::ffi::c_void, *const std::ffi::c_void>::wrap_under_create_rule(
            raw,
        )
    };
    let value = |key: CFStringRef| {
        // SAFETY: The dictionary owns its values and each key is a public CoreGraphics constant.
        let raw = unsafe { CFDictionaryGetValue(dictionary.as_concrete_TypeRef(), key.cast()) };
        if raw.is_null() || unsafe { CFGetTypeID(raw) != CFBooleanGetTypeID() } {
            Err(PermissionOutcome::AuthorizationUnavailable)
        } else {
            Ok(bool::from(unsafe {
                CFBoolean::wrap_under_get_rule(raw as CFBooleanRef)
            }))
        }
    };
    Ok(value(unsafe { kCGSessionOnConsoleKey })? && value(unsafe { kCGSessionLoginDoneKey })?)
}

struct Monitor {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Monitor {
    fn start(
        session: Session,
        graphic: bool,
        tty: Option<File>,
        operation: &OperationContext,
        revocation: LoginRevocation,
    ) -> Result<Self, PermissionOutcome> {
        let foreground = operation.interaction != Interaction::NonInteractive;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let (ready, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("spaceprune-login-macos".into())
            .spawn(move || {
                autoreleasepool(|_| {
                    if graphic {
                        observe_graphics(session, foreground, &stopped, &revocation, ready);
                    } else {
                        if !same_session(session)
                            || tty.as_ref().is_some_and(|file| !tty_alive(file, 0))
                        {
                            let _ = ready.send(Err(PermissionOutcome::AuthorizationUnavailable));
                            return;
                        }
                        let _ = ready.send(Ok(()));
                        while !stopped.load(Ordering::Acquire) && !revocation.is_revoked() {
                            let alive = tty.as_ref().map_or_else(
                                || {
                                    thread::sleep(Duration::from_millis(100));
                                    true
                                },
                                |file| tty_alive(file, 100),
                            );
                            if !alive || !same_session(session) {
                                revocation.invalidate();
                                break;
                            }
                        }
                    }
                });
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

fn same_session(original: Session) -> bool {
    Session::current().is_ok_and(|current| {
        current.id == original.id
            && current.attributes & ROOT_SESSION == 0
            && current.attributes & GRAPHICS == original.attributes & GRAPHICS
    })
}

fn tty_alive(tty: &File, timeout_ms: i32) -> bool {
    let mut descriptor = libc::pollfd {
        fd: tty.as_raw_fd(),
        events: 0,
        revents: 0,
    };
    // SAFETY: Poll only hangup/error events on an owned controlling-TTY descriptor. Never read input.
    let result = unsafe { libc::poll(&raw mut descriptor, 1, timeout_ms) };
    if result < 0 {
        return std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted;
    }
    descriptor.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) == 0
}

fn observe_graphics(
    session: Session,
    foreground: bool,
    stop: &AtomicBool,
    revocation: &LoginRevocation,
    ready: mpsc::SyncSender<Result<(), PermissionOutcome>>,
) {
    let workspace = NSWorkspace::sharedWorkspace();
    let center = workspace.notificationCenter();
    let departed = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&departed);
    // NSWorkspace may post on an app thread. Only latch there; native teardown runs on this monitor.
    let callback = RcBlock::new(move |_| signal.store(true, Ordering::Release));
    // SAFETY: The copied block captures only a Send+Sync atomic flag. Tokens and center
    // remain on this monitor thread until the registrations are removed below.
    let observers = unsafe {
        [
            foreground.then_some(NSWorkspaceSessionDidResignActiveNotification),
            Some(NSWorkspaceWillPowerOffNotification),
        ]
        .into_iter()
        .flatten()
        .map(|name| {
            center.addObserverForName_object_queue_usingBlock(
                Some(name),
                Some(&workspace),
                None,
                &callback,
            )
        })
        .collect::<Vec<_>>()
    };
    let result = (|| {
        if (foreground && !graphics_active()?)
            || !same_session(session)
            || departed.load(Ordering::Acquire)
        {
            return Err(PermissionOutcome::AuthorizationUnavailable);
        }
        let run_loop = CFRunLoop::get_current();
        let source = keep_alive_source()?;
        let mode = unsafe { kCFRunLoopDefaultMode };
        run_loop.add_source(&source, mode);
        let listening = ready.send(Ok(())).is_ok();
        while listening && !stop.load(Ordering::Acquire) && !revocation.is_revoked() {
            // The real workspace notification latches even when switch-out and switch-back
            // happen between snapshots. Rechecking identity also catches SessionCreate changes.
            if departed.load(Ordering::Acquire)
                || !same_session(session)
                || (foreground && !graphics_active().unwrap_or(false))
            {
                revocation.invalidate();
                break;
            }
            CFRunLoop::run_in_mode(mode, Duration::from_millis(100), false);
        }
        run_loop.remove_source(&source, mode);
        Ok(())
    })();
    if let Err(error) = result {
        let _ = ready.send(Err(error));
    }
    for observer in observers {
        // SAFETY: This removes exactly the registrations created on the same retained center.
        unsafe {
            center.removeObserver((*observer).as_ref());
        }
    }
}

fn keep_alive_source() -> Result<CFRunLoopSource, PermissionOutcome> {
    extern "C" fn perform(_: *const std::ffi::c_void) {}
    let mut context = CFRunLoopSourceContext {
        version: 0,
        info: ptr::null_mut(),
        retain: None,
        release: None,
        copyDescription: None,
        equal: None,
        hash: None,
        schedule: None,
        cancel: None,
        perform,
    };
    // SAFETY: A version-zero source with no borrowed context keeps the monitor run loop alive.
    let raw = unsafe { CFRunLoopSourceCreate(ptr::null(), 0, &raw mut context) };
    if raw.is_null() {
        return Err(PermissionOutcome::AuthorizationUnavailable);
    }
    Ok(unsafe { CFRunLoopSource::wrap_under_create_rule(raw) })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_hangup_is_observed_without_consuming_input() {
        use std::{
            io::{Read, Write},
            os::fd::FromRawFd,
        };
        let mut descriptors = [0; 2];
        // A local pipe has the same POLLHUP readiness contract; no controlling terminal is touched.
        assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
        let mut reader = unsafe { File::from_raw_fd(descriptors[0]) };
        let mut writer = unsafe { File::from_raw_fd(descriptors[1]) };
        writer.write_all(b"input").unwrap();
        assert!(tty_alive(&reader, 100));
        let mut bytes = [0; 5];
        reader.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"input");
        drop(writer);
        assert!(!tty_alive(&reader, 100));
    }
}
