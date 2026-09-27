//! Allocation-free text encoders for fixed-size identifiers (issue #201).
//!
//! Snapshot capture, packaging, checksum validation and restore serialize
//! every resting order's `user_id` ([`Hash32`](crate::Hash32)); before #201
//! each one was hex-encoded through a heap `String` built from 32 more
//! `format!` temporaries. These helpers write the text into a caller-owned
//! stack buffer instead and hand back the borrowed `&str`.
//!
//! Every helper uses checked access only (no indexing or slicing
//! expressions) and reports an impossible-in-practice failure as a typed
//! [`fmt::Error`] instead of panicking, so callers can surface it as a
//! `Display` or serde error.
//!
//! This module is a leaf: it depends only on `std`.

use std::fmt;

/// Lowercase hexadecimal digits, indexed by nibble value.
const HEX_DIGITS_LOWER: &[u8; 16] = b"0123456789abcdef";

/// Bytes in a [`Hash32`](crate::Hash32).
pub(crate) const HASH32_LEN: usize = 32;

/// Characters in the lowercase hex text of a [`Hash32`](crate::Hash32).
pub(crate) const HASH32_HEX_LEN: usize = 64;

/// Returns the lowercase hex digit for `nibble` through a checked table
/// lookup. `nibble` is always `< 16` at the call sites (a byte shifted right
/// by 4, or masked with `0x0f`), so the `Err` arm is never taken.
#[inline]
fn hex_digit(nibble: u8) -> Result<u8, fmt::Error> {
    HEX_DIGITS_LOWER
        .get(usize::from(nibble))
        .copied()
        .ok_or(fmt::Error)
}

/// Writes the lowercase hex encoding of `bytes` into `buf` and returns it as
/// a borrowed `&str`, without allocating.
///
/// The output is exactly `bytes.iter().map(|b| format!("{b:02x}")).collect()`
/// (the pre-#201 `Hash32::to_hex` form): two digits per byte, high nibble
/// first. Each byte is zipped with its two-byte output chunk, so every slot
/// is written exactly once without an index.
///
/// # Errors
///
/// [`fmt::Error`] if a nibble lookup or the UTF-8 view of the buffer fails.
/// Neither can happen (nibbles are `< 16` and every digit is ASCII); the
/// typed error keeps the path panic-free.
#[inline]
pub(crate) fn encode_hash32_hex<'b>(
    bytes: &[u8; HASH32_LEN],
    buf: &'b mut [u8; HASH32_HEX_LEN],
) -> Result<&'b str, fmt::Error> {
    let (pairs, _) = buf.as_chunks_mut::<2>();
    for (pair, byte) in pairs.iter_mut().zip(bytes) {
        *pair = [hex_digit(byte >> 4)?, hex_digit(byte & 0x0f)?];
    }
    let buf: &[u8] = buf;
    std::str::from_utf8(buf).map_err(|_| fmt::Error)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pre-#201 allocating reference form.
    fn reference_hex(bytes: &[u8; HASH32_LEN]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn test_encode_hash32_hex_matches_reference_on_edges() {
        let mut ascending = [0u8; HASH32_LEN];
        for (value, slot) in (0u8..).zip(ascending.iter_mut()) {
            *slot = value.wrapping_mul(37);
        }
        for bytes in [
            [0u8; HASH32_LEN],
            [0xff; HASH32_LEN],
            [0x0f; HASH32_LEN],
            ascending,
        ] {
            let mut buf = [0u8; HASH32_HEX_LEN];
            assert_eq!(
                encode_hash32_hex(&bytes, &mut buf),
                Ok(reference_hex(&bytes).as_str())
            );
        }
    }

    #[test]
    fn test_hex_digit_rejects_out_of_range_nibble() {
        assert_eq!(hex_digit(15), Ok(b'f'));
        assert_eq!(hex_digit(16), Err(fmt::Error));
    }

    proptest::proptest! {
        #[test]
        fn prop_encode_hash32_hex_is_byte_identical_to_reference(bytes: [u8; HASH32_LEN]) {
            let mut buf = [0u8; HASH32_HEX_LEN];
            let expected = reference_hex(&bytes);
            proptest::prop_assert_eq!(encode_hash32_hex(&bytes, &mut buf), Ok(expected.as_str()));
        }
    }
}
