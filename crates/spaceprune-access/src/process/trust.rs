//! Native executable identity checks used before privileged system operations.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// Resolve a system executable only through root-owned, non-writable ancestors.
pub fn trusted_executable(path: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let path = fs::canonicalize(path).ok()?;
    let trusted = fs::symlink_metadata(&path).is_ok_and(|metadata| {
        metadata.is_file()
            && metadata.uid() == 0
            && metadata.permissions().mode() & 0o022 == 0
            && metadata.permissions().mode() & 0o111 != 0
    }) && path.ancestors().skip(1).all(|directory| {
        fs::symlink_metadata(directory).is_ok_and(|metadata| {
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == 0
                && metadata.permissions().mode() & 0o022 == 0
        })
    });
    trusted.then_some(path)
}

/// Locate a trusted native system program without consulting the user's PATH.
pub fn system_program(name: &str) -> Option<PathBuf> {
    if Path::new(name).components().count() != 1 {
        return None;
    }
    ["/usr/bin", "/usr/sbin", "/bin", "/sbin"]
        .into_iter()
        .find_map(|directory| trusted_executable(&Path::new(directory).join(name)))
}
