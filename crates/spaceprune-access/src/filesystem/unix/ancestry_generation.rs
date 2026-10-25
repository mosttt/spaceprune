//! Decode the FD-based Darwin directory generation reply, including attribute support.

use crate::{IoFailureKind, error::failure};
use spaceprune_contracts::messages;
use std::io;

// sys/attr.h: ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_GEN_COUNT with FSOPT_ATTR_CMN_EXTENDED.
pub(super) const COMMON_ATTRIBUTES: u32 = 0x8008_0000;
pub(super) type Reply = [u32; 7];

pub(super) fn decode(reply: &Reply) -> io::Result<u32> {
    // Length + five returned attribute bitmaps + one u32 generation, all native-endian.
    // Without support the kernel can succeed with only the returned-attributes header.
    if reply[0] != std::mem::size_of::<Reply>() as u32
        || reply[1] != COMMON_ATTRIBUTES
        || reply[2..6] != [0; 4]
        || reply[6] == 0
    {
        return Err(failure(
            IoFailureKind::Unsupported,
            messages::text("native.ancestry_generation_unavailable"),
        ));
    }
    Ok(reply[6])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unsupported_or_incomplete_attribute_is_not_an_unchanged_directory() {
        // Independent wire layouts: unsupported header, missing support bit, zero counter,
        // truncated/oversized reply and unexpected fields that would change attribute packing.
        for reply in [
            [24, 0x8000_0000, 0, 0, 0, 0, 0],
            [28, 0x8000_0000, 0, 0, 0, 0, 17],
            [28, 0x8008_0000, 0, 0, 0, 0, 0],
            [24, 0x8008_0000, 0, 0, 0, 0, 17],
            [32, 0x8008_0000, 0, 0, 0, 0, 17],
            [28, 0x8008_0000, 1, 0, 0, 0, 17],
        ] {
            let error = decode(&reply).unwrap_err();
            assert_eq!(
                crate::error::classify(&error).kind,
                IoFailureKind::Unsupported
            );
        }
    }

    #[test]
    fn supported_generation_keeps_the_full_unsigned_counter() {
        assert_eq!(decode(&[28, 0x8008_0000, 0, 0, 0, 0, 17]).unwrap(), 17);
        assert_eq!(
            decode(&[28, 0x8008_0000, 0, 0, 0, 0, u32::MAX]).unwrap(),
            u32::MAX
        );
    }
}
