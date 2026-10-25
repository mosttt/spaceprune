//! Elevated installer entry. No arbitrary destination, command, or policy is accepted.

use super::*;
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
};

/// Dispatch only the native-install protocol and fixed component installation operation.
pub fn run_installer_from_environment() -> Option<i32> {
    let mut arguments = std::env::args_os().skip(1);
    let argument = arguments.next()?;
    if argument == INSTALLER_SUPPORT {
        return Some(if arguments.next().is_none() { 0 } else { 64 });
    }
    if argument != INSTALL_ARGUMENT && argument != INSTALL_METADATA_ARGUMENT {
        return None;
    }
    if !crate::process::is_elevated() {
        return Some(77);
    }
    let Some(request) = arguments.next().and_then(|value| value.into_string().ok()) else {
        return Some(64);
    };
    if request.len() > MAX_REQUEST_BYTES || arguments.next().is_some() {
        return Some(64);
    }
    let request = match serde_json::from_str::<InstallRequest>(&request) {
        Ok(request) => request,
        Err(_) => return Some(64),
    };
    Some(
        if run(request, argument == INSTALL_METADATA_ARGUMENT).is_ok() {
            0
        } else {
            1
        },
    )
}

fn ensure_directory(path: &Path) -> io::Result<()> {
    for directory in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        match fs::symlink_metadata(directory) {
            Ok(metadata)
                if metadata.is_dir()
                    && !metadata.file_type().is_symlink()
                    && metadata.uid() == 0
                    && metadata.permissions().mode() & 0o022 == 0 => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::DirBuilder::new().mode(0o755).create(directory)?;
                fs::set_permissions(directory, fs::Permissions::from_mode(0o755))?;
            }
            _ => return Err(io::Error::other("Untrusted installation ancestor")),
        }
    }
    Ok(())
}

fn acquire_lock(root: &Path, check: &mut impl FnMut() -> io::Result<()>) -> io::Result<fs::File> {
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .mode(0o600)
        .open(root.join(".install.lock"))?;
    let metadata = lock.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(io::Error::other("Untrusted installer lock"));
    }
    wait_for_lock(&lock, check)?;
    Ok(lock)
}

pub(super) fn wait_for_lock(
    lock: &fs::File,
    check: &mut impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    loop {
        check()?;
        match rustix::fs::flock(lock, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => return Ok(()),
            Err(rustix::io::Errno::WOULDBLOCK) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => return Err(error.into()),
        }
    }
}

fn install_policy() -> io::Result<()> {
    let destination = Path::new(POLICY_PATH);
    if !install::destination_is_safe(destination) {
        return Err(io::Error::other("Untrusted policy destination"));
    }
    let parent = destination.parent().expect("fixed policy parent");
    ensure_directory(parent)?;
    let mut policy = tempfile::NamedTempFile::new_in(parent)?;
    policy.write_all(include_bytes!("native.policy"))?;
    policy
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o644))?;
    policy.as_file().sync_all()?;
    policy.persist(destination)?;
    fs::File::open(parent)?.sync_all()
}

fn run(request: InstallRequest, start_metadata: bool) -> io::Result<()> {
    let context = OperationContext::from_transfer(
        &request.operation,
        Duration::from_secs(120),
        crate::ResourceBudget::default(),
    )?;
    let caller = match std::env::var("PKEXEC_UID").or_else(|_| std::env::var("SUDO_UID")) {
        Ok(uid) => uid
            .parse::<u32>()
            .map_err(|_| io::Error::other("Invalid installer caller"))?,
        Err(_) => rustix::process::getuid().as_raw(),
    };
    // Poll the inherited pipe between chunks/lock attempts. Root need not be signalable by the host.
    let mut input = fs::File::from(rustix::io::dup(std::io::stdin())?);
    let flags = rustix::fs::fcntl_getfl(&input)?;
    rustix::fs::fcntl_setfl(&input, flags | rustix::fs::OFlags::NONBLOCK)?;
    let mut check = || {
        context.check()?;
        match input.read(&mut [0]) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(()),
            _ => Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Installation host disconnected",
            )),
        }
    };
    check()?;
    let root = Path::new(INSTALL_ROOT);
    ensure_directory(root)?;
    let _lock = acquire_lock(root, &mut check)?;
    let destination = Path::new(INSTALL_DIRECTORY);
    if !install::destination_is_safe(destination) {
        return Err(io::Error::other("Untrusted installation destination"));
    }
    let exists = staging::validate_existing(destination)?;
    let staged = staging::stage(root, &request, caller, &mut check)?;
    check()?;
    install_policy()?;
    check()?;
    staging::publish(staged, destination, exists)?;
    check()?;
    // The session reader inherits stdin: restore blocking mode before handing it over.
    rustix::fs::fcntl_setfl(&input, flags)?;
    if start_metadata {
        use std::os::unix::process::CommandExt;
        let helper = probe_helper(HelperKind::Metadata, &context).map_err(|outcome| {
            io::Error::other(format!("Installed metadata component: {outcome:?}"))
        })?;
        context.check()?;
        drop(_lock);
        // Keep the original pipes, PID and native caller identity; no second authenticator.
        return Err(Command::new(helper).arg(METADATA_SESSION_ARGUMENT).exec());
    }
    Ok(())
}
