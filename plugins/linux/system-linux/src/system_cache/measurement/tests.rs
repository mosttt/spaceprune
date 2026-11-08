use super::*;
use std::os::unix::{
    ffi::OsStringExt,
    fs::{PermissionsExt, symlink},
};

#[test]
fn native_expiration_preview_respects_exclusions_without_removing_files() {
    if !command_exists("systemd-tmpfiles") {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("cache");
    fs::create_dir(&cache).unwrap();
    let old = cache.join("old file");
    let retained = cache.join("keep");
    fs::write(&old, b"eligible").unwrap();
    fs::write(&retained, b"preserved").unwrap();
    let configuration = root.path().join("fixture.conf");
    fs::write(
        &configuration,
        format!(
            "d {} - - - 0\nx {} - - - -\n",
            cache.display(),
            retained.display()
        ),
    )
    .unwrap();
    let mut command = Command::new("systemd-tmpfiles");
    command.arg(&configuration);
    match preview(command, &OperationContext::default()) {
        Ok(size) => {
            assert_eq!(size.logical_bytes, 8);
            assert!(!size.is_empty);
        }
        Err(issue) if issue.code.as_deref() == Some("systemd-preview-unsupported") => {}
        Err(issue) => panic!("native fixture preview failed: {issue:?}"),
    }
    assert_eq!(fs::read(old).unwrap(), b"eligible");
    assert_eq!(fs::read(retained).unwrap(), b"preserved");
}

#[test]
fn preview_counts_only_reported_files_without_following_links_or_double_counting() {
    let root = tempfile::tempdir().unwrap();
    let selected = root
        .path()
        .join(std::ffi::OsString::from_vec(b"cache-\xff".to_vec()));
    fs::write(&selected, b"four").unwrap();
    let alias = root.path().join("hardlink");
    fs::hard_link(&selected, &alias).unwrap();
    let link = root.path().join("link");
    symlink(&selected, &link).unwrap();
    fs::write(root.path().join("not reported"), b"do not count this").unwrap();
    let mut output = Vec::new();
    for path in [&selected, &selected, &alias, &link, root.path()] {
        output.extend_from_slice(b"Would remove \"");
        output.extend_from_slice(path.as_os_str().as_bytes());
        output.extend_from_slice(b"\"\n");
    }
    assert_eq!(
        measure_preview(&output, &OperationContext::default())
            .unwrap()
            .logical_bytes,
        4
    );
    assert_eq!(fs::read(&selected).unwrap(), b"four");
    let aliased_parent = root.path().join("alias");
    symlink(root.path(), &aliased_parent).unwrap();
    let aliased = format!(
        "Would remove \"{}/not reported\"\n",
        aliased_parent.display()
    );
    assert!(measure_preview(aliased.as_bytes(), &OperationContext::default()).is_err());
    assert!(measure_preview(b"Would remove \"/a\"bad\"\n", &OperationContext::default()).is_err());
    assert!(
        measure_preview(
            b"A warning, not an empty result\n",
            &OperationContext::default()
        )
        .is_err()
    );
    assert_eq!(
        measure_preview(b"", &OperationContext::default())
            .unwrap()
            .logical_bytes,
        0
    );
    assert!(
        measure_preview(b"", &OperationContext::default())
            .unwrap()
            .is_empty
    );
    let context = OperationContext::default();
    context.cancel();
    assert!(measure_preview(&output, &context).is_err());
}

#[test]
fn unsupported_native_preview_never_retries_without_dry_run() {
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("old-systemd");
    let args = root.path().join("args");
    fs::write(&program, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' \"unrecognized option '--dry-run'\" >&2\nexit 1\n", args.display())).unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    let error = preview(Command::new(&program), &OperationContext::default()).unwrap_err();
    assert_eq!(error.code.as_deref(), Some("systemd-preview-unsupported"));
    assert_eq!(fs::read_to_string(args).unwrap(), "--clean\n--dry-run\n");
}

#[test]
fn permission_failures_remain_eligible_for_native_readonly_retry() {
    let error = output_issue("Failed to open directory: Permission denied");
    assert_eq!(
        error.io_failure.unwrap().kind,
        spaceprune_access::IoFailureKind::AccessDenied
    );
    let unknown = output_issue("Failed to process configuration: Invalid argument");
    assert!(unknown.io_failure.is_none());
}
