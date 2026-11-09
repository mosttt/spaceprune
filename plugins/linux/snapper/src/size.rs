use std::{
    collections::BTreeMap,
    io,
    path::Path,
    process::{Command, Output},
    time::Duration,
};

use super::*;

/// Measure snapshot file metadata without treating shared data as reclaimable space.
#[allow(clippy::too_many_arguments)]
pub(super) fn measure_snapshots(
    task: &TaskRequest,
    config: &str,
    subvolume: &Path,
    numbers: Vec<u64>,
    options: &Options,
    progress: &dyn ProgressSink,
    context: &OperationContext,
    issues: &mut Vec<HandlerIssue>,
) -> BTreeMap<u64, u64> {
    if numbers.is_empty() || context.is_stopped() {
        return BTreeMap::new();
    }
    let mut values = BTreeMap::new();
    for (index, number) in numbers.iter().enumerate() {
        if context.is_stopped() {
            break;
        }
        let path = subvolume
            .join(".snapshots")
            .join(number.to_string())
            .join("snapshot");
        progress.emit(ProgressEvent {
            phase: ProgressPhase::Measure,
            module_id: Some(task.module_id.clone()),
            task_id: Some(task.task_id.clone()),
            handler_id: Some(task.handler_id.clone()),
            current: Some(index),
            total: Some(numbers.len()),
            message: spaceprune_plugin_api::messages::message(
                "snapper.measure_snapshot",
                &[
                    ("config", config.to_owned()),
                    ("number", number.to_string()),
                ],
            ),
        });
        // A failed or disappearing snapshot must not discard successful siblings. Separate
        // invocations also prevent du from deduplicating hard links across distinct snapshots.
        let measured = measure_snapshot(&path, options, context);
        if context.is_stopped() {
            break;
        }
        match measured {
            Ok(bytes) => {
                values.insert(*number, bytes);
            }
            Err(error) => issues.push(HandlerIssue {
                code: Some("snapshot-logical-size-unavailable".to_owned()),
                io_failure: Some(spaceprune_access::error::classify(&error)),
                level: IssueLevel::Warning,
                message: spaceprune_plugin_api::messages::message(
                    "snapper.logical_unknown",
                    &[("error", format!("{config} #{number}: {error}"))],
                ),
                location: Some(path.to_string_lossy().into_owned()),
            }),
        }
    }
    progress.emit(ProgressEvent {
        phase: ProgressPhase::Measure,
        module_id: Some(task.module_id.clone()),
        task_id: Some(task.task_id.clone()),
        handler_id: Some(task.handler_id.clone()),
        current: Some(values.len()),
        total: Some(numbers.len()),
        message: spaceprune_plugin_api::messages::message(
            "snapper.logical_measured",
            &[
                ("p0", values.len().to_string()),
                ("config", config.to_owned()),
            ],
        ),
    });
    values
}

fn measure_snapshot(path: &Path, options: &Options, context: &OperationContext) -> io::Result<u64> {
    let mut measurement = command(&options.du);
    measurement
        .env("LC_ALL", "C")
        .args([
            "--summarize",
            "--one-file-system",
            "--apparent-size",
            "--block-size=1",
            "--null",
            "--",
        ])
        .arg(path);
    let output = capture_cancellable(measurement, context)?;
    if !output.status.success() {
        return Err(measurement_error(
            "du",
            &output.stderr,
            &[path.to_path_buf()],
            None,
            None,
            context,
        ));
    }
    let mut records = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty());
    let record = records.next().ok_or_else(|| {
        io::Error::other(spaceprune_plugin_api::messages::text(
            "snapper.incomplete_du",
        ))
    })?;
    if records.next().is_some() {
        return Err(io::Error::other(spaceprune_plugin_api::messages::text(
            "snapper.duplicate_du_size",
        )));
    }
    let tab = record
        .iter()
        .position(|byte| *byte == b'\t')
        .ok_or_else(|| {
            io::Error::other(spaceprune_plugin_api::messages::text(
                "snapper.invalid_du_record",
            ))
        })?;
    if path.as_os_str().as_encoded_bytes() != &record[tab + 1..] {
        return Err(io::Error::other(spaceprune_plugin_api::messages::text(
            "snapper.unexpected_du_path",
        )));
    }
    std::str::from_utf8(&record[..tab])
        .map_err(io::Error::other)?
        .parse::<u64>()
        .map_err(io::Error::other)
}

pub(super) fn capture_cancellable(
    command: Command,
    context: &OperationContext,
) -> io::Result<Output> {
    let mut operation = context.child(Duration::from_secs(300));
    operation.budget.output_bytes = operation.budget.output_bytes.min(32 * 1024 * 1024);
    operation.budget.line_bytes = operation.budget.line_bytes.min(32 * 1024 * 1024);
    spaceprune_access::process::capture(command, &operation)
}

#[cfg(all(test, target_os = "linux"))]
#[path = "size/tests.rs"]
mod isolation_tests;

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    use super::*;
    use std::thread;
    use tempfile::tempdir;

    #[test]
    fn artifact_measurement_preserves_native_names_and_does_not_follow_links() {
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::symlink;
        let temp = tempdir().unwrap();
        let root = temp
            .path()
            .join(std::ffi::OsString::from_vec(b"project-\xff".to_vec()));
        let target = root.join("target");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("artifact"), [7; 2048]).unwrap();
        let outside = temp.path().join("outside");
        fs::File::create(&outside)
            .unwrap()
            .set_len(1 << 30)
            .unwrap();
        symlink(&outside, target.join("link")).unwrap();
        let bytes =
            measure_targets(&root, &Options::default(), &OperationContext::default()).unwrap();
        assert!((2048..1 << 30).contains(&bytes));
        assert_eq!(fs::metadata(outside).unwrap().len(), 1 << 30);
    }

    #[test]
    fn failed_measurement_does_not_publish_partial_du_totals() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join(".snapshots/1/snapshot")).unwrap();
        let executable = temp.path().join("du-fixture");
        fs::write(&executable, "#!/usr/bin/env python3\nimport sys\np = sys.argv[-1]\nsys.stdout.buffer.write(('999\\t' + p + '\\0').encode())\nsys.stderr.write('Permission denied\\n')\nsys.exit(1)\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let mut issues = Vec::new();
        let values = measure_snapshots(
            &super::super::tests::metadata_task(),
            "home",
            temp.path(),
            vec![1],
            &Options {
                du: executable.to_string_lossy().into_owned(),
                elevation: "never".to_owned(),
                ..Options::default()
            },
            &spaceprune_plugin_api::NullProgress,
            &OperationContext::default(),
            &mut issues,
        );
        assert!(values.is_empty());
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("Permission denied"));
        assert_eq!(
            issues[0].io_failure.as_ref().unwrap().kind,
            spaceprune_access::IoFailureKind::Other
        );
    }

    #[test]
    fn cancelling_size_measurement_stops_the_readonly_process_promptly() {
        let temp = tempdir().unwrap();
        let executable = temp.path().join("du-fixture");
        let started = temp.path().join("started");
        fs::write(&executable, format!("#!/usr/bin/env python3\nimport pathlib, time\npathlib.Path({}).write_text('started')\ntime.sleep(60)\n", serde_json::to_string(&started).unwrap())).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let context = OperationContext::default();
        let cancel = context.clone();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !started.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            cancel.cancel();
        });
        let start = Instant::now();
        let error = capture_cancellable(Command::new(executable), &context).unwrap_err();
        worker.join().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(start.elapsed() < Duration::from_secs(4));
    }
}

pub(super) fn measure_targets(
    root: &Path,
    options: &Options,
    context: &OperationContext,
) -> std::io::Result<u64> {
    context.check()?;
    let mut find = command(&options.find);
    find.env("LC_ALL", "C").arg(root).args([
        "-xdev",
        "-maxdepth",
        &options.max_depth.to_string(),
        "-type",
        "d",
        "-name",
        "target",
        "-prune",
        "-print0",
    ]);
    let output = size::capture_cancellable(find, context)?;
    if !output.status.success() {
        return Err(measurement_error(
            "find",
            &output.stderr,
            &[root.to_path_buf()],
            Some(options.max_depth),
            Some(std::ffi::OsStr::new("target")),
            context,
        ));
    }
    let targets: Vec<_> = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .map(native_path)
        .collect::<io::Result<Vec<_>>>()?;
    let mut total = 0_u64;
    for targets in targets.chunks(64) {
        let mut du = command(&options.du);
        du.env("LC_ALL", "C")
            .args([
                "-s",
                "-x",
                "--apparent-size",
                "--block-size=1",
                "--null",
                "--",
            ])
            .args(targets);
        let output = size::capture_cancellable(du, context)?;
        if !output.status.success() {
            return Err(measurement_error(
                "du",
                &output.stderr,
                targets,
                None,
                None,
                context,
            ));
        }
        for record in output.stdout.split(|byte| *byte == 0) {
            if record.is_empty() {
                continue;
            }
            let tab = record
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        spaceprune_plugin_api::messages::text("snapper.invalid_du_record"),
                    )
                })?;
            let value = std::str::from_utf8(&record[..tab])
                .map_err(std::io::Error::other)?
                .parse::<u64>()
                .map_err(std::io::Error::other)?;
            total = total.saturating_add(value);
        }
    }
    Ok(total)
}

#[cfg(unix)]
fn native_path(bytes: &[u8]) -> io::Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn native_path(bytes: &[u8]) -> io::Result<PathBuf> {
    std::str::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(io::Error::other)
}
