//! Read local volume capacity without traversing files or acquiring authorization.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub use spaceprune_contracts::storage::StorageVolume;

/// Query mounted local filesystems. Virtual and network mounts are excluded.
pub fn local_storage_volumes() -> std::io::Result<Vec<StorageVolume>> {
    #[cfg(target_os = "linux")]
    let mut volumes = linux::volumes()?;
    #[cfg(target_os = "macos")]
    let mut volumes = macos::volumes()?;
    #[cfg(windows)]
    let mut volumes = windows::volumes()?;
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    let mut volumes: Vec<StorageVolume> = Vec::new();
    for volume in &mut volumes {
        volume
            .mount_points
            .sort_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)));
        volume.mount_points.dedup();
    }
    volumes.sort_by(|left, right| left.mount_points.first().cmp(&right.mount_points.first()));
    Ok(volumes)
}

fn capacity(total: u64, free: u64, available: u64) -> (Option<u64>, Option<u64>, Option<u64>) {
    if total == 0 || free > total || available > free {
        return (None, None, None);
    }
    (Some(total), Some(total - free), Some(available))
}

#[cfg(test)]
mod tests {
    #[test]
    fn reserved_free_capacity_is_not_reported_as_used_or_available() {
        assert_eq!(
            super::capacity(1000, 300, 200),
            (Some(1000), Some(700), Some(200))
        );
        assert_eq!(super::capacity(0, 0, 0), (None, None, None));
        assert_eq!(super::capacity(100, 200, 50), (None, None, None));
    }
}
