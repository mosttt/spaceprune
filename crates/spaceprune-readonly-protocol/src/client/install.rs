//! Compatibility facade for Core's shared component installer.

pub use spaceprune_access::helper::{InstallError, can_install, install};
use std::path::{Path, PathBuf};

/// Basename of the metadata component bundled beside a host executable.
pub const HELPER_NAME: &str = "spaceprune-readonly-helper";

/// Find only the fixed adjacent metadata executable without following a symlink.
pub fn packaged_helper_at(host: &Path) -> Option<PathBuf> {
    spaceprune_access::helper::packaged_at(host, HELPER_NAME)
}
