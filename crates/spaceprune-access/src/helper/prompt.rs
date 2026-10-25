//! Fixed native prompt variants; language changes presentation, never the granted capability.

use super::*;
use std::ffi::OsStr;

impl HelperKind {
    /// Select the native prompt using the same effective language as CLI and Desktop.
    pub fn session_argument(self) -> &'static str {
        self.localized_session_argument(spaceprune_contracts::messages::display_language())
    }

    fn localized_session_argument(self, language: &str) -> &'static str {
        match (self, language) {
            (Self::Metadata, "zh-CN") => "--spaceprune-readonly-session-zh-CN",
            (Self::Metadata, _) => "--spaceprune-readonly-session-en",
            (Self::Cleanup, "zh-CN") => "--spaceprune-cleanup-session-zh-CN",
            (Self::Cleanup, _) => "--spaceprune-cleanup-session-en",
        }
    }

    /// Accept only this capability's existing or language-specific session entrypoint.
    pub fn accepts_session_argument(self, argument: &OsStr) -> bool {
        let legacy = match self {
            Self::Metadata => METADATA_SESSION_ARGUMENT,
            Self::Cleanup => "--spaceprune-cleanup-session",
        };
        [
            legacy,
            self.localized_session_argument("en"),
            self.localized_session_argument("zh-CN"),
        ]
        .iter()
        .any(|value| argument == OsStr::new(value))
    }
}

pub(super) fn probe_policy(path: &Path) -> Result<(), PermissionOutcome> {
    use std::os::unix::fs::OpenOptionsExt;
    if !install::destination_is_safe(path) {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    let policy = fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)
        .map_err(|_| PermissionOutcome::HelperOutdated)?;
    let metadata = policy
        .metadata()
        .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.permissions().mode() & 0o022 != 0 {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    let expected = include_bytes!("native.policy");
    let mut contents = Vec::new();
    policy
        .take(expected.len() as u64 + 1)
        .read_to_end(&mut contents)
        .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    if contents != expected {
        return Err(PermissionOutcome::HelperOutdated);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_selection_preserves_capability_and_falls_back_to_english() {
        for kind in [HelperKind::Metadata, HelperKind::Cleanup] {
            let other = if kind == HelperKind::Metadata {
                HelperKind::Cleanup
            } else {
                HelperKind::Metadata
            };
            for language in ["en", "zh-CN", "unsupported"] {
                let argument = kind.localized_session_argument(language);
                assert!(kind.accepts_session_argument(OsStr::new(argument)));
                assert!(!other.accepts_session_argument(OsStr::new(argument)));
                assert!(!kind.accepts_session_argument(OsStr::new(&format!("{argument} --extra"))));
            }
            assert_eq!(
                kind.localized_session_argument("unsupported"),
                kind.localized_session_argument("en")
            );
        }
    }
}
