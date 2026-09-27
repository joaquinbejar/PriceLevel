use crate::errors::PriceLevelError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Domain value type representing a price.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Price(u128);

impl Price {
    /// Zero price value.
    pub const ZERO: Self = Self(0);

    /// Creates a new price from a raw integer value.
    #[must_use]
    pub const fn new(value: u128) -> Self {
        Self(value)
    }

    /// Creates a validated price from a raw integer value.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidFieldValue`] if `value` ever fails the
    /// price invariant. Every `u128` is currently a valid price, so this
    /// constructor is presently infallible; the `Result` is part of its stable
    /// contract so validation can be tightened without a breaking change.
    pub fn try_new(value: u128) -> Result<Self, PriceLevelError> {
        Ok(Self(value))
    }

    /// Creates a price from an `f64` value (rounded to the nearest integer).
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if `value` is not finite
    /// (NaN or infinite), is negative, or (after rounding) does not fit in a
    /// `u128`. The range check is explicit because an `f64`-to-`u128` `as` cast
    /// saturates rather than failing, which would silently clamp out-of-range
    /// input to `u128::MAX`.
    pub fn from_f64(value: f64) -> Result<Self, PriceLevelError> {
        if !value.is_finite() || value < 0.0 {
            return Err(PriceLevelError::InvalidOperation {
                message: format!("invalid price from f64: {value}"),
            });
        }

        let rounded = value.round();
        // A finite, non-negative f64 fits in u128 iff it is strictly below 2^128.
        if rounded >= 2.0_f64.powi(128) {
            return Err(PriceLevelError::InvalidOperation {
                message: format!("price from f64 out of u128 range: {value}"),
            });
        }

        Ok(Self(rounded as u128))
    }

    /// Converts the price to `f64` with potential precision loss.
    #[must_use]
    pub fn to_f64_lossy(self) -> f64 {
        self.0 as f64
    }

    /// Returns the inner raw value.
    #[must_use]
    pub const fn as_u128(self) -> u128 {
        self.0
    }
}

impl fmt::Display for Price {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Price {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u128>()
            .map(Self)
            .map_err(|_| PriceLevelError::InvalidFieldValue {
                field: "price".to_string(),
                value: s.to_string(),
            })
    }
}

/// Domain value type representing a quantity.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Quantity(u64);

impl Quantity {
    /// Zero quantity value.
    pub const ZERO: Self = Self(0);

    /// Creates a new quantity from a raw integer value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Creates a validated quantity from a raw integer value.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidFieldValue`] if `value` ever fails the
    /// quantity invariant. Every `u64` is currently a valid quantity, so this
    /// constructor is presently infallible; the `Result` is part of its stable
    /// contract so validation can be tightened without a breaking change.
    pub fn try_new(value: u64) -> Result<Self, PriceLevelError> {
        Ok(Self(value))
    }

    /// Creates a quantity from an `f64` value (rounded to the nearest integer).
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidOperation`] if `value` is not finite
    /// (NaN or infinite), is negative, or (after rounding) does not fit in a
    /// `u64`. The range check is explicit because an `f64`-to-`u64` `as` cast
    /// saturates rather than failing, which would silently clamp out-of-range
    /// input to `u64::MAX`.
    pub fn from_f64(value: f64) -> Result<Self, PriceLevelError> {
        if !value.is_finite() || value < 0.0 {
            return Err(PriceLevelError::InvalidOperation {
                message: format!("invalid quantity from f64: {value}"),
            });
        }

        let rounded = value.round();
        // A finite, non-negative f64 fits in u64 iff it is strictly below 2^64.
        if rounded >= 2.0_f64.powi(64) {
            return Err(PriceLevelError::InvalidOperation {
                message: format!("quantity from f64 out of u64 range: {value}"),
            });
        }

        Ok(Self(rounded as u64))
    }

    /// Converts the quantity to `f64` with potential precision loss.
    #[must_use]
    pub fn to_f64_lossy(self) -> f64 {
        self.0 as f64
    }

    /// Returns the inner raw value.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Quantity {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>()
            .map(Self)
            .map_err(|_| PriceLevelError::InvalidFieldValue {
                field: "quantity".to_string(),
                value: s.to_string(),
            })
    }
}

/// Domain value type representing a timestamp in milliseconds.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct TimestampMs(u64);

impl TimestampMs {
    /// Zero timestamp value.
    pub const ZERO: Self = Self(0);

    /// Creates a new timestamp from milliseconds.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Creates a validated timestamp from milliseconds.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::InvalidFieldValue`] if `value` ever fails the
    /// timestamp invariant. Every `u64` is currently a valid millisecond
    /// timestamp, so this constructor is presently infallible; the `Result` is
    /// part of its stable contract so validation can be tightened without a
    /// breaking change.
    pub fn try_new(value: u64) -> Result<Self, PriceLevelError> {
        Ok(Self(value))
    }

    /// Converts an already-read [`SystemTime`] into milliseconds since the
    /// Unix epoch, truncating any sub-millisecond part.
    ///
    /// This does **not** read the clock; the caller supplies `time` (for
    /// example from its own [`UnixClock`](crate::UnixClock) implementation).
    /// No value is clamped or defaulted.
    ///
    /// # Errors
    ///
    /// - [`PriceLevelError::InvalidOperation`] if `time` is before the epoch.
    /// - [`PriceLevelError::InvalidFieldValue`] (field `timestamp_ms`) if the
    ///   millisecond count does not fit in `u64`.
    pub fn try_from_system_time(time: SystemTime) -> Result<Self, PriceLevelError> {
        let since_epoch =
            time.duration_since(UNIX_EPOCH)
                .map_err(|error| PriceLevelError::InvalidOperation {
                    message: format!("time is before the unix epoch: {error}"),
                })?;
        Self::try_from_duration_since_epoch(since_epoch)
    }

    /// Converts a span since the Unix epoch into whole milliseconds with a
    /// checked `u128 -> u64` narrowing (issue #171). This is the injectable
    /// core of [`Self::try_from_system_time`]: a `Duration` can represent more
    /// than `u64::MAX` milliseconds, so the boundary is testable
    /// deterministically without a platform `SystemTime` that reaches it.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::InvalidFieldValue`] (field `timestamp_ms`) if the
    /// millisecond count does not fit in `u64`.
    pub(crate) fn try_from_duration_since_epoch(
        since_epoch: Duration,
    ) -> Result<Self, PriceLevelError> {
        let millis = since_epoch.as_millis();
        let millis = u64::try_from(millis).map_err(|_| PriceLevelError::InvalidFieldValue {
            field: "timestamp_ms".to_string(),
            value: millis.to_string(),
        })?;
        Ok(Self(millis))
    }

    /// Returns the inner raw milliseconds value.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for TimestampMs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for TimestampMs {
    type Err = PriceLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>()
            .map(Self)
            .map_err(|_| PriceLevelError::InvalidFieldValue {
                field: "timestamp".to_string(),
                value: s.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{Price, Quantity, TimestampMs};
    use crate::errors::PriceLevelError;
    use std::str::FromStr;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn timestamp_from_system_time_epoch_is_zero() {
        let ts = TimestampMs::try_from_system_time(UNIX_EPOCH).unwrap();
        assert_eq!(ts, TimestampMs::ZERO);
    }

    #[test]
    fn timestamp_from_system_time_truncates_sub_millisecond_part() {
        let time = UNIX_EPOCH + Duration::from_micros(1_716_000_000_123_999);
        let ts = TimestampMs::try_from_system_time(time).unwrap();
        assert_eq!(ts.as_u64(), 1_716_000_000_123);
    }

    #[test]
    fn timestamp_from_system_time_rejects_pre_epoch() {
        let before = UNIX_EPOCH.checked_sub(Duration::from_millis(1)).unwrap();
        let err = TimestampMs::try_from_system_time(before).unwrap_err();
        assert!(matches!(err, PriceLevelError::InvalidOperation { .. }));
    }

    #[test]
    fn timestamp_from_system_time_rejects_millis_beyond_u64() {
        // u64::MAX ms + 1 ms, built from seconds + millis to avoid overflow.
        let span =
            Duration::from_secs(u64::MAX / 1_000) + Duration::from_millis(u64::MAX % 1_000 + 1);
        // Only meaningful on platforms whose `SystemTime` can represent it.
        if let Some(far_future) = UNIX_EPOCH.checked_add(span) {
            match TimestampMs::try_from_system_time(far_future).unwrap_err() {
                PriceLevelError::InvalidFieldValue { field, value } => {
                    assert_eq!(field, "timestamp_ms");
                    assert_eq!(value, (u128::from(u64::MAX) + 1).to_string());
                }
                other => panic!("unexpected error {other:?}"),
            }
        }
        if let Some(max) = UNIX_EPOCH.checked_add(Duration::from_millis(u64::MAX)) {
            let ts = TimestampMs::try_from_system_time(max).unwrap();
            assert_eq!(ts.as_u64(), u64::MAX);
        }
    }

    // Deterministic conversion boundaries through the injectable helper
    // (issue #171): epoch, ordinary time, the largest representable
    // millisecond count and the first unrepresentable one.
    #[test]
    fn duration_conversion_epoch_is_zero() {
        let ts = TimestampMs::try_from_duration_since_epoch(Duration::ZERO).unwrap();
        assert_eq!(ts, TimestampMs::ZERO);
    }

    #[test]
    fn duration_conversion_ordinary_time() {
        let ts = TimestampMs::try_from_duration_since_epoch(Duration::from_micros(
            1_716_000_000_123_999,
        ))
        .unwrap();
        assert_eq!(ts.as_u64(), 1_716_000_000_123);
    }

    #[test]
    fn duration_conversion_max_representable_millis() {
        let ts =
            TimestampMs::try_from_duration_since_epoch(Duration::from_millis(u64::MAX)).unwrap();
        assert_eq!(ts.as_u64(), u64::MAX);
        // Sub-millisecond excess still truncates to u64::MAX.
        let just_below_next = Duration::from_millis(u64::MAX) + Duration::from_nanos(999_999);
        let ts = TimestampMs::try_from_duration_since_epoch(just_below_next).unwrap();
        assert_eq!(ts.as_u64(), u64::MAX);
    }

    #[test]
    fn duration_conversion_first_unrepresentable_millis() {
        let first_bad = Duration::from_millis(u64::MAX) + Duration::from_millis(1);
        match TimestampMs::try_from_duration_since_epoch(first_bad).unwrap_err() {
            PriceLevelError::InvalidFieldValue { field, value } => {
                assert_eq!(field, "timestamp_ms");
                assert_eq!(value, (u128::from(u64::MAX) + 1).to_string());
            }
            other => panic!("unexpected error {other:?}"),
        }
        let err = TimestampMs::try_from_duration_since_epoch(Duration::MAX).unwrap_err();
        assert!(matches!(err, PriceLevelError::InvalidFieldValue { .. }));
    }

    #[test]
    fn price_roundtrip() {
        let value = Price::new(1_000);
        let parsed = Price::from_str(&value.to_string());
        assert!(parsed.is_ok());
        assert_eq!(parsed.unwrap_or_default(), value);
    }

    #[test]
    fn quantity_roundtrip() {
        let value = Quantity::new(42);
        let parsed = Quantity::from_str(&value.to_string());
        assert!(parsed.is_ok());
        assert_eq!(parsed.unwrap_or_default(), value);
    }

    #[test]
    fn timestamp_roundtrip() {
        let value = TimestampMs::new(1_716_000_000_000);
        let parsed = TimestampMs::from_str(&value.to_string());
        assert!(parsed.is_ok());
        assert_eq!(parsed.unwrap_or_default(), value);
    }

    #[test]
    fn from_f64_rejects_negative() {
        assert!(Price::from_f64(-1.0).is_err());
        assert!(Quantity::from_f64(-1.0).is_err());
    }

    #[test]
    fn from_f64_rejects_out_of_range() {
        // f64 -> int `as` casts saturate; from_f64 must reject instead of
        // silently clamping to u128::MAX / u64::MAX.
        assert!(matches!(
            Price::from_f64(2.0_f64.powi(128)),
            Err(crate::errors::PriceLevelError::InvalidOperation { .. })
        ));
        assert!(matches!(
            Quantity::from_f64(2.0_f64.powi(64)),
            Err(crate::errors::PriceLevelError::InvalidOperation { .. })
        ));
        // Just-in-range values still convert.
        assert!(Price::from_f64(1_000_000.0).is_ok());
        assert!(Quantity::from_f64(1_000_000.0).is_ok());
    }

    #[test]
    fn from_f64_rejects_non_finite() {
        assert!(Price::from_f64(f64::NAN).is_err());
        assert!(Price::from_f64(f64::INFINITY).is_err());
        assert!(Quantity::from_f64(f64::NAN).is_err());
    }
}
