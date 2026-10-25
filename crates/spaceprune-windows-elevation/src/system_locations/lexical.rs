//! Normalize supported Windows namespaces while preserving native UTF-16 filename units.

#[derive(Clone, Debug)]
pub(super) struct WindowsPath {
    pub(super) units: Vec<u16>,
}

const SLASH: u16 = b'\\' as u16;

impl WindowsPath {
    pub(super) fn parse(input: &[u16]) -> Option<Self> {
        if input.is_empty() || input.len() > 32_767 || input.contains(&0) {
            return None;
        }
        let verbatim = starts_ascii(input, r"\\?\");
        if verbatim && input.contains(&(b'/' as u16)) {
            return None;
        }
        let mut units = input
            .iter()
            .map(|&unit| if unit == b'/' as u16 { SLASH } else { unit })
            .collect::<Vec<_>>();
        if starts_ascii(&units, r"\\?\UNC\") {
            units.drain(..8);
            units.splice(..0, [SLASH, SLASH]);
        } else if verbatim && drive_root(&units[4..]) {
            units.drain(..4);
        } else if verbatim && !starts_ascii(&units, r"\\?\Volume{") {
            return None;
        }
        let root_end = if drive_root(&units) {
            3
        } else if starts_ascii(&units, r"\\?\Volume{") {
            let guid = units.get(11..47)?;
            if units.get(47..49)? != [b'}' as u16, SLASH]
                || !guid.iter().enumerate().all(|(index, &unit)| {
                    if [8, 13, 18, 23].contains(&index) {
                        unit == b'-' as u16
                    } else {
                        u8::try_from(unit).is_ok_and(|byte| byte.is_ascii_hexdigit())
                    }
                })
            {
                return None;
            }
            49
        } else if units.starts_with(&[SLASH, SLASH]) {
            let server_end = 2 + units[2..].iter().position(|&unit| unit == SLASH)?;
            let share_start = server_end + 1;
            let share_end = share_start
                + units[share_start..]
                    .iter()
                    .position(|&unit| unit == SLASH)
                    .unwrap_or(units.len() - share_start);
            if !component(&units[2..server_end], verbatim)
                || units[2..server_end] == [b'?' as u16]
                || !component(&units[share_start..share_end], verbatim)
            {
                return None;
            }
            share_end + usize::from(share_end < units.len())
        } else {
            return None;
        };
        while units.last() == Some(&SLASH) {
            units.pop();
        }
        if units.len() > root_end
            && !units[root_end..]
                .split(|&unit| unit == SLASH)
                .all(|part| component(part, verbatim))
        {
            return None;
        }
        Some(Self { units })
    }

    pub(super) fn same(
        &self,
        other: &Self,
        equal: impl Fn(&[u16], &[u16]) -> Option<bool>,
    ) -> Option<bool> {
        equal(&self.units, &other.units)
    }

    pub(super) fn contains(
        &self,
        child: &Self,
        equal: impl Fn(&[u16], &[u16]) -> Option<bool>,
    ) -> Option<bool> {
        if child.units.len() < self.units.len()
            || (child.units.len() > self.units.len() && child.units[self.units.len()] != SLASH)
        {
            return Some(false);
        }
        equal(&self.units, &child.units[..self.units.len()])
    }
}

fn drive_root(units: &[u16]) -> bool {
    units.len() >= 3
        && u8::try_from(units[0]).is_ok_and(|byte| byte.is_ascii_alphabetic())
        && units[1] == b':' as u16
        && units[2] == SLASH
}

fn starts_ascii(units: &[u16], prefix: &str) -> bool {
    units.len() >= prefix.len()
        && units.iter().zip(prefix.bytes()).all(|(&unit, byte)| {
            u8::try_from(unit).is_ok_and(|unit| unit.eq_ignore_ascii_case(&byte))
        })
}

fn component(units: &[u16], verbatim: bool) -> bool {
    !units.is_empty()
        && units != [b'.' as u16]
        && units != [b'.' as u16, b'.' as u16]
        && !units.contains(&(b':' as u16))
        && (verbatim
            || !matches!(units.last(), Some(unit) if *unit == b'.' as u16 || *unit == b' ' as u16))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn path(value: &str) -> WindowsPath {
        WindowsPath::parse(&value.encode_utf16().collect::<Vec<_>>()).unwrap()
    }
    fn ascii_equal(left: &[u16], right: &[u16]) -> Option<bool> {
        Some(left.len() == right.len() && left.iter().zip(right).all(|(&a, &b)| {
            a == b || matches!((u8::try_from(a), u8::try_from(b)), (Ok(a), Ok(b)) if a.eq_ignore_ascii_case(&b))
        }))
    }
    #[test]
    fn dos_and_extended_paths_match_at_directory_boundaries() {
        let root = path(r"C:\Windows");
        for value in [
            r"C:\Windows",
            r"\\?\C:\Windows\System32",
            r"c:\WINDOWS\System32",
            "C:/Windows/System32",
            "C:\\Windows\\",
        ] {
            assert_eq!(
                root.contains(&path(value), ascii_equal),
                Some(true),
                "{value}"
            );
        }
        for value in [
            r"C:\Windows.old",
            r"C:\WindowsBackup\System32",
            r"D:\Windows\System32",
            r"C:\Users\move\Windows",
        ] {
            assert_eq!(
                root.contains(&path(value), ascii_equal),
                Some(false),
                "{value}"
            );
        }
        assert_eq!(
            path(r"C:\").same(&path(r"\\?\c:\"), ascii_equal),
            Some(true)
        );
        assert_eq!(
            path(r"C:\").same(&path(r"C:\cache"), ascii_equal),
            Some(false)
        );
    }
    #[test]
    fn volume_and_unc_namespaces_preserve_their_identity() {
        let volume = path(r"\\?\Volume{12345678-1234-1234-1234-123456789abc}\");
        assert_eq!(
            volume.contains(
                &path(r"\\?\Volume{12345678-1234-1234-1234-123456789ABC}\Windows"),
                ascii_equal
            ),
            Some(true)
        );
        assert_eq!(
            volume.contains(
                &path(r"\\?\Volume{22345678-1234-1234-1234-123456789abc}\Windows"),
                ascii_equal
            ),
            Some(false)
        );
        let unc = path(r"\\server\share\managed");
        assert_eq!(
            unc.contains(&path(r"\\?\UNC\SERVER\share\managed\file"), ascii_equal),
            Some(true)
        );
        assert_eq!(
            unc.contains(&path(r"\\server\share\managed-other"), ascii_equal),
            Some(false)
        );
        assert_eq!(
            unc.contains(&path(r"\\other\share\managed"), ascii_equal),
            Some(false)
        );
    }
    #[test]
    fn ambiguous_and_unrecognized_paths_cannot_be_classified_as_ordinary() {
        for value in [
            r"C:Windows",
            r"\Windows",
            r"C:\Windows\..\cache",
            r"C:\Windows.",
            r"C:\Windows ",
            r"C:\Windows::$DATA",
            r"\\.\pipe\anything",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\Windows",
            r"\\server",
            r"\\?\Volume{bad}\Windows",
            r"\\?\C:/Windows",
        ] {
            assert!(
                WindowsPath::parse(&value.encode_utf16().collect::<Vec<_>>()).is_none(),
                "{value}"
            );
        }
        let mut native = r"\\?\C:\Users\move\".encode_utf16().collect::<Vec<_>>();
        native.push(0xd800);
        let value = WindowsPath::parse(&native).unwrap();
        assert_eq!(value.units.last(), Some(&0xd800));
        assert_eq!(
            path(r"C:\Windows").contains(&value, ascii_equal),
            Some(false)
        );
        assert!(WindowsPath::parse(&[b'C' as u16, b':' as u16, SLASH, 0]).is_none());
        assert_eq!(
            path(r"C:\Windows").contains(&path(r"C:\Windows\System32"), |_, _| None),
            None
        );
    }
}
