//! Hasher-free duplicate-id detection (pre-release hardening).
//!
//! Input-driven duplicate checks (decoded `MatchResult::filled_order_ids`,
//! restored snapshot orders) used a `std::collections::HashSet` with the
//! default `RandomState`. Building a `RandomState` reads a thread-local key
//! and, the first time on a thread, the OS random source: on some platforms
//! that panics when the OS RNG fails or when it runs during thread-local
//! destruction. These checks are validation of caller bytes, not a long-lived
//! index keyed by caller ids, so they need no hashing at all: the ids are
//! copied into a fallibly reserved `Vec`, sorted by a total order and scanned
//! for adjacent equal keys. `sort_unstable` is in place (no scratch
//! allocation) and cannot panic on a total order.

use crate::errors::{CapacityResource, PriceLevelError};
use crate::utils::Id;
use crate::utils::alloc::try_reserve_exact_vec;

/// A total-order key for an [`Id`] that distinguishes variants: two ids map
/// to the same key if and only if they are equal under `Id`'s `Eq`.
///
/// `Id::as_bytes` alone is not injective across variants (a `Uuid` and a
/// `Ulid` with the same 128 bits are different ids), so the variant tag is
/// part of the key.
#[inline]
fn sort_key(id: &Id) -> (u8, [u8; 16]) {
    let tag = match id {
        Id::Uuid(_) => 0,
        Id::Ulid(_) => 1,
        Id::Sequential(_) => 2,
    };
    (tag, id.as_bytes())
}

/// Returns the position of the earliest *repeat* in `ids`: the smallest index
/// `i` such that `ids[i]` equals some `ids[j]` with `j < i`. `None` when every
/// id is distinct.
///
/// This is exactly the index at which a left-to-right `HashSet::insert` walk
/// would first see `insert` return `false`, so callers that interleave the
/// duplicate check with other per-element checks keep their error precedence.
///
/// # Errors
///
/// [`PriceLevelError::CapacityExceeded`] (`resource`, `ids.len()`) when the
/// scratch vector cannot be reserved (attempted for every non-empty input, as
/// the former set reservation was). Nothing else can fail.
pub(crate) fn first_repeat_position<I>(
    ids: I,
    resource: CapacityResource,
) -> Result<Option<usize>, PriceLevelError>
where
    I: ExactSizeIterator<Item = Id>,
{
    let len = ids.len();
    if len == 0 {
        return Ok(None);
    }
    let mut keyed: Vec<((u8, [u8; 16]), usize)> = Vec::new();
    try_reserve_exact_vec(&mut keyed, len, resource)?;
    // `take(len)` keeps the pushes within the exact reservation even if an
    // iterator misreports its length.
    keyed.extend(
        ids.take(len)
            .enumerate()
            .map(|(position, id)| (sort_key(&id), position)),
    );
    // Positions are distinct, so the order is total and deterministic; within
    // one key the entries are sorted by position.
    keyed.sort_unstable();

    // In each run of equal keys, the entry right after the run's first one is
    // that id's second occurrence; later entries of the run are later
    // occurrences. The earliest repeat is the minimum over those.
    let mut earliest: Option<usize> = None;
    let mut previous: Option<&(u8, [u8; 16])> = None;
    for (key, position) in &keyed {
        if previous == Some(key) {
            earliest = Some(match earliest {
                Some(current) if current <= *position => current,
                _ => *position,
            });
        }
        previous = Some(key);
    }
    Ok(earliest)
}

#[cfg(test)]
// Test-only arithmetic and narrowing casts (Testing section of
// `rules/global_rules.md`).
#[allow(clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use ulid::Ulid;
    use uuid::Uuid;

    /// The pre-hardening shape: first `insert` refusal of a left-to-right walk.
    fn reference(ids: &[Id]) -> Option<usize> {
        let mut seen = HashSet::new();
        ids.iter().position(|id| !seen.insert(*id))
    }

    fn check(ids: &[Id]) {
        let got = first_repeat_position(ids.iter().copied(), CapacityResource::ValidationScratch)
            .expect("reservation");
        assert_eq!(got, reference(ids), "ids: {ids:?}");
    }

    #[test]
    fn empty_and_single_have_no_repeat() {
        check(&[]);
        check(&[Id::from_u64(1)]);
    }

    #[test]
    fn variants_with_equal_bytes_are_distinct() {
        let bytes = Id::sequential(7).as_bytes();
        let ids = [
            Id::sequential(7),
            Id::from_uuid(Uuid::from_bytes(bytes)),
            Id::from_ulid(Ulid::from_bytes(bytes)),
        ];
        check(&ids);
        assert_eq!(
            first_repeat_position(ids.iter().copied(), CapacityResource::ValidationScratch)
                .expect("reservation"),
            None
        );
    }

    #[test]
    fn earliest_repeat_not_earliest_first_occurrence() {
        // B's repeat (index 2) comes before A's repeat (index 3).
        let a = Id::from_u64(1);
        let b = Id::from_u64(2);
        check(&[a, b, b, a]);
        check(&[a, b, a, a, b]);
    }

    #[test]
    fn matches_hashset_reference_on_random_inputs() {
        // Deterministic xorshift so failures reproduce.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..2_000 {
            let len = (next() % 24) as usize;
            let alphabet = 1 + next() % 12;
            let ids: Vec<Id> = (0..len)
                .map(|_| {
                    let value = next() % alphabet;
                    match next() % 3 {
                        0 => Id::sequential(value),
                        1 => Id::from_uuid(Uuid::from_u128(u128::from(value))),
                        _ => Id::from_ulid(Ulid::from(u128::from(value))),
                    }
                })
                .collect();
            check(&ids);
        }
    }

    #[test]
    fn refused_reservation_is_typed() {
        let _fail = crate::utils::alloc::test_seam::fail_after(CapacityResource::RestoreScratch, 0);
        let ids = [Id::from_u64(1), Id::from_u64(1)];
        assert_eq!(
            first_repeat_position(ids.iter().copied(), CapacityResource::RestoreScratch),
            Err(PriceLevelError::capacity_exceeded(
                CapacityResource::RestoreScratch,
                2
            ))
        );
    }
}
