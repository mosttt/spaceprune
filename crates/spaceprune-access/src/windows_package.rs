//! Pin exact native payloads and use immutable generation directories for Windows updates.
use crate::PermissionOutcome;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(any(windows, all(test, target_os = "linux")))]
pub(crate) mod install_process;
#[cfg(any(windows, all(test, target_os = "linux")))]
pub(crate) mod installer_protocol;
#[cfg(any(windows, test))]
pub(crate) mod preparation;
#[cfg(any(windows, test))]
pub(crate) mod verification;

/// Fixed MSI basename beside a portable host.
pub const PACKAGE_NAME: &str = "SpacePruneNative.msi";
/// Fixed directory beneath the system Program Files known folder.
pub const INSTALL_ROOT_NAME: &str = "SpacePruneNative";
/// Windows login-bound metadata bootstrap/IPC protocol; independent of the request payload schema.
pub const METADATA_PROTOCOL: u16 = 7;
/// Windows login-bound cleanup bootstrap/pipe protocol; independent of the confirmed grant schema.
pub const CLEANUP_PROTOCOL: u16 = 5;

/// Windows Installer architecture; user-controlled paths never select the target location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Architecture {
    /// Windows x86-64.
    X64,
    /// Windows ARM64.
    Arm64,
}
impl Architecture {
    /// Interpret supported Rust distribution targets.
    pub fn for_target(target: &str) -> Result<Self, PermissionOutcome> {
        match target {
            "x86_64-pc-windows-msvc" | "x86_64-pc-windows-gnu" => Ok(Self::X64),
            "aarch64-pc-windows-msvc" => Ok(Self::Arm64),
            _ => Err(PermissionOutcome::Unsupported),
        }
    }
    /// Native MSI architecture spelling.
    pub const fn msi_name(self) -> &'static str {
        match self {
            Self::X64 => "x64",
            Self::Arm64 => "arm64",
        }
    }
}

/// A component has one fixed capability; installation does not grant that capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    /// Metadata inspection only.
    Metadata,
    /// Execution of a separately confirmed cleanup plan.
    Cleanup,
}
impl Component {
    /// Fixed filename inside every generation directory.
    pub const fn filename(self) -> &'static str {
        match self {
            Self::Metadata => "spaceprune-readonly-helper.exe",
            Self::Cleanup => "spaceprune-cleanup-helper.exe",
        }
    }
    /// Protocol query that must not request elevation or perform filesystem work.
    pub fn support_argument(self) -> String {
        match self {
            Self::Metadata => format!("--supports-windows-metadata-v{METADATA_PROTOCOL}"),
            Self::Cleanup => format!("--supports-cleanup-v{CLEANUP_PROTOCOL}"),
        }
    }
}

/// Identity of the two signed executables before their MSI is constructed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadIdentity {
    /// Architecture of both executable payloads.
    pub architecture: Architecture,
    /// SHA-256 of the exact metadata executable, including its Authenticode signature.
    pub metadata_sha256: String,
    /// SHA-256 of the exact cleanup executable, including its Authenticode signature.
    pub cleanup_sha256: String,
    /// Digest of the ordered public license materials included with the executables.
    pub notices_sha256: String,
}
impl PayloadIdentity {
    /// Reject malformed identities before deriving an installation directory.
    pub fn validate(&self) -> Result<(), PermissionOutcome> {
        if valid_digest(&self.metadata_sha256, 64)
            && valid_digest(&self.cleanup_sha256, 64)
            && valid_digest(&self.notices_sha256, 64)
        {
            Ok(())
        } else {
            Err(PermissionOutcome::HelperUntrusted)
        }
    }
    /// Content-specific location allows a new package to coexist with a running old helper.
    pub fn generation_directory(&self) -> Result<String, PermissionOutcome> {
        self.validate()?;
        let input = format!(
            "spaceprune.windows.native.v1\narchitecture={}\nmetadata-v{METADATA_PROTOCOL}={}\ncleanup-v{CLEANUP_PROTOCOL}={}\nnotices={}\n",
            self.architecture.msi_name(),
            self.metadata_sha256,
            self.cleanup_sha256,
            self.notices_sha256
        );
        Ok(format!(
            "native-{METADATA_PROTOCOL}-{CLEANUP_PROTOCOL}-{:x}",
            Sha256::digest(input.as_bytes())
        ))
    }
    /// Expected executable digest for an immutable role.
    pub fn digest(&self, component: Component) -> &str {
        match component {
            Component::Metadata => &self.metadata_sha256,
            Component::Cleanup => &self.cleanup_sha256,
        }
    }
    /// Describe the same fixed layout used by packaging and future native preparation.
    pub fn layout(&self) -> Result<serde_json::Value, PermissionOutcome> {
        let components = [Component::Metadata, Component::Cleanup].map(|component| serde_json::json!({
            "filename": component.filename(), "sha256": self.digest(component), "probe": component.support_argument(),
        }));
        Ok(serde_json::json!({
            "schemaVersion": 1, "version": env!("CARGO_PKG_VERSION"), "packageName": PACKAGE_NAME,
            "architecture": self.architecture, "installRoot": INSTALL_ROOT_NAME, "noticesSha256": self.notices_sha256,
            "generationDirectory": self.generation_directory()?,
            "components": components,
        }))
    }
}

/// Exact package pin. A successful installation still requires signature, ACL and protocol checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageIdentity {
    /// Package identity schema version.
    pub schema_version: u16,
    /// Immutable Core source revision used by the producer.
    pub core_revision: String,
    /// Hash of the final signed MSI, not an intermediate unsigned package.
    pub msi_sha256: String,
    /// Exact, signed helper payloads installed by this package.
    pub payload: PayloadIdentity,
}
impl PackageIdentity {
    /// Decode a build-time pin. Absence permits unbundled development; malformed pins fail closed.
    pub fn from_embedded(bytes: &[u8], target: &str) -> Result<Option<Self>, PermissionOutcome> {
        if bytes.is_empty() {
            return Ok(None);
        }
        if bytes.len() > 16 * 1024 {
            return Err(PermissionOutcome::HelperUntrusted);
        }
        let identity: Self =
            serde_json::from_slice(bytes).map_err(|_| PermissionOutcome::HelperUntrusted)?;
        identity.validate()?;
        if identity.payload.architecture != Architecture::for_target(target)? {
            return Err(PermissionOutcome::HelperUntrusted);
        }
        Ok(Some(identity))
    }
    /// Validate untrusted metadata without touching the filesystem or granting installation.
    pub fn validate(&self) -> Result<(), PermissionOutcome> {
        if self.schema_version != 1
            || !valid_digest(&self.core_revision, 40)
            || !valid_digest(&self.msi_sha256, 64)
        {
            return Err(PermissionOutcome::HelperUntrusted);
        }
        self.payload.validate()
    }
}

fn valid_digest(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_payload_and_architecture_change_selects_a_different_generation() {
        let original = PayloadIdentity {
            architecture: Architecture::X64,
            metadata_sha256: "a".repeat(64),
            cleanup_sha256: "b".repeat(64),
            notices_sha256: "c".repeat(64),
        };
        let directory = original.generation_directory().unwrap();
        assert_eq!(std::path::Path::new(&directory).components().count(), 1);
        for changed in [
            PayloadIdentity {
                notices_sha256: "e".repeat(64),
                ..original.clone()
            },
            PayloadIdentity {
                architecture: Architecture::Arm64,
                ..original.clone()
            },
            PayloadIdentity {
                metadata_sha256: "c".repeat(64),
                ..original.clone()
            },
            PayloadIdentity {
                cleanup_sha256: "c".repeat(64),
                ..original.clone()
            },
            PayloadIdentity {
                metadata_sha256: original.cleanup_sha256.clone(),
                cleanup_sha256: original.metadata_sha256.clone(),
                ..original.clone()
            },
        ] {
            assert_ne!(changed.generation_directory().unwrap(), directory);
        }
        let mut invalid = original;
        invalid.metadata_sha256 = "../untrusted".into();
        assert_eq!(
            invalid.generation_directory(),
            Err(PermissionOutcome::HelperUntrusted)
        );
    }
    #[test]
    fn package_pins_reject_unknown_fields_and_do_not_accept_paths_as_identity() {
        let identity = PackageIdentity {
            schema_version: 1,
            core_revision: "a".repeat(40),
            msi_sha256: "b".repeat(64),
            payload: PayloadIdentity {
                architecture: Architecture::X64,
                metadata_sha256: "c".repeat(64),
                cleanup_sha256: "d".repeat(64),
                notices_sha256: "e".repeat(64),
            },
        };
        identity.validate().unwrap();
        let bytes = serde_json::to_vec(&identity).unwrap();
        assert_eq!(
            PackageIdentity::from_embedded(&bytes, "x86_64-pc-windows-msvc"),
            Ok(Some(identity.clone()))
        );
        assert_eq!(
            PackageIdentity::from_embedded(&bytes, "aarch64-pc-windows-msvc"),
            Err(PermissionOutcome::HelperUntrusted)
        );
        assert_eq!(
            PackageIdentity::from_embedded(b"", "x86_64-pc-windows-msvc"),
            Ok(None)
        );
        assert_eq!(
            PackageIdentity::from_embedded(b" ", "x86_64-pc-windows-msvc"),
            Err(PermissionOutcome::HelperUntrusted)
        );
        assert_eq!(
            PackageIdentity::from_embedded(&vec![b' '; 16 * 1024 + 1], "x86_64-pc-windows-msvc"),
            Err(PermissionOutcome::HelperUntrusted)
        );
        let mut value = serde_json::to_value(&identity).unwrap();
        value["destination"] = serde_json::json!("C:\\untrusted");
        assert!(serde_json::from_value::<PackageIdentity>(value).is_err());
        for digest in ["B".repeat(64), "0".repeat(63), "C:\\untrusted".into()] {
            let mut invalid = identity.clone();
            invalid.msi_sha256 = digest;
            assert_eq!(invalid.validate(), Err(PermissionOutcome::HelperUntrusted));
        }
    }
}
