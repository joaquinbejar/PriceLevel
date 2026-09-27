use crate::errors::{CapacityResource, PriceLevelError};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use uuid::Uuid;

/// # UuidGenerator
///
/// A thread-safe generator of deterministic, name-based (v5) UUIDs within a
/// namespace, used by [`PriceLevel::match_order`](crate::PriceLevel::match_order)
/// to mint trade ids.
///
/// Each identifier is `Uuid::new_v5(namespace, decimal(n))`, where `n` is a
/// `u64` sequence value reserved atomically from the generator's counter and
/// `decimal(n)` is its ASCII decimal text (for example `b"0"`, `b"1"`, ...).
///
/// ## Finite sequence and the exhaustion sentinel (issue #168)
///
/// The counter holds the NEXT sequence value to issue. The usable values are
/// `0 ..= u64::MAX - 1`; the final value [`u64::MAX`] is reserved as the
/// exhaustion sentinel ([`UuidGenerator::EXHAUSTED`]) and is never issued.
/// Reservation is a checked compare-and-swap (`checked_add`), so once the
/// counter reaches the sentinel every further request fails with
/// [`PriceLevelError::CapacityExceeded`] (resource
/// [`CapacityResource::IdSequence`]) forever. The counter never wraps,
/// saturates, resets, or substitutes another identifier, so an identifier is
/// never issued twice by one generator state. (Before v0.10 the counter
/// was advanced with an unchecked `fetch_add`, which does not panic but wraps
/// `u64::MAX -> 0` and re-issues the counter-zero id: a duplicate-id
/// correctness defect, not a panic.)
///
/// Every successfully reserved value maps to exactly the same UUID bytes as
/// before v0.10 (same namespace, same decimal name).
///
/// ## Serialization
///
/// The generator serializes as `{"namespace": <uuid>, "counter": <u64>}`,
/// unchanged. An exhausted generator serializes with
/// `"counter": 18446744073709551615` (`u64::MAX`) and deserializes back to the
/// exhausted terminal state. Restoring an arbitrary older state is the
/// caller's responsibility: the generator cannot detect that the restored
/// counter was already issued by a different instance.
///
/// ## Example
///
/// ```
/// use pricelevel::{PriceLevelError, UuidGenerator};
/// use uuid::Uuid;
///
/// let namespace = Uuid::from_u128(0x6ba7_b810_9dad_11d1_80b4_00c0_4fd4_30c8);
/// let generator = UuidGenerator::new(namespace);
///
/// let id1 = generator.try_next()?;
/// let id2 = generator.try_next()?;
/// assert_ne!(id1, id2);
/// # Ok::<(), PriceLevelError>(())
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct UuidGenerator {
    namespace: Uuid,
    counter: AtomicU64,
}

/// A contiguous range of sequence values reserved up front from a
/// [`UuidGenerator`] (issue #168).
///
/// The fill-or-kill sweep reserves exactly the number of trade ids its dry run
/// predicts BEFORE its first maker mutation, then draws them from this block
/// without touching the shared counter. Values left unused are skipped (never
/// reissued), which preserves uniqueness.
#[derive(Debug)]
pub(crate) struct IdBlock {
    /// Next value to hand out.
    next: u64,
    /// One past the last reserved value.
    end: u64,
}

impl IdBlock {
    /// Takes the next reserved value, or `None` once the block is used up.
    #[inline]
    #[must_use]
    pub(crate) fn take(&mut self) -> Option<u64> {
        if self.next >= self.end {
            return None;
        }
        let value = self.next;
        // `value < end <= u64::MAX`, so this cannot overflow; checked anyway.
        self.next = value.checked_add(1)?;
        Some(value)
    }
}

impl UuidGenerator {
    /// The counter value that marks an exhausted generator. It is the
    /// exhaustion sentinel and is never issued as an identifier name; the last
    /// issuable sequence value is `EXHAUSTED - 1`.
    pub const EXHAUSTED: u64 = u64::MAX;

    /// Creates a new `UuidGenerator` with the specified namespace.
    ///
    /// The namespace is used as a base for all generated UUIDs.
    ///
    /// # Arguments
    ///
    /// * `namespace` - The UUID to use as the namespace for generating v5 UUIDs
    ///
    /// # Returns
    ///
    /// A new `UuidGenerator` instance initialized with the provided namespace and a counter set to 0.
    #[must_use]
    pub fn new(namespace: Uuid) -> Self {
        Self {
            namespace,
            counter: AtomicU64::new(0),
        }
    }

    /// The namespace every identifier of this generator is derived from.
    #[must_use]
    #[inline]
    pub fn namespace(&self) -> Uuid {
        self.namespace
    }

    /// Returns `true` once the generator has issued its final sequence value
    /// and every further request fails.
    #[must_use]
    #[inline]
    pub fn is_exhausted(&self) -> bool {
        // `Relaxed`: an advisory point-in-time read; it publishes nothing.
        self.counter.load(Ordering::Relaxed) == Self::EXHAUSTED
    }

    /// The number of identifiers the generator can still issue (a
    /// point-in-time read under concurrent use).
    #[must_use]
    #[inline]
    pub fn remaining(&self) -> u64 {
        // `Relaxed`: an advisory point-in-time read; it publishes nothing.
        // `EXHAUSTED` is `u64::MAX`, so `EXHAUSTED - counter` is exactly the
        // bitwise complement of `counter`: an exact, non-saturating form that
        // cannot underflow for any `u64`.
        !self.counter.load(Ordering::Relaxed)
    }

    /// Generates the next UUID in sequence.
    ///
    /// Atomically reserves the next sequence value `n` and returns
    /// `Uuid::new_v5(namespace, decimal(n))`. Concurrent callers always
    /// receive distinct values.
    ///
    /// # Errors
    ///
    /// Returns [`PriceLevelError::CapacityExceeded`] with resource
    /// [`CapacityResource::IdSequence`] and `additional: 1` when the generator
    /// is exhausted (the counter is at [`UuidGenerator::EXHAUSTED`]). The
    /// generator stays exhausted; no identifier is produced.
    pub fn try_next(&self) -> Result<Uuid, PriceLevelError> {
        let value = self.try_reserve_one()?;
        Ok(self.uuid_for(value))
    }

    /// Atomically reserves one sequence value (allocation-free CAS loop).
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] ([`CapacityResource::IdSequence`],
    /// `additional: 1`) if the generator is exhausted; the counter is left
    /// unchanged.
    #[inline]
    pub(crate) fn try_reserve_one(&self) -> Result<u64, PriceLevelError> {
        // `Relaxed` on both CAS orderings: the only invariant is that every
        // caller reserves a DISTINCT value, and a read-modify-write on a single
        // atomic always operates on the latest value in its modification order
        // regardless of the memory ordering. No other memory is published
        // through this counter (the reserved value is consumed locally to
        // build the name), so no happens-before edge is needed. The retry body
        // is a single `checked_add`: allocation-free and panic-free. It fails
        // exactly when the counter is already at the `u64::MAX` sentinel.
        self.counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| PriceLevelError::capacity_exceeded(CapacityResource::IdSequence, 1))
    }

    /// Atomically reserves `count` consecutive sequence values, all or nothing.
    ///
    /// Used by the fill-or-kill sweep to secure every trade id before its first
    /// maker mutation. `count == 0` reserves nothing and leaves the counter
    /// unchanged.
    ///
    /// # Errors
    ///
    /// [`PriceLevelError::CapacityExceeded`] ([`CapacityResource::IdSequence`],
    /// `additional: count`) if fewer than `count` values remain; the counter is
    /// left unchanged (no partial reservation).
    pub(crate) fn try_reserve_block(&self, count: usize) -> Result<IdBlock, PriceLevelError> {
        let exhausted = || PriceLevelError::capacity_exceeded(CapacityResource::IdSequence, count);
        let n = u64::try_from(count).map_err(|_| exhausted())?;
        if n == 0 {
            return Ok(IdBlock { next: 0, end: 0 });
        }
        // Same ordering argument as `try_reserve_one`.
        let start = self
            .counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(n)
            })
            .map_err(|_| exhausted())?;
        // `start + n` was just validated inside the CAS; recomputed checked.
        let end = start.checked_add(n).ok_or_else(exhausted)?;
        Ok(IdBlock { next: start, end })
    }

    /// Builds the UUID for a reserved sequence value: v5 over the value's
    /// decimal text. Pure; the caller must have reserved `value` from this
    /// generator.
    ///
    /// This runs once per emitted trade inside
    /// [`PriceLevel::match_order`](crate::PriceLevel::match_order), so it is
    /// on the fill hot path. The decimal name is encoded into a fixed stack
    /// buffer (issue #146) instead of a heap `String`; the hashed bytes are
    /// exactly `value.to_string().as_bytes()`, so every id is byte-identical
    /// to the pre-#146 allocating form.
    #[must_use]
    #[inline]
    pub(crate) fn uuid_for(&self, value: u64) -> Uuid {
        let mut buf = [0u8; MAX_U64_DECIMAL_DIGITS];
        Uuid::new_v5(&self.namespace, encode_decimal(value, &mut buf))
    }
}

/// Decimal digits in `u64::MAX` (`18446744073709551615`): the widest name
/// any sequence value can produce.
const MAX_U64_DECIMAL_DIGITS: usize = 20;

/// Decimal radix as a type-level nonzero divisor for [`encode_decimal`].
///
/// Built without `Option` so there is no panic form or dead fallback:
/// `MIN` is 1 and `1 + 9` cannot reach `u64::MAX`, so this compile-time
/// constant is exactly 10 (pinned by a unit test). It is not counter state.
// panic-policy-allow-saturating: compile-time-only constant, provably exact
// (1 + 9 cannot saturate), pinned by `test_decimal_radix_is_exactly_ten`
// (issue #173).
const DECIMAL_RADIX: std::num::NonZeroU64 = std::num::NonZeroU64::MIN.saturating_add(9);

/// Writes the ASCII decimal representation of `value` (no sign, no leading
/// zeros, `b"0"` for zero) right-aligned into `buf` and returns the used
/// suffix: exactly the bytes of `value.to_string()`, without allocating.
///
/// Digits are written least-significant first through the reversed mutable
/// iterator, so there is no indexing or slicing expression, no narrowing
/// cast and no arithmetic that can overflow (`% 10` and `/ 10` on a `u64`
/// cannot). `start` is always a position yielded by the iterator (`0..20`)
/// and the loop always stops by `remaining == 0` because a `u64` has at most
/// 20 decimal digits, so the checked `get(start..)` always succeeds; its
/// `unwrap_or` arm is dead and only there to stay panic-free.
#[inline]
fn encode_decimal(value: u64, buf: &mut [u8; MAX_U64_DECIMAL_DIGITS]) -> &[u8] {
    let mut remaining = value;
    let mut start = 0;
    for (position, slot) in buf.iter_mut().enumerate().rev() {
        // Division and remainder by `NonZeroU64` (`u64: Div<NonZeroU64>` /
        // `Rem<NonZeroU64>`) have no divide-by-zero or overflow case: the
        // divisor is nonzero by type, so there is no failure branch to handle.
        // `remaining % TEN` is in `0..=9`, so its lowest little-endian byte is
        // the whole digit, and `b'0' | digit == b'0' + digit` (0x30 has its
        // low nibble clear).
        let [digit, ..] = (remaining % DECIMAL_RADIX).to_le_bytes();
        *slot = b'0' | digit;
        remaining /= DECIMAL_RADIX;
        if remaining == 0 {
            start = position;
            break;
        }
    }
    let buf: &[u8] = buf;
    buf.get(start..).unwrap_or(buf)
}

#[cfg(test)]
// Scoped to this co-located test module only (issue #173): a `u64`-to-`usize`
// narrowing cast on a test bound-count constant is permitted inside `mod
// tests` per the Testing section of `rules/global_rules.md`. Production code
// outside this module keeps the full deny list.
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn test_decimal_radix_is_exactly_ten() {
        assert_eq!(DECIMAL_RADIX.get(), 10);
    }

    // Helper function to create a test namespace
    fn create_test_namespace() -> Uuid {
        Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap()
    }

    #[test]
    fn test_uuid_generator_creation() {
        let namespace = create_test_namespace();
        let generator = UuidGenerator::new(namespace);

        assert_eq!(generator.namespace, namespace);
        assert_eq!(generator.counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_uuid_generator_next() {
        let generator = UuidGenerator::new(create_test_namespace());

        // Generate first UUID
        let uuid1 = generator.try_next().unwrap();
        assert_eq!(generator.counter.load(Ordering::SeqCst), 1);

        // Generate second UUID
        let uuid2 = generator.try_next().unwrap();
        assert_eq!(generator.counter.load(Ordering::SeqCst), 2);

        // UUIDs should be different
        assert_ne!(uuid1, uuid2);

        // Both should be version 5 (name-based) UUIDs
        assert_eq!(uuid1.get_version(), Some(uuid::Version::Sha1));
        assert_eq!(uuid2.get_version(), Some(uuid::Version::Sha1));
    }

    #[test]
    fn test_uuid_generator_deterministic() {
        // Create two generators with the same namespace
        let namespace = create_test_namespace();
        let generator1 = UuidGenerator::new(namespace);
        let generator2 = UuidGenerator::new(namespace);

        // They should generate the same UUIDs for the same counter values
        assert_eq!(
            generator1.try_next().unwrap(),
            generator2.try_next().unwrap()
        );
        assert_eq!(
            generator1.try_next().unwrap(),
            generator2.try_next().unwrap()
        );
    }

    #[test]
    fn test_uuid_generator_different_namespaces() {
        // Create two generators with different namespaces
        let namespace1 = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        let namespace2 = Uuid::parse_str("6ba7b811-9dad-11d1-80b4-00c04fd430c8").unwrap();

        let generator1 = UuidGenerator::new(namespace1);
        let generator2 = UuidGenerator::new(namespace2);

        // They should generate different UUIDs for the same counter values
        assert_ne!(
            generator1.try_next().unwrap(),
            generator2.try_next().unwrap()
        );
        assert_ne!(
            generator1.try_next().unwrap(),
            generator2.try_next().unwrap()
        );
    }

    #[test]
    fn test_uuid_generator_sequential() {
        let generator = UuidGenerator::new(create_test_namespace());
        let mut uuids = Vec::new();

        // Generate 100 UUIDs
        for _ in 0..100 {
            uuids.push(generator.try_next().unwrap());
        }

        // Check they're all unique
        let unique_uuids: HashSet<_> = uuids.iter().collect();
        assert_eq!(unique_uuids.len(), 100);

        // Check that the counter is properly incremented
        assert_eq!(generator.counter.load(Ordering::SeqCst), 100);
    }

    #[test]
    fn test_uuid_generator_thread_safety() {
        let generator = Arc::new(UuidGenerator::new(create_test_namespace()));
        let num_threads = 10;
        let uuids_per_thread = 100;
        let total_uuids = num_threads * uuids_per_thread;

        // Use a barrier to ensure all threads start at the same time
        let barrier = Arc::new(Barrier::new(num_threads));

        // Shared container to collect all generated UUIDs
        let all_uuids = Arc::new(std::sync::Mutex::new(Vec::with_capacity(total_uuids)));

        let mut handles = vec![];

        for _ in 0..num_threads {
            let thread_generator = Arc::clone(&generator);
            let thread_barrier = Arc::clone(&barrier);
            let thread_uuids = Arc::clone(&all_uuids);

            let handle = thread::spawn(move || {
                thread_barrier.wait(); // Wait for all threads to be ready

                let mut local_uuids = Vec::with_capacity(uuids_per_thread);
                for _ in 0..uuids_per_thread {
                    local_uuids.push(thread_generator.try_next().unwrap());
                }

                // Add thread's UUIDs to the shared collection
                let mut all = thread_uuids.lock().unwrap();
                all.extend(local_uuids);
            });

            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().unwrap();
        }

        // Check that all UUIDs are unique
        let all_uuids = all_uuids.lock().unwrap();
        let unique_uuids: HashSet<_> = all_uuids.iter().collect();

        assert_eq!(
            unique_uuids.len(),
            total_uuids,
            "All generated UUIDs should be unique"
        );

        // Verify the counter was incremented correctly
        assert_eq!(
            generator.counter.load(Ordering::SeqCst),
            total_uuids as u64,
            "Counter should match the total number of generated UUIDs"
        );
    }

    #[test]
    fn test_uuid_generator_with_initial_counter() {
        // Create a generator with a custom initial counter value
        let namespace = create_test_namespace();
        let initial_counter = 1000;

        let mut generator = UuidGenerator::new(namespace);
        generator.counter = AtomicU64::new(initial_counter);

        // Generate a UUID
        let _ = generator.try_next().unwrap();

        // Verify counter was incremented
        assert_eq!(
            generator.counter.load(Ordering::SeqCst),
            initial_counter + 1
        );

        // Create another generator with initial counter at 1001
        let mut generator2 = UuidGenerator::new(namespace);
        generator2.counter = AtomicU64::new(initial_counter + 1);

        // The next UUID from generator2 should match the next from generator1
        assert_eq!(
            generator.try_next().unwrap(),
            generator2.try_next().unwrap()
        );
    }

    // ---- issue #168: checked sequence reservation and exhaustion ----

    fn generator_at(counter: u64) -> UuidGenerator {
        let json = format!(
            r#"{{"namespace":"{}","counter":{counter}}}"#,
            create_test_namespace()
        );
        serde_json::from_str(&json).unwrap()
    }

    fn assert_exhausted_error(err: &PriceLevelError, additional: usize) {
        assert_eq!(
            *err,
            PriceLevelError::CapacityExceeded {
                resource: CapacityResource::IdSequence,
                additional,
            }
        );
    }

    #[test]
    fn test_uuid_generator_try_next_ids_byte_identical_to_v5_decimal_names() {
        let namespace = create_test_namespace();
        let generator = UuidGenerator::new(namespace);
        for n in 0u64..16 {
            let expected = Uuid::new_v5(&namespace, n.to_string().as_bytes());
            assert_eq!(generator.try_next().unwrap(), expected);
        }
        // Pinned vector: counter-zero id of this namespace.
        assert_eq!(
            UuidGenerator::new(namespace).try_next().unwrap(),
            Uuid::new_v5(&namespace, b"0")
        );
    }

    #[test]
    fn test_uuid_generator_deserialized_at_max_minus_one_issues_last_then_errors_forever() {
        let generator = generator_at(u64::MAX - 1);
        assert!(!generator.is_exhausted());
        assert_eq!(generator.remaining(), 1);

        let last = generator.try_next().unwrap();
        assert_eq!(
            last,
            Uuid::new_v5(&create_test_namespace(), b"18446744073709551614")
        );
        assert!(generator.is_exhausted());
        assert_eq!(generator.remaining(), 0);

        for _ in 0..5 {
            let err = generator.try_next().unwrap_err();
            assert_exhausted_error(&err, 1);
            // Never cycles back to zero.
            assert_eq!(generator.counter.load(Ordering::SeqCst), u64::MAX);
        }
    }

    #[test]
    fn test_uuid_generator_deserialized_at_max_is_exhausted() {
        let generator = generator_at(u64::MAX);
        assert!(generator.is_exhausted());
        assert_eq!(generator.remaining(), 0);
        assert_exhausted_error(&generator.try_next().unwrap_err(), 1);
        assert_eq!(
            generator.counter.load(Ordering::SeqCst),
            UuidGenerator::EXHAUSTED
        );
    }

    #[test]
    fn test_uuid_generator_exhausted_state_round_trips_through_json() {
        let generator = generator_at(u64::MAX - 1);
        generator.try_next().unwrap();
        assert!(generator.is_exhausted());

        let json = serde_json::to_string(&generator).unwrap();
        assert!(json.contains("\"counter\":18446744073709551615"), "{json}");
        let restored: UuidGenerator = serde_json::from_str(&json).unwrap();
        assert!(restored.is_exhausted());
        assert_eq!(restored.namespace(), generator.namespace());
        assert_exhausted_error(&restored.try_next().unwrap_err(), 1);

        // A live generator round-trips its position too.
        let live = UuidGenerator::new(create_test_namespace());
        live.try_next().unwrap();
        let live_restored: UuidGenerator =
            serde_json::from_str(&serde_json::to_string(&live).unwrap()).unwrap();
        assert_eq!(live.try_next().unwrap(), live_restored.try_next().unwrap());
    }

    #[test]
    fn test_uuid_generator_issue_reproduction_no_longer_reuses_counter_zero_id() {
        let exhausted: UuidGenerator = serde_json::from_str(
            r#"{"namespace":"00000000-0000-0000-0000-000000000000","counter":18446744073709551615}"#,
        )
        .unwrap();
        let fresh: UuidGenerator = serde_json::from_str(
            r#"{"namespace":"00000000-0000-0000-0000-000000000000","counter":0}"#,
        )
        .unwrap();
        let fresh_first = fresh.try_next().unwrap();
        // Previously `next()` issued name "18446744073709551615" and then the
        // wrapped counter-zero id (equal to `fresh_first`). Now both fail.
        for _ in 0..3 {
            let result = exhausted.try_next();
            assert!(result.is_err());
            assert_ne!(result.ok(), Some(fresh_first));
        }
    }

    #[test]
    fn test_uuid_generator_concurrent_reservations_at_boundary_exact_success_count() {
        const AVAILABLE: u64 = 50;
        let generator = Arc::new(generator_at(u64::MAX - AVAILABLE));
        let num_threads = 8;
        let attempts_per_thread = 20; // 160 attempts for 50 values
        let barrier = Arc::new(Barrier::new(num_threads));

        let handles: Vec<_> = (0..num_threads)
            .map(|_| {
                let generator = Arc::clone(&generator);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    let mut ok = Vec::new();
                    let mut failures = 0usize;
                    for _ in 0..attempts_per_thread {
                        match generator.try_next() {
                            Ok(id) => ok.push(id),
                            Err(err) => {
                                assert_exhausted_error(&err, 1);
                                failures += 1;
                            }
                        }
                    }
                    (ok, failures)
                })
            })
            .collect();

        let mut ids = Vec::new();
        let mut failures = 0usize;
        for handle in handles {
            let (ok, failed) = handle.join().unwrap();
            ids.extend(ok);
            failures += failed;
        }
        assert_eq!(ids.len() as u64, AVAILABLE);
        assert_eq!(
            failures,
            num_threads * attempts_per_thread - AVAILABLE as usize
        );
        let unique: HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "duplicate ids issued");
        // Exactly the last 50 names were issued.
        let namespace = create_test_namespace();
        for n in (u64::MAX - AVAILABLE)..u64::MAX {
            assert!(unique.contains(&Uuid::new_v5(&namespace, n.to_string().as_bytes())));
        }
        assert!(generator.is_exhausted());
    }

    #[test]
    fn test_uuid_generator_reserve_block_is_all_or_nothing() {
        let generator = generator_at(u64::MAX - 2);

        // Two values remain; asking for three fails and leaves the counter.
        assert_exhausted_error(&generator.try_reserve_block(3).unwrap_err(), 3);
        assert_eq!(generator.remaining(), 2);

        // Zero is a no-op.
        let mut empty = generator.try_reserve_block(0).unwrap();
        assert_eq!(empty.take(), None);
        assert_eq!(generator.remaining(), 2);

        let mut block = generator.try_reserve_block(2).unwrap();
        assert!(generator.is_exhausted());
        assert_eq!(block.take(), Some(u64::MAX - 2));
        assert_eq!(block.take(), Some(u64::MAX - 1));
        assert_eq!(block.take(), None);
        assert_exhausted_error(&generator.try_reserve_block(1).unwrap_err(), 1);
    }

    // ---- issue #146: allocation-free decimal name encoding ----

    /// The pre-#146 reference form: v5 over the heap-allocated decimal text.
    fn reference_uuid(namespace: &Uuid, value: u64) -> Uuid {
        Uuid::new_v5(namespace, value.to_string().as_bytes())
    }

    fn assert_equivalent(generator: &UuidGenerator, value: u64) {
        let mut buf = [0u8; MAX_U64_DECIMAL_DIGITS];
        assert_eq!(
            encode_decimal(value, &mut buf),
            value.to_string().as_bytes(),
            "decimal name mismatch for {value}"
        );
        assert_eq!(
            generator.uuid_for(value),
            reference_uuid(&generator.namespace(), value),
            "uuid mismatch for {value}"
        );
    }

    #[test]
    fn test_encode_decimal_matches_to_string_at_width_boundaries() {
        let generator = UuidGenerator::new(create_test_namespace());
        for value in [0, 1, 9, 10, 11, 99, 100, u64::MAX - 1, u64::MAX] {
            assert_equivalent(&generator, value);
        }
        // Every power of ten and its neighbours (each decimal-width boundary).
        let mut power: u64 = 1;
        loop {
            assert_equivalent(&generator, power - 1);
            assert_equivalent(&generator, power);
            assert_equivalent(&generator, power + 1);
            match power.checked_mul(10) {
                Some(next) => power = next,
                None => break,
            }
        }
        // Dense low range.
        for value in 0..100_000u64 {
            assert_equivalent(&generator, value);
        }
    }

    #[test]
    fn test_try_next_matches_reference_from_restored_counters() {
        let namespace = create_test_namespace();
        for start in [0, 9, 99, 999_999, 1 << 32, u64::MAX - 3] {
            let generator = generator_at(start);
            for offset in 0..3 {
                assert_eq!(
                    generator.try_next().unwrap(),
                    reference_uuid(&namespace, start + offset)
                );
            }
        }
    }

    proptest::proptest! {
        #[test]
        fn prop_uuid_for_is_byte_identical_to_to_string_path(value: u64, ns: u128) {
            let namespace = Uuid::from_u128(ns);
            let generator = UuidGenerator::new(namespace);
            let mut buf = [0u8; MAX_U64_DECIMAL_DIGITS];
            let expected = value.to_string();
            proptest::prop_assert_eq!(
                encode_decimal(value, &mut buf),
                expected.as_bytes()
            );
            proptest::prop_assert_eq!(generator.uuid_for(value), reference_uuid(&namespace, value));
        }
    }

    #[test]
    fn test_uuid_generator_reserve_block_then_next_continues_after_block() {
        let generator = UuidGenerator::new(create_test_namespace());
        let mut block = generator.try_reserve_block(3).unwrap();
        let next = generator.try_next().unwrap();
        assert_eq!(next, generator.uuid_for(3));
        assert_eq!(
            block.take().map(|v| generator.uuid_for(v)),
            Some(generator.uuid_for(0))
        );
    }
}
