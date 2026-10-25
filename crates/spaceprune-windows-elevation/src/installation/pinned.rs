//! Pin directory components before opening the leaf, including ordinary-user package sources.
mod relative;

use std::{
    fs::{self, File, OpenOptions},
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf, Prefix},
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE,
    READ_CONTROL,
};

pub(super) struct PinnedFile {
    pub(super) path: PathBuf,
    handles: Vec<File>,
}
impl PinnedFile {
    pub(super) fn file(&self) -> &File {
        self.handles.last().expect("pinned leaf file")
    }

    pub(super) fn open(path: &Path, installed: bool) -> Option<Self> {
        if !local_absolute(path)
            || !path.ancestors().all(|part| {
                fs::symlink_metadata(part).is_ok_and(|metadata| {
                    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
                })
            })
        {
            return None;
        }
        let path = fs::canonicalize(path).ok()?;
        if !local_absolute(&path) {
            return None;
        }
        let root = if installed {
            Some(
                crate::native::program_files_roots()
                    .into_iter()
                    .filter_map(|root| fs::canonicalize(root).ok())
                    .find(|root| path != *root && path.starts_with(root))?,
            )
        } else {
            None
        };
        let mut handles = Vec::new();
        let ancestors: Vec<_> = path.ancestors().collect();
        for part in ancestors.into_iter().rev() {
            let leaf = part == path;
            let protected = root.as_ref().is_some_and(|root| part.starts_with(root));
            let access = FILE_READ_ATTRIBUTES
                | if leaf { FILE_READ_DATA } else { FILE_TRAVERSE }
                | if protected { READ_CONTROL } else { 0 };
            let sharing = FILE_SHARE_READ | if leaf { 0 } else { FILE_SHARE_WRITE };
            let file = if let Some(parent) = handles.last() {
                relative::open(parent, part.file_name()?, access, sharing, !leaf)?
            } else {
                OpenOptions::new()
                    .read(true)
                    .access_mode(access)
                    .share_mode(sharing)
                    .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                    .open(part)
                    .ok()?
            };
            let metadata = file.metadata().ok()?;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
                || if leaf {
                    !metadata.is_file()
                } else {
                    !metadata.is_dir()
                }
                || (protected && !super::acl::administrator_owned(&file))
            {
                return None;
            }
            handles.push(file);
        }
        Some(Self { path, handles })
    }
}

fn local_absolute(path: &Path) -> bool {
    path.is_absolute()
        && matches!(path.components().next(), Some(Component::Prefix(prefix))
        if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_handles_prevent_leaf_writes_and_directory_replacement_until_released() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("package");
        fs::create_dir(&directory).unwrap();
        let source = directory.join("fixture.msi");
        fs::write(
            &source,
            b"unsigned fixture, tested only below the signature boundary",
        )
        .unwrap();
        let pinned = PinnedFile::open(&source, false).unwrap();
        assert!(OpenOptions::new().write(true).open(&source).is_err());
        assert!(fs::rename(&directory, root.path().join("moved")).is_err());
        drop(pinned);
        fs::write(&source, b"released").unwrap();
        fs::rename(directory, root.path().join("moved")).unwrap();
    }
    #[test]
    fn network_and_device_names_are_not_local_package_sources() {
        for path in [
            r"\\server\share\package.msi",
            r"\\?\UNC\server\share\package.msi",
            r"\\.\pipe\package",
            r"C:relative.msi",
        ] {
            assert!(!local_absolute(Path::new(path)));
        }
        assert!(local_absolute(Path::new(r"C:\package.msi")));
        assert!(local_absolute(Path::new(r"\\?\C:\package.msi")));
    }
}
