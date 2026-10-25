//! Read OS-owned system locations and compare native path spellings without trusting environment variables.
mod lexical;
#[cfg(windows)]
#[allow(unsafe_code)]
mod native;
#[cfg(windows)]
pub use native::SystemLocations;
