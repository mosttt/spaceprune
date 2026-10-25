#![allow(unsafe_code)]

use super::{StorageVolume, capacity};
use std::{io, mem::MaybeUninit};

pub(super) fn volumes() -> io::Result<Vec<StorageVolume>> {
    // SAFETY: A null buffer and zero length query only the mount count.
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut entries = Vec::<MaybeUninit<libc::statfs>>::new();
    entries.resize_with(count as usize + 8, MaybeUninit::uninit);
    let bytes = i32::try_from(entries.len() * std::mem::size_of::<libc::statfs>())
        .map_err(io::Error::other)?;
    // SAFETY: The buffer is aligned and writable for exactly `bytes`; the OS initializes returned entries.
    let written = unsafe { libc::getfsstat(entries.as_mut_ptr().cast(), bytes, libc::MNT_NOWAIT) };
    if written < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut volumes = Vec::new();
    for entry in entries.iter().take((written as usize).min(entries.len())) {
        // SAFETY: These entries were initialized by the successful getfsstat call.
        let entry = unsafe { entry.assume_init_ref() };
        if entry.f_flags & libc::MNT_LOCAL as u32 == 0 {
            continue;
        }
        let mount = text(&entry.f_mntonname);
        if mount != "/" && mount != "/System/Volumes/Data" && !mount.starts_with("/Volumes/") {
            continue;
        }
        let device = text(&entry.f_mntfromname);
        let block_size = u64::from(entry.f_bsize);
        let (total_bytes, used_bytes, available_bytes) = capacity(
            entry.f_blocks.saturating_mul(block_size),
            entry.f_bfree.saturating_mul(block_size),
            entry.f_bavail.saturating_mul(block_size),
        );
        volumes.push(StorageVolume {
            id: device.clone(),
            name: device,
            mount_points: vec![mount],
            total_bytes,
            used_bytes,
            available_bytes,
        });
    }
    Ok(volumes)
}

fn text(value: &[libc::c_char]) -> String {
    String::from_utf8_lossy(
        &value
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect::<Vec<_>>(),
    )
    .into_owned()
}
