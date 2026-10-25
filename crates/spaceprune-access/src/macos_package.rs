//! Pure native package metadata, shared by runtime verification and build tooling on every host.

use crate::PermissionOutcome;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Code-signing identifier of the independent public CLI.
pub const CLI_IDENTIFIER: &str = "com.spaceprune.cli";
/// Code-signing identifier of the desktop host allowed to use the common native service.
pub const DESKTOP_IDENTIFIER: &str = "com.spaceprune.desktop";
/// Code-signing identifier of the standalone ServiceManagement app's executable.
pub const INSTALLER_IDENTIFIER: &str = concat!(
    "com.spaceprune.authorization.",
    env!("SPACEPRUNE_MACOS_DISTRIBUTION")
);
/// Name of the authorization app shipped with both CLI and Desktop.
pub const AGENT_BUNDLE: &str = "SpacePruneAuthorization.app";
/// Fixed executable inside the authorization app.
pub const AGENT_EXECUTABLE: &str = "spaceprune-helper-installer";
macro_rules! agent_protocol {
    ($version:literal) => {
        /// Current private host/authorization-agent wire version.
        pub const AGENT_PROTOCOL: u16 = $version;
        /// Query the agent without opening an authorization dialog.
        pub const SUPPORT_ARGUMENT: &str = concat!("--supports-macos-access-agent-v", $version);
        /// Run one bounded action in the dedicated signed authorization process.
        pub const AGENT_ARGUMENT: &str = concat!("--macos-access-agent-v", $version);
    };
}
agent_protocol!(6);

/// A service instance has one immutable capability, including on older macOS releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceKind {
    /// Metadata reads; no cleanup messages can be dispatched to this service.
    Metadata,
    /// Execution of a separately confirmed cleanup plan.
    Cleanup,
}

impl ServiceKind {
    pub(crate) fn authorization_right(self) -> &'static std::ffi::CStr {
        match self {
            Self::Metadata => c"com.spaceprune.metadata.read",
            Self::Cleanup => c"com.spaceprune.cleanup.execute",
        }
    }

    /// Fixed launchd and code-signing identifier for this capability/protocol.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Metadata => concat!(
                "com.spaceprune.metadata-v6-",
                env!("SPACEPRUNE_MACOS_DISTRIBUTION")
            ),
            Self::Cleanup => concat!(
                "com.spaceprune.cleanup-v4-",
                env!("SPACEPRUNE_MACOS_DISTRIBUTION")
            ),
        }
    }

    /// Fixed daemon entrypoint recorded in the signed launchd job.
    pub const fn daemon_argument(self) -> &'static str {
        match self {
            Self::Metadata => "--spaceprune-macos-metadata-service",
            Self::Cleanup => "--spaceprune-macos-cleanup-service",
        }
    }

    /// Private worker entrypoint; only a verified root service may supply its requests.
    pub const fn worker_argument(self) -> &'static str {
        match self {
            Self::Metadata => "--spaceprune-macos-metadata-worker",
            Self::Cleanup => "--spaceprune-macos-cleanup-worker",
        }
    }

    /// Plist basename inside the signed authorization app.
    pub fn plist_name(self) -> String {
        format!("{}.plist", self.label())
    }

    /// Bundled executable location required by the legacy SMJobBless installation API.
    pub fn bundled_executable(self, bundle: &Path) -> PathBuf {
        bundle
            .join("Contents/Library/LaunchServices")
            .join(self.label())
    }

    /// Root-owned location managed by SMJobBless on macOS releases before 13.
    pub fn legacy_executable(self) -> PathBuf {
        Path::new("/Library/PrivilegedHelperTools").join(self.label())
    }
}

/// Build the fixed publisher/role requirement; unknown identifiers and malformed teams are rejected.
pub fn signing_requirement(team: &str, identifier: &str) -> Result<String, PermissionOutcome> {
    if team.len() != 10
        || !team
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        || ![
            INSTALLER_IDENTIFIER,
            CLI_IDENTIFIER,
            DESKTOP_IDENTIFIER,
            ServiceKind::Metadata.label(),
            ServiceKind::Cleanup.label(),
        ]
        .contains(&identifier)
    {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    Ok(format!(
        "anchor apple generic and certificate leaf[subject.OU] = \"{team}\" and identifier \"{identifier}\""
    ))
}

/// Describe the fixed native package without installing components or requesting authorization.
/// Build tooling consumes this manifest for both public CLI and Desktop app bundles.
pub fn package_layout(team: &str) -> Result<serde_json::Value, PermissionOutcome> {
    let agent_requirement = signing_requirement(team, INSTALLER_IDENTIFIER)?;
    let services = [ServiceKind::Metadata, ServiceKind::Cleanup].map(|kind| {
        let (binary, probe) = match kind {
            ServiceKind::Metadata => ("spaceprune-readonly-helper", "--supports-scope-v6"),
            ServiceKind::Cleanup => ("spaceprune-cleanup-helper", "--supports-cleanup-v4"),
        };
        serde_json::json!({
            "kind": kind, "binary": binary, "probe": probe,
            "authorizationRight": kind.authorization_right().to_str().expect("fixed right"),
            "requirement": signing_requirement(team, kind.label()).expect("validated team and role"),
            "label": kind.label(), "plist": kind.plist_name(),
            "daemonArgument": kind.daemon_argument(), "workerArgument": kind.worker_argument(),
            "bundleProgram": format!("Contents/Library/LaunchServices/{}", kind.label()),
        })
    });
    Ok(serde_json::json!({
        "team": team,
        "distribution": env!("SPACEPRUNE_MACOS_DISTRIBUTION"),
        "authorizationRequirement": agent_requirement,
        "cliRequirement": signing_requirement(team, CLI_IDENTIFIER)?,
        "desktopRequirement": signing_requirement(team, DESKTOP_IDENTIFIER)?,
        "schemaVersion": 1,
        "version": env!("CARGO_PKG_VERSION"),
        "agentProtocol": AGENT_PROTOCOL,
        "authorizationApp": AGENT_BUNDLE,
        "authorizationExecutable": AGENT_EXECUTABLE,
        "authorizationIdentifier": INSTALLER_IDENTIFIER,
        "authorizationProbe": SUPPORT_ARGUMENT,
        "cliIdentifier": CLI_IDENTIFIER,
        "desktopIdentifier": DESKTOP_IDENTIFIER,
        "services": services,
    }))
}
