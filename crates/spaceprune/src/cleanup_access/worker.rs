//! Native cleanup entrypoints and platform-independent plan validation.
use super::*;
use std::io;
pub(super) mod plan;
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod stdio;
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(super) use stdio::frame_with_limit;
#[cfg(all(test, target_os = "linux"))]
use stdio::{frame, serve, serve_bound};
#[cfg(target_os = "windows")]
pub(super) use stdio::{frame, serve_grant};

#[cfg(target_os = "linux")]
use spaceprune_contracts::messages;
pub(super) fn run_if_requested() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    let argument = args.next()?;
    #[cfg(target_os = "windows")]
    let supported = argument
        == std::ffi::OsStr::new(
            &spaceprune_access::windows_package::Component::Cleanup.support_argument(),
        );
    #[cfg(not(target_os = "windows"))]
    let supported = argument == "--supports-cleanup-v4";
    if supported {
        return Some(if args.next().is_none() { 0 } else { 64 });
    }
    #[cfg(target_os = "macos")]
    return None;
    #[cfg(target_os = "windows")]
    return super::windows_bridge::run_if_requested(argument, args);
    #[cfg(target_os = "linux")]
    if !spaceprune_access::helper::HelperKind::Cleanup.accepts_session_argument(&argument) {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        // The kernel owns /proc/self, so its UID does not depend on an environment variable.
        if args.next().is_some() || !std::fs::metadata("/proc/self").is_ok_and(|m| m.uid() == 0) {
            return Some(77);
        }
        Some(if stdio::run().is_ok() { 0 } else { 1 })
    }
}

pub(super) fn prepare_system_programs(task: &mut TaskRequest) -> io::Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = task;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        if task.handler_id != "builtin.snapper/snapshots" {
            return Ok(());
        }
        let options = task.options.as_object_mut().ok_or_else(|| {
            io::Error::other(messages::text("cleanup_access.invalid_snapshot_options"))
        })?;
        for name in ["snapper", "du", "find"] {
            if let Some(value) = options.get(name) {
                let value = value.as_str().ok_or_else(|| {
                    io::Error::other(messages::text("cleanup_access.invalid_program"))
                })?;
                if value != name
                    && !["/usr/bin", "/usr/sbin", "/bin", "/sbin"]
                        .iter()
                        .any(|directory| {
                            std::path::Path::new(value)
                                == std::path::Path::new(directory).join(name)
                        })
                {
                    return Err(io::Error::other(messages::text(
                        "cleanup_access.custom_program",
                    )));
                }
            }
            let program = spaceprune_access::process::system_program(name).ok_or_else(|| {
                io::Error::other(messages::message(
                    "cleanup_access.trusted_program",
                    &[("name", name.to_string())],
                ))
            })?;
            options.insert(name.to_owned(), serde_json::json!(program));
        }
        Ok(())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
