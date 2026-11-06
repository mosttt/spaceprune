//! Versioned, account-independent application update discovery. This is not an installer contract.
use serde::{Deserialize, Serialize};

/// Maximum build number accepted by all supported package managers.
pub const MAX_BUILD_NUMBER: u32 = 2_100_000_000;

/// Installed application target; distribution is a build property, never an authorization grant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateTarget {
    /// Operating system identifier.
    pub platform: String,
    /// Distribution channel identifier.
    pub channel: String,
}
impl UpdateTarget {
    /// Reject unsupported platform/channel pairs instead of falling back to another channel.
    pub fn validate(&self) -> Result<(), &'static str> {
        let valid = match self.platform.as_str() {
            "windows" => matches!(self.channel.as_str(), "website" | "microsoft-store"),
            "macos" => matches!(self.channel.as_str(), "website" | "app-store"),
            "linux" => self.channel == "website",
            "android" => matches!(self.channel.as_str(), "website" | "google-play"),
            "ios" => self.channel == "app-store",
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err("Unsupported update target")
        }
    }
}

/// A single latest release for one platform and distribution channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppRelease {
    /// Display name, unrelated to ordering.
    pub version_name: String,
    /// Monotonically increasing package build number, not displayed to users.
    pub build_number: u32,
    /// Chinese release notes, rendered as plain text.
    pub notes_zh: String,
    /// English release notes, rendered as plain text.
    pub notes_en: String,
    /// HTTPS download or store product page, opened only after a user click.
    pub url: String,
}
impl AppRelease {
    /// Validate limits and constrain store builds to their own store.
    pub fn validate(&self, target: &UpdateTarget) -> Result<(), &'static str> {
        target.validate()?;
        if self.build_number == 0
            || self.build_number > MAX_BUILD_NUMBER
            || self.version_name.trim().is_empty()
            || self.version_name.len() > 80
            || self.version_name.chars().any(char::is_control)
            || self.notes_zh.len() > 16_000
            || self.notes_en.len() > 16_000
            || self.url.len() > 2048
            || self.url.chars().any(char::is_control)
        {
            return Err("Invalid release fields");
        }
        let url = url::Url::parse(&self.url).map_err(|_| "Invalid update URL")?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("Update URL must be an HTTPS page without credentials");
        }
        let store = match target.channel.as_str() {
            "google-play" => Some("play.google.com"),
            "microsoft-store" => Some("apps.microsoft.com"),
            "app-store" => Some("apps.apple.com"),
            _ => None,
        };
        if store.is_some_and(|host| url.host_str() != Some(host)) {
            return Err("Store builds must update through their own store");
        }
        Ok(())
    }
}

/// Public update response. Disabled or unconfigured targets return no release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateManifest {
    /// Protocol version; clients reject unsupported versions.
    pub schema_version: u32,
    /// Echoed target prevents applying a response for another distribution.
    pub target: UpdateTarget,
    /// Latest advertised release, if reminders are enabled.
    pub release: Option<AppRelease>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn channels_and_links_cannot_cross_store_boundaries() {
        let target = UpdateTarget {
            platform: "android".into(),
            channel: "google-play".into(),
        };
        let mut release = AppRelease {
            version_name: "1.10".into(),
            build_number: 110,
            notes_zh: String::new(),
            notes_en: String::new(),
            url: "https://play.google.com/store/apps/details?id=example".into(),
        };
        assert!(release.validate(&target).is_ok());
        for url in [
            "javascript:alert(1)",
            "file:///tmp/app",
            "https://play.google.com.evil.test/app",
            "https://user@play.google.com/app",
            "http://play.google.com/app",
        ] {
            release.url = url.into();
            assert!(release.validate(&target).is_err(), "{url}");
        }
        assert!(
            UpdateTarget {
                platform: "linux".into(),
                channel: "google-play".into()
            }
            .validate()
            .is_err()
        );
    }
}
