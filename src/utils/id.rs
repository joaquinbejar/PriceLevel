use crate::errors::PriceLevelError;
use crate::utils::TimestampMs;
use crate::utils::entropy::{EntropySource, UnixClock};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::io::Write as _;
use std::str::FromStr;
use ulid::Ulid;
use uuid::Uuid;

/// Represents a unique identifier in the trading system.
///
/// This enum supports multiple ID formats to provide flexibility
/// in identifier handling across different systems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Id {
    /// UUID (Universally Unique Identifier) format.
    /// A 128-bit identifier that is globally unique across space and time.
    Uuid(Uuid),

    /// ULID (Universally Unique Lexicographically Sortable Identifier) format.
    /// A 128-bit identifier that is lexicographically sortable and globally unique.
    Ulid(Ulid),

    /// Sequential u64 identifier.
    /// Useful for CEX systems where orders are assigned sequential IDs per market.
    Sequential(u64),
}

/// Parses the text form written by [`Id`]'s `Display` impl.
///
/// The grammar is disambiguated by **shape**, most specific first:
///
/// 1. exactly [`ulid::ULID_LEN`] (26) characters that decode as Crockford
///    Base32 and whose first character is `0`..=`7` (26 × 5 = 130 bits, so a
///    larger leading digit would overflow the 128-bit value) → [`Id::Ulid`];
/// 2. any textual form `uuid` accepts (simple 32-hex, hyphenated 36, braced
///    38, `urn:uuid:` 45) → [`Id::Uuid`];
/// 3. otherwise a decimal `u64` as `u64::from_str` accepts it (leading zeros
///    and a leading `+` included) → [`Id::Sequential`].
///
/// A canonical `u64` rendering is at most 20 characters, so it never has a
/// ULID or UUID shape and `Sequential(n).to_string()` always parses back to
/// `Sequential(n)`. Trying the ULID and UUID shapes first is what stops an
/// all-digit ULID (e.g. the nil ULID `00000000000000000000000000`) from being
/// claimed as `Sequential` through its leading zeros, so
/// `id.to_string().parse::<Id>() == Ok(id)` holds for every [`Id`].
///
/// # Errors
///
/// [`PriceLevelError::ParseError`], carrying the input, when no rule matches.
impl FromStr for Id {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // `Ulid::from_string` also checks the length; the explicit guard keeps
        // the shape rule independent of that crate's internals. It does NOT
        // check overflow: a first character above `7` silently loses its top
        // bits (e.g. `8000…0` would decode to the nil ULID), so reject it here.
        if s.len() == ulid::ULID_LEN
            && matches!(s.as_bytes().first(), Some(b'0'..=b'7'))
            && let Ok(ulid) = Ulid::from_string(s)
        {
            return Ok(Self::Ulid(ulid));
        }

        if let Ok(uuid) = Uuid::from_str(s) {
            return Ok(Self::Uuid(uuid));
        }

        s.parse::<u64>()
            .map(Self::Sequential)
            .map_err(|_| PriceLevelError::ParseError {
                message: format!("Failed to parse Id as ULID, UUID, or u64: {s}"),
            })
    }
}

/// Size of the stack buffer [`Id::encode_text`] writes into: the widest
/// text form, a hyphenated UUID (36 characters). A ULID (26) and a decimal
/// `u64` (at most 20) both fit in its prefix.
const ID_TEXT_MAX_LEN: usize = uuid::fmt::Hyphenated::LENGTH;

impl Id {
    /// Writes the canonical text of this id into `buf` and returns it as a
    /// borrowed `&str`, without allocating (issue #201).
    ///
    /// The text is byte-identical to the pre-#201 `to_string` form and is
    /// exactly what [`FromStr`] parses back:
    ///
    /// - [`Id::Uuid`]: hyphenated lowercase (`Uuid::hyphenated().encode_lower`);
    /// - [`Id::Ulid`]: 26-character Crockford Base32 (`Ulid::array_to_str`);
    /// - [`Id::Sequential`]: plain decimal, no sign or leading zeros.
    ///
    /// # Errors
    ///
    /// [`fmt::Error`] if the fixed buffer prefix for a ULID or decimal cannot
    /// be borrowed or the decimal digits are not UTF-8. Neither can happen
    /// (both prefixes are shorter than the buffer and digits are ASCII); the
    /// typed error keeps the path panic-free.
    #[inline]
    fn encode_text<'b>(&self, buf: &'b mut [u8; ID_TEXT_MAX_LEN]) -> Result<&'b str, fmt::Error> {
        match self {
            // `encode_lower` requires a buffer of at least
            // `Hyphenated::LENGTH` bytes; `buf` is exactly that long by type.
            Self::Uuid(uuid) => Ok(uuid.hyphenated().encode_lower(buf)),
            Self::Ulid(ulid) => {
                let prefix = buf
                    .first_chunk_mut::<{ ulid::ULID_LEN }>()
                    .ok_or(fmt::Error)?;
                Ok(ulid.array_to_str(prefix))
            }
            Self::Sequential(value) => {
                // `std`'s integer formatting writes straight into the slice
                // through `io::Write for &mut [u8]` (no heap), so the digits
                // are exactly `value.to_string()`. The unwritten tail left in
                // `rest` gives the used length by checked subtraction.
                let capacity = buf.len();
                let mut rest: &mut [u8] = buf;
                write!(rest, "{value}").map_err(|_| fmt::Error)?;
                let used = capacity.checked_sub(rest.len()).ok_or(fmt::Error)?;
                let digits = buf.get(..used).ok_or(fmt::Error)?;
                std::str::from_utf8(digits).map_err(|_| fmt::Error)
            }
        }
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0u8; ID_TEXT_MAX_LEN];
        match self.encode_text(&mut buf) {
            Ok(text) => f.write_str(text),
            // Unreachable in practice; fall back to the components' own
            // `Display` so `to_string` never observes an encoder error.
            Err(fmt::Error) => match self {
                Self::Uuid(uuid) => write!(f, "{uuid}"),
                Self::Ulid(ulid) => write!(f, "{ulid}"),
                Self::Sequential(id) => write!(f, "{id}"),
            },
        }
    }
}

impl Serialize for Id {
    /// Serializes the canonical text through a stack buffer: no per-id heap
    /// allocation (issue #201). The wire form is unchanged.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut buf = [0u8; ID_TEXT_MAX_LEN];
        let text = self
            .encode_text(&mut buf)
            .map_err(|_| serde::ser::Error::custom("failed to encode Id text"))?;
        serializer.serialize_str(text)
    }
}

/// Serde visitor for [`Id`]: parses the borrowed string through
/// [`Id::from_str`] (issue #178 shape rules) without copying it.
struct IdVisitor;

impl serde::de::Visitor<'_> for IdVisitor {
    type Value = Id;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a string")
    }

    fn visit_str<E>(self, v: &str) -> Result<Id, E>
    where
        E: serde::de::Error,
    {
        Id::from_str(v).map_err(E::custom)
    }

    /// Owned-string fallback for deserializers that only hand out `String`s;
    /// parses in place without a further copy.
    fn visit_string<E>(self, v: String) -> Result<Id, E>
    where
        E: serde::de::Error,
    {
        self.visit_str(&v)
    }

    /// UTF-8 byte input, accepted exactly as `String`'s visitor accepts it
    /// (the pre-#201 `String::deserialize` path): invalid UTF-8 is an
    /// `invalid_value` error naming the bytes, valid text goes through
    /// [`Id::from_str`]. `char` input needs no override: the default
    /// `visit_char` forwards to `visit_str`.
    fn visit_bytes<E>(self, v: &[u8]) -> Result<Id, E>
    where
        E: serde::de::Error,
    {
        match std::str::from_utf8(v) {
            Ok(s) => self.visit_str(s),
            Err(_) => Err(E::invalid_value(serde::de::Unexpected::Bytes(v), &self)),
        }
    }

    /// Borrowed UTF-8 byte input; same rules as `visit_bytes`.
    fn visit_borrowed_bytes<E>(self, v: &[u8]) -> Result<Id, E>
    where
        E: serde::de::Error,
    {
        self.visit_bytes(v)
    }

    /// Owned UTF-8 byte input; same rules as `visit_bytes`.
    fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Id, E>
    where
        E: serde::de::Error,
    {
        self.visit_bytes(&v)
    }
}

impl<'de> Deserialize<'de> for Id {
    /// Deserializes through a borrowing `str` visitor: input is parsed in place
    /// (issue #201), with the exact #178 shape-based grammar of
    /// [`Id::from_str`].
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(IdVisitor)
    }
}

/// Number of random bytes in a ULID (its 80-bit randomness field).
const ULID_RANDOM_BYTES: usize = 10;

impl Id {
    /// Largest timestamp a ULID can carry: its time field is 48 bits wide.
    pub const ULID_MAX_TIMESTAMP_MS: u64 = (1_u64 << Ulid::TIME_BITS) - 1;

    /// Creates a new random id, ULID-based for lexicographic sortability.
    ///
    /// Equivalent to [`Id::try_new_ulid`]. Replaces the infallible `Id::new()`
    /// and the random `Default` impl, both removed in v0.10.
    ///
    /// # Errors
    ///
    /// See [`Id::try_new_ulid`].
    pub fn try_new<C, E>(clock: &C, entropy: &mut E) -> Result<Self, PriceLevelError>
    where
        C: UnixClock + ?Sized,
        E: EntropySource + ?Sized,
    {
        Self::try_new_ulid(clock, entropy)
    }

    /// Creates a new random version 4 UUID id.
    ///
    /// Sixteen bytes are drawn from `entropy`; the version (`4`) and variant
    /// (RFC 4122 / RFC 9562) bits are then set exactly as `Uuid::new_v4` sets
    /// them, so the wire format is unchanged.
    ///
    /// # Errors
    ///
    /// Returns the error reported by `entropy` unchanged (conventionally
    /// [`PriceLevelError::EntropyUnavailable`]). No identifier is produced and
    /// no fallback value is substituted.
    pub fn try_new_uuid<E>(entropy: &mut E) -> Result<Self, PriceLevelError>
    where
        E: EntropySource + ?Sized,
    {
        let mut bytes = [0_u8; 16];
        entropy.try_fill_bytes(&mut bytes)?;
        Ok(Self::Uuid(
            uuid::Builder::from_random_bytes(bytes).into_uuid(),
        ))
    }

    /// Creates a new random ULID id stamped with `clock`'s current time.
    ///
    /// The clock is read and validated **before** any entropy is drawn, so a
    /// clock or range failure leaves the entropy source untouched.
    ///
    /// # Errors
    ///
    /// - The error reported by `clock`, unchanged.
    /// - [`PriceLevelError::InvalidFieldValue`] (field `timestamp_ms`) if the
    ///   time exceeds [`Id::ULID_MAX_TIMESTAMP_MS`].
    /// - The error reported by `entropy`, unchanged.
    pub fn try_new_ulid<C, E>(clock: &C, entropy: &mut E) -> Result<Self, PriceLevelError>
    where
        C: UnixClock + ?Sized,
        E: EntropySource + ?Sized,
    {
        let timestamp = clock.try_now_ms()?;
        Self::try_new_ulid_at(timestamp, entropy)
    }

    /// Creates a new ULID id with an explicit timestamp and random bytes drawn
    /// from `entropy`.
    ///
    /// The timestamp is validated against the 48-bit ULID time field before
    /// any entropy is drawn; it is never masked or clamped.
    ///
    /// # Errors
    ///
    /// - [`PriceLevelError::InvalidFieldValue`] (field `timestamp_ms`) if
    ///   `timestamp` exceeds [`Id::ULID_MAX_TIMESTAMP_MS`]. The entropy source
    ///   is not consulted.
    /// - The error reported by `entropy`, unchanged.
    pub fn try_new_ulid_at<E>(
        timestamp: TimestampMs,
        entropy: &mut E,
    ) -> Result<Self, PriceLevelError>
    where
        E: EntropySource + ?Sized,
    {
        let timestamp_ms = timestamp.as_u64();
        if timestamp_ms > Self::ULID_MAX_TIMESTAMP_MS {
            return Err(PriceLevelError::InvalidFieldValue {
                field: "timestamp_ms".to_string(),
                value: timestamp_ms.to_string(),
            });
        }
        let mut random_bytes = [0_u8; ULID_RANDOM_BYTES];
        entropy.try_fill_bytes(&mut random_bytes)?;
        // The 80 random bits are the low ten bytes of a big-endian `u128`, so
        // `from_parts` discards nothing.
        let [r0, r1, r2, r3, r4, r5, r6, r7, r8, r9] = random_bytes;
        let random =
            u128::from_be_bytes([0, 0, 0, 0, 0, 0, r0, r1, r2, r3, r4, r5, r6, r7, r8, r9]);
        Ok(Self::Ulid(Ulid::from_parts(timestamp_ms, random)))
    }

    /// Create a nil UUID id.
    #[must_use]
    pub fn nil() -> Self {
        Self::Uuid(Uuid::nil())
    }

    /// Create an id from an existing UUID.
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self::Uuid(uuid)
    }

    /// Create an id from an existing ULID.
    #[must_use]
    pub fn from_ulid(ulid: Ulid) -> Self {
        Self::Ulid(ulid)
    }

    /// Get identifier bytes.
    ///
    /// UUID and ULID return 16 bytes.
    /// Sequential returns 8 bytes zero-padded to 16 bytes.
    #[must_use]
    pub fn as_bytes(&self) -> [u8; 16] {
        match self {
            Self::Uuid(uuid) => *uuid.as_bytes(),
            Self::Ulid(ulid) => ulid.to_bytes(),
            Self::Sequential(id) => {
                // Zero-leading padding: the big-endian value occupies the
                // trailing eight bytes.
                let [b0, b1, b2, b3, b4, b5, b6, b7] = id.to_be_bytes();
                [0, 0, 0, 0, 0, 0, 0, 0, b0, b1, b2, b3, b4, b5, b6, b7]
            }
        }
    }

    /// Create a sequential id from a u64.
    #[must_use]
    pub fn sequential(id: u64) -> Self {
        Self::Sequential(id)
    }

    /// Create an id from a u64 by embedding it in a UUID.
    ///
    /// This exists for backward compatibility.
    #[must_use]
    pub fn from_u64(id: u64) -> Self {
        // The big-endian value occupies the leading eight UUID bytes; the
        // trailing eight are zero.
        let [b0, b1, b2, b3, b4, b5, b6, b7] = id.to_be_bytes();
        Self::Uuid(Uuid::from_bytes([
            b0, b1, b2, b3, b4, b5, b6, b7, 0, 0, 0, 0, 0, 0, 0, 0,
        ]))
    }

    /// Returns the u64 value when the id is sequential.
    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Sequential(id) => Some(*id),
            _ => None,
        }
    }

    /// Returns `true` if the id is sequential.
    #[must_use]
    pub fn is_sequential(&self) -> bool {
        matches!(self, Self::Sequential(_))
    }

    /// Returns `true` if the id is UUID-based.
    #[must_use]
    pub fn is_uuid(&self) -> bool {
        matches!(self, Self::Uuid(_))
    }

    /// Returns `true` if the id is ULID-based.
    #[must_use]
    pub fn is_ulid(&self) -> bool {
        matches!(self, Self::Ulid(_))
    }
}

#[cfg(test)]
mod tests {
    use super::Id;
    use crate::Side;
    use crate::errors::PriceLevelError;
    use crate::utils::{EntropySource, TimestampMs, UnixClock};
    use std::str::FromStr;
    use uuid::{Uuid, Variant};

    /// Deterministic entropy: emits `next, next+1, ...` (wrapping) and counts
    /// calls / bytes drawn.
    struct CountingEntropy {
        next: u8,
        calls: usize,
        bytes_drawn: usize,
    }

    impl CountingEntropy {
        fn starting_at(next: u8) -> Self {
            Self {
                next,
                calls: 0,
                bytes_drawn: 0,
            }
        }
    }

    impl EntropySource for CountingEntropy {
        fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), PriceLevelError> {
            self.calls += 1;
            for byte in dest.iter_mut() {
                *byte = self.next;
                self.next = self.next.wrapping_add(1);
            }
            self.bytes_drawn += dest.len();
            Ok(())
        }
    }

    /// Fills every byte with one constant value.
    struct ConstEntropy(u8);

    impl EntropySource for ConstEntropy {
        fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), PriceLevelError> {
            dest.fill(self.0);
            Ok(())
        }
    }

    /// Simulates an OS entropy / RNG (re)seed failure.
    struct FailingEntropy {
        calls: usize,
    }

    impl EntropySource for FailingEntropy {
        fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), PriceLevelError> {
            self.calls += 1;
            // Scribble on the buffer to prove partial output is discarded.
            dest.fill(0xAB);
            Err(PriceLevelError::EntropyUnavailable {
                message: "injected getrandom failure".to_string(),
            })
        }
    }

    struct FixedClock(u64);

    impl UnixClock for FixedClock {
        fn try_now_ms(&self) -> Result<TimestampMs, PriceLevelError> {
            Ok(TimestampMs::new(self.0))
        }
    }

    struct FailingClock;

    impl UnixClock for FailingClock {
        fn try_now_ms(&self) -> Result<TimestampMs, PriceLevelError> {
            Err(PriceLevelError::InvalidOperation {
                message: "injected clock failure".to_string(),
            })
        }
    }

    fn expect_uuid(id: Id) -> Uuid {
        match id {
            Id::Uuid(uuid) => uuid,
            other => panic!("expected a UUID id, got {other:?}"),
        }
    }

    fn expect_ulid(id: Id) -> ulid::Ulid {
        match id {
            Id::Ulid(ulid) => ulid,
            other => panic!("expected a ULID id, got {other:?}"),
        }
    }

    #[test]
    fn test_try_new_uuid_sets_v4_version_and_variant_bits() {
        for fill in [0x00_u8, 0xFF, 0x5A, 0xA5] {
            let uuid = expect_uuid(Id::try_new_uuid(&mut ConstEntropy(fill)).unwrap());
            assert_eq!(uuid.get_version_num(), 4, "fill {fill:#04x}");
            assert_eq!(uuid.get_variant(), Variant::RFC4122, "fill {fill:#04x}");
        }
    }

    #[test]
    fn test_try_new_uuid_preserves_random_bits() {
        let mut entropy = CountingEntropy::starting_at(0);
        let uuid = expect_uuid(Id::try_new_uuid(&mut entropy).unwrap());
        assert_eq!(entropy.calls, 1);
        assert_eq!(entropy.bytes_drawn, 16);

        let expected: [u8; 16] = std::array::from_fn(|i| u8::try_from(i).unwrap());
        let bytes = uuid.as_bytes();
        for (index, (got, want)) in bytes.iter().zip(expected.iter()).enumerate() {
            match index {
                // Version nibble lives in the high half of byte 6.
                6 => assert_eq!(*got, (want & 0x0F) | 0x40),
                // Variant bits `10` live in the top of byte 8.
                8 => assert_eq!(*got, (want & 0x3F) | 0x80),
                _ => assert_eq!(got, want, "byte {index}"),
            }
        }
    }

    #[test]
    fn test_try_new_uuid_propagates_entropy_failure() {
        let mut entropy = FailingEntropy { calls: 0 };
        let err = Id::try_new_uuid(&mut entropy).unwrap_err();
        assert!(matches!(
            err,
            PriceLevelError::EntropyUnavailable { ref message } if message == "injected getrandom failure"
        ));
        assert_eq!(entropy.calls, 1);
    }

    #[test]
    fn test_try_new_ulid_at_encodes_timestamp_and_random_fields() {
        let mut entropy = CountingEntropy::starting_at(1);
        let timestamp = TimestampMs::new(1_716_000_000_123);
        let ulid = expect_ulid(Id::try_new_ulid_at(timestamp, &mut entropy).unwrap());
        assert_eq!(entropy.calls, 1);
        assert_eq!(entropy.bytes_drawn, 10);
        assert_eq!(ulid.timestamp_ms(), 1_716_000_000_123);
        assert_eq!(ulid.random(), 0x0102_0304_0506_0708_090A_u128);
    }

    #[test]
    fn test_as_bytes_sequential_is_zero_leading_big_endian() {
        let id = Id::sequential(0x0102_0304_0506_0708);
        assert_eq!(
            id.as_bytes(),
            [0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]
        );
        assert_eq!(Id::sequential(0).as_bytes(), [0; 16]);
        let mut max = [0xFF_u8; 16];
        max[..8].fill(0);
        assert_eq!(Id::sequential(u64::MAX).as_bytes(), max);
    }

    #[test]
    fn test_from_u64_encodes_value_in_leading_uuid_bytes() {
        let id = Id::from_u64(0x0102_0304_0506_0708);
        assert_eq!(
            id.as_bytes(),
            [1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert!(id.is_uuid());
        assert_eq!(Id::from_u64(0), Id::nil());
        let mut max = [0_u8; 16];
        max[..8].fill(0xFF);
        assert_eq!(Id::from_u64(u64::MAX).as_bytes(), max);
    }

    #[test]
    fn test_try_new_ulid_at_accepts_timestamp_boundaries() {
        for ts in [
            0,
            1,
            Id::ULID_MAX_TIMESTAMP_MS - 1,
            Id::ULID_MAX_TIMESTAMP_MS,
        ] {
            let ulid = expect_ulid(
                Id::try_new_ulid_at(TimestampMs::new(ts), &mut ConstEntropy(0xFF)).unwrap(),
            );
            assert_eq!(ulid.timestamp_ms(), ts);
            assert_eq!(ulid.random(), (1_u128 << 80) - 1);
        }
        assert_eq!(Id::ULID_MAX_TIMESTAMP_MS, (1_u64 << 48) - 1);
    }

    #[test]
    fn test_try_new_ulid_at_rejects_out_of_range_timestamp_without_drawing_entropy() {
        for ts in [Id::ULID_MAX_TIMESTAMP_MS + 1, u64::MAX] {
            let mut entropy = CountingEntropy::starting_at(0);
            let err = Id::try_new_ulid_at(TimestampMs::new(ts), &mut entropy).unwrap_err();
            match err {
                PriceLevelError::InvalidFieldValue { field, value } => {
                    assert_eq!(field, "timestamp_ms");
                    assert_eq!(value, ts.to_string());
                }
                other => panic!("unexpected error {other:?}"),
            }
            assert_eq!(entropy.calls, 0);
            assert_eq!(entropy.next, 0);
        }
    }

    #[test]
    fn test_try_new_ulid_propagates_entropy_failure() {
        let mut entropy = FailingEntropy { calls: 0 };
        let err = Id::try_new_ulid(&FixedClock(42), &mut entropy).unwrap_err();
        assert!(matches!(err, PriceLevelError::EntropyUnavailable { .. }));
        assert_eq!(entropy.calls, 1);
    }

    #[test]
    fn test_try_new_ulid_propagates_clock_failure_without_drawing_entropy() {
        let mut entropy = CountingEntropy::starting_at(0);
        let err = Id::try_new_ulid(&FailingClock, &mut entropy).unwrap_err();
        assert!(matches!(
            err,
            PriceLevelError::InvalidOperation { ref message } if message == "injected clock failure"
        ));
        assert_eq!(entropy.calls, 0);

        let err = Id::try_new(&FixedClock(u64::MAX), &mut entropy).unwrap_err();
        assert!(matches!(err, PriceLevelError::InvalidFieldValue { .. }));
        assert_eq!(entropy.calls, 0);
    }

    #[test]
    fn test_try_new_is_ulid_and_matches_try_new_ulid() {
        let clock = FixedClock(7);
        let a = Id::try_new(&clock, &mut CountingEntropy::starting_at(9)).unwrap();
        let b = Id::try_new_ulid(&clock, &mut CountingEntropy::starting_at(9)).unwrap();
        assert!(a.is_ulid());
        assert_eq!(a, b);
    }

    #[test]
    fn test_random_constructors_accept_trait_objects() {
        let mut source = CountingEntropy::starting_at(0);
        let entropy: &mut dyn EntropySource = &mut source;
        let clock: &dyn UnixClock = &FixedClock(1_716_000_000_000);
        let first = Id::try_new(clock, entropy).unwrap();
        let second = Id::try_new_uuid(entropy).unwrap();
        assert!(first.is_ulid());
        assert!(second.is_uuid());
        assert_ne!(first.as_bytes(), second.as_bytes());
        assert_eq!(source.calls, 2);
    }

    #[test]
    fn test_distinct_entropy_yields_distinct_ids() {
        let mut entropy = CountingEntropy::starting_at(0);
        let clock = FixedClock(1_000);
        let a = Id::try_new(&clock, &mut entropy).unwrap();
        let b = Id::try_new(&clock, &mut entropy).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn test_random_ids_round_trip_through_serde_and_from_str() {
        let mut entropy = CountingEntropy::starting_at(0x10);
        let ids = [
            Id::try_new_uuid(&mut entropy).unwrap(),
            Id::try_new_ulid(&FixedClock(1_716_000_000_000), &mut entropy).unwrap(),
            Id::try_new_ulid_at(
                TimestampMs::new(Id::ULID_MAX_TIMESTAMP_MS),
                &mut ConstEntropy(0xFF),
            )
            .unwrap(),
            Id::try_new_ulid_at(TimestampMs::ZERO, &mut ConstEntropy(0xFF)).unwrap(),
            // All-zero (nil) ULID: its text form is all digits (#178).
            Id::try_new_ulid_at(TimestampMs::ZERO, &mut ConstEntropy(0x00)).unwrap(),
        ];
        for id in ids {
            let json = serde_json::to_string(&id).unwrap();
            let back: Id = serde_json::from_str(&json).unwrap();
            assert_eq!(back, id, "serde round-trip of {json}");
            assert_eq!(Id::from_str(&id.to_string()).unwrap(), id);
        }
    }

    #[test]
    fn test_id_creation() {
        let id = Id::from_u64(12345);
        assert_eq!(id, Id::from_u64(12345));

        let uuid = Uuid::new_v4();
        let id = Id::from_uuid(uuid);
        assert_eq!(id, Id::Uuid(uuid));

        let nil_id = Id::nil();
        assert_eq!(nil_id, Id::Uuid(Uuid::nil()));
    }

    #[test]
    fn test_id_serialize_deserialize() {
        let id = Id::from_u64(12345);
        let serialized = serde_json::to_string(&id).unwrap();
        let expected_uuid = id.to_string();
        assert!(serialized.contains(&expected_uuid));

        let deserialized: Id = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, id);
    }

    #[test]
    fn test_from_str_valid() {
        let uuid_str = "550e8400-e29b-41d4-a716-446655440000";
        let id = Id::from_str(uuid_str).unwrap();
        assert_eq!(id.to_string(), uuid_str);

        let id_from_u64 = Id::from_u64(12345);
        let parsed = Id::from_str(&id_from_u64.to_string()).unwrap();
        assert_eq!(id_from_u64, parsed);
    }

    #[test]
    fn test_from_str_invalid() {
        assert!(Id::from_str("").is_err());
        assert!(Id::from_str("not-a-uuid").is_err());
    }

    #[test]
    fn test_side_opposite() {
        assert_eq!(Side::Buy.opposite(), Side::Sell);
        assert_eq!(Side::Sell.opposite(), Side::Buy);
    }

    #[test]
    fn test_sequential_helpers() {
        let id = Id::sequential(42);
        assert!(id.is_sequential());
        assert_eq!(id.as_u64(), Some(42));
        assert_eq!(id.to_string(), "42");

        let parsed: Id = "42".parse().unwrap();
        assert_eq!(parsed, id);
    }

    /// Asserts `Display` -> `FromStr` and serde JSON both reproduce `id`.
    fn assert_round_trips(id: Id) {
        let text = id.to_string();
        assert_eq!(Id::from_str(&text).unwrap(), id, "from_str of {text}");
        let json = serde_json::to_string(&id).unwrap();
        let back: Id = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id, "serde round-trip of {json}");
    }

    #[test]
    fn test_from_str_nil_ulid_is_ulid_not_sequential() {
        let nil = Id::from_ulid(ulid::Ulid::nil());
        assert_eq!(nil.to_string(), "00000000000000000000000000");
        let parsed = Id::from_str("00000000000000000000000000").unwrap();
        assert!(parsed.is_ulid(), "got {parsed:?}");
        assert_eq!(parsed, nil);
        assert_round_trips(nil);
    }

    #[test]
    fn test_from_str_near_epoch_all_digit_ulids_are_ulid() {
        // Each Crockford digit 0-9 encodes 0-9, so these ULIDs render as
        // decimal digits only and (with >= 6 leading zeros) fit in `u64`.
        let cases = [
            (0_u64, 1_u128),
            (0, 9),
            (0, 0x1234),
            (1, 0),
            (9, 0),
            (1, 1),
            (0, u128::from(u32::MAX)),
        ];
        for (ts, random) in cases {
            let id = Id::from_ulid(ulid::Ulid::from_parts(ts, random));
            let text = id.to_string();
            assert_eq!(text.len(), 26);
            if ts == 0 && random <= u128::from(u32::MAX) && text.bytes().all(|b| b.is_ascii_digit())
            {
                // The ambiguous shape: also a valid (non-canonical) `u64`.
                assert!(text.parse::<u64>().is_ok(), "text {text}");
            }
            assert!(Id::from_str(&text).unwrap().is_ulid(), "text {text}");
            assert_round_trips(id);
        }
        // A literal all-digit 26-char text that is also a valid `u64`.
        let text = "00000000000000000000000042";
        assert_eq!(text.parse::<u64>().unwrap(), 42);
        let parsed = Id::from_str(text).unwrap();
        assert_eq!(
            parsed,
            Id::from_ulid(ulid::Ulid::from_parts(0, 0x4 * 32 + 2))
        );
    }

    #[test]
    fn test_from_str_sequential_boundaries() {
        for n in [0, 1, 42, u64::MAX - 1, u64::MAX] {
            let id = Id::sequential(n);
            assert_eq!(Id::from_str(&n.to_string()).unwrap(), id);
            assert_round_trips(id);
        }
        assert_eq!(u64::MAX.to_string().len(), 20);
    }

    #[test]
    fn test_from_str_keeps_non_canonical_decimal_acceptance() {
        // Non-canonical decimals that are neither ULID- nor UUID-shaped keep
        // parsing as `Sequential`, exactly as before #178.
        assert_eq!(Id::from_str("007").unwrap(), Id::sequential(7));
        assert_eq!(Id::from_str("+42").unwrap(), Id::sequential(42));
        assert_eq!(
            Id::from_str("0000000000000000000000042").unwrap(),
            Id::sequential(42)
        );
        assert_eq!(
            Id::from_str("000000000000000000000000042").unwrap(),
            Id::sequential(42)
        );
        // A 26-char text that is not Crockford Base32 still falls back to u64.
        assert_eq!(
            Id::from_str("+0000000000000000000000042").unwrap(),
            Id::sequential(42)
        );
    }

    #[test]
    fn test_from_str_uuid_forms() {
        let uuid = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        for text in [
            "550e8400-e29b-41d4-a716-446655440000",
            "550E8400-E29B-41D4-A716-446655440000",
            "550e8400e29b41d4a716446655440000",
            "{550e8400-e29b-41d4-a716-446655440000}",
            "urn:uuid:550e8400-e29b-41d4-a716-446655440000",
        ] {
            assert_eq!(Id::from_str(text).unwrap(), Id::from_uuid(uuid), "{text}");
        }
        // A 32-digit simple-form text is a UUID, even though `u64` would
        // accept it through its leading zeros.
        let text = "00000000000000000000000000000042";
        assert_eq!(
            Id::from_str(text).unwrap(),
            Id::from_uuid(Uuid::from_u128(0x42))
        );
        for id in [
            Id::nil(),
            Id::from_uuid(uuid),
            Id::from_uuid(Uuid::from_u128(1)),
            Id::from_uuid(Uuid::max()),
            Id::from_u64(12345),
        ] {
            assert_round_trips(id);
        }
    }

    #[test]
    fn test_from_str_errors_carry_input() {
        for text in [
            "",
            "not-a-uuid",
            "-1",
            "18446744073709551616",
            // 26 chars, first digit > 7: overflows 128 bits and u64.
            "80000000000000000000000000",
            "0000000000000000000000000U",
        ] {
            match Id::from_str(text) {
                Err(PriceLevelError::ParseError { message }) => {
                    assert!(message.ends_with(text), "{message}");
                }
                other => panic!("expected ParseError for {text:?}, got {other:?}"),
            }
            let json = serde_json::to_string(text).unwrap();
            assert!(serde_json::from_str::<Id>(&json).is_err(), "{json}");
        }
    }

    mod proptests {
        use super::super::Id;
        use proptest::prelude::*;
        use std::str::FromStr;
        use ulid::Ulid;
        use uuid::Uuid;

        fn any_id() -> impl Strategy<Value = Id> {
            prop_oneof![
                any::<u64>().prop_map(Id::sequential),
                any::<u128>().prop_map(|v| Id::from_uuid(Uuid::from_u128(v))),
                any::<u128>().prop_map(|v| Id::from_ulid(Ulid(v))),
                // Near-epoch ULIDs with small randomness: all-digit texts.
                (0_u64..1_000, 0_u128..1_000_000)
                    .prop_map(|(ts, r)| Id::from_ulid(Ulid::from_parts(ts, r))),
                // Small sequentials, including 0.
                (0_u64..1_000).prop_map(Id::sequential),
            ]
        }

        proptest! {
            #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]

            #[test]
            fn prop_display_from_str_round_trip(id in any_id()) {
                let text = id.to_string();
                prop_assert_eq!(Id::from_str(&text).ok(), Some(id));
            }

            #[test]
            fn prop_serde_round_trip(id in any_id()) {
                let json = serde_json::to_string(&id).map_err(|e| TestCaseError::fail(e.to_string()))?;
                let back: Id = serde_json::from_str(&json).map_err(|e| TestCaseError::fail(e.to_string()))?;
                prop_assert_eq!(back, id);
            }

            #[test]
            fn prop_from_str_never_panics(text in ".{0,48}") {
                let _ = Id::from_str(&text);
            }

            // ---- issue #201: stack-buffer text / serde equivalence ----

            #[test]
            fn prop_display_is_byte_identical_to_pre_201_form(id in any_id()) {
                let expected = reference_text(id);
                prop_assert_eq!(id.to_string(), expected.clone());
                let mut buf = [0u8; super::super::ID_TEXT_MAX_LEN];
                prop_assert_eq!(id.encode_text(&mut buf), Ok(expected.as_str()));
                let json = serde_json::to_string(&id)
                    .map_err(|e| TestCaseError::fail(e.to_string()))?;
                prop_assert_eq!(json, reference_json(id));
            }

            #[test]
            fn prop_deserialize_matches_from_str(text in id_like_text()) {
                let expected = Id::from_str(&text).ok();
                let json = serde_json::to_string(&text)
                    .map_err(|e| TestCaseError::fail(e.to_string()))?;
                // Borrowed path (`visit_str`).
                prop_assert_eq!(serde_json::from_str::<Id>(&json).ok(), expected);
                // Reader path (scratch-buffer `visit_str`).
                prop_assert_eq!(serde_json::from_reader::<_, Id>(json.as_bytes()).ok(), expected);
                // Owned path (`visit_string`).
                let value = serde_json::Value::String(text.clone());
                prop_assert_eq!(serde_json::from_value::<Id>(value).ok(), expected);
            }
        }

        /// The pre-#201 allocating text form (test-only reference).
        fn reference_text(id: Id) -> String {
            match id {
                Id::Uuid(uuid) => uuid.to_string(),
                Id::Ulid(ulid) => ulid.to_string(),
                Id::Sequential(n) => n.to_string(),
            }
        }

        /// The pre-#201 JSON form: a string holding [`reference_text`].
        fn reference_json(id: Id) -> String {
            format!("\"{}\"", reference_text(id))
        }

        /// Arbitrary strings plus every id shape and its near misses, so the
        /// #178 shape rules are exercised on valid and invalid input alike.
        fn id_like_text() -> impl Strategy<Value = String> {
            prop_oneof![
                any_id().prop_map(reference_text),
                any_id().prop_map(|id| reference_text(id).to_uppercase()),
                any_id().prop_map(|id| format!("+{}", reference_text(id))),
                any::<u128>().prop_map(|v| Uuid::from_u128(v).simple().to_string()),
                any::<u128>().prop_map(|v| Uuid::from_u128(v).urn().to_string()),
                "[0-9A-Za-z]{26}",
                "[0-9]{0,24}",
                ".{0,48}",
            ]
        }

        #[test]
        fn test_encode_text_sequential_width_boundaries() {
            let mut power: u64 = 1;
            let mut values = vec![0, u64::MAX - 1, u64::MAX];
            loop {
                values.extend([power - 1, power, power + 1]);
                match power.checked_mul(10) {
                    Some(next) => power = next,
                    None => break,
                }
            }
            for value in values {
                let id = Id::sequential(value);
                let mut buf = [0u8; super::super::ID_TEXT_MAX_LEN];
                assert_eq!(id.encode_text(&mut buf), Ok(value.to_string().as_str()));
            }
        }

        #[test]
        fn test_deserialize_rejects_non_string_with_serde_type_error() {
            let err = serde_json::from_str::<Id>("42")
                .err()
                .map(|e| e.to_string());
            assert_eq!(
                err.as_deref(),
                Some("invalid type: integer `42`, expected a string at line 1 column 2")
            );
        }

        #[test]
        fn test_deserialize_bytes_and_char_match_pre_201_string_path() {
            use crate::utils::encode::serde_parity::assert_byte_and_char_parity;
            let texts = [
                reference_text(Id::sequential(1)),
                reference_text(Id::sequential(u64::MAX)),
                reference_text(Id::from_uuid(Uuid::from_u128(0x0123_4567_89ab_cdef))),
                reference_text(Id::from_ulid(Ulid(u128::MAX >> 2))),
                "00000000000000000000000000".to_string(),
                "+7".to_string(),
                "not-an-id".to_string(),
                String::new(),
            ];
            for text in &texts {
                assert_byte_and_char_parity::<Id>(text.as_bytes(), &[]);
            }
            // Invalid UTF-8 and single-char input.
            assert_byte_and_char_parity::<Id>(&[0xff, b'1'], &['7', '0', 'x', 'é']);
            assert_byte_and_char_parity::<Id>(&[b'1', 0xc3], &[]);
        }

        #[test]
        fn test_deserialize_bytes_accepts_utf8_like_base() {
            use serde::Deserialize;
            use serde::de::value::{BytesDeserializer, Error as ValueError};
            let de = BytesDeserializer::<ValueError>::new(b"1");
            assert_eq!(Id::deserialize(de).ok(), Some(Id::sequential(1)));
            let bad = BytesDeserializer::<ValueError>::new(&[0xff]);
            assert_eq!(
                Id::deserialize(bad).err().map(|e| e.to_string()).as_deref(),
                Some("invalid value: byte array, expected a string")
            );
        }

        #[test]
        fn test_deserialize_error_carries_input() {
            let err = serde_json::from_str::<Id>("\"not-an-id\"")
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(err.contains("not-an-id"), "{err}");
        }

        proptest! {
            #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]
            #[test]
            fn prop_escaped_json_uses_owned_fallback(id in any_id()) {
                // An escaped character forces serde_json to unescape into an
                // owned scratch buffer; the result must still match.
                let text = reference_text(id);
                let mut chars = text.chars();
                let first = chars.next().map_or(0, u32::from);
                let json = format!("\"\\u{first:04x}{}\"", chars.as_str());
                prop_assert_eq!(serde_json::from_str::<Id>(&json).ok(), Some(id));
            }
        }
    }
}
