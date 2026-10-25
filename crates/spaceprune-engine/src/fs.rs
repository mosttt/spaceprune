//! 平台中立的路径、容量和时间辅助函数。

use std::{
    fs,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use spaceprune_plugin_api::{PluginError, PluginResult};
use walkdir::WalkDir;

/// 一次目录遍历得到的路径统计信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathStats {
    /// 文件或目录树内文件的逻辑字节数。
    pub logical_bytes: u64,
    /// 文件或目录树中最新的修改时间戳。
    pub latest_modified: Option<u64>,
}

/// 忽略不可读子树后的路径容量计量结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BestEffortMeasurement {
    /// 已成功读取元数据的普通文件逻辑字节数。
    pub logical_bytes: u64,
    /// 是否完整遍历了路径中的全部项目。
    pub complete: bool,
    /// A representative native error; an access denial takes priority over other gaps.
    pub failure: Option<spaceprune_access::IoFailure>,
}

/// 解析人类可读的持续时间，例如 `7d` 或 `12h`。
pub fn parse_duration(value: &str) -> PluginResult<Duration> {
    humantime::parse_duration(value).map_err(|error| {
        PluginError::new(
            "invalid-duration",
            format!("Could not parse duration `{value}`: {error}"),
        )
    })
}

/// 解析二进制容量，例如 `512MiB`、`2G` 或 `0B`。
pub fn parse_bytes(value: &str) -> PluginResult<u64> {
    let value = value.trim();
    if value.is_empty() {
        return Err(PluginError::new("invalid-size", "Size must not be empty"));
    }
    let number_end = value
        .char_indices()
        .find_map(|(index, ch)| (!ch.is_ascii_digit() && ch != '.').then_some(index))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(number_end);
    let number: f64 = number.parse().map_err(|_| {
        PluginError::new("invalid-size", format!("Could not parse size `{number}`"))
    })?;
    if !number.is_finite() || number < 0.0 {
        return Err(PluginError::new(
            "invalid-size",
            "Size must be a finite non-negative number",
        ));
    }
    let multiplier = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1024_u64,
        "m" | "mb" | "mib" => 1024_u64.pow(2),
        "g" | "gb" | "gib" => 1024_u64.pow(3),
        "t" | "tb" | "tib" => 1024_u64.pow(4),
        "p" | "pb" | "pib" => 1024_u64.pow(5),
        unit => {
            return Err(PluginError::new(
                "invalid-size",
                format!("Unsupported size unit `{unit}`"),
            ));
        }
    };
    let bytes = number * multiplier as f64;
    if bytes > u64::MAX as f64 {
        return Err(PluginError::new(
            "invalid-size",
            "Size exceeds the supported range",
        ));
    }
    Ok(bytes.round() as u64)
}

/// 将字节数格式化为便于阅读的二进制单位。
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// 统计文件或目录树的逻辑字节数，不跟随符号链接。
pub fn measure_path(path: &Path) -> std::io::Result<u64> {
    path_stats(path).map(|stats| stats.logical_bytes)
}

/// 统计可读文件的逻辑容量，并在遇到不可读子树时返回部分结果。
///
/// 根路径本身不可访问时仍返回错误；该接口用于系统缓存等允许明确展示部分计量的领域扫描。
pub fn measure_path_best_effort(path: &Path) -> std::io::Result<BestEffortMeasurement> {
    measure_path_best_effort_cancellable(path, &spaceprune_plugin_api::OperationContext::default())
}

/// Measure readable content without replacing the caller's cancellation or deadline.
pub fn measure_path_best_effort_cancellable(
    path: &Path,
    context: &spaceprune_plugin_api::OperationContext,
) -> std::io::Result<BestEffortMeasurement> {
    context.check()?;
    let root_metadata = fs::symlink_metadata(path)?;
    if !root_metadata.is_dir() {
        return Ok(BestEffortMeasurement {
            logical_bytes: root_metadata.len(),
            complete: true,
            failure: None,
        });
    }

    let mut logical_bytes = 0_u64;
    let mut complete = true;
    let mut failure = None;
    for entry in WalkDir::new(path).follow_links(false) {
        context.check()?;
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                complete = false;
                record_measurement_failure(&mut failure, &walk_error(error));
                continue;
            }
        };
        match entry.metadata() {
            Ok(metadata) if metadata.is_file() => {
                logical_bytes = logical_bytes.saturating_add(metadata.len());
            }
            Ok(_) => {}
            Err(error) => {
                complete = false;
                record_measurement_failure(&mut failure, &walk_error(error));
            }
        }
    }
    Ok(BestEffortMeasurement {
        logical_bytes,
        complete,
        failure,
    })
}

fn record_measurement_failure(
    failure: &mut Option<spaceprune_access::IoFailure>,
    error: &std::io::Error,
) {
    let observed = spaceprune_access::error::classify(error);
    if failure.as_ref().is_none_or(|previous| {
        previous.kind != spaceprune_access::IoFailureKind::AccessDenied
            && observed.kind == spaceprune_access::IoFailureKind::AccessDenied
    }) {
        *failure = Some(observed);
    }
}

/// Check metadata access within the same directory search scope as a failed domain command.
/// No file contents are opened, links are not followed, and traversal stays on the root filesystem.
pub fn probe_directory_access(
    path: &Path,
    max_depth: Option<usize>,
    prune_name: Option<&std::ffi::OsStr>,
    context: &spaceprune_plugin_api::OperationContext,
) -> std::io::Result<()> {
    context.check()?;
    fs::symlink_metadata(path)?;
    let mut entries = WalkDir::new(path)
        .follow_links(false)
        .follow_root_links(false)
        .same_file_system(true)
        .max_depth(max_depth.unwrap_or(usize::MAX))
        .into_iter();
    let mut failure: Option<std::io::Error> = None;
    while let Some(entry) = entries.next() {
        context.check()?;
        let inspected = entry.map_err(walk_error).and_then(|entry| {
            let metadata = entry.metadata().map_err(walk_error)?;
            if metadata.is_dir() && prune_name == Some(entry.file_name()) {
                entries.skip_current_dir();
            }
            Ok(())
        });
        if let Err(error) = inspected {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                return Err(error);
            }
            failure.get_or_insert(error);
        }
    }
    context.check()?;
    failure.map_or(Ok(()), Err)
}

/// 返回文件或目录树中最新的修改时间戳。
pub fn latest_modified(path: &Path) -> std::io::Result<Option<u64>> {
    path_stats(path).map(|stats| stats.latest_modified)
}

/// 一次遍历统计路径的逻辑字节数和最新修改时间，不跟随符号链接。
pub fn path_stats(path: &Path) -> std::io::Result<PathStats> {
    path_stats_cancellable(path, &spaceprune_plugin_api::OperationContext::default())
}

/// Measure a subtree while honoring the original operation's cancellation and deadline.
pub fn path_stats_cancellable(
    path: &Path,
    context: &spaceprune_plugin_api::OperationContext,
) -> std::io::Result<PathStats> {
    context.check()?;
    let root_metadata = fs::symlink_metadata(path)?;
    if !root_metadata.is_dir() {
        return Ok(PathStats {
            logical_bytes: root_metadata.len(),
            latest_modified: root_metadata.modified().ok().and_then(unix_timestamp),
        });
    }

    let mut logical_bytes = 0_u64;
    let mut latest: Option<u64> = None;
    for entry in WalkDir::new(path).follow_links(false) {
        context.check()?;
        let entry = entry.map_err(walk_error)?;
        let metadata = entry.metadata().map_err(walk_error)?;
        if metadata.is_file() {
            logical_bytes = logical_bytes.saturating_add(metadata.len());
        }
        let modified = metadata.modified().ok().and_then(unix_timestamp);
        latest = match (latest, modified) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (None, value) => value,
            (value, None) => value,
        };
    }
    Ok(PathStats {
        logical_bytes,
        latest_modified: latest,
    })
}

fn walk_error(error: walkdir::Error) -> std::io::Error {
    let message = error.to_string();
    error
        .into_io_error()
        .unwrap_or_else(|| std::io::Error::other(message))
}

/// 返回给定修改时间距今是否达到指定时长。
pub fn is_older_than(timestamp: Option<u64>, threshold: Duration) -> bool {
    let Some(timestamp) = timestamp else {
        return false;
    };
    unix_timestamp(SystemTime::now())
        .is_some_and(|now| now.saturating_sub(timestamp) >= threshold.as_secs())
}

/// 删除文件、符号链接或目录树。
pub fn remove_path(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// 将系统时间转换为 Unix 时间戳。
pub fn unix_timestamp(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_secs())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{format_bytes, measure_path, measure_path_best_effort, parse_bytes, path_stats};

    #[test]
    fn parses_and_formats_sizes() {
        assert_eq!(parse_bytes("1.5GiB").unwrap(), 1_610_612_736);
        assert_eq!(format_bytes(1024), "1.00 KiB");
    }

    #[test]
    fn measures_directory_tree() {
        let temp = tempdir().unwrap();
        fs::create_dir(temp.path().join("child")).unwrap();
        fs::write(temp.path().join("child/data"), [0_u8; 32]).unwrap();
        assert_eq!(measure_path(temp.path()).unwrap(), 32);
        assert_eq!(path_stats(temp.path()).unwrap().logical_bytes, 32);
    }

    #[test]
    fn best_effort_measurement_reports_complete_readable_tree() {
        let temp = tempdir().unwrap();
        fs::create_dir(temp.path().join("child")).unwrap();
        fs::write(temp.path().join("child/data"), [0_u8; 48]).unwrap();
        let measurement = measure_path_best_effort(temp.path()).unwrap();
        assert_eq!(measurement.logical_bytes, 48);
        assert!(measurement.complete);
        assert!(measurement.failure.is_none());
    }

    #[test]
    fn best_effort_measurement_does_not_hide_cancel_or_deadline_as_unreadable_content() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("data"), b"keep").unwrap();
        let context = spaceprune_plugin_api::OperationContext::default();
        context.cancel();
        assert_eq!(
            super::measure_path_best_effort_cancellable(temp.path(), &context)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::Interrupted
        );
        let expired = spaceprune_plugin_api::OperationContext {
            deadline: std::time::Instant::now(),
            ..spaceprune_plugin_api::OperationContext::default()
        };
        assert_eq!(
            super::measure_path_best_effort_cancellable(temp.path(), &expired)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::TimedOut
        );
        assert_eq!(fs::read(temp.path().join("data")).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn access_probe_preserves_missing_path_errors_and_never_follows_links() {
        use std::os::unix::fs::symlink;
        let temp = tempdir().unwrap();
        let context = spaceprune_plugin_api::OperationContext::default();
        let missing = temp.path().join("missing");
        let native = fs::symlink_metadata(&missing).unwrap_err();
        let probe = super::probe_directory_access(&missing, None, None, &context).unwrap_err();
        assert_eq!(probe.raw_os_error(), native.raw_os_error());
        symlink(&missing, temp.path().join("link")).unwrap();
        super::probe_directory_access(temp.path(), None, None, &context).unwrap();
        context.cancel();
        assert_eq!(
            super::probe_directory_access(temp.path(), None, None, &context)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::Interrupted
        );
    }
}
