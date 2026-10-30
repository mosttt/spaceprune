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
    let mut mounts = Vec::new();
    for entry in entries.iter().take((written as usize).min(entries.len())) {
        // SAFETY: These entries were initialized by the successful getfsstat call.
        let entry = unsafe { entry.assume_init_ref() };
        let mount = text(&entry.f_mntonname);
        let device = text(&entry.f_mntfromname);
        let block_size = u64::from(entry.f_bsize);
        let (total_bytes, used_bytes, available_bytes) = capacity(
            entry.f_blocks.saturating_mul(block_size),
            entry.f_bfree.saturating_mul(block_size),
            entry.f_bavail.saturating_mul(block_size),
        );
        mounts.push(Mount {
            flags: entry.f_flags,
            filesystem: text(&entry.f_fstypename),
            volume: StorageVolume {
                id: device.clone(),
                name: device,
                mount_points: vec![mount],
                total_bytes,
                used_bytes,
                available_bytes,
            },
        });
    }
    Ok(visible_volumes(mounts))
}

struct Mount {
    flags: u32,
    filesystem: String,
    volume: StorageVolume,
}

fn visible_volumes(mounts: Vec<Mount>) -> Vec<StorageVolume> {
    let mut mounts: Vec<_> = mounts
        .into_iter()
        .filter(|entry| {
            let mount = entry.volume.mount_points[0].as_str();
            entry.flags & libc::MNT_LOCAL as u32 != 0
                && (mount == "/"
                    || mount == "/System/Volumes/Data"
                    || (mount.starts_with("/Volumes/")
                        && entry.flags & libc::MNT_DONTBROWSE as u32 == 0))
        })
        .collect();
    // macOS exposes the startup volume group through both the sealed root and
    // its writable Data volume. Keep both access paths, but show capacity once.
    let root = mounts
        .iter()
        .position(|entry| entry.filesystem == "apfs" && entry.volume.mount_points[0] == "/");
    let data = mounts.iter().position(|entry| {
        entry.filesystem == "apfs" && entry.volume.mount_points[0] == "/System/Volumes/Data"
    });
    if let (Some(root), Some(data)) = (root, data)
        && let Some(container) = apfs_container(&mounts[root].volume.id)
        && apfs_container(&mounts[data].volume.id) == Some(container)
    {
        // Data has a stable device identity, unlike the boot snapshot.
        mounts[data].volume.mount_points.insert(0, "/".into());
        mounts.remove(root);
    }
    mounts.into_iter().map(|entry| entry.volume).collect()
}

fn apfs_container(device: &str) -> Option<&str> {
    let disk = device.strip_prefix("/dev/disk")?;
    let (container, slices) = disk.split_once('s')?;
    if container.is_empty()
        || !container.bytes().all(|byte| byte.is_ascii_digit())
        || slices
            .split('s')
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    Some(container)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mount(device: &str, path: &str, flags: u32) -> Mount {
        Mount {
            flags,
            filesystem: "apfs".into(),
            volume: StorageVolume {
                id: device.into(),
                name: device.into(),
                mount_points: vec![path.into()],
                total_bytes: Some(1000),
                used_bytes: Some(700),
                available_bytes: Some(300),
            },
        }
    }

    #[test]
    fn startup_group_has_one_capacity_and_preserves_both_access_paths() {
        let local = libc::MNT_LOCAL as u32;
        let hidden = local | libc::MNT_DONTBROWSE as u32;
        let result = visible_volumes(vec![
            mount("/dev/disk3s1s1", "/", local),
            mount("/dev/disk3s3", "/Volumes/Recovery", hidden),
            mount("/dev/disk3s5", "/System/Volumes/Data", hidden),
            mount("/dev/disk4s1", "/Volumes/Portable Disk", local),
            mount("//server/share", "/Volumes/Network", 0),
        ]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].id, "/dev/disk3s5");
        assert_eq!(result[0].mount_points, ["/", "/System/Volumes/Data"]);
        assert_eq!(result[0].total_bytes, Some(1000));
        assert_eq!(result[1].mount_points, ["/Volumes/Portable Disk"]);
    }

    #[test]
    fn equal_capacity_does_not_merge_unrelated_volumes() {
        let local = libc::MNT_LOCAL as u32;
        assert_eq!(
            visible_volumes(vec![
                mount("/dev/disk3s1s1", "/", local),
                mount("/dev/disk4s5", "/System/Volumes/Data", local),
                mount("/dev/disk3s6", "/Volumes/Work", local),
            ])
            .len(),
            3
        );
        assert_eq!(apfs_container("/dev/disk3s1s1"), Some("3"));
        assert_eq!(apfs_container("/dev/disk3s"), None);
        assert_eq!(apfs_container("disk3s5"), None);
    }
}
