#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! Linux package-manager and systemd lifecycle capabilities.
#[cfg(target_os = "linux")]
include!("linux.rs");
