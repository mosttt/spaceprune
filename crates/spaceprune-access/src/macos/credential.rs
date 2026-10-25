//! Authorization Services transfer material exists only inside authenticated native IPC.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct ExternalAuthorization(pub(super) [u8; 32]);

impl std::fmt::Debug for ExternalAuthorization {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ExternalAuthorization([redacted])")
    }
}
