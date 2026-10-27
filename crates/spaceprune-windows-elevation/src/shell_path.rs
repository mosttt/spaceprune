//! Shell and Installer APIs do not accept every path supported by CreateFileW.
use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Component, Path, PathBuf, Prefix},
};

/// Convert a pinned local path without allowing Win32 normalization to change its target.
pub(crate) fn local_path(path: &Path) -> Option<PathBuf> {
    let canonical = std::fs::canonicalize(path).ok()?;
    let Some(Component::Prefix(prefix)) = canonical.components().next() else {
        return None;
    };
    if !matches!(prefix.kind(), Prefix::VerbatimDisk(_)) {
        return None;
    }
    let wide: Vec<_> = canonical.as_os_str().encode_wide().collect();
    let ordinary = PathBuf::from(OsString::from_wide(&wide[4..]));
    // MSI's source resolver rejects the verbatim prefix. Retain all original handles and
    // reject names that would resolve differently after removing that prefix.
    (std::fs::canonicalize(&ordinary).ok()? == canonical).then_some(ordinary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_unicode_package_path_preserves_the_pinned_file() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("package 测试.msi");
        std::fs::write(&source, b"fixture").unwrap();
        let pinned = std::fs::canonicalize(&source).unwrap();
        let converted = local_path(&pinned).unwrap();
        assert!(
            matches!(converted.components().next(), Some(Component::Prefix(p))
            if matches!(p.kind(), Prefix::Disk(_)))
        );
        assert_eq!(std::fs::canonicalize(converted).unwrap(), pinned);
    }

    #[test]
    fn normalization_cannot_substitute_a_different_file() {
        let root = tempfile::tempdir().unwrap();
        let ordinary = root.path().join("package.msi");
        std::fs::write(&ordinary, b"other").unwrap();
        let verbatim = std::fs::canonicalize(root.path())
            .unwrap()
            .join("package.msi.");
        std::fs::write(&verbatim, b"pinned").unwrap();
        assert!(local_path(&verbatim).is_none());
        std::fs::remove_file(verbatim).unwrap();
    }
}
