use super::lexical::WindowsPath;
use std::{
    ffi::OsString,
    fs,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Globalization::{CSTR_EQUAL, CompareStringOrdinal},
    Storage::FileSystem::{GetVolumeNameForVolumeMountPointW, GetVolumePathNameW},
    System::SystemInformation::GetSystemWindowsDirectoryW,
    UI::Shell::{
        FOLDERID_ProgramData, FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX64,
        FOLDERID_ProgramFilesX86, KF_FLAG_DONT_VERIFY,
    },
};

const WIDE_CAPACITY: usize = 32_768;

/// A process-local snapshot of OS-configured system directories, not an authorization grant.
#[derive(Debug, Clone)]
pub struct SystemLocations {
    windows: PathBuf,
    system_volume: PathBuf,
    system_volume_aliases: Vec<WindowsPath>,
    protected: Vec<WindowsPath>,
}

impl SystemLocations {
    /// Query Windows and Known Folder APIs. Failure leaves system classification unavailable.
    /// No SystemDrive, SystemRoot, ProgramFiles or PATH environment variable selects these paths.
    pub fn query() -> Option<Self> {
        let mut buffer = vec![0_u16; WIDE_CAPACITY];
        // SAFETY: The output buffer has the exact writable capacity passed to the OS.
        let length = unsafe { GetSystemWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) }
            as usize;
        if length == 0 || length >= buffer.len() {
            return None;
        }
        let windows = PathBuf::from(OsString::from_wide(&buffer[..length]));
        let canonical_windows = fs::canonicalize(&windows).ok()?;
        let system_volume = volume_root(&canonical_windows)?;
        let system_volume_aliases = aliases(&system_volume)?;
        let mut protected = aliases(&windows)?;
        for id in [
            FOLDERID_ProgramFiles,
            FOLDERID_ProgramFilesX86,
            FOLDERID_ProgramData,
        ] {
            let path =
                crate::native::known_folder_path_with_flags(&id, KF_FLAG_DONT_VERIFY as u32)?;
            protected.extend(aliases(&path)?);
        }
        // This ID is unavailable to some architectures; the native Program Files ID above is mandatory.
        if let Some(path) = crate::native::known_folder_path_with_flags(
            &FOLDERID_ProgramFilesX64,
            KF_FLAG_DONT_VERIFY as u32,
        ) {
            protected.extend(aliases(&path)?);
        }
        Some(Self {
            windows,
            system_volume,
            system_volume_aliases,
            protected,
        })
    }

    /// The actual shared Windows directory returned by the OS, including a relocated installation.
    pub fn windows_directory(&self) -> &Path {
        &self.windows
    }

    /// The mount root containing Windows, usable as an inventory fallback without environment variables.
    pub fn system_volume_root(&self) -> &Path {
        &self.system_volume
    }

    /// Match only the exact system volume root, including its DOS and volume-GUID aliases.
    pub fn is_system_volume(&self, path: &Path) -> bool {
        let Some(path) = path_units(path) else {
            return false;
        };
        self.system_volume_aliases
            .iter()
            .any(|root| root.same(&path, ordinal_equal) == Some(true))
    }

    /// Classify canonical host paths against configured system directories on any drive.
    /// Malformed namespaces and comparison failures remain protected; this grants no filesystem access.
    pub fn is_protected(&self, path: &Path) -> bool {
        let Some(path) = path_units(path) else {
            return true;
        };
        self.protected
            .iter()
            .any(|root| root.contains(&path, ordinal_equal) != Some(false))
    }
}

fn path_units(path: &Path) -> Option<WindowsPath> {
    WindowsPath::parse(&path.as_os_str().encode_wide().collect::<Vec<_>>())
}

fn ordinal_equal(left: &[u16], right: &[u16]) -> Option<bool> {
    // SAFETY: Both slices remain live for the call, and checked lengths exclude truncation.
    let result = unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            i32::try_from(left.len()).ok()?,
            right.as_ptr(),
            i32::try_from(right.len()).ok()?,
            1,
        )
    };
    (result != 0).then_some(result == CSTR_EQUAL)
}

fn wide_null(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn volume_root(path: &Path) -> Option<PathBuf> {
    let path = wide_null(path);
    let mut buffer = vec![0_u16; WIDE_CAPACITY];
    // SAFETY: The path is NUL terminated and output storage has the declared capacity.
    if unsafe { GetVolumePathNameW(path.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) } == 0 {
        return None;
    }
    let length = buffer.iter().position(|&unit| unit == 0)?;
    Some(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

fn volume_alias(path: &Path) -> Option<PathBuf> {
    let root = volume_root(path)?;
    let root_units = path_units(&root)?;
    let units = path_units(path)?;
    if root_units.contains(&units, ordinal_equal) != Some(true) {
        return None;
    }
    let root = wide_null(&root);
    let mut buffer = vec![0_u16; WIDE_CAPACITY];
    // SAFETY: The OS-provided mount root includes its trailing separator; all buffers stay live.
    if unsafe {
        GetVolumeNameForVolumeMountPointW(root.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32)
    } == 0
    {
        return None;
    }
    let length = buffer.iter().position(|&unit| unit == 0)?;
    let mut alias = PathBuf::from(OsString::from_wide(&buffer[..length]));
    let relative = &units.units[root_units.units.len()..];
    let relative = relative.strip_prefix(&[b'\\' as u16]).unwrap_or(relative);
    if !relative.is_empty() {
        alias.push(OsString::from_wide(relative));
    }
    Some(alias)
}

fn aliases(path: &Path) -> Option<Vec<WindowsPath>> {
    // Resolve an existing configured junction before deriving its physical volume alias.
    // The original configured spelling remains protected as well.
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let paths = vec![
        path_units(path)?,
        path_units(&canonical)?,
        path_units(&volume_alias(&canonical)?)?,
    ];
    let mut unique = Vec::<WindowsPath>::new();
    for path in paths {
        if !unique
            .iter()
            .any(|other| other.same(&path, ordinal_equal) == Some(true))
        {
            unique.push(path);
        }
    }
    Some(unique)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protection_tracks_configured_directories_on_other_drives_and_native_case_rules() {
        let locations = SystemLocations {
            windows: PathBuf::from(r"E:\Système"),
            system_volume: PathBuf::from(r"E:\"),
            system_volume_aliases: vec![path_units(Path::new(r"E:\")).unwrap()],
            protected: [r"E:\Système", r"D:\Installed Apps", r"D:\Machine Data"]
                .map(|value| path_units(Path::new(value)).unwrap())
                .to_vec(),
        };
        for path in [
            r"\\?\e:\SYSTÈME\System32",
            r"D:\Installed Apps\tool",
            r"\\?\d:\MACHINE DATA\app",
        ] {
            assert!(locations.is_protected(Path::new(path)), "{path}");
        }
        for path in [
            r"C:\Windows",
            r"E:\Windows",
            r"D:\Installed Apps-old",
            r"D:\Users\Machine Data",
            r"E:\cache",
        ] {
            assert!(!locations.is_protected(Path::new(path)), "{path}");
        }
        assert!(locations.is_system_volume(Path::new(r"\\?\e:\")));
        assert!(!locations.is_system_volume(Path::new(r"D:\")));
        assert!(!locations.is_system_volume(Path::new(r"E:\cache")));
    }
    #[test]
    fn native_locations_protect_canonical_and_volume_aliases_without_touching_system_files() {
        let locations = SystemLocations::query().expect("OS system locations");
        assert!(locations.is_system_volume(locations.system_volume_root()));
        for path in [
            locations.windows_directory().to_owned(),
            fs::canonicalize(locations.windows_directory()).unwrap(),
            volume_alias(locations.windows_directory()).unwrap(),
        ] {
            assert!(locations.is_protected(&path));
            assert!(locations.is_protected(&path.join("System32")));
        }
        assert!(locations.is_protected(Path::new(r"\\.\pipe\not-a-file")));
    }

    #[test]
    fn changing_environment_variables_does_not_redirect_system_classification() {
        let locations = SystemLocations::query().expect("OS system locations");
        let program_files = crate::native::known_folder_path_with_flags(
            &FOLDERID_ProgramFiles,
            KF_FLAG_DONT_VERIFY as u32,
        )
        .unwrap();
        let program_data = crate::native::known_folder_path_with_flags(
            &FOLDERID_ProgramData,
            KF_FLAG_DONT_VERIFY as u32,
        )
        .unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "system_locations::native::tests::system_locations_environment_child",
            ])
            .env("SystemDrive", "Q:")
            .env("SystemRoot", r"Q:\UntrustedWindows")
            .env("ProgramFiles", r"Q:\UntrustedPrograms")
            .env("ProgramData", r"Q:\UntrustedData")
            .env(
                "SPACEPRUNE_TEST_EXPECTED_WINDOWS",
                locations.windows_directory(),
            )
            .env(
                "SPACEPRUNE_TEST_EXPECTED_VOLUME",
                locations.system_volume_root(),
            )
            .env("SPACEPRUNE_TEST_EXPECTED_PROGRAMS", program_files)
            .env("SPACEPRUNE_TEST_EXPECTED_DATA", program_data)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    #[ignore = "Spawned with isolated environment variables by the native locations test"]
    fn system_locations_environment_child() {
        let expected =
            |name| PathBuf::from(std::env::var_os(name).expect("native test environment"));
        let locations =
            SystemLocations::query().expect("OS system locations after environment changes");
        assert_eq!(
            locations.windows_directory(),
            expected("SPACEPRUNE_TEST_EXPECTED_WINDOWS")
        );
        assert!(locations.is_system_volume(&expected("SPACEPRUNE_TEST_EXPECTED_VOLUME")));
        assert!(locations.is_protected(&expected("SPACEPRUNE_TEST_EXPECTED_PROGRAMS")));
        assert!(locations.is_protected(&expected("SPACEPRUNE_TEST_EXPECTED_DATA")));
    }
}
