//! Validate actual code signatures without trusting plist identity strings alone.
#![allow(unsafe_code)]

use super::*;
use core_foundation::{
    base::{CFType, CFTypeRef, TCFType},
    boolean::CFBoolean,
    data::CFData,
    dictionary::{CFDictionary, CFDictionaryRef},
    number::CFNumber,
    string::{CFString, CFStringRef},
    url::CFURL,
};
use security_framework::os::macos::code_signing::{
    Flags, GuestAttributes, SecCode, SecRequirement, SecStaticCode,
};

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecCodeInfoTeamIdentifier: CFStringRef;
    static kSecCodeInfoUnique: CFStringRef;
    static kSecCodeInfoIdentifier: CFStringRef;
    static kSecCodeInfoFlags: CFStringRef;
    static kSecCodeInfoStatus: CFStringRef;
    static kSecCodeInfoEntitlements: CFStringRef;
    static kSecCodeInfoEntitlementsDict: CFStringRef;
    fn SecCodeCopySigningInformation(
        code: CFTypeRef,
        flags: u32,
        information: *mut CFDictionaryRef,
    ) -> i32;
}

fn information(
    code: CFTypeRef,
    flags: u32,
) -> Result<CFDictionary<CFString, CFType>, PermissionOutcome> {
    let mut result = std::ptr::null();
    // SAFETY: The caller retains a validated SecCode or SecStaticCode; Apple accepts either.
    let status = unsafe { SecCodeCopySigningInformation(code, flags, &mut result) };
    if status != 0 || result.is_null() {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    // SAFETY: A successful Copy call returns one retained dictionary with CFType keys/values.
    Ok(unsafe { CFDictionary::wrap_under_create_rule(result) })
}

fn string_value(
    info: &CFDictionary<CFString, CFType>,
    key: CFStringRef,
) -> Result<String, PermissionOutcome> {
    // SAFETY: All callers pass an immortal Security CFString constant.
    let key = unsafe { CFString::wrap_under_get_rule(key) };
    info.find(&key)
        .and_then(|value| value.downcast::<CFString>())
        .map(|value| value.to_string())
        .ok_or(PermissionOutcome::HelperUntrusted)
}

fn number_value(
    info: &CFDictionary<CFString, CFType>,
    key: CFStringRef,
) -> Result<u32, PermissionOutcome> {
    // SAFETY: All callers pass an immortal Security CFString constant.
    let key = unsafe { CFString::wrap_under_get_rule(key) };
    info.find(&key)
        .and_then(|value| value.downcast::<CFNumber>())
        .and_then(|value| value.to_i64())
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(PermissionOutcome::HelperUntrusted)
}

fn validate_protections(
    info: &CFDictionary<CFString, CFType>,
    dynamic: bool,
) -> Result<(), PermissionOutcome> {
    use objc2_foundation::{NSOperatingSystemVersion, NSProcessInfo};
    let runtime_supported =
        NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
            majorVersion: 10,
            minorVersion: 14,
            patchVersion: 0,
        });
    // SAFETY: These are immortal Security dictionary-key constants.
    let flags = number_value(info, unsafe { kSecCodeInfoFlags })?;
    if !code_policy::protected_flags(flags, runtime_supported) {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    if dynamic {
        // SAFETY: This is an immortal Security dictionary-key constant.
        let status = number_value(info, unsafe { kSecCodeInfoStatus })?;
        if status & 1 == 0 || !code_policy::protected_flags(status, runtime_supported) {
            return Err(PermissionOutcome::HelperUntrusted);
        }
    }
    // SAFETY: Both keys are immortal CFString constants exported by Security.
    let (raw_key, dict_key) = unsafe {
        (
            CFString::wrap_under_get_rule(kSecCodeInfoEntitlements),
            CFString::wrap_under_get_rule(kSecCodeInfoEntitlementsDict),
        )
    };
    if let Some(value) = info.find(&dict_key) {
        let dict = value
            .downcast::<CFDictionary>()
            .ok_or(PermissionOutcome::HelperUntrusted)?;
        // SAFETY: Security's entitlement dictionaries have CFString keys and CFType values.
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(dict.as_concrete_TypeRef()) };
        for entitlement in code_policy::UNSAFE_ENTITLEMENTS {
            if let Some(value) = dict.find(CFString::new(entitlement)) {
                let allowed = value
                    .downcast::<CFBoolean>()
                    .is_some_and(|value| !bool::from(value));
                if !allowed {
                    return Err(PermissionOutcome::HelperUntrusted);
                }
            }
        }
    } else if info.find(&raw_key).is_some() {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    Ok(())
}

fn validate_code(
    code: &SecCode,
    identifiers: &[&str],
    expected_team: Option<&str>,
) -> Result<(String, Vec<u8>), PermissionOutcome> {
    let anchor: SecRequirement = "anchor apple generic"
        .parse()
        .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    code.check_validity(Flags::STRICT_VALIDATE, &anchor)
        .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    // Signing information (2) includes entitlements; dynamic information (8) includes kernel status.
    let info = information(code.as_CFTypeRef(), 2 | 8)?;
    // SAFETY: Security exports these immortal dictionary-key constants.
    let (team, identifier) = unsafe {
        (
            string_value(&info, kSecCodeInfoTeamIdentifier)?,
            string_value(&info, kSecCodeInfoIdentifier)?,
        )
    };
    if !identifiers.contains(&identifier.as_str())
        || expected_team.is_some_and(|expected| expected != team)
    {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    let requirement: SecRequirement = signing_requirement(&team, &identifier)?
        .parse()
        .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    validate_protections(&info, true)?;
    code.check_validity(Flags::STRICT_VALIDATE, &requirement)
        .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    Ok((team, code_hash(&info)?))
}

pub(super) fn own_team() -> Result<String, PermissionOutcome> {
    let code = SecCode::for_self(Flags::NONE).map_err(|_| PermissionOutcome::HelperUntrusted)?;
    validate_code(&code, &[INSTALLER_IDENTIFIER], None).map(|(team, _)| team)
}

pub(super) fn system_authenticator() -> Result<PathBuf, PermissionOutcome> {
    let path = crate::process::trusted_executable(Path::new("/usr/bin/security"))
        .ok_or(PermissionOutcome::AuthorizationUnavailable)?;
    let url = CFURL::from_path(&path, false).ok_or(PermissionOutcome::HelperUntrusted)?;
    let code = SecStaticCode::from_path(&url, Flags::NONE)
        .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    // This is the fixed Apple OS binary, not a third-party Team-signed application.
    let requirement: SecRequirement = "anchor apple"
        .parse()
        .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    code.check_validity(
        Flags::STRICT_VALIDATE | Flags::CHECK_ALL_ARCHITECTURES,
        &requirement,
    )
    .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    Ok(path)
}

pub(super) fn host_team() -> Result<String, PermissionOutcome> {
    let code = SecCode::for_self(Flags::NONE).map_err(|_| PermissionOutcome::HelperUntrusted)?;
    validate_code(&code, &[CLI_IDENTIFIER, DESKTOP_IDENTIFIER], None).map(|(team, _)| team)
}

pub(super) fn service_team(kind: ServiceKind) -> Result<String, PermissionOutcome> {
    let code = SecCode::for_self(Flags::NONE).map_err(|_| PermissionOutcome::HelperUntrusted)?;
    validate_code(&code, &[kind.label()], None).map(|(team, _)| team)
}

pub(super) fn verify_guest(
    token: &[u8],
    team: &str,
    identifiers: &[&str],
    expected_hash: Option<&[u8]>,
) -> Result<(), PermissionOutcome> {
    let hash = guest_hash(token, team, identifiers)?;
    if expected_hash.is_some_and(|expected| expected != hash) {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    Ok(())
}

pub(super) fn guest_hash(
    token: &[u8],
    team: &str,
    identifiers: &[&str],
) -> Result<Vec<u8>, PermissionOutcome> {
    let data = CFData::from_buffer(token);
    let mut attributes = GuestAttributes::new();
    attributes.set_audit_token(data.as_concrete_TypeRef());
    let code = SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE)
        .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    validate_code(&code, identifiers, Some(team)).map(|(_, hash)| hash)
}

fn code_hash(info: &CFDictionary<CFString, CFType>) -> Result<Vec<u8>, PermissionOutcome> {
    // SAFETY: Security exports an immortal CFString constant.
    let key = unsafe { CFString::wrap_under_get_rule(kSecCodeInfoUnique) };
    let hash = info
        .find(&key)
        .and_then(|value| value.downcast::<CFData>())
        .ok_or(PermissionOutcome::HelperUntrusted)?;
    if hash.is_empty() {
        return Err(PermissionOutcome::HelperUntrusted);
    }
    Ok(hash.bytes().to_vec())
}

pub(super) fn verify_file(
    path: &Path,
    team: &str,
    identifier: &str,
) -> Result<Vec<u8>, PermissionOutcome> {
    let url = CFURL::from_path(path, path.is_dir()).ok_or(PermissionOutcome::HelperUntrusted)?;
    let code = SecStaticCode::from_path(&url, Flags::NONE)
        .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    let requirement: SecRequirement = signing_requirement(team, identifier)?
        .parse()
        .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    code.check_validity(
        Flags::STRICT_VALIDATE | Flags::CHECK_ALL_ARCHITECTURES,
        &requirement,
    )
    .map_err(|_| PermissionOutcome::HelperUntrusted)?;
    let info = information(code.as_CFTypeRef(), 2)?;
    validate_protections(&info, false)?;
    code_hash(&info)
}
