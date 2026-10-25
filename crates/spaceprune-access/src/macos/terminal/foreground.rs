//! Job control uses only terminal attributes; SpacePrune never reads terminal input.
#![allow(unsafe_code)]

use std::{
    fs::{File, OpenOptions},
    io,
    os::fd::AsRawFd,
};

pub(super) struct Foreground {
    terminal: File,
    original_group: libc::pid_t,
    agent_group: libc::pid_t,
    attributes: libc::termios,
    claimed: bool,
}

impl Foreground {
    pub(super) fn claim(agent: u32) -> io::Result<Self> {
        let terminal = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        let fd = terminal.as_raw_fd();
        let agent_group = i32::try_from(agent).map_err(|_| io::ErrorKind::InvalidInput)?;
        // SAFETY: These queries use a live controlling-terminal FD and process IDs.
        let original_group = unsafe { libc::tcgetpgrp(fd) };
        if original_group <= 0
            || original_group != unsafe { libc::getpgrp() }
            || unsafe { libc::getpgid(agent_group) } != agent_group
        {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let mut attributes = std::mem::MaybeUninit::uninit();
        // SAFETY: tcgetattr initializes the public termios buffer on success.
        if unsafe { libc::tcgetattr(fd, attributes.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut guard = Self {
            terminal,
            original_group,
            agent_group,
            attributes: unsafe { attributes.assume_init() },
            claimed: false,
        };
        with_job_control(|| {
            // SAFETY: The child is the verified agent's newly created process group in this session.
            if unsafe { libc::tcsetpgrp(fd, agent_group) } == 0 {
                guard.claimed = true;
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        })?;
        Ok(guard)
    }
}

impl Drop for Foreground {
    fn drop(&mut self) {
        if !self.claimed {
            return;
        }
        let fd = self.terminal.as_raw_fd();
        let _ = with_job_control(|| {
            // SAFETY: Restore only the group we owned, after all agent children have stopped.
            let current = unsafe { libc::tcgetpgrp(fd) };
            if current == self.agent_group {
                if unsafe { libc::tcsetpgrp(fd, self.original_group) } != 0 {
                    return Err(io::Error::last_os_error());
                }
            } else if current != self.original_group {
                return Ok(());
            }
            // SAFETY: Saved attributes belong to this terminal. Flush pending secret keystrokes.
            if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &self.attributes) } == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
    }
}

fn with_job_control(action: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
    let mut blocked = std::mem::MaybeUninit::uninit();
    let mut previous = std::mem::MaybeUninit::uninit();
    // SAFETY: Only this thread temporarily blocks SIGTTOU while reclaiming its foreground group.
    unsafe {
        libc::sigemptyset(blocked.as_mut_ptr());
        let mut blocked = blocked.assume_init();
        libc::sigaddset(&mut blocked, libc::SIGTTOU);
        let status = libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, previous.as_mut_ptr());
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status));
        }
        let previous = previous.assume_init();
        let result = action();
        let restored = libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
        if restored != 0 {
            return Err(io::Error::from_raw_os_error(restored));
        }
        result
    }
}
