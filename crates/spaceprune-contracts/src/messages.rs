//! Product-owned messages shared by native frontends and built-in handlers.
//!
//! Language only affects presentation. Protocol identifiers, user content and external diagnostics
//! are never looked up in this catalog. Hosts select English for stable machine output.

use std::{
    collections::BTreeMap,
    sync::{
        LazyLock,
        atomic::{AtomicU8, Ordering},
    },
};

static LANGUAGE: AtomicU8 = AtomicU8::new(0);
static CATALOG: LazyLock<BTreeMap<String, [String; 2]>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("messages/core.json")).expect("valid product message catalog")
});

/// Set process-local display language; unknown identifiers resolve to English.
///
/// This setting cannot affect execution rules, selection or permission decisions.
pub fn set_display_language(language: &str) {
    LANGUAGE.store(u8::from(language == "zh-CN"), Ordering::Relaxed);
}

/// Return the process-local product display language, initially English.
pub fn display_language() -> &'static str {
    if LANGUAGE.load(Ordering::Relaxed) == 1 {
        "zh-CN"
    } else {
        "en"
    }
}

/// Look up a fixed product message key in the current display language.
pub fn text(key: &str) -> &'static str {
    let entry = CATALOG
        .get(key)
        .unwrap_or_else(|| panic!("unknown product message key: {key}"));
    &entry[usize::from(LANGUAGE.load(Ordering::Relaxed))]
}

/// Render a product message, inserting values once without interpreting their contents.
pub fn message(key: &str, parameters: &[(&str, String)]) -> String {
    interpolate(text(key), parameters)
}

fn interpolate(template: &str, parameters: &[(&str, String)]) -> String {
    let mut result = String::new();
    let mut remaining = template;
    while let Some((prefix, after_open)) = remaining.split_once('{') {
        result.push_str(prefix);
        let (name, suffix) = after_open
            .split_once('}')
            .expect("closed product message parameter");
        let (_, value) = parameters
            .iter()
            .find(|(parameter, _)| *parameter == name)
            .unwrap_or_else(|| panic!("missing product message parameter: {name}"));
        result.push_str(value);
        remaining = suffix;
    }
    result.push_str(remaining);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserted_values_remain_literal() {
        assert_eq!(
            interpolate(
                "{path}: {reason}",
                &[
                    ("path", "/tmp/{reason}".into()),
                    ("reason", "denied".into())
                ]
            ),
            "/tmp/{reason}: denied"
        );
    }

    #[test]
    fn catalogs_keep_the_same_parameters() {
        fn parameters(value: &str) -> std::collections::BTreeSet<&str> {
            value
                .split('{')
                .skip(1)
                .filter_map(|part| part.split_once('}').map(|(name, _)| name))
                .collect()
        }
        for (key, [english, chinese]) in CATALOG.iter() {
            assert!(
                !english.is_empty() && !chinese.is_empty(),
                "empty product message: {key}"
            );
            assert_eq!(
                parameters(english),
                parameters(chinese),
                "parameter mismatch: {key}"
            );
        }
    }
}
