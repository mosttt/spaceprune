//! Platform-independent age/size conditions over measurements supplied by native plugins.
use crate::fs;
use spaceprune_plugin_api::{PluginResult, messages};
use std::time::{Duration, SystemTime};

/// The native timestamp used to evaluate an age condition.
#[derive(Clone, Copy)]
pub enum TimeBasis {
    /// Latest modification within a file or directory tree.
    Modified,
    /// Last use of a tool-owned resource.
    LastUsed,
    /// Resource creation, without implying when it was last used.
    Created,
    /// Last container stop, or creation when it has never run.
    Stopped,
}

impl TimeBasis {
    /// Stable tag carried by existing item contracts.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Modified => "time:modified",
            Self::LastUsed => "time:last-used",
            Self::Created => "time:created",
            Self::Stopped => "time:stopped",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::Modified => "measurement.modified",
            Self::LastUsed => "measurement.last_used",
            Self::Created => "measurement.created",
            Self::Stopped => "measurement.stopped",
        }
    }
}

/// Shared conditions; native plugins supply bytes and an explicitly defined timestamp.
#[derive(Clone, Copy, Default)]
pub struct MeasurementPolicy {
    /// Required elapsed age, if configured.
    pub older_than: Option<Duration>,
    /// Minimum logical bytes, if configured.
    pub min_bytes: Option<u64>,
}

impl MeasurementPolicy {
    /// Parse the common `older-than` and `min-size` Recipe conditions.
    pub fn parse(age: Option<&str>, size: Option<&str>) -> PluginResult<Self> {
        Ok(Self {
            older_than: age.map(fs::parse_duration).transpose()?,
            min_bytes: size.map(fs::parse_bytes).transpose()?,
        })
    }

    /// Evaluate all configured conditions. Missing required measurements never match.
    pub fn matches(self, bytes: Option<u64>, timestamp: Option<u64>, now: u64) -> bool {
        self.older_than.is_none_or(|age| {
            timestamp.is_some_and(|time| time <= now && now - time >= age.as_secs())
        }) && self
            .min_bytes
            .is_none_or(|minimum| bytes.is_some_and(|size| size >= minimum))
    }

    /// Describe the actual conditions and whether all of them matched.
    pub fn explanation(
        self,
        bytes: Option<u64>,
        timestamp: Option<u64>,
        basis: TimeBasis,
    ) -> String {
        let mut conditions = Vec::new();
        if let Some(age) = self.older_than {
            let duration = if age.as_secs().is_multiple_of(86400) {
                messages::message(
                    "measurement.days",
                    &[("days", (age.as_secs() / 86400).to_string())],
                )
            } else {
                humantime::format_duration(age).to_string()
            };
            conditions.push(messages::message(basis.key(), &[("age", duration)]));
        }
        if let Some(minimum) = self.min_bytes {
            conditions.push(messages::message(
                "measurement.minimum",
                &[("size", fs::format_bytes(minimum))],
            ));
        }
        if conditions.is_empty() {
            return messages::text("measurement.no_threshold").into();
        }
        let now = fs::unix_timestamp(SystemTime::now()).unwrap_or(0);
        let key = if self.matches(bytes, timestamp, now) {
            "measurement.matches"
        } else if (self.older_than.is_some() && timestamp.is_none())
            || (self.min_bytes.is_some() && bytes.is_none())
        {
            "measurement.missing"
        } else {
            "measurement.not_matched"
        };
        messages::message(
            key,
            &[(
                "conditions",
                conditions.join(messages::text("measurement.and")),
            )],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conditions_are_conjunctive_and_never_guess_missing_or_future_times() {
        let policy = MeasurementPolicy::parse(Some("60d"), Some("10MiB")).unwrap();
        let now = 100 * 86400;
        assert!(policy.matches(Some(10 * 1024 * 1024), Some(40 * 86400), now));
        assert!(!policy.matches(Some(10 * 1024 * 1024), Some(41 * 86400), now));
        assert!(!policy.matches(Some(1), Some(1), now));
        assert!(!policy.matches(None, Some(1), now));
        assert!(!policy.matches(Some(u64::MAX), None, now));
        assert!(!policy.matches(Some(u64::MAX), Some(now + 1), now));
        assert!(MeasurementPolicy::default().matches(None, None, now));
    }
}
