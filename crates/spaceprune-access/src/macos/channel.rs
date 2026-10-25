//! Native Mach transport; Unix sockets only exercise the shared lifecycle on Linux.

#[cfg(target_os = "macos")]
mod mach;
#[cfg(target_os = "macos")]
pub(super) use mach::{Channel, Endpoint, Monitor, ServiceEndpoint};
#[cfg(all(test, target_os = "linux"))]
mod unix;
#[cfg(all(test, target_os = "linux"))]
pub(super) use unix::{Channel, Endpoint, Monitor};

#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
mod framing;
