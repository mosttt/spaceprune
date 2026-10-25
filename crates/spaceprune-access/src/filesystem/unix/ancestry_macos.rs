//! Observe parent generations and retain vnode invalidation across an atomic capture.
#![allow(unsafe_code)]

use super::*;
use std::{
    mem::MaybeUninit,
    os::fd::{AsRawFd, FromRawFd},
};

const CHILDREN_CHANGED: u32 =
    libc::NOTE_WRITE | libc::NOTE_EXTEND | libc::NOTE_ATTRIB | libc::NOTE_LINK;
const ANCESTOR_CHANGED: u32 = libc::NOTE_RENAME | libc::NOTE_DELETE | libc::NOTE_REVOKE;
const _: () = assert!(
    ancestry_generation::COMMON_ATTRIBUTES
        == libc::ATTR_CMN_RETURNED_ATTRS | libc::ATTR_CMN_GEN_COUNT
);

pub(super) struct Guard<'a> {
    parents: &'a [OwnedFd],
    generations: Vec<u32>,
    events: OwnedFd,
}

fn unavailable(error: impl Into<io::Error>) -> io::Error {
    crate::error::failure_with_cause(
        IoFailureKind::Unsupported,
        messages::text("native.ancestry_monitor"),
        &error.into(),
    )
}

fn ancestry_changed() -> io::Error {
    failure(
        IoFailureKind::TargetChanged,
        messages::text("native.ancestry_changed"),
    )
}

impl<'a> Guard<'a> {
    pub(super) fn watch(
        chain: &'a [OwnedFd],
        names: &[&OsStr],
        context: &OperationContext,
    ) -> io::Result<Option<Self>> {
        if names.is_empty() {
            return Ok(None);
        }
        // SAFETY: kqueue takes no arguments and returns a new owned descriptor on success.
        let raw = unsafe { libc::kqueue() };
        if raw < 0 {
            return Err(unavailable(io::Error::last_os_error()));
        }
        // SAFETY: The successful kqueue descriptor has no other owner.
        let events = unsafe { OwnedFd::from_raw_fd(raw) };
        rustix::io::fcntl_setfd(&events, rustix::io::FdFlags::CLOEXEC).map_err(unavailable)?;
        let parents = &chain[..chain.len() - 1];
        for (index, directory) in chain.iter().enumerate() {
            context.check()?;
            // For root/A/B/file, root and A observe all direct-child changes, including
            // unrelated sibling renames that could wrap a generation counter. A and B also
            // observe their own removal/movement. B's children include our staging, so B must
            // not subscribe to child changes. The root remains an identity anchor, not a path.
            let flags = if index < parents.len() {
                CHILDREN_CHANGED
            } else {
                0
            } | if index > 0 { ANCESTOR_CHANGED } else { 0 };
            subscribe(&events, directory, flags)?;
        }
        let generations = parents
            .iter()
            .map(|parent| {
                context.check()?;
                generation(parent)
            })
            .collect::<io::Result<Vec<_>>>()?;
        // Register before sampling and revalidating the names. Do not drain events or reset
        // the baseline: a move away and back must remain visible for this selection.
        validate_ancestry(chain, names, context)?;
        Ok(Some(Self {
            parents,
            generations,
            events,
        }))
    }

    pub(super) fn after_capture(&self, context: &OperationContext) -> io::Result<()> {
        // HFS updates parent generations inside the same cnode lock used by getattr. This
        // observes a namespace change even before the subsequent XNU vnode notification.
        // APFS ordering and event coverage remain part of the native acceptance gate.
        for (parent, expected) in self.parents.iter().zip(&self.generations) {
            context.check()?;
            if generation(parent)? != *expected {
                return Err(ancestry_changed());
            }
        }
        context.check()?;
        check_events(&self.events)
    }
}

fn generation(directory: &OwnedFd) -> io::Result<u32> {
    let mut attributes = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT as _,
        reserved: 0,
        commonattr: ancestry_generation::COMMON_ATTRIBUTES,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut reply: ancestry_generation::Reply = [0; 7];
    // SAFETY: The borrowed FD stays open; attrlist and the aligned reply are writable for
    // their stated sizes. Only returned, supported attributes are decoded after success.
    let status = unsafe {
        libc::fgetattrlist(
            directory.as_raw_fd(),
            std::ptr::from_mut(&mut attributes).cast(),
            reply.as_mut_ptr().cast(),
            std::mem::size_of_val(&reply),
            libc::FSOPT_ATTR_CMN_EXTENDED,
        )
    };
    if status < 0 {
        let error = io::Error::last_os_error();
        return Err(match error.raw_os_error() {
            Some(libc::EINVAL | libc::ENOTSUP | libc::ENOSYS) => unavailable(error),
            _ => error,
        });
    }
    ancestry_generation::decode(&reply)
}

fn subscribe(events: &OwnedFd, directory: &OwnedFd, flags: u32) -> io::Result<()> {
    let change = libc::kevent {
        ident: directory.as_raw_fd() as _,
        filter: libc::EVFILT_VNODE,
        flags: libc::EV_ADD | libc::EV_ENABLE,
        fflags: flags,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    // SAFETY: One fully initialized change is submitted. No output events are requested,
    // so installing a later watch cannot consume an earlier watch's invalidation.
    let status = unsafe {
        libc::kevent(
            events.as_raw_fd(),
            &change,
            1,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
        )
    };
    if status < 0 {
        Err(unavailable(io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

fn check_events(events: &OwnedFd) -> io::Result<()> {
    let mut event = MaybeUninit::<libc::kevent>::uninit();
    let timeout = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: There are no changes; one writable event slot and a valid zero timeout are
    // supplied. The slot is read only when kevent reports that it initialized one event.
    let count = unsafe {
        libc::kevent(
            events.as_raw_fd(),
            std::ptr::null(),
            0,
            event.as_mut_ptr(),
            1,
            &timeout,
        )
    };
    match count {
        0 => Ok(()),
        -1 => Err(unavailable(io::Error::last_os_error())),
        _ => {
            // SAFETY: kevent returned an event into the single supplied slot.
            let event = unsafe { event.assume_init() };
            if event.flags & libc::EV_ERROR != 0 {
                Err(unavailable(io::Error::from_raw_os_error(event.data as i32)))
            } else {
                // Includes movement, revocation/EOF and any direct-child invalidation.
                Err(ancestry_changed())
            }
        }
    }
}

#[cfg(test)]
#[path = "ancestry_macos_tests.rs"]
mod tests;
