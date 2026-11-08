//! Decode Docker's native measurements without treating rounded display units as missing.
use serde_json::Value;

pub(super) fn bytes(value: &Value) -> Option<u64> {
    if let Some(bytes) = value.as_u64() {
        return Some(bytes);
    }
    let text = value.as_str()?.trim();
    let end = text
        .find(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(end);
    let scale = match unit.trim() {
        "" | "B" => 1u128,
        "kB" | "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        "TB" => 1_000_000_000_000,
        "PB" => 1_000_000_000_000_000,
        "EB" => 1_000_000_000_000_000_000,
        "KiB" => 1024,
        "MiB" => 1024u128.pow(2),
        "GiB" => 1024u128.pow(3),
        "TiB" => 1024u128.pow(4),
        _ => return None,
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let divisor = 10u128.checked_pow(fraction.len().try_into().ok()?)?;
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u128>().ok()?
    };
    let rounded = fraction.checked_mul(scale)?.checked_add(divisor / 2)? / divisor;
    whole
        .parse::<u128>()
        .ok()?
        .checked_mul(scale)?
        .checked_add(rounded)?
        .try_into()
        .ok()
}

pub(super) fn timestamp(value: Option<&Value>) -> Option<u64> {
    let value = value?.as_str()?;
    // Docker inspect uses RFC3339. Docker's df formatter uses Go's absolute UTC time.
    let normalized = if let Some(utc) = value.strip_suffix(" +0000 UTC") {
        format!("{}Z", utc.replacen(' ', "T", 1))
    } else {
        value.to_owned()
    };
    humantime::parse_rfc3339(&normalized)
        .ok()
        .and_then(spaceprune_engine::fs::unix_timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn decimal_units_preserve_estimates_and_integer_bytes_preserve_precision() {
        for (input, expected) in [
            ("4.096kB", 4096),
            ("1.5GB", 1_500_000_000),
            ("0B", 0),
            ("1.5KiB", 1536),
            ("18446744073709551615B", u64::MAX),
        ] {
            assert_eq!(bytes(&json!(input)), Some(expected));
        }
        assert_eq!(bytes(&json!(u64::MAX)), Some(u64::MAX));
        for input in [
            "unknown",
            "-1B",
            "NaN",
            "1.2.3GB",
            "18446744073709551616B",
            "99EB",
            "1GB extra",
        ] {
            assert_eq!(bytes(&json!(input)), None);
        }
    }
    #[test]
    fn timestamps_are_absolute_and_relative_descriptions_are_not_guessed() {
        assert_eq!(
            timestamp(Some(&json!("2026-10-27 17:12:55.241756731 +0000 UTC"))),
            timestamp(Some(&json!("2026-10-27T17:12:55.241756731Z")))
        );
        assert!(timestamp(Some(&json!("2026-10-27T17:12:55Z"))).is_some());
        assert_eq!(timestamp(Some(&json!("2 months ago"))), None);
        assert_eq!(timestamp(Some(&json!(""))), None);
    }
}
