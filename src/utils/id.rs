use crate::errors::PriceLevelError;
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

impl Default for Id {
    fn default() -> Self {
        Self::new()
    }
}

impl Id {
    /// Create a new random id (defaults to ULID for better sortability).
    #[must_use]
    pub fn new() -> Self {
        Self::Ulid(Ulid::new())
    }

    /// Create a new UUID-based id.
    #[must_use]
    pub fn new_uuid() -> Self {
        Self::Uuid(Uuid::new_v4())
    }

    /// Create a new ULID-based id.
    #[must_use]
    pub fn new_ulid() -> Self {
        Self::Ulid(Ulid::new())
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
    use crate::Side;
    use std::str::FromStr;
    use uuid::Uuid;

    #[test]
    fn test_id_creation() {
        let id = Id::from_u64(12345);
        assert_eq!(id, Id::from_u64(12345));

        let id1 = Id::new();
        let id2 = Id::new();
        assert_ne!(id1, id2);

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
