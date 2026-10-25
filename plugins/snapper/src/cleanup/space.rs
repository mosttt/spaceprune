//! Sample free blocks around one exact deletion batch, never estimate snapshot ownership.

use spaceprune_plugin_api::VolumeSpaceChange;
use std::path::{Path, PathBuf};

struct Sample {
    filesystem: u64,
    free_bytes: u64,
}

pub(super) struct SpaceProbe {
    scope: String,
    path: Result<PathBuf, String>,
    before: Result<Sample, String>,
}

impl SpaceProbe {
    pub(super) fn begin(scope: String, path: Result<PathBuf, String>) -> Self {
        let before = path
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|path| sample(path));
        Self {
            scope,
            path,
            before,
        }
    }

    pub(super) fn finish(self) -> VolumeSpaceChange {
        let after = self
            .path
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|path| sample(path));
        observation(self.scope, self.before, after)
    }
}

fn observation(
    scope: String,
    before: Result<Sample, String>,
    after: Result<Sample, String>,
) -> VolumeSpaceChange {
    let mut change = VolumeSpaceChange {
        scope,
        free_bytes_before: before.as_ref().ok().map(|sample| sample.free_bytes),
        free_bytes_after: after.as_ref().ok().map(|sample| sample.free_bytes),
        error: None,
    };
    match (before, after) {
        (Ok(before), Ok(after)) if before.filesystem != after.filesystem => {
            change.free_bytes_after = None;
            change.error = Some(
                spaceprune_plugin_api::messages::text("snapper.filesystem_changed").to_owned(),
            );
        }
        (Err(error), _) => {
            change.error = Some(spaceprune_plugin_api::messages::message(
                "snapper.before_cleanup",
                &[("error", (error).to_string())],
            ))
        }
        (_, Err(error)) => {
            change.error = Some(spaceprune_plugin_api::messages::message(
                "snapper.after_cleanup",
                &[("error", (error).to_string())],
            ))
        }
        _ => {}
    }
    change
}

#[cfg(target_os = "linux")]
fn sample(path: &Path) -> Result<Sample, String> {
    let stats = rustix::fs::statvfs(path).map_err(|error| error.to_string())?;
    Ok(Sample {
        filesystem: stats.f_fsid,
        free_bytes: stats.f_bfree.saturating_mul(stats.f_frsize),
    })
}

#[cfg(not(target_os = "linux"))]
fn sample(_: &Path) -> Result<Sample, String> {
    Err(spaceprune_plugin_api::messages::text("snapper.measure_linux_only").to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_replacement_and_missing_samples_never_claim_freed_space() {
        let before = || {
            Ok(Sample {
                filesystem: 1,
                free_bytes: 100,
            })
        };
        let replaced = observation(
            "home".into(),
            before(),
            Ok(Sample {
                filesystem: 2,
                free_bytes: 500,
            }),
        );
        assert_eq!(replaced.free_bytes_before, Some(100));
        assert_eq!(replaced.free_bytes_after, None);
        assert!(replaced.error.is_some());
        let denied = observation("home".into(), before(), Err("Permission denied".into()));
        assert_eq!(denied.free_bytes_after, None);
        assert_eq!(
            denied.error.as_deref(),
            Some("After cleanup: Permission denied")
        );
        for free_bytes in [50, 100, 200] {
            let changed = observation(
                "home".into(),
                before(),
                Ok(Sample {
                    filesystem: 1,
                    free_bytes,
                }),
            );
            assert_eq!(changed.free_bytes_after, Some(free_bytes));
            assert!(changed.error.is_none());
        }
    }
}
