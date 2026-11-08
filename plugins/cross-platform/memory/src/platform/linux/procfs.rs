//! Parse only the kernel fields required for inspection and private mapping advice.
use std::io;
pub(super) fn status_value(text: &str, field: &str) -> io::Result<u64> {
    let value = text
        .lines()
        .find_map(|s| s.strip_prefix(field))
        .ok_or_else(|| io::Error::other(format!("Missing memory field {field}")))?;
    let mut parts = value.split_whitespace();
    let number = parts
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| io::Error::other("Invalid memory value"))?;
    if parts.next() != Some("kB") || parts.next().is_some() {
        return Err(io::Error::other("Invalid memory unit"));
    }
    number
        .checked_mul(1024)
        .ok_or_else(|| io::Error::other("Memory value overflow"))
}
pub(super) fn private_bytes(text: &str) -> io::Result<u64> {
    let huge = if text.lines().any(|s| s.starts_with("Private_Hugetlb:")) {
        status_value(text, "Private_Hugetlb:")?
    } else {
        0
    };
    status_value(text, "Private_Clean:")?
        .checked_add(status_value(text, "Private_Dirty:")?)
        .and_then(|n| n.checked_add(huge))
        .ok_or_else(|| io::Error::other("Memory value overflow"))
}
pub(super) fn start_ticks(stat: &str) -> io::Result<u64> {
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().nth(19))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| io::Error::other("Invalid process start identity"))
}
pub(super) fn ranges(maps: &str) -> io::Result<Vec<(usize, usize)>> {
    let mut regions = Vec::new();
    for line in maps.lines() {
        let mut fields = line.split_whitespace();
        let addresses = fields
            .next()
            .ok_or_else(|| io::Error::other("Invalid process mapping"))?;
        let flags = fields
            .next()
            .ok_or_else(|| io::Error::other("Invalid process mapping"))?;
        if !flags.starts_with('r')
            || !flags.ends_with('p')
            || line.contains("[vdso]")
            || line.contains("[vvar")
            || line.contains("[vsyscall]")
        {
            continue;
        }
        let (start, end) = addresses
            .split_once('-')
            .ok_or_else(|| io::Error::other("Invalid process range"))?;
        let start = usize::from_str_radix(start, 16).map_err(io::Error::other)?;
        let end = usize::from_str_radix(end, 16).map_err(io::Error::other)?;
        if start >= end {
            return Err(io::Error::other("Invalid process range"));
        }
        regions.push((start, end));
    }
    Ok(regions)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_huge_pages_are_optional_but_invalid_values_are_not_zero() {
        let base = "Private_Clean: 1 kB\nPrivate_Dirty: 2 kB\n";
        assert_eq!(private_bytes(base).unwrap(), 3072);
        assert!(private_bytes(&format!("{base}Private_Hugetlb: corrupt kB\n")).is_err());
    }

    #[test]
    fn kernel_units_and_parentheses_are_not_guessed() {
        assert_eq!(status_value("VmRSS:\t123 kB\n", "VmRSS:").unwrap(), 125952);
        assert!(status_value("VmRSS: 123 MB", "VmRSS:").is_err());
        let stat = format!(
            "42 (name with ) parentheses) S {} 12345 1",
            vec!["0"; 18].join(" ")
        );
        assert_eq!(start_ticks(&stat).unwrap(), 12345);
    }
    #[test]
    fn only_readable_private_user_mappings_are_advised() {
        let maps = "1000-2000 rw-p 0 0:0 0 [heap]\n2000-3000 rw-s 0 0:0 0 shared\n3000-4000 r--p 0 0:0 0 [vvar]\n4000-5000 ---p 0 0:0 0\n5000-6000 r-xp 0 0:0 0 /app";
        assert_eq!(ranges(maps).unwrap(), [(0x1000, 0x2000), (0x5000, 0x6000)]);
        assert!(ranges("2000-1000 rw-p 0 0:0 0").is_err());
    }
}
