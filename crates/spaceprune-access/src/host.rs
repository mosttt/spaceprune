//! Stable local host and user-root resolution, without cloud account or model identities.

use std::{io, path::PathBuf};

/// Read the native machine identity used to bind local-only rule locations.
pub fn identity() -> io::Result<String> {
    #[cfg(target_os = "linux")]
    {
        let value = std::fs::read_to_string("/etc/machine-id")?;
        let value = value.trim();
        if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(io::Error::other("Invalid local machine identity"));
        }
        Ok(format!("linux:{}", value.to_ascii_lowercase()))
    }
    #[cfg(target_os = "windows")]
    {
        native::windows_identity()
    }
    #[cfg(target_os = "macos")]
    {
        native::macos_identity()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Local machine binding is unavailable on this platform",
        ))
    }
}

/// The invoking user's home. Elevated workers consume paths resolved by the original host.
pub fn user_home() -> io::Result<PathBuf> {
    #[cfg(windows)]
    let value = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let value = std::env::var_os("HOME");
    value
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Current user's home is unavailable",
            )
        })
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[allow(unsafe_code)]
mod native {
    #[cfg(windows)]
    pub(super) fn windows_identity() -> std::io::Result<String> {
        use windows_sys::Win32::System::Registry::*;
        let key = "SOFTWARE\\Microsoft\\Cryptography\0"
            .encode_utf16()
            .collect::<Vec<_>>();
        let name = "MachineGuid\0".encode_utf16().collect::<Vec<_>>();
        let mut value = [0_u16; 128];
        let mut bytes = std::mem::size_of_val(&value) as u32;
        // SAFETY: Both names are terminated and the buffer's byte capacity is exact. The registry
        // API is read-only and receives no handles or values supplied by a remote caller.
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
                std::ptr::null_mut(),
                value.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if status != 0 {
            return Err(std::io::Error::from_raw_os_error(status as i32));
        }
        let length = value
            .iter()
            .position(|unit| *unit == 0)
            .ok_or_else(|| std::io::Error::other("Invalid MachineGuid"))?;
        let value = String::from_utf16(&value[..length]).map_err(std::io::Error::other)?;
        if value.is_empty() {
            return Err(std::io::Error::other("Missing MachineGuid"));
        }
        Ok(format!("windows:{}", value.to_ascii_lowercase()))
    }

    #[cfg(target_os = "macos")]
    pub(super) fn macos_identity() -> std::io::Result<String> {
        let mut bytes = [0_u8; 128];
        let mut length = bytes.len();
        // SAFETY: This is a fixed read-only sysctl with a bounded writable buffer and no new value.
        let result = unsafe {
            libc::sysctlbyname(
                c"kern.hostuuid".as_ptr(),
                bytes.as_mut_ptr().cast(),
                &mut length,
                std::ptr::null_mut(),
                0,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let value = std::str::from_utf8(&bytes[..length.min(bytes.len())])
            .map_err(std::io::Error::other)?
            .trim_end_matches('\0');
        if value.is_empty() {
            return Err(std::io::Error::other("Missing host UUID"));
        }
        Ok(format!("macos:{}", value.to_ascii_lowercase()))
    }
}
