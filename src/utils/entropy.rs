//! Fallible entropy and wall-clock providers for random [`Id`](crate::Id)
//! construction.
//!
//! The crate deliberately owns no source of randomness: every random
//! identifier is built from bytes a caller-supplied [`EntropySource`] hands
//! over, so an operating-system entropy failure surfaces as a typed
//! [`PriceLevelError`] instead of a panic inside a dependency's RNG. ULIDs
//! additionally read a [`UnixClock`]; [`SystemClock`] is the standard
//! implementation backed by [`std::time::SystemTime`].

use crate::errors::PriceLevelError;
use crate::utils::TimestampMs;
use std::time::{SystemTime, UNIX_EPOCH};

/// A fallible source of unpredictable bytes used to build random identifiers.
///
/// Implement it over the randomness facility your application already
/// trusts, for example an OS entropy call (`getrandom`) or a cryptographically
/// secure RNG, and map that facility's failure into a [`PriceLevelError`]
/// (conventionally [`PriceLevelError::EntropyUnavailable`]).
///
/// # Contract
///
/// - **Must not panic.** Report every failure, including RNG initialization
///   and reseeding failures, through `Err`. A panicking implementation defeats
///   the purpose of the fallible [`Id`](crate::Id) constructors.
/// - On `Ok(())` every byte of `dest` must have been written with fresh,
///   unpredictable data. Never fall back to zeros, a constant, a counter or
///   any other predictable value; return `Err` instead.
/// - On `Err` the contents of `dest` are unspecified; the constructors discard
///   the buffer and return the error unchanged.
///
/// # Example
///
/// ```
/// use pricelevel::{EntropySource, Id, PriceLevelError};
///
/// /// Test-only xorshift source. NOT suitable for production identifiers.
/// struct XorShift(u64);
///
/// impl EntropySource for XorShift {
///     fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), PriceLevelError> {
///         for byte in dest.iter_mut() {
///             self.0 ^= self.0 << 13;
///             self.0 ^= self.0 >> 7;
///             self.0 ^= self.0 << 17;
///             *byte = self.0.to_le_bytes()[0];
///         }
///         Ok(())
///     }
/// }
///
/// let id = Id::try_new_uuid(&mut XorShift(0x9E37_79B9_7F4A_7C15))?;
/// assert!(id.is_uuid());
/// # Ok::<(), PriceLevelError>(())
/// ```
pub trait EntropySource {
    /// Fills `dest` entirely with unpredictable bytes.
    ///
    /// # Errors
    ///
    /// Returns a [`PriceLevelError`] (conventionally
    /// [`PriceLevelError::EntropyUnavailable`]) when the underlying randomness
    /// facility cannot produce bytes. Implementations must not panic.
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), PriceLevelError>;
}

/// A fallible wall clock reporting milliseconds since the Unix epoch.
///
/// Used to stamp the timestamp field of ULID identifiers.
///
/// # Contract
///
/// Implementations **must not panic** and must report clock failures through
/// `Err` rather than substituting zero or another fallback value.
pub trait UnixClock {
    /// Returns the current time in milliseconds since the Unix epoch.
    ///
    /// # Errors
    ///
    /// Returns a [`PriceLevelError`] when the clock cannot be read or its value
    /// cannot be represented as a [`TimestampMs`].
    fn try_now_ms(&self) -> Result<TimestampMs, PriceLevelError>;
}

/// [`UnixClock`] backed by [`SystemTime::now`].
///
/// A clock set before the Unix epoch yields
/// [`PriceLevelError::InvalidOperation`]; a time whose millisecond count does
/// not fit in `u64` yields [`PriceLevelError::InvalidFieldValue`]. No value is
/// clamped or defaulted.
///
/// Reading the OS clock goes through the standard library, which itself
/// aborts the read with a panic if the platform clock call fails outright
/// (for example `clock_gettime(CLOCK_REALTIME)` returning an error). That
/// path is owned by `std`, not by this crate, and is shared with every other
/// `SystemTime::now` caller; supply your own [`UnixClock`] if you need to
/// avoid it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemClock;

impl SystemClock {
    /// Converts a [`SystemTime`] into milliseconds since the Unix epoch.
    ///
    /// # Errors
    ///
    /// - [`PriceLevelError::InvalidOperation`] if `time` is before the epoch.
    /// - [`PriceLevelError::InvalidFieldValue`] if the millisecond count does
    ///   not fit in `u64`.
    pub fn timestamp_ms_from(time: SystemTime) -> Result<TimestampMs, PriceLevelError> {
        let since_epoch =
            time.duration_since(UNIX_EPOCH)
                .map_err(|error| PriceLevelError::InvalidOperation {
                    message: format!("system clock is before the unix epoch: {error}"),
                })?;
        let millis = since_epoch.as_millis();
        let millis = u64::try_from(millis).map_err(|_| PriceLevelError::InvalidFieldValue {
            field: "timestamp_ms".to_string(),
            value: millis.to_string(),
        })?;
        Ok(TimestampMs::new(millis))
    }
}

impl UnixClock for SystemClock {
    fn try_now_ms(&self) -> Result<TimestampMs, PriceLevelError> {
        Self::timestamp_ms_from(SystemTime::now())
    }
}

#[cfg(test)]
mod tests {
    use super::{SystemClock, UnixClock};
    use crate::errors::PriceLevelError;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn test_system_clock_epoch_is_zero() {
        let ts = SystemClock::timestamp_ms_from(UNIX_EPOCH).unwrap();
        assert_eq!(ts.as_u64(), 0);
    }

    #[test]
    fn test_system_clock_truncates_sub_millisecond_part() {
        let time = UNIX_EPOCH + Duration::from_micros(1_716_000_000_123_999);
        let ts = SystemClock::timestamp_ms_from(time).unwrap();
        assert_eq!(ts.as_u64(), 1_716_000_000_123);
    }

    #[test]
    fn test_system_clock_rejects_pre_epoch_time() {
        let before = UNIX_EPOCH.checked_sub(Duration::from_millis(1)).unwrap();
        let err = SystemClock::timestamp_ms_from(before).unwrap_err();
        assert!(matches!(err, PriceLevelError::InvalidOperation { .. }));
    }

    #[test]
    fn test_system_clock_rejects_millis_beyond_u64() {
        // u64::MAX ms + 1 ms, built from seconds + millis to avoid overflow.
        let secs = u64::MAX / 1_000;
        let extra_millis = u64::MAX % 1_000 + 1;
        let span = Duration::from_secs(secs) + Duration::from_millis(extra_millis);
        // Only meaningful on platforms whose `SystemTime` can represent it.
        if let Some(far_future) = UNIX_EPOCH.checked_add(span) {
            let err = SystemClock::timestamp_ms_from(far_future).unwrap_err();
            match err {
                PriceLevelError::InvalidFieldValue { field, value } => {
                    assert_eq!(field, "timestamp_ms");
                    assert_eq!(value, (u128::from(u64::MAX) + 1).to_string());
                }
                other => panic!("unexpected error {other:?}"),
            }
        }
        // The largest representable millisecond count is accepted.
        if let Some(max) = UNIX_EPOCH.checked_add(Duration::from_millis(u64::MAX)) {
            assert_eq!(
                SystemClock::timestamp_ms_from(max).unwrap().as_u64(),
                u64::MAX
            );
        }
    }

    #[test]
    fn test_system_clock_now_is_after_2020() {
        let now = SystemClock.try_now_ms().unwrap();
        assert!(now.as_u64() > 1_577_836_800_000);
    }
}
