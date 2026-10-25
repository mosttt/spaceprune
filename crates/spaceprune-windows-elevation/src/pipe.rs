//! Local duplex IPC with explicit access control, kernel peer IDs and nonblocking operations.

use std::{
    ffi::OsStr,
    fs::File,
    io,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle},
    },
    ptr,
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING,
        INVALID_HANDLE_VALUE, LocalFree,
    },
    Security::{
        Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
        PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile,
        SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile,
    },
    System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
        GetNamedPipeServerProcessId, PIPE_NOWAIT, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, SetNamedPipeHandleState,
    },
};

/// A local pipe endpoint. Callers retain their operation budget while polling these methods.
pub struct LocalPipe {
    file: File,
    server: bool,
}

fn name(value: &str) -> io::Result<Vec<u16>> {
    let Some(suffix) = value.strip_prefix(r"\\.\pipe\SpacePrune.") else {
        return Err(io::ErrorKind::InvalidInput.into());
    };
    if suffix.is_empty()
        || suffix.len() > 128
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(OsStr::new(value).encode_wide().chain(Some(0)).collect())
}

impl LocalPipe {
    /// Create the first and only instance, granting access only to the caller's logon, SYSTEM and admins.
    pub fn create(value: &str) -> io::Result<Self> {
        let name = name(value)?;
        let owner =
            crate::private_acl::current_logon_sid().map_err(|_| io::ErrorKind::PermissionDenied)?;
        let descriptor = Descriptor::new(&format!(
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{owner})(A;;RC;;;OW)"
        ))?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        // SAFETY: Name and security descriptor are held; the unique pipe rejects remote clients.
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                64 * 1024,
                64 * 1024,
                0,
                &attributes,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            file: unsafe { File::from_raw_handle(handle) },
            server: true,
        })
    }

    /// Connect without granting the server the ability to impersonate an elevated client.
    pub fn connect(value: &str) -> io::Result<Self> {
        let name = name(value)?;
        // SAFETY: The local endpoint is validated; SECURITY_IDENTIFICATION prevents token impersonation.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                0x8000_0000 | 0x4000_0000,
                0,
                ptr::null(),
                OPEN_EXISTING,
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let endpoint = Self {
            file: unsafe { File::from_raw_handle(handle) },
            server: false,
        };
        let mode = PIPE_READMODE_BYTE | PIPE_NOWAIT;
        // SAFETY: Endpoint owns the handle; byte/nonblocking mode preserves a single ordered stream.
        if unsafe { SetNamedPipeHandleState(handle, &mode, ptr::null(), ptr::null()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(endpoint)
    }

    /// Poll a server connection without blocking cancellation or the authorization deadline.
    pub fn poll_connected(&self) -> io::Result<bool> {
        if !self.server {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        // SAFETY: PIPE_NOWAIT makes a synchronous null-overlapped connect nonblocking.
        if unsafe { ConnectNamedPipe(self.file.as_raw_handle(), ptr::null_mut()) } != 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error().map(|code| code as u32) {
            Some(ERROR_PIPE_CONNECTED) => Ok(true),
            Some(ERROR_PIPE_LISTENING) => Ok(false),
            Some(ERROR_NO_DATA) => {
                self.disconnect();
                Ok(false)
            }
            _ => Err(error),
        }
    }

    /// Get the connected peer's actual kernel PID, never a value supplied inside a message.
    pub fn peer_process_id(&self) -> io::Result<u32> {
        let mut pid = 0;
        // SAFETY: The pipe is held and the output PID is writable.
        let result = unsafe {
            if self.server {
                GetNamedPipeClientProcessId(self.file.as_raw_handle(), &raw mut pid)
            } else {
                GetNamedPipeServerProcessId(self.file.as_raw_handle(), &raw mut pid)
            }
        };
        if result == 0 || pid == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(pid)
        }
    }

    /// Clone a local endpoint for a separately owned reader or writer.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            file: self.file.try_clone()?,
            server: self.server,
        })
    }

    /// Withdraw the server connection, waking both peers' readers and writers.
    pub fn disconnect(&self) {
        if self.server {
            // SAFETY: The server handle is held. Disconnection does not close Rust-owned handles.
            unsafe {
                DisconnectNamedPipe(self.file.as_raw_handle());
            }
        }
    }

    /// Read available bytes; an empty live pipe returns WouldBlock, disconnection returns zero.
    pub fn try_read(&self, bytes: &mut [u8]) -> io::Result<usize> {
        let mut read = 0;
        let length = bytes.len().min(u32::MAX as usize) as u32;
        // SAFETY: The writable slice covers length bytes; PIPE_NOWAIT prevents an empty-pipe wait.
        if unsafe {
            ReadFile(
                self.file.as_raw_handle(),
                bytes.as_mut_ptr(),
                length,
                &raw mut read,
                ptr::null_mut(),
            )
        } != 0
        {
            return Ok(read as usize);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error().map(|code| code as u32) {
            Some(ERROR_NO_DATA | ERROR_PIPE_LISTENING) => Err(io::ErrorKind::WouldBlock.into()),
            Some(ERROR_BROKEN_PIPE) => Ok(0),
            _ => Err(error),
        }
    }

    /// Write available capacity; a full pipe returns WouldBlock for a budget-aware caller to retry.
    pub fn try_write(&self, bytes: &[u8]) -> io::Result<usize> {
        let mut written = 0;
        let length = bytes.len().min(u32::MAX as usize) as u32;
        // SAFETY: The input slice covers length bytes and the pipe is explicitly nonblocking.
        if unsafe {
            WriteFile(
                self.file.as_raw_handle(),
                bytes.as_ptr(),
                length,
                &raw mut written,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if written == 0 && !bytes.is_empty() {
            Err(io::ErrorKind::WouldBlock.into())
        } else {
            Ok(written as usize)
        }
    }
}

struct Descriptor(PSECURITY_DESCRIPTOR);
impl Descriptor {
    fn new(value: &str) -> io::Result<Self> {
        let value = OsStr::new(value)
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let mut descriptor = ptr::null_mut();
        // SAFETY: The SDDL buffer is terminated; the returned allocation is owned by Descriptor.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                value.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: The descriptor allocation is released once after CreateNamedPipe has copied it.
        unsafe {
            LocalFree(self.0);
        }
    }
}

#[cfg(test)]
mod tests;
