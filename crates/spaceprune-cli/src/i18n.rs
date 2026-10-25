//! Domain catalogs for application-owned terminal copy.

use spaceprune::api::Language;
use std::{
    collections::BTreeMap,
    sync::{
        LazyLock,
        atomic::{AtomicU8, Ordering},
    },
};

static LANGUAGE: AtomicU8 = AtomicU8::new(0);
static CATALOG: LazyLock<BTreeMap<String, [String; 2]>> = LazyLock::new(|| {
    let mut catalog = BTreeMap::new();
    for source in [
        include_str!("i18n/commands.json"),
        include_str!("i18n/terminal.json"),
        include_str!("i18n/plugins.json"),
    ] {
        let entries: BTreeMap<String, [String; 2]> =
            serde_json::from_str(source).expect("valid language catalog");
        for (key, value) in entries {
            assert!(
                catalog.insert(key, value).is_none(),
                "duplicate language key"
            );
        }
    }
    catalog
});

pub(crate) fn set_language(language: Language) {
    spaceprune::api::set_display_language(language.id());
    LANGUAGE.store(
        if language == Language::SimplifiedChinese {
            1
        } else {
            0
        },
        Ordering::Relaxed,
    );
}

pub(crate) fn is_chinese() -> bool {
    LANGUAGE.load(Ordering::Relaxed) == 1
}

pub(crate) fn text(key: &str) -> &'static str {
    let entry = CATALOG
        .get(key)
        .unwrap_or_else(|| panic!("unknown language key: {key}"));
    &entry[usize::from(LANGUAGE.load(Ordering::Relaxed))]
}

pub(crate) fn message(key: &str, parameters: &[(&str, String)]) -> String {
    let template = text(key);
    let mut result = String::new();
    let mut remaining = template;
    while let Some((prefix, after_open)) = remaining.split_once('{') {
        result.push_str(prefix);
        let (name, suffix) = after_open
            .split_once('}')
            .expect("closed message parameter");
        let (_, value) = parameters
            .iter()
            .find(|(parameter, _)| *parameter == name)
            .unwrap_or_else(|| panic!("missing parameter {name} for {key}"));
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
    fn catalogs_preserve_parameter_contracts() {
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
                "empty message: {key}"
            );
            assert_eq!(
                parameters(english),
                parameters(chinese),
                "parameter mismatch: {key}"
            );
        }
    }
}
