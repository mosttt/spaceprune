//! Windows bootstrap binding and dedicated cleanup entry. No metadata connection grants deletion.

use super::*;
use spaceprune_windows_elevation::{LocalPipe, PrivatePathKind, TrustedExecutable};
use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_BOOTSTRAP: u64 = 64 * 1024;
const IPC_VERSION: u32 = spaceprune_access::windows_package::CLEANUP_PROTOCOL as u32;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bootstrap {
    version: u32,
    nonce: String,
    host_pid: u32,
    host_created: u64,
    operation: OperationTransfer,
}

pub(super) struct Prepared {
    directory: tempfile::TempDir,
    nonce: String,
    host_pid: u32,
    host_created: u64,
    pub(super) pipe: LocalPipe,
}

impl Prepared {
    pub(super) fn create(context: &OperationContext) -> io::Result<Self> {
        context.check()?;
        let directory = tempfile::Builder::new()
            .prefix("spaceprune-cleanup-")
            .tempdir()?;
        if !spaceprune_windows_elevation::valid_metadata_root(directory.path()) {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        spaceprune_windows_elevation::secure_created_logon_path(
            directory.path(),
            PrivatePathKind::Directory,
        )
        .map_err(|_| io::ErrorKind::PermissionDenied)?;
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
        let nonce = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let pipe = LocalPipe::create(&pipe_name(&nonce))?;
        let bootstrap = Bootstrap {
            version: IPC_VERSION,
            nonce: nonce.clone(),
            host_pid: std::process::id(),
            host_created: spaceprune_windows_elevation::current_process_creation_time()
                .ok_or_else(|| io::Error::other("Host identity unavailable"))?,
            operation: context.transfer()?,
        };
        let path = directory.path().join("bootstrap.json");
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        serde_json::to_writer(&mut file, &bootstrap)?;
        file.flush()?;
        spaceprune_windows_elevation::secure_created_logon_path(&path, PrivatePathKind::File)
            .map_err(|_| io::ErrorKind::PermissionDenied)?;
        Ok(Self {
            directory,
            nonce,
            host_pid: bootstrap.host_pid,
            host_created: bootstrap.host_created,
            pipe,
        })
    }

    pub(super) fn arguments(&self) -> [OsString; 5] {
        [
            WORKER_FLAG.into(),
            self.directory
                .path()
                .join("bootstrap.json")
                .into_os_string(),
            self.nonce.clone().into(),
            self.host_pid.to_string().into(),
            self.host_created.to_string().into(),
        ]
    }

    pub(super) fn close(&self) {
        self.pipe.disconnect();
        let _ = fs::remove_dir_all(self.directory.path());
    }
}

fn pipe_name(nonce: &str) -> String {
    format!(r"\\.\pipe\SpacePrune.cleanup.v{IPC_VERSION}.{nonce}")
}

fn helper_names() -> Vec<String> {
    let target = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let environment = if cfg!(target_env = "msvc") {
        "msvc"
    } else {
        "gnu"
    };
    vec![
        spaceprune_access::windows_package::Component::Cleanup
            .filename()
            .into(),
        format!("spaceprune-cleanup-helper-{target}-pc-windows-{environment}.exe"),
    ]
}

pub(super) fn probe(operation: &OperationContext) -> Result<TrustedExecutable, PermissionOutcome> {
    operation
        .check()
        .map_err(super::session::operation_failure)?;
    if let Some(image) = spaceprune_access::windows_native::ensure_helper(
        spaceprune_access::windows_package::Component::Cleanup,
        operation,
    )? {
        return Ok(image);
    }
    let host = std::env::current_exe().map_err(|_| PermissionOutcome::AuthorizationUnavailable)?;
    let directory = host.parent().ok_or(PermissionOutcome::HelperMissing)?;
    for name in helper_names() {
        let path = directory.join(name);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return Err(PermissionOutcome::HelperUntrusted),
            Ok(_) => {}
        }
        let image = spaceprune_windows_elevation::trusted_installed_executable(&path)
            .ok_or(PermissionOutcome::HelperUntrusted)?;
        let mut command = std::process::Command::new(image.path());
        command
            .arg(spaceprune_access::windows_package::Component::Cleanup.support_argument())
            .current_dir(directory);
        let output =
            spaceprune_access::process::capture(command, &operation.child(Duration::from_secs(5)))
                .map_err(super::session::operation_failure)?;
        return if output.status.success() {
            Ok(image)
        } else {
            Err(PermissionOutcome::HelperOutdated)
        };
    }
    Err(PermissionOutcome::HelperMissing)
}

fn current_image() -> io::Result<TrustedExecutable> {
    let image = std::env::current_exe()?;
    if !image
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            helper_names()
                .iter()
                .any(|allowed| name.eq_ignore_ascii_case(allowed))
        })
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    spaceprune_windows_elevation::trusted_installed_executable(&image)
        .ok_or_else(|| io::ErrorKind::PermissionDenied.into())
}

pub(super) fn run_if_requested(
    argument: OsString,
    mut args: impl Iterator<Item = OsString>,
) -> Option<i32> {
    if argument != WORKER_FLAG {
        return None;
    }
    let (Some(path), Some(nonce), Some(host_pid), Some(host_created), None) = (
        args.next(),
        args.next(),
        args.next(),
        args.next(),
        args.next(),
    ) else {
        return Some(64);
    };
    let (Some(nonce), Some(host_pid), Some(host_created)) = (
        nonce.to_str(),
        host_pid
            .to_str()
            .and_then(|value| value.parse::<u32>().ok()),
        host_created
            .to_str()
            .and_then(|value| value.parse::<u64>().ok()),
    ) else {
        return Some(64);
    };
    Some(
        if serve(&PathBuf::from(path), nonce, host_pid, host_created).is_ok() {
            0
        } else {
            77
        },
    )
}

fn serve(path: &Path, nonce: &str, host_pid: u32, host_created: u64) -> io::Result<()> {
    let _image = current_image()?;
    if !spaceprune_windows_elevation::is_elevated()
        || nonce.len() != 64
        || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
        || path.file_name() != Some(std::ffi::OsStr::new("bootstrap.json"))
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    if !spaceprune_windows_elevation::valid_metadata_root(directory) {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    if !directory
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("spaceprune-cleanup-"))
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let file = File::open(path)?;
    let identity = spaceprune_windows_elevation::logon_open_file_identity(path, &file)
        .map_err(|_| io::ErrorKind::PermissionDenied)?;
    if spaceprune_windows_elevation::logon_path_identity(directory, PrivatePathKind::Directory)
        .map_err(|_| io::ErrorKind::PermissionDenied)?
        != identity
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BOOTSTRAP + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BOOTSTRAP {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let bootstrap: Bootstrap = serde_json::from_slice(&bytes)?;
    if bootstrap.version != IPC_VERSION
        || bootstrap.nonce != nonce
        || bootstrap.host_pid != host_pid
        || bootstrap.host_created != host_created
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let context = OperationContext::from_transfer(
        &bootstrap.operation,
        Duration::from_secs(3600),
        spaceprune_access::ResourceBudget::workflow(),
    )?;
    context.check()?;
    let host = spaceprune_windows_elevation::HostProcess::observe_logon(
        bootstrap.host_pid,
        bootstrap.host_created,
        &identity,
    )
    .ok_or_else(|| io::Error::from(io::ErrorKind::PermissionDenied))?;
    let startup = context.child(Duration::from_secs(120));
    let pipe = loop {
        startup.check()?;
        if !host.is_running() || !directory.is_dir() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        match LocalPipe::connect(&pipe_name(nonce)) {
            Ok(pipe) => break pipe,
            Err(error) if matches!(error.raw_os_error(), Some(2 | 231)) => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(error) => return Err(error),
        }
    };
    if pipe.peer_process_id()? != bootstrap.host_pid || !host.is_running() {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    fs::remove_file(path)?;
    let finished = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !finished.load(Ordering::Acquire) && !context.is_stopped() {
                if !host.is_running() || !directory.is_dir() {
                    context.cancel();
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let result = (|| {
            let input = PipeReader {
                pipe: pipe.try_clone()?,
                context: startup,
            };
            let mut input = std::io::BufReader::new(input);
            let init = super::worker::frame(&mut input)?
                .ok_or_else(|| io::Error::other("Missing confirmed plan"))?;
            input.get_mut().context = context.clone();
            super::worker::serve_grant(input, Box::new(PipeWriter(pipe)), Some(&context), init)
        })();
        context.cancel();
        finished.store(true, Ordering::Release);
        result
    })
}

pub(super) struct PipeReader {
    pub(super) pipe: LocalPipe,
    pub(super) context: OperationContext,
}
impl Read for PipeReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            self.context.check().map_err(|error| {
                // Buffered readers retry Interrupted indefinitely. Cancellation is permanent,
                // so return a terminal connection error and let shutdown join this reader.
                if error.kind() == io::ErrorKind::Interrupted {
                    io::Error::new(io::ErrorKind::ConnectionAborted, error)
                } else {
                    error
                }
            })?;
            match self.pipe.try_read(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                result => return result,
            }
        }
    }
}
struct PipeWriter(LocalPipe);
impl Write for PipeWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.try_write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_pipe_reader_ends_a_buffered_frame() {
        let temp = tempfile::tempdir().unwrap();
        let pipe = LocalPipe::create(&pipe_name(
            temp.path().file_name().unwrap().to_str().unwrap(),
        ))
        .unwrap();
        let context = OperationContext::default();
        context.cancel();
        let (sender, receiver) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let result = super::super::worker::frame(&mut std::io::BufReader::new(PipeReader {
                pipe,
                context,
            }));
            sender.send(result).unwrap();
        });
        let result = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("cancelled frame reader must exit so cleanup shutdown can join it");
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
        reader.join().unwrap();
    }
}
