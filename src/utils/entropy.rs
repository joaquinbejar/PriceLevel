//! Fallible entropy and wall-clock providers for random [`Id`](crate::Id)
//! construction.
//!
//! The crate deliberately owns no source of randomness: every random
//! identifier is built from bytes a caller-supplied [`EntropySource`] hands
//! over, so an operating-system entropy failure surfaces as a typed
//! [`PriceLevelError`] instead of a panic inside a dependency's RNG.
//!
//! Likewise the crate owns no clock reader: ULIDs take their timestamp from a
//! caller-supplied [`UnixClock`] or an explicit [`TimestampMs`]
//! ([`Id::try_new_ulid_at`](crate::Id::try_new_ulid_at)). Reading the OS
//! clock through `std::time::SystemTime::now` can itself panic inside `std`
//! if the platform clock call fails, so that choice (and its failure policy)
//! stays with the caller. [`TimestampMs::try_from_system_time`] performs the
//! checked conversion of an already-read `SystemTime`.

use crate::errors::PriceLevelError;
use crate::utils::TimestampMs;

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
///
/// The crate provides no implementation. Note that
/// `std::time::SystemTime::now` panics inside `std` if the platform clock
/// call fails; an implementation that must be panic-free needs a clock read
/// with a fallible error channel. Convert an already-read `SystemTime` with
/// [`TimestampMs::try_from_system_time`].
pub trait UnixClock {
    /// Returns the current time in milliseconds since the Unix epoch.
    ///
    /// # Errors
    ///
    /// Returns a [`PriceLevelError`] when the clock cannot be read or its value
    /// cannot be represented as a [`TimestampMs`].
    fn try_now_ms(&self) -> Result<TimestampMs, PriceLevelError>;
}
