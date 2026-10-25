//! A private inherited pipe binds installation receipts; these messages grant no file capability.
use crate::{Interaction, OperationIdentity, OperationTransfer, PermissionOutcome};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead};

#[cfg(windows)]
pub(crate) const FLAG: &str = "--spaceprune-prepare-windows-native-v1";
pub(crate) const MAX_FRAME: u64 = 16 * 1024;
pub(crate) const VERSION: u16 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub version: u16,
    pub host_pid: u32,
    pub host_created: u64,
    pub operation: OperationTransfer,
    pub interaction: Interaction,
    pub package_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Installed {
    Ready,
    RestartRequired,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reply {
    pub version: u16,
    pub operation: OperationIdentity,
    pub package_sha256: String,
    pub result: Result<Installed, PermissionOutcome>,
}

impl Reply {
    pub(crate) fn validate(self, request: &Request) -> Result<Installed, PermissionOutcome> {
        if self.version != VERSION
            || self.operation != request.operation.identity
            || self.package_sha256 != request.package_sha256
        {
            return Err(PermissionOutcome::HelperUntrusted);
        }
        self.result
    }
}

pub(crate) fn read_request(input: &mut impl BufRead) -> io::Result<Request> {
    let mut bytes = Vec::new();
    std::io::Read::take(input, MAX_FRAME + 1).read_until(b'\n', &mut bytes)?;
    if bytes.len() as u64 > MAX_FRAME || bytes.last() != Some(&b'\n') {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let request: Request = serde_json::from_slice(&bytes)?;
    if request.version != VERSION
        || request.interaction == Interaction::NonInteractive
        || request.host_pid == 0
        || request.host_created == 0
        || request.package_sha256.len() != 64
        || !request
            .package_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(request)
}

pub(crate) fn installer_result(code: u32) -> Result<Installed, PermissionOutcome> {
    match code {
        0 => Ok(Installed::Ready),
        3010 => Ok(Installed::RestartRequired),
        1602 | 1223 => Err(PermissionOutcome::Denied),
        1625 => Err(PermissionOutcome::PolicyBlocked),
        _ => Err(PermissionOutcome::AuthorizationUnavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OperationContext;
    #[test]
    fn bootstrap_rejects_background_unknown_fields_and_unbounded_input() {
        let request = Request {
            version: VERSION,
            host_pid: 1,
            host_created: 1,
            operation: OperationContext::default().transfer().unwrap(),
            interaction: Interaction::Foreground,
            package_sha256: "a".repeat(64),
        };
        let mut value = serde_json::to_value(&request).unwrap();
        let read = |value: &serde_json::Value| {
            read_request(&mut std::io::Cursor::new(format!("{value}\n")))
        };
        assert!(read(&value).is_ok());
        value["interaction"] = serde_json::json!("non_interactive");
        assert!(read(&value).is_err());
        value["interaction"] = serde_json::json!("foreground");
        value["destination"] = serde_json::json!("C:\\arbitrary");
        assert!(read(&value).is_err());
        assert!(
            read_request(&mut std::io::Cursor::new(vec![
                b' ';
                MAX_FRAME as usize + 2
            ]))
            .is_err()
        );
    }
    #[test]
    fn native_exit_codes_keep_refusal_policy_and_restart_separate() {
        assert_eq!(installer_result(0), Ok(Installed::Ready));
        assert_eq!(installer_result(3010), Ok(Installed::RestartRequired));
        assert_eq!(installer_result(1602), Err(PermissionOutcome::Denied));
        assert_eq!(
            installer_result(1625),
            Err(PermissionOutcome::PolicyBlocked)
        );
        assert_eq!(
            installer_result(1603),
            Err(PermissionOutcome::AuthorizationUnavailable)
        );
        assert_eq!(
            installer_result(1618),
            Err(PermissionOutcome::AuthorizationUnavailable)
        );
    }
}
