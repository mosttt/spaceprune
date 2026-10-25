use super::{StorageVolume, capacity};
use std::{collections::BTreeMap, fs, io};

pub(super) fn volumes() -> io::Result<Vec<StorageVolume>> {
    let mounts = fs::read_to_string("/proc/self/mountinfo")?;
    let mut volumes = parse_mounts(&mounts);
    for volume in &mut volumes {
        if let Some(stats) = volume
            .mount_points
            .iter()
            .find_map(|mount| rustix::fs::statvfs(mount).ok())
        {
            (
                volume.total_bytes,
                volume.used_bytes,
                volume.available_bytes,
            ) = capacity(
                stats.f_blocks.saturating_mul(stats.f_frsize),
                stats.f_bfree.saturating_mul(stats.f_frsize),
                stats.f_bavail.saturating_mul(stats.f_frsize),
            );
        }
    }
    Ok(volumes)
}

fn parse_mounts(source: &str) -> Vec<StorageVolume> {
    let mut volumes = BTreeMap::<String, StorageVolume>::new();
    for line in source.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        let fields: Vec<_> = after.split_whitespace().collect();
        let [filesystem, device, ..] = fields.as_slice() else {
            continue;
        };
        if !device.starts_with("/dev/") && *filesystem != "zfs" {
            continue;
        }
        // Loop-mounted images and pseudo filesystems are not local storage devices.
        if device.starts_with("/dev/loop")
            || matches!(*filesystem, "tmpfs" | "devtmpfs" | "squashfs" | "overlay")
        {
            continue;
        }
        let Some(mount) = before.split_whitespace().nth(4) else {
            continue;
        };
        let device = decode(device);
        let id = format!("{filesystem}:{device}");
        volumes
            .entry(id.clone())
            .or_insert_with(|| StorageVolume {
                id,
                name: device,
                mount_points: Vec::new(),
                total_bytes: None,
                used_bytes: None,
                available_bytes: None,
            })
            .mount_points
            .push(decode(mount));
    }
    volumes.into_values().collect()
}

fn decode(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

#[cfg(test)]
mod tests {
    #[test]
    fn subvolumes_and_bind_mounts_share_capacity_but_different_devices_do_not() {
        let volumes = super::parse_mounts(concat!(
            "1 0 259:1 /@ / rw - btrfs /dev/nvme0n1p2 rw\n",
            "2 1 259:2 /@home /home rw - btrfs /dev/nvme0n1p2 rw\n",
            "3 1 259:3 / /media/Work\\040Disk rw - ext4 /dev/sdb1 rw\n",
            "4 1 0:1 / /tmp rw - tmpfs tmpfs rw\n",
            "5 1 0:2 / /remote rw - nfs server:/data rw\n",
            "6 1 7:1 / /image rw - squashfs /dev/loop1 ro\n",
        ));
        assert_eq!(volumes.len(), 2);
        assert_eq!(volumes[0].mount_points, ["/", "/home"]);
        assert_eq!(volumes[1].mount_points, ["/media/Work Disk"]);
        assert!(volumes.iter().all(|volume| volume.total_bytes.is_none()));
    }
}
