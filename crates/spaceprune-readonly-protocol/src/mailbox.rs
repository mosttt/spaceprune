//! Private, sequenced IPC for UAC processes, whose standard streams cannot be inherited.

use crate::*;
use spaceprune_access::NativePath;

pub(super) const ARGUMENT: &str = "--spaceprune-windows-metadata-session";
#[cfg(target_os = "windows")]
pub(super) fn support_argument() -> String {
    spaceprune_access::windows_package::Component::Metadata.support_argument()
}
const VERSION: u16 = spaceprune_access::windows_package::METADATA_PROTOCOL;
#[cfg(target_os = "windows")]
const IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const MAX_FRAME: usize = 64 * 1024;
const LEASE_FILE: &str = "session.active";
#[cfg(target_os = "windows")]
type IpcOwner = spaceprune_windows_elevation::MetadataIpcIdentity;
#[cfg(all(test, target_os = "linux"))]
type IpcOwner = String;

fn account_sid(owner: &IpcOwner) -> &str {
    #[cfg(target_os = "windows")]
    {
        owner.account_sid()
    }
    #[cfg(all(test, target_os = "linux"))]
    {
        owner
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bootstrap {
    version: u16,
    nonce: String,
    context: OperationTransfer,
    host_pid: u32,
    host_created: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandFrame {
    nonce: String,
    sequence: u64,
    request_path: NativePath,
    request_nonce: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    version: u16,
    nonce: String,
    sequence: u64,
    code: u16,
}

pub(super) struct Mailbox {
    directory: tempfile::TempDir,
    nonce: String,
    sequence: u64,
}

impl Mailbox {
    pub(super) fn create(context: &OperationContext) -> Result<Self, HelperError> {
        context.check().map_err(|_| HelperError)?;
        let directory = tempfile::Builder::new()
            .prefix("spaceprune-readonly-")
            .tempdir()
            .map_err(|_| HelperError)?;
        tighten_directory_permissions(directory.path())?;
        let nonce = random_nonce()?;
        #[cfg(target_os = "windows")]
        let host_created =
            spaceprune_windows_elevation::current_process_creation_time().ok_or(HelperError)?;
        #[cfg(all(test, target_os = "linux"))]
        let host_created = 0;
        let bootstrap = Bootstrap {
            host_pid: std::process::id(),
            host_created,
            version: VERSION,
            nonce: nonce.clone(),
            context: context
                .child(REQUEST_LIFETIME)
                .transfer()
                .map_err(|_| HelperError)?,
        };
        publish(&directory.path().join(LEASE_FILE), &nonce, true)?;
        publish(&directory.path().join("bootstrap.json"), &bootstrap, true)?;
        Ok(Self {
            directory,
            nonce,
            sequence: 0,
        })
    }

    pub(super) fn arguments(&self) -> [OsString; 3] {
        [
            ARGUMENT.into(),
            self.directory
                .path()
                .join("bootstrap.json")
                .into_os_string(),
            self.nonce.clone().into(),
        ]
    }

    pub(super) fn send(&mut self, request: &PreparedRequest) -> Result<(), HelperError> {
        self.sequence = self.sequence.checked_add(1).ok_or(HelperError)?;
        publish(
            &self.directory.path().join("command.json"),
            &CommandFrame {
                nonce: self.nonce.clone(),
                sequence: self.sequence,
                request_path: NativePath::from_path(&request.request_path),
                request_nonce: request.nonce.clone(),
            },
            true,
        )
    }

    pub(super) fn poll(&self) -> Result<Option<u16>, HelperError> {
        let path = self.directory.path().join("reply.json");
        let Some(reply) = read::<Reply>(&path, false)? else {
            return Ok(None);
        };
        if reply.version != VERSION || reply.nonce != self.nonce || reply.sequence != self.sequence
        {
            return Err(HelperError);
        }
        fs::remove_file(path).map_err(|_| HelperError)?;
        Ok(Some(reply.code))
    }

    pub(super) fn close(&self) -> Result<(), HelperError> {
        // Withdraw the lease first: unrelated leftover files must not keep a request alive.
        let revoke = fs::remove_file(self.directory.path().join(LEASE_FILE));
        let cleanup = fs::remove_dir_all(self.directory.path());
        if revoke.is_ok()
            || revoke.is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            || cleanup.is_ok()
            || cleanup.is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            Ok(())
        } else {
            Err(HelperError)
        }
    }
}

#[cfg(target_os = "windows")]
pub(super) fn run(bootstrap: &Path, nonce: &str) -> Result<(), HelperError> {
    let _installation = crate::client::trust::current_helper()?;
    if !spaceprune_windows_elevation::is_elevated() {
        return Err(HelperError);
    }
    serve(
        bootstrap,
        nonce,
        IDLE_TIMEOUT,
        |path, nonce, owner, alive| {
            let stopped = || !alive();
            worker::run_helper_for_owner(path, nonce, Some(owner), Some(&stopped))
        },
    )
}

fn serve(
    bootstrap: &Path,
    nonce: &str,
    idle: Duration,
    mut execute: impl FnMut(&Path, &str, &str, &(dyn Fn() -> bool + Sync)) -> Result<(), HelperError>,
) -> Result<(), HelperError> {
    if !valid_nonce(nonce) || bootstrap.file_name() != Some(OsStr::new("bootstrap.json")) {
        return Err(HelperError);
    }
    let directory = bootstrap.parent().ok_or(HelperError)?;
    let parent_name = directory
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or(HelperError)?;
    if !parent_name.starts_with("spaceprune-readonly-") {
        return Err(HelperError);
    }
    let caller = owner(directory)?;
    let header = read::<Bootstrap>(bootstrap, true)?.ok_or(HelperError)?;
    if header.version != VERSION || header.nonce != nonce {
        return Err(HelperError);
    }
    OperationContext::from_transfer(
        &header.context,
        REQUEST_LIFETIME,
        ResourceBudget::workflow(),
    )
    .map_err(|_| HelperError)?
    .check()
    .map_err(|_| HelperError)?;
    let alive = observe_host(&header, &caller)?;
    let lease = directory.join(LEASE_FILE);
    let session_alive = || alive() && lease.is_file();
    if !session_alive() || owner(directory)? != caller {
        return Err(HelperError);
    }
    fs::remove_file(bootstrap).map_err(|_| HelperError)?;
    let reply = directory.join("reply.json");
    publish(
        &reply,
        &Reply {
            version: VERSION,
            nonce: nonce.into(),
            sequence: 0,
            code: 0,
        },
        false,
    )?;
    let mut sequence = 0_u64;
    let mut deadline = Instant::now() + idle;
    let command = directory.join("command.json");
    loop {
        if !session_alive() || Instant::now() >= deadline {
            return Ok(());
        }
        if let Some(frame) = read::<CommandFrame>(&command, true)? {
            if owner(directory)? != caller
                || frame.nonce != nonce
                || sequence.checked_add(1) != Some(frame.sequence)
                || !valid_nonce(&frame.request_nonce)
            {
                return Err(HelperError);
            }
            let path = frame.request_path.to_path_buf().ok_or(HelperError)?;
            if !request_path_matches_nonce(&path, &frame.request_nonce)
                || owner(path.parent().ok_or(HelperError)?)? != caller
            {
                return Err(HelperError);
            }
            fs::remove_file(&command).map_err(|_| HelperError)?;
            sequence = frame.sequence;
            if !session_alive() {
                return Ok(());
            }
            let code = if execute(
                &path,
                &frame.request_nonce,
                account_sid(&caller),
                &session_alive,
            )
            .is_ok()
            {
                0
            } else {
                70
            };
            // Closing the host's private directory withdraws this connection.
            if !session_alive() {
                return Ok(());
            }
            publish(
                &reply,
                &Reply {
                    version: VERSION,
                    nonce: nonce.into(),
                    sequence,
                    code,
                },
                false,
            )?;
            deadline = Instant::now() + idle;
        } else {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn observe_host(
    header: &Bootstrap,
    caller: &IpcOwner,
) -> Result<Box<dyn Fn() -> bool + Sync>, HelperError> {
    #[cfg(target_os = "windows")]
    {
        let host = spaceprune_windows_elevation::HostProcess::observe_metadata(
            header.host_pid,
            header.host_created,
            caller,
        )
        .ok_or(HelperError)?;
        Ok(Box::new(move || host.is_running()))
    }
    #[cfg(all(test, target_os = "linux"))]
    {
        let _ = caller;
        if header.host_pid != std::process::id() || header.host_created != 0 {
            return Err(HelperError);
        }
        Ok(Box::new(|| true))
    }
}

fn owner(directory: &Path) -> Result<IpcOwner, HelperError> {
    #[cfg(target_os = "windows")]
    {
        spaceprune_windows_elevation::metadata_path_identity(
            directory,
            spaceprune_windows_elevation::PrivatePathKind::Directory,
        )
        .map_err(|_| HelperError)
    }
    #[cfg(all(test, target_os = "linux"))]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = fs::symlink_metadata(directory).map_err(|_| HelperError)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(HelperError);
        }
        Ok(metadata.uid().to_string())
    }
}

fn read<T: serde::de::DeserializeOwned>(
    path: &Path,
    private: bool,
) -> Result<Option<T>, HelperError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(HelperError),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(HelperError);
    }
    let file = File::open(path).map_err(|_| HelperError)?;
    if private {
        validate_open_ipc_file(path, &file)?;
    }
    let mut bytes = Vec::new();
    file.take(MAX_FRAME as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| HelperError)?;
    if bytes.len() > MAX_FRAME {
        return Err(HelperError);
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| HelperError)
}

fn publish(path: &Path, value: &impl Serialize, private: bool) -> Result<(), HelperError> {
    let bytes = serde_json::to_vec(value).map_err(|_| HelperError)?;
    if bytes.len() > MAX_FRAME {
        return Err(HelperError);
    }
    let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or(HelperError)?)
        .map_err(|_| HelperError)?;
    if private {
        secure_created_file(file.path())?;
    }
    file.write_all(&bytes).map_err(|_| HelperError)?;
    file.flush().map_err(|_| HelperError)?;
    file.persist(path).map_err(|_| HelperError)?;
    Ok(())
}

#[cfg(test)]
mod tests;
