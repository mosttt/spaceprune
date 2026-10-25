//! Runtime protection required before trusting a signed process as an IPC principal.

const HARD: u32 = 0x0100;
const KILL: u32 = 0x0200;
const LIBRARY_VALIDATION: u32 = 0x2000;
const RUNTIME: u32 = 0x10000;

pub(super) const UNSAFE_ENTITLEMENTS: [&str; 4] = [
    "com.apple.security.get-task-allow",
    "com.apple.security.cs.disable-library-validation",
    "com.apple.security.cs.allow-dyld-environment-variables",
    "com.apple.security.cs.allow-unsigned-executable-memory",
];

pub(super) fn protected_flags(flags: u32, runtime_supported: bool) -> bool {
    let legacy = HARD | KILL | LIBRARY_VALIDATION;
    (runtime_supported && flags & RUNTIME != 0) || flags & legacy == legacy
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_runtime_bit_on_an_old_os_is_not_a_protection_and_library_validation_remains_required() {
        assert!(!protected_flags(RUNTIME, false));
        assert!(protected_flags(RUNTIME, true));
        assert!(!protected_flags(HARD | KILL, false));
        assert!(!protected_flags(LIBRARY_VALIDATION, false));
        assert!(protected_flags(HARD | KILL | LIBRARY_VALIDATION, false));
        assert!(!protected_flags(0, true));
        assert!(UNSAFE_ENTITLEMENTS.contains(&"com.apple.security.get-task-allow"));
    }
}
