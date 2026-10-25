#![allow(unsafe_code)]

use super::{StorageVolume, capacity};
use std::io;
use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives,
};

pub(super) fn volumes() -> io::Result<Vec<StorageVolume>> {
    // SAFETY: GetLogicalDrives takes no pointers and returns the OS drive-letter mask.
    let drives = unsafe { GetLogicalDrives() };
    if drives == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut volumes = Vec::new();
    for index in 0..26 {
        if drives & (1 << index) == 0 {
            continue;
        }
        let mount = format!("{}:\\", char::from(b'A' + index));
        let path: Vec<u16> = mount.encode_utf16().chain(Some(0)).collect();
        // SAFETY: The drive root is a valid NUL-terminated UTF-16 string for the entire call.
        let kind = unsafe { GetDriveTypeW(path.as_ptr()) };
        // DRIVE_REMOVABLE and DRIVE_FIXED exclude remote shares, RAM disks and optical media.
        if !matches!(kind, 2 | 3) {
            continue;
        }
        let (mut available, mut total, mut free) = (0, 0, 0);
        // SAFETY: All output pointers refer to writable u64 values; the path stays alive.
        let measured =
            unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut available, &mut total, &mut free) };
        let (total_bytes, used_bytes, available_bytes) = if measured != 0 {
            capacity(total, free, available)
        } else {
            (None, None, None)
        };
        volumes.push(StorageVolume {
            id: mount.clone(),
            name: mount.clone(),
            mount_points: vec![mount],
            total_bytes,
            used_bytes,
            available_bytes,
        });
    }
    Ok(volumes)
}
