//! Linux notification fence for unselected ancestors of a captured selection.

use super::*;
use rustix::fs::inotify::{self, CreateFlags, WatchFlags};
use spaceprune_contracts::messages;
use std::{mem::MaybeUninit, os::fd::AsRawFd};

pub(super) struct Guard<'a> {
    chain: &'a [OwnedFd],
    events: OwnedFd,
}

fn unavailable(error: impl Into<io::Error>) -> io::Error {
    crate::error::failure_with_cause(
        IoFailureKind::Unsupported,
        messages::text("native.ancestry_monitor"),
        &error.into(),
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
        let events =
            inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK).map_err(unavailable)?;
        // The verified root is the identity anchor. Watch only the unselected ancestors below
        // it, by held descriptor; watching the selected object would flag our own capture.
        for ancestor in &chain[1..] {
            context.check()?;
            inotify::add_watch(
                &events,
                format!("/proc/self/fd/{}", ancestor.as_raw_fd()),
                WatchFlags::MOVE_SELF | WatchFlags::DELETE_SELF | WatchFlags::ONLYDIR,
            )
            .map_err(unavailable)?;
        }
        // Watches precede the baseline check. Never drain/reset their events here: a move away
        // and back between registration and capture must remain visible.
        validate_ancestry(chain, names, context)?;
        Ok(Some(Self { chain, events }))
    }

    pub(super) fn after_capture(&self, context: &OperationContext) -> io::Result<()> {
        // Linux rename holds the original parent's i_rwsem until fsnotify_move has queued its
        // events. A fresh getdents takes that inode's shared lock, fencing a pre-capture rename
        // whose dentry change was already visible but whose notification was still in flight.
        // Constructing a Dir alone, or reading a cached buffer, would not provide this fence.
        for parent in &self.chain[..self.chain.len() - 1] {
            context.check()?;
            let mut reader = Dir::read_from(parent).map_err(unavailable)?;
            reader.read().transpose().map_err(unavailable)?;
        }
        context.check()?;
        let mut buffer = [MaybeUninit::uninit(); 512];
        let mut reader = inotify::Reader::new(&self.events, &mut buffer);
        match reader.next() {
            // Only self-movement/deletion is requested. Automatic UNMOUNT, IGNORED and queue
            // overflow events also invalidate the observation; none is a harmless event to skip.
            Ok(_) => Err(changed()),
            Err(rustix::io::Errno::AGAIN) => Ok(()),
            Err(error) => Err(unavailable(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lost_watch_is_not_treated_as_an_unchanged_ancestor() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("ancestor")).unwrap();
        let root = open_root(temp.path()).unwrap();
        let ancestor = directory(&root, OsStr::new("ancestor")).unwrap();
        let chain = vec![root, ancestor];
        let context = OperationContext::default();
        let guard = Guard::watch(&chain, &[OsStr::new("ancestor")], &context)
            .unwrap()
            .unwrap();
        let watch = inotify::add_watch(
            &guard.events,
            format!("/proc/self/fd/{}", chain[1].as_raw_fd()),
            WatchFlags::MOVE_SELF,
        )
        .unwrap();
        inotify::remove_watch(&guard.events, watch).unwrap();
        assert_eq!(
            crate::error::classify(&guard.after_capture(&context).unwrap_err()).kind,
            IoFailureKind::TargetChanged
        );
    }

    #[test]
    fn baseline_revalidation_rejects_movement_before_watch_registration() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("ancestor")).unwrap();
        let root = open_root(temp.path()).unwrap();
        let ancestor = directory(&root, OsStr::new("ancestor")).unwrap();
        let chain = vec![root, ancestor];
        std::fs::rename(temp.path().join("ancestor"), temp.path().join("moved")).unwrap();
        let result = Guard::watch(
            &chain,
            &[OsStr::new("ancestor")],
            &OperationContext::default(),
        );
        assert!(
            matches!(result, Err(ref error) if crate::error::classify(error).kind == IoFailureKind::TargetChanged)
        );
    }
}
