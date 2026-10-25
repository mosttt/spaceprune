//! Shared application language preferences and native locale resolution.

use serde::{Deserialize, Serialize};

/// A device-local language override shared by CLI and Desktop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LanguagePreference {
    /// Follow the operating system language.
    #[default]
    #[serde(rename = "system")]
    System,
    /// Use Simplified Chinese.
    #[serde(rename = "zh-CN")]
    SimplifiedChinese,
    /// Use English; unknown saved language IDs also resolve to English.
    #[serde(rename = "en", other)]
    English,
}

/// A supported product display language, independent of recipe content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    /// English is the fallback for unsupported or unidentified locales.
    #[default]
    #[serde(rename = "en")]
    English,
    /// Simplified Chinese.
    #[serde(rename = "zh-CN")]
    SimplifiedChinese,
}

/// Shared preference and the language currently resolved from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageSettings {
    /// Saved override, or system detection when no override is saved.
    pub preference: LanguagePreference,
    /// Supported language to render now.
    pub resolved: Language,
}

impl Default for LanguageSettings {
    fn default() -> Self {
        LanguagePreference::default().settings()
    }
}

impl LanguagePreference {
    /// Canonical identifier persisted in application config.toml.
    pub const fn id(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::English => "en",
            Self::SimplifiedChinese => "zh-CN",
        }
    }

    /// Resolve the override, consulting the operating system only for `system`.
    pub fn resolve(self) -> Language {
        match self {
            Self::System => Language::from_locale(system_locale().as_deref()),
            Self::English => Language::English,
            Self::SimplifiedChinese => Language::SimplifiedChinese,
        }
    }

    /// Read a preference for persistence, rejecting unsupported manual choices.
    pub fn parse(id: &str) -> Option<Self> {
        match id {
            "system" => Some(Self::System),
            "en" => Some(Self::English),
            "zh-CN" => Some(Self::SimplifiedChinese),
            _ => None,
        }
    }

    /// Resolve a complete UI settings response without loading any recipe.
    pub fn settings(self) -> LanguageSettings {
        LanguageSettings {
            preference: self,
            resolved: self.resolve(),
        }
    }
}

impl Language {
    /// Stable BCP 47 identifier used by both application frontends.
    pub const fn id(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::SimplifiedChinese => "zh-CN",
        }
    }

    /// Match a native POSIX or BCP 47 locale to the supported language catalog.
    pub fn from_locale(locale: Option<&str>) -> Self {
        let base = locale
            .unwrap_or("")
            .trim()
            .split(['-', '_', '.', '@'])
            .next()
            .unwrap_or("");
        if base.eq_ignore_ascii_case("zh") {
            Self::SimplifiedChinese
        } else {
            Self::English
        }
    }
}

fn system_locale() -> Option<String> {
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    {
        unix_locale(|key| std::env::var(key).ok())
    }
    #[cfg(not(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    )))]
    {
        sys_locale::get_locale()
    }
}

#[cfg(any(
    test,
    all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    )
))]
fn unix_locale(mut get: impl FnMut(&str) -> Option<String>) -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|key| get(key).filter(|value| !value.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_matching_has_explicit_english_fallback() {
        for locale in ["zh_CN.UTF-8", "zh-Hans-CN", "ZH_tw", "zh", "zh_SG@modifier"] {
            assert_eq!(
                Language::from_locale(Some(locale)),
                Language::SimplifiedChinese
            );
        }
        for locale in [
            Some("en_GB.UTF-8"),
            Some("fr-FR"),
            Some("C.UTF-8"),
            Some(""),
            None,
        ] {
            assert_eq!(Language::from_locale(locale), Language::English);
        }
    }

    #[test]
    fn unix_locale_uses_message_precedence_and_ignores_empty_values() {
        let values = [
            ("LC_ALL", "fr_FR.UTF-8"),
            ("LC_MESSAGES", "zh_CN.UTF-8"),
            ("LANG", "en_US.UTF-8"),
        ];
        let get = |key: &str| {
            values
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.to_string())
        };
        assert_eq!(unix_locale(get).as_deref(), Some("fr_FR.UTF-8"));
        assert_eq!(
            unix_locale(|key| if key == "LC_ALL" {
                Some(String::new())
            } else {
                get(key)
            })
            .as_deref(),
            Some("zh_CN.UTF-8")
        );
    }

    #[test]
    fn manual_language_overrides_system_and_unknown_saved_values_fall_back() {
        assert_eq!(LanguagePreference::English.resolve(), Language::English);
        assert_eq!(
            LanguagePreference::SimplifiedChinese.resolve(),
            Language::SimplifiedChinese
        );
        #[derive(Deserialize)]
        struct Saved {
            language: LanguagePreference,
        }
        let saved: Saved = toml::from_str("language = 'future-language'").unwrap();
        assert_eq!(saved.language, LanguagePreference::English);
        assert_eq!(LanguagePreference::parse("future-language"), None);
    }
}
