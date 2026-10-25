//! Distinguish signed package data from administrator-protected executable authority.
mod acl;
mod pinned;
mod signature;

use pinned::PinnedFile;
use std::{fs::File, path::Path};

/// A signed file held against modification or replacement. This alone never authorizes execution.
pub struct SignedFile {
    pin: PinnedFile,
}
impl SignedFile {
    /// Canonical, locally pinned file path.
    pub fn path(&self) -> &Path {
        &self.pin.path
    }
    /// Read-only handle to the exact bytes whose signature was checked.
    pub fn file(&self) -> &File {
        self.pin.file()
    }
}

/// An administrator-owned, signed native image held until the elevated process starts.
pub struct TrustedExecutable {
    image: SignedFile,
}
impl TrustedExecutable {
    /// Canonical executable path protected by this guard.
    pub fn path(&self) -> &Path {
        self.image.path()
    }
    /// Read the already verified executable through the held handle when matching an exact payload.
    pub fn file(&self) -> &File {
        self.image.file()
    }
}

pub(super) fn trusted_executable(path: &Path) -> Option<TrustedExecutable> {
    let pin = PinnedFile::open(path, true)?;
    let image = signed(pin)?;
    Some(TrustedExecutable { image })
}

pub(super) fn signed_file(path: &Path) -> Option<SignedFile> {
    signed(PinnedFile::open(path, false)?)
}

fn signed(pin: PinnedFile) -> Option<SignedFile> {
    signature::signed_image(&pin.path, pin.file()).then_some(SignedFile { pin })
}
