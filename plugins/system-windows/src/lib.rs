#![deny(missing_docs)]
//! Native Windows maintenance. Shell objects and servicing units never become raw directory rules.
#[cfg(windows)]
#[allow(unsafe_code)]
mod native;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{discover, register};
