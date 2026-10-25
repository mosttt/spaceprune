//! Verify complete pinned bytes under the original operation's cancellation and deadline.
use crate::{OperationContext, PermissionOutcome};
use sha2::{Digest, Sha256};
use std::io::Read;

pub(crate) fn check(operation: &OperationContext) -> Result<(), PermissionOutcome> {
    operation.check().map_err(|error| match error.kind() {
        std::io::ErrorKind::Interrupted => PermissionOutcome::Cancelled,
        std::io::ErrorKind::TimedOut => PermissionOutcome::TimedOut,
        _ => PermissionOutcome::AuthorizationUnavailable,
    })
}

pub(crate) fn digest_matches(
    mut reader: impl Read,
    expected: &str,
    operation: &OperationContext,
) -> Result<(), PermissionOutcome> {
    let mut digest = Sha256::new();
    let mut bytes = [0; 64 * 1024];
    loop {
        check(operation)?;
        let count = reader
            .read(&mut bytes)
            .map_err(|_| PermissionOutcome::HelperUntrusted)?;
        if count == 0 {
            break;
        }
        digest.update(&bytes[..count]);
    }
    check(operation)?;
    if format!("{:x}", digest.finalize()) == expected {
        Ok(())
    } else {
        Err(PermissionOutcome::HelperUntrusted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CancellationToken, Interaction};
    use std::{
        io::{self, Cursor},
        time::Duration,
    };

    #[test]
    fn exact_bytes_are_required_and_cancellation_interrupts_hashing() {
        let operation = OperationContext::default();
        // SHA-256's published "abc" test vector, independent of this implementation.
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(
            digest_matches(Cursor::new(b"abc"), expected, &operation),
            Ok(())
        );
        assert_eq!(
            digest_matches(Cursor::new(b"abc!"), expected, &operation),
            Err(PermissionOutcome::HelperUntrusted)
        );
        let expired = OperationContext::new(
            CancellationToken::default(),
            Interaction::Foreground,
            Duration::ZERO,
        );
        assert_eq!(
            digest_matches(Cursor::new(b"abc"), expected, &expired),
            Err(PermissionOutcome::TimedOut)
        );
        struct CancelsAfterRead<'a>(&'a OperationContext);
        impl Read for CancelsAfterRead<'_> {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                assert!(!self.0.is_stopped(), "no reads after cancellation");
                output[0] = b'a';
                self.0.cancel();
                Ok(1)
            }
        }
        assert_eq!(
            digest_matches(CancelsAfterRead(&operation), expected, &operation),
            Err(PermissionOutcome::Cancelled)
        );
    }
}
