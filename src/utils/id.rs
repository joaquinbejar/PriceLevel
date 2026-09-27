use crate::errors::PriceLevelError;
use crate::utils::TimestampMs;
use crate::utils::entropy::{EntropySource, UnixClock};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
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

impl FromStr for Id {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(id) = s.parse::<u64>() {
            return Ok(Self::Sequential(id));
        }

        if let Ok(uuid) = Uuid::from_str(s) {
            Ok(Self::Uuid(uuid))
        } else if let Ok(ulid) = Ulid::from_string(s) {
            Ok(Self::Ulid(ulid))
        } else {
            Err(PriceLevelError::ParseError {
                message: format!("Failed to parse Id as u64, UUID, or ULID: {s}"),
            })
        }
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Uuid(uuid) => write!(f, "{uuid}"),
            Self::Ulid(ulid) => write!(f, "{ulid}"),
            Self::Sequential(id) => write!(f, "{id}"),
        }
    }
}

impl Serialize for Id {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
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
    /// - The error reported by `clock`, unchanged (for [`SystemClock`]:
    ///   [`PriceLevelError::InvalidOperation`] before the Unix epoch).
    /// - [`PriceLevelError::InvalidFieldValue`] (field `timestamp_ms`) if the
    ///   time exceeds [`Id::ULID_MAX_TIMESTAMP_MS`].
    /// - The error reported by `entropy`, unchanged.
    ///
    /// [`SystemClock`]: crate::SystemClock
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
        // 10 bytes = 80 bits, so the fold never shifts a set bit past bit 79
        // and `from_parts` discards nothing.
        let random = random_bytes
            .iter()
            .fold(0_u128, |acc, byte| (acc << 8) | u128::from(*byte));
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
                let mut bytes = [0_u8; 16];
                bytes[8..16].copy_from_slice(&id.to_be_bytes());
                bytes
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
        let bytes = [
            ((id >> 56) & 0xFF) as u8,
            ((id >> 48) & 0xFF) as u8,
            ((id >> 40) & 0xFF) as u8,
            ((id >> 32) & 0xFF) as u8,
            ((id >> 24) & 0xFF) as u8,
            ((id >> 16) & 0xFF) as u8,
            ((id >> 8) & 0xFF) as u8,
            (id & 0xFF) as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        Self::Uuid(Uuid::from_bytes(bytes))
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
    use crate::errors::PriceLevelError;
    use crate::utils::{EntropySource, TimestampMs, UnixClock};
    use crate::{Side, SystemClock};
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
        let clock: &dyn UnixClock = &SystemClock;
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
            // Not an all-zero ULID: its text form is all digits and `Id::from_str`
            // tries `u64` first, so it would parse back as `Sequential(0)`.
            Id::try_new_ulid_at(TimestampMs::ZERO, &mut ConstEntropy(0xFF)).unwrap(),
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
}
